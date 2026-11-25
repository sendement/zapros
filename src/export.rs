//! Saving a result: CSV / TSV / JSON / SQL INSERTs / Markdown / HTML /
//! XLSX, whole or cut into files of so many rows each.

use std::path::PathBuf;

use crate::app::ResultView;
use crate::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Csv,
    Tsv,
    Json,
    Insert,
    Markdown,
    Html,
    Xlsx,
}

impl Format {
    pub const ALL: [Format; 7] = [Format::Csv, Format::Tsv, Format::Json, Format::Insert, Format::Markdown, Format::Html, Format::Xlsx];

    pub fn name(self) -> &'static str {
        match self {
            Format::Csv => "CSV",
            Format::Tsv => "TSV",
            Format::Json => "JSON",
            Format::Insert => "SQL INSERT",
            Format::Markdown => "Markdown",
            Format::Html => "HTML",
            Format::Xlsx => "Excel (XLSX)",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Tsv => "tsv",
            Format::Json => "json",
            Format::Insert => "sql",
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Xlsx => "xlsx",
        }
    }
}

/// What to save, and how.
#[derive(Clone, Debug)]
pub struct Export {
    pub format: Format,
    /// The file (with a cut: its name gets _1, _2…).
    pub path: PathBuf,
    /// A header row (CSV / TSV / XLSX).
    pub header: bool,
    /// CSV's separator.
    pub sep: char,
    /// Rows per file; 0: one file.
    pub split: usize,
    /// INSERT INTO which table.
    pub table: String,
    /// Load the rest of the rows first.
    pub load_all: bool,
}

/// Rows to save (tests: a result as plain rows).
#[cfg(test)]
pub struct Snapshot {
    pub cols: Vec<String>,
    pub numeric: Vec<bool>,
    pub rows: Vec<Vec<Option<String>>>,
}

/// The table a SELECT reads from (for INSERT INTO), or "result".
pub fn table_of(sql: &str) -> String {
    let words: Vec<&str> = sql.split_whitespace().collect();
    words
        .iter()
        .position(|w| w.eq_ignore_ascii_case("from"))
        .and_then(|i| words.get(i + 1))
        .map(|w| w.trim_matches(|c: char| c == ';' || c == '(' || c == ')').to_string())
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| "result".into())
}

fn csv_cell(v: &Option<String>, sep: char) -> String {
    match v {
        None => String::new(),
        Some(s) if s.contains([sep, '"', '\n', '\r']) => format!("\"{}\"", s.replace('"', "\"\"")),
        Some(s) => s.clone(),
    }
}

fn html_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// What goes before a file's rows.
fn head(e: &Export, cols: &[String], numeric: &[bool]) -> String {
    let mut out = String::new();
    match e.format {
        Format::Csv | Format::Tsv if e.header => {
            let sep = if e.format == Format::Tsv { '\t' } else { e.sep };
            out += &cols.iter().map(|c| text_cell(e, &Some(c.clone()))).collect::<Vec<_>>().join(&sep.to_string());
            out.push('\n');
        }
        Format::Markdown => {
            let cell = |v: &str| v.replace('|', "\\|").replace('\n', " ");
            out += &format!("| {} |\n", cols.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | "));
            out += &format!("|{}\n", numeric.iter().map(|&n| if n { "---:|" } else { "---|" }).collect::<String>());
        }
        Format::Html => {
            out += "<!doctype html>\n<meta charset=\"utf-8\">\n<table border=\"1\" cellspacing=\"0\" cellpadding=\"4\">\n<tr>";
            for c in cols {
                out += &format!("<th>{}</th>", html_esc(c));
            }
            out += "</tr>\n";
        }
        _ => {}
    }
    out
}

fn text_cell(e: &Export, v: &Option<String>) -> String {
    if e.format == Format::Tsv { v.as_deref().unwrap_or("").replace(['\t', '\n', '\r'], " ") } else { csv_cell(v, e.sep) }
}

/// One row's text (`first`: the file's first row).
fn row_text(e: &Export, cols: &[String], numeric: &[bool], r: &[&Option<String>], first: bool) -> String {
    match e.format {
        Format::Csv | Format::Tsv => {
            let sep = if e.format == Format::Tsv { '\t' } else { e.sep };
            let mut s = r.iter().map(|v| text_cell(e, v)).collect::<Vec<_>>().join(&sep.to_string());
            s.push('\n');
            s
        }
        Format::Json => {
            let m: serde_json::Map<String, serde_json::Value> = cols
                .iter()
                .zip(r)
                .zip(numeric)
                .map(|((c, v), &num)| {
                    let val = match v {
                        None => serde_json::Value::Null,
                        Some(x) if num => x.parse::<serde_json::Number>().map(serde_json::Value::Number).unwrap_or_else(|_| serde_json::Value::String(x.clone())),
                        Some(x) => serde_json::Value::String(x.clone()),
                    };
                    (c.clone(), val)
                })
                .collect();
            let obj = serde_json::to_string_pretty(&serde_json::Value::Object(m)).unwrap_or_default();
            let indented = obj.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n");
            format!("{}{indented}", if first { "[\n" } else { ",\n" })
        }
        Format::Insert => {
            let names = cols.iter().map(|c| if c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') { c.clone() } else { format!("\"{}\"", c.replace('"', "\"\"")) }).collect::<Vec<_>>().join(", ");
            let vals = r
                .iter()
                .zip(numeric)
                .map(|(v, &num)| match v {
                    None => "NULL".to_string(),
                    Some(x) if num && x.parse::<f64>().is_ok() => x.clone(),
                    Some(x) => format!("'{}'", x.replace('\'', "''")),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("INSERT INTO {} ({names}) VALUES ({vals});\n", e.table)
        }
        Format::Markdown => {
            let cell = |v: &str| v.replace('|', "\\|").replace('\n', " ");
            format!("| {} |\n", r.iter().map(|v| v.as_deref().map_or("NULL".into(), cell)).collect::<Vec<_>>().join(" | "))
        }
        Format::Html => {
            let mut s = String::from("<tr>");
            for (v, &num) in r.iter().zip(numeric) {
                let align = if num { " align=\"right\"" } else { "" };
                s += &format!("<td{align}>{}</td>", v.as_deref().map_or("<i>NULL</i>".into(), html_esc));
            }
            s += "</tr>\n";
            s
        }
        Format::Xlsx => String::new(),
    }
}

/// What closes a file (`any`: it has rows).
fn tail(e: &Export, any: bool) -> &'static str {
    match e.format {
        Format::Json if any => "\n]\n",
        Format::Json => "[]\n",
        Format::Html => "</table>\n",
        _ => "",
    }
}

/// One file's text (not XLSX).
#[cfg(test)]
pub fn render(e: &Export, s: &Snapshot, rows: &[Vec<Option<String>>]) -> String {
    let mut out = head(e, &s.cols, &s.numeric);
    for (k, r) in rows.iter().enumerate() {
        out += &row_text(e, &s.cols, &s.numeric, &r.iter().collect::<Vec<_>>(), k == 0);
    }
    out += tail(e, !rows.is_empty());
    out
}

/// The files `rows` rows make: one, or `name_1.ext`, `name_2.ext`…
pub fn paths(e: &Export, rows: usize) -> Vec<PathBuf> {
    let sink = Sink::new(e.clone(), Vec::new(), Vec::new(), Vec::new());
    let cap = sink.cap();
    if rows <= cap {
        return vec![e.path.clone()];
    }
    (1..=rows.div_ceil(cap)).map(|i| sink.numbered(i)).collect()
}

/// A sheet holds this many rows at most (past it: the next file).
const XLSX_ROWS: usize = 1_048_575;

enum Out {
    Text(std::io::BufWriter<std::fs::File>),
    /// (Its rows written straight to a temporary file as they come: not
    /// kept in memory.)
    Xlsx(Box<rust_xlsxwriter::Workbook>, u32),
}

/// Where rows go as they come: written to the file (the next one when a
/// part is full) a batch at a time, none of them kept — a result of any
/// size saved in the memory of one batch.
pub struct Sink {
    e: Export,
    cols: Vec<String>,
    numeric: Vec<bool>,
    /// The table's column for each one saved (the order shown).
    map: Vec<usize>,
    files: Vec<PathBuf>,
    in_file: usize,
    total: usize,
    out: Option<Out>,
}

impl Sink {
    pub fn new(e: Export, cols: Vec<String>, numeric: Vec<bool>, map: Vec<usize>) -> Self {
        Self { e, cols, numeric, map, files: Vec::new(), in_file: 0, total: 0, out: None }
    }

    /// For a result's rows, its columns as shown (moved by hand or not).
    pub fn of(e: Export, v: &ResultView) -> Self {
        let map: Vec<usize> = if v.cols.len() == v.table.cols.len() { v.cols.clone() } else { (0..v.table.cols.len()).collect() };
        let cols = map.iter().map(|&c| v.table.cols[c].name.clone()).collect();
        let numeric = map.iter().map(|&c| v.table.cols[c].numeric).collect();
        Self::new(e, cols, numeric, map)
    }

    pub fn rows(&self) -> usize {
        self.total
    }

    /// Rows a file takes.
    fn cap(&self) -> usize {
        let split = if self.e.split == 0 { usize::MAX } else { self.e.split };
        if self.e.format == Format::Xlsx { split.min(XLSX_ROWS - usize::from(self.e.header)) } else { split }
    }

    fn numbered(&self, n: usize) -> PathBuf {
        let stem = self.e.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "result".into());
        let ext = self.e.path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| self.e.format.ext().into());
        self.e.path.with_file_name(format!("{stem}_{n}.{ext}"))
    }

    fn open(&mut self) -> Result<(), String> {
        if let Some(dir) = self.e.path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        // The first file plainly named; a second one: both numbered.
        let path = match self.files.len() {
            0 => self.e.path.clone(),
            n => {
                if n == 1 {
                    let first = self.numbered(1);
                    std::fs::rename(&self.files[0], &first).map_err(|x| format!("{}: {x}", self.files[0].display()))?;
                    self.files[0] = first;
                }
                self.numbered(n + 1)
            }
        };
        let out = if self.e.format == Format::Xlsx {
            let mut wb = rust_xlsxwriter::Workbook::new();
            let ws = wb.add_worksheet_with_constant_memory().map_err(|e| e.to_string())?;
            let mut y = 0;
            if self.e.header {
                let bold = rust_xlsxwriter::Format::new().set_bold();
                for (x, c) in self.cols.iter().enumerate() {
                    ws.write_string_with_format(0, x as u16, c, &bold).map_err(|e| e.to_string())?;
                }
                y = 1;
            }
            Out::Xlsx(Box::new(wb), y)
        } else {
            use std::io::Write;
            let f = std::fs::File::create(&path).map_err(|x| format!("{}: {x}", path.display()))?;
            let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
            w.write_all(head(&self.e, &self.cols, &self.numeric).as_bytes()).map_err(|x| x.to_string())?;
            Out::Text(w)
        };
        self.files.push(path);
        self.out = Some(out);
        self.in_file = 0;
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        let path = self.files.last().cloned().unwrap_or_default();
        match self.out.take() {
            Some(Out::Text(mut w)) => {
                use std::io::Write;
                w.write_all(tail(&self.e, self.in_file > 0).as_bytes()).and_then(|_| w.flush()).map_err(|x| format!("{}: {x}", path.display()))
            }
            Some(Out::Xlsx(mut wb, _)) => wb.save(&path).map_err(|x| format!("{}: {x}", path.display())),
            None => Ok(()),
        }
    }

    /// Rows (each in the table's column order) written.
    pub fn push<'a>(&mut self, rows: impl IntoIterator<Item = &'a Vec<Option<String>>>) -> Result<(), String> {
        let none: Option<String> = None;
        for r in rows {
            if self.out.is_some() && self.in_file >= self.cap() {
                self.close()?;
            }
            if self.out.is_none() {
                self.open()?;
            }
            let cells: Vec<&Option<String>> = self.map.iter().map(|&c| r.get(c).unwrap_or(&none)).collect();
            match self.out.as_mut().unwrap() {
                Out::Text(w) => {
                    use std::io::Write;
                    let line = row_text(&self.e, &self.cols, &self.numeric, &cells, self.in_file == 0);
                    w.write_all(line.as_bytes()).map_err(|x| x.to_string())?;
                }
                Out::Xlsx(wb, y) => {
                    let ws = wb.worksheet_from_index(0).map_err(|e| e.to_string())?;
                    for (x, (v, &num)) in cells.iter().zip(&self.numeric).enumerate() {
                        let Some(v) = v else { continue };
                        match v.parse::<f64>() {
                            Ok(n) if num => ws.write_number(*y, x as u16, n),
                            _ => ws.write_string(*y, x as u16, v),
                        }
                        .map_err(|e| e.to_string())?;
                    }
                    *y += 1;
                }
            }
            self.in_file += 1;
            self.total += 1;
        }
        Ok(())
    }

    /// The last file closed; what to tell.
    pub fn finish(mut self) -> Result<String, String> {
        if self.files.is_empty() {
            // (No rows: a file with the header still.)
            self.open()?;
        }
        self.close()?;
        let n = self.total;
        Ok(if self.files.len() == 1 {
            format!("{n} {} → {}", t("строк сохранено", "rows saved"), self.files[0].display())
        } else {
            format!("{n} {} → {} {} ({})", t("строк сохранено", "rows saved"), self.files.len(), t("файлов", "files"), self.files[0].parent().map(|d| d.display().to_string()).unwrap_or_default())
        })
    }
}

/// Writes the files; what to tell.
#[cfg(test)]
pub fn save(e: &Export, s: &Snapshot) -> String {
    let mut sink = Sink::new(e.clone(), s.cols.clone(), s.numeric.clone(), (0..s.cols.len()).collect());
    match sink.push(&s.rows).and_then(|_| sink.finish()) {
        Ok(m) | Err(m) => m,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn snap() -> Snapshot {
        Snapshot { cols: vec!["id".into(), "name".into()], numeric: vec![true, false], rows: (1..=5).map(|i| vec![Some(i.to_string()), if i == 3 { None } else { Some(format!("n,'{i}")) }]).collect() }
    }

    fn ex(format: Format, split: usize, dir: &Path) -> Export {
        Export { format, path: dir.join(format!("out.{}", format.ext())), header: true, sep: ',', split, table: "t".into(), load_all: false }
    }

    #[test]
    fn formats() {
        let s = snap();
        let d = std::env::temp_dir();
        let csv = render(&ex(Format::Csv, 0, &d), &s, &s.rows);
        assert!(csv.starts_with("id,name\n1,\"n,'1\"\n"), "{csv}");
        let ins = render(&ex(Format::Insert, 0, &d), &s, &s.rows);
        assert!(ins.contains("INSERT INTO t (id, name) VALUES (1, 'n,''1');"), "{ins}");
        assert!(ins.contains("VALUES (3, NULL);"), "{ins}");
        let json: serde_json::Value = serde_json::from_str(&render(&ex(Format::Json, 0, &d), &s, &s.rows)).unwrap();
        assert_eq!(json[0]["id"], serde_json::json!(1));
        assert!(render(&ex(Format::Markdown, 0, &d), &s, &s.rows).contains("|---:|---|"));
    }

    #[test]
    fn split_into_files() {
        let d = std::env::temp_dir().join(format!("zapros-export-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let s = snap();
        let e = ex(Format::Csv, 2, &d);
        let msg = save(&e, &s);
        assert!(msg.contains('3'), "{msg}");
        let third = std::fs::read_to_string(d.join("out_3.csv")).unwrap();
        assert_eq!(third, "id,name\n5,\"n,'5\"\n");
        assert!(!d.join("out.csv").exists(), "the first part numbered too");
        assert!(d.join("out_1.csv").exists());
        let e = ex(Format::Xlsx, 0, &d);
        save(&e, &s);
        assert!(std::fs::metadata(d.join("out.xlsx")).unwrap().len() > 1000);
        // JSON a batch at a time: the same array as at once.
        let e = ex(Format::Json, 0, &d);
        let mut sink = Sink::new(e.clone(), s.cols.clone(), s.numeric.clone(), vec![0, 1]);
        sink.push(&s.rows[..2]).unwrap();
        sink.push(&s.rows[2..]).unwrap();
        sink.finish().unwrap();
        let got: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("out.json")).unwrap()).unwrap();
        assert_eq!(got.as_array().unwrap().len(), 5);
        assert_eq!(std::fs::read_to_string(d.join("out.json")).unwrap(), render(&e, &s, &s.rows));
        // Columns as shown: moved.
        let e = ex(Format::Csv, 0, &d);
        let mut sink = Sink::new(e, vec!["name".into(), "id".into()], vec![false, true], vec![1, 0]);
        sink.push(&s.rows[..1]).unwrap();
        sink.finish().unwrap();
        assert_eq!(std::fs::read_to_string(d.join("out.csv")).unwrap(), "name,id\n\"n,'1\",1\n");
        std::fs::remove_dir_all(&d).ok();
    }
}
