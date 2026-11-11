//! Results under the editor: a tab per statement that gave rows, and the
//! messages (what ran, how long, errors, NOTICEs / DBMS_OUTPUT). Tables
//! only lay out the rows on screen, sort by a header click, filter by text,
//! copy a cell / row / all, save as CSV.

use egui::{Align, Color32, Layout, RichText};
use egui_extras::{Column, TableBuilder};

use crate::app::{App, LogKind, Pane, ResultView};
use crate::i18n::t;

fn tsv_cell(v: &Option<String>) -> String {
    v.as_deref().unwrap_or("").replace(['\t', '\n'], " ")
}

pub fn as_tsv(v: &ResultView) -> String {
    let mut s = v.cols.iter().map(|&c| v.table.cols[c].name.clone()).collect::<Vec<_>>().join("\t");
    for &r in &v.order {
        s.push('\n');
        s += &v.cols.iter().map(|&c| tsv_cell(&v.table.rows[r][c])).collect::<Vec<_>>().join("\t");
    }
    s
}

/// Two values of a column, NULLs last.
fn cmp_vals(x: Option<&str>, y: Option<&str>, numeric: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match (x, y) {
        (None, None) => Equal,
        (None, _) => Greater,
        (_, None) => Less,
        (Some(x), Some(y)) if numeric => x.parse::<f64>().unwrap_or(f64::NAN).partial_cmp(&y.parse::<f64>().unwrap_or(f64::NAN)).unwrap_or(x.cmp(y)),
        (Some(x), Some(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
    }
}

/// Rows shown: filtered by text, sorted by the columns asked, in turn.
pub fn reorder(v: &mut ResultView) {
    let f = v.filter.to_lowercase();
    let rows = &v.table.rows;
    let mut order: Vec<usize> = (0..rows.len()).filter(|&r| f.is_empty() || rows[r].iter().any(|c| c.as_deref().is_some_and(|s| s.to_lowercase().contains(&f)))).collect();
    if !v.sort.is_empty() {
        let keys: Vec<(usize, bool, bool)> = v.sort.iter().map(|&(c, asc)| (c, asc, v.table.cols.get(c).is_some_and(|c| c.numeric))).collect();
        order.sort_by(|&a, &b| {
            for &(c, asc, numeric) in &keys {
                let (x, y) = (rows[a].get(c).and_then(|v| v.as_deref()), rows[b].get(c).and_then(|v| v.as_deref()));
                // NULLs last, either way.
                let o = match (x, y) {
                    (Some(_), Some(_)) if !asc => cmp_vals(x, y, numeric).reverse(),
                    _ => cmp_vals(x, y, numeric),
                };
                if o.is_ne() {
                    return o;
                }
            }
            a.cmp(&b)
        });
    }
    v.order = order;
    v.totals_stale = true;
}

/// A column's total, worked out when asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agg {
    Count,
    Sum,
    Min,
    Max,
    Avg,
}

impl Agg {
    pub const ALL: [Agg; 5] = [Agg::Sum, Agg::Min, Agg::Max, Agg::Avg, Agg::Count];

    pub fn name(self) -> &'static str {
        match self {
            Agg::Count => t("Количество", "Count"),
            Agg::Sum => t("Сумма", "Sum"),
            Agg::Min => t("Минимум", "Min"),
            Agg::Max => t("Максимум", "Max"),
            Agg::Avg => t("Среднее", "Average"),
        }
    }

    /// Short, for the totals row.
    pub fn mark(self) -> &'static str {
        match self {
            Agg::Count => "n",
            Agg::Sum => "Σ",
            Agg::Min => "min",
            Agg::Max => "max",
            Agg::Avg => "avg",
        }
    }
}

/// One total of a column over the rows shown (numbers as numbers; text
/// gets min / max / count; NULLs don't count).
pub fn total(v: &ResultView, c: usize, agg: Agg) -> String {
    let vals: Vec<&str> = v.order.iter().filter_map(|&r| v.table.rows.get(r).and_then(|row| row.get(c)).and_then(|x| x.as_deref())).collect();
    let nums: Vec<f64> = vals.iter().filter_map(|s| s.trim().parse::<f64>().ok()).collect();
    let numeric = !nums.is_empty() && nums.len() == vals.len();
    let fmt = |x: f64| crate::db::fmt_float((x * 1e9).round() / 1e9);
    match agg {
        Agg::Count => vals.len().to_string(),
        _ if vals.is_empty() => "NULL".into(),
        Agg::Sum if numeric => fmt(nums.iter().sum()),
        Agg::Avg if numeric => fmt(nums.iter().sum::<f64>() / nums.len() as f64),
        Agg::Min if numeric => fmt(nums.iter().cloned().fold(f64::INFINITY, f64::min)),
        Agg::Max if numeric => fmt(nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max)),
        Agg::Min => vals.iter().min().unwrap().to_string(),
        Agg::Max => vals.iter().max().unwrap().to_string(),
        Agg::Sum | Agg::Avg => t("не числа", "not numbers").into(),
    }
}

/// Where results are saved by default.
fn save_dir(app: &App) -> std::path::PathBuf {
    app.session.project.clone().or_else(dirs::download_dir).or_else(dirs::home_dir).unwrap_or_default()
}

fn log_lines(ui: &mut egui::Ui, lines: &[crate::app::LogLine], goto: &mut Option<usize>, mono: bool) {
    for l in lines {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&l.when).small().weak().monospace());
            let (icon, color) = match l.kind {
                LogKind::Run => ("▶", ui.visuals().weak_text_color()),
                LogKind::Ok => ("✔", Color32::from_rgb(80, 180, 90)),
                LogKind::Note => ("ℹ", ui.visuals().text_color()),
                LogKind::Error => ("⛔", Color32::from_rgb(230, 80, 80)),
            };
            let text = if mono { RichText::new(&l.text).monospace() } else { RichText::new(format!("{icon} {}", l.text)).color(color) };
            let text = if l.kind == LogKind::Run { text.monospace() } else { text };
            let r = ui.add(egui::Label::new(text).sense(if l.at.is_some() { egui::Sense::click() } else { egui::Sense::hover() }));
            if let Some(at) = l.at {
                if r.on_hover_text(t("Щелчок — к месту в скрипте", "Click: go there in the script")).clicked() {
                    *goto = Some(at);
                }
            }
        });
    }
}

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    let tab_id = app.tab().id.clone();
    panel_for(app, ui, &tab_id);
}

/// The results of a tab, or of a notebook's cell (`key`).
pub fn panel_for(app: &mut App, ui: &mut egui::Ui, key: &str) {
    let tab_id = key.to_string();
    let mut rt = app.rt.remove(&tab_id).unwrap_or_default();
    if let Pane::Result(i) = rt.shown {
        if i >= rt.results.len() {
            rt.shown = Pane::Messages;
        }
    }
    // What the buttons ask (done once `rt` is back in the app).
    let mut more: Option<(usize, bool)> = None;
    let mut stop: Option<usize> = None;
    let mut export: Option<usize> = None;
    ui.horizontal(|ui| {
        egui::ScrollArea::horizontal().id_salt("res-tabs").max_width((ui.available_width() - 120.0).max(80.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, r) in rt.results.iter().enumerate() {
                    let label = format!("▦ {} ({}{})", r.title, r.table.rows.len(), if r.table.more { "+" } else { "" });
                    if ui.selectable_label(rt.shown == Pane::Result(i), label).on_hover_text(crate::app::head(&r.sql, 400)).clicked() {
                        rt.shown = Pane::Result(i);
                    }
                }
                let errs = rt.log.iter().filter(|l| l.kind == LogKind::Error).count();
                let label = if errs > 0 { RichText::new(format!("✉ {} ⛔{errs}", t("Сообщения", "Messages"))).color(Color32::from_rgb(230, 90, 90)) } else { RichText::new(format!("✉ {}", t("Сообщения", "Messages"))) };
                if ui.selectable_label(rt.shown == Pane::Messages, label).clicked() {
                    rt.shown = Pane::Messages;
                }
                let n = rt.output.len();
                let label = if n > 0 { format!("🖹 {} ({n})", t("Вывод", "Output")) } else { format!("🖹 {}", t("Вывод", "Output")) };
                if ui.selectable_label(rt.shown == Pane::Output, label).on_hover_text(t("Что напечатал сервер: DBMS_OUTPUT, RAISE NOTICE…", "What the server printed: DBMS_OUTPUT, RAISE NOTICE…")).clicked() {
                    rt.shown = Pane::Output;
                }
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match rt.shown {
            Pane::Messages if !rt.log.is_empty() => {
                if ui.button(t("Очистить", "Clear")).clicked() {
                    rt.log.clear();
                }
            }
            Pane::Output if !rt.output.is_empty() => {
                if ui.button(t("Очистить", "Clear")).clicked() {
                    rt.output.clear();
                }
                if ui.button("🗐").on_hover_text(t("Копировать весь вывод", "Copy all the output")).clicked() {
                    ui.ctx().copy_text(rt.output.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"));
                }
            }
            _ => {}
        });
    });
    // A result's own bar: loading, export, copy, filter.
    if let Pane::Result(i) = rt.shown {
        let v = &mut rt.results[i];
        ui.horizontal(|ui| {
            let open = v.cursor.is_some();
            let idle = open && v.loading.is_none();
            let closed_tip = t("Курсор закрыт — чтобы получить остальные строки, выполните запрос снова", "The cursor's closed: run the statement again for the rest");
            let batch = app.batch(v.cursor.as_ref().map_or("", |c| c.1.as_str()));
            let r = ui.add_enabled(idle, crate::ui::tinted(format!("⏬ {} {batch}", t("Ещё", "More")), crate::ui::GREEN)).on_hover_text(t("Догрузить следующую порцию строк", "Load the next batch of rows"));
            let r = if v.table.more && !open { r.on_disabled_hover_text(closed_tip) } else { r };
            if r.clicked() {
                more = Some((i, false));
            }
            if ui.add_enabled(idle, crate::ui::tinted(format!("⏬ {}", t("Все", "All")), crate::ui::TEAL)).on_hover_text(t("Догрузить все оставшиеся строки", "Load every row left")).clicked() {
                more = Some((i, true));
            }
            if ui.add_enabled(open, crate::ui::tinted(format!("■ {}", t("Стоп", "Stop")), crate::ui::RED)).on_hover_text(t("Остановить загрузку и закрыть курсор (загруженное остаётся)", "Stop loading and close the cursor (what's loaded stays)")).clicked() {
                stop = Some(i);
            }
            ui.separator();
            if ui.add(crate::ui::tinted(format!("💾 {}", t("Экспорт…", "Export…")), crate::ui::BLUE)).on_hover_text(t("Сохранить в файл: CSV, TSV, JSON, INSERT, Markdown, HTML, XLSX — целиком или частями", "Save to a file: CSV, TSV, JSON, INSERT, Markdown, HTML, XLSX — whole or in parts")).clicked() {
                export = Some(i);
            }
            if ui.button("🗐").on_hover_text(t("Копировать всё (TSV, для таблиц)", "Copy all (TSV, for spreadsheets)")).clicked() {
                ui.ctx().copy_text(as_tsv(v));
            }
            ui.separator();
            let fr = ui.add(egui::TextEdit::singleline(&mut v.filter).hint_text(t("фильтр…", "filter…")).desired_width(140.0));
            if fr.changed() {
                reorder(v);
            }
            if v.loading.is_some() {
                ui.add(egui::Spinner::new().size(14.0));
            }
            let state = if open {
                format!(" · {}", t("есть ещё", "more on the server"))
            } else if v.table.more {
                format!(" · {}", t("загружены не все", "not all loaded"))
            } else {
                String::new()
            };
            ui.label(RichText::new(format!("{} / {} {}{state} · {}", v.order.len(), v.table.rows.len(), t("строк", "rows"), crate::app::fmt_took(v.took))).small().weak());
        });
    }
    ui.separator();
    let mut goto = None;
    match rt.shown {
        Pane::Result(i) => {
            if table(ui, &tab_id, i, &mut rt.results[i]) {
                let v = &rt.results[i];
                rt.col_orders.insert(v.shape(), v.cols.clone());
            }
        }
        Pane::Messages => {
            egui::ScrollArea::vertical().id_salt(("log", &tab_id)).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                if rt.log.is_empty() {
                    ui.label(RichText::new(t("Ctrl+Enter — выполнить запрос под курсором, Ctrl+Shift+Enter — весь скрипт", "Ctrl+Enter: run the statement at the cursor, Ctrl+Shift+Enter: the whole script")).weak());
                }
                log_lines(ui, &rt.log, &mut goto, false);
            });
        }
        Pane::Output => {
            egui::ScrollArea::vertical().id_salt(("output", &tab_id)).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                if rt.output.is_empty() {
                    ui.label(RichText::new(t("Здесь появится то, что печатает сервер: DBMS_OUTPUT.PUT_LINE, RAISE NOTICE…", "What the server prints shows here: DBMS_OUTPUT.PUT_LINE, RAISE NOTICE…")).weak());
                }
                log_lines(ui, &rt.output, &mut goto, true);
            });
        }
    }
    if let Some(g) = goto {
        rt.ed.goto = Some(g);
    }
    if let Some(i) = export {
        let v = &rt.results[i];
        let format = crate::export::Format::Csv;
        let name = format!("result-{}.{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), format.ext());
        app.export = Some((tab_id.clone(), i, crate::export::Export { format, path: save_dir(app).join(name), header: true, sep: ',', split: 0, table: crate::export::table_of(&v.sql), load_all: v.cursor.is_some() }));
    }
    app.rt.insert(tab_id.clone(), rt);
    if let Some((i, all)) = more {
        app.fetch_more(&tab_id, i, all);
    }
    if let Some(i) = stop {
        app.fetch_stop(&tab_id, i);
    }
    export_window(app, ui.ctx());
}

/// The save-a-result window.
fn export_window(app: &mut App, ctx: &egui::Context) {
    use crate::export::Format;
    let Some((tab, i, mut e)) = app.export.take() else { return };
    let Some(v) = app.rt.get(&tab).and_then(|r| r.results.get(i)) else { return };
    let (open, rows) = (v.cursor.is_some(), v.order.len());
    let mut keep = true;
    let mut go = false;
    egui::Window::new(t("Экспорт результата", "Export the result")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        egui::Grid::new("export-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(t("Формат", "Format"));
            let was = e.format;
            egui::ComboBox::from_id_salt("export-format").width(crate::dialogs::FIELD_W).selected_text(e.format.name()).show_ui(ui, |ui| {
                for f in Format::ALL {
                    ui.selectable_value(&mut e.format, f, f.name());
                }
            });
            if e.format != was {
                e.path.set_extension(e.format.ext());
            }
            ui.end_row();
            ui.label(t("Файл", "File"));
            let mut p = e.path.display().to_string();
            if ui.add(egui::TextEdit::singleline(&mut p).desired_width(crate::dialogs::FIELD_W)).changed() {
                e.path = p.into();
            }
            ui.end_row();
            if matches!(e.format, Format::Csv | Format::Tsv | Format::Xlsx) {
                ui.label(t("Заголовок", "Header"));
                ui.checkbox(&mut e.header, t("первой строкой — имена колонок", "column names as the first row"));
                ui.end_row();
            }
            if e.format == Format::Csv {
                ui.label(t("Разделитель", "Separator"));
                ui.horizontal(|ui| {
                    for (c, name) in [(',', ","), (';', ";"), ('|', "|"), ('\t', "Tab")] {
                        ui.selectable_value(&mut e.sep, c, name);
                    }
                });
                ui.end_row();
            }
            if e.format == Format::Insert {
                ui.label(t("Таблица", "Table"));
                ui.add(egui::TextEdit::singleline(&mut e.table).desired_width(crate::dialogs::FIELD_W));
                ui.end_row();
            }
            ui.label(t("Разбить", "Split"));
            ui.horizontal(|ui| {
                let mut on = e.split > 0;
                if ui.checkbox(&mut on, t("по строк в файле:", "rows per file:")).changed() {
                    e.split = if on { 100_000 } else { 0 };
                }
                if on {
                    ui.add(egui::DragValue::new(&mut e.split).range(1..=100_000_000).speed(100.0));
                }
            });
            ui.end_row();
            if open {
                ui.label(t("Строки", "Rows"));
                ui.checkbox(&mut e.load_all, t("сначала догрузить все с сервера", "load all from the server first"));
                ui.end_row();
            }
        });
        let total = if open && e.load_all { t("все строки", "every row").to_string() } else { format!("{rows} {}", t("строк", "rows")) };
        let files = if e.split > 0 && !(open && e.load_all) { format!(" · {} {}", crate::export::paths(&e, rows).len(), t("файл(ов)", "file(s)")) } else { String::new() };
        ui.label(RichText::new(format!("{} {total}{files}", t("Будет сохранено:", "To save:"))).weak().small());
        ui.separator();
        ui.horizontal(|ui| {
            if ui.add(crate::ui::tinted(format!("💾 {}", t("Сохранить", "Save")), crate::ui::GREEN)).clicked() {
                go = true;
            }
            if ui.button(t("Отмена", "Cancel")).clicked() {
                keep = false;
            }
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        keep = false;
    }
    if go {
        if open && e.load_all {
            if let Some(v) = app.rt.get_mut(&tab).and_then(|r| r.results.get_mut(i)) {
                v.export_after = Some(e);
            }
            app.fetch_more(&tab, i, true);
            app.toast(t("Догружаю строки — файл сохранится, когда загрузятся все", "Loading the rows: the file is saved once they're all in"));
        } else {
            let msg = crate::export::save(&e, &crate::export::Snapshot::of(&app.rt[&tab].results[i]));
            app.toast(msg);
        }
    } else if keep {
        app.export = Some((tab, i, e));
    }
}

/// The table of a result. True when its columns were moved (the order is
/// kept for the next runs with the same columns).
fn table(ui: &mut egui::Ui, tab: &str, i: usize, v: &mut ResultView) -> bool {
    let ctx = ui.ctx().clone();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    if v.table.cols.is_empty() {
        ui.label(t("Нет колонок", "No columns"));
        return false;
    }
    if v.cols.len() != v.table.cols.len() {
        v.cols = (0..v.table.cols.len()).collect();
    }
    // Totals asked for, worked out again only when the rows changed.
    if v.totals_stale {
        v.totals_stale = false;
        let asked: Vec<(usize, Agg)> = v.totals.iter().map(|(&c, (a, _))| (c, *a)).collect();
        for (c, a) in asked {
            let val = total(v, c, a);
            v.totals.insert(c, (a, val));
        }
    }
    // The chosen cell, whole.
    if let Some((r, c)) = v.sel {
        if let Some(val) = v.table.rows.get(r).and_then(|row| row.get(c)) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}:", v.table.cols[c].name)).small().strong());
                let s = val.clone().unwrap_or_else(|| "NULL".into());
                let one = crate::app::head(&s, 240);
                ui.label(RichText::new(one).small().monospace());
                if ui.button("🗐").on_hover_text(t("Копировать значение", "Copy value")).clicked() {
                    ctx.copy_text(s);
                }
            });
        }
    }
    let widths: Vec<f32> = v
        .cols
        .iter()
        .map(|&c| {
            let col = &v.table.cols[c];
            let longest = v.table.rows.iter().take(100).map(|r| r.get(c).and_then(|x| x.as_deref()).map_or(4, |s| s.chars().count().min(60))).max().unwrap_or(4).max(col.name.chars().count() + 4);
            (longest as f32 * char_w + 16.0).clamp(48.0, 420.0)
        })
        .collect();
    let num_w = (v.table.rows.len().to_string().len() as f32 * char_w + 14.0).max(30.0);
    // What the clicks ask, done after the table's drawn.
    let mut sort_click: Option<(usize, bool)> = None;
    let mut pick: Option<(usize, egui::Modifiers)> = None;
    let mut sort_set: Option<Vec<(usize, bool)>> = None;
    let mut agg_set: Option<(Vec<usize>, Agg)> = None;
    let mut move_to: Option<(Vec<usize>, Place)> = None;
    let mut sel = v.sel;
    let mut copy: Option<String> = None;
    let mut filter_to: Option<String> = None;
    let sel_bg = ui.visuals().selection.stroke.color;
    let picked_bg = ui.visuals().selection.bg_fill.gamma_multiply(0.35);
    let null_color = ui.visuals().weak_text_color();
    let footer_h = if v.totals.is_empty() { 0.0 } else { 24.0 };
    let mut heads: Vec<egui::Rect> = vec![egui::Rect::NOTHING; v.cols.len()];
    let mut dragging: Option<usize> = None;
    let view = v.cols.clone();
    let cols = v.table.cols.clone();
    let order = v.order.clone();
    let sort = v.sort.clone();
    let picked = v.picked.clone();
    let rows = &v.table.rows;
    let size = egui::vec2(ui.available_width(), (ui.available_height() - footer_h).max(40.0));
    let table_rect = ui
        .allocate_ui_with_layout(size, Layout::top_down(Align::Min), |ui| {
            // (Its id follows the columns' order: widths go with them.)
            let mut tb = TableBuilder::new(ui).id_salt(("res", tab, i, view.clone())).striped(true).resizable(true).auto_shrink([false, false]).min_scrolled_height(0.0).cell_layout(Layout::left_to_right(Align::Center)).column(Column::exact(num_w));
            for w in &widths {
                tb = tb.column(Column::initial(*w).at_least(30.0).clip(true));
            }
            tb.header(22.0, |mut h| {
                h.col(|ui| {
                    ui.label(RichText::new("#").weak());
                });
                for (k, &c) in view.iter().enumerate() {
                    let col = &cols[c];
                    let (rect, _) = h.col(|ui| {
                        if picked.contains(&c) {
                            ui.painter().rect_filled(ui.max_rect().expand2(egui::vec2(4.0, 2.0)), 0.0, picked_bg);
                        }
                        // The sort's arrow, with its level when there are several.
                        let level = sort.iter().position(|s| s.0 == c);
                        let arrow = match level.map(|l| (l, sort[l].1)) {
                            Some((l, asc)) if sort.len() > 1 => format!("{}{}", if asc { "▲" } else { "▼" }, l + 1),
                            Some((_, asc)) => (if asc { "▲" } else { "▼" }).to_string(),
                            None => "△".to_string(),
                        };
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let color = if level.is_some() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
                            let a = ui.add(egui::Label::new(RichText::new(arrow).small().color(color)).sense(egui::Sense::click()).selectable(false));
                            if a.clicked() {
                                sort_click = Some((c, ui.input(|i| i.modifiers.shift)));
                            }
                            a.on_hover_text(t("Сортировка: щелчок — по этой колонке, Shift+щелчок — добавить уровнем ниже", "Sort: click — by this column, Shift+click — add as the next level"));
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                let r = ui.add(egui::Label::new(RichText::new(&col.name).strong()).sense(egui::Sense::click_and_drag()).truncate().selectable(false));
                                let tip = t("Щелчок — выделить (Ctrl, Shift — несколько), перетащить — переставить", "Click: pick (Ctrl, Shift: several), drag: move");
                                let r = if col.ty.is_empty() { r.on_hover_text(tip) } else { r.on_hover_text(format!("{}\n{tip}", col.ty)) };
                                if r.clicked() {
                                    pick = Some((c, ui.input(|i| i.modifiers)));
                                }
                                if r.dragged() {
                                    dragging = Some(k);
                                }
                                if r.drag_stopped() {
                                    if let Some(p) = ui.ctx().pointer_interact_pos() {
                                        move_to = Some((vec![c], Place::Drop(p.x)));
                                    }
                                }
                                r.context_menu(|ui| {
                                    // Every picked column, when this one's among them.
                                    let group: Vec<usize> = if picked.contains(&c) { view.iter().copied().filter(|x| picked.contains(x)).collect() } else { vec![c] };
                                    let many = group.len() > 1;
                                    if many {
                                        ui.label(RichText::new(format!("{} {}", group.len(), t("колонок выделено", "columns picked"))).weak().small());
                                    }
                                    if ui.button(t("Копировать имя", "Copy name")).clicked() {
                                        copy = Some(group.iter().map(|&x| cols[x].name.clone()).collect::<Vec<_>>().join("\t"));
                                        ui.close();
                                    }
                                    if ui.button(if many { t("Копировать колонки", "Copy columns") } else { t("Копировать колонку", "Copy column") }).clicked() {
                                        copy = Some(order.iter().map(|&r| group.iter().map(|&x| tsv_cell(&rows[r][x])).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n"));
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.button(format!("▲ {}", t("Сортировать по возрастанию", "Sort ascending"))).clicked() {
                                        sort_set = Some(group.iter().map(|&x| (x, true)).collect());
                                        ui.close();
                                    }
                                    if ui.button(format!("▼ {}", t("Сортировать по убыванию", "Sort descending"))).clicked() {
                                        sort_set = Some(group.iter().map(|&x| (x, false)).collect());
                                        ui.close();
                                    }
                                    if !sort.is_empty() {
                                        for (asc, label) in [(true, t("Добавить уровень сортировки ▲", "Add a sort level ▲")), (false, t("Добавить уровень сортировки ▼", "Add a sort level ▼"))] {
                                            if ui.button(format!("+ {label}")).clicked() {
                                                let mut s: Vec<(usize, bool)> = sort.iter().copied().filter(|x| !group.contains(&x.0)).collect();
                                                s.extend(group.iter().map(|&x| (x, asc)));
                                                sort_set = Some(s);
                                                ui.close();
                                            }
                                        }
                                        if ui.button(t("Сбросить сортировку", "Clear sorting")).clicked() {
                                            sort_set = Some(Vec::new());
                                            ui.close();
                                        }
                                    }
                                    ui.separator();
                                    ui.menu_button(format!("Σ {}", t("Итоги", "Totals")), |ui| {
                                        for a in Agg::ALL {
                                            if ui.button(a.name()).clicked() {
                                                agg_set = Some((group.clone(), a));
                                                ui.close();
                                            }
                                        }
                                    });
                                    ui.separator();
                                    let k0 = view.iter().position(|x| group.contains(x)).unwrap_or(0);
                                    let k1 = view.iter().rposition(|x| group.contains(x)).unwrap_or(0);
                                    if ui.add_enabled(k0 > 0, egui::Button::new(format!("⏴ {}", t("Переместить влево", "Move left")))).clicked() {
                                        move_to = Some((group.clone(), Place::Left));
                                        ui.close();
                                    }
                                    if ui.add_enabled(k1 + 1 < view.len(), egui::Button::new(format!("⏵ {}", t("Переместить вправо", "Move right")))).clicked() {
                                        move_to = Some((group.clone(), Place::Right));
                                        ui.close();
                                    }
                                    if ui.add_enabled(k0 > 0, egui::Button::new(format!("⏮ {}", t("В начало", "To the start")))).clicked() {
                                        move_to = Some((group.clone(), Place::At(0)));
                                        ui.close();
                                    }
                                    if ui.add_enabled(k1 + 1 < view.len(), egui::Button::new(format!("⏭ {}", t("В конец", "To the end")))).clicked() {
                                        move_to = Some((group.clone(), Place::At(usize::MAX)));
                                        ui.close();
                                    }
                                });
                            });
                        });
                    });
                    heads[k] = rect;
                }
            })
            .body(|body| {
                body.rows(20.0, order.len(), |mut row| {
                    let ri = order[row.index()];
                    row.col(|ui| {
                        ui.label(RichText::new((ri + 1).to_string()).weak().small());
                    });
                    for &c in &view {
                        let col = &cols[c];
                        let (rect, resp) = row.col(|ui| {
                            if picked.contains(&c) {
                                ui.painter().rect_filled(ui.max_rect().expand2(egui::vec2(4.0, 2.0)), 0.0, picked_bg);
                            }
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
                                copy = Some(view.iter().map(|&x| tsv_cell(&rows[ri][x])).collect::<Vec<_>>().join("\t"));
                                ui.close();
                            }
                            if ui.button(t("Копировать строку как JSON", "Copy row as JSON")).clicked() {
                                let m: serde_json::Map<String, serde_json::Value> = view.iter().map(|&x| (cols[x].name.clone(), rows[ri][x].clone().map_or(serde_json::Value::Null, serde_json::Value::String))).collect();
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
        })
        .response
        .rect;
    // A header being dragged: where it would land.
    if let (Some(k), Some(p)) = (dragging, ctx.pointer_interact_pos()) {
        let to = slot(&heads, p.x);
        if to != k && to != k + 1 {
            let x = heads.get(to).map_or_else(|| heads.last().map_or(p.x, |r| r.right()), |r| r.left());
            ui.painter().line_segment([egui::pos2(x, table_rect.top()), egui::pos2(x, table_rect.bottom())], egui::Stroke::new(2.0, sel_bg));
        }
    }
    // Totals: a row of their own under the table, under their columns.
    if !v.totals.is_empty() {
        let (strip, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), footer_h), egui::Sense::hover());
        let p = ui.painter_at(strip);
        p.rect_filled(strip, 4.0, ui.visuals().faint_bg_color);
        p.line_segment([strip.left_top(), strip.right_top()], ui.visuals().widgets.noninteractive.bg_stroke);
        let mut drop: Option<Option<usize>> = None;
        for (k, &c) in view.iter().enumerate() {
            let Some((a, val)) = v.totals.get(&c).cloned() else { continue };
            let head = heads[k];
            if !head.is_positive() {
                continue;
            }
            let cell = egui::Rect::from_min_max(egui::pos2(head.left(), strip.top()), egui::pos2(head.right(), strip.bottom()));
            let r = ui.interact(cell, ui.id().with(("total", tab, i, c)), egui::Sense::click());
            if r.hovered() {
                p.rect_filled(cell.shrink(1.0), 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let strong = ui.visuals().strong_text_color();
            let g = p.layout_no_wrap(format!("{} {val}", a.mark()), font.clone(), strong);
            let x = if cols[c].numeric { cell.right() - 6.0 - g.size().x } else { cell.left() + 6.0 };
            p.with_clip_rect(cell.intersect(strip)).galley(egui::pos2(x, cell.center().y - g.size().y / 2.0), g, strong);
            let r = r.on_hover_text(format!("{} · {}\n{}", a.name(), cols[c].name, t("Щелчок — копировать, правый — другой итог или убрать", "Click: copy; right click: another total or remove")));
            if r.clicked() {
                copy = Some(val.clone());
            }
            r.context_menu(|ui| {
                for b in Agg::ALL {
                    if ui.button(b.name()).clicked() {
                        agg_set = Some((vec![c], b));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button(t("Убрать итог", "Remove the total")).clicked() {
                    drop = Some(Some(c));
                    ui.close();
                }
                if ui.button(t("Убрать все итоги", "Remove all totals")).clicked() {
                    drop = Some(None);
                    ui.close();
                }
            });
        }
        match drop {
            Some(None) => v.totals.clear(),
            Some(Some(c)) => {
                v.totals.remove(&c);
            }
            None => {}
        }
    }
    if let Some((c, shift)) = sort_click {
        let at = v.sort.iter().position(|s| s.0 == c);
        if shift {
            // Another level; or this level turned around, then dropped.
            match at {
                Some(l) if v.sort[l].1 => v.sort[l].1 = false,
                Some(l) => {
                    v.sort.remove(l);
                }
                None => v.sort.push((c, true)),
            }
        } else {
            v.sort = match at.map(|l| v.sort[l].1) {
                Some(true) if v.sort.len() == 1 => vec![(c, false)],
                Some(false) if v.sort.len() == 1 => Vec::new(),
                _ => vec![(c, true)],
            };
        }
        reorder(v);
    }
    if let Some(s) = sort_set {
        v.sort = s;
        reorder(v);
    }
    if let Some((c, m)) = pick {
        pick_column(v, c, m);
    }
    if let Some((group, a)) = agg_set {
        for c in group {
            let val = total(v, c, a);
            v.totals.insert(c, (a, val));
        }
    }
    let mut moved = false;
    if let Some((mut group, place)) = move_to {
        let to = match place {
            Place::Drop(x) => {
                // The picked ones go along when the dragged one's among them.
                if v.picked.contains(&group[0]) {
                    group = v.cols.iter().copied().filter(|x| v.picked.contains(x)).collect();
                }
                slot(&heads, x)
            }
            Place::Left => v.cols.iter().position(|x| group.contains(x)).unwrap_or(0).saturating_sub(1),
            Place::Right => v.cols.iter().rposition(|x| group.contains(x)).map_or(0, |k| k + 2),
            Place::At(k) => k,
        };
        moved = move_cols(&mut v.cols, &group, to);
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
    moved
}

/// Where columns go.
enum Place {
    /// Dropped at this x.
    Drop(f32),
    Left,
    Right,
    /// Before the column shown at this place (past the end: last).
    At(usize),
}

/// The place (between headers) at x.
fn slot(heads: &[egui::Rect], x: f32) -> usize {
    heads.iter().position(|r| x < r.center().x).unwrap_or(heads.len())
}

/// Puts `group` (in its order as shown) before the column shown at `to`;
/// whether anything moved.
pub fn move_cols(cols: &mut Vec<usize>, group: &[usize], to: usize) -> bool {
    let before = cols.clone();
    let to = to.min(cols.len());
    let left = cols[..to].iter().filter(|x| group.contains(x)).count();
    let moving: Vec<usize> = cols.iter().copied().filter(|x| group.contains(x)).collect();
    cols.retain(|x| !group.contains(x));
    let at = (to - left).min(cols.len());
    for (n, c) in moving.into_iter().enumerate() {
        cols.insert(at + n, c);
    }
    *cols != before
}

/// A header's click: picks it alone; Ctrl adds / drops it; Shift picks the
/// run from the first picked to it.
pub fn pick_column(v: &mut ResultView, c: usize, m: egui::Modifiers) {
    if m.command || m.ctrl {
        if !v.picked.remove(&c) {
            v.picked.insert(c);
        }
    } else if m.shift && !v.picked.is_empty() {
        let pos = |x: usize| v.cols.iter().position(|&y| y == x).unwrap_or(0);
        let first = v.picked.iter().map(|&x| pos(x)).min().unwrap_or(0);
        let (a, b) = (first.min(pos(c)), first.max(pos(c)));
        v.picked = v.cols[a..=b].iter().copied().collect();
    } else if v.picked.len() == 1 && v.picked.contains(&c) {
        v.picked.clear();
    } else {
        v.picked = [c].into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_columns() {
        let mut c = vec![0, 1, 2, 3, 4];
        assert!(move_cols(&mut c, &[3], 0));
        assert_eq!(c, vec![3, 0, 1, 2, 4]);
        assert!(move_cols(&mut c, &[0, 1], 5));
        assert_eq!(c, vec![3, 2, 4, 0, 1]);
        // In front of where it stands: nothing moves.
        assert!(!move_cols(&mut c, &[2], 2));
        assert!(move_cols(&mut c, &[2, 0], 1));
        assert_eq!(c, vec![3, 2, 0, 4, 1]);
    }

    #[test]
    fn sorting_by_levels_and_totals() {
        let table = crate::db::Table {
            cols: vec![crate::db::Col { name: "k".into(), ..Default::default() }, crate::db::Col { name: "v".into(), numeric: true, ..Default::default() }],
            rows: vec![vec![Some("b".into()), Some("2".into())], vec![Some("a".into()), Some("10".into())], vec![Some("b".into()), Some("1".into())], vec![Some("a".into()), None]],
            more: false,
        };
        let mut v = ResultView::new("select".into(), table, Default::default(), None);
        v.sort = vec![(0, true), (1, false)];
        reorder(&mut v);
        assert_eq!(v.order, vec![1, 3, 0, 2]);
        assert_eq!(total(&v, 1, Agg::Sum), "13");
        assert_eq!(total(&v, 1, Agg::Avg), "4.333333333");
        assert_eq!(total(&v, 1, Agg::Count), "3");
        assert_eq!(total(&v, 0, Agg::Max), "b");
        assert_eq!(total(&v, 0, Agg::Sum), t("не числа", "not numbers"));
    }
}
