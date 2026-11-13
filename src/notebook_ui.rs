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
    // Narrow: the buttons keep their icons only (names in the tips).
    let narrow = ui.available_width() < 800.0;
    let lbl = move |icon: &str, name: &str| if narrow { icon.to_string() } else { format!("{icon} {name}") };
    // The notebook's own bar.
    ui.horizontal(|ui| {
        if ui.add(tinted(format!("+ {}", "SQL"), crate::ui::BLUE)).on_hover_text(t("Добавить блок с запросом в конец", "Add a SQL cell at the end")).clicked() {
            ops.push(Op::Add(usize::MAX, CellKind::Sql));
        }
        if ui.add(tinted(format!("+ {}", if narrow { "MD" } else { "Markdown" }), crate::ui::VIOLET)).on_hover_text(t("Добавить текстовый блок в конец", "Add a text cell at the end")).clicked() {
            ops.push(Op::Add(usize::MAX, CellKind::Markdown));
        }
        ui.separator();
        if ui.add(tinted(lbl("⏩", t("Выполнить все", "Run all")), crate::ui::TEAL)).on_hover_text(t("Все блоки по порядку   Ctrl+Shift+Enter", "Every cell in order   Ctrl+Shift+Enter")).clicked() {
            actions.push(Action::RunScript);
        }
        ui.separator();
        if ui.button(lbl("▸", t("Свернуть все", "Fold all"))).on_hover_text(t("Свернуть все блоки до первой строки", "Fold every cell to its first line")).clicked() {
            ops.push(Op::FoldAll(true));
        }
        if ui.button(lbl("▾", t("Развернуть все", "Unfold all"))).on_hover_text(t("Развернуть все блоки", "Unfold every cell")).clicked() {
            ops.push(Op::FoldAll(false));
        }
        ui.separator();
        ui.menu_button(lbl("📄", t("В обычный SQL", "To plain SQL")), |ui| {
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
            // Only in the room left (cut short, never over the buttons).
            ui.add(egui::Label::new(RichText::new(t("Ctrl+Enter — выполнить блок · блок из одного WITH отдаёт свои CTE блокам ниже", "Ctrl+Enter: run the cell · a cell of a WITH alone lends its CTEs to the cells below")).weak().small()).truncate());
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
    // Markdown cells being edited (the rest shown), each until switched back.
    let md_edit = app.rt.get(&tab.id).map(|r| r.md_edit.clone()).unwrap_or_default();
    let mut new_focus: Option<String> = None;
    let mut md_now = md_edit.clone();
    let n = nb.cells.len();
    // (The notebook's scroll, as egui names it: moved along when an output's
    // top edge is pulled.)
    let nb_scroll = ui.make_persistent_id(egui::IdSalt::new(("notebook", &tab.id)));
    let mut pull = 0.0f32;
    egui::ScrollArea::vertical().id_salt(("notebook", &tab.id)).auto_shrink([false, false]).show(ui, |ui| {
        for (i, cell) in nb.cells.iter().enumerate() {
            app.restore_cell(&tab.id, cell);
            let key = App::cell_key(&tab.id, &cell.id);
            let focused = focus.as_deref() == Some(cell.id.as_str());
            let stroke = if focused { Stroke::new(1.0, accent) } else { Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color) };
            let framed = egui::Frame::NONE.corner_radius(RADIUS).inner_margin(egui::Margin::symmetric(8, 6)).fill(ui.visuals().panel_fill).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let running = app.rt.get(&key).is_some_and(|r| r.run.is_some());
                let has_out = app.rt.get(&key).is_some_and(|r| !r.results.is_empty() || !r.output.is_empty() || !r.log.is_empty());
                // Its header: kind, run, what it gave; moving, adding, deleting.
                ui.horizontal(|ui| {
                    let fold = ui.add(egui::Button::new("").min_size(egui::vec2(24.0, 24.0)));
                    crate::ui::paint_chevron(ui.painter(), fold.rect.center(), 11.0, if cell.collapsed { 0.0 } else { 1.0 }, if fold.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() });
                    let fold = fold.on_hover_text(if cell.collapsed { t("Развернуть блок", "Unfold the cell") } else { t("Свернуть блок", "Fold the cell") });
                    if fold.clicked() {
                        ops.push(Op::Fold(i));
                    }
                    ui.label(RichText::new(format!("[{}]", i + 1)).weak().small().monospace());
                    for (k, name) in [(CellKind::Sql, "SQL"), (CellKind::Markdown, if narrow { "MD" } else { "Markdown" })] {
                        if ui.selectable_label(cell.kind == k, name).clicked() && cell.kind != k {
                            ops.push(Op::Kind(i, k));
                        }
                    }
                    if cell.kind == CellKind::Markdown {
                        // Shown or its text being edited.
                        ui.separator();
                        let editing = md_now.contains(&cell.id);
                        if ui.selectable_label(!editing, t("Просмотр", "View")).on_hover_text(t("Показать оформленным   Esc", "Show it formatted   Esc")).clicked() {
                            md_now.remove(&cell.id);
                        }
                        if ui.selectable_label(editing, t("Правка", "Edit")).on_hover_text(t("Редактировать текст (или двойной щелчок по нему)", "Edit the text (or double-click it)")).clicked() && !editing {
                            md_now.insert(cell.id.clone());
                            ctx.data_mut(|d| d.insert_temp(egui::Id::new("md-focus"), cell.id.clone()));
                        }
                    }
                    if cell.kind == CellKind::Sql {
                        ui.separator();
                        if ui.add_enabled(!running, tinted(lbl("▶", t("Выполнить", "Run")), crate::ui::GREEN)).on_hover_text(t("Выполнить блок   Ctrl+Enter", "Run the cell   Ctrl+Enter")).clicked() {
                            ops.push(Op::Run(cell.id.clone()));
                        }
                        if running {
                            ui.add(egui::Spinner::new().size(14.0));
                        }
                        if crate::sql::notebook::parse_with(cell.text.trim().trim_end_matches(';'), dialect).is_some_and(|w| w.main.is_empty()) {
                            ui.label(RichText::new(if narrow { "WITH" } else { t("WITH — для блоков ниже", "WITH — for the cells below") }).small().color(crate::ui::TEAL)).on_hover_text(t("Блок из одного WITH: его CTE получат блоки ниже", "A WITH alone: the cells below get its CTEs"));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new("🗑").min_size(egui::vec2(28.0, 24.0))).on_hover_text(t("Удалить блок", "Delete the cell")).clicked() {
                            ops.push(Op::Delete(i));
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("⏷").min_size(egui::vec2(28.0, 24.0))).on_hover_text(t("Ниже", "Down")).clicked() {
                            ops.push(Op::Down(i));
                        }
                        if ui.add_enabled(i > 0, egui::Button::new("⏶").min_size(egui::vec2(28.0, 24.0))).on_hover_text(t("Выше", "Up")).clicked() {
                            ops.push(Op::Up(i));
                        }
                        egui::containers::menu::MenuButton::from_button(egui::Button::new("+").min_size(egui::vec2(28.0, 24.0))).ui(ui, |ui| {
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
                            if has_out && ui.add(egui::Button::new("✖").min_size(egui::vec2(28.0, 24.0))).on_hover_text(t("Очистить вывод (сохранённый остаётся)", "Clear the output (a kept one stays)")).clicked() {
                                ops.push(Op::Clear(key.clone()));
                            }
                            let pin = ui.add_enabled(has_out && !running, tinted("📌".into(), crate::ui::VIOLET).min_size(egui::vec2(28.0, 24.0))).on_hover_text(t("Сохранить вывод блока на диск (иначе он живёт только до закрытия)", "Keep the cell's output on disk (else it lives only until closing)"));
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
                        // The code on a rounded ground of its own (a shade
                        // off the cell's), as wide as the output's table.
                        let v = ui.visuals();
                        let ground = crate::themes::mix(v.panel_fill, v.text_color(), if v.dark_mode { 0.045 } else { 0.035 });
                        let out = egui::Frame::NONE
                            .fill(ground)
                            .corner_radius(6)
                            .inner_margin(egui::Margin { left: 0, right: 4, top: 2, bottom: 2 })
                            .outer_margin(egui::Margin { left: 0, right: crate::ui::TABLE_BAR_ROOM as i8, top: 0, bottom: 0 })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                egui::ScrollArea::horizontal().id_salt(("cell-h", &cell.id)).show(ui, |ui| editor::show(ui, id, &mut text, &mut rt.ed, &cx)).inner
                            })
                            .inner;
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
                            // Its top edge: pulled up, the output grows upward (the
                            // notebook scrolls along so the edge stays under the mouse),
                            // as the results panel's edge does.
                            let (edge, top) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::drag());
                            let top = top.on_hover_cursor(egui::CursorIcon::ResizeVertical).on_hover_text(t("Тянуть — высота вывода", "Drag: the output's height"));
                            let c = if top.hovered() || top.dragged() { accent } else { ui.visuals().widgets.noninteractive.bg_stroke.color };
                            // (From where the grounds' roundings start.)
                            ui.painter().hline(egui::Rangef::new(edge.left() + 6.0, edge.right() - 6.0 - crate::ui::TABLE_BAR_ROOM), edge.center().y, Stroke::new(if top.hovered() || top.dragged() { 2.0 } else { 1.0 }, c));
                            if top.dragged() {
                                let dy = top.drag_delta().y;
                                let nh = (h - dy).clamp(80.0, 1600.0);
                                ctx.data_mut(|d| d.insert_temp(drag_id, nh));
                                // (Grown by so much: everything moves up by as much.)
                                pull += nh - h;
                            }
                            if top.drag_stopped() {
                                ops.push(Op::Height(i, h));
                                ctx.data_mut(|d| d.remove::<f32>(drag_id));
                            }
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
                        if md_edit.contains(&cell.id) {
                            let mut text = cell.text.clone();
                            let lines = text.lines().count().max(3);
                            let r = ui.add(egui::TextEdit::multiline(&mut text).id(egui::Id::new(("md", &cell.id))).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(lines).hint_text(t("# Заголовок, **жирный**, *курсив*, `код`, - список", "# Heading, **bold**, *italic*, `code`, - list")));
                            if r.changed() {
                                ops_text(app, &tab.id, &cell.id, text);
                            }
                            if r.has_focus() {
                                new_focus = Some(cell.id.clone());
                            }
                            // Just switched to: the caret goes in.
                            let want = egui::Id::new("md-focus");
                            if ctx.data(|d| d.get_temp::<String>(want)).as_deref() == Some(cell.id.as_str()) {
                                r.request_focus();
                                ctx.data_mut(|d| d.remove::<String>(want));
                            }
                            // Done: Esc or Ctrl+Enter (or the switch).
                            if r.has_focus() && ctx.input(|i| i.key_pressed(egui::Key::Escape) || (i.modifiers.command && i.key_pressed(egui::Key::Enter))) {
                                md_now.remove(&cell.id);
                                r.surrender_focus();
                            }
                        } else {
                            let r = ui.scope(|ui| markdown(ui, &cell.text)).response;
                            let r = ui.interact(r.rect, egui::Id::new(("md-view", &cell.id)), egui::Sense::click()).on_hover_text(t("Двойной щелчок — редактировать", "Double-click to edit"));
                            if r.double_clicked() || (cell.text.trim().is_empty() && r.clicked()) {
                                md_now.insert(cell.id.clone());
                                ctx.data_mut(|d| d.insert_temp(egui::Id::new("md-focus"), cell.id.clone()));
                                new_focus = Some(cell.id.clone());
                            }
                        }
                    }
                }
            });
            // The edge drawn twice: its rounded corners as strong as its
            // sides (a thin line's corners come out paler).
            let edge = framed.response.rect.shrink(0.5);
            for _ in 0..2 {
                ui.painter().rect_stroke(edge, RADIUS, stroke, egui::StrokeKind::Middle);
            }
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
    if pull != 0.0 {
        if let Some(mut st) = egui::scroll_area::State::load(ui.ctx(), nb_scroll) {
            st.offset.y = (st.offset.y + pull).max(0.0);
            st.store(ui.ctx(), nb_scroll);
        }
    }
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
    let mut edit: Option<String> = None;
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
    if let Some(id) = edit {
        rt.md_edit.insert(id.clone());
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("md-focus"), id));
    }
    app.touch();
}

/// Inline Markdown: **bold**, *italic* / _italic_, `code`, [text](link).
/// Whether `mark` at `i` opens (or, `closing`, closes) emphasis, as in
/// CommonMark: `_` inside a word (my_table_name) is just a character, and
/// nothing opens without a mark to close it further on.
fn emphasis(text: &str, i: usize, mark: &str, closing: bool) -> bool {
    let underscore = mark.starts_with('_');
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let can_close = |at: usize| {
        let before = text[..at].chars().next_back();
        let after = text[at + mark.len()..].chars().next();
        before.is_some_and(|c| !c.is_whitespace()) && !(underscore && word(after))
    };
    if closing {
        return can_close(i);
    }
    let before = text[..i].chars().next_back();
    let after = text[i + mark.len()..].chars().next();
    let can_open = after.is_some_and(|c| !c.is_whitespace()) && !(underscore && word(before));
    can_open && text[i + mark.len()..].match_indices(mark).any(|(j, _)| j > 0 && can_close(i + mark.len() + j))
}

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
    // Which mark opened the bold / italic now on (`**` / `__`, `*` / `_`).
    let mut bold_by: Option<&str> = None;
    let mut italic_by: Option<&str> = None;
    while let Some((i, c)) = chars.next() {
        let rest = &text[i..];
        if c == '\\' && rest[1..].starts_with(|n: char| n.is_ascii_punctuation()) {
            // \_ \* \` …: the mark itself.
            let (_, n) = chars.next().unwrap();
            run.push(n);
        } else if let Some(mark) = ["**", "__"].into_iter().find(|m| rest.starts_with(m)).filter(|m| emphasis(text, i, m, bold_by == Some(*m))) {
            push(job, &mut run, bold, italic);
            bold_by = if bold_by == Some(mark) { None } else { Some(mark) };
            bold = !bold;
            chars.next();
        } else if let Some(mark) = ["*", "_"].into_iter().find(|m| rest.starts_with(m)).filter(|m| emphasis(text, i, m, italic_by == Some(*m))) {
            push(job, &mut run, bold, italic);
            italic_by = if italic_by == Some(mark) { None } else { Some(mark) };
            italic = !italic;
        } else if c == '_' || c == '*' {
            // Marks that open nothing: the whole run of them as written.
            run.push(c);
            while chars.peek().is_some_and(|&(_, n)| n == c) {
                run.push(chars.next().unwrap().1);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The text as laid out, and which bits came out italic.
    fn laid(md: &str) -> (String, Vec<String>) {
        let ctx = egui::Context::default();
        let mut job = LayoutJob::default();
        let mut out = ctx.run_ui(Default::default(), |ui| inline(ui, md, 14.0, false, &mut job));
        out.textures_delta.clear();
        let italic = job.sections.iter().filter(|s| s.format.italics).map(|s| job.text[s.byte_range.start.0..s.byte_range.end.0].to_string()).collect();
        (job.text, italic)
    }

    #[test]
    fn underscores_in_words_stay() {
        assert_eq!(laid("my_table_name and t_1").0, "my_table_name and t_1");
        assert_eq!(laid("a lone _ here").0, "a lone _ here");
        assert_eq!(laid("_tmp table").0, "_tmp table");
        let (text, it) = laid("this is _very_ nice, *so* sure");
        assert_eq!(text, "this is very nice, so sure");
        assert_eq!(it, vec!["very".to_string(), "so".to_string()]);
        assert_eq!(laid("snake__case__name").0, "snake__case__name");
        assert_eq!(laid(r"\_escaped\_ and \*star\*").0, "_escaped_ and *star*");
        assert_eq!(laid("x * y * z").0, "x * y * z");
    }
}
