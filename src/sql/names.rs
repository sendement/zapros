//! What each name in a statement is — a schema, a table, a column, a
//! table's or a column's alias — for the formatter's case settings.
//! Guessed from where it stands (FROM x, x.y, AS x…), no catalogue asked.

use super::aliases::{self, Alias};
use super::lex::{Tk, Tok};
use super::{Case, Dialect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Name {
    Schema,
    Table,
    Column,
    TableAlias,
    ColumnAlias,
}

/// Words naming a table (or a view, a CTE…) right after them.
const BEFORE_TABLE: &[&str] = &["FROM", "JOIN", "UPDATE", "INTO", "TABLE", "VIEW", "ONLY", "LATERAL", "TRUNCATE", "DESCRIBE", "EXISTS"];

/// The names among `toks` (all of a text's tokens): their indexes and
/// what they are. Quoted names, keywords and calls aren't among them.
pub fn classify(src: &str, toks: &[Tok]) -> Vec<(usize, Name)> {
    let mut of: Vec<Option<Name>> = vec![None; toks.len()];
    for (k, a) in aliases::find(src, toks) {
        of[k] = Some(match a {
            Alias::Table => Name::TableAlias,
            Alias::Column => Name::ColumnAlias,
            Alias::Schema => Name::Schema,
        });
    }
    let code: Vec<usize> = (0..toks.len()).filter(|&k| !toks[k].trivia()).collect();
    let kind = |c: usize| code.get(c).map(|&k| toks[k].kind);
    let up = |c: usize| code.get(c).map_or(String::new(), |&k| toks[k].text(src).to_ascii_uppercase());
    let is_name = |c: usize| matches!(kind(c), Some(Tk::Word | Tk::Quoted));
    // Brackets' clauses, as in aliases: FROM's commas name tables.
    let mut clause: Vec<&str> = vec![""];
    let mut c = 0;
    while c < code.len() {
        let k = code[c];
        match toks[k].kind {
            Tk::Open => clause.push(""),
            Tk::Close => {
                clause.pop();
                if clause.is_empty() {
                    clause.push("");
                }
            }
            Tk::Keyword => {
                let w = up(c);
                let to = match w.as_str() {
                    "FROM" | "JOIN" => "FROM",
                    "WITH" => "WITH",
                    "SELECT" | "WHERE" | "GROUP" | "ORDER" | "HAVING" | "ON" | "USING" | "SET" | "VALUES" | "LIMIT" | "RETURNING" | "QUALIFY" | "PREWHERE" | "WINDOW" => "",
                    _ => clause.last().copied().unwrap_or(""),
                };
                *clause.last_mut().unwrap() = to;
            }
            _ => {}
        }
        if !is_name(c) || (c > 0 && kind(c - 1) == Some(Tk::Dot)) {
            c += 1;
            continue;
        }
        // A dotted chain from here: a.b.c.
        let mut last = c;
        while kind(last + 1) == Some(Tk::Dot) && is_name(last + 2) {
            last += 2;
        }
        let parts: Vec<usize> = (c..=last).step_by(2).collect();
        let prev = c.checked_sub(1);
        let before = prev.map_or(String::new(), |p| if kind(p) == Some(Tk::Keyword) { up(p) } else { String::new() });
        let here = clause.last().copied().unwrap_or("");
        // WITH x AS (…), y (a, b) AS (…)
        let cte = (matches!(before.as_str(), "WITH" | "RECURSIVE") || (prev.is_some_and(|p| kind(p) == Some(Tk::Comma)) && here == "WITH")) && (up(last + 1) == "AS" || kind(last + 1) == Some(Tk::Open));
        let names_table = BEFORE_TABLE.contains(&before.as_str()) || (prev.is_some_and(|p| kind(p) == Some(Tk::Comma)) && here == "FROM") || cte;
        // A bracket after it: a call, but columns after CREATE TABLE t,
        // INSERT INTO t, a CTE's name.
        let called = kind(last + 1) == Some(Tk::Open) && !(cte || matches!(before.as_str(), "TABLE" | "INTO" | "VIEW"));
        let mut put = |c: usize, n: Name| {
            let k = code[c];
            if toks[k].kind == Tk::Word && of[k].is_none() {
                of[k] = Some(n);
            }
        };
        if called {
            // schema.package.f(…): all but the call are schemas.
            for &p in &parts[..parts.len() - 1] {
                put(p, Name::Schema);
            }
        } else if names_table {
            for &p in &parts[..parts.len() - 1] {
                put(p, Name::Schema);
            }
            put(last, Name::Table);
        } else {
            // col, t.col, s.t.col
            let n = parts.len();
            for (i, &p) in parts.iter().enumerate() {
                let is = match n - i {
                    1 => Name::Column,
                    2 => Name::Table,
                    _ => Name::Schema,
                };
                // A type: id int, cast(x as int).
                if n == 1 && super::is_type(&up(p)) && prev.is_some_and(|q| matches!(kind(q), Some(Tk::Word | Tk::Quoted)) || up(q) == "AS") {
                    continue;
                }
                put(p, is);
            }
        }
        c = last + 1;
    }
    of.into_iter().enumerate().filter_map(|(k, n)| n.map(|n| (k, n))).collect()
}

/// Cases for each kind of name (`AsIs`: left alone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cases {
    pub schemas: Case,
    pub tables: Case,
    pub columns: Case,
    pub table_aliases: Case,
    pub column_aliases: Case,
}

impl Cases {
    pub fn of(&self, n: Name) -> Case {
        match n {
            Name::Schema => self.schemas,
            Name::Table => self.tables,
            Name::Column => self.columns,
            Name::TableAlias => self.table_aliases,
            Name::ColumnAlias => self.column_aliases,
        }
    }

    pub fn any(&self) -> bool {
        [self.schemas, self.tables, self.columns, self.table_aliases, self.column_aliases].iter().any(|c| *c != Case::AsIs)
    }
}

/// The new text of each name token the cases change. ClickHouse's names
/// are case-sensitive: its schemas, tables and columns stay; an alias
/// changes only where every word spelled like it is that alias (else a
/// use the guess missed, WHERE y on a SELECT's y, would no longer match).
pub fn recased(src: &str, toks: &[Tok], d: Dialect, cases: Cases) -> Vec<(usize, String)> {
    let names = classify(src, toks);
    let mut out = Vec::new();
    for &(k, n) in &names {
        let case = cases.of(n);
        if case == Case::AsIs {
            continue;
        }
        let text = toks[k].text(src);
        if d == Dialect::ClickHouse {
            if !matches!(n, Name::TableAlias | Name::ColumnAlias) {
                continue;
            }
            let all_alias = toks.iter().enumerate().filter(|(_, t)| t.kind == Tk::Word && t.text(src) == text).all(|(j, _)| names.iter().any(|&(m, x)| m == j && x == n));
            if !all_alias {
                continue;
            }
        }
        let new = case.apply(text);
        if new != text {
            out.push((k, new));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::lex;

    fn names(src: &str) -> Vec<(String, Name)> {
        let toks = lex::lex(src, Dialect::Postgres);
        let mut v = classify(src, &toks);
        v.sort_by_key(|x| toks[x.0].start);
        v.into_iter().map(|(k, n)| (toks[k].text(src).to_string(), n)).collect()
    }

    #[test]
    fn what_names_are() {
        use Name::*;
        let got = names("select o.id, c.name as who, count(*) n, orders.total from shop.orders o join customers c on c.id = o.customer_id where shop.f(1) > 0");
        assert_eq!(
            got,
            vec![
                ("o".into(), TableAlias),
                ("id".into(), Column),
                ("c".into(), TableAlias),
                ("name".into(), Column),
                ("who".into(), ColumnAlias),
                ("n".into(), ColumnAlias),
                ("orders".into(), Table),
                ("total".into(), Column),
                ("shop".into(), Schema),
                ("orders".into(), Table),
                ("o".into(), TableAlias),
                ("customers".into(), Table),
                ("c".into(), TableAlias),
                ("c".into(), TableAlias),
                ("id".into(), Column),
                ("o".into(), TableAlias),
                ("customer_id".into(), Column),
                ("shop".into(), Schema),
            ]
        );
        // CTEs are tables; types aren't columns.
        let got = names("with x as (select 1 a), y (b) as (select 2) select cast(a as int) from x, y; create table t (id int, d date)");
        let tables: Vec<&str> = got.iter().filter(|x| x.1 == Table).map(|x| x.0.as_str()).collect();
        assert_eq!(tables, ["x", "y", "x", "y", "t"]);
        let cols: Vec<&str> = got.iter().filter(|x| x.1 == Column).map(|x| x.0.as_str()).collect();
        assert_eq!(cols, ["b", "a", "id", "d"]);
    }

    #[test]
    fn clickhouse_keeps_names() {
        let cases = Cases { schemas: Case::Upper, tables: Case::Upper, columns: Case::Upper, table_aliases: Case::Upper, column_aliases: Case::Upper };
        let src = "select t.x as y, t.z as w from db.events t where y > 1 order by w";
        let toks = lex::lex(src, Dialect::ClickHouse);
        let got: Vec<String> = recased(src, &toks, Dialect::ClickHouse, cases).into_iter().map(|(_, s)| s).collect();
        // t: every use an alias; w: too (ORDER BY); y: also a plain word in WHERE.
        assert_eq!(got, ["T", "T", "W", "T", "W"]);
        let toks = lex::lex(src, Dialect::Postgres);
        assert_eq!(recased(src, &toks, Dialect::Postgres, cases).len(), 11);
    }
}
