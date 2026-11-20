//! The reference of functions, operators and keywords (help.txt): what
//! Ctrl+click on one of them shows — how it's written, what it does, an
//! example.

use std::sync::OnceLock;

use super::Dialect;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub names: Vec<String>,
    /// "pg", "ch", "ora", "lite"; empty: every database.
    pub dialects: Vec<String>,
    /// 'f' function, 'o' operator, 'k' keyword.
    pub kind: char,
    pub sig: String,
    pub ru: Vec<String>,
    pub en: Vec<String>,
    pub example: Vec<String>,
    /// Where it's meant (empty: anywhere): a bracket's word ("keep",
    /// "over", "within", "call" — an aggregate's own), the word before it,
    /// ">word" after it, "case" inside CASE, the statement's first word.
    pub ctx: Vec<String>,
    /// Its own window title (ru, en): ORDER BY in KEEP.
    pub head: Option<(String, String)>,
}

impl Entry {
    /// What it does, in the program's language.
    pub fn about(&self) -> String {
        let v = if crate::i18n::is_ru() && !self.ru.is_empty() { &self.ru } else { &self.en };
        v.join("\n")
    }

    pub fn title(&self) -> String {
        match &self.head {
            Some((ru, en)) => if crate::i18n::is_ru() { ru.clone() } else { en.clone() },
            None => self.names.join(", "),
        }
    }

    pub fn example(&self) -> String {
        self.example.join("\n")
    }

    fn fits(&self, d: Dialect) -> bool {
        self.dialects.is_empty() || d == Dialect::Generic || self.dialects.iter().any(|x| x == tag(d))
    }
}

fn tag(d: Dialect) -> &'static str {
    match d {
        Dialect::Postgres => "pg",
        Dialect::ClickHouse => "ch",
        Dialect::Oracle => "ora",
        Dialect::Sqlite => "lite",
        Dialect::Generic => "",
    }
}

/// help.txt read into entries.
pub fn parse(src: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for line in src.lines() {
        let line = line.trim_end();
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(head) = line.strip_prefix("@ ") {
            let parts: Vec<&str> = head.split(" | ").collect();
            let names = parts[0].split(", ").map(|s| s.trim().to_string()).collect();
            let dialects = parts.get(1).map_or(Vec::new(), |d| if d.trim() == "*" { Vec::new() } else { d.split_whitespace().map(str::to_string).collect() });
            let kind = parts.get(2).and_then(|k| k.trim().chars().next()).unwrap_or('f');
            let ctx = parts.get(3).map_or(Vec::new(), |c| c.split_whitespace().map(str::to_string).collect());
            out.push(Entry { names, dialects, kind, sig: String::new(), ru: Vec::new(), en: Vec::new(), example: Vec::new(), ctx, head: None });
            continue;
        }
        let Some(e) = out.last_mut() else { continue };
        let (key, val) = line.split_once(": ").unwrap_or((line, ""));
        match key {
            "sig" => e.sig = val.to_string(),
            "title" => e.head = val.split_once(" | ").map(|(a, b)| (a.to_string(), b.to_string())),
            "ru" => e.ru.push(val.to_string()),
            "en" => e.en.push(val.to_string()),
            // (An example keeps its own indent after "ex: ".)
            "ex" => e.example.push(val.to_string()),
            _ => {}
        }
    }
    out
}

pub fn entries() -> &'static [Entry] {
    static E: OnceLock<Vec<Entry>> = OnceLock::new();
    E.get_or_init(|| parse(include_str!("help.txt")))
}

/// The entry for `name` (a word or an operator) in dialect `d`; `call`:
/// written as a call (a function before a keyword then). The dialect's
/// own entry before one for every database.
pub fn lookup(name: &str, d: Dialect, call: bool) -> Option<&'static Entry> {
    lookup_in(name, d, call, &[])
}

/// As `lookup`, but where it stands counts: the first of `ctx` (nearest
/// first) an entry is meant for wins; entries meant for a place are found
/// only there.
pub fn lookup_in(name: &str, d: Dialect, call: bool, ctx: &[String]) -> Option<&'static Entry> {
    let all = entries();
    let named: Vec<&Entry> = all.iter().filter(|e| e.fits(d) && e.names.iter().any(|n| n.eq_ignore_ascii_case(name))).collect();
    for c in ctx {
        let here: Vec<&&Entry> = named.iter().filter(|e| e.ctx.contains(c)).collect();
        if let Some(e) = here.iter().find(|e| !e.dialects.is_empty()).or(here.first()) {
            return Some(e);
        }
    }
    let named: Vec<&Entry> = named.into_iter().filter(|e| e.ctx.is_empty()).collect();
    let want = |e: &&Entry| if call { e.kind == 'f' } else { e.kind != 'f' };
    let own = |e: &&Entry| !e.dialects.is_empty();
    named
        .iter()
        .find(|e| want(e) && own(e))
        .or_else(|| named.iter().find(|e| want(e)))
        .or_else(|| named.iter().find(|e| own(e)))
        .or_else(|| named.first())
        .copied()
}

/// The function, operator or keyword at byte `at` of `sql` (one
/// statement) the reference has help for, as meant there: where it is
/// and its entry's index. ORDER BY in KEEP (…), in OVER (…), in WITHIN
/// GROUP (…), in string_agg(…), in a ClickHouse table's definition each
/// has its own. Not a qualified name (schema.f), not a column named like a
/// function (no bracket after it).
pub fn at(sql: &str, at: usize, d: Dialect) -> Option<(std::ops::Range<usize>, usize)> {
    use super::lex::{self, Tk};
    let toks: Vec<lex::Tok> = lex::lex(sql, d).into_iter().filter(|t| !t.trivia()).collect();
    let mut k = toks.iter().position(|t| t.start <= at && at < t.end).or_else(|| toks.iter().position(|t| t.end == at && at > t.start))?;
    let t = toks[k];
    if !matches!(t.kind, Tk::Word | Tk::Keyword | Tk::Op) {
        return None;
    }
    if k > 0 && toks[k - 1].kind == Tk::Dot || toks.get(k + 1).is_some_and(|n| n.kind == Tk::Dot) {
        return None;
    }
    let word = |k: usize| toks.get(k).filter(|t| matches!(t.kind, Tk::Word | Tk::Keyword)).map_or(String::new(), |t| t.text(sql).to_lowercase());
    // BY of ORDER BY, GROUP of WITHIN GROUP: the word they go with.
    let w = word(k);
    if k > 0 && ((w == "by" && matches!(word(k - 1).as_str(), "order" | "group" | "partition" | "connect" | "limit")) || (w == "group" && word(k - 1) == "within")) {
        k -= 1;
    }
    let name = toks[k].text(sql);
    let call = toks.get(k + 1).is_some_and(|n| n.kind == Tk::Open && n.text(sql) == "(");
    // Where it stands, nearest first.
    let mut ctx: Vec<String> = Vec::new();
    if k > 0 && !word(k - 1).is_empty() {
        ctx.push(word(k - 1));
    }
    if !word(k + 1).is_empty() {
        ctx.push(format!(">{}", word(k + 1)));
    }
    let cases = toks[..k].iter().filter(|t| t.is_kw(sql, "CASE")).count();
    let ends = toks[..k].iter().filter(|t| t.is_kw(sql, "END")).count();
    if cases > ends {
        ctx.push("case".into());
    }
    // The brackets it's in, inner first, up to a subquery's.
    let mut depth = 0usize;
    let mut sub = false;
    for j in (0..k).rev() {
        match toks[j].kind {
            Tk::Close => depth += 1,
            Tk::Open if depth > 0 => depth -= 1,
            Tk::Open => {
                if matches!(word(j + 1).as_str(), "select" | "with") {
                    sub = true;
                    break;
                }
                let before = j.checked_sub(1).map(word).unwrap_or_default();
                if before == "group" && j >= 2 && word(j - 2) == "within" {
                    ctx.push("within".into());
                } else if !before.is_empty() {
                    let own = matches!(before.as_str(), "keep" | "over" | "filter" | "in" | "exists" | "as" | "values" | "using" | "on" | "and" | "or" | "not" | "from" | "join" | "where" | "by" | "into" | "table");
                    ctx.push(before.clone());
                    if !own && toks[j - 1].kind == Tk::Word || lookup(&before, d, true).is_some_and(|e| e.kind == 'f') {
                        ctx.push("call".into());
                    }
                }
            }
            _ => {}
        }
    }
    // The statement: CREATE TABLE … ORDER BY, MERGE … ON (not in its query).
    if !sub {
        let selects = toks[..k].iter().any(|t| t.is_kw(sql, "SELECT"));
        let first = word(0);
        if !(selects && first != "select" && first != "with") {
            ctx.push(first);
        }
    }
    let e = lookup_in(name, d, call, &ctx)?;
    // A bare word: a keyword's help, never a function's.
    if toks[k].kind == Tk::Word && !call && e.kind == 'f' {
        return None;
    }
    let idx = entries().iter().position(|x| std::ptr::eq(x, e))?;
    Some((t.start..t.end, idx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_help(sql: &str, i: usize, d: Dialect) -> String {
        crate::i18n::set_ru(true);
        at(sql, i, d).map_or("—".into(), |(_, k)| entries()[k].title())
    }

    #[test]
    fn reference_reads_and_finds() {
        let all = entries();
        assert!(all.len() > 120, "{}", all.len());
        for e in all {
            assert!(!e.sig.is_empty() && !e.ru.is_empty() && !e.en.is_empty() && !e.example.is_empty(), "{:?}", e.names);
        }
        // The dialect's own first: length counts bytes in ClickHouse.
        assert!(lookup("length", Dialect::ClickHouse, true).unwrap().about().contains("БАЙТАХ") || lookup("length", Dialect::ClickHouse, true).unwrap().en[0].contains("BYTES"));
        assert_eq!(lookup("length", Dialect::Postgres, true).unwrap().dialects, ["pg", "ora", "lite"]);
        // A call or a keyword: LEFT JOIN or left(s, n).
        assert_eq!(lookup("left", Dialect::Postgres, false).unwrap().kind, 'k');
        assert_eq!(lookup("LEFT", Dialect::Postgres, true).unwrap().kind, 'f');
        // Operators; ones a database lacks aren't found.
        assert!(lookup("::", Dialect::Postgres, false).is_some());
        assert!(lookup("::", Dialect::Oracle, false).is_none());
        assert!(lookup("coalesce", Dialect::Oracle, true).is_some());
        assert!(lookup("nvl", Dialect::Postgres, true).is_none());
        // Where it stands: ORDER BY in KEEP, OVER, WITHIN GROUP, an
        // aggregate, a MergeTree table; DENSE_RANK in KEEP; BY as its ORDER.
        let title = |sql: &str, word: &str, d: Dialect| {
            let at = sql.find(word).unwrap() + 1;
            at_help(sql, at, d)
        };
        let q = "select max(price) keep (dense_rank last order by d) over (partition by p order by d rows between unbounded preceding and current row), listagg(x, ',') within group (order by x) from t order by 1 nulls last";
        assert_eq!(title(q, "order by d)", Dialect::Oracle), "ORDER BY в KEEP (…)");
        assert_eq!(title(q, "dense_rank", Dialect::Oracle), "DENSE_RANK в KEEP (…)");
        assert_eq!(title(q, "last order", Dialect::Oracle), "FIRST / LAST в KEEP (…)");
        assert_eq!(title(q, "order by d rows", Dialect::Oracle), "ORDER BY в OVER (…)");
        assert_eq!(title(q, "by d rows", Dialect::Oracle), "ORDER BY в OVER (…)");
        assert_eq!(title(q, "unbounded", Dialect::Oracle), "Границы рамки окна");
        assert_eq!(title(q, "order by x", Dialect::Oracle), "ORDER BY в WITHIN GROUP (…)");
        assert_eq!(title(q, "group (", Dialect::Oracle), "WITHIN GROUP");
        assert_eq!(title(q, "order by 1", Dialect::Oracle), "order");
        assert_eq!(title("select string_agg(a, ',' order by a) from t", "order", Dialect::Postgres), "ORDER BY в агрегате");
        assert_eq!(title("create table e (d Date) engine = MergeTree order by d", "order", Dialect::ClickHouse), "ORDER BY в таблице MergeTree");
        assert_eq!(title("create table e engine = MergeTree order by d as select * from t order by d", "order by d as", Dialect::ClickHouse), "ORDER BY в таблице MergeTree");
        assert_eq!(title("select distinct on (a) a from t", "on (", Dialect::Postgres), "DISTINCT ON");
        assert_eq!(title("select case when a then 1 end from t join u on 1 = 1", "when", Dialect::Postgres), "CASE … WHEN … THEN … ELSE … END");
        assert_eq!(title("select 1 from t join u on 1 = 1", "on 1", Dialect::Postgres), "ON в JOIN");
        assert_eq!(title("select * from t start with a is null connect by prior id = p", "with", Dialect::Oracle), "START WITH");
        // An example keeps its lines.
        assert!(lookup("over", Dialect::Postgres, false).unwrap().example().lines().count() > 2);
    }
}
