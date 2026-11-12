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

fn find<'a>(rels: &'a [Relation], r: &Ref) -> Option<&'a Relation> {
    rels.iter().find(|x| x.name.eq_ignore_ascii_case(&r.table) && r.schema.as_ref().is_none_or(|s| s.eq_ignore_ascii_case(&x.schema)))
}

/// A name as it must be written (quoted when it'd fold or clash).
pub fn quote(name: &str, d: Dialect) -> String {
    // (ClickHouse's own database `default` reads fine bare.)
    let keyword = super::is_keyword(name, d) && !(d == Dialect::ClickHouse && name == "default");
    let plain = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()) && !keyword;
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

/// Columns that look like a join between two tables: b.x_id = a.id, or
/// the same *_id on both sides.
fn join_guesses(rels: &[Relation], refs: &[Ref], d: Dialect) -> Vec<Item> {
    let Some((last, others)) = refs.split_last() else { return Vec::new() };
    let Some(b) = find(rels, last) else { return Vec::new() };
    let mut out = Vec::new();
    for o in others {
        let Some(a) = find(rels, o) else { continue };
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
pub fn hints(src: &str, at: usize, d: Dialect, rels: &[Relation], case: Case, forced: bool, quote_schemas: bool) -> Option<Hints> {
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
    let before: Vec<Tok> = lex::lex(&src[stmt.start..start], d).into_iter().filter(|t| !t.trivia()).collect();
    let prev = before.last().map(|t| t.text(&src[stmt.start..start]).to_ascii_uppercase()).unwrap_or_default();
    let mut items: Vec<Item> = Vec::new();
    let col = |name: &str, ty: &str, owner: &str| Item { label: name.to_string(), insert: quote(name, d), kind: Kind::Column, detail: format!("{ty} · {owner}") };
    if let Some(q) = &qualifier {
        // alias. or table. → columns; schema. → its tables.
        if let Some(r) = refs.iter().find(|r| r.alias.eq_ignore_ascii_case(q)).cloned().or_else(|| Some(Ref { alias: q.clone(), schema: None, table: q.clone() })) {
            if let Some(rel) = find(rels, &r) {
                items.extend(rel.cols.iter().map(|(c, ty)| col(c, ty, &rel.name)));
            }
        }
        items.extend(rels.iter().filter(|r| r.schema.eq_ignore_ascii_case(q)).map(|r| Item { label: r.name.clone(), insert: quote(&r.name, d), kind: Kind::Table, detail: r.schema.clone() }));
    } else if matches!(prev.as_str(), "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE" | "TRUNCATE" | "DESCRIBE" | "DESC") || (prev == "," && before.iter().rev().find(|t| t.kind == Tk::Keyword).is_some_and(|t| t.text(&src[stmt.start..start]).eq_ignore_ascii_case("FROM"))) {
        let mut schemas: Vec<&str> = rels.iter().map(|r| r.schema.as_str()).collect();
        schemas.dedup();
        items.extend(rels.iter().map(|r| Item { label: r.name.clone(), insert: qualified(r, d, quote_schemas), kind: Kind::Table, detail: format!("{} · {}", r.schema, r.cols.len()) }));
        items.extend(schemas.iter().map(|s| Item { label: s.to_string(), insert: schema_name(s, d, quote_schemas), kind: Kind::Schema, detail: String::new() }));
    } else {
        if prev == "ON" {
            items.extend(join_guesses(rels, &refs, d));
        }
        for r in &refs {
            if let Some(rel) = find(rels, r) {
                items.extend(rel.cols.iter().map(|(c, ty)| col(c, ty, &r.alias)));
            }
        }
        items.extend(SNIPPETS.iter().map(|(k, body, label)| Item { label: k.to_string(), insert: cased_snippet(body, case), kind: Kind::Snippet, detail: label.to_string() }));
        items.extend(super::keywords(d).into_iter().map(|k| Item { label: k.to_string(), insert: case.apply(k), kind: Kind::Keyword, detail: String::new() }));
        items.extend(super::functions(d).into_iter().map(|f| {
            let name = if d == Dialect::ClickHouse { f.to_string() } else { case.apply(f) };
            Item { label: f.to_string(), insert: format!("{name}($0)"), kind: Kind::Function, detail: String::new() }
        }));
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

    #[test]
    fn alias_columns() {
        let src = "select u. from users u";
        let h = hints(src, 9, Dialect::Postgres, &rels(), Case::Upper, false, false).unwrap();
        assert_eq!(h.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), vec!["id", "name"]);
        let h = hints("select o. from orders o", 9, Dialect::Postgres, &rels(), Case::Upper, false, false).unwrap();
        assert_eq!(h.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), vec!["id", "user_id", "total"]);
        let src = "select o.to from orders as o";
        let h = hints(src, 11, Dialect::Postgres, &rels(), Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].label, "total");
        assert_eq!(h.range, 9..11);
    }

    #[test]
    fn tables_after_from_and_joins() {
        let src = "select * from us";
        let h = hints(src, src.len(), Dialect::Postgres, &rels(), Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].label, "users");
        let src = "select * from users u join orders o on ";
        let h = hints(src, src.len(), Dialect::Postgres, &rels(), Case::Upper, true, false).unwrap();
        assert_eq!(h.items[0].insert, "o.user_id = u.id");
    }

    #[test]
    fn tables_off_the_search_path_keep_their_schema() {
        let mut r = rels();
        r.push(Relation { schema: "sales".into(), name: "invoices".into(), cols: vec![("id".into(), "int".into())], qualify: true });
        let src = "select * from inv";
        let h = hints(src, src.len(), Dialect::Postgres, &r, Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].insert, "sales.invoices");
        // Its columns by alias all the same.
        let h = hints("select i. from sales.invoices i", 9, Dialect::Postgres, &r, Case::Upper, false, false).unwrap();
        assert_eq!(h.items[0].label, "id");
    }

    #[test]
    fn keywords_by_case_and_none_in_strings() {
        let h = hints("sel", 3, Dialect::Postgres, &[], Case::Lower, false, false).unwrap();
        assert!(h.items.iter().any(|i| i.insert == "select"));
        assert_eq!(h.items[0].kind, Kind::Snippet);
        assert!(hints("select 'fro", 11, Dialect::Postgres, &[], Case::Upper, false, false).is_none());
    }

    #[test]
    fn schemas_quoted_when_asked() {
        let r = vec![Relation { schema: "sales".into(), name: "invoices".into(), cols: vec![("id".into(), "int".into())], qualify: true }];
        let src = "select * from inv";
        let plain = hints(src, src.len(), Dialect::Postgres, &r, Case::Upper, false, false).unwrap();
        assert!(plain.items.iter().any(|i| i.insert == "sales.invoices"));
        let quoted = hints(src, src.len(), Dialect::Postgres, &r, Case::Upper, false, true).unwrap();
        assert!(quoted.items.iter().any(|i| i.insert == "\"sales\".invoices"), "{:?}", quoted.items.iter().map(|i| &i.insert).collect::<Vec<_>>());
        let ch = hints(src, src.len(), Dialect::ClickHouse, &r, Case::Upper, false, true).unwrap();
        assert!(ch.items.iter().any(|i| i.insert == "`sales`.invoices"));
        let s = "select * from sal";
        let schema = hints(s, s.len(), Dialect::Postgres, &r, Case::Upper, false, true).unwrap();
        assert!(schema.items.iter().any(|i| i.insert == "\"sales\""), "{:?}", schema.items.iter().map(|i| &i.insert).collect::<Vec<_>>());
    }


    #[test]
    fn clickhouse_default_database() {
        let r = vec![Relation { schema: "default".into(), name: "orders".into(), cols: vec![("id".into(), "UInt64".into()), ("total".into(), "Float64".into())], qualify: true }];
        let ins = |src: &str, at: usize| hints(src, at, Dialect::ClickHouse, &r, Case::Upper, false, false).map(|h| h.items.iter().map(|i| i.insert.clone()).collect::<Vec<_>>()).unwrap_or_default();
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
