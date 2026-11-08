//! Results under the editor: a tab per statement that gave rows, and the
//! messages (what ran, how long, errors, NOTICEs / DBMS_OUTPUT). Tables
//! only lay out the rows on screen, sort by a header click, filter by text,
//! copy a cell / row / all, save as CSV.

use egui::{Align, Color32, Layout, RichText};
use egui_extras::{Column, TableBuilder};

use crate::app::{App, LogKind, ResultView};
use crate::i18n::t;

fn tsv_cell(v: &Option<String>) -> String {
    v.as_deref().unwrap_or("").replace(['\t', '\n'], " ")
}

fn csv_cell(v: &Option<String>) -> String {
    match v {
        None => String::new(),
        Some(s) if s.contains([',', '"', '\n', '\r', ';']) => format!("\"{}\"", s.replace('"', "\"\"")),
        Some(s) => s.clone(),
    }
}

pub fn as_tsv(v: &ResultView) -> String {
    let mut s = v.table.cols.iter().map(|c| c.name.clone()).collect::<Vec<_>>().join("\t");
    for &r in &v.order {
        s.push('\n');
        s += &v.table.rows[r].iter().map(tsv_cell).collect::<Vec<_>>().join("\t");
    }
    s
}

pub fn as_csv(v: &ResultView) -> String {
    let mut s = v.table.cols.iter().map(|c| csv_cell(&Some(c.name.clone()))).collect::<Vec<_>>().join(",");
    for &r in &v.order {
        s.push('\n');
        s += &v.table.rows[r].iter().map(csv_cell).collect::<Vec<_>>().join(",");
    }
    s.push('\n');
    s
}

/// Rows shown: filtered by text, sorted by a column.
pub fn reorder(v: &mut ResultView) {
    let f = v.filter.to_lowercase();
    let rows = &v.table.rows;
    let mut order: Vec<usize> = (0..rows.len()).filter(|&r| f.is_empty() || rows[r].iter().any(|c| c.as_deref().is_some_and(|s| s.to_lowercase().contains(&f)))).collect();
    if let Some((c, asc)) = v.sort {
        let numeric = v.table.cols.get(c).is_some_and(|c| c.numeric);
        order.sort_by(|&a, &b| {
            let (x, y) = (rows[a].get(c).and_then(|v| v.as_deref()), rows[b].get(c).and_then(|v| v.as_deref()));
            let o = match (x, y) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, _) => std::cmp::Ordering::Greater,
                (_, None) => std::cmp::Ordering::Less,
                (Some(x), Some(y)) if numeric => x.parse::<f64>().unwrap_or(f64::NAN).partial_cmp(&y.parse::<f64>().unwrap_or(f64::NAN)).unwrap_or(x.cmp(y)),
                (Some(x), Some(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
            };
            if asc { o } else { o.reverse() }
        });
    }
    v.order = order;
}

fn export_csv(app: &mut App, v: &ResultView) {
    let dir = app.session.project.clone().or_else(dirs::download_dir).or_else(dirs::home_dir).unwrap_or_default();
    let name = format!("result-{}.csv", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let path = dir.join(name);
    match std::fs::write(&path, as_csv(v)) {
        Ok(()) => app.toast(format!("{} {}", t("Сохранено в", "Saved to"), path.display())),
        Err(e) => app.toast(format!("{}: {e}", path.display())),
    }
}

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    let tab_id = app.tab().id.clone();
    let mut rt = app.rt.remove(&tab_id).unwrap_or_default();
    let mut export: Option<usize> = None;
    ui.horizontal(|ui| {
        egui::ScrollArea::horizontal().id_salt("res-tabs").max_width(ui.available_width() - 320.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, r) in rt.results.iter().enumerate() {
                    let label = format!("▦ {} ({}{})", r.title, r.table.rows.len(), if r.table.more { "+" } else { "" });
                    if ui.selectable_label(rt.shown == Some(i), label).on_hover_text(crate::app::head(&r.sql, 400)).clicked() {
                        rt.shown = Some(i);
                    }
                }
                let errs = rt.log.iter().filter(|l| l.kind == LogKind::Error).count();
                let label = if errs > 0 { RichText::new(format!("✉ {} ⛔{errs}", t("Сообщения", "Messages"))).color(Color32::from_rgb(230, 90, 90)) } else { RichText::new(format!("✉ {}", t("Сообщения", "Messages"))) };
                if ui.selectable_label(rt.shown.is_none(), label).clicked() {
                    rt.shown = None;
                }
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(v) = rt.shown.and_then(|i| rt.results.get_mut(i)) {
                if ui.button("CSV").on_hover_text(t("Сохранить как CSV", "Save as CSV")).clicked() {
                    export = rt.shown;
                }
                if ui.button("🗐").on_hover_text(t("Копировать всё (TSV, для таблиц)", "Copy all (TSV, for spreadsheets)")).clicked() {
                    ui.ctx().copy_text(as_tsv(v));
                }
                let fr = ui.add(egui::TextEdit::singleline(&mut v.filter).hint_text(t("фильтр…", "filter…")).desired_width(140.0));
                if fr.changed() {
                    reorder(v);
                }
                let more = if v.table.more { format!(" · {}", t("показаны первые", "first shown")) } else { String::new() };
                ui.label(RichText::new(format!("{} / {} {}{more} · {}", v.order.len(), v.table.rows.len(), t("строк", "rows"), crate::app::fmt_took(v.took))).small().weak());
            } else if !rt.log.is_empty() && ui.button(t("Очистить", "Clear")).clicked() {
                rt.log.clear();
            }
        });
    });
    ui.separator();
    match rt.shown.filter(|&i| i < rt.results.len()) {
        Some(i) => table(ui, &tab_id, i, &mut rt.results[i]),
        None => {
            let mut goto = None;
            egui::ScrollArea::vertical().id_salt(("log", &tab_id)).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                if rt.log.is_empty() {
                    ui.label(RichText::new(t("Ctrl+Enter — выполнить запрос под курсором, Ctrl+Shift+Enter — весь скрипт", "Ctrl+Enter: run the statement at the cursor, Ctrl+Shift+Enter: the whole script")).weak());
                }
                for l in &rt.log {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&l.when).small().weak().monospace());
                        let (icon, color) = match l.kind {
                            LogKind::Run => ("▶", ui.visuals().weak_text_color()),
                            LogKind::Ok => ("✔", Color32::from_rgb(80, 180, 90)),
                            LogKind::Note => ("ℹ", ui.visuals().text_color()),
                            LogKind::Error => ("⛔", Color32::from_rgb(230, 80, 80)),
                        };
                        let text = RichText::new(format!("{icon} {}", l.text)).color(color);
                        let text = if l.kind == LogKind::Run { text.monospace() } else { text };
                        let r = ui.add(egui::Label::new(text).sense(if l.at.is_some() { egui::Sense::click() } else { egui::Sense::hover() }));
                        if let Some(at) = l.at {
                            if r.on_hover_text(t("Щелчок — к месту в скрипте", "Click: go there in the script")).clicked() {
                                goto = Some(at);
                            }
                        }
                    });
                }
            });
            if let Some(g) = goto {
                rt.ed.goto = Some(g);
            }
        }
    }
    if let Some(i) = export {
        let v = std::mem::replace(&mut rt.results[i], ResultView { title: String::new(), sql: String::new(), table: Default::default(), took: Default::default(), order: Vec::new(), sort: None, sel: None, filter: String::new() });
        export_csv(app, &v);
        rt.results[i] = v;
    }
    app.rt.insert(tab_id, rt);
}

fn table(ui: &mut egui::Ui, tab: &str, i: usize, v: &mut ResultView) {
    let ctx = ui.ctx().clone();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    if v.table.cols.is_empty() {
        ui.label(t("Нет колонок", "No columns"));
        return;
    }
    // The chosen cell, whole.
    if let Some((r, c)) = v.sel {
        if let Some(val) = v.table.rows.get(r).and_then(|row| row.get(c)) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}:", v.table.cols[c].name)).small().strong());
                let s = val.clone().unwrap_or_else(|| "NULL".into());
                let one = crate::app::head(&s, 240);
                ui.label(RichText::new(one).small().monospace());
                if ui.small_button("🗐").on_hover_text(t("Копировать значение", "Copy value")).clicked() {
                    ctx.copy_text(s);
                }
            });
        }
    }
    let widths: Vec<f32> = v
        .table
        .cols
        .iter()
        .enumerate()
        .map(|(c, col)| {
            let longest = v.table.rows.iter().take(100).map(|r| r.get(c).and_then(|x| x.as_deref()).map_or(4, |s| s.chars().count().min(60))).max().unwrap_or(4).max(col.name.chars().count() + 2);
            (longest as f32 * char_w + 16.0).clamp(48.0, 420.0)
        })
        .collect();
    let num_w = (v.table.rows.len().to_string().len() as f32 * char_w + 14.0).max(30.0);
    let mut sort_click: Option<usize> = None;
    let mut sel = v.sel;
    let mut copy: Option<String> = None;
    let mut filter_to: Option<String> = None;
    let sel_bg = ui.visuals().selection.stroke.color;
    let null_color = ui.visuals().weak_text_color();
    let mut tb = TableBuilder::new(ui).id_salt(("res", tab, i)).striped(true).resizable(true).auto_shrink([false, false]).min_scrolled_height(0.0).cell_layout(Layout::left_to_right(Align::Center)).column(Column::exact(num_w));
    for w in &widths {
        tb = tb.column(Column::initial(*w).at_least(30.0).clip(true));
    }
    let cols = v.table.cols.clone();
    let order = v.order.clone();
    let sort = v.sort;
    let rows = &v.table.rows;
    tb.header(22.0, |mut h| {
        h.col(|ui| {
            ui.label(RichText::new("#").weak());
        });
        for (c, col) in cols.iter().enumerate() {
            h.col(|ui| {
                let arrow = match sort {
                    Some((sc, true)) if sc == c => " ▲",
                    Some((sc, false)) if sc == c => " ▼",
                    _ => "",
                };
                let r = ui.add(egui::Label::new(RichText::new(format!("{}{arrow}", col.name)).strong()).sense(egui::Sense::click()).truncate());
                let r = if col.ty.is_empty() { r } else { r.on_hover_text(&col.ty) };
                if r.clicked() {
                    sort_click = Some(c);
                }
                r.context_menu(|ui| {
                    if ui.button(t("Копировать имя", "Copy name")).clicked() {
                        copy = Some(col.name.clone());
                        ui.close();
                    }
                    if ui.button(t("Копировать колонку", "Copy column")).clicked() {
                        copy = Some(order.iter().map(|&r| tsv_cell(&rows[r][c])).collect::<Vec<_>>().join("\n"));
                        ui.close();
                    }
                });
            });
        }
    })
    .body(|body| {
        body.rows(20.0, order.len(), |mut row| {
            let ri = order[row.index()];
            row.col(|ui| {
                ui.label(RichText::new((ri + 1).to_string()).weak().small());
            });
            for (c, col) in cols.iter().enumerate() {
                let (rect, resp) = row.col(|ui| {
                    let val = rows[ri].get(c).and_then(|x| x.as_ref());
                    let layout = if col.numeric { Layout::right_to_left(Align::Center) } else { Layout::left_to_right(Align::Center) };
                    ui.with_layout(layout, |ui| match val {
                        None => {
                            ui.add(egui::Label::new(RichText::new("NULL").italics().color(null_color).small()).selectable(false));
                        }
                        Some(s) => {
                            let one = if s.contains('\n') { s.replace('\n', "⏎") } else { s.clone() };
                            ui.add(egui::Label::new(RichText::new(one).monospace()).truncate().selectable(false));
                        }
                    });
                });
                let resp = resp.interact(egui::Sense::click());
                if sel == Some((ri, c)) {
                    resp.ctx.layer_painter(resp.layer_id).rect_stroke(rect.shrink(1.0), 2.0, egui::Stroke::new(1.5, sel_bg), egui::StrokeKind::Inside);
                }
                if resp.clicked() {
                    sel = Some((ri, c));
                }
                resp.context_menu(|ui| {
                    let val = rows[ri].get(c).cloned().flatten();
                    if ui.button(t("Копировать значение", "Copy value")).clicked() {
                        copy = Some(val.clone().unwrap_or_default());
                        ui.close();
                    }
                    if ui.button(t("Копировать строку", "Copy row")).clicked() {
                        copy = Some(rows[ri].iter().map(tsv_cell).collect::<Vec<_>>().join("\t"));
                        ui.close();
                    }
                    if ui.button(t("Копировать строку как JSON", "Copy row as JSON")).clicked() {
                        let m: serde_json::Map<String, serde_json::Value> = cols.iter().zip(rows[ri].iter()).map(|(c, v)| (c.name.clone(), v.clone().map_or(serde_json::Value::Null, serde_json::Value::String))).collect();
                        copy = Some(serde_json::to_string_pretty(&m).unwrap_or_default());
                        ui.close();
                    }
                    if let Some(v) = &val {
                        if ui.button(t("Фильтр по значению", "Filter by value")).clicked() {
                            filter_to = Some(v.clone());
                            ui.close();
                        }
                    }
                });
            }
        });
    });
    if let Some(c) = sort_click {
        v.sort = match v.sort {
            Some((sc, true)) if sc == c => Some((c, false)),
            Some((sc, false)) if sc == c => None,
            _ => Some((c, true)),
        };
        reorder(v);
    }
    if let Some(f) = filter_to {
        v.filter = f;
        reorder(v);
    }
    v.sel = sel;
    if let Some(s) = copy {
        ctx.copy_text(s);
    }
    // Ctrl+C on a chosen cell (when no text field has focus).
    if let Some((r, c)) = v.sel {
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            ctx.copy_text(v.table.rows[r][c].clone().unwrap_or_default());
        }
    }
}
