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
    /// Rows a result takes at most.
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
    pub auto_pairs: bool,
    pub format: FmtOptions,
    pub dark: Palette,
    pub light: Palette,
    /// Confirm UPDATE / DELETE without WHERE before running.
    pub confirm_unsafe: bool,
    pub autosave_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            russian: None,
            theme: ThemePref::System,
            ui_scale: 1.0,
            font_size: 14.0,
            tab_width: 4,
            row_limit: 1000,
            blank_line_splits: true,
            continue_on_error: false,
            check: true,
            check_names: true,
            hints: true,
            hints_auto: true,
            hint_case: Case::Upper,
            auto_pairs: true,
            format: FmtOptions::default(),
            dark: Palette::dark(),
            light: Palette::light(),
            confirm_unsafe: true,
            autosave_secs: 3,
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
}

impl Default for Tab {
    fn default() -> Self {
        Self { id: crate::db::new_id(), title: String::new(), text: String::new(), path: None, conn: None, modified: false }
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
}

impl Default for Session {
    fn default() -> Self {
        Self { workspaces: vec![Workspace { name: "Main".into(), ..Default::default() }], active: 0, project: None, results_height: 260.0, side_width: 280.0, side_files: false }
    }
}

pub const COLORS: &[Rgb] = &[[92, 140, 230], [76, 175, 80], [239, 108, 0], [229, 57, 53], [171, 71, 188], [0, 172, 193], [253, 216, 53], [141, 110, 99], [120, 144, 156], [236, 64, 122]];
