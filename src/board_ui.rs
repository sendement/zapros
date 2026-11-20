//! A board's tab: the tools above, the canvas (panned by dragging empty
//! space or with the middle button, the wheel; zoomed with Ctrl+wheel),
//! what's picked set up on the right.

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::App;
use crate::board::{self, Board, Field, Item, Kind, Link, NbCell, Shape, CARD_LINES, FLOW_BOX_H, FLOW_GAP, HEAD_H, NOTE_LINE_H, ROW_H};
use crate::config::Rgb;
use crate::db::ObjKind;
use crate::editor::rgb;
use crate::i18n::t;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Rect,
    Ellipse,
    Diamond,
    Text,
    Brush,
    Entity,
    Arrow,
}

impl Tool {
    const ALL: [Tool; 8] = [Tool::Select, Tool::Rect, Tool::Ellipse, Tool::Diamond, Tool::Text, Tool::Brush, Tool::Entity, Tool::Arrow];

    fn tip(self) -> &'static str {
        match self {
            Tool::Select => t("Выбор и перемещение (V); пустое место тянется — холст едет, с Shift — рамка выделения", "Select and move (V); drag empty space to pan, with Shift to select by a box"),
            Tool::Rect => t("Рамка (R): протяните или щёлкните", "Frame (R): drag or click"),
            Tool::Ellipse => t("Овал (O)", "Ellipse (O)"),
            Tool::Diamond => t("Ромб — условие в блок-схеме (D)", "Diamond: a flowchart's condition (D)"),
            Tool::Text => t("Текст (T): щёлкните, где писать", "Text (T): click where to write"),
            Tool::Brush => t("Кисть (B)", "Brush (B)"),
            Tool::Entity => t("Таблица-сущность (E): имя, поля, описание", "Table entity (E): a name, fields, a note"),
            Tool::Arrow => t("Связь (A): от элемента к элементу; от поля таблицы к полю", "Link (A): from item to item; from a table's field to a field"),
        }
    }

    fn key(self) -> egui::Key {
        match self {
            Tool::Select => egui::Key::V,
            Tool::Rect => egui::Key::R,
            Tool::Ellipse => egui::Key::O,
            Tool::Diamond => egui::Key::D,
            Tool::Text => egui::Key::T,
            Tool::Brush => egui::Key::B,
            Tool::Entity => egui::Key::E,
            Tool::Arrow => egui::Key::A,
        }
    }
}

#[derive(Clone, Debug, Default)]
enum Gesture {
    #[default]
    None,
    Pan,
    /// Moving the picked items.
    Move,
    /// Resizing an item by its corner.
    Resize(u64),
    Draw(Vec<[f32; 2]>),
    /// A frame or an entity being drawn: from, to.
    Make(Pos2, Pos2),
    /// A link being drawn: from the item (and its field), to the pointer.
    Link(u64, Option<usize>, Pos2),
    /// Picking by a box.
    Band(Pos2, Pos2),
}

/// The boards' tools: one set for every board.
pub struct State {
    pub tool: Tool,
    pub color: Rgb,
    pub width: f32,
    pub sel: Vec<u64>,
    /// A link picked (instead of items).
    pub sel_link: Option<u64>,
    /// An item whose text is being typed on the canvas.
    pub edit: Option<u64>,
    /// Frames the text field still asks for the keys (the click that
    /// made it lets go meanwhile).
    edit_fresh: u8,
    gesture: Gesture,
    /// The main field of the properties gets the keys (next frame).
    focus_prop: bool,
    /// The search in "Add → database object".
    pub find: String,
}

impl Default for State {
    fn default() -> Self {
        Self { tool: Tool::Select, color: crate::config::COLORS[0], width: 3.0, sel: Vec::new(), sel_link: None, edit: None, edit_fresh: 0, gesture: Gesture::None, focus_prop: false, find: String::new() }
    }
}

/// Board units ⇄ the screen.
#[derive(Clone, Copy)]
struct View {
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
}

impl View {
    fn s(&self, p: Pos2) -> Pos2 {
        self.origin + (p.to_vec2() - self.pan) * self.zoom
    }

    fn w(&self, p: Pos2) -> Pos2 {
        (self.pan + (p - self.origin) / self.zoom).to_pos2()
    }

    fn rect(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.s(r.min), self.s(r.max))
    }
}

/// The tab of board `id`.
pub fn show(app: &mut App, ui: &mut egui::Ui, id: &str) {
    if app.boards.get(id).is_none() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| ui.label(RichText::new(t("Доска удалена", "The board was deleted")).weak()));
        return;
    }
    app.boards.last = Some(id.to_string());
    let mut open = app.boards.open.remove(id).unwrap();
    toolbar(app, ui, &mut open);
    ui.add_space(4.0);
    if !app.board_ui.sel.is_empty() || app.board_ui.sel_link.is_some() {
        let r = egui::Panel::right("board-props")
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 10, right: 0, top: 0, bottom: 0 }))
            .show_separator_line(false)
            .resizable(false)
            .exact_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("board-props").auto_shrink([false, false]).show(ui, |ui| props(app, ui, &mut open.board));
            });
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        ui.painter().vline(r.response.rect.left() + 3.0, r.response.rect.y_range(), stroke);
    }
    canvas(app, ui, &mut open);
    open.board.fit_heights();
    // A step when nothing's held or typed.
    let busy = !matches!(app.board_ui.gesture, Gesture::None) || ui.ctx().memory(|m| m.focused()).is_some_and(|f| ui.ctx().read_response(f).is_some_and(|r| r.changed()) || ui.input(|i| i.pointer.any_down()));
    if !busy && open.commit() {
        let _ = board::save(&open.board);
    }
    app.boards.open.insert(id.to_string(), open);
}

fn toolbar(app: &mut App, ui: &mut egui::Ui, open: &mut board::Open) {
    ui.horizontal(|ui| {
        let st = &mut app.board_ui;
        ui.spacing_mut().item_spacing.x = 3.0;
        for tool in Tool::ALL {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
            let on = st.tool == tool;
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tool.tip()));
            let v = ui.visuals();
            let fill = if on { v.selection.bg_fill.gamma_multiply(0.6) } else if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { Color32::TRANSPARENT };
            ui.painter().rect_filled(r, crate::ui::RADIUS, fill);
            tool_icon(ui.painter(), r.shrink(7.0), tool, if on || resp.hovered() { v.strong_text_color() } else { v.text_color() });
            if resp.on_hover_text(tool.tip()).clicked() {
                st.tool = tool;
                st.edit = None;
            }
        }
        ui.separator();
        // The colour: of what's drawn next, and of what's picked.
        for c in crate::config::COLORS.iter().take(8) {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
            ui.painter().circle_filled(r.center(), if st.color == *c { 8.5 } else { 6.5 }, rgb(*c));
            if st.color == *c {
                ui.painter().circle_stroke(r.center(), 9.5, Stroke::new(1.5, ui.visuals().strong_text_color()));
            }
            if resp.on_hover_text(t("Цвет", "Colour")).clicked() {
                st.color = *c;
                for it in open.board.items.iter_mut().filter(|i| st.sel.contains(&i.id)) {
                    it.color = *c;
                }
                if let Some(l) = open.board.links.iter_mut().find(|l| Some(l.id) == st.sel_link) {
                    l.color = *c;
                }
            }
        }
        if st.tool == Tool::Brush {
            ui.separator();
            for w in [2.0, 4.0, 8.0] {
                if ui.selectable_label(st.width == w, format!("{w:.0} px")).clicked() {
                    st.width = w;
                }
            }
        }
        ui.separator();
        let _ = st;
        let add = ui.menu_button(format!("➕ {}", t("Добавить", "Add")), |ui| add_menu(app, ui, open));
        add.response.on_hover_text(t("Ноутбук из вкладки или объект из базы (их можно и перетащить из дерева)", "A notebook from a tab or an object from a database (those can also be dragged from the tree)"));
        ui.separator();
        let undo = ui.add_enabled(!open.undo.is_empty(), egui::Button::new("↶")).on_hover_text(t("Отменить (Ctrl+Z)", "Undo (Ctrl+Z)"));
        let redo = ui.add_enabled(!open.redo.is_empty(), egui::Button::new("↷")).on_hover_text(t("Вернуть (Ctrl+Shift+Z)", "Redo (Ctrl+Shift+Z)"));
        if undo.clicked() {
            open.undo();
            let _ = board::save(&open.board);
        }
        if redo.clicked() {
            open.redo();
            let _ = board::save(&open.board);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("⤢").on_hover_text(t("Показать всё", "Show everything")).clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("board-fit"), true));
            }
            if ui.button("+").on_hover_text(t("Крупнее (Ctrl+колесо)", "Zoom in (Ctrl+wheel)")).clicked() {
                open.board.zoom = (open.board.zoom * 1.25).min(4.0);
            }
            ui.label(format!("{:.0}%", open.board.zoom * 100.0));
            if ui.button("−").on_hover_text(t("Мельче", "Zoom out")).clicked() {
                open.board.zoom = (open.board.zoom / 1.25).max(0.2);
            }
        });
    });
}

fn tool_icon(p: &egui::Painter, r: Rect, tool: Tool, c: Color32) {
    let s = Stroke::new(1.5, c);
    match tool {
        Tool::Select => {
            let pts = vec![r.left_top(), Pos2::new(r.left(), r.bottom() - 2.0), Pos2::new(r.left() + 4.0, r.bottom() - 5.0), Pos2::new(r.left() + 8.0, r.bottom()), Pos2::new(r.left() + 10.0, r.bottom() - 1.5), Pos2::new(r.left() + 6.0, r.bottom() - 6.5), Pos2::new(r.right() - 2.0, r.bottom() - 7.0)];
            p.add(egui::Shape::closed_line(pts, s));
        }
        Tool::Rect => {
            p.rect_stroke(r.shrink2(Vec2::new(0.0, 2.0)), 2.0, s, egui::StrokeKind::Middle);
        }
        Tool::Ellipse => {
            p.add(egui::Shape::ellipse_stroke(r.center(), Vec2::new(r.width() / 2.0, r.height() / 2.0 - 1.5), s));
        }
        Tool::Diamond => {
            p.add(egui::Shape::closed_line(vec![r.center_top(), r.right_center(), r.center_bottom(), r.left_center()], s));
        }
        Tool::Text => {
            p.text(r.center(), Align2::CENTER_CENTER, "T", FontId::proportional(16.0), c);
        }
        Tool::Brush => {
            let pts: Vec<Pos2> = (0..=12).map(|k| {
                let f = k as f32 / 12.0;
                Pos2::new(r.left() + r.width() * f, r.center().y + (f * 6.5).sin() * r.height() * 0.3)
            }).collect();
            p.add(egui::Shape::line(pts, Stroke::new(2.0, c)));
        }
        Tool::Entity => {
            p.rect_stroke(r, 2.0, s, egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(r.min, Pos2::new(r.right(), r.top() + 4.0)), 2.0, c);
            p.hline(r.x_range(), r.top() + 8.5, Stroke::new(1.0, c));
        }
        Tool::Arrow => {
            arrow(p, r.left_bottom(), r.right_top(), s, 5.0);
        }
    }
}

fn arrow(p: &egui::Painter, a: Pos2, b: Pos2, s: Stroke, head: f32) {
    p.line_segment([a, b], s);
    let d = (b - a).normalized();
    if d.length_sq() > 0.0 {
        let n = Vec2::new(-d.y, d.x);
        p.add(egui::Shape::convex_polygon(vec![b, b - d * head * 1.6 + n * head * 0.8, b - d * head * 1.6 - n * head * 0.8], s.color, Stroke::NONE));
    }
}

/// "Add": a notebook from the tabs, an object of a database from what the
/// tree has seen.
fn add_menu(app: &mut App, ui: &mut egui::Ui, open: &mut board::Open) {
    ui.set_min_width(300.0);
    ui.label(RichText::new(t("Ноутбук или скрипт из вкладки", "A notebook or script from a tab")).strong());
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("board-add-tabs").max_height(220.0).show(ui, |ui| {
        for ws in &app.session.workspaces {
            let tabs: Vec<&crate::config::Tab> = ws.tabs.iter().filter(|tb| tb.board.is_none() && tb.object.is_none() && (tb.is_notebook() || !tb.text.trim().is_empty())).collect();
            if tabs.is_empty() {
                continue;
            }
            ui.label(RichText::new(&ws.name).small().weak());
            for tb in tabs {
                let mark = if tb.is_notebook() { "📓" } else { "📜" };
                if ui.button(format!("{mark} {}", tb.title)).clicked() {
                    picked = Some(tb.id.clone());
                }
            }
        }
    });
    if let Some(tab) = picked {
        let at = center_of_view(app, ui.ctx(), &open.board);
        add_notebook(app, &mut open.board, &tab, at);
        ui.close();
    }
    ui.separator();
    ui.label(RichText::new(t("Объект базы", "A database object")).strong());
    ui.add(egui::TextEdit::singleline(&mut app.board_ui.find).hint_text(t("🔎 таблица, вью, функция…", "🔎 a table, view, function…")).desired_width(280.0));
    let q = app.board_ui.find.trim().to_lowercase();
    let mut hits: Vec<board::Drop> = Vec::new();
    for ((conn, path), node) in &app.tree {
        let crate::app::Node::Ready(items) = node else { continue };
        for o in items {
            if !matches!(o.kind, ObjKind::Table | ObjKind::View | ObjKind::MatView | ObjKind::Function | ObjKind::Procedure | ObjKind::Package | ObjKind::Dictionary) {
                continue;
            }
            if !q.is_empty() && !o.name.to_lowercase().contains(&q) {
                continue;
            }
            let mut p = path.clone();
            p.push(o.name.clone());
            hits.push(board::Drop { conn: conn.clone(), path: p, obj: o.kind, name: o.name.clone() });
        }
    }
    hits.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.path.cmp(&b.path)));
    egui::ScrollArea::vertical().id_salt("board-add-objs").max_height(260.0).show(ui, |ui| {
        if hits.is_empty() {
            ui.label(RichText::new(t("Раскройте подключение в дереве — его объекты появятся здесь", "Open a connection in the tree: its objects show here")).weak().small());
        }
        for h in hits.iter().take(60) {
            let conn = app.spec(&h.conn).map(|s| s.title()).unwrap_or_default();
            let schema = if h.path.len() > 2 { format!("{}.", h.path[0]) } else { String::new() };
            if ui.button(format!("{} {schema}{}   · {conn}", h.obj.icon(), h.name)).clicked() {
                let at = center_of_view(app, ui.ctx(), &open.board);
                add_object(app, &mut open.board, h, at, false);
                ui.close();
            }
        }
    });
}

/// Something put on the last board shown (from the tree's or a tab's menu).
pub fn put_on_last(app: &mut App, what: impl FnOnce(&mut App, &mut Board, Pos2)) {
    let Some(id) = app.boards.last.clone().filter(|id| app.boards.list.iter().any(|(i, _)| i == id)) else { return };
    let Some(mut open) = app.boards.get(&id).map(std::mem::take) else { return };
    let at = center_of_view(app, &app.ctx.clone(), &open.board);
    what(app, &mut open.board, at);
    open.board.fit_heights();
    if open.commit() {
        let _ = board::save(&open.board);
    }
    let name = open.board.name.clone();
    app.boards.open.insert(id, open);
    app.toast(format!("{} «{name}»", t("Добавлено на доску", "Put on the board")));
}

/// The board point at the canvas' middle (last frame's canvas).
fn center_of_view(_app: &App, ctx: &egui::Context, b: &Board) -> Pos2 {
    let r: Rect = ctx.data(|d| d.get_temp(egui::Id::new("board-canvas"))).unwrap_or(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)));
    Pos2::new(b.pan[0] + r.width() / 2.0 / b.zoom, b.pan[1] + r.height() / 2.0 / b.zoom)
}

/// A notebook's (or a script's statements') cells as they are now.
pub fn cells_of(tab: &crate::config::Tab, d: crate::sql::Dialect) -> Vec<NbCell> {
    match &tab.notebook {
        Some(nb) if nb.on => nb.cells.iter().map(|c| NbCell { md: c.kind == crate::config::CellKind::Markdown, text: c.text.trim().to_string() }).collect(),
        _ => crate::sql::split(&tab.text, d, true).into_iter().map(|r| NbCell { md: false, text: tab.text[r].trim().to_string() }).filter(|c| !c.text.is_empty()).collect(),
    }
}

fn find_tab<'a>(app: &'a App, id: &str) -> Option<(usize, usize, &'a crate::config::Tab)> {
    app.session.workspaces.iter().enumerate().find_map(|(wi, ws)| ws.tabs.iter().position(|tb| tb.id == id).map(|ti| (wi, ti, &ws.tabs[ti])))
}

pub fn add_notebook(app: &mut App, b: &mut Board, tab: &str, at: Pos2) -> Option<u64> {
    let (_, _, tb) = find_tab(app, tab)?;
    let d = app.dialect_of(app.tab_conn(&app.session.workspaces[0], tb).as_deref());
    let cells = cells_of(tb, d);
    let kind = Kind::Notebook { tab: tab.to_string(), title: tb.title.clone(), cells, flow: false, note: String::new() };
    let mut probe = Item { id: 0, rect: [0.0, 0.0, 340.0, 100.0], color: [0, 0, 0], kind };
    let size = Vec2::new(340.0, probe.natural_h().unwrap_or(100.0));
    let at = b.free_spot(at, size);
    let kind = std::mem::replace(&mut probe.kind, Kind::Text { text: String::new(), size: 0.0 });
    let id = b.add(Rect::from_center_size(at, size), app.board_ui.color, kind);
    app.board_ui.sel = vec![id];
    app.board_ui.sel_link = None;
    Some(id)
}

/// An object's columns (a table's, a view's) or parameters (a routine's)
/// as the tree or the hints last saw them.
fn columns_of(app: &App, d: &board::Drop) -> Vec<(String, String)> {
    let mut p = d.path.clone();
    p.push("columns".into());
    if let Some(crate::app::Node::Ready(v)) = app.tree.get(&(d.conn.clone(), p)) {
        return v.iter().map(|o| (o.name.clone(), o.note.clone())).collect();
    }
    let schema = if d.path.len() > 2 { d.path[0].clone() } else { String::new() };
    let bare = d.name.split('(').next().unwrap_or(&d.name).to_string();
    if let Some(l) = app.live.get(&d.conn) {
        if let Some(r) = l.rels.iter().find(|r| r.name.eq_ignore_ascii_case(&bare) && (schema.is_empty() || r.schema.eq_ignore_ascii_case(&schema))) {
            return r.cols.clone();
        }
        if let Some(r) = l.routines.iter().find(|r| r.name.eq_ignore_ascii_case(&bare) && (schema.is_empty() || r.schema.eq_ignore_ascii_case(&schema))) {
            return r.args.iter().map(|a| (a.name.clone(), a.ty.clone())).collect();
        }
    }
    Vec::new()
}

pub fn add_object(app: &mut App, b: &mut Board, d: &board::Drop, at: Pos2, exact: bool) -> u64 {
    // (Its columns: asked for when not seen yet.)
    let mut p = d.path.clone();
    p.push("columns".into());
    if matches!(d.obj, ObjKind::Table | ObjKind::View | ObjKind::MatView) && !app.tree.contains_key(&(d.conn.clone(), p.clone())) && app.live.contains_key(&d.conn) {
        app.request_children(&d.conn, p);
    }
    let cols = columns_of(app, d);
    let kind = Kind::Object { conn: d.conn.clone(), path: d.path.clone(), obj: d.obj, name: d.name.clone(), cols, note: String::new() };
    let mut probe = Item { id: 0, rect: [0.0, 0.0, 240.0, 100.0], color: [0, 0, 0], kind };
    let size = Vec2::new(240.0, probe.natural_h().unwrap_or(100.0));
    // (Dropped: right there; from a menu: where there's room.)
    let at = if exact { at } else { b.free_spot(at, size) };
    let kind = std::mem::replace(&mut probe.kind, Kind::Text { text: String::new(), size: 0.0 });
    let id = b.add(Rect::from_center_size(at, size), app.board_ui.color, kind);
    app.board_ui.sel = vec![id];
    app.board_ui.sel_link = None;
    id
}

/// Columns arrived since an object was put on: filled in.
fn fill_columns(app: &App, b: &mut Board) {
    for it in &mut b.items {
        if let Kind::Object { conn, path, obj, name, cols, .. } = &mut it.kind {
            if cols.is_empty() {
                let d = board::Drop { conn: conn.clone(), path: path.clone(), obj: *obj, name: name.clone() };
                let got = columns_of(app, &d);
                if !got.is_empty() {
                    *cols = got;
                }
            }
        }
    }
}

fn canvas(app: &mut App, ui: &mut egui::Ui, open: &mut board::Open) {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("board-canvas"), rect));
    let painter = ui.painter_at(rect);
    let dark = ui.visuals().dark_mode;
    painter.rect_filled(rect, 8.0, if dark { Color32::from_gray(24) } else { Color32::from_gray(250) });
    fill_columns(app, &mut open.board);
    let b = &mut open.board;
    // Show everything.
    if ui.ctx().data_mut(|d| d.remove_temp::<bool>(egui::Id::new("board-fit"))).unwrap_or(false) {
        if let Some(bb) = b.bounds() {
            let bb = bb.expand(40.0);
            b.zoom = (rect.width() / bb.width()).min(rect.height() / bb.height()).clamp(0.2, 2.0);
            b.pan = [bb.center().x - rect.width() / 2.0 / b.zoom, bb.center().y - rect.height() / 2.0 / b.zoom];
        } else {
            b.pan = [0.0, 0.0];
            b.zoom = 1.0;
        }
    }
    let mut view = View { origin: rect.min, pan: Vec2::new(b.pan[0], b.pan[1]), zoom: b.zoom };
    // Dots every 24 units.
    let step = 24.0 * view.zoom;
    if step >= 8.0 {
        let c = if dark { Color32::from_gray(48) } else { Color32::from_gray(214) };
        let first = view.w(rect.min);
        let (x0, y0) = ((first.x / 24.0).floor() * 24.0, (first.y / 24.0).floor() * 24.0);
        let mut y = y0;
        while view.s(Pos2::new(0.0, y)).y < rect.bottom() {
            let mut x = x0;
            while view.s(Pos2::new(x, 0.0)).x < rect.right() {
                painter.circle_filled(view.s(Pos2::new(x, y)), 1.0, c);
                x += 24.0;
            }
            y += 24.0;
        }
    }
    let st = &mut app.board_ui;
    let pointer = ui.ctx().pointer_latest_pos();
    let wp = pointer.map(|p| view.w(p));
    // Wheel: pan; Ctrl+wheel (or a pinch): zoom about the pointer.
    if resp.hovered() || resp.contains_pointer() {
        let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
        if zoom != 1.0 {
            if let Some(p) = pointer {
                let before = view.w(p);
                b.zoom = (b.zoom * zoom).clamp(0.2, 4.0);
                view.zoom = b.zoom;
                let after = view.w(p);
                view.pan += before - after;
            }
        } else if scroll != Vec2::ZERO {
            view.pan -= scroll / view.zoom;
        }
    }
    // The middle button pans whatever the tool.
    if ui.input(|i| i.pointer.middle_down()) && resp.contains_pointer() {
        view.pan -= ui.input(|i| i.pointer.delta()) / view.zoom;
    }
    // Keys, while no field takes them.
    let typing = ui.ctx().memory(|m| m.focused()).is_some_and(|f| f != resp.id);
    if !typing && st.edit.is_none() {
        let (del, redo, undo, esc, all, dup) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace),
                // (Before Ctrl+Z: that one takes Ctrl+Shift+Z too.)
                i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::D),
            )
        });
        if resp.contains_pointer() || resp.has_focus() {
            for tool in Tool::ALL {
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, tool.key())) {
                    st.tool = tool;
                }
            }
        }
        if del {
            let mut ids = st.sel.clone();
            ids.extend(st.sel_link);
            b.remove(&ids);
            st.sel.clear();
            st.sel_link = None;
        }
        if esc {
            st.sel.clear();
            st.sel_link = None;
            st.tool = Tool::Select;
        }
        if all {
            st.sel = b.items.iter().map(|i| i.id).collect();
        }
        if dup && !st.sel.is_empty() {
            let copies: Vec<Item> = b.items.iter().filter(|i| st.sel.contains(&i.id)).cloned().collect();
            st.sel.clear();
            for mut c in copies {
                c.id = b.take_id();
                c.shift(Vec2::splat(24.0));
                st.sel.push(c.id);
                b.items.push(c);
            }
        }
        if undo {
            b.pan = [view.pan.x, view.pan.y];
            open.undo();
            st.sel.retain(|id| open.board.item(*id).is_some());
            return finish(open, view);
        }
        if redo {
            b.pan = [view.pan.x, view.pan.y];
            open.redo();
            st.sel.retain(|id| open.board.item(*id).is_some());
            return finish(open, view);
        }
    }
    // Pointer gestures.
    let slack = 4.0 / view.zoom;
    let shift = ui.input(|i| i.modifiers.shift);
    let handle = |view: View, it: &Item| Rect::from_center_size(view.s(it.rect().max), Vec2::splat(10.0));
    if resp.drag_started_by(egui::PointerButton::Primary) {
        // (Where the button went down: the drag counts a few pixels on.)
        if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()) {
            let w = view.w(p);
            let on = b.hit(w, slack);
            st.edit = None;
            st.gesture = match st.tool {
                Tool::Select => {
                    let resize = st.sel.iter().rev().find(|id| b.item(**id).is_some_and(|it| handle(view, it).contains(p))).copied();
                    if let Some(id) = resize.filter(|_| st.sel.len() == 1) {
                        Gesture::Resize(id)
                    } else if let Some(id) = on {
                        if !st.sel.contains(&id) {
                            if !shift {
                                st.sel.clear();
                            }
                            st.sel.push(id);
                        }
                        st.sel_link = None;
                        Gesture::Move
                    } else if shift {
                        Gesture::Band(w, w)
                    } else {
                        Gesture::Pan
                    }
                }
                Tool::Rect | Tool::Ellipse | Tool::Diamond | Tool::Entity => Gesture::Make(w, w),
                Tool::Brush => Gesture::Draw(vec![[w.x, w.y]]),
                Tool::Arrow => match on {
                    Some(id) => Gesture::Link(id, b.item(id).and_then(|it| it.row_at(w.y)), w),
                    None => Gesture::Pan,
                },
                Tool::Text => Gesture::Pan,
            };
        }
    }
    if resp.dragged_by(egui::PointerButton::Primary) {
        let d = resp.drag_delta() / view.zoom;
        match &mut st.gesture {
            Gesture::Pan => view.pan -= d,
            Gesture::Move => {
                for it in b.items.iter_mut().filter(|i| st.sel.contains(&i.id)) {
                    it.shift(d);
                }
            }
            Gesture::Resize(id) => {
                if let (Some(it), Some(w)) = (b.item_mut(*id), wp) {
                    let r = it.rect();
                    let to = Rect::from_min_max(r.min, Pos2::new(w.x.max(r.min.x + 30.0), w.y.max(r.min.y + 20.0)));
                    it.resize(to);
                }
            }
            Gesture::Draw(pts) => {
                if let Some(w) = wp {
                    if pts.last().is_none_or(|l| Pos2::new(l[0], l[1]).distance(w) * view.zoom > 1.5) {
                        pts.push([w.x, w.y]);
                    }
                }
            }
            Gesture::Make(_, to) | Gesture::Link(_, _, to) | Gesture::Band(_, to) => {
                if let Some(w) = wp {
                    *to = w;
                }
            }
            Gesture::None => {}
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        match std::mem::take(&mut st.gesture) {
            Gesture::Make(a, z) => {
                let r = Rect::from_two_pos(a, z);
                let id = make(b, st, r, a);
                st.sel = vec![id];
                st.sel_link = None;
            }
            Gesture::Draw(pts) => {
                if pts.len() > 1 {
                    let half = st.width / 2.0;
                    let bb = pts.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_center_size(Pos2::new(p[0], p[1]), Vec2::splat(st.width)))).expand(half * 0.0);
                    b.add(bb, st.color, Kind::Stroke { points: pts, width: st.width });
                }
            }
            Gesture::Link(from, field, to) => {
                if let Some(target) = b.items.iter().rev().find(|i| i.id != from && i.hit(to, slack)).map(|i| (i.id, i.row_at(to.y))) {
                    let id = b.take_id();
                    b.links.push(Link { id, from, to: target.0, from_field: field, to_field: target.1, label: String::new(), color: st.color });
                    st.sel.clear();
                    st.sel_link = Some(id);
                }
            }
            Gesture::Band(a, z) => {
                let r = Rect::from_two_pos(a, z);
                st.sel = b.items.iter().filter(|i| r.intersects(i.rect())).map(|i| i.id).collect();
                st.sel_link = None;
            }
            _ => {}
        }
    }
    if resp.clicked() {
        resp.request_focus();
        if let Some(p) = resp.interact_pointer_pos() {
            let w = view.w(p);
            let on = b.hit(w, slack);
            match st.tool {
                Tool::Select => {
                    st.edit = None;
                    match on {
                        Some(id) if shift => {
                            if let Some(k) = st.sel.iter().position(|x| *x == id) {
                                st.sel.remove(k);
                            } else {
                                st.sel.push(id);
                            }
                        }
                        Some(id) => {
                            st.sel = vec![id];
                            st.sel_link = None;
                        }
                        None => {
                            // A link near the pointer, or nothing.
                            st.sel.clear();
                            st.sel_link = b.links.iter().find(|l| b.link_dist(l, w) <= slack + 2.0 / view.zoom).map(|l| l.id);
                        }
                    }
                }
                Tool::Text => {
                    let id = b.add(Rect::from_min_size(w, Vec2::new(60.0, 20.0)), st.color, Kind::Text { text: String::new(), size: 18.0 });
                    st.sel = vec![id];
                    st.edit = Some(id);
                    st.edit_fresh = 3;
                    st.tool = Tool::Select;
                }
                Tool::Rect | Tool::Ellipse | Tool::Diamond | Tool::Entity => {
                    let id = make(b, st, Rect::from_min_size(w, Vec2::ZERO), w);
                    st.sel = vec![id];
                }
                _ => {}
            }
        }
    }
    if resp.double_clicked() && st.tool == Tool::Select {
        if let Some(p) = resp.interact_pointer_pos() {
            let w = view.w(p);
            match b.hit(w, slack).and_then(|id| b.item(id)).map(|it| (it.id, it.kind.clone())) {
                Some((id, Kind::Frame { .. } | Kind::Text { .. })) => {
                    st.edit = Some(id);
                    st.edit_fresh = 3;
                }
                Some((_, Kind::Entity { .. })) => st.focus_prop = true,
                Some((_, Kind::Object { conn, path, obj, name, .. })) => open_object(app, &conn, &path, obj, &name),
                Some((_, Kind::Notebook { tab, .. })) => go_to_tab(app, &tab),
                Some(_) => {}
                None => {
                    // Writing right where it was double-clicked.
                    let id = b.add(Rect::from_min_size(w, Vec2::new(60.0, 20.0)), st.color, Kind::Text { text: String::new(), size: 18.0 });
                    st.sel = vec![id];
                    st.edit = Some(id);
                    st.edit_fresh = 3;
                }
            }
        }
    }
    let st = &mut app.board_ui;
    // Something from the tree dropped here.
    if let Some(d) = resp.dnd_release_payload::<board::Drop>() {
        if let Some(p) = pointer {
            let at = view.w(p);
            b.pan = [view.pan.x, view.pan.y];
            add_object(app, &mut open.board, &d, at, true);
            return finish(open, view);
        }
    }
    let dropping = resp.dnd_hover_payload::<board::Drop>().is_some();
    if dropping {
        painter.rect_stroke(rect.shrink(2.0), 8.0, Stroke::new(2.0, ui.visuals().selection.stroke.color), egui::StrokeKind::Inside);
    }
    // Drawing: links under the items, then the items in their order.
    let text_c = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    let weak = ui.visuals().weak_text_color();
    let card = crate::ui::card(ui.visuals());
    let accent = ui.visuals().selection.stroke.color;
    let b = &mut open.board;
    for l in &b.links {
        let Some(path) = b.link_path(l) else { continue };
        let pts: Vec<Pos2> = path.iter().map(|p| view.s(*p)).collect();
        let picked = st.sel_link == Some(l.id);
        let c = rgb(l.color);
        let line = Stroke::new((1.6 * view.zoom).clamp(1.0, 3.0), c);
        if picked {
            painter.add(egui::Shape::line(pts.clone(), Stroke::new(6.0, accent.gamma_multiply(0.35))));
        }
        let n = pts.len();
        painter.add(egui::Shape::line(pts[..n - 1].to_vec(), line));
        arrow(&painter, pts[n - 2], pts[n - 1], line, (5.0 * view.zoom).clamp(3.0, 9.0));
        if !l.label.is_empty() && view.zoom > 0.35 {
            let g = painter.layout_no_wrap(l.label.clone(), FontId::proportional(12.0 * view.zoom), text_c);
            // On the way's middle piece.
            let k = (n - 1) / 2;
            let (a, z) = (pts[k], pts[k + 1]);
            let m = a + (z - a) / 2.0;
            let r = Rect::from_center_size(m, g.size() + Vec2::new(8.0, 4.0));
            painter.rect_filled(r, 4.0, card);
            painter.galley(r.min + Vec2::new(4.0, 2.0), g, text_c);
        }
    }
    let editing = st.edit;
    for it in b.items.iter_mut() {
        draw_item(&painter, ui, it, view, text_c, strong, weak, card, editing == Some(it.id));
    }
    // The picked: framed, with a corner to resize by (one picked).
    for id in &st.sel {
        if let Some(it) = b.item(*id) {
            let r = view.rect(it.rect()).expand(3.0);
            painter.rect_stroke(r, 4.0, Stroke::new(1.5, accent), egui::StrokeKind::Middle);
            if st.sel.len() == 1 {
                let h = handle(view, it);
                painter.rect_filled(h, 2.0, accent);
            }
        }
    }
    // What's being drawn.
    let c = rgb(st.color);
    match &st.gesture {
        Gesture::Make(a, z) => {
            painter.rect_stroke(view.rect(Rect::from_two_pos(*a, *z)), 4.0, Stroke::new(1.5, c), egui::StrokeKind::Middle);
        }
        Gesture::Draw(pts) => {
            painter.add(egui::Shape::line(pts.iter().map(|p| view.s(Pos2::new(p[0], p[1]))).collect(), Stroke::new(st.width * view.zoom, c)));
        }
        Gesture::Link(from, field, to) => {
            if let Some(it) = b.item(*from) {
                let start = match field {
                    Some(f) => {
                        let r = it.rect();
                        Pos2::new(if to.x >= r.center().x { r.right() } else { r.left() }, it.row_y(*f))
                    }
                    None => board::edge_point(it.rect(), *to),
                };
                arrow(&painter, view.s(start), view.s(*to), Stroke::new(1.6, c), 6.0);
            }
        }
        Gesture::Band(a, z) => {
            let r = view.rect(Rect::from_two_pos(*a, *z));
            painter.rect_filled(r, 2.0, accent.gamma_multiply(0.12));
            painter.rect_stroke(r, 2.0, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
        }
        _ => {}
    }
    // The link tool: the field row under the pointer lit.
    if st.tool == Tool::Arrow {
        if let Some(w) = wp {
            if let Some(it) = b.hit(w, slack).and_then(|id| b.item(id)) {
                let r = match it.row_at(w.y) {
                    Some(k) => Rect::from_min_size(Pos2::new(it.rect[0], it.row_y(k) - ROW_H / 2.0), Vec2::new(it.rect[2], ROW_H)),
                    None => it.rect(),
                };
                painter.rect_filled(view.rect(r), 3.0, accent.gamma_multiply(0.15));
            }
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    } else if matches!(st.tool, Tool::Rect | Tool::Ellipse | Tool::Diamond | Tool::Entity | Tool::Brush) && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    } else if matches!(st.gesture, Gesture::Pan) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if st.tool == Tool::Select && st.sel.len() == 1 && pointer.is_some_and(|p| b.item(st.sel[0]).is_some_and(|it| handle(view, it).contains(p))) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
    }
    // Text typed right on the canvas.
    if let Some(id) = st.edit {
        let fresh = st.edit_fresh > 0;
        st.edit_fresh = st.edit_fresh.saturating_sub(1);
        if let Some(it) = b.item_mut(id) {
            let r = view.rect(it.rect());
            let zoom = view.zoom;
            let (text, size, inner) = match &mut it.kind {
                Kind::Frame { text, .. } => (text, 14.0, r.shrink(6.0 * zoom)),
                Kind::Text { text, size } => {
                    let s = *size;
                    (text, s, Rect::from_min_size(r.min, Vec2::new(r.width().max(160.0), r.height().max(s * zoom + 8.0))))
                }
                _ => return finish(open, view),
            };
            let te = egui::TextEdit::multiline(text).font(FontId::proportional(size * zoom)).frame(egui::Frame::NONE).desired_width(inner.width()).text_color(rgb(it.color).lerp_to_gamma(strong, 0.3));
            let out = ui.put(inner, te);
            if fresh && !out.has_focus() {
                out.request_focus();
            }
            if (out.lost_focus() && !fresh) || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                st.edit = None;
            }
        }
        // An empty text left: gone.
        if st.edit.is_none() {
            if b.item(id).is_some_and(|it| matches!(&it.kind, Kind::Text { text, .. } if text.trim().is_empty())) {
                b.remove(&[id]);
                st.sel.retain(|x| *x != id);
            }
        }
    }
    finish(open, view);
}

fn finish(open: &mut board::Open, view: View) {
    open.board.pan = [view.pan.x, view.pan.y];
    open.board.zoom = view.zoom;
}

/// A frame or an entity from a drawn rect (a click: the usual size).
fn make(b: &mut Board, st: &mut State, r: Rect, at: Pos2) -> u64 {
    let small = r.width() < 12.0 && r.height() < 12.0;
    let id = match st.tool {
        Tool::Entity => {
            let r = if small { Rect::from_min_size(at, Vec2::new(200.0, 80.0)) } else { r };
            let n = b.items.iter().filter(|i| matches!(i.kind, Kind::Entity { .. })).count() + 1;
            b.add(r, st.color, Kind::Entity { name: format!("{}{n}", t("таблица_", "table_")), fields: vec![Field { name: "id".into(), ty: "bigint".into(), key: true, note: String::new() }], note: String::new() })
        }
        tool => {
            let shape = match tool {
                Tool::Ellipse => Shape::Ellipse,
                Tool::Diamond => Shape::Diamond,
                _ => Shape::Rect,
            };
            let r = if small { Rect::from_min_size(at, if shape == Shape::Diamond { Vec2::new(160.0, 110.0) } else { Vec2::new(180.0, 90.0) }) } else { r };
            let id = b.add(r, st.color, Kind::Frame { shape, text: String::new(), fill: true });
            st.edit = Some(id);
            st.edit_fresh = 3;
            id
        }
    };
    st.focus_prop = st.tool == Tool::Entity;
    st.tool = Tool::Select;
    id
}

#[allow(clippy::too_many_arguments)]
fn draw_item(p: &egui::Painter, ui: &egui::Ui, it: &mut Item, view: View, text_c: Color32, strong: Color32, weak: Color32, card: Color32, editing: bool) {
    let z = view.zoom;
    let r = view.rect(it.rect());
    let c = rgb(it.color);
    let line = Stroke::new((1.5 * z).clamp(1.0, 3.0), c);
    let legible = 12.0 * z >= 5.0;
    match &it.kind {
        Kind::Frame { shape, text, fill } => {
            let bg = if *fill { c.gamma_multiply(0.16) } else { Color32::TRANSPARENT };
            match shape {
                Shape::Rect => {
                    p.rect_filled(r, 6.0 * z, bg);
                    p.rect_stroke(r, 6.0 * z, line, egui::StrokeKind::Middle);
                }
                Shape::Ellipse => {
                    p.add(egui::Shape::ellipse_filled(r.center(), r.size() / 2.0, bg));
                    p.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, line));
                }
                Shape::Diamond => {
                    let pts = vec![r.center_top(), r.right_center(), r.center_bottom(), r.left_center()];
                    p.add(egui::Shape::convex_polygon(pts, bg, line));
                }
            }
            if !editing && legible && !text.is_empty() {
                let inner = if *shape == Shape::Rect { r.shrink(8.0 * z) } else { r.shrink2(r.size() * 0.18) };
                let g = p.layout(text.clone(), FontId::proportional(14.0 * z), strong, inner.width().max(10.0));
                p.with_clip_rect(r.intersect(p.clip_rect())).galley(Pos2::new(inner.center().x - g.size().x / 2.0, inner.center().y - g.size().y / 2.0), g, strong);
            }
        }
        Kind::Text { text, size } => {
            let g = p.layout_no_wrap(if text.is_empty() { " ".into() } else { text.clone() }, FontId::proportional(size * z), c.lerp_to_gamma(strong, 0.3));
            let sz = g.size() / z;
            if !editing {
                p.galley(r.min, g, strong);
            }
            it.rect[2] = sz.x.max(20.0);
            it.rect[3] = sz.y.max(*size);
        }
        Kind::Stroke { points, width } => {
            let pts: Vec<Pos2> = points.iter().map(|q| view.s(Pos2::new(q[0], q[1]))).collect();
            p.add(egui::Shape::line(pts, Stroke::new(width * z, c)));
        }
        Kind::Entity { name, fields, note } => {
            let rows: Vec<(String, String, bool)> = fields.iter().map(|f| (f.name.clone(), f.ty.clone(), f.key)).collect();
            table_card(p, ui, r, z, c, name, None, &rows, note, text_c, weak, card);
        }
        Kind::Object { name, obj, cols, note, conn, .. } => {
            let rows: Vec<(String, String, bool)> = cols.iter().map(|(n, ty)| (n.clone(), ty.clone(), false)).collect();
            let rows = if rows.is_empty() { vec![(t("колонки — после подключения", "columns: once connected").to_string(), String::new(), false)] } else { rows };
            let _ = conn;
            table_card(p, ui, r, z, c, name, Some(*obj), &rows, note, text_c, weak, card);
        }
        Kind::Notebook { title, cells, flow, note, .. } => {
            p.rect_filled(r, 8.0 * z, card);
            p.rect_stroke(r, 8.0 * z, line, egui::StrokeKind::Middle);
            let head = Rect::from_min_size(r.min, Vec2::new(r.width(), HEAD_H * z));
            p.rect_filled(head, egui::CornerRadius { nw: (8.0 * z) as u8, ne: (8.0 * z) as u8, sw: 0, se: 0 }, c.gamma_multiply(0.85));
            if legible {
                let ic = Rect::from_center_size(Pos2::new(head.left() + 14.0 * z, head.center().y), Vec2::splat(16.0 * z));
                if !crate::icons::paint(ui, ic, crate::icons::Ic::Notebook, 15.0 * z, Color32::WHITE) {
                    p.text(ic.center(), Align2::CENTER_CENTER, "📓", FontId::proportional(13.0 * z), Color32::WHITE);
                }
                p.text(Pos2::new(head.left() + 26.0 * z, head.center().y), Align2::LEFT_CENTER, title, FontId::proportional(13.5 * z), Color32::WHITE);
            }
            let mut y = head.bottom() + 8.0 * z;
            let x = r.left() + 10.0 * z;
            let w = r.width() - 20.0 * z;
            let clip = p.with_clip_rect(r.intersect(p.clip_rect()));
            for (k, cell) in cells.iter().enumerate() {
                if *flow {
                    // A box each, an arrow down to the next.
                    let bx = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, FLOW_BOX_H * z));
                    let (fill, round) = if cell.md { (c.gamma_multiply(0.10), 14.0 * z) } else { (c.gamma_multiply(0.20), 4.0 * z) };
                    clip.rect_filled(bx, round, fill);
                    clip.rect_stroke(bx, round, Stroke::new(1.0, c.gamma_multiply(0.8)), egui::StrokeKind::Middle);
                    if legible {
                        let txt: String = cell.text.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect::<Vec<_>>().join("\n");
                        let font = if cell.md { FontId::proportional(12.0 * z) } else { FontId::monospace(11.0 * z) };
                        let g = clip.layout(txt, font, text_c, bx.width() - 12.0 * z);
                        clip.with_clip_rect(bx.intersect(clip.clip_rect())).galley(bx.min + Vec2::new(6.0 * z, 5.0 * z), g, text_c);
                    }
                    if k + 1 < cells.len() {
                        arrow(&clip, Pos2::new(bx.center().x, bx.bottom()), Pos2::new(bx.center().x, bx.bottom() + FLOW_GAP * z), Stroke::new(1.4, c), 4.5 * z.max(0.6));
                    }
                    y += (FLOW_BOX_H + FLOW_GAP) * z;
                } else {
                    let n = cell.text.lines().count().clamp(1, CARD_LINES);
                    let h = (NOTE_LINE_H * n as f32 + 10.0) * z;
                    let bx = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h - 4.0 * z));
                    clip.rect_filled(bx, 4.0 * z, if cell.md { Color32::TRANSPARENT } else { c.gamma_multiply(0.08) });
                    clip.vline(bx.left(), bx.y_range(), Stroke::new(2.0 * z.min(1.0), c.gamma_multiply(if cell.md { 0.4 } else { 0.9 })));
                    if legible {
                        let txt: String = cell.text.lines().take(CARD_LINES).collect::<Vec<_>>().join("\n");
                        let font = if cell.md { FontId::proportional(12.0 * z) } else { FontId::monospace(11.0 * z) };
                        let g = clip.layout_no_wrap(txt, font, text_c);
                        clip.with_clip_rect(bx.intersect(clip.clip_rect())).galley(bx.min + Vec2::new(7.0 * z, 3.0 * z), g, text_c);
                    }
                    y += h;
                }
            }
            if cells.is_empty() && legible {
                clip.text(Pos2::new(x, y), Align2::LEFT_TOP, t("пусто", "empty"), FontId::proportional(12.0 * z), weak);
            }
            note_text(&clip, r, z, note, weak);
        }
    }
}

/// A table's card: a coloured head (its icon and name), a row per field
/// (a key's dot, the name, the type), the note below.
#[allow(clippy::too_many_arguments)]
fn table_card(p: &egui::Painter, ui: &egui::Ui, r: Rect, z: f32, c: Color32, name: &str, obj: Option<ObjKind>, rows: &[(String, String, bool)], note: &str, text_c: Color32, weak: Color32, card: Color32) {
    p.rect_filled(r, 6.0 * z, card);
    p.rect_stroke(r, 6.0 * z, Stroke::new((1.4 * z).clamp(1.0, 2.5), c), egui::StrokeKind::Middle);
    let head = Rect::from_min_size(r.min, Vec2::new(r.width(), HEAD_H * z));
    let round = (6.0 * z).min(255.0) as u8;
    p.rect_filled(head, egui::CornerRadius { nw: round, ne: round, sw: 0, se: 0 }, c.gamma_multiply(0.85));
    if 12.0 * z < 5.0 {
        return;
    }
    let mut x = head.left() + 9.0 * z;
    if let Some(k) = obj {
        let ic = Rect::from_center_size(Pos2::new(x + 6.0 * z, head.center().y), Vec2::splat(15.0 * z));
        if !crate::icons::paint(ui, ic, crate::icons::Ic::of(k), 14.0 * z, Color32::WHITE) {
            p.text(ic.center(), Align2::CENTER_CENTER, k.icon(), FontId::proportional(12.0 * z), Color32::WHITE);
        }
        x += 18.0 * z;
    }
    let clip = p.with_clip_rect(r.intersect(p.clip_rect()));
    clip.text(Pos2::new(x, head.center().y), Align2::LEFT_CENTER, name, FontId::proportional(14.0 * z), Color32::WHITE);
    for (k, (n, ty, key)) in rows.iter().enumerate() {
        let y = head.bottom() + ROW_H * z * (k as f32 + 0.5);
        if k > 0 {
            clip.hline(r.x_range().shrink(6.0 * z), y - ROW_H * z / 2.0, Stroke::new(0.5, weak.gamma_multiply(0.4)));
        }
        if *key {
            clip.circle_filled(Pos2::new(r.left() + 10.0 * z, y), 3.5 * z, c);
        }
        clip.text(Pos2::new(r.left() + 19.0 * z, y), Align2::LEFT_CENTER, n, FontId::proportional(12.5 * z), text_c);
        if !ty.is_empty() {
            clip.text(Pos2::new(r.right() - 8.0 * z, y), Align2::RIGHT_CENTER, ty, FontId::monospace(11.0 * z), weak);
        }
    }
    note_text(&clip, r, z, note, weak);
}

/// An item's note at its bottom.
fn note_text(p: &egui::Painter, r: Rect, z: f32, note: &str, weak: Color32) {
    if note.trim().is_empty() || 12.0 * z < 5.0 {
        return;
    }
    let n = note.lines().count().max(1) as f32;
    let top = r.bottom() - (NOTE_LINE_H * n + 6.0) * z;
    p.hline(r.x_range().shrink(6.0 * z), top - 2.0 * z, Stroke::new(0.5, weak.gamma_multiply(0.5)));
    let g = p.layout(note.to_string(), FontId::proportional(12.0 * z), weak, r.width() - 16.0 * z);
    p.galley(Pos2::new(r.left() + 8.0 * z, top), g, weak);
}

fn open_object(app: &mut App, conn: &str, path: &[String], obj: ObjKind, name: &str) {
    let title = name.split('(').next().unwrap_or(name).to_string();
    match obj {
        ObjKind::Table | ObjKind::Job => app.open_object(conn, path.to_vec(), obj, title),
        k if k.has_source() => app.open_source_next(conn, path.to_vec(), k, title),
        _ => {}
    }
}

fn go_to_tab(app: &mut App, tab: &str) {
    match find_tab(app, tab).map(|(wi, ti, _)| (wi, ti)) {
        Some((wi, ti)) => {
            app.session.active = wi;
            app.session.workspaces[wi].active = ti;
            app.touch();
        }
        None => app.toast(t("Этой вкладки больше нет", "That tab is gone")),
    }
}

/// What's picked, set up: its text, colour, fields, note…
fn props(app: &mut App, ui: &mut egui::Ui, b: &mut Board) {
    let st = &mut app.board_ui;
    if let Some(lid) = st.sel_link {
        let Some(l) = b.links.iter_mut().find(|l| l.id == lid) else {
            st.sel_link = None;
            return;
        };
        ui.label(RichText::new(t("Связь", "Link")).strong());
        ui.label(t("Подпись", "Label"));
        ui.add(egui::TextEdit::singleline(&mut l.label).hint_text(t("например, 1 : N", "e.g. 1 : N")).desired_width(f32::INFINITY));
        if ui.button(t("Развернуть", "Reverse")).clicked() {
            std::mem::swap(&mut l.from, &mut l.to);
            std::mem::swap(&mut l.from_field, &mut l.to_field);
        }
        ui.add_space(6.0);
        if ui.add(crate::ui::tinted(t("Удалить связь", "Delete the link").into(), crate::ui::RED)).clicked() {
            b.links.retain(|x| x.id != lid);
            st.sel_link = None;
        }
        return;
    }
    if st.sel.len() > 1 {
        ui.label(RichText::new(format!("{}: {}", t("Выбрано", "Picked"), st.sel.len())).strong());
        ui.label(RichText::new(t("Цвет — кружками сверху, Ctrl+D — копия, Delete — удалить", "Colour: the dots above; Ctrl+D: a copy; Delete: delete")).weak().small());
        if ui.add(crate::ui::tinted(t("Удалить", "Delete").into(), crate::ui::RED)).clicked() {
            b.remove(&st.sel.clone());
            st.sel.clear();
        }
        return;
    }
    let Some(&id) = st.sel.first() else { return };
    let Some(pos) = b.items.iter().position(|i| i.id == id) else {
        st.sel.clear();
        return;
    };
    let focus = std::mem::take(&mut st.focus_prop);
    let mut open_obj = None;
    let mut go_tab = None;
    let mut refresh_tab = None;
    let mut refresh_obj = false;
    {
        let it = &mut b.items[pos];
        match &mut it.kind {
            Kind::Frame { shape, text, fill } => {
                ui.label(RichText::new(t("Рамка", "Frame")).strong());
                ui.horizontal(|ui| {
                    ui.radio_value(shape, Shape::Rect, t("прямоугольник", "rectangle"));
                    ui.radio_value(shape, Shape::Ellipse, t("овал", "ellipse"));
                    ui.radio_value(shape, Shape::Diamond, t("ромб", "diamond"));
                });
                ui.checkbox(fill, t("С заливкой", "Filled"));
                ui.label(t("Текст", "Text"));
                let r = ui.add(egui::TextEdit::multiline(text).desired_rows(4).desired_width(f32::INFINITY));
                if focus {
                    r.request_focus();
                }
            }
            Kind::Text { text, size } => {
                ui.label(RichText::new(t("Текст", "Text")).strong());
                let r = ui.add(egui::TextEdit::multiline(text).desired_rows(4).desired_width(f32::INFINITY));
                if focus {
                    r.request_focus();
                }
                ui.horizontal(|ui| {
                    ui.label(t("Размер", "Size"));
                    ui.add(egui::Slider::new(size, 10.0..=64.0));
                });
            }
            Kind::Stroke { width, .. } => {
                ui.label(RichText::new(t("Линия кистью", "A brush line")).strong());
                ui.horizontal(|ui| {
                    ui.label(t("Толщина", "Width"));
                    ui.add(egui::Slider::new(width, 1.0..=24.0));
                });
            }
            Kind::Entity { name, fields, note } => {
                ui.label(RichText::new(t("Таблица-сущность", "Table entity")).strong());
                let r = ui.add(egui::TextEdit::singleline(name).hint_text(t("имя", "name")).desired_width(f32::INFINITY));
                if focus {
                    r.request_focus();
                }
                ui.add_space(4.0);
                ui.label(RichText::new(t("Поля (● — ключ)", "Fields (● — a key)")).small().weak());
                let mut drop = None;
                let mut up = None;
                let full = ui.available_width();
                let h = ui.spacing().interact_size.y;
                for (k, f) in fields.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.checkbox(&mut f.key, "").on_hover_text(t("Ключ", "Key"));
                        // Name and type share what the key and the buttons leave.
                        let rest = (full - 28.0 - 50.0 - 9.0).max(80.0);
                        ui.add_sized([rest * 0.58, h], egui::TextEdit::singleline(&mut f.name).hint_text(t("поле", "field")));
                        ui.add_sized([rest * 0.42, h], egui::TextEdit::singleline(&mut f.ty).hint_text(t("тип", "type")));
                        if k > 0 && ui.small_button("↑").on_hover_text(t("Выше", "Up")).clicked() {
                            up = Some(k);
                        }
                        if ui.small_button("✕").on_hover_text(t("Убрать поле", "Remove the field")).clicked() {
                            drop = Some(k);
                        }
                    });
                }
                if let Some(k) = up {
                    fields.swap(k, k - 1);
                }
                if let Some(k) = drop {
                    fields.remove(k);
                }
                if ui.button(format!("➕ {}", t("Поле", "Field"))).clicked() {
                    fields.push(Field::default());
                }
                // Links to fields follow their rows.
                if let Some(k) = drop {
                    for l in b.links.iter_mut() {
                        for (end, f) in [(l.from, &mut l.from_field), (l.to, &mut l.to_field)] {
                            if end == id {
                                *f = match *f {
                                    Some(x) if x == k => None,
                                    Some(x) if x > k => Some(x - 1),
                                    o => o,
                                };
                            }
                        }
                    }
                } else if let Some(k) = up {
                    for l in b.links.iter_mut() {
                        for (end, f) in [(l.from, &mut l.from_field), (l.to, &mut l.to_field)] {
                            if end == id {
                                *f = match *f {
                                    Some(x) if x == k => Some(k - 1),
                                    Some(x) if x == k - 1 => Some(k),
                                    o => o,
                                };
                            }
                        }
                    }
                }
                ui.add_space(4.0);
                ui.label(t("Описание", "Note"));
                ui.add(egui::TextEdit::multiline(note).desired_rows(3).desired_width(f32::INFINITY));
                let b_items = &b.items;
                let _ = b_items;
            }
            Kind::Object { conn, path, obj, name, cols, note } => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(obj.icon()).weak());
                    ui.label(RichText::new(name.as_str()).strong());
                });
                let title = app.conns.list.iter().find(|s| s.id == *conn).map(|s| s.title()).unwrap_or_else(|| t("подключение удалено", "connection deleted").to_string());
                ui.label(RichText::new(format!("{title} · {}", path[..path.len().saturating_sub(1)].iter().filter(|p| !matches!(p.as_str(), "tables" | "views" | "functions" | "procedures" | "packages" | "matviews" | "dictionaries")).cloned().collect::<Vec<_>>().join("."))).small().weak());
                ui.horizontal(|ui| {
                    if ui.button(t("Открыть", "Open")).on_hover_text(t("Конструктор или код (двойной щелчок по карточке)", "The designer or the code (double-click the card)")).clicked() {
                        open_obj = Some((conn.clone(), path.clone(), *obj, name.clone()));
                    }
                    if ui.button(t("Обновить колонки", "Refresh columns")).clicked() {
                        cols.clear();
                        refresh_obj = true;
                    }
                });
                ui.add_space(4.0);
                ui.label(t("Заметка", "Note"));
                let r = ui.add(egui::TextEdit::multiline(note).desired_rows(4).desired_width(f32::INFINITY));
                if focus {
                    r.request_focus();
                }
            }
            Kind::Notebook { tab, title, flow, note, .. } => {
                ui.label(RichText::new(title.as_str()).strong());
                ui.horizontal(|ui| {
                    ui.radio_value(flow, false, t("карточка", "card"));
                    ui.radio_value(flow, true, t("блок-схема", "flowchart"));
                });
                ui.horizontal(|ui| {
                    if ui.button(t("Открыть вкладку", "Open the tab")).clicked() {
                        go_tab = Some(tab.clone());
                    }
                    if ui.button(t("Обновить ячейки", "Refresh cells")).on_hover_text(t("Взять ячейки из вкладки, как они сейчас", "Take the cells from the tab as they are now")).clicked() {
                        refresh_tab = Some(tab.clone());
                    }
                });
                ui.add_space(4.0);
                ui.label(t("Заметка", "Note"));
                let r = ui.add(egui::TextEdit::multiline(note).desired_rows(4).desired_width(f32::INFINITY));
                if focus {
                    r.request_focus();
                }
            }
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button(t("Наверх", "To front")).clicked() {
            let it = b.items.remove(pos);
            b.items.push(it);
        } else if ui.button(t("Вниз", "To back")).clicked() {
            let it = b.items.remove(pos);
            b.items.insert(0, it);
        }
    });
    if ui.add(crate::ui::tinted(t("Удалить", "Delete").into(), crate::ui::RED)).clicked() {
        b.remove(&[id]);
        app.board_ui.sel.clear();
    }
    if let Some((conn, path, obj, name)) = open_obj {
        open_object(app, &conn, &path, obj, &name);
    }
    if let Some(tab) = go_tab {
        go_to_tab(app, &tab);
    }
    if refresh_obj {
        if let Some(Kind::Object { conn, path, .. }) = b.item(id).map(|i| &i.kind) {
            let mut p = path.clone();
            p.push("columns".into());
            let conn = conn.clone();
            app.tree.remove(&(conn.clone(), p.clone()));
            let ctx = app.ctx.clone();
            if app.ensure_live(&ctx, &conn, None) {
                app.request_children(&conn, p);
            }
        }
    }
    if let Some(tab) = refresh_tab {
        match find_tab(app, &tab) {
            Some((wi, _, tb)) => {
                let d = app.dialect_of(app.tab_conn(&app.session.workspaces[wi], tb).as_deref());
                let (cells, name) = (cells_of(tb, d), tb.title.clone());
                if let Some(Kind::Notebook { cells: c, title, .. }) = b.item_mut(id).map(|i| &mut i.kind) {
                    *c = cells;
                    *title = name;
                }
            }
            None => app.toast(t("Этой вкладки больше нет", "That tab is gone")),
        }
    }
}
