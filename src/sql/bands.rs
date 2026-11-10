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
    /// BEGIN / IF / LOOP: the line's indent; DECLARE (its BEGIN goes on).
    Block(usize, bool),
    Begin(usize),
    /// END; END IF / END LOOP (true: only a block).
    End(bool),
    /// A clause word: its column, and whether it opens a band (UNION only
    /// ends them).
    Clause(usize, bool),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Bracket,
    Case,
    Block,
    Clause,
}

/// An open construct: its column, kind and colour's level; `next`: the
/// level of the next clause inside it; `decl`: DECLARE waiting for BEGIN.
#[derive(Clone, Copy, Debug)]
struct Open {
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
    /// The next top-level clause's level.
    next: usize,
}

impl Stack {
    fn push(&mut self, col: usize, kind: Kind, level: usize, decl: bool) {
        self.open.push(Open { col, kind, level, next: level + 1, decl });
    }

    fn inner(&self) -> usize {
        self.open.last().map_or(0, |o| o.level + 1)
    }

    /// Ends the constructs back to the last of `kinds` (not past a bracket,
    /// nor — for a block's END — past a block).
    fn pop_to(&mut self, kinds: &[Kind]) {
        if let Some(n) = self.open.iter().rposition(|s| kinds.contains(&s.kind) || s.kind == Kind::Bracket) {
            if kinds.contains(&self.open[n].kind) {
                self.open.truncate(n);
            }
        }
    }

    /// Clause tails at or right of `col` end.
    fn hit(&mut self, col: usize) {
        while self.open.last().is_some_and(|s| s.kind == Kind::Clause && s.col >= col) {
            self.open.pop();
        }
    }

    /// An event (`open`: false — only its closing part).
    fn apply(&mut self, e: Ev, open: bool) {
        match e {
            Ev::Open(col) if open => self.push(col, Kind::Bracket, self.inner(), false),
            Ev::Case(col) if open => self.push(col, Kind::Case, self.inner(), false),
            Ev::Block(col, decl) => {
                self.hit(col);
                if open {
                    self.push(col, Kind::Block, self.inner(), decl);
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
                if let Some(n) = self.open.iter().rposition(|s| matches!(s.kind, Kind::Case | Kind::Block | Kind::Bracket)) {
                    if self.open[n].kind != Kind::Bracket {
                        self.open.truncate(n);
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

/// The bands of the whole text, line by line, in order.
pub fn bands(code: &str, d: Dialect) -> Vec<Band> {
    let starts: Vec<usize> = std::iter::once(0).chain(code.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_of = |at: usize| starts.partition_point(|&s| s <= at) - 1;
    let col = |at: usize| code[starts[line_of(at)]..at].chars().count();
    let toks = lex::lex(code, d);
    // Lines that start inside a string or comment have no bands.
    let mut quiet = vec![false; starts.len()];
    for t in toks.iter().filter(|t| matches!(t.kind, Tk::Str | Tk::BlockComment | Tk::Quoted)) {
        for (li, s) in starts.iter().enumerate().skip(line_of(t.start) + 1) {
            if *s >= t.end {
                break;
            }
            quiet[li] = true;
        }
    }
    // The events in order, at the first char of their construct.
    let mut events: Vec<(usize, Ev)> = Vec::new();
    let nt: Vec<_> = toks.iter().filter(|t| !t.trivia()).collect();
    let up = |k: usize| nt.get(k).filter(|t| matches!(t.kind, Tk::Word | Tk::Keyword)).map(|t| t.text(code).to_ascii_uppercase()).unwrap_or_default();
    let line_indent = |at: usize| {
        let s = starts[line_of(at)];
        indent(&code[s..])
    };
    // A subprogram's (or package's) header seen: its IS / AS opens its
    // declarations like DECLARE: (the header line's indent, bracket depth).
    let mut header: Option<(usize, i32)> = None;
    let mut depth = 0i32;
    let mut k = 0;
    while k < nt.len() {
        let t = nt[k];
        let at = t.start;
        match t.kind {
            Tk::Open if t.text(code) == "(" => {
                depth += 1;
                events.push((at, Ev::Open(opener_col(&code[starts[line_of(at)]..at]))));
            }
            Tk::Close if t.text(code) == ")" => {
                depth -= 1;
                events.push((at, Ev::Close));
            }
            Tk::Semi => header = None,
            Tk::Word | Tk::Keyword => {
                let word = up(k);
                // Words before it on its line (LEFT JOIN, GROUP BY).
                let same_line = |j: usize| j < k && line_of(nt[j].start) == line_of(at);
                match word.as_str() {
                    "PROCEDURE" | "FUNCTION" | "PACKAGE" | "BODY" if header.is_none() => header = Some((line_indent(at), depth)),
                    // (Not PostgreSQL's AS $$: its body is a string.)
                    "IS" | "AS" if header.is_some_and(|h| h.1 == depth) && !nt.get(k + 1).is_some_and(|x| x.kind == Tk::Str) => {
                        let (col, _) = header.take().unwrap();
                        events.push((at, Ev::Block(col, true)));
                    }
                    "CASE" => events.push((at, Ev::Case(col(at)))),
                    "END" => {
                        let next = up(k + 1);
                        if matches!(next.as_str(), "IF" | "LOOP") {
                            events.push((at, Ev::End(true)));
                            k += 2;
                            continue;
                        }
                        events.push((at, Ev::End(false)));
                        if next == "CASE" {
                            k += 2;
                            continue;
                        }
                    }
                    "BEGIN" => events.push((at, Ev::Begin(line_indent(at)))),
                    "DECLARE" => events.push((at, Ev::Block(line_indent(at), true))),
                    // IF NOT EXISTS isn't a block.
                    "IF" if !matches!(up(k + 1).as_str(), "EXISTS" | "NOT") => events.push((at, Ev::Block(line_indent(at), false))),
                    "LOOP" => events.push((at, Ev::Block(line_indent(at), false))),
                    "SELECT" | "FROM" | "WHERE" | "HAVING" => events.push((at, Ev::Clause(col(at), true))),
                    "BY" if k > 0 && same_line(k - 1) && matches!(up(k - 1).as_str(), "GROUP" | "ORDER" | "CONNECT") => {
                        let from = nt[k - 1].start;
                        events.push((from, Ev::Clause(col(from), true)));
                    }
                    "JOIN" => {
                        // From LEFT / INNER / FULL OUTER… before it.
                        let mods = ["LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "NATURAL"];
                        let mut j = k;
                        while j > 0 && same_line(j - 1) && mods.contains(&up(j - 1).as_str()) {
                            j -= 1;
                        }
                        let from = nt[j].start;
                        events.push((from, Ev::Clause(col(from), true)));
                    }
                    "UNION" | "INTERSECT" | "EXCEPT" | "MINUS" | "LIMIT" | "OFFSET" | "FETCH" | "INTO" | "VALUES" | "SET" | "WITH" | "START" | "RETURNING" | "WINDOW" | "QUALIFY" => events.push((at, Ev::Clause(0, false))),
                    _ => {}
                }
            }
            _ => {}
        }
        k += 1;
    }
    events.sort_by_key(|e| e.0);
    let mut st = Stack::default();
    let mut e = 0;
    let mut out = Vec::new();
    for (li, &ls) in starts.iter().enumerate() {
        let le = starts.get(li + 1).map_or(code.len(), |&n| n - 1);
        let l = &code[ls..le];
        let first = ls + (l.len() - l.trim_start().len());
        while e < events.len() && events[e].0 < first {
            st.apply(events[e].1, true);
            e += 1;
        }
        if l.trim().is_empty() || quiet[li] {
            continue;
        }
        let w = indent(l);
        // Text at or left of a clause's column: its tail ends there.
        st.hit(w);
        // What the line starts with closes before it (") x", "END", "FROM t").
        let mut here = st.clone();
        for ev in events[e..].iter().take_while(|ev| ev.0 == first) {
            here.apply(ev.1, false);
        }
        let o = &here.open;
        for (n, c) in o.iter().enumerate() {
            let a = c.col;
            let b = o.get(n + 1).map_or(usize::MAX, |b| b.col).min(w);
            if a < b {
                out.push(Band { range: ls + byte_at_col(l, a)..ls + byte_at_col(l, b), level: c.level });
            }
        }
    }
    out
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
}
