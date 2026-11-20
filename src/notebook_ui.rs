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
    /// The cell's plan.
    Explain(String),
    /// What the cell runs (or loads) stops.
    Stop(String),
    /// A hung cell's run ended by force.
    Force(String),
    Pin(String),
    Unpin(usize),
    Clear(String),
    Fold(usize),
    FoldAll(bool),
    Height(usize, f32),
    Format(usize),
    /// A label on / off.
    Label(usize, u8),
    /// A cell dragged from one place to before another (0…n).
    Move(usize, usize),
}

use crate::config::{LabelDef, label_name};

/// How a label's dot looks.
#[derive(Clone, Copy, PartialEq)]
enum Dot {
    /// Filled: on.
    On,
    /// A ring: off (or, without a colour, "no labels").
    Off,
    /// Crossed: left out (the filter).
    Out,
}

/// A label's dot in its colour (`color` None: an empty ring); `sense`:
/// whether it takes clicks itself.
fn label_dot(ui: &mut egui::Ui, color: Option<Color32>, dot: Dot, size: f32, sense: egui::Sense) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(size, size), sense);
    let c = color.unwrap_or(ui.visuals().weak_text_color());
    let rad = size / 2.0 - 1.0;
    let p = ui.painter();
    match dot {
        Dot::On => {
            p.circle_filled(rect.center(), rad, c);
        }
        Dot::Off => {
            p.circle_stroke(rect.center(), rad - 0.5, Stroke::new(if color.is_some() { 1.2 } else { 1.0 }, c.gamma_multiply(0.85)));
        }
        Dot::Out => {
            p.circle_stroke(rect.center(), rad - 0.5, Stroke::new(1.2, c));
            let d = rad * 0.7;
            p.line_segment([rect.center() + egui::vec2(-d, d), rect.center() + egui::vec2(d, -d)], Stroke::new(1.4, c));
        }
    }
    if r.hovered() && sense.senses_click() {
        p.circle_stroke(rect.center(), rad + 1.0, Stroke::new(1.0, ui.visuals().strong_text_color()));
    }
    r
}

fn color_of(labels: &[LabelDef], k: usize) -> Option<Color32> {
    labels.get(k).map(|l| crate::editor::rgb(l.color))
}

/// Whether a cell fits the notebook's filter.
fn fits(rt: &crate::app::TabRt, c: &crate::config::Cell) -> bool {
    let q = rt.nb_query.trim().to_lowercase();
    if !q.is_empty() && !c.text.to_lowercase().contains(&q) {
        return false;
    }
    if !rt.nb_labels.is_empty() {
        let has = |l: &u8| c.labels.contains(l);
        let ok = if rt.nb_all { rt.nb_labels.iter().all(has) } else { rt.nb_labels.iter().any(has) };
        if !ok {
            return false;
        }
    }
    if rt.nb_exclude.iter().any(|l| c.labels.contains(l)) {
        return false;
    }
    if rt.nb_date != 0 {
        let Ok(made) = chrono::NaiveDateTime::parse_from_str(&c.created, "%Y-%m-%d %H:%M") else { return false };
        let today = chrono::Local::now().date_naive();
        let day = made.date();
        let ok = match rt.nb_date {
            1 => day == today,
            2 => day > today - chrono::Duration::days(7),
            3 => day > today - chrono::Duration::days(30),
            _ => {
                let from = chrono::NaiveDate::parse_from_str(rt.nb_from.trim(), "%Y-%m-%d").ok();
                let to = chrono::NaiveDate::parse_from_str(rt.nb_to.trim(), "%Y-%m-%d").ok();
                from.is_none_or(|f| day >= f) && to.is_none_or(|t| day <= t)
            }
        };
        if !ok {
            return false;
        }
    }
    true
}

fn filtering(rt: &crate::app::TabRt) -> bool {
    !rt.nb_query.trim().is_empty() || !rt.nb_labels.is_empty() || !rt.nb_exclude.is_empty() || rt.nb_date != 0
}

/// The filter's menu: labels wanted / left out, any or all, when made;
/// and the labels themselves (colour, name, more, fewer).
fn filter_menu(ui: &mut egui::Ui, rt: &mut crate::app::TabRt, labels: &mut Vec<LabelDef>, removed: &mut Option<u8>) {
    ui.set_min_width(300.0);
    ui.label(RichText::new(t("Метки", "Labels")).strong());
    for k in 0..labels.len() {
        let id = k as u8;
        ui.horizontal(|ui| {
            let c = color_of(labels, k);
            label_dot(ui, c, Dot::On, 13.0, egui::Sense::hover());
            ui.add_sized([110.0, 18.0], egui::Label::new(label_name(labels, k)).truncate());
            let state = if rt.nb_labels.contains(&id) { 1 } else if rt.nb_exclude.contains(&id) { 2 } else { 0 };
            let mut want = state;
            ui.selectable_value(&mut want, 0, "—").on_hover_text(t("Не важно", "Any"));
            ui.selectable_value(&mut want, 1, "✓").on_hover_text(t("Нужна", "Wanted"));
            ui.selectable_value(&mut want, 2, "✕").on_hover_text(t("Исключить", "Left out"));
            if want != state {
                rt.nb_labels.retain(|x| *x != id);
                rt.nb_exclude.retain(|x| *x != id);
                match want {
                    1 => rt.nb_labels.push(id),
                    2 => rt.nb_exclude.push(id),
                    _ => {}
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label(t("Нужные метки:", "Wanted labels:"));
        ui.radio_value(&mut rt.nb_all, false, t("любая", "any"));
        ui.radio_value(&mut rt.nb_all, true, t("все", "all"));
    });
    ui.separator();
    ui.label(RichText::new(t("Создан", "Made")).strong());
    ui.horizontal_wrapped(|ui| {
        for (v, n) in [(0u8, t("когда угодно", "any time")), (1, t("сегодня", "today")), (2, t("за 7 дней", "in 7 days")), (3, t("за 30 дней", "in 30 days")), (4, t("между датами", "between dates"))] {
            ui.selectable_value(&mut rt.nb_date, v, n);
        }
    });
    if rt.nb_date == 4 {
        ui.horizontal(|ui| {
            ui.label(t("с", "from"));
            ui.add(egui::TextEdit::singleline(&mut rt.nb_from).hint_text("2026-11-01").desired_width(90.0));
            ui.label(t("по", "to"));
            ui.add(egui::TextEdit::singleline(&mut rt.nb_to).hint_text("2026-11-30").desired_width(90.0));
        });
    }
    if ui.button(t("Сбросить фильтр", "Clear the filter")).clicked() {
        rt.nb_query.clear();
        rt.nb_labels.clear();
        rt.nb_exclude.clear();
        rt.nb_date = 0;
    }
    ui.separator();
    ui.label(RichText::new(t("Настройка меток", "The labels")).strong());
    for k in 0..labels.len() {
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut labels[k].color);
            let mut name = label_name(labels, k);
            if ui.add(egui::TextEdit::singleline(&mut name).desired_width(150.0)).changed() {
                labels[k].name = name;
            }
            if labels.len() > 1 && ui.small_button("🗑").on_hover_text(t("Убрать метку (и с блоков)", "Remove the label (from the cells too)")).clicked() {
                *removed = Some(k as u8);
            }
        });
    }
    if ui.button(format!("➕ {}", t("Метка", "Label"))).clicked() {
        let palette = [[0, 150, 136], [233, 30, 99], [121, 85, 72], [63, 81, 181]];
        labels.push(LabelDef { name: format!("{} {}", t("Метка", "Label"), labels.len() + 1), color: palette[labels.len() % palette.len()] });
    }
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
            // The cells' filter: what they hold, labels (a click: wanted,
            // again: left out, again: any), more in its menu.
            let tab_id = app.tab().id.clone();
            let mut labels = app.settings.labels.clone();
            let mut removed: Option<u8> = None;
            let rt = app.rt.entry(tab_id).or_default();
            let active = filtering(rt);
            let fb = match crate::icons::image(crate::icons::Ic::Filter, 15.0, if active { ui.visuals().hyperlink_color } else { ui.visuals().text_color() }) {
                Some(img) => egui::Button::image(img),
                None => egui::Button::new("⛃"),
            };
            let fr = ui.add(fb.selected(active)).on_hover_text(t("Фильтр блоков: метки, исключения, дата создания; настройка меток", "The cells' filter: labels, left out, when made; the labels themselves"));
            egui::Popup::menu(&fr).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| filter_menu(ui, rt, &mut labels, &mut removed));
            // The labels' dots: folded to a little stack (and those in use)
            // until the pointer comes over them.
            let open_id = ui.id().with("nb-chips-open");
            let unfolded: bool = ui.ctx().data(|d| d.get_temp(open_id)).unwrap_or(false);
            let chips = ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for k in (0..labels.len()).rev() {
                    let id = k as u8;
                    let dot = if rt.nb_labels.contains(&id) { Dot::On } else if rt.nb_exclude.contains(&id) { Dot::Out } else { Dot::Off };
                    if !unfolded && dot == Dot::Off {
                        continue;
                    }
                    if label_dot(ui, color_of(&labels, k), dot, 13.0, egui::Sense::click()).on_hover_text(format!("{} · {}", label_name(&labels, k), t("щелчок: нужна → исключить → не важно", "click: wanted → left out → any"))).clicked() {
                        rt.nb_labels.retain(|x| *x != id);
                        rt.nb_exclude.retain(|x| *x != id);
                        match dot {
                            Dot::Off => rt.nb_labels.push(id),
                            Dot::On => rt.nb_exclude.push(id),
                            Dot::Out => {}
                        }
                    }
                }
                if !unfolded {
                    // Three colours overlapping: "labels here".
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 14.0), egui::Sense::hover());
                    for (j, k) in (0..labels.len().min(3)).rev().enumerate() {
                        let c = color_of(&labels, k).unwrap_or(Color32::GRAY);
                        let at = egui::pos2(rect.right() - 6.0 - j as f32 * 6.0, rect.center().y);
                        ui.painter().circle_filled(at, 5.0, c);
                        ui.painter().circle_stroke(at, 5.0, Stroke::new(1.0, ui.visuals().panel_fill));
                    }
                }
            });
            let over = ui.rect_contains_pointer(chips.response.rect.expand(6.0));
            if over != unfolded {
                ui.ctx().data_mut(|d| d.insert_temp(open_id, over));
                ui.ctx().request_repaint();
            }
            ui.add(egui::TextEdit::singleline(&mut rt.nb_query).hint_text(t("🔎 фильтр блоков", "🔎 filter cells")).desired_width(150.0))
                .on_hover_text(t("Подходящие блоки раскрыты, остальные скрыты", "The cells that fit are open, the rest hidden"));
            if labels != app.settings.labels || removed.is_some() {
                if let Some(k) = removed {
                    labels.remove(k as usize);
                    // Off every cell; the later ones a place back.
                    for ws in app.session.workspaces.iter_mut() {
                        for tb in ws.tabs.iter_mut() {
                            if let Some(nb) = tb.notebook.as_mut() {
                                for c in nb.cells.iter_mut() {
                                    c.labels.retain(|x| *x != k);
                                    for x in c.labels.iter_mut() {
                                        if *x > k {
                                            *x -= 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    for r in app.rt.values_mut() {
                        r.nb_labels.clear();
                        r.nb_exclude.clear();
                    }
                    app.touch();
                }
                app.settings.labels = labels;
                app.save_settings();
            }
            // Only in the room left (cut short, never over the buttons).
            ui.add(egui::Label::new(RichText::new(t("Ctrl+Enter — выполнить блок · блок из одного WITH отдаёт свои CTE блокам ниже", "Ctrl+Enter: run the cell · a cell of a WITH alone lends its CTEs to the cells below")).weak().small()).truncate());
        });
    });
    ui.add_space(4.0);
    let tab = app.tab().clone();
    let Some(nb) = tab.notebook.as_ref() else { return actions };
    // The filter: what the cells hold, their labels, when made.
    let frt = app.rt.get(&tab.id).map(|r| crate::app::TabRt { nb_query: r.nb_query.clone(), nb_labels: r.nb_labels.clone(), nb_exclude: r.nb_exclude.clone(), nb_all: r.nb_all, nb_date: r.nb_date, nb_from: r.nb_from.clone(), nb_to: r.nb_to.clone(), ..Default::default() }).unwrap_or_default();
    let filtering = filtering(&frt);
    let nb_q = frt.nb_query.trim().to_lowercase();
    let fits = |c: &crate::config::Cell| fits(&frt, c);
    let labels = app.settings.labels.clone();
    let conn = app.tab_conn(app.ws(), &tab);
    let dialect = app.dialect_of(conn.as_deref());
    let rels = conn.as_deref().and_then(|c| app.live.get(c)).map(|l| l.rels.clone()).unwrap_or_default();
    let routines = conn.as_deref().and_then(|c| app.live.get(c)).map(|l| l.routines.clone()).unwrap_or_default();
    let settings = app.settings.clone();
    let accent = crate::editor::rgb(app.ws().color);
    let dark = ui.visuals().dark_mode;
    let focus = app.rt.get(&tab.id).and_then(|r| r.cell_focus.clone());
    // Being typed in another window: shown as it goes, not changed here.
    if app.sync.held_elsewhere(&tab.id) && crate::ui::held_bar(ui) {
        app.sync.take_over(&tab.id);
        app.save_session();
    }
    let held = app.sync.held_elsewhere(&tab.id);
    // Markdown cells being edited (the rest shown), each until switched back.
    let md_edit = app.rt.get(&tab.id).map(|r| r.md_edit.clone()).unwrap_or_default();
    let mut new_focus: Option<String> = None;
    let mut md_now = md_edit.clone();
    let n = nb.cells.len();
    // (The notebook's scroll, as egui names it: moved along when an output's
    // top edge is pulled.)
    let nb_scroll = ui.make_persistent_id(egui::IdSalt::new(("notebook", &tab.id)));
    let mut pull = 0.0f32;
    // A cell being dragged by its grip, the one let go; where the cells are.
    let mut dragging: Option<usize> = None;
    let mut dropped: Option<usize> = None;
    // (As of the drag's start: the cell and the pointer's height then.)
    let lift_id = egui::Id::new(("nb-lift", &tab.id));
    let lift: Option<(usize, f32)> = ctx.data(|d| d.get_temp(lift_id));
    // Where the dragged cell would go (as of the last frame) and its height.
    let drop_id = egui::Id::new(("nb-drop", &tab.id));
    let drop_at: Option<(usize, f32)> = if lift.is_some() { ctx.data(|d| d.get_temp(drop_id)) } else { None };
    let mut rects: Vec<egui::Rect> = Vec::new();
    egui::ScrollArea::vertical().id_salt(("notebook", &tab.id)).auto_shrink([false, false]).show(ui, |ui| {
        // CTEs of WITH cells above, for the hints of the one being typed in.
        let mut known = crate::sql::notebook::Known::default();
        for (i, cell) in nb.cells.iter().enumerate() {
            let lent = if cell.kind == CellKind::Sql && focus.as_deref() == Some(cell.id.as_str()) { known.relations(dialect, &rels) } else { Vec::new() };
            if cell.kind == CellKind::Sql && cell.text.trim_start().get(..4).is_some_and(|w| w.eq_ignore_ascii_case("with")) {
                known.learn(cell.text.trim().trim_end_matches(';'), dialect);
            }
            // Filtered: the cells that don't fit hidden, those that do open.
            if filtering && !fits(cell) {
                continue;
            }
            app.restore_cell(&tab.id, cell);
            let key = App::cell_key(&tab.id, &cell.id);
            if !app.find.open {
                let marks = if filtering { crate::search::find_in(&cell.text, &nb_q, false, false) } else { Vec::new() };
                if let Some(rt) = app.rt.get_mut(&key) {
                    rt.ed.marks = marks;
                    rt.ed.mark_now = None;
                }
            }
            let focused = focus.as_deref() == Some(cell.id.as_str());
            let stroke = if focused { Stroke::new(1.0, accent) } else { Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color) };
            // How its last run went, on its header.
            let shade = flag_shade(app, &key, focused, &ctx);
            // Being dragged: folded to its header for the move.
            let carried = lift.is_some_and(|(k, _)| k == i);
            // (Its header stuck on top: the band's top, where its edge closes.)
            let mut stuck_top: Option<f32> = None;
            let mut draw = |ui: &mut egui::Ui| egui::Frame::NONE.corner_radius(RADIUS).inner_margin(egui::Margin::symmetric(8, 6)).fill(ui.visuals().panel_fill).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let running = app.rt.get(&key).is_some_and(|r| r.run.is_some());
                let has_out = app.rt.get(&key).is_some_and(|r| !r.results.is_empty() || !r.output.is_empty() || !r.log.is_empty());
                let inner = ui.max_rect();
                // Its header's place is kept; the header is drawn after the body
                // (over it), at the top of the view while the cell's scrolled
                // up under it.
                let head_id = egui::Id::new(("nb-head-h", &cell.id));
                let head_h: f32 = ctx.data(|d| d.get_temp(head_id)).unwrap_or(26.0);
                let slot = ui.allocate_exact_size(egui::vec2(ui.available_width(), head_h), egui::Sense::hover()).0;
                (|ui: &mut egui::Ui| {
                    if (cell.collapsed || carried) && !filtering {
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
                            let cx = editor::Ctx { dialect, settings: &settings, dark, issues: &issues, server_err: server_err.as_ref(), rels: &rels, lent: &lent, routines: &routines, rows, read_only: held, outer_wheel: true };
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
                                let md_id = egui::Id::new(("md", &cell.id));
                                let r = ui.add(egui::TextEdit::multiline(&mut text).id(md_id).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(lines).hint_text(t("# Заголовок, **жирный**, *курсив*, `код`, - список   (правый щелчок — ещё)", "# Heading, **bold**, *italic*, `code`, - list   (right click: more)")));
                                let mut changed = r.changed();
                                // What's picked as of before a right click (a press
                                // moves egui's caret): the menu works on it.
                                let sel_key = md_id.with("sel");
                                if !ctx.input(|i| i.pointer.secondary_pressed() || i.pointer.secondary_down()) {
                                    if let Some(cr) = egui::TextEdit::load_state(&ctx, md_id).and_then(|s| s.cursor.char_range()) {
                                        ctx.data_mut(|d| d.insert_temp(sel_key, (cr.primary.index.0, cr.secondary.index.0)));
                                    }
                                }
                                r.context_menu(|ui| {
                                    let (p, q) = ctx.data(|d| d.get_temp::<(usize, usize)>(sel_key)).unwrap_or((text.chars().count(), text.chars().count()));
                                    let byte = |c: usize| text.char_indices().nth(c).map_or(text.len(), |(b, _)| b);
                                    let (a, b) = (byte(p.min(q)), byte(p.max(q)));
                                    for (k, group) in crate::md::MENU.iter().enumerate() {
                                        if k > 0 {
                                            ui.separator();
                                        }
                                        for op in group.iter() {
                                            let row = ui.horizontal(|ui| {
                                                let r = ui.button(op.label());
                                                ui.label(RichText::new(op.hint()).monospace().weak());
                                                r
                                            });
                                            if row.inner.clicked() {
                                                let (s, x, y) = crate::md::apply(&text, a, b, *op);
                                                let ch = |b: usize| s[..b].chars().count();
                                                let (cx, cy) = (ch(x), ch(y));
                                                text = s;
                                                changed = true;
                                                let mut st = egui::TextEdit::load_state(&ctx, md_id).unwrap_or_default();
                                                st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(cx), egui::text::CCursor::new(cy))));
                                                st.store(&ctx, md_id);
                                                ctx.memory_mut(|m| m.request_focus(md_id));
                                                ui.close();
                                            }
                                        }
                                    }
                                });
                                if changed {
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
                })(ui);
                let bottom = ui.min_rect().bottom();
                let top = if lift.is_none() { slot.top().max(ui.clip_rect().top() + 7.0).min(bottom - head_h) } else { slot.top() }.max(slot.top());
                let stuck = top > slot.top() + 0.5;
                let hrect = egui::Rect::from_min_size(egui::pos2(slot.left(), top), slot.size());
                // (Its own id: its widgets keep theirs whatever the body
                // drew before it — a grip mid-drag must.)
                let head = ui.scope_builder(egui::UiBuilder::new().id(egui::Id::new(("nb-head", &tab.id, &cell.id))).max_rect(hrect), |ui| {
                    let band = egui::Rect::from_min_max(egui::pos2(inner.left() - 8.0, top - 6.0), egui::pos2(inner.right() + 8.0, top + head_h + 3.0));
                    let mut under = Vec::new();
                    if stuck {
                        stuck_top = Some(band.top());
                        // (Nothing of the body above it or in its rounded
                        // corners: the view's ground there.)
                        let above = egui::Rect::from_min_max(egui::pos2(band.left() - 1.0, ui.clip_rect().top()), egui::pos2(band.right() + 1.0, band.top() + RADIUS as f32));
                        under.push(egui::Shape::rect_filled(above, 0.0, ui.visuals().panel_fill));
                        // Over the body: opaque, a shade under it, and the body
                        // beneath out of the pointer's reach.
                        under.push(egui::Shape::rect_filled(band, egui::CornerRadius { nw: RADIUS, ne: RADIUS, sw: 0, se: 0 }, ui.visuals().panel_fill));
                        let shadow = egui::Rect::from_min_max(egui::pos2(band.left(), band.bottom()), egui::pos2(band.right(), band.bottom() + 3.0));
                        under.push(egui::Shape::rect_filled(shadow, 0.0, Color32::from_black_alpha(if dark { 60 } else { 22 })));
                        ui.interact(band, egui::Id::new(("nb-head-cover", &cell.id)), egui::Sense::click());
                    }
                    if let Some((c, k)) = shade {
                        let round = egui::CornerRadius { nw: RADIUS, ne: RADIUS, sw: 0, se: 0 };
                        // (A hairline under it: a pixel.)
                        let px = 1.0 / ui.ctx().pixels_per_point();
                        let line = egui::Rect::from_min_max(egui::pos2(band.left(), band.bottom() - px), band.max);
                        under.push(egui::Shape::rect_filled(band, round, c.gamma_multiply(if dark { 0.22 } else { 0.16 } * k)));
                        under.push(egui::Shape::rect_filled(line, 0.0, c.gamma_multiply(0.85 * k)));
                    }
                    ui.painter().extend(under);
                    // Its header: kind, run, what it gave; moving, adding, deleting.
                    // (Too narrow for both: the right group on a line of its own.)
                    let right_id = egui::Id::new(("nb-head-right", &cell.id));
                    let right_w: f32 = ctx.data(|d| d.get_temp(right_id)).unwrap_or(0.0);
                    let r = ui.horizontal_wrapped(|ui| {
                        // A grip: dragged, the cell goes up or down.
                        // (Not while filtered: the cells hidden would be in the way.)
                        let (gr, grip) = ui.allocate_exact_size(egui::vec2(14.0, 24.0), if filtering { egui::Sense::hover() } else { egui::Sense::drag() });
                        grip.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "⠿"));
                        let gc = if grip.hovered() || grip.dragged() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
                        if !crate::icons::paint(ui, gr, crate::icons::Ic::Grip, 16.0, gc) {
                            ui.painter().text(gr.center(), egui::Align2::CENTER_CENTER, "⠿", egui::FontId::proportional(14.0), gc);
                        }
                        let grip = grip.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text(t("Тянуть — переставить блок", "Drag: move the cell"));
                        if grip.drag_started() {
                            if let Some(p) = ctx.pointer_interact_pos() {
                                ctx.data_mut(|d| d.insert_temp(lift_id, (i, p.y)));
                            }
                        }
                        if grip.dragged() {
                            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                            dragging = Some(i);
                        }
                        if grip.drag_stopped() {
                            dropped = Some(i);
                            ctx.data_mut(|d| d.remove::<(usize, f32)>(lift_id));
                        }
                        let fold = ui.add(egui::Button::new("").min_size(egui::vec2(24.0, 24.0)));
                        crate::ui::paint_chevron(ui.painter(), fold.rect.center(), 11.0, if cell.collapsed { 0.0 } else { 1.0 }, if fold.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() });
                        let fold = fold.on_hover_text(if cell.collapsed { t("Развернуть блок", "Unfold the cell") } else { t("Свернуть блок", "Fold the cell") });
                        if fold.clicked() {
                            ops.push(Op::Fold(i));
                        }
                        ui.label(RichText::new(format!("[{}]", i + 1)).weak().small().monospace());
                        // Its labels (a click: the menu to set them) and when it was made.
                        // (The dots only draw; their row as a whole takes the click.)
                        let shown = ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for &k in &cell.labels {
                                label_dot(ui, color_of(&labels, k as usize), Dot::On, 11.0, egui::Sense::hover());
                            }
                            if cell.labels.is_empty() {
                                label_dot(ui, None, Dot::Off, 11.0, egui::Sense::hover());
                            }
                        });
                        let r = ui.interact(shown.response.rect.expand(2.0), ui.id().with(("cell-labels", &cell.id)), egui::Sense::click());
                        if r.hovered() {
                            ui.painter().rect_stroke(shown.response.rect.expand(2.0), 4.0, Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color), egui::StrokeKind::Inside);
                        }
                        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "labels"));
                        let r = r.on_hover_text(t("Метки блока", "The cell's labels"));
                        egui::Popup::menu(&r).show(|ui| {
                            for k in 0..labels.len() {
                                let mut on = cell.labels.contains(&(k as u8));
                                ui.horizontal(|ui| {
                                    label_dot(ui, color_of(&labels, k), Dot::On, 12.0, egui::Sense::hover());
                                    if ui.checkbox(&mut on, label_name(&labels, k)).changed() {
                                        ops.push(Op::Label(i, k as u8));
                                    }
                                });
                            }
                        });
                        if !cell.created.is_empty() {
                            let when = chrono::NaiveDateTime::parse_from_str(&cell.created, "%Y-%m-%d %H:%M").map(|d| d.format("%d.%m %H:%M").to_string()).unwrap_or_else(|_| cell.created.clone());
                            ui.label(RichText::new(when).weak().small()).on_hover_text(format!("{} {}", t("Создан", "Made"), cell.created));
                        }
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
                            if ui.add_enabled(!running, tinted(lbl("🔍", t("План", "Plan")), crate::ui::BLUE)).on_hover_text(t("План запроса блока   Ctrl+E / F7", "The cell's plan   Ctrl+E / F7")).clicked() {
                                ops.push(Op::Explain(cell.id.clone()));
                            }
                            let loading = app.rt.get(&key).is_some_and(|r| r.results.iter().any(|v| v.loading.is_some()));
                            let run_now = app.rt.get(&key).and_then(|r| r.run.as_ref());
                            if run_now.is_some_and(crate::app::Running::hung) {
                                // The server didn't answer the stop.
                                if ui.add(crate::ui::force_button(lbl("⚡", t("Прервать", "Kill")))).on_hover_text(crate::ui::force_tip()).clicked() {
                                    ops.push(Op::Force(key.clone()));
                                }
                            } else if running || loading {
                                let stopping = run_now.is_some_and(|r| r.stop_asked.is_some());
                                let text = if stopping { t("Останавливается…", "Stopping…") } else { t("Стоп", "Stop") };
                                if ui.add(tinted(lbl("■", text), crate::ui::RED)).on_hover_text(t("Остановить блок   Shift+Esc — все блоки", "Stop the cell   Shift+Esc: every cell")).clicked() {
                                    ops.push(Op::Stop(key.clone()));
                                }
                                if stopping {
                                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                                }
                            }
                            if let Some(since) = app.rt.get(&key).and_then(|r| r.run.as_ref()).map(|r| r.since) {
                                ui.add(egui::Spinner::new().size(14.0));
                                crate::app::stopwatch_label(ui, since, true);
                            } else if let Some((took, cancelled)) = app.rt.get(&key).and_then(|r| r.last_run) {
                                crate::app::last_run_label(ui, took, cancelled);
                            }
                            if crate::sql::notebook::parse_with(cell.text.trim().trim_end_matches(';'), dialect).is_some_and(|w| w.main.is_empty()) {
                                ui.label(RichText::new(if narrow { "WITH" } else { t("WITH — для блоков ниже", "WITH — for the cells below") }).small().color(crate::ui::TEAL)).on_hover_text(t("Блок из одного WITH: его CTE получат блоки ниже", "A WITH alone: the cells below get its CTEs"));
                            }
                        }
                        // (What's left of the row: the wrapped layout's
                        // available width is the whole line.)
                        if ui.max_rect().right() - ui.cursor().left() < right_w + 8.0 {
                            ui.end_row();
                        }
                        let right = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                            // Its text to the clipboard; a SQL cell's own formatting.
                            if ui.add(tinted("🗐".into(), crate::ui::BLUE).min_size(egui::vec2(28.0, 24.0))).on_hover_text(t("Скопировать текст блока", "Copy the cell's text")).clicked() {
                                ctx.copy_text(cell.text.clone());
                            }
                            if cell.kind == CellKind::Sql && ui.add(tinted("✨".into(), crate::ui::VIOLET).min_size(egui::vec2(28.0, 24.0))).on_hover_text(t("Форматировать этот блок", "Format this cell")).clicked() {
                                ops.push(Op::Format(i));
                            }
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
                            ui.min_rect().width()
                        });
                        if (right.inner - right_w).abs() > 0.5 {
                            ctx.data_mut(|d| d.insert_temp(right_id, right.inner));
                            ctx.request_repaint();
                        }
                    }).response;
                    r
                })
                .inner;
                if (head.rect.height() - head_h).abs() > 0.5 {
                    ctx.data_mut(|d| d.insert_temp(head_id, head.rect.height()));
                    ctx.request_repaint();
                }
            });
            // The cell being dragged: lifted onto a layer of its own, moved
            // with the pointer (its place stays empty meanwhile).
            let lifted = lift.filter(|(k, _)| *k == i);
            // Cells between where the dragged one was and where it'd go:
            // moved aside (smoothly) to make room for it.
            let aside = match (lift, drop_at) {
                (Some((li, _)), Some((to, h))) if lifted.is_none() => {
                    if to <= i && i < li {
                        h
                    } else if li < i && i < to {
                        -h
                    } else {
                        0.0
                    }
                }
                _ => 0.0,
            };
            let aside = ctx.animate_value_with_time(egui::Id::new(("nb-aside", &cell.id)), aside, if lift.is_some() { 0.14 } else { 0.0 });
            // (Drawn the same way whatever its layer: its widgets keep their
            // ids, the drag goes on.)
            let on = if lifted.is_some() {
                egui::LayerId::new(egui::Order::Tooltip, egui::Id::new(("nb-lifted", &tab.id)))
            } else if aside.abs() > 0.5 {
                egui::LayerId::new(egui::Order::Middle, egui::Id::new(("nb-aside", &cell.id)))
            } else {
                ui.layer_id()
            };
            let framed = ui.scope_builder(egui::UiBuilder::new().layer_id(on), |ui| draw(ui)).inner;
            rects.push(framed.response.rect);
            // Its edge (drawn twice: the rounded corners as strong as the
            // sides), on its layer before that's moved.
            let mut edge = framed.response.rect.shrink(0.5);
            if let Some(t) = stuck_top {
                // Stuck: closed (and rounded) along the header's top.
                edge.min.y = edge.min.y.max(t + 0.5);
            }
            let lp = ctx.layer_painter(on);
            match lifted {
                Some((_, y0)) => {
                    let dy = ctx.pointer_interact_pos().map_or(0.0, |p| p.y - y0);
                    lp.rect_stroke(edge, RADIUS, Stroke::new(1.5, accent), egui::StrokeKind::Middle);
                    ctx.transform_layer_shapes(on, egui::emath::TSTransform::from_translation(egui::vec2(0.0, dy)));
                }
                None => {
                    let lp = lp.with_clip_rect(ui.clip_rect());
                    for _ in 0..2 {
                        lp.rect_stroke(edge, RADIUS, stroke, egui::StrokeKind::Middle);
                    }
                    if on != ui.layer_id() {
                        ctx.transform_layer_shapes(on, egui::emath::TSTransform::from_translation(egui::vec2(0.0, aside)));
                    }
                }
            }
            ui.add_space(6.0);
        }
        // Dragging a cell: where it'd go (a line between cells); the notebook
        // scrolls when the pointer is near its top or bottom.
        let target = |y: f32| rects.iter().filter(|r| r.center().y < y).count();
        if let (Some(from), Some(p)) = (dragging.or(dropped), ctx.pointer_interact_pos().or(ctx.pointer_hover_pos())) {
            let to = target(p.y);
            if dropped.is_some() {
                ops.push(Op::Move(from, to));
                ctx.data_mut(|d| d.remove::<(usize, f32)>(drop_id));
            } else if let Some(r) = rects.get(from) {
                // Where it'd go, and how far the others move aside for it.
                ctx.data_mut(|d| d.insert_temp(drop_id, (to, r.height() + 6.0)));
            }
            if dragging.is_some() {
                let clip = ui.clip_rect();
                let edge = 40.0;
                let dy = if p.y < clip.top() + edge { 8.0 } else if p.y > clip.bottom() - edge { -8.0 } else { 0.0 };
                if dy != 0.0 {
                    ui.scroll_with_delta(egui::vec2(0.0, dy));
                    ctx.request_repaint();
                }
            }
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
        // Room below: the last cell can be scrolled up to the top.
        let tail = rects.last().map_or(0.0, |r| r.height());
        ui.add_space((ui.clip_rect().height() - tail - 40.0).max(40.0));
        // The wheel a cell's selection passed on (see editor::Ctx::outer_wheel):
        // taken here, after every scroll area inside the cells.
        if let Some(d) = ctx.data_mut(|m| m.remove_temp::<egui::Vec2>(crate::editor::outer_wheel_id())) {
            ui.scroll_with_delta_animation(d, egui::style::ScrollAnimation::none());
        }
        if let Some(r) = ctx.data_mut(|m| m.remove_temp::<Option<egui::Rect>>(crate::editor::outer_caret_id()).flatten()) {
            ui.scroll_to_rect_animation(r, None, egui::style::ScrollAnimation::none());
        }
    });
    // (Where it's scrolled to, for the tests.)
    #[cfg(test)]
    if let Some(st) = egui::scroll_area::State::load(ui.ctx(), nb_scroll) {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(("nb-offset", &tab.id)), st.offset.y));
    }
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
/// A run's colour stays while its cell has the focus, and a while after
/// it's gone; then it fades.
const FLAG_HOLD: f32 = 4.0;
const FLAG_FADE: f32 = 2.0;

pub const YELLOW: Color32 = Color32::from_rgb(240, 180, 30);

/// The colour of the cell's last run and how strong it is now.
fn flag_shade(app: &mut App, key: &str, focused: bool, ctx: &egui::Context) -> Option<(Color32, f32)> {
    use crate::app::Flag;
    let rt = app.rt.get_mut(key)?;
    let f = rt.flag.as_mut()?;
    let c = match f.kind {
        Flag::Running => crate::ui::BLUE,
        Flag::Ok => crate::ui::GREEN,
        Flag::Warn => YELLOW,
        Flag::Error => crate::ui::RED,
    };
    if f.kind == Flag::Running {
        // A slow breath while it runs.
        ctx.request_repaint();
        let t = ctx.input(|i| i.time) as f32;
        return Some((c, 0.8 + 0.2 * (t * 3.0).sin()));
    }
    if focused {
        f.quiet = None;
        return Some((c, 1.0));
    }
    let e = f.quiet.get_or_insert_with(std::time::Instant::now).elapsed().as_secs_f32();
    if e < FLAG_HOLD {
        ctx.request_repaint_after(std::time::Duration::from_secs_f32(FLAG_HOLD - e + 0.01));
        return Some((c, 1.0));
    }
    let k = 1.0 - (e - FLAG_HOLD) / FLAG_FADE;
    if k <= 0.0 {
        rt.flag = None;
        return None;
    }
    ctx.request_repaint();
    Some((c, k))
}

fn ops_text(app: &mut App, tab: &str, cell: &str, text: String) {
    if app.sync.held_elsewhere(tab) {
        return;
    }
    let wi = app.session.active;
    let ws = &mut app.session.workspaces[wi];
    let Some(tb) = ws.tabs.iter_mut().find(|t| t.id == tab) else { return };
    if let Some(c) = tb.notebook.as_mut().and_then(|n| n.cells.iter_mut().find(|c| c.id == cell)) {
        c.text = text;
    }
    if tb.path.is_some() {
        tb.modified = true;
    }
    app.hold_tab(tab);
}

fn apply(app: &mut App, ctx: &egui::Context, op: Op) {
    let tab_id = app.tab().id.clone();
    if let Op::Run(c) | Op::Explain(c) = &op {
        app.rt.entry(tab_id.clone()).or_default().cell_focus = Some(c.clone());
        app.run_cells_as(ctx, Some(c.clone()), matches!(op, Op::Explain(_)));
        return;
    }
    if let Op::Stop(key) = &op {
        app.cancel_key(key);
        return;
    }
    if let Op::Force(key) = &op {
        app.force_key(key);
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
    if let Op::Format(i) = op {
        app.format_cell(i);
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
        Op::Label(i, k) => {
            if let Some(c) = nb.cells.get_mut(i) {
                if c.labels.contains(&k) {
                    c.labels.retain(|x| *x != k);
                } else {
                    c.labels.push(k);
                    c.labels.sort_unstable();
                }
            }
        }
        Op::Move(from, to) if from < nb.cells.len() && to <= nb.cells.len() && to != from && to != from + 1 => {
            let c = nb.cells.remove(from);
            nb.cells.insert(if to > from { to - 1 } else { to }, c);
        }
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
    let mut strike = false;
    let mut chars = text.char_indices().peekable();
    let mut run = String::new();
    let push = |job: &mut LayoutJob, s: &mut String, bold: bool, italic: bool, strike: bool| {
        if s.is_empty() {
            return;
        }
        let font = if bold { FontId::new(size, egui::FontFamily::Name("bold".into())) } else { FontId::proportional(size) };
        let c = if bold { strong_c } else { base };
        let mut f = egui::TextFormat::simple(font, c);
        f.italics = italic;
        if strike {
            f.strikethrough = Stroke::new(1.0, c);
        }
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
        } else if rest.starts_with("~~") {
            push(job, &mut run, bold, italic, strike);
            strike = !strike;
            chars.next();
        } else if let Some(mark) = ["**", "__"].into_iter().find(|m| rest.starts_with(m)).filter(|m| emphasis(text, i, m, bold_by == Some(*m))) {
            push(job, &mut run, bold, italic, strike);
            bold_by = if bold_by == Some(mark) { None } else { Some(mark) };
            bold = !bold;
            chars.next();
        } else if let Some(mark) = ["*", "_"].into_iter().find(|m| rest.starts_with(m)).filter(|m| emphasis(text, i, m, italic_by == Some(*m))) {
            push(job, &mut run, bold, italic, strike);
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
                push(job, &mut run, bold, italic, strike);
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
                    push(job, &mut run, bold, italic, strike);
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
    push(job, &mut run, bold, italic, strike);
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
    let all: Vec<&str> = text.lines().collect();
    let mut at = 0;
    while at < all.len() {
        let line = all[at];
        at += 1;
        // A table: its lines of | cells | together (the --- line under the
        // header left out).
        if code.is_none() && line.trim_start().starts_with('|') {
            let mut rows: Vec<Vec<String>> = Vec::new();
            let mut l = line;
            loop {
                let cells: Vec<String> = l.trim().trim_matches('|').split('|').map(|c| c.trim().to_string()).collect();
                if !cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':'))) {
                    rows.push(cells);
                }
                if at < all.len() && all[at].trim_start().starts_with('|') {
                    l = all[at];
                    at += 1;
                } else {
                    break;
                }
            }
            egui::Frame::NONE.stroke(Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)).corner_radius(4).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                egui::Grid::new(ui.next_auto_id()).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
                    for (k, r) in rows.iter().enumerate() {
                        for c in r {
                            let mut job = LayoutJob::default();
                            inline(ui, c, body, k == 0, &mut job);
                            ui.label(job);
                        }
                        ui.end_row();
                    }
                });
            });
            continue;
        }
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
        } else if let Some((done, rest)) = ["- [ ] ", "- [x] ", "- [X] "].iter().find_map(|m| l.trim_start().strip_prefix(m).map(|r| (m.as_bytes()[3] != b' ', r))) {
            // A checklist's item: its box, ticked or not.
            ui.horizontal_wrapped(|ui| {
                ui.add_space(6.0);
                let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                let v = ui.visuals();
                ui.painter().rect_stroke(r.shrink(1.0), 3.0, Stroke::new(1.2, v.text_color()), egui::StrokeKind::Inside);
                if done {
                    let p = |x: f32, y: f32| egui::pos2(r.left() + x * r.width(), r.top() + y * r.height());
                    ui.painter().add(egui::Shape::line(vec![p(0.25, 0.52), p(0.43, 0.7), p(0.76, 0.32)], Stroke::new(1.8, v.hyperlink_color)));
                }
                inline(ui, rest, body, false, &mut job);
                ui.label(job);
            });
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
