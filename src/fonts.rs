//! The system's sans and monospace fonts (fontconfig), a bold monospace
//! for keywords, egui's own fonts after them for anything they lack.

use std::sync::Arc;

use egui::FontFamily;

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

pub fn install(ctx: &egui::Context) {
    let mut defs = egui::FontDefinitions::default();
    let add = |defs: &mut egui::FontDefinitions, key: &str, (bytes, index): (Vec<u8>, u32)| {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        defs.font_data.insert(key.into(), Arc::new(data));
    };
    if let Some(f) = fc_match("sans-serif") {
        add(&mut defs, "sys-sans", f);
        defs.families.entry(FontFamily::Proportional).or_default().insert(0, "sys-sans".into());
    }
    if let Some(f) = fc_match("monospace") {
        add(&mut defs, "sys-mono", f);
        defs.families.entry(FontFamily::Monospace).or_default().insert(0, "sys-mono".into());
    }
    let mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut bold = mono.clone();
    if let Some(f) = fc_match("monospace:bold") {
        add(&mut defs, "sys-mono-bold", f);
        bold.insert(0, "sys-mono-bold".into());
    }
    defs.families.insert(FontFamily::Name("mono-bold".into()), bold);
    // Letters and symbols the monospace font lacks.
    let prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for k in prop {
        for fam in [FontFamily::Monospace, FontFamily::Name("mono-bold".into())] {
            let v = defs.families.entry(fam).or_default();
            if !v.contains(&k) {
                v.push(k.clone());
            }
        }
    }
    // Bold text (a notebook's Markdown): the sans font's bold, then the rest.
    let mut sans_bold = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(f) = fc_match("sans-serif:bold") {
        add(&mut defs, "sys-sans-bold", f);
        sans_bold.insert(0, "sys-sans-bold".into());
    }
    defs.families.insert(FontFamily::Name("bold".into()), sans_bold);
    // And the other way: shapes (● ▦ ▸ ⋈) the sans font lacks.
    if defs.font_data.contains_key("sys-mono") {
        defs.families.entry(FontFamily::Proportional).or_default().push("sys-mono".into());
    }
    ctx.set_fonts(defs);
}
