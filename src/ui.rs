//! The window's parts: menu, the two rows of tabs (workspaces, then the
//! active workspace's scripts), the toolbar, the editor over the results,
//! the status line.

use egui::{Color32, RichText, Stroke, Vec2};

use crate::app::{App, ConnState};
use crate::config::{COLORS, Tab, Workspace};
use crate::editor::{self, Action, rgb};
use crate::i18n::t;
use crate::sql::check::Sev;

fn contrast(c: Color32) -> Color32 {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if l > 150.0 { Color32::from_gray(20) } else { Color32::WHITE }
}

pub fn conn_dot(app: &App, conn: &str) -> (Color32, String) {
    match app.live.get(conn).map(|l| &l.state) {
        None => (Color32::GRAY, t("не подключено", "not connected").into()),
        Some(ConnState::Connecting) => (Color32::from_rgb(240, 180, 40), t("подключение…", "connecting…").into()),
        Some(ConnState::Ready) => (Color32::from_rgb(70, 190, 90), app.live[conn].version.clone()),
        Some(ConnState::Failed(m)) => (Color32::from_rgb(230, 70, 70), m.clone()),
    }
}

fn menus(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button(t("Файл", "File"), |ui| {
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
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button(t("Подключения", "Connections"), |ui| {
            if ui.button(t("Новое подключение…", "New connection…")).clicked() {
                app.conn_edit = Some(crate::dialogs::ConnEdit::new(None));
            }
            ui.separator();
            let list: Vec<(String, String)> = app.conns.list.iter().map(|s| (s.id.clone(), s.title())).collect();
            for (id, title) in list {
                ui.menu_button(title, |ui| {
                    let on = app.live.contains_key(&id);
                    if !on && ui.button(t("Подключиться", "Connect")).clicked() {
                        app.ensure_live(&ctx, &id, None);
                    }
                    if on && ui.button(t("Переподключиться", "Reconnect")).clicked() {
                        app.reconnect(&ctx, &id);
                    }
                    if on && ui.button(t("Отключиться", "Disconnect")).clicked() {
                        app.disconnect(&id);
                    }
                    if ui.button(t("Изменить…", "Edit…")).clicked() {
                        app.conn_edit = Some(crate::dialogs::ConnEdit::new(app.spec(&id).cloned()));
                    }
                    if ui.button(t("Новый скрипт с ним", "New script on it")).clicked() {
                        app.new_tab(None, String::new(), Some(id.clone()));
                    }
                });
            }
        });
        ui.menu_button(t("Запрос", "Query"), |ui| {
            let mut act = |ui: &mut egui::Ui, label: &str, keys: &str, a: Action| {
                if ui.button(format!("{label}   {keys}")).clicked() {
                    match a {
                        Action::RunStatement => app.run(&ctx, false, false),
                        Action::RunScript => app.run(&ctx, true, false),
                        Action::Explain => app.run(&ctx, false, true),
                        Action::Format => app.format(),
                        Action::Cancel => app.cancel(),
                        Action::Save => {}
                    }
                }
            };
            act(ui, t("Выполнить запрос", "Run statement"), "Ctrl+Enter", Action::RunStatement);
            act(ui, t("Выполнить скрипт", "Run script"), "Ctrl+Shift+Enter / F5", Action::RunScript);
            act(ui, t("План запроса", "Explain plan"), "Ctrl+E / F7", Action::Explain);
            act(ui, t("Остановить", "Stop"), "Shift+Esc", Action::Cancel);
            ui.separator();
            act(ui, t("Форматировать", "Format"), "Ctrl+Shift+F", Action::Format);
        });
        ui.menu_button(t("Вид", "View"), |ui| {
            if ui.radio(!app.session.side_files, t("Объекты базы", "Database objects")).clicked() {
                app.session.side_files = false;
            }
            if ui.radio(app.session.side_files, t("Файлы проекта", "Project files")).clicked() {
                app.session.side_files = true;
            }
            ui.separator();
            ui.checkbox(&mut app.results_max, t("Результаты во всю высоту", "Results full height"));
        });
        if ui.button(t("Настройки", "Settings")).clicked() {
            app.settings_open = true;
        }
        ui.menu_button(t("Справка", "Help"), |ui| {
            if ui.button(t("Клавиши и о программе", "Keys and about")).clicked() {
                app.about = true;
            }
        });
    });
}

/// First row: workspaces, each a coloured chip.
fn workspaces(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let n = app.session.workspaces.len();
        let mut close: Option<usize> = None;
        for i in 0..n {
            let ws = &app.session.workspaces[i];
            let color = rgb(ws.color);
            let active = i == app.session.active;
            let name = if ws.name.trim().is_empty() { format!("{} {}", t("Пространство", "Workspace"), i + 1) } else { ws.name.clone() };
            let count = ws.tabs.len();
            let running = ws.tabs.iter().any(|tb| app.rt.get(&tb.id).is_some_and(|r| r.run.is_some()));
            let label = format!("{}{name}  {count}", if running { "⏳ " } else { "" });
            let text = RichText::new(label).strong().color(if active { contrast(color) } else { ui.visuals().text_color() });
            let btn = egui::Button::new(text).fill(if active { color } else { color.gamma_multiply(0.18) }).stroke(Stroke::new(if active { 0.0 } else { 1.0 }, color)).corner_radius(10.0).min_size(Vec2::new(60.0, 24.0));
            let r = ui.add(btn);
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
        if ui.add(egui::Button::new("+").corner_radius(10.0)).on_hover_text(t("Новое рабочее пространство", "New workspace")).clicked() {
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
    });
}

fn tab_title(tab: &Tab) -> String {
    let mut s = tab.title.clone();
    if tab.modified {
        s.push_str(" ●");
    }
    s
}

/// Second row: the workspace's scripts, on a strip of its colour.
fn script_tabs(app: &mut App, ui: &mut egui::Ui) {
    let color = rgb(app.ws().color);
    let strip = egui::Frame::NONE.fill(color.gamma_multiply(0.14)).inner_margin(egui::Margin::symmetric(4, 3)).corner_radius(4.0);
    strip.show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::horizontal().id_salt("tabs").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
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
                    let fill = if active { ui.visuals().panel_fill } else { Color32::TRANSPARENT };
                    let id = tab.id.clone();
                    let path = tab.path.clone();
                    let title = tab_title(tab);
                    let fr = egui::Frame::NONE.fill(fill).corner_radius(egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }).inner_margin(egui::Margin::symmetric(8, 3)).stroke(if active { Stroke::new(1.0, color) } else { Stroke::NONE });
                    let r = fr
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
                                let x = ui.add(egui::Button::new(RichText::new("🗙").small()).frame(false));
                                (lr, x)
                            })
                            .inner
                        })
                        .inner;
                    let (lr, x) = r;
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
                            app.new_tab(Some(format!("{} (2)", tb.title)), tb.text, tb.conn);
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
        egui::ComboBox::from_id_salt("tab-conn").width(260.0).selected_text(label(app, &cur)).show_ui(ui, |ui| {
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
        if ui.add_enabled(running.is_none(), egui::Button::new(format!("▶ {}", t("Выполнить", "Run")))).on_hover_text("Ctrl+Enter").clicked() {
            app.run(&ctx, false, false);
        }
        if ui.add_enabled(running.is_none(), egui::Button::new(format!("⏩ {}", t("Скрипт", "Script")))).on_hover_text("Ctrl+Shift+Enter / F5").clicked() {
            app.run(&ctx, true, false);
        }
        if ui.add_enabled(running.is_some(), egui::Button::new(format!("■ {}", t("Стоп", "Stop")))).on_hover_text("Shift+Esc").clicked() {
            app.cancel();
        }
        if ui.add_enabled(running.is_none(), egui::Button::new(format!("🔍 {}", t("План", "Plan")))).on_hover_text("Ctrl+E / F7").clicked() {
            app.run(&ctx, false, true);
        }
        if ui.button(format!("✨ {}", t("Формат", "Format"))).on_hover_text("Ctrl+Shift+F").clicked() {
            app.format();
        }
        if let Some(d) = running {
            ui.add(egui::Spinner::new());
            ui.label(crate::app::fmt_took(d));
        }
    });
}

pub fn top(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("top").show(ui, |ui| {
        menus(app, ui);
        ui.add_space(2.0);
        workspaces(app, ui);
        ui.add_space(3.0);
        script_tabs(app, ui);
        ui.add_space(2.0);
        toolbar(app, ui);
        ui.add_space(2.0);
    });
}

pub fn status(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            let ws = app.ws().clone();
            let tab = app.tab().clone();
            let conn = app.tab_conn(&ws, &tab);
            if let Some(c) = &conn {
                let (dot, tip) = conn_dot(app, c);
                ui.colored_label(dot, "●");
                ui.label(RichText::new(crate::app::head(&tip, 80)).small());
                if let Some(l) = app.live.get(c) {
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
                        let col = tab.text[..p.min(tab.text.len())].rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
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
    egui::CentralPanel::default().show(ui, |ui| {
        let full = ui.available_height();
        let h = if app.results_max { full - 60.0 } else { app.session.results_height.clamp(80.0, (full - 80.0).max(80.0)) };
        let r = egui::Panel::bottom("results").resizable(!app.results_max).default_size(h).size_range(60.0..=(full - 60.0).max(61.0)).show(ui, |ui| crate::results::panel(app, ui));
        let got = r.response.rect.height();
        if !app.results_max && (got - app.session.results_height).abs() > 1.0 {
            app.session.results_height = got;
            app.touch();
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
        let issues = std::mem::take(&mut rt.issues);
        let server_err = rt.server_err.clone();
        let cx = editor::Ctx { dialect, settings: &settings, dark, issues: &issues, server_err: server_err.as_ref(), rels: &rels };
        let out = egui::ScrollArea::both()
            .id_salt(("editor-scroll", &tab_id))
            .auto_shrink([false, false])
            .show(ui, |ui| editor::show(ui, egui::Id::new(("editor", &tab_id)), &mut tab.text, &mut rt.ed, &cx))
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
