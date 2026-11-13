//! The left panel: the databases' objects (each level loaded in the
//! background when opened) or the project folder's files.

use std::path::{Path, PathBuf};

use egui::{Color32, RichText};

use crate::app::{App, Node};
use crate::db::{Kind, Obj, ObjKind, Req};
use crate::i18n::t;
use crate::sql::complete::quote;

pub fn folder_label(key: &str) -> &'static str {
    match key {
        "tables" => t("Таблицы", "Tables"),
        "views" => t("Представления", "Views"),
        "matviews" => t("Мат. представления", "Materialized views"),
        "functions" => t("Функции", "Functions"),
        "procedures" => t("Процедуры", "Procedures"),
        "packages" => t("Пакеты", "Packages"),
        "sequences" => t("Последовательности", "Sequences"),
        "triggers" => t("Триггеры", "Triggers"),
        "types" => t("Типы", "Types"),
        "synonyms" => t("Синонимы", "Synonyms"),
        "jobs" => t("Джобы", "Jobs"),
        "dictionaries" => t("Словари", "Dictionaries"),
        "columns" => t("Колонки", "Columns"),
        "keys" => t("Ключи и ограничения", "Keys and constraints"),
        "indexes" => t("Индексы", "Indexes"),
        _ => "…",
    }
}

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    // Full height, its top row (icons) level with the workspace tabs.
    let frame = egui::Frame::NONE.fill(crate::ui::backdrop(ui.visuals())).inner_margin(egui::Margin { left: 6, right: 6, top: 6, bottom: 4 });
    // (No line drawn at its edge under the pointer: egui takes its look
    // from the parent's hovered / active strokes, hidden here meanwhile.)
    let own = ui.style().clone();
    {
        let w = &mut ui.style_mut().visuals.widgets;
        w.hovered.fg_stroke = egui::Stroke::NONE;
        w.active.fg_stroke = egui::Stroke::NONE;
    }
    let inner_style = own.clone();
    let r = egui::Panel::left("side").frame(frame).show_separator_line(false).resizable(true).default_size(app.session.side_width).size_range(240.0..=700.0).show(ui, |ui| {
        ui.set_style(inner_style);
        crate::ui::icon_bar(app, ui);
        // The list on a card of its own: no edge, rounded, as tall as the
        // sheet beside it (its top and bottom from the last frame).
        let max = ui.max_rect();
        let sheet = ui.ctx().data(|d| d.get_temp::<egui::Rect>(egui::Id::new(crate::ui::SHEET_KEY)));
        let (top, bottom) = sheet.map_or((ui.cursor().top(), max.bottom() - 22.0), |s| (s.top(), s.bottom()));
        let card = egui::Rect::from_min_max(egui::pos2(max.left(), top), egui::pos2(max.right(), bottom.max(top + 40.0)));
        ui.painter().rect_filled(card, crate::ui::SHEET_R, crate::ui::card(ui.visuals()));
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(card.shrink2(egui::vec2(7.0, 7.0))).layout(egui::Layout::top_down(egui::Align::Min)));
        inner.set_clip_rect(card.shrink(1.0).intersect(ui.clip_rect()));
        for st in [egui::TextStyle::Body, egui::TextStyle::Button] {
            if let Some(f) = inner.style_mut().text_styles.get_mut(&st) {
                f.size += 1.5;
            }
        }
        if app.session.side_files { files(app, &mut inner) } else { databases(app, &mut inner) }
        ui.allocate_rect(egui::Rect::from_min_max(ui.cursor().min, egui::pos2(max.right(), card.bottom())), egui::Sense::hover());
    });
    ui.set_style(own);
    let w = r.response.rect.width();
    if (w - app.session.side_width).abs() > 1.0 {
        app.session.side_width = w;
        app.touch();
    }
}

/// How an object is written in SQL (schema-qualified where there are schemas).
fn qualified(kind: Kind, path: &[String], name: &str) -> String {
    let d = kind.dialect();
    let bare = name.split('(').next().unwrap_or(name);
    match kind {
        Kind::Sqlite => quote(bare, d),
        _ if path.len() >= 2 && path[0] != "jobs" && path[0] != "functions" => format!("{}.{}", quote(&path[0], d), quote(bare, d)),
        _ => quote(bare, d),
    }
}

fn first_rows(kind: Kind, rel: &str) -> String {
    match kind {
        Kind::Oracle => format!("SELECT *\n  FROM {rel}\n FETCH FIRST 200 ROWS ONLY"),
        _ => format!("SELECT *\n  FROM {rel}\n LIMIT 200"),
    }
}

fn insert_at_cursor(app: &mut App, s: &str) {
    let id = app.tab().id.clone();
    let at = app.rt.get(&id).and_then(|r| r.ed.cursor).map_or(app.tab().text.len(), |c| c.0.min(app.tab().text.len()));
    app.tab_mut().text.insert_str(at, s);
    let rt = app.rt.entry(id).or_default();
    rt.ed.goto = Some(at + s.len());
    app.touch();
}

fn databases(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    // The button first (right to left), the field takes what's left: it
    // never pushes the panel wider.
    let row = egui::Vec2::new(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.button("➕").on_hover_text(t("Новое подключение", "New connection")).clicked() {
            app.conn_edit = Some(crate::dialogs::ConnEdit::new(None));
        }
        ui.add(egui::TextEdit::singleline(&mut app.tree_filter).hint_text(t("🔎 фильтр объектов", "🔎 filter objects")).desired_width(ui.available_width()));
    });
    egui::ScrollArea::both().id_salt("db-tree").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        if app.conns.list.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(t("Подключений пока нет", "No connections yet")).weak());
                if ui.button(t("Добавить подключение…", "Add a connection…")).clicked() {
                    app.conn_edit = Some(crate::dialogs::ConnEdit::new(None));
                }
            });
        }
        let specs = app.conns.list.clone();
        for spec in specs {
            let key = (spec.id.clone(), Vec::<String>::new());
            let open = app.open_nodes.contains(&key);
            let (dot, tip) = crate::ui::conn_dot(app, &spec.id);
            let r = ui
                .horizontal(|ui| {
                    let arrow = crate::ui::chevron(ui, open);
                    ui.colored_label(dot, "●");
                    if let Some(c) = spec.color {
                        ui.colored_label(crate::editor::rgb(c), "▌");
                    }
                    let name = ui.add(egui::Label::new(RichText::new(spec.title()).strong()).sense(egui::Sense::click()).selectable(false));
                    ui.label(RichText::new(spec.kind.name()).small().weak());
                    (arrow, name)
                })
                .inner;
            let (arrow, name) = r;
            let name = name.on_hover_text(tip);
            if arrow.clicked() || name.double_clicked() {
                if open {
                    app.open_nodes.remove(&key);
                } else {
                    app.open_nodes.insert(key.clone());
                    if app.ensure_live(&ctx, &spec.id, None) && !app.tree.contains_key(&key) {
                        app.request_children(&spec.id, Vec::new());
                    }
                }
            }
            name.context_menu(|ui| {
                let on = app.live.contains_key(&spec.id);
                if !on && ui.button(t("Подключиться", "Connect")).clicked() {
                    app.open_nodes.insert(key.clone());
                    app.ensure_live(&ctx, &spec.id, None);
                    ui.close();
                }
                if on && ui.button(t("Переподключиться", "Reconnect")).clicked() {
                    app.leave(&ctx, &spec.id, crate::app::TxThen::Reconnect);
                    ui.close();
                }
                if on && ui.button(t("Отключиться", "Disconnect")).clicked() {
                    app.leave(&ctx, &spec.id, crate::app::TxThen::Disconnect);
                    ui.close();
                }
                if on && ui.button(t("Обновить дерево и подсказки", "Refresh tree and hints")).clicked() {
                    app.tree.retain(|(c, _), _| *c != spec.id);
                    app.request_children(&spec.id, Vec::new());
                    app.refresh_relations(&spec.id);
                    ui.close();
                }
                ui.separator();
                if ui.button(t("Новый скрипт с ним", "New script on it")).clicked() {
                    app.new_tab(None, String::new(), Some(spec.id.clone()));
                    ui.close();
                }
                if ui.button(t("Сделать подключением пространства", "Make the workspace's connection")).clicked() {
                    app.ws_mut().conn = Some(spec.id.clone());
                    app.touch();
                    ui.close();
                }
                if ui.button(t("Изменить…", "Edit…")).clicked() {
                    app.conn_edit = Some(crate::dialogs::ConnEdit::new(Some(spec.clone())));
                    ui.close();
                }
                if ui.button(t("Копия подключения", "Duplicate")).clicked() {
                    let mut s = spec.clone();
                    s.id = crate::db::new_id();
                    s.name = format!("{} (2)", spec.title());
                    s.save_password = false;
                    app.conn_edit = Some(crate::dialogs::ConnEdit::new(Some(s)));
                    ui.close();
                }
            });
            if open {
                ui.indent(("conn", &spec.id), |ui| children(app, ui, &spec.id, spec.kind, Vec::new()));
            }
        }
    });
}

/// An object kind's icon in the tree: the pack's (in the kind's colour),
/// or the symbol.
fn kind_icon(app: &App, ui: &mut egui::Ui, k: ObjKind) {
    let weak = ui.visuals().weak_text_color();
    let p = if ui.visuals().dark_mode { &app.settings.dark } else { &app.settings.light };
    let c = if app.settings.icon_colors { crate::icons::kind_color(k, p, weak) } else { weak };
    let size = (ui.text_style_height(&egui::TextStyle::Body) - 1.0).max(12.0);
    match crate::icons::image(crate::icons::Ic::of(k), size, c) {
        Some(img) => {
            ui.add(img);
        }
        None => {
            ui.label(RichText::new(k.icon()).weak());
        }
    }
}

/// A file's or folder's icon in the files panel.
fn file_icon(ui: &mut egui::Ui, ic: crate::icons::Ic, glyph: &str, c: Color32) {
    let size = (ui.text_style_height(&egui::TextStyle::Body) - 1.0).max(12.0);
    match crate::icons::image(ic, size, c) {
        Some(img) => {
            ui.add(img);
        }
        None => {
            ui.label(RichText::new(glyph).color(c));
        }
    }
}

fn matches_filter(app: &App, o: &Obj) -> bool {
    let f = app.tree_filter.trim().to_lowercase();
    f.is_empty() || o.kind == ObjKind::Folder || o.kind == ObjKind::Schema || o.kind == ObjKind::Database || o.name.to_lowercase().contains(&f)
}

fn children(app: &mut App, ui: &mut egui::Ui, conn: &str, kind: Kind, path: Vec<String>) {
    let key = (conn.to_string(), path.clone());
    let items = match app.tree.get(&key) {
        None => {
            if app.live.contains_key(conn) {
                app.request_children(conn, path);
            }
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(RichText::new(t("загрузка…", "loading…")).weak());
            });
            return;
        }
        Some(Node::Loading) => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(RichText::new(t("загрузка…", "loading…")).weak());
            });
            return;
        }
        Some(Node::Failed(e)) => {
            let e = e.clone();
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(230, 80, 80), crate::app::head(&e, 160)).on_hover_text(&e);
                if ui.button("↻").clicked() {
                    app.request_children(conn, path.clone());
                }
            });
            return;
        }
        Some(Node::Ready(v)) => v.clone(),
    };
    if items.is_empty() {
        ui.label(RichText::new(t("пусто", "empty")).weak().small());
    }
    let ctx = ui.ctx().clone();
    let items: Vec<Obj> = items.into_iter().filter(|o| matches_filter(app, o)).collect();
    for o in &items {
        let mut p = path.clone();
        p.push(o.name.clone());
        let nkey = (conn.to_string(), p.clone());
        let open = app.open_nodes.contains(&nkey);
        let label = if o.kind == ObjKind::Folder { folder_label(&o.name).to_string() } else { o.name.clone() };
        let r = ui
            .horizontal(|ui| {
                let arrow = if o.leaf {
                    ui.add_space(14.0);
                    None
                } else {
                    Some(crate::ui::chevron(ui, open))
                };
                kind_icon(app, ui, o.kind);
                let n = ui.add(egui::Label::new(&label).sense(egui::Sense::click()).selectable(false));
                if !o.note.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&o.note).small().weak()).truncate());
                }
                (arrow, n)
            })
            .inner;
        let (arrow, name) = r;
        // Ctrl+click: the object in a tab beside (a table or a job: its
        // designer; the rest: its code).
        let ctrl = name.clicked() && ui.input(|i| i.modifiers.command);
        if ctrl {
            let title = o.name.split('(').next().unwrap_or(&o.name).to_string();
            match o.kind {
                ObjKind::Table | ObjKind::Job => app.open_object(conn, p.clone(), o.kind, title),
                k if k.has_source() => app.open_source_next(conn, p.clone(), k, title),
                _ => {}
            }
        }
        let toggle = arrow.is_some_and(|a| a.clicked()) || (!o.leaf && name.clicked() && !ctrl && !o.kind.has_source()) || (!o.leaf && name.double_clicked() && !o.kind.has_source());
        if toggle {
            if open {
                app.open_nodes.remove(&nkey);
            } else {
                app.open_nodes.insert(nkey.clone());
            }
        }
        let jobs = kind == crate::db::Kind::Oracle || kind == crate::db::Kind::Postgres;
        if name.double_clicked() && o.kind == ObjKind::Job && jobs {
            app.open_object(conn, p.clone(), ObjKind::Job, o.name.clone());
        } else if name.double_clicked() && o.kind.has_source() {
            if let Some(l) = app.live.get(conn) {
                l.meta.send(Req::Source { path: p.clone(), kind: o.kind, open_as: o.name.split('(').next().unwrap_or(&o.name).to_string(), into: None });
            }
        }
        let name = match o.kind {
            ObjKind::Table => name.on_hover_text(t("Двойной щелчок — исходный код (DDL), Ctrl+щелчок — конструктор во вкладке рядом", "Double-click: source (DDL); Ctrl+click: the designer in a tab beside")),
            ObjKind::Job if jobs => name.on_hover_text(t("Двойной щелчок или Ctrl+щелчок — расписание и свойства во вкладке рядом", "Double-click or Ctrl+click: schedule and properties in a tab beside")),
            k if k.has_source() => name.on_hover_text(t("Двойной щелчок — исходный код (DDL), Ctrl+щелчок — во вкладке рядом", "Double-click: source (DDL); Ctrl+click: in a tab beside")),
            _ => name,
        };
        name.context_menu(|ui| {
            let q = qualified(kind, &path, &o.name);
            if o.kind.is_relation() && ui.button(t("Первые 200 строк", "First 200 rows")).clicked() {
                app.new_tab(Some(o.name.clone()), first_rows(kind, &q), Some(conn.to_string()));
                app.run(&ctx, true, false);
                ui.close();
            }
            if o.kind == ObjKind::Table && ui.button(t("Конструктор (Ctrl+щелчок)", "Designer (Ctrl+click)")).clicked() {
                app.open_object(conn, p.clone(), ObjKind::Table, o.name.clone());
                ui.close();
            }
            if o.kind == ObjKind::Job && jobs {
                if ui.button(t("Расписание и свойства (Ctrl+щелчок)", "Schedule and properties (Ctrl+click)")).clicked() {
                    app.open_object(conn, p.clone(), ObjKind::Job, o.name.clone());
                    ui.close();
                }
            }
            if o.kind == ObjKind::Folder && o.name == "jobs" && jobs && ui.button(t("Новый джоб…", "New job…")).clicked() {
                app.new_job(conn, p.clone());
                ui.close();
            }
            if o.kind.has_source() && ui.button(t("Исходный код (DDL)", "Source (DDL)")).clicked() {
                if let Some(l) = app.live.get(conn) {
                    l.meta.send(Req::Source { path: p.clone(), kind: o.kind, open_as: o.name.clone(), into: None });
                }
                ui.close();
            }
            if o.kind != ObjKind::Folder {
                if ui.button(t("Вставить имя в скрипт", "Insert the name")).clicked() {
                    let s = if o.kind == ObjKind::Column { quote(&o.name, kind.dialect()) } else { q.clone() };
                    insert_at_cursor(app, &s);
                    ui.close();
                }
                if ui.button(t("Копировать имя", "Copy name")).clicked() {
                    ui.ctx().copy_text(if o.kind == ObjKind::Column { o.name.clone() } else { q.clone() });
                    ui.close();
                }
            }
            if o.kind.is_relation() {
                // Columns from the hints' cache when there, else the tree's.
                let cols: Vec<String> = app.live.get(conn).and_then(|l| l.rels.iter().find(|r| r.name == o.name).map(|r| r.cols.iter().map(|c| quote(&c.0, kind.dialect())).collect())).unwrap_or_default();
                if !cols.is_empty() && ui.button(t("Вставить SELECT со всеми колонками", "Insert SELECT with all columns")).clicked() {
                    insert_at_cursor(app, &format!("SELECT {}\n  FROM {q}", cols.join("\n     , ")));
                    ui.close();
                }
            }
            if !o.leaf && ui.button(t("Обновить", "Refresh")).clicked() {
                app.tree.retain(|(c, pp), _| !(c == conn && pp.starts_with(&p)));
                app.request_children(conn, p.clone());
                app.open_nodes.insert(nkey.clone());
                ui.close();
            }
        });
        if open && !o.leaf {
            let salt = ("n", conn.to_string(), p.clone());
            ui.indent(salt, |ui| children(app, ui, conn, kind, p));
        }
    }
}

fn list_dir(dir: &Path) -> Vec<(String, bool)> {
    let mut v: Vec<(String, bool)> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            Some((name, e.file_type().is_ok_and(|t| t.is_dir())))
        }).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    v
}

fn files(app: &mut App, ui: &mut egui::Ui) {
    let Some(root) = app.session.project.clone() else {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(t("Папка проекта не выбрана", "No project folder")).weak());
            if ui.button(t("Открыть папку…", "Open a folder…")).clicked() {
                app.project_input = Some(dirs::home_dir().unwrap_or_default().to_string_lossy().into_owned());
            }
        });
        return;
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).strong()).on_hover_text(root.display().to_string());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("📂").on_hover_text(t("Другая папка", "Another folder")).clicked() {
                app.project_input = Some(root.to_string_lossy().into_owned());
            }
            if ui.button("↻").on_hover_text(t("Обновить", "Refresh")).clicked() {
                app.files.clear();
            }
            if ui.button("➕").on_hover_text(t("Новый SQL-файл", "New SQL file")).clicked() {
                app.save_as = Some((String::new(), root.join("new.sql").to_string_lossy().into_owned()));
            }
        });
    });
    egui::ScrollArea::both().id_salt("files").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        dir(app, ui, &root);
    });
}

fn dir(app: &mut App, ui: &mut egui::Ui, d: &Path) {
    if !app.files.contains_key(d) {
        app.files.insert(d.to_path_buf(), list_dir(d));
    }
    let entries = app.files[d].clone();
    let open_tab = app.tab().path.clone();
    for (name, is_dir) in entries {
        let p: PathBuf = d.join(&name);
        if is_dir {
            let open = app.open_dirs.contains(&p);
            let r = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    let chev = crate::ui::chevron(ui, open);
                    let c = ui.visuals().text_color();
                    file_icon(ui, crate::icons::Ic::Folder, "📁", c);
                    let n = ui.add(egui::Label::new(&name).sense(egui::Sense::click()).selectable(false));
                    (chev, n)
                })
                .inner;
            if r.0.clicked() || r.1.clicked() {
                if open {
                    app.open_dirs.remove(&p);
                } else {
                    app.open_dirs.insert(p.clone());
                }
            }
            if open {
                ui.indent(("d", &p), |ui| dir(app, ui, &p));
            }
        } else {
            let sql = name.to_lowercase().ends_with(".sql");
            let mut text = RichText::new(&name);
            if !sql {
                text = text.weak();
            }
            if open_tab.as_deref() == Some(p.as_path()) {
                text = text.strong();
            }
            let r = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.add_space(16.0);
                    let c = if sql { ui.visuals().text_color() } else { ui.visuals().weak_text_color() };
                    file_icon(ui, crate::icons::Ic::File, if sql { "📜" } else { "📄" }, c);
                    ui.add(egui::Label::new(text).sense(egui::Sense::click()).selectable(false))
                })
                .inner;
            if r.clicked() {
                app.open_file(&p);
            }
            r.context_menu(|ui| {
                if ui.button(t("Открыть", "Open")).clicked() {
                    app.open_file(&p);
                    ui.close();
                }
                if ui.button(t("Копировать путь", "Copy path")).clicked() {
                    ui.ctx().copy_text(p.display().to_string());
                    ui.close();
                }
            });
        }
    }
}
