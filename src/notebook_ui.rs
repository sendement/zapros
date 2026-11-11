//! A tab as a notebook: cells one under another, each SQL (with a table
//! of its own under it) or Markdown (shown laid out, edited on a double
//! click). Outputs live only in memory unless a cell's 📌 keeps them.

use egui::{Color32, FontId, RichText, Stroke, text::LayoutJob};

use crate::app::App;
use crate::config::{Cell, CellKind};
use crate::editor::{self, Action};
use crate::i18n::t;
use crate::ui::{RADIUS, tinted};

/// What a cell's buttons ask, done after the cells are drawn.
enum Op {
    Add(usize, CellKind),
    Delete(usize),
    Up(usize),
    Down(usize),
    Kind(usize, CellKind),
    Run(String),
    Pin(String),
    Unpin(usize),
    Clear(String),
    Fold(usize),
    FoldAll(bool),
    Height(usize, f32),
}

pub fn show(app: &mut App, ui: &mut egui::Ui) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut ops: Vec<Op> = Vec::new();
    let ctx = ui.ctx().clone();
    // The notebook's own bar.
    ui.horizontal(|ui| {
        if ui.add(tinted(format!("+ {}", "SQL"), crate::ui::BLUE)).on_hover_text(t("Добавить блок с запросом в конец", "Add a SQL cell at the end")).clicked() {
            ops.push(Op::Add(usize::MAX, CellKind::Sql));
        }
        if ui.add(tinted(format!("+ {}", "Markdown"), crate::ui::VIOLET)).on_hover_text(t("Добавить текстовый блок в конец", "Add a text cell at the end")).clicked() {
            ops.push(Op::Add(usize::MAX, CellKind::Markdown));
        }
        ui.separator();
        if ui.add(tinted(format!("⏩ {}", t("Выполнить все", "Run all")), crate::ui::TEAL)).on_hover_text(t("Все блоки по порядку   Ctrl+Shift+Enter", "Every cell in order   Ctrl+Shift+Enter")).clicked() {
            actions.push(Action::RunScript);
        }
        ui.separator();
        if ui.button(format!("▸ {}", t("Свернуть все", "Fold all"))).on_hover_text(t("Свернуть все блоки до первой строки", "Fold every cell to its first line")).clicked() {
            ops.push(Op::FoldAll(true));
        }
        if ui.button(format!("▾ {}", t("Развернуть все", "Unfold all"))).on_hover_text(t("Развернуть все блоки", "Unfold every cell")).clicked() {
            ops.push(Op::FoldAll(false));
        }
        ui.separator();
        ui.menu_button(format!("📄 {}", t("В обычный SQL", "To plain SQL")), |ui| {
            if ui.button(t("Как есть (Markdown — комментариями)", "As it is (Markdown as comments)")).on_hover_text(t("Открыть в новой вкладке; блоки WITH остаются отдельными", "Opens in a new tab; WITH cells stay apart")).clicked() {
                app.export_notebook(false);
                ui.close();
            }
            if ui.button(t("Самостоятельные запросы (WITH подставлены)", "Standalone statements (WITHs put in)")).on_hover_text(t("Каждый запрос со своими CTE из блоков выше — выполняется отдельно", "Each statement with the CTEs it uses from the cells above — runs on its own")).clicked() {
                app.export_notebook(true);
                ui.close();
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(t("Ctrl+Enter — выполнить блок · блок из одного WITH отдаёт свои CTE блокам ниже", "Ctrl+Enter: run the cell · a cell of a WITH alone lends its CTEs to the cells below")).weak().small());
        });
    });
    ui.add_space(4.0);
    let tab = app.tab().clone();
    let Some(nb) = tab.notebook.as_ref() else { return actions };
    let conn = app.tab_conn(app.ws(), &tab);
    let dialect = app.dialect_of(conn.as_deref());
    let rels = conn.as_deref().and_then(|c| app.live.get(c)).map(|l| l.rels.clone()).unwrap_or_default();
    let settings = app.settings.clone();
    let accent = crate::editor::rgb(app.ws().color);
    let dark = ui.visuals().dark_mode;
    let focus = app.rt.get(&tab.id).and_then(|r| r.cell_focus.clone());
    let md_edit = app.rt.get(&tab.id).and_then(|r| r.md_edit.clone());
    let mut new_focus: Option<String> = None;
    let mut md_now = md_edit.clone();
    let n = nb.cells.len();
    egui::ScrollArea::vertical().id_salt(("notebook", &tab.id)).auto_shrink([false, false]).show(ui, |ui| {
        for (i, cell) in nb.cells.iter().enumerate() {
            app.restore_cell(&tab.id, cell);
            let key = App::cell_key(&tab.id, &cell.id);
            let focused = focus.as_deref() == Some(cell.id.as_str());
            let stroke = if focused { Stroke::new(1.0, accent) } else { Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color) };
            egui::Frame::NONE.stroke(stroke).corner_radius(RADIUS).inner_margin(egui::Margin::symmetric(8, 6)).fill(ui.visuals().panel_fill).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let running = app.rt.get(&key).is_some_and(|r| r.run.is_some());
                let has_out = app.rt.get(&key).is_some_and(|r| !r.results.is_empty() || !r.output.is_empty() || !r.log.is_empty());
                // Its header: kind, run, what it gave; moving, adding, deleting.
                ui.horizontal(|ui| {
                    let fold = ui.add(egui::Button::new(RichText::new(if cell.collapsed { "▸" } else { "▾" }).size(16.0)).frame(false)).on_hover_text(if cell.collapsed { t("Развернуть блок", "Unfold the cell") } else { t("Свернуть блок", "Fold the cell") });
                    if fold.clicked() {
                        ops.push(Op::Fold(i));
                    }
                    ui.label(RichText::new(format!("[{}]", i + 1)).weak().small().monospace());
                    for (k, name) in [(CellKind::Sql, "SQL"), (CellKind::Markdown, "Markdown")] {
                        if ui.selectable_label(cell.kind == k, RichText::new(name).small()).clicked() && cell.kind != k {
                            ops.push(Op::Kind(i, k));
                        }
                    }
                    if cell.kind == CellKind::Sql {
                        ui.separator();
                        if ui.add_enabled(!running, tinted(format!("▶ {}", t("Выполнить", "Run")), crate::ui::GREEN)).on_hover_text(t("Выполнить блок   Ctrl+Enter", "Run the cell   Ctrl+Enter")).clicked() {
                            ops.push(Op::Run(cell.id.clone()));
                        }
                        if running {
                            ui.add(egui::Spinner::new().size(14.0));
                        }
                        if crate::sql::notebook::parse_with(cell.text.trim().trim_end_matches(';'), dialect).is_some_and(|w| w.main.is_empty()) {
                            ui.label(RichText::new(t("WITH — для блоков ниже", "WITH — for the cells below")).small().color(crate::ui::TEAL));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("🗑").on_hover_text(t("Удалить блок", "Delete the cell")).clicked() {
                            ops.push(Op::Delete(i));
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("⏷").small()).on_hover_text(t("Ниже", "Down")).clicked() {
                            ops.push(Op::Down(i));
                        }
                        if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).on_hover_text(t("Выше", "Up")).clicked() {
                            ops.push(Op::Up(i));
                        }
                        ui.menu_button(" + ", |ui| {
                            if ui.button(t("SQL-блок ниже", "SQL cell below")).clicked() {
                                ops.push(Op::Add(i + 1, CellKind::Sql));
                                ui.close();
                            }
                            if ui.button(t("Markdown-блок ниже", "Markdown cell below")).clicked() {
                                ops.push(Op::Add(i + 1, CellKind::Markdown));
                                ui.close();
                            }
                            if ui.button(t("SQL-блок выше", "SQL cell above")).clicked() {
                                ops.push(Op::Add(i, CellKind::Sql));
                                ui.close();
                            }
                        });
                        if cell.kind == CellKind::Sql {
                            ui.separator();
                            if has_out && ui.small_button("✖").on_hover_text(t("Очистить вывод (сохранённый остаётся)", "Clear the output (a kept one stays)")).clicked() {
                                ops.push(Op::Clear(key.clone()));
                            }
                            let pin = ui.add_enabled(has_out && !running, tinted("📌".into(), crate::ui::VIOLET)).on_hover_text(t("Сохранить вывод блока на диск (иначе он живёт только до закрытия)", "Keep the cell's output on disk (else it lives only until closing)"));
                            if pin.clicked() {
                                ops.push(Op::Pin(cell.id.clone()));
                            }
                            if let Some(s) = &cell.saved {
                                let r = ui.label(RichText::new(format!("📌 {}", s.when)).small().weak()).on_hover_text(t("Сохранённый вывод; правый щелчок — убрать", "Kept output; right click to drop it"));
                                r.context_menu(|ui| {
                                    if ui.button(t("Убрать сохранённый вывод", "Drop the kept output")).clicked() {
                                        ops.push(Op::Unpin(i));
                                        ui.close();
                                    }
                                });
                            }
                        }
                    });
                });
                if cell.collapsed {
                    // Folded: its first line, a click opens it.
                    let first = cell.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
                    let more = cell.text.trim().lines().count().saturating_sub(1);
                    let label = if more > 0 { format!("{first}   ⋯ {more}") } else { first.to_string() };
                    let r = ui.add(egui::Label::new(RichText::new(label).monospace().weak()).truncate().sense(egui::Sense::click())).on_hover_text(t("Свёрнуто — щелчок развернёт", "Folded: click to open"));
                    if r.clicked() {
                        ops.push(Op::Fold(i));
                    }
                    return;
                }
                match cell.kind {
                    CellKind::Sql => {
                        let mut text = cell.text.clone();
                        let mut rt = app.rt.remove(&key).unwrap_or_default();
                        let issues = std::mem::take(&mut rt.issues);
                        let server_err = rt.server_err.clone();
                        let rows = text.lines().count().max(2) + 1;
                        let cx = editor::Ctx { dialect, settings: &settings, dark, issues: &issues, server_err: server_err.as_ref(), rels: &rels, rows };
                        let id = egui::Id::new(("cell", &tab.id, &cell.id));
                        let out = egui::ScrollArea::horizontal().id_salt(("cell-h", &cell.id)).show(ui, |ui| editor::show(ui, id, &mut text, &mut rt.ed, &cx)).inner;
                        rt.issues = issues;
                        if ctx.memory(|m| m.has_focus(id)) || !out.actions.is_empty() {
                            new_focus = Some(cell.id.clone());
                        }
                        actions.extend(out.actions);
                        if out.changed {
                            rt.server_err = None;
                            ops_text(app, &tab.id, &cell.id, text);
                        }
                        app.rt.insert(key.clone(), rt);
                        // Its output: the same results as a tab's, in a box of its own.
                        let r = &app.rt[&key];
                        if !r.results.is_empty() || !r.log.is_empty() || !r.output.is_empty() {
                            let usual = if r.results.is_empty() { 110.0 } else { 300.0 };
                            // Being dragged: the height as the drag goes.
                            let drag_id = egui::Id::new(("cell-out-h", &cell.id));
                            let h = ctx.data(|d| d.get_temp::<f32>(drag_id)).unwrap_or(if cell.out_h > 0.0 { cell.out_h } else { usual });
                            ui.add_space(4.0);
                            ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
                                ui.set_min_height(h);
                                ui.set_max_height(h);
                                ui.push_id(("cell-out", &cell.id), |ui| crate::results::panel_for(app, ui, &key));
                            });
                            // The grip under it: drag to make it taller or lower.
                            let (grip, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::click_and_drag());
                            let resp = resp.on_hover_cursor(egui::CursorIcon::ResizeVertical).on_hover_text(t("Тянуть — высота вывода; двойной щелчок — обычная", "Drag: the output's height; double-click: the usual"));
                            let c = if resp.hovered() || resp.dragged() { accent } else { ui.visuals().widgets.noninteractive.bg_stroke.color };
                            ui.painter().line_segment([egui::pos2(grip.center().x - 24.0, grip.center().y), egui::pos2(grip.center().x + 24.0, grip.center().y)], Stroke::new(3.0, c));
                            if resp.dragged() {
                                let nh = (h + resp.drag_delta().y).clamp(80.0, 1600.0);
                                ctx.data_mut(|d| d.insert_temp(drag_id, nh));
                            }
                            if resp.drag_stopped() {
                                ops.push(Op::Height(i, h));
                                ctx.data_mut(|d| d.remove::<f32>(drag_id));
                            }
                            if resp.double_clicked() {
                                ops.push(Op::Height(i, 0.0));
                            }
                        }
                    }
                    CellKind::Markdown => {
                        if md_edit.as_deref() == Some(cell.id.as_str()) {
                            let mut text = cell.text.clone();
                            let lines = text.lines().count().max(3);
                            let r = ui.add(egui::TextEdit::multiline(&mut text).id(egui::Id::new(("md", &cell.id))).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(lines).hint_text(t("# Заголовок, **жирный**, *курсив*, `код`, - список", "# Heading, **bold**, *italic*, `code`, - list")));
                            if r.changed() {
                                ops_text(app, &tab.id, &cell.id, text);
                            }
                            if r.has_focus() {
                                new_focus = Some(cell.id.clone());
                            }
                            // Done: Esc, Ctrl+Enter, or a click away.
                            if r.lost_focus() || (r.has_focus() && ctx.input(|i| i.key_pressed(egui::Key::Escape) || (i.modifiers.command && i.key_pressed(egui::Key::Enter)))) {
                                md_now = None;
                            } else if !r.has_focus() && md_now.as_deref() == Some(cell.id.as_str()) && ctx.memory(|m| m.focused().is_none()) {
                                r.request_focus();
                            }
                        } else {
                            let r = ui.scope(|ui| markdown(ui, &cell.text)).response;
                            let r = ui.interact(r.rect, egui::Id::new(("md-view", &cell.id)), egui::Sense::click()).on_hover_text(t("Двойной щелчок — редактировать", "Double-click to edit"));
                            if r.double_clicked() || (cell.text.trim().is_empty() && r.clicked()) {
                                md_now = Some(cell.id.clone());
                                new_focus = Some(cell.id.clone());
                            }
                        }
                    }
                }
            });
            ui.add_space(6.0);
        }
        // Room to add one more at the end.
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new(format!("+ {}", t("SQL-блок", "SQL cell"))).weak()).frame(false)).clicked() {
                ops.push(Op::Add(usize::MAX, CellKind::Sql));
            }
            if ui.add(egui::Button::new(RichText::new(format!("+ {}", t("Markdown-блок", "Markdown cell"))).weak()).frame(false)).clicked() {
                ops.push(Op::Add(usize::MAX, CellKind::Markdown));
            }
        });
        ui.add_space(40.0);
    });
    {
        let rt = app.rt.entry(tab.id.clone()).or_default();
        if new_focus.is_some() {
            rt.cell_focus = new_focus;
        }
        rt.md_edit = md_now;
    }
    for op in ops {
        apply(app, &ctx, op);
    }
    actions
}

/// A cell's new text.
fn ops_text(app: &mut App, tab: &str, cell: &str, text: String) {
    let wi = app.session.active;
    let ws = &mut app.session.workspaces[wi];
    let Some(tb) = ws.tabs.iter_mut().find(|t| t.id == tab) else { return };
    if let Some(c) = tb.notebook.as_mut().and_then(|n| n.cells.iter_mut().find(|c| c.id == cell)) {
        c.text = text;
    }
    if tb.path.is_some() {
        tb.modified = true;
    }
    app.touch();
}

fn apply(app: &mut App, ctx: &egui::Context, op: Op) {
    let tab_id = app.tab().id.clone();
    if let Op::Run(c) = &op {
        app.rt.entry(tab_id.clone()).or_default().cell_focus = Some(c.clone());
        app.run_cells(ctx, Some(c.clone()));
        return;
    }
    if let Op::Pin(c) = &op {
        app.pin_cell(c);
        return;
    }
    if let Op::Clear(key) = &op {
        app.rt.insert(key.clone(), crate::app::TabRt::default());
        return;
    }
    let Some(nb) = app.tab_mut().notebook.as_mut() else { return };
    let mut focus = None;
    let mut edit = None;
    match op {
        Op::Add(at, kind) => {
            let c = Cell { kind, ..Default::default() };
            focus = Some(c.id.clone());
            if kind == CellKind::Markdown {
                edit = Some(c.id.clone());
            }
            let at = at.min(nb.cells.len());
            nb.cells.insert(at, c);
        }
        Op::Delete(i) => {
            if i < nb.cells.len() {
                nb.cells.remove(i);
            }
            if nb.cells.is_empty() {
                nb.cells.push(Cell::default());
            }
        }
        Op::Up(i) if i > 0 => nb.cells.swap(i, i - 1),
        Op::Down(i) if i + 1 < nb.cells.len() => nb.cells.swap(i, i + 1),
        Op::Kind(i, k) => {
            if let Some(c) = nb.cells.get_mut(i) {
                c.kind = k;
                if k == CellKind::Markdown {
                    edit = Some(c.id.clone());
                }
            }
        }
        Op::Fold(i) => {
            if let Some(c) = nb.cells.get_mut(i) {
                c.collapsed = !c.collapsed;
            }
        }
        Op::FoldAll(on) => {
            for c in nb.cells.iter_mut() {
                c.collapsed = on;
            }
        }
        Op::Height(i, h) => {
            if let Some(c) = nb.cells.get_mut(i) {
                c.out_h = h;
            }
        }
        Op::Unpin(i) => {
            if let Some(c) = nb.cells.get_mut(i) {
                c.saved = None;
            }
        }
        _ => {}
    }
    let rt = app.rt.entry(tab_id).or_default();
    if focus.is_some() {
        rt.cell_focus = focus;
    }
    if edit.is_some() {
        rt.md_edit = edit;
    }
    app.touch();
}

/// Inline Markdown: **bold**, *italic* / _italic_, `code`, [text](link).
fn inline(ui: &egui::Ui, text: &str, size: f32, strong: bool, job: &mut LayoutJob) {
    let base = ui.visuals().text_color();
    let strong_c = ui.visuals().strong_text_color();
    let code_bg = ui.visuals().code_bg_color;
    let link = ui.visuals().hyperlink_color;
    let mut bold = strong;
    let mut italic = false;
    let mut chars = text.char_indices().peekable();
    let mut run = String::new();
    let push = |job: &mut LayoutJob, s: &mut String, bold: bool, italic: bool| {
        if s.is_empty() {
            return;
        }
        let font = if bold { FontId::new(size, egui::FontFamily::Name("bold".into())) } else { FontId::proportional(size) };
        let mut f = egui::TextFormat::simple(font, if bold { strong_c } else { base });
        f.italics = italic;
        job.append(s, 0.0, f);
        s.clear();
    };
    while let Some((i, c)) = chars.next() {
        let rest = &text[i..];
        if rest.starts_with("**") || rest.starts_with("__") {
            push(job, &mut run, bold, italic);
            bold = !bold;
            chars.next();
        } else if (c == '*' || c == '_') && !(text[..i].ends_with(' ') && rest[1..].starts_with(' ')) {
            push(job, &mut run, bold, italic);
            italic = !italic;
        } else if c == '`' {
            if let Some(end) = rest[1..].find('`') {
                push(job, &mut run, bold, italic);
                let mut f = egui::TextFormat::simple(FontId::monospace(size * 0.92), base);
                f.background = code_bg;
                job.append(&rest[1..1 + end], 0.0, f);
                for _ in 0..end + 1 {
                    chars.next();
                }
            } else {
                run.push(c);
            }
        } else if c == '[' {
            // [text](link): the text, as a link would look.
            match (rest.find("]("), rest.find(')')) {
                (Some(a), Some(b)) if a < b => {
                    push(job, &mut run, bold, italic);
                    let mut f = egui::TextFormat::simple(FontId::proportional(size), link);
                    f.underline = Stroke::new(1.0, link);
                    job.append(&rest[1..a], 0.0, f);
                    for _ in 0..b {
                        chars.next();
                    }
                }
                _ => run.push(c),
            }
        } else {
            run.push(c);
        }
    }
    push(job, &mut run, bold, italic);
}

/// Markdown laid out: headings, lists, quotes, code blocks, rules,
/// paragraphs.
pub fn markdown(ui: &mut egui::Ui, text: &str) {
    if text.trim().is_empty() {
        ui.label(RichText::new(t("Пустой текстовый блок — щёлкните, чтобы написать", "An empty text cell: click to write")).weak().italics());
        return;
    }
    let body = 14.0;
    let mut code: Option<Vec<&str>> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            match code.take() {
                Some(lines) => {
                    egui::Frame::NONE.fill(ui.visuals().code_bg_color).corner_radius(4).inner_margin(6).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(lines.join("\n")).monospace());
                    });
                }
                None => code = Some(Vec::new()),
            }
            continue;
        }
        if let Some(c) = code.as_mut() {
            c.push(line);
            continue;
        }
        let l = line.trim_end();
        if l.trim().is_empty() {
            ui.add_space(4.0);
            continue;
        }
        let hashes = l.chars().take_while(|c| *c == '#').count();
        let mut job = LayoutJob::default();
        job.wrap.max_width = ui.available_width();
        if (1..=6).contains(&hashes) && l[hashes..].starts_with(' ') {
            let size = [22.0, 19.0, 16.5, 15.0, 14.0, 13.0][hashes - 1];
            inline(ui, l[hashes..].trim(), size, true, &mut job);
            ui.add_space(2.0);
            ui.label(job);
            if hashes <= 2 {
                let r = ui.min_rect();
                ui.painter().hline(r.x_range(), ui.cursor().top(), Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color));
                ui.add_space(3.0);
            }
        } else if l.trim() == "---" || l.trim() == "***" {
            ui.separator();
        } else if let Some(rest) = l.trim_start().strip_prefix("- ").or_else(|| l.trim_start().strip_prefix("* ")) {
            let indent = (l.len() - l.trim_start().len()) as f32 * 6.0;
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent + 6.0);
                ui.label("•");
                inline(ui, rest, body, false, &mut job);
                ui.label(job);
            });
        } else if let Some((num, rest)) = l.trim_start().split_once(". ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(6.0);
                ui.label(format!("{num}."));
                inline(ui, rest, body, false, &mut job);
                ui.label(job);
            });
        } else if let Some(rest) = l.trim_start().strip_prefix("> ") {
            ui.horizontal_wrapped(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(3.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 1.0, Color32::GRAY);
                inline(ui, rest, body, false, &mut job);
                ui.label(job);
            });
        } else {
            inline(ui, l, body, false, &mut job);
            ui.label(job);
        }
    }
    if let Some(lines) = code {
        ui.label(RichText::new(lines.join("\n")).monospace());
    }
}
