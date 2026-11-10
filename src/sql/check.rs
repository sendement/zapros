//! Finds trouble in a script before it's run: strings, comments and
//! brackets left open, CASE without END (and in PL/SQL and PL/pgSQL bodies
//! BEGIN / IF / LOOP without theirs), stray commas, empty clauses, an
//! operator with nothing after it, UPDATE / DELETE with no WHERE, JOIN with
//! no ON, and — when the database's tables are known — tables and columns
//! that aren't there.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use super::lex::{self, Tk, Tok};
use super::{Dialect, split};
use crate::db::Relation;
use crate::i18n::t as tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Sev {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Issue {
    pub range: Range<usize>,
    pub sev: Sev,
    pub msg: String,
}

/// Tables known to the checker (lower-cased names).
#[derive(Default)]
pub struct Known {
    /// schema → table → columns.
    pub tables: HashMap<String, HashMap<String, HashSet<String>>>,
}

impl Known {
    pub fn new(rels: &[Relation]) -> Self {
        let mut k = Known::default();
        for r in rels {
            let cols = r.cols.iter().map(|c| c.0.to_lowercase()).collect();
            k.tables.entry(r.schema.to_lowercase()).or_default().insert(r.name.to_lowercase(), cols);
        }
        k
    }

    fn find(&self, schema: Option<&str>, name: &str) -> Option<Option<&HashSet<String>>> {
        match schema {
            Some(s) => self.tables.get(s).map(|t| t.get(name)),
            None => Some(self.tables.values().find_map(|t| t.get(name))),
        }
    }
}

pub fn check(src: &str, d: Dialect, known: Option<&Known>) -> Vec<Issue> {
    let toks = lex::lex(src, d);
    let mut out = Vec::new();
    for t in &toks {
        if t.open {
            let msg = match t.kind {
                Tk::Str => tr("Строка не закрыта", "String isn't closed"),
                Tk::Quoted => tr("Кавычка имени не закрыта", "Quoted name isn't closed"),
                _ => tr("Комментарий не закрыт", "Comment isn't closed"),
            };
            out.push(Issue { range: t.start..t.end, sev: Sev::Error, msg: msg.into() });
        }
    }
    for r in split(src, d, false) {
        let k = toks.partition_point(|t| t.start < r.start);
        let st: Vec<Tok> = toks[k..].iter().take_while(|t| t.end <= r.end).copied().collect();
        let plsql = d == Dialect::Oracle && super::plsql_start(src, &st[..st.len().min(16)]);
        brackets(src, &st, r.end, &mut out);
        blocks(src, &st, r.end, plsql, &mut out);
        if !plsql {
            clauses(src, &st, d, known, &mut out);
        }
        // PL/pgSQL bodies: their blocks too.
        if d == Dialect::Postgres && !st.iter().any(|t| t.is_word(src, "sql") && matches!(st.iter().rev().find(|x| x.end <= t.start && !x.trivia()), Some(p) if p.is_kw(src, "LANGUAGE"))) {
            let do_or_fn = st.iter().filter(|t| !t.trivia()).take(5).any(|t| t.is_word(src, "DO") || t.is_word(src, "FUNCTION") || t.is_word(src, "PROCEDURE"));
            for b in st.iter().filter(|t| t.kind == Tk::Str && do_or_fn && src[t.start..].starts_with('$') && !t.open) {
                let tag = src[b.start + 1..].find('$').unwrap() + 2;
                let body = b.start + tag..b.end - tag;
                let inner: Vec<Tok> = lex::lex(&src[body.clone()], d).into_iter().map(|t| Tok { start: t.start + body.start, end: t.end + body.start, ..t }).collect();
                brackets(src, &inner, body.end, &mut out);
                blocks(src, &inner, body.end, true, &mut out);
            }
        }
    }
    out.sort_by_key(|i| (i.range.start, std::cmp::Reverse(i.sev)));
    out.dedup_by(|a, b| a.range == b.range);
    out
}

fn brackets(src: &str, st: &[Tok], end: usize, out: &mut Vec<Issue>) {
    let mut stack: Vec<&Tok> = Vec::new();
    for t in st {
        match t.kind {
            Tk::Open => stack.push(t),
            Tk::Close => {
                let want = lex::pair_of(src.as_bytes()[t.start]);
                match stack.pop() {
                    Some(o) if Some(src.as_bytes()[o.start]) == want => {}
                    Some(o) => {
                        out.push(Issue { range: o.start..o.end, sev: Sev::Error, msg: tr("Скобка закрыта не той скобкой", "Bracket closed by the wrong one").into() });
                        out.push(Issue { range: t.start..t.end, sev: Sev::Error, msg: tr("Скобка не той формы", "Bracket of the wrong shape").into() });
                    }
                    None => out.push(Issue { range: t.start..t.end, sev: Sev::Error, msg: tr("Лишняя закрывающая скобка", "Closing bracket without an opening one").into() }),
                }
            }
            _ => {}
        }
    }
    for o in stack {
        out.push(Issue { range: o.start..end.max(o.end), sev: Sev::Error, msg: tr("Скобка не закрыта", "Bracket isn't closed").into() });
    }
}

fn up(src: &str, t: &Tok) -> String {
    if matches!(t.kind, Tk::Keyword | Tk::Word) { t.text(src).to_ascii_uppercase() } else { String::new() }
}

/// CASE … END; in PL/SQL also BEGIN … END, IF … END IF, LOOP … END LOOP,
/// and CREATE PACKAGE (BODY) … END.
fn blocks(src: &str, st: &[Tok], end: usize, plsql: bool, out: &mut Vec<Issue>) {
    let s: Vec<&Tok> = st.iter().filter(|t| !t.trivia()).collect();
    let words: Vec<String> = s.iter().map(|t| up(src, t)).collect();
    let mut stack: Vec<(&str, usize)> = Vec::new();
    let unclosed = |what: &str, k: usize, out: &mut Vec<Issue>| {
        let msg = match what {
            "CASE" => tr("CASE без END", "CASE without END"),
            "IF" => tr("IF без END IF", "IF without END IF"),
            "LOOP" => tr("LOOP без END LOOP", "LOOP without END LOOP"),
            "PACKAGE" => tr("Пакет без END", "Package without END"),
            _ => tr("BEGIN без END", "BEGIN without END"),
        };
        out.push(Issue { range: s[k].start..end.max(s[k].end), sev: Sev::Error, msg: msg.into() });
    };
    if plsql && words.first().is_some_and(|w| w == "CREATE") && words.iter().take(8).any(|w| w == "BODY" || w == "PACKAGE") {
        stack.push(("PACKAGE", 0));
    }
    let mut k = 0;
    while k < s.len() {
        let w = words[k].as_str();
        let prev = if k > 0 { words[k - 1].as_str() } else { "" };
        let next = words.get(k + 1).map_or("", String::as_str);
        match w {
            "CASE" if prev != "END" => stack.push(("CASE", k)),
            "BEGIN" if plsql => stack.push(("BEGIN", k)),
            "IF" if plsql && prev != "END" => stack.push(("IF", k)),
            "LOOP" if plsql && prev != "END" => stack.push(("LOOP", k)),
            "END" if s[k].kind == Tk::Keyword => {
                let want: &[&str] = match next {
                    "IF" if plsql => &["IF"],
                    "LOOP" if plsql => &["LOOP"],
                    "CASE" => &["CASE"],
                    _ => &["CASE", "BEGIN", "PACKAGE"],
                };
                if matches!(next, "IF" | "LOOP" | "CASE") {
                    k += 1;
                }
                // Openers left unclosed inside go first.
                loop {
                    match stack.last() {
                        Some((top, _)) if want.contains(top) => {
                            stack.pop();
                            break;
                        }
                        Some(&(top, at)) if stack.iter().any(|(x, _)| want.contains(x)) => {
                            unclosed(top, at, out);
                            stack.pop();
                        }
                        _ => {
                            let msg = if want.len() == 1 { format!("END {} {}", want[0], tr("без открывающего", "without its opening")) } else { tr("END без начала конструкции", "END without a construct to end").to_string() };
                            out.push(Issue { range: s[k].start..s[k].end.max(s[k.min(s.len() - 1)].end), sev: Sev::Error, msg });
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
        k += 1;
    }
    for (what, at) in stack {
        unclosed(what, at, out);
    }
}

const CLAUSES: &[&str] = &["FROM", "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION", "INTERSECT", "EXCEPT", "MINUS", "WINDOW", "FETCH", "RETURNING", "PREWHERE", "SETTINGS", "FORMAT", "QUALIFY", "INTO", "VALUES", "SET", "ON", "USING", "JOIN", "LEFT", "RIGHT", "INNER", "FULL", "CROSS", "THEN", "ELSE", "END", "WHEN"];

fn is_clause(w: &str) -> bool {
    CLAUSES.contains(&w)
}

/// Clauses that need something after them.
const NEEDS: &[&str] = &["WHERE", "FROM", "HAVING", "ON", "SET", "PREWHERE"];

fn clauses(src: &str, st: &[Tok], d: Dialect, known: Option<&Known>, out: &mut Vec<Issue>) {
    let s: Vec<&Tok> = st.iter().filter(|t| !t.trivia()).collect();
    if s.is_empty() {
        return;
    }
    let words: Vec<String> = s.iter().map(|t| up(src, t)).collect();
    let text = |k: usize| s[k].text(src);
    let is_end = |k: usize| k >= s.len() || matches!(s[k].kind, Tk::Semi | Tk::Close | Tk::Slash);
    // Depth of each solid token.
    let mut depth = vec![0i32; s.len()];
    let mut dd = 0;
    for (k, tk) in s.iter().enumerate() {
        if tk.kind == Tk::Close {
            dd -= 1;
        }
        depth[k] = dd;
        if tk.kind == Tk::Open {
            dd += 1;
        }
    }
    let err = |k: usize, msg: &str, out: &mut Vec<Issue>| out.push(Issue { range: s[k].start..s[k].end, sev: Sev::Error, msg: msg.into() });
    let warn = |k: usize, msg: &str, out: &mut Vec<Issue>| out.push(Issue { range: s[k].start..s[k].end, sev: Sev::Warning, msg: msg.into() });
    for k in 0..s.len() {
        let w = words[k].as_str();
        let next_w = words.get(k + 1).map_or("", String::as_str);
        if s[k].kind == Tk::Comma {
            if s.get(k + 1).is_some_and(|n| n.kind == Tk::Comma) {
                err(k, tr("Две запятые подряд", "Two commas in a row"), out);
            } else if is_end(k + 1) && s.get(k + 1).is_none_or(|n| n.kind != Tk::Close || depth[k] > 0) || matches!(next_w, "FROM" | "WHERE" | "GROUP" | "ORDER" | "HAVING" | "LIMIT" | "UNION") {
                err(k, tr("Лишняя запятая", "Stray comma"), out);
            }
        }
        if NEEDS.contains(&w) && s[k].kind == Tk::Keyword {
            // DELETE FROM, ON CONFLICT, ON DELETE CASCADE … aren't clauses to fill.
            let skip = (w == "ON" && matches!(next_w, "CONFLICT" | "DELETE" | "UPDATE" | "COMMIT" | "CLUSTER")) || (w == "SET" && k > 0 && matches!(words[k - 1].as_str(), "DO" | "UPDATE" | "CHARACTER"));
            if !skip && (is_end(k + 1) || (is_clause(next_w) && s[k + 1].kind == Tk::Keyword && !(w == "FROM" && next_w == "ONLY") && next_w != "CASE")) {
                err(k, &format!("{w}: {}", tr("пусто после него", "nothing after it")), out);
            }
        }
        // An operator with nothing to work on after it.
        let op = s[k].kind == Tk::Op && matches!(text(k), "=" | "<" | ">" | "<=" | ">=" | "<>" | "!=" | "+" | "-" | "/" | "||" | "%" | "::");
        if (op || (matches!(w, "AND" | "OR") && s[k].kind == Tk::Keyword)) && (is_end(k + 1) || (is_clause(next_w) && s[k + 1].kind == Tk::Keyword && !matches!(next_w, "CASE"))) {
            err(k, tr("После оператора нет выражения", "Nothing after the operator"), out);
        }
        if matches!(w, "AND" | "OR") && k > 0 && matches!(words[k - 1].as_str(), "WHERE" | "AND" | "OR" | "ON" | "HAVING") && s[k - 1].kind == Tk::Keyword {
            err(k, tr("Условие пропущено перед этим", "A condition is missing before it"), out);
        }
        // JOIN with no ON / USING.
        if w == "JOIN" && s[k].kind == Tk::Keyword {
            let prev = if k > 0 { words[k - 1].as_str() } else { "" };
            let free = matches!(prev, "CROSS" | "NATURAL" | "ARRAY" | "PASTE") || (k > 1 && words[k - 2] == "ARRAY") || (prev == "LATERAL");
            if !free {
                let mut j = k + 1;
                let mut found = false;
                while j < s.len() && depth[j] >= depth[k] && !(depth[j] == depth[k] && (is_end(j) || matches!(words[j].as_str(), "JOIN" | "WHERE" | "GROUP" | "ORDER" | "UNION" | "LIMIT" | "WINDOW" | "HAVING" | "PREWHERE" | "SETTINGS"))) {
                    if depth[j] == depth[k] && matches!(words[j].as_str(), "ON" | "USING") {
                        found = true;
                        break;
                    }
                    j += 1;
                }
                // CH: comma joins and CROSS-like ones are fine; LEFT JOIN … with no ON isn't.
                if !found && words.get(k + 1).is_none_or(|n| n != "LATERAL") {
                    warn(k, tr("JOIN без ON / USING", "JOIN without ON / USING"), out);
                }
            }
        }
    }
    // UPDATE / DELETE without WHERE (top level).
    let first = words[0].as_str();
    if matches!(first, "UPDATE" | "DELETE") && !(0..s.len()).any(|k| depth[k] == 0 && words[k] == "WHERE") && !(d == Dialect::Sqlite && words.contains(&"LIMIT".to_string())) {
        warn(0, tr("Без WHERE изменит все строки", "No WHERE: every row changes"), out);
    }
    if let Some(known) = known.filter(|k| !k.tables.is_empty()) {
        unknown_names(src, &s, &words, &depth, d, known, out);
    }
}

/// Tables after FROM / JOIN / UPDATE / INTO that aren't in the database,
/// and alias.column where the alias's table has no such column.
fn unknown_names(src: &str, s: &[&Tok], words: &[String], depth: &[i32], d: Dialect, known: &Known, out: &mut Vec<Issue>) {
    let name = |k: usize| -> Option<String> {
        let t = s.get(k)?;
        match t.kind {
            Tk::Word | Tk::Keyword => Some(t.text(src).to_lowercase()),
            Tk::Quoted => Some(super::unquote(t.text(src)).to_lowercase()),
            _ => None,
        }
    };
    // CTE names.
    let mut ctes: HashSet<String> = HashSet::new();
    for k in 0..s.len() {
        if words[k] == "AS" && s.get(k + 1).is_some_and(|t| t.kind == Tk::Open) && k > 0 && depth[k] == 0 {
            if let Some(n) = name(k - 1) {
                ctes.insert(n);
            }
        }
    }
    let mut aliases: HashMap<String, (Option<String>, String)> = HashMap::new();
    for k in 0..s.len() {
        let w = words[k].as_str();
        let starts = matches!(w, "FROM" | "JOIN" | "UPDATE" | "INTO" | "TRUNCATE") && s[k].kind == Tk::Keyword;
        // FROM a, b
        let comma_from = s[k].kind == Tk::Comma && (0..k).rev().take_while(|&j| depth[j] >= depth[k]).filter(|&j| depth[j] == depth[k]).find_map(|j| matches!(words[j].as_str(), "FROM" | "WHERE" | "SELECT" | "JOIN" | "SET" | "GROUP" | "ORDER" | "VALUES" | "BY").then(|| words[j].clone())).as_deref() == Some("FROM");
        if !starts && !comma_from {
            continue;
        }
        let mut j = k + 1;
        if words.get(j).is_some_and(|w| matches!(w.as_str(), "TABLE" | "ONLY" | "LATERAL")) {
            j += 1;
        }
        let Some(first) = name(j) else { continue };
        if s[j].kind == Tk::Keyword && !matches!(s[j].kind, Tk::Quoted) && super::is_keyword(&first, d) {
            continue;
        }
        let (schema, table, end) = if s.get(j + 1).is_some_and(|t| t.kind == Tk::Dot) {
            match name(j + 2) {
                Some(t) => (Some(first), t, j + 2),
                None => continue,
            }
        } else {
            (None, first, j)
        };
        // A function (generate_series(…)) or more dots (db.schema.t).
        if s.get(end + 1).is_some_and(|t| t.kind == Tk::Open || t.kind == Tk::Dot) {
            continue;
        }
        let mut a = end + 1;
        if words.get(a).is_some_and(|w| w == "AS") {
            a += 1;
        }
        if let Some(al) = name(a).filter(|al| s[a].kind != Tk::Keyword || !super::is_keyword(al, d)) {
            aliases.insert(al, (schema.clone(), table.clone()));
        }
        aliases.insert(table.clone(), (schema.clone(), table.clone()));
        if schema.is_none() && ctes.contains(&table) {
            continue;
        }
        let system = table == "dual" || table.starts_with("pg_") || table.starts_with("all_") || table.starts_with("user_") || table.starts_with("dba_") || table.starts_with("v$") || table.starts_with("gv$") || table.starts_with("sqlite_");
        if system {
            continue;
        }
        if let Some(None) = known.find(schema.as_deref(), &table) {
            let r = s[if schema.is_some() { j } else { end }].start..s[end].end;
            out.push(Issue { range: r, sev: Sev::Warning, msg: tr("Такой таблицы нет в базе", "No such table in the database").into() });
        }
    }
    // alias.column
    for k in 0..s.len().saturating_sub(2) {
        if s[k + 1].kind != Tk::Dot || !matches!(s[k + 2].kind, Tk::Word | Tk::Keyword | Tk::Quoted) || (k > 0 && s[k - 1].kind == Tk::Dot) || s.get(k + 3).is_some_and(|t| t.kind == Tk::Open || t.kind == Tk::Dot) {
            continue;
        }
        let (Some(al), Some(col)) = (name(k), name(k + 2)) else { continue };
        let Some((schema, table)) = aliases.get(&al) else { continue };
        if let Some(Some(cols)) = known.find(schema.as_deref(), table) {
            if !cols.contains(&col) && !matches!(col.as_str(), "rowid" | "ctid" | "xmin" | "oid" | "rownum" | "_part" | "_partition_id" | "_table") {
                out.push(Issue { range: s[k + 2].start..s[k + 2].end, sev: Sev::Warning, msg: format!("{} {}", tr("Нет такой колонки в", "No such column in"), table) });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errs(src: &str, d: Dialect) -> Vec<(String, Sev)> {
        check(src, d, None).into_iter().map(|i| (src[i.range].to_string(), i.sev)).collect()
    }

    #[test]
    fn open_things() {
        assert_eq!(errs("select (a from t", Dialect::Postgres), vec![("(a from t".into(), Sev::Error)]);
        assert_eq!(errs("select a) from t", Dialect::Postgres), vec![(")".into(), Sev::Error)]);
        assert_eq!(errs("select 'abc from t", Dialect::Postgres), vec![("'abc from t".into(), Sev::Error)]);
        let e = errs("select case when a then 1 from t", Dialect::Postgres);
        assert_eq!(e, vec![("case when a then 1 from t".into(), Sev::Error)]);
        assert!(errs("select case when a then 1 end, case b when 1 then 2 else 3 end from t", Dialect::Postgres).is_empty());
    }

    #[test]
    fn plsql_blocks() {
        let ok = "begin\n  if x > 1 then\n    for r in (select 1 from dual) loop\n      null;\n    end loop;\n  end if;\nend;\n/";
        assert!(errs(ok, Dialect::Oracle).is_empty(), "{:?}", errs(ok, Dialect::Oracle));
        let bad = "begin\n  if x > 1 then\n    null;\nend;\n/";
        assert_eq!(errs(bad, Dialect::Oracle)[0].0.split_whitespace().next(), Some("if"));
        let pkg = "create or replace package body p as\n  procedure a is\n  begin\n    null;\n  end a;\nend p;\n/";
        assert!(errs(pkg, Dialect::Oracle).is_empty(), "{:?}", errs(pkg, Dialect::Oracle));
        let pg = "do $$\nbegin\n  if true then\n    raise notice 'x';\nend $$;";
        assert!(errs(pg, Dialect::Postgres).iter().any(|e| e.0.starts_with("if")));
    }

    #[test]
    fn clause_slips() {
        assert_eq!(errs("select a, from t", Dialect::Postgres), vec![(",".into(), Sev::Error)]);
        assert_eq!(errs("select a from t where", Dialect::Postgres), vec![("where".into(), Sev::Error)]);
        assert_eq!(errs("select a from t where a = order by 1", Dialect::Postgres), vec![("=".into(), Sev::Error)]);
        assert_eq!(errs("delete from t", Dialect::Postgres), vec![("delete".into(), Sev::Warning)]);
        assert_eq!(errs("select * from a join b where 1=1", Dialect::Postgres), vec![("join".into(), Sev::Warning)]);
        assert!(errs("select * from a cross join b; select count(*) from t; insert into t (a, b) values (1, 2)", Dialect::Postgres).is_empty());
        assert!(errs("select * from a left join b on a.id = b.id where a.x in (1, 2)", Dialect::Postgres).is_empty());
        assert!(errs("insert into t values (1) on conflict (id) do update set a = 1", Dialect::Postgres).is_empty());
    }

    #[test]
    fn unknown_tables_and_columns() {
        let k = Known::new(&[Relation { schema: "public".into(), name: "users".into(), cols: vec![("id".into(), "int".into()), ("name".into(), "text".into())] }]);
        let src = "with x as (select 1) select u.id, u.nmae from users u join orders o on o.uid = u.id, x";
        let e: Vec<String> = check(src, Dialect::Postgres, Some(&k)).into_iter().map(|i| src[i.range].to_string()).collect();
        assert_eq!(e, vec!["nmae", "orders"]);
    }
}
