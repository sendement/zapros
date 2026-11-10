//! Windows over the main one: a connection's settings, the password,
//! "run anyway?", settings, a workspace's name and colour, renaming a tab,
//! saving as, the project folder, keys.

use std::sync::{Arc, Mutex};

use egui::{Color32, RichText};

use crate::app::App;
use crate::config::{COLORS, Palette, ThemePref};
use crate::db::{Kind, Spec, Tls};
use crate::editor::rgb;
use crate::i18n::t;
use crate::sql::{Case, Dialect};

pub struct ConnEdit {
    pub spec: Spec,
    pub password: String,
    pub is_new: bool,
    test: Option<Arc<Mutex<Option<Result<String, String>>>>>,
    confirm_delete: bool,
}

impl ConnEdit {
    pub fn new(spec: Option<Spec>) -> Self {
        let is_new = spec.is_none();
        Self { spec: spec.unwrap_or_default(), password: String::new(), is_new, test: None, confirm_delete: false }
    }
}

pub fn all(app: &mut App, ctx: &egui::Context) {
    conn_edit(app, ctx);
    password(app, ctx);
    confirm(app, ctx);
    tx_ask(app, ctx);
    settings(app, ctx);
    workspace(app, ctx);
    small(app, ctx);
}

fn conn_edit(app: &mut App, ctx: &egui::Context) {
    let Some(mut ed) = app.conn_edit.take() else { return };
    let mut open = true;
    let mut save = false;
    let mut delete = false;
    let title = if ed.is_new { t("Новое подключение", "New connection") } else { t("Подключение", "Connection") };
    egui::Window::new(title).collapsible(false).resizable(false).open(&mut open).default_width(440.0).show(ctx, |ui| {
        let s = &mut ed.spec;
        egui::Grid::new("conn-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(t("Тип", "Type"));
            ui.horizontal(|ui| {
                for k in Kind::ALL {
                    if ui.selectable_label(s.kind == k, k.name()).clicked() && s.kind != k {
                        if s.port == s.kind.default_port() || s.port == 0 {
                            s.port = k.default_port();
                        }
                        if k == Kind::ClickHouse && s.tls == Tls::Prefer {
                            s.tls = Tls::Disable;
                        }
                        s.kind = k;
                    }
                }
            });
            ui.end_row();
            ui.label(t("Название", "Name"));
            let hint = s.title();
            ui.add(egui::TextEdit::singleline(&mut s.name).hint_text(hint));
            ui.end_row();
            if s.kind != Kind::Sqlite {
                ui.label(t("Хост", "Host"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut s.host).desired_width(220.0).hint_text(if s.kind == Kind::Oracle { t("пусто — TNS / дескриптор ниже", "empty: TNS / descriptor below") } else { "localhost" }));
                    ui.label(t("порт", "port"));
                    ui.add(egui::DragValue::new(&mut s.port).range(0..=65535));
                });
                ui.end_row();
            }
            ui.label(match s.kind {
                Kind::Oracle => t("Сервис", "Service"),
                Kind::Sqlite => t("Файл", "File"),
                _ => t("База", "Database"),
            });
            let hint = match s.kind {
                Kind::Oracle => "FREEPDB1 / ORCLPDB1",
                Kind::Sqlite => "~/data/app.db",
                Kind::ClickHouse => "default",
                Kind::Postgres => "postgres",
            };
            ui.add(egui::TextEdit::singleline(&mut s.database).hint_text(hint).desired_width(300.0));
            ui.end_row();
            if s.kind != Kind::Sqlite {
                ui.label(t("Пользователь", "User"));
                ui.add(egui::TextEdit::singleline(&mut s.user).hint_text(match s.kind {
                    Kind::Oracle => "system / sys as sysdba",
                    Kind::ClickHouse => "default",
                    _ => "postgres",
                }));
                ui.end_row();
                ui.label(t("Пароль", "Password"));
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut ed.password).password(true).hint_text(if ed.is_new { "" } else { t("не менять", "unchanged") }));
                    ui.checkbox(&mut s.save_password, t("Сохранить пароль (в связке ключей системы)", "Save password (in the system keyring)"));
                });
                ui.end_row();
                ui.label(if s.kind == Kind::ClickHouse { "HTTPS" } else { "TLS" });
                ui.horizontal(|ui| match s.kind {
                    Kind::ClickHouse => {
                        let mut on = s.tls == Tls::Require;
                        if ui.checkbox(&mut on, t("через https (порт 8443)", "over https (port 8443)")).changed() {
                            s.tls = if on { Tls::Require } else { Tls::Disable };
                            if on && s.port == 8123 {
                                s.port = 8443;
                            } else if !on && s.port == 8443 {
                                s.port = 8123;
                            }
                        }
                    }
                    Kind::Postgres => {
                        for (v, n) in [(Tls::Disable, "disable"), (Tls::Prefer, "prefer"), (Tls::Require, "require")] {
                            ui.radio_value(&mut s.tls, v, n);
                        }
                    }
                    _ => {
                        ui.label(RichText::new(t("настраивается в Oracle Net (sqlnet.ora)", "set in Oracle Net (sqlnet.ora)")).weak());
                    }
                });
                ui.end_row();
                ui.label(t("Таймаут, с", "Timeout, s"));
                ui.add(egui::DragValue::new(&mut s.connect_timeout).range(1..=600));
                ui.end_row();
            }
            ui.label(t("Строк максимум", "Row limit"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut s.row_limit).range(0..=10_000_000));
                ui.label(RichText::new(t("0 — как в настройках", "0: from settings")).weak().small());
            });
            ui.end_row();
            ui.label(t("Только чтение", "Read only"));
            ui.checkbox(&mut s.read_only, t("запретить изменения (на стороне сервера)", "forbid changes (server side)"));
            if s.kind.transactions() {
                ui.checkbox(&mut s.auto_commit, t("автокоммит (иначе фиксировать вручную)", "auto-commit (else commit by hand)"));
            }
            ui.end_row();
            ui.label(t("Цвет", "Colour"));
            ui.horizontal(|ui| {
                if ui.selectable_label(s.color.is_none(), "∅").clicked() {
                    s.color = None;
                }
                for c in COLORS {
                    let (rect, r) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                    ui.painter().circle_filled(rect.center(), 7.0, rgb(*c));
                    if s.color == Some(*c) {
                        ui.painter().circle_stroke(rect.center(), 9.0, egui::Stroke::new(2.0, ui.visuals().strong_text_color()));
                    }
                    if r.clicked() {
                        s.color = Some(*c);
                    }
                }
            });
            ui.end_row();
        });
        if s.kind == Kind::Oracle {
            ui.label(RichText::new(t("Нужен Oracle Instant Client (libclntsh.so), например AUR: oracle-instantclient-basic", "Needs Oracle Instant Client (libclntsh.so), e.g. AUR: oracle-instantclient-basic")).small().weak());
        }
        ui.separator();
        // The test's answer.
        if let Some(res) = ed.test.as_ref().and_then(|r| r.lock().unwrap().clone()) {
            match res {
                Ok(v) => ui.colored_label(Color32::from_rgb(80, 180, 90), format!("✔ {v}")),
                Err(e) => ui.colored_label(Color32::from_rgb(230, 80, 80), format!("⛔ {e}")),
            };
        } else if ed.test.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t("проверяю…", "testing…"));
            });
        }
        ui.horizontal(|ui| {
            if ui.button(t("Проверить", "Test")).clicked() {
                let slot: Arc<Mutex<Option<Result<String, String>>>> = Arc::default();
                let (spec, mut pw, slot2, ctx2) = (ed.spec.clone(), ed.password.clone(), slot.clone(), ctx.clone());
                std::thread::spawn(move || {
                    if pw.is_empty() && spec.kind != Kind::Sqlite {
                        pw = crate::secrets::get(&spec.id).unwrap_or_default();
                    }
                    let r = crate::db::open(&spec, &pw).map(|mut s| s.version()).map_err(|f| f.message);
                    *slot2.lock().unwrap() = Some(r);
                    ctx2.request_repaint();
                });
                ed.test = Some(slot);
            }
            if ui.button(t("Сохранить", "Save")).clicked() {
                save = true;
            }
            if !ed.is_new {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ed.confirm_delete {
                        if ui.button(RichText::new(t("Точно удалить", "Really delete")).color(Color32::from_rgb(230, 80, 80))).clicked() {
                            delete = true;
                        }
                    } else if ui.button(t("Удалить", "Delete")).clicked() {
                        ed.confirm_delete = true;
                    }
                });
            }
        });
    });
    if save {
        let spec = ed.spec.clone();
        let id = spec.id.clone();
        let pw = ed.password.clone();
        if spec.save_password && !pw.is_empty() {
            let id2 = id.clone();
            std::thread::spawn(move || {
                crate::secrets::set(&id2, &pw).ok();
            });
        } else if !spec.save_password {
            let id2 = id.clone();
            std::thread::spawn(move || crate::secrets::remove(&id2));
        }
        match app.conns.list.iter_mut().find(|s| s.id == id) {
            Some(s) => *s = spec.clone(),
            None => app.conns.list.push(spec.clone()),
        }
        app.save_conns();
        // Running ones start over with the new settings.
        if app.live.get(&id).is_some_and(|l| l.pending) {
            app.toast(t("Есть незафиксированные изменения — новые настройки вступят в силу после переподключения", "Uncommitted changes: the new settings apply after reconnecting"));
        } else if app.live.contains_key(&id) {
            app.disconnect(&id);
            if !ed.password.is_empty() || spec.save_password || spec.kind == Kind::Sqlite {
                let p = if spec.save_password { String::new() } else { ed.password.clone() };
                app.start_live(ctx, spec, p);
            }
        } else if ed.is_new {
            if !ed.password.is_empty() && !spec.save_password {
                app.start_live(ctx, spec.clone(), ed.password.clone());
            }
            // The first connection becomes the workspace's.
            if app.ws().conn.is_none() {
                app.ws_mut().conn = Some(id.clone());
                app.touch();
            }
            app.open_nodes.insert((id.clone(), Vec::new()));
        }
        return;
    }
    if delete {
        let id = ed.spec.id.clone();
        app.disconnect(&id);
        app.conns.list.retain(|s| s.id != id);
        std::thread::spawn(move || crate::secrets::remove(&id));
        app.save_conns();
        return;
    }
    if open {
        app.conn_edit = Some(ed);
    }
}

fn password(app: &mut App, ctx: &egui::Context) {
    let Some(mut ask) = app.ask.take() else { return };
    let Some(spec) = app.spec(&ask.conn).cloned() else { return };
    let mut done = None;
    egui::Modal::new(egui::Id::new("password")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.heading(format!("{} {}", t("Пароль для", "Password for"), spec.title()));
        ui.label(RichText::new(format!("{} @ {}:{}", spec.user, spec.host, spec.port)).weak());
        if let Some(e) = &ask.error {
            ui.colored_label(Color32::from_rgb(230, 80, 80), crate::app::head(e, 300));
        }
        let r = ui.add(egui::TextEdit::singleline(&mut ask.password).password(true).desired_width(f32::INFINITY));
        r.request_focus();
        ui.checkbox(&mut ask.save, t("Сохранить в связке ключей", "Save in the keyring"));
        ui.horizontal(|ui| {
            if ui.button(t("Подключиться", "Connect")).clicked() || (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                done = Some(true);
            }
            if ui.button(t("Отмена", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
    });
    match done {
        Some(true) => {
            let mut spec = spec;
            if ask.save {
                let (id, pw) = (spec.id.clone(), ask.password.clone());
                std::thread::spawn(move || {
                    crate::secrets::set(&id, &pw).ok();
                });
                if !spec.save_password {
                    spec.save_password = true;
                    if let Some(s) = app.conns.list.iter_mut().find(|s| s.id == spec.id) {
                        s.save_password = true;
                    }
                    app.save_conns();
                }
            }
            app.live.remove(&spec.id);
            app.start_live(ctx, spec.clone(), ask.password.clone());
            if let Some((tab, stmts)) = ask.then.take() {
                app.dispatch(ctx, tab, stmts, &spec.id);
            }
        }
        Some(false) => {}
        None => app.ask = Some(ask),
    }
}

fn confirm(app: &mut App, ctx: &egui::Context) {
    let Some(c) = app.confirm.take() else { return };
    let mut done = None;
    egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(t("Выполнить?", "Run?"));
        ui.label(&c.text);
        ui.horizontal(|ui| {
            if ui.button(RichText::new(t("Выполнить", "Run")).strong()).clicked() {
                done = Some(true);
            }
            if ui.button(t("Отмена", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
    });
    match done {
        Some(true) => {
            let ws = app.ws().clone();
            let conn = ws.tabs.iter().find(|t| t.id == c.tab).and_then(|tab| app.tab_conn(&ws, tab));
            if let Some(conn) = conn {
                app.dispatch(ctx, c.tab, c.stmts, &conn);
            }
        }
        Some(false) => {}
        None => app.confirm = Some(c),
    }
}

/// Uncommitted changes would be dropped (or committed unseen): which?
fn tx_ask(app: &mut App, ctx: &egui::Context) {
    let Some((conns, then)) = app.tx_ask.clone() else { return };
    let mut done = None;
    egui::Modal::new(egui::Id::new("tx-ask")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(t("Незафиксированные изменения", "Uncommitted changes"));
        let names: Vec<String> = conns.iter().filter_map(|c| app.spec(c)).map(|s| s.title()).collect();
        ui.label(format!(
            "{}: {}",
            t("В открытой транзакции есть изменения", "An open transaction holds changes"),
            names.join(", ")
        ));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new(format!("✔ {}", t("Зафиксировать", "Commit"))).strong()).clicked() {
                done = Some(Some(true));
            }
            if ui.button(format!("⟲ {}", t("Откатить", "Roll back"))).clicked() {
                done = Some(Some(false));
            }
            if ui.button(t("Отмена", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(None);
            }
        });
    });
    if let Some(answer) = done {
        app.tx_ask = None;
        if let Some(commit) = answer {
            app.tx_answer(ctx, conns, then, commit);
        }
    }
}

const SAMPLE: &str = "-- orders by customer\nselect c.name, count(o.id) as orders, sum(o.total) total,\n  case when sum(o.total) > 1000 then 'vip' else 'regular' end kind\nfrom customers c left join orders o on o.customer_id = c.id\nwhere o.created_at >= date '2026-01-01' and c.active = true and o.note <> 'x' group by c.name having count(o.id) > :min order by total desc limit 10;";

fn case_pick(ui: &mut egui::Ui, v: &mut Case, id: &str) -> bool {
    let mut changed = false;
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            for (c, n) in [(Case::Upper, "UPPER"), (Case::Lower, "lower"), (Case::AsIs, t("как есть", "as is"))] {
                changed |= ui.radio_value(v, c, n).changed();
            }
        });
    });
    changed
}

fn palette_ui(ui: &mut egui::Ui, p: &mut Palette) {
    egui::Grid::new("pal").num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, n: &str, c: &mut [u8; 3]| {
            ui.color_edit_button_srgb(c);
            ui.label(n);
        };
        row(ui, t("Ключевые слова", "Keywords"), &mut p.keyword);
        row(ui, t("Функции", "Functions"), &mut p.function);
        ui.end_row();
        row(ui, t("Типы", "Types"), &mut p.type_);
        row(ui, t("Строки", "Strings"), &mut p.string);
        ui.end_row();
        row(ui, t("Числа", "Numbers"), &mut p.number);
        row(ui, t("Комментарии", "Comments"), &mut p.comment);
        ui.end_row();
        row(ui, t("Имена в кавычках", "Quoted names"), &mut p.quoted);
        row(ui, t("Параметры", "Parameters"), &mut p.param);
        ui.end_row();
        row(ui, t("Операторы", "Operators"), &mut p.operator);
        row(ui, t("Текст", "Text"), &mut p.text);
        ui.end_row();
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut p.bold_keywords, t("Ключевые слова жирным", "Bold keywords"));
        ui.checkbox(&mut p.italic_comments, t("Комментарии курсивом", "Italic comments"));
        ui.checkbox(&mut p.rainbow, t("Скобки цветом по глубине", "Brackets coloured by depth"));
    });
}

fn settings(app: &mut App, ctx: &egui::Context) {
    if !app.settings_open {
        return;
    }
    let before = app.settings.clone();
    let mut open = true;
    let id = egui::Id::new("settings-page");
    let mut page: usize = ctx.data(|d| d.get_temp(id)).unwrap_or(0);
    egui::Window::new(t("Настройки", "Settings")).open(&mut open).default_size([640.0, 520.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            for (i, n) in [t("Общие", "General"), t("Редактор", "Editor"), t("Анализ и подсказки", "Checks and hints"), t("Бьютифаер", "Formatter"), t("Подсветка", "Colours")].iter().enumerate() {
                if ui.selectable_label(page == i, *n).clicked() {
                    page = i;
                }
            }
        });
        ui.separator();
        let s = &mut app.settings;
        egui::ScrollArea::vertical().show(ui, |ui| match page {
            0 => {
                ui.horizontal(|ui| {
                    ui.label(t("Язык", "Language"));
                    let mut ru = s.russian;
                    ui.radio_value(&mut ru, None, t("как в системе", "system"));
                    ui.radio_value(&mut ru, Some(true), "Русский");
                    ui.radio_value(&mut ru, Some(false), "English");
                    if ru != s.russian {
                        s.russian = ru;
                        crate::i18n::set_ru(ru.unwrap_or_else(crate::i18n::system_is_ru));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(t("Тема", "Theme"));
                    ui.radio_value(&mut s.theme, ThemePref::System, t("как в системе", "system"));
                    ui.radio_value(&mut s.theme, ThemePref::Dark, t("тёмная", "dark"));
                    ui.radio_value(&mut s.theme, ThemePref::Light, t("светлая", "light"));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Масштаб интерфейса", "UI scale"));
                    ui.add(egui::Slider::new(&mut s.ui_scale, 0.7..=2.0).step_by(0.05));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Строк в результате максимум", "Rows per result at most"));
                    ui.add(egui::DragValue::new(&mut s.row_limit).range(1..=10_000_000));
                });
                ui.checkbox(&mut s.continue_on_error, t("Продолжать скрипт после ошибки", "Go on with the script after an error"));
                ui.checkbox(&mut s.confirm_unsafe, t("Спрашивать перед UPDATE / DELETE без WHERE", "Ask before UPDATE / DELETE without WHERE"));
            }
            1 => {
                ui.horizontal(|ui| {
                    ui.label(t("Размер шрифта", "Font size"));
                    ui.add(egui::Slider::new(&mut s.font_size, 9.0..=28.0).step_by(0.5));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Ширина табуляции", "Tab width"));
                    ui.add(egui::DragValue::new(&mut s.tab_width).range(1..=8));
                });
                ui.checkbox(&mut s.auto_pairs, t("Закрывать скобки и кавычки парой", "Close brackets and quotes in pairs"));
                ui.checkbox(&mut s.blank_line_splits, t("Пустая строка разделяет запросы (для Ctrl+Enter)", "An empty line separates statements (for Ctrl+Enter)"));
            }
            2 => {
                ui.checkbox(&mut s.check, t("Статический анализ скрипта", "Check the script while typing"));
                ui.add_enabled(s.check, egui::Checkbox::new(&mut s.check_names, t("Проверять таблицы и колонки по схеме базы", "Check tables and columns against the database")));
                ui.label(RichText::new(t("Ошибки (незакрытые скобки, CASE без END, строки, лишние запятые…) — красным фоном, предупреждения — подчёркиванием. Наведите мышь — будет пояснение.", "Errors (open brackets, CASE without END, strings, stray commas…) get a red background, warnings an underline. Hover for what's wrong.")).weak().small());
                ui.separator();
                ui.checkbox(&mut s.hints, t("Подсказки ввода", "Typing hints"));
                ui.add_enabled(s.hints, egui::Checkbox::new(&mut s.hints_auto, t("Показывать сами при наборе (иначе Ctrl+Space)", "Pop up while typing (else Ctrl+Space)")));
                ui.horizontal(|ui| {
                    ui.label(t("Регистр ключевых слов в подсказках", "Keyword case in hints"));
                    case_pick(ui, &mut s.hint_case, "hint-case");
                });
            }
            3 => {
                ui.horizontal(|ui| {
                    ui.label(t("Ключевые слова", "Keywords"));
                    case_pick(ui, &mut s.format.keywords, "fk");
                });
                ui.horizontal(|ui| {
                    ui.label(t("Функции", "Functions"));
                    case_pick(ui, &mut s.format.functions, "ff");
                });
                ui.label(RichText::new(t("Стиль «река»: ключевые слова выровнены по правому краю SELECT, запятые в начале строки. PL/SQL-блоки и тела в $$ не трогаются.", "\"River\" style: clause words end where SELECT does, leading commas. PL/SQL blocks and $$ bodies are left alone.")).weak().small());
                ui.separator();
                ui.label(RichText::new(t("Пример", "Preview")).strong());
                let f = crate::sql::fmt::format(SAMPLE, Dialect::Postgres, s.format);
                ui.add(egui::Label::new(RichText::new(f).monospace()).wrap_mode(egui::TextWrapMode::Extend));
            }
            _ => {
                let dark = ui.visuals().dark_mode;
                ui.label(RichText::new(if dark { t("Палитра тёмной темы", "Dark theme palette") } else { t("Палитра светлой темы", "Light theme palette") }).strong());
                let p = if dark { &mut s.dark } else { &mut s.light };
                palette_ui(ui, p);
                if ui.button(t("Вернуть по умолчанию", "Reset to defaults")).clicked() {
                    *p = if dark { Palette::dark() } else { Palette::light() };
                }
                ui.separator();
                let sample = crate::sql::fmt::format(SAMPLE, Dialect::Postgres, s.format) + "\nselect (1 + 2";
                let issues = crate::sql::check::check(&sample, Dialect::Postgres, None);
                let snapshot = s.clone();
                let cx = crate::editor::Ctx { dialect: Dialect::Postgres, settings: &snapshot, dark, issues: &issues, server_err: None, rels: &[] };
                let mono_bold = ui.ctx().fonts(|f| f.families().iter().any(|fam| *fam == egui::FontFamily::Name("mono-bold".into())));
                let job = crate::editor::layout(&sample, &cx, None, snapshot.font_size, mono_bold);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Extend));
                });
            }
        });
    });
    ctx.data_mut(|d| d.insert_temp(id, page));
    if app.settings != before {
        app.save_settings();
    }
    if !open {
        app.settings_open = false;
    }
}

fn workspace(app: &mut App, ctx: &egui::Context) {
    let Some(i) = app.ws_edit else { return };
    if i >= app.session.workspaces.len() {
        app.ws_edit = None;
        return;
    }
    let mut open = true;
    let conns: Vec<(String, String)> = app.conns.list.iter().map(|s| (s.id.clone(), format!("{} · {}", s.title(), s.kind.name()))).collect();
    let mut changed = false;
    egui::Window::new(t("Рабочее пространство", "Workspace")).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
        let ws = &mut app.session.workspaces[i];
        egui::Grid::new("ws").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label(t("Название", "Name"));
            changed |= ui.text_edit_singleline(&mut ws.name).changed();
            ui.end_row();
            ui.label(t("Цвет", "Colour"));
            ui.horizontal(|ui| {
                for c in COLORS {
                    let (rect, r) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                    ui.painter().circle_filled(rect.center(), 8.0, rgb(*c));
                    if ws.color == *c {
                        ui.painter().circle_stroke(rect.center(), 10.0, egui::Stroke::new(2.0, ui.visuals().strong_text_color()));
                    }
                    if r.clicked() {
                        ws.color = *c;
                        changed = true;
                    }
                }
                changed |= ui.color_edit_button_srgb(&mut ws.color).changed();
            });
            ui.end_row();
            ui.label(t("Подключение", "Connection"));
            let cur = ws.conn.as_ref().and_then(|c| conns.iter().find(|(id, _)| id == c)).map_or(t("нет", "none").to_string(), |(_, n)| n.clone());
            egui::ComboBox::from_id_salt("ws-conn").selected_text(cur).width(260.0).show_ui(ui, |ui| {
                if ui.selectable_label(ws.conn.is_none(), t("нет", "none")).clicked() {
                    ws.conn = None;
                    changed = true;
                }
                for (id, n) in &conns {
                    if ui.selectable_label(ws.conn.as_deref() == Some(id), n).clicked() {
                        ws.conn = Some(id.clone());
                        changed = true;
                    }
                }
            });
            ui.end_row();
        });
        ui.label(RichText::new(t("Вкладки пространства без своего подключения работают через это.", "Tabs without their own connection use this one.")).weak().small());
    });
    if changed {
        app.touch();
    }
    if !open {
        app.ws_edit = None;
    }
}

fn small(app: &mut App, ctx: &egui::Context) {
    // Renaming a tab.
    if let Some((id, mut name)) = app.tab_rename.take() {
        let mut done = None;
        egui::Modal::new(egui::Id::new("rename")).show(ctx, |ui| {
            ui.label(t("Название вкладки", "Tab name"));
            let r = ui.text_edit_singleline(&mut name);
            r.request_focus();
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                done = Some(true);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
        match done {
            Some(true) => {
                for ws in &mut app.session.workspaces {
                    if let Some(tb) = ws.tabs.iter_mut().find(|tb| tb.id == id) {
                        tb.title = name.clone();
                    }
                }
                app.touch();
            }
            Some(false) => {}
            None => app.tab_rename = Some((id, name)),
        }
    }
    // Save as / new file.
    if let Some((id, mut path)) = app.save_as.take() {
        let mut done = None;
        egui::Modal::new(egui::Id::new("save-as")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(if id.is_empty() { t("Новый файл", "New file") } else { t("Сохранить как", "Save as") });
            let r = ui.add(egui::TextEdit::singleline(&mut path).desired_width(f32::INFINITY));
            r.request_focus();
            ui.horizontal(|ui| {
                if ui.button(t("Сохранить", "Save")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(true);
                }
                if ui.button(t("Отмена", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                let p = crate::config::expand(path.trim());
                if id.is_empty() {
                    if p.exists() {
                        app.toast(t("Такой файл уже есть", "That file exists"));
                    } else if let Err(e) = std::fs::write(&p, "") {
                        app.toast(format!("{}: {e}", p.display()));
                    } else {
                        app.files.clear();
                        app.open_file(&p);
                    }
                } else {
                    for (wi, ws) in app.session.workspaces.iter_mut().enumerate() {
                        if let Some(ti) = ws.tabs.iter().position(|tb| tb.id == id) {
                            ws.tabs[ti].path = Some(p.clone());
                            ws.tabs[ti].title = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            app.files.clear();
                            app.save_tab(wi, ti);
                            break;
                        }
                    }
                }
            }
            Some(false) => {}
            None => app.save_as = Some((id, path)),
        }
    }
    // The project folder.
    if let Some(mut path) = app.project_input.take() {
        let mut done = None;
        egui::Modal::new(egui::Id::new("project")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(t("Папка проекта со скриптами", "Project folder with scripts"));
            let r = ui.add(egui::TextEdit::singleline(&mut path).desired_width(f32::INFINITY));
            r.request_focus();
            ui.horizontal(|ui| {
                if ui.button(t("Открыть", "Open")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(true);
                }
                if ui.button(t("Отмена", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                let p = crate::config::expand(path.trim());
                if p.is_dir() {
                    app.session.project = Some(p.clone());
                    app.open_dirs.insert(p);
                    app.files.clear();
                    app.session.side_files = true;
                    app.touch();
                } else {
                    app.toast(t("Нет такой папки", "No such folder"));
                    app.project_input = Some(path);
                }
            }
            Some(false) => {}
            None => app.project_input = Some(path),
        }
    }
    if app.about {
        let mut open = true;
        egui::Window::new("zapros").open(&mut open).collapsible(false).show(ctx, |ui| {
            ui.label(RichText::new(format!("zapros {}", env!("CARGO_PKG_VERSION"))).strong());
            ui.label(t("SQL IDE для PostgreSQL, ClickHouse, Oracle и SQLite", "SQL IDE for PostgreSQL, ClickHouse, Oracle and SQLite"));
            ui.separator();
            egui::Grid::new("keys").num_columns(2).show(ui, |ui| {
                for (k, v) in [
                    ("Ctrl+Enter", t("выполнить запрос под курсором / выделение", "run the statement at the cursor / selection")),
                    ("Ctrl+Shift+Enter, F5", t("выполнить весь скрипт", "run the whole script")),
                    ("Ctrl+E, F7", t("план запроса", "explain plan")),
                    ("Shift+Esc", t("остановить запрос", "stop the query")),
                    ("Ctrl+Alt+Enter", t("зафиксировать транзакцию (без автокоммита)", "commit (auto-commit off)")),
                    ("Ctrl+Alt+Backspace", t("откатить транзакцию", "roll back")),
                    ("Ctrl+Shift+F", t("форматировать", "format")),
                    ("Ctrl+Space", t("подсказки", "hints")),
                    ("Ctrl+/", t("закомментировать строки", "comment lines")),
                    ("Ctrl+D", t("дублировать строку", "duplicate line")),
                    ("Tab / Shift+Tab", t("отступ строк", "indent lines")),
                    ("Ctrl+T / Ctrl+W", t("новая / закрыть вкладку", "new / close tab")),
                    ("Ctrl+Tab", t("следующая вкладка", "next tab")),
                    ("Ctrl+S", t("сохранить в файл", "save to file")),
                ] {
                    ui.label(RichText::new(k).monospace());
                    ui.label(v);
                    ui.end_row();
                }
            });
        });
        if !open {
            app.about = false;
        }
    }
}
