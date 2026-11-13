//! Settings, connections and the session (workspaces, tabs, their text),
//! as JSON under ~/.config/zapros and ~/.local/share/zapros. Passwords
//! never go here (see secrets.rs).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::Spec;
use crate::sql::Case;
use crate::sql::fmt::Options as FmtOptions;

pub fn config_dir() -> PathBuf {
    std::env::var_os("ZAPROS_CONFIG").map(PathBuf::from).unwrap_or_else(|| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("zapros"))
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("ZAPROS_DATA").map(PathBuf::from).unwrap_or_else(|| dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("zapros"))
}

/// ~ at the start → home.
pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

pub fn load<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Writes through a temporary file, so a crash never leaves half a file.
pub fn save<T: Serialize>(path: &Path, v: &T) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v)?)?;
    std::fs::rename(tmp, path)
}

pub type Rgb = [u8; 3];

/// Colours of the editor's words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Palette {
    pub keyword: Rgb,
    pub function: Rgb,
    pub type_: Rgb,
    pub string: Rgb,
    pub number: Rgb,
    pub comment: Rgb,
    pub quoted: Rgb,
    pub param: Rgb,
    pub operator: Rgb,
    pub text: Rgb,
    pub bold_keywords: bool,
    pub italic_comments: bool,
    /// Brackets coloured by depth.
    pub rainbow: bool,
    /// Rainbow bands under the indents, one per construct a line is in.
    pub bands: bool,
    /// A frame round the block (bracket, CASE, BEGIN…END…) at the caret and
    /// under the pointer.
    pub frames: bool,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            keyword: [198, 120, 221],
            function: [97, 175, 239],
            type_: [229, 192, 123],
            string: [152, 195, 121],
            number: [209, 154, 102],
            comment: [110, 118, 129],
            quoted: [86, 182, 194],
            param: [224, 108, 117],
            operator: [171, 178, 191],
            text: [220, 223, 228],
            bold_keywords: true,
            italic_comments: true,
            rainbow: true,
            bands: true,
            frames: true,
        }
    }

    pub fn light() -> Self {
        Self {
            keyword: [0, 51, 179],
            function: [0, 98, 122],
            type_: [128, 80, 0],
            string: [6, 125, 23],
            number: [23, 80, 235],
            comment: [140, 140, 140],
            quoted: [135, 16, 148],
            param: [190, 30, 60],
            operator: [60, 60, 60],
            text: [30, 30, 30],
            bold_keywords: true,
            italic_comments: true,
            rainbow: true,
            bands: true,
            frames: true,
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::dark()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemePref {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// None: by the system's language.
    pub russian: Option<bool>,
    pub theme: ThemePref,
    pub ui_scale: f32,
    pub font_size: f32,
    pub tab_width: usize,
    /// Rows a result loads in one batch (more on demand).
    pub row_limit: usize,
    /// An empty line ends a statement (for "run statement").
    pub blank_line_splits: bool,
    /// Go on after a failed statement when running a script.
    pub continue_on_error: bool,
    pub check: bool,
    /// Warn about tables and columns the database doesn't have.
    pub check_names: bool,
    pub hints: bool,
    /// Hints pop up while typing (else: Ctrl+Space).
    pub hints_auto: bool,
    pub hint_case: Case,
    /// Schemas put in by hints always in quotes (`"sales".orders`).
    pub hint_quote_schemas: bool,
    /// The hints' first row is the word as typed (picked at first): Enter
    /// then just goes to a new line; a hint is taken by ↓ and Enter / Tab.
    pub hints_typed_first: bool,
    pub auto_pairs: bool,
    /// SQL inside strings (dynamic SQL, $$ bodies) coloured as SQL.
    pub embedded_sql: bool,
    pub format: FmtOptions,
    pub dark: Palette,
    pub light: Palette,
    /// Confirm UPDATE / DELETE without WHERE before running.
    pub confirm_unsafe: bool,
    pub autosave_secs: u64,
    /// The script tabs' ✕: always there, or only under the pointer.
    pub close_always: bool,
    /// A colour scheme's id ("" — the standard look, by `theme`).
    pub scheme: String,
    /// The backdrop (round the sheet) as a gradient.
    pub backdrop: crate::themes::Backdrop,
    /// The icon pack's id ("" — the font's symbols).
    pub icons: String,
    /// The tree's icons in their kinds' colours (else in the text's).
    pub icon_colors: bool,
    /// Fonts of the interface and of the tables (the code's: `code_font`,
    /// its size `font_size`).
    pub fonts: Fonts,
    /// The desktop's title bar on the window; else the tabs' row is it,
    /// with the window's own buttons (look, order — one of
    /// titlebar::ORDERS — and side).
    pub system_titlebar: bool,
    pub win_style: crate::titlebar::Style,
    pub win_order: u8,
    pub win_left: bool,
    /// The notebook cells' colour labels.
    pub labels: Vec<LabelDef>,
}

/// Font families by name as fontconfig knows them ("" — the system's
/// default sans / monospace).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fonts {
    pub ui: String,
    pub ui_size: f32,
    pub code: String,
    /// "" — the code's.
    pub table: String,
    pub table_size: f32,
    /// Extra room between the code's lines (points).
    pub code_spacing: f32,
}

impl Default for Fonts {
    fn default() -> Self {
        Self { ui: String::new(), ui_size: 13.0, code: String::new(), table: String::new(), table_size: 13.0, code_spacing: 0.0 }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            russian: None,
            theme: ThemePref::System,
            ui_scale: 1.0,
            font_size: 14.0,
            tab_width: 4,
            row_limit: 500,
            blank_line_splits: true,
            continue_on_error: false,
            check: true,
            check_names: true,
            hints: true,
            hints_auto: true,
            hint_case: Case::Upper,
            hint_quote_schemas: false,
            hints_typed_first: false,
            auto_pairs: true,
            format: FmtOptions::default(),
            dark: Palette::dark(),
            light: Palette::light(),
            confirm_unsafe: true,
            autosave_secs: 3,
            embedded_sql: true,
            close_always: false,
            scheme: String::new(),
            backdrop: crate::themes::Backdrop::default(),
            icons: "lucide".into(),
            icon_colors: true,
            fonts: Fonts::default(),
            system_titlebar: true,
            win_style: crate::titlebar::Style::Auto,
            win_order: 0,
            win_left: false,
            labels: default_labels(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Connections {
    pub list: Vec<Spec>,
}

/// A script tab.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub text: String,
    /// Saved in this file (else only in the session).
    pub path: Option<PathBuf>,
    /// The connection it runs on (None: the workspace's).
    pub conn: Option<String>,
    /// The text differs from the file's.
    pub modified: bool,
    /// A package in two parts: the one not on screen (spec or body).
    pub other: Option<String>,
    /// The body is on screen (else the specification).
    pub on_body: bool,
    /// As a notebook (cells), when on.
    pub notebook: Option<Notebook>,
    /// A database object's tab: its designer, or its code (the text).
    pub object: Option<ObjRef>,
}

/// The object a tab shows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjRef {
    pub conn: String,
    /// Its path in the tree.
    pub path: Vec<String>,
    pub kind: crate::db::ObjKind,
    /// The code shown (else the designer).
    #[serde(default)]
    pub code: bool,
}

/// A tab as a notebook: cells of SQL or Markdown.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Notebook {
    /// Shown as cells (off: as one text, the cells kept for their saved
    /// outputs).
    pub on: bool,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellKind {
    #[default]
    Sql,
    Markdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Cell {
    pub id: String,
    pub kind: CellKind,
    pub text: String,
    /// What it gave, kept by hand (the pin button) — nothing else is
    /// written to disk.
    pub saved: Option<Saved>,
    /// Shown as its first line only.
    pub collapsed: bool,
    /// Its output's height (0: the usual).
    pub out_h: f32,
    /// When it was made (local "YYYY-MM-DD HH:MM"; "" — before there was
    /// such a thing).
    #[serde(default)]
    pub created: String,
    /// Its colour labels (indexes into the settings' labels).
    pub labels: Vec<u8>,
}

impl Default for Cell {
    fn default() -> Self {
        Self { id: crate::db::new_id(), kind: CellKind::Sql, text: String::new(), saved: None, collapsed: false, out_h: 0.0, created: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(), labels: Vec::new() }
    }
}

/// A cell label as set: its name ("" — the default one's) and colour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabelDef {
    pub name: String,
    pub color: Rgb,
}

pub fn default_labels() -> Vec<LabelDef> {
    LABELS.iter().map(|l| LabelDef { name: String::new(), color: l.0 }).collect()
}

/// A label's name as shown.
pub fn label_name(labels: &[LabelDef], k: usize) -> String {
    match labels.get(k) {
        Some(l) if !l.name.trim().is_empty() => l.name.clone(),
        _ => LABELS.get(k).map_or(format!("#{}", k + 1), |l| crate::i18n::t(l.1, l.2).to_string()),
    }
}

/// The cells' colour labels to start with: colour and name.
pub const LABELS: [(Rgb, &str, &str); 8] = [
    ([229, 57, 53], "Красная", "Red"),
    ([239, 108, 0], "Оранжевая", "Orange"),
    ([253, 216, 53], "Жёлтая", "Yellow"),
    ([76, 175, 80], "Зелёная", "Green"),
    ([0, 172, 193], "Бирюзовая", "Teal"),
    ([92, 140, 230], "Синяя", "Blue"),
    ([171, 71, 188], "Фиолетовая", "Purple"),
    ([120, 144, 156], "Серая", "Grey"),
];

/// Which schemas / databases a connection's tree shows: all but those
/// named (`only` false), or only those.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SchemaFilter {
    pub only: bool,
    pub names: Vec<String>,
}

impl SchemaFilter {
    pub fn shows(&self, name: &str) -> bool {
        let named = self.names.iter().any(|n| n == name);
        if self.only { named } else { !named }
    }

    pub fn active(&self) -> bool {
        self.only || !self.names.is_empty()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub when: String,
    pub tables: Vec<SavedTable>,
    pub output: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTable {
    pub sql: String,
    /// (name, type, numeric).
    pub cols: Vec<(String, String, bool)>,
    pub rows: Vec<Vec<Option<String>>>,
    pub more: bool,
}

impl Notebook {
    pub fn pieces(&self) -> Vec<crate::sql::notebook::Piece> {
        use crate::sql::notebook::{Kind, Piece};
        self.cells.iter().map(|c| Piece { kind: if c.kind == CellKind::Markdown { Kind::Markdown } else { Kind::Sql }, text: c.text.clone() }).collect()
    }

    /// Cells from a text (its marks, or a cell per statement); saved
    /// outputs of cells with the same text kept.
    pub fn from_text(text: &str, d: crate::sql::Dialect, old: &[Cell]) -> Self {
        use crate::sql::notebook::Kind;
        let cells = crate::sql::notebook::parse(text, d)
            .into_iter()
            .map(|p| {
                let kind = if p.kind == Kind::Markdown { CellKind::Markdown } else { CellKind::Sql };
                let was = old.iter().find(|c| c.kind == kind && c.text == p.text);
                Cell { id: was.map_or_else(crate::db::new_id, |c| c.id.clone()), kind, saved: was.and_then(|c| c.saved.clone()), collapsed: was.is_some_and(|c| c.collapsed), out_h: was.map_or(0.0, |c| c.out_h), created: was.map_or_else(|| Cell::default().created, |c| c.created.clone()), labels: was.map(|c| c.labels.clone()).unwrap_or_default(), text: p.text }
            })
            .collect();
        Self { on: true, cells }
    }
}

impl Default for Tab {
    fn default() -> Self {
        Self { id: crate::db::new_id(), title: String::new(), text: String::new(), path: None, conn: None, modified: false, other: None, on_body: false, notebook: None, object: None }
    }
}

/// A PL/SQL unit with the "/" that ends it in a script.
fn slashed(s: &str) -> String {
    let t = s.trim_end();
    if t.ends_with("\n/") || t == "/" { format!("{t}\n") } else { format!("{t}\n/\n") }
}

/// A part shown apart: without the "/" (sessions from before kept it).
pub fn unslashed(s: &str) -> String {
    let t = s.trim_end();
    match t.strip_suffix("\n/") {
        Some(rest) => format!("{}\n", rest.trim_end()),
        None => s.to_string(),
    }
}

impl Tab {
    /// Both parts of a package, specification first (as saved / run whole).
    pub fn full_text(&self) -> String {
        if let Some(nb) = self.notebook.as_ref().filter(|n| n.on) {
            return crate::sql::notebook::render(&nb.pieces());
        }
        match self.parts() {
            None => self.text.clone(),
            Some((spec, body)) => format!("{}\n{}", slashed(spec), slashed(body)),
        }
    }

    /// The (specification, body) texts, when split.
    pub fn parts(&self) -> Option<(&str, &str)> {
        let o = self.other.as_deref()?;
        Some(if self.on_body { (o, &self.text) } else { (&self.text, o) })
    }

    /// Splits a package's text in two (the specification shown).
    pub fn split_package(&mut self) -> bool {
        if self.other.is_some() {
            return false;
        }
        match crate::sql::outline::split_package(&self.text) {
            Some((spec, body)) => {
                self.text = spec;
                self.other = Some(body);
                self.on_body = false;
                true
            }
            None => false,
        }
    }

    pub fn join_parts(&mut self) {
        if self.other.is_some() {
            self.text = self.full_text();
            self.other = None;
            self.on_body = false;
        }
    }

    /// Whether it's shown as a notebook.
    pub fn is_notebook(&self) -> bool {
        self.notebook.as_ref().is_some_and(|n| n.on)
    }

    /// As a notebook (on) or as one text (off).
    pub fn set_notebook(&mut self, on: bool, d: crate::sql::Dialect) {
        if on == self.is_notebook() {
            return;
        }
        if on {
            self.join_parts();
            let old = self.notebook.take().map(|n| n.cells).unwrap_or_default();
            self.notebook = Some(Notebook::from_text(&self.text, d, &old));
        } else {
            self.text = self.full_text();
            if let Some(n) = self.notebook.as_mut() {
                n.on = false;
            }
        }
    }

    /// Shows the body (or the specification).
    pub fn show_part(&mut self, body: bool) -> bool {
        if body == self.on_body {
            return false;
        }
        let Some(o) = self.other.as_mut() else { return false };
        std::mem::swap(&mut self.text, o);
        self.on_body = body;
        true
    }
}

/// A workspace: a named, coloured group of tabs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub color: Rgb,
    /// The connection its tabs use unless they pick one.
    pub conn: Option<String>,
    pub tabs: Vec<Tab>,
    pub active: usize,
}

impl Default for Workspace {
    fn default() -> Self {
        Self { id: crate::db::new_id(), name: String::new(), color: [92, 140, 230], conn: None, tabs: vec![Tab::default()], active: 0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub workspaces: Vec<Workspace>,
    pub active: usize,
    pub project: Option<PathBuf>,
    pub results_height: f32,
    pub side_width: f32,
    /// The side panel shows the project's files (else the database's objects).
    pub side_files: bool,
    /// The structure panel beside the editor.
    pub outline: bool,
    pub outline_width: f32,
    /// Per connection: the schemas its tree shows.
    pub schema_filters: std::collections::BTreeMap<String, SchemaFilter>,
}

impl Default for Session {
    fn default() -> Self {
        Self { workspaces: vec![Workspace { name: "Main".into(), ..Default::default() }], active: 0, project: None, results_height: 260.0, side_width: 280.0, side_files: false, outline: true, outline_width: 250.0, schema_filters: Default::default() }
    }
}

pub const COLORS: &[Rgb] = &[[92, 140, 230], [76, 175, 80], [239, 108, 0], [229, 57, 53], [171, 71, 188], [0, 172, 193], [253, 216, 53], [141, 110, 99], [120, 144, 156], [236, 64, 122]];
