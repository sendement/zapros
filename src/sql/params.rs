//! Parameters typed in a script: `:name` (a value: quoted or not as it
//! is) and `&name` (`&&name`; text put in as it is, SQL*Plus style), asked
//! for before it runs. Strings and comments are left alone, and so are
//! `::type` casts, `:=`, a trigger's `:new` / `:old`, `a & b`.

use std::ops::Range;

use super::Dialect;
use super::lex::{self, Tk};

/// How a `:name`'s value goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum As {
    /// A number bare, anything else in quotes, nothing: NULL.
    #[default]
    Auto,
    Text,
    Number,
    /// As typed (an expression, a date literal…).
    Raw,
    Null,
}

impl As {
    pub const ALL: [As; 5] = [As::Auto, As::Text, As::Number, As::Raw, As::Null];

    pub fn name(self) -> &'static str {
        use crate::i18n::t;
        match self {
            As::Auto => t("Авто", "Auto"),
            As::Text => t("Текст", "Text"),
            As::Number => t("Число", "Number"),
            As::Raw => t("Как есть", "As is"),
            As::Null => "NULL",
        }
    }
}

/// A parameter: its name, `&` (text) or `:` (a value).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Param {
    pub name: String,
    pub amp: bool,
}

impl Param {
    /// As written: ":id", "&table".
    pub fn shown(&self) -> String {
        format!("{}{}", if self.amp { "&" } else { ":" }, self.name)
    }

    /// Its key among remembered values (names compared in any case).
    pub fn key(&self) -> String {
        self.shown().to_lowercase()
    }
}

/// Where the parameters are in a statement.
pub fn find(sql: &str, d: Dialect) -> Vec<(Range<usize>, Param)> {
    let toks = lex::lex(sql, d);
    let trigger = sql.to_ascii_uppercase().contains("TRIGGER");
    let mut out = Vec::new();
    // `:name`: the lexer's.
    for t in toks.iter().filter(|t| t.kind == Tk::Param) {
        let text = t.text(sql);
        let Some(name) = text.strip_prefix(':') else { continue };
        if name.is_empty() || (trigger && matches!(name.to_ascii_lowercase().as_str(), "new" | "old")) {
            continue;
        }
        out.push((t.start..t.end, Param { name: name.to_string(), amp: false }));
    }
    // `&name`, `&&name` (a "." right after ends the name and goes too),
    // outside strings and comments, not after a name ("a&b").
    let dead: Vec<Range<usize>> = toks.iter().filter(|t| matches!(t.kind, Tk::Str | Tk::LineComment | Tk::BlockComment | Tk::Quoted)).map(|t| t.start..t.end).collect();
    let b = sql.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'&' || dead.iter().any(|r| r.contains(&i)) || (i > 0 && (b[i - 1].is_ascii_alphanumeric() || matches!(b[i - 1], b'_' | b')' | b'&'))) {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        if b.get(j) == Some(&b'&') {
            j += 1;
        }
        let name_start = j;
        while j < b.len() && (sql[j..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_')) {
            j += sql[j..].chars().next().map_or(1, char::len_utf8);
        }
        if j == name_start || !sql[name_start..].starts_with(|c: char| c.is_alphabetic() || c == '_') {
            i = j.max(i + 1);
            continue;
        }
        let name = sql[name_start..j].to_string();
        let end = if b.get(j) == Some(&b'.') { j + 1 } else { j };
        out.push((start..end, Param { name, amp: true }));
        i = end;
    }
    out.sort_by_key(|(r, _)| r.start);
    out
}

/// The parameters of statements, each once, in order.
pub fn names<'a>(sqls: impl IntoIterator<Item = &'a str>, d: Dialect) -> Vec<Param> {
    let mut out: Vec<Param> = Vec::new();
    for s in sqls {
        for (_, p) in find(s, d) {
            if !out.iter().any(|x| x.key() == p.key()) {
                out.push(p);
            }
        }
    }
    out
}

fn is_number(v: &str) -> bool {
    let s = v.trim().strip_prefix(['-', '+']).unwrap_or(v.trim());
    !s.is_empty() && s.parse::<f64>().is_ok_and(f64::is_finite) && s.starts_with(|c: char| c.is_ascii_digit() || c == '.') && !(s.len() > 1 && s.starts_with('0') && !s.starts_with("0."))
}

/// A value as it goes in.
pub fn value(v: &str, how: As, d: Dialect) -> String {
    let quoted = || if d == Dialect::ClickHouse { crate::db::ch::literal(v) } else { format!("'{}'", v.replace('\'', "''")) };
    match how {
        As::Null => "NULL".into(),
        As::Raw => v.to_string(),
        As::Number => v.trim().to_string(),
        As::Text => quoted(),
        As::Auto if v.is_empty() => "NULL".into(),
        As::Auto if is_number(v) => v.trim().to_string(),
        As::Auto => quoted(),
    }
}

/// The statement with its parameters put in (`get`: a parameter's text,
/// None — left as it is).
pub fn put(sql: &str, d: Dialect, get: impl Fn(&Param) -> Option<String>) -> String {
    let mut out = sql.to_string();
    for (r, p) in find(sql, d).into_iter().rev() {
        if let Some(v) = get(&p) {
            out.replace_range(r, &v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_and_put() {
        let sql = "select a::text, x := 1, '&no :no' -- &no :no\n, :id, :Name, &&tbl., &col_x from t where a & b and c&d and n = :id and s = &s";
        let found: Vec<String> = find(sql, Dialect::Postgres).into_iter().map(|(_, p)| p.shown()).collect();
        assert_eq!(found, [":id", ":Name", "&tbl", "&col_x", ":id", "&s"]);
        assert_eq!(names([sql], Dialect::Postgres).len(), 5);
        let v = |p: &Param| Some(match p.shown().as_str() {
            ":id" => value("42", As::Auto, Dialect::Postgres),
            ":Name" => value("O'Neil", As::Auto, Dialect::Postgres),
            "&tbl" => "orders".into(),
            "&col_x" => "total".into(),
            "&s" => value("", As::Auto, Dialect::Postgres),
            _ => return None,
        });
        assert_eq!(put(sql, Dialect::Postgres, v), "select a::text, x := 1, '&no :no' -- &no :no\n, 42, 'O''Neil', orders, total from t where a & b and c&d and n = 42 and s = NULL");
        // A trigger's :new / :old aren't asked for.
        let trg = "create trigger t before insert on x for each row begin :new.id := :seq; end;";
        assert_eq!(find(trg, Dialect::Oracle).into_iter().map(|(_, p)| p.shown()).collect::<Vec<_>>(), [":seq"]);
        assert_eq!(value("007", As::Auto, Dialect::Oracle), "'007'");
        assert_eq!(value("2.5", As::Auto, Dialect::Oracle), "2.5");
        assert_eq!(value("sysdate", As::Raw, Dialect::Oracle), "sysdate");
    }
}
