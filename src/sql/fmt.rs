//! SQL laid out river style: clause words end where SELECT does (the
//! "river"), lists go on below with a leading comma, AND / OR start lines
//! of their own on the river, JOIN keeps its ON, a subquery sits two in
//! from its clause with the same rules inside; WITH starts the line, its
//! queries two in and each next one at ", name AS ("; CREATE TABLE has a
//! column to a line two in, commas leading. Keywords and built-in
//! functions in capitals (then put in the case the settings ask for).
//! Taken from listok's code-block tidier, fed by this crate's lexer.

use super::lex::{self, Tk};
use super::{Case, Dialect};

const RIVER: usize = 6;

const KEYWORDS: &[&str] = &[
    "ADD", "ALL", "ALTER", "AND", "ANY", "AS", "ASC", "BETWEEN", "BY", "CASCADE", "CASE", "CHECK", "COLUMN", "CONFLICT", "CONSTRAINT", "CREATE", "CROSS", "CURRENT_DATE", "CURRENT_TIMESTAMP", "DEFAULT", "DELETE", "DESC",
    "DISTINCT", "DO", "DROP", "ELSE", "END", "EXCEPT", "EXISTS", "FALSE", "FETCH", "FILTER", "FIRST", "FOLLOWING", "FOR", "FOREIGN", "FROM", "FULL", "GROUP", "HAVING", "IF", "ILIKE", "IN", "INDEX", "INNER", "INSERT",
    "INTERSECT", "INTERVAL", "INTO", "IS", "JOIN", "KEY", "LAST", "LATERAL", "LEFT", "LIKE", "LIMIT", "NATURAL", "NOT", "NOTHING", "NULL", "NULLS", "OFFSET", "ON", "OR", "ORDER", "OUTER", "OVER", "PARTITION",
    "PRECEDING", "PRIMARY", "RANGE", "RECURSIVE", "REFERENCES", "RETURNING", "RIGHT", "ROWS", "SELECT", "SET", "TABLE", "THEN", "TO", "TRUE", "UNBOUNDED", "UNION", "UNIQUE", "UPDATE", "USING", "VALUES", "VIEW",
    "WHEN", "WHERE", "WINDOW", "WITH",
];

const FUNCTIONS: &[&str] = &[
    "ABS", "ARRAY_AGG", "AVG", "BOOL_AND", "BOOL_OR", "CAST", "CEIL", "CEILING", "CHAR_LENGTH", "COALESCE", "CONCAT", "CONCAT_WS", "CONVERT", "COUNT", "CUME_DIST", "DATE", "DATE_PART", "DATE_TRUNC", "DATEADD",
    "DATEDIFF", "DENSE_RANK", "EXP", "EXTRACT", "FIRST_VALUE", "FLOOR", "FORMAT", "GENERATE_SERIES", "GREATEST", "GROUP_CONCAT", "IFNULL", "IIF", "INITCAP", "ISNULL", "JSON_AGG", "JSON_BUILD_OBJECT",
    "JSONB_AGG", "JSONB_BUILD_OBJECT", "LAG", "LAST_VALUE", "LEAD", "LEAST", "LEFT", "LENGTH", "LN", "LOG", "LOWER", "LPAD", "LTRIM", "MAX", "MIN", "MOD", "NOW", "NTILE", "NULLIF", "NVL", "PERCENT_RANK",
    "POSITION", "POWER", "RANK", "REGEXP_REPLACE", "REPLACE", "REVERSE", "RIGHT", "ROUND", "ROW_NUMBER", "RPAD", "RTRIM", "SIGN", "SPLIT_PART", "SQRT", "STDDEV", "STRING_AGG", "STRFTIME", "SUBSTR", "SUBSTRING",
    "SUM", "TO_CHAR", "TO_DATE", "TO_TIMESTAMP", "TRIM", "TRUNC", "UPPER", "VARIANCE",
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum K {
    Word,
    Keyword,
    Func,
    Text,
    Open,
    Close,
    Comma,
    Semi,
    Dot,
    Op,
    LineComment,
    BlockComment,
}

#[derive(Clone, Debug)]
struct Tok {
    k: K,
    s: String,
    /// First on its line in the source.
    own: bool,
}

fn tokens(src: &str, d: Dialect) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    let mut own = true;
    for t in lex::lex(src, d) {
        let text = t.text(src);
        let k = match t.kind {
            Tk::Space => {
                if text.contains('\n') {
                    own = true;
                }
                continue;
            }
            Tk::LineComment => K::LineComment,
            Tk::BlockComment => K::BlockComment,
            Tk::Str => K::Text,
            Tk::Open if text == "(" => K::Open,
            Tk::Close if text == ")" => K::Close,
            Tk::Comma => K::Comma,
            Tk::Semi => K::Semi,
            Tk::Dot => K::Dot,
            Tk::Open | Tk::Close | Tk::Op | Tk::Slash | Tk::Other => K::Op,
            _ => K::Word,
        };
        let s = if k == K::LineComment { text.trim_end().to_string() } else { text.to_string() };
        out.push(Tok { k, s, own: std::mem::take(&mut own) });
    }
    // Capitals: keywords; built-in functions where they're called; not
    // after a dot (t.date is a column).
    for j in 0..out.len() {
        if out[j].k != K::Word || (j > 0 && out[j - 1].k == K::Dot) {
            continue;
        }
        let up = out[j].s.to_uppercase();
        let called = out.get(j + 1).is_some_and(|t| t.k == K::Open);
        if called && (FUNCTIONS.contains(&up.as_str()) || super::functions(d).iter().any(|f| f.eq_ignore_ascii_case(&up))) {
            // ClickHouse's functions are case-sensitive: kept as written.
            let s = if d == Dialect::ClickHouse { out[j].s.clone() } else { up };
            out[j] = Tok { k: K::Func, s, ..out[j].clone() };
        } else if KEYWORDS.contains(&up.as_str()) || (!called && super::is_keyword(&up, d)) {
            out[j] = Tok { k: K::Keyword, s: up, ..out[j].clone() };
        }
    }
    // A sign before a number with no operand before it stays on it: -12.
    let mut j = 0;
    while j + 1 < out.len() {
        let unary = out[j].k == K::Op
            && matches!(out[j].s.as_str(), "-" | "+")
            && (j == 0
                || matches!(out[j - 1].k, K::Open | K::Comma | K::Op)
                || out[j - 1].k == K::Keyword
                    && matches!(
                        out[j - 1].s.as_str(),
                        "SELECT" | "THEN" | "ELSE" | "WHEN" | "AND" | "OR" | "NOT" | "RETURN" | "IN" | "BETWEEN" | "BY" | "VALUES" | "IS" | "LIKE" | "WHERE" | "ON" | "SET" | "LIMIT" | "OFFSET" | "DEFAULT" | "INTERVAL" | "HAVING"
                    ))
            && out[j + 1].k == K::Word
            && out[j + 1].s.starts_with(|c: char| c.is_ascii_digit() || c == '.');
        if unary {
            let num = out.remove(j + 1);
            out[j] = Tok { k: K::Word, s: format!("{}{}", out[j].s, num.s), own: out[j].own };
        }
        j += 1;
    }
    out
}

/// Tokens with parentheses gathered into groups.
#[derive(Clone, Debug)]
enum Node {
    T(Tok),
    Group(Vec<Node>),
}

fn tree(toks: &[Tok], i: &mut usize) -> Vec<Node> {
    let mut out = Vec::new();
    while *i < toks.len() {
        let t = &toks[*i];
        *i += 1;
        match t.k {
            K::Open => out.push(Node::Group(tree(toks, i))),
            K::Close => return out,
            _ => out.push(Node::T(t.clone())),
        }
    }
    out
}

fn is_kw(n: Option<&Node>, w: &str) -> bool {
    matches!(n, Some(Node::T(t)) if t.k == K::Keyword && t.s == w)
}

fn is_subquery(g: &[Node]) -> bool {
    let first = g.iter().find(|n| !matches!(n, Node::T(t) if matches!(t.k, K::LineComment | K::BlockComment)));
    is_kw(first, "SELECT") || is_kw(first, "WITH") || matches!(first, Some(Node::Group(inner)) if is_subquery(inner))
}

/// A group written on one line: (a, b), f(x), IN (1, 2).
fn inline(nodes: &[Node]) -> String {
    let mut s = String::new();
    let mut prev: Option<&Node> = None;
    for n in nodes {
        let piece = match n {
            // On one line a -- comment would hide what follows it.
            Node::T(t) if t.k == K::LineComment => format!("/* {} */", t.s.trim_start_matches('-').trim()),
            Node::T(t) => t.s.clone(),
            Node::Group(g) => format!("({})", inline(g)),
        };
        if let Some(p) = prev {
            if space_between(p, n) {
                s.push(' ');
            }
        }
        s += &piece;
        prev = Some(n);
    }
    s
}

fn space_between(prev: &Node, cur: &Node) -> bool {
    let pk = match prev {
        Node::T(t) => Some(t),
        Node::Group(_) => None,
    };
    match cur {
        Node::T(t) if matches!(t.k, K::Comma | K::Semi | K::Dot) => false,
        Node::T(t) if t.k == K::Op && t.s == "::" => false,
        // f(x) but IN (1, 2), AS (…).
        Node::Group(_) => !matches!(pk, Some(t) if matches!(t.k, K::Word | K::Func)),
        _ => !matches!(pk, Some(t) if t.k == K::Dot || (t.k == K::Op && t.s == "::")),
    }
}

/// Lines being written: each with its text.
struct Out {
    lines: Vec<String>,
    cur: String,
    /// The next start() goes on this line (a subquery right after its "(").
    cont: bool,
}

impl Out {
    fn flush(&mut self) {
        let l = self.cur.trim_end().to_string();
        if !l.trim().is_empty() {
            self.lines.push(l);
        }
        self.cur.clear();
    }

    fn start(&mut self, indent: usize) {
        if std::mem::take(&mut self.cont) {
            let n = self.col();
            if n < indent {
                self.cur += &" ".repeat(indent - n);
            }
            return;
        }
        self.flush();
        self.cur = " ".repeat(indent);
    }

    /// The column the next char goes to.
    fn col(&self) -> usize {
        self.cur.chars().count()
    }

    /// Writes a word (a space before it if asked); returns its column.
    fn word(&mut self, w: &str, space: bool) -> usize {
        if space && !self.cur.trim().is_empty() && !self.cur.ends_with(' ') && !self.cur.ends_with('(') {
            self.cur.push(' ');
        }
        let c = self.col();
        self.cur += w;
        c
    }
}

/// Subqueries and CASEs this short stay on one line.
const INLINE_MAX: usize = 50;
/// A WHEN … THEN longer than this has its THEN on a line of its own.
const WHEN_MAX: usize = 60;

/// The clause a keyword starts, with the words it takes along
/// (GROUP BY, LEFT OUTER JOIN, UNION ALL…). None: not a clause.
fn clause_at(nodes: &[Node], i: usize) -> Option<Vec<String>> {
    let Some(Node::T(t)) = nodes.get(i) else { return None };
    if t.k != K::Keyword {
        return None;
    }
    let w = |k: usize| match nodes.get(i + k) {
        Some(Node::T(t)) if t.k == K::Keyword => Some(t.s.as_str()),
        _ => None,
    };
    let s = t.s.as_str();
    let words: Vec<&str> = match s {
        "SELECT" | "FROM" | "WHERE" | "HAVING" | "LIMIT" | "OFFSET" | "VALUES" | "SET" | "RETURNING" | "WITH" | "WINDOW" | "JOIN" | "UPDATE" | "PREWHERE" | "QUALIFY" | "SETTINGS" => vec![s],
        "GROUP" | "ORDER" | "PARTITION" if w(1) == Some("BY") => vec![s, "BY"],
        "UNION" | "INTERSECT" | "EXCEPT" | "MINUS" => {
            if w(1) == Some("ALL") {
                vec![s, "ALL"]
            } else {
                vec![s]
            }
        }
        "INSERT" if w(1) == Some("INTO") => vec![s, "INTO"],
        "DELETE" if w(1) == Some("FROM") => vec![s, "FROM"],
        "LEFT" | "RIGHT" | "FULL" | "INNER" | "CROSS" | "NATURAL" => match (w(1), w(2)) {
            (Some("JOIN"), _) => vec![s, "JOIN"],
            (Some("OUTER"), Some("JOIN")) => vec![s, "OUTER", "JOIN"],
            _ => return None,
        },
        _ => return None,
    };
    Some(words.into_iter().map(String::from).collect())
}

/// Where a clause's word starts: on the river, but WITH at the statement's
/// start (its queries two in from it).
fn clause_indent(base: usize, lead: &str) -> usize {
    if lead == "WITH" { base } else { (base + RIVER).saturating_sub(lead.len()).max(base) }
}

/// Clauses whose commas start lines (a list).
fn is_list(c: &str) -> bool {
    matches!(c, "SELECT" | "GROUP BY" | "ORDER BY" | "SET" | "RETURNING" | "WITH" | "FROM" | "VALUES" | "INSERT INTO" | "PARTITION BY")
}

/// Clauses whose AND / OR start lines.
fn breaks_logic(c: &str) -> bool {
    matches!(c, "WHERE" | "HAVING" | "PREWHERE" | "QUALIFY") || c.ends_with("JOIN")
}

/// A CASE … END from nodes[i] (nested ones inside): the index of its END.
fn case_end(nodes: &[Node], i: usize) -> usize {
    let mut depth = 0;
    for (k, n) in nodes.iter().enumerate().skip(i) {
        if is_kw(Some(n), "CASE") {
            depth += 1;
        } else if is_kw(Some(n), "END") {
            depth -= 1;
            if depth == 0 {
                return k;
            }
        }
    }
    nodes.len() - 1
}

/// A long CASE: WHENs two in from CASE (THEN below its WHEN if long),
/// ELSE with them, END under CASE.
fn case_block(nodes: &[Node], at: usize, out: &mut Out) {
    let inner = &nodes[1..nodes.len().saturating_sub(1)];
    // Split at this CASE's own WHEN / THEN / ELSE.
    let mut parts: Vec<(String, Vec<Node>)> = vec![(String::new(), Vec::new())];
    let mut depth = 0;
    for n in inner {
        if is_kw(Some(n), "CASE") {
            depth += 1;
        } else if is_kw(Some(n), "END") {
            depth -= 1;
        }
        match n {
            Node::T(t) if depth == 0 && t.k == K::Keyword && matches!(t.s.as_str(), "WHEN" | "THEN" | "ELSE") => parts.push((t.s.clone(), Vec::new())),
            _ => parts.last_mut().unwrap().1.push(n.clone()),
        }
    }
    // CASE x WHEN …: the operand stays by CASE.
    if !parts[0].1.is_empty() {
        out.word(&inline(&parts[0].1), true);
    }
    let mut k = 1;
    while k < parts.len() {
        let (kw, body) = &parts[k];
        if kw == "WHEN" && parts.get(k + 1).is_some_and(|(t, _)| t == "THEN") {
            let when = format!("WHEN {}", inline(body));
            let then = format!("THEN {}", inline(&parts[k + 1].1));
            out.start(at + 2);
            if when.chars().count() + 1 + then.chars().count() <= WHEN_MAX {
                out.cur += &format!("{when} {then}");
            } else {
                out.cur += &when;
                out.start(at + 2);
                out.cur += &then;
            }
            k += 2;
            continue;
        }
        out.start(at + 2);
        out.cur += &format!("{kw} {}", inline(body));
        k += 1;
    }
    out.start(at);
    out.cur += "END";
}

fn has_subquery(g: &[Node]) -> bool {
    g.iter().any(|n| matches!(n, Node::Group(x) if is_subquery(x) || has_subquery(x)))
}

fn has_line_comment(g: &[Node]) -> bool {
    g.iter().any(|n| match n {
        Node::T(t) => t.k == K::LineComment,
        Node::Group(x) => has_line_comment(x),
    })
}

/// A statement from column `base`; `anchor`: where what opened it starts
/// (its bracket's FROM / JOIN / operator, or the bracket), for comments.
fn statement(nodes: &[Node], base: usize, anchor: usize, out: &mut Out) {
    let mut clause = String::new();
    // Where a clause's word and its items start (commas go two before items).
    let mut clause_left = base;
    let mut item = base;
    // Just after the clause's words; the column of the last thing written.
    let mut fresh = false;
    let mut last_col = base;
    let mut between = false;
    let mut i = 0;
    let mut first = true;
    // CREATE TABLE t (…): its columns one to a line, two in from CREATE.
    let mut columns = is_kw(nodes.first(), "CREATE") && nodes.iter().take(8).any(|n| is_kw(Some(n), "TABLE"));
    while i < nodes.len() {
        if let Some(words) = clause_at(nodes, i) {
            let lead = &words[0];
            let indent = clause_indent(base, lead);
            out.start(indent);
            let phrase = words.join(" ");
            out.cur += &phrase;
            clause_left = indent;
            item = indent + phrase.len() + 1;
            let set_ops = matches!(lead.as_str(), "UNION" | "INTERSECT" | "EXCEPT" | "MINUS");
            clause = phrase;
            i += words.len();
            first = false;
            fresh = true;
            between = false;
            // UNION stands on a line of its own.
            if set_ops {
                out.flush();
            }
            continue;
        }
        let n = &nodes[i];
        if first && !matches!(n, Node::T(t) if matches!(t.k, K::LineComment | K::BlockComment)) {
            out.start(base);
            first = false;
        }
        match n {
            Node::T(t) if t.k == K::Comma && is_list(&clause) => {
                // WITH's next query: its comma at WITH's start.
                out.start(if clause == "WITH" { clause_left } else { item.saturating_sub(2) });
                out.cur += ", ";
            }
            Node::Group(g) if columns && clause.is_empty() && !is_subquery(g) => {
                columns = false;
                out.word("(", true);
                let mut part: Vec<Node> = Vec::new();
                let mut k = 0;
                let column = |part: &mut Vec<Node>, out: &mut Out, k: &mut usize| {
                    if !part.is_empty() {
                        out.start(base);
                        out.cur += if *k == 0 { "  " } else { ", " };
                        out.cur += &inline(part);
                        part.clear();
                        *k += 1;
                    }
                };
                for n in g {
                    match n {
                        Node::T(t) if t.k == K::Comma => column(&mut part, out, &mut k),
                        // On a line of its own: there, two in.
                        Node::T(t) if matches!(t.k, K::LineComment | K::BlockComment) && t.own => {
                            column(&mut part, out, &mut k);
                            out.flush();
                            out.lines.push(format!("{}{}", " ".repeat(base + 2), t.s));
                        }
                        // After a column (or its comma): at its line's end.
                        Node::T(t) if t.k == K::LineComment => {
                            column(&mut part, out, &mut k);
                            out.word(&t.s, true);
                        }
                        _ => part.push(n.clone()),
                    }
                }
                column(&mut part, out, &mut k);
                out.start(base);
                out.cur.push(')');
            }
            Node::T(t) if t.k == K::Keyword && (t.s == "AND" || t.s == "OR") && breaks_logic(&clause) && !(between && t.s == "AND") => {
                out.start((base + RIVER).saturating_sub(t.s.len()));
                out.cur += &t.s;
                out.cur.push(' ');
            }
            // A comment on a line of its own: with what it's in — the clause
            // it's before, or the one it's inside, or at a bracket's start
            // with what opened the bracket.
            Node::T(t) if matches!(t.k, K::LineComment | K::BlockComment) && (t.own || out.cur.trim().is_empty()) => {
                // Right after a bare "(": the bracket ends its line then.
                let after_open = std::mem::take(&mut out.cont);
                let next = (i + 1..nodes.len()).find(|&j| !matches!(&nodes[j], Node::T(t) if matches!(t.k, K::LineComment | K::BlockComment)));
                let col = match next.and_then(|j| clause_at(nodes, j)) {
                    _ if clause.is_empty() => anchor,
                    Some(words) => clause_indent(base, &words[0]),
                    None => clause_left,
                };
                let line = format!("{}{}", " ".repeat(col), t.s);
                let pending = out.cur.trim();
                if (pending.is_empty() || pending == ",") && !after_open {
                    // Above the line being started (", " stays for what follows).
                    out.lines.push(line);
                } else {
                    out.flush();
                    out.lines.push(line);
                    out.cur = " ".repeat(item);
                }
            }
            Node::T(t) if t.k == K::LineComment => {
                if out.cur.trim() == "," && !out.lines.is_empty() {
                    // "a, -- note": the note stays with a, the comma goes on.
                    let last = out.lines.last_mut().unwrap();
                    *last += " ";
                    *last += &t.s;
                } else {
                    out.word(&t.s, true);
                    let cont = std::mem::take(&mut out.cont);
                    out.flush();
                    out.cur = " ".repeat(if cont { 0 } else { item });
                }
            }
            Node::T(t) if t.k == K::Keyword && t.s == "CASE" => {
                let j = case_end(nodes, i);
                let whole = inline(&nodes[i..=j]);
                let space = i > 0 && !out.cur.ends_with(", ") && !out.cur.ends_with('(');
                if whole.chars().count() <= INLINE_MAX && !has_line_comment(&nodes[i..=j]) {
                    last_col = out.word(&whole, space);
                } else {
                    let at = out.word("CASE", space);
                    last_col = at;
                    case_block(&nodes[i..=j], at, out);
                }
                i = j + 1;
                fresh = false;
                continue;
            }
            Node::Group(g) if is_subquery(g) && (has_subquery(g) || has_line_comment(g) || inline(g).chars().count() + 2 > INLINE_MAX || ((clause == "WITH" || (clause.is_empty() && is_kw(nodes.first(), "CREATE"))) && i > 0 && is_kw(Some(&nodes[i - 1]), "AS"))) => {
                let prev = if i > 0 { Some(&nodes[i - 1]) } else { None };
                // CREATE … AS (query): two in from CREATE.
                let create_as = clause.is_empty() && is_kw(nodes.first(), "CREATE") && is_kw(prev, "AS");
                let opened_by_clause = fresh && (clause == "FROM" || clause.ends_with("JOIN") || clause == "WITH");
                let as_in_with = clause == "WITH" && is_kw(prev, "AS");
                let by_operator = matches!(prev, Some(Node::T(t)) if t.k == K::Op || (t.k == K::Keyword && matches!(t.s.as_str(), "IN" | "EXISTS" | "ANY" | "ALL" | "SOME" | "AS" | "NOT")));
                if create_as {
                    out.word("(", true);
                    out.flush();
                    statement(g, base + 2, base, out);
                    out.start(base);
                    out.cur.push(')');
                } else if opened_by_clause || as_in_with || by_operator {
                    // ( ends the line; the query two in from FROM / JOIN / the
                    // operator; ) back there.
                    let at = if opened_by_clause || as_in_with { clause_left } else { last_col };
                    out.word("(", true);
                    out.flush();
                    statement(g, at + 2, at, out);
                    out.start(at);
                    out.cur.push(')');
                } else {
                    // Just a bracket: the query goes on right after it.
                    let open = out.word("(", true);
                    out.cont = true;
                    statement(g, open + 1, open, out);
                    out.start(open);
                    out.cur.push(')');
                }
                fresh = false;
            }
            _ => {
                let piece = match n {
                    Node::T(t) => t.s.clone(),
                    Node::Group(g) => format!("({})", inline(g)),
                };
                // INSERT INTO t (a, b): the column list apart from the table.
                let cols = clause == "INSERT INTO" && matches!(n, Node::Group(_));
                let space = i > 0 && !out.cur.ends_with(", ") && !out.cur.ends_with('(') && (cols || space_between(&nodes[i - 1], n));
                last_col = out.word(&piece, space);
                fresh = false;
                if let Node::T(t) = n {
                    if t.k == K::Keyword && t.s == "BETWEEN" {
                        between = true;
                    } else if t.k == K::Keyword && t.s == "AND" {
                        between = false;
                    }
                }
            }
        }
        i += 1;
    }
}

/// Formats SQL (several statements: each on its own, a blank line between).
fn format_one(src: &str, d: Dialect) -> String {
    let toks = tokens(src, d);
    let mut i = 0;
    let nodes = tree(&toks, &mut i);
    let mut parts: Vec<Vec<Node>> = vec![Vec::new()];
    for n in nodes {
        if matches!(&n, Node::T(t) if t.k == K::Semi) {
            parts.push(Vec::new());
        } else {
            parts.last_mut().unwrap().push(n);
        }
    }
    let stmts: Vec<&Vec<Node>> = parts.iter().filter(|p| !p.is_empty()).collect();
    let ended = toks.last().is_some_and(|t| t.k == K::Semi);
    let mut blocks = Vec::new();
    for (k, p) in stmts.iter().enumerate() {
        let mut out = Out { lines: Vec::new(), cur: String::new(), cont: false };
        statement(p, 0, 0, &mut out);
        out.flush();
        let mut text = out.lines.join("\n");
        if k + 1 < stmts.len() || ended {
            // Before a comment that ends the statement, not inside it.
            match p.last() {
                Some(Node::T(t)) if t.k == K::LineComment && text.ends_with(&t.s) => {
                    let at = text.len() - t.s.len();
                    let head = text[..at].trim_end().to_string();
                    text = format!("{head}; {}", t.s);
                }
                _ => text.push(';'),
            }
        }
        blocks.push(text);
    }
    blocks.join("\n\n")
}

/// Layout options.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Options {
    pub keywords: Case,
    pub functions: Case,
    /// SQL in strings (dynamic SQL, $$ bodies) laid out too.
    pub embedded: bool,
    /// SQL in a string may be laid out over several lines (else only its
    /// keywords' case changes, the string stays as it was).
    pub split_strings: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { keywords: Case::Upper, functions: Case::Upper, embedded: true, split_strings: true }
    }
}

/// Lays out a script statement by statement; PL/SQL units and bodies in
/// $$ are left as written. Comments between statements stay.
pub fn format(src: &str, d: Dialect, o: Options) -> String {
    let ranges = super::split(src, d, false);
    let mut s = String::new();
    let mut prev = 0;
    // Comments just above a statement stay right above it.
    let gap = |text: &str| text.lines().map(str::trim_end).filter(|l| !matches!(l.trim(), "" | "/" | ";")).collect::<Vec<_>>().join("\n");
    for r in &ranges {
        let g = gap(&src[prev..r.start]);
        if !s.is_empty() {
            s += "\n\n";
        }
        if !g.is_empty() {
            s += &g;
            s.push('\n');
        }
        let st = &src[r.clone()];
        let toks = lex::lex(st, d);
        let verbatim = super::is_plsql_block(st, d) || toks.iter().any(|t| t.kind == Tk::Str && t.text(st).starts_with('$')) || toks.iter().any(|t| t.open);
        s += &if verbatim { st.trim().to_string() } else { cased(st, &format_one(st, d), d, o) };
        if d == Dialect::Oracle && (src[r.end..].trim_start().starts_with('/') || super::is_plsql_block(st, d)) {
            s += "\n/";
        }
        prev = r.end;
    }
    let tail = gap(&src[prev..]);
    if !tail.is_empty() {
        if !s.is_empty() {
            s += "\n\n";
        }
        s += &tail;
    }
    if src.ends_with('\n') {
        s.push('\n');
    }
    if o.embedded { super::embed::format_strings(&s, d, o, 0) } else { s }
}

/// Keywords and functions in the case asked for ("as is": as in `orig`).
fn cased(orig: &str, formatted: &str, d: Dialect, o: Options) -> String {
    if o.keywords == Case::Upper && o.functions == Case::Upper {
        return formatted.to_string();
    }
    let a: Vec<lex::Tok> = lex::lex(orig, d).into_iter().filter(|t| !t.trivia()).collect();
    let b = lex::lex(formatted, d);
    let same = a.len() == b.iter().filter(|t| !t.trivia()).count();
    let mut out = String::with_capacity(formatted.len());
    let mut k = 0;
    for (i, t) in b.iter().enumerate() {
        let text = t.text(formatted);
        if t.trivia() {
            out += text;
            continue;
        }
        let was = a.get(k).filter(|_| same).map(|x| x.text(orig)).filter(|w| w.eq_ignore_ascii_case(text));
        k += 1;
        if !matches!(t.kind, Tk::Keyword | Tk::Word) || text.to_uppercase() != text {
            out += text;
            continue;
        }
        let up = text.to_uppercase();
        let called = b[i + 1..].iter().find(|x| x.kind != Tk::Space).is_some_and(|x| x.kind == Tk::Open);
        let case = if called && (FUNCTIONS.contains(&up.as_str()) || super::functions(d).iter().any(|f| f.eq_ignore_ascii_case(&up))) {
            o.functions
        } else if KEYWORDS.contains(&up.as_str()) || super::is_keyword(&up, d) {
            o.keywords
        } else {
            out += text;
            continue;
        };
        match case {
            Case::Upper => out += text,
            Case::Lower => out += &text.to_lowercase(),
            Case::AsIs => out += was.unwrap_or(text),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt_pg(s: &str) -> String {
        format_one(s, Dialect::Postgres)
    }

    #[test]
    fn scripts() {
        let o = Options::default();
        let src = "-- head\nselect a,b from t;\nbegin\n  null;\nend;\n/\n";
        assert_eq!(format(src, Dialect::Oracle, o), "-- head\nSELECT a\n     , b\n  FROM t;\n\nbegin\n  null;\nend;\n/\n");
        let src = "do $$ begin perform 1; end $$;";
        assert_eq!(format(src, Dialect::Postgres, o), src);
        let lower = Options { keywords: Case::Lower, functions: Case::Lower, ..o };
        assert_eq!(format("SELECT Count(x) FROM t", Dialect::Postgres, lower), "select count(x)\n  from t");
        let keep = Options { keywords: Case::AsIs, functions: Case::AsIs, ..o };
        assert_eq!(format("Select Count(x) from t", Dialect::Postgres, keep), "Select Count(x)\n  from t");
        assert_eq!(format("select countIf(x > 1) from t", Dialect::ClickHouse, o), "SELECT countIf(x > 1)\n  FROM t");
    }

    #[test]
    fn signs_and_binds_stay_put() {
        let o = Options::default();
        let f = format("select add_months(d, -12), x - 1, case when a then -1 else +2 end from t where a = :1 and b = -0.5", Dialect::Oracle, o);
        assert!(f.contains("ADD_MONTHS(d, -12)") && f.contains("x - 1") && f.contains("THEN -1 ELSE +2") && f.contains(":1") && f.contains("= -0.5"), "{f}");
    }

    #[test]
    fn river() {
        let q = "select a.id, a.name, count(b.id) as cnt from users a join orders b on b.user_id = a.id left join items i on i.order_id = b.id and i.ok = true where a.active = 1 and b.total > 100 or b.vip = 1 group by a.id, a.name having count(b.id) > 2 order by cnt desc limit 10";
        assert_eq!(
            fmt_pg(q),
            "SELECT a.id
     , a.name
     , COUNT(b.id) AS cnt
  FROM users a
  JOIN orders b ON b.user_id = a.id
  LEFT JOIN items i ON i.order_id = b.id
   AND i.ok = TRUE
 WHERE a.active = 1
   AND b.total > 100
    OR b.vip = 1
 GROUP BY a.id
        , a.name
HAVING COUNT(b.id) > 2
 ORDER BY cnt DESC
 LIMIT 10"
        );
    }

    #[test]
    fn subqueries_two_in() {
        // Opened by an operator: two in from it, ) back under it.
        let q = "select u.id from users u where u.id in (select o.user_id from orders o where o.total > 100 and o.paid) and u.ok";
        assert_eq!(
            fmt_pg(q),
            "SELECT u.id
  FROM users u
 WHERE u.id IN (
              SELECT o.user_id
                FROM orders o
               WHERE o.total > 100
                 AND o.paid
            )
   AND u.ok"
        );
        // Opened by FROM / JOIN: two in from the clause word.
        let q = "select s.n from (select count(*) n, max(x) m, min(y) k from t where t.a = 1) s join (select id, sum(v) v from w group by id having sum(v) > 0) z on z.id = s.n";
        assert_eq!(
            fmt_pg(q),
            "SELECT s.n
  FROM (
    SELECT COUNT(*) n
         , MAX(x) m
         , MIN(y) k
      FROM t
     WHERE t.a = 1
  ) s
  JOIN (
    SELECT id
         , SUM(v) v
      FROM w
     GROUP BY id
    HAVING SUM(v) > 0
  ) z ON z.id = s.n"
        );
        // Just a bracket (a list item): the query goes on right after it.
        let q = "select u.id, (select max(o.total) from orders o where o.user_id = u.id and o.paid) as mx from users u";
        assert_eq!(
            fmt_pg(q),
            "SELECT u.id
     , (SELECT MAX(o.total)
          FROM orders o
         WHERE o.user_id = u.id
           AND o.paid
       ) AS mx
  FROM users u"
        );
        // Short ones stay on one line.
        assert_eq!(fmt_pg("select a from t where id in (select id from top)"), "SELECT a\n  FROM t\n WHERE id IN (SELECT id FROM top)");
    }

    #[test]
    fn cases() {
        // Short: one line. Long: WHENs two in, ELSE with them, END under CASE.
        let q = "select case when a > 1 then 'big' when a > 0 then 'small' else 'zero' end as size, case when x then 1 end y from t";
        assert_eq!(
            fmt_pg(q),
            "SELECT CASE
         WHEN a > 1 THEN 'big'
         WHEN a > 0 THEN 'small'
         ELSE 'zero'
       END AS size
     , CASE WHEN x THEN 1 END y
  FROM t"
        );
        // A long WHEN: its THEN below it.
        let q = "select case when customer_status = 'active' and last_order_date > current_date - 30 then 'recent buyer' else 'other' end k from c";
        assert_eq!(
            fmt_pg(q),
            "SELECT CASE
         WHEN customer_status = 'active' AND last_order_date > CURRENT_DATE - 30
         THEN 'recent buyer'
         ELSE 'other'
       END k
  FROM c"
        );
    }

    #[test]
    fn keeps_what_isnt_ours() {
        // Strings, quoted names, columns named like functions, BETWEEN's AND.
        let q = "select 'it''s', \"Order\", t.date, left(x, 2) from t where d between 1 and 5 and x like 'a%' -- note\n;update t set a = 1, b = 'x' where id = 3;";
        assert_eq!(
            fmt_pg(q),
            "SELECT 'it''s'
     , \"Order\"
     , t.date
     , LEFT(x, 2)
  FROM t
 WHERE d BETWEEN 1 AND 5
   AND x LIKE 'a%'; -- note

UPDATE t
   SET a = 1
     , b = 'x'
 WHERE id = 3;"
        );
    }

    #[test]
    fn comments_with_their_construct() {
        let q = "select a, -- first\n  -- about b\n  b\nfrom (\n  -- inner\n  select x from t -- tail\n  where y = 1\n  -- second condition\n  and z = 2) s\n-- the join\njoin u on u.id = s.x";
        assert_eq!(
            fmt_pg(q),
            "SELECT a -- first
-- about b
     , b
  FROM (
  -- inner
    SELECT x
      FROM t -- tail
     WHERE y = 1
     -- second condition
       AND z = 2
  ) s
  -- the join
  JOIN u ON u.id = s.x"
        );
        // Tidying it again changes nothing.
        assert_eq!(fmt_pg(&fmt_pg(q)), fmt_pg(q));
        // In a bare bracket: from the bracket; a one-line group keeps its code.
        let q = "select (\n -- top one\n select max(v) from w) m, f(1, -- one\n 2) from t";
        assert_eq!(fmt_pg(&fmt_pg(q)), fmt_pg(q));
        assert_eq!(fmt_pg(q), "SELECT (\n       -- top one\n        SELECT MAX(v)\n          FROM w\n       ) m\n     , f(1, /* one */ 2)\n  FROM t");
    }

    #[test]
    fn ctes_and_inserts() {
        let q = "with paid as (select user_id, sum(total) s from orders group by user_id), top as (select user_id from paid) select u.name from users u join top t on t.user_id = u.id;";
        assert_eq!(
            fmt_pg(q),
            "WITH paid AS (
  SELECT user_id
       , SUM(total) s
    FROM orders
   GROUP BY user_id
)
, top AS (
  SELECT user_id
    FROM paid
)
SELECT u.name
  FROM users u
  JOIN top t ON t.user_id = u.id;"
        );
        assert_eq!(fmt_pg("insert into logs (id, msg) values (1, 'a'), (2, now());"), "INSERT INTO logs (id, msg)\nVALUES (1, 'a')\n     , (2, NOW());");
        // CREATE TABLE: a column to a line, two in from CREATE.
        assert_eq!(
            fmt_pg("create table users (id int primary key, name varchar(100) not null, created timestamp default now(), primary key (id, name));"),
            "CREATE TABLE users (\n  id int PRIMARY KEY\n, name varchar(100) NOT NULL\n, created timestamp DEFAULT NOW()\n, PRIMARY KEY (id, name)\n);"
        );
        // Tidying it again changes nothing.
        assert_eq!(fmt_pg(&fmt_pg(q)), fmt_pg(q));
        // CREATE TABLE … AS: the query laid out as ever (in brackets two in).
        assert_eq!(fmt_pg("create table t as select a, b from x where y = 1"), "CREATE TABLE t AS\nSELECT a\n     , b\n  FROM x\n WHERE y = 1");
        assert_eq!(fmt_pg("create table t as (select a, b from x where y = 1)"), "CREATE TABLE t AS (\n  SELECT a\n       , b\n    FROM x\n   WHERE y = 1\n)");
        assert_eq!(fmt_pg("create table t (a, b) as (select 1, 2)"), "CREATE TABLE t (\n  a\n, b\n) AS (\n  SELECT 1\n       , 2\n)");
        // Comments by the columns stay where they were.
        let q = "create table if not exists t (\n  id int, -- the key\n  -- values\n  v text, w int -- last\n)";
        assert_eq!(fmt_pg(q), "CREATE TABLE IF NOT EXISTS t (\n  id int -- the key\n  -- values\n, v text\n, w int -- last\n)");
        assert_eq!(fmt_pg(&fmt_pg(q)), fmt_pg(q));
        // UNION on its own line between the two.
        assert_eq!(fmt_pg("select a from x union all select a from y"), "SELECT a\n  FROM x\n UNION ALL\nSELECT a\n  FROM y");
    }
}
