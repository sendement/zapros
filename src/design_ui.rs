//! The designers: a table's columns, a job and its schedule — in a tab of
//! their own (Ctrl+click in the tree; Designer | Code over it) or, for a
//! job, in a window (Designer | SQL).

use egui::{Color32, RichText};

use crate::app::App;
use crate::db::{Kind, ObjKind, Req};
use crate::design::{ColEdit, Column, JobDef, ObjDef, TableDef, edits_of, job_sql, table_changes};
use crate::i18n::t;
use crate::sched::Plan;

/// The job window's key (a tab's is its id).
pub const JOB_WIN: &str = "\u{1}job-window";

pub struct TableEdit {
    pub orig: TableDef,
    pub rows: Vec<ColEdit>,
    pub comment: String,
}

pub struct JobEdit {
    /// None: a new job.
    pub orig: Option<JobDef>,
    pub cur: JobDef,
    /// The schedule as picked (None: an expression the designer can't read).
    pub plan: Option<Plan>,
    /// The expression typed by hand (shown under the plan).
    pub raw: bool,
}

pub enum Loaded {
    Waiting,
    Failed(String),
    Table(TableEdit),
    Job(JobEdit),
}

pub struct Designer {
    pub conn: String,
    pub kind: Kind,
    pub path: Vec<String>,
    pub obj: ObjKind,
    pub loaded: Loaded,
    /// Statements sent, no answer yet.
    pub busy: bool,
    pub status: Option<Result<String, String>>,
}

pub struct JobWin {
    pub d: Designer,
    /// The tree folder it's listed in (refreshed after a change).
    pub folder: Vec<String>,
    /// SQL instead of the designer, and that SQL (as typed).
    pub sql_mode: bool,
    pub sql: String,
    pub open: bool,
}

impl Designer {
    pub fn new(conn: &str, kind: Kind, path: Vec<String>, obj: ObjKind) -> Self {
        Self { conn: conn.into(), kind, path, obj, loaded: Loaded::Waiting, busy: false, status: None }
    }

    /// What the database said it is.
    pub fn take(&mut self, def: Result<ObjDef, String>) {
        self.loaded = match def {
            Err(e) => Loaded::Failed(e),
            Ok(ObjDef::Table(tb)) => Loaded::Table(TableEdit { rows: edits_of(&tb), comment: tb.comment.clone(), orig: tb }),
            Ok(ObjDef::Job(j)) => Loaded::Job(JobEdit::of(self.kind, Some(j))),
        };
    }

    /// The statements that make the changes (none: nothing changed).
    pub fn changes(&self) -> Vec<String> {
        match &self.loaded {
            Loaded::Table(te) => table_changes(self.kind, &te.orig, &te.rows, &te.comment),
            Loaded::Job(je) => split(self.kind, &job_sql(self.kind, je.orig.as_ref(), &je.cur)),
            _ => Vec::new(),
        }
    }

    /// The changes as a script.
    pub fn script(&self) -> String {
        match &self.loaded {
            Loaded::Job(je) => job_sql(self.kind, je.orig.as_ref(), &je.cur),
            _ => crate::design::script(self.kind, &self.changes()),
        }
    }
}

impl JobEdit {
    pub fn of(kind: Kind, j: Option<JobDef>) -> Self {
        let cur = j.clone().unwrap_or_else(|| JobDef {
            enabled: true,
            action_type: if kind == Kind::Oracle { "PLSQL_BLOCK".into() } else { String::new() },
            action: if kind == Kind::Oracle { "BEGIN\n  NULL;\nEND;".into() } else { String::new() },
            schedule: if kind == Kind::Oracle { Plan::default().oracle() } else { Plan::default().cron() },
            ..Default::default()
        });
        let plan = if kind == Kind::Oracle { Plan::from_oracle(&cur.schedule) } else { Plan::from_cron(&cur.schedule) };
        Self { orig: j, cur, raw: plan.is_none(), plan }
    }
}

/// A script's statements (Oracle blocks up to their "/"), without the
/// comment-only ones.
fn split(kind: Kind, s: &str) -> Vec<String> {
    crate::sql::split(s, kind.dialect(), false)
        .into_iter()
        .map(|r| s[r].trim().to_string())
        .filter(|x| !x.is_empty() && !x.lines().all(|l| l.trim().is_empty() || l.trim_start().starts_with("--")))
        .collect()
}

/// Sends the statements for `key`'s designer.
fn apply(app: &mut App, ctx: &egui::Context, d: &mut Designer, key: &str, stmts: Vec<String>) {
    if stmts.is_empty() {
        return;
    }
    if !app.ensure_live(ctx, &d.conn, None) {
        d.status = Some(Err(t("Нет подключения", "Not connected").into()));
        return;
    }
    if let Some(l) = app.live.get(&d.conn) {
        l.meta.send(Req::Apply { key: key.into(), stmts });
        d.busy = true;
        d.status = None;
    }
}

/// Asks for the object (again).
pub fn reload(app: &mut App, ctx: &egui::Context, d: &mut Designer, key: &str) {
    if app.ensure_live(ctx, &d.conn, None) {
        if let Some(l) = app.live.get(&d.conn) {
            l.meta.send(Req::Describe { key: key.into(), path: d.path.clone(), kind: d.obj });
        }
    }
}

/// The SQL, coloured, as a label (copyable).
fn sql_view(app: &App, ui: &mut egui::Ui, d: crate::sql::Dialect, sql: &str) {
    let dark = ui.visuals().dark_mode;
    let cx = crate::editor::Ctx { dialect: d, settings: &app.settings, dark, issues: &[], server_err: None, rels: &[], rows: 1 };
    let mono_bold = ui.ctx().fonts(|f| f.families().iter().any(|fam| *fam == egui::FontFamily::Name("mono-bold".into())));
    let job = crate::editor::layout(sql, &cx, None, app.settings.font_size * 0.92, mono_bold);
    ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Extend).selectable(true));
}

/// SQL typed in a field, coloured.
fn sql_edit(app: &App, ui: &mut egui::Ui, d: crate::sql::Dialect, text: &mut String, rows: usize, id: &str) -> egui::Response {
    let dark = ui.visuals().dark_mode;
    let settings = app.settings.clone();
    let mono_bold = ui.ctx().fonts(|f| f.families().iter().any(|fam| *fam == egui::FontFamily::Name("mono-bold".into())));
    let mut layouter = |ui: &egui::Ui, s: &dyn egui::TextBuffer, _w: f32| {
        let cx = crate::editor::Ctx { dialect: d, settings: &settings, dark, issues: &[], server_err: None, rels: &[], rows: 1 };
        let job = crate::editor::layout(s.as_str(), &cx, None, settings.font_size * 0.92, mono_bold);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    ui.add(egui::TextEdit::multiline(text).id_salt(id).code_editor().desired_rows(rows).desired_width(f32::INFINITY).layouter(&mut layouter))
}

const CHANGED: Color32 = Color32::from_rgb(240, 180, 60);

/// Common types to pick from.
fn types(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Postgres => &["integer", "bigint", "smallint", "numeric(12,2)", "real", "double precision", "boolean", "text", "varchar(255)", "date", "timestamp", "timestamptz", "uuid", "jsonb", "bytea"],
        Kind::Oracle => &["NUMBER", "INTEGER", "NUMBER(12,2)", "VARCHAR2(255 CHAR)", "CHAR(1)", "CLOB", "BLOB", "DATE", "TIMESTAMP", "TIMESTAMP WITH TIME ZONE", "RAW(16)"],
        Kind::ClickHouse => &["UInt8", "UInt32", "UInt64", "Int32", "Int64", "Float64", "Decimal(12,2)", "String", "LowCardinality(String)", "Date", "DateTime", "DateTime64(3)", "UUID", "Bool", "Array(String)"],
        Kind::Sqlite => &["INTEGER", "REAL", "TEXT", "BLOB", "NUMERIC"],
    }
}

/// A text field; `was`: what it was (shown changed when it differs).
fn field(ui: &mut egui::Ui, s: &mut String, width: f32, was: Option<&str>, hint: &str) -> egui::Response {
    let changed = was.is_some_and(|w| w.trim() != s.trim());
    let mut te = egui::TextEdit::singleline(s).desired_width(width).hint_text(RichText::new(hint).weak());
    if changed {
        te = te.text_color(CHANGED);
    }
    ui.add_sized([width, ui.spacing().interact_size.y], te)
}

fn table_ui(app: &App, ui: &mut egui::Ui, kind: Kind, te: &mut TableEdit) {
    let d = kind.dialect();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("▦ {}", crate::design::table_name(kind, &te.orig))).strong().size(16.0));
        if kind != Kind::Sqlite {
            ui.add_space(12.0);
            ui.label(t("Комментарий", "Comment"));
            field(ui, &mut te.comment, ui.available_width().min(420.0), Some(te.orig.comment.as_str()), "");
        }
    });
    ui.add_space(6.0);
    let w = ui.available_width();
    let name_w = (w * 0.18).clamp(110.0, 240.0);
    let ty_w = (w * 0.18).clamp(110.0, 220.0);
    let def_w = (w * 0.15).clamp(80.0, 200.0);
    let com_w = (w - name_w - ty_w - def_w - 190.0).max(100.0);
    let no_comments = kind == Kind::Sqlite;
    egui::Grid::new("cols").num_columns(7).striped(true).spacing([8.0, 4.0]).show(ui, |ui| {
        for h in ["", t("Имя", "Name"), t("Тип", "Type"), "NULL", t("По умолчанию", "Default"), if no_comments { "" } else { t("Комментарий", "Comment") }, ""] {
            ui.label(RichText::new(h).weak());
        }
        ui.end_row();
        let mut remove = None;
        for (i, r) in te.rows.iter_mut().enumerate() {
            let o = r.from.map(|k| te.orig.cols[k].clone());
            let o = o.unwrap_or(Column { nullable: r.col.nullable, ..Default::default() });
            let new = r.from.is_none();
            ui.label(if r.col.pk { "🔑" } else if new { "+" } else { "" }).on_hover_text(if r.col.pk { t("Первичный ключ", "Primary key") } else { "" });
            if r.dropped {
                let strike = |s: &str| RichText::new(s.to_string()).strikethrough().color(crate::ui::RED);
                ui.label(strike(&o.name));
                ui.label(strike(&o.ty));
                ui.label("");
                ui.label("");
                ui.label("");
                if ui.small_button("↶").on_hover_text(t("Не удалять", "Keep it")).clicked() {
                    r.dropped = false;
                }
                ui.end_row();
                continue;
            }
            field(ui, &mut r.col.name, name_w, (!new).then_some(o.name.as_str()), t("имя", "name"));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                field(ui, &mut r.col.ty, ty_w - 24.0, (!new).then_some(o.ty.as_str()), t("тип", "type"));
                ui.menu_button("▾", |ui| {
                    for ty in types(kind) {
                        if ui.button(*ty).clicked() {
                            r.col.ty = ty.to_string();
                            if kind == Kind::ClickHouse && r.col.nullable {
                                r.col.ty = format!("Nullable({ty})");
                            }
                            ui.close();
                        }
                    }
                });
            });
            let was = r.col.nullable;
            let cb = ui.checkbox(&mut r.col.nullable, "");
            if !new && r.col.nullable != o.nullable {
                ui.painter().rect_stroke(cb.rect.expand(1.0), 4.0, egui::Stroke::new(1.0, CHANGED), egui::StrokeKind::Outside);
            }
            if kind == Kind::ClickHouse && was != r.col.nullable {
                let inner = r.col.ty.strip_prefix("Nullable(").and_then(|s| s.strip_suffix(')')).unwrap_or(&r.col.ty).to_string();
                r.col.ty = if r.col.nullable { format!("Nullable({inner})") } else { inner };
            }
            field(ui, &mut r.col.default, def_w, (!new).then_some(o.default.as_str()), "");
            if no_comments {
                ui.label("");
            } else {
                field(ui, &mut r.col.comment, com_w, (!new).then_some(o.comment.as_str()), "");
            }
            if ui.small_button("🗑").on_hover_text(t("Удалить колонку", "Drop the column")).clicked() {
                if new {
                    remove = Some(i);
                } else {
                    r.dropped = true;
                }
            }
            ui.end_row();
        }
        if let Some(i) = remove {
            te.rows.remove(i);
        }
    });
    ui.add_space(4.0);
    if ui.button(format!("➕ {}", t("Колонка", "Column"))).clicked() {
        te.rows.push(ColEdit { from: None, col: Column { nullable: true, ty: types(kind)[0].to_string(), ..Default::default() }, dropped: false });
        if kind == Kind::ClickHouse {
            te.rows.last_mut().unwrap().col.ty = format!("Nullable({})", types(kind)[0]);
        }
    }
    if !te.orig.info.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new(t("Ключи и прочее", "Keys and more")).weak());
        egui::Grid::new("info").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
            for (k, v) in &te.orig.info {
                ui.label(RichText::new(k).monospace());
                ui.label(RichText::new(v).monospace().weak());
                ui.end_row();
            }
        });
    }
    let _ = (app, d);
}

/// The job's fields and its schedule.
pub fn job_ui(app: &App, ui: &mut egui::Ui, kind: Kind, je: &mut JobEdit) {
    let d = kind.dialect();
    let new = je.orig.is_none();
    let orig = je.orig.clone().unwrap_or_default();
    egui::Grid::new("job").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(t("Имя", "Name"));
        if new {
            field(ui, &mut je.cur.name, 280.0, None, t("имя джоба", "job name"));
        } else {
            ui.label(RichText::new(if kind == Kind::Oracle { format!("{}.{}", je.cur.owner, je.cur.name) } else { je.cur.name.clone() }).strong());
        }
        ui.end_row();
        ui.label(t("Включён", "Enabled"));
        let r = ui.checkbox(&mut je.cur.enabled, "");
        if !new && je.cur.enabled != orig.enabled {
            ui.painter().rect_stroke(r.rect.expand(1.0), 4.0, egui::Stroke::new(1.0, CHANGED), egui::StrokeKind::Outside);
        }
        ui.end_row();
        if kind == Kind::Oracle {
            ui.label(t("Что запускает", "Runs"));
            egui::ComboBox::from_id_salt("job-type").selected_text(je.cur.action_type.clone()).show_ui(ui, |ui| {
                for ty in ["PLSQL_BLOCK", "STORED_PROCEDURE"] {
                    ui.selectable_value(&mut je.cur.action_type, ty.to_string(), ty);
                }
            });
            ui.end_row();
        } else {
            ui.label(t("База", "Database"));
            field(ui, &mut je.cur.database, 200.0, (!new).then_some(orig.database.as_str()), t("текущая", "current"));
            ui.end_row();
        }
        ui.label(t("Действие", "Action"));
        ui.vertical(|ui| {
            let plsql = je.cur.action_type != "STORED_PROCEDURE";
            if plsql {
                sql_edit(app, ui, d, &mut je.cur.action, 4, "job-action");
            } else {
                field(ui, &mut je.cur.action, 320.0, (!new).then_some(orig.action.as_str()), "SCHEMA.PROC");
            }
        });
        ui.end_row();
        if kind == Kind::Oracle {
            ui.label(t("Комментарий", "Comment"));
            field(ui, &mut je.cur.comment, 420.0, (!new).then_some(orig.comment.as_str()), "");
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    egui::Frame::group(ui.style()).corner_radius(8).inner_margin(10).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("Расписание", "Schedule")).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if je.plan.is_some() && ui.selectable_label(je.raw, t("как выражение", "as text")).clicked() {
                    je.raw = !je.raw;
                }
            });
        });
        ui.add_space(4.0);
        let cron = kind != Kind::Oracle;
        if let Some(p) = je.plan.as_mut() {
            if crate::sched::plan_ui(ui, p, cron) {
                je.cur.schedule = if cron { p.cron() } else { p.oracle() };
            }
            ui.add_space(4.0);
            ui.label(RichText::new(p.describe()).color(ui.visuals().strong_text_color()));
        } else {
            ui.label(RichText::new(t("Это выражение конструктор не разбирает — оно правится как текст.", "The designer can't read this expression: it's edited as text.")).weak());
            if ui.button(t("Задать в конструкторе", "Set it in the designer")).clicked() {
                let p = Plan::default();
                je.cur.schedule = if cron { p.cron() } else { p.oracle() };
                je.plan = Some(p);
                je.raw = false;
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(if cron { "cron:" } else { "repeat_interval:" }).weak());
            if je.raw || je.plan.is_none() {
                let w = ui.available_width().min(460.0);
                if field(ui, &mut je.cur.schedule, w, (!new).then_some(orig.schedule.as_str()), "").changed() {
                    let p = if cron { Plan::from_cron(&je.cur.schedule) } else { Plan::from_oracle(&je.cur.schedule) };
                    if p.is_some() {
                        je.plan = p;
                    }
                }
            } else {
                ui.label(RichText::new(&je.cur.schedule).monospace().color(if !new && je.cur.schedule != orig.schedule { CHANGED } else { ui.visuals().text_color() }));
            }
        });
        if let Some(p) = &je.plan {
            ui.add_space(4.0);
            crate::sched::next_ui(ui, p);
        }
    });
    if let Some(o) = &je.orig {
        if !o.info.is_empty() {
            ui.add_space(6.0);
            egui::Grid::new("job-info").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                for (k, v) in &o.info {
                    ui.label(RichText::new(k).weak());
                    ui.label(RichText::new(v).monospace());
                    ui.end_row();
                }
            });
        }
    }
}

/// The changes' SQL with Apply / Reset / To a script, and how it went.
fn changes_bar(app: &mut App, ui: &mut egui::Ui, d: &mut Designer, key: &str) -> bool {
    let ctx = ui.ctx().clone();
    let script = d.script();
    let mut reset = false;
    egui::Frame::NONE.fill(crate::ui::chrome(ui.visuals()).gamma_multiply(0.9)).corner_radius(8).inner_margin(10).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("Изменения", "Changes")).strong());
            if d.busy {
                ui.add(egui::Spinner::new().size(14.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let some = !script.trim().is_empty();
                if ui.add_enabled(some && !d.busy, crate::ui::tinted(format!("✔ {}", t("Применить", "Apply")), crate::ui::GREEN)).clicked() {
                    let stmts = d.changes().into_iter().filter(|s| !s.starts_with("--")).collect();
                    apply(app, &ctx, d, key, stmts);
                }
                if ui.add_enabled(some, egui::Button::new(t("Сбросить", "Reset"))).clicked() {
                    reset = true;
                }
                if ui.add_enabled(some, egui::Button::new(t("В новый скрипт", "To a new script"))).on_hover_text(t("Открыть эти команды во вкладке рядом", "Open these statements in a tab beside")).clicked() {
                    app.new_tab_next(Some(format!("{} — {}", d.path.last().cloned().unwrap_or_default(), t("изменения", "changes"))), script.clone(), Some(d.conn.clone()));
                }
            });
        });
        match &d.status {
            Some(Ok(m)) => {
                ui.label(RichText::new(m).color(crate::ui::GREEN));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(crate::ui::RED));
            }
            None => {}
        }
        if script.trim().is_empty() {
            ui.label(RichText::new(t("Пока ничего не изменено", "Nothing changed yet")).weak());
        } else {
            egui::ScrollArea::both().id_salt("changes").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| sql_view(app, ui, d.kind.dialect(), &script));
        }
    });
    reset
}

/// A tab's designer (in place of its text and results).
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let tab = app.tab().clone();
    let Some(obj) = tab.object.clone() else { return };
    let ctx = ui.ctx().clone();
    let mut d = match app.rt.get_mut(&tab.id).and_then(|r| r.design.take()) {
        Some(d) => d,
        None => {
            let kind = app.spec(&obj.conn).map_or(Kind::Postgres, |s| s.kind);
            let mut d = Designer::new(&obj.conn, kind, obj.path.clone(), obj.kind);
            reload(app, &ctx, &mut d, &tab.id);
            d
        }
    };
    egui::ScrollArea::vertical().id_salt(("designer", &tab.id)).auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(4.0);
        match &mut d.loaded {
            Loaded::Waiting => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(RichText::new(t("загрузка…", "loading…")).weak());
                });
            }
            Loaded::Failed(e) => {
                ui.colored_label(crate::ui::RED, e.as_str());
                if ui.button(t("Ещё раз", "Again")).clicked() {
                    d.loaded = Loaded::Waiting;
                    reload(app, &ctx, &mut d, &tab.id);
                }
            }
            Loaded::Table(te) => table_ui(app, ui, d.kind, te),
            Loaded::Job(je) => job_ui(app, ui, d.kind, je),
        }
        if matches!(d.loaded, Loaded::Table(_) | Loaded::Job(_)) {
            ui.add_space(10.0);
            if changes_bar(app, ui, &mut d, &tab.id) {
                d.loaded = Loaded::Waiting;
                reload(app, &ctx, &mut d, &tab.id);
            }
        }
    });
    if let Some(rt) = app.rt.get_mut(&tab.id) {
        rt.design = Some(d);
    }
}

/// Designer | Code over an object's tab, ↻ beside.
pub fn mode_bar(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let tab_id = app.tab().id.clone();
    let Some(obj) = app.tab().object.clone() else { return };
    ui.horizontal(|ui| {
        let icon = obj.kind.icon();
        for (code, label) in [(false, format!("{icon} {}", t("Конструктор", "Designer"))), (true, format!("{{ }} {}", t("Код", "Code")))] {
            if ui.selectable_label(obj.code == code, label).clicked() && obj.code != code {
                if let Some(o) = app.tab_mut().object.as_mut() {
                    o.code = code;
                }
                app.touch();
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("↻").on_hover_text(t("Перечитать из базы", "Read it from the database again")).clicked() {
                let kind = app.spec(&obj.conn).map_or(Kind::Postgres, |s| s.kind);
                let mut d = Designer::new(&obj.conn, kind, obj.path.clone(), obj.kind);
                reload(app, &ctx, &mut d, &tab_id);
                app.rt.entry(tab_id.clone()).or_default().design = Some(d);
                app.load_source(&obj.conn, &obj.path, obj.kind, &tab_id);
            }
            if let Some(d) = app.rt.get(&tab_id).and_then(|r| r.design.as_ref()) {
                let n = d.changes().len();
                if n > 0 {
                    ui.label(RichText::new(format!("● {}: {n}", t("не применено изменений", "changes not applied"))).color(CHANGED).small());
                }
            }
        });
    });
}

/// The job window: Designer | SQL.
pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(mut w) = app.job_win.take() else { return };
    let title = match &w.d.loaded {
        Loaded::Job(je) if je.orig.is_none() => t("Новый джоб", "New job").to_string(),
        _ => format!("{} {}", t("Джоб", "Job"), w.d.path.last().cloned().unwrap_or_default()),
    };
    let mut open = w.open;
    let center = ctx.content_rect().center();
    egui::Window::new(title).id(egui::Id::new("job-window")).open(&mut open).pivot(egui::Align2::CENTER_CENTER).default_pos(center).default_width(680.0).default_height((ctx.content_rect().height() - 80.0).min(760.0)).resizable(true).collapsible(false).show(ctx, |ui| {
        ui.horizontal(|ui| {
            let was = w.sql_mode;
            if ui.selectable_label(!w.sql_mode, format!("⏱ {}", t("Конструктор", "Designer"))).clicked() {
                w.sql_mode = false;
            }
            if ui.selectable_label(w.sql_mode, format!("{{ }} {}", t("SQL", "SQL"))).clicked() {
                w.sql_mode = true;
            }
            if w.sql_mode && !was {
                w.sql = w.d.script();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_tab = !w.d.path.is_empty() && matches!(&w.d.loaded, Loaded::Job(je) if je.orig.is_some());
                if can_tab && ui.button(t("Во вкладке", "In a tab")).on_hover_text(t("Открыть джоб во вкладке рядом", "Open the job in a tab beside")).clicked() {
                    let title = w.d.path.last().cloned().unwrap_or_default();
                    app.open_object(&w.d.conn, w.d.path.clone(), ObjKind::Job, title);
                    w.open = false;
                }
            });
        });
        ui.separator();
        let busy = w.d.busy;
        egui::ScrollArea::vertical().id_salt("job-win-scroll").max_height(ui.available_height() - 70.0).auto_shrink([false, true]).show(ui, |ui| {
            if w.sql_mode {
                ui.label(RichText::new(t("Что будет выполнено (можно править):", "What runs (can be edited):")).weak());
                let d = w.d.kind.dialect();
                sql_edit(app, ui, d, &mut w.sql, 12, "job-sql");
            } else {
                match &mut w.d.loaded {
                    Loaded::Waiting => {
                        ui.add(egui::Spinner::new());
                    }
                    Loaded::Failed(e) => {
                        ui.colored_label(crate::ui::RED, e.as_str());
                    }
                    Loaded::Job(je) => job_ui(app, ui, w.d.kind, je),
                    Loaded::Table(_) => {}
                }
            }
        });
        ui.separator();
        match &w.d.status {
            Some(Ok(m)) => {
                ui.colored_label(crate::ui::GREEN, m.as_str());
            }
            Some(Err(e)) => {
                ui.colored_label(crate::ui::RED, e.as_str());
            }
            None => {}
        }
        ui.horizontal(|ui| {
            let script = if w.sql_mode { w.sql.clone() } else { w.d.script() };
            let some = !script.trim().is_empty();
            if busy {
                ui.add(egui::Spinner::new().size(14.0));
            }
            if ui.add_enabled(some && !busy, crate::ui::tinted(format!("✔ {}", t("Применить", "Apply")), crate::ui::GREEN)).clicked() {
                let stmts = split(w.d.kind, &script);
                apply(app, ctx, &mut w.d, JOB_WIN, stmts);
            }
            if !w.sql_mode && !some {
                ui.label(RichText::new(t("Пока ничего не изменено", "Nothing changed yet")).weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t("Закрыть", "Close")).clicked() {
                    w.open = false;
                }
            });
        });
    });
    if open && w.open {
        app.job_win = Some(w);
    }
}
