//! The window: workspaces and their tabs, connections and what they're
//! doing, results. Everything a database does happens on the connection's
//! own threads (db.rs); here events from them are taken in each frame.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::config::{self, Connections, Session, Settings, Tab, ThemePref, Workspace};
use crate::db::{Event, Kind, Obj, Outcome, Relation, Req, Spec, Stmt, Table, TxOp, What, Worker};
use crate::editor::{self, Action, EditorState};
use crate::i18n::t;
use crate::sql::check::{self, Issue, Known, Sev};
use crate::sql::{self, Dialect};

#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Connecting,
    Ready,
    Failed(String),
}

/// A connection in use: its two workers and what's known of its database.
pub struct Live {
    pub spec: Spec,
    pub password: String,
    pub query: Worker,
    pub meta: Worker,
    pub state: ConnState,
    pub version: String,
    pub rels: Arc<Vec<Relation>>,
    pub known: Arc<Known>,
    pub rels_version: u64,
    pub rels_loading: bool,
    /// Each statement committed on its own (else by the Commit button).
    pub auto: bool,
    /// An open transaction holds changes not committed.
    pub pending: bool,
}

/// After the user says what becomes of uncommitted changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxThen {
    AutoOn,
    Disconnect,
    Reconnect,
    Quit,
}

pub enum Node {
    Loading,
    Ready(Vec<Obj>),
    Failed(String),
}

pub struct ResultView {
    pub title: String,
    pub sql: String,
    pub table: Table,
    pub took: Duration,
    /// Rows in the order shown.
    pub order: Vec<usize>,
    pub sort: Option<(usize, bool)>,
    pub sel: Option<(usize, usize)>,
    pub filter: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Run,
    Ok,
    Note,
    Error,
}

pub struct LogLine {
    pub when: String,
    pub kind: LogKind,
    pub text: String,
    /// Where in the tab's text (bytes) it's about.
    pub at: Option<usize>,
}

pub struct Running {
    pub run: u64,
    pub conn: String,
    pub since: Instant,
}

/// What a tab has that isn't saved: its results, problems, editor state.
#[derive(Default)]
pub struct TabRt {
    pub ed: EditorState,
    pub results: Vec<ResultView>,
    /// The result shown; None: the messages.
    pub shown: Option<usize>,
    pub log: Vec<LogLine>,
    pub run: Option<Running>,
    pub issues: Vec<Issue>,
    pub check_key: u64,
    pub check_due: Option<Instant>,
    pub server_err: Option<(Range<usize>, String)>,
    /// The structure panel's entities: (text key, specification / all, body).
    pub outline: Option<(u64, Vec<sql::outline::Entity>, Vec<sql::outline::Entity>)>,
    /// The text changed: worked out again once typing pauses.
    pub outline_due: Option<Instant>,
}

pub struct PasswordAsk {
    pub conn: String,
    pub password: String,
    pub save: bool,
    pub error: Option<String>,
    /// Run this once connected.
    pub then: Option<(String, Vec<Stmt>)>,
}

pub struct Confirm {
    pub text: String,
    pub tab: String,
    pub stmts: Vec<Stmt>,
}

pub struct App {
    pub settings: Settings,
    pub conns: Connections,
    pub session: Session,
    pub live: HashMap<String, Live>,
    pub tx: Sender<Event>,
    rx: Receiver<Event>,
    pub rt: HashMap<String, TabRt>,
    pub runs: HashMap<u64, String>,
    next_run: u64,
    pub tree: HashMap<(String, Vec<String>), Node>,
    pub open_nodes: HashSet<(String, Vec<String>)>,
    pub tree_filter: String,
    pub files: HashMap<std::path::PathBuf, Vec<(String, bool)>>,
    pub open_dirs: HashSet<std::path::PathBuf>,
    pub conn_edit: Option<crate::dialogs::ConnEdit>,
    pub ask: Option<PasswordAsk>,
    pub confirm: Option<Confirm>,
    /// Uncommitted changes on these connections: commit or roll back, then…
    pub tx_ask: Option<(Vec<String>, TxThen)>,
    /// Closing once the commits asked for are done (since when).
    pub quitting: Option<Instant>,
    pub outline_filter: String,
    /// The structure panel's search takes the keys next frame.
    pub outline_focus: bool,
    force_close: bool,
    pub settings_open: bool,
    pub ws_edit: Option<usize>,
    pub tab_rename: Option<(String, String)>,
    pub save_as: Option<(String, String)>,
    pub project_input: Option<String>,
    pub toast: Option<(String, Instant)>,
    dirty: bool,
    last_save: Instant,
    pub results_max: bool,
    applied: Option<(ThemePref, u32)>,
    pub about: bool,
    ctx: egui::Context,
    /// Connections started on their own once (not again after failing).
    auto_tried: HashSet<String>,
}

pub fn now() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

fn settings_path() -> std::path::PathBuf {
    config::config_dir().join("settings.json")
}
fn conns_path() -> std::path::PathBuf {
    config::config_dir().join("connections.json")
}
fn session_path() -> std::path::PathBuf {
    config::data_dir().join("session.json")
}

/// Typical auth failures: ask for the password then.
fn auth_failed(m: &str) -> bool {
    let l = m.to_lowercase();
    ["password", "authentication", "ora-01017", "ora-01005", "code: 516", "code: 194", "пароль", "подлинност"].iter().any(|s| l.contains(s))
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        let settings: Settings = config::load(&settings_path());
        crate::i18n::set_ru(settings.russian.unwrap_or_else(crate::i18n::system_is_ru));
        crate::fonts::install(&cc.egui_ctx);
        let mut session: Session = config::load(&session_path());
        if session.workspaces.is_empty() {
            session = Session::default();
        }
        session.active = session.active.min(session.workspaces.len() - 1);
        for ws in &mut session.workspaces {
            if ws.tabs.is_empty() {
                ws.tabs.push(Tab::default());
            }
            ws.active = ws.active.min(ws.tabs.len() - 1);
        }
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = Self {
            settings,
            conns: config::load(&conns_path()),
            session,
            live: HashMap::new(),
            tx,
            rx,
            rt: HashMap::new(),
            runs: HashMap::new(),
            next_run: 1,
            tree: HashMap::new(),
            open_nodes: HashSet::new(),
            tree_filter: String::new(),
            files: HashMap::new(),
            open_dirs: HashSet::new(),
            conn_edit: None,
            ask: None,
            confirm: None,
            tx_ask: None,
            quitting: None,
            outline_filter: String::new(),
            outline_focus: false,
            force_close: false,
            settings_open: false,
            ws_edit: None,
            tab_rename: None,
            save_as: None,
            project_input: None,
            toast: None,
            dirty: false,
            last_save: Instant::now(),
            results_max: false,
            applied: None,
            about: false,
            ctx: cc.egui_ctx.clone(),
            auto_tried: HashSet::new(),
        };
        if let Some(p) = app.session.project.clone() {
            app.open_dirs.insert(p);
        }
        app
    }

    pub fn ws(&self) -> &Workspace {
        &self.session.workspaces[self.session.active]
    }

    pub fn ws_mut(&mut self) -> &mut Workspace {
        let a = self.session.active;
        &mut self.session.workspaces[a]
    }

    pub fn tab(&self) -> &Tab {
        let ws = self.ws();
        &ws.tabs[ws.active]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        let ws = self.ws_mut();
        let a = ws.active;
        &mut ws.tabs[a]
    }

    pub fn touch(&mut self) {
        self.dirty = true;
    }

    pub fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    pub fn spec(&self, id: &str) -> Option<&Spec> {
        self.conns.list.iter().find(|s| s.id == id)
    }

    /// The connection a tab runs on.
    pub fn tab_conn(&self, ws: &Workspace, tab: &Tab) -> Option<String> {
        tab.conn.clone().or_else(|| ws.conn.clone()).filter(|c| self.spec(c).is_some())
    }

    pub fn dialect_of(&self, conn: Option<&str>) -> Dialect {
        conn.and_then(|c| self.spec(c)).map_or(Dialect::Generic, |s| s.kind.dialect())
    }

    pub fn save_conns(&mut self) {
        if let Err(e) = config::save(&conns_path(), &self.conns) {
            self.toast(format!("{}: {e}", t("Не сохранить подключения", "Can't save connections")));
        }
    }

    pub fn save_settings(&mut self) {
        config::save(&settings_path(), &self.settings).ok();
    }

    pub fn save_session(&mut self) {
        if config::save(&session_path(), &self.session).is_ok() {
            self.dirty = false;
            self.last_save = Instant::now();
        }
    }

    /// Starts the connection's workers (asking for the password first when
    /// it isn't saved). False: waiting for the password.
    pub fn ensure_live(&mut self, ctx: &egui::Context, conn: &str, then: Option<(String, Vec<Stmt>)>) -> bool {
        if self.live.contains_key(conn) {
            return true;
        }
        let Some(spec) = self.spec(conn).cloned() else { return false };
        if !spec.save_password && spec.kind != Kind::Sqlite {
            self.ask = Some(PasswordAsk { conn: conn.to_string(), password: String::new(), save: false, error: None, then });
            return false;
        }
        self.start_live(ctx, spec, String::new());
        true
    }

    pub fn start_live(&mut self, ctx: &egui::Context, spec: Spec, password: String) {
        let query = Worker::spawn(spec.clone(), password.clone(), self.tx.clone(), ctx.clone());
        let meta = Worker::spawn(spec.clone(), password.clone(), self.tx.clone(), ctx.clone());
        meta.send(Req::Connect);
        let id = spec.id.clone();
        let id_ = id.clone();
        self.tree.retain(|(c, _), _| *c != id);
        self.live.insert(id, Live { spec, password, query, meta, state: ConnState::Connecting, version: String::new(), rels: Arc::default(), known: Arc::default(), rels_version: 0, rels_loading: false, auto: true, pending: false });
        if let Some(l) = self.live.get_mut(&id_) {
            l.auto = l.spec.auto_commit || !l.spec.kind.transactions();
        }
    }

    /// Commit, roll back, or switch auto-commit; asks first when switching
    /// it on would commit changes unseen.
    pub fn tx(&mut self, conn: &str, op: TxOp) {
        let Some(l) = self.live.get_mut(conn) else { return };
        if op == TxOp::Auto(true) && l.pending {
            self.tx_ask = Some((vec![conn.to_string()], TxThen::AutoOn));
            return;
        }
        if let TxOp::Auto(on) = op {
            l.auto = on;
        }
        l.query.send(Req::Tx(op));
    }

    /// Shows the package's body (or specification) in the current tab.
    pub fn show_part(&mut self, body: bool) {
        let id = self.tab().id.clone();
        if self.tab_mut().show_part(body) {
            let rt = self.rt.entry(id).or_default();
            rt.issues.clear();
            rt.server_err = None;
            rt.check_key = 0;
            rt.check_due = None;
            rt.ed.popup = None;
            rt.ed.focus = true;
            self.touch();
        }
    }

    /// Commit / Roll back for the current tab's connection (by hand only).
    pub fn commit_current(&mut self, commit: bool) {
        let ws = self.ws().clone();
        let Some(c) = self.tab_conn(&ws, self.tab()) else { return };
        match self.live.get(&c) {
            Some(l) if !l.auto => self.tx(&c, if commit { TxOp::Commit } else { TxOp::Rollback }),
            Some(_) => self.toast(t("Автокоммит включён — фиксировать нечего", "Auto-commit is on: nothing to commit")),
            None => {}
        }
    }

    /// Disconnecting or reconnecting drops what isn't committed: asks.
    pub fn leave(&mut self, ctx: &egui::Context, conn: &str, then: TxThen) {
        if self.live.get(conn).is_some_and(|l| l.pending) {
            self.tx_ask = Some((vec![conn.to_string()], then));
        } else if then == TxThen::Reconnect {
            self.reconnect(ctx, conn);
        } else {
            self.disconnect(conn);
        }
    }

    /// The answer to `tx_ask`: Some(commit?) or None (cancel).
    pub fn tx_answer(&mut self, ctx: &egui::Context, conns: Vec<String>, then: TxThen, commit: bool) {
        let op = if commit { TxOp::Commit } else { TxOp::Rollback };
        for c in &conns {
            if let Some(l) = self.live.get(c) {
                l.query.send(Req::Tx(op));
            }
        }
        for c in &conns {
            match then {
                // (The worker does them in order: the commit first.)
                TxThen::AutoOn => {
                    if let Some(l) = self.live.get_mut(c) {
                        l.auto = true;
                        l.pending = false;
                        l.query.send(Req::Tx(TxOp::Auto(true)));
                    }
                }
                TxThen::Disconnect => self.disconnect(c),
                TxThen::Reconnect => self.reconnect(ctx, c),
                TxThen::Quit => {}
            }
        }
        if then == TxThen::Quit {
            self.quitting = Some(Instant::now());
        }
    }

    pub fn disconnect(&mut self, conn: &str) {
        self.live.remove(conn);
        self.tree.retain(|(c, _), _| c != conn);
        self.open_nodes.retain(|(c, _)| c != conn);
        for rt in self.rt.values_mut() {
            if rt.run.as_ref().is_some_and(|r| r.conn == conn) {
                rt.run = None;
                rt.log.push(LogLine { when: now(), kind: LogKind::Note, text: t("Отключено", "Disconnected").into(), at: None });
            }
        }
    }

    pub fn reconnect(&mut self, ctx: &egui::Context, conn: &str) {
        let pw = self.live.get(conn).map(|l| l.password.clone());
        self.disconnect(conn);
        match pw {
            Some(p) if !p.is_empty() => {
                if let Some(spec) = self.spec(conn).cloned() {
                    self.start_live(ctx, spec, p);
                }
            }
            _ => {
                self.ensure_live(ctx, conn, None);
            }
        }
    }

    pub fn request_children(&mut self, conn: &str, path: Vec<String>) {
        if let Some(l) = self.live.get(conn) {
            self.tree.insert((conn.to_string(), path.clone()), Node::Loading);
            l.meta.send(Req::Children { path });
        }
    }

    pub fn refresh_relations(&mut self, conn: &str) {
        if let Some(l) = self.live.get_mut(conn) {
            if !l.rels_loading {
                l.rels_loading = true;
                l.meta.send(Req::Relations { schemas: Vec::new() });
            }
        }
    }

    pub fn new_tab(&mut self, title: Option<String>, text: String, conn: Option<String>) {
        let ws = self.ws_mut();
        let n = ws.tabs.len() + 1;
        let tab = Tab { title: title.unwrap_or_else(|| format!("{} {n}", t("Скрипт", "Script"))), text, conn, ..Default::default() };
        let id = tab.id.clone();
        ws.tabs.push(tab);
        ws.active = ws.tabs.len() - 1;
        self.rt.entry(id).or_default().ed.focus = true;
        self.touch();
    }

    pub fn close_tab(&mut self, wi: usize, ti: usize) {
        let ws = &mut self.session.workspaces[wi];
        if ti >= ws.tabs.len() {
            return;
        }
        let tab = ws.tabs.remove(ti);
        if let Some(rt) = self.rt.remove(&tab.id) {
            if let Some(r) = rt.run {
                if let Some(l) = self.live.get(&r.conn) {
                    l.query.cancel();
                }
            }
        }
        if ws.tabs.is_empty() {
            ws.tabs.push(Tab { title: format!("{} 1", t("Скрипт", "Script")), ..Default::default() });
        }
        if ws.active >= ti && ws.active > 0 {
            ws.active -= 1;
        }
        ws.active = ws.active.min(ws.tabs.len() - 1);
        self.touch();
    }

    /// Opens a file (or the tab that has it).
    pub fn open_file(&mut self, path: &std::path::Path) {
        for (wi, ws) in self.session.workspaces.iter_mut().enumerate() {
            if let Some(ti) = ws.tabs.iter().position(|t| t.path.as_deref() == Some(path)) {
                ws.active = ti;
                self.session.active = wi;
                return;
            }
        }
        match std::fs::read(path) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
                let title = path.file_name().map(|n| n.to_string_lossy().into_owned());
                // An untouched empty tab gives way.
                let ws = self.ws_mut();
                if ws.tabs.len() == 1 && ws.tabs[0].text.is_empty() && ws.tabs[0].path.is_none() {
                    ws.tabs.clear();
                }
                self.new_tab(title, text, None);
                self.tab_mut().path = Some(path.to_path_buf());
                self.tab_mut().split_package();
            }
            Err(e) => self.toast(format!("{}: {e}", path.display())),
        }
    }

    pub fn save_tab(&mut self, wi: usize, ti: usize) {
        let tab = &self.session.workspaces[wi].tabs[ti];
        match tab.path.clone() {
            Some(p) => match std::fs::write(&p, tab.full_text()) {
                Ok(()) => {
                    self.session.workspaces[wi].tabs[ti].modified = false;
                    if let Some(dir) = p.parent() {
                        self.files.remove(dir);
                    }
                    self.toast(format!("{} {}", t("Сохранено:", "Saved:"), p.display()));
                    self.touch();
                }
                Err(e) => self.toast(format!("{}: {e}", p.display())),
            },
            None => {
                let dir = self.session.project.clone().or_else(dirs::home_dir).unwrap_or_default();
                let name = format!("{}.sql", tab.title.replace(['/', ' '], "_"));
                self.save_as = Some((tab.id.clone(), dir.join(name).to_string_lossy().into_owned()));
            }
        }
    }

    /// Statements to run: the selection, or the statement at the cursor, or all.
    fn pick(&self, all: bool) -> Option<(String, Vec<Stmt>, Dialect)> {
        let ws = self.ws();
        let tab = self.tab();
        let conn = self.tab_conn(ws, tab)?;
        let d = self.dialect_of(Some(&conn));
        let rt = self.rt.get(&tab.id);
        let text = &tab.text;
        let (base, part) = match rt.and_then(|r| editor::selection(&r.ed)) {
            Some(r) => (r.start, &text[r]),
            None if all => (0, text.as_str()),
            None => {
                let at = rt.and_then(|r| r.ed.cursor).map_or(0, |c| c.0);
                let r = sql::statement_at(text, at, d, self.settings.blank_line_splits)?;
                (r.start, &text[r])
            }
        };
        let stmts: Vec<Stmt> = sql::split(part, d, self.settings.blank_line_splits && all).into_iter().map(|r| Stmt { sql: sql::runnable(&part[r.clone()], d), at: base + r.start }).filter(|s| !s.sql.is_empty()).collect();
        (!stmts.is_empty()).then_some((conn, stmts, d))
    }

    pub fn run(&mut self, ctx: &egui::Context, all: bool, explain: bool) {
        let Some((conn, mut stmts, d)) = self.pick(all) else {
            if self.tab_conn(self.ws(), self.tab()).is_none() {
                self.toast(t("Выберите подключение для вкладки", "Pick a connection for the tab"));
            } else {
                self.toast(t("Нечего выполнять", "Nothing to run"));
            }
            return;
        };
        if explain {
            stmts.truncate(1);
            let s = &stmts[0].sql;
            let q = s.trim_end_matches(';');
            stmts = match d {
                Dialect::Oracle => vec![Stmt { sql: format!("EXPLAIN PLAN FOR {q}"), at: stmts[0].at }, Stmt { sql: "SELECT plan_table_output AS plan FROM TABLE(DBMS_XPLAN.DISPLAY())".into(), at: stmts[0].at }],
                Dialect::Sqlite => vec![Stmt { sql: format!("EXPLAIN QUERY PLAN {q}"), at: stmts[0].at }],
                Dialect::ClickHouse => vec![Stmt { sql: format!("EXPLAIN indexes = 1 {q}"), at: stmts[0].at }],
                _ => vec![Stmt { sql: format!("EXPLAIN {q}"), at: stmts[0].at }],
            };
        }
        let tab_id = self.tab().id.clone();
        if self.rt.get(&tab_id).is_some_and(|r| r.run.is_some()) {
            self.toast(t("Вкладка ещё выполняет запрос", "The tab is still running"));
            return;
        }
        // Problems found before running: ask first.
        let errors: Vec<String> = {
            let rt = self.rt.get(&tab_id);
            let text = &self.tab().text;
            stmts.iter().flat_map(|s| {
                let r = s.at..s.at + s.sql.len();
                rt.into_iter().flat_map(|rt| rt.issues.iter()).filter(move |i| i.sev == Sev::Error && i.range.start >= r.start && i.range.start < r.end).map(|i| format!("{}: {}", line_of(text, i.range.start), i.msg)).collect::<Vec<_>>()
            }).collect()
        };
        let unsafe_dml: Vec<&Stmt> = stmts.iter().filter(|s| check::check(&s.sql, d, None).iter().any(|i| i.sev == Sev::Warning && i.range.start == 0 && matches!(sql::first_word(&s.sql).to_ascii_uppercase().as_str(), "UPDATE" | "DELETE"))).collect();
        if !errors.is_empty() || (self.settings.confirm_unsafe && !unsafe_dml.is_empty()) {
            let mut text = String::new();
            if !errors.is_empty() {
                text += t("В запросе есть ошибки:\n", "The script has errors:\n");
                for e in errors.iter().take(8) {
                    text += &format!("  • {} {e}\n", t("строка", "line"));
                }
            }
            if !unsafe_dml.is_empty() {
                text += t("UPDATE / DELETE без WHERE изменит все строки таблицы.\n", "UPDATE / DELETE without WHERE changes every row.\n");
            }
            text += t("Всё равно выполнить?", "Run anyway?");
            self.confirm = Some(Confirm { text, tab: tab_id, stmts });
            return;
        }
        self.dispatch(ctx, tab_id, stmts, &conn);
    }

    pub fn dispatch(&mut self, ctx: &egui::Context, tab_id: String, stmts: Vec<Stmt>, conn: &str) {
        if !self.ensure_live(ctx, conn, Some((tab_id.clone(), stmts.clone()))) {
            return;
        }
        let run = self.next_run;
        self.next_run += 1;
        self.runs.insert(run, tab_id.clone());
        let limit = self.spec(conn).map(|s| s.row_limit).filter(|&n| n > 0).unwrap_or(self.settings.row_limit).max(1);
        let rt = self.rt.entry(tab_id).or_default();
        rt.results.clear();
        rt.shown = None;
        rt.server_err = None;
        rt.run = Some(Running { run, conn: conn.to_string(), since: Instant::now() });
        let l = &self.live[conn];
        l.query.send(Req::Run { run, stmts, limit, go_on: self.settings.continue_on_error });
    }

    pub fn cancel(&mut self) {
        let id = self.tab().id.clone();
        if let Some(r) = self.rt.get(&id).and_then(|r| r.run.as_ref()) {
            if let Some(l) = self.live.get(&r.conn) {
                l.query.cancel();
                // A stuck link: past a few seconds, the worker is dropped
                // and made anew, the run given up.
                if r.since.elapsed() > Duration::from_secs(5) && l.query.busy_since().is_some_and(|b| b.elapsed() > Duration::from_secs(5)) {
                    let conn = r.conn.clone();
                    let spec = l.spec.clone();
                    let pw = l.password.clone();
                    let rt = self.rt.get_mut(&id).unwrap();
                    rt.run = None;
                    rt.log.push(LogLine { when: now(), kind: LogKind::Error, text: t("Запрос брошен: сервер не отвечает на отмену, сессия пересоздана", "Run abandoned: the server didn't answer the cancel, session recreated").into(), at: None });
                    let ctx = self.ctx.clone();
                    if let Some(l) = self.live.get_mut(&conn) {
                        l.query = Worker::spawn(spec, pw, self.tx.clone(), ctx);
                    }
                }
            }
        }
    }

    pub fn format(&mut self) {
        let d = self.dialect_of(self.tab_conn(self.ws(), self.tab()).as_deref());
        let o = self.settings.format;
        let id = self.tab().id.clone();
        let sel = self.rt.get(&id).and_then(|r| editor::selection(&r.ed));
        let tab = self.tab_mut();
        match sel {
            Some(r) => {
                let f = sql::fmt::format(&tab.text[r.clone()], d, o);
                tab.text.replace_range(r.clone(), &f);
            }
            None => tab.text = sql::fmt::format(&tab.text, d, o),
        }
        tab.modified = tab.path.is_some();
        self.touch();
    }

    fn take_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.rx.try_recv() {
            let Some(l) = self.live.get_mut(&ev.conn) else { continue };
            let from_meta = ev.generation == l.meta.generation;
            if !from_meta && ev.generation != l.query.generation {
                continue;
            }
            match ev.what {
                What::Connecting => {
                    if from_meta || l.state != ConnState::Ready {
                        l.state = ConnState::Connecting;
                    }
                }
                What::Connected { version } => {
                    let first = l.state != ConnState::Ready;
                    l.state = ConnState::Ready;
                    l.version = version;
                    if from_meta && first {
                        let conn = ev.conn.clone();
                        self.request_children(&conn, Vec::new());
                        self.refresh_relations(&conn);
                        self.open_nodes.insert((conn, Vec::new()));
                    }
                }
                What::ConnectFailed(m) => {
                    l.state = ConnState::Failed(m.clone());
                    let conn = ev.conn.clone();
                    // Runs waiting on it end.
                    for rt in self.rt.values_mut() {
                        if rt.run.as_ref().is_some_and(|r| r.conn == conn) && !from_meta {
                            rt.run = None;
                            rt.log.push(LogLine { when: now(), kind: LogKind::Error, text: format!("{}: {m}", t("Нет подключения", "Not connected")), at: None });
                            rt.shown = None;
                        }
                    }
                    if from_meta && auth_failed(&m) && self.ask.is_none() {
                        let save = self.spec(&conn).is_some_and(|s| s.save_password);
                        self.live.remove(&conn);
                        self.ask = Some(PasswordAsk { conn, password: String::new(), save, error: Some(m), then: None });
                    } else if from_meta {
                        self.tree.insert((conn, Vec::new()), Node::Failed(m));
                    }
                }
                What::Lost(m) => {
                    let conn = ev.conn.clone();
                    let dropped = !from_meta && std::mem::take(&mut l.pending);
                    l.state = ConnState::Failed(m.clone());
                    let mut text = format!("{}: {m}", t("Связь потеряна, переподключусь при следующем запросе", "Link lost, will reconnect on the next request"));
                    if dropped {
                        text += &format!("\n{}", t("Незафиксированные изменения потеряны.", "Uncommitted changes are lost."));
                    }
                    let _ = conn;
                    self.toast(text);
                }
                What::Tx { auto, pending } => {
                    if !from_meta {
                        l.auto = auto;
                        l.pending = pending;
                    }
                }
                What::TxDone { op, result } => {
                    let name = l.spec.title();
                    match result {
                        Ok(()) => match op {
                            TxOp::Commit => self.toast(format!("{name}: {}", t("изменения зафиксированы", "changes committed"))),
                            TxOp::Rollback => self.toast(format!("{name}: {}", t("изменения откачены", "changes rolled back"))),
                            TxOp::Auto(_) => {}
                        },
                        Err(m) => {
                            self.quitting = None;
                            self.toast(format!("{name}: {m}"));
                        }
                    }
                }
                What::Started { run, index: _, at, sql } => {
                    if let Some(rt) = self.runs.get(&run).and_then(|t| self.rt.get_mut(t)) {
                        rt.log.push(LogLine { when: now(), kind: LogKind::Run, text: head(&sql, 200), at: Some(at) });
                    }
                }
                What::Note { run, text } => {
                    if let Some(rt) = self.runs.get(&run).and_then(|t| self.rt.get_mut(t)) {
                        rt.log.push(LogLine { when: now(), kind: LogKind::Note, text, at: None });
                    }
                }
                What::Result { run, index: _, at, sql, outcome, took } => {
                    let ddl = matches!(sql::first_word(&sql).to_ascii_uppercase().as_str(), "CREATE" | "DROP" | "ALTER" | "RENAME" | "TRUNCATE" | "COMMENT");
                    if let Some(rt) = self.runs.get(&run).and_then(|t| self.rt.get_mut(t)) {
                        let ms = fmt_took(took);
                        match outcome {
                            Ok(Outcome::Rows(table)) => {
                                let more = if table.more { format!(" ({})", t("есть ещё", "more exist")) } else { String::new() };
                                rt.log.push(LogLine { when: now(), kind: LogKind::Ok, text: format!("{} {}{more} · {ms}", table.rows.len(), t("строк", "rows")), at: Some(at) });
                                let order = (0..table.rows.len()).collect();
                                rt.results.push(ResultView { title: head(&sql, 28), sql, table, took, order, sort: None, sel: None, filter: String::new() });
                                if rt.shown.is_none() {
                                    rt.shown = Some(rt.results.len() - 1);
                                }
                            }
                            Ok(Outcome::Done(n)) => {
                                let what = match n {
                                    Some(n) => format!("{} {n}", t("Готово, строк затронуто:", "Done, rows affected:")),
                                    None => t("Готово", "Done").to_string(),
                                };
                                rt.log.push(LogLine { when: now(), kind: LogKind::Ok, text: format!("{what} · {ms}"), at: Some(at) });
                            }
                            Err(f) => {
                                let pos = f.at.map(|p| at + p);
                                if let Some(p) = pos {
                                    rt.server_err = Some((p..p + 1, f.message.clone()));
                                } else {
                                    rt.server_err = Some((at..at + sql.len().min(1), f.message.clone()));
                                }
                                rt.log.push(LogLine { when: now(), kind: LogKind::Error, text: f.message, at: pos.or(Some(at)) });
                                rt.shown = None;
                            }
                        }
                    }
                    if ddl {
                        let conn = ev.conn.clone();
                        self.refresh_relations(&conn);
                        // Open folders get looked at again.
                        let open: Vec<Vec<String>> = self.tree.keys().filter(|(c, _)| *c == conn).map(|(_, p)| p.clone()).collect();
                        for p in open {
                            if self.open_nodes.contains(&(conn.clone(), p.clone())) {
                                self.request_children(&conn, p);
                            } else {
                                self.tree.remove(&(conn.clone(), p));
                            }
                        }
                    }
                }
                What::Finished { run, cancelled } => {
                    if let Some(tab) = self.runs.remove(&run) {
                        if let Some(rt) = self.rt.get_mut(&tab) {
                            if rt.run.as_ref().is_some_and(|r| r.run == run) {
                                let took = rt.run.take().map(|r| r.since.elapsed()).unwrap_or_default();
                                if cancelled {
                                    rt.log.push(LogLine { when: now(), kind: LogKind::Note, text: t("Отменено", "Cancelled").into(), at: None });
                                } else {
                                    rt.log.push(LogLine { when: now(), kind: LogKind::Note, text: format!("{} · {}", t("Завершено", "Finished"), fmt_took(took)), at: None });
                                }
                            }
                        }
                    }
                }
                What::Children { path, items } => {
                    let node = match items {
                        Ok(v) => Node::Ready(v),
                        Err(e) => Node::Failed(e),
                    };
                    self.tree.insert((ev.conn.clone(), path), node);
                }
                What::Source { path: _, open_as, text } => match text {
                    Ok(s) => {
                        let conn = ev.conn.clone();
                        self.new_tab(Some(open_as), s, Some(conn));
                        // A package: specification and body apart.
                        self.tab_mut().split_package();
                    }
                    Err(e) => self.toast(e),
                },
                What::Relations(r) => {
                    l.rels_loading = false;
                    match r {
                        Ok(v) => {
                            l.known = Arc::new(Known::new(&v));
                            l.rels = Arc::new(v);
                            l.rels_version += 1;
                        }
                        Err(e) => {
                            let m = format!("{}: {e}", t("Не загрузить список таблиц", "Can't load the tables"));
                            self.toast(m);
                        }
                    }
                }
            }
        }
        let _ = ctx;
    }

    /// Re-checks tabs whose text changed (a moment after typing stops).
    fn checks(&mut self) {
        let ids: Vec<(String, String, Option<String>)> = self.session.workspaces.iter().flat_map(|ws| ws.tabs.iter().map(move |t| (t.id.clone(), t.text.clone(), t.conn.clone().or_else(|| ws.conn.clone())))).collect();
        let active = self.tab().id.clone();
        for (id, text, conn) in ids {
            if id != active {
                continue;
            }
            let d = self.dialect_of(conn.as_deref());
            let live = conn.as_deref().and_then(|c| self.live.get(c));
            let known = live.filter(|_| self.settings.check_names).map(|l| l.known.clone());
            let ver = live.map_or(0, |l| l.rels_version);
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (&text, d, ver, self.settings.check, self.settings.check_names).hash(&mut h);
                h.finish()
            };
            let rt = self.rt.entry(id).or_default();
            if rt.check_key == key {
                continue;
            }
            match rt.check_due {
                None => {
                    rt.check_due = Some(Instant::now() + Duration::from_millis(if rt.check_key == 0 { 0 } else { 250 }));
                }
                Some(due) if Instant::now() >= due => {
                    rt.check_due = None;
                    rt.check_key = key;
                    rt.issues = if self.settings.check && text.len() < 2_000_000 { check::check(&text, d, known.as_deref()) } else { Vec::new() };
                }
                Some(_) => {}
            }
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let want = (self.settings.theme, self.settings.ui_scale.to_bits());
        if self.applied == Some(want) {
            return;
        }
        self.applied = Some(want);
        ctx.set_theme(match self.settings.theme {
            ThemePref::System => egui::ThemePreference::System,
            ThemePref::Dark => egui::ThemePreference::Dark,
            ThemePref::Light => egui::ThemePreference::Light,
        });
        ctx.set_zoom_factor(self.settings.ui_scale.clamp(0.5, 3.0));
        ctx.all_styles_mut(crate::ui::standard);
    }

    /// The active tab's connection starts in the background (when no
    /// password needs asking), so hints and checks know its tables.
    fn auto_connect(&mut self, ctx: &egui::Context) {
        let Some(conn) = self.tab_conn(self.ws(), self.tab()) else { return };
        if self.live.contains_key(&conn) || self.auto_tried.contains(&conn) {
            return;
        }
        let Some(spec) = self.spec(&conn).cloned() else { return };
        self.auto_tried.insert(conn);
        if spec.save_password || spec.kind == Kind::Sqlite {
            self.start_live(ctx, spec, String::new());
        }
    }

    /// The window closing with changes uncommitted: asks first; once the
    /// commits are through (or 10 s gone), closes.
    fn closing(&mut self, ctx: &egui::Context) {
        if let Some(since) = self.quitting {
            let busy = self.live.values().any(|l| l.pending || l.query.busy_since().is_some());
            if !busy || since.elapsed() > Duration::from_secs(10) {
                self.quitting = None;
                self.save_session();
                self.force_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.force_close {
            let pending: Vec<String> = self.live.iter().filter(|(_, l)| l.pending).map(|(c, _)| c.clone()).collect();
            if !pending.is_empty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.tx_ask = Some((pending, TxThen::Quit));
            }
        }
    }

    fn global_keys(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        let (structure, spec, body) = ctx.input_mut(|i| (i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::O), i.consume_key(Modifiers::ALT, Key::Num1), i.consume_key(Modifiers::ALT, Key::Num2)));
        if structure {
            // Shown: to its search; shown and searched: hidden.
            if self.session.outline && ctx.memory(|m| m.has_focus(egui::Id::new("outline-filter"))) {
                self.session.outline = false;
            } else {
                self.session.outline = true;
                self.outline_focus = true;
            }
            self.touch();
        }
        if spec || body {
            self.show_part(body);
        }
        let (new, close, next, prev) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::T) || i.consume_key(Modifiers::COMMAND, Key::N),
                i.consume_key(Modifiers::COMMAND, Key::W),
                i.consume_key(Modifiers::COMMAND, Key::Tab) || i.consume_key(Modifiers::COMMAND, Key::PageDown),
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab) || i.consume_key(Modifiers::COMMAND, Key::PageUp),
            )
        });
        if new {
            self.new_tab(None, String::new(), None);
        }
        if close {
            let (wi, ti) = (self.session.active, self.ws().active);
            self.close_tab(wi, ti);
        }
        let n = self.ws().tabs.len();
        if next {
            let ws = self.ws_mut();
            ws.active = (ws.active + 1) % n;
        }
        if prev {
            let ws = self.ws_mut();
            ws.active = (ws.active + n - 1) % n;
        }
    }
}

pub fn line_of(text: &str, at: usize) -> usize {
    text[..crate::editor::floor(text, at)].matches('\n').count() + 1
}

pub fn head(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > n { format!("{}…", one.chars().take(n).collect::<String>()) } else { one }
}

pub fn fmt_took(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 { format!("{ms} ms") } else if ms < 60_000 { format!("{:.2} s", d.as_secs_f32()) } else { format!("{}m {}s", ms / 60_000, (ms / 1000) % 60) }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.take_events(&ctx);
        self.auto_connect(&ctx);
        self.checks();
        self.global_keys(&ctx);
        // Ticking clocks of running queries; checks waiting to happen.
        let busy = self.rt.values().any(|r| r.run.is_some()) || self.live.values().any(|l| l.state == ConnState::Connecting);
        if busy {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if self.rt.values().any(|r| r.check_due.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
        self.closing(&ctx);
        crate::side::panel(self, ui);
        crate::ui::status(self, ui);
        let actions = crate::ui::center(self, ui);
        for a in actions {
            match a {
                Action::RunStatement => self.run(&ctx, false, false),
                Action::RunScript => self.run(&ctx, true, false),
                Action::Explain => self.run(&ctx, false, true),
                Action::Format => self.format(),
                Action::Commit => self.commit_current(true),
                Action::Rollback => self.commit_current(false),
                Action::Cancel => self.cancel(),
                Action::Save => {
                    let (wi, ti) = (self.session.active, self.ws().active);
                    self.save_tab(wi, ti);
                }
            }
        }
        crate::dialogs::all(self, &ctx);
        if let Some((m, at)) = &self.toast {
            if at.elapsed() > Duration::from_secs(5) {
                self.toast = None;
            } else {
                let m = m.clone();
                egui::Area::new(egui::Id::new("toast")).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0)).order(egui::Order::Tooltip).show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(560.0);
                        ui.label(m);
                    });
                });
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
        if self.dirty && self.last_save.elapsed() > Duration::from_secs(self.settings.autosave_secs.max(1)) {
            self.save_session();
        }
        if self.dirty {
            ctx.request_repaint_after(Duration::from_secs(self.settings.autosave_secs.max(1)));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_session();
    }
}
