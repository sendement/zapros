//! The window's parts: menu, the two rows of tabs (workspaces, then the
//! active workspace's scripts), the toolbar, the editor over the results,
//! the status line.

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};

use crate::app::{App, ConnState, TxThen};
use crate::config::{COLORS, Tab, Workspace};
use crate::editor::{self, Action, rgb};
use crate::i18n::t;
use crate::sql::check::Sev;



pub fn conn_dot(app: &App, conn: &str) -> (Color32, String) {
    match app.live.get(conn).map(|l| &l.state) {
        None => (Color32::GRAY, t("не подключено", "not connected").into()),
        Some(ConnState::Connecting) => (Color32::from_rgb(240, 180, 40), t("подключение…", "connecting…").into()),
        Some(ConnState::Ready) => (Color32::from_rgb(70, 190, 90), app.live[conn].version.clone()),
        Some(ConnState::Failed(m)) => (Color32::from_rgb(230, 70, 70), m.clone()),
    }
}

/// Height of the row of workspace tabs (and of the icons beside it).
pub const ROW_H: f32 = 32.0;
const SHEET_R: f32 = 8.0;
const TAB_R: f32 = 8.0;
const FILLET: f32 = 6.0;
/// Where the first workspace tab starts, from the sheet's left edge.
const TABS_FROM: f32 = 16.0;

/// One look for every field, button, list and menu: corners, height,
/// padding (the painted tabs and icons use the same radius).
pub const RADIUS: u8 = 6;

pub fn standard(s: &mut egui::Style) {
    let r = egui::CornerRadius::same(RADIUS);
    s.spacing.interact_size.y = 24.0;
    s.spacing.button_padding = Vec2::new(8.0, 3.0);
    s.spacing.item_spacing = Vec2::new(6.0, 4.0);
    s.spacing.menu_margin = egui::Margin::same(6);
    s.spacing.window_margin = egui::Margin::same(12);
    s.spacing.combo_width = 120.0;
    let v = &mut s.visuals;
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = r;
    }
    // Fields and buttons edged alike when at rest.
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color);
    v.window_corner_radius = egui::CornerRadius::same(10);
    v.menu_corner_radius = egui::CornerRadius::same(8);
}

/// The window's backdrop: the side panel and around the sheet (a little
/// darker than the sheet the editor and results lie on).
pub fn chrome(v: &egui::Visuals) -> Color32 {
    let p = v.panel_fill;
    let k = if v.dark_mode { 0.70 } else { 0.91 };
    Color32::from_rgb((p.r() as f32 * k) as u8, (p.g() as f32 * k) as u8, (p.b() as f32 * k) as u8)
}

fn elided(ui: &egui::Ui, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat { font_id: font, color, ..Default::default() });
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// A square icon button of the side panel's row; `on`: shown pressed.
fn icon(ui: &mut egui::Ui, glyph: &str, on: bool, tip: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(ROW_H - 2.0), egui::Sense::click());
    let v = ui.visuals();
    let fill = if on {
        v.selection.bg_fill.gamma_multiply(if v.dark_mode { 0.55 } else { 0.35 })
    } else if r.hovered() || r.has_focus() {
        v.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, RADIUS, fill);
    let color = if on || r.hovered() { v.strong_text_color() } else { v.text_color() };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(17.0), color);
    r.on_hover_text(tip)
}

/// A menu dropping from an icon.
fn icon_menu(ui: &mut egui::Ui, glyph: &str, tip: &str, add: impl FnOnce(&mut egui::Ui)) {
    let r = icon(ui, glyph, false, tip);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&r));
    if open {
        ui.painter().rect_stroke(r.rect, RADIUS, ui.visuals().widgets.hovered.bg_stroke, egui::StrokeKind::Inside);
    }
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(220.0);
        add(ui)
    });
}

/// The side panel's top row, level with the workspace tabs: what the
/// panel shows (objects / files), then the menus as icons.
pub fn icon_bar(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.horizontal(|ui| {
        ui.set_height(ROW_H);
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon(ui, "🗄", !app.session.side_files, t("Подключения и объекты баз", "Connections and database objects")).clicked() {
            app.session.side_files = false;
            app.touch();
        }
        if icon(ui, "🗁", app.session.side_files, t("Файлы проекта", "Project files")).clicked() {
            app.session.side_files = true;
            app.touch();
        }
        let (sep, _) = ui.allocate_exact_size(Vec2::new(9.0, ROW_H - 10.0), egui::Sense::hover());
        ui.painter().vline(sep.center().x, sep.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
        icon_menu(ui, "🗋", t("Файл", "File"), |ui| file_menu(app, ui));
        icon_menu(ui, "🔌", t("Подключения", "Connections"), |ui| conn_menu(app, ui, &ctx));
        icon_menu(ui, "▶", t("Запрос", "Query"), |ui| query_menu(app, ui, &ctx));
        if icon(ui, "⚙", app.settings_open, t("Настройки", "Settings")).clicked() {
            app.settings_open = true;
        }
        if icon(ui, "?", app.about, t("Клавиши и о программе", "Keys and about")).clicked() {
            app.about = true;
        }
    });
}

fn file_menu(app: &mut App, ui: &mut egui::Ui) {
    if ui.button(format!("{}   Ctrl+T", t("Новый скрипт", "New script"))).clicked() {
        app.new_tab(None, String::new(), None);
    }
    if ui.button(t("Открыть папку проекта…", "Open project folder…")).clicked() {
        app.project_input = Some(app.session.project.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| dirs::home_dir().unwrap_or_default().to_string_lossy().into_owned()));
    }
    if ui.button(format!("{}   Ctrl+S", t("Сохранить", "Save"))).clicked() {
        let (wi, ti) = (app.session.active, app.ws().active);
        app.save_tab(wi, ti);
    }
    if ui.button(t("Сохранить как…", "Save as…")).clicked() {
        let tab = app.tab();
        let p = tab.path.clone().unwrap_or_else(|| app.session.project.clone().or_else(dirs::home_dir).unwrap_or_default().join(format!("{}.sql", tab.title)));
        app.save_as = Some((tab.id.clone(), p.to_string_lossy().into_owned()));
    }
    ui.separator();
    if ui.button(t("Выход", "Quit")).clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn conn_menu(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    if ui.button(t("Новое подключение…", "New connection…")).clicked() {
        app.conn_edit = Some(crate::dialogs::ConnEdit::new(None));
    }
    ui.separator();
    let list: Vec<(String, String)> = app.conns.list.iter().map(|s| (s.id.clone(), s.title())).collect();
    for (id, title) in list {
        ui.menu_button(title, |ui| {
            let on = app.live.contains_key(&id);
            if !on && ui.button(t("Подключиться", "Connect")).clicked() {
                app.ensure_live(ctx, &id, None);
            }
            if on && ui.button(t("Переподключиться", "Reconnect")).clicked() {
                app.leave(ctx, &id, TxThen::Reconnect);
            }
            if on && ui.button(t("Отключиться", "Disconnect")).clicked() {
                app.leave(ctx, &id, TxThen::Disconnect);
            }
            if ui.button(t("Изменить…", "Edit…")).clicked() {
                app.conn_edit = Some(crate::dialogs::ConnEdit::new(app.spec(&id).cloned()));
            }
            if ui.button(t("Новый скрипт с ним", "New script on it")).clicked() {
                app.new_tab(None, String::new(), Some(id.clone()));
            }
        });
    }
}

fn query_menu(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut act = |ui: &mut egui::Ui, label: &str, keys: &str, a: Action| {
        if ui.button(format!("{label}   {keys}")).clicked() {
            match a {
                Action::RunStatement => app.run(ctx, false, false),
                Action::RunScript => app.run(ctx, true, false),
                Action::Explain => app.run(ctx, false, true),
                Action::Format => app.format(),
                Action::Cancel => app.cancel(),
                Action::Commit => app.commit_current(true),
                Action::Rollback => app.commit_current(false),
                Action::Save => {}
            }
        }
    };
    act(ui, t("Выполнить запрос", "Run statement"), "Ctrl+Enter", Action::RunStatement);
    act(ui, t("Выполнить скрипт", "Run script"), "Ctrl+Shift+Enter / F5", Action::RunScript);
    act(ui, t("План запроса", "Explain plan"), "Ctrl+E / F7", Action::Explain);
    act(ui, t("Остановить", "Stop"), "Shift+Esc", Action::Cancel);
    ui.separator();
    act(ui, t("Зафиксировать транзакцию", "Commit"), "Ctrl+Alt+Enter", Action::Commit);
    act(ui, t("Откатить транзакцию", "Roll back"), "Ctrl+Alt+Backspace", Action::Rollback);
    ui.separator();
    act(ui, t("Форматировать", "Format"), "Ctrl+Shift+F", Action::Format);
    ui.separator();
    ui.checkbox(&mut app.results_max, t("Результаты во всю высоту", "Results full height"));
}

fn arc(pts: &mut Vec<Pos2>, c: Pos2, r: f32, a0: f32, a1: f32) {
    const N: usize = 8;
    for k in 0..=N {
        let a = (a0 + (a1 - a0) * k as f32 / N as f32).to_radians();
        pts.push(c + r * Vec2::new(a.cos(), a.sin()));
    }
}

/// The sheet's outline, running up round the active tab (its top corners
/// round, its foot curving out into the sheet's top edge).
fn outline(sheet: Rect, tab: Option<Rect>) -> Vec<Pos2> {
    let (l, top, r, b) = (sheet.left(), sheet.top(), sheet.right(), sheet.bottom());
    let mut p = Vec::new();
    arc(&mut p, Pos2::new(l + SHEET_R, top + SHEET_R), SHEET_R, 180.0, 270.0);
    if let Some(tb) = tab {
        let (a, z, y) = (tb.left(), tb.right(), tb.top());
        arc(&mut p, Pos2::new(a - FILLET, top - FILLET), FILLET, 90.0, 0.0);
        arc(&mut p, Pos2::new(a + TAB_R, y + TAB_R), TAB_R, 180.0, 270.0);
        arc(&mut p, Pos2::new(z - TAB_R, y + TAB_R), TAB_R, 270.0, 360.0);
        arc(&mut p, Pos2::new(z + FILLET, top - FILLET), FILLET, 180.0, 90.0);
    }
    arc(&mut p, Pos2::new(r - SHEET_R, top + SHEET_R), SHEET_R, 270.0, 360.0);
    arc(&mut p, Pos2::new(r - SHEET_R, b - SHEET_R), SHEET_R, 0.0, 90.0);
    arc(&mut p, Pos2::new(l + SHEET_R, b - SHEET_R), SHEET_R, 90.0, 180.0);
    p
}

/// Fills a tab's curved foot: the bit between the curve and the corner.
fn foot(painter: &egui::Painter, corner: Pos2, c: Pos2, a0: f32, a1: f32, color: Color32) {
    let mut pts = Vec::new();
    arc(&mut pts, c, FILLET, a0, a1);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(corner, color);
    for q in &pts {
        mesh.colored_vertex(*q, color);
    }
    for k in 0..pts.len() as u32 - 1 {
        mesh.add_triangle(0, k + 1, k + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// The workspaces as tabs over the sheet; the active one's outline (in its
/// colour) runs on into the sheet's.
fn workspaces(app: &mut App, ui: &mut egui::Ui, row: Rect, sheet: Rect) {
    let v = ui.visuals().clone();
    let font = FontId::proportional(14.0);
    let small = FontId::proportional(11.5);
    let n = app.session.workspaces.len();
    let names: Vec<String> = app.session.workspaces.iter().enumerate().map(|(i, ws)| if ws.name.trim().is_empty() { format!("{} {}", t("Пространство", "Workspace"), i + 1) } else { ws.name.clone() }).collect();
    let counts: Vec<String> = app.session.workspaces.iter().map(|ws| ws.tabs.len().to_string()).collect();
    let running: Vec<bool> = app.session.workspaces.iter().map(|ws| ws.tabs.iter().any(|tb| app.rt.get(&tb.id).is_some_and(|r| r.run.is_some()))).collect();
    // Room: from TABS_FROM to the "+" (the sheet's corner and the tabs'
    // feet keep clear of the edges).
    let plus_w = ROW_H - 6.0;
    let gap = 4.0;
    let lo = sheet.left() + TABS_FROM;
    let hi = sheet.right() - SHEET_R - FILLET - plus_w - gap;
    let painter0 = ui.painter().clone();
    let extras: Vec<f32> = (0..n).map(|i| 30.0 + painter0.layout_no_wrap(counts[i].clone(), small.clone(), v.text_color()).size().x + if running[i] { 18.0 } else { 0.0 }).collect();
    let extra = |i: usize| extras[i];
    let natural: Vec<f32> = names.iter().enumerate().map(|(i, nm)| ui.painter().layout_no_wrap(nm.clone(), font.clone(), v.text_color()).size().x.min(220.0) + extra(i)).collect();
    let room = (hi - lo - gap * n as f32).max(60.0);
    let mut cap = f32::MAX;
    if natural.iter().sum::<f32>() > room {
        let (mut a, mut b) = (48.0f32, natural.iter().cloned().fold(48.0, f32::max));
        for _ in 0..20 {
            let m = (a + b) / 2.0;
            if natural.iter().map(|w| w.min(m)).sum::<f32>() <= room { a = m } else { b = m }
        }
        cap = a;
    }
    let painter = ui.painter().clone();
    let active = app.session.active;
    let mut x = lo;
    let mut rects = Vec::new();
    for w in &natural {
        let w = w.min(cap);
        rects.push(Rect::from_min_max(Pos2::new(x, row.top()), Pos2::new(x + w, sheet.top())));
        x += w + gap;
    }
    let mut close: Option<usize> = None;
    let mut active_rect = None;
    let mut active_text = Vec::new();
    for i in 0..n {
        let rect = rects[i];
        let color = rgb(app.session.workspaces[i].color);
        let r = ui.interact(rect, ui.id().with(("ws-tab", i)), egui::Sense::click());
        let is_active = i == active;
        let body = if is_active { rect } else { Rect::from_min_max(rect.min + Vec2::new(0.0, 4.0), rect.max) };
        if is_active {
            active_rect = Some(body);
        } else {
            let k = if r.hovered() { 0.40 } else { 0.22 };
            let k = if v.dark_mode { k } else { k * 0.7 };
            painter.rect_filled(body, egui::CornerRadius { nw: TAB_R as u8, ne: TAB_R as u8, sw: 0, se: 0 }, color.gamma_multiply(k));
        }
        // Its text (the active one's drawn after the sheet).
        let fg = if is_active { v.strong_text_color() } else { v.text_color() };
        let ex = extra(i);
        let name = elided(ui, &names[i], font.clone(), fg, (rect.width() - ex).max(10.0));
        let cnt = ui.painter().layout_no_wrap(counts[i].clone(), small.clone(), v.weak_text_color());
        let cy = body.center().y + if is_active { 1.0 } else { 0.0 };
        let mut tx = body.left() + 13.0;
        if running[i] {
            ui.put(Rect::from_center_size(Pos2::new(tx + 6.0, cy), Vec2::splat(12.0)), egui::Spinner::new().size(12.0));
            tx += 18.0;
        }
        let shapes = vec![
            egui::Shape::galley(Pos2::new(tx, cy - name.size().y / 2.0), name.clone(), fg),
            egui::Shape::galley(Pos2::new(body.right() - 11.0 - cnt.size().x, cy - cnt.size().y / 2.0 + 1.0), cnt, v.weak_text_color()),
        ];
        if is_active {
            active_text = shapes;
        } else {
            painter.extend(shapes);
        }
        if r.clicked() {
            app.session.active = i;
            app.touch();
        }
        if r.double_clicked() {
            app.ws_edit = Some(i);
        }
        r.context_menu(|ui| {
            if ui.button(t("Изменить…", "Edit…")).clicked() {
                app.ws_edit = Some(i);
                ui.close();
            }
            ui.horizontal_wrapped(|ui| {
                for c in COLORS {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::click());
                    ui.painter().circle_filled(rect.center(), 7.0, rgb(*c));
                    if resp.clicked() {
                        app.session.workspaces[i].color = *c;
                        app.touch();
                    }
                }
            });
            if n > 1 && ui.button(t("Закрыть пространство", "Close workspace")).clicked() {
                close = Some(i);
                ui.close();
            }
        });
        r.on_hover_text(t("Двойной щелчок — имя, цвет, подключение", "Double-click: name, colour, connection"));
    }
    // The sheet, and the active tab growing out of it.
    let color = rgb(app.ws().color);
    let fill = v.panel_fill;
    let tab = active_rect.map(|r| Rect::from_min_max(r.min, Pos2::new(r.max.x, sheet.top() + 1.0)));
    painter.rect_filled(sheet, SHEET_R, fill);
    if let Some(tb) = tab {
        painter.rect_filled(tb, egui::CornerRadius { nw: TAB_R as u8, ne: TAB_R as u8, sw: 0, se: 0 }, fill);
        let top = sheet.top();
        foot(&painter, Pos2::new(tb.left(), top), Pos2::new(tb.left() - FILLET, top - FILLET), 90.0, 0.0, fill);
        foot(&painter, Pos2::new(tb.right(), top), Pos2::new(tb.right() + FILLET, top - FILLET), 180.0, 90.0, fill);
    }
    painter.add(egui::Shape::closed_line(outline(sheet, tab), Stroke::new(1.5, color)));
    painter.extend(active_text);
    // "+": a new workspace.
    let plus = Rect::from_min_size(Pos2::new(x, row.top() + 4.0 + (row.height() - 4.0 - plus_w) / 2.0), Vec2::splat(plus_w));
    let r = ui.interact(plus, ui.id().with("ws-new"), egui::Sense::click());
    if r.hovered() {
        painter.rect_filled(plus, RADIUS, v.widgets.hovered.weak_bg_fill);
    }
    painter.text(plus.center(), Align2::CENTER_CENTER, "+", FontId::proportional(18.0), v.text_color());
    if r.on_hover_text(t("Новое рабочее пространство", "New workspace")).clicked() {
        let color = COLORS[app.session.workspaces.len() % COLORS.len()];
        let name = format!("{} {}", t("Пространство", "Workspace"), app.session.workspaces.len() + 1);
        let conn = app.ws().conn.clone();
        app.session.workspaces.push(Workspace { name, color, conn, ..Default::default() });
        app.session.active = app.session.workspaces.len() - 1;
        app.ws_edit = Some(app.session.active);
        app.touch();
    }
    if let Some(i) = close {
        let ids: Vec<String> = app.session.workspaces[i].tabs.iter().map(|t| t.id.clone()).collect();
        for id in ids {
            app.rt.remove(&id);
        }
        app.session.workspaces.remove(i);
        app.session.active = app.session.active.min(app.session.workspaces.len() - 1);
        app.touch();
    }
}

fn tab_title(tab: &Tab) -> String {
    let mut s = tab.title.clone();
    if tab.modified {
        s.push_str(" ●");
    }
    s
}

/// Second row, on the sheet: the workspace's scripts, rounded boxes; the
/// current one in the workspace's colour.
fn script_tabs(app: &mut App, ui: &mut egui::Ui) {
    let color = rgb(app.ws().color);
    let v = ui.visuals().clone();
    egui::Frame::NONE.show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::horizontal().id_salt("tabs").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let wi = app.session.active;
                let n = app.ws().tabs.len();
                let mut close = None;
                let mut drag: Option<(usize, usize)> = None;
                for i in 0..n {
                    let tab = &app.ws().tabs[i];
                    let active = i == app.ws().active;
                    let running = app.rt.get(&tab.id).is_some_and(|r| r.run.is_some());
                    let errors = app.rt.get(&tab.id).is_some_and(|r| r.issues.iter().any(|x| x.sev == Sev::Error));
                    let conn_color = app.tab_conn(app.ws(), tab).and_then(|c| app.spec(&c)).and_then(|s| s.color).map(rgb);
                    let id = tab.id.clone();
                    let hover_id = ui.id().with(("tab-hover", &id));
                    let hovered = ui.ctx().data(|d| d.get_temp::<bool>(hover_id)).unwrap_or(false);
                    let (fill, stroke) = if active {
                        (color.gamma_multiply(if v.dark_mode { 0.30 } else { 0.18 }), Stroke::new(1.0, color))
                    } else if hovered {
                        (v.widgets.hovered.weak_bg_fill, Stroke::new(1.0, v.widgets.hovered.weak_bg_fill))
                    } else {
                        (v.widgets.inactive.weak_bg_fill.gamma_multiply(0.55), Stroke::new(1.0, Color32::TRANSPARENT))
                    };
                    let path = tab.path.clone();
                    let title = tab_title(tab);
                    let fr = egui::Frame::NONE.fill(fill).corner_radius(RADIUS).inner_margin(egui::Margin { left: 9, right: 5, top: 4, bottom: 4 }).stroke(stroke);
                    let shown = fr
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if let Some(c) = conn_color {
                                    let (rect, _) = ui.allocate_exact_size(Vec2::new(4.0, 14.0), egui::Sense::hover());
                                    ui.painter().rect_filled(rect, 2.0, c);
                                }
                                if running {
                                    ui.add(egui::Spinner::new().size(12.0));
                                }
                                let mut txt = RichText::new(title);
                                if active {
                                    txt = txt.strong();
                                }
                                if errors {
                                    txt = txt.color(Color32::from_rgb(230, 90, 90));
                                }
                                let lr = ui.add(egui::Label::new(txt).sense(egui::Sense::click_and_drag()).selectable(false));
                                let x = ui.add_visible(active || hovered, egui::Button::new(RichText::new("🗙").small()).frame(false));
                                (lr, x)
                            })
                            .inner
                        });
                    let now_hovered = ui.rect_contains_pointer(shown.response.rect);
                    if now_hovered != hovered {
                        ui.ctx().data_mut(|d| d.insert_temp(hover_id, now_hovered));
                        ui.ctx().request_repaint();
                    }
                    let (lr, x) = shown.inner;
                    if lr.clicked() {
                        app.ws_mut().active = i;
                        app.rt.entry(id.clone()).or_default().ed.focus = true;
                        app.touch();
                    }
                    if lr.middle_clicked() || x.clicked() {
                        close = Some(i);
                    }
                    if lr.double_clicked() {
                        app.tab_rename = Some((id.clone(), app.ws().tabs[i].title.clone()));
                    }
                    if lr.dragged() {
                        if let Some(p) = ui.ctx().pointer_interact_pos() {
                            let dx = p.x - lr.rect.center().x;
                            if dx > lr.rect.width() * 0.6 && i + 1 < n {
                                drag = Some((i, i + 1));
                            } else if dx < -lr.rect.width() * 0.6 && i > 0 {
                                drag = Some((i, i - 1));
                            }
                        }
                    }
                    let lr = match &path {
                        Some(p) => lr.on_hover_text(p.display().to_string()),
                        None => lr,
                    };
                    lr.context_menu(|ui| {
                        if ui.button(t("Переименовать", "Rename")).clicked() {
                            app.tab_rename = Some((id.clone(), app.ws().tabs[i].title.clone()));
                            ui.close();
                        }
                        if ui.button(t("Сохранить", "Save")).clicked() {
                            app.save_tab(wi, i);
                            ui.close();
                        }
                        if ui.button(t("Дублировать", "Duplicate")).clicked() {
                            let tb = app.ws().tabs[i].clone();
                            app.new_tab(Some(format!("{} (2)", tb.title)), tb.full_text(), tb.conn);
                            app.tab_mut().split_package();
                            ui.close();
                        }
                        if app.ws().tabs[i].other.is_some() {
                            if ui.button(t("Объединить спецификацию и тело", "Join specification and body")).clicked() {
                                app.session.workspaces[wi].tabs[i].join_parts();
                                app.touch();
                                ui.close();
                            }
                        } else if crate::sql::outline::split_package(&app.ws().tabs[i].text).is_some() && ui.button(t("Разделить пакет на спецификацию и тело", "Split the package into specification and body")).clicked() {
                            app.session.workspaces[wi].tabs[i].split_package();
                            app.touch();
                            ui.close();
                        }
                        ui.menu_button(t("Перенести в пространство", "Move to workspace"), |ui| {
                            for k in 0..app.session.workspaces.len() {
                                if k == wi {
                                    continue;
                                }
                                let name = app.session.workspaces[k].name.clone();
                                if ui.button(name).clicked() {
                                    let tb = app.session.workspaces[wi].tabs.remove(i);
                                    let ws = &mut app.session.workspaces[wi];
                                    if ws.tabs.is_empty() {
                                        ws.tabs.push(Tab::default());
                                    }
                                    ws.active = ws.active.min(ws.tabs.len() - 1);
                                    app.session.workspaces[k].tabs.push(tb);
                                    app.touch();
                                    ui.close();
                                    return;
                                }
                            }
                        });
                        if ui.button(t("Закрыть остальные", "Close others")).clicked() {
                            let keep = app.ws().tabs[i].clone();
                            let gone: Vec<String> = app.ws().tabs.iter().filter(|t| t.id != keep.id).map(|t| t.id.clone()).collect();
                            for g in gone {
                                app.rt.remove(&g);
                            }
                            let ws = app.ws_mut();
                            ws.tabs = vec![keep];
                            ws.active = 0;
                            app.touch();
                            ui.close();
                        }
                        if ui.button(t("Закрыть", "Close")).clicked() {
                            close = Some(i);
                            ui.close();
                        }
                    });
                }
                if ui.add(egui::Button::new(" + ").frame(false)).on_hover_text(t("Новый скрипт (Ctrl+T)", "New script (Ctrl+T)")).clicked() {
                    app.new_tab(None, String::new(), None);
                }
                if let Some((a, b)) = drag {
                    let ws = app.ws_mut();
                    ws.tabs.swap(a, b);
                    if ws.active == a {
                        ws.active = b;
                    } else if ws.active == b {
                        ws.active = a;
                    }
                    app.touch();
                }
                if let Some(i) = close {
                    let tab = &app.ws().tabs[i];
                    if tab.modified {
                        app.toast(t("Несохранённые правки файла остались в сессии — вкладка закрыта", "Unsaved file edits were in the tab — closed"));
                    }
                    app.close_tab(wi, i);
                }
            });
        });
    });
}

fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.horizontal(|ui| {
        let ws = app.ws().clone();
        let tab = app.tab().clone();
        let cur = app.tab_conn(&ws, &tab);
        let label = |app: &App, c: &Option<String>| c.as_ref().and_then(|c| app.spec(c)).map_or(t("— подключение —", "— connection —").to_string(), |s| format!("{}  ·  {}", s.title(), s.kind.name()));
        let mut chosen: Option<Option<String>> = None;
        // Narrow: the buttons keep only their icons (names in the tips).
        let compact = ui.available_width() < 1120.0;
        let tiny = ui.available_width() < 780.0;
        let lbl = |icon: &str, name: &str| if compact { icon.to_string() } else { format!("{icon} {name}") };
        let tx_lbl = |icon: &str, name: &str| if tiny { icon.to_string() } else { format!("{icon} {name}") };
        let tip = |name: &str, keys: &str| format!("{name}   {keys}");
        egui::ComboBox::from_id_salt("tab-conn").width(if compact { 200.0 } else { 260.0 }).selected_text(label(app, &cur)).show_ui(ui, |ui| {
            let ws_label = match &ws.conn {
                Some(c) => format!("{} ({})", t("Как у пространства", "Workspace's"), app.spec(c).map(|s| s.title()).unwrap_or_default()),
                None => t("Как у пространства (нет)", "Workspace's (none)").to_string(),
            };
            if ui.selectable_label(tab.conn.is_none(), ws_label).clicked() {
                chosen = Some(None);
            }
            for s in &app.conns.list {
                if ui.selectable_label(tab.conn.as_deref() == Some(&s.id), format!("{}  ·  {}", s.title(), s.kind.name())).clicked() {
                    chosen = Some(Some(s.id.clone()));
                }
            }
            ui.separator();
            if ui.button(t("Новое подключение…", "New connection…")).clicked() {
                app.conn_edit = Some(crate::dialogs::ConnEdit::new(None));
            }
        });
        if let Some(c) = chosen {
            app.tab_mut().conn = c.clone();
            // The workspace takes the first one picked.
            if app.ws().conn.is_none() {
                app.ws_mut().conn = c;
            }
            app.touch();
        }
        if let Some(c) = &cur {
            let (dot, tip) = conn_dot(app, c);
            let (rect, r) = ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::click());
            ui.painter().circle_filled(rect.center(), 5.0, dot);
            if r.on_hover_text(format!("{tip}\n{}", t("Щелчок — подключиться заново", "Click: reconnect"))).clicked() {
                app.reconnect(&ctx, c);
            }
        }
        ui.separator();
        let running = app.rt.get(&tab.id).and_then(|r| r.run.as_ref()).map(|r| r.since.elapsed());
        if ui.add_enabled(running.is_none(), egui::Button::new(lbl("▶", t("Выполнить", "Run")))).on_hover_text(tip(t("Выполнить запрос", "Run statement"), "Ctrl+Enter")).clicked() {
            app.run(&ctx, false, false);
        }
        if ui.add_enabled(running.is_none(), egui::Button::new(lbl("⏩", t("Скрипт", "Script")))).on_hover_text(tip(t("Выполнить скрипт", "Run script"), "Ctrl+Shift+Enter / F5")).clicked() {
            app.run(&ctx, true, false);
        }
        if ui.add_enabled(running.is_some(), egui::Button::new(lbl("■", t("Стоп", "Stop")))).on_hover_text(tip(t("Остановить", "Stop"), "Shift+Esc")).on_disabled_hover_text(tip(t("Остановить", "Stop"), "Shift+Esc")).clicked() {
            app.cancel();
        }
        if ui.add_enabled(running.is_none(), egui::Button::new(lbl("🔍", t("План", "Plan")))).on_hover_text(tip(t("План запроса", "Explain plan"), "Ctrl+E / F7")).clicked() {
            app.run(&ctx, false, true);
        }
        if ui.button(lbl("✨", t("Формат", "Format"))).on_hover_text(tip(t("Форматировать", "Format"), "Ctrl+Shift+F")).clicked() {
            app.format();
        }
        // Transactions, where the database has them.
        if let Some(c) = cur.as_ref().filter(|c| app.spec(c).is_some_and(|s| s.kind.transactions())) {
            ui.separator();
            let (auto, pending) = match app.live.get(c) {
                Some(l) => (l.auto, l.pending),
                None => (app.spec(c).is_none_or(|s| s.auto_commit), false),
            };
            let mut on = auto;
            let auto_tip = t(
                "Автокоммит: каждый запрос фиксируется сразу.\nВыключен — изменения копятся в транзакции до «Зафиксировать» или «Откатить».",
                "Auto-commit: each statement is committed at once.\nOff: changes gather in a transaction until Commit or Roll back.",
            );
            if ui.add_enabled(running.is_none() && app.live.contains_key(c), egui::Checkbox::new(&mut on, if tiny { t("Авто", "Auto") } else { t("Автокоммит", "Auto-commit") })).on_hover_text(auto_tip).on_disabled_hover_text(auto_tip).changed() {
                app.tx(c, crate::db::TxOp::Auto(on));
            }
            if !auto {
                let warm = Color32::from_rgb(230, 160, 40);
                let mut commit = egui::Button::new(tx_lbl("✔", t("Зафиксировать", "Commit")));
                let mut commit_tip = tip(t("Зафиксировать транзакцию", "Commit"), "Ctrl+Alt+Enter");
                if pending {
                    commit = commit.fill(warm.gamma_multiply(0.35)).stroke(Stroke::new(1.0, warm));
                    commit_tip = format!("{commit_tip}\n{}", t("Есть незафиксированные изменения", "There are uncommitted changes"));
                }
                let can = running.is_none() && app.live.contains_key(c);
                if ui.add_enabled(can, commit).on_hover_text(commit_tip).clicked() {
                    app.tx(c, crate::db::TxOp::Commit);
                }
                if ui.add_enabled(can, egui::Button::new(tx_lbl("⟲", t("Откатить", "Roll back")))).on_hover_text(tip(t("Откатить транзакцию", "Roll back"), "Ctrl+Alt+Backspace")).clicked() {
                    app.tx(c, crate::db::TxOp::Rollback);
                }
            }
        }
        if let Some(d) = running {
            ui.add(egui::Spinner::new());
            ui.label(crate::app::fmt_took(d));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let on = app.session.outline;
            if ui.add(egui::Button::selectable(on, if compact { "☰".to_string() } else { format!("☰ {}", t("Структура", "Structure")) })).on_hover_text(t("Панель структуры: функции, процедуры, типы, запросы   Ctrl+Shift+O", "Structure panel: functions, procedures, types, statements   Ctrl+Shift+O")).clicked() {
                app.session.outline = !on;
                app.touch();
            }
        });
    });
}

pub fn status(app: &mut App, ui: &mut egui::Ui) {
    let bg = chrome(ui.visuals());
    egui::Panel::bottom("status").frame(egui::Frame::NONE.fill(bg).inner_margin(egui::Margin { left: 8, right: 10, top: 2, bottom: 3 })).show_separator_line(false).show(ui, |ui| {
        ui.horizontal(|ui| {
            let ws = app.ws().clone();
            let tab = app.tab().clone();
            let conn = app.tab_conn(&ws, &tab);
            if let Some(c) = &conn {
                let (dot, tip) = conn_dot(app, c);
                ui.colored_label(dot, "●");
                ui.label(RichText::new(crate::app::head(&tip, 80)).small());
                if let Some(l) = app.live.get(c) {
                    if l.pending {
                        ui.label(RichText::new(format!("· ● {}", t("транзакция: есть незафиксированные изменения", "transaction: uncommitted changes"))).small().color(Color32::from_rgb(230, 160, 40)));
                    } else if !l.auto {
                        ui.label(RichText::new(format!("· {}", t("ручная фиксация", "manual commit"))).small().weak());
                    }
                    if l.rels_loading {
                        ui.label(RichText::new(t("· загружаю схему…", "· loading schema…")).small().weak());
                    } else if !l.rels.is_empty() {
                        ui.label(RichText::new(format!("· {} {}", l.rels.len(), t("таблиц в подсказках", "tables in hints"))).small().weak());
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let d = app.dialect_of(conn.as_deref());
                ui.label(RichText::new(d.name()).small());
                if let Some(rt) = app.rt.get(&tab.id) {
                    if let Some((p, _)) = rt.ed.cursor {
                        let line = crate::app::line_of(&tab.text, p);
                        let col = tab.text[..crate::editor::floor(&tab.text, p)].rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
                        ui.label(RichText::new(format!("{line}:{col}")).small());
                    }
                    let e = rt.issues.iter().filter(|i| i.sev == Sev::Error).count();
                    let w = rt.issues.len() - e;
                    if e + w > 0 {
                        let r = ui.add(egui::Label::new(RichText::new(format!("⛔ {e}  ⚠ {w}")).small()).sense(egui::Sense::click()));
                        if r.on_hover_text(t("Щелчок — к первой проблеме", "Click: go to the first problem")).clicked() {
                            if let Some(i) = rt.issues.first() {
                                let at = i.range.start;
                                app.rt.get_mut(&tab.id).unwrap().ed.goto = Some(at);
                            }
                        }
                    } else if app.settings.check {
                        ui.label(RichText::new(format!("✔ {}", t("без ошибок", "no problems"))).small().weak());
                    }
                }
            });
        });
    });
}

/// The editor and, below it, the results. Returns the editor's actions.
pub fn center(app: &mut App, ui: &mut egui::Ui) -> Vec<Action> {
    let mut actions = Vec::new();
    let bg = chrome(ui.visuals());
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(bg)).show(ui, |ui| {
        let full = ui.max_rect();
        let row = Rect::from_min_size(full.min + Vec2::new(0.0, 6.0), Vec2::new(full.width() - 6.0, ROW_H));
        let sheet = Rect::from_min_max(Pos2::new(full.left() + 2.0, row.bottom()), Pos2::new(full.right() - 6.0, full.bottom() - 2.0));
        workspaces(app, ui, row, sheet);
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(sheet.shrink2(Vec2::new(8.0, 7.0))).layout(egui::Layout::top_down(egui::Align::Min)));
        let ui = &mut ui;
        ui.set_clip_rect(sheet.shrink(1.0));
        script_tabs(app, ui);
        ui.add_space(3.0);
        toolbar(app, ui);
        ui.add_space(2.0);
        if app.tab().other.is_some() {
            crate::outline_ui::parts_bar(app, ui);
            ui.add_space(2.0);
        }
        let full = ui.available_height();
        let h = if app.results_max { full - 60.0 } else { app.session.results_height.clamp(80.0, (full - 80.0).max(80.0)) };
        let r = egui::Panel::bottom("results").frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 0, right: 0, top: 4, bottom: 0 })).resizable(!app.results_max).default_size(h).size_range(60.0..=(full - 60.0).max(61.0)).show(ui, |ui| crate::results::panel(app, ui));
        let got = r.response.rect.height();
        if !app.results_max && (got - app.session.results_height).abs() > 1.0 {
            app.session.results_height = got;
            app.touch();
        }
        let top = ui.cursor().top();
        ui.painter().hline(sheet.x_range().shrink(1.0), top - 2.0, ui.visuals().widgets.noninteractive.bg_stroke);
        if app.session.outline {
            let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
            let r = egui::Panel::right("outline")
                .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 10, right: 0, top: 4, bottom: 4 }))
                .show_separator_line(false)
                .resizable(true)
                .default_size(app.session.outline_width)
                .size_range(170.0..=520.0)
                .show(ui, |ui| crate::outline_ui::panel(app, ui));
            let rect = r.response.rect;
            ui.painter().vline(rect.left() + 2.0, rect.y_range(), stroke);
            if (rect.width() - app.session.outline_width).abs() > 1.0 {
                app.session.outline_width = rect.width();
                app.touch();
            }
        }
        let ws = app.ws().clone();
        let tab_id = app.tab().id.clone();
        let conn = app.tab_conn(&ws, app.tab());
        let dialect = app.dialect_of(conn.as_deref());
        let dark = ui.visuals().dark_mode;
        let rels = conn.as_deref().and_then(|c| app.live.get(c)).map(|l| l.rels.clone()).unwrap_or_default();
        let mut rt = app.rt.remove(&tab_id).unwrap_or_default();
        let wi = app.session.active;
        let ti = app.session.workspaces[wi].active;
        let settings = app.settings.clone();
        let tab = &mut app.session.workspaces[wi].tabs[ti];
        let part = tab.on_body;
        let issues = std::mem::take(&mut rt.issues);
        let server_err = rt.server_err.clone();
        let cx = editor::Ctx { dialect, settings: &settings, dark, issues: &issues, server_err: server_err.as_ref(), rels: &rels };
        let out = egui::ScrollArea::both()
            .id_salt(("editor-scroll", &tab_id, tab.on_body))
            .auto_shrink([false, false])
            .show(ui, |ui| editor::show(ui, egui::Id::new(("editor", &tab_id, part)), &mut tab.text, &mut rt.ed, &cx))
            .inner;
        rt.issues = issues;
        if out.changed {
            if tab.path.is_some() {
                tab.modified = true;
            }
            // The server's mark no longer fits the text.
            rt.server_err = None;
            app.touch();
        }
        actions = out.actions;
        app.rt.insert(tab_id, rt);
    });
    actions
}
