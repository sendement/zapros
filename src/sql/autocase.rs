//! Keywords taking the style's case while typing: when a word is ended
//! (a space, a bracket, a comma…), SQL's own words — and only where they
//! are keywords — get upper / lower case. Names, functions (ClickHouse's
//! are case-sensitive), strings, comments, `x.word` stay as typed.

use std::ops::Range;

use super::lex::{self, Tk};
use super::{Case, Dialect};

/// Keywords anywhere.
const ALWAYS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "IS", "NULL", "LIKE", "ILIKE", "BETWEEN", "JOIN", "INNER", "OUTER", "FULL", "CROSS", "LEFT", "RIGHT", "ON", "USING", "GROUP", "ORDER", "BY",
    "HAVING", "LIMIT", "OFFSET", "UNION", "ALL", "DISTINCT", "AS", "CASE", "WHEN", "THEN", "ELSE", "END", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "DROP", "ALTER", "WITH",
    "EXISTS", "ASC", "DESC", "NULLS", "TRUE", "FALSE", "RETURNING", "TRUNCATE", "GRANT", "REVOKE", "INTERSECT", "EXCEPT", "OVER", "PARTITION", "WINDOW", "RECURSIVE", "PRIMARY", "FOREIGN",
    "REFERENCES", "UNIQUE", "CHECK", "CONSTRAINT", "COMMIT", "ROLLBACK", "EXPLAIN",
];

/// Per database.
fn own(d: Dialect) -> &'static [&'static str] {
    match d {
        Dialect::ClickHouse => &["PREWHERE", "FINAL", "SAMPLE", "SETTINGS", "ENGINE", "GLOBAL", "ARRAY", "TTL", "OPTIMIZE", "SYSTEM", "SHOW", "DESCRIBE"],
        Dialect::Oracle => &[
            "DECLARE", "BEGIN", "EXCEPTION", "LOOP", "EXIT", "ELSIF", "RETURN", "IS", "CURSOR", "OPEN", "FETCH", "CLOSE", "RAISE", "FOR", "WHILE", "IF", "MINUS", "CONNECT", "PRIOR", "BULK",
            "COLLECT", "FORALL", "IMMEDIATE", "EXECUTE", "PRAGMA", "MERGE", "MATCHED",
        ],
        Dialect::Postgres => &["DECLARE", "BEGIN", "RETURNS", "RETURN", "LOOP", "EXIT", "ELSIF", "IF", "RAISE", "PERFORM", "EXECUTE", "FOR", "WHILE", "LATERAL", "CONFLICT", "DO", "NOTHING", "VACUUM", "ANALYZE"],
        _ => &["PRAGMA", "VACUUM", "CONFLICT", "DO", "NOTHING", "REPLACE"],
    }
}

/// Keywords only after certain words (else often names: a column `key`…).
fn after(w: &str, prev: &str) -> bool {
    match w {
        "TABLE" | "VIEW" | "INDEX" | "SEQUENCE" | "TRIGGER" | "SCHEMA" | "DATABASE" | "FUNCTION" | "PROCEDURE" | "PACKAGE" | "MATERIALIZED" | "DICTIONARY" => {
            matches!(prev, "CREATE" | "DROP" | "ALTER" | "TRUNCATE" | "REPLACE" | "MATERIALIZED" | "TEMPORARY" | "TEMP" | "UNIQUE" | "EXISTS" | "OR" | "PACKAGE" | "BODY")
        }
        "BODY" => prev == "PACKAGE",
        "KEY" => matches!(prev, "PRIMARY" | "FOREIGN"),
        "FIRST" | "LAST" => prev == "NULLS",
        "COLUMN" => matches!(prev, "ADD" | "DROP" | "ALTER" | "MODIFY" | "RENAME"),
        "ADD" | "MODIFY" | "RENAME" => matches!(prev, "TABLE") || prev.is_empty(),
        "REPLACE" => prev == "OR",
        "EXISTS" | "NOT" => true,
        _ => false,
    }
}

/// What a `(` right after the word may follow (else it's a call).
const BEFORE_BRACKET: &[&str] = &["IN", "EXISTS", "VALUES", "AS", "USING", "ON", "OVER", "NOT", "FROM", "JOIN", "WHERE", "AND", "OR", "SELECT", "THEN", "ELSE", "WHEN", "INTO", "ALL", "UNION"];

/// The text was `text` with a delimiter just typed at `at` (the byte it's
/// at): the word before it, to be put in `case` — (its range, its text).
pub fn on_typed(text: &str, at: usize, d: Dialect, case: Case) -> Option<(Range<usize>, String)> {
    if case == Case::AsIs || at == 0 || at > text.len() || !text.is_char_boundary(at) {
        return None;
    }
    let typed = text[at..].chars().next()?;
    if !matches!(typed, ' ' | '\n' | '\t' | '(' | ')' | ',' | ';') {
        return None;
    }
    let before = &text[..at];
    let start = before.char_indices().rev().take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_').last().map(|(i, _)| i)?;
    let word = &before[start..];
    if word.is_empty() || !word.is_ascii() || word.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    // Not part of x.word, a parameter, a quoted name.
    if before[..start].ends_with(|c: char| matches!(c, '.' | ':' | '@' | '$' | '"' | '`' | '[' | '#')) {
        return None;
    }
    // Not inside a string or a comment (as lexed up to here).
    let toks = lex::lex(before, d);
    let tok = toks.iter().find(|t| t.start <= start && start < t.end)?;
    if !matches!(tok.kind, Tk::Word | Tk::Keyword) || tok.end != at {
        return None;
    }
    let up = word.to_ascii_uppercase();
    let prev = toks.iter().rev().filter(|t| t.end <= start && !t.trivia()).find(|_| true).filter(|t| matches!(t.kind, Tk::Word | Tk::Keyword)).map(|t| t.text(before).to_ascii_uppercase()).unwrap_or_default();
    let keyword = ALWAYS.contains(&up.as_str()) || own(d).contains(&up.as_str()) || after(&up, &prev);
    if !keyword {
        return None;
    }
    if typed == '(' && !BEFORE_BRACKET.contains(&up.as_str()) {
        return None;
    }
    let want = case.apply(word);
    (want != word).then(|| (start..at, want))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str, d: Dialect) -> String {
        // The last char was just typed.
        let at = s.len() - s.chars().last().unwrap().len_utf8();
        let mut t = s.to_string();
        if let Some((r, w)) = on_typed(s, at, d, Case::Upper) {
            t.replace_range(r, &w);
        }
        t
    }

    #[test]
    fn keywords_only() {
        let pg = Dialect::Postgres;
        assert_eq!(typed("select ", pg), "SELECT ");
        assert_eq!(typed("SELECT a from ", pg), "SELECT a FROM ");
        assert_eq!(typed("select a\n", pg), "select a\n");
        assert_eq!(typed("where x in(", pg), "where x IN(");
        // Names, x.word, strings, comments, calls stay.
        assert_eq!(typed("select t.from ", pg), "select t.from ");
        assert_eq!(typed("select 'from ", pg), "select 'from ");
        assert_eq!(typed("-- select ", pg), "-- select ");
        assert_eq!(typed("select left(", pg), "select left(");
        assert_eq!(typed("select key,", pg), "select key,");
        assert_eq!(typed("select date ", pg), "select date ");
        // Words that are keywords only somewhere.
        assert_eq!(typed("create table ", pg), "create TABLE ");
        assert_eq!(typed("select table ", pg), "select table ");
        assert_eq!(typed("primary key ", pg), "primary KEY ");
        assert_eq!(typed("order by x nulls first ", pg), "order by x nulls FIRST ");
    }

    #[test]
    fn clickhouse_keeps_its_functions_and_names() {
        let ch = Dialect::ClickHouse;
        assert_eq!(typed("select ", ch), "SELECT ");
        assert_eq!(typed("select if(", ch), "select if(");
        assert_eq!(typed("select any(", ch), "select any(");
        assert_eq!(typed("select toDate(", ch), "select toDate(");
        assert_eq!(typed("from t final ", ch), "from t FINAL ");
        assert_eq!(typed("from default.events prewhere ", ch), "from default.events PREWHERE ");
        assert_eq!(typed("select default ", ch), "select default ");
        // Lower case wanted: down.
        assert_eq!(on_typed("SELECT ", 6, ch, Case::Lower), Some((0..6, "select".into())));
        assert_eq!(on_typed("select ", 6, ch, Case::AsIs), None);
    }
}
