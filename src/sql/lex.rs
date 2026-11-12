//! Tokens of SQL, by dialect: PostgreSQL's $tag$ … $tag$ and E'…',
//! Oracle's q'[…]' and the "/" that ends a block, ClickHouse's `names` and
//! {param:Type}, SQLite's [names]. Tokens keep their byte span; nothing is
//! dropped, so tokens laid end to end give the source back.

use super::Dialect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tk {
    Space,
    LineComment,
    BlockComment,
    Str,
    /// "name", `name`, [name].
    Quoted,
    Number,
    Word,
    Keyword,
    /// :name, $1, ?, @var, {p:T}.
    Param,
    Op,
    Open,
    Close,
    Comma,
    Semi,
    Dot,
    /// Oracle: "/" alone on its line, ending a statement.
    Slash,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tok {
    pub kind: Tk,
    pub start: usize,
    pub end: usize,
    /// A string, name or comment that runs to the end of the text.
    pub open: bool,
}

impl Tok {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }

    pub fn is_kw(&self, src: &str, w: &str) -> bool {
        self.kind == Tk::Keyword && self.text(src).eq_ignore_ascii_case(w)
    }

    /// A word or keyword spelled so (case aside).
    pub fn is_word(&self, src: &str, w: &str) -> bool {
        matches!(self.kind, Tk::Keyword | Tk::Word) && self.text(src).eq_ignore_ascii_case(w)
    }

    pub fn trivia(&self) -> bool {
        matches!(self.kind, Tk::Space | Tk::LineComment | Tk::BlockComment)
    }
}

/// The opening bracket a closing one pairs with.
pub fn pair_of(c: u8) -> Option<u8> {
    match c {
        b')' => Some(b'('),
        b']' => Some(b'['),
        b'}' => Some(b'{'),
        _ => None,
    }
}

pub fn lex(src: &str, d: Dialect) -> Vec<Tok> {
    let b = src.as_bytes();
    let n = b.len();
    let mut out: Vec<Tok> = Vec::with_capacity(n / 3);
    let mut i = 0;
    // At a line's start (only blanks before): for Oracle's "/".
    let mut line_start = true;
    while i < n {
        let start = i;
        let c = b[i];
        let mut open = false;
        let kind;
        if c == b'\n' || c.is_ascii_whitespace() {
            while i < n && b[i].is_ascii_whitespace() {
                if b[i] == b'\n' {
                    line_start = true;
                }
                i += 1;
            }
            out.push(Tok { kind: Tk::Space, start, end: i, open: false });
            continue;
        }
        let at_line_start = std::mem::replace(&mut line_start, false);
        if c == b'-' && b.get(i + 1) == Some(&b'-') || (c == b'#' && d == Dialect::ClickHouse && b.get(i + 1) == Some(&b' ')) {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            kind = Tk::LineComment;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            // PostgreSQL's nest.
            let mut depth = 1;
            while i < n {
                if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else if d == Dialect::Postgres && b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            open = depth > 0;
            kind = Tk::BlockComment;
        } else if c == b'/' && d == Dialect::Oracle && at_line_start && rest_blank(b, i + 1) {
            i += 1;
            kind = Tk::Slash;
        } else if d == Dialect::Oracle && matches!(c, b'q' | b'Q' | b'n' | b'N') && quote_q(b, i).is_some() {
            let (end, o) = quote_q(b, i).unwrap();
            i = end;
            open = o;
            kind = Tk::Str;
        } else if c == b'\'' || (matches!(c, b'E' | b'e' | b'N' | b'n' | b'B' | b'b' | b'X' | b'x') && b.get(i + 1) == Some(&b'\'')) {
            let backslash = matches!(c, b'E' | b'e') && d == Dialect::Postgres || d == Dialect::ClickHouse;
            if c != b'\'' {
                i += 1;
            }
            let (end, o) = quoted(b, i, b'\'', backslash);
            i = end;
            open = o;
            kind = Tk::Str;
        } else if c == b'"' || c == b'`' && matches!(d, Dialect::ClickHouse | Dialect::Sqlite | Dialect::Generic) {
            let (end, o) = quoted(b, i, c, d == Dialect::ClickHouse);
            i = end;
            open = o;
            kind = Tk::Quoted;
        } else if c == b'[' && d == Dialect::Sqlite {
            i += 1;
            while i < n && b[i] != b']' {
                i += 1;
            }
            open = i >= n;
            i = (i + 1).min(n);
            kind = Tk::Quoted;
        } else if c == b'$' && d == Dialect::Postgres && dollar_tag(b, i).is_some() {
            let tag_end = dollar_tag(b, i).unwrap();
            let tag = &src[i..tag_end];
            match src[tag_end..].find(tag) {
                Some(k) => i = tag_end + k + tag.len(),
                None => {
                    i = n;
                    open = true;
                }
            }
            kind = Tk::Str;
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            if c == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
                i += 2;
                while i < n && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < n && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
                if i < n && b[i] == b'.' && b.get(i + 1) != Some(&b'.') {
                    i += 1;
                    while i < n && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i < n && matches!(b[i], b'e' | b'E') && (b.get(i + 1).is_some_and(u8::is_ascii_digit) || matches!(b.get(i + 1), Some(b'+' | b'-')) && b.get(i + 2).is_some_and(u8::is_ascii_digit)) {
                    i += 2;
                    while i < n && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            kind = Tk::Number;
        } else if is_word_start(src, i) {
            i = word_end(src, i);
            // Right by a dot it's a name, keyword or not (default.orders, t.date).
            let dotted = b.get(i) == Some(&b'.') || (start > 0 && b[start - 1] == b'.');
            kind = if !dotted && super::is_keyword(&src[start..i], d) { Tk::Keyword } else { Tk::Word };
        } else if (c == b':' && b.get(i + 1).is_some_and(|x| x.is_ascii_alphabetic() || *x == b'_' || x.is_ascii_digit() && d != Dialect::Postgres) && !(i > 0 && b[i - 1] == b':'))
            || (c == b'$' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
            || (c == b'@' && b.get(i + 1).is_some_and(|x| x.is_ascii_alphabetic()))
            || c == b'?'
        {
            i += 1;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            kind = Tk::Param;
        } else if c == b'{' && d == Dialect::ClickHouse && src[i..].find('}').is_some_and(|k| src[i + 1..i + k].contains(':') && !src[i + 1..i + k].contains(['\n', '{'])) {
            i += src[i..].find('}').unwrap() + 1;
            kind = Tk::Param;
        } else {
            kind = match c {
                b'(' | b'[' | b'{' => Tk::Open,
                b')' | b']' | b'}' => Tk::Close,
                b',' => Tk::Comma,
                b';' => Tk::Semi,
                b'.' => Tk::Dot,
                _ if c >= 0x80 => Tk::Other,
                _ => Tk::Op,
            };
            if kind == Tk::Op {
                // (Bytes: a letter of two bytes may follow.)
                let ops = ["->>", "#>>", "<=>", "<=", ">=", "<>", "!=", "||", "::", "->", "#>", "=>", ":=", "@>", "<@", "&&", "**", "!~", "~*"];
                i += ops.iter().find(|op| b[i..].starts_with(op.as_bytes())).map_or(1, |op| op.len());
            } else if c >= 0x80 {
                i += src[i..].chars().next().map_or(1, char::len_utf8);
            } else {
                i += 1;
            }
        }
        out.push(Tok { kind, start, end: i, open });
    }
    out
}

fn rest_blank(b: &[u8], mut i: usize) -> bool {
    while i < b.len() && b[i] != b'\n' {
        if !b[i].is_ascii_whitespace() {
            return false;
        }
        i += 1;
    }
    true
}

/// A string or quoted name from its quote: (end, still open).
fn quoted(b: &[u8], start: usize, q: u8, backslash: bool) -> (usize, bool) {
    let mut i = start + 1;
    while i < b.len() {
        if backslash && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q {
            if b.get(i + 1) == Some(&q) {
                i += 2;
                continue;
            }
            return (i + 1, false);
        }
        i += 1;
    }
    (b.len(), true)
}

/// Oracle q'[…]' (and nq'…'): (end, still open).
fn quote_q(b: &[u8], i: usize) -> Option<(usize, bool)> {
    let mut k = i;
    if matches!(b[k], b'n' | b'N') {
        k += 1;
        if b.get(k) == Some(&b'\'') {
            return Some(quoted(b, k, b'\'', false));
        }
    }
    if !matches!(b.get(k), Some(b'q' | b'Q')) || b.get(k + 1) != Some(&b'\'') {
        return None;
    }
    let open = *b.get(k + 2)?;
    if open.is_ascii_whitespace() {
        return None;
    }
    let close = match open {
        b'[' => b']',
        b'(' => b')',
        b'{' => b'}',
        b'<' => b'>',
        o => o,
    };
    let mut j = k + 3;
    while j + 1 < b.len() {
        if b[j] == close && b[j + 1] == b'\'' {
            return Some((j + 2, false));
        }
        j += 1;
    }
    Some((b.len(), true))
}

/// $tag$ at i: where the tag ends.
fn dollar_tag(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
        if j == i + 1 && b[j].is_ascii_digit() {
            return None;
        }
        j += 1;
    }
    (b.get(j) == Some(&b'$')).then_some(j + 1)
}

pub fn is_word_start(src: &str, i: usize) -> bool {
    src[i..].chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
}

pub fn word_end(src: &str, mut i: usize) -> usize {
    for c in src[i..].chars() {
        if c.is_alphanumeric() || c == '_' || c == '$' || c == '#' {
            i += c.len_utf8();
        } else {
            break;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str, d: Dialect) -> Vec<(Tk, &str)> {
        lex(src, d).into_iter().filter(|t| t.kind != Tk::Space).map(|t| (t.kind, t.text(src))).collect()
    }

    #[test]
    fn dialect_strings() {
        assert_eq!(kinds("select $f$ a ' b $f$, 1", Dialect::Postgres)[1], (Tk::Str, "$f$ a ' b $f$"));
        assert_eq!(kinds("select q'[it's]' from dual", Dialect::Oracle)[1], (Tk::Str, "q'[it's]'"));
        assert_eq!(kinds("select 'a\\'b', `x y`", Dialect::ClickHouse)[1], (Tk::Str, "'a\\'b'"));
        assert_eq!(kinds("select 'a\\'b', `x y`", Dialect::ClickHouse)[3], (Tk::Quoted, "`x y`"));
        assert_eq!(kinds("select E'a\\'b'", Dialect::Postgres)[1], (Tk::Str, "E'a\\'b'"));
        assert_eq!(kinds("x::int", Dialect::Postgres)[1], (Tk::Op, "::"));
        let t = lex("select 'abc", Dialect::Postgres);
        assert!(t.last().unwrap().open);
        assert_eq!(kinds("begin null; end;\n/\nselect 1 / 2", Dialect::Oracle).iter().filter(|t| t.0 == Tk::Slash).count(), 1);
        assert_eq!(kinds("select {id:UInt32}", Dialect::ClickHouse)[1], (Tk::Param, "{id:UInt32}"));
    }

    #[test]
    fn round_trip() {
        let s = "select a.\"B\", 'x''y' -- c\n/* d */ from t where z = :p and w = $1;";
        let t = lex(s, Dialect::Postgres);
        assert_eq!(t.iter().map(|t| t.text(s)).collect::<String>(), s);
    }
}
