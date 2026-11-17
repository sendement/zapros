//! Notebooks: a script as cells (SQL to run, or Markdown), saved as SQL
//! with `-- %%` marks (`-- %% [markdown]` cells as comments). A cell may
//! be only a WITH (its CTEs); a later cell using those names gets them put
//! in front of its statement when it runs, merged into its own WITH.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use super::Dialect;
use super::lex::{self, Tk};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sql,
    Markdown,
}

/// A cell as parsed from text.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub kind: Kind,
    pub text: String,
}

pub const MARK: &str = "-- %%";
const MD_MARK: &str = "-- %% [markdown]";

/// Whether the text is a notebook saved as SQL.
pub fn is_notebook(text: &str) -> bool {
    text.lines().any(|l| l.trim_end() == MARK || l.trim_end() == MD_MARK)
}

/// The text's cells: by its marks, or (plain SQL) a cell per statement,
/// the comments above one going with it.
pub fn parse(text: &str, d: Dialect) -> Vec<Piece> {
    if !is_notebook(text) {
        let ranges = super::split(text, d, true);
        let mut out = Vec::new();
        let mut prev = 0;
        for r in ranges {
            let piece = text[prev..r.end].trim_matches('\n').trim_end();
            prev = r.end;
            if !piece.trim().is_empty() {
                out.push(Piece { kind: Kind::Sql, text: piece.trim_start_matches(['\n', '\r']).to_string() });
            }
        }
        let tail = text[prev..].trim();
        if !tail.is_empty() {
            out.push(Piece { kind: Kind::Sql, text: tail.to_string() });
        }
        if out.is_empty() {
            out.push(Piece { kind: Kind::Sql, text: String::new() });
        }
        return out;
    }
    let mut out: Vec<Piece> = Vec::new();
    let mut cur: Option<(Kind, Vec<&str>)> = None;
    let flush = |cur: &mut Option<(Kind, Vec<&str>)>, out: &mut Vec<Piece>| {
        if let Some((kind, lines)) = cur.take() {
            let text = match kind {
                Kind::Sql => lines.join("\n"),
                Kind::Markdown => lines.iter().map(|l| l.strip_prefix("-- ").or_else(|| l.strip_prefix("--")).unwrap_or(l)).collect::<Vec<_>>().join("\n"),
            };
            out.push(Piece { kind, text: text.trim_matches('\n').to_string() });
        }
    };
    for line in text.lines() {
        let l = line.trim_end();
        if l == MD_MARK {
            flush(&mut cur, &mut out);
            cur = Some((Kind::Markdown, Vec::new()));
        } else if l == MARK {
            flush(&mut cur, &mut out);
            cur = Some((Kind::Sql, Vec::new()));
        } else {
            match &mut cur {
                Some((_, lines)) => lines.push(line),
                // Text before the first mark: a cell of its own.
                None if !l.is_empty() => cur = Some((Kind::Sql, vec![line])),
                None => {}
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Cells as SQL with marks (read back by `parse`).
pub fn render(cells: &[Piece]) -> String {
    let mut s = String::new();
    for c in cells {
        if !s.is_empty() {
            s.push('\n');
        }
        match c.kind {
            Kind::Sql => {
                s += MARK;
                s.push('\n');
                s += c.text.trim_end();
            }
            Kind::Markdown => {
                s += MD_MARK;
                for l in c.text.trim_end().lines() {
                    s.push('\n');
                    s += if l.is_empty() { "--".to_string() } else { format!("-- {l}") }.as_str();
                }
            }
        }
        s.push('\n');
    }
    s
}

/// One CTE: its name (as compared: lower case, unquoted) and its text
/// ("name AS (…)").
#[derive(Clone, Debug, PartialEq)]
pub struct Cte {
    pub name: String,
    pub text: String,
}

/// A statement's WITH, and what follows it.
#[derive(Clone, Debug, PartialEq)]
pub struct With {
    pub recursive: bool,
    pub ctes: Vec<Cte>,
    /// The statement after the WITH (empty: a WITH only).
    pub main: Range<usize>,
}

fn name_of(src: &str, t: &lex::Tok) -> String {
    let s = t.text(src);
    if t.kind == Tk::Quoted { super::unquote(s).to_string() } else { s.to_lowercase() }
}

/// The WITH a statement starts with (None: it doesn't).
pub fn parse_with(sql: &str, d: Dialect) -> Option<With> {
    let toks: Vec<lex::Tok> = lex::lex(sql, d).into_iter().filter(|t| !t.trivia()).collect();
    let word = |k: usize, w: &str| toks.get(k).is_some_and(|t| matches!(t.kind, Tk::Word | Tk::Keyword) && t.text(sql).eq_ignore_ascii_case(w));
    if !word(0, "with") {
        return None;
    }
    let mut k = 1;
    let recursive = word(k, "recursive");
    if recursive {
        k += 1;
    }
    // The bracket closing the one opened at k.
    let close = |k: usize| -> Option<usize> {
        let mut depth = 0;
        for (j, t) in toks.iter().enumerate().skip(k) {
            match t.kind {
                Tk::Open if t.text(sql) == "(" => depth += 1,
                Tk::Close if t.text(sql) == ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
        }
        None
    };
    let mut ctes = Vec::new();
    loop {
        let name_tok = toks.get(k).filter(|t| matches!(t.kind, Tk::Word | Tk::Keyword | Tk::Quoted))?;
        let mut j = k + 1;
        if toks.get(j).is_some_and(|t| t.kind == Tk::Open) {
            j = close(j)? + 1;
        }
        if !word(j, "as") {
            return None;
        }
        j += 1;
        if word(j, "not") {
            j += 1;
        }
        if word(j, "materialized") {
            j += 1;
        }
        if !toks.get(j).is_some_and(|t| t.kind == Tk::Open) {
            return None;
        }
        let end = close(j)?;
        ctes.push(Cte { name: name_of(sql, name_tok), text: sql[name_tok.start..toks[end].end].to_string() });
        k = end + 1;
        if toks.get(k).is_some_and(|t| t.kind == Tk::Comma) {
            k += 1;
            continue;
        }
        break;
    }
    let rest: Vec<&lex::Tok> = toks[k..].iter().filter(|t| t.kind != Tk::Semi).collect();
    let main = match (rest.first(), rest.last()) {
        (Some(a), Some(b)) => a.start..b.end,
        _ => sql.len()..sql.len(),
    };
    Some(With { recursive, ctes, main })
}

/// The names a text uses (lower case, unquoted).
fn words(src: &str, d: Dialect) -> HashSet<String> {
    lex::lex(src, d).iter().filter(|t| matches!(t.kind, Tk::Word | Tk::Keyword | Tk::Quoted)).map(|t| name_of(src, t)).collect()
}

/// CTEs known from cells above (a WITH-only statement's), the later of
/// the same name winning.
#[derive(Clone, Debug, Default)]
pub struct Known {
    pub ctes: Vec<Cte>,
    pub recursive: bool,
}

impl Known {
    /// Takes in a statement's CTEs when it's only a WITH.
    pub fn learn(&mut self, sql: &str, d: Dialect) {
        if let Some(w) = parse_with(sql, d).filter(|w| w.main.is_empty()) {
            for c in w.ctes {
                self.ctes.retain(|x| x.name != c.name);
                self.ctes.push(c);
            }
            self.recursive |= w.recursive;
        }
    }
}

impl Known {
    /// Its CTEs as relations, with the columns they give (for hints).
    pub fn relations(&self, d: Dialect, rels: &[crate::db::Relation]) -> Vec<crate::db::Relation> {
        if self.ctes.is_empty() {
            return Vec::new();
        }
        let list: Vec<&str> = self.ctes.iter().map(|c| c.text.as_str()).collect();
        let sql = format!("WITH {}{} SELECT 1", if self.recursive { "RECURSIVE " } else { "" }, list.join(", "));
        super::complete::derived(&sql, d, &[rels]).into_iter().filter(|(_, cte)| *cte).map(|(r, _)| r).collect()
    }
}

/// A statement made ready to run: the CTEs from above it uses (and the
/// ones those use) put into its WITH; a WITH alone shows its last CTE.
pub fn glue(sql: &str, known: &Known, d: Dialect) -> String {
    // PL/SQL blocks go as they are (their END; included).
    if super::is_plsql_block(sql.trim(), d) {
        return sql.to_string();
    }
    let sql_t = sql.trim().trim_end_matches(';').trim_end();
    let own = parse_with(sql_t, d);
    let (own_ctes, main, recursive) = match &own {
        Some(w) => (w.ctes.clone(), sql_t[w.main.clone()].to_string(), w.recursive),
        None => (Vec::new(), sql_t.to_string(), false),
    };
    let own_names: HashSet<&str> = own_ctes.iter().map(|c| c.name.as_str()).collect();
    let by_name: HashMap<&str, &Cte> = known.ctes.iter().map(|c| (c.name.as_str(), c)).collect();
    // What it uses, then what those use.
    let mut want: HashSet<String> = HashSet::new();
    let mut todo: Vec<String> = words(&main, d).into_iter().chain(own_ctes.iter().flat_map(|c| words(&c.text, d))).filter(|w| by_name.contains_key(w.as_str()) && !own_names.contains(w.as_str())).collect();
    while let Some(w) = todo.pop() {
        if want.insert(w.clone()) {
            let c = by_name[w.as_str()];
            todo.extend(words(&c.text, d).into_iter().filter(|x| by_name.contains_key(x.as_str()) && !own_names.contains(x.as_str()) && *x != c.name));
        }
    }
    let main = if main.trim().is_empty() {
        // A WITH alone: what its last CTE gives.
        match own_ctes.last() {
            Some(c) => format!("SELECT * FROM {}", &c.text[..c.text.find(|ch: char| ch.is_whitespace() || ch == '(').unwrap_or(c.text.len())]),
            None => return sql_t.to_string(),
        }
    } else {
        main
    };
    if want.is_empty() && own.is_none() {
        return sql.to_string();
    }
    let mut defs: Vec<&str> = known.ctes.iter().filter(|c| want.contains(&c.name)).map(|c| c.text.as_str()).collect();
    defs.extend(own_ctes.iter().map(|c| c.text.as_str()));
    let rec = if recursive || (known.recursive && !want.is_empty()) { "RECURSIVE " } else { "" };
    format!("WITH {rec}{}\n{main}", defs.join(",\n"))
}

/// The notebook as plain SQL: Markdown as comments; `standalone`: each
/// statement with the CTEs from above it put in (runs on its own).
pub fn export(cells: &[Piece], d: Dialect, standalone: bool) -> String {
    let mut out = Vec::new();
    let mut known = Known::default();
    for c in cells {
        match c.kind {
            Kind::Markdown => {
                let text: Vec<String> = c.text.trim().lines().map(|l| if l.is_empty() { "--".into() } else { format!("-- {l}") }).collect();
                if !text.is_empty() {
                    out.push(text.join("\n"));
                }
            }
            Kind::Sql => {
                let mut stmts = Vec::new();
                for r in super::split(&c.text, d, false) {
                    let s = &c.text[r];
                    let only_with = parse_with(s.trim().trim_end_matches(';'), d).is_some_and(|w| w.main.is_empty());
                    if standalone {
                        // A WITH alone isn't a statement: it goes into the
                        // ones below.
                        if !only_with {
                            let g = glue(s.trim(), &known, d);
                            stmts.push(if g.trim_end().ends_with(';') { g.trim_end().to_string() } else { format!("{};", g.trim_end()) });
                        }
                    } else {
                        let s = s.trim();
                        let plain = s.trim_end_matches(';').trim_end();
                        stmts.push(if only_with { format!("{plain}\n-- (WITH only: used by the statements below)") } else if s.ends_with(';') { s.to_string() } else { format!("{s};") });
                    }
                    known.learn(s.trim().trim_end_matches(';'), d);
                }
                if !stmts.is_empty() {
                    out.push(stmts.join("\n\n"));
                }
            }
        }
    }
    let mut s = out.join("\n\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_parsed() {
        let w = parse_with("with a as (select 1 x), \"B\"(y) as materialized (select x from a) select * from \"B\";", Dialect::Postgres).unwrap();
        assert_eq!(w.ctes.len(), 2);
        assert_eq!(w.ctes[1].name, "B");
        assert_eq!(w.ctes[1].text, "\"B\"(y) as materialized (select x from a)");
        assert!(!w.main.is_empty());
        let only = parse_with("WITH a AS (select 1)", Dialect::Postgres).unwrap();
        assert!(only.main.is_empty());
        assert!(parse_with("select 1", Dialect::Postgres).is_none());
    }

    #[test]
    fn glued_from_above() {
        let d = Dialect::Postgres;
        let mut k = Known::default();
        k.learn("with paid as (select * from orders where status = 'paid'), big as (select * from paid where total > 100), unused as (select 1)", d);
        // Uses big: big and what it uses (paid) come along, not the rest.
        let g = glue("select count(*) from big;", &k, d);
        assert_eq!(g, "WITH paid as (select * from orders where status = 'paid'),\nbig as (select * from paid where total > 100)\nselect count(*) from big");
        // Its own WITH merged: one WITH, own CTEs after the ones above.
        let g = glue("with top as (select * from big order by total desc limit 5) select * from top", &k, d);
        assert!(g.starts_with("WITH paid as") && g.contains("big as") && g.contains(",\ntop as (") && g.matches("WITH").count() == 1 && !g.to_lowercase().contains("with top"), "{g}");
        // Nothing used: left as it is.
        assert_eq!(glue("select 1;", &k, d), "select 1;");
        // PL/SQL keeps its END; even when a word matches a CTE.
        let block = "begin\n  insert into log select * from paid;\nend;";
        assert_eq!(glue(block, &k, Dialect::Oracle), block);
        // A WITH alone shows its last CTE.
        assert_eq!(glue("with x as (select 2)", &Known::default(), d), "WITH x as (select 2)\nSELECT * FROM x");
        // Export of a PL/SQL cell: one END;, not two.
        let cells = vec![Piece { kind: Kind::Sql, text: "begin\n  null;\nend;".into() }];
        assert_eq!(export(&cells, Dialect::Oracle, true), "begin\n  null;\nend;\n");
        assert_eq!(export(&cells, Dialect::Oracle, false), "begin\n  null;\nend;\n");
        // A name of its own isn't taken from above.
        let g = glue("with paid as (select 1) select * from paid", &k, d);
        assert_eq!(g.matches("paid as").count(), 1, "{g}");
    }

    #[test]
    fn marks_round_trip() {
        let cells = vec![Piece { kind: Kind::Markdown, text: "# Report\n\nPaid orders".into() }, Piece { kind: Kind::Sql, text: "with paid as (select 1)".into() }, Piece { kind: Kind::Sql, text: "select * from paid;".into() }];
        let text = render(&cells);
        assert!(is_notebook(&text));
        assert_eq!(parse(&text, Dialect::Postgres), cells);
        // Plain SQL: a cell per statement.
        let p = parse("-- first\nselect 1;\n\nselect 2;\n", Dialect::Postgres);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].text, "-- first\nselect 1;");
    }

    #[test]
    fn exported() {
        let cells = vec![Piece { kind: Kind::Markdown, text: "# Report".into() }, Piece { kind: Kind::Sql, text: "with paid as (select 1 x)".into() }, Piece { kind: Kind::Sql, text: "select * from paid".into() }];
        let plain = export(&cells, Dialect::Postgres, false);
        assert!(plain.starts_with("-- # Report\n\nwith paid as (select 1 x)\n-- (WITH only"), "{plain}");
        let alone = export(&cells, Dialect::Postgres, true);
        assert_eq!(alone, "-- # Report\n\nWITH paid as (select 1 x)\nselect * from paid;\n");
    }
}
