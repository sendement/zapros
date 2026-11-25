//! A board's tab: the tools above, the library on the left (shapes,
//! lines, backgrounds, workspaces and their tabs, the databases' objects —
//! dragged onto the canvas or clicked), the canvas (panned by dragging
//! empty space or with the middle button, the wheel; zoomed with
//! Ctrl+wheel), what's picked set up on the right.

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::App;
use crate::board::{self, Backdrop, Board, Dash, Field, Item, Kind, LineStyle, Link, Part, Pattern, Shape, Snap, What, ARROW_W, CODE_LINE_H, HEAD_H, MAX_ROWS, NOTE_LINE_H, PAD, PART_W, ROW_H, SUB_HEAD};
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
            Tool::Arrow => t("Связь (A): от элемента к элементу; от поля таблицы к полю. Вид линии — в библиотеке слева", "Link (A): from item to item; from a table's field to a field. The line's look: in the library on the left"),
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

fn shape_name(s: Shape) -> &'static str {
    match s {
        Shape::Rect => t("Прямоугольник", "Rectangle"),
        Shape::Ellipse => t("Овал", "Ellipse"),
        Shape::Diamond => t("Ромб — условие", "Diamond: a condition"),
        Shape::Pill => t("Начало / конец", "Start / end"),
        Shape::Parallelogram => t("Ввод / вывод", "Input / output"),
        Shape::Cylinder => t("База данных", "Database"),
        Shape::Hexagon => t("Подготовка", "Preparation"),
        Shape::Note => t("Стикер", "Sticky note"),
    }
}

fn pattern_name(p: Pattern) -> &'static str {
    match p {
        Pattern::Plain => t("Зона — заливка", "Zone: plain"),
        Pattern::Dots => t("Зона в точку", "Zone: dots"),
        Pattern::Grid => t("Зона в клетку", "Zone: grid"),
        Pattern::Stripes => t("Зона в полоску", "Zone: stripes"),
    }
}

/// The lines in the library: their look and name.
fn line_presets() -> Vec<(LineStyle, &'static str)> {
    let s = LineStyle::default();
    vec![
        (s, t("Стрелка", "Arrow")),
        (LineStyle { end: false, ..s }, t("Линия", "Line")),
        (LineStyle { dash: Dash::Dashed, ..s }, t("Пунктирная стрелка", "Dashed arrow")),
        (LineStyle { dash: Dash::Dotted, end: false, ..s }, t("Точечная линия", "Dotted line")),
        (LineStyle { start: true, ..s }, t("Двусторонняя", "Both ways")),
        (LineStyle { width: 3.5, ..s }, t("Жирная стрелка", "Thick arrow")),
    ]
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
    /// A line's end dragged (true: its second).
    End(u64, bool),
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
    /// The look of links drawn next.
    pub line: LineStyle,
    pub sel: Vec<u64>,
    /// A link picked (instead of items).
    pub sel_link: Option<u64>,
    /// In the picked notebook or workspace: the statement picked (tab, part).
    pub sub: Option<(usize, usize)>,
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
    /// The library on the left shown.
    pub lib: bool,
}

impl Default for State {
    fn default() -> Self {
        Self { tool: Tool::Select, color: crate::config::COLORS[0], width: 3.0, line: LineStyle::default(), sel: Vec::new(), sel_link: None, sub: None, edit: None, edit_fresh: 0, gesture: Gesture::None, focus_prop: false, find: String::new(), lib: true }
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
    if app.board_ui.lib {
        let r = egui::Panel::left("board-lib")
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 0, right: 8, top: 0, bottom: 0 }))
            .show_separator_line(false)
            .resizable(true)
            .default_size(240.0)
            .size_range(180.0..=420.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("board-lib").auto_shrink([false, false]).show(ui, |ui| library(app, ui, &mut open));
            });
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        ui.painter().vline(r.response.rect.right() - 3.0, r.response.rect.y_range(), stroke);
    }
    if !app.board_ui.sel.is_empty() || app.board_ui.sel_link.is_some() {
        let r = egui::Panel::right("board-props")
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 10, right: 0, top: 0, bottom: 0 }))
            .show_separator_line(false)
            .resizable(false)
            .exact_size(300.0)
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
        {
            let tip = t("Библиотека: объекты баз, пространства, фигуры, линии и фоны — перетащите на холст", "The library: the databases' objects, workspaces, shapes, lines and backgrounds — drag them onto the canvas");
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
            let v = ui.visuals();
            let fill = if st.lib { v.selection.bg_fill.gamma_multiply(0.6) } else if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { Color32::TRANSPARENT };
            ui.painter().rect_filled(r, crate::ui::RADIUS, fill);
            let c = if st.lib || resp.hovered() { v.strong_text_color() } else { v.text_color() };
            let q = r.shrink(7.0);
            let s = Stroke::new(1.4, c);
            let half = q.width() / 2.0 - 1.5;
            ui.painter().rect_stroke(Rect::from_min_size(q.min, Vec2::splat(half)), 1.5, s, egui::StrokeKind::Middle);
            ui.painter().circle_stroke(Pos2::new(q.right() - half / 2.0, q.top() + half / 2.0), half / 2.0, s);
            ui.painter().add(egui::Shape::closed_line(vec![Pos2::new(q.left() + half / 2.0, q.bottom() - half), Pos2::new(q.left() + half, q.bottom()), Pos2::new(q.left(), q.bottom())], s));
            ui.painter().line_segment([Pos2::new(q.right() - half, q.bottom()), Pos2::new(q.right(), q.bottom() - half)], s);
            if resp.on_hover_text(tip).clicked() {
                st.lib = !st.lib;
            }
        }
        ui.separator();
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
        add.response.on_hover_text(t("Скрипт, ноутбук или пространство из вкладок, объект из базы (всё это есть и в библиотеке слева)", "A script, notebook or workspace from the tabs, an object from a database (all of these are in the library on the left too)"));
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
        Tool::Rect => shape_icon(p, r.shrink2(Vec2::new(0.0, 2.0)), Shape::Rect, c),
        Tool::Ellipse => shape_icon(p, r.shrink2(Vec2::new(0.0, 1.5)), Shape::Ellipse, c),
        Tool::Diamond => shape_icon(p, r, Shape::Diamond, c),
        Tool::Text => {
            p.text(r.center(), Align2::CENTER_CENTER, "T", FontId::proportional(16.0), c);
        }
        Tool::Brush => {
            let pts: Vec<Pos2> = (0..=12)
                .map(|k| {
                    let f = k as f32 / 12.0;
                    Pos2::new(r.left() + r.width() * f, r.center().y + (f * 6.5).sin() * r.height() * 0.3)
                })
                .collect();
            p.add(egui::Shape::line(pts, Stroke::new(2.0, c)));
        }
        Tool::Entity => entity_icon(p, r, c),
        Tool::Arrow => {
            styled_line(p, &[r.left_bottom(), r.right_top()], Stroke::new(1.5, c), &LineStyle::default(), 1.0);
        }
    }
}

fn entity_icon(p: &egui::Painter, r: Rect, c: Color32) {
    p.rect_stroke(r, 2.0, Stroke::new(1.5, c), egui::StrokeKind::Middle);
    p.rect_filled(Rect::from_min_max(r.min, Pos2::new(r.right(), r.top() + 4.0)), 2.0, c);
    p.hline(r.x_range(), r.top() + 8.5, Stroke::new(1.0, c));
}

/// A shape's outline in a small rect (for icons and tiles).
fn shape_icon(p: &egui::Painter, r: Rect, s: Shape, c: Color32) {
    draw_shape(p, r, s, Color32::TRANSPARENT, Stroke::new(1.5, c), 1.0);
}

/// A shape: filled and outlined.
fn draw_shape(p: &egui::Painter, r: Rect, shape: Shape, bg: Color32, line: Stroke, z: f32) {
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
            p.add(egui::Shape::convex_polygon(vec![r.center_top(), r.right_center(), r.center_bottom(), r.left_center()], bg, line));
        }
        Shape::Pill => {
            let round = (r.height() / 2.0).min(255.0);
            p.rect_filled(r, round, bg);
            p.rect_stroke(r, round, line, egui::StrokeKind::Middle);
        }
        Shape::Parallelogram => {
            let k = (r.height() * 0.35).min(r.width() * 0.25);
            p.add(egui::Shape::convex_polygon(vec![Pos2::new(r.left() + k, r.top()), r.right_top(), Pos2::new(r.right() - k, r.bottom()), r.left_bottom()], bg, line));
        }
        Shape::Hexagon => {
            let k = (r.height() * 0.4).min(r.width() * 0.22);
            p.add(egui::Shape::convex_polygon(
                vec![Pos2::new(r.left() + k, r.top()), Pos2::new(r.right() - k, r.top()), r.right_center(), Pos2::new(r.right() - k, r.bottom()), Pos2::new(r.left() + k, r.bottom()), r.left_center()],
                bg,
                line,
            ));
        }
        Shape::Cylinder => {
            let ry = (r.height() * 0.14).min(16.0 * z).max(2.0);
            let rx = r.width() / 2.0;
            let top_c = Pos2::new(r.center().x, r.top() + ry);
            let bot_c = Pos2::new(r.center().x, r.bottom() - ry);
            p.rect_filled(Rect::from_min_max(Pos2::new(r.left(), top_c.y), Pos2::new(r.right(), bot_c.y)), 0.0, bg);
            p.add(egui::Shape::ellipse_filled(bot_c, Vec2::new(rx, ry), bg));
            p.add(egui::Shape::ellipse_filled(top_c, Vec2::new(rx, ry), bg));
            p.add(egui::Shape::ellipse_stroke(top_c, Vec2::new(rx, ry), line));
            p.line_segment([Pos2::new(r.left(), top_c.y), Pos2::new(r.left(), bot_c.y)], line);
            p.line_segment([Pos2::new(r.right(), top_c.y), Pos2::new(r.right(), bot_c.y)], line);
            let arc: Vec<Pos2> = (0..=16).map(|k| {
                let a = std::f32::consts::PI * k as f32 / 16.0;
                Pos2::new(bot_c.x + rx * a.cos(), bot_c.y + ry * a.sin())
            }).collect();
            p.add(egui::Shape::line(arc, line));
        }
        Shape::Note => {
            let fold = (r.width().min(r.height()) * 0.18).min(18.0 * z);
            let pts = vec![r.left_top(), r.right_top(), Pos2::new(r.right(), r.bottom() - fold), Pos2::new(r.right() - fold, r.bottom()), r.left_bottom()];
            let solid = if bg == Color32::TRANSPARENT { bg } else { line.color.gamma_multiply(0.42) };
            p.add(egui::Shape::convex_polygon(pts, solid, line));
            p.add(egui::Shape::convex_polygon(vec![Pos2::new(r.right(), r.bottom() - fold), Pos2::new(r.right() - fold, r.bottom() - fold), Pos2::new(r.right() - fold, r.bottom())], line.color.gamma_multiply(0.7), Stroke::NONE));
        }
    }
}

/// An arrowhead at `b`, pointing away from `a`.
fn head(p: &egui::Painter, a: Pos2, b: Pos2, c: Color32, size: f32) {
    let d = (b - a).normalized();
    if d.length_sq() > 0.0 {
        let n = Vec2::new(-d.y, d.x);
        p.add(egui::Shape::convex_polygon(vec![b, b - d * size * 1.6 + n * size * 0.8, b - d * size * 1.6 - n * size * 0.8], c, Stroke::NONE));
    }
}

fn arrow(p: &egui::Painter, a: Pos2, b: Pos2, s: Stroke, size: f32) {
    p.line_segment([a, b], s);
    head(p, a, b, s.color, size);
}

/// A way drawn as the style says: solid, dashed or dotted, its heads.
fn styled_line(p: &egui::Painter, pts: &[Pos2], s: Stroke, style: &LineStyle, z: f32) {
    if pts.len() < 2 {
        return;
    }
    match style.dash {
        Dash::Solid => {
            p.add(egui::Shape::line(pts.to_vec(), s));
        }
        Dash::Dashed => {
            p.extend(egui::Shape::dashed_line(pts, s, (8.0 * z).max(3.0), (5.0 * z).max(2.0)));
        }
        Dash::Dotted => {
            p.extend(egui::Shape::dotted_line(pts, s.color, (s.width * 2.6).max(3.0), (s.width * 0.75).max(0.8)));
        }
    }
    let size = (3.0 + s.width * 1.6).clamp(3.0, 12.0);
    let n = pts.len();
    if style.end {
        head(p, pts[n - 2], pts[n - 1], s.color, size);
    }
    if style.start {
        head(p, pts[1], pts[0], s.color, size);
    }
}

/// "Add": a script, a notebook or a workspace from the tabs, an object of
/// a database from what the tree has seen.
fn add_menu(app: &mut App, ui: &mut egui::Ui, open: &mut board::Open) {
    ui.set_min_width(300.0);
    ui.label(RichText::new(t("Скрипт, ноутбук или пространство", "A script, notebook or workspace")).strong());
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("board-add-tabs").max_height(220.0).show(ui, |ui| {
        for ws in &app.session.workspaces {
            let tabs: Vec<&crate::config::Tab> = ws.tabs.iter().filter(|tb| placeable(tb)).collect();
            if tabs.is_empty() {
                continue;
            }
            if ui.button(RichText::new(format!("▦ {}", ws.name)).color(rgb(ws.color))).on_hover_text(t("Всё пространство: рамка с его вкладками", "The whole workspace: a frame with its tabs")).clicked() {
                picked = Some(board::Lib::Workspace(ws.id.clone()));
            }
            ui.indent(("add-ws", &ws.id), |ui| {
                for tb in tabs {
                    let mark = if tb.is_notebook() { "📓" } else { "📜" };
                    if ui.button(format!("{mark} {}", tb.title)).clicked() {
                        picked = Some(board::Lib::Tab(tb.id.clone()));
                    }
                }
            });
        }
    });
    if let Some(lib) = picked {
        let at = center_of_view(app, ui.ctx(), &open.board);
        add_lib(app, &mut open.board, &lib, at, false);
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

/// Tabs that can go on a board (not boards nor objects' designers, not empty).
fn placeable(tb: &crate::config::Tab) -> bool {
    tb.board.is_none() && tb.object.is_none() && (tb.is_notebook() || !tb.text.trim().is_empty())
}

/// The library: tiles and rows to drag onto the canvas (a click puts
/// them in the view's middle).
fn library(app: &mut App, ui: &mut egui::Ui, open: &mut board::Open) {
    let mut put: Option<board::Lib> = None;
    let strong = ui.visuals().strong_text_color();
    let accent = ui.visuals().selection.stroke.color;
    let c = rgb(app.board_ui.color);
    egui::CollapsingHeader::new(RichText::new(t("Базы данных", "Databases")).strong()).id_salt("lib-db").default_open(true).show(ui, |ui| {
        ui.label(RichText::new(t("Таблицы, вью, функции и процедуры перетаскиваются на холст", "Tables, views, functions and procedures drag onto the canvas")).small().weak());
        crate::side::tree_rows(app, ui);
    });
    egui::CollapsingHeader::new(RichText::new(t("Рабочие пространства", "Workspaces")).strong()).id_salt("lib-ws").default_open(true).show(ui, |ui| {
        ui.label(RichText::new(t("Перетащите пространство целиком, вкладку или ячейку ноутбука", "Drag a whole workspace, a tab or a notebook's cell")).small().weak());
        let wss: Vec<(String, String, Rgb, Vec<(String, String, bool, Vec<String>)>)> = app
            .session
            .workspaces
            .iter()
            .map(|ws| {
                let tabs = ws
                    .tabs
                    .iter()
                    .filter(|tb| placeable(tb))
                    .map(|tb| {
                        let cells = if tb.is_notebook() { tb.notebook.as_ref().map(|n| n.cells.iter().map(|c| c.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(40).collect()).collect()).unwrap_or_default() } else { Vec::new() };
                        (tb.id.clone(), tb.title.clone(), tb.is_notebook(), cells)
                    })
                    .collect();
                (ws.id.clone(), ws.name.clone(), ws.color, tabs)
            })
            .collect();
        for (wid, name, color, tabs) in wss {
            let key = egui::Id::new(("lib-ws-open", &wid));
            let mut opened = ui.ctx().data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
            let row = ui.horizontal(|ui| {
                if crate::ui::chevron(ui, opened).clicked() {
                    opened = !opened;
                }
                ui.colored_label(rgb(color), "▌");
                drag_row(ui, board::Lib::Workspace(wid.clone()), &name, true)
            });
            if row.inner {
                put = Some(board::Lib::Workspace(wid.clone()));
            }
            ui.ctx().data_mut(|d| d.insert_temp(key, opened));
            if !opened {
                continue;
            }
            ui.indent(("lib-ws", &wid), |ui| {
                if tabs.is_empty() {
                    ui.label(RichText::new(t("пусто", "empty")).small().weak());
                }
                for (tid, title, nb, cells) in tabs {
                    let mark = if nb { "📓" } else { "📜" };
                    if drag_row(ui, board::Lib::Tab(tid.clone()), &format!("{mark} {title}"), false) {
                        put = Some(board::Lib::Tab(tid.clone()));
                    }
                    if nb {
                        ui.indent(("lib-cells", &tid), |ui| {
                            for (k, first) in cells.iter().enumerate() {
                                let label = format!("{} {}", k + 1, if first.is_empty() { "…" } else { first });
                                if drag_row(ui, board::Lib::Cell(tid.clone(), k), &label, false) {
                                    put = Some(board::Lib::Cell(tid.clone(), k));
                                }
                            }
                        });
                    }
                }
            });
        }
    });
    egui::CollapsingHeader::new(RichText::new(t("Фигуры", "Shapes")).strong()).id_salt("lib-shapes").default_open(true).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for s in Shape::ALL {
                if tile(ui, board::Lib::Shape(s), shape_name(s), |p, r| draw_shape(p, r.shrink2(Vec2::new(4.0, 7.0)), s, c.gamma_multiply(0.16), Stroke::new(1.5, c), 0.6)) {
                    put = Some(board::Lib::Shape(s));
                }
            }
            if tile(ui, board::Lib::Text, t("Текст", "Text"), |p, r| {
                p.text(r.center(), Align2::CENTER_CENTER, "Aa", FontId::proportional(17.0), strong);
            }) {
                put = Some(board::Lib::Text);
            }
            if tile(ui, board::Lib::Entity, t("Таблица-сущность", "Table entity"), |p, r| entity_icon(p, r.shrink2(Vec2::new(9.0, 8.0)), c)) {
                put = Some(board::Lib::Entity);
            }
        });
    });
    egui::CollapsingHeader::new(RichText::new(t("Линии", "Lines")).strong()).id_salt("lib-lines").default_open(true).show(ui, |ui| {
        ui.label(RichText::new(t("Щелчок — так рисует инструмент «Связь»; перетащите — отдельная линия", "A click: how the Link tool draws; drag one: a line on its own")).small().weak());
        ui.horizontal_wrapped(|ui| {
            for (style, name) in line_presets() {
                let on = app.board_ui.line == style;
                let clicked = tile(ui, board::Lib::Line(style), name, |p, r| {
                    if on {
                        p.rect_stroke(r.shrink(1.0), 5.0, Stroke::new(1.5, accent), egui::StrokeKind::Inside);
                    }
                    styled_line(p, &[Pos2::new(r.left() + 7.0, r.bottom() - 9.0), Pos2::new(r.right() - 7.0, r.top() + 9.0)], Stroke::new(style.width, strong), &style, 0.7);
                });
                if clicked {
                    app.board_ui.line = style;
                    app.board_ui.tool = Tool::Arrow;
                }
            }
        });
    });
    egui::CollapsingHeader::new(RichText::new(t("Фоны", "Backgrounds")).strong()).id_salt("lib-backs").default_open(true).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for pat in [Pattern::Plain, Pattern::Dots, Pattern::Grid, Pattern::Stripes] {
                if tile(ui, board::Lib::Zone(pat), pattern_name(pat), |p, r| {
                    let r = r.shrink(5.0);
                    p.rect_filled(r, 4.0, c.gamma_multiply(0.12));
                    pattern(&p.with_clip_rect(r), r, pat, c.gamma_multiply(0.5), 0.5);
                    p.rect_stroke(r, 4.0, Stroke::new(1.0, c.gamma_multiply(0.7)), egui::StrokeKind::Middle);
                }) {
                    put = Some(board::Lib::Zone(pat));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(t("Фон доски:", "The board:")).small());
            for (b, name) in [(Backdrop::Dots, t("точки", "dots")), (Backdrop::Grid, t("клетка", "grid")), (Backdrop::Lines, t("линейка", "lines")), (Backdrop::Plain, t("чистый", "plain"))] {
                if ui.selectable_label(open.board.backdrop == b, name).clicked() {
                    open.board.backdrop = b;
                }
            }
        });
    });
    if let Some(lib) = put {
        let at = center_of_view(app, ui.ctx(), &open.board);
        add_lib(app, &mut open.board, &lib, at, false);
    }
}

/// A library tile: drawn by `paint`, dragged as `lib`; true: clicked.
fn tile(ui: &mut egui::Ui, lib: board::Lib, name: &str, paint: impl FnOnce(&egui::Painter, Rect)) -> bool {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(46.0, 40.0), Sense::click_and_drag());
    let v = ui.visuals();
    let fill = if resp.hovered() || resp.dragged() { v.widgets.hovered.weak_bg_fill } else { v.faint_bg_color };
    ui.painter().rect_filled(r, 6.0, fill);
    paint(ui.painter(), r);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    resp.dnd_set_drag_payload(lib);
    if resp.dragged() {
        crate::side::drag_ghost(ui.ctx(), name);
    }
    let resp = resp.on_hover_text(format!("{name}\n{}", t("перетащите на холст или щёлкните", "drag onto the canvas or click")));
    resp.clicked()
}

/// A library row (a workspace, a tab, a cell): dragged as `lib`; true:
/// clicked.
fn drag_row(ui: &mut egui::Ui, lib: board::Lib, label: &str, strong: bool) -> bool {
    let text = if strong { RichText::new(label).strong() } else { RichText::new(label) };
    let r = ui.add(egui::Label::new(text).sense(Sense::click_and_drag()).selectable(false).truncate());
    r.dnd_set_drag_payload(lib);
    if r.dragged() {
        crate::side::drag_ghost(ui.ctx(), label);
    }
    r.on_hover_text(t("Перетащите на холст или щёлкните", "Drag onto the canvas or click")).clicked()
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

/// A tab's cells as they are now: (markdown, text); a script: its text.
pub fn cells_of(tab: &crate::config::Tab) -> Vec<(bool, String)> {
    match &tab.notebook {
        Some(nb) if nb.on => nb.cells.iter().map(|c| (c.kind == crate::config::CellKind::Markdown, c.text.clone())).collect(),
        _ => vec![(false, tab.text.clone())],
    }
}

fn find_tab<'a>(app: &'a App, id: &str) -> Option<(usize, usize, &'a crate::config::Tab)> {
    app.session.workspaces.iter().enumerate().find_map(|(wi, ws)| ws.tabs.iter().position(|tb| tb.id == id).map(|ti| (wi, ti, &ws.tabs[ti])))
}

/// A tab (or one cell of its notebook) taken as it is now: its statements
/// and what they are (the hints' tables known for their columns).
pub fn snap_of(app: &App, tab: &str, cell: Option<usize>) -> Option<Snap> {
    let (wi, _, tb) = find_tab(app, tab)?;
    let conn = app.tab_conn(&app.session.workspaces[wi], tb);
    let d = app.dialect_of(conn.as_deref());
    let rels = app.hints_of(conn.as_deref()).rels;
    let mut cells = cells_of(tb);
    let title = match cell {
        Some(k) => {
            let c = cells.get(k)?.clone();
            cells = vec![c];
            format!("{} · {}", tb.title, k + 1)
        }
        None => tb.title.clone(),
    };
    let mut parts = board::parts_of(&cells, d, &rels);
    if let Some(k) = cell {
        for p in &mut parts {
            p.cell = k;
        }
    }
    Some(Snap { tab: tab.to_string(), title, cell, parts })
}

/// Taken anew, the blocks folded before kept folded.
fn refreshed(app: &App, old: &Snap) -> Option<Snap> {
    let mut s = snap_of(app, &old.tab, old.cell)?;
    for p in &mut s.parts {
        p.compact = old.parts.iter().any(|o| o.compact && o.sql == p.sql);
    }
    Some(s)
}

/// Puts an item of this kind at `at` (its middle; `exact`: right there,
/// else where it covers nothing), picked; its id.
fn place(app: &mut App, b: &mut Board, kind: Kind, w: f32, color: Rgb, at: Pos2, exact: bool) -> u64 {
    let mut probe = Item { id: 0, rect: [0.0, 0.0, w, 100.0], color, kind };
    let w = if matches!(probe.kind, Kind::Notebook { .. } | Kind::Workspace { .. }) { probe.natural_w() } else { w };
    probe.rect[2] = w;
    let size = Vec2::new(w, probe.natural_h().unwrap_or(100.0));
    let at = if exact { at } else { b.free_spot(at, size) };
    let id = b.add(Rect::from_center_size(at, size), color, probe.kind);
    app.board_ui.sel = vec![id];
    app.board_ui.sel_link = None;
    app.board_ui.sub = None;
    id
}

pub fn add_notebook(app: &mut App, b: &mut Board, tab: &str, at: Pos2) -> Option<u64> {
    add_snap(app, b, tab, None, at, false)
}

fn add_snap(app: &mut App, b: &mut Board, tab: &str, cell: Option<usize>, at: Pos2, exact: bool) -> Option<u64> {
    let snap = snap_of(app, tab, cell)?;
    let color = app.board_ui.color;
    Some(place(app, b, Kind::Notebook { snap, note: String::new() }, 340.0, color, at, exact))
}

/// A workspace put on the board at `at`.
pub fn put_ws(app: &mut App, b: &mut Board, ws: &str, at: Pos2) -> Option<u64> {
    add_workspace(app, b, ws, at, true)
}

fn add_workspace(app: &mut App, b: &mut Board, ws: &str, at: Pos2, exact: bool) -> Option<u64> {
    let w = app.session.workspaces.iter().find(|w| w.id == ws)?;
    let (name, color) = (w.name.clone(), w.color);
    let ids: Vec<String> = w.tabs.iter().filter(|tb| placeable(tb)).map(|tb| tb.id.clone()).collect();
    let tabs: Vec<Snap> = ids.iter().filter_map(|id| snap_of(app, id, None)).collect();
    Some(place(app, b, Kind::Workspace { ws: ws.to_string(), name, tabs, note: String::new() }, 600.0, color, at, exact))
}

/// Something from the library put on the board.
fn add_lib(app: &mut App, b: &mut Board, lib: &board::Lib, at: Pos2, exact: bool) {
    let color = app.board_ui.color;
    match lib {
        board::Lib::Workspace(ws) => {
            add_workspace(app, b, ws, at, exact);
            return;
        }
        board::Lib::Tab(tab) => {
            add_snap(app, b, tab, None, at, exact);
            return;
        }
        board::Lib::Cell(tab, k) => {
            add_snap(app, b, tab, Some(*k), at, exact);
            return;
        }
        _ => {}
    }
    let st = &mut app.board_ui;
    let id = match lib {
        board::Lib::Shape(s) => {
            let size = match s {
                Shape::Diamond => Vec2::new(160.0, 110.0),
                Shape::Note => Vec2::new(170.0, 150.0),
                Shape::Cylinder => Vec2::new(130.0, 120.0),
                _ => Vec2::new(180.0, 90.0),
            };
            let id = b.add(Rect::from_center_size(at, size), color, Kind::Frame { shape: *s, text: String::new(), fill: true });
            st.edit = Some(id);
            st.edit_fresh = 3;
            Some(id)
        }
        board::Lib::Text => {
            let id = b.add(Rect::from_min_size(at, Vec2::new(60.0, 20.0)), color, Kind::Text { text: String::new(), size: 18.0 });
            st.edit = Some(id);
            st.edit_fresh = 3;
            Some(id)
        }
        board::Lib::Entity => {
            let n = b.items.iter().filter(|i| matches!(i.kind, Kind::Entity { .. })).count() + 1;
            let id = b.add(Rect::from_center_size(at, Vec2::new(200.0, 80.0)), color, Kind::Entity { name: format!("{}{n}", t("таблица_", "table_")), fields: vec![Field { name: "id".into(), ty: "bigint".into(), key: true, note: String::new() }], note: String::new() });
            st.focus_prop = true;
            Some(id)
        }
        board::Lib::Line(style) => {
            let (a, z) = (at - Vec2::new(80.0, 0.0), at + Vec2::new(80.0, 0.0));
            Some(b.add(Rect::from_two_pos(a, z), color, Kind::Line { a: [a.x, a.y], b: [z.x, z.y], style: *style, label: String::new() }))
        }
        board::Lib::Zone(p) => Some(b.add(Rect::from_center_size(at, Vec2::new(480.0, 320.0)), color, Kind::Zone { title: t("Зона", "Zone").to_string(), pattern: *p })),
        board::Lib::Workspace(_) | board::Lib::Tab(_) | board::Lib::Cell(..) => None,
    };
    if let Some(id) = id {
        st.sel = vec![id];
        st.sel_link = None;
        st.sub = None;
        st.tool = Tool::Select;
    }
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
    let l = app.hints_of(Some(&d.conn));
    if let Some(r) = l.rels.iter().find(|r| r.name.eq_ignore_ascii_case(&bare) && (schema.is_empty() || r.schema.eq_ignore_ascii_case(&schema))) {
        return r.cols.clone();
    }
    if let Some(r) = l.routines.iter().find(|r| r.name.eq_ignore_ascii_case(&bare) && (schema.is_empty() || r.schema.eq_ignore_ascii_case(&schema))) {
        return r.args.iter().map(|a| (a.name.clone(), a.ty.clone())).collect();
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
    let color = app.board_ui.color;
    place(app, b, Kind::Object { conn: d.conn.clone(), path: d.path.clone(), obj: d.obj, name: d.name.clone(), cols, note: String::new() }, 240.0, color, at, exact)
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

/// A block of code's fold button (board units): its head's right end.
fn fold_button(r: Rect) -> Rect {
    Rect::from_min_max(Pos2::new(r.right() - 26.0, r.top()), Pos2::new(r.right(), r.top() + HEAD_H))
}

fn backdrop(painter: &egui::Painter, rect: Rect, view: View, kind: Backdrop, dark: bool) {
    let step = 24.0 * view.zoom;
    if step < 8.0 || kind == Backdrop::Plain {
        return;
    }
    let c = if dark { Color32::from_gray(48) } else { Color32::from_gray(214) };
    let first = view.w(rect.min);
    let (x0, y0) = ((first.x / 24.0).floor() * 24.0, (first.y / 24.0).floor() * 24.0);
    let line = Stroke::new(1.0, if dark { Color32::from_gray(36) } else { Color32::from_gray(232) });
    match kind {
        Backdrop::Dots => {
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
        Backdrop::Grid | Backdrop::Lines => {
            let mut y = y0;
            while view.s(Pos2::new(0.0, y)).y < rect.bottom() {
                painter.hline(rect.x_range(), view.s(Pos2::new(0.0, y)).y, line);
                y += 24.0;
            }
            if kind == Backdrop::Grid {
                let mut x = x0;
                while view.s(Pos2::new(x, 0.0)).x < rect.right() {
                    painter.vline(view.s(Pos2::new(x, 0.0)).x, rect.y_range(), line);
                    x += 24.0;
                }
            }
        }
        Backdrop::Plain => {}
    }
}

/// A zone's pattern over `r` (screen).
fn pattern(p: &egui::Painter, r: Rect, pat: Pattern, c: Color32, z: f32) {
    let sp = 16.0 * z.max(0.4);
    let s = Stroke::new(1.0, c);
    match pat {
        Pattern::Plain => {}
        Pattern::Dots => {
            let mut y = r.top() + sp / 2.0;
            while y < r.bottom() {
                let mut x = r.left() + sp / 2.0;
                while x < r.right() {
                    p.circle_filled(Pos2::new(x, y), 1.3, c);
                    x += sp;
                }
                y += sp;
            }
        }
        Pattern::Grid => {
            let mut y = r.top() + sp;
            while y < r.bottom() {
                p.hline(r.x_range(), y, s);
                y += sp;
            }
            let mut x = r.left() + sp;
            while x < r.right() {
                p.vline(x, r.y_range(), s);
                x += sp;
            }
        }
        Pattern::Stripes => {
            let mut x = r.left() - r.height();
            while x < r.right() {
                p.line_segment([Pos2::new(x, r.bottom()), Pos2::new(x + r.height(), r.top())], s);
                x += sp;
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
    backdrop(&painter, rect, view, b.backdrop, dark);
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
    // A line's ends (screen).
    let ends = |view: View, it: &Item| match &it.kind {
        Kind::Line { a, b, .. } => Some((Rect::from_center_size(view.s(Pos2::new(a[0], a[1])), Vec2::splat(12.0)), Rect::from_center_size(view.s(Pos2::new(b[0], b[1])), Vec2::splat(12.0)))),
        _ => None,
    };
    if resp.drag_started_by(egui::PointerButton::Primary) {
        // (Where the button went down: the drag counts a few pixels on.)
        if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()) {
            let w = view.w(p);
            let on = b.hit(w, slack);
            st.edit = None;
            st.gesture = match st.tool {
                Tool::Select => {
                    let one = (st.sel.len() == 1).then(|| st.sel[0]);
                    let end = one.and_then(|id| b.item(id)).and_then(|it| ends(view, it).map(|e| (it.id, e))).and_then(|(id, (ea, eb))| if eb.contains(p) { Some((id, true)) } else if ea.contains(p) { Some((id, false)) } else { None });
                    let resize = one.filter(|id| b.item(*id).is_some_and(|it| !matches!(it.kind, Kind::Line { .. }) && handle(view, it).contains(p)));
                    if let Some((id, which)) = end {
                        Gesture::End(id, which)
                    } else if let Some(id) = resize {
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
            Gesture::End(id, which) => {
                if let (Some(it), Some(w)) = (b.item_mut(*id), wp) {
                    it.set_end(*which, w);
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
                    let bb = pts.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_center_size(Pos2::new(p[0], p[1]), Vec2::splat(st.width))));
                    b.add(bb, st.color, Kind::Stroke { points: pts, width: st.width });
                }
            }
            Gesture::Link(from, field, to) => {
                if let Some(target) = b.items.iter().rev().find(|i| i.id != from && i.hit(to, slack)).map(|i| (i.id, i.row_at(to.y))) {
                    let id = b.take_id();
                    b.links.push(Link { id, from, to: target.0, from_field: field, to_field: target.1, label: String::new(), color: st.color, style: st.line });
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
                            if st.sel != [id] {
                                st.sub = None;
                            }
                            st.sel = vec![id];
                            st.sel_link = None;
                            // A statement inside: picked; a block's fold
                            // button: folds it.
                            if let Some(it) = b.item_mut(id) {
                                let hit = it.inside().parts.into_iter().find(|(_, _, r)| r.contains(w));
                                if let Some((k, j, r)) = hit {
                                    st.sub = Some((k, j));
                                    if fold_button(r).contains(w) {
                                        if let Some(part) = part_mut(it, k, j).filter(|p| matches!(p.what, What::Code { .. })) {
                                            part.compact = !part.compact;
                                        }
                                    }
                                }
                            }
                        }
                        None => {
                            // A link near the pointer, or nothing.
                            st.sel.clear();
                            st.sub = None;
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
            match b.hit(w, slack).and_then(|id| b.item(id)).map(|it| (it.id, it.kind.clone(), it.inside())) {
                Some((id, Kind::Frame { .. } | Kind::Text { .. }, _)) => {
                    st.edit = Some(id);
                    st.edit_fresh = 3;
                }
                Some((_, Kind::Entity { .. } | Kind::Zone { .. } | Kind::Line { .. }, _)) => st.focus_prop = true,
                Some((_, Kind::Object { conn, path, obj, name, .. }, _)) => open_object(app, &conn, &path, obj, &name),
                Some((_, Kind::Notebook { snap, .. }, _)) => go_to_tab(app, &snap.tab),
                Some((_, Kind::Workspace { tabs, .. }, inside)) => {
                    if let Some((k, _)) = inside.subs.iter().find(|(_, r)| r.contains(w)) {
                        go_to_tab(app, &tabs[*k].tab);
                    }
                }
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
    // Something from the tree or the library dropped here.
    // (Asked by type first: taking a payload of the other type drops it.)
    let from_tree = egui::DragAndDrop::has_payload_of_type::<board::Drop>(ui.ctx());
    if let Some(d) = if from_tree { resp.dnd_release_payload::<board::Drop>() } else { None } {
        if let Some(p) = pointer {
            let at = view.w(p);
            open.board.pan = [view.pan.x, view.pan.y];
            add_object(app, &mut open.board, &d, at, true);
            return finish(open, view);
        }
    }
    if let Some(lib) = if from_tree { None } else { resp.dnd_release_payload::<board::Lib>() } {
        if let Some(p) = pointer {
            let at = view.w(p);
            open.board.pan = [view.pan.x, view.pan.y];
            add_lib(app, &mut open.board, &lib, at, true);
            return finish(open, view);
        }
    }
    let st = &mut app.board_ui;
    let dropping = resp.dnd_hover_payload::<board::Drop>().is_some() || resp.dnd_hover_payload::<board::Lib>().is_some();
    if dropping {
        painter.rect_stroke(rect.shrink(2.0), 8.0, Stroke::new(2.0, ui.visuals().selection.stroke.color), egui::StrokeKind::Inside);
    }
    // Drawing: zones, links under the items, then the items in their order.
    let colors = Colors { text: ui.visuals().text_color(), strong: ui.visuals().strong_text_color(), weak: ui.visuals().weak_text_color(), card: crate::ui::card(ui.visuals()) };
    let accent = ui.visuals().selection.stroke.color;
    let b = &mut open.board;
    let zones = b.items.iter().take_while(|i| matches!(i.kind, Kind::Zone { .. })).count();
    let editing = st.edit;
    for it in b.items[..zones].iter_mut() {
        draw_item(&painter, ui, it, view, &colors, editing == Some(it.id), None);
    }
    for l in &b.links {
        let Some(path) = b.link_path(l) else { continue };
        let pts: Vec<Pos2> = path.iter().map(|p| view.s(*p)).collect();
        let picked = st.sel_link == Some(l.id);
        let line = Stroke::new((l.style.width * view.zoom).clamp(1.0, 6.0), rgb(l.color));
        if picked {
            painter.add(egui::Shape::line(pts.clone(), Stroke::new(6.0, accent.gamma_multiply(0.35))));
        }
        styled_line(&painter, &pts, line, &l.style, view.zoom);
        if !l.label.is_empty() && view.zoom > 0.35 {
            let n = pts.len();
            let k = (n - 1) / 2;
            label_on(&painter, pts[k], pts[k + 1], &l.label, view.zoom, &colors);
        }
    }
    for it in b.items[zones..].iter_mut() {
        let sub = if st.sel == [it.id] { st.sub } else { None };
        draw_item(&painter, ui, it, view, &colors, editing == Some(it.id), sub);
    }
    // The picked: framed, with a corner to resize by (one picked; a line:
    // its two ends).
    for id in &st.sel {
        if let Some(it) = b.item(*id) {
            if let Some((ea, eb)) = ends(view, it) {
                for e in [ea, eb] {
                    painter.circle_filled(e.center(), 5.0, accent);
                }
                continue;
            }
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
                styled_line(&painter, &[view.s(start), view.s(*to)], Stroke::new(st.line.width, c), &st.line, view.zoom);
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
    } else if st.tool == Tool::Select && st.sel.len() == 1 && pointer.is_some_and(|p| b.item(st.sel[0]).is_some_and(|it| handle(view, it).contains(p) || ends(view, it).is_some_and(|(a, z)| a.contains(p) || z.contains(p)))) {
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
                Kind::Frame { text, shape, .. } => {
                    let inner = if matches!(shape, Shape::Rect | Shape::Note | Shape::Pill) { r.shrink(8.0 * zoom) } else { r.shrink2(r.size() * 0.18) };
                    (text, 14.0, inner)
                }
                Kind::Text { text, size } => {
                    let s = *size;
                    (text, s, Rect::from_min_size(r.min, Vec2::new(r.width().max(160.0), r.height().max(s * zoom + 8.0))))
                }
                _ => return finish(open, view),
            };
            let te = egui::TextEdit::multiline(text).font(FontId::proportional(size * zoom)).frame(egui::Frame::NONE).desired_width(inner.width()).text_color(rgb(it.color).lerp_to_gamma(colors.strong, 0.3));
            let out = ui.put(inner, te);
            if fresh && !out.has_focus() {
                out.request_focus();
            }
            if (out.lost_focus() && !fresh) || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                st.edit = None;
            }
        }
        // An empty text left: gone.
        if st.edit.is_none() && b.item(id).is_some_and(|it| matches!(&it.kind, Kind::Text { text, .. } if text.trim().is_empty())) {
            b.remove(&[id]);
            st.sel.retain(|x| *x != id);
        }
    }
    finish(open, view);
}

fn part_mut(it: &mut Item, k: usize, j: usize) -> Option<&mut Part> {
    match &mut it.kind {
        Kind::Notebook { snap, .. } => snap.parts.get_mut(j),
        Kind::Workspace { tabs, .. } => tabs.get_mut(k)?.parts.get_mut(j),
        _ => None,
    }
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

struct Colors {
    text: Color32,
    strong: Color32,
    weak: Color32,
    card: Color32,
}

/// A label on a way's piece from `a` to `z` (screen), on a card.
fn label_on(p: &egui::Painter, a: Pos2, z: Pos2, text: &str, zoom: f32, col: &Colors) {
    let g = p.layout_no_wrap(text.to_string(), FontId::proportional(12.0 * zoom.max(0.6)), col.text);
    let m = a + (z - a) / 2.0;
    let r = Rect::from_center_size(m, g.size() + Vec2::new(8.0, 4.0));
    p.rect_filled(r, 4.0, col.card);
    p.galley(r.min + Vec2::new(4.0, 2.0), g, col.text);
}

fn draw_item(p: &egui::Painter, ui: &egui::Ui, it: &mut Item, view: View, col: &Colors, editing: bool, sub: Option<(usize, usize)>) {
    let z = view.zoom;
    let r = view.rect(it.rect());
    if !r.intersects(p.clip_rect()) && !matches!(it.kind, Kind::Text { .. }) {
        return;
    }
    let c = rgb(it.color);
    let line = Stroke::new((1.5 * z).clamp(1.0, 3.0), c);
    let legible = 12.0 * z >= 5.0;
    let inside = it.inside();
    match &it.kind {
        Kind::Frame { shape, text, fill } => {
            let bg = if *fill { c.gamma_multiply(0.16) } else { Color32::TRANSPARENT };
            draw_shape(p, r, *shape, bg, line, z);
            if !editing && legible && !text.is_empty() {
                let inner = if matches!(shape, Shape::Rect | Shape::Note | Shape::Pill) { r.shrink(8.0 * z) } else { r.shrink2(r.size() * 0.18) };
                let g = p.layout(text.clone(), FontId::proportional(14.0 * z), col.strong, inner.width().max(10.0));
                p.with_clip_rect(r.intersect(p.clip_rect())).galley(Pos2::new(inner.center().x - g.size().x / 2.0, inner.center().y - g.size().y / 2.0), g, col.strong);
            }
        }
        Kind::Text { text, size } => {
            let g = p.layout_no_wrap(if text.is_empty() { " ".into() } else { text.clone() }, FontId::proportional(size * z), c.lerp_to_gamma(col.strong, 0.3));
            let sz = g.size() / z;
            if !editing {
                p.galley(r.min, g, col.strong);
            }
            it.rect[2] = sz.x.max(20.0);
            it.rect[3] = sz.y.max(*size);
        }
        Kind::Stroke { points, width } => {
            let pts: Vec<Pos2> = points.iter().map(|q| view.s(Pos2::new(q[0], q[1]))).collect();
            p.add(egui::Shape::line(pts, Stroke::new(width * z, c)));
        }
        Kind::Line { a, b, style, label } => {
            let (pa, pb) = (view.s(Pos2::new(a[0], a[1])), view.s(Pos2::new(b[0], b[1])));
            styled_line(p, &[pa, pb], Stroke::new((style.width * z).clamp(1.0, 8.0), c), style, z);
            if !label.is_empty() && legible {
                label_on(p, pa, pb, label, z, col);
            }
        }
        Kind::Zone { title, pattern: pat } => {
            p.rect_filled(r, 10.0 * z, c.gamma_multiply(0.07));
            pattern(&p.with_clip_rect(r.shrink(2.0).intersect(p.clip_rect())), r, *pat, c.gamma_multiply(0.22), z);
            p.rect_stroke(r, 10.0 * z, Stroke::new(1.2, c.gamma_multiply(0.55)), egui::StrokeKind::Middle);
            if legible && !title.is_empty() {
                p.text(r.min + Vec2::new(12.0 * z, HEAD_H * z / 2.0), Align2::LEFT_CENTER, title, FontId::proportional(15.0 * z), c);
            }
        }
        Kind::Entity { name, fields, note } => {
            let rows: Vec<(String, String, bool)> = fields.iter().map(|f| (f.name.clone(), f.ty.clone(), f.key)).collect();
            table_card(p, ui, r, z, c, name, None, &rows, note, col);
        }
        Kind::Object { name, obj, cols, note, .. } => {
            let rows: Vec<(String, String, bool)> = cols.iter().map(|(n, ty)| (n.clone(), ty.clone(), false)).collect();
            let rows = if rows.is_empty() { vec![(t("колонки — после подключения", "columns: once connected").to_string(), String::new(), false)] } else { rows };
            table_card(p, ui, r, z, c, name, Some(*obj), &rows, note, col);
        }
        Kind::Notebook { snap, note } => {
            if snap.alone() {
                draw_part(p, ui, view.rect(inside.parts[0].2), z, c, &snap.parts[0], Some(&snap.title), col, sub.is_some());
                note_text(p, r, z, note, col.weak);
                return;
            }
            frame_head(p, ui, r, z, c, &snap.title, if snap.parts.iter().any(|x| x.cell > 0) { crate::icons::Ic::Notebook } else { crate::icons::Ic::File }, col);
            let clip = p.with_clip_rect(r.intersect(p.clip_rect()));
            for (_, j, pr) in &inside.parts {
                draw_part(&clip, ui, view.rect(*pr), z, c, &snap.parts[*j], None, col, sub == Some((0, *j)));
            }
            if snap.parts.is_empty() && legible {
                clip.text(r.min + Vec2::new(PAD, HEAD_H + PAD) * z, Align2::LEFT_TOP, t("пусто", "empty"), FontId::proportional(12.0 * z), col.weak);
            }
            note_text(&clip, r, z, note, col.weak);
        }
        Kind::Workspace { name, tabs, note, .. } => {
            p.rect_filled(r, 12.0 * z, c.gamma_multiply(0.06));
            p.rect_stroke(r, 12.0 * z, Stroke::new((2.0 * z).clamp(1.0, 3.0), c), egui::StrokeKind::Middle);
            let head = Rect::from_min_size(r.min, Vec2::new(r.width(), HEAD_H * z));
            let round = (12.0 * z).min(255.0) as u8;
            p.rect_filled(head, egui::CornerRadius { nw: round, ne: round, sw: 0, se: 0 }, c.gamma_multiply(0.85));
            if legible {
                p.text(Pos2::new(head.left() + 12.0 * z, head.center().y), Align2::LEFT_CENTER, format!("▦ {name}"), FontId::proportional(14.0 * z), Color32::WHITE);
            }
            let clip = p.with_clip_rect(r.intersect(p.clip_rect()));
            for (k, sr) in &inside.subs {
                let s = view.rect(*sr);
                clip.rect_filled(s, 8.0 * z, col.card);
                clip.rect_stroke(s, 8.0 * z, Stroke::new(1.0, c.gamma_multiply(0.6)), egui::StrokeKind::Middle);
                if legible {
                    let mark = if tabs[*k].parts.iter().any(|x| x.cell > 0) || tabs[*k].parts.iter().any(|x| matches!(x.what, What::Note)) { "📓" } else { "📜" };
                    clip.with_clip_rect(s.intersect(clip.clip_rect())).text(s.min + Vec2::new(10.0 * z, SUB_HEAD * z / 2.0), Align2::LEFT_CENTER, format!("{mark} {}", tabs[*k].title), FontId::proportional(13.0 * z), col.strong);
                    if tabs[*k].parts.is_empty() {
                        clip.text(s.min + Vec2::new(PAD, SUB_HEAD + PAD) * z, Align2::LEFT_TOP, t("пусто", "empty"), FontId::proportional(12.0 * z), col.weak);
                    }
                }
            }
            for (k, j, pr) in &inside.parts {
                draw_part(&clip, ui, view.rect(*pr), z, c, &tabs[*k].parts[*j], None, col, sub == Some((*k, *j)));
            }
            note_text(&clip, r, z, note, col.weak);
        }
    }
}

/// A notebook's frame: a card with a coloured head (an icon, the title).
#[allow(clippy::too_many_arguments)]
fn frame_head(p: &egui::Painter, ui: &egui::Ui, r: Rect, z: f32, c: Color32, title: &str, icon: crate::icons::Ic, col: &Colors) {
    p.rect_filled(r, 8.0 * z, col.card);
    p.rect_stroke(r, 8.0 * z, Stroke::new((1.5 * z).clamp(1.0, 3.0), c), egui::StrokeKind::Middle);
    let head = Rect::from_min_size(r.min, Vec2::new(r.width(), HEAD_H * z));
    let round = (8.0 * z).min(255.0) as u8;
    p.rect_filled(head, egui::CornerRadius { nw: round, ne: round, sw: 0, se: 0 }, c.gamma_multiply(0.85));
    if 12.0 * z >= 5.0 {
        let ic = Rect::from_center_size(Pos2::new(head.left() + 14.0 * z, head.center().y), Vec2::splat(16.0 * z));
        if !crate::icons::paint(ui, ic, icon, 15.0 * z, Color32::WHITE) {
            p.text(ic.center(), Align2::CENTER_CENTER, "📓", FontId::proportional(13.0 * z), Color32::WHITE);
        }
        p.with_clip_rect(head.intersect(p.clip_rect())).text(Pos2::new(head.left() + 26.0 * z, head.center().y), Align2::LEFT_CENTER, title, FontId::proportional(13.5 * z), Color32::WHITE);
    }
}

/// Columns as a table's rows, the ones past the most shown counted.
fn rows_of(cols: &[(String, String)]) -> Vec<(String, String, bool)> {
    let mut v: Vec<(String, String, bool)> = cols.iter().take(MAX_ROWS).map(|(n, ty)| (n.clone(), ty.clone(), false)).collect();
    if cols.len() > MAX_ROWS {
        v.push((format!("… {} {}", t("ещё", "more"), cols.len() - MAX_ROWS), String::new(), false));
    }
    v
}

/// A statement drawn in `r` (screen): a query's table; INSERT's scheme
/// (where from → into which); a change's (UPDATE → the table); a block's
/// code (folded: its first line); a markdown cell's text.
#[allow(clippy::too_many_arguments)]
fn draw_part(p: &egui::Painter, ui: &egui::Ui, r: Rect, z: f32, c: Color32, part: &Part, title: Option<&str>, col: &Colors, picked: bool) {
    let legible = 12.0 * z >= 5.0;
    let accent = ui.visuals().selection.stroke.color;
    match &part.what {
        What::Query { title: t0, cols } => {
            table_card(p, ui, r, z, c, title.unwrap_or(t0), None, &rows_of(cols), "", col);
        }
        What::Insert { verb, target, cols, source, values } => {
            p.rect_filled(r, 8.0 * z, c.gamma_multiply(0.05));
            p.rect_stroke(r, 8.0 * z, Stroke::new(1.0, c.gamma_multiply(0.5)), egui::StrokeKind::Middle);
            if legible {
                p.text(r.min + Vec2::new(10.0 * z, SUB_HEAD * z / 2.0), Align2::LEFT_CENTER, format!("{verb} → {target}"), FontId::monospace(11.5 * z), col.strong);
            }
            let top = r.top() + (SUB_HEAD + PAD) * z;
            let from_h = source.as_ref().map_or(HEAD_H + ROW_H + 6.0, |(_, c)| board::table_h(c.len()));
            let from = Rect::from_min_size(Pos2::new(r.left() + PAD * z, top), Vec2::new(PART_W, from_h) * z);
            let into = Rect::from_min_size(Pos2::new(from.right() + ARROW_W * z, top), Vec2::new(PART_W, board::table_h(cols.len())) * z);
            match source {
                Some((st, sc)) => table_card(p, ui, from, z, c, st, None, &rows_of(sc), "", col),
                None => {
                    let n = format!("{values} {}", t("строк(и)", "row(s)"));
                    table_card(p, ui, from, z, c, "VALUES", None, &[(n, String::new(), false)], "", col);
                }
            }
            table_card(p, ui, into, z, c, target, Some(ObjKind::Table), &rows_of(cols), "", col);
            let y = top + (HEAD_H / 2.0) * z;
            let (a, b) = (Pos2::new(from.right() + 4.0 * z, y), Pos2::new(into.left() - 4.0 * z, y));
            arrow(p, a, b, Stroke::new((1.8 * z).clamp(1.0, 3.0), c), (5.0 * z).clamp(3.0, 8.0));
            if legible {
                p.text(Pos2::new((a.x + b.x) / 2.0, y - 4.0 * z), Align2::CENTER_BOTTOM, verb.split(' ').next().unwrap_or(verb), FontId::proportional(10.5 * z), col.weak);
            }
        }
        What::Change { verb, target, cols } => {
            p.rect_filled(r, 8.0 * z, c.gamma_multiply(0.05));
            p.rect_stroke(r, 8.0 * z, Stroke::new(1.0, c.gamma_multiply(0.5)), egui::StrokeKind::Middle);
            if legible {
                p.text(r.min + Vec2::new(10.0 * z, SUB_HEAD * z / 2.0), Align2::LEFT_CENTER, format!("{verb} {target}"), FontId::monospace(11.5 * z), col.strong);
            }
            let top = r.top() + (SUB_HEAD + PAD) * z;
            let bx = Rect::from_min_size(Pos2::new(r.left() + PAD * z, top), Vec2::new(PART_W * 0.6, 56.0) * z);
            p.rect_filled(bx, 6.0 * z, c.gamma_multiply(0.22));
            p.rect_stroke(bx, 6.0 * z, Stroke::new(1.2, c), egui::StrokeKind::Middle);
            if legible {
                p.text(bx.center(), Align2::CENTER_CENTER, verb, FontId::proportional(12.5 * z), col.strong);
            }
            let into = Rect::from_min_size(Pos2::new(bx.right() + ARROW_W * z, top), Vec2::new(PART_W, board::table_h(cols.len())) * z);
            table_card(p, ui, into, z, c, target, Some(ObjKind::Table), &rows_of(cols), "", col);
            arrow(p, Pos2::new(bx.right() + 4.0 * z, bx.center().y), Pos2::new(into.left() - 4.0 * z, bx.center().y), Stroke::new((1.8 * z).clamp(1.0, 3.0), c), (5.0 * z).clamp(3.0, 8.0));
        }
        What::Code { title: t0 } => {
            card_with_head(p, r, z, c, title.unwrap_or(t0), col);
            if legible {
                let b = fold_button_screen(r, z);
                p.text(b.center(), Align2::CENTER_CENTER, if part.compact { "▸" } else { "▾" }, FontId::proportional(14.0 * z), Color32::WHITE);
                let n = part.sql.lines().count();
                let txt = if part.compact { format!("{}\n… {} {}", part.sql.lines().next().unwrap_or(""), n.saturating_sub(1), t("строк ещё — ▸ развернуть", "more lines — ▸ to unfold")) } else { part.sql.clone() };
                code_text(p, r, z, &txt, col);
            }
        }
        What::Other { title: t0 } => {
            card_with_head(p, r, z, c, t0, col);
            if legible {
                let txt: String = part.sql.lines().take(4).collect::<Vec<_>>().join("\n");
                code_text(p, r, z, &txt, col);
            }
        }
        What::Note => {
            p.vline(r.left(), r.y_range(), Stroke::new(2.0 * z.min(1.0), c.gamma_multiply(0.5)));
            if legible {
                let g = p.layout(part.sql.clone(), FontId::proportional(12.5 * z), col.text, r.width() - 12.0 * z);
                p.with_clip_rect(r.intersect(p.clip_rect())).galley(r.min + Vec2::new(8.0 * z, 4.0 * z), g, col.text);
            }
        }
    }
    if picked {
        p.rect_stroke(r.expand(2.0), 6.0, Stroke::new(1.5, accent.gamma_multiply(0.8)), egui::StrokeKind::Middle);
    }
}

fn fold_button_screen(r: Rect, z: f32) -> Rect {
    Rect::from_min_max(Pos2::new(r.right() - 26.0 * z, r.top()), Pos2::new(r.right(), r.top() + HEAD_H * z))
}

fn card_with_head(p: &egui::Painter, r: Rect, z: f32, c: Color32, title: &str, col: &Colors) {
    p.rect_filled(r, 6.0 * z, col.card);
    p.rect_stroke(r, 6.0 * z, Stroke::new((1.4 * z).clamp(1.0, 2.5), c), egui::StrokeKind::Middle);
    let head = Rect::from_min_size(r.min, Vec2::new(r.width(), HEAD_H * z));
    let round = (6.0 * z).min(255.0) as u8;
    p.rect_filled(head, egui::CornerRadius { nw: round, ne: round, sw: 0, se: 0 }, c.gamma_multiply(0.85));
    if 12.0 * z >= 5.0 {
        p.with_clip_rect(Rect::from_min_max(head.min, Pos2::new(head.right() - 26.0 * z, head.bottom())).intersect(p.clip_rect())).text(Pos2::new(head.left() + 9.0 * z, head.center().y), Align2::LEFT_CENTER, title, FontId::monospace(11.5 * z), Color32::WHITE);
    }
}

fn code_text(p: &egui::Painter, r: Rect, z: f32, txt: &str, col: &Colors) {
    let g = p.layout_no_wrap(txt.to_string(), FontId::monospace(11.0 * z), col.text);
    let body = Rect::from_min_max(Pos2::new(r.left(), r.top() + HEAD_H * z), r.max);
    p.with_clip_rect(body.shrink(2.0).intersect(p.clip_rect())).galley(body.min + Vec2::new(9.0 * z, 5.0 * z), g, col.text);
    let _ = CODE_LINE_H;
}

/// A table's card: a coloured head (its icon and name), a row per field
/// (a key's dot, the name, the type), the note below.
#[allow(clippy::too_many_arguments)]
fn table_card(p: &egui::Painter, ui: &egui::Ui, r: Rect, z: f32, c: Color32, name: &str, obj: Option<ObjKind>, rows: &[(String, String, bool)], note: &str, col: &Colors) {
    p.rect_filled(r, 6.0 * z, col.card);
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
            clip.hline(r.x_range().shrink(6.0 * z), y - ROW_H * z / 2.0, Stroke::new(0.5, col.weak.gamma_multiply(0.4)));
        }
        if *key {
            clip.circle_filled(Pos2::new(r.left() + 10.0 * z, y), 3.5 * z, c);
        }
        let tw = if ty.is_empty() { 0.0 } else { clip.layout_no_wrap(ty.clone(), FontId::monospace(11.0 * z), col.weak).size().x + 12.0 * z };
        let name_clip = Rect::from_min_max(Pos2::new(r.left(), y - ROW_H * z / 2.0), Pos2::new(r.right() - tw, y + ROW_H * z / 2.0));
        clip.with_clip_rect(name_clip.intersect(clip.clip_rect())).text(Pos2::new(r.left() + 19.0 * z, y), Align2::LEFT_CENTER, n, FontId::proportional(12.5 * z), col.text);
        if !ty.is_empty() {
            clip.text(Pos2::new(r.right() - 8.0 * z, y), Align2::RIGHT_CENTER, ty, FontId::monospace(11.0 * z), col.weak);
        }
    }
    note_text(&clip, r, z, note, col.weak);
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

/// How a line or a link is drawn: dash, heads, width.
fn style_props(ui: &mut egui::Ui, s: &mut LineStyle) {
    ui.horizontal(|ui| {
        ui.radio_value(&mut s.dash, Dash::Solid, t("сплошная", "solid"));
        ui.radio_value(&mut s.dash, Dash::Dashed, t("пунктир", "dashed"));
        ui.radio_value(&mut s.dash, Dash::Dotted, t("точки", "dotted"));
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.start, t("стрелка в начале", "head at start"));
        ui.checkbox(&mut s.end, t("в конце", "at end"));
    });
    ui.horizontal(|ui| {
        ui.label(t("Толщина", "Width"));
        ui.add(egui::Slider::new(&mut s.width, 1.0..=8.0));
    });
}

/// A statement's script, read only, in code's font.
fn script_box(ui: &mut egui::Ui, sql: &str) {
    let mut s = sql;
    let rows = sql.lines().count().clamp(2, 18);
    ui.add(egui::TextEdit::multiline(&mut s).code_editor().desired_rows(rows).desired_width(f32::INFINITY));
}

fn part_kind_name(w: &What) -> String {
    match w {
        What::Query { title, cols } => format!("SELECT · {title} · {} {}", cols.len(), t("кол.", "cols")),
        What::Insert { verb, target, .. } => format!("{verb} → {target}"),
        What::Change { verb, target, .. } => format!("{verb} {target}"),
        What::Code { title } => title.clone(),
        What::Note => t("Заметка (markdown)", "Note (markdown)").to_string(),
        What::Other { title } => title.clone(),
    }
}

/// A notebook's or a workspace tab's statements: the picked one first,
/// open, then all of them folded.
fn parts_props(ui: &mut egui::Ui, parts: &mut [Part], picked: Option<usize>, salt: &str) {
    if let Some(j) = picked.filter(|j| *j < parts.len()) {
        let part = &mut parts[j];
        ui.label(RichText::new(part_kind_name(&part.what)).strong());
        if matches!(part.what, What::Code { .. }) {
            ui.checkbox(&mut part.compact, t("Компактно — только первая строка", "Compact: the first line only"));
        }
        script_box(ui, &part.sql);
        ui.add_space(6.0);
    }
    ui.label(RichText::new(format!("{} ({})", t("Скрипты", "Scripts"), parts.len())).small().weak());
    for (j, part) in parts.iter_mut().enumerate() {
        egui::CollapsingHeader::new(part_kind_name(&part.what)).id_salt((salt, j)).show(ui, |ui| {
            if matches!(part.what, What::Code { .. }) {
                ui.checkbox(&mut part.compact, t("Компактно", "Compact"));
            }
            script_box(ui, &part.sql);
        });
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
        style_props(ui, &mut l.style);
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
    let sub = st.sub;
    let mut open_obj = None;
    let mut go_tab = None;
    let mut refresh = false;
    let mut refresh_obj = false;
    {
        let it = &mut b.items[pos];
        match &mut it.kind {
            Kind::Frame { shape, text, fill } => {
                ui.label(RichText::new(t("Фигура", "Shape")).strong());
                egui::ComboBox::from_id_salt("board-shape").selected_text(shape_name(*shape)).show_ui(ui, |ui| {
                    for s in Shape::ALL {
                        ui.selectable_value(shape, s, shape_name(s));
                    }
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
            Kind::Line { style, label, .. } => {
                ui.label(RichText::new(t("Линия", "Line")).strong());
                ui.label(RichText::new(t("Концы тянутся мышью", "Drag its ends with the mouse")).small().weak());
                ui.label(t("Подпись", "Label"));
                let r = ui.add(egui::TextEdit::singleline(label).desired_width(f32::INFINITY));
                if focus {
                    r.request_focus();
                }
                style_props(ui, style);
            }
            Kind::Zone { title, pattern: pat } => {
                ui.label(RichText::new(t("Зона", "Zone")).strong());
                ui.label(RichText::new(t("Фон под остальным: берётся за заголовок или край", "A background under the rest: grab it by its title or edge")).small().weak());
                let r = ui.add(egui::TextEdit::singleline(title).desired_width(f32::INFINITY));
                if focus {
                    r.request_focus();
                }
                egui::ComboBox::from_id_salt("board-pattern").selected_text(pattern_name(*pat)).show_ui(ui, |ui| {
                    for x in [Pattern::Plain, Pattern::Dots, Pattern::Grid, Pattern::Stripes] {
                        ui.selectable_value(pat, x, pattern_name(x));
                    }
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
            Kind::Notebook { snap, note } => {
                ui.label(RichText::new(snap.title.as_str()).strong());
                ui.horizontal(|ui| {
                    if ui.button(t("Открыть вкладку", "Open the tab")).on_hover_text(t("Двойной щелчок по элементу — то же", "Double-clicking the item does the same")).clicked() {
                        go_tab = Some(snap.tab.clone());
                    }
                    if ui.button(t("Обновить", "Refresh")).on_hover_text(t("Взять скрипт из вкладки, как он сейчас", "Take the script from the tab as it is now")).clicked() {
                        refresh = true;
                    }
                });
                ui.label(t("Заметка", "Note"));
                let r = ui.add(egui::TextEdit::multiline(note).desired_rows(3).desired_width(f32::INFINITY));
                if focus {
                    r.request_focus();
                }
                ui.add_space(6.0);
                parts_props(ui, &mut snap.parts, sub.map(|s| s.1), "nb-parts");
            }
            Kind::Workspace { name, tabs, note, .. } => {
                ui.label(RichText::new(format!("▦ {name}")).strong());
                if ui.button(t("Обновить", "Refresh")).on_hover_text(t("Взять вкладки пространства, как они сейчас", "Take the workspace's tabs as they are now")).clicked() {
                    refresh = true;
                }
                ui.label(t("Заметка", "Note"));
                let r = ui.add(egui::TextEdit::multiline(note).desired_rows(3).desired_width(f32::INFINITY));
                if focus {
                    r.request_focus();
                }
                ui.add_space(6.0);
                match sub.and_then(|(k, j)| tabs.get_mut(k).map(|s| (s, j))) {
                    Some((snap, j)) => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(snap.title.as_str()).strong());
                            if ui.small_button(t("к вкладке", "to the tab")).clicked() {
                                go_tab = Some(snap.tab.clone());
                            }
                        });
                        parts_props(ui, &mut snap.parts, Some(j), "ws-parts");
                    }
                    None => {
                        ui.label(RichText::new(t("Щёлкните скрипт внутри — здесь будет его текст", "Click a script inside: its text shows here")).small().weak());
                    }
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
    if refresh {
        refresh_item(app, b, id);
    }
}

/// A notebook's or a workspace's scripts taken anew from the tabs.
fn refresh_item(app: &mut App, b: &mut Board, id: u64) {
    let Some(kind) = b.item(id).map(|i| i.kind.clone()) else { return };
    match kind {
        Kind::Notebook { snap, .. } => match refreshed(app, &snap) {
            Some(new) => {
                if let Some(Kind::Notebook { snap, .. }) = b.item_mut(id).map(|i| &mut i.kind) {
                    *snap = new;
                }
            }
            None => app.toast(t("Этой вкладки больше нет", "That tab is gone")),
        },
        Kind::Workspace { ws, tabs, .. } => {
            let Some(w) = app.session.workspaces.iter().find(|w| w.id == ws) else {
                app.toast(t("Этого пространства больше нет", "That workspace is gone"));
                return;
            };
            let name = w.name.clone();
            let ids: Vec<String> = w.tabs.iter().filter(|tb| placeable(tb)).map(|tb| tb.id.clone()).collect();
            let new: Vec<Snap> = ids
                .iter()
                .filter_map(|tid| match tabs.iter().find(|s| s.tab == *tid) {
                    Some(old) => refreshed(app, old),
                    None => snap_of(app, tid, None),
                })
                .collect();
            if let Some(Kind::Workspace { tabs, name: n, .. }) = b.item_mut(id).map(|i| &mut i.kind) {
                *tabs = new;
                *n = name;
            }
        }
        _ => {}
    }
    app.board_ui.sub = None;
}
