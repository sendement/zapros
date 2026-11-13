//! Fonts: the interface's, the code's (and its bold, for keywords), the
//! tables' — any family fontconfig knows, the system's sans / monospace
//! by default; egui's own fonts after them for anything they lack.

use std::sync::{Arc, OnceLock};

use egui::FontFamily;

use crate::config::Fonts;

fn fc_match(pattern: &str) -> Option<(Vec<u8>, u32)> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}\t%{index}", pattern]).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let (path, index) = text.split_once('\t')?;
    let lower = path.to_lowercase();
    if ![".ttf", ".otf", ".ttc", ".otc"].iter().any(|e| lower.ends_with(e)) {
        return None;
    }
    Some((std::fs::read(path.trim()).ok()?, index.trim().parse().unwrap_or(0)))
}

/// The families installed: (name, monospaced), by name.
pub fn families() -> &'static [(String, bool)] {
    static LIST: OnceLock<Vec<(String, bool)>> = OnceLock::new();
    LIST.get_or_init(|| {
        let Ok(out) = std::process::Command::new("fc-list").args(["--format", "%{family[0]}\t%{spacing}\t%{file}\n"]).output() else { return Vec::new() };
        let mut v: Vec<(String, bool)> = Vec::new();
        for l in String::from_utf8_lossy(&out.stdout).lines() {
            let mut p = l.split('\t');
            let (name, spacing, file) = (p.next().unwrap_or("").trim(), p.next().unwrap_or(""), p.next().unwrap_or("").to_lowercase());
            // (Only what egui can load: TrueType / OpenType.)
            if name.is_empty() || name.starts_with('.') || ![".ttf", ".otf", ".ttc", ".otc"].iter().any(|e| file.ends_with(e)) {
                continue;
            }
            let mono = spacing.trim() == "100" || spacing.trim() == "110";
            match v.iter_mut().find(|(n, _)| n == name) {
                Some(e) => e.1 |= mono,
                None => v.push((name.to_string(), mono)),
            }
        }
        v.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        v
    })
}

/// The pattern for a family ("" — the generic one), bold or not.
fn pattern(family: &str, generic: &str, bold: bool) -> String {
    let base = if family.trim().is_empty() { generic.to_string() } else { family.replace('-', "\\-").replace(':', "\\:") };
    if bold { format!("{base}:bold") } else { base }
}

pub fn install(ctx: &egui::Context, f: &Fonts) {
    let mut defs = egui::FontDefinitions::default();
    let add = |defs: &mut egui::FontDefinitions, key: &str, (bytes, index): (Vec<u8>, u32)| {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        defs.font_data.insert(key.into(), Arc::new(data));
    };
    let egui_prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let egui_mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    // A family: the chosen font first, then egui's (and the other fonts'
    // glyphs: symbols, Cyrillic, whatever the first lacks).
    let chain = |defs: &mut egui::FontDefinitions, key: &str, pat: String, rest: &[String]| -> Vec<String> {
        let mut v = Vec::new();
        if let Some(font) = fc_match(&pat) {
            add(defs, key, font);
            v.push(key.to_string());
        }
        for r in rest {
            if !v.contains(r) {
                v.push(r.clone());
            }
        }
        v
    };
    let ui = chain(&mut defs, "ui", pattern(&f.ui, "sans-serif", false), &egui_prop);
    let ui_bold = chain(&mut defs, "ui-bold", pattern(&f.ui, "sans-serif", true), &ui);
    let mut code = chain(&mut defs, "code", pattern(&f.code, "monospace", false), &egui_mono);
    for k in &ui {
        if !code.contains(k) {
            code.push(k.clone());
        }
    }
    let code_bold = chain(&mut defs, "code-bold", pattern(&f.code, "monospace", true), &code);
    let table = if f.table.trim().is_empty() { code.clone() } else { chain(&mut defs, "table", pattern(&f.table, "monospace", false), &code) };
    // Shapes (● ▦ ▸ ⋈) the sans font lacks: the code font has them.
    let mut ui_all = ui.clone();
    if defs.font_data.contains_key("code") && !ui_all.contains(&"code".to_string()) {
        ui_all.push("code".into());
    }
    defs.families.insert(FontFamily::Proportional, ui_all);
    defs.families.insert(FontFamily::Monospace, code);
    defs.families.insert(FontFamily::Name("mono-bold".into()), code_bold);
    defs.families.insert(FontFamily::Name("bold".into()), ui_bold);
    defs.families.insert(FontFamily::Name("table".into()), table);
    ctx.set_fonts(defs);
}

/// The text styles' sizes from the interface's size; the tables' style.
pub fn sizes(s: &mut egui::Style, f: &Fonts) {
    use egui::{FontId, TextStyle};
    let k = f.ui_size.clamp(8.0, 32.0);
    s.text_styles.insert(TextStyle::Body, FontId::proportional(k));
    s.text_styles.insert(TextStyle::Button, FontId::proportional(k));
    // (egui's own proportions: 9 / 13 / 18.)
    s.text_styles.insert(TextStyle::Small, FontId::proportional((k * 9.0 / 13.0).round()));
    s.text_styles.insert(TextStyle::Heading, FontId::proportional((k * 18.0 / 13.0).round()));
    s.text_styles.insert(TextStyle::Monospace, FontId::monospace(k));
    s.text_styles.insert(TextStyle::Name("table".into()), FontId::new(f.table_size.clamp(7.0, 32.0), FontFamily::Name("table".into())));
}
