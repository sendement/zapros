//! Databases: each connection talks to its server from threads of its own
//! (one session for scripts, one for the object tree and hints), so a slow
//! network or a dropped link never holds up the window. Requests go in over
//! a channel, answers come back as events tagged with the connection and
//! the worker's generation (a worker given up on is simply forgotten: its
//! late answers are dropped).

pub mod ch;
#[cfg(feature = "oracle")]
pub mod ora;
pub mod pg;
pub mod lite;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Postgres,
    ClickHouse,
    Oracle,
    Sqlite,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Postgres, Kind::ClickHouse, Kind::Oracle, Kind::Sqlite];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Postgres => "PostgreSQL",
            Kind::ClickHouse => "ClickHouse",
            Kind::Oracle => "Oracle",
            Kind::Sqlite => "SQLite",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Kind::Postgres => 5432,
            Kind::ClickHouse => 8123,
            Kind::Oracle => 1521,
            Kind::Sqlite => 0,
        }
    }

    pub fn dialect(self) -> crate::sql::Dialect {
        match self {
            Kind::Postgres => crate::sql::Dialect::Postgres,
            Kind::ClickHouse => crate::sql::Dialect::ClickHouse,
            Kind::Oracle => crate::sql::Dialect::Oracle,
            Kind::Sqlite => crate::sql::Dialect::Sqlite,
        }
    }
}

/// How to reach a database (the password lives in the secret store).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Spec {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    /// PostgreSQL / ClickHouse: database; Oracle: service name (or a full
    /// connect string when host is empty); SQLite: file.
    pub database: String,
    pub user: String,
    pub save_password: bool,
    /// PostgreSQL: TLS (disable / prefer / require); ClickHouse: https.
    pub tls: Tls,
    /// Rows a result takes at most (0: the setting's).
    pub row_limit: usize,
    pub connect_timeout: u64,
    pub read_only: bool,
    pub color: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Tls {
    Disable,
    #[default]
    Prefer,
    Require,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: String::new(),
            kind: Kind::Postgres,
            host: "localhost".into(),
            port: 5432,
            database: String::new(),
            user: String::new(),
            save_password: true,
            tls: Tls::Prefer,
            row_limit: 0,
            connect_timeout: 10,
            read_only: false,
            color: None,
        }
    }
}

impl Spec {
    pub fn title(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.clone();
        }
        match self.kind {
            Kind::Sqlite => std::path::Path::new(&self.database).file_name().map_or("SQLite".into(), |n| n.to_string_lossy().into_owned()),
            _ => format!("{}@{}", self.user, self.host),
        }
    }
}

pub fn new_id() -> String {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).ok();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A column of a result.
#[derive(Clone, Debug, Default)]
pub struct Col {
    pub name: String,
    pub ty: String,
    pub numeric: bool,
}

/// What a statement gave back.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub cols: Vec<Col>,
    pub rows: Vec<Vec<Option<String>>>,
    /// More rows were there than the limit let in.
    pub more: bool,
}

impl Table {
    /// Columns whose every value is a number lean right.
    pub fn guess_numeric(&mut self) {
        for (i, c) in self.cols.iter_mut().enumerate() {
            if c.numeric || !c.ty.is_empty() && !is_numeric_type(&c.ty) {
                continue;
            }
            let mut any = false;
            c.numeric = self.rows.iter().take(200).all(|r| match r.get(i).and_then(|v| v.as_deref()) {
                Some(v) => {
                    any = true;
                    v.parse::<f64>().is_ok()
                }
                None => true,
            }) && any;
        }
    }
}

pub fn is_numeric_type(t: &str) -> bool {
    let t = t.to_lowercase();
    ["int", "float", "double", "numeric", "decimal", "number", "real", "money", "serial", "oid"].iter().any(|n| t.contains(n)) && !t.contains("interval") && !t.contains("point")
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Rows(Table),
    /// Rows changed (or None: the server doesn't say).
    Done(Option<u64>),
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub message: String,
    /// Byte offset in the statement the server points at.
    pub at: Option<usize>,
    /// The link is gone: the session is dropped and made anew next time.
    pub lost: bool,
}

impl Failure {
    pub fn new(m: impl Into<String>) -> Self {
        Self { message: m.into(), at: None, lost: false }
    }
    pub fn lost(m: impl Into<String>) -> Self {
        Self { message: m.into(), at: None, lost: true }
    }
}

pub type Res<T> = Result<T, Failure>;

/// The object tree: a node's path is the names down to it, e.g.
/// [schema, "tables", table, "columns"].
#[derive(Clone, Debug, PartialEq)]
pub struct Obj {
    pub name: String,
    pub kind: ObjKind,
    /// A comment, a type, a row count…
    pub note: String,
    pub leaf: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjKind {
    Database,
    Schema,
    Folder,
    Table,
    View,
    MatView,
    Function,
    Procedure,
    Package,
    Trigger,
    Sequence,
    Index,
    Job,
    Dictionary,
    Type,
    Synonym,
    Column,
    Key,
}

impl ObjKind {
    pub fn icon(self) -> &'static str {
        match self {
            ObjKind::Database => "🗄",
            ObjKind::Schema => "📂",
            ObjKind::Folder => "📁",
            ObjKind::Table => "▦",
            ObjKind::View => "👁",
            ObjKind::MatView => "◈",
            ObjKind::Function => "ƒ",
            ObjKind::Procedure => "⚙",
            ObjKind::Package => "📦",
            ObjKind::Trigger => "⚡",
            ObjKind::Sequence => "#",
            ObjKind::Index => "↯",
            ObjKind::Job => "⏱",
            ObjKind::Dictionary => "📖",
            ObjKind::Type => "T",
            ObjKind::Synonym => "↪",
            ObjKind::Column => "▸",
            ObjKind::Key => "🔑",
        }
    }

    /// Whether it has a definition to open as text.
    pub fn has_source(self) -> bool {
        matches!(
            self,
            ObjKind::Table | ObjKind::View | ObjKind::MatView | ObjKind::Function | ObjKind::Procedure | ObjKind::Package | ObjKind::Trigger | ObjKind::Sequence | ObjKind::Index | ObjKind::Job | ObjKind::Dictionary | ObjKind::Type
        )
    }

    pub fn is_relation(self) -> bool {
        matches!(self, ObjKind::Table | ObjKind::View | ObjKind::MatView | ObjKind::Dictionary)
    }
}

pub fn folder(name: &str) -> Obj {
    Obj { name: name.into(), kind: ObjKind::Folder, note: String::new(), leaf: false }
}

pub fn obj(name: impl Into<String>, kind: ObjKind, note: impl Into<String>, leaf: bool) -> Obj {
    Obj { name: name.into(), kind, note: note.into(), leaf }
}

/// A table (or view) known for hints: its columns.
#[derive(Clone, Debug, Default)]
pub struct Relation {
    pub schema: String,
    pub name: String,
    pub cols: Vec<(String, String)>,
}

/// Talks to one server; lives on its worker's thread.
pub trait Session: Send {
    /// Runs one statement, taking at most `limit` rows.
    fn run(&mut self, sql: &str, limit: usize, notes: &mut dyn FnMut(String)) -> Res<Outcome>;
    /// What's under a node of the tree.
    fn children(&mut self, path: &[String]) -> Res<Vec<Obj>>;
    /// An object's source (CREATE …).
    fn source(&mut self, path: &[String], kind: ObjKind) -> Res<String>;
    /// Tables and their columns of the schemas given (empty: the current).
    fn relations(&mut self, schemas: &[String]) -> Res<Vec<Relation>>;
    /// Stops what `run` is doing, from another thread.
    fn canceller(&self) -> Canceller;
    fn version(&mut self) -> String;
    fn alive(&mut self) -> bool {
        true
    }
}

pub type Canceller = Arc<dyn Fn() + Send + Sync>;

pub fn open(spec: &Spec, password: &str) -> Res<Box<dyn Session>> {
    match spec.kind {
        Kind::Postgres => pg::Pg::open(spec, password).map(|s| Box::new(s) as Box<dyn Session>),
        Kind::ClickHouse => ch::Ch::open(spec, password).map(|s| Box::new(s) as Box<dyn Session>),
        #[cfg(feature = "oracle")]
        Kind::Oracle => ora::Ora::open(spec, password).map(|s| Box::new(s) as Box<dyn Session>),
        #[cfg(not(feature = "oracle"))]
        Kind::Oracle => Err(Failure::new("built without Oracle support")),
        Kind::Sqlite => lite::Lite::open(spec).map(|s| Box::new(s) as Box<dyn Session>),
    }
}

/// What a worker is asked.
pub enum Req {
    Connect,
    /// A script's statements, one after another; stops at the first
    /// failure unless `go_on`.
    Run { run: u64, stmts: Vec<Stmt>, limit: usize, go_on: bool },
    Children { path: Vec<String> },
    Source { path: Vec<String>, kind: ObjKind, open_as: String },
    Relations { schemas: Vec<String> },
    Close,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub sql: String,
    /// Where it starts in the editor's text (bytes).
    pub at: usize,
}

/// What a worker tells.
#[derive(Debug)]
pub struct Event {
    pub conn: String,
    pub generation: u64,
    pub what: What,
}

#[derive(Debug)]
#[allow(dead_code)] // index / path: for debugging output
pub enum What {
    Connecting,
    Connected { version: String },
    ConnectFailed(String),
    Lost(String),
    Started { run: u64, index: usize, at: usize, sql: String },
    Note { run: u64, text: String },
    Result { run: u64, index: usize, at: usize, sql: String, outcome: Result<Outcome, Failure>, took: Duration },
    Finished { run: u64, cancelled: bool },
    Children { path: Vec<String>, items: Result<Vec<Obj>, String> },
    Source { path: Vec<String>, open_as: String, text: Result<String, String> },
    Relations(Result<Vec<Relation>, String>),
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

/// A thread holding one session.
pub struct Worker {
    tx: Sender<Req>,
    pub generation: u64,
    cancel: Arc<Mutex<Option<Canceller>>>,
    busy: Arc<Mutex<Option<Instant>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Worker {
    pub fn spawn(spec: Spec, password: String, events: Sender<Event>, ctx: egui::Context) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
        let cancel: Arc<Mutex<Option<Canceller>>> = Arc::default();
        let busy: Arc<Mutex<Option<Instant>>> = Arc::default();
        let stop: Arc<std::sync::atomic::AtomicBool> = Arc::default();
        let (c2, b2, s2) = (cancel.clone(), busy.clone(), stop.clone());
        std::thread::Builder::new()
            .name(format!("zapros-db-{}", spec.kind.name()))
            .spawn(move || work(spec, password, rx, events, ctx, generation, c2, b2, s2))
            .expect("thread");
        Self { tx, generation, cancel, busy, stop }
    }

    pub fn send(&self, r: Req) {
        self.tx.send(r).ok();
    }

    /// Stops the running statement (when the server lets one stop).
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(c) = self.cancel.lock().unwrap().clone() {
            std::thread::spawn(move || c());
        }
    }

    /// Since when it's been busy.
    pub fn busy_since(&self) -> Option<Instant> {
        *self.busy.lock().unwrap()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx.send(Req::Close).ok();
    }
}

#[allow(clippy::too_many_arguments)]
fn work(spec: Spec, password: String, rx: Receiver<Req>, events: Sender<Event>, ctx: egui::Context, generation: u64, cancel: Arc<Mutex<Option<Canceller>>>, busy: Arc<Mutex<Option<Instant>>>, stop: Arc<std::sync::atomic::AtomicBool>) {
    let conn = spec.id.clone();
    // The saved password, read here: the secret store may take a while.
    let password = if password.is_empty() && spec.save_password && spec.kind != Kind::Sqlite { crate::secrets::get(&spec.id).unwrap_or_default() } else { password };
    let tell = |what: What| {
        events.send(Event { conn: conn.clone(), generation, what }).ok();
        ctx.request_repaint();
    };
    let mut session: Option<Box<dyn Session>> = None;
    // Makes the session when there's none; tells when it can't.
    let ensure = |session: &mut Option<Box<dyn Session>>| -> bool {
        if session.as_mut().is_some_and(|s| s.alive()) {
            return true;
        }
        tell(What::Connecting);
        match open(&spec, &password) {
            Ok(mut s) => {
                *cancel.lock().unwrap() = Some(s.canceller());
                tell(What::Connected { version: s.version() });
                *session = Some(s);
                true
            }
            Err(e) => {
                *session = None;
                tell(What::ConnectFailed(e.message));
                false
            }
        }
    };
    while let Ok(req) = rx.recv() {
        *busy.lock().unwrap() = Some(Instant::now());
        match req {
            Req::Close => break,
            Req::Connect => {
                ensure(&mut session);
            }
            Req::Run { run, stmts, limit, go_on } => {
                let mut cancelled = false;
                stop.store(false, Ordering::Relaxed);
                if ensure(&mut session) {
                    for (index, st) in stmts.into_iter().enumerate() {
                        if stop.load(Ordering::Relaxed) {
                            cancelled = true;
                            break;
                        }
                        tell(What::Started { run, index, at: st.at, sql: st.sql.clone() });
                        let t = Instant::now();
                        let s = session.as_mut().unwrap();
                        let mut note = |text: String| tell(What::Note { run, text });
                        let outcome = s.run(&st.sql, limit, &mut note);
                        let failed = outcome.is_err();
                        if let Err(f) = &outcome {
                            if f.lost {
                                session = None;
                                tell(What::Lost(f.message.clone()));
                            }
                            cancelled = stop.load(Ordering::Relaxed) || is_cancel_message(&f.message);
                        }
                        tell(What::Result { run, index, at: st.at, sql: st.sql, outcome, took: t.elapsed() });
                        if failed && (!go_on || session.is_none() || cancelled) {
                            break;
                        }
                    }
                }
                tell(What::Finished { run, cancelled });
            }
            Req::Children { path } => {
                let items = if ensure(&mut session) { session.as_mut().unwrap().children(&path) } else { Err(Failure::new("no connection")) };
                if items.as_ref().is_err_and(|f| f.lost) {
                    session = None;
                }
                tell(What::Children { path, items: items.map_err(|f| f.message) });
            }
            Req::Source { path, kind, open_as } => {
                let text = if ensure(&mut session) { session.as_mut().unwrap().source(&path, kind) } else { Err(Failure::new("no connection")) };
                tell(What::Source { path, open_as, text: text.map_err(|f| f.message) });
            }
            Req::Relations { schemas } => {
                if ensure(&mut session) {
                    let r = session.as_mut().unwrap().relations(&schemas);
                    tell(What::Relations(r.map_err(|f| f.message)));
                }
            }
        }
        *busy.lock().unwrap() = None;
    }
}

fn is_cancel_message(m: &str) -> bool {
    let m = m.to_lowercase();
    m.contains("cancel") || m.contains("ora-01013") || m.contains("interrupted") || m.contains("отмен")
}

/// Values printed the usual way.
pub fn fmt_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 { format!("{f:.0}") } else { format!("{f}") }
}
#[cfg(test)]
mod tests;
