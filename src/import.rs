//! A table made from a file: CSV / TSV, Excel / LibreOffice sheets, JSON.
//! The file read into rows of text; each column's type guessed (and the
//! SQL type for it per database); the CREATE, the indexes and the INSERTs.

use std::path::Path;

use crate::db::Kind;
use crate::design::{Column, IndexDef, TableDef};
use crate::sql::complete::quote;


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Sheet,
    Json,
}

impl Format {
    pub fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        Some(match ext.as_str() {
            "csv" | "tsv" | "txt" => Format::Csv,
            "xlsx" | "xlsm" | "xls" | "xlsb" | "ods" => Format::Sheet,
            "json" | "ndjson" | "jsonl" => Format::Json,
            _ => return None,
        })
    }
}

/// How the file is read.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadOpts {
    /// CSV: the separator (None: guessed).
    pub sep: Option<char>,
    /// The first row: names (CSV, sheets).
    pub header: bool,
    /// A sheet by its name (None: the first).
    pub sheet: Option<String>,
}

impl Default for ReadOpts {
    fn default() -> Self {
        Self { sep: None, header: true, sheet: None }
    }
}

/// What a file holds.
#[derive(Clone, Debug, Default)]
pub struct Data {
    pub names: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// A workbook's sheets.
    pub sheets: Vec<String>,
    /// CSV: the separator used; the text's encoding.
    pub sep: char,
    pub encoding: &'static str,
}

pub fn read(path: &Path, o: &ReadOpts) -> Result<Data, String> {
    let f = Format::of(path).ok_or_else(|| crate::i18n::t("Такие файлы не читаются: нужен CSV, TSV, XLSX, XLS, ODS или JSON", "Not a file it reads: CSV, TSV, XLSX, XLS, ODS or JSON").to_string())?;
    let mut d = match f {
        Format::Csv => read_csv(path, o)?,
        Format::Sheet => read_sheet(path, o)?,
        Format::Json => read_json(path)?,
    };
    // Every row as wide as the widest; names for all.
    let w = d.rows.iter().map(Vec::len).max().unwrap_or(0).max(d.names.len());
    for r in &mut d.rows {
        r.resize(w, None);
    }
    while d.names.len() < w {
        d.names.push(String::new());
    }
    // Trailing empty rows (sheets have them) go.
    while d.rows.last().is_some_and(|r| r.iter().all(Option::is_none)) {
        d.rows.pop();
    }
    Ok(d)
}

/// UTF-8 (a BOM dropped), else Windows-1251.
fn decode(bytes: &[u8]) -> (String, &'static str) {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => (s.to_string(), "UTF-8"),
        Err(_) => (encoding_rs::WINDOWS_1251.decode(b).0.into_owned(), "Windows-1251"),
    }
}

/// The separator its first lines have most evenly.
fn guess_sep(text: &str) -> char {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).take(20).collect();
    let mut best = (',', 0usize);
    for sep in ['\t', ';', ',', '|'] {
        let counts: Vec<usize> = lines.iter().map(|l| l.matches(sep).count()).collect();
        let Some(&first) = counts.first() else { continue };
        if first == 0 {
            continue;
        }
        let even = counts.iter().filter(|&&c| c == first).count();
        let score = even * 1000 + first;
        if score > best.1 {
            best = (sep, score);
        }
    }
    best.0
}

fn read_csv(path: &Path, o: &ReadOpts) -> Result<Data, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (text, encoding) = decode(&bytes);
    let sep = o.sep.unwrap_or_else(|| if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("tsv")) { '\t' } else { guess_sep(&text) });
    let mut rd = csv::ReaderBuilder::new().delimiter(sep as u8).has_headers(false).flexible(true).from_reader(text.as_bytes());
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    for rec in rd.records() {
        let rec = rec.map_err(|e| e.to_string())?;
        rows.push(rec.iter().map(|v| (!v.is_empty()).then(|| v.to_string())).collect());
    }
    let names = if o.header && !rows.is_empty() { rows.remove(0).into_iter().map(Option::unwrap_or_default).collect() } else { Vec::new() };
    Ok(Data { names, rows, sheets: Vec::new(), sep, encoding })
}

fn cell(c: &calamine::Data) -> Option<String> {
    use calamine::{Data as C, DataType};
    match c {
        C::Empty | C::Error(_) => None,
        C::String(s) if s.is_empty() => None,
        C::String(s) => Some(s.clone()),
        C::Int(i) => Some(i.to_string()),
        C::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => Some(format!("{}", *f as i64)),
        C::Float(f) => Some(f.to_string()),
        C::Bool(b) => Some(b.to_string()),
        C::DateTime(_) => c.as_datetime().map(|t| if t.time() == chrono::NaiveTime::MIN { t.format("%Y-%m-%d").to_string() } else { t.format("%Y-%m-%d %H:%M:%S").to_string() }),
        C::DateTimeIso(s) | C::DurationIso(s) => Some(s.replace('T', " ")),
    }
}

fn read_sheet(path: &Path, o: &ReadOpts) -> Result<Data, String> {
    use calamine::Reader;
    let mut wb = calamine::open_workbook_auto(path).map_err(|e| e.to_string())?;
    let sheets = wb.sheet_names().to_vec();
    let name = o.sheet.clone().filter(|s| sheets.contains(s)).or_else(|| sheets.first().cloned()).ok_or_else(|| crate::i18n::t("В книге нет листов", "The workbook has no sheets").to_string())?;
    let range = wb.worksheet_range(&name).map_err(|e| e.to_string())?;
    let mut rows: Vec<Vec<Option<String>>> = range.rows().map(|r| r.iter().map(cell).collect()).collect();
    // (Empty rows before the table: skipped.)
    while rows.first().is_some_and(|r| r.iter().all(Option::is_none)) {
        rows.remove(0);
    }
    let names = if o.header && !rows.is_empty() { rows.remove(0).into_iter().map(Option::unwrap_or_default).collect() } else { Vec::new() };
    Ok(Data { names, rows, sheets, sep: ',', encoding: "" })
}

/// An array of objects, or one object a line; keys in the order first met.
fn read_json(path: &Path) -> Result<Data, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (text, encoding) = decode(&bytes);
    let items: Vec<serde_json::Value> = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Array(a)) => a,
        Ok(v @ serde_json::Value::Object(_)) => vec![v],
        _ => text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).map_err(|e| e.to_string())).collect::<Result<_, _>>()?,
    };
    let mut names: Vec<String> = Vec::new();
    for it in &items {
        if let serde_json::Value::Object(m) = it {
            for k in m.keys() {
                if !names.contains(k) {
                    names.push(k.clone());
                }
            }
        }
    }
    let rows = items
        .iter()
        .map(|it| {
            names
                .iter()
                .map(|k| match it.get(k) {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(s)) => Some(s.clone()),
                    Some(v) => Some(v.to_string()),
                })
                .collect()
        })
        .collect();
    Ok(Data { names, rows, sheets: Vec::new(), sep: ',', encoding })
}

/// What a column's values look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guess {
    Int,
    Big,
    Num,
    Bool,
    Date,
    Stamp,
    /// Text, as long as its longest value.
    Text(usize),
}

fn is_int(v: &str) -> bool {
    let s = v.strip_prefix(['-', '+']).unwrap_or(v);
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) && !(s.len() > 1 && s.starts_with('0'))
}

/// A number as SQL takes it: "1 234,5" → "1234.5" (None: not one).
pub fn number(v: &str) -> Option<String> {
    let s: String = v.trim().chars().filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}')).collect();
    let s = if s.contains(',') && !s.contains('.') && s.matches(',').count() == 1 { s.replace(',', ".") } else { s };
    let body = s.strip_prefix(['-', '+']).unwrap_or(&s);
    let ok = !body.is_empty() && body.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '-' | '+')) && body.starts_with(|c: char| c.is_ascii_digit() || c == '.') && s.parse::<f64>().is_ok_and(f64::is_finite);
    // (Leading zeros: a code, not a number.)
    let code = body.len() > 1 && body.starts_with('0') && !body.starts_with("0.");
    (ok && !code).then_some(s)
}

/// A date or a date and time as ISO ("2024-03-01", "2024-03-01 10:20:00"):
/// from ISO, "01.03.2024" or "01/03/2024" (day first).
pub fn date(v: &str) -> Option<(String, bool)> {
    let v = v.trim();
    let (d, t) = match v.split_once([' ', 'T']) {
        Some((d, t)) => (d, Some(t.trim_end_matches('Z'))),
        None => (v, None),
    };
    let parts: Vec<&str> = d.split(['-', '.', '/']).collect();
    if parts.len() != 3 || !parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let (y, m, dd) = if parts[0].len() == 4 && d.contains('-') {
        (parts[0], parts[1], parts[2])
    } else if parts[2].len() == 4 && !d.contains('-') {
        (parts[2], parts[1], parts[0])
    } else {
        return None;
    };
    let (y, m, dd): (i32, u32, u32) = (y.parse().ok()?, m.parse().ok()?, dd.parse().ok()?);
    chrono::NaiveDate::from_ymd_opt(y, m, dd)?;
    let day = format!("{y:04}-{m:02}-{dd:02}");
    match t {
        None => Some((day, false)),
        Some(t) => {
            let t = t.split(['+']).next().unwrap_or(t);
            let tp: Vec<&str> = t.split(':').collect();
            if !(2..=3).contains(&tp.len()) || !tp.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit() || c == '.')) {
                return None;
            }
            let (h, mi): (u32, u32) = (tp[0].parse().ok()?, tp[1].parse().ok()?);
            let s: f64 = tp.get(2).map_or(Some(0.0), |s| s.parse().ok())?;
            if h > 23 || mi > 59 || s >= 61.0 {
                return None;
            }
            let secs = tp.get(2).map_or("00".to_string(), |s| if s.contains('.') { format!("{:09.6}", s.parse::<f64>().unwrap_or(0.0)).trim_end_matches('0').trim_end_matches('.').to_string() } else { format!("{:02}", s.parse::<u32>().unwrap_or(0)) });
            Some((format!("{day} {h:02}:{mi:02}:{secs}"), true))
        }
    }
}

fn is_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "true" | "false")
}

/// A column's values' type (empty ones aside).
pub fn guess<'a>(vals: impl Iterator<Item = &'a str>) -> Guess {
    let (mut int, mut big, mut num, mut boolean, mut day, mut stamp) = (true, false, true, true, true, false);
    let mut longest = 0;
    let mut any = false;
    for v in vals {
        any = true;
        longest = longest.max(v.chars().count());
        if int {
            if is_int(v) {
                big |= v.parse::<i32>().is_err();
                int = v.parse::<i64>().is_ok();
            } else {
                int = false;
            }
        }
        num = num && number(v).is_some();
        boolean = boolean && is_bool(v);
        if day {
            match date(v) {
                Some((_, t)) => stamp |= t,
                None => day = false,
            }
        }
    }
    match () {
        _ if !any => Guess::Text(longest),
        _ if int => if big { Guess::Big } else { Guess::Int },
        _ if num => Guess::Num,
        _ if boolean => Guess::Bool,
        _ if day && stamp => Guess::Stamp,
        _ if day => Guess::Date,
        _ => Guess::Text(longest),
    }
}

/// The SQL type for it.
pub fn sql_type(kind: Kind, g: Guess) -> String {
    match (kind, g) {
        (Kind::Postgres, Guess::Int) => "integer".into(),
        (Kind::Postgres, Guess::Big) => "bigint".into(),
        (Kind::Postgres, Guess::Num) => "numeric".into(),
        (Kind::Postgres, Guess::Bool) => "boolean".into(),
        (Kind::Postgres, Guess::Date) => "date".into(),
        (Kind::Postgres, Guess::Stamp) => "timestamp".into(),
        (Kind::Postgres, Guess::Text(_)) => "text".into(),
        (Kind::Oracle, Guess::Int) => "NUMBER(10)".into(),
        (Kind::Oracle, Guess::Big) => "NUMBER(19)".into(),
        (Kind::Oracle, Guess::Num) => "NUMBER".into(),
        (Kind::Oracle, Guess::Bool) => "NUMBER(1)".into(),
        (Kind::Oracle, Guess::Date) => "DATE".into(),
        (Kind::Oracle, Guess::Stamp) => "TIMESTAMP".into(),
        (Kind::Oracle, Guess::Text(n)) if n > 4000 => "CLOB".into(),
        (Kind::Oracle, Guess::Text(n)) => format!("VARCHAR2({} CHAR)", room(n)),
        (Kind::ClickHouse, Guess::Int) => "Int32".into(),
        (Kind::ClickHouse, Guess::Big) => "Int64".into(),
        (Kind::ClickHouse, Guess::Num) => "Float64".into(),
        (Kind::ClickHouse, Guess::Bool) => "Bool".into(),
        (Kind::ClickHouse, Guess::Date) => "Date".into(),
        (Kind::ClickHouse, Guess::Stamp) => "DateTime".into(),
        (Kind::ClickHouse, Guess::Text(_)) => "String".into(),
        (Kind::Sqlite, Guess::Int | Guess::Big | Guess::Bool) => "INTEGER".into(),
        (Kind::Sqlite, Guess::Num) => "REAL".into(),
        (Kind::Sqlite, _) => "TEXT".into(),
    }
}

/// Text room with some to spare: 50, 100, 255, 500, 1000, 2000, 4000.
fn room(n: usize) -> usize {
    [50, 100, 255, 500, 1000, 2000, 4000].into_iter().find(|&r| r >= n).unwrap_or(4000)
}

/// Types to pick from.
pub fn type_choices(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Postgres => &["integer", "bigint", "smallint", "numeric", "double precision", "real", "boolean", "date", "timestamp", "timestamptz", "time", "text", "varchar(255)", "uuid", "jsonb"],
        Kind::Oracle => &["NUMBER", "NUMBER(10)", "NUMBER(19)", "NUMBER(1)", "BINARY_DOUBLE", "DATE", "TIMESTAMP", "VARCHAR2(255 CHAR)", "VARCHAR2(4000 CHAR)", "CLOB"],
        Kind::ClickHouse => &["Int32", "Int64", "UInt32", "UInt64", "Float64", "Decimal(18, 4)", "Bool", "Date", "DateTime", "DateTime64(3)", "String", "LowCardinality(String)", "UUID"],
        Kind::Sqlite => &["INTEGER", "REAL", "NUMERIC", "TEXT", "BLOB"],
    }
}

/// A name from a file's header as a column's: words joined by "_",
/// lower case (Oracle: upper), letters of any alphabet kept.
pub fn column_name(kind: Kind, raw: &str, k: usize) -> String {
    let mut s = String::new();
    for c in raw.trim().chars() {
        if c.is_alphanumeric() {
            s.extend(c.to_lowercase());
        } else if !s.ends_with('_') && !s.is_empty() {
            s.push('_');
        }
    }
    let s = s.trim_end_matches('_').to_string();
    let s = if s.is_empty() { format!("col_{}", k + 1) } else if s.starts_with(|c: char| c.is_ascii_digit()) { format!("c_{s}") } else { s };
    let s: String = s.chars().take(if kind == Kind::Oracle { 128 } else { 63 }).collect();
    if kind == Kind::Oracle { s.to_uppercase() } else { s }
}

/// A file's column as it'll go in.
#[derive(Clone, Debug, PartialEq)]
pub struct Col {
    /// Its place in the file.
    pub from: usize,
    pub on: bool,
    pub name: String,
    pub ty: String,
    pub nullable: bool,
    pub pk: bool,
    pub guess: Guess,
}

/// The columns for what's read, their types guessed.
pub fn columns(kind: Kind, d: &Data) -> Vec<Col> {
    let mut seen: Vec<String> = Vec::new();
    (0..d.names.len())
        .map(|k| {
            let g = guess(d.rows.iter().filter_map(|r| r[k].as_deref()));
            let mut name = column_name(kind, &d.names[k], k);
            let base = name.clone();
            let mut n = 2;
            while seen.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                name = format!("{base}_{n}");
                n += 1;
            }
            seen.push(name.clone());
            let nullable = d.rows.iter().any(|r| r[k].is_none());
            Col { from: k, on: true, name, ty: sql_type(kind, g), nullable, pk: false, guess: g }
        })
        .collect()
}

/// A table's name from a file's.
pub fn table_name(kind: Kind, path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let n = column_name(kind, &stem, 0);
    if n == "col_1" || n == "COL_1" { if kind == Kind::Oracle { "IMPORTED".into() } else { "imported".into() } } else { n }
}

/// What it makes: the table (with its key), then its indexes.
pub fn ddl(kind: Kind, schema: &str, table: &str, cols: &[Col], idx: &[IndexDef]) -> Vec<String> {
    let d = kind.dialect();
    let t = TableDef { schema: schema.to_string(), name: table.to_string(), ..Default::default() };
    let tn = crate::design::table_name(kind, &t);
    let on: Vec<&Col> = cols.iter().filter(|c| c.on).collect();
    let pk: Vec<String> = on.iter().filter(|c| c.pk).map(|c| quote(&c.name, d)).collect();
    let specs: Vec<String> = on
        .iter()
        .map(|c| {
            let col = Column { name: c.name.clone(), ty: c.ty.clone(), nullable: c.nullable && !c.pk, ..Default::default() };
            crate::design::col_spec(kind, &col)
        })
        .collect();
    let mut out = Vec::new();
    match kind {
        Kind::ClickHouse => {
            // The key: its sorting order.
            let order = if pk.is_empty() { "tuple()".to_string() } else { format!("({})", pk.join(", ")) };
            out.push(format!("CREATE TABLE {tn} (\n  {}\n) ENGINE = MergeTree ORDER BY {order}", specs.join(",\n  ")));
        }
        _ => {
            let key = if pk.is_empty() { String::new() } else { format!(",\n  PRIMARY KEY ({})", pk.join(", ")) };
            out.push(format!("CREATE TABLE {tn} (\n  {}{key}\n)", specs.join(",\n  ")));
        }
    }
    for x in idx.iter().filter(|x| !x.name.trim().is_empty() && !x.cols.trim().is_empty()) {
        out.push(crate::design::create_index(kind, &t, x));
    }
    out
}

/// A value as a literal for its column's type.
pub fn literal(kind: Kind, ty: &str, v: Option<&str>) -> String {
    let Some(v) = v else { return "NULL".into() };
    let t = ty.to_ascii_lowercase();
    let t = t.strip_prefix("nullable(").unwrap_or(&t);
    let s = |v: &str| match kind {
        Kind::ClickHouse => crate::db::ch::literal(v),
        _ => format!("'{}'", v.replace('\'', "''")),
    };
    let numeric = ["int", "number", "numeric", "decimal", "real", "float", "double", "binary_double", "serial"].iter().any(|w| t.contains(w)) && !t.contains("interval");
    let boolean = t.starts_with("bool") || (kind == Kind::Oracle && t == "number(1)");
    if boolean || (numeric && is_bool(v)) {
        let b = v.eq_ignore_ascii_case("true") || v == "1";
        if is_bool(v) || v == "0" || v == "1" {
            return match kind {
                Kind::Postgres if t.starts_with("bool") => if b { "TRUE".into() } else { "FALSE".into() },
                Kind::ClickHouse if t.starts_with("bool") => if b { "true".into() } else { "false".into() },
                _ => if b { "1".into() } else { "0".into() },
            };
        }
    }
    if numeric {
        if let Some(n) = number(v) {
            return n;
        }
    }
    if t.starts_with("date") || t.starts_with("timestamp") {
        if let Some((iso, with_time)) = date(v) {
            return match kind {
                Kind::Oracle if t.starts_with("timestamp") => format!("TIMESTAMP '{}'", if with_time { iso } else { format!("{iso} 00:00:00") }),
                Kind::Oracle if with_time => format!("TO_DATE('{}', 'YYYY-MM-DD HH24:MI:SS')", &iso[..19.min(iso.len())]),
                Kind::Oracle => format!("DATE '{iso}'"),
                _ => s(&iso),
            };
        }
    }
    s(v)
}

/// The rows as INSERTs, `per` rows each.
pub fn inserts(kind: Kind, schema: &str, table: &str, cols: &[Col], rows: &[Vec<Option<String>>], per: usize) -> Vec<String> {
    let d = kind.dialect();
    let t = TableDef { schema: schema.to_string(), name: table.to_string(), ..Default::default() };
    let tn = crate::design::table_name(kind, &t);
    let on: Vec<&Col> = cols.iter().filter(|c| c.on).collect();
    let names = on.iter().map(|c| quote(&c.name, d)).collect::<Vec<_>>().join(", ");
    // (Dates as the file has them, "01.02.2024", go in as ISO.)
    let value = |c: &Col, v: Option<&str>| -> Option<String> {
        let v = v?;
        Some(match c.guess {
            Guess::Date | Guess::Stamp => date(v).map_or_else(|| v.to_string(), |(iso, _)| iso),
            _ => v.to_string(),
        })
    };
    let tuple = |r: &Vec<Option<String>>| format!("({})", on.iter().map(|c| literal(kind, &c.ty, value(c, r.get(c.from).and_then(|v| v.as_deref())).as_deref())).collect::<Vec<_>>().join(", "));
    rows.chunks(per.max(1))
        .map(|chunk| match kind {
            // (Any Oracle: many rows through INSERT ALL.)
            Kind::Oracle => format!("INSERT ALL\n{}\nSELECT 1 FROM DUAL", chunk.iter().map(|r| format!("  INTO {tn} ({names}) VALUES {}", tuple(r))).collect::<Vec<_>>().join("\n")),
            _ => format!("INSERT INTO {tn} ({names}) VALUES\n{}", chunk.iter().map(|r| format!("  {}", tuple(r))).collect::<Vec<_>>().join(",\n")),
        })
        .collect()
}

/// Rows in an INSERT for the database.
pub fn batch(kind: Kind) -> usize {
    match kind {
        Kind::Oracle => 100,
        Kind::ClickHouse => 5000,
        _ => 500,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_typed() {
        assert_eq!(guess(["1", "2", "-3"].into_iter()), Guess::Int);
        assert_eq!(guess(["1", "9999999999"].into_iter()), Guess::Big);
        assert_eq!(guess(["1", "2,5", "1 000"].into_iter()), Guess::Num);
        assert_eq!(guess(["007", "12"].into_iter()), Guess::Text(3), "codes keep their zeros");
        assert_eq!(guess(["true", "FALSE"].into_iter()), Guess::Bool);
        assert_eq!(guess(["2024-03-01", "01.02.2023"].into_iter()), Guess::Date);
        assert_eq!(guess(["2024-03-01 10:20", "2024-03-01T10:20:30"].into_iter()), Guess::Stamp);
        assert_eq!(guess(["2024-02-30"].into_iter()), Guess::Text(10));
        assert_eq!(guess(["abc", "1"].into_iter()), Guess::Text(3));
        assert_eq!(date("01.02.2023"), Some(("2023-02-01".into(), false)));
        assert_eq!(date("2024-03-01T10:20:30.5Z"), Some(("2024-03-01 10:20:30.5".into(), true)));
        assert_eq!(number("1 234,5").as_deref(), Some("1234.5"));
        assert_eq!(column_name(Kind::Postgres, " Дата заказа (UTC) ", 0), "дата_заказа_utc");
        assert_eq!(column_name(Kind::Oracle, "total sum", 0), "TOTAL_SUM");
        assert_eq!(column_name(Kind::Postgres, "", 2), "col_3");
        assert_eq!(column_name(Kind::Postgres, "2024", 0), "c_2024");
    }

    #[test]
    fn literals_per_type() {
        assert_eq!(literal(Kind::Postgres, "numeric", Some("1 234,5")), "1234.5");
        assert_eq!(literal(Kind::Postgres, "text", Some("it's")), "'it''s'");
        assert_eq!(literal(Kind::Postgres, "boolean", Some("True")), "TRUE");
        assert_eq!(literal(Kind::Postgres, "date", Some("01.02.2023")), "'2023-02-01'");
        assert_eq!(literal(Kind::Oracle, "DATE", Some("2023-02-01")), "DATE '2023-02-01'");
        assert_eq!(literal(Kind::Oracle, "DATE", Some("2023-02-01 10:00")), "TO_DATE('2023-02-01 10:00:00', 'YYYY-MM-DD HH24:MI:SS')");
        assert_eq!(literal(Kind::Oracle, "NUMBER(1)", Some("false")), "0");
        assert_eq!(literal(Kind::ClickHouse, "Nullable(Int32)", None), "NULL");
        assert_eq!(literal(Kind::Sqlite, "INTEGER", Some("abc")), "'abc'");
    }

    #[test]
    fn files_read() {
        let dir = std::env::temp_dir().join(format!("zapros-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Заказы 2024.csv");
        std::fs::write(&p, "id;Имя;Сумма;Дата\n1;Анна;10,5;01.02.2024\n2;\"Б; и ко\";;2024-02-03\n").unwrap();
        let d = read(&p, &ReadOpts::default()).unwrap();
        assert_eq!(d.sep, ';');
        assert_eq!(d.names, ["id", "Имя", "Сумма", "Дата"]);
        assert_eq!(d.rows[1], [Some("2".into()), Some("Б; и ко".into()), None, Some("2024-02-03".into())]);
        let cols = columns(Kind::Postgres, &d);
        assert_eq!(cols.iter().map(|c| c.ty.as_str()).collect::<Vec<_>>(), ["integer", "text", "numeric", "date"]);
        assert!(cols[2].nullable && !cols[0].nullable);
        assert_eq!(table_name(Kind::Postgres, &p), "заказы_2024");
        // Windows-1251.
        let (w, _, _) = encoding_rs::WINDOWS_1251.encode("a,б\n1,в\n");
        std::fs::write(dir.join("w.csv"), &w).unwrap();
        let d = read(&dir.join("w.csv"), &ReadOpts::default()).unwrap();
        assert_eq!((d.encoding, d.names[1].as_str()), ("Windows-1251", "б"));
        // JSON: an array, and one a line.
        std::fs::write(dir.join("a.json"), r#"[{"a":1,"b":"x"},{"b":null,"c":true}]"#).unwrap();
        let d = read(&dir.join("a.json"), &ReadOpts::default()).unwrap();
        assert_eq!(d.names, ["a", "b", "c"]);
        assert_eq!(d.rows[1], [None, None, Some("true".into())]);
        std::fs::write(dir.join("a.ndjson"), "{\"a\":1}\n{\"a\":2}\n").unwrap();
        assert_eq!(read(&dir.join("a.ndjson"), &ReadOpts::default()).unwrap().rows.len(), 2);
        // A workbook (made by the exporter).
        let x = dir.join("b.xlsx");
        let mut wb = rust_xlsxwriter::Workbook::new();
        let ws = wb.add_worksheet().set_name("Лист").unwrap();
        ws.write(0, 0, "n").unwrap();
        ws.write(0, 1, "when").unwrap();
        ws.write(1, 0, 1.0).unwrap();
        ws.write(1, 1, "2024-01-02").unwrap();
        ws.write(2, 0, 2.5).unwrap();
        wb.save(&x).unwrap();
        let d = read(&x, &ReadOpts::default()).unwrap();
        assert_eq!(d.sheets, ["Лист"]);
        assert_eq!(d.rows, [vec![Some("1".into()), Some("2024-01-02".into())], vec![Some("2.5".into()), None]]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn statements_made() {
        let cols = vec![
            Col { from: 0, on: true, name: "id".into(), ty: "integer".into(), nullable: false, pk: true, guess: Guess::Int },
            Col { from: 1, on: false, name: "skip".into(), ty: "text".into(), nullable: true, pk: false, guess: Guess::Text(1) },
            Col { from: 2, on: true, name: "name".into(), ty: "text".into(), nullable: true, pk: false, guess: Guess::Text(1) },
        ];
        let idx = vec![IndexDef { name: "t_name_ix".into(), cols: "name".into(), ..Default::default() }];
        let s = ddl(Kind::Postgres, "public", "t", &cols, &idx);
        assert_eq!(s[0], "CREATE TABLE public.t (\n  id integer NOT NULL,\n  name text,\n  PRIMARY KEY (id)\n)");
        assert_eq!(s[1], "CREATE INDEX t_name_ix ON public.t (name)");
        let ch = ddl(Kind::ClickHouse, "default", "t", &cols, &[]);
        assert!(ch[0].ends_with("ENGINE = MergeTree ORDER BY (id)"), "{}", ch[0]);
        assert!(ch[0].contains("name Nullable(text)") || ch[0].contains("Nullable("), "{}", ch[0]);
        let rows = vec![vec![Some("1".into()), Some("x".into()), Some("a".into())], vec![Some("2".into()), None, None]];
        assert_eq!(inserts(Kind::Postgres, "public", "t", &cols, &rows, 500), ["INSERT INTO public.t (id, name) VALUES\n  (1, 'a'),\n  (2, NULL)"]);
        let o = inserts(Kind::Oracle, "SHOP", "T", &cols, &rows, 1);
        assert_eq!(o.len(), 2);
        assert!(o[0].starts_with("INSERT ALL\n  INTO \"SHOP\".\"T\"") || o[0].starts_with("INSERT ALL\n  INTO SHOP.T"), "{}", o[0]);
    }
}
