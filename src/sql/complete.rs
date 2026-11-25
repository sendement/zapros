//! Hints while typing: after "alias." its table's columns, after FROM /
//! JOIN tables, after JOIN … ON a join condition guessed from column names,
//! elsewhere columns of the tables in scope, keywords, functions and a few
//! snippets. Items are ranked: starts-with first, then contains.

use std::ops::Range;

use super::lex::{self, Tk, Tok};
use super::{Case, Dialect};
use crate::db::{Relation, Routine};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A routine's parameter by name (`p_id => `).
    Param,
    /// From the query's own SELECT (GROUP BY, ORDER BY), an aggregate (HAVING).
    Expr,
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
            Kind::Param => "≔",
            Kind::Expr => "≡",
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

/// A name as written in the statement (a word, or quoted with its quotes).
fn written(src: &str, t: &Tok) -> Option<String> {
    matches!(t.kind, Tk::Word | Tk::Keyword | Tk::Quoted).then(|| t.text(src).to_string())
}

/// A relation's column as it goes into the text.
pub fn col_text(rel: &Relation, c: &str, d: Dialect) -> String {
    if rel.written { c.to_string() } else { quote(c, d) }
}

/// A column's name as shown (quotes off).
pub fn col_label(c: &str) -> String {
    if c.starts_with(['"', '`', '[']) && c.len() >= 2 { super::unquote(c).to_string() } else { c.to_string() }
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
                    names = toks[k + 1..c].iter().filter_map(|t| written(src, t)).collect();
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
                        names = toks[k + 1..e].iter().filter_map(|t| written(src, t)).collect();
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
        out.push((Relation { schema: String::new(), name, cols, qualify: false, written: true }, cte));
    }
    out
}

/// The columns a query gives: its first SELECT's list (names as given,
/// `*` and `t.*` opened up), with their types when they're plain columns.
fn out_cols(body: &str, d: Dialect, made: &[Relation], bases: &[&[Relation]]) -> Vec<(String, String)> {
    out_cols_of(body, d, made, bases, false)
}

/// The columns a whole query gives, as a board draws them: its CTEs and
/// subqueries known, `*` opened up, an expression with no name as written.
/// (Tables looked for in these lists, the first first.)
pub fn query_columns_in(sql: &str, d: Dialect, bases: &[&[Relation]]) -> Vec<(String, String)> {
    let made: Vec<Relation> = derived(sql, d, bases).into_iter().map(|(r, _)| r).collect();
    out_cols_of(sql, d, &made, bases, true).into_iter().map(|(n, ty)| (col_label(&n), ty)).collect()
}

fn out_cols_of(body: &str, d: Dialect, made: &[Relation], bases: &[&[Relation]], exprs: bool) -> Vec<(String, String)> {
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
            .find_map(|rel| rel.cols.iter().find(|(c, _)| col_label(c).eq_ignore_ascii_case(col)).map(|(_, ty)| ty.clone()))
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
                    out.extend(rel.cols.iter().map(|(c, ty)| (col_text(rel, c, d), ty.clone())));
                }
            }
            continue;
        }
        let as_written = || {
            let s = &body[it[0].start..last.end];
            let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if s.chars().count() > 40 { format!("{}…", s.chars().take(39).collect::<String>()) } else { s }
        };
        let Some(name) = written(body, last) else {
            if exprs {
                out.push((as_written(), String::new()));
            }
            continue;
        };
        let plain = it.len() == 1;
        let dotted = prev.is_some_and(|p| p.kind == Tk::Dot);
        let as_ = prev.is_some_and(|p| up(p) == "AS");
        // `expr alias`: a name after something that ends an expression.
        let bare = matches!(last.kind, Tk::Word | Tk::Quoted) && prev.is_some_and(|p| matches!(p.kind, Tk::Close | Tk::Word | Tk::Quoted | Tk::Number | Tk::Str));
        if !(plain || dotted || as_ || bare) {
            if exprs {
                out.push((as_written(), String::new()));
            }
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

/// Aggregate functions (HAVING's, and what GROUP BY leaves out).
const AGGS: &[&str] = &["COUNT", "SUM", "AVG", "MIN", "MAX", "ARRAY_AGG", "STRING_AGG", "LISTAGG", "GROUP_CONCAT", "STDDEV", "VARIANCE", "BOOL_AND", "BOOL_OR", "EVERY", "JSON_AGG", "JSONB_AGG", "XMLAGG", "MEDIAN", "UNIQ", "UNIQEXACT", "ANY", "ANYLAST", "ARGMIN", "ARGMAX", "GROUPARRAY", "GROUPUNIQARRAY", "QUANTILE", "COUNTIF", "SUMIF", "AVGIF"];

/// The clause the cursor's in at its query's level (after `before`, the
/// statement's tokens up to it): "GROUP BY", "HAVING", "ORDER BY"…; and
/// where that level's SELECT is (an index into `before`).
fn clause_here(src: &str, before: &[Tok]) -> (String, Option<usize>) {
    let mut depth = 0;
    let mut clause = String::new();
    let mut select = None;
    for k in (0..before.len()).rev() {
        let t = &before[k];
        match t.kind {
            Tk::Close => depth += 1,
            Tk::Open if depth == 0 => break,
            Tk::Open => depth -= 1,
            Tk::Keyword | Tk::Word if depth == 0 => {
                let u = t.text(src).to_ascii_uppercase();
                if clause.is_empty() {
                    if u == "BY" && k > 0 {
                        let p = before[k - 1].text(src).to_ascii_uppercase();
                        if matches!(p.as_str(), "GROUP" | "ORDER" | "PARTITION") {
                            clause = format!("{p} BY");
                        }
                    } else if matches!(u.as_str(), "SELECT" | "FROM" | "WHERE" | "HAVING" | "ON" | "SET" | "VALUES" | "LIMIT" | "JOIN" | "USING" | "QUALIFY" | "PREWHERE" | "WINDOW") {
                        clause = u.clone();
                    }
                }
                if u == "SELECT" {
                    select = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    (clause, select)
}

/// A SELECT list's items (from the token after SELECT): (expression,
/// alias, an aggregate in it). `*` and `t.*` left out.
fn select_items(src: &str, toks: &[Tok]) -> Vec<(String, Option<String>, bool)> {
    let up = |t: &Tok| t.text(src).to_ascii_uppercase();
    let mut k = 0;
    while toks.get(k).is_some_and(|t| matches!(up(t).as_str(), "DISTINCT" | "ALL")) {
        k += 1;
    }
    let mut items: Vec<&[Tok]> = Vec::new();
    let (mut from, mut depth) = (k, 0);
    let mut end = toks.len();
    for (j, t) in toks.iter().enumerate().skip(k) {
        match t.kind {
            Tk::Open => depth += 1,
            Tk::Close if depth == 0 => {
                end = j;
                break;
            }
            Tk::Close => depth -= 1,
            Tk::Comma if depth == 0 => {
                items.push(&toks[from..j]);
                from = j + 1;
            }
            Tk::Keyword if depth == 0 && matches!(up(t).as_str(), "FROM" | "INTO" | "WHERE" | "GROUP" | "ORDER" | "HAVING" | "LIMIT" | "UNION") => {
                end = j;
                break;
            }
            _ => {}
        }
    }
    if from < end {
        items.push(&toks[from..end]);
    }
    let mut out = Vec::new();
    for it in items {
        let Some(last) = it.last() else { continue };
        if last.text(src) == "*" {
            continue;
        }
        let prev = it.len().checked_sub(2).map(|k| &it[k]);
        let as_ = prev.is_some_and(|p| up(p) == "AS");
        let bare = it.len() > 1 && matches!(last.kind, Tk::Word | Tk::Quoted) && prev.is_some_and(|p| matches!(p.kind, Tk::Close | Tk::Word | Tk::Quoted | Tk::Number | Tk::Str)) && !prev.is_some_and(|p| p.kind == Tk::Dot);
        let (expr, alias) = if as_ && it.len() > 2 {
            (&it[..it.len() - 2], ident_text(src, last))
        } else if bare {
            (&it[..it.len() - 1], ident_text(src, last))
        } else {
            (it, None)
        };
        let (Some(a), Some(b)) = (expr.first(), expr.last()) else { continue };
        let text = src[a.start..b.end].split_whitespace().collect::<Vec<_>>().join(" ");
        let agg = expr.windows(2).any(|w| w[1].kind == Tk::Open && matches!(w[0].kind, Tk::Word | Tk::Keyword) && AGGS.contains(&up(&w[0]).as_str()));
        out.push((text, alias, agg));
    }
    out
}

/// In a routine's call at an argument's start: (its qualifier — package
/// or schema —, its name, the parameters already named).
fn call_here(src: &str, before: &[Tok]) -> Option<(Option<String>, String, Vec<String>)> {
    let last = before.last()?;
    if !matches!(last.kind, Tk::Open | Tk::Comma) {
        return None;
    }
    let mut depth = 0;
    let mut open = None;
    for k in (0..before.len()).rev() {
        match before[k].kind {
            Tk::Close => depth += 1,
            Tk::Open if depth == 0 => {
                open = Some(k);
                break;
            }
            Tk::Open => depth -= 1,
            _ => {}
        }
    }
    let o = open?;
    let name_t = before.get(o.checked_sub(1)?)?;
    if !matches!(name_t.kind, Tk::Word | Tk::Quoted | Tk::Keyword) {
        return None;
    }
    let name = ident_text(src, name_t)?;
    let qual = (o >= 3 && before[o - 2].kind == Tk::Dot).then(|| ident_text(src, &before[o - 3])).flatten();
    let mut given = Vec::new();
    let mut depth = 0;
    for k in o + 1..before.len() {
        match before[k].kind {
            Tk::Open => depth += 1,
            Tk::Close => depth -= 1,
            Tk::Word | Tk::Quoted | Tk::Keyword if depth == 0 && before.get(k + 1).is_some_and(|t| t.text(src).starts_with("=>")) => {
                given.extend(ident_text(src, &before[k]));
            }
            _ => {}
        }
    }
    Some((qual, name, given))
}

/// The routines a call may be: by name, in the package or schema named.
pub fn routines_named<'a>(routines: &'a [Routine], qual: Option<&str>, name: &str) -> Vec<&'a Routine> {
    routines
        .iter()
        .filter(|r| r.name.eq_ignore_ascii_case(name))
        .filter(|r| match qual {
            Some(q) => r.package.eq_ignore_ascii_case(q) || (r.package.is_empty() && r.schema.eq_ignore_ascii_case(q)),
            None => r.package.is_empty(),
        })
        .collect()
}

/// Ctrl+click on a `*` of a SELECT list: what it stands for — the range to
/// replace (with `t.` when there), the columns (name, type, whose) and
/// whether they need their table's name (more than one table).
pub fn star_at(src: &str, at: usize, d: Dialect, rels: &[Relation], lent: &[Relation]) -> Option<(Range<usize>, Vec<(String, String, String)>, bool)> {
    let toks: Vec<Tok> = lex::lex(src, d).into_iter().filter(|t| !t.trivia()).collect();
    let k = toks.iter().position(|t| t.start <= at && at < t.end.max(t.start + 1) && t.text(src) == "*")?;
    let prev = k.checked_sub(1).map(|j| &toks[j]);
    let up = |t: &Tok| t.text(src).to_ascii_uppercase();
    let alias = if prev.is_some_and(|p| p.kind == Tk::Dot) && k >= 2 { ident_text(src, &toks[k - 2]) } else { None };
    let item_start = prev.is_none_or(|p| p.kind == Tk::Comma || matches!(up(p).as_str(), "SELECT" | "DISTINCT" | "ALL"));
    if alias.is_none() && !item_start {
        return None;
    }
    let stmt = super::statement_at(src, at, d, true).unwrap_or(0..src.len());
    let stmt_text = &src[stmt.clone()];
    // Its query's level: inside the innermost bracket around it.
    let rel_at = at - stmt.start;
    let st: Vec<Tok> = lex::lex(stmt_text, d).into_iter().filter(|t| !t.trivia()).collect();
    let mut opens = Vec::new();
    let mut level = 0..stmt_text.len();
    for t in &st {
        if t.start >= rel_at {
            break;
        }
        match t.kind {
            Tk::Open => opens.push(t.end),
            Tk::Close => {
                opens.pop();
            }
            _ => {}
        }
    }
    if let Some(&o) = opens.last() {
        level.start = o;
        let mut depth = 0;
        for t in st.iter().filter(|t| t.start >= o) {
            match t.kind {
                Tk::Open => depth += 1,
                Tk::Close if depth == 0 => {
                    level.end = t.start;
                    break;
                }
                Tk::Close => depth -= 1,
                _ => {}
            }
        }
    }
    // (Brackets inside it blanked: their tables aren't its.)
    let mut body: Vec<u8> = stmt_text[level.clone()].as_bytes().to_vec();
    let mut depth = 0;
    for t in lex::lex(&stmt_text[level.clone()], d) {
        match t.kind {
            Tk::Open => {
                depth += 1;
                continue;
            }
            Tk::Close => {
                depth -= 1;
                continue;
            }
            _ => {}
        }
        if depth > 0 {
            for b in &mut body[t.start..t.end] {
                if *b != b'\n' {
                    *b = b' ';
                }
            }
        }
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let refs = scope(&body, d);
    let made: Vec<Relation> = derived(stmt_text, d, &[lent, rels]).into_iter().map(|(r, _)| r).collect();
    let look = |r: &Ref| find(&made, r).or_else(|| find(lent, r)).or_else(|| find(rels, r));
    let mut cols = Vec::new();
    let mut tables = 0;
    for r in refs.iter().filter(|r| alias.as_deref().is_none_or(|a| r.alias.eq_ignore_ascii_case(a))) {
        if let Some(rel) = look(r) {
            tables += 1;
            cols.extend(rel.cols.iter().map(|(c, ty)| (col_text(rel, c, d), ty.clone(), r.alias.clone())));
        }
    }
    if cols.is_empty() {
        return None;
    }
    let range = if alias.is_some() { toks[k - 2].start..toks[k].end } else { toks[k].start..toks[k].end };
    Some((range, cols, alias.is_some() || tables > 1))
}

/// What's most likely typed next at `at` (shown dimmed after the cursor,
/// Tab takes it): the rest of it after what's typed of it. In GROUP BY the
/// SELECT's own expressions not grouped by yet, in ORDER BY its next alias,
/// in HAVING its aggregate, after ON a join's condition, in a call its next
/// parameter, in INSERT INTO t ( its columns.
pub fn ghost(src: &str, at: usize, d: Dialect, rels: &[Relation], lent: &[Relation], routines: &[Routine]) -> Option<String> {
    let at = at.min(src.len());
    if !src.is_char_boundary(at) {
        return None;
    }
    // Only at a line's end (or before a closing bracket).
    let rest = src[at..].split('\n').next().unwrap_or("");
    if !(rest.trim().is_empty() || rest.trim_start().starts_with(')')) {
        return None;
    }
    let toks = lex::lex(src, d);
    if toks.iter().any(|t| t.start < at && (at < t.end || t.open && at <= t.end) && matches!(t.kind, Tk::Str | Tk::LineComment | Tk::BlockComment | Tk::Quoted)) {
        return None;
    }
    // (What's typed of it: dotted names too, "u.na".)
    let start = src[..at].char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || matches!(*c, '_' | '$' | '.' | '"')).last().map_or(at, |(i, _)| i);
    let prefix = &src[start..at];
    let stmt = super::statement_at(src, at, d, true).unwrap_or(0..src.len());
    let head = &src[stmt.start..start];
    let before: Vec<Tok> = lex::lex(head, d).into_iter().filter(|t| !t.trivia()).collect();
    let last = before.last()?;
    let prev = last.text(head).to_ascii_uppercase();
    let norm = |s: &str| s.split_whitespace().collect::<String>().to_lowercase();
    let mut guess: Option<String> = None;
    // A call's next parameter.
    if let Some((q, name, given)) = call_here(head, &before) {
        guess = routines_named(routines, q.as_deref(), &name).into_iter().flat_map(|r| r.args.iter()).find(|a| !a.name.is_empty() && !given.iter().any(|g| g.eq_ignore_ascii_case(&a.name))).map(|a| format!("{} => ", quote(&a.name, d)));
        // INSERT INTO t (: its columns.
        if guess.is_none() && last.kind == Tk::Open && before.len() >= 3 {
            let k = before.len() - 1;
            let into = before[..k].iter().rposition(|t| t.text(head).eq_ignore_ascii_case("INTO"));
            if let Some(i) = into.filter(|&i| i + 1 < k && i + 4 >= k) {
                let parts: Vec<String> = before[i + 1..k].iter().filter_map(|t| ident_text(head, t)).collect();
                let r = match parts.as_slice() {
                    [t] => Some(Ref { alias: t.clone(), schema: None, table: t.clone() }),
                    [s, t] => Some(Ref { alias: t.clone(), schema: Some(s.clone()), table: t.clone() }),
                    _ => None,
                };
                if let Some(rel) = r.and_then(|r| find(rels, &r)) {
                    guess = Some(rel.cols.iter().map(|(c, _)| col_text(rel, c, d)).collect::<Vec<_>>().join(", "));
                }
            }
        }
    }
    if guess.is_none() {
        let (clause, select) = clause_here(head, &before);
        let sel = select.map(|k| select_items(head, &before[k + 1..])).unwrap_or_default();
        // What the clause has so far.
        let so_far = |kw: &str| -> Vec<String> {
            let up = head.to_ascii_uppercase();
            up.rfind(kw).map(|i| head[i + kw.len()..].split(',').map(norm).filter(|x| !x.is_empty()).collect()).unwrap_or_default()
        };
        let fresh = matches!(prev.as_str(), "BY" | "HAVING" | ",") && clause != "SELECT";
        match clause.as_str() {
            "GROUP BY" if fresh => {
                let had = so_far("GROUP BY");
                let left: Vec<&str> = sel.iter().filter(|(e, _, agg)| !agg && !had.contains(&norm(e))).map(|(e, _, _)| e.as_str()).collect();
                if !left.is_empty() {
                    guess = Some(left.join(", "));
                }
            }
            "ORDER BY" if fresh => {
                let had = so_far("ORDER BY");
                guess = sel.iter().map(|(e, a, _)| a.clone().unwrap_or_else(|| e.clone())).find(|x| !had.contains(&norm(x)));
            }
            "HAVING" if prev == "HAVING" => guess = sel.iter().find(|(_, _, agg)| *agg).map(|(e, _, _)| format!("{e} ")),
            _ => {}
        }
        // After ON: the join's condition.
        if guess.is_none() && prev == "ON" {
            let stmt_text = &src[stmt.start..stmt.end.max(at)];
            let refs = scope(stmt_text, d);
            let made: Vec<Relation> = derived(stmt_text, d, &[lent, rels]).into_iter().map(|(r, _)| r).collect();
            let look = |r: &Ref| find(&made, r).or_else(|| find(lent, r)).or_else(|| find(rels, r));
            // (Only the tables joined so far.)
            let n = scope(head, d).len();
            guess = join_guesses(look, &refs[..n.min(refs.len())], d).into_iter().next().map(|i| i.insert);
        }
    }
    let g = guess?;
    if g.len() <= prefix.len() || !g.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(prefix)) {
        return None;
    }
    Some(g[prefix.len()..].to_string())
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
    // ClickHouse takes nearly any word as a name; not these, where a
    // clause or an expression would be read instead.
    const CH: &[&str] = &[
        "ALL", "AND", "ANY", "AS", "ASC", "BETWEEN", "BY", "CASE", "CAST", "DESC", "DISTINCT", "ELSE", "END", "EXCEPT", "FALSE", "FINAL", "FROM", "GLOBAL", "GROUP", "HAVING", "IN", "INDEX", "INTERSECT", "INTO", "IS", "JOIN", "LIKE",
        "ILIKE", "LIMIT", "NOT", "NULL", "OFFSET", "ON", "OR", "ORDER", "PREWHERE", "PRIMARY", "PROJECTION", "CONSTRAINT", "SAMPLE", "SELECT", "SETTINGS", "THEN", "TRUE", "UNION", "USING", "WHEN", "WHERE", "WITH",
    ];
    // SQLite's words, so far as they'd clash as names.
    const LITE: &[&str] = &[
        "ADD", "ALL", "ALTER", "AND", "AS", "AUTOINCREMENT", "BETWEEN", "CASE", "CHECK", "COLLATE", "COMMIT", "CONSTRAINT", "CREATE", "CROSS", "DEFAULT", "DEFERRABLE", "DELETE", "DISTINCT", "DROP", "ELSE", "ESCAPE", "EXCEPT", "EXISTS",
        "FOREIGN", "FROM", "GROUP", "HAVING", "IN", "INDEX", "INNER", "INSERT", "INTERSECT", "INTO", "IS", "ISNULL", "JOIN", "LEFT", "LIMIT", "NATURAL", "NOT", "NOTNULL", "NULL", "ON", "OR", "ORDER", "OUTER", "PRIMARY", "REFERENCES",
        "RIGHT", "SELECT", "SET", "TABLE", "THEN", "TO", "TRANSACTION", "UNION", "UNIQUE", "UPDATE", "USING", "VALUES", "WHEN", "WHERE",
    ];
    let up = name.to_ascii_uppercase();
    match d {
        Dialect::Oracle => ORA.contains(&up.as_str()),
        Dialect::Postgres => PG.contains(&up.as_str()),
        Dialect::Sqlite => LITE.contains(&up.as_str()),
        Dialect::Generic => PG.contains(&up.as_str()) || ORA.contains(&up.as_str()),
        Dialect::ClickHouse => CH.contains(&up.as_str()),
    }
}

/// A name as it must be written (quoted when it'd fold or clash).
pub fn quote(name: &str, d: Dialect) -> String {
    // (Only words the database won't take bare: the highlighter's
    // keywords — TYPE, COMMENT, KEY, ClickHouse's SYSTEM, QUERY… — make
    // fine names.)
    let keyword = reserved(name, d);
    // `$` (and Oracle's `#`) go bare after the first letter where the
    // database takes them so.
    let also = |c: char| match d {
        Dialect::Oracle => c == '$' || c == '#',
        Dialect::Postgres | Dialect::Sqlite => c == '$',
        _ => false,
    };
    // (ClickHouse reads only Latin letters bare.)
    let letter = |c: char| c.is_alphanumeric() && (d != Dialect::ClickHouse || c.is_ascii());
    let plain = !name.is_empty() && name.chars().all(|c| letter(c) || c == '_' || also(c)) && name.starts_with(|c: char| (c.is_alphabetic() && letter(c)) || c == '_') && !keyword;
    let folds = match d {
        Dialect::Postgres => name.chars().any(|c| c.is_uppercase()),
        Dialect::Oracle => name.chars().any(|c| c.is_lowercase()),
        _ => false,
    };
    if plain && !folds {
        name.to_string()
    } else if d == Dialect::ClickHouse {
        format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`"))
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
                    let text = format!("{}.{} = {}.{}", quote(&last.alias, d), col_text(b, bc, d), quote(&o.alias, d), col_text(a, ac, d));
                    out.push(Item { label: text.clone(), insert: text, kind: Kind::Join, detail: format!("{} ⋈ {}", b.name, a.name) });
                }
            }
        }
    }
    out
}

/// Hints at `at` (None inside strings and comments, or with nothing to say).
/// (`lent`: relations from elsewhere, a notebook's CTEs from cells above.)
pub fn hints(src: &str, at: usize, d: Dialect, rels: &[Relation], lent: &[Relation], routines: &[Routine], case: Case, forced: bool, quote_schemas: bool) -> Option<Hints> {
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
    let stmt = super::statement_at(src, at, d, true).unwrap_or(0..src.len());
    // A routine's call at an argument's start: its parameters by name.
    let call = if qualifier.is_none() {
        let b: Vec<Tok> = lex::lex(&src[stmt.start..start], d).into_iter().filter(|t| !t.trivia()).collect();
        call_here(&src[stmt.start..start], &b)
    } else {
        None
    };
    let mut params: Vec<Item> = Vec::new();
    if let Some((q, name, given)) = &call {
        for r in routines_named(routines, q.as_deref(), name) {
            for a in &r.args {
                if a.name.is_empty() || given.iter().any(|g| g.eq_ignore_ascii_case(&a.name)) || params.iter().any(|p| p.label.eq_ignore_ascii_case(&a.name)) {
                    continue;
                }
                let mut detail = a.ty.clone();
                if !a.mode.is_empty() && a.mode != "IN" {
                    detail = format!("{} {detail}", a.mode);
                }
                if a.default {
                    detail += crate::i18n::t(" · есть по умолчанию", " · has a default");
                }
                params.push(Item { label: a.name.clone(), insert: format!("{} => $0", quote(&a.name, d)), kind: Kind::Param, detail });
            }
        }
    }
    if prefix.is_empty() && qualifier.is_none() && !forced && params.is_empty() {
        return None;
    }
    // (Just opened: only its parameters.)
    if prefix.is_empty() && !forced && !params.is_empty() {
        return Some(Hints { range: start..at, items: params });
    }
    let stmt_text = &src[stmt.start..stmt.end.max(at)];
    let refs = scope(stmt_text, d);
    // Its own CTEs and subqueries, then what's lent, then the database's.
    let made: Vec<(Relation, bool)> = derived(stmt_text, d, &[lent, rels]);
    let own: Vec<Relation> = made.iter().map(|(r, _)| r.clone()).collect();
    let look = |r: &Ref| find(&own, r).or_else(|| find(lent, r)).or_else(|| find(rels, r));
    let ctes = || made.iter().filter(|(_, c)| *c).map(|(r, _)| r).chain(lent).map(|r| Item { label: r.name.clone(), insert: quote(&r.name, d), kind: Kind::Table, detail: format!("WITH · {}", r.cols.len()) });
    let before: Vec<Tok> = lex::lex(&src[stmt.start..start], d).into_iter().filter(|t| !t.trivia()).collect();
    let prev = before.last().map(|t| t.text(&src[stmt.start..start]).to_ascii_uppercase()).unwrap_or_default();
    let mut items: Vec<Item> = params;
    // GROUP BY, ORDER BY: the query's own SELECT first; HAVING: aggregates.
    let (clause, select) = clause_here(&src[stmt.start..start], &before);
    if qualifier.is_none() {
        let sel = select.map(|k| select_items(&src[stmt.start..start], &before[k + 1..])).unwrap_or_default();
        let from_select = crate::i18n::t("из SELECT", "from SELECT");
        match clause.as_str() {
            "GROUP BY" => items.extend(sel.iter().filter(|(_, _, agg)| !agg).map(|(e, _, _)| Item { label: e.clone(), insert: e.clone(), kind: Kind::Expr, detail: from_select.into() })),
            "ORDER BY" => items.extend(sel.iter().map(|(e, a, _)| {
                let x = a.as_ref().map_or_else(|| e.clone(), |a| quote(a, d));
                Item { label: x.clone(), insert: x, kind: Kind::Expr, detail: if a.is_some() { format!("{from_select}: {e}") } else { from_select.into() } }
            })),
            "HAVING" => {
                let agg = crate::i18n::t("агрегат", "aggregate");
                items.extend(sel.iter().filter(|(_, _, agg)| *agg).map(|(e, _, _)| Item { label: e.clone(), insert: e.clone(), kind: Kind::Expr, detail: from_select.into() }));
                for f in ["COUNT(*)", "COUNT(DISTINCT $0)", "SUM($0)", "AVG($0)", "MIN($0)", "MAX($0)"] {
                    let f = if d == Dialect::ClickHouse { f.to_lowercase().replace("distinct", "DISTINCT") } else { case.apply(f) };
                    items.push(Item { label: f.replace("$0", "…"), insert: f, kind: Kind::Expr, detail: agg.into() });
                }
            }
            _ => {}
        }
    }
    let col = |rel: &Relation, name: &str, ty: &str, owner: &str| Item { label: col_label(name), insert: col_text(rel, name, d), kind: Kind::Column, detail: format!("{ty} · {owner}") };
    if let Some(q) = &qualifier {
        // alias. or table. → columns; schema. → its tables.
        if let Some(r) = refs.iter().find(|r| r.alias.eq_ignore_ascii_case(q)).cloned().or_else(|| Some(Ref { alias: q.clone(), schema: None, table: q.clone() })) {
            if let Some(rel) = look(&r) {
                items.extend(rel.cols.iter().map(|(c, ty)| col(rel, c, ty, &rel.name)));
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
                items.extend(rel.cols.iter().map(|(c, ty)| col(rel, c, ty, &r.alias)));
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
            // What the place itself suggests goes first: its own that start
            // with what's typed, then its own that hold it, then the rest.
            let context = matches!(it.kind, Kind::Param | Kind::Expr | Kind::Join);
            let rank = if context { s } else { s + 2 };
            // Snippets only by their exact start; keywords need a letter typed.
            if (it.kind == Kind::Snippet && s != 0 || matches!(it.kind, Kind::Keyword | Kind::Function | Kind::Snippet) && p.is_empty()) && qualifier.is_none() && !(forced && it.kind != Kind::Snippet) {
                return None;
            }
            // The word itself, typed out.
            if l == p && it.kind == Kind::Keyword {
                return None;
            }
            Some((rank, it))
        })
        .collect();
    // Columns, the SELECT's own and parameters keep their order; the rest
    // go shortest first.
    scored.sort_by(|a, b| {
        let o = a.0.cmp(&b.0).then(a.1.kind.cmp(&b.1.kind));
        if matches!(a.1.kind, Kind::Column | Kind::Expr | Kind::Param) { o } else { o.then(a.1.label.len().cmp(&b.1.label.len())).then(a.1.label.cmp(&b.1.label)) }
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
            Relation { schema: "public".into(), name: "users".into(), cols: vec![("id".into(), "int".into()), ("name".into(), "text".into())], qualify: false, written: false },
            Relation { schema: "public".into(), name: "orders".into(), cols: vec![("id".into(), "int".into()), ("user_id".into(), "int".into()), ("total".into(), "numeric".into())], qualify: false, written: false },
        ]
    }

    fn labels_at(src: &str, at: &str, lent: &[Relation]) -> Vec<(String, String)> {
        let at = src.find(at).unwrap() + at.len();
        hints(src, at, Dialect::Postgres, &rels(), lent, &[], Case::Upper, true, false).map(|h| h.items.into_iter().filter(|i| i.kind == Kind::Column || i.kind == Kind::Table).map(|i| (i.label, i.detail)).collect()).unwrap_or_default()
    }

    fn items_at(src: &str, at: &str, routines: &[Routine], forced: bool) -> Vec<Item> {
        let at = src.find(at).unwrap() + at.len();
        hints(src, at, Dialect::Postgres, &rels(), &[], routines, Case::Upper, forced, false).map(|h| h.items).unwrap_or_default()
    }

    #[test]
    fn clauses_hint_their_own() {
        let src = "select u.name, count(*) as n, sum(o.total) total from users u join orders o on o.user_id = u.id group by  order by ";
        // GROUP BY: what the SELECT has, its aggregates left out.
        let h = items_at(src, "group by ", &[], true);
        let exprs: Vec<&str> = h.iter().filter(|i| i.kind == Kind::Expr).map(|i| i.label.as_str()).collect();
        assert_eq!(exprs, ["u.name"], "{h:?}");
        assert_eq!(h[0].label, "u.name");
        // ORDER BY: aliases where given.
        let h = items_at(src, "order by ", &[], true);
        let exprs: Vec<&str> = h.iter().filter(|i| i.kind == Kind::Expr).map(|i| i.insert.as_str()).collect();
        assert_eq!(exprs, ["u.name", "n", "total"], "{h:?}");
        // HAVING: the aggregates first, then the usual ones; typed: by it.
        let src = "select u.name, count(*) as n from users u group by u.name having s";
        let h = items_at(src, "having s", &[], false);
        assert_eq!(h[0].label, "SUM(…)", "{h:?}");
        let src = "select u.name, count(*) as n from users u group by u.name having ";
        let h = items_at(src, "having ", &[], true);
        assert_eq!(h.iter().take(2).map(|i| i.label.as_str()).collect::<Vec<_>>(), ["count(*)", "COUNT(*)"], "{h:?}");
        // A subquery's own SELECT, not the outer one's.
        let src = "select a from (select b, max(c) m from t group by ) x";
        let h = items_at(src, "group by ", &[], true);
        assert_eq!(h.iter().filter(|i| i.kind == Kind::Expr).map(|i| i.label.as_str()).collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn ghosts_of_what_comes_next() {
        let g = |src: &str| ghost(src, src.len(), Dialect::Postgres, &rels(), &[], &[]);
        let src = "select u.name, o.user_id, count(*) as n from users u join orders o on o.user_id = u.id group by ";
        assert_eq!(g(src).as_deref(), Some("u.name, o.user_id"));
        // Typed some of it: the rest.
        assert_eq!(g(&format!("{src}u.na")).as_deref(), Some("me, o.user_id"));
        // One grouped already: the other.
        assert_eq!(g(&format!("{src}u.name, ")).as_deref(), Some("o.user_id"));
        // Something else typed: nothing.
        assert_eq!(g(&format!("{src}x")), None);
        assert_eq!(g("select u.name, count(*) as n from users u group by u.name having ").as_deref(), Some("count(*) "));
        assert_eq!(g("select u.name, count(*) as n from users u group by u.name order by ").as_deref(), Some("u.name"));
        assert_eq!(g("select * from users u join orders o on ").as_deref(), Some("o.user_id = u.id"));
        assert_eq!(g("insert into orders (").as_deref(), Some("id, user_id, total"));
        // Mid-line: nothing.
        assert_eq!(ghost("select a from t group by  -- x", 25, Dialect::Postgres, &rels(), &[], &[]), None);
        let r = vec![Routine { schema: "public".into(), package: String::new(), name: "f".into(), args: vec![crate::db::Arg { name: "p_a".into(), ..Default::default() }, crate::db::Arg { name: "p_b".into(), ..Default::default() }] }];
        assert_eq!(ghost("select f(p_a => 1, ", 19, Dialect::Postgres, &rels(), &[], &r).as_deref(), Some("p_b => "));
    }

    #[test]
    fn a_subquerys_columns_as_written() {
        // Oracle keeps customer_id as CUSTOMER_ID: written so, it needs no quotes.
        let ora = vec![Relation { schema: "SHOP".into(), name: "ORDERS".into(), cols: vec![("CUSTOMER_ID".into(), "NUMBER".into()), ("TOTAL".into(), "NUMBER".into())], qualify: false, written: false }];
        let src = "select s.customer_id, s.\"Sum\" from (select customer_id, sum(total) \"Sum\" from orders group by customer_id) s group by s.";
        let at = src.len();
        let h = hints(src, at, Dialect::Oracle, &ora, &[], &[], Case::Upper, true, false).unwrap();
        let cols: Vec<(&str, &str)> = h.items.iter().filter(|i| i.kind == Kind::Column).map(|i| (i.label.as_str(), i.insert.as_str())).collect();
        assert_eq!(cols, [("customer_id", "customer_id"), ("Sum", "\"Sum\"")]);
        // A * over a real table: its own names, quoted where they must be.
        let src = "select x.* from (select * from orders) x";
        let (_, c, _) = star_at(src, src.find('*').unwrap(), Dialect::Oracle, &ora, &[]).unwrap();
        assert_eq!(c.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["CUSTOMER_ID", "TOTAL"]);
    }

    #[test]
    fn calls_hint_their_parameters() {
        let r = vec![
            Routine { schema: "public".into(), package: String::new(), name: "add_order".into(), args: vec![crate::db::Arg { name: "p_user".into(), ty: "integer".into(), mode: "IN".into(), default: false }, crate::db::Arg { name: "p_total".into(), ty: "numeric".into(), mode: "IN".into(), default: true }] },
            Routine { schema: "SHOP".into(), package: "PKG".into(), name: "ADD_ORDER".into(), args: vec![crate::db::Arg { name: "P_X".into(), ty: "NUMBER".into(), mode: "IN OUT".into(), default: false }] },
        ];
        // Just opened: its parameters only, each to be named.
        let h = items_at("select add_order(", "add_order(", &r, false);
        assert_eq!(h.iter().map(|i| i.insert.as_str()).collect::<Vec<_>>(), ["p_user => $0", "p_total => $0"]);
        assert!(h[1].detail.contains("numeric"), "{h:?}");
        // Named already: not again.
        let h = items_at("select add_order(p_user => 1, ", "1, ", &r, false);
        assert_eq!(h.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["p_total"]);
        // A package's: by its name.
        let h = items_at("begin pkg.add_order( end;", "add_order(", &r, false);
        assert_eq!(h.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["P_X"]);
        assert!(h[0].detail.starts_with("IN OUT"));
        // Not a call of one known: nothing pops up.
        assert!(items_at("select count(", "count(", &r, false).is_empty());
    }

    #[test]
    fn star_stands_for_columns() {
        let src = "select * from users u join orders o on o.user_id = u.id";
        let (r, cols, q) = star_at(src, 7, Dialect::Postgres, &rels(), &[]).unwrap();
        assert_eq!(r, 7..8);
        assert_eq!(cols.iter().map(|c| format!("{}.{}", c.2, c.0)).collect::<Vec<_>>(), ["u.id", "u.name", "o.id", "o.user_id", "o.total"]);
        assert!(q);
        let src = "select o.*, 1 from orders o";
        let (r, cols, _) = star_at(src, 9, Dialect::Postgres, &rels(), &[]).unwrap();
        assert_eq!(&src[r], "o.*");
        assert_eq!(cols.len(), 3);
        // A subquery's own tables; count(*) isn't a list's.
        let src = "select x.* from (select * from users) x";
        let at = src.rfind('*').unwrap();
        let (_, cols, q) = star_at(src, at, Dialect::Postgres, &rels(), &[]).unwrap();
        assert_eq!((cols.len(), q), (2, false));
        assert!(star_at("select count(*) from users", 13, Dialect::Postgres, &rels(), &[]).is_none());
        assert!(star_at("select 2 * 3 from users", 9, Dialect::Postgres, &rels(), &[]).is_none());
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
        let h = hints(src, 9, Dialect::Postgres, &rels(), &[], &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), vec!["id", "name"]);
        let h = hints("select o. from orders o", 9, Dialect::Postgres, &rels(), &[], &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), vec!["id", "user_id", "total"]);
        let src = "select o.to from orders as o";
        let h = hints(src, 11, Dialect::Postgres, &rels(), &[], &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].label, "total");
        assert_eq!(h.range, 9..11);
    }

    #[test]
    fn tables_after_from_and_joins() {
        let src = "select * from us";
        let h = hints(src, src.len(), Dialect::Postgres, &rels(), &[], &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].label, "users");
        let src = "select * from users u join orders o on ";
        let h = hints(src, src.len(), Dialect::Postgres, &rels(), &[], &[], Case::Upper, true, false).unwrap();
        assert_eq!(h.items[0].insert, "o.user_id = u.id");
    }

    #[test]
    fn tables_off_the_search_path_keep_their_schema() {
        let mut r = rels();
        r.push(Relation { schema: "sales".into(), name: "invoices".into(), cols: vec![("id".into(), "int".into())], qualify: true, written: false });
        let src = "select * from inv";
        let h = hints(src, src.len(), Dialect::Postgres, &r, &[], &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].insert, "sales.invoices");
        // Its columns by alias all the same.
        let h = hints("select i. from sales.invoices i", 9, Dialect::Postgres, &r, &[], &[], Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].label, "id");
    }

    #[test]
    fn keywords_by_case_and_none_in_strings() {
        let h = hints("sel", 3, Dialect::Postgres, &[], &[], &[], Case::Lower, false, false).unwrap();
        assert!(h.items.iter().any(|i| i.insert == "select"));
        assert_eq!(h.items[0].kind, Kind::Snippet);
        assert!(hints("select 'fro", 11, Dialect::Postgres, &[], &[], &[], Case::Upper, false, false).is_none());
    }

    #[test]
    fn schemas_quoted_when_asked() {
        let r = vec![Relation { schema: "sales".into(), name: "invoices".into(), cols: vec![("id".into(), "int".into())], qualify: true, written: false }];
        let src = "select * from inv";
        let plain = hints(src, src.len(), Dialect::Postgres, &r, &[], &[], Case::Upper, false, false).unwrap();
        assert!(plain.items.iter().any(|i| i.insert == "sales.invoices"));
        let quoted = hints(src, src.len(), Dialect::Postgres, &r, &[], &[], Case::Upper, false, true).unwrap();
        assert!(quoted.items.iter().any(|i| i.insert == "\"sales\".invoices"), "{:?}", quoted.items.iter().map(|i| &i.insert).collect::<Vec<_>>());
        let ch = hints(src, src.len(), Dialect::ClickHouse, &r, &[], &[], Case::Upper, false, true).unwrap();
        assert!(ch.items.iter().any(|i| i.insert == "`sales`.invoices"));
        let s = "select * from sal";
        let schema = hints(s, s.len(), Dialect::Postgres, &r, &[], &[], Case::Upper, false, true).unwrap();
        assert!(schema.items.iter().any(|i| i.insert == "\"sales\""), "{:?}", schema.items.iter().map(|i| &i.insert).collect::<Vec<_>>());
    }


    #[test]
    fn clickhouse_default_database() {
        let r = vec![Relation { schema: "default".into(), name: "orders".into(), cols: vec![("id".into(), "UInt64".into()), ("total".into(), "Float64".into())], qualify: true, written: false }];
        let ins = |src: &str, at: usize| hints(src, at, Dialect::ClickHouse, &r, &[], &[], Case::Upper, false, false).map(|h| h.items.iter().map(|i| i.insert.clone()).collect::<Vec<_>>()).unwrap_or_default();
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
        // The highlighter's words that make fine names stay bare.
        for (n, d) in [("system", Dialect::ClickHouse), ("query", Dialect::ClickHouse), ("type", Dialect::ClickHouse), ("default", Dialect::ClickHouse), ("type", Dialect::Postgres), ("comment", Dialect::Postgres), ("key", Dialect::Postgres), ("date", Dialect::Sqlite), ("TYPE", Dialect::Oracle), ("NAME", Dialect::Oracle)] {
            assert_eq!(quote(n, d), n, "{d:?}");
        }
        for (n, d, q) in [("order", Dialect::ClickHouse, "`order`"), ("user", Dialect::Postgres, "\"user\""), ("LEVEL", Dialect::Oracle, "\"LEVEL\""), ("group", Dialect::Sqlite, "\"group\"")] {
            assert_eq!(quote(n, d), q);
        }
        assert_eq!(quote("$x", Dialect::Postgres), "\"$x\"");
    }
}
