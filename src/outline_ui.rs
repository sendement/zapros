//! The structure panel beside the editor: the script's statements and the
//! objects they make; a package's functions, procedures, types, cursors…
//! (both its parts when split). A click goes there; the entity the cursor
//! is in is lit. And the specification / body switch over the editor.

use egui::{Align2, Color32, FontId, RichText, Sense, Stroke, Vec2};

use crate::app::App;
use crate::editor::rgb;
use crate::i18n::t;
use crate::sql::outline::{self, Ent, Entity};

/// The current tab's entities: (specification or whole text, body).
fn entities(app: &mut App, ctx: &egui::Context) -> (Vec<Entity>, Vec<Entity>) {
    let ws = app.ws().clone();
    let tab = app.tab().clone();
    let d = app.dialect_of(app.tab_conn(&ws, &tab).as_deref());
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&tab.text, &tab.other, tab.on_body, d).hash(&mut h);
        h.finish()
    };
    let rt = app.rt.entry(tab.id.clone()).or_default();
    if let Some((k, a, b)) = &rt.outline {
        if *k == key {
            rt.outline_due = None;
            return (a.clone(), b.clone());
        }
        // Typing: the last one stays a moment (a long script takes a while).
        let due = *rt.outline_due.get_or_insert_with(|| std::time::Instant::now() + std::time::Duration::from_millis(300));
        let now = std::time::Instant::now();
        if now < due {
            ctx.request_repaint_after(due - now);
            return (a.clone(), b.clone());
        }
    }
    rt.outline_due = None;
    let (a, b) = match tab.parts() {
        Some((spec, body)) => (outline::outline(spec, d), outline::outline(body, d)),
        None => (outline::outline(&tab.text, d), Vec::new()),
    };
    rt.outline = Some((key, a.clone(), b.clone()));
    (a, b)
}

/// Specification | Body over the editor of a split package.
pub fn parts_bar(app: &mut App, ui: &mut egui::Ui) {
    let tab = app.tab();
    if tab.other.is_none() {
        return;
    }
    let on_body = tab.on_body;
    let color = rgb(app.ws().color);
    let v = ui.visuals().clone();
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // A segmented control: two halves of one rounded box.
        for (i, (label, tip)) in [(t("Спецификация", "Specification"), "PACKAGE"), (t("Тело", "Body"), "PACKAGE BODY")].into_iter().enumerate() {
            let body = i == 1;
            let on = body == on_body;
            let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(13.5), if on { v.strong_text_color() } else { v.text_color() });
            let size = Vec2::new(galley.size().x + 28.0, ui.spacing().interact_size.y);
            let (rect, r) = ui.allocate_exact_size(size, Sense::click());
            let cr = if body { egui::CornerRadius { nw: 0, sw: 0, ne: crate::ui::RADIUS, se: crate::ui::RADIUS } } else { egui::CornerRadius { nw: crate::ui::RADIUS, sw: crate::ui::RADIUS, ne: 0, se: 0 } };
            let fill = if on {
                color.gamma_multiply(if v.dark_mode { 0.30 } else { 0.18 })
            } else if r.hovered() {
                v.widgets.hovered.weak_bg_fill
            } else {
                v.widgets.inactive.weak_bg_fill
            };
            ui.painter().rect(rect, cr, fill, Stroke::new(1.0, if on { color } else { v.widgets.inactive.bg_stroke.color }), egui::StrokeKind::Inside);
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, v.text_color());
            if r.on_hover_text(format!("{tip}   Alt+1 / Alt+2")).clicked() {
                pick = Some(body);
            }
        }
        ui.add_space(10.0);
        let hint = if on_body { t("тело пакета — выполняется (компилируется) отдельно", "the package body: runs (compiles) on its own") } else { t("спецификация — выполняется (компилируется) отдельно", "the specification: runs (compiles) on its own") };
        ui.label(RichText::new(hint).small().weak());
    });
    if let Some(b) = pick {
        app.show_part(b);
    }
}

/// The panel (when shown).
pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    let (spec, body) = entities(app, &ui.ctx().clone());
    let split = app.tab().other.is_some();
    let on_body = app.tab().on_body;
    let tab_id = app.tab().id.clone();
    let cursor = app.rt.get(&tab_id).and_then(|r| r.ed.cursor).map(|c| c.0);
    let v = ui.visuals().clone();
    let mut go: Option<(Option<bool>, usize)> = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(t("Структура", "Structure")).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new("🗙").frame(false)).on_hover_text(t("Скрыть   Ctrl+Shift+O", "Hide   Ctrl+Shift+O")).clicked() {
                app.session.outline = false;
                app.touch();
            }
        });
    });
    let filter_id = egui::Id::new("outline-filter");
    let r = ui.add(egui::TextEdit::singleline(&mut app.outline_filter).id(filter_id).hint_text(t("🔎 найти", "🔎 find")).desired_width(f32::INFINITY));
    if std::mem::take(&mut app.outline_focus) {
        r.request_focus();
    }
    let filter = app.outline_filter.to_lowercase();
    ui.add_space(2.0);
    // The lists shown: (entities, part, the other part's, cursor's entity).
    let mut lists: Vec<(Vec<Entity>, Option<bool>, Vec<Entity>)> = Vec::new();
    if split {
        // (Members at the package's level.)
        let flat = |ents: &[Entity]| ents.iter().filter(|e| !matches!(e.kind, Ent::Package | Ent::PackageBody)).map(|e| Entity { depth: e.depth.saturating_sub(1), ..e.clone() }).collect::<Vec<_>>();
        lists.push((flat(&spec), Some(false), flat(&body)));
        lists.push((flat(&body), Some(true), flat(&spec)));
    } else {
        lists.push((spec, None, Vec::new()));
    }
    enum Row {
        Head(bool),
        Note(&'static str),
        Ent(usize, usize),
    }
    let mut rows = Vec::new();
    let mut here: Vec<Option<usize>> = Vec::new();
    for (li, (ents, part, _)) in lists.iter().enumerate() {
        if let Some(b) = part {
            rows.push(Row::Head(*b));
        }
        // The cursor's entity: only in the part on screen.
        here.push(if part.is_none_or(|p| p == on_body) { cursor.and_then(|c| outline::current(ents, c)) } else { None });
        let before = rows.len();
        rows.extend((0..ents.len()).filter(|&i| filter.is_empty() || ents[i].name.to_lowercase().contains(&filter)).map(|i| Row::Ent(li, i)));
        if rows.len() == before {
            rows.push(Row::Note(if ents.is_empty() { t("пусто", "empty") } else { t("ничего не найдено", "nothing found") }));
        }
    }
    const ROW: f32 = 22.0;
    let mut show_part = None;
    // Only the rows in sight are laid out (a script may have thousands).
    egui::ScrollArea::vertical().id_salt("outline").auto_shrink([false, false]).show_rows(ui, ROW + 1.0, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 1.0;
        for row in &rows[range] {
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW), Sense::click());
            let (li, i) = match row {
                Row::Head(body) => {
                    let head = if *body { t("Тело", "Body") } else { t("Спецификация", "Specification") };
                    let color = if *body == on_body { v.strong_text_color() } else { v.weak_text_color() };
                    ui.painter().text(egui::pos2(rect.left() + 2.0, rect.bottom() - 3.0), Align2::LEFT_BOTTOM, head, FontId::proportional(11.5), color);
                    if resp.on_hover_text(t("Показать эту часть", "Show this part")).clicked() {
                        show_part = Some(*body);
                    }
                    continue;
                }
                Row::Note(n) => {
                    ui.painter().text(egui::pos2(rect.left() + 6.0, rect.center().y), Align2::LEFT_CENTER, *n, FontId::proportional(11.5), v.weak_text_color());
                    continue;
                }
                Row::Ent(li, i) => (*li, *i),
            };
            let (ents, part, other) = &lists[li];
            let e = &ents[i];
            if Some(i) == here[li] {
                ui.painter().rect_filled(rect, crate::ui::RADIUS, v.selection.bg_fill.gamma_multiply(if v.dark_mode { 0.45 } else { 0.30 }));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, crate::ui::RADIUS, v.widgets.hovered.weak_bg_fill);
            }
            let x = rect.left() + 6.0 + 14.0 * e.depth as f32;
            ui.painter().text(egui::pos2(x + 7.0, rect.center().y), Align2::CENTER_CENTER, e.kind.icon(), FontId::proportional(13.0), kind_color(e.kind, &v));
            let label = if e.name.is_empty() { e.kind.name().to_string() } else { e.name.clone() };
            let color = if e.depth == 0 { v.strong_text_color() } else { v.text_color() };
            let room = (rect.right() - x - 24.0).max(10.0);
            let mut job = egui::text::LayoutJob::single_section(label, egui::TextFormat::simple(FontId::proportional(13.5), color));
            job.wrap = egui::text::TextWrapping { max_width: room, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let name = ui.painter().layout_job(job);
            let nw = name.size().x;
            ui.painter().galley(egui::pos2(x + 18.0, rect.center().y - name.size().y / 2.0), name, color);
            // The signature after the name, as much as fits.
            if !e.detail.is_empty() && x + 26.0 + nw < rect.right() - 20.0 {
                let left = rect.right() - (x + 26.0 + nw) - 4.0;
                let mut job = egui::text::LayoutJob::single_section(e.detail.clone(), egui::TextFormat::simple(FontId::proportional(11.5), v.weak_text_color()));
                job.wrap = egui::text::TextWrapping { max_width: left, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = ui.painter().layout_job(job);
                ui.painter().galley(egui::pos2(x + 24.0 + nw, rect.center().y - g.size().y / 2.0 + 1.0), g, v.weak_text_color());
            }
            let tip = format!("{} {}\n{}", e.kind.name(), e.name, e.detail);
            let resp = resp.on_hover_text(tip.trim());
            if resp.clicked() {
                go = Some((*part, e.at));
            }
            // Double click on a declaration: to its body.
            if resp.double_clicked() && e.decl && matches!(e.kind, Ent::Function | Ent::Procedure) {
                if let Some(b) = other.iter().find(|b| b.kind == e.kind && b.name.eq_ignore_ascii_case(&e.name) && !b.decl) {
                    go = Some((part.map(|p| !p), b.at));
                }
            }
        }
    });
    if let Some(b) = show_part {
        app.show_part(b);
    }
    if let Some((part, at)) = go {
        if let Some(b) = part {
            app.show_part(b);
        }
        let id = app.tab().id.clone();
        let rt = app.rt.entry(id).or_default();
        rt.ed.goto = Some(at);
    }
}

fn kind_color(k: Ent, v: &egui::Visuals) -> Color32 {
    let dark = v.dark_mode;
    let pick = |d: (u8, u8, u8), l: (u8, u8, u8)| if dark { Color32::from_rgb(d.0, d.1, d.2) } else { Color32::from_rgb(l.0, l.1, l.2) };
    match k {
        Ent::Function => pick((97, 175, 239), (30, 100, 190)),
        Ent::Procedure => pick((198, 120, 221), (140, 60, 170)),
        Ent::Package | Ent::PackageBody | Ent::Type | Ent::TypeBody | Ent::Subtype => pick((229, 192, 123), (170, 110, 0)),
        Ent::Cursor | Ent::Table | Ent::View | Ent::MatView | Ent::Index => pick((86, 182, 194), (0, 120, 140)),
        Ent::Exception => pick((224, 108, 117), (190, 40, 50)),
        Ent::Constant | Ent::Variable => pick((152, 195, 121), (60, 130, 40)),
        _ => v.weak_text_color(),
    }
}
