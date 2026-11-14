//! Aliases in a statement, for their own colours: a table's ("orders o",
//! "(select …) t", and "o." where it's used) and a column's ("sum(x) AS
//! total", "count(*) n", and "total" in ORDER BY / GROUP BY / HAVING).

use super::lex::{Tk, Tok};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alias {
    Table,
    Column,
}

fn upper(src: &str, t: &Tok) -> String {
    t.text(src).to_ascii_uppercase()
}

fn name(src: &str, t: &Tok) -> String {
    super::unquote(t.text(src)).to_lowercase()
}

/// The clause a keyword starts (JOIN for every kind of join).
fn clause_of(w: &str) -> Option<&'static str> {
    Some(match w {
        "SELECT" => "SELECT",
        "FROM" => "FROM",
        "JOIN" => "JOIN",
        "WHERE" | "PREWHERE" => "WHERE",
        "ON" | "USING" => "ON",
        "GROUP" => "GROUP",
        "ORDER" => "ORDER",
        "HAVING" => "HAVING",
        "QUALIFY" => "QUALIFY",
        "WINDOW" => "WINDOW",
        "LIMIT" | "OFFSET" | "FETCH" => "LIMIT",
        "SET" => "SET",
        "VALUES" => "VALUES",
        "UPDATE" => "UPDATE",
        "INTO" => "INTO",
        "UNION" | "INTERSECT" | "EXCEPT" | "MINUS" => "UNION",
        "RETURNING" => "RETURNING",
        _ => return None,
    })
}

/// The aliases among `toks` (all of a text's tokens, spaces too): the
/// tokens' indexes and what they are.
pub fn find(src: &str, toks: &[Tok]) -> Vec<(usize, Alias)> {
    // Spaces and comments aside: (index into toks, token).
    let code: Vec<(usize, Tok)> = toks.iter().copied().enumerate().filter(|(_, t)| !t.trivia()).collect();
    let mut out = Vec::new();
    let mut from = 0;
    while from < code.len() {
        // A statement: up to ";" (or Oracle's "/").
        let to = code[from..].iter().position(|(_, t)| matches!(t.kind, Tk::Semi | Tk::Slash)).map_or(code.len(), |p| from + p);
        statement(src, &code[from..to], &mut out);
        from = to + 1;
    }
    out
}

fn statement(src: &str, code: &[(usize, Tok)], out: &mut Vec<(usize, Alias)>) {
    let kind = |k: usize| code.get(k).map(|c| c.1.kind);
    let is_name = |k: usize| matches!(kind(k), Some(Tk::Word | Tk::Quoted));
    let kw = |k: usize, w: &str| code.get(k).is_some_and(|c| c.1.kind == Tk::Keyword && upper(src, &c.1) == w);
    let mut clause: Vec<&'static str> = vec![""];
    let mut tables: Vec<(usize, String)> = Vec::new();
    let mut columns: Vec<(usize, String)> = Vec::new();
    // A table's alias at `k` ([AS] name), if it's there.
    let alias_at = |k: usize| -> Option<usize> {
        let a = if kw(k, "AS") { k + 1 } else { k };
        (is_name(a) && !code.get(a + 1).is_some_and(|c| matches!(c.1.kind, Tk::Dot | Tk::Open))).then_some(a)
    };
    let mut k = 0;
    while k < code.len() {
        let t = code[k].1;
        let here = *clause.last().unwrap();
        match t.kind {
            Tk::Open => clause.push(""),
            Tk::Close => {
                clause.pop();
                if clause.is_empty() {
                    clause.push("");
                }
                // (subquery) t, generate_series(…) g, TABLE(…) t
                if matches!(*clause.last().unwrap(), "FROM" | "JOIN") {
                    if let Some(a) = alias_at(k + 1) {
                        tables.push((a, name(src, &code[a].1)));
                        k = a + 1;
                        continue;
                    }
                }
            }
            Tk::Keyword => {
                let w = upper(src, &t);
                if let Some(c) = clause_of(&w) {
                    *clause.last_mut().unwrap() = c;
                }
                // FROM / JOIN / UPDATE table [AS] alias
                if matches!(w.as_str(), "FROM" | "JOIN" | "UPDATE") {
                    let mut j = k + 1;
                    while kw(j, "ONLY") || kw(j, "LATERAL") {
                        j += 1;
                    }
                    if is_name(j) && kind(j + 1) != Some(Tk::Open) {
                        while kind(j + 1) == Some(Tk::Dot) && is_name(j + 2) {
                            j += 2;
                        }
                        if let Some(a) = alias_at(j + 1) {
                            tables.push((a, name(src, &code[a].1)));
                            k = a + 1;
                            continue;
                        }
                    }
                }
                // AS name in the select list.
                if w == "AS" && here == "SELECT" && is_name(k + 1) {
                    columns.push((k + 1, name(src, &code[k + 1].1)));
                    k += 2;
                    continue;
                }
            }
            Tk::Comma if matches!(here, "FROM" | "JOIN" | "ON") => {
                // FROM a x, b y (after a JOIN's ON too: a new item of FROM)
                *clause.last_mut().unwrap() = "FROM";
                let mut j = k + 1;
                if is_name(j) && kind(j + 1) != Some(Tk::Open) {
                    while kind(j + 1) == Some(Tk::Dot) && is_name(j + 2) {
                        j += 2;
                    }
                    if let Some(a) = alias_at(j + 1) {
                        tables.push((a, name(src, &code[a].1)));
                        k = a + 1;
                        continue;
                    }
                }
            }
            Tk::Word | Tk::Quoted if here == "SELECT" && k > 0 => {
                // An alias with no AS: right after an expression's end, then
                // a comma, FROM or the end.
                let prev = code[k - 1].1.kind;
                let ends = matches!(prev, Tk::Word | Tk::Quoted | Tk::Number | Tk::Str | Tk::Close) || (prev == Tk::Keyword && upper(src, &code[k - 1].1) == "END");
                let then = code.get(k + 1).is_none_or(|c| matches!(c.1.kind, Tk::Comma | Tk::Close) || (c.1.kind == Tk::Keyword && matches!(upper(src, &c.1).as_str(), "FROM" | "INTO")));
                if ends && then {
                    columns.push((k, name(src, &code[k].1)));
                }
            }
            _ => {}
        }
        k += 1;
    }
    // Their uses: a table's before a dot, a column's in ORDER / GROUP BY,
    // HAVING, QUALIFY.
    let declared: std::collections::HashSet<usize> = tables.iter().chain(&columns).map(|(k, _)| *k).collect();
    let table_names: std::collections::HashSet<&str> = tables.iter().map(|(_, n)| n.as_str()).collect();
    let column_names: std::collections::HashSet<&str> = columns.iter().map(|(_, n)| n.as_str()).collect();
    let mut clause: Vec<&'static str> = vec![""];
    for k in 0..code.len() {
        let t = code[k].1;
        match t.kind {
            Tk::Open => clause.push(""),
            Tk::Close => {
                clause.pop();
                if clause.is_empty() {
                    clause.push("");
                }
            }
            Tk::Keyword => {
                if let Some(c) = clause_of(&upper(src, &t)) {
                    *clause.last_mut().unwrap() = c;
                }
            }
            Tk::Word | Tk::Quoted => {
                if declared.contains(&k) {
                    continue;
                }
                let n = name(src, &t);
                let after_dot = k > 0 && code[k - 1].1.kind == Tk::Dot;
                if kind(k + 1) == Some(Tk::Dot) && !after_dot && table_names.contains(n.as_str()) {
                    out.push((code[k].0, Alias::Table));
                } else if !after_dot && kind(k + 1) != Some(Tk::Dot) && kind(k + 1) != Some(Tk::Open) && matches!(*clause.last().unwrap(), "ORDER" | "GROUP" | "HAVING" | "QUALIFY") && column_names.contains(n.as_str()) {
                    out.push((code[k].0, Alias::Column));
                }
            }
            _ => {}
        }
    }
    out.extend(tables.iter().map(|(k, _)| (code[*k].0, Alias::Table)));
    out.extend(columns.iter().map(|(k, _)| (code[*k].0, Alias::Column)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::{Dialect, lex};

    fn found(src: &str) -> Vec<(String, Alias)> {
        let toks = lex::lex(src, Dialect::Postgres);
        let mut v: Vec<(usize, Alias)> = find(src, &toks);
        v.sort_by_key(|x| toks[x.0].start);
        v.into_iter().map(|(k, a)| (toks[k].text(src).to_string(), a)).collect()
    }

    #[test]
    fn tables_and_columns() {
        use Alias::*;
        let q = "select o.id, c.name as who, count(*) n, o.total from orders o join shop.customers as c on c.id = o.customer_id, (select 1 x) s group by o.id, who order by n desc";
        assert_eq!(
            found(q),
            vec![
                ("o".into(), Table),
                ("c".into(), Table),
                ("who".into(), Column),
                ("n".into(), Column),
                ("o".into(), Table),
                ("o".into(), Table),
                ("c".into(), Table),
                ("c".into(), Table),
                ("o".into(), Table),
                ("x".into(), Column),
                ("s".into(), Table),
                ("o".into(), Table),
                ("who".into(), Column),
                ("n".into(), Column),
            ]
        );
        // No alias: nothing; a schema before a dot isn't one; CAST's AS isn't.
        assert!(found("select orders.id, cast(x as int) from orders where shop.f(1) > 0").is_empty());
        // Statements apart: an alias of one isn't the next one's.
        assert_eq!(found("select 1 from t a; select a.x from b").len(), 1);
    }
}
