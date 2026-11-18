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
    help(app, ctx);
    star(app, ctx);
    params(app, ctx);
}

/// A run waiting for its parameters.
pub struct ParamAsk {
    pub tab: String,
    pub stmts: Vec<crate::db::Stmt>,
    pub conn: String,
    pub params: Vec<crate::sql::params::Param>,
    pub d: Dialect,
}

fn params(app: &mut App, ctx: &egui::Context) {
    use crate::sql::params::{self, As};
    if app.param_ask.is_empty() {
        return;
    }
    let ask = app.param_ask.remove(0);
    let mut done: Option<Option<bool>> = None;
    egui::Modal::new(egui::Id::new("params")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading(t("Параметры", "Parameters"));
        ui.label(RichText::new(t(":имя — значение (число как есть, остальное в кавычках); &имя — текст вставляется как есть", ":name: a value (a number as is, the rest quoted); &name: text put in as it is")).small().weak());
        ui.add_space(4.0);
        egui::Grid::new("params-grid").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
            for (k, p) in ask.params.iter().enumerate() {
                let entry = app.param_values.entry(p.key()).or_insert_with(|| (String::new(), if p.amp { As::Raw } else { As::Auto }));
                ui.label(RichText::new(p.shown()).monospace().strong());
                let r = ui.add_enabled_ui(entry.1 != As::Null, |ui| ui.add_sized([320.0, 22.0], egui::TextEdit::singleline(&mut entry.0).font(egui::TextStyle::Monospace))).inner;
                if k == 0 && ui.memory(|m| m.focused().is_none()) {
                    r.request_focus();
                }
                if p.amp {
                    ui.label(RichText::new(t("как есть", "as is")).small().weak());
                } else {
                    egui::ComboBox::from_id_salt(("param-as", k)).width(100.0).selected_text(entry.1.name()).show_ui(ui, |ui| {
                        for a in As::ALL {
                            ui.selectable_value(&mut entry.1, a, a.name());
                        }
                    });
                }
                ui.end_row();
            }
        });
        // What will run (the first statement).
        let get = |p: &params::Param| app.param_values.get(&p.key()).map(|(v, a)| if p.amp { v.clone() } else { params::value(v, *a, ask.d) });
        if let Some(s) = ask.stmts.first() {
            let shown = params::put(&s.sql, ask.d, get);
            ui.add_space(4.0);
            egui::Frame::NONE.fill(ui.visuals().code_bg_color).corner_radius(4).inner_margin(6).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(crate::app::head(&shown, 400)).monospace().small());
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add(crate::ui::tinted(format!("▶ {}", t("Выполнить", "Run")), crate::ui::GREEN)).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
                done = Some(Some(true));
            }
            if ui.button(t("Без подстановки", "Run as written")).on_hover_text(t("Выполнить текст как есть, с :имя и &имя", "Run the text as it is, :name and &name included")).clicked() {
                done = Some(Some(false));
            }
            if ui.button(t("Отмена", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(None);
            }
        });
    });
    match done {
        Some(Some(fill)) => {
            let mut stmts = ask.stmts;
            if fill {
                let vals = app.param_values.clone();
                let get = |p: &params::Param| vals.get(&p.key()).map(|(v, a)| if p.amp { v.clone() } else { params::value(v, *a, ask.d) });
                for s in &mut stmts {
                    s.sql = params::put(&s.sql, ask.d, get);
                }
            }
            app.dispatch_now(ctx, ask.tab, stmts, &ask.conn);
        }
        Some(None) => {}
        None => app.param_ask.insert(0, ask),
    }
}

/// What a `*` stands for: its columns, to copy or to put in its place.
pub struct StarWin {
    pub cols: Vec<(String, String, String)>,
    pub on: Vec<bool>,
    pub range: std::ops::Range<usize>,
    pub editor: egui::Id,
    pub qualify: bool,
    pub at: egui::Pos2,
    pub d: Dialect,
}

impl StarWin {
    /// The columns picked as written: `t.name` when they need their table's.
    pub fn names(&self) -> Vec<String> {
        let q = |s: &str| crate::sql::complete::quote(s, self.d);
        self.cols.iter().zip(&self.on).filter(|(_, on)| **on).map(|((c, _, t), _)| if self.qualify { format!("{}.{}", q(t), q(c)) } else { q(c) }).collect()
    }
}

fn star(app: &mut App, ctx: &egui::Context) {
    let Some(mut w) = app.star.take() else { return };
    let mut open = true;
    let mut done = false;
    egui::Window::new(format!("{} ({})", t("Колонки вместо *", "Columns for *"), w.cols.len()))
        .id(egui::Id::new("star-window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_pos(w.at + egui::vec2(12.0, 12.0))
        .default_width(360.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button(t("Все", "All")).clicked() {
                    w.on.iter_mut().for_each(|x| *x = true);
                }
                if ui.small_button(t("Ни одной", "None")).clicked() {
                    w.on.iter_mut().for_each(|x| *x = false);
                }
            });
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                egui::Grid::new("star-cols").striped(true).num_columns(3).show(ui, |ui| {
                    for (k, (c, ty, owner)) in w.cols.iter().enumerate() {
                        ui.checkbox(&mut w.on[k], RichText::new(c).monospace());
                        ui.label(RichText::new(ty).small().weak());
                        ui.label(RichText::new(owner).small().weak());
                        ui.end_row();
                    }
                });
            });
            ui.separator();
            let names = w.names();
            ui.add_enabled_ui(!names.is_empty(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button(t("Копировать через запятую", "Copy, comma-separated")).clicked() {
                        ctx.copy_text(names.join(", "));
                        app.toast(t("Скопировано", "Copied"));
                    }
                    if ui.button(t("Копировать столбиком", "Copy one a line")).clicked() {
                        ctx.copy_text(names.join("\n"));
                        app.toast(t("Скопировано", "Copied"));
                    }
                    if ui.add(crate::ui::tinted(t("Заменить * списком", "Replace * with them").into(), crate::ui::GREEN)).clicked() {
                        crate::editor::replace_later(ctx, w.editor, w.range.clone(), names.join(", "));
                        done = true;
                    }
                });
            });
        });
    if open && !done {
        app.star = Some(w);
    }
}

/// A function's, operator's or keyword's help (Ctrl+click on it): how it's
/// written, what it does, an example to copy.
fn help(app: &mut App, ctx: &egui::Context) {
    let Some((i, at)) = app.help else { return };
    let Some(e) = crate::sql::help::entries().get(i) else {
        app.help = None;
        return;
    };
    let mut open = true;
    let dark = ctx.theme() == egui::Theme::Dark;
    let d = app.dialect_of(app.tab_conn(app.ws(), app.tab()).as_deref());
    let snapshot = app.settings.clone();
    let mut copy = false;
    let kind = match e.kind {
        'o' => t("оператор", "operator"),
        'k' => t("ключевое слово", "keyword"),
        _ => t("функция", "function"),
    };
    egui::Window::new(format!("{} — {kind}", e.title()))
        .id(egui::Id::new("help-window"))
        .open(&mut open)
        .default_pos(at + egui::vec2(12.0, 16.0))
        .resizable(false)
        .collapsible(false)
        .default_width(520.0)
        .show(ctx, |ui| {
            ui.set_max_width(560.0);
            let font = snapshot.font_size;
            let mono = egui::FontId::monospace(font);
            ui.label(RichText::new(&e.sig).font(mono.clone()).strong());
            if !e.dialects.is_empty() {
                let names: Vec<&str> = e.dialects.iter().map(|x| match x.as_str() {
                    "pg" => "PostgreSQL",
                    "ch" => "ClickHouse",
                    "ora" => "Oracle",
                    "lite" => "SQLite",
                    o => o,
                }).collect();
                ui.label(RichText::new(names.join(" · ")).small().weak());
            }
            ui.add_space(4.0);
            ui.label(e.about());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("Пример", "Example")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(crate::ui::tinted(format!("🗐 {}", t("Копировать", "Copy")), crate::ui::BLUE)).clicked() {
                        copy = true;
                    }
                });
            });
            let ex = e.example();
            let cx = crate::editor::Ctx { dialect: d, settings: &snapshot, dark, issues: &[], server_err: None, rels: &[], lent: &[], routines: &[], rows: 1, read_only: false, outer_wheel: false };
            let mono_bold = ui.ctx().fonts(|f| f.families().iter().any(|fam| *fam == egui::FontFamily::Name("mono-bold".into())));
            let job = crate::editor::layout(&ex, &cx, None, font, mono_bold);
            egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                ui.add(egui::Label::new(job).selectable(true).wrap_mode(egui::TextWrapMode::Extend));
            });
            ui.add_space(2.0);
            ui.label(RichText::new(t("Ctrl+клик по функции, оператору или ключевому слову — его справка", "Ctrl+click a function, operator or keyword: its help")).small().weak());
        });
    if copy {
        ctx.copy_text(e.example());
        app.toast(t("Пример скопирован", "Example copied"));
    }
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.help = None;
    }
}

/// Width of a dialog's text fields; of its number fields.
pub const FIELD_W: f32 = 320.0;
pub const NUM_W: f32 = 64.0;

/// A number field of the standard size.
fn num<T: egui::emath::Numeric>(ui: &mut egui::Ui, v: &mut T, range: std::ops::RangeInclusive<T>) -> egui::Response {
    ui.add_sized([NUM_W, ui.spacing().interact_size.y], egui::DragValue::new(v).range(range))
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
            ui.add(egui::TextEdit::singleline(&mut s.name).hint_text(hint).desired_width(FIELD_W));
            ui.end_row();
            if s.kind != Kind::Sqlite {
                ui.label(t("Хост", "Host"));
                ui.horizontal(|ui| {
                    // Host, "port", port: as wide as the fields under it.
                    let label = ui.painter().layout_no_wrap(t("порт", "port").into(), egui::TextStyle::Body.resolve(ui.style()), Color32::WHITE).size().x;
                    let gap = ui.spacing().item_spacing.x;
                    ui.add(egui::TextEdit::singleline(&mut s.host).desired_width(FIELD_W - NUM_W - label - gap * 2.0).hint_text(if s.kind == Kind::Oracle { t("пусто — TNS / дескриптор ниже", "empty: TNS / descriptor below") } else { "localhost" }));
                    ui.label(t("порт", "port"));
                    num(ui, &mut s.port, 0..=65535);
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
            ui.add(egui::TextEdit::singleline(&mut s.database).hint_text(hint).desired_width(FIELD_W));
            ui.end_row();
            if s.kind != Kind::Sqlite {
                ui.label(t("Пользователь", "User"));
                ui.add(
                    egui::TextEdit::singleline(&mut s.user)
                        .hint_text(match s.kind {
                            Kind::Oracle => "system / sys as sysdba",
                            Kind::ClickHouse => "default",
                            _ => "postgres",
                        })
                        .desired_width(FIELD_W),
                );
                ui.end_row();
                ui.label(t("Пароль", "Password"));
                ui.add(egui::TextEdit::singleline(&mut ed.password).password(true).hint_text(if ed.is_new { "" } else { t("не менять", "unchanged") }).desired_width(FIELD_W));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut s.save_password, t("Сохранить пароль (в связке ключей системы)", "Save password (in the system keyring)"));
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
                num(ui, &mut s.connect_timeout, 1..=600);
                ui.end_row();
            }
            ui.label(t("Строк за раз", "Rows per batch"));
            ui.horizontal(|ui| {
                num(ui, &mut s.row_limit, 0..=10_000_000);
                ui.label(RichText::new(t("0 — как в настройках", "0: from settings")).weak().small());
            });
            ui.end_row();
            ui.label(t("Только чтение", "Read only"));
            ui.checkbox(&mut s.read_only, t("запретить изменения (на стороне сервера)", "forbid changes (server side)"));
            ui.end_row();
            if s.kind.transactions() {
                ui.label(t("Транзакции", "Transactions"));
                ui.checkbox(&mut s.auto_commit, t("автокоммит (иначе фиксировать вручную)", "auto-commit (else commit by hand)"));
                ui.end_row();
            }
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
            ui.label(RichText::new(t("Нужен Oracle Instant Client — его можно скачать в Настройки → Драйверы", "Needs Oracle Instant Client — download it in Settings → Drivers")).small().weak());
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
                app.dispatch_now(ctx, tab, stmts, &spec.id);
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

/// A font family picked from those installed ("" — `default`); the
/// monospaced ones first when `mono`.
fn font_pick(ui: &mut egui::Ui, id: &str, family: &mut String, mono: bool, default: &str) {
    let shown = if family.is_empty() { default.to_string() } else { family.clone() };
    let fid = egui::Id::new((id, "filter"));
    egui::ComboBox::from_id_salt(id).width(230.0).height(320.0).selected_text(shown).show_ui(ui, |ui| {
        let mut q: String = ui.ctx().data(|d| d.get_temp(fid)).unwrap_or_default();
        ui.add(egui::TextEdit::singleline(&mut q).hint_text(t("🔎 найти", "🔎 find")).desired_width(200.0)).request_focus();
        ui.ctx().data_mut(|d| d.insert_temp(fid, q.clone()));
        if ui.selectable_label(family.is_empty(), default).clicked() {
            family.clear();
        }
        let q = q.to_lowercase();
        let all = crate::fonts::families();
        let list: Vec<&(String, bool)> = if mono { all.iter().filter(|f| f.1).chain(all.iter().filter(|f| !f.1)).collect() } else { all.iter().collect() };
        for (name, is_mono) in list {
            if !q.is_empty() && !name.to_lowercase().contains(&q) {
                continue;
            }
            let label = if mono && *is_mono { format!("{name}  ·  mono") } else { name.clone() };
            if ui.selectable_label(family == name, label).clicked() {
                *family = name.clone();
            }
        }
    });
}

/// The desktop's title bar, or the window's own: its buttons' look, order
/// and side (as in Polka).
fn titlebar_ui(ui: &mut egui::Ui, s: &mut crate::config::Settings) {
    use crate::titlebar::{ORDERS, STYLES, preview};
    ui.label(RichText::new(t("Заголовок окна", "Title bar")).strong());
    ui.checkbox(&mut s.system_titlebar, t("Системный заголовок окна", "The desktop's title bar"))
        .on_hover_text(t("Выключено: строка вкладок сама служит заголовком — тянуть за пустое место, двойной щелчок разворачивает, свои кнопки окна; края окна меняют размер", "Off: the tabs' row is the title bar — drag its empty room, double-click to maximize, the window's own buttons; the edges resize it"));
    if s.system_titlebar {
        return;
    }
    let order = ORDERS[s.win_order as usize % ORDERS.len()];
    ui.label(RichText::new(t("Кнопки окна", "Window buttons")).weak());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for st in STYLES {
            let (rect, r) = ui.allocate_exact_size(egui::vec2(132.0, 52.0), egui::Sense::click());
            let v = ui.visuals();
            let on = s.win_style == st;
            ui.painter().rect_filled(rect, 8.0, if on { v.selection.bg_fill.gamma_multiply(0.5) } else { v.extreme_bg_color });
            ui.painter().rect_stroke(rect, 8.0, if on { egui::Stroke::new(1.5, v.selection.stroke.color) } else { v.widgets.noninteractive.bg_stroke }, egui::StrokeKind::Inside);
            preview(ui, st, &order, egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + 34.0)), 0.8);
            ui.painter().text(egui::pos2(rect.center().x, rect.bottom() - 9.0), egui::Align2::CENTER_CENTER, st.name(), egui::FontId::proportional(11.0), v.text_color());
            if r.clicked() {
                s.win_style = st;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label(t("Сторона", "Side"));
        ui.radio_value(&mut s.win_left, false, t("справа", "right"));
        ui.radio_value(&mut s.win_left, true, t("слева", "left"));
    });
    ui.label(RichText::new(t("Порядок", "Order")).weak());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for (k, o) in ORDERS.iter().enumerate() {
            let (rect, r) = ui.allocate_exact_size(egui::vec2(132.0, 36.0), egui::Sense::click());
            let v = ui.visuals();
            let on = s.win_order as usize % ORDERS.len() == k;
            ui.painter().rect_filled(rect, 8.0, if on { v.selection.bg_fill.gamma_multiply(0.5) } else { v.extreme_bg_color });
            ui.painter().rect_stroke(rect, 8.0, if on { egui::Stroke::new(1.5, v.selection.stroke.color) } else { v.widgets.noninteractive.bg_stroke }, egui::StrokeKind::Inside);
            preview(ui, s.win_style, o, rect, 0.8);
            if r.clicked() {
                s.win_order = k as u8;
            }
        }
    });
}

/// The fonts of the interface, the code and the tables, with their sizes.
fn fonts_ui(ui: &mut egui::Ui, s: &mut crate::config::Settings) {
    ui.label(RichText::new(t("Шрифты", "Fonts")).strong());
    let f = &mut s.fonts;
    egui::Grid::new("fonts").num_columns(3).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label(t("Интерфейс", "Interface"));
        font_pick(ui, "font-ui", &mut f.ui, false, t("системный", "system's"));
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut f.ui_size).range(9.0..=24.0).speed(0.1).suffix(" pt"));
        });
        ui.end_row();
        ui.label(t("Код", "Code"));
        font_pick(ui, "font-code", &mut f.code, true, t("системный моноширинный", "system's monospace"));
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut s.font_size).range(9.0..=28.0).speed(0.1).suffix(" pt"));
            ui.label(t("межстрочный", "line gap"));
            ui.add(egui::DragValue::new(&mut f.code_spacing).range(0.0..=12.0).speed(0.1).suffix(" pt"));
        });
        ui.end_row();
        ui.label(t("Таблицы", "Tables"));
        font_pick(ui, "font-table", &mut f.table, true, t("как у кода", "the code's"));
        ui.add(egui::DragValue::new(&mut f.table_size).range(8.0..=24.0).speed(0.1).suffix(" pt"));
        ui.end_row();
    });
    // How they look.
    let text = ui.visuals().text_color();
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(t("Интерфейс: Подключения, Структура, Выполнить — 0123", "Interface: Connections, Structure, Run — 0123")).font(egui::FontId::proportional(f.ui_size)).color(text));
        ui.label(RichText::new("SELECT name, count(*) FROM orders -- 0O 1lI {}").font(egui::FontId::monospace(s.font_size)).color(text));
        ui.label(RichText::new("id │ customer_id │ total  925.5 │ NULL").font(egui::FontId::new(f.table_size, egui::FontFamily::Name("table".into()))).color(text));
    });
    if ui.button(t("Шрифты по умолчанию", "Default fonts")).clicked() {
        s.fonts = crate::config::Fonts::default();
        s.font_size = 14.0;
    }
}

/// A gradient as a strip.
fn gradient_strip(ui: &mut egui::Ui, b: &crate::themes::Backdrop, size: egui::Vec2) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
    let mut flat = b.clone();
    flat.radial = false;
    flat.angle = 0.0;
    flat.strength = 1.0;
    let p = ui.painter().with_clip_rect(rect);
    flat.paint(&p, rect, Color32::BLACK);
    ui.painter().rect_stroke(rect, 4.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    r
}

/// Colour schemes and the backdrop's gradient.
fn look(ui: &mut egui::Ui, s: &mut crate::config::Settings) {
    use crate::icons::Ic;
    use crate::themes::{SCHEMES, Stop, mix};
    ui.label(RichText::new(t("Цветовая схема", "Colour scheme")).strong());
    ui.label(RichText::new(t("Схема задаёт цвета окна и палитру редактора (её можно доправить на вкладке «Подсветка»).", "A scheme sets the window's colours and the editor's palette (fine-tune it under Colours).")).weak().small());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.selectable_label(s.scheme.is_empty(), t("Стандартная", "Standard")).clicked() {
            s.scheme.clear();
        }
        ui.add_enabled_ui(s.scheme.is_empty(), |ui| {
            ui.radio_value(&mut s.theme, ThemePref::System, t("как в системе", "system"));
            ui.radio_value(&mut s.theme, ThemePref::Dark, t("тёмная", "dark"));
            ui.radio_value(&mut s.theme, ThemePref::Light, t("светлая", "light"));
        });
    });
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for sc in SCHEMES {
            if crate::themes::swatch(ui, sc, s.scheme == sc.id).on_hover_text(sc.name).clicked() {
                s.scheme = sc.id.to_string();
                let keep = |old: &Palette| Palette { bold_keywords: old.bold_keywords, italic_comments: old.italic_comments, rainbow: old.rainbow, bands: old.bands, frames: old.frames, ..sc.palette() };
                if sc.dark {
                    s.dark = keep(&s.dark);
                } else {
                    s.light = keep(&s.light);
                }
            }
        }
    });
    ui.add_space(6.0);
    if ui.button(t("Вернуть стандартные палитры редактора", "Standard editor palettes back")).clicked() {
        s.dark = Palette::dark();
        s.light = Palette::light();
    }
    ui.separator();
    ui.label(RichText::new(t("Иконки", "Icons")).strong());
    let sample = [Ic::Connection, Ic::Table, Ic::View, Ic::Function, Ic::Procedure, Ic::Package, Ic::Trigger, Ic::Job, Ic::Key, Ic::Run, Ic::RunAll, Ic::Format, Ic::Commit, Ic::Settings];
    let weak = ui.visuals().weak_text_color();
    let pal = if ui.visuals().dark_mode { s.dark.clone() } else { s.light.clone() };
    egui::Grid::new("icon-packs").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
        let text = ui.visuals().text_color();
        if ui.radio(s.icons.is_empty(), t("Символы шрифта", "Font symbols")).clicked() {
            s.icons.clear();
        }
        ui.label(RichText::new("🗄 ▦ 👁 ƒ ⚙ 📦 ⚡ ⏱ 🔑 ▶ ⏩ ✨ ✔ ⚙").color(text));
        ui.label("");
        ui.end_row();
        for (i, (id, name, lic)) in crate::icons_data::PACKS.iter().enumerate() {
            if ui.radio(s.icons == *id, *name).clicked() {
                s.icons = id.to_string();
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                for (k, ic) in sample.iter().enumerate() {
                    // The tree's kinds in their colours, the rest plain.
                    let kind = [crate::db::ObjKind::Database, crate::db::ObjKind::Table, crate::db::ObjKind::View, crate::db::ObjKind::Function, crate::db::ObjKind::Procedure, crate::db::ObjKind::Package, crate::db::ObjKind::Trigger, crate::db::ObjKind::Job, crate::db::ObjKind::Key].get(k);
                    let c = match kind {
                        Some(&kd) if s.icon_colors => crate::icons::kind_color(kd, &pal, text),
                        _ => text,
                    };
                    ui.add(crate::icons::image_of(i, *ic, 18.0, c));
                }
            });
            ui.label(RichText::new(*lic).small().color(weak));
            ui.end_row();
        }
    });
    ui.checkbox(&mut s.icon_colors, t("Значки дерева в цветах палитры (таблицы, функции, триггеры…)", "Tree icons in the palette's colours (tables, functions, triggers…)"));
    ui.separator();
    titlebar_ui(ui, s);
    ui.separator();
    fonts_ui(ui, s);
    ui.separator();
    let b = &mut s.backdrop;
    ui.label(RichText::new(t("Фон окна", "Window backdrop")).strong());
    ui.checkbox(&mut b.on, t("Градиент за вкладками, панелями и строкой состояния", "A gradient behind the tabs, panels and status line"));
    ui.add_enabled_ui(b.on, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Готовые:", "Presets:"));
            for (i, p) in crate::themes::PRESETS.iter().enumerate() {
                let pb = crate::themes::preset(i);
                if gradient_strip(ui, &pb, egui::vec2(64.0, 18.0)).on_hover_text(p.0).clicked() {
                    *b = crate::themes::Backdrop { strength: b.strength, ..pb };
                }
            }
        });
        ui.horizontal(|ui| {
            ui.radio_value(&mut b.radial, false, t("линейный", "linear"));
            ui.radio_value(&mut b.radial, true, t("от центра", "radial"));
            if !b.radial {
                ui.add_space(8.0);
                ui.label(t("угол", "angle"));
                ui.add(egui::Slider::new(&mut b.angle, 0.0..=360.0).step_by(5.0).suffix("°"));
            }
        });
        ui.horizontal(|ui| {
            ui.label(t("Насыщенность", "Strength"));
            ui.add(egui::Slider::new(&mut b.strength, 0.05..=1.0).step_by(0.05));
        });
        ui.label(RichText::new(t("Цвета (уровни градиента)", "Colours (the gradient's stops)")).weak());
        let mut remove = None;
        let n = b.stops.len();
        for (i, st) in b.stops.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.color_edit_button_srgb(&mut st.color);
                ui.add(egui::Slider::new(&mut st.at, 0.0..=1.0).step_by(0.01));
                if n > 2 && ui.small_button("🗑").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            b.stops.remove(i);
        }
        ui.horizontal(|ui| {
            if ui.button(format!("➕ {}", t("Цвет", "Colour"))).clicked() && !b.stops.is_empty() {
                // Into the widest gap, its colour in between.
                b.stops.sort_by(|x, y| x.at.total_cmp(&y.at));
                let k = b.stops.windows(2).enumerate().map(|(k, w)| (k, w[1].at - w[0].at)).max_by(|x, y| x.1.total_cmp(&y.1)).map_or(0, |x| x.0);
                let (a, z) = (b.stops[k].clone(), b.stops.get(k + 1).cloned().unwrap_or_else(|| b.stops[k].clone()));
                let c = mix(Color32::from_rgb(a.color[0], a.color[1], a.color[2]), Color32::from_rgb(z.color[0], z.color[1], z.color[2]), 0.5);
                b.stops.insert(k + 1, Stop { at: (a.at + z.at) / 2.0, color: [c.r(), c.g(), c.b()] });
            }
            if ui.button(t("Наоборот", "Reverse")).clicked() {
                for st in b.stops.iter_mut() {
                    st.at = 1.0 - st.at;
                }
            }
        });
        let w = ui.available_width().min(420.0);
        gradient_strip(ui, b, egui::vec2(w, 22.0));
    });
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
        row(ui, t("Алиасы таблиц", "Table aliases"), &mut p.table_alias);
        row(ui, t("Алиасы полей", "Column aliases"), &mut p.column_alias);
        ui.end_row();
        row(ui, t("Схемы / пользователи", "Schemas / users"), &mut p.schema);
        ui.end_row();
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut p.bold_keywords, t("Ключевые слова жирным", "Bold keywords"));
        ui.checkbox(&mut p.italic_comments, t("Комментарии курсивом", "Italic comments"));
        ui.checkbox(&mut p.rainbow, t("Скобки цветом по глубине", "Brackets coloured by depth"));
        ui.checkbox(&mut p.bands, t("Радужные полосы под отступами (скобки, CASE, BEGIN…END, IF, LOOP, SELECT / FROM / WHERE…)", "Rainbow bands under indents (brackets, CASE, BEGIN…END, IF, LOOP, SELECT / FROM / WHERE…)"));
        ui.checkbox(&mut p.frames, t("Рамка вокруг блока под кареткой и под мышью", "A frame round the block at the caret and under the pointer"));
    });
}

/// Drivers: the built-in ones, and Oracle's client — the system's or one
/// downloaded here, version and kind as picked.
fn drivers_ui(ui: &mut egui::Ui, s: &mut crate::config::Settings) {
    use crate::drivers::{self, Job};
    let st = drivers::state();
    let ctx = ui.ctx().clone();
    // The installed list, read again after a download or a deletion.
    let list_id = egui::Id::new("drivers-installed");
    let mut installed: Option<std::sync::Arc<Vec<drivers::Installed>>> = ctx.data(|d| d.get_temp(list_id));
    if installed.is_none() || std::mem::take(&mut st.lock().unwrap().changed) {
        installed = Some(std::sync::Arc::new(drivers::installed()));
        ctx.data_mut(|d| d.insert_temp(list_id, installed.clone().unwrap()));
    }
    let installed = installed.unwrap();
    let ok = Color32::from_rgb(80, 180, 90);
    let bad = Color32::from_rgb(230, 80, 80);

    ui.label(RichText::new(t("Встроенные", "Built in")).strong());
    for n in ["PostgreSQL", "ClickHouse", "SQLite"] {
        ui.horizontal(|ui| {
            ui.colored_label(ok, "✔");
            ui.label(n);
            ui.label(RichText::new(t("— входит в программу, ставить ничего не нужно", "— part of the program, nothing to install")).weak());
        });
    }
    ui.separator();
    ui.label(RichText::new("Oracle Instant Client").strong());
    ui.label(RichText::new(t(
        "Oracle работает через библиотеку Oracle (libclntsh.so). Её можно скачать прямо отсюда — в папку программы, без установки в систему и без настройки LD_LIBRARY_PATH.",
        "Oracle works through Oracle's library (libclntsh.so). It can be downloaded right here — into the program's folder, no system install, no LD_LIBRARY_PATH.",
    )).weak().small());
    #[cfg(feature = "oracle")]
    {
        let now = drivers::loaded_version();
        let at_start = drivers::chosen_at_start();
        match &now {
            Some(v) => ui.label(format!("{} {v} ({})", t("Загружен:", "Loaded:"), at_start.as_ref().map_or_else(|| t("системный", "the system's").to_string(), |p| p.display().to_string()))),
            None => ui.label(RichText::new(t("Ещё не загружен — загрузится при первом подключении к Oracle", "Not loaded yet — it loads on the first Oracle connection")).weak()),
        };
        let wanted = (!s.oracle_client.is_empty()).then(|| std::path::PathBuf::from(&s.oracle_client));
        if wanted != at_start {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(220, 160, 60), t("Выбран другой клиент — он подключится после перезапуска программы", "Another client is picked — it's used once the program restarts"));
                if ui.button(t("Перезапустить", "Restart")).clicked() {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("drivers-restart"), true));
                }
            });
        }
    }
    ui.add_space(4.0);
    // The choice: the system's, or one downloaded.
    let sys = drivers::system_client();
    ui.horizontal(|ui| {
        ui.radio_value(&mut s.oracle_client, String::new(), t("Системный", "The system's"));
        match &sys {
            Some(p) => ui.label(RichText::new(p).weak()),
            None => ui.label(RichText::new(t("— в системе не найден", "— none in the system")).weak()),
        };
    });
    let mut delete: Option<std::path::PathBuf> = None;
    for i in installed.iter() {
        ui.horizontal(|ui| {
            let dir = i.lib_dir.to_string_lossy().into_owned();
            ui.radio_value(&mut s.oracle_client, dir.clone(), &i.name);
            ui.label(RichText::new(drivers::human(i.size)).weak());
            if !i.missing.is_empty() {
                ui.colored_label(bad, format!("⚠ {} {}", t("в системе нет:", "the system lacks:"), i.missing.join(", ")))
                    .on_hover_text(t("Поставьте эти библиотеки пакетным менеджером (например, libaio)", "Install these libraries with the package manager (e.g. libaio)"));
            }
            let busy = !matches!(st.lock().unwrap().job, Job::Idle | Job::Failed(_) | Job::Done(_));
            if ui.add_enabled(!busy, egui::Button::new(t("Удалить", "Delete"))).clicked() {
                delete = Some(drivers::root().join(&i.name));
                if s.oracle_client == dir {
                    s.oracle_client.clear();
                }
            }
        });
    }
    if let Some(d) = delete {
        let _ = std::fs::remove_dir_all(d);
        st.lock().unwrap().changed = true;
        ctx.request_repaint();
    }
    ui.separator();
    // Downloading one.
    ui.label(RichText::new(t("Скачать с сайта Oracle", "Download from Oracle's site")).strong());
    let (releases, job) = {
        let g = st.lock().unwrap();
        (g.releases.clone(), g.job.clone())
    };
    let busy = matches!(job, Job::Listing | Job::Downloading { .. } | Job::Unpacking { .. });
    let pick_id = egui::Id::new("drivers-pick");
    let mut pick: usize = ctx.data(|d| d.get_temp(pick_id)).unwrap_or(0);
    ui.horizontal(|ui| {
        if releases.is_empty() {
            ui.label(RichText::new(t("Список версий ещё не получен", "No list of versions yet")).weak());
        } else {
            pick = pick.min(releases.len() - 1);
            egui::ComboBox::from_id_salt("drivers-release").width(260.0).selected_text(releases[pick].label()).show_ui(ui, |ui| {
                for (k, r) in releases.iter().enumerate() {
                    ui.selectable_value(&mut pick, k, r.label());
                }
            });
            if ui.add_enabled(!busy, egui::Button::new(t("Скачать", "Download"))).clicked() {
                drivers::download(&ctx, releases[pick].clone());
            }
        }
        if ui.add_enabled(!busy, egui::Button::new(if releases.is_empty() { t("Получить список версий", "Get the versions") } else { t("Обновить список", "Refresh") })).clicked() {
            drivers::list(&ctx);
        }
    });
    ctx.data_mut(|d| d.insert_temp(pick_id, pick));
    match &job {
        Job::Idle => {}
        Job::Listing => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t("Читаю страницу загрузок Oracle…", "Reading Oracle's download page…"));
            });
        }
        Job::Downloading { label, got, total } => {
            let frac = if *total > 0 { *got as f32 / *total as f32 } else { 0.0 };
            let text = if *total > 0 { format!("{label}: {} / {}", drivers::human(*got), drivers::human(*total)) } else { format!("{label}: {}", drivers::human(*got)) };
            ui.add(egui::ProgressBar::new(frac).text(text).animate(*total == 0));
        }
        Job::Unpacking { label } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("{label}: {}", t("распаковка…", "unpacking…")));
            });
        }
        Job::Failed(e) => {
            ui.colored_label(bad, format!("⛔ {e}"));
        }
        Job::Done(l) => {
            ui.colored_label(ok, format!("✔ {l} {}", t("скачан — выберите его выше", "downloaded — pick it above")));
        }
    }
    ui.label(RichText::new(t(
        "Basic — все языки и кодировки; Basic Light — меньше, сообщения на английском и основные кодировки (UTF-8, Latin-1…). Загрузка с download.oracle.com на условиях лицензии Oracle (OTN) — входа не нужно. Клиент 23 работает с Oracle 19c и новее, 21 — с 12.1+, 19 — с 11.2+.",
        "Basic: all languages and character sets; Basic Light: smaller, English messages and the main character sets (UTF-8, Latin-1…). Downloaded from download.oracle.com under Oracle's licence (OTN), no login. Client 23 works with Oracle 19c and newer, 21 with 12.1+, 19 with 11.2+.",
    )).weak().small());
}

fn settings(app: &mut App, ctx: &egui::Context) {
    if !app.settings_open {
        return;
    }
    let before = app.settings.clone();
    let mut open = true;
    let id = egui::Id::new("settings-page");
    let mut page: usize = ctx.data(|d| d.get_temp(id)).unwrap_or(0);
    // One size whatever the page (a page's content scrolls inside it).
    let size = egui::vec2(720.0, 560.0).min(ctx.content_rect().size() - egui::vec2(40.0, 60.0));
    egui::Window::new(t("Настройки", "Settings")).id(egui::Id::new("settings-window")).open(&mut open).fixed_size(size).show(ctx, |ui| {
        ui.horizontal(|ui| {
            for (i, n) in [t("Общие", "General"), t("Редактор", "Editor"), t("Анализ и подсказки", "Checks and hints"), t("Стиль кода", "Code style"), t("Подсветка", "Colours"), t("Оформление", "Look"), t("Драйверы", "Drivers")].iter().enumerate() {
                if ui.selectable_label(page == i, *n).clicked() {
                    page = i;
                }
            }
        });
        ui.separator();
        let s = &mut app.settings;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| match page {
            5 => look(ui, s),
            6 => drivers_ui(ui, s),
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
                    if (ui.ctx().zoom_factor() - s.ui_scale).abs() > 0.001 {
                        ui.label(RichText::new(t("— после закрытия настроек", "— once the settings are closed")).weak().small());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(t("Крестик закрытия вкладки", "Tab's close button"));
                    ui.radio_value(&mut s.close_always, true, t("всегда", "always"));
                    ui.radio_value(&mut s.close_always, false, t("при наведении", "on hover"));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Строк за раз (порция загрузки результата)", "Rows per batch (a result loads in batches)"));
                    num(ui, &mut s.row_limit, 1..=10_000_000);
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
                    num(ui, &mut s.tab_width, 1..=8);
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
                ui.add_enabled(s.hints, egui::Checkbox::new(&mut s.hints_typed_first, t("Первая строка подсказок — набранное слово: Enter просто переносит строку (подсказка — стрелкой ↓ и Enter / Tab)", "The hints' first row is the word as typed: Enter just starts a new line (a hint: ↓, then Enter / Tab)")));
                ui.add_enabled(s.hints, egui::Checkbox::new(&mut s.hint_quote_schemas, t("Схемы в кавычках (\"sales\".orders; в ClickHouse — `sales`.orders)", "Schemas in quotes (\"sales\".orders; in ClickHouse `sales`.orders)")));
            }
            3 => {
                ui.label(RichText::new(t("Регистр", "Case")).strong());
                ui.horizontal(|ui| {
                    ui.label(t("Ключевые слова", "Keywords"));
                    case_pick(ui, &mut s.format.keywords, "fk");
                });
                ui.horizontal(|ui| {
                    ui.label(t("Функции", "Functions"));
                    case_pick(ui, &mut s.format.functions, "ff");
                });
                ui.checkbox(&mut s.format.case_while_typing, t("Менять регистр ключевых слов прямо при вводе («select» и пробел → «SELECT »)", "Keywords take the case as they're typed («select» and a space → «SELECT »)"));
                ui.label(RichText::new(t(
                    "Меняются только слова самого SQL и только там, где они ключевые: имена таблиц и колонок, x.поле, строки, комментарии и вызовы функций остаются как набраны. В ClickHouse функции и имена чувствительны к регистру (toDate, countIf) — их не трогает ни ввод, ни форматирование.",
                    "Only SQL's own words, and only where they're keywords: table and column names, x.field, strings, comments and function calls stay as typed. In ClickHouse functions and names are case-sensitive (toDate, countIf): neither typing nor formatting touches them.",
                )).weak().small());
                ui.separator();
                ui.label(RichText::new(t("Раскладка (форматирование, Ctrl+Shift+F)", "Layout (formatting, Ctrl+Shift+F)")).strong());
                ui.horizontal(|ui| {
                    ui.radio_value(&mut s.format.river, true, t("«река»: слова выровнены по SELECT", "\"river\": clause words end where SELECT does"));
                    ui.radio_value(&mut s.format.river, false, t("обычная: слова слева, списки с отступом", "plain: clause words at the left, lists indented"));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Запятые", "Commas"));
                    ui.radio_value(&mut s.format.leading_commas, true, t("в начале строки", "at the line's start"));
                    ui.radio_value(&mut s.format.leading_commas, false, t("в конце строки", "at the line's end"));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Отступ уровня", "Indent per level"));
                    ui.add(egui::Slider::new(&mut s.format.indent, 1..=8).suffix(t(" пр.", " sp.")));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Ширина строки", "Line width"));
                    ui.add(egui::Slider::new(&mut s.format.wrap, 0..=200).custom_formatter(|v, _| if v == 0.0 { t("не переносить", "no wrapping").to_string() } else { format!("{v:.0}") }));
                })
                .response
                .on_hover_text(t("Длиннее: вызов функции раскладывается по аргументу на строку, цепочка || + − переносится перед оператором — с теми же отступами", "Longer: a call gets an argument per line, a || + − chain breaks before its operators — with the same indents"));
                ui.horizontal(|ui| {
                    ui.label(t("Короткие запросы в одну строку, до", "Short statements on one line, up to"));
                    ui.add(egui::Slider::new(&mut s.format.one_line, 0..=160).custom_formatter(|v, _| if v == 0.0 { t("никогда", "never").to_string() } else { format!("{v:.0}") }));
                })
                .response
                .on_hover_text(t("Например, select * from orders остаётся одной строкой", "E.g. select * from orders stays one line"));
                ui.horizontal(|ui| {
                    ui.label(t("Делить вызов или окно длиннее", "Split a call or window longer than"));
                    ui.add(egui::Slider::new(&mut s.format.split, 0..=200).custom_formatter(|v, _| if v == 0.0 { t("только по ширине строки", "only by the line's width").to_string() } else { format!("{v:.0}") }));
                })
                .response
                .on_hover_text(t("if(…), coalesce(…) — по аргументу на строку; sum(…) OVER (…) — PARTITION BY, ORDER BY, ROWS каждое с новой строки. Даже если строка целиком помещается", "if(…), coalesce(…): an argument per line; sum(…) OVER (…): PARTITION BY, ORDER BY, ROWS each on its line. Even when the whole line fits"));
                ui.checkbox(&mut s.format.bare_names, t("Кавычки у имён — только где нужны", "Quote names only where needed")).on_hover_text(t("\"SHOP\".\"ORDERS\" → SHOP.ORDERS, `id` → id; в кавычках остаются имена с пробелами, в другом регистре и совпадающие с ключевыми словами. При форматировании и в коде объектов, открытых из базы", "\"SHOP\".\"ORDERS\" → SHOP.ORDERS, `id` → id; names with spaces, in another case or matching keywords keep theirs. When formatting and in the code of objects opened from the database"));
                ui.checkbox(&mut s.format.embedded, t("Форматировать SQL внутри строк (динамический SQL)", "Format SQL inside strings (dynamic SQL)"));
                ui.add_enabled_ui(s.format.embedded, |ui| {
                    ui.checkbox(&mut s.format.split_strings, t("Разбивать строку с запросом на несколько строк", "Split a string holding a statement over several lines"))
                        .on_hover_text(t("Выключено: строка остаётся как была, меняется только регистр ключевых слов в ней", "Off: the string stays as it was, only its keywords' case changes"));
                });
                ui.label(RichText::new(t("Сами PL/SQL-блоки и тела в $$ не перекладываются, но запросы в строках внутри них (EXECUTE IMMEDIATE, EXECUTE, format()) раскладываются: склейка через || — по куску на строку, кусок запроса — только регистр.", "PL/SQL blocks and $$ bodies keep their layout, but statements in strings inside them (EXECUTE IMMEDIATE, EXECUTE, format()) are laid out: a || chain gets a piece per line, a piece of a statement only its case.")).weak().small());
                ui.separator();
                ui.label(RichText::new(t("Пример", "Preview")).strong());
                let f = crate::sql::fmt::format(SAMPLE, Dialect::Postgres, s.format);
                ui.add(egui::Label::new(RichText::new(f).monospace()).wrap_mode(egui::TextWrapMode::Extend));
            }
            _ => {
                let dark = ui.visuals().dark_mode;
                ui.label(RichText::new(if dark { t("Палитра тёмной темы", "Dark theme palette") } else { t("Палитра светлой темы", "Light theme palette") }).strong());
                ui.checkbox(&mut s.embedded_sql, t("SQL внутри строк подсвечивать как SQL (динамический SQL, тела $$)", "Colour SQL inside strings as SQL (dynamic SQL, $$ bodies)"));
                let p = if dark { &mut s.dark } else { &mut s.light };
                palette_ui(ui, p);
                if ui.button(t("Вернуть по умолчанию", "Reset to defaults")).clicked() {
                    *p = if dark { Palette::dark() } else { Palette::light() };
                }
                ui.separator();
                let sample = crate::sql::fmt::format(SAMPLE, Dialect::Postgres, s.format) + "\n\ndo $$ begin\n  execute 'delete from ' || quote_ident(t) || ' where note = ''x''';\nend $$;\nselect (1 + 2";
                let issues = crate::sql::check::check(&sample, Dialect::Postgres, None);
                let snapshot = s.clone();
                let cx = crate::editor::Ctx { dialect: Dialect::Postgres, settings: &snapshot, dark, issues: &issues, server_err: None, rels: &[], lent: &[], routines: &[], rows: 30, read_only: false, outer_wheel: false };
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
    if ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("drivers-restart"))).is_some() {
        app.save_settings();
        app.save_session();
        crate::drivers::restart();
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
            changed |= ui.add(egui::TextEdit::singleline(&mut ws.name).desired_width(FIELD_W)).changed();
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
            egui::ComboBox::from_id_salt("ws-conn").selected_text(cur).width(FIELD_W).show_ui(ui, |ui| {
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
                    ("Ctrl+Shift+O", t("панель структуры и поиск по ней", "structure panel and its search")),
                    ("Alt+1 / Alt+2", t("спецификация / тело пакета", "package specification / body")),
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
            ui.separator();
            ui.label(RichText::new(format!("{}: {}", t("Иконки", "Icons"), crate::icons::credits())).small().weak());
        });
        if !open {
            app.about = false;
        }
    }
}
