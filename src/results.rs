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

#[allow(dead_code)]
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
        Pane::Result(i) => table(ui, &tab_id, i, &mut rt.results[i]),
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

/// A column's totals over the rows shown: count, NULLs, sum, min, max,
/// average (numbers; text gets min / max).
pub fn totals(v: &ResultView, c: usize) -> Vec<(&'static str, String)> {
    let vals: Vec<Option<&str>> = v.order.iter().map(|&r| v.table.rows.get(r).and_then(|row| row.get(c)).and_then(|x| x.as_deref())).collect();
    let nulls = vals.iter().filter(|x| x.is_none()).count();
    let some: Vec<&str> = vals.iter().flatten().copied().collect();
    let nums: Vec<f64> = some.iter().filter_map(|s| s.trim().parse::<f64>().ok()).collect();
    let mut out = vec![(t("Количество", "Count"), some.len().to_string())];
    if nulls > 0 {
        out.push(("NULL", nulls.to_string()));
    }
    if !nums.is_empty() && nums.len() == some.len() {
        let sum: f64 = nums.iter().sum();
        let fmt = |x: f64| crate::db::fmt_float((x * 1e9).round() / 1e9);
        out.push((t("Сумма", "Sum"), fmt(sum)));
        out.push((t("Минимум", "Min"), fmt(nums.iter().cloned().fold(f64::INFINITY, f64::min))));
        out.push((t("Максимум", "Max"), fmt(nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max))));
        out.push((t("Среднее", "Average"), fmt(sum / nums.len() as f64)));
    } else if !some.is_empty() {
        out.push((t("Минимум", "Min"), some.iter().min().unwrap().to_string()));
        out.push((t("Максимум", "Max"), some.iter().max().unwrap().to_string()));
    }
    out
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
                if ui.button("🗐").on_hover_text(t("Копировать значение", "Copy value")).clicked() {
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
    let sums = |c: usize| totals(v, c);
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
                    ui.separator();
                    ui.label(RichText::new(format!("Σ {}", t("Итоги по колонке", "Column totals"))).strong());
                    for (name, val) in &sums(c) {
                        if ui.button(format!("{name}: {val}")).on_hover_text(t("Щелчок — копировать", "Click: copy")).clicked() {
                            copy = Some(val.clone());
                            ui.close();
                        }
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
