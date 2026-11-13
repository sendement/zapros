//! Icons from free packs (Lucide, Tabler, Phosphor, Codicons, Fluent,
//! Material Symbols, Iconoir, Bootstrap) — SVGs drawn white and tinted —
//! or, with none picked, the font's symbols as before.

use std::sync::atomic::{AtomicUsize, Ordering};

use egui::{Color32, Vec2};

use crate::db::ObjKind;
use crate::icons_data::{CONCEPTS, PACKS, SVG};

/// What an icon shows (in the order of `CONCEPTS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ic {
    Connection,
    Schema,
    Folder,
    Table,
    View,
    MatView,
    Function,
    Procedure,
    Package,
    Trigger,
    Sequence,
    Index,
    Job,
    Dictionary,
    Type,
    Synonym,
    Column,
    Key,
    Run,
    RunAll,
    Stop,
    Explain,
    Format,
    Commit,
    Rollback,
    Notebook,
    Files,
    File,
    Connections,
    Settings,
    Help,
    Outline,
    Filter,
    Export,
}

impl Ic {
    pub fn of(k: ObjKind) -> Ic {
        match k {
            ObjKind::Database => Ic::Connection,
            ObjKind::Schema => Ic::Schema,
            ObjKind::Folder => Ic::Folder,
            ObjKind::Table => Ic::Table,
            ObjKind::View => Ic::View,
            ObjKind::MatView => Ic::MatView,
            ObjKind::Function => Ic::Function,
            ObjKind::Procedure => Ic::Procedure,
            ObjKind::Package => Ic::Package,
            ObjKind::Trigger => Ic::Trigger,
            ObjKind::Sequence => Ic::Sequence,
            ObjKind::Index => Ic::Index,
            ObjKind::Job => Ic::Job,
            ObjKind::Dictionary => Ic::Dictionary,
            ObjKind::Type => Ic::Type,
            ObjKind::Synonym => Ic::Synonym,
            ObjKind::Column => Ic::Column,
            ObjKind::Key => Ic::Key,
        }
    }

    /// The symbol a button or label had before the packs.
    pub fn for_glyph(g: &str) -> Option<Ic> {
        Some(match g {
            "▶" => Ic::Run,
            "⏩" => Ic::RunAll,
            "■" => Ic::Stop,
            "🔍" => Ic::Explain,
            "✨" => Ic::Format,
            "✔" => Ic::Commit,
            "⟲" => Ic::Rollback,
            "📓" => Ic::Notebook,
            "☰" => Ic::Outline,
            "🗄" => Ic::Connection,
            "🗁" => Ic::Files,
            "🗋" => Ic::File,
            "🔌" => Ic::Connections,
            "⚙" => Ic::Settings,
            "?" => Ic::Help,
            "📁" => Ic::Folder,
            "💾" => Ic::Export,
            "📜" | "📄" => Ic::File,
            _ => return None,
        })
    }
}

const NONE: usize = usize::MAX;
static PACK: AtomicUsize = AtomicUsize::new(NONE);

/// The pack in use ("" or unknown: the font's symbols).
pub fn set_pack(id: &str) {
    PACK.store(PACKS.iter().position(|p| p.0 == id).unwrap_or(NONE), Ordering::Relaxed);
}

/// The icon of pack `pack` (an index into PACKS), `size` points, tinted.
pub fn image_of(pack: usize, ic: Ic, size: f32, tint: Color32) -> egui::Image<'static> {
    let c = ic as usize;
    let uri = format!("bytes://icons/{}/{}.svg", PACKS[pack].0, CONCEPTS[c]);
    egui::Image::from_bytes(uri, SVG[pack][c]).fit_to_exact_size(Vec2::splat(size)).tint(tint)
}

/// The icon in the pack in use (None: symbols are in use).
pub fn image(ic: Ic, size: f32, tint: Color32) -> Option<egui::Image<'static>> {
    let p = PACK.load(Ordering::Relaxed);
    (p != NONE).then(|| image_of(p, ic, size, tint))
}

/// Painted into `rect` (centred, `size` points).
pub fn paint(ui: &egui::Ui, rect: egui::Rect, ic: Ic, size: f32, tint: Color32) -> bool {
    match image(ic, size, tint) {
        Some(img) => {
            img.paint_at(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(size)));
            true
        }
        None => false,
    }
}

/// An object kind's colour in the tree, from the editor's palette.
pub fn kind_color(k: ObjKind, p: &crate::config::Palette, weak: Color32) -> Color32 {
    let c = |x: crate::config::Rgb| Color32::from_rgb(x[0], x[1], x[2]);
    match k {
        ObjKind::Table => c(p.function),
        ObjKind::View | ObjKind::MatView => c(p.quoted),
        ObjKind::Function | ObjKind::Procedure => c(p.keyword),
        ObjKind::Package | ObjKind::Type => c(p.type_),
        ObjKind::Trigger | ObjKind::Job => c(p.number),
        ObjKind::Sequence | ObjKind::Dictionary => c(p.string),
        ObjKind::Key => c(p.type_),
        ObjKind::Synonym | ObjKind::Index => c(p.param),
        _ => weak,
    }
}

/// The licences of the packs in use, for "About".
pub fn credits() -> String {
    PACKS.iter().map(|p| format!("{} — {}", p.1, p.2)).collect::<Vec<_>>().join(" · ")
}
