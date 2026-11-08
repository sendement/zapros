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
    let r = egui::Panel::left("side").resizable(true).default_size(app.session.side_width).size_range(180.0..=700.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            if ui.selectable_label(!app.session.side_files, format!("🗄 {}", t("База", "Database"))).clicked() {
                app.session.side_files = false;
                app.touch();
            }
            if ui.selectable_label(app.session.side_files, format!("📁 {}", t("Проект", "Project"))).clicked() {
                app.session.side_files = true;
                app.touch();
            }
        });
        ui.separator();
        if app.session.side_files { files(app, ui) } else { databases(app, ui) }
    });
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
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut app.tree_filter).hint_text(t("🔎 фильтр объектов", "🔎 filter objects")).desired_width(ui.available_width() - 34.0));
        if ui.button("➕").on_hover_text(t("Новое подключение", "New connection")).clicked() {
            app.conn_edit = Some(crate::dialogs::ConnEdit::new(None));
        }
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
                    let arrow = ui.add(egui::Label::new(if open { "⏷" } else { "⏵" }).sense(egui::Sense::click()));
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
                    app.reconnect(&ctx, &spec.id);
                    ui.close();
                }
                if on && ui.button(t("Отключиться", "Disconnect")).clicked() {
                    app.disconnect(&spec.id);
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
                if ui.small_button("↻").clicked() {
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
                    Some(ui.add(egui::Label::new(if open { "⏷" } else { "⏵" }).sense(egui::Sense::click())))
                };
                ui.label(RichText::new(o.kind.icon()).weak());
                let n = ui.add(egui::Label::new(&label).sense(egui::Sense::click()).selectable(false));
                if !o.note.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&o.note).small().weak()).truncate());
                }
                (arrow, n)
            })
            .inner;
        let (arrow, name) = r;
        let toggle = arrow.is_some_and(|a| a.clicked()) || (!o.leaf && name.clicked() && !o.kind.has_source()) || (!o.leaf && name.double_clicked() && !o.kind.has_source());
        if toggle {
            if open {
                app.open_nodes.remove(&nkey);
            } else {
                app.open_nodes.insert(nkey.clone());
            }
        }
        if name.double_clicked() && o.kind.has_source() {
            if let Some(l) = app.live.get(conn) {
                l.meta.send(Req::Source { path: p.clone(), kind: o.kind, open_as: o.name.split('(').next().unwrap_or(&o.name).to_string() });
            }
        }
        let name = if o.kind.has_source() { name.on_hover_text(t("Двойной щелчок — исходный код (DDL)", "Double-click: source (DDL)")) } else { name };
        name.context_menu(|ui| {
            let q = qualified(kind, &path, &o.name);
            if o.kind.is_relation() && ui.button(t("Первые 200 строк", "First 200 rows")).clicked() {
                app.new_tab(Some(o.name.clone()), first_rows(kind, &q), Some(conn.to_string()));
                app.run(&ctx, true, false);
                ui.close();
            }
            if o.kind.has_source() && ui.button(t("Исходный код (DDL)", "Source (DDL)")).clicked() {
                if let Some(l) = app.live.get(conn) {
                    l.meta.send(Req::Source { path: p.clone(), kind: o.kind, open_as: o.name.clone() });
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
            if ui.small_button("📂").on_hover_text(t("Другая папка", "Another folder")).clicked() {
                app.project_input = Some(root.to_string_lossy().into_owned());
            }
            if ui.small_button("↻").on_hover_text(t("Обновить", "Refresh")).clicked() {
                app.files.clear();
            }
            if ui.small_button("➕").on_hover_text(t("Новый SQL-файл", "New SQL file")).clicked() {
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
            let r = ui.add(egui::Label::new(format!("{} 📁 {name}", if open { "⏷" } else { "⏵" })).sense(egui::Sense::click()).selectable(false));
            if r.clicked() {
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
            let mut text = RichText::new(format!("   {} {name}", if sql { "📜" } else { "📄" }));
            if !sql {
                text = text.weak();
            }
            if open_tab.as_deref() == Some(p.as_path()) {
                text = text.strong();
            }
            let r = ui.add(egui::Label::new(text).sense(egui::Sense::click()).selectable(false));
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
