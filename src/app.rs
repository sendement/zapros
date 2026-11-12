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

#[derive(Default)]
pub struct ResultView {
    pub title: String,
    pub sql: String,
    pub table: Table,
    pub took: Duration,
    /// Rows in the order shown.
    pub order: Vec<usize>,
    /// Sorted by these columns, first first (ascending: true).
    pub sort: Vec<(usize, bool)>,
    /// The columns in the order shown.
    pub cols: Vec<usize>,
    /// Columns picked (by their header).
    pub picked: std::collections::BTreeSet<usize>,
    /// Rows picked (by their number).
    pub rows_picked: std::collections::BTreeSet<usize>,
    /// Totals asked for, shown under the table: column → (what, value).
    pub totals: std::collections::BTreeMap<usize, (crate::results::Agg, String)>,
    /// Rows changed since the totals were worked out.
    pub totals_stale: bool,
    pub sel: Option<(usize, usize)>,
    pub filter: String,
    /// Its cursor's still open on the server: (run, connection).
    pub cursor: Option<(u64, String)>,
    /// More rows asked for, not here yet (true: to the end).
    pub loading: Option<bool>,
    /// Saved once every row's in.
    pub export_after: Option<crate::export::Export>,
}

impl ResultView {
    pub fn new(sql: String, table: Table, took: Duration, cursor: Option<(u64, String)>) -> Self {
        let order = (0..table.rows.len()).collect();
        let cols = (0..table.cols.len()).collect();
        Self { title: head(&sql, 28), sql, table, took, order, cols, cursor, ..Default::default() }
    }

    /// Its columns' names, as a key for the order they're shown in.
    pub fn shape(&self) -> String {
        self.table.cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join("\u{1}")
    }

    /// Shows the columns in the order kept for this shape, if any.
    pub fn arrange(&mut self, kept: &HashMap<String, Vec<usize>>) {
        if let Some(o) = kept.get(&self.shape()).filter(|o| o.len() == self.table.cols.len()) {
            self.cols = o.clone();
        }
    }
}

/// What the results area shows.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Pane {
    #[default]
    Messages,
    /// DBMS_OUTPUT, RAISE NOTICE…
    Output,
    Result(usize),
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
    /// An object's tab: its designer.
    pub design: Option<crate::design_ui::Designer>,
    pub results: Vec<ResultView>,
    pub shown: Pane,
    pub log: Vec<LogLine>,
    /// What the server printed (DBMS_OUTPUT, RAISE NOTICE…).
    pub output: Vec<LogLine>,
    /// Column orders set by hand, by the columns' names (kept for the next
    /// runs giving the same columns).
    pub col_orders: HashMap<String, Vec<usize>>,
    /// A notebook: the cell last edited, the Markdown cells being edited.
    pub cell_focus: Option<String>,
    pub md_edit: std::collections::BTreeSet<String>,
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
    /// Open cursors: run → tab.
    pub cursors: HashMap<u64, String>,
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
    /// When the last frame began (frames are paced by hand: see main.rs).
    pub last_frame: Option<Instant>,
    /// The save-a-result window: (tab, result, what).
    pub export: Option<(String, usize, crate::export::Export)>,
    dirty: bool,
    last_save: Instant,
    pub results_max: bool,
    applied: Option<(ThemePref, u32, String)>,
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
            for tb in ws.tabs.iter_mut().filter(|t| t.other.is_some()) {
                tb.text = config::unslashed(&tb.text);
                tb.other = tb.other.as_deref().map(config::unslashed);
            }
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
            cursors: HashMap::new(),
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
            last_frame: None,
            export: None,
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

    /// A new tab right after the current one (its id).
    pub fn new_tab_next(&mut self, title: Option<String>, text: String, conn: Option<String>) -> String {
        let ws = self.ws_mut();
        let n = ws.tabs.len() + 1;
        let tab = Tab { title: title.unwrap_or_else(|| format!("{} {n}", t("Скрипт", "Script"))), text, conn, ..Default::default() };
        let id = tab.id.clone();
        let at = (ws.active + 1).min(ws.tabs.len());
        ws.tabs.insert(at, tab);
        ws.active = at;
        self.rt.entry(id.clone()).or_default().ed.focus = true;
        self.touch();
        id
    }

    /// The object's code into tab `into`.
    pub fn load_source(&mut self, conn: &str, path: &[String], kind: crate::db::ObjKind, into: &str) {
        let ctx = self.ctx.clone();
        if self.ensure_live(&ctx, conn, None) {
            if let Some(l) = self.live.get(conn) {
                l.meta.send(Req::Source { path: path.to_vec(), kind, open_as: String::new(), into: Some(into.to_string()) });
            }
        }
    }

    /// The object's code in a tab beside the current one.
    pub fn open_source_next(&mut self, conn: &str, path: Vec<String>, kind: crate::db::ObjKind, title: String) {
        let id = self.new_tab_next(Some(title), String::new(), Some(conn.to_string()));
        self.load_source(conn, &path, kind, &id);
    }

    /// A table or a job in a tab beside the current one: its designer
    /// (its code a switch away).
    pub fn open_object(&mut self, conn: &str, path: Vec<String>, kind: crate::db::ObjKind, title: String) {
        // Already open: there.
        let wi = self.session.active;
        if let Some(i) = self.session.workspaces[wi].tabs.iter().position(|tb| tb.object.as_ref().is_some_and(|o| o.conn == conn && o.path == path)) {
            self.session.workspaces[wi].active = i;
            self.touch();
            return;
        }
        let id = self.new_tab_next(Some(format!("{} {title}", kind.icon())), String::new(), Some(conn.to_string()));
        self.tab_mut().object = Some(config::ObjRef { conn: conn.to_string(), path: path.clone(), kind, code: false });
        self.load_source(conn, &path, kind, &id);
    }

    /// A name Ctrl+clicked in the code: looked up on the tab's connection
    /// (opened when found).
    pub fn locate(&mut self, ctx: &egui::Context, schema: Option<String>, name: String, call: bool) {
        let ws = self.ws().clone();
        let Some(conn) = self.tab_conn(&ws, self.tab()) else {
            self.toast(t("У вкладки нет подключения", "The tab has no connection"));
            return;
        };
        if self.ensure_live(ctx, &conn, None) {
            if let Some(l) = self.live.get(&conn) {
                l.meta.send(Req::Locate { schema, name, call });
            }
        }
    }

    /// A new job's tab (its designer), to be made in `folder`.
    pub fn new_job(&mut self, conn: &str, folder: Vec<String>) {
        self.new_tab_next(Some(format!("⏱ {}", t("Новый джоб", "New job"))), String::new(), Some(conn.to_string()));
        self.tab_mut().object = Some(config::ObjRef { conn: conn.to_string(), path: folder, kind: crate::db::ObjKind::Job, code: false });
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
                // Saved as a notebook (its -- %% marks): opened as one.
                if sql::notebook::is_notebook(&self.tab().text) {
                    let d = self.dialect_of(self.tab_conn(self.ws(), self.tab()).as_deref());
                    self.tab_mut().set_notebook(true, d);
                } else {
                    self.tab_mut().split_package();
                }
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

    /// A notebook cell's own state (results, editor…), apart from its tab's.
    pub fn cell_key(tab: &str, cell: &str) -> String {
        format!("{tab}#{cell}")
    }

    /// Runs notebook cells of the current tab (None: all of them, in
    /// order); each statement gets the CTEs it uses from WITH cells above.
    pub fn run_cells(&mut self, ctx: &egui::Context, only: Option<String>) {
        let tab = self.tab().clone();
        let Some(nb) = tab.notebook.as_ref() else { return };
        let Some(conn) = self.tab_conn(self.ws(), &tab) else {
            self.toast(t("Выберите подключение для вкладки", "Pick a connection for the tab"));
            return;
        };
        let d = self.dialect_of(Some(&conn));
        let mut known = sql::notebook::Known::default();
        let mut todo: Vec<(String, Vec<Stmt>)> = Vec::new();
        for c in nb.cells.iter().filter(|c| c.kind == crate::config::CellKind::Sql) {
            let mut stmts = Vec::new();
            for r in sql::split(&c.text, d, false) {
                let s = sql::runnable(&c.text[r.clone()], d);
                if s.is_empty() {
                    continue;
                }
                let only_with = sql::notebook::parse_with(&s, d).is_some_and(|w| w.main.is_empty());
                // A WITH cell run on its own shows its last CTE; run with
                // the rest, it only lends its CTEs to the cells below.
                if only.is_some() || !only_with {
                    stmts.push(Stmt { sql: sql::notebook::glue(&s, &known, d), at: r.start });
                }
                known.learn(&s, d);
            }
            let wanted = only.as_ref().is_none_or(|o| *o == c.id);
            if wanted && !stmts.is_empty() {
                todo.push((c.id.clone(), stmts));
            }
            if only.as_ref().is_some_and(|o| *o == c.id) {
                break;
            }
        }
        if todo.is_empty() {
            if only.is_some() {
                self.toast(t("Нечего выполнять", "Nothing to run"));
            }
            return;
        }
        for (cell, stmts) in todo {
            let key = Self::cell_key(&tab.id, &cell);
            if self.rt.get(&key).is_some_and(|r| r.run.is_some()) {
                continue;
            }
            self.rt.entry(key.clone()).or_default().output.clear();
            self.dispatch(ctx, key, stmts, &conn);
        }
    }

    /// The cell's results and output, kept in the notebook (written to
    /// disk with the session).
    pub fn pin_cell(&mut self, cell: &str) {
        let key = Self::cell_key(&self.tab().id, cell);
        let Some(rt) = self.rt.get(&key) else { return };
        let saved = crate::config::Saved {
            when: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            tables: rt
                .results
                .iter()
                .map(|v| crate::config::SavedTable {
                    sql: v.sql.clone(),
                    cols: v.table.cols.iter().map(|c| (c.name.clone(), c.ty.clone(), c.numeric)).collect(),
                    rows: v.table.rows.clone(),
                    more: v.table.more,
                })
                .collect(),
            output: rt.output.iter().map(|l| l.text.clone()).collect(),
        };
        if let Some(c) = self.tab_mut().notebook.as_mut().and_then(|n| n.cells.iter_mut().find(|c| c.id == cell)) {
            c.saved = Some(saved);
        }
        self.touch();
        self.toast(t("Вывод блока сохранён", "The cell's output is kept"));
    }

    /// A cell's saved output, shown when nothing's been run there yet.
    pub fn restore_cell(&mut self, tab: &str, cell: &crate::config::Cell) {
        let key = Self::cell_key(tab, &cell.id);
        if self.rt.contains_key(&key) {
            return;
        }
        let mut rt = TabRt::default();
        if let Some(s) = &cell.saved {
            for tb in &s.tables {
                let table = Table { cols: tb.cols.iter().map(|(name, ty, numeric)| crate::db::Col { name: name.clone(), ty: ty.clone(), numeric: *numeric }).collect(), rows: tb.rows.clone(), more: tb.more };
                rt.results.push(ResultView::new(tb.sql.clone(), table, Duration::ZERO, None));
            }
            rt.output = s.output.iter().map(|o| LogLine { when: s.when.clone(), kind: LogKind::Note, text: o.clone(), at: None }).collect();
            rt.log.push(LogLine { when: s.when.clone(), kind: LogKind::Note, text: t("Сохранённый вывод", "Saved output").into(), at: None });
            rt.shown = if rt.results.is_empty() { if rt.output.is_empty() { Pane::Messages } else { Pane::Output } } else { Pane::Result(0) };
        }
        self.rt.insert(key, rt);
    }

    /// The notebook as plain SQL, in a new tab.
    pub fn export_notebook(&mut self, standalone: bool) {
        let tab = self.tab().clone();
        let Some(nb) = tab.notebook.as_ref() else { return };
        let d = self.dialect_of(self.tab_conn(self.ws(), &tab).as_deref());
        let text = sql::notebook::export(&nb.pieces(), d, standalone);
        let title = format!("{} (SQL)", if tab.title.is_empty() { t("блокнот", "notebook") } else { tab.title.as_str() });
        self.new_tab(Some(title), text, tab.conn.clone());
    }

    /// The current tab as a notebook or as one text.
    pub fn toggle_notebook(&mut self) {
        let d = self.dialect_of(self.tab_conn(self.ws(), self.tab()).as_deref());
        let on = !self.tab().is_notebook();
        self.tab_mut().set_notebook(on, d);
        self.touch();
    }

    pub fn run(&mut self, ctx: &egui::Context, all: bool, explain: bool) {
        // A notebook: the cell being edited, or every cell.
        if self.tab().is_notebook() {
            let focus = self.rt.get(&self.tab().id).and_then(|r| r.cell_focus.clone());
            match (all, focus) {
                (false, Some(c)) => self.run_cells(ctx, Some(c)),
                (false, None) => self.toast(t("Выберите блок", "Pick a cell")),
                (true, _) => self.run_cells(ctx, None),
            }
            return;
        }
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
        let limit = self.batch(conn);
        let rt = self.rt.entry(tab_id).or_default();
        rt.results.clear();
        rt.shown = Pane::Messages;
        rt.server_err = None;
        rt.run = Some(Running { run, conn: conn.to_string(), since: Instant::now() });
        let l = &self.live[conn];
        l.query.send(Req::Run { run, stmts, limit, go_on: self.settings.continue_on_error });
    }

    /// Rows a batch takes.
    pub fn batch(&self, conn: &str) -> usize {
        self.spec(conn).map(|s| s.row_limit).filter(|&n| n > 0).unwrap_or(self.settings.row_limit).max(1)
    }

    /// More rows of a result whose cursor's open (`all`: to the end).
    pub fn fetch_more(&mut self, tab: &str, i: usize, all: bool) {
        let Some((run, conn)) = self.rt.get(tab).and_then(|r| r.results.get(i)).and_then(|v| v.cursor.clone()) else { return };
        let n = self.batch(&conn);
        let Some(l) = self.live.get(&conn) else { return };
        l.query.send(Req::More { run, n, all });
        if let Some(v) = self.rt.get_mut(tab).and_then(|r| r.results.get_mut(i)) {
            v.loading = Some(all);
        }
    }

    /// The result's cursor closes (what's loaded stays).
    pub fn fetch_stop(&mut self, tab: &str, i: usize) {
        let Some((_, conn)) = self.rt.get(tab).and_then(|r| r.results.get(i)).and_then(|v| v.cursor.clone()) else { return };
        if let Some(l) = self.live.get(&conn) {
            l.query.send(Req::StopFetch);
        }
    }

    pub fn cancel(&mut self) {
        let id = self.tab().id.clone();
        // A notebook's cells: each one running stops.
        if self.tab().is_notebook() {
            let prefix = format!("{id}#");
            let running: Vec<String> = self.rt.iter().filter(|(k, r)| k.starts_with(&prefix) && r.run.is_some()).map(|(k, _)| k.clone()).collect();
            let conns: Vec<String> = running.iter().filter_map(|k| self.rt[k].run.as_ref().map(|r| r.conn.clone())).collect();
            for c in conns {
                if let Some(l) = self.live.get(&c) {
                    l.query.cancel();
                }
            }
            if running.is_empty() {
                let loading: Vec<(String, usize)> = self.rt.iter().filter(|(k, _)| k.starts_with(&prefix)).flat_map(|(k, r)| r.results.iter().enumerate().filter(|(_, v)| v.loading.is_some()).map(|(i, _)| (k.clone(), i)).collect::<Vec<_>>()).collect();
                for (k, i) in loading {
                    self.fetch_stop(&k, i);
                }
            }
            return;
        }
        // Nothing running: rows being loaded stop.
        if self.rt.get(&id).is_some_and(|r| r.run.is_none()) {
            let loading: Vec<usize> = self.rt[&id].results.iter().enumerate().filter(|(_, v)| v.loading.is_some()).map(|(i, _)| i).collect();
            for i in loading {
                self.fetch_stop(&id, i);
            }
            return;
        }
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
        if self.tab().is_notebook() {
            let d = self.dialect_of(self.tab_conn(self.ws(), self.tab()).as_deref());
            let o = self.settings.format;
            let focus = self.rt.get(&self.tab().id).and_then(|r| r.cell_focus.clone());
            if let Some(c) = self.tab_mut().notebook.as_mut().and_then(|n| n.cells.iter_mut().find(|c| Some(&c.id) == focus.as_ref() && c.kind == crate::config::CellKind::Sql)) {
                c.text = sql::fmt::format(&c.text, d, o).trim_end().to_string();
            }
            self.touch();
            return;
        }
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
                            rt.shown = Pane::Messages;
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
                        rt.output.push(LogLine { when: now(), kind: LogKind::Note, text, at: None });
                    }
                }
                What::Page { run, at, sql, table, took } => {
                    let conn = ev.conn.clone();
                    if let Some(tab) = self.runs.get(&run).cloned() {
                        let rt = self.rt.entry(tab.clone()).or_default();
                        rt.log.push(LogLine { when: now(), kind: LogKind::Ok, text: format!("{} {} ({}) · {}", table.rows.len(), t("строк", "rows"), t("есть ещё — догружаются по запросу", "more to load on demand"), fmt_took(took)), at: Some(at) });
                        let mut v = ResultView::new(sql, table, took, Some((run, conn)));
                        v.arrange(&rt.col_orders);
                        rt.results.push(v);
                        if rt.shown == Pane::Messages {
                            rt.shown = Pane::Result(rt.results.len() - 1);
                        }
                        self.cursors.insert(run, tab);
                    }
                }
                What::Rows { run, rows, more, open, error } => {
                    let Some(tab) = self.cursors.get(&run).cloned() else { continue };
                    if !open {
                        self.cursors.remove(&run);
                    }
                    let Some(rt) = self.rt.get_mut(&tab) else { continue };
                    if let Some(e) = error {
                        rt.log.push(LogLine { when: now(), kind: LogKind::Error, text: e, at: None });
                    }
                    let mut done_export = None;
                    if let Some(v) = rt.results.iter_mut().find(|v| v.cursor.as_ref().is_some_and(|c| c.0 == run)) {
                        let from = v.table.rows.len();
                        v.table.rows.extend(rows);
                        v.table.more = more;
                        v.totals_stale = true;
                        if !v.sort.is_empty() || !v.filter.is_empty() {
                            crate::results::reorder(v);
                        } else {
                            v.order.extend(from..v.table.rows.len());
                        }
                        if !open {
                            v.cursor = None;
                            v.loading = None;
                            done_export = v.export_after.take().map(|e| (e, crate::export::Snapshot::of(v)));
                        } else if v.loading == Some(false) {
                            v.loading = None;
                        }
                    }
                    if let Some((e, snap)) = done_export {
                        let msg = crate::export::save(&e, &snap);
                        self.toast(msg);
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
                                let mut v = ResultView::new(sql, table, took, None);
                                v.arrange(&rt.col_orders);
                                rt.results.push(v);
                                if rt.shown == Pane::Messages {
                                    rt.shown = Pane::Result(rt.results.len() - 1);
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
                                rt.shown = Pane::Messages;
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
                What::Source { path: _, open_as: _, into: Some(into), text } => {
                    let s = match text {
                        Ok(s) => s,
                        Err(e) => format!("-- {}\n", e.replace('\n', "\n-- ")),
                    };
                    if let Some(tb) = self.session.workspaces.iter_mut().flat_map(|w| w.tabs.iter_mut()).find(|tb| tb.id == into) {
                        tb.text = s;
                        tb.other = None;
                        if tb.object.is_none() {
                            tb.split_package();
                        }
                        self.touch();
                    }
                }
                What::Located { name, found } => match found {
                    Ok(Some((path, kind))) => {
                        let conn = ev.conn.clone();
                        let title = path.last().cloned().unwrap_or(name).split('(').next().unwrap_or_default().to_string();
                        match kind {
                            crate::db::ObjKind::Table | crate::db::ObjKind::Job => self.open_object(&conn, path, kind, title),
                            k if k.has_source() || k == crate::db::ObjKind::Synonym => self.open_source_next(&conn, path, k, title),
                            _ => {}
                        }
                    }
                    Ok(None) => self.toast(format!("«{name}» — {}", t("в базе не найдено", "not found in the database"))),
                    Err(e) => self.toast(e),
                },
                What::Described { key, def } => {
                    if let Some(d) = self.rt.get_mut(&key).and_then(|r| r.design.as_mut()) {
                        d.take(def);
                    }
                }
                What::Applied { key, result } => {
                    let conn = ev.conn.clone();
                    let ctx = self.ctx.clone();
                    let msg = |n: usize| format!("{}: {n}", t("Готово, выполнено команд", "Done, statements run"));
                    if let Some(mut d) = self.rt.get_mut(&key).and_then(|r| r.design.take()) {
                        d.busy = false;
                        match result {
                            Ok(n) => {
                                d.status = Some(Ok(msg(n)));
                                // A new job: from now on the one made (by the name given).
                                let mut folder = None;
                                if let crate::design_ui::Loaded::Job(je) = &d.loaded {
                                    if je.orig.is_none() {
                                        let name = if d.kind == Kind::Oracle { je.cur.name.to_uppercase() } else { je.cur.name.clone() };
                                        folder = Some(d.path.clone());
                                        d.path.push(name.clone());
                                        if let Some(tb) = self.session.workspaces.iter_mut().flat_map(|w| w.tabs.iter_mut()).find(|tb| tb.id == key) {
                                            tb.title = format!("⏱ {name}");
                                            if let Some(o) = tb.object.as_mut() {
                                                o.path = d.path.clone();
                                            }
                                        }
                                    }
                                }
                                if let Some(f) = folder {
                                    self.request_children(&conn, f);
                                }
                                crate::design_ui::reload(self, &ctx, &mut d, &key);
                                let (path, obj) = (d.path.clone(), d.obj);
                                self.load_source(&conn, &path, obj, &key);
                                // The tree's columns and the hints.
                                self.tree.retain(|(c, p), _| !(c == &conn && p.starts_with(&path)));
                                self.refresh_relations(&conn);
                            }
                            Err(e) => d.status = Some(Err(e)),
                        }
                        if let Some(r) = self.rt.get_mut(&key) {
                            r.design = Some(d);
                        }
                    }
                }
                What::Source { path: _, open_as, into: None, text } => match text {
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
                // (Left before its check came: done when it's shown again.)
                if let Some(rt) = self.rt.get_mut(&id) {
                    rt.check_due = None;
                }
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
        let want = (self.settings.theme, self.settings.ui_scale.to_bits(), self.settings.scheme.clone());
        if self.applied.as_ref() == Some(&want) {
            return;
        }
        self.applied = Some(want);
        // A scheme: its own colours, dark or light as it is.
        match crate::themes::find(&self.settings.scheme) {
            Some(s) => {
                let theme = if s.dark { egui::Theme::Dark } else { egui::Theme::Light };
                ctx.set_visuals_of(theme, crate::themes::visuals(s));
                ctx.set_theme(theme);
                let back = Some(egui::Color32::from_rgb(s.back[0], s.back[1], s.back[2]));
                if s.dark { crate::themes::set_back(back, None) } else { crate::themes::set_back(None, back) }
            }
            None => {
                ctx.set_visuals_of(egui::Theme::Dark, egui::Visuals::dark());
                ctx.set_visuals_of(egui::Theme::Light, egui::Visuals::light());
                crate::themes::set_back(None, None);
                ctx.set_theme(match self.settings.theme {
                    ThemePref::System => egui::ThemePreference::System,
                    ThemePref::Dark => egui::ThemePreference::Dark,
                    ThemePref::Light => egui::ThemePreference::Light,
                });
            }
        }
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
        // At most ~120 frames a second when they come back to back (a
        // caret, a running query's clock): without vsync nothing else holds them.
        const FRAME: Duration = Duration::from_micros(8_333);
        if let Some(t) = self.last_frame {
            if let Some(rest) = FRAME.checked_sub(t.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        self.last_frame = Some(Instant::now());
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
        // The backdrop's gradient, under the panels (they leave it bare).
        crate::themes::set_gradient(self.settings.backdrop.on);
        if self.settings.backdrop.on {
            let base = crate::ui::chrome(ui.visuals());
            self.settings.backdrop.paint(ui.painter(), ui.max_rect(), base);
        }
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
                Action::Open { schema, name, call } => self.locate(&ctx, schema, name, call),
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
        crate::ui::hand_cursor(&ctx);
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
