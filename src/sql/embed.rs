//! SQL inside strings: dynamic SQL (EXECUTE IMMEDIATE, EXECUTE, OPEN … FOR,
//! format(), DBMS_SQL.PARSE), strings kept in variables that end up there,
//! literals that read as a statement, and PostgreSQL's $$ bodies.
//!
//! A statement built by concatenation ('select a from t where id = ' || p
//! || ' and x = ''y''') is read as one text with each expression standing
//! in as a placeholder word, so a statement missing its pieces still lexes
//! whole: quotes, brackets and clauses pair up across the pieces.

use std::collections::HashSet;
use std::ops::Range;

use super::lex::{self, Tk, Tok};
use super::{Case, Dialect};

/// How deep strings in strings are followed.
const MAX_LEVEL: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quote {
    /// '…' with '' inside; `prefix`: N.
    Plain { prefix: usize },
    /// Backslash escapes as well (E'…', ClickHouse).
    Backslash { prefix: usize },
    /// Oracle q'[…]': `open` is q'[ (or nq'[), `close` the delimiter's pair.
    Q { open: usize, close: u8 },
    /// $tag$…$tag$: the tag's length.
    Dollar { tag: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// A string literal: its token and the bytes between its quotes.
    Text { tok: Range<usize>, content: Range<usize>, quote: Quote },
    Expr(Range<usize>),
}

#[derive(Clone, Debug)]
pub struct Region {
    pub range: Range<usize>,
    pub parts: Vec<Part>,
    /// A $$ body: procedural code, not one statement.
    pub body: bool,
    /// Reads as a whole statement (else a piece of one).
    pub statement: bool,
    /// Only its parts and || (no comments in between to lose).
    pub clean: bool,
}

/// A region as one SQL text.
#[derive(Debug, Default)]
pub struct Virtual {
    pub text: String,
    /// Runs copied byte for byte: (virtual start, source start, length).
    pub map: Vec<(usize, usize, usize)>,
    /// Placeholders: (virtual range, part index).
    pub holes: Vec<(Range<usize>, usize)>,
}

pub fn placeholder(i: usize) -> String {
    format!("zq{i}zq")
}

/// A string token's quoting and the bytes between its quotes.
pub fn quote_of(src: &str, t: &Tok, d: Dialect) -> Option<(Quote, Range<usize>)> {
    if t.kind != Tk::Str {
        return None;
    }
    let s = t.text(src);
    let b = s.as_bytes();
    let (start, end) = (t.start, t.end);
    if b.first() == Some(&b'$') {
        let tag = s[1..].find('$')? + 2;
        let close = if t.open || s.len() < tag * 2 { end } else { end - tag };
        return Some((Quote::Dollar { tag }, (start + tag).min(close)..close));
    }
    let lower = s.get(..3).unwrap_or(s).to_ascii_lowercase();
    if d == Dialect::Oracle && (lower.starts_with("q'") || lower.starts_with("nq'")) {
        let open = if lower.starts_with('n') { 4 } else { 3 };
        let delim = *b.get(open - 1)?;
        let close = match delim {
            b'[' => b']',
            b'(' => b')',
            b'{' => b'}',
            b'<' => b'>',
            o => o,
        };
        let stop = if t.open || s.len() < open + 2 { end } else { end - 2 };
        return Some((Quote::Q { open, close }, (start + open).min(stop)..stop));
    }
    let (prefix, backslash) = match b.first()? {
        b'\'' => (0, d == Dialect::ClickHouse),
        b'N' | b'n' => (1, d == Dialect::ClickHouse),
        b'E' | b'e' => (1, true),
        _ => return None,
    };
    let stop = if t.open || s.len() < prefix + 2 { end } else { end - 1 };
    let q = if backslash { Quote::Backslash { prefix } } else { Quote::Plain { prefix } };
    Some((q, (start + prefix + 1).min(stop)..stop))
}

/// A literal's content as text, and where each run of it came from.
fn unescape(src: &str, content: Range<usize>, quote: Quote, out: &mut Virtual) {
    let b = src.as_bytes();
    let push = |out: &mut Virtual, from: usize, to: usize| {
        if from < to {
            out.map.push((out.text.len(), from, to - from));
            out.text.push_str(&src[from..to]);
        }
    };
    match quote {
        Quote::Dollar { .. } | Quote::Q { .. } => push(out, content.start, content.end),
        Quote::Plain { .. } | Quote::Backslash { .. } => {
            let backslash = matches!(quote, Quote::Backslash { .. });
            let mut run = content.start;
            let mut i = content.start;
            while i < content.end {
                if b[i] == b'\'' && i + 1 < content.end && b[i + 1] == b'\'' {
                    // '' → ': the first one stands for it.
                    push(out, run, i + 1);
                    i += 2;
                    run = i;
                } else if backslash && b[i] == b'\\' && i + 1 < content.end && matches!(b[i + 1], b'\'' | b'\\' | b'"') {
                    push(out, run, i);
                    push(out, i + 1, i + 2);
                    i += 2;
                    run = i;
                } else {
                    i += 1;
                }
            }
            push(out, run, content.end);
        }
    }
}

/// The region read as one text.
pub fn virtual_text(src: &str, r: &Region) -> Virtual {
    let mut v = Virtual::default();
    for (i, p) in r.parts.iter().enumerate() {
        match p {
            Part::Text { content, quote, .. } => unescape(src, content.clone(), *quote, &mut v),
            Part::Expr(_) => {
                let at = v.text.len();
                v.text += &placeholder(i);
                v.holes.push((at..v.text.len(), i));
            }
        }
    }
    v
}

fn lower(src: &str, t: &Tok) -> String {
    t.text(src).to_ascii_lowercase()
}

fn is_word(t: &Tok) -> bool {
    matches!(t.kind, Tk::Word | Tk::Keyword)
}

/// Words a statement may start with (and what must come with them, so a
/// message like 'Update failed' isn't taken for one).
fn reads_as_statement(text: &str, d: Dialect) -> bool {
    let toks = lex::lex(text, d);
    let nt: Vec<&Tok> = toks.iter().filter(|t| !t.trivia()).take(400).collect();
    let words: Vec<String> = nt.iter().map(|t| lower(text, t)).collect();
    let Some(first) = words.first() else { return false };
    // A word on its own is a value, not a statement.
    if words.len() < 2 {
        return false;
    }
    let has = |w: &[&str]| words.iter().skip(1).any(|x| w.contains(&x.as_str()));
    match first.as_str() {
        "select" => has(&["from", "*", ",", "("]) || nt.get(1).is_some_and(|t| matches!(t.kind, Tk::Number | Tk::Param | Tk::Str)),
        "insert" | "merge" => has(&["into"]),
        "update" => has(&["set"]),
        "delete" => has(&["from", "where"]),
        "with" => has(&["as"]) && has(&["select", "insert", "update", "delete"]),
        "create" | "alter" | "drop" => has(&["table", "view", "index", "function", "procedure", "package", "trigger", "sequence", "type", "user", "schema", "database", "materialized", "synonym", "dictionary", "replace", "role", "partition", "column", "constraint"]),
        "truncate" => has(&["table"]) || words.len() == 2,
        "begin" | "declare" => has(&["end"]),
        "grant" | "revoke" => has(&["on", "to", "from"]),
        "call" | "exec" => has(&["("]),
        "comment" => has(&["on"]),
        "analyze" | "vacuum" | "optimize" | "explain" => true,
        _ => false,
    }
}

/// Finds the strings holding SQL. `toks`: of `src` in dialect `d`.
pub fn regions(src: &str, toks: &[Tok], d: Dialect) -> Vec<Region> {
    let nt: Vec<Tok> = toks.iter().filter(|t| !t.trivia()).copied().collect();
    let n = nt.len();
    let text = |k: usize| nt.get(k).map(|t| lower(src, t)).unwrap_or_default();
    let close_of = |k: usize| -> usize {
        let mut depth = 0i32;
        for (j, t) in nt.iter().enumerate().skip(k) {
            match t.kind {
                Tk::Open => depth += 1,
                Tk::Close => {
                    depth -= 1;
                    if depth == 0 {
                        return j;
                    }
                }
                _ => {}
            }
        }
        n.saturating_sub(1)
    };
    // One operand of a concatenation: its token span.
    let operand = |k: usize| -> Option<usize> {
        let t = nt.get(k)?;
        let after_name = |mut e: usize| {
            while nt.get(e).is_some_and(|x| x.kind == Tk::Dot) && nt.get(e + 1).is_some_and(|x| matches!(x.kind, Tk::Word | Tk::Keyword | Tk::Quoted)) {
                e += 2;
            }
            if nt.get(e).is_some_and(|x| x.kind == Tk::Open && x.text(src) == "(") {
                e = close_of(e) + 1;
            }
            e
        };
        match t.kind {
            Tk::Str | Tk::Number => Some(k + 1),
            Tk::Param | Tk::Word | Tk::Quoted => Some(after_name(k + 1)),
            Tk::Keyword => {
                let called = nt.get(k + 1).is_some_and(|x| x.kind == Tk::Open && x.text(src) == "(");
                let value = matches!(lower(src, t).as_str(), "sysdate" | "systimestamp" | "user" | "current_date" | "current_timestamp" | "current_user" | "null" | "chr");
                (called || value).then(|| after_name(k + 1))
            }
            Tk::Open if t.text(src) == "(" => Some(close_of(k) + 1),
            _ => None,
        }
    };
    let is_concat = |k: usize| nt.get(k).is_some_and(|t| t.kind == Tk::Op && t.text(src) == "||");
    // Where dynamic SQL goes: after these, or as an argument of these calls.
    let dynamic_before = |k: usize| -> bool {
        if k == 0 {
            return false;
        }
        let p = text(k - 1);
        if p == "immediate" || p == "execute" || (p == "for" && (k >= 3 && text(k - 3) == "open" || k >= 2 && text(k - 2) == "open")) {
            return true;
        }
        // Inside a call: which one, which argument.
        let mut depth = 0i32;
        let mut commas = 0;
        for j in (0..k).rev() {
            match nt[j].kind {
                Tk::Close => depth += 1,
                Tk::Open => {
                    if depth == 0 {
                        let name = if j > 0 { text(j - 1) } else { String::new() };
                        return match name.as_str() {
                            "format" | "execute" | "prepare" | "query" | "sp_executesql" | "exec" => commas == 0,
                            "parse" | "open_cursor" | "execute_immediate" => true,
                            _ => false,
                        };
                    }
                    depth -= 1;
                }
                Tk::Comma if depth == 0 => commas += 1,
                Tk::Semi => return false,
                _ => {}
            }
        }
        false
    };
    // The variable a chain is given to: v := …, or v varchar2(100) := ….
    let target = |k: usize| -> Option<String> {
        if k < 2 || text(k - 1) != ":=" {
            return None;
        }
        let mut j = k - 2;
        if nt[j].kind == Tk::Close {
            let mut depth = 0i32;
            loop {
                match nt[j].kind {
                    Tk::Close => depth += 1,
                    Tk::Open => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                if j == 0 {
                    return None;
                }
                j -= 1;
            }
            j = j.checked_sub(1)?;
        }
        if j > 0 && nt[j - 1].kind == Tk::Word {
            return Some(text(j - 1));
        }
        is_word(&nt[j]).then(|| text(j))
    };

    struct Cand {
        ops: Vec<Range<usize>>,
        var: Option<String>,
        dynamic: bool,
    }
    let mut cands: Vec<Cand> = Vec::new();
    let mut bodies: Vec<Region> = Vec::new();
    let mut sql_vars: HashSet<String> = HashSet::new();
    let mut k = 0;
    while k < n {
        let t = nt[k];
        if t.kind == Tk::Str && d == Dialect::Postgres && t.text(src).starts_with('$') {
            if let Some((quote, content)) = quote_of(src, &t, d) {
                let prev = if k > 0 { text(k - 1) } else { String::new() };
                let first = super::first_word(&src[content.clone()]).to_ascii_lowercase();
                let code = matches!(prev.as_str(), "as" | "do" | "is") || matches!(first.as_str(), "begin" | "declare" | "select" | "insert" | "update" | "delete" | "with" | "return");
                if code && !content.is_empty() {
                    bodies.push(Region { range: t.start..t.end, parts: vec![Part::Text { tok: t.start..t.end, content, quote }], body: true, statement: false, clean: true });
                    k += 1;
                    continue;
                }
            }
        }
        if let Some(mut e) = operand(k) {
            let mut ops = vec![k..e];
            while is_concat(e) {
                match operand(e + 1) {
                    Some(e2) => {
                        ops.push(e + 1..e2);
                        e = e2;
                    }
                    None => break,
                }
            }
            let strs = ops.iter().filter(|r| r.len() == 1 && quote_of(src, &nt[r.start], d).is_some()).count();
            let dynamic = dynamic_before(k);
            if dynamic {
                // EXECUTE IMMEDIATE v_sql: v_sql holds SQL.
                for r in &ops {
                    if r.len() == 1 && nt[r.start].kind == Tk::Word {
                        sql_vars.insert(text(r.start));
                    }
                }
            }
            if strs > 0 && (ops.len() > 1 || nt[k].kind == Tk::Str) {
                cands.push(Cand { ops, var: target(k), dynamic });
                k = e;
                continue;
            }
        }
        k += 1;
    }

    let mut out = bodies;
    let mut built: Vec<(Region, Option<String>, bool, Vec<String>)> = Vec::new();
    for c in cands {
        let parts: Vec<Part> = c
            .ops
            .iter()
            .map(|r| match quote_of(src, &nt[r.start], d).filter(|_| r.len() == 1) {
                Some((quote, content)) => Part::Text { tok: nt[r.start].start..nt[r.start].end, content, quote },
                None => Part::Expr(nt[r.start].start..nt[r.end - 1].end),
            })
            .collect();
        let range = nt[c.ops[0].start].start..nt[c.ops.last().unwrap().end - 1].end;
        let clean = !toks.iter().any(|t| t.start >= range.start && t.end <= range.end && matches!(t.kind, Tk::LineComment | Tk::BlockComment));
        let words: Vec<String> = c.ops.iter().filter(|r| r.len() == 1 && nt[r.start].kind == Tk::Word).map(|r| text(r.start)).collect();
        let mut reg = Region { range, parts, body: false, statement: false, clean };
        let v = virtual_text(src, &reg);
        reg.statement = reads_as_statement(&v.text, d);
        let sure = c.dynamic || reg.statement;
        if sure {
            if let Some(var) = &c.var {
                sql_vars.insert(var.clone());
            }
        }
        built.push((reg, c.var, sure, words));
    }
    // Pieces given to (or taken from) a variable holding SQL count too.
    for (reg, var, sure, words) in built {
        if sure || var.as_ref().is_some_and(|v| sql_vars.contains(v)) || words.iter().any(|w| sql_vars.contains(w)) {
            out.push(reg);
        }
    }
    out.sort_by_key(|r| r.range.start);
    out
}

// ---------------------------------------------------------------- colours

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Text,
    Keyword,
    Function,
    Type,
    Str,
    Number,
    Comment,
    Quoted,
    Param,
    Op,
    /// A bracket at this depth.
    Bracket(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub class: Class,
    /// 0: the script itself; 1: SQL in a string; 2: in a string in that…
    /// ($$ bodies are code: they don't count.)
    pub level: u8,
}

/// Every byte of `src` in a class; SQL in strings coloured as SQL (when
/// `deep`).
pub fn classify(src: &str, d: Dialect, deep: bool) -> Vec<Span> {
    classify_at(src, d, if deep { 0 } else { MAX_LEVEL }, 0)
}

fn base_spans(src: &str, toks: &[Tok], level: u8) -> Vec<Span> {
    let mut depth = 0u8;
    let next_is_call = |k: usize| toks[k + 1..].iter().find(|x| x.kind != Tk::Space).is_some_and(|x| x.kind == Tk::Open && x.text(src) == "(");
    toks.iter()
        .enumerate()
        .map(|(k, t)| {
            let class = match t.kind {
                Tk::Keyword => {
                    let not_fn = matches!(t.text(src).to_ascii_uppercase().as_str(), "IN" | "EXISTS" | "AS" | "VALUES" | "ON" | "USING" | "OVER" | "AND" | "OR" | "NOT" | "INTO" | "ANY" | "ALL" | "FILTER" | "WITHIN");
                    if next_is_call(k) && !not_fn { Class::Function } else { Class::Keyword }
                }
                Tk::Word => {
                    let after_dot = k > 0 && toks[k - 1].kind == Tk::Dot;
                    if next_is_call(k) && !after_dot {
                        Class::Function
                    } else if super::is_type(t.text(src)) && !after_dot {
                        Class::Type
                    } else {
                        Class::Text
                    }
                }
                Tk::Str => Class::Str,
                Tk::Number => Class::Number,
                Tk::LineComment | Tk::BlockComment => Class::Comment,
                Tk::Quoted => Class::Quoted,
                Tk::Param => Class::Param,
                Tk::Open => {
                    depth = depth.wrapping_add(1);
                    Class::Bracket(depth - 1)
                }
                Tk::Close => {
                    depth = depth.saturating_sub(1);
                    Class::Bracket(depth)
                }
                Tk::Op | Tk::Comma | Tk::Semi | Tk::Dot | Tk::Slash => Class::Op,
                Tk::Space | Tk::Other => Class::Text,
            };
            Span { range: t.start..t.end, class, level }
        })
        .collect()
}

/// `depth`: how many strings in; `level`: of those, not $$ bodies.
fn classify_at(src: &str, d: Dialect, depth: u8, level: u8) -> Vec<Span> {
    let toks = lex::lex(src, d);
    let base = base_spans(src, &toks, level);
    if depth >= MAX_LEVEL {
        return base;
    }
    let mut over: Vec<Span> = Vec::new();
    for r in regions(src, &toks, d) {
        let v = virtual_text(src, &r);
        let inner = if r.body { level } else { level + 1 };
        for s in classify_at(&v.text, d, depth + 1, inner) {
            map_back(&v, &s, &mut over);
        }
    }
    over.sort_by_key(|s| s.range.start);
    flatten(base, over)
}

/// A span of the virtual text onto the source bytes it came from.
fn map_back(v: &Virtual, s: &Span, out: &mut Vec<Span>) {
    let first = v.map.partition_point(|&(vs, _, len)| vs + len <= s.range.start);
    for &(vs, ss, len) in &v.map[first..] {
        if vs >= s.range.end {
            break;
        }
        let lo = s.range.start.max(vs);
        let hi = s.range.end.min(vs + len);
        if lo < hi {
            out.push(Span { range: ss + (lo - vs)..ss + (hi - vs), class: s.class, level: s.level });
        }
    }
}

/// `base` covers the text; `over` (sorted, not overlapping) wins where it is.
fn flatten(base: Vec<Span>, over: Vec<Span>) -> Vec<Span> {
    if over.is_empty() {
        return base;
    }
    let mut out = Vec::with_capacity(base.len() + over.len());
    let mut o = 0;
    for b in base {
        while o < over.len() && over[o].range.end <= b.range.start {
            o += 1;
        }
        let mut p = b.range.start;
        let mut j = o;
        while j < over.len() && over[j].range.start < b.range.end {
            let s = &over[j];
            let lo = s.range.start.max(p);
            if lo > p {
                out.push(Span { range: p..lo, ..b.clone() });
            }
            let hi = s.range.end.min(b.range.end);
            if lo < hi {
                out.push(Span { range: lo..hi, class: s.class, level: s.level });
                p = hi;
            }
            if s.range.end > b.range.end {
                break;
            }
            j += 1;
        }
        if p < b.range.end {
            out.push(Span { range: p..b.range.end, ..b });
        }
    }
    out
}

// ---------------------------------------------------------------- layout

/// The column (in characters) of `at`; `col0`: of the text's first byte.
fn column(src: &str, at: usize, col0: usize) -> usize {
    match src[..at].rfind('\n') {
        Some(i) => src[i + 1..at].chars().count(),
        None => col0 + src[..at].chars().count(),
    }
}

/// A literal in the given quoting (falling back to '…' when the text
/// would end a q'[…]' early).
fn literal(s: &str, q: Quote) -> String {
    match q {
        Quote::Q { open, close } if !s.contains(&format!("{}'", close as char)) => {
            let opener = if open == 4 { "nq'" } else { "q'" };
            let delim = match close {
                b']' => '[',
                b')' => '(',
                b'}' => '{',
                b'>' => '<',
                c => c as char,
            };
            format!("{opener}{delim}{s}{}'", close as char)
        }
        Quote::Plain { prefix: 1 } | Quote::Backslash { prefix: 1 } => format!("N'{}'", s.replace('\'', "''")),
        _ => format!("'{}'", s.replace('\'', "''")),
    }
}

/// The opening's width (what goes before the text).
fn opening(q: Quote) -> usize {
    match q {
        Quote::Q { open, .. } => open,
        Quote::Plain { prefix } | Quote::Backslash { prefix } => prefix + 1,
        Quote::Dollar { tag } => tag,
    }
}

/// PostgreSQL's format() codes (%I, %L, %s, %1$I) kept from the
/// formatter as words.
fn protect_codes(text: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut codes = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'$') {
                j += 1;
            }
            if j < b.len() && matches!(b[j], b'I' | b'L' | b's') {
                out += &format!("zqf{}zq", codes.len());
                codes.push(text[i..=j].to_string());
                i = j + 1;
                continue;
            }
        }
        let c = text[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    (out, codes)
}

/// Splits `s` at placeholders: Ok(text) / Err(part index).
fn split_holes(s: &str) -> Vec<Result<String, usize>> {
    let low = s.to_ascii_lowercase();
    let b = low.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut run) = (0, 0);
    while i + 4 < b.len() + 1 {
        if low[i..].starts_with("zq") {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 2 && low[j..].starts_with("zq") {
                if run < i {
                    out.push(Ok(s[run..i].to_string()));
                }
                out.push(Err(low[i + 2..j].parse().unwrap()));
                i = j + 2;
                run = i;
                continue;
            }
        }
        i += 1;
    }
    if run < s.len() {
        out.push(Ok(s[run..].to_string()));
    }
    out
}

fn same_tokens(a: &str, b: &str, d: Dialect) -> bool {
    let words = |s: &str| lex::lex(s, d).into_iter().filter(|t| !t.trivia()).map(|t| t.text(s).to_lowercase()).collect::<Vec<_>>();
    words(a) == words(b)
}

/// A whole statement in a region laid out by the formatter; None when it
/// can't be done safely.
fn format_region(src: &str, r: &Region, d: Dialect, o: super::fmt::Options, col: usize) -> Option<String> {
    let first_q = r.parts.iter().find_map(|p| match p {
        Part::Text { quote, content, .. } => Some((*quote, content.clone())),
        _ => None,
    })?;
    for p in &r.parts {
        if let Part::Text { quote, content, .. } = p {
            match quote {
                Quote::Dollar { .. } => return None,
                Quote::Backslash { .. } if src[content.clone()].contains('\\') => return None,
                _ => {}
            }
        }
    }
    let v = virtual_text(src, r);
    if lex::lex(&v.text, d).iter().any(|t| t.open) {
        return None;
    }
    let (prot, codes) = protect_codes(&v.text);
    let inner = super::fmt::Options { embedded: false, ..o };
    let mut f = super::fmt::format(&prot, d, inner).trim().to_string();
    if !prot.trim_end().ends_with(';') {
        f = f.trim_end_matches(';').trim_end().to_string();
    }
    if !same_tokens(&prot, &f, d) {
        return None;
    }
    for (i, c) in codes.iter().enumerate() {
        let ph = format!("zqf{i}zq");
        let at = f.to_ascii_lowercase().find(&ph)?;
        f.replace_range(at..at + ph.len(), c);
    }
    let lines: Vec<&str> = f.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
    if r.parts.len() == 1 {
        // One literal: it stays one, its lines under its first.
        let pad = " ".repeat(col + opening(first_q.0));
        return Some(literal(&lines.join(&format!("\n{pad}")), first_q.0));
    }
    // A chain: a piece per line, || leading, quotes under each other.
    let mut seen = vec![false; r.parts.len()];
    let mut codes_out = Vec::new();
    for (k, line) in lines.iter().enumerate() {
        let line = if k > 0 && !line.starts_with(char::is_whitespace) { format!(" {line}") } else { line.to_string() };
        let mut els = Vec::new();
        for el in split_holes(&line) {
            match el {
                Ok(t) if t.trim().is_empty() => els.push(literal(" ", first_q.0)),
                Ok(t) => els.push(literal(&t, first_q.0)),
                Err(i) => {
                    let Part::Expr(e) = r.parts.get(i)? else { return None };
                    if std::mem::replace(&mut seen[i], true) {
                        return None;
                    }
                    els.push(src[e.clone()].to_string());
                }
            }
        }
        codes_out.push(els.join(" || "));
    }
    // Every expression placed once.
    if r.parts.iter().enumerate().any(|(i, p)| matches!(p, Part::Expr(_)) && !seen[i]) {
        return None;
    }
    let lead = " ".repeat(col.saturating_sub(3));
    Some(codes_out.join(&format!("\n{lead}|| ")))
}

/// Keywords in the case asked for, where the region is only a piece of a
/// statement (nothing else is moved). Same length: done in place.
fn case_keywords(src: &str, r: &Region, d: Dialect, case: Case, out: &mut String, shift: usize) {
    if case == Case::AsIs {
        return;
    }
    let v = virtual_text(src, r);
    for t in lex::lex(&v.text, d) {
        if t.kind != Tk::Keyword {
            continue;
        }
        let w = t.text(&v.text);
        let new = case.apply(w);
        if new.len() != w.len() || new == w {
            continue;
        }
        // Only a word copied whole from one run.
        if let Some(&(vs, ss, _)) = v.map.iter().find(|&&(vs, _, len)| vs <= t.start && t.end <= vs + len) {
            let at = shift + ss + (t.start - vs);
            out.replace_range(at..at + w.len(), &new);
        }
    }
}

/// SQL in the strings of `src` laid out (whole statements) or cased
/// (pieces). `col0`: the column `src` starts at.
pub fn format_strings(src: &str, d: Dialect, o: super::fmt::Options, col0: usize) -> String {
    format_level(src, d, o, col0, 0)
}

fn format_level(src: &str, d: Dialect, o: super::fmt::Options, col0: usize, level: u8) -> String {
    if level >= MAX_LEVEL {
        return src.to_string();
    }
    let toks = lex::lex(src, d);
    let mut out = src.to_string();
    for r in regions(src, &toks, d).into_iter().rev() {
        if r.body {
            let Some(Part::Text { content, .. }) = r.parts.first() else { continue };
            let inner = &src[content.clone()];
            let new = format_level(inner, d, o, column(src, content.start, col0), level + 1);
            if new != inner {
                out.replace_range(content.clone(), &new);
            }
            continue;
        }
        let col = column(src, r.range.start, col0);
        match (r.statement && r.clean && o.split_strings).then(|| format_region(src, &r, d, o, col)).flatten() {
            Some(new) => out.replace_range(r.range.clone(), &new),
            None => {
                let mut piece = src.to_string();
                case_keywords(src, &r, d, o.keywords, &mut piece, 0);
                out.replace_range(r.range.clone(), &piece[r.range.clone()]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(src: &str, d: Dialect) -> Vec<String> {
        regions(src, &lex::lex(src, d), d).iter().map(|r| virtual_text(src, r).text).collect()
    }

    #[test]
    fn finds_dynamic_sql() {
        let src = "begin\n  v_sql := 'select a, b from t where id = ' || p_id || ' and s = ''new''';\n  execute immediate v_sql;\n  raise_application_error(-20001, 'Update failed for ' || p_id);\nend;";
        assert_eq!(found(src, Dialect::Oracle), vec!["select a, b from t where id = zq1zq and s = 'new'"]);
        // A piece added to a variable that holds SQL.
        let src = "v_sql := 'select * from t';\nv_sql := v_sql || ' where x = ' || n;\nexecute immediate v_sql;";
        assert_eq!(found(src, Dialect::Oracle), vec!["select * from t", "zq0zq where x = zq2zq"]);
        // q-strings, and format() in PL/pgSQL inside a $$ body.
        assert_eq!(found("execute immediate q'[delete from t where d < sysdate]';", Dialect::Oracle), vec!["delete from t where d < sysdate"]);
        let src = "do $$ begin execute format('drop table %I', t); end $$;";
        assert_eq!(found(src, Dialect::Postgres), vec![" begin execute format('drop table %I', t); end "]);
        // Plain messages aren't SQL.
        assert!(found("raise notice 'Select a customer';", Dialect::Postgres).is_empty());
        assert!(found("select 'delete' from dual", Dialect::Oracle).is_empty());
    }

    #[test]
    fn colours_inside_strings() {
        let src = "execute immediate 'select x from t where n = ''a''' || v;";
        let spans = classify(src, Dialect::Oracle, true);
        let class_at = |w: &str| spans.iter().find(|s| s.range.start == src.find(w).unwrap()).map(|s| (s.class, s.level));
        assert_eq!(class_at("select"), Some((Class::Keyword, 1)));
        assert_eq!(class_at("from"), Some((Class::Keyword, 1)));
        // The quoted value inside: a string again.
        assert_eq!(class_at("''a''").map(|c| c.0), Some(Class::Str));
        assert_eq!(class_at("v;"), Some((Class::Text, 0)));
        // Spans cover the text in order.
        let mut p = 0;
        for s in &spans {
            assert_eq!(s.range.start, p, "{spans:?}");
            p = s.range.end;
        }
        assert_eq!(p, src.len());
        // A $$ body is SQL; EXECUTE in it a level deeper.
        let src = "create function f() returns void as $$ begin execute 'delete from t where a = 1'; end $$ language plpgsql;";
        let spans = classify(src, Dialect::Postgres, true);
        let at = |w: &str| spans.iter().find(|s| s.range.start == src.find(w).unwrap()).map(|s| (s.class, s.level));
        assert_eq!(at("begin"), Some((Class::Keyword, 0)));
        assert_eq!(at("delete"), Some((Class::Keyword, 1)));
    }

    fn fmt(src: &str, d: Dialect) -> String {
        format_strings(src, d, super::super::fmt::Options::default(), 0)
    }

    #[test]
    fn lays_out_a_chain() {
        let src = "  v_sql := 'select a, b from t join u on u.id = t.u_id where t.id = ' || p_id || ' and t.s = ''new''';";
        let out = fmt(src, Dialect::Oracle);
        assert!(out.contains("SELECT"), "{out}");
        assert!(out.contains("' || p_id\n"), "{out}");
        assert!(out.contains("''new''"), "{out}");
        // The SQL it builds is the same, spaces aside.
        // (Placeholders are numbered by piece: compared as one word.)
        let plain = |s: String| split_holes(&s).into_iter().map(|e| e.unwrap_or_else(|_| " zq ".into())).collect::<String>();
        let v_in = plain(found(src, Dialect::Oracle).join(" "));
        let v_out = plain(found(&out, Dialect::Oracle).join(" "));
        assert!(same_tokens(&v_in, &v_out, Dialect::Oracle), "{out}");
        // Each line's piece starts under the first quote.
        let col = out.find('\'').unwrap();
        for l in out.lines().skip(1) {
            assert_eq!(l.find("|| '").map(|i| i + 3), Some(col), "{out}");
        }
    }

    #[test]
    fn one_literal_stays_one() {
        let src = "execute immediate q'[select a, b from t where x = 'y' and z = 1]';";
        let out = fmt(src, Dialect::Oracle);
        assert!(out.starts_with("execute immediate q'[SELECT"), "{out}");
        assert!(out.ends_with("]';"), "{out}");
        assert!(out.contains("'y'"), "{out}");
        assert!(out.lines().count() > 1, "{out}");
    }

    #[test]
    fn strings_kept_whole_when_asked() {
        let src = "begin\n  execute immediate q'[select a, b from t where x = 'y' and z = 1]';\nend;";
        let o = super::super::fmt::Options { split_strings: false, ..Default::default() };
        let out = format_strings(src, Dialect::Oracle, o, 0);
        assert_eq!(out, "begin\n  execute immediate q'[SELECT a, b FROM t WHERE x = 'y' AND z = 1]';\nend;");
    }

    #[test]
    fn pieces_and_broken_ones_only_cased() {
        let src = "v := 'select * from t';\nv := v || ' where x = 1 and y in (' || list;\nexecute immediate v;";
        let out = fmt(src, Dialect::Oracle);
        assert!(out.contains("' WHERE x = 1 AND y IN (' || list"), "{out}");
        // An unclosed bracket in a whole statement: not laid out, cased.
        let src = "execute immediate 'select a from t where (x = 1';";
        assert_eq!(fmt(src, Dialect::Oracle), "execute immediate 'SELECT a FROM t WHERE (x = 1';");
    }

    #[test]
    fn format_codes_survive() {
        let src = "do $$ begin execute format('select %I from %I.%I where id = %L', c, s, t, v); end $$;";
        let out = fmt(src, Dialect::Postgres);
        assert!(out.contains("%I") && out.contains("%I.%I") && out.contains("%L"), "{out}");
        assert!(out.contains("SELECT"), "{out}");
    }
}
