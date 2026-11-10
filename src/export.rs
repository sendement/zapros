//! Saving a result: CSV / TSV / JSON / SQL INSERTs / Markdown / HTML /
//! XLSX, whole or cut into files of so many rows each.

use std::path::{Path, PathBuf};

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

/// The rows to save, in the order shown.
pub struct Snapshot {
    pub cols: Vec<String>,
    pub numeric: Vec<bool>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl Snapshot {
    pub fn of(v: &ResultView) -> Self {
        // The columns as shown (moved by hand or not).
        let cols: Vec<usize> = if v.cols.len() == v.table.cols.len() { v.cols.clone() } else { (0..v.table.cols.len()).collect() };
        Self {
            cols: cols.iter().map(|&c| v.table.cols[c].name.clone()).collect(),
            numeric: cols.iter().map(|&c| v.table.cols[c].numeric).collect(),
            rows: v.order.iter().filter_map(|&r| v.table.rows.get(r)).map(|row| cols.iter().map(|&c| row.get(c).cloned().flatten()).collect()).collect(),
        }
    }
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

/// One file's text (not XLSX).
pub fn render(e: &Export, s: &Snapshot, rows: &[Vec<Option<String>>]) -> String {
    let mut out = String::new();
    match e.format {
        Format::Csv | Format::Tsv => {
            let sep = if e.format == Format::Tsv { '\t' } else { e.sep };
            let cell = |v: &Option<String>| if e.format == Format::Tsv { v.as_deref().unwrap_or("").replace(['\t', '\n', '\r'], " ") } else { csv_cell(v, sep) };
            if e.header {
                out += &s.cols.iter().map(|c| cell(&Some(c.clone()))).collect::<Vec<_>>().join(&sep.to_string());
                out.push('\n');
            }
            for r in rows {
                out += &r.iter().map(cell).collect::<Vec<_>>().join(&sep.to_string());
                out.push('\n');
            }
        }
        Format::Json => {
            let arr: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    let m: serde_json::Map<String, serde_json::Value> = s
                        .cols
                        .iter()
                        .zip(r)
                        .zip(&s.numeric)
                        .map(|((c, v), &num)| {
                            let val = match v {
                                None => serde_json::Value::Null,
                                Some(x) if num => x.parse::<serde_json::Number>().map(serde_json::Value::Number).unwrap_or_else(|_| serde_json::Value::String(x.clone())),
                                Some(x) => serde_json::Value::String(x.clone()),
                            };
                            (c.clone(), val)
                        })
                        .collect();
                    serde_json::Value::Object(m)
                })
                .collect();
            out = serde_json::to_string_pretty(&arr).unwrap_or_default();
            out.push('\n');
        }
        Format::Insert => {
            let cols = s.cols.iter().map(|c| if c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') { c.clone() } else { format!("\"{}\"", c.replace('"', "\"\"")) }).collect::<Vec<_>>().join(", ");
            for r in rows {
                let vals = r
                    .iter()
                    .zip(&s.numeric)
                    .map(|(v, &num)| match v {
                        None => "NULL".to_string(),
                        Some(x) if num && x.parse::<f64>().is_ok() => x.clone(),
                        Some(x) => format!("'{}'", x.replace('\'', "''")),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                out += &format!("INSERT INTO {} ({cols}) VALUES ({vals});\n", e.table);
            }
        }
        Format::Markdown => {
            let cell = |v: &str| v.replace('|', "\\|").replace('\n', " ");
            out += &format!("| {} |\n", s.cols.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | "));
            out += &format!("|{}\n", s.numeric.iter().map(|&n| if n { "---:|" } else { "---|" }).collect::<String>());
            for r in rows {
                out += &format!("| {} |\n", r.iter().map(|v| v.as_deref().map_or("NULL".into(), cell)).collect::<Vec<_>>().join(" | "));
            }
        }
        Format::Html => {
            out += "<!doctype html>\n<meta charset=\"utf-8\">\n<table border=\"1\" cellspacing=\"0\" cellpadding=\"4\">\n<tr>";
            for c in &s.cols {
                out += &format!("<th>{}</th>", html_esc(c));
            }
            out += "</tr>\n";
            for r in rows {
                out += "<tr>";
                for (v, &num) in r.iter().zip(&s.numeric) {
                    let align = if num { " align=\"right\"" } else { "" };
                    out += &format!("<td{align}>{}</td>", v.as_deref().map_or("<i>NULL</i>".into(), html_esc));
                }
                out += "</tr>\n";
            }
            out += "</table>\n";
        }
        Format::Xlsx => {}
    }
    out
}

fn xlsx(e: &Export, s: &Snapshot, rows: &[Vec<Option<String>>], path: &Path) -> Result<(), String> {
    use rust_xlsxwriter::{Format as XFormat, Workbook};
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let bold = XFormat::new().set_bold();
    let mut y = 0u32;
    if e.header {
        for (x, c) in s.cols.iter().enumerate() {
            ws.write_string_with_format(0, x as u16, c, &bold).map_err(|e| e.to_string())?;
        }
        y = 1;
    }
    for r in rows {
        for (x, (v, &num)) in r.iter().zip(&s.numeric).enumerate() {
            let Some(v) = v else { continue };
            match v.parse::<f64>() {
                Ok(n) if num => ws.write_number(y, x as u16, n),
                _ => ws.write_string(y, x as u16, v),
            }
            .map_err(|e| e.to_string())?;
        }
        y += 1;
    }
    wb.save(path).map_err(|e| e.to_string())
}

/// The files it makes: one, or `name_1.ext`, `name_2.ext`…
pub fn paths(e: &Export, rows: usize) -> Vec<PathBuf> {
    if e.split == 0 || rows <= e.split {
        return vec![e.path.clone()];
    }
    let parts = rows.div_ceil(e.split);
    let stem = e.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "result".into());
    let ext = e.path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| e.format.ext().into());
    (1..=parts).map(|i| e.path.with_file_name(format!("{stem}_{i}.{ext}"))).collect()
}

/// Writes the files; what to tell.
pub fn save(e: &Export, s: &Snapshot) -> String {
    let files = paths(e, s.rows.len());
    let chunk = if files.len() > 1 { e.split } else { s.rows.len().max(1) };
    if let Some(dir) = e.path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    for (i, p) in files.iter().enumerate() {
        let rows = &s.rows[(i * chunk).min(s.rows.len())..((i + 1) * chunk).min(s.rows.len())];
        let r = if e.format == Format::Xlsx { xlsx(e, s, rows, p) } else { std::fs::write(p, render(e, s, rows)).map_err(|x| x.to_string()) };
        if let Err(err) = r {
            return format!("{}: {err}", p.display());
        }
    }
    if files.len() == 1 {
        format!("{} {} → {}", s.rows.len(), t("строк сохранено", "rows saved"), files[0].display())
    } else {
        format!("{} {} → {} {} ({})", s.rows.len(), t("строк сохранено", "rows saved"), files.len(), t("файлов", "files"), files[0].parent().map(|d| d.display().to_string()).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let e = ex(Format::Xlsx, 0, &d);
        save(&e, &s);
        assert!(std::fs::metadata(d.join("out.xlsx")).unwrap().len() > 1000);
        std::fs::remove_dir_all(&d).ok();
    }
}
