//! Rainbow bands under the indents (after listok): each line's indent gets
//! one band per construct it's nested in — brackets, CASE…END, PL/SQL's
//! BEGIN / DECLARE / IF / LOOP … END, and the clauses SELECT, FROM, JOIN,
//! WHERE, GROUP BY, ORDER BY, CONNECT BY, HAVING (to the next clause or the
//! bracket's end). A band runs from where a construct starts to where the
//! next one inside it does, or to the text. Each clause takes the colour
//! after the one before it (SELECT, then FROM…), what's inside a construct
//! the colour after its own.

use std::ops::Range;

use super::Dialect;
use super::embed::{self, Part};
use super::lex::{self, Tk};

/// A band: bytes of a line's indent, and its colour's level.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub range: Range<usize>,
    pub level: usize,
}

/// The indent's width of a line (tabs to 4).
fn indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

/// The byte of the line where its indent reaches column `col`.
fn byte_at_col(line: &str, col: usize) -> usize {
    let mut at = 0;
    for (i, c) in line.char_indices() {
        if at >= col || !(c == ' ' || c == '\t') {
            return i;
        }
        at += if c == '\t' { 4 } else { 1 };
    }
    line.len()
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ev {
    /// A bracket, its construct's column.
    Open(usize),
    Close,
    Case(usize),
    /// BEGIN / IF / LOOP: the line's indent; DECLARE (its BEGIN goes on);
    /// the byte it starts at (FOR's of FOR … LOOP).
    Block(usize, bool, usize),
    Begin(usize),
    /// END; END IF / END LOOP (true: only a block).
    End(bool),
    /// A clause word: its column, and whether it opens a band (UNION only
    /// ends them).
    Clause(usize, bool),
    /// SQL in a string starts / ends.
    Enter,
    Leave,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Bracket,
    Case,
    Block,
    Clause,
    /// SQL in a string: no band; what's outside it isn't closed from in it.
    Fence,
}

/// An open construct: its column, kind and colour's level; `next`: the
/// level of the next clause inside it; `decl`: DECLARE waiting for BEGIN.
#[derive(Clone, Copy, Debug)]
struct Open {
    /// The byte it starts at.
    at: usize,
    col: usize,
    kind: Kind,
    level: usize,
    next: usize,
    decl: bool,
}

/// The open constructs. A clause stays open after the next one starts, as
/// a tail under it, while the next starts right of it.
#[derive(Clone, Default)]
struct Stack {
    open: Vec<Open>,
    /// The byte of what's being applied.
    pos: usize,
    /// Note what ends in `closed` (not when only looking ahead).
    rec: bool,
    closed: Vec<Block>,
    /// The next top-level clause's level.
    next: usize,
}

impl Stack {
    fn push(&mut self, col: usize, kind: Kind, level: usize, decl: bool) {
        self.open.push(Open { at: self.pos, col, kind, level, next: level + 1, decl });
    }

    /// Ends the constructs from `n` on (at `pos`; noted when `rec`).
    fn cut(&mut self, n: usize) {
        if self.rec {
            for o in self.open.drain(n..) {
                if matches!(o.kind, Kind::Bracket | Kind::Case | Kind::Block) {
                    self.closed.push(Block { range: o.at..self.pos, col: o.col, level: o.level });
                }
            }
        } else {
            self.open.truncate(n);
        }
    }

    fn inner(&self) -> usize {
        self.open.iter().rev().find(|o| o.kind != Kind::Fence).map_or(0, |o| o.level + 1)
    }

    /// Ends the constructs back to the last of `kinds` (not past a bracket,
    /// nor — for a block's END — past a block).
    fn pop_to(&mut self, kinds: &[Kind]) {
        if let Some(n) = self.open.iter().rposition(|s| kinds.contains(&s.kind) || matches!(s.kind, Kind::Bracket | Kind::Fence)) {
            if kinds.contains(&self.open[n].kind) {
                self.cut(n);
            }
        }
    }

    /// Clause tails at or right of `col` end.
    fn hit(&mut self, col: usize) {
        while self.open.last().is_some_and(|s| s.kind == Kind::Clause && s.col >= col) {
            self.cut(self.open.len() - 1);
        }
    }

    /// An event (`open`: false — only its closing part).
    fn apply(&mut self, e: Ev, open: bool) {
        match e {
            Ev::Open(col) if open => self.push(col, Kind::Bracket, self.inner(), false),
            Ev::Case(col) if open => self.push(col, Kind::Case, self.inner(), false),
            Ev::Block(col, decl, from) => {
                self.hit(col);
                if open {
                    self.push(col, Kind::Block, self.inner(), decl);
                    if let Some(o) = self.open.last_mut() {
                        o.at = from;
                    }
                }
            }
            Ev::Begin(col) => {
                self.hit(col);
                // DECLARE … BEGIN is one block.
                if let Some(o) = self.open.last_mut().filter(|o| o.kind == Kind::Block && o.decl) {
                    if open {
                        o.decl = false;
                    }
                } else if open {
                    self.push(col, Kind::Block, self.inner(), false);
                }
            }
            Ev::Close => self.pop_to(&[Kind::Bracket]),
            Ev::End(true) => self.pop_to(&[Kind::Block]),
            Ev::End(false) => {
                // END closes the last CASE or block, clause tails with it.
                if let Some(n) = self.open.iter().rposition(|s| matches!(s.kind, Kind::Case | Kind::Block | Kind::Bracket | Kind::Fence)) {
                    if matches!(self.open[n].kind, Kind::Case | Kind::Block) {
                        self.cut(n);
                    }
                }
            }
            Ev::Clause(col, true) => {
                self.hit(col);
                if open {
                    // The colour after the last clause's here.
                    let owner = match self.open.iter_mut().rev().find(|o| o.kind != Kind::Clause) {
                        Some(o) => &mut o.next,
                        None => &mut self.next,
                    };
                    let level = *owner;
                    *owner += 1;
                    self.push(col, Kind::Clause, level, false);
                }
            }
            Ev::Clause(_, false) => self.hit(0),
            Ev::Enter if open => {
                let level = self.inner();
                self.open.push(Open { at: self.pos, col: 0, kind: Kind::Fence, level, next: level, decl: false });
            }
            Ev::Leave => {
                if let Some(n) = self.open.iter().rposition(|s| s.kind == Kind::Fence) {
                    self.cut(n);
                }
            }
            _ => {}
        }
    }
}

/// Where a bracket's construct starts, given its line up to it: the word or
/// operator right before it (FROM (, IN (, = (), else the bracket itself.
fn opener_col(before: &str) -> usize {
    let chars: Vec<char> = before.chars().collect();
    let mut i = chars.len();
    while i > 0 && chars[i - 1] == ' ' {
        i -= 1;
    }
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
    let op = |c: char| "=<>!+-*/|%".contains(c);
    match chars.get(i.wrapping_sub(1)) {
        Some(&c) if word(c) || op(c) => {
            let kind: &dyn Fn(char) -> bool = if word(c) { &word } else { &op };
            while i > 0 && kind(chars[i - 1]) {
                i -= 1;
            }
            i
        }
        _ => chars.len(),
    }
}

/// How deep strings in strings are followed.
const MAX_DEPTH: usize = 4;

/// What one pass over the script gathers: events at source bytes, lines
/// that start inside a string or comment, and where SQL strings' parts
/// start (their quote, text, and text's first char past blanks).
struct Gather<'a> {
    code: &'a str,
    starts: Vec<usize>,
    events: Vec<(usize, Ev)>,
    quiet: Vec<bool>,
    parts: Vec<(usize, usize, usize)>,
}

impl Gather<'_> {
    fn line_of(&self, at: usize) -> usize {
        self.starts.partition_point(|&s| s <= at) - 1
    }

    fn col(&self, at: usize) -> usize {
        self.code[self.starts[self.line_of(at)]..at].chars().count()
    }

    fn line_indent(&self, at: usize) -> usize {
        indent(&self.code[self.starts[self.line_of(at)]..])
    }

    /// Lines after the one with `a`, up to `b`, start inside: no bands.
    fn hush(&mut self, a: usize, b: usize) {
        for li in self.line_of(a) + 1..self.starts.len() {
            if self.starts[li] >= b {
                break;
            }
            self.quiet[li] = true;
        }
    }
}

/// The events of `text` (the script, or the SQL in its strings: `map`
/// takes its bytes to the script's), and of the SQL in its strings.
fn gather(g: &mut Gather, text: &str, d: Dialect, map: &dyn Fn(usize) -> Option<usize>, depth: usize) {
    let toks = lex::lex(text, d);
    let regions = if depth < MAX_DEPTH { embed::regions(text, &toks, d) } else { Vec::new() };
    let sql_part = |t: &lex::Tok| regions.iter().any(|r| r.parts.iter().any(|p| matches!(p, Part::Text { tok, .. } if tok.start == t.start)));
    for t in toks.iter().filter(|t| matches!(t.kind, Tk::Str | Tk::BlockComment | Tk::Quoted)) {
        if t.kind == Tk::Str && sql_part(t) {
            continue;
        }
        if let (Some(a), Some(b)) = (map(t.start), map(t.end.saturating_sub(1)).map(|b| b + 1)) {
            g.hush(a, b);
        }
    }
    let nt: Vec<_> = toks.iter().filter(|t| !t.trivia()).collect();
    let up = |k: usize| nt.get(k).filter(|t| matches!(t.kind, Tk::Word | Tk::Keyword)).map(|t| t.text(text).to_ascii_uppercase()).unwrap_or_default();
    // A subprogram's (or package's) header seen: its IS / AS opens its
    // declarations like DECLARE: (the header line's indent, bracket depth).
    let mut header: Option<(usize, i32)> = None;
    // FOR / WHILE … LOOP: the loop starts on FOR's line.
    let mut head: Option<(usize, i32, usize)> = None;
    let mut brackets = 0i32;
    let mut k = 0;
    while k < nt.len() {
        let t = nt[k];
        let Some(at) = map(t.start) else {
            k += 1;
            continue;
        };
        match t.kind {
            Tk::Open if t.text(text) == "(" => {
                brackets += 1;
                let ls = g.starts[g.line_of(at)];
                g.events.push((at, Ev::Open(opener_col(&g.code[ls..at]))));
            }
            Tk::Close if t.text(text) == ")" => {
                brackets -= 1;
                g.events.push((at, Ev::Close));
            }
            Tk::Semi => {
                header = None;
                head = None;
            }
            Tk::Word | Tk::Keyword => {
                let word = up(k);
                // Words before it on its line (LEFT JOIN, GROUP BY).
                let same_line = |j: usize| j < k && map(nt[j].start).is_some_and(|s| g.line_of(s) == g.line_of(at));
                let ev = match word.as_str() {
                    "PROCEDURE" | "FUNCTION" | "PACKAGE" | "BODY" if header.is_none() => {
                        header = Some((g.line_indent(at), brackets));
                        None
                    }
                    // (Not PostgreSQL's AS $$: its body is a string.)
                    "IS" | "AS" if header.is_some_and(|h| h.1 == brackets) && !nt.get(k + 1).is_some_and(|x| x.kind == Tk::Str) => {
                        let (col, _) = header.take().unwrap();
                        Some((at, Ev::Block(col, true, at)))
                    }
                    "CASE" => Some((at, Ev::Case(g.col(at)))),
                    "END" => {
                        let next = up(k + 1);
                        if matches!(next.as_str(), "IF" | "LOOP") {
                            g.events.push((at, Ev::End(true)));
                            k += 2;
                            continue;
                        }
                        g.events.push((at, Ev::End(false)));
                        if next == "CASE" {
                            k += 2;
                            continue;
                        }
                        None
                    }
                    "BEGIN" => Some((at, Ev::Begin(g.line_indent(at)))),
                    "DECLARE" => Some((at, Ev::Block(g.line_indent(at), true, at))),
                    // IF NOT EXISTS isn't a block.
                    "IF" if !matches!(up(k + 1).as_str(), "EXISTS" | "NOT") => Some((at, Ev::Block(g.line_indent(at), false, at))),
                    "FOR" | "WHILE" if head.is_none() => {
                        head = Some((g.line_indent(at), brackets, at));
                        None
                    }
                    "LOOP" => {
                        let (col, from) = head.take().filter(|h| h.1 == brackets).map_or((g.line_indent(at), at), |h| (h.0, h.2));
                        Some((at, Ev::Block(col, false, from)))
                    }
                    "SELECT" | "FROM" | "WHERE" | "HAVING" => Some((at, Ev::Clause(g.col(at), true))),
                    "BY" if k > 0 && same_line(k - 1) && matches!(up(k - 1).as_str(), "GROUP" | "ORDER" | "CONNECT") => map(nt[k - 1].start).map(|from| (from, Ev::Clause(g.col(from), true))),
                    "JOIN" => {
                        // From LEFT / INNER / FULL OUTER… before it.
                        let mods = ["LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "NATURAL"];
                        let mut j = k;
                        while j > 0 && same_line(j - 1) && mods.contains(&up(j - 1).as_str()) {
                            j -= 1;
                        }
                        map(nt[j].start).map(|from| (from, Ev::Clause(g.col(from), true)))
                    }
                    "UNION" | "INTERSECT" | "EXCEPT" | "MINUS" | "LIMIT" | "OFFSET" | "FETCH" | "INTO" | "VALUES" | "SET" | "WITH" | "START" | "RETURNING" | "WINDOW" | "QUALIFY" => Some((at, Ev::Clause(0, false))),
                    _ => None,
                };
                g.events.extend(ev);
            }
            _ => {}
        }
        k += 1;
    }
    // The SQL in strings: its constructs live inside a fence the string's
    // end takes down.
    for r in &regions {
        let (Some(a), Some(b)) = (map(r.range.start), map(r.range.end.saturating_sub(1)).map(|b| b + 1)) else {
            continue;
        };
        g.events.push((a, Ev::Enter));
        g.events.push((b, Ev::Leave));
        for p in &r.parts {
            if let Part::Text { tok, content, .. } = p {
                let first = content.start + (text[content.clone()].len() - text[content.clone()].trim_start_matches([' ', '\t']).len());
                if first < content.end && !text[content.start..first].contains('\n') {
                    if let (Some(q), Some(c), Some(f)) = (map(tok.start), map(content.start), map(first)) {
                        g.parts.push((q, c, f));
                    }
                }
            }
        }
        let v = embed::virtual_text(text, r);
        let inner = |p: usize| {
            let i = v.map.partition_point(|&(vs, _, len)| vs + len <= p);
            v.map.get(i).filter(|&&(vs, _, _)| vs <= p).and_then(|&(vs, ss, _)| map(ss + p - vs))
        };
        gather(g, &v.text, d, &inner, depth + 1);
    }
}

/// The byte of the line at column `col` (tabs to 4).
fn byte_at(line: &str, col: usize) -> usize {
    let mut at = 0;
    for (i, c) in line.char_indices() {
        if at >= col {
            return i;
        }
        at += if c == '\t' { 4 } else { 1 };
    }
    line.len()
}

/// A bracket, CASE or block: from its start to its end's first byte, its
/// band's column and colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub range: Range<usize>,
    pub col: usize,
    pub level: usize,
}

/// The bands of the whole text, line by line, in order.
#[cfg(test)]
pub fn bands(code: &str, d: Dialect) -> Vec<Band> {
    analyse(code, d).0
}

/// The bands, and the brackets / CASEs / blocks (by where they start).
pub fn analyse(code: &str, d: Dialect) -> (Vec<Band>, Vec<Block>) {
    let starts: Vec<usize> = std::iter::once(0).chain(code.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let n = starts.len();
    let mut g = Gather { code, starts, events: Vec::new(), quiet: vec![false; n], parts: Vec::new() };
    gather(&mut g, code, d, &Some, 0);
    let Gather { starts, mut events, quiet, mut parts, .. } = g;
    // (A string's end before what follows it right there.)
    events.sort_by_key(|e| (e.0, e.1 != Ev::Leave));
    parts.sort_unstable();
    let mut st = Stack { rec: true, ..Default::default() };
    let mut e = 0;
    let mut out = Vec::new();
    for (li, &ls) in starts.iter().enumerate() {
        let le = starts.get(li + 1).map_or(code.len(), |&n| n - 1);
        let l = &code[ls..le];
        let mut first = ls + (l.len() - l.trim_start().len());
        // A line that starts a SQL string's part ('…, || '…): its indent
        // goes on into the string.
        let mut lead = first;
        if code[lead..le].starts_with("||") {
            lead += 2;
            lead += code[lead..le].len() - code[lead..le].trim_start().len();
        }
        let part = parts.binary_search_by_key(&lead, |p| p.0).ok().map(|i| parts[i]).filter(|p| p.2 < le);
        let into = part.map(|p| p.2);
        if let Some(f) = into {
            first = f;
        }
        while e < events.len() && events[e].0 < first {
            st.pos = events[e].0;
            st.apply(events[e].1, true);
            e += 1;
        }
        if l.trim().is_empty() || quiet[li] {
            continue;
        }
        let w = match into {
            Some(f) => code[ls..f].chars().map(|c| if c == '\t' { 4 } else { 1 }).sum(),
            None => indent(l),
        };
        // Text at or left of a clause's column: its tail ends there.
        st.pos = ls.saturating_sub(1);
        st.hit(w);
        // What the line starts with closes before it (") x", "END", "FROM t").
        let mut here = Stack { open: st.open.clone(), next: st.next, pos: st.pos, rec: false, closed: Vec::new() };
        for ev in events[e..].iter().take_while(|ev| ev.0 == first) {
            here.apply(ev.1, false);
        }
        let o: Vec<&Open> = here.open.iter().filter(|o| o.kind != Kind::Fence).collect();
        let at = |col: usize| if into.is_some() { byte_at(l, col) } else { byte_at_col(l, col) };
        for (n, c) in o.iter().enumerate() {
            let a = c.col;
            // To where the next one starts (one right of a later one, as
            // "IS (" with its text under IS, hides behind it).
            let b = o[n + 1..].iter().map(|b| b.col).min().unwrap_or(usize::MAX).min(w);
            if a >= b {
                continue;
            }
            let (a, b) = (at(a), at(b));
            if into.is_none() {
                out.push(Band { range: ls + a..ls + b, level: c.level });
                continue;
            }
            // Only on the indent and the blanks inside the quote: not over
            // the || and the quote.
            let (lead, text) = (l.len() - l.trim_start().len(), part.map_or(0, |p| p.1 - ls));
            let mut run: Option<usize> = None;
            for (i, ch) in l[a..b].char_indices().map(|(i, ch)| (a + i, ch)).chain(std::iter::once((b, 'x'))) {
                let blank = (ch == ' ' || ch == '\t') && (i < lead || i >= text);
                match (blank, run) {
                    (true, None) => run = Some(i),
                    (false, Some(r)) => {
                        out.push(Band { range: ls + r..ls + i, level: c.level });
                        run = None;
                    }
                    _ => {}
                }
            }
        }
    }
    for &(at, ev) in &events[e..] {
        st.pos = at;
        st.apply(ev, true);
    }
    st.pos = code.len();
    st.cut(0);
    let mut blocks = st.closed;
    blocks.sort_by_key(|b| b.range.start);
    (out, blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per line: (from col, to col, level).
    fn cols(code: &str, d: Dialect) -> Vec<Vec<(usize, usize, usize)>> {
        let starts: Vec<usize> = std::iter::once(0).chain(code.match_indices('\n').map(|(i, _)| i + 1)).collect();
        let mut out = vec![Vec::new(); starts.len()];
        for b in bands(code, d) {
            let li = starts.partition_point(|&s| s <= b.range.start) - 1;
            out[li].push((b.range.start - starts[li], b.range.end - starts[li], b.level));
        }
        out
    }

    #[test]
    fn clauses_and_brackets() {
        let src = "SELECT a\n     , b\n  FROM t\n WHERE x IN (\n         SELECT 1\n           FROM u\n       )\n   AND y = 2";
        let c = cols(src, Dialect::Postgres);
        assert_eq!(c[1], vec![(0, 5, 0)]);
        // FROM's line: under SELECT's tail only up to FROM.
        assert_eq!(c[2], vec![(0, 2, 0)]);
        // Inside the bracket: SELECT's tail, WHERE's band, then the bracket's from "IN".
        assert_eq!(c[4], vec![(0, 1, 0), (1, 9, 2), (9, 9, 3)].into_iter().filter(|b| b.0 < b.1).collect::<Vec<_>>(), "{c:?}");
        assert!(c[7].iter().all(|b| b.1 <= 3), "{:?}", c[7]);
    }

    #[test]
    fn plsql_blocks() {
        let src = "declare\n  v number;\nbegin\n  if v > 0 then\n    null;\n  end if;\n  for r in (select 1 from dual) loop\n    null;\n  end loop;\nend;\nselect 1 from dual;";
        let c = cols(src, Dialect::Oracle);
        assert_eq!(c[1], vec![(0, 2, 0)]);
        assert_eq!(c[3], vec![(0, 2, 0)]);
        assert_eq!(c[4], vec![(0, 2, 0), (2, 4, 1)]);
        assert_eq!(c[5], vec![(0, 2, 0)]);
        assert_eq!(c[7], vec![(0, 2, 0), (2, 4, 1)]);
        assert_eq!(c[9], Vec::<(usize, usize, usize)>::new());
        // IF NOT EXISTS opens nothing.
        let c = cols("create table if not exists t (\n  id int\n);\nselect 1;", Dialect::Postgres);
        assert_eq!(c[3], Vec::<(usize, usize, usize)>::new());
    }

    #[test]
    fn subprogram_declarations() {
        let src = "create or replace procedure p(a number) is\n  v number;\n  cursor c is select 1 from dual;\nbegin\n  null;\nend;\n/\ncreate function f() returns int as $$ select 1 $$ language sql;\nselect 1;";
        let c = cols(src, Dialect::Oracle);
        assert_eq!(c[1], vec![(0, 2, 0)]);
        assert_eq!(c[2], vec![(0, 2, 0)]);
        assert_eq!(c[4], vec![(0, 2, 0)]);
        assert!(c[8].is_empty());
    }

    #[test]
    fn strings_have_none() {
        let src = "select 'a\n    b' from t\n  where x = 1";
        let c = cols(src, Dialect::Postgres);
        assert!(c[1].is_empty());
    }

    #[test]
    fn sql_in_strings() {
        // A multi-line string: its lines get the SQL's bands.
        let src = "begin\n  execute immediate 'select a\n                       from t\n                      where x = 1';\nend;";
        let c = cols(src, Dialect::Oracle);
        assert_eq!(c[2], vec![(0, 21, 0), (21, 23, 1)], "{c:?}");
        assert_eq!(c[3], vec![(0, 21, 0), (21, 22, 1)], "{c:?}");
        // A $$ body is code with bands.
        let src = "create function f() returns int as $$\nbegin\n  if x then\n    return 1;\n  end if;\nend;\n$$ language plpgsql;\nselect 1;";
        let c = cols(src, Dialect::Postgres);
        assert_eq!(c[3], vec![(0, 2, 0), (2, 4, 1)], "{c:?}");
        assert!(c[7].is_empty(), "{c:?}");
        // Glued with ||: the blanks inside the quote, not the || itself.
        let src = "begin\n  v := 'select a '\n    || 'from t '\n    || 'where x = 1 '\n    || '  and y = 2';\nend;";
        let c = cols(src, Dialect::Oracle);
        assert_eq!(c[2], vec![(0, 4, 0)], "{c:?}");
        assert_eq!(c[4], vec![(0, 4, 0), (8, 10, 3)], "{c:?}");
        // A string's unmatched END doesn't close the block it's in.
        let src = "begin\n  v := 'select case when a then 1 end from t';\n  x := 1;\nend;";
        let c = cols(src, Dialect::Oracle);
        assert_eq!(c[2], vec![(0, 2, 0)], "{c:?}");
    }

    #[test]
    fn bands_never_overlap() {
        // The bracket's text left of where its construct starts.
        let src = "declare\n  cursor c is (\n    select a\n         , b\n      from dual\n  );\nbegin\n  null;\nend;";
        let c = cols(src, Dialect::Oracle);
        for line in &c {
            for w in line.windows(2) {
                assert!(w[0].1 <= w[1].0, "{c:?}");
            }
        }
        assert_eq!(c[3], vec![(0, 4, 0), (4, 9, 2)], "{c:?}");
    }

    #[test]
    fn loop_on_a_later_line() {
        let src = "begin\n  for r in (select id\n              from t) loop\n    null;\n  end loop;\nend;";
        let c = cols(src, Dialect::Oracle);
        assert_eq!(c[3], vec![(0, 2, 0), (2, 4, 1)], "{c:?}");
        // The loop's block from FOR to END LOOP.
        let (_, blocks) = analyse(src, Dialect::Oracle);
        let at = |w: &str| src.find(w).unwrap();
        assert!(blocks.contains(&Block { range: at("for")..at("end loop"), col: 2, level: 1 }), "{blocks:?}");
        assert!(blocks.iter().any(|b| b.range.start == at("begin") && b.range.end == src.rfind("end;").unwrap()), "{blocks:?}");
    }
}
