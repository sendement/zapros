//! Results under the editor: a tab per statement that gave rows, and the
//! messages (what ran, how long, errors, NOTICEs / DBMS_OUTPUT). Tables
//! only lay out the rows on screen, sort by a header click, filter by text,
//! copy a cell / row / all, save as CSV.

use egui::{Align, Color32, Layout, RichText};
use egui::emath::GuiRounding as _;
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
            if let Some(img) = crate::icons::image(crate::icons::Ic::Filter, 15.0, ui.visuals().weak_text_color()) {
                ui.add(img);
            }
            let fr = ui.add(egui::TextEdit::singleline(&mut v.filter).hint_text(t("фильтр…", "filter…")).desired_width(140.0));
            if fr.changed() {
                reorder(v);
            }
            if v.loading.is_some() {
                ui.add(egui::Spinner::new().size(14.0));
                if let Some(since) = v.loading_since {
                    crate::app::stopwatch_label(ui, since, true);
                }
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
    // A line under the bar, from where the table's rounding starts.
    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
    ui.painter().hline(egui::Rangef::new(line.left() + 6.0, line.right() - 6.0 - crate::ui::TABLE_BAR_ROOM), line.center().y, ui.visuals().widgets.noninteractive.bg_stroke);
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
    // The cells in the tables' font (the headers in the interface's).
    ui.scope(|ui| {
        if let Some(tf) = ui.style().text_styles.get(&egui::TextStyle::Name("table".into())).cloned() {
            ui.style_mut().text_styles.insert(egui::TextStyle::Monospace, tf);
        }
        table_in(ui, tab, i, v)
    })
    .inner
}

fn table_in(ui: &mut egui::Ui, tab: &str, i: usize, v: &mut ResultView) -> bool {
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
    // The numbers' column: narrow, in the small font.
    let small = egui::TextStyle::Small.resolve(ui.style());
    let small_w = ui.fonts_mut(|f| f.glyph_width(&small, '0'));
    let num_w = (v.table.rows.len().to_string().len() as f32 * small_w + 8.0).max(18.0);
    // What the clicks ask, done after the table's drawn.
    let mut sort_click: Option<(usize, bool)> = None;
    // From the opened sort square: column, ▲ / ▼ (None: drop it), Shift.
    let mut sort_pick: Option<(usize, Option<bool>, bool)> = None;
    let mut pick: Option<(usize, egui::Modifiers)> = None;
    let mut sort_set: Option<Vec<(usize, bool)>> = None;
    let mut agg_set: Option<(Vec<usize>, Agg)> = None;
    let mut move_to: Option<(Vec<usize>, Place)> = None;
    let mut sel = v.sel;
    let mut clear_picks = false;
    let mut copy: Option<String> = None;
    let mut filter_to: Option<String> = None;
    // Grey picks: a picked column only shades what's under it (the stripes
    // show through); a picked row one grey; the chosen cell a deeper one.
    let dark = ui.visuals().dark_mode;
    let sel_fill = if dark { egui::Color32::from_rgb(84, 84, 92) } else { egui::Color32::from_rgb(192, 192, 200) };
    let sel_bg = if dark { egui::Color32::from_rgb(135, 135, 145) } else { egui::Color32::from_rgb(140, 140, 150) };
    let row_fill = if dark { egui::Color32::from_rgb(58, 58, 64) } else { egui::Color32::from_rgb(214, 214, 220) };
    let null_bg = if dark { egui::Color32::from_rgba_unmultiplied(140, 120, 200, 20) } else { egui::Color32::from_rgba_unmultiplied(120, 100, 190, 20) };
    let picked_bg = if dark { egui::Color32::from_rgba_unmultiplied(255, 255, 255, 14) } else { egui::Color32::from_rgba_unmultiplied(0, 0, 0, 14) };
    // The chosen cell's row and its column's header: a shade lighter.
    let cross_bg = if dark { egui::Color32::from_rgba_unmultiplied(255, 255, 255, 22) } else { egui::Color32::from_rgba_unmultiplied(0, 0, 0, 20) };
    let head_bg = if dark { egui::Color32::from_rgb(50, 50, 54) } else { egui::Color32::from_rgb(226, 226, 230) };
    let head_sel = if dark { egui::Color32::from_rgb(74, 74, 82) } else { egui::Color32::from_rgb(204, 204, 212) };
    let edge = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let sel_row = sel.map(|s| s.0);
    let sel_col = sel.map(|s| s.1);
    // The numbers stay put at the left when the table scrolls sideways:
    // their cells are drawn (and clicked) over the table, after it.
    let mut pins: Vec<(usize, egui::Rect, egui::Rect)> = Vec::new();
    let mut hash_head = egui::Rect::NOTHING;
    // A cell's background reaches halfway into the gaps (as egui's stripes do).
    let half = ui.spacing().item_spacing * 0.5;
    let rows_picked = v.rows_picked.clone();
    let mut row_click: Option<(usize, egui::Modifiers)> = None;
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
    let mut size = egui::vec2(ui.available_width(), (ui.available_height() - footer_h).max(40.0));
    // The rows' scroll bar: the table's own one sits at its content's right
    // end (out of sight when it's wider than the room), so it's hidden and
    // one of ours goes at the right of the room, beside the table.
    let stride = 20.0 + ui.spacing().item_spacing.y;
    let content_h = order.len() as f32 * stride;
    let need_bar = content_h > size.y - 30.0;
    let bar_room = crate::ui::TABLE_BAR_ROOM;
    size.x -= bar_room;
    let mut body_scroll_id = egui::Id::NULL;
    let mut body_clip = egui::Rect::NOTHING;
    // The header's own colour goes under it (set once its height's known).
    let back = ui.painter().add(egui::Shape::Noop);
    let table_rect = ui
        .allocate_ui_with_layout(size, Layout::top_down(Align::Min), |ui| {
            // (Its id follows the columns' order: widths go with them.)
            // Wider than the room: it scrolls sideways (the header with it).
            // (No fade at the left edge: the pinned numbers are there.)
            ui.spacing_mut().scroll.fade.strength = 0.0;
            egui::ScrollArea::horizontal().id_salt(("res-h", tab, i)).auto_shrink([false, false]).show(ui, |ui| {
            // (A little room inside the rounded edge.)
            egui::Frame::NONE.inner_margin(egui::Margin { left: 4, right: 4, top: 0, bottom: 3 }).show(ui, |ui| {
            // (Rounding at fractional scales made it a fraction wider than
            // the room: a scroll bar for nothing.)
            ui.set_max_width((ui.available_width() - 0.5).floor());
            // A picked row's colour (egui fills it gapless).
            ui.visuals_mut().selection.bg_fill = row_fill;
            ui.visuals_mut().selection.stroke.color = ui.visuals().strong_text_color();
            let view_clip = ui.clip_rect();
            // (As egui_extras names its body's scroll area.)
            let state_id = ui.id().with(egui::IdSalt::new(("res", tab, i, view.clone())));
            body_scroll_id = ui.make_persistent_id(egui::IdSalt::new(state_id.with("__scroll_area")));
            let mut tb = TableBuilder::new(ui).id_salt(("res", tab, i, view.clone())).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).striped(true).resizable(true).auto_shrink([false, false]).min_scrolled_height(0.0).cell_layout(Layout::left_to_right(Align::Center)).column(Column::exact(num_w));
            for w in &widths {
                tb = tb.column(Column::initial(*w).at_least(30.0).clip(true));
            }
            tb.header(22.0, |mut h| {
                h.col(|ui| {
                    hash_head = ui.max_rect().expand2(half).round_ui();
                });
                for (k, &c) in view.iter().enumerate() {
                    let col = &cols[c];
                    let mut head_rect = egui::Rect::NOTHING;
                    h.col(|ui| {
                        if picked.contains(&c) || sel_col == Some(c) {
                            // Down to the header's line (as it's put below), not above the
                            // table's edge, not past what's in view.
                            let m = ui.max_rect();
                            let cell = egui::Rect::from_min_max(egui::pos2(m.left() - half.x, m.top()), egui::pos2(m.right() + half.x, m.bottom().max(m.top() + 24.0) + 2.0)).round_ui();
                            let mut p = ui.painter().clone();
                            // (What's in view: in a notebook scrolled past it, nothing —
                            // not the buttons above.)
                            let clip = cell.intersect(view_clip);
                            p.set_clip_rect(clip);
                            if !clip.is_positive() {
                                p.set_opacity(0.0);
                            }
                            if sel_col == Some(c) {
                                p.rect_filled(cell, 0.0, head_sel);
                            }
                            if picked.contains(&c) {
                                p.rect_filled(cell, 0.0, picked_bg);
                            }
                        }
                        // The whole header picks (Ctrl, Shift: several), drags, has
                        // the menu; the sort's square at the right opens under the
                        // mouse into ▲ ▼ (✕ when sorted by it).
                        let full = ui.max_rect();
                        head_rect = full;
                        let level = sort.iter().position(|s| s.0 == c);
                        let open_id = ui.id().with(("sort-open", c));
                        let open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
                        let opts: Vec<(&str, Option<bool>)> = if level.is_some() { vec![("▲", Some(true)), ("▼", Some(false)), ("✕", None)] } else { vec![("▲", Some(true)), ("▼", Some(false))] };
                        let side = 16.0;
                        let shut = match level {
                            Some(l) if sort.len() > 1 => format!("{}{}", if sort[l].1 { "▲" } else { "▼" }, l + 1),
                            Some(l) => (if sort[l].1 { "▲" } else { "▼" }).to_string(),
                            None => "△".to_string(),
                        };
                        let small_font = egui::TextStyle::Small.resolve(ui.style());
                        let shut_w = ui.fonts_mut(|f| f.layout_no_wrap(shut.clone(), small_font.clone(), egui::Color32::WHITE).size().x + 8.0).max(side);
                        let w = if open { opts.len() as f32 * side + 2.0 } else { shut_w };
                        let pill = egui::Rect::from_min_size(egui::pos2(full.right() - w - 2.0, full.center().y - side / 2.0), egui::vec2(w, side)).round_ui();
                        // The name, in what's left of it.
                        let name_rect = egui::Rect::from_min_max(full.min, egui::pos2(pill.left() - 2.0, full.max.y));
                        ui.scope_builder(egui::UiBuilder::new().max_rect(name_rect).layout(Layout::left_to_right(Align::Center)), |ui| {
                            ui.add(egui::Label::new(RichText::new(&col.name).strong()).truncate().selectable(false));
                        });
                        // (Over the name; the sort square goes over it in turn.)
                        let r = ui.interact(full, ui.id().with(("head", c)), egui::Sense::click_and_drag());
                        let p = ui.painter();
                        let strong = ui.visuals().strong_text_color();
                        let weak = ui.visuals().weak_text_color();
                        let edge_c = ui.visuals().widgets.noninteractive.bg_stroke.color;
                        let shift_held = ui.input(|i| i.modifiers.shift);
                        let tip_sort = t("Сортировка: ▲ / ▼ — по этой колонке, с Shift — добавить уровнем ниже, ✕ — убрать", "Sort: ▲ / ▼ by this column, with Shift: add as the next level, ✕: drop it");
                        if open {
                            p.rect(pill, 3.0, head_sel, egui::Stroke::new(1.0, edge_c), egui::StrokeKind::Inside);
                            for (n, (glyph, what)) in opts.iter().enumerate() {
                                let b = egui::Rect::from_min_size(egui::pos2(pill.left() + 1.0 + n as f32 * side, pill.top()), egui::vec2(side, side));
                                let br = ui.interact(b, ui.id().with(("sort-opt", c, n)), egui::Sense::click());
                                let on = matches!((level.map(|l| sort[l].1), what), (Some(x), Some(y)) if x == *y);
                                if br.hovered() {
                                    p.rect_filled(b.shrink(1.0), 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                                }
                                p.text(b.center(), egui::Align2::CENTER_CENTER, *glyph, small_font.clone(), if on || br.hovered() { strong } else { weak });
                                if br.clicked() {
                                    sort_pick = Some((c, *what, shift_held));
                                }
                                br.on_hover_text(tip_sort);
                            }
                            let still = ui.ctx().pointer_hover_pos().is_some_and(|q| pill.expand(3.0).contains(q));
                            if !still {
                                ui.data_mut(|d| d.insert_temp(open_id, false));
                            }
                        } else {
                            let br = ui.interact(pill, ui.id().with(("sort-shut", c)), egui::Sense::click());
                            p.rect(pill, 3.0, egui::Color32::TRANSPARENT, egui::Stroke::new(1.0, edge_c), egui::StrokeKind::Inside);
                            p.text(pill.center(), egui::Align2::CENTER_CENTER, shut, small_font.clone(), if level.is_some() { strong } else { weak });
                            if br.hovered() {
                                ui.data_mut(|d| d.insert_temp(open_id, true));
                                ui.ctx().request_repaint();
                            }
                            if br.clicked() {
                                sort_click = Some((c, shift_held));
                            }
                        }
                        {
                            {
                                let tip = t("Щелчок — выделить (Ctrl, Shift — несколько), перетащить — переставить, правый — меню", "Click: pick (Ctrl, Shift: several), drag: move, right click: menu");
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
                            }
                        }
                    });
                    heads[k] = head_rect;
                }
            })
            .body(|mut body| {
                body_clip = body.ui_mut().clip_rect();
                // Fills stay inside what's in view (not over the header or past
                // the right edge).
                let vp = body_clip.intersect(view_clip);
                body.rows(20.0, order.len(), |mut row| {
                    let ri = order[row.index()];
                    row.set_selected(rows_picked.contains(&ri));
                    row.col(|ui| {
                        pins.push((ri, ui.max_rect().expand2(half).round_ui(), ui.clip_rect()));
                    });
                    for &c in &view {
                        let col = &cols[c];
                        let mut hit = None;
                        row.col(|ui| {
                            // Gapless, like the stripes: halfway into the gaps.
                            let cell = ui.max_rect().expand2(half).round_ui();
                            let mut p = ui.painter().clone();
                            let clip = cell.intersect(vp);
                            p.set_clip_rect(clip);
                            if !clip.is_positive() {
                                p.set_opacity(0.0);
                            }
                            let val = rows[ri].get(c).and_then(|x| x.as_ref());
                            // NULL on a shade of its own; the chosen cell filled.
                            if val.is_none() {
                                p.rect_filled(cell, 0.0, null_bg);
                            }
                            if picked.contains(&c) {
                                p.rect_filled(cell, 0.0, picked_bg);
                            }
                            if sel_row == Some(ri) && !rows_picked.contains(&ri) {
                                p.rect_filled(cell, 0.0, cross_bg);
                            }
                            if sel == Some((ri, c)) {
                                p.rect(cell, 0.0, sel_fill, egui::Stroke::new(1.0, sel_bg), egui::StrokeKind::Inside);
                            }
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
                            // The whole cell clicks (over its label). (The cell's own
                            // response won't: egui sets its Ui back to hover-only.)
                            hit = Some(ui.interact(ui.max_rect(), ui.id().with("cell"), egui::Sense::click()));
                        });
                        let Some(resp) = hit else { continue };
                        if resp.hovered() {
                            crate::ui::keep_arrow(&resp.ctx);
                        }
                        if resp.clicked() {
                            sel = Some((ri, c));
                            clear_picks = true;
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
            // The numbers, pinned at the left edge of what's in view.
            let shift = |r: egui::Rect| r.translate(egui::vec2((view_clip.left() - r.left()).max(0.0), 0.0));
            if hash_head.is_positive() {
                let r = shift(hash_head);
                let mut clip = view_clip.intersect(r);
                clip.min.y = clip.min.y.max(hash_head.top() + half.y);
                let mut p = ui.painter().with_clip_rect(clip);
                if !clip.is_positive() {
                    p.set_opacity(0.0);
                }
                p.rect_filled(r, 0.0, head_bg);
                p.text(r.center(), egui::Align2::CENTER_CENTER, "#", small.clone(), ui.visuals().weak_text_color());
                p.vline(r.right() - 0.5, r.y_range(), egui::Stroke::new(1.0, edge));
            }
            for (ri, cell, _) in pins {
                let r = shift(cell);
                // (Only the body's top and bottom: sideways it's the view.)
                let rows_clip = egui::Rect::from_x_y_ranges(view_clip.x_range(), body_clip.y_range());
                let shown = r.intersect(rows_clip).intersect(view_clip);
                if !shown.is_positive() {
                    continue;
                }
                let p = ui.painter().with_clip_rect(shown);
                let picked_row = rows_picked.contains(&ri);
                let fill = if picked_row { sel_fill } else if sel_row == Some(ri) { head_sel } else { head_bg };
                p.rect_filled(r, 0.0, fill);
                let color = if picked_row || sel_row == Some(ri) { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
                p.text(egui::pos2(r.right() - 4.0, r.center().y), egui::Align2::RIGHT_CENTER, (ri + 1).to_string(), small.clone(), color);
                p.vline(r.right() - 0.5, r.y_range(), egui::Stroke::new(1.0, edge));
                let num = ui.interact(shown, ui.id().with(("row-num", tab, i, ri)), egui::Sense::click());
                if num.hovered() && !picked_row {
                    p.rect_filled(r, 0.0, picked_bg);
                }
                let num = num.on_hover_text(t("Щелчок — выделить строку (Ctrl, Shift — несколько)", "Click: pick the row (Ctrl, Shift: several)"));
                if num.clicked() {
                    row_click = Some((ri, num.ctx.input(|i| i.modifiers)));
                }
                num.context_menu(|ui| {
                    let these: Vec<usize> = if rows_picked.contains(&ri) { order.iter().copied().filter(|r| rows_picked.contains(r)).collect() } else { vec![ri] };
                    if ui.button(if these.len() > 1 { t("Копировать строки", "Copy rows") } else { t("Копировать строку", "Copy row") }).clicked() {
                        copy = Some(these.iter().map(|&r| view.iter().map(|&x| tsv_cell(&rows[r][x])).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n"));
                        ui.close();
                    }
                    if ui.button(t("Копировать как JSON", "Copy as JSON")).clicked() {
                        let arr: Vec<serde_json::Value> = these.iter().map(|&r| serde_json::Value::Object(view.iter().map(|&x| (cols[x].name.clone(), rows[r][x].clone().map_or(serde_json::Value::Null, serde_json::Value::String))).collect())).collect();
                        copy = Some(serde_json::to_string_pretty(&arr).unwrap_or_default());
                        ui.close();
                    }
                });
            }
            });
            });
        })
        .response
        .rect;
    // The header on a colour of its own; the table's corners rounded (the
    // rows' stripes cut by the panel's colour), a thin edge round it.
    {
        let bottom = heads.iter().filter(|r| r.is_positive()).map(|r| r.bottom()).fold(table_rect.top() + 24.0, f32::max);
        let head = egui::Rect::from_min_max(table_rect.min, egui::pos2(table_rect.right(), bottom + 2.0));
        let r = 6u8;
        ui.painter().set(back, egui::Shape::rect_filled(head, egui::CornerRadius { nw: r, ne: r, sw: 0, se: 0 }, head_bg));
        let p = ui.painter();
        corners(p, table_rect, r as f32, ui.visuals().panel_fill);
        let edge = ui.visuals().widgets.noninteractive.bg_stroke.color;
        for _ in 0..2 {
            p.rect_stroke(table_rect.shrink(0.5), r, egui::Stroke::new(1.0, edge), egui::StrokeKind::Middle);
        }
        p.hline(head.x_range().shrink(0.5), head.bottom(), egui::Stroke::new(1.0, edge));
    }
    if need_bar && hash_head.is_positive() && body_clip.height() > 0.0 {
        let view_h = body_clip.height().max(1.0);
        let track = egui::Rect::from_min_max(egui::pos2(table_rect.right() + 3.0, hash_head.bottom() + 2.0), egui::pos2(table_rect.right() + bar_room - 1.0, table_rect.bottom() - 2.0));
        if let Some(mut st) = egui::scroll_area::State::load(&ctx, body_scroll_id) {
            let max = (content_h - view_h).max(1.0);
            let handle_h = (track.height() * view_h / content_h).clamp(20.0_f32.min(track.height()), track.height());
            let room = (track.height() - handle_h).max(1.0);
            let at = |o: f32| egui::Rect::from_min_size(egui::pos2(track.left(), track.top() + (o / max).clamp(0.0, 1.0) * room), egui::vec2(track.width(), handle_h));
            let id = ui.id().with(("res-vbar", tab, i));
            let r = ui.interact(track, id, egui::Sense::click_and_drag());
            let grab_id = id.with("grab");
            if let Some(p) = r.interact_pointer_pos() {
                if r.drag_started() || r.clicked() || r.is_pointer_button_down_on() && ui.data(|d| d.get_temp::<f32>(grab_id)).is_none() {
                    let h = at(st.offset.y);
                    let grab = if h.contains(p) { p.y - h.top() } else { handle_h / 2.0 };
                    ui.data_mut(|d| d.insert_temp(grab_id, grab));
                }
                let grab = ui.data(|d| d.get_temp::<f32>(grab_id)).unwrap_or(handle_h / 2.0);
                let f = (p.y - grab - track.top()) / room;
                // (At the very end: past it, the scroll area stops at its true end.)
                st.offset.y = if f >= 1.0 { content_h } else { (f * max).clamp(0.0, max) };
                st.store(&ctx, body_scroll_id);
                ctx.request_repaint();
            } else {
                ui.data_mut(|d| d.remove_temp::<f32>(grab_id));
            }
            // (As egui's own bars are styled.)
            let sc = ui.spacing().scroll;
            let over = ui.ctx().pointer_hover_pos().is_some_and(|q| table_rect.union(track).contains(q));
            let (bg, handle) = if r.dragged() || r.hovered() {
                (sc.interact_background_opacity, sc.interact_handle_opacity)
            } else if over {
                (sc.active_background_opacity, sc.active_handle_opacity)
            } else {
                (sc.dormant_background_opacity, sc.dormant_handle_opacity)
            };
            // Thin at rest, its full width under the pointer (at the right).
            let k = ctx.animate_bool_responsive(id.with("wide"), r.hovered() || r.dragged());
            let w = egui::lerp(crate::ui::SCROLL_THIN..=track.width(), k);
            let thin = |x: egui::Rect| egui::Rect::from_min_max(egui::pos2(x.right() - w, x.top()), x.max);
            let p = ui.painter();
            let rad = (w / 2.0) as u8;
            p.rect_filled(thin(track), rad, ui.visuals().widgets.inactive.fg_stroke.color.gamma_multiply(bg * 0.4));
            p.rect_filled(thin(at(st.offset.y)), rad, ui.visuals().widgets.inactive.fg_stroke.color.gamma_multiply(handle));
        }
    }
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
    if let Some((c, dir, shift)) = sort_pick {
        let at = v.sort.iter().position(|s| s.0 == c);
        match (dir, at) {
            (None, Some(l)) => {
                v.sort.remove(l);
            }
            (None, None) => {}
            (Some(asc), Some(l)) if shift || v.sort.len() > 1 => v.sort[l].1 = asc,
            (Some(asc), None) if shift => v.sort.push((c, asc)),
            (Some(asc), _) => v.sort = vec![(c, asc)],
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
    if clear_picks {
        v.picked.clear();
        v.rows_picked.clear();
    }
    if let Some((r, m)) = row_click {
        if m.command || m.ctrl {
            if !v.rows_picked.remove(&r) {
                v.rows_picked.insert(r);
            }
        } else if m.shift && !v.rows_picked.is_empty() {
            let pos = |x: usize| v.order.iter().position(|&y| y == x).unwrap_or(0);
            let first = v.rows_picked.iter().map(|&x| pos(x)).min().unwrap_or(0);
            let (a, b) = (first.min(pos(r)), first.max(pos(r)));
            v.rows_picked = v.order[a..=b].iter().copied().collect();
        } else if v.rows_picked.len() == 1 && v.rows_picked.contains(&r) {
            v.rows_picked.clear();
        } else {
            v.rows_picked = [r].into();
        }
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
    // Ctrl+C (when no text field has focus): the picked rows, else the
    // chosen cell.
    let typing = ctx.memory(|m| m.focused().is_some());
    if !typing && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
        if !v.rows_picked.is_empty() {
            let text = v.order.iter().filter(|r| v.rows_picked.contains(r)).map(|&r| v.cols.iter().map(|&x| tsv_cell(&v.table.rows[r][x])).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n");
            ctx.copy_text(text);
        } else if let Some((r, c)) = v.sel {
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

/// Paints `bg` over a rectangle's corners outside its rounding (what's
/// under them looks rounded).
fn corners(p: &egui::Painter, r: egui::Rect, radius: f32, bg: egui::Color32) {
    let n = 8;
    for (corner, center, a0) in [
        (r.left_top(), r.left_top() + egui::vec2(radius, radius), 180.0f32),
        (r.right_top(), r.right_top() + egui::vec2(-radius, radius), 270.0),
        (r.right_bottom(), r.right_bottom() + egui::vec2(-radius, -radius), 0.0),
        (r.left_bottom(), r.left_bottom() + egui::vec2(radius, -radius), 90.0),
    ] {
        // Triangles from the corner to each stretch of the arc.
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(corner, bg);
        for k in 0..=n {
            let a = (a0 + 90.0 * k as f32 / n as f32).to_radians();
            mesh.colored_vertex(center + egui::vec2(a.cos(), a.sin()) * radius, bg);
            if k > 0 {
                mesh.add_triangle(0, k as u32, k as u32 + 1);
            }
        }
        p.add(egui::Shape::mesh(mesh));
    }
}
