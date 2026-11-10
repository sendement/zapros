//! SQL text: tokens, statements, checks, layout and hints, per dialect.

pub mod check;
pub mod complete;
pub mod embed;
pub mod fmt;
pub mod lex;
pub mod outline;

use std::ops::Range;

use lex::{Tk, Tok};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Dialect {
    #[default]
    Generic,
    Postgres,
    ClickHouse,
    Oracle,
    Sqlite,
}

impl Dialect {
    pub const ALL: [Dialect; 5] = [Dialect::Generic, Dialect::Postgres, Dialect::ClickHouse, Dialect::Oracle, Dialect::Sqlite];

    pub fn name(self) -> &'static str {
        match self {
            Dialect::Generic => "SQL",
            Dialect::Postgres => "PostgreSQL",
            Dialect::ClickHouse => "ClickHouse",
            Dialect::Oracle => "Oracle",
            Dialect::Sqlite => "SQLite",
        }
    }
}

const COMMON: &[&str] = &[
    "ADD", "ALL", "ALTER", "AND", "ANY", "AS", "ASC", "BEGIN", "BETWEEN", "BY", "CASCADE", "CASE", "CAST", "CHECK", "COLUMN", "COMMENT", "COMMIT", "CONSTRAINT", "CREATE", "CROSS", "CURRENT_DATE",
    "CURRENT_TIMESTAMP", "DEFAULT", "DELETE", "DESC", "DISTINCT", "DROP", "ELSE", "END", "EXCEPT", "EXISTS", "FALSE", "FETCH", "FIRST", "FOR", "FOREIGN", "FROM", "FULL", "FUNCTION", "GRANT", "GROUP",
    "HAVING", "IF", "IN", "INDEX", "INNER", "INSERT", "INTERSECT", "INTERVAL", "INTO", "IS", "JOIN", "KEY", "LAST", "LEFT", "LIKE", "LIMIT", "NATURAL", "NOT", "NULL", "NULLS", "OF", "OFFSET", "ON", "OR",
    "ORDER", "OUTER", "OVER", "PARTITION", "PRIMARY", "PROCEDURE", "RANGE", "RECURSIVE", "REFERENCES", "RENAME", "REPLACE", "REVOKE", "RIGHT", "ROLLBACK", "ROW", "ROWS", "SCHEMA", "SELECT", "SET", "TABLE",
    "THEN", "TO", "TRIGGER", "TRUE", "TRUNCATE", "UNION", "UNIQUE", "UPDATE", "USING", "VALUES", "VIEW", "WHEN", "WHERE", "WINDOW", "WITH", "PRECEDING", "FOLLOWING", "UNBOUNDED", "CURRENT", "EXPLAIN",
    "MERGE", "MATCHED", "SEQUENCE", "TEMPORARY", "TEMP", "LATERAL", "ONLY", "SOME", "FILTER", "WITHIN", "COLLATE", "DATABASE", "USE", "SHOW", "DESCRIBE", "ANALYZE", "GRANTED", "BOTH", "LEADING", "TRAILING",
];

const PG: &[&str] = &[
    "ILIKE", "RETURNING", "CONFLICT", "DO", "NOTHING", "LANGUAGE", "RETURNS", "DECLARE", "LOOP", "WHILE", "EXIT", "RAISE", "NOTICE", "EXCEPTION", "PERFORM", "RETURN", "ELSIF", "VOLATILE", "STABLE",
    "IMMUTABLE", "SECURITY", "DEFINER", "MATERIALIZED", "REFRESH", "CONCURRENTLY", "VACUUM", "EXTENSION", "OWNER", "SIMILAR", "TABLESAMPLE", "VARIADIC", "INHERITS", "UNLOGGED", "COPY", "LISTEN", "NOTIFY",
    "TYPE", "DOMAIN", "ENUM", "STRICT", "SETOF", "GENERATED", "ALWAYS", "IDENTITY", "STORED", "OVERRIDING", "LOCK", "SKIP", "LOCKED", "NOWAIT", "SHARE", "FOREACH", "ARRAY", "SLICE", "QUERY", "EXECUTE",
    "OUT", "INOUT", "CALL", "ANALYSE", "REINDEX", "CLUSTER", "TABLESPACE", "POLICY", "ROLE", "USER",
];

const ORA: &[&str] = &[
    "DECLARE", "LOOP", "WHILE", "EXIT", "EXCEPTION", "RAISE", "RETURN", "ELSIF", "PACKAGE", "BODY", "TYPE", "IS", "OUT", "NOCOPY", "PRAGMA", "AUTONOMOUS_TRANSACTION", "CONNECT", "START", "PRIOR", "LEVEL",
    "ROWNUM", "ROWID", "SYSDATE", "SYSTIMESTAMP", "DUAL", "MINUS", "MERGE", "PIVOT", "UNPIVOT", "BULK", "COLLECT", "FORALL", "SAVE", "EXCEPTIONS", "CURSOR", "OPEN", "CLOSE", "RECORD", "ROWTYPE", "NOCYCLE",
    "SIBLINGS", "MODEL", "PURGE", "FLASHBACK", "SYNONYM", "PUBLIC", "EXECUTE", "IMMEDIATE", "USING", "RETURNING", "GOTO", "WHENEVER", "SQLERROR", "EDITIONABLE", "NONEDITIONABLE", "DETERMINISTIC",
    "PIPELINED", "PARALLEL_ENABLE", "RESULT_CACHE", "AUTHID", "DEFINER", "CURRENT_USER", "OTHERS", "NO_DATA_FOUND", "TOO_MANY_ROWS", "NOWAIT", "LOCK", "MODE", "NOLOGGING", "COMPRESS", "FORCE", "VIRTUAL",
];

const CH: &[&str] = &[
    "ARRAY", "FINAL", "SAMPLE", "PREWHERE", "SETTINGS", "FORMAT", "ENGINE", "TTL", "CLUSTER", "GLOBAL", "ANY", "ASOF", "SEMI", "ANTI", "PASTE", "LIMIT", "OPTIMIZE", "DEDUPLICATE", "ATTACH", "DETACH",
    "MATERIALIZED", "POPULATE", "DICTIONARY", "SOURCE", "LIFETIME", "LAYOUT", "SYSTEM", "KILL", "QUERY", "SYNC", "ASYNC", "ILIKE", "TOTALS", "CUBE", "ROLLUP", "FILL", "STEP", "INTERPOLATE", "TIES",
    "DATABASES", "TABLES", "PROCESSLIST", "MODIFY", "CODEC", "ALIAS", "EPHEMERAL", "PROJECTION", "MUTATION", "WATCH", "LIVE", "REFRESH", "EVERY", "APPEND", "EXCHANGE", "QUALIFY",
];

const LITE: &[&str] = &["AUTOINCREMENT", "PRAGMA", "VACUUM", "ATTACH", "DETACH", "GLOB", "REGEXP", "ABORT", "FAIL", "IGNORE", "CONFLICT", "INSTEAD", "RAISE", "WITHOUT", "ROWID", "STRICT", "RETURNING", "DO", "NOTHING", "INDEXED"];

pub fn keywords(d: Dialect) -> Vec<&'static str> {
    let extra: &[&str] = match d {
        Dialect::Postgres => PG,
        Dialect::Oracle => ORA,
        Dialect::ClickHouse => CH,
        Dialect::Sqlite => LITE,
        Dialect::Generic => &[],
    };
    let mut v: Vec<&str> = COMMON.iter().chain(extra).copied().collect();
    v.sort_unstable();
    v.dedup();
    v
}

pub fn is_keyword(w: &str, d: Dialect) -> bool {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    static SETS: OnceLock<Vec<HashSet<&'static str>>> = OnceLock::new();
    let sets = SETS.get_or_init(|| Dialect::ALL.iter().map(|d| keywords(*d).into_iter().collect()).collect());
    if w.len() > 24 || !w.is_ascii() {
        return false;
    }
    let up = w.to_ascii_uppercase();
    sets[Dialect::ALL.iter().position(|x| *x == d).unwrap()].contains(up.as_str())
}

pub const FUNCTIONS: &[&str] = &[
    "ABS", "AVG", "CEIL", "COALESCE", "CONCAT", "COUNT", "DENSE_RANK", "EXTRACT", "FIRST_VALUE", "FLOOR", "GREATEST", "LAG", "LAST_VALUE", "LEAD", "LEAST", "LENGTH", "LOWER", "LPAD", "LTRIM", "MAX", "MIN",
    "MOD", "NTILE", "NULLIF", "POWER", "RANK", "REPLACE", "ROUND", "ROW_NUMBER", "RPAD", "RTRIM", "SUBSTR", "SUBSTRING", "SUM", "TRIM", "UPPER", "TRUNC", "SQRT", "SIGN", "EXP", "LN", "LOG",
];

pub fn functions(d: Dialect) -> Vec<&'static str> {
    let extra: &[&str] = match d {
        Dialect::Postgres => &[
            "NOW", "DATE_TRUNC", "DATE_PART", "TO_CHAR", "TO_DATE", "TO_TIMESTAMP", "STRING_AGG", "ARRAY_AGG", "JSON_AGG", "JSONB_AGG", "JSONB_BUILD_OBJECT", "JSON_BUILD_OBJECT", "GENERATE_SERIES",
            "UNNEST", "REGEXP_REPLACE", "REGEXP_MATCHES", "SPLIT_PART", "STRING_TO_ARRAY", "ARRAY_LENGTH", "AGE", "MAKE_DATE", "CURRENT_SETTING", "SET_CONFIG", "PG_SLEEP", "FORMAT", "INITCAP",
            "BOOL_AND", "BOOL_OR", "PERCENTILE_CONT", "MODE", "JSONB_SET", "JSONB_EXTRACT_PATH_TEXT", "TO_JSONB", "ROW_TO_JSON", "PG_SIZE_PRETTY", "PG_TOTAL_RELATION_SIZE", "NEXTVAL", "CURRVAL",
            "SETVAL", "MD5", "GEN_RANDOM_UUID", "CLOCK_TIMESTAMP", "DATE_BIN", "LEFT", "RIGHT", "REVERSE", "POSITION", "CHAR_LENGTH",
        ],
        Dialect::Oracle => &[
            "NVL", "NVL2", "DECODE", "TO_CHAR", "TO_DATE", "TO_NUMBER", "TO_TIMESTAMP", "SYSDATE", "ADD_MONTHS", "MONTHS_BETWEEN", "LAST_DAY", "NEXT_DAY", "LISTAGG", "INSTR", "REGEXP_SUBSTR",
            "REGEXP_REPLACE", "REGEXP_LIKE", "REGEXP_INSTR", "INITCAP", "TRANSLATE", "SYS_GUID", "SYS_CONTEXT", "USERENV", "XMLAGG", "XMLELEMENT", "JSON_VALUE", "JSON_QUERY", "JSON_TABLE",
            "JSON_OBJECT", "NUMTODSINTERVAL", "NUMTOYMINTERVAL", "RATIO_TO_REPORT", "KEEP", "CHR", "ASCII", "LENGTHB", "RAWTOHEX", "HEXTORAW", "DBMS_OUTPUT.PUT_LINE",
        ],
        Dialect::ClickHouse => &[
            "now", "today", "yesterday", "toDate", "toDateTime", "toStartOfDay", "toStartOfHour", "toStartOfMonth", "toStartOfWeek", "toYYYYMM", "toYYYYMMDD", "formatDateTime", "dateDiff", "addDays",
            "toString", "toUInt32", "toUInt64", "toInt32", "toInt64", "toFloat64", "toDecimal64", "uniq", "uniqExact", "uniqCombined", "groupArray", "groupUniqArray", "arrayJoin", "arrayMap", "arrayFilter",
            "has", "hasAny", "hasAll", "length", "empty", "notEmpty", "countIf", "sumIf", "avgIf", "anyIf", "argMax", "argMin", "any", "anyLast", "quantile", "quantiles", "quantileExact", "median",
            "topK", "if", "multiIf", "ifNull", "isNull", "isNotNull", "assumeNotNull", "toTypeName", "JSONExtractString", "JSONExtractInt", "JSONExtract", "visitParamExtractString", "splitByChar",
            "splitByString", "arrayStringConcat", "position", "positionCaseInsensitive", "match", "extract", "replaceAll", "replaceRegexpAll", "lower", "upper", "lowerUTF8", "upperUTF8", "substring",
            "cityHash64", "sipHash64", "rand", "generateUUIDv4", "dictGet", "dictGetOrDefault", "runningDifference", "neighbor", "toUnixTimestamp", "fromUnixTimestamp", "intDiv", "round", "sum", "count",
            "avg", "min", "max",
        ],
        Dialect::Sqlite => &["DATE", "TIME", "DATETIME", "JULIANDAY", "STRFTIME", "IFNULL", "IIF", "INSTR", "PRINTF", "GROUP_CONCAT", "JSON_EXTRACT", "JSON_OBJECT", "JSON_ARRAY", "RANDOM", "TYPEOF", "TOTAL", "CHANGES", "LAST_INSERT_ROWID", "HEX", "QUOTE", "UNICODE", "CHAR", "LIKELIHOOD", "ZEROBLOB"],
        Dialect::Generic => &[],
    };
    let mut v: Vec<&str> = FUNCTIONS.iter().chain(extra).copied().collect();
    v.sort_unstable_by_key(|f| f.to_ascii_uppercase());
    v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    v
}

pub const TYPES: &[&str] = &[
    "BIGINT", "BIGSERIAL", "BINARY_DOUBLE", "BINARY_FLOAT", "BLOB", "BOOL", "BOOLEAN", "BPCHAR", "BYTEA", "CHAR", "CLOB", "DATE", "DATETIME", "DATETIME64", "DECIMAL", "DOUBLE", "FLOAT", "FLOAT32", "FLOAT64",
    "INT", "INT8", "INT16", "INT32", "INT64", "INT2", "INT4", "INTEGER", "JSON", "JSONB", "LONG", "LOWCARDINALITY", "MONEY", "NCHAR", "NCLOB", "NULLABLE", "NUMBER", "NUMERIC", "NVARCHAR2", "RAW", "REAL",
    "SERIAL", "SMALLINT", "STRING", "TEXT", "TIME", "TIMESTAMP", "TIMESTAMPTZ", "UINT8", "UINT16", "UINT32", "UINT64", "UUID", "VARCHAR", "VARCHAR2", "XMLTYPE", "INET", "CIDR", "TSVECTOR", "ENUM8",
    "ENUM16", "FIXEDSTRING", "MAP", "TUPLE", "NESTED", "IPV4", "IPV6", "DATE32", "PLS_INTEGER", "BINARY_INTEGER", "INTERVAL",
];

pub fn is_type(w: &str) -> bool {
    w.len() <= 16 && TYPES.iter().any(|t| t.eq_ignore_ascii_case(w))
}

/// The first word of a statement (comments skipped).
pub fn first_word(sql: &str) -> &str {
    let t = lex::lex(sql, Dialect::Generic);
    t.iter().find(|t| !t.trivia()).filter(|t| matches!(t.kind, Tk::Keyword | Tk::Word)).map_or("", |t| t.text(sql))
}

/// A quoted name's text: without its quotes (the closing one may be
/// missing while it's being typed).
pub fn unquote(s: &str) -> &str {
    let inner = s.get(1..).unwrap_or("");
    inner.strip_suffix(['"', '`', ']']).unwrap_or(inner)
}

/// The first `n` words of a statement, lower case (comments skipped).
pub fn first_words(sql: &str, n: usize) -> Vec<String> {
    let t = lex::lex(sql, Dialect::Generic);
    lead_words(sql, &t, n).into_iter().map(|w| w.to_lowercase()).collect()
}

/// Words a statement starts with, upper-cased (comments skipped).
fn lead_words(src: &str, toks: &[Tok], n: usize) -> Vec<String> {
    toks.iter().filter(|t| !t.trivia()).take(n).map(|t| t.text(src).to_ascii_uppercase()).collect()
}

/// An Oracle PL/SQL unit: a block or CREATE of something with a body;
/// its ";"s are inside it, it ends with "/".
pub fn is_plsql_block(sql: &str, d: Dialect) -> bool {
    if d != Dialect::Oracle {
        return false;
    }
    let toks = lex::lex(sql, d);
    plsql_start(sql, &toks)
}

fn plsql_start(src: &str, toks: &[Tok]) -> bool {
    let w = lead_words(src, toks, 7);
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    match w.as_slice() {
        ["DECLARE", ..] | ["BEGIN", ..] => true,
        ["CREATE", rest @ ..] => {
            let rest: Vec<&str> = rest.iter().copied().filter(|x| !matches!(*x, "OR" | "REPLACE" | "EDITIONABLE" | "NONEDITIONABLE" | "EDITIONING")).collect();
            matches!(rest.first(), Some(&("FUNCTION" | "PROCEDURE" | "PACKAGE" | "TRIGGER" | "TYPE")))
        }
        _ => false,
    }
}

/// Where statements are in a text: split at ";" outside brackets and
/// strings; Oracle blocks run to their "/". With `blank_lines`, an empty
/// line ends a statement too (when not inside one's brackets or a block).
pub fn split(src: &str, d: Dialect, blank_lines: bool) -> Vec<Range<usize>> {
    let toks = lex::lex(src, d);
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut depth = 0i32;
    let mut block = false;
    let mut first_tok = 0;
    for (k, t) in toks.iter().enumerate() {
        if start.is_none() {
            if t.trivia() || t.kind == Tk::Semi || t.kind == Tk::Slash {
                continue;
            }
            start = Some(t.start);
            first_tok = k;
            depth = 0;
            block = d == Dialect::Oracle && plsql_start(src, &toks[k..(k + 16).min(toks.len())]);
        }
        let s = start.unwrap();
        match t.kind {
            Tk::Open => depth += 1,
            Tk::Close => depth -= 1,
            // ";" ends a statement even with a bracket left open: the
            // next one isn't swallowed by the mistake.
            Tk::Semi if !block => {
                out.push(s..t.end);
                start = None;
            }
            Tk::Slash => {
                let end = toks[first_tok..k].iter().rev().find(|t| !t.trivia()).map_or(t.start, |t| t.end);
                out.push(s..end);
                start = None;
            }
            Tk::Space if blank_lines && !block && depth <= 0 && t.text(src).matches('\n').count() >= 2 => {
                let end = toks[first_tok..k].iter().rev().find(|t| !t.trivia()).map_or(t.start, |t| t.end);
                // A comment alone above: not a statement.
                if toks[first_tok..k].iter().any(|t| !t.trivia()) {
                    out.push(s..end);
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        let end = toks.iter().rev().find(|t| !t.trivia()).map_or(src.len(), |t| t.end);
        if toks.iter().any(|t| t.start >= s && !t.trivia()) && end > s {
            out.push(s..end);
        }
    }
    // A statement that's only comments isn't one (ranges start at a solid
    // token, so: one that isn't a lone ";").
    out.retain(|r| {
        let k = toks.partition_point(|t| t.start < r.start);
        toks[k..].iter().take_while(|t| t.end <= r.end).any(|t| !t.trivia() && t.kind != Tk::Semi)
    });
    out
}

/// The statement the cursor is in (or the one just before it on its line).
pub fn statement_at(src: &str, at: usize, d: Dialect, blank_lines: bool) -> Option<Range<usize>> {
    let all = split(src, d, blank_lines);
    if let Some(r) = all.iter().find(|r| r.start <= at && at <= r.end) {
        return Some(r.clone());
    }
    // After the ";": the statement before, while still on its line.
    let line_start = src[..crate::editor::floor(src, at)].rfind('\n').map_or(0, |i| i + 1);
    all.iter().rev().find(|r| r.end <= at && r.end + 1 >= line_start && src[r.end..at].trim_start_matches(';').trim().is_empty()).cloned()
}

/// Text without its trailing ";" (Oracle blocks keep theirs).
pub fn runnable(sql: &str, d: Dialect) -> String {
    let s = sql.trim();
    if is_plsql_block(s, d) {
        return s.trim_end_matches('/').trim_end().to_string();
    }
    if d == Dialect::Postgres {
        return s.to_string();
    }
    s.trim_end_matches(';').trim_end().to_string()
}

/// Upper or lower case words as the setting wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Case {
    #[default]
    Upper,
    Lower,
    AsIs,
}

impl Case {
    pub fn apply(self, w: &str) -> String {
        match self {
            Case::Upper => w.to_uppercase(),
            Case::Lower => w.to_lowercase(),
            Case::AsIs => w.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits() {
        let s = "select 1; select ';' from t;\n\nselect 2";
        let r: Vec<&str> = split(s, Dialect::Postgres, false).into_iter().map(|r| &s[r]).collect();
        assert_eq!(r, vec!["select 1;", "select ';' from t;", "select 2"]);
        let s = "select 1\nfrom dual\n\nselect 2 from dual";
        let r: Vec<&str> = split(s, Dialect::Oracle, true).into_iter().map(|r| &s[r]).collect();
        assert_eq!(r, vec!["select 1\nfrom dual", "select 2 from dual"]);
        let s = "create or replace procedure p is\nbegin\n  null;\n\n  null;\nend;\n/\nselect 1 from dual;";
        let r: Vec<&str> = split(s, Dialect::Oracle, true).into_iter().map(|r| &s[r]).collect();
        assert_eq!(r, vec!["create or replace procedure p is\nbegin\n  null;\n\n  null;\nend;", "select 1 from dual;"]);
        let s = "do $$ begin raise notice 'x;'; end $$;\nselect 1;";
        assert_eq!(split(s, Dialect::Postgres, true).len(), 2);
    }

    #[test]
    fn statement_under_cursor() {
        let s = "select 1;\nselect 2;\n";
        assert_eq!(&s[statement_at(s, 12, Dialect::Postgres, true).unwrap()], "select 2;");
        assert_eq!(&s[statement_at(s, 9, Dialect::Postgres, true).unwrap()], "select 1;");
    }
}
