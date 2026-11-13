//! Ctrl+click on a name in the code: where it leads — a place in the text
//! (an alias's table, a CTE, a function of the script) or an object of the
//! database (a table, a view, a function…).

use std::ops::Range;

use super::Dialect;
use super::lex::{self, Tk, Tok};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// In the text: this range picked.
    Here(Range<usize>),
    /// In the database (names as the database keeps them).
    Object { schema: Option<String>, name: String, call: bool },
}

/// A table named in a statement, with where its name and alias are.
#[derive(Clone, Debug)]
struct Named {
    alias: String,
    table: String,
    /// The table's name (with its schema) in the text.
    at: Range<usize>,
}

fn ident(src: &str, t: &Tok) -> Option<String> {
    match t.kind {
        Tk::Word | Tk::Keyword => Some(t.text(src).to_string()),
        Tk::Quoted => Some(super::unquote(t.text(src)).to_string()),
        _ => None,
    }
}

/// As the database keeps an unquoted name.
fn folded(t: &Tok, s: String, d: Dialect) -> String {
    if t.kind == Tk::Quoted {
        return s;
    }
    match d {
        Dialect::Postgres => s.to_lowercase(),
        Dialect::Oracle => s.to_uppercase(),
        _ => s,
    }
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
}

/// Tables after FROM / JOIN / UPDATE / INTO, with their aliases.
fn tables(src: &str, toks: &[Tok]) -> Vec<Named> {
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
        let Some(first) = toks.get(j).and_then(|t| ident(src, t)) else { continue };
        if toks[j].kind == Tk::Keyword {
            continue;
        }
        let (table, end) = match (toks.get(j + 1), toks.get(j + 2)) {
            (Some(dot), Some(t)) if dot.kind == Tk::Dot && ident(src, t).is_some() => (ident(src, t).unwrap(), j + 2),
            _ => (first, j),
        };
        if toks.get(end + 1).is_some_and(|t| t.kind == Tk::Open) {
            continue;
        }
        let mut a = end + 1;
        if up(a) == "AS" {
            a += 1;
        }
        let alias = toks.get(a).filter(|t| t.kind == Tk::Word || t.kind == Tk::Quoted).and_then(|t| ident(src, t)).unwrap_or_else(|| table.clone());
        out.push(Named { alias, table, at: toks[j].start..toks[end].end });
    }
    out
}

/// WITH name AS (…), name2 AS (…): the names and where they are.
fn ctes(src: &str, toks: &[Tok]) -> Vec<(String, Range<usize>)> {
    let up = |k: usize| toks.get(k).map(|t| t.text(src).to_ascii_uppercase()).unwrap_or_default();
    // Past the bracket opened at `k`.
    let skip = |mut k: usize| {
        let mut depth = 0;
        while k < toks.len() {
            match toks[k].kind {
                Tk::Open => depth += 1,
                Tk::Close => {
                    depth -= 1;
                    if depth == 0 {
                        return k + 1;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        k
    };
    let mut out = Vec::new();
    for k in 0..toks.len() {
        if up(k) != "WITH" {
            continue;
        }
        let mut j = k + 1;
        if up(j) == "RECURSIVE" {
            j += 1;
        }
        loop {
            let Some(name) = toks.get(j).filter(|t| t.kind != Tk::Keyword).and_then(|t| ident(src, t)) else { break };
            let at = toks[j].start..toks[j].end;
            j += 1;
            if toks.get(j).is_some_and(|t| t.kind == Tk::Open) {
                j = skip(j);
            }
            if up(j) != "AS" {
                break;
            }
            j += 1;
            if up(j) == "NOT" {
                j += 1;
            }
            if up(j) == "MATERIALIZED" {
                j += 1;
            }
            if !toks.get(j).is_some_and(|t| t.kind == Tk::Open) {
                break;
            }
            out.push((name, at));
            j = skip(j);
            if toks.get(j).is_some_and(|t| t.kind == Tk::Comma) {
                j += 1;
            } else {
                break;
            }
        }
    }
    out
}

/// Where the name at `at` leads. `has_col(table, column)`: what the
/// database knows of tables' columns.
pub fn follow(src: &str, at: usize, d: Dialect, has_col: &dyn Fn(&str, &str) -> bool) -> Option<(Range<usize>, Target)> {
    let all = lex::lex(src, d);
    let t = *all.iter().find(|t| t.start <= at && at < t.end)?;
    if !matches!(t.kind, Tk::Word | Tk::Quoted | Tk::Keyword) {
        return None;
    }
    let word = ident(src, &t)?;
    let stmt = super::statement_at(src, at, d, false).unwrap_or(0..src.len());
    let toks: Vec<Tok> = all.iter().copied().filter(|x| !x.trivia() && x.start >= stmt.start && x.end <= stmt.end).collect();
    let k = toks.iter().position(|x| x.start == t.start)?;
    let next = toks.get(k + 1).map(|x| x.kind);
    let qual = (k >= 2 && toks[k - 1].kind == Tk::Dot).then(|| ident(src, &toks[k - 2])).flatten();
    let qual_tok = (k >= 2).then(|| toks[k - 2]);
    let named = tables(src, &toks);
    let ctes = ctes(src, &toks);
    let me = t.start..t.end;
    let here = |r: &Range<usize>| Some((me.clone(), Target::Here(r.clone())));
    let by_alias = |a: &str| named.iter().find(|n| eq(&n.alias, a));
    let cte = |a: &str| ctes.iter().find(|(n, _)| eq(n, a));
    // A table of the statement: a CTE's definition, or its name there.
    let table_at = |n: &Named| cte(&n.table).map_or(n.at.clone(), |(_, r)| r.clone());
    // Keywords lead nowhere, unless a function's called (count(…) is the
    // database's; FROM (…), BY (…) are keywords still).
    if t.kind == Tk::Keyword && qual.is_none() && !(next == Some(Tk::Open) && super::is_function_in(&word, &super::functions(d))) {
        return None;
    }
    // A qualifier itself: alias / CTE → its table; else a table or schema.
    if next == Some(Tk::Dot) && qual.is_none() {
        if let Some(n) = by_alias(&word) {
            return here(&table_at(n));
        }
        if let Some((_, r)) = cte(&word) {
            return here(r);
        }
        return Some((me, Target::Object { schema: None, name: folded(&t, word, d), call: false }));
    }
    if let Some(q) = &qual {
        // alias.column → the alias's table.
        if let Some(n) = by_alias(q) {
            return here(&table_at(n));
        }
        if let Some((_, r)) = cte(q) {
            return here(r);
        }
        // schema.object (or package.member).
        let q = folded(&qual_tok.unwrap(), q.clone(), d);
        return Some((me, Target::Object { schema: Some(q), name: folded(&t, word, d), call: next == Some(Tk::Open) }));
    }
    // The alias where it's given, or used bare: its table.
    if let Some(n) = by_alias(&word).filter(|n| !eq(&n.alias, &n.table) || n.at.start != me.start) {
        if !n.at.contains(&me.start) {
            return here(&table_at(n));
        }
    }
    if let Some((_, r)) = cte(&word) {
        if *r != me {
            return here(r);
        }
    }
    // A function, procedure, cursor… of the script itself.
    if let Some(e) = super::outline::outline(src, d).into_iter().find(|e| eq(&e.name, &word) && !e.range.contains(&at) && !e.decl) {
        let r = src[e.at..].find(&*word).filter(|&i| i < 200).map_or(e.at..e.at + word.len(), |i| e.at + i..e.at + i + word.len());
        return here(&r);
    }
    // A column of a table of the statement: that table.
    if next != Some(Tk::Open) {
        if let Some(n) = named.iter().find(|n| has_col(&n.table, &word)) {
            return here(&table_at(n));
        }
    }
    Some((me, Target::Object { schema: None, name: folded(&t, word, d), call: next == Some(Tk::Open) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: &str = "with recent as (select * from orders where total > 10)\nselect c.name, r.total, note\n  from shop.customers c\n  join recent r on r.customer_id = c.id\n where vip(c.id)";

    fn go(src: &str, word: &str, nth: usize) -> Option<Target> {
        let at = src.match_indices(word).nth(nth).unwrap().0;
        follow(src, at, Dialect::Postgres, &|t, c| t == "orders" && c == "note").map(|x| x.1)
    }

    fn range(src: &str, s: &str) -> Range<usize> {
        let a = src.find(s).unwrap();
        a..a + s.len()
    }

    #[test]
    fn aliases_ctes_and_objects() {
        // An alias (used or where it's given) → its table.
        assert_eq!(go(Q, "c.name", 0), Some(Target::Here(range(Q, "shop.customers"))));
        assert_eq!(go(Q, "name", 0), Some(Target::Here(range(Q, "shop.customers"))));
        assert_eq!(go(Q, "c\n", 0), Some(Target::Here(range(Q, "shop.customers"))));
        // A CTE's alias → the CTE.
        assert_eq!(go(Q, "r.total", 0), Some(Target::Here(range(Q, "recent"))));
        assert_eq!(go(Q, "recent r", 0), Some(Target::Here(range(Q, "recent"))));
        // Tables and functions → the database.
        assert_eq!(go(Q, "orders", 0), Some(Target::Object { schema: None, name: "orders".into(), call: false }));
        assert_eq!(go(Q, "customers", 0), Some(Target::Object { schema: Some("shop".into()), name: "customers".into(), call: false }));
        assert_eq!(go(Q, "vip", 0), Some(Target::Object { schema: None, name: "vip".into(), call: true }));
        // A schema's name before the dot: that object (looked up by the app).
        assert_eq!(go(Q, "shop", 0), Some(Target::Object { schema: None, name: "shop".into(), call: false }));
        // Keywords lead nowhere.
        assert_eq!(go(Q, "where", 0), None);
    }

    #[test]
    fn columns_and_script_functions() {
        let s = "select note from orders o where total > 1";
        assert_eq!(go(s, "note", 0), Some(Target::Here(range(s, "orders"))));
        let p = "create or replace package body p as\n  function helper(x number) return number is begin return x; end;\n  procedure run is begin dbms_output.put_line(helper(1)); end;\nend p;";
        let at = p.find("helper(1)").unwrap() + 2;
        let t = follow(p, at, Dialect::Oracle, &|_, _| false).unwrap().1;
        let a = p.find("helper").unwrap();
        assert_eq!(t, Target::Here(a..a + 6));
        // Oracle folds unquoted names to upper case.
        let at = p.find("put_line").unwrap() + 1;
        assert_eq!(follow(p, at, Dialect::Oracle, &|_, _| false).unwrap().1, Target::Object { schema: Some("DBMS_OUTPUT".into()), name: "PUT_LINE".into(), call: true });
    }
}
