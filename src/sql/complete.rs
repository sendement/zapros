//! Hints while typing: after "alias." its table's columns, after FROM /
//! JOIN tables, after JOIN … ON a join condition guessed from column names,
//! elsewhere columns of the tables in scope, keywords, functions and a few
//! snippets. Items are ranked: starts-with first, then contains.

use std::ops::Range;

use super::lex::{self, Tk, Tok};
use super::{Case, Dialect};
use crate::db::Relation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Join,
    Column,
    Table,
    Schema,
    Snippet,
    Keyword,
    Function,
}

impl Kind {
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Join => "⋈",
            Kind::Column => "▸",
            Kind::Table => "▦",
            Kind::Schema => "📂",
            Kind::Snippet => "✂",
            Kind::Keyword => "k",
            Kind::Function => "ƒ",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: String,
    /// What replaces the word typed; "$0" marks where the cursor goes.
    pub insert: String,
    pub kind: Kind,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct Hints {
    /// The word being typed (replaced by the item).
    pub range: Range<usize>,
    pub items: Vec<Item>,
}

/// A table in a statement: what it's called there, and what it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    pub alias: String,
    pub schema: Option<String>,
    pub table: String,
}

const SNIPPETS: &[(&str, &str, &str)] = &[
    ("sel", "SELECT *\n  FROM $0", "SELECT * FROM …"),
    ("selc", "SELECT COUNT(*)\n  FROM $0", "SELECT COUNT(*) FROM …"),
    ("case", "CASE\n  WHEN $0 THEN \n  ELSE \nEND", "CASE WHEN … END"),
    ("ins", "INSERT INTO $0 ()\nVALUES ()", "INSERT INTO … VALUES"),
    ("upd", "UPDATE $0\n   SET \n WHERE ", "UPDATE … SET … WHERE"),
    ("del", "DELETE FROM $0\n WHERE ", "DELETE FROM … WHERE"),
    ("cte", "WITH $0 AS (\n  \n)\nSELECT *\n  FROM ", "WITH … AS (…)"),
    ("lj", "LEFT JOIN $0 ON ", "LEFT JOIN … ON"),
];

/// How a table goes into the text: bare on the search path, else with
/// its schema.
fn qualified(r: &Relation, d: Dialect, qs: bool) -> String {
    if r.qualify { format!("{}.{}", schema_name(&r.schema, d, qs), quote(&r.name, d)) } else { quote(&r.name, d) }
}

/// A schema's name as it goes in: in quotes always when asked (the
/// setting), else only when it must be.
pub fn schema_name(name: &str, d: Dialect, always: bool) -> String {
    if !always {
        return quote(name, d);
    }
    if d == Dialect::ClickHouse {
        format!("`{}`", name.replace('`', "\\`"))
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

fn ident_text(src: &str, t: &Tok) -> Option<String> {
    match t.kind {
        Tk::Word | Tk::Keyword => Some(t.text(src).to_string()),
        Tk::Quoted => Some(super::unquote(t.text(src)).to_string()),
        _ => None,
    }
}

/// Tables named in a statement (FROM, JOIN, UPDATE, INTO; FROM a, b).
pub fn scope(src: &str, d: Dialect) -> Vec<Ref> {
    let toks: Vec<Tok> = lex::lex(src, d).into_iter().filter(|t| !t.trivia()).collect();
    let up = |k: usize| toks.get(k).filter(|t| matches!(t.kind, Tk::Keyword | Tk::Word)).map(|t| t.text(src).to_ascii_uppercase()).unwrap_or_default();
    let mut out = Vec::new();
    let mut in_from = false;
    let mut depth = 0;
    let mut from_depth = -1;
    for k in 0..toks.len() {
        match toks[k].kind {
            Tk::Open => depth += 1,
            Tk::Close => depth -= 1,
            _ => {}
        }
        let w = up(k);
        if matches!(w.as_str(), "WHERE" | "GROUP" | "ORDER" | "SET" | "ON" | "USING" | "HAVING" | "LIMIT" | "SELECT" | "VALUES" | "PREWHERE") && depth == from_depth {
            in_from = false;
        }
        let starts = matches!(w.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") || (toks[k].kind == Tk::Comma && in_from && depth == from_depth);
        if !starts {
            continue;
        }
        if w == "FROM" {
            in_from = true;
            from_depth = depth;
        }
        let mut j = k + 1;
        if matches!(up(j).as_str(), "ONLY" | "LATERAL" | "TABLE") {
            j += 1;
        }
        // A subquery: known by its alias.
        if toks.get(j).is_some_and(|t| t.kind == Tk::Open) && w != "INTO" {
            let Some(c) = close_of(&toks, j) else { continue };
            if let Some(alias) = alias_after(src, &toks, c + 1) {
                out.push(Ref { alias: alias.clone(), schema: None, table: alias });
            }
            continue;
        }
        let Some(first) = toks.get(j).and_then(|t| ident_text(src, t)) else { continue };
        if toks[j].kind == Tk::Keyword {
            continue;
        }
        let (schema, table, end) = match (toks.get(j + 1), toks.get(j + 2)) {
            (Some(dot), Some(t)) if dot.kind == Tk::Dot => match ident_text(src, t) {
                Some(n) => (Some(first), n, j + 2),
                None => (Some(first), String::new(), j + 1),
            },
            _ => (None, first, j),
        };
        if toks.get(end + 1).is_some_and(|t| t.kind == Tk::Open) {
            continue;
        }
        let mut a = end + 1;
        if up(a) == "AS" {
            a += 1;
        }
        let alias = toks.get(a).filter(|t| t.kind == Tk::Word || t.kind == Tk::Quoted).and_then(|t| ident_text(src, t)).unwrap_or_else(|| table.clone());
        out.push(Ref { alias, schema, table });
    }
    out
}

/// The bracket closing the one at `open`.
fn close_of(toks: &[Tok], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (k, t) in toks.iter().enumerate().skip(open) {
        match t.kind {
            Tk::Open => depth += 1,
            Tk::Close => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// A name given at `k`: `[AS] alias`.
fn alias_after(src: &str, toks: &[Tok], mut k: usize) -> Option<String> {
    if toks.get(k).is_some_and(|t| t.kind == Tk::Keyword && t.text(src).eq_ignore_ascii_case("AS")) {
        k += 1;
    }
    toks.get(k).filter(|t| matches!(t.kind, Tk::Word | Tk::Quoted)).and_then(|t| ident_text(src, t))
}

/// Relations a statement makes itself, with the columns they give: its
/// CTEs (`true`) and subqueries in FROM / JOIN under an alias (`false`).
/// Those inside come first, so a `*` over one is known to those around.
pub fn derived(src: &str, d: Dialect, bases: &[&[Relation]]) -> Vec<(Relation, bool)> {
    let toks: Vec<Tok> = lex::lex(src, d).into_iter().filter(|t| !t.trivia()).collect();
    let word = |k: usize, w: &str| toks.get(k).is_some_and(|t| matches!(t.kind, Tk::Word | Tk::Keyword) && t.text(src).eq_ignore_ascii_case(w));
    let starts_query = |k: usize| word(k, "SELECT") || word(k, "WITH");
    // (Where its body opens, its name, its own column names, a CTE.)
    let mut found: Vec<(usize, usize, String, Vec<String>, bool)> = Vec::new();
    for w in 0..toks.len() {
        if word(w, "WITH") {
            let mut k = w + 1;
            if word(k, "RECURSIVE") {
                k += 1;
            }
            loop {
                let Some(name) = toks.get(k).filter(|t| matches!(t.kind, Tk::Word | Tk::Quoted)).and_then(|t| ident_text(src, t)) else { break };
                k += 1;
                let mut names = Vec::new();
                if toks.get(k).is_some_and(|t| t.kind == Tk::Open) {
                    let Some(c) = close_of(&toks, k) else { break };
                    names = toks[k + 1..c].iter().filter_map(|t| ident_text(src, t)).collect();
                    k = c + 1;
                }
                if !word(k, "AS") {
                    break;
                }
                k += 1;
                if word(k, "NOT") {
                    k += 1;
                }
                if word(k, "MATERIALIZED") {
                    k += 1;
                }
                if !toks.get(k).is_some_and(|t| t.kind == Tk::Open) {
                    break;
                }
                let Some(c) = close_of(&toks, k) else { break };
                found.push((k, c, name, names, true));
                k = c + 1;
                if toks.get(k).is_some_and(|t| t.kind == Tk::Comma) {
                    k += 1;
                } else {
                    break;
                }
            }
        }
        if toks[w].kind == Tk::Open && starts_query(w + 1) {
            // In FROM: the nearest clause word before it, at its depth.
            let mut depth = 0;
            let mut clause = String::new();
            for t in toks[..w].iter().rev() {
                match t.kind {
                    Tk::Close => depth += 1,
                    Tk::Open if depth == 0 => break,
                    Tk::Open => depth -= 1,
                    _ if depth == 0 => {
                        let u = t.text(src).to_ascii_uppercase();
                        if matches!(u.as_str(), "FROM" | "JOIN" | "SELECT" | "WHERE" | "ON" | "SET" | "GROUP" | "ORDER" | "HAVING" | "VALUES" | "AS" | "IN" | "EXISTS") {
                            clause = u;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if matches!(clause.as_str(), "FROM" | "JOIN") {
                let Some(c) = close_of(&toks, w) else { continue };
                let Some(alias) = alias_after(src, &toks, c + 1) else { continue };
                let mut k = c + 1 + if word(c + 1, "AS") { 2 } else { 1 };
                let mut names = Vec::new();
                if toks.get(k).is_some_and(|t| t.kind == Tk::Open) {
                    if let Some(e) = close_of(&toks, k) {
                        names = toks[k + 1..e].iter().filter_map(|t| ident_text(src, t)).collect();
                        k = e;
                    }
                }
                let _ = k;
                found.push((w, c, alias, names, false));
            }
        }
    }
    found.sort_by_key(|f| f.1);
    let mut out: Vec<(Relation, bool)> = Vec::new();
    for (o, c, name, names, cte) in found {
        let body = &src[toks[o].end..toks[c].start];
        let made: Vec<Relation> = out.iter().map(|(r, _)| r.clone()).collect();
        let mut cols = out_cols(body, d, &made, bases);
        // Its own names over what the select gives (as many as there are).
        for (k, n) in names.into_iter().enumerate() {
            match cols.get_mut(k) {
                Some(c) => c.0 = n,
                None => cols.push((n, String::new())),
            }
        }
        // (Recursive CTEs: their columns as of the first branch only.)
        out.retain(|(r, _)| !r.name.eq_ignore_ascii_case(&name));
        out.push((Relation { schema: String::new(), name, cols, qualify: false }, cte));
    }
    out
}

/// The columns a query gives: its first SELECT's list (names as given,
/// `*` and `t.*` opened up), with their types when they're plain columns.
fn out_cols(body: &str, d: Dialect, made: &[Relation], bases: &[&[Relation]]) -> Vec<(String, String)> {
    let toks: Vec<Tok> = lex::lex(body, d).into_iter().filter(|t| !t.trivia()).collect();
    let up = |t: &Tok| t.text(body).to_ascii_uppercase();
    let mut depth = 0;
    let mut sel = None;
    for (k, t) in toks.iter().enumerate() {
        match t.kind {
            Tk::Open => depth += 1,
            Tk::Close => depth -= 1,
            _ if depth == 0 && up(t) == "SELECT" => {
                sel = Some(k);
                break;
            }
            _ => {}
        }
    }
    let Some(mut k) = sel.map(|k| k + 1) else { return Vec::new() };
    while toks.get(k).is_some_and(|t| matches!(up(t).as_str(), "DISTINCT" | "ALL")) {
        k += 1;
        if toks.get(k).is_some_and(|t| up(t) == "ON") && toks.get(k + 1).is_some_and(|t| t.kind == Tk::Open) {
            k = close_of(&toks, k + 1).map_or(k + 1, |c| c + 1);
        }
    }
    if toks.get(k).is_some_and(|t| up(t) == "TOP") {
        k += 2;
    }
    // The items, up to the list's end.
    let mut items: Vec<&[Tok]> = Vec::new();
    let (mut from, mut depth) = (k, 0);
    let mut end = toks.len();
    for (j, t) in toks.iter().enumerate().skip(k) {
        match t.kind {
            Tk::Open => depth += 1,
            Tk::Close => depth -= 1,
            Tk::Comma if depth == 0 => {
                items.push(&toks[from..j]);
                from = j + 1;
            }
            Tk::Keyword if depth == 0 && matches!(up(t).as_str(), "FROM" | "INTO" | "WHERE" | "UNION" | "INTERSECT" | "EXCEPT" | "MINUS" | "GROUP" | "ORDER" | "LIMIT" | "HAVING" | "WINDOW" | "FETCH" | "OFFSET") => {
                end = j;
                break;
            }
            _ => {}
        }
    }
    if from < end {
        items.push(&toks[from..end]);
    }
    let refs = scope(body, d);
    let look = |r: &Ref| find(made, r).or_else(|| bases.iter().find_map(|b| find(b, r)));
    let typed = |alias: Option<&str>, col: &str| -> String {
        refs.iter()
            .filter(|r| alias.is_none_or(|a| r.alias.eq_ignore_ascii_case(a)))
            .filter_map(look)
            .find_map(|rel| rel.cols.iter().find(|(c, _)| c.eq_ignore_ascii_case(col)).map(|(_, ty)| ty.clone()))
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for it in items {
        let Some(last) = it.last() else { continue };
        let prev = it.len().checked_sub(2).map(|k| &it[k]);
        if last.text(body) == "*" {
            let alias = (it.len() == 3 && it[1].kind == Tk::Dot).then(|| ident_text(body, &it[0])).flatten();
            for r in refs.iter().filter(|r| alias.as_deref().is_none_or(|a| r.alias.eq_ignore_ascii_case(a))) {
                if let Some(rel) = look(r) {
                    out.extend(rel.cols.iter().cloned());
                }
            }
            continue;
        }
        let Some(name) = ident_text(body, last) else { continue };
        let plain = it.len() == 1;
        let dotted = prev.is_some_and(|p| p.kind == Tk::Dot);
        let as_ = prev.is_some_and(|p| up(p) == "AS");
        // `expr alias`: a name after something that ends an expression.
        let bare = matches!(last.kind, Tk::Word | Tk::Quoted) && prev.is_some_and(|p| matches!(p.kind, Tk::Close | Tk::Word | Tk::Quoted | Tk::Number | Tk::Str));
        if !(plain || dotted || as_ || bare) {
            continue;
        }
        let ty = if plain {
            typed(None, &name)
        } else if dotted && it.len() == 3 {
            typed(ident_text(body, &it[0]).as_deref(), &name)
        } else {
            String::new()
        };
        out.push((name, ty));
    }
    out
}

fn find<'a>(rels: &'a [Relation], r: &Ref) -> Option<&'a Relation> {
    rels.iter().find(|x| x.name.eq_ignore_ascii_case(&r.table) && r.schema.as_ref().is_none_or(|s| s.eq_ignore_ascii_case(&x.schema)))
}

/// Words a database won't take as a bare name (types and others beyond
/// the SQL keywords the highlighter knows).
fn reserved(name: &str, d: Dialect) -> bool {
    const ORA: &[&str] = &[
        "ACCESS", "ADD", "ALL", "ALTER", "AND", "ANY", "AS", "ASC", "AUDIT", "BETWEEN", "BY", "CHAR", "CHECK", "CLUSTER", "COLUMN", "COMMENT", "COMPRESS", "CONNECT", "CREATE", "CURRENT", "DATE", "DECIMAL", "DEFAULT", "DELETE", "DESC", "DISTINCT",
        "DROP", "ELSE", "EXCLUSIVE", "EXISTS", "FILE", "FLOAT", "FOR", "FROM", "GRANT", "GROUP", "HAVING", "IDENTIFIED", "IMMEDIATE", "IN", "INCREMENT", "INDEX", "INITIAL", "INSERT", "INTEGER", "INTERSECT", "INTO", "IS", "LEVEL", "LIKE", "LOCK",
        "LONG", "MAXEXTENTS", "MINUS", "MLSLABEL", "MODE", "MODIFY", "NOAUDIT", "NOCOMPRESS", "NOT", "NOWAIT", "NULL", "NUMBER", "OF", "OFFLINE", "ON", "ONLINE", "OPTION", "OR", "ORDER", "PCTFREE", "PRIOR", "PUBLIC", "RAW", "RENAME", "RESOURCE",
        "REVOKE", "ROW", "ROWID", "ROWNUM", "ROWS", "SELECT", "SESSION", "SET", "SHARE", "SIZE", "SMALLINT", "START", "SUCCESSFUL", "SYNONYM", "SYSDATE", "TABLE", "THEN", "TO", "TRIGGER", "UID", "UNION", "UNIQUE", "UPDATE", "USER", "VALIDATE",
        "VALUES", "VARCHAR", "VARCHAR2", "VIEW", "WHENEVER", "WHERE", "WITH",
    ];
    const PG: &[&str] = &[
        "ALL", "ANALYSE", "ANALYZE", "AND", "ANY", "ARRAY", "AS", "ASC", "ASYMMETRIC", "AUTHORIZATION", "BINARY", "BOTH", "CASE", "CAST", "CHECK", "COLLATE", "COLLATION", "COLUMN", "CONCURRENTLY", "CONSTRAINT", "CREATE", "CROSS", "CURRENT_CATALOG",
        "CURRENT_DATE", "CURRENT_ROLE", "CURRENT_SCHEMA", "CURRENT_TIME", "CURRENT_TIMESTAMP", "CURRENT_USER", "DEFAULT", "DEFERRABLE", "DESC", "DISTINCT", "DO", "ELSE", "END", "EXCEPT", "FALSE", "FETCH", "FOR", "FOREIGN", "FREEZE", "FROM", "FULL",
        "GRANT", "GROUP", "HAVING", "ILIKE", "IN", "INITIALLY", "INNER", "INTERSECT", "INTO", "IS", "ISNULL", "JOIN", "LATERAL", "LEADING", "LEFT", "LIKE", "LIMIT", "LOCALTIME", "LOCALTIMESTAMP", "NATURAL", "NOT", "NOTNULL", "NULL", "OFFSET", "ON",
        "ONLY", "OR", "ORDER", "OUTER", "OVERLAPS", "PLACING", "PRIMARY", "REFERENCES", "RETURNING", "RIGHT", "SELECT", "SESSION_USER", "SIMILAR", "SOME", "SYMMETRIC", "SYSTEM_USER", "TABLE", "TABLESAMPLE", "THEN", "TO", "TRAILING", "TRUE", "UNION",
        "UNIQUE", "USER", "USING", "VARIADIC", "VERBOSE", "WHEN", "WHERE", "WINDOW", "WITH",
    ];
    let up = name.to_ascii_uppercase();
    match d {
        Dialect::Oracle => ORA.contains(&up.as_str()),
        Dialect::Postgres => PG.contains(&up.as_str()),
        Dialect::Sqlite | Dialect::Generic => PG.contains(&up.as_str()) || ORA.contains(&up.as_str()),
        Dialect::ClickHouse => false,
    }
}

/// A name as it must be written (quoted when it'd fold or clash).
pub fn quote(name: &str, d: Dialect) -> String {
    // (ClickHouse's own database `default` reads fine bare.)
    let keyword = (super::is_keyword(name, d) || reserved(name, d)) && !(d == Dialect::ClickHouse && name == "default");
    // `$` (and Oracle's `#`) go bare after the first letter where the
    // database takes them so.
    let also = |c: char| match d {
        Dialect::Oracle => c == '$' || c == '#',
        Dialect::Postgres | Dialect::Sqlite => c == '$',
        _ => false,
    };
    let plain = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || also(c)) && name.starts_with(|c: char| c.is_alphabetic() || c == '_') && !keyword;
    let folds = match d {
        Dialect::Postgres => name.chars().any(|c| c.is_uppercase()),
        Dialect::Oracle => name.chars().any(|c| c.is_lowercase()),
        _ => false,
    };
    if plain && !folds {
        name.to_string()
    } else if d == Dialect::ClickHouse {
        format!("`{name}`")
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// Quotes taken off the names that don't need them ("SHOP"."ORDERS" →
/// SHOP.ORDERS, `id` → id); ones with spaces, other case or keywords keep
/// theirs.
pub fn bare_names(src: &str, d: Dialect) -> String {
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    for t in super::lex::lex(src, d) {
        if t.kind != super::lex::Tk::Quoted {
            continue;
        }
        let text = t.text(src);
        let inner = super::unquote(text);
        // (ClickHouse's `default` stays quoted: as a column it'd read as DEFAULT.)
        if inner.is_empty() || quote(inner, d) != inner || (d == Dialect::ClickHouse && inner.eq_ignore_ascii_case("default")) {
            continue;
        }
        out.push_str(&src[at..t.start]);
        out.push_str(inner);
        at = t.end;
    }
    out.push_str(&src[at..]);
    out
}

/// Columns that look like a join between two tables: b.x_id = a.id, or
/// the same *_id on both sides.
fn join_guesses<'a>(look: impl Fn(&Ref) -> Option<&'a Relation>, refs: &[Ref], d: Dialect) -> Vec<Item> {
    let Some((last, others)) = refs.split_last() else { return Vec::new() };
    let Some(b) = look(last) else { return Vec::new() };
    let mut out = Vec::new();
    for o in others {
        let Some(a) = look(o) else { continue };
        let sing = |t: &str| t.trim_end_matches('s').to_lowercase();
        for (bc, _) in &b.cols {
            for (ac, _) in &a.cols {
                let (bl, al) = (bc.to_lowercase(), ac.to_lowercase());
                let hit = (bl == al && bl.ends_with("_id"))
                    || (al == "id" && (bl == format!("{}_id", sing(&a.name)) || bl == format!("{}id", sing(&a.name))))
                    || (bl == "id" && (al == format!("{}_id", sing(&b.name)) || al == format!("{}id", sing(&b.name))));
                if hit {
                    let text = format!("{}.{} = {}.{}", quote(&last.alias, d), quote(bc, d), quote(&o.alias, d), quote(ac, d));
                    out.push(Item { label: text.clone(), insert: text, kind: Kind::Join, detail: format!("{} ⋈ {}", b.name, a.name) });
                }
            }
        }
    }
    out
}

/// Hints at `at` (None inside strings and comments, or with nothing to say).
/// (`lent`: relations from elsewhere, a notebook's CTEs from cells above.)
pub fn hints(src: &str, at: usize, d: Dialect, rels: &[Relation], lent: &[Relation], case: Case, forced: bool, quote_schemas: bool) -> Option<Hints> {
    let at = at.min(src.len());
    let toks = lex::lex(src, d);
    if toks.iter().any(|t| t.start < at && (at < t.end || t.open && at <= t.end) && matches!(t.kind, Tk::Str | Tk::LineComment | Tk::BlockComment | Tk::Quoted)) {
        return None;
    }
    let start = src[..at].char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '$').last().map_or(at, |(i, _)| i);
    let prefix = &src[start..at];
    let qualifier = src[..start].strip_suffix('.').map(|before| {
        let s = before.char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '"' || *c == '`').last().map_or(before.len(), |(i, _)| i);
        before[s..].trim_matches(|c| c == '"' || c == '`').to_string()
    });
    if prefix.is_empty() && qualifier.is_none() && !forced {
        return None;
    }
    let stmt = super::statement_at(src, at, d, true).unwrap_or(0..src.len());
    let stmt_text = &src[stmt.start..stmt.end.max(at)];
    let refs = scope(stmt_text, d);
    // Its own CTEs and subqueries, then what's lent, then the database's.
    let made: Vec<(Relation, bool)> = derived(stmt_text, d, &[lent, rels]);
    let own: Vec<Relation> = made.iter().map(|(r, _)| r.clone()).collect();
    let look = |r: &Ref| find(&own, r).or_else(|| find(lent, r)).or_else(|| find(rels, r));
    let ctes = || made.iter().filter(|(_, c)| *c).map(|(r, _)| r).chain(lent).map(|r| Item { label: r.name.clone(), insert: quote(&r.name, d), kind: Kind::Table, detail: format!("WITH · {}", r.cols.len()) });
    let before: Vec<Tok> = lex::lex(&src[stmt.start..start], d).into_iter().filter(|t| !t.trivia()).collect();
    let prev = before.last().map(|t| t.text(&src[stmt.start..start]).to_ascii_uppercase()).unwrap_or_default();
    let mut items: Vec<Item> = Vec::new();
    let col = |name: &str, ty: &str, owner: &str| Item { label: name.to_string(), insert: quote(name, d), kind: Kind::Column, detail: format!("{ty} · {owner}") };
    if let Some(q) = &qualifier {
        // alias. or table. → columns; schema. → its tables.
        if let Some(r) = refs.iter().find(|r| r.alias.eq_ignore_ascii_case(q)).cloned().or_else(|| Some(Ref { alias: q.clone(), schema: None, table: q.clone() })) {
            if let Some(rel) = look(&r) {
                items.extend(rel.cols.iter().map(|(c, ty)| col(c, ty, &rel.name)));
            }
        }
        items.extend(rels.iter().filter(|r| r.schema.eq_ignore_ascii_case(q)).map(|r| Item { label: r.name.clone(), insert: quote(&r.name, d), kind: Kind::Table, detail: r.schema.clone() }));
    } else if matches!(prev.as_str(), "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE" | "TRUNCATE" | "DESCRIBE" | "DESC") || (prev == "," && before.iter().rev().find(|t| t.kind == Tk::Keyword).is_some_and(|t| t.text(&src[stmt.start..start]).eq_ignore_ascii_case("FROM"))) {
        let mut schemas: Vec<&str> = rels.iter().map(|r| r.schema.as_str()).collect();
        schemas.dedup();
        items.extend(ctes());
        items.extend(rels.iter().map(|r| Item { label: r.name.clone(), insert: qualified(r, d, quote_schemas), kind: Kind::Table, detail: format!("{} · {}", r.schema, r.cols.len()) }));
        items.extend(schemas.iter().map(|s| Item { label: s.to_string(), insert: schema_name(s, d, quote_schemas), kind: Kind::Schema, detail: String::new() }));
    } else {
        if prev == "ON" {
            items.extend(join_guesses(look, &refs, d));
        }
        for r in &refs {
            if let Some(rel) = look(r) {
                items.extend(rel.cols.iter().map(|(c, ty)| col(c, ty, &r.alias)));
            }
        }
        items.extend(SNIPPETS.iter().map(|(k, body, label)| Item { label: k.to_string(), insert: cased_snippet(body, case), kind: Kind::Snippet, detail: label.to_string() }));
        items.extend(super::keywords(d).into_iter().map(|k| Item { label: k.to_string(), insert: case.apply(k), kind: Kind::Keyword, detail: String::new() }));
        items.extend(super::functions(d).into_iter().map(|f| {
            let name = if d == Dialect::ClickHouse { f.to_string() } else { case.apply(f) };
            Item { label: f.to_string(), insert: format!("{name}($0)"), kind: Kind::Function, detail: String::new() }
        }));
        items.extend(ctes());
        items.extend(rels.iter().map(|r| Item { label: r.name.clone(), insert: qualified(r, d, quote_schemas), kind: Kind::Table, detail: r.schema.clone() }));
    }
    let p = prefix.to_lowercase();
    let mut scored: Vec<(u8, Item)> = items
        .into_iter()
        .filter_map(|it| {
            let l = it.label.to_lowercase();
            let s = if p.is_empty() || l.starts_with(&p) {
                0
            } else if l.contains(&p) {
                1
            } else if it.kind == Kind::Column && subsequence(&p, &l) {
                2
            } else {
                return None;
            };
            // Snippets only by their exact start; keywords need a letter typed.
            if (it.kind == Kind::Snippet && s != 0 || matches!(it.kind, Kind::Keyword | Kind::Function | Kind::Snippet) && p.is_empty()) && qualifier.is_none() && !(forced && it.kind != Kind::Snippet) {
                return None;
            }
            // The word itself, typed out.
            if l == p && it.kind == Kind::Keyword {
                return None;
            }
            Some((s, it))
        })
        .collect();
    // Columns keep their table's order; the rest go shortest first.
    scored.sort_by(|a, b| {
        let o = a.0.cmp(&b.0).then(a.1.kind.cmp(&b.1.kind));
        if a.1.kind == Kind::Column { o } else { o.then(a.1.label.len().cmp(&b.1.label.len())).then(a.1.label.cmp(&b.1.label)) }
    });
    scored.dedup_by(|a, b| a.1.label == b.1.label && a.1.kind == b.1.kind && a.1.detail == b.1.detail);
    let items: Vec<Item> = scored.into_iter().map(|(_, i)| i).take(60).collect();
    (!items.is_empty()).then_some(Hints { range: start..at, items })
}

fn subsequence(p: &str, s: &str) -> bool {
    let mut it = s.chars();
    p.chars().all(|c| it.any(|x| x == c))
}

fn cased_snippet(body: &str, case: Case) -> String {
    let toks = lex::lex(body, Dialect::Generic);
    toks.iter().map(|t| if t.kind == Tk::Keyword || t.text(body) == "COUNT" { case.apply(t.text(body)) } else { t.text(body).to_string() }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rels() -> Vec<Relation> {
        vec![
            Relation { schema: "public".into(), name: "users".into(), cols: vec![("id".into(), "int".into()), ("name".into(), "text".into())], qualify: false },
            Relation { schema: "public".into(), name: "orders".into(), cols: vec![("id".into(), "int".into()), ("user_id".into(), "int".into()), ("total".into(), "numeric".into())], qualify: false },
        ]
    }

    fn labels_at(src: &str, at: &str, lent: &[Relation]) -> Vec<(String, String)> {
        let at = src.find(at).unwrap() + at.len();
        hints(src, at, Dialect::Postgres, &rels(), lent, Case::Upper, true, false).map(|h| h.items.into_iter().filter(|i| i.kind == Kind::Column || i.kind == Kind::Table).map(|i| (i.label, i.detail)).collect()).unwrap_or_default()
    }

    #[test]
    fn ctes_and_subqueries_hinted() {
        let has = |v: &[(String, String)], l: &str| v.iter().any(|(x, _)| x == l);
        // A CTE: its columns after its alias, plain, `*`, types kept.
        let src = "with big as (select o.id, o.total as sum, count(*) n, u.* from orders o join users u on u.id = o.user_id) select b. from big b";
        let h = labels_at(src, "select b.", &[]);
        assert_eq!(h.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["id", "sum", "n", "id", "name"], "{h:?}");
        assert!(h[0].1.starts_with("int"), "{h:?}");
        // Its name after FROM.
        let src = "with big as (select id from orders) select * from b";
        assert!(has(&labels_at(src, "from b", &[]), "big"));
        // Without an alias; a column list of its own; a CTE over a CTE.
        let src = "with a(x, y) as (select id, total from orders), b as (select * from a) select  from b";
        let h = labels_at(src, "select ", &[]);
        assert!(has(&h, "x") && has(&h, "y"), "{h:?}");
        // A subquery under an alias, and one in a JOIN.
        let src = "select s. from (select user_id, sum(total) as spent from orders group by user_id) s";
        let h = labels_at(src, "select s.", &[]);
        assert_eq!(h.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["user_id", "spent"], "{h:?}");
        let src = "select x. from users u join (select user_id as uid from orders) as x on x.uid = u.id";
        assert!(has(&labels_at(src, "select x.", &[]), "uid"));
        // A scalar subquery in the list is no table.
        let src = "select (select max(id) from orders) m, u. from users u";
        assert!(has(&labels_at(src, "u.", &[]), "name"));
        // Lent (a notebook's WITH cell above).
        let lent = crate::sql::notebook::Known { ctes: vec![crate::sql::notebook::Cte { name: "big".into(), text: "big as (select id, total from orders where total > 900)".into() }], recursive: false }.relations(Dialect::Postgres, &rels());
        let h = labels_at("select g. from big g", "select g.", &lent);
        assert_eq!(h.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["id", "total"], "{h:?}");
        assert!(has(&labels_at("select * from bi", "from bi", &lent), "big"));
    }

    #[test]
    fn alias_columns() {
        let src = "select u. from users u";
        let h = hints(src, 9, Dialect::Postgres, &rels(), &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), vec!["id", "name"]);
        let h = hints("select o. from orders o", 9, Dialect::Postgres, &rels(), &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), vec!["id", "user_id", "total"]);
        let src = "select o.to from orders as o";
        let h = hints(src, 11, Dialect::Postgres, &rels(), &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].label, "total");
        assert_eq!(h.range, 9..11);
    }

    #[test]
    fn tables_after_from_and_joins() {
        let src = "select * from us";
        let h = hints(src, src.len(), Dialect::Postgres, &rels(), &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].label, "users");
        let src = "select * from users u join orders o on ";
        let h = hints(src, src.len(), Dialect::Postgres, &rels(), &[], Case::Upper, true, false).unwrap();
        assert_eq!(h.items[0].insert, "o.user_id = u.id");
    }

    #[test]
    fn tables_off_the_search_path_keep_their_schema() {
        let mut r = rels();
        r.push(Relation { schema: "sales".into(), name: "invoices".into(), cols: vec![("id".into(), "int".into())], qualify: true });
        let src = "select * from inv";
        let h = hints(src, src.len(), Dialect::Postgres, &r, &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].insert, "sales.invoices");
        // Its columns by alias all the same.
        let h = hints("select i. from sales.invoices i", 9, Dialect::Postgres, &r, &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].label, "id");
    }

    #[test]
    fn keywords_by_case_and_none_in_strings() {
        let h = hints("sel", 3, Dialect::Postgres, &[], &[], Case::Lower, false, false).unwrap();
        assert!(h.items.iter().any(|i| i.insert == "select"));
        assert_eq!(h.items[0].kind, Kind::Snippet);
        assert!(hints("select 'fro", 11, Dialect::Postgres, &[], &[], Case::Upper, false, false).is_none());
    }

    #[test]
    fn schemas_quoted_when_asked() {
        let r = vec![Relation { schema: "sales".into(), name: "invoices".into(), cols: vec![("id".into(), "int".into())], qualify: true }];
        let src = "select * from inv";
        let plain = hints(src, src.len(), Dialect::Postgres, &r, &[], Case::Upper, false, false).unwrap();
        assert!(plain.items.iter().any(|i| i.insert == "sales.invoices"));
        let quoted = hints(src, src.len(), Dialect::Postgres, &r, &[], Case::Upper, false, true).unwrap();
        assert!(quoted.items.iter().any(|i| i.insert == "\"sales\".invoices"), "{:?}", quoted.items.iter().map(|i| &i.insert).collect::<Vec<_>>());
        let ch = hints(src, src.len(), Dialect::ClickHouse, &r, &[], Case::Upper, false, true).unwrap();
        assert!(ch.items.iter().any(|i| i.insert == "`sales`.invoices"));
        let s = "select * from sal";
        let schema = hints(s, s.len(), Dialect::Postgres, &r, &[], Case::Upper, false, true).unwrap();
        assert!(schema.items.iter().any(|i| i.insert == "\"sales\""), "{:?}", schema.items.iter().map(|i| &i.insert).collect::<Vec<_>>());
    }


    #[test]
    fn clickhouse_default_database() {
        let r = vec![Relation { schema: "default".into(), name: "orders".into(), cols: vec![("id".into(), "UInt64".into()), ("total".into(), "Float64".into())], qualify: true }];
        let ins = |src: &str, at: usize| hints(src, at, Dialect::ClickHouse, &r, &[], Case::Upper, false, false).map(|h| h.items.iter().map(|i| i.insert.clone()).collect::<Vec<_>>()).unwrap_or_default();
        // Put in bare: `default` is a fine name there.
        let src = "select * from ord";
        assert!(ins(src, src.len()).contains(&"default.orders".to_string()), "{:?}", ins(src, src.len()));
        let src = "select * from def";
        assert!(ins(src, src.len()).contains(&"default".to_string()), "{:?}", ins(src, src.len()));
        // Written bare, the statement's hints still work.
        let src = "select o. from default.orders o";
        assert!(ins(src, 9).contains(&"total".to_string()), "{:?}", ins(src, 9));
        let src = "select * from default.";
        assert!(ins(src, src.len()).contains(&"orders".to_string()), "{:?}", ins(src, src.len()));
    }

}

#[cfg(test)]
mod bare_tests {
    use super::*;

    #[test]
    fn needless_quotes_off() {
        let ora = "CREATE TABLE \"SHOP\".\"ORDERS\" (\"ID\" NUMBER, \"lower\" VARCHAR2(5), \"TWO WORDS\" DATE, \"DATE\" DATE, \"V$X#1\" INT, \"_A\" INT)";
        assert_eq!(bare_names(ora, Dialect::Oracle), "CREATE TABLE SHOP.ORDERS (ID NUMBER, \"lower\" VARCHAR2(5), \"TWO WORDS\" DATE, \"DATE\" DATE, V$X#1 INT, \"_A\" INT)".replace("\"_A\"", "_A"));
        assert_eq!(bare_names("CREATE TABLE default.t (`id` UInt64, `my col` String, `default` Int8)", Dialect::ClickHouse), "CREATE TABLE default.t (id UInt64, `my col` String, `default` Int8)");
        assert_eq!(bare_names("select \"order_id\", \"Total\", 'a \"b\"' from \"x$y\"", Dialect::Postgres), "select order_id, \"Total\", 'a \"b\"' from x$y");
        assert_eq!(quote("x$", Dialect::ClickHouse), "`x$`");
        assert_eq!(quote("$x", Dialect::Postgres), "\"$x\"");
    }
}
