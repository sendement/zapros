//! Databases: each connection talks to its server from threads of its own
//! (one session for scripts, one for the object tree and hints), so a slow
//! network or a dropped link never holds up the window. Requests go in over
//! a channel, answers come back as events tagged with the connection and
//! the worker's generation (a worker given up on is simply forgotten: its
//! late answers are dropped).

pub mod ch;
pub mod errors;
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

    /// Whether its scripts can be committed or rolled back.
    pub fn transactions(self) -> bool {
        self != Kind::ClickHouse
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
    /// Each statement committed on its own; off: commit / rollback by hand.
    pub auto_commit: bool,
    /// Oracle: NLS_LANG as LANGUAGE_TERRITORY.CHARSET ("" — the default).
    pub nls_lang: String,
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
            auto_commit: false,
            nls_lang: String::new(),
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Obj {
    pub name: String,
    pub kind: ObjKind,
    /// A comment, a type, a row count…
    pub note: String,
    pub leaf: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Relation {
    pub schema: String,
    pub name: String,
    pub cols: Vec<(String, String)>,
    /// Off the search path (another schema / database): written with its
    /// schema when a hint puts it in.
    pub qualify: bool,
    /// Made by the statement itself (a CTE, a subquery): its columns' names
    /// as written there (quotes and all), put in as they are.
    pub written: bool,
}

/// A function or procedure, with its parameters (for hints in its call).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Routine {
    pub schema: String,
    /// Oracle: its package ("" — standalone).
    pub package: String,
    pub name: String,
    pub args: Vec<Arg>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Arg {
    pub name: String,
    pub ty: String,
    /// IN, OUT, IN OUT / INOUT, VARIADIC.
    pub mode: String,
    pub default: bool,
}

/// Marks the relations outside `visible` schemas as needing their schema.
pub fn qualify(mut rels: Vec<Relation>, visible: &[String]) -> Vec<Relation> {
    for r in &mut rels {
        r.qualify = !visible.iter().any(|v| v.eq_ignore_ascii_case(&r.schema));
    }
    rels
}

/// How rows are taken: a batch, then `more` is asked what next (it may
/// take the rows out, hand them on, wait…): how many more, 0 to stop.
pub struct Fetch<'a> {
    want: usize,
    more: Box<dyn FnMut(&mut Table) -> usize + 'a>,
}

impl<'a> Fetch<'a> {
    pub fn new(first: usize, more: impl FnMut(&mut Table) -> usize + 'a) -> Self {
        Self { want: first.max(1), more: Box::new(more) }
    }

    /// At most `n` rows.
    pub fn limit(n: usize) -> Fetch<'static> {
        Fetch::new(n, |_| 0)
    }

    /// Before a row that's there is taken: false — stop here (the table
    /// says there's more).
    pub fn room(&mut self, t: &mut Table) -> bool {
        if t.rows.len() < self.want {
            return true;
        }
        let n = (self.more)(t);
        if n == 0 {
            t.more = true;
            return false;
        }
        self.want = t.rows.len() + n;
        true
    }
}

/// Talks to one server; lives on its worker's thread.
pub trait Session: Send {
    /// Runs one statement, taking rows as `fetch` says.
    fn run(&mut self, sql: &str, fetch: &mut Fetch, notes: &mut dyn FnMut(String)) -> Res<Outcome>;
    /// Runs one statement, taking at most `limit` rows.
    #[allow(dead_code)]
    fn run_limit(&mut self, sql: &str, limit: usize, notes: &mut dyn FnMut(String)) -> Res<Outcome> {
        self.run(sql, &mut Fetch::limit(limit), notes)
    }
    /// What's under a node of the tree.
    fn children(&mut self, path: &[String]) -> Res<Vec<Obj>>;
    /// An object's source (CREATE …).
    fn source(&mut self, path: &[String], kind: ObjKind) -> Res<String>;
    /// A table's columns or a job, for the designers.
    fn describe(&mut self, _path: &[String], _kind: ObjKind) -> Res<crate::design::ObjDef> {
        Err(Failure::new(crate::i18n::t("Конструктор для этого объекта здесь не поддерживается", "No designer for this object here")))
    }
    /// An object named in the code (`schema` None: as the session sees
    /// it): its path in the tree and what it is.
    fn locate(&mut self, _schema: Option<&str>, _name: &str, _call: bool) -> Res<Option<(Vec<String>, ObjKind)>> {
        Ok(None)
    }
    /// Objects whose names hold `q` (any case), every schema the session
    /// sees but the system's.
    fn find_objects(&mut self, _q: &str) -> Res<Vec<Found>> {
        Ok(Vec::new())
    }
    /// Objects whose code holds `q` (views, functions, procedures,
    /// packages, triggers…), with the lines it's on.
    fn search_code(&mut self, _q: &str) -> Res<Vec<Found>> {
        Ok(Vec::new())
    }
    /// Tables and their columns of the schemas given (empty: the current).
    fn relations(&mut self, schemas: &[String]) -> Res<Vec<Relation>>;
    /// Functions and procedures with their parameters (where the
    /// database names them).
    fn routines(&mut self) -> Res<Vec<Routine>> {
        Ok(Vec::new())
    }
    /// Stops what `run` is doing, from another thread.
    fn canceller(&self) -> Canceller;
    fn version(&mut self) -> String;
    fn alive(&mut self) -> bool {
        true
    }
    /// Answers within a few seconds (a session left idle a while may be
    /// gone without a word: a firewall or a VPN dropped it).
    fn ping(&mut self) -> bool {
        true
    }
    /// By-hand transactions: called before each statement (opens the
    /// transaction, a savepoint…) and after it.
    fn tx_before(&mut self, _sql: &str) -> Res<()> {
        Ok(())
    }
    fn tx_after(&mut self, _sql: &str, _ok: bool) {}
    /// Each statement committed on its own, or not.
    fn set_auto(&mut self, _on: bool) {}
    /// The session as the server knows it, for ending it from another
    /// one (PostgreSQL's backend pid, Oracle's "sid,serial#").
    fn server_id(&mut self) -> Option<String> {
        None
    }
    /// Ends the server's session `id` (a hung one given up on).
    fn kill(&mut self, _id: &str) -> Res<()> {
        Err(Failure::new("not supported"))
    }
    /// Commits (or rolls back) the open transaction.
    fn finish(&mut self, _commit: bool) -> Res<()> {
        Err(Failure::new("transactions aren't supported"))
    }
    /// Whether a transaction holds what isn't committed yet.
    fn pending(&mut self) -> bool {
        false
    }
}

/// How a statement touches the transaction it runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxStmt {
    /// Opens one (BEGIN, START TRANSACTION).
    Begin,
    /// Ends it (COMMIT, ROLLBACK, END…).
    End,
    /// Savepoints of its own.
    Savepoint,
    Other,
}

pub fn verb(sql: &str) -> String {
    crate::sql::first_word(sql).to_lowercase()
}

pub fn tx_stmt(sql: &str) -> TxStmt {
    let w = crate::sql::first_words(sql, 2);
    let (a, b) = (w.first().map_or("", |s| s.as_str()), w.get(1).map_or("", |s| s.as_str()));
    match a {
        "begin" | "start" => TxStmt::Begin,
        "commit" | "end" | "abort" => TxStmt::End,
        "rollback" if b == "to" => TxStmt::Savepoint,
        "rollback" => TxStmt::End,
        "prepare" if b == "transaction" => TxStmt::End,
        "savepoint" | "release" => TxStmt::Savepoint,
        _ => TxStmt::Other,
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

/// An object found by a search: where it is in the tree, and (a code
/// search) the lines that hold what was looked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub path: Vec<String>,
    pub kind: ObjKind,
    pub schema: String,
    pub name: String,
    /// (Line number from 1, its text.)
    pub lines: Vec<(usize, String)>,
}

/// The lines of `text` holding `q` (any case): at most `n`, with their
/// numbers.
pub fn lines_with(text: &str, q: &str, n: usize) -> Vec<(usize, String)> {
    let ql = q.to_lowercase();
    text.lines().enumerate().filter(|(_, l)| l.to_lowercase().contains(&ql)).take(n).map(|(i, l)| (i + 1, around(l.trim(), q))).collect()
}

/// A long line cut to a stretch round the first `q` in it (… at the cuts).
pub fn around(line: &str, q: &str) -> String {
    around_n(line, q, 90)
}

/// As [`around`], at most `max` chars (the match about a third in).
pub fn around_n(line: &str, q: &str, max: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= max {
        return line.to_string();
    }
    let low: String = line.to_lowercase();
    // (Char index of the match; lower case may shift bytes: count chars.)
    let at = low.find(&q.to_lowercase()).map_or(0, |b| low[..b].chars().count());
    let before = max / 3;
    let from = at.saturating_sub(before).min(chars.len().saturating_sub(max));
    let to = (from + max).min(chars.len());
    let mut s = String::new();
    if from > 0 {
        s.push('…');
    }
    s.extend(&chars[from..to]);
    if to < chars.len() {
        s.push('…');
    }
    s
}

/// A tree folder and kind for a kind's name as the databases call it.
pub fn folder_of(kind: &str) -> Option<(&'static str, ObjKind)> {
    Some(match kind.to_ascii_uppercase().as_str() {
        "TABLE" | "R" | "P" | "F" | "BASE TABLE" => ("tables", ObjKind::Table),
        "VIEW" | "V" => ("views", ObjKind::View),
        "MATERIALIZED VIEW" | "M" | "MATERIALIZEDVIEW" => ("matviews", ObjKind::MatView),
        "FUNCTION" => ("functions", ObjKind::Function),
        "PROCEDURE" => ("procedures", ObjKind::Procedure),
        "PACKAGE" | "PACKAGE BODY" => ("packages", ObjKind::Package),
        "SEQUENCE" | "S" => ("sequences", ObjKind::Sequence),
        "TRIGGER" => ("triggers", ObjKind::Trigger),
        "TYPE" | "TYPE BODY" => ("types", ObjKind::Type),
        "DICTIONARY" => ("dictionaries", ObjKind::Dictionary),
        "JOB" => ("jobs", ObjKind::Job),
        "INDEX" => ("indexes", ObjKind::Index),
        _ => return None,
    })
}

/// What a worker is asked.
pub enum Req {
    Connect,
    /// A script's statements, one after another; stops at the first
    /// failure unless `go_on`.
    /// `limit`: rows in a batch; the last statement's cursor stays open
    /// after it, for `More` / `StopFetch`.
    Run { run: u64, stmts: Vec<Stmt>, limit: usize, go_on: bool },
    /// A script with directives (`crate::script`): its statements run as
    /// its loops and conditions say. `scope`: whose variables it shares
    /// (a notebook's cells), kept here between runs.
    Script { run: u64, prog: Box<crate::script::Program>, vars: Box<crate::script::Vars>, scope: Option<String>, limit: usize, go_on: bool },
    /// The open cursor: `n` more rows (`all`: batch after batch to the end).
    More { run: u64, n: usize, all: bool },
    /// The open cursor's rest written to a file as it comes (none of it
    /// sent back: a result of any size in the memory of one batch).
    Drain { run: u64, sink: Box<crate::export::Sink> },
    /// The open cursor closes.
    StopFetch,
    Children { path: Vec<String> },
    /// `into`: the tab it goes to (else a new one).
    Source { path: Vec<String>, kind: ObjKind, open_as: String, into: Option<String> },
    /// For a designer (`key`: whose).
    Describe { key: String, path: Vec<String>, kind: ObjKind },
    /// Where a name in the code leads (Ctrl+click).
    Locate { schema: Option<String>, name: String, call: bool },
    /// Objects by name (`code`: false) or by what their code holds.
    Search { q: String, code: bool },
    /// A designer's statements, one after another (stops at a failure).
    Apply { key: String, stmts: Vec<String> },
    /// A table made from a file: `make` (CREATE, indexes), then `rows`
    /// (INSERTs), progress told; failed or stopped, `undo` (DROP) runs.
    Import { key: String, make: Vec<String>, rows: Vec<String>, undo: String },
    Relations { schemas: Vec<String> },
    Tx(TxOp),
    /// The server's session `id` ended (from the other session).
    Kill { id: String },
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxOp {
    Commit,
    Rollback,
    /// Auto-commit on / off.
    Auto(bool),
}

#[derive(Clone, Debug, PartialEq)]
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
    /// A hung session ended on the server (or why not).
    Killed(Result<(), String>),
    ConnectFailed(String),
    Lost(String),
    Started { run: u64, index: usize, at: usize, sql: String },
    Note { run: u64, text: String },
    Result { run: u64, index: usize, at: usize, sql: String, outcome: Result<Outcome, Failure>, took: Duration },
    Finished { run: u64, cancelled: bool },
    /// Where a script's got to ("7/16 · i = 06").
    Progress { run: u64, text: String },
    /// A script's directive went wrong (at its line).
    ScriptError { run: u64, line: usize, msg: String },
    /// The first batch of a result whose cursor stays open (the run counts
    /// as finished after it).
    Page { run: u64, at: usize, sql: String, table: Table, took: Duration },
    /// More rows of it; `open`: the cursor's still there (more can come);
    /// `more`: rows were left when it closed.
    Rows { run: u64, rows: Vec<Vec<Option<String>>>, more: bool, open: bool, error: Option<String> },
    /// Rows written to the file so far.
    Exporting { run: u64, rows: usize },
    /// The file's done (or why not).
    Exported { run: u64, msg: Result<String, String> },
    Children { path: Vec<String>, items: Result<Vec<Obj>, String> },
    Source { path: Vec<String>, open_as: String, into: Option<String>, text: Result<String, String> },
    Described { key: String, def: Result<crate::design::ObjDef, String> },
    Located { name: String, found: Result<Option<(Vec<String>, ObjKind)>, String> },
    Searched { q: String, code: bool, found: Result<Vec<Found>, String> },
    /// How many ran, or the failure (with the statement).
    Applied { key: String, result: Result<usize, String> },
    /// An import's INSERTs done so far, of all.
    ImportStep { key: String, done: usize, total: usize },
    /// It ended: rows' statements run, or why not (`stopped`: asked to).
    Imported { key: String, result: Result<usize, String>, stopped: bool },
    Relations(Result<Vec<Relation>, String>),
    Routines(Vec<Routine>),
    /// Where the transaction stands (after a run or a commit).
    Tx { auto: bool, pending: bool },
    TxDone { op: TxOp, result: Result<(), String> },
}

/// Idle longer than this, a session is pinged before it's used.
const IDLE_CHECK: Duration = if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(60) };
/// How long a ping may take.
pub const PING_WAIT: Duration = Duration::from_secs(10);

static GENERATION: AtomicU64 = AtomicU64::new(1);

/// A thread holding one session.
pub struct Worker {
    tx: Sender<Req>,
    pub generation: u64,
    cancel: Arc<Mutex<Option<Canceller>>>,
    busy: Arc<Mutex<Option<Instant>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// Its session as the server knows it.
    server: Arc<Mutex<Option<String>>>,
    /// Given up on: once free, it runs nothing more and closes.
    gone: Arc<std::sync::atomic::AtomicBool>,
    /// The run it's on (0: none); runs stopped while they waited their
    /// turn, skipped when it comes. (Several tabs and notebooks share the
    /// session: a stop is for one run, not for whatever runs at the time.)
    current: Arc<Mutex<u64>>,
    dropped: Arc<Mutex<std::collections::HashSet<u64>>>,
    /// What waits in its queue (for a worker taking over from it).
    queue: Receiver<Req>,
}

impl Worker {
    pub fn spawn(spec: Spec, password: String, events: Sender<Event>, ctx: egui::Context) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
        let cancel: Arc<Mutex<Option<Canceller>>> = Arc::default();
        let busy: Arc<Mutex<Option<Instant>>> = Arc::default();
        let stop: Arc<std::sync::atomic::AtomicBool> = Arc::default();
        let server: Arc<Mutex<Option<String>>> = Arc::default();
        let gone: Arc<std::sync::atomic::AtomicBool> = Arc::default();
        let current: Arc<Mutex<u64>> = Arc::default();
        let dropped: Arc<Mutex<std::collections::HashSet<u64>>> = Arc::default();
        let w = Worker { tx, generation, cancel: cancel.clone(), busy: busy.clone(), stop: stop.clone(), server: server.clone(), gone: gone.clone(), current: current.clone(), dropped: dropped.clone(), queue: rx.clone() };
        std::thread::Builder::new()
            .name(format!("zapros-db-{}", spec.kind.name()))
            .spawn(move || work(spec, password, rx, events, ctx, generation, Shared { cancel, busy, stop, server, gone, current, dropped }))
            .expect("thread");
        w
    }

    /// Its session as the server knows it (when the database tells).
    pub fn server_id(&self) -> Option<String> {
        self.server.lock().unwrap().clone()
    }

    /// Given up on: what's queued is dropped, the session closed once the
    /// statement that hangs returns (if ever).
    pub fn abandon(&self) {
        self.gone.store(true, Ordering::Relaxed);
        self.cancel();
    }

    pub fn send(&self, r: Req) {
        self.tx.send(r).ok();
    }

    /// What waited in `old`'s queue goes on here (the runs of other tabs
    /// survive one being ended by force).
    pub fn take_over(&self, old: &Worker) {
        self.dropped.lock().unwrap().extend(old.dropped.lock().unwrap().iter().copied());
        for r in old.queue.try_iter() {
            self.tx.send(r).ok();
        }
    }

    /// The run it's on now (0: none).
    pub fn current(&self) -> u64 {
        *self.current.lock().unwrap()
    }

    /// Stops `run`: its statement, when it's the one running (true), or
    /// else its turn, skipped when it comes (false: it's over already).
    pub fn cancel_run(&self, run: u64) -> bool {
        let cur = self.current.lock().unwrap();
        if *cur != run {
            self.dropped.lock().unwrap().insert(run);
            return false;
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(c) = self.cancel.lock().unwrap().clone() {
            std::thread::spawn(move || c());
        }
        true
    }

    /// Stops the running statement (when the server lets one stop),
    /// whoever's it is.
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

/// Where a script's statements run: the worker's session.
struct ScriptHost<'a> {
    session: &'a mut Option<Box<dyn Session>>,
    tell: &'a dyn Fn(What),
    run: u64,
    limit: usize,
    auto: &'a std::cell::Cell<bool>,
    stop: &'a std::sync::atomic::AtomicBool,
    index: usize,
    cancelled: bool,
}

impl ScriptHost<'_> {
    /// A statement run in the session (`fetch`: how its rows come).
    fn exec(&mut self, sql: &str, fetch: &mut Fetch) -> Result<Outcome, Failure> {
        let Some(s) = self.session.as_mut() else { return Err(Failure::new("no connection")) };
        let (run, tell) = (self.run, self.tell);
        let mut note = |text: String| tell(What::Note { run, text });
        let outcome = match if self.auto.get() { Ok(()) } else { s.tx_before(sql) } {
            Ok(()) => s.run(sql, fetch, &mut note),
            Err(e) => Err(e),
        };
        if !self.auto.get() && !outcome.as_ref().is_err_and(|f| f.lost) {
            s.tx_after(sql, outcome.is_ok());
        }
        if let Err(f) = &outcome {
            if f.lost {
                *self.session = None;
                (self.tell)(What::Lost(f.message.clone()));
            }
            if self.stop.load(Ordering::Relaxed) || is_cancel_message(&f.message) {
                self.cancelled = true;
            }
        }
        outcome
    }
}

impl crate::script::Host for ScriptHost<'_> {
    fn sql(&mut self, st: Stmt) -> crate::script::SqlEnd {
        use crate::script::SqlEnd;
        if self.session.is_none() {
            return SqlEnd::Over;
        }
        let index = self.index;
        self.index += 1;
        (self.tell)(What::Started { run: self.run, index, at: st.at, sql: st.sql.clone() });
        let t = Instant::now();
        let outcome = self.exec(&st.sql, &mut Fetch::limit(self.limit));
        let end = match &outcome {
            Ok(_) => SqlEnd::Ok,
            Err(f) if f.lost || self.cancelled => SqlEnd::Over,
            Err(_) => SqlEnd::Failed,
        };
        (self.tell)(What::Result { run: self.run, index, at: st.at, sql: st.sql, outcome, took: t.elapsed() });
        end
    }

    fn query(&mut self, sql: &str) -> Result<Table, String> {
        // The whole answer (up to a million rows).
        let mut fetch = Fetch::new(10_000, |t: &mut Table| if t.rows.len() >= 1_000_000 { 0 } else { 10_000 });
        match self.exec(sql, &mut fetch) {
            Ok(Outcome::Rows(t)) => Ok(t),
            Ok(Outcome::Done(_)) => Ok(Table::default()),
            Err(f) => Err(f.message),
        }
    }

    fn print(&mut self, text: String) {
        (self.tell)(What::Note { run: self.run, text });
    }

    fn progress(&mut self, text: String) {
        (self.tell)(What::Progress { run: self.run, text });
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.session.is_none()
    }
}

/// What a worker's thread shares with its handle.
struct Shared {
    cancel: Arc<Mutex<Option<Canceller>>>,
    busy: Arc<Mutex<Option<Instant>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    server: Arc<Mutex<Option<String>>>,
    gone: Arc<std::sync::atomic::AtomicBool>,
    current: Arc<Mutex<u64>>,
    dropped: Arc<Mutex<std::collections::HashSet<u64>>>,
}

fn work(spec: Spec, password: String, rx: Receiver<Req>, events: Sender<Event>, ctx: egui::Context, generation: u64, sh: Shared) {
    let Shared { cancel, busy, stop, server, gone, current, dropped } = sh;
    let conn = spec.id.clone();
    // The saved password, read here: the secret store may take a while.
    let password = if password.is_empty() && spec.save_password && spec.kind != Kind::Sqlite { crate::secrets::get(&spec.id).unwrap_or_default() } else { password };
    let tell = |what: What| {
        events.send(Event { conn: conn.clone(), generation, what }).ok();
        ctx.request_repaint();
    };
    let mut session: Option<Box<dyn Session>> = None;
    let auto = std::cell::Cell::new(spec.auto_commit || !spec.kind.transactions());
    // When the session was last used: idle a while, it's asked first.
    let used = std::cell::Cell::new(Instant::now());
    // Makes the session when there's none; tells when it can't.
    let ensure = |session: &mut Option<Box<dyn Session>>| -> bool {
        if let Some(s) = session.as_mut()
            && s.alive()
        {
            let idle = used.replace(Instant::now()).elapsed();
            if idle < IDLE_CHECK || s.ping() {
                return true;
            }
            // Dropped while idle: made anew (an open transaction is lost).
            if !auto.get() {
                tell(What::Lost(crate::i18n::t("сессия оборвалась, пока простаивала", "the session dropped while idle").into()));
            }
            // (Let go aside: closing it may wait on the dead link too.)
            if let Some(dead) = session.take() {
                std::thread::spawn(move || drop(dead));
            }
        }
        tell(What::Connecting);
        match open(&spec, &password) {
            Ok(mut s) => {
                s.set_auto(auto.get());
                *cancel.lock().unwrap() = Some(s.canceller());
                *server.lock().unwrap() = s.server_id();
                tell(What::Connected { version: s.version() });
                *session = Some(s);
                used.set(Instant::now());
                true
            }
            Err(e) => {
                *session = None;
                tell(What::ConnectFailed(e.message));
                false
            }
        }
    };
    // Scripts' variables by whose they are (a notebook's), between runs.
    let mut scopes: std::collections::HashMap<String, crate::script::Vars> = std::collections::HashMap::new();
    // A request that came while a cursor was open: next.
    let mut stash: Option<Req> = None;
    loop {
        let Some(req) = stash.take().or_else(|| rx.recv().ok()) else { break };
        if gone.load(Ordering::Relaxed) {
            break;
        }
        // Its turn: on, unless it was stopped while it waited.
        if let Req::Run { run, .. } | Req::Script { run, .. } = &req {
            let mut cur = current.lock().unwrap();
            if dropped.lock().unwrap().remove(run) {
                continue;
            }
            *cur = *run;
            stop.store(false, Ordering::Relaxed);
        }
        *busy.lock().unwrap() = Some(Instant::now());
        match req {
            Req::Close => break,
            Req::Kill { id } => {
                let r = if ensure(&mut session) { session.as_mut().unwrap().kill(&id).map_err(|f| f.message) } else { Err("no connection".into()) };
                tell(What::Killed(r));
            }
            Req::Connect => {
                ensure(&mut session);
            }
            Req::Run { run, stmts, limit, go_on } => {
                let mut cancelled = false;
                let mut paged = false;
                if ensure(&mut session) {
                    let last = stmts.len().saturating_sub(1);
                    for (index, st) in stmts.into_iter().enumerate() {
                        if stop.load(Ordering::Relaxed) {
                            cancelled = true;
                            break;
                        }
                        tell(What::Started { run, index, at: st.at, sql: st.sql.clone() });
                        let t = Instant::now();
                        let s = session.as_mut().unwrap();
                        let mut note = |text: String| tell(What::Note { run, text });
                        // The last statement's cursor stays open after a
                        // batch: the window asks for more (or anything
                        // else ends it).
                        let mut all = false;
                        // Draining to a file: where to, a failure, stopped.
                        let mut sink: Option<Box<crate::export::Sink>> = None;
                        let mut sink_err: Option<String> = None;
                        let mut sink_stopped = false;
                        let (at, sql) = (st.at, st.sql.clone());
                        let mut more = |tb: &mut Table| -> usize {
                            if index != last || stash.is_some() {
                                return 0;
                            }
                            if !paged {
                                paged = true;
                                for c in tb.cols.iter_mut() {
                                    c.numeric = c.numeric || is_numeric_type(&c.ty);
                                }
                                tb.guess_numeric();
                                let first = Table { cols: tb.cols.clone(), rows: std::mem::take(&mut tb.rows), more: true };
                                tell(What::Page { run, at, sql: sql.clone(), table: first, took: t.elapsed() });
                                tell(What::Finished { run, cancelled: false });
                            } else if let Some(sk) = sink.as_mut() {
                                if let Err(e) = sk.push(&tb.rows) {
                                    sink_err = Some(e);
                                    return 0;
                                }
                                tb.rows.clear();
                                tell(What::Exporting { run, rows: sk.rows() });
                            } else {
                                tell(What::Rows { run, rows: std::mem::take(&mut tb.rows), more: true, open: true, error: None });
                            }
                            if all {
                                // Batch after batch, unless told otherwise.
                                match rx.try_recv() {
                                    Err(_) => return limit.max(5000),
                                    Ok(Req::StopFetch) | Ok(Req::More { .. }) => sink_stopped = true,
                                    Ok(other) => {
                                        sink_stopped = true;
                                        stash = Some(other);
                                    }
                                }
                                return 0;
                            }
                            *busy.lock().unwrap() = None;
                            let n = loop {
                                match rx.recv() {
                                    Ok(Req::More { run: r, n, all: a }) if r == run => {
                                        all = a;
                                        break if a { limit.max(5000) } else { n.max(1) };
                                    }
                                    Ok(Req::More { .. }) => continue,
                                    Ok(Req::Drain { run: r, sink: s }) if r == run => {
                                        sink = Some(s);
                                        all = true;
                                        break limit.max(5000);
                                    }
                                    Ok(Req::StopFetch) => break 0,
                                    Ok(other) => {
                                        stash = Some(other);
                                        break 0;
                                    }
                                    Err(_) => {
                                        stash = Some(Req::Close);
                                        break 0;
                                    }
                                }
                            };
                            *busy.lock().unwrap() = Some(Instant::now());
                            n
                        };
                        let mut fetch = Fetch::new(limit, &mut more);
                        let outcome = match if auto.get() { Ok(()) } else { s.tx_before(&st.sql) } {
                            Ok(()) => s.run(&st.sql, &mut fetch, &mut note),
                            Err(e) => Err(e),
                        };
                        drop(fetch);
                        let failed = outcome.is_err();
                        if !auto.get() && !outcome.as_ref().is_err_and(|f| f.lost) {
                            s.tx_after(&st.sql, !failed);
                        }
                        if let Err(f) = &outcome {
                            if f.lost {
                                session = None;
                                tell(What::Lost(f.message.clone()));
                            }
                            cancelled = stop.load(Ordering::Relaxed) || is_cancel_message(&f.message);
                        }
                        if let (true, Some(mut sk)) = (paged, sink.take()) {
                            // Drained to a file: its last rows, the file closed.
                            let (rows, err) = match outcome {
                                Ok(Outcome::Rows(tb)) => (tb.rows, None),
                                Ok(_) => (Vec::new(), None),
                                Err(f) => (Vec::new(), Some(f.message)),
                            };
                            let pushed = match sink_err.take() {
                                Some(e) => Err(e),
                                None => sk.push(&rows),
                            };
                            let msg = pushed.and_then(|_| sk.finish()).map(|m| match (&err, sink_stopped) {
                                (Some(e), _) => format!("{m} — {}: {e}", crate::i18n::t("прервано ошибкой", "cut short by an error")),
                                (None, true) => format!("{m} — {}", crate::i18n::t("остановлено, записано не всё", "stopped, not all written")),
                                _ => m,
                            });
                            tell(What::Exported { run, msg });
                            tell(What::Rows { run, rows: Vec::new(), more: err.is_some() || sink_stopped, open: false, error: err });
                            break;
                        }
                        if paged {
                            // The rest of a paged result, and its cursor's end.
                            match outcome {
                                Ok(Outcome::Rows(tb)) => tell(What::Rows { run, rows: tb.rows, more: tb.more, open: false, error: None }),
                                Ok(_) => tell(What::Rows { run, rows: Vec::new(), more: false, open: false, error: None }),
                                Err(f) => tell(What::Rows { run, rows: Vec::new(), more: true, open: false, error: Some(f.message) }),
                            }
                            break;
                        }
                        tell(What::Result { run, index, at: st.at, sql: st.sql, outcome, took: t.elapsed() });
                        if failed && (!go_on || session.is_none() || cancelled) {
                            break;
                        }
                    }
                }
                if !auto.get() && let Some(s) = session.as_mut() {
                    tell(What::Tx { auto: false, pending: s.pending() });
                }
                if !paged {
                    tell(What::Finished { run, cancelled });
                }
                *current.lock().unwrap() = 0;
            }
            Req::Script { run, prog, vars, scope, limit, go_on } => {
                let mut vars = *vars;
                if let Some(saved) = scope.as_ref().and_then(|s| scopes.get(s)) {
                    for (k, v) in &saved.globals {
                        vars.globals.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                let mut cancelled = false;
                if ensure(&mut session) {
                    let mut host = ScriptHost { session: &mut session, tell: &tell, run, limit, auto: &auto, stop: &stop, index: 0, cancelled: false };
                    let end = crate::script::run(&prog, &mut vars, spec.kind.dialect(), go_on, &mut host);
                    cancelled = host.cancelled || stop.load(Ordering::Relaxed);
                    if let Err(m) = end {
                        tell(What::ScriptError { run, line: m.line, msg: m.msg });
                    }
                }
                if let Some(s) = scope {
                    scopes.insert(s, vars);
                }
                if !auto.get() && let Some(s) = session.as_mut() {
                    tell(What::Tx { auto: false, pending: s.pending() });
                }
                tell(What::Finished { run, cancelled });
                *current.lock().unwrap() = 0;
            }
            // (No cursor open any more — the session was made anew: what's
            // loaded stays, the rest can't come.)
            Req::More { run, .. } => tell(What::Rows { run, rows: Vec::new(), more: true, open: false, error: None }),
            Req::StopFetch => {}
            // (No cursor open any more: what's in it so far, saved.)
            Req::Drain { run, sink } => {
                tell(What::Exported { run, msg: sink.finish() });
                tell(What::Rows { run, rows: Vec::new(), more: true, open: false, error: None });
            }
            Req::Tx(op) => {
                let result = match op {
                    TxOp::Auto(on) => {
                        auto.set(on);
                        if let Some(s) = session.as_mut() {
                            s.set_auto(on);
                        }
                        Ok(())
                    }
                    _ => match session.as_mut() {
                        Some(s) => s.finish(op == TxOp::Commit),
                        // No session: nothing was open.
                        None => Ok(()),
                    },
                };
                if result.as_ref().is_err_and(|f| f.lost) {
                    session = None;
                }
                let pending = session.as_mut().is_some_and(|s| s.pending());
                tell(What::TxDone { op, result: result.map_err(|f| f.message) });
                tell(What::Tx { auto: auto.get(), pending });
            }
            Req::Children { path } => {
                let items = if ensure(&mut session) { session.as_mut().unwrap().children(&path) } else { Err(Failure::new("no connection")) };
                if items.as_ref().is_err_and(|f| f.lost) {
                    session = None;
                }
                tell(What::Children { path, items: items.map_err(|f| f.message) });
            }
            Req::Source { path, kind, open_as, into } => {
                let text = if ensure(&mut session) { session.as_mut().unwrap().source(&path, kind) } else { Err(Failure::new("no connection")) };
                tell(What::Source { path, open_as, into, text: text.map_err(|f| f.message) });
            }
            Req::Describe { key, path, kind } => {
                let def = if ensure(&mut session) { session.as_mut().unwrap().describe(&path, kind) } else { Err(Failure::new("no connection")) };
                if def.as_ref().is_err_and(|f| f.lost) {
                    session = None;
                }
                tell(What::Described { key, def: def.map_err(|f| f.message) });
            }
            Req::Search { q, code } => {
                let found = if ensure(&mut session) {
                    let s = session.as_mut().unwrap();
                    if code { s.search_code(&q) } else { s.find_objects(&q) }
                } else {
                    Err(Failure::new("no connection"))
                };
                tell(What::Searched { q, code, found: found.map_err(|f| f.message) });
            }
            Req::Locate { schema, name, call } => {
                let found = if ensure(&mut session) { session.as_mut().unwrap().locate(schema.as_deref(), &name, call) } else { Err(Failure::new("no connection")) };
                tell(What::Located { name, found: found.map_err(|f| f.message) });
            }
            Req::Apply { key, stmts } => {
                let result = if ensure(&mut session) {
                    let s = session.as_mut().unwrap();
                    let mut done = 0;
                    let mut err = None;
                    for sql in &stmts {
                        match s.run(sql, &mut Fetch::limit(0), &mut |_| {}) {
                            Ok(_) => done += 1,
                            Err(f) => {
                                if f.lost {
                                    session = None;
                                }
                                err = Some(format!("{}\n\n{}", f.message, crate::app::head(sql, 300)));
                                break;
                            }
                        }
                    }
                    // (Its own session: nothing else is in its transaction.)
                    if spec.kind.transactions() && !auto.get() && let Some(s) = session.as_mut() {
                        s.finish(err.is_none()).ok();
                    }
                    match err {
                        Some(e) => Err(e),
                        None => Ok(done),
                    }
                } else {
                    Err("no connection".to_string())
                };
                tell(What::Applied { key, result });
            }
            Req::Import { key, make, rows, undo } => {
                stop.store(false, Ordering::Relaxed);
                let total = rows.len();
                let mut stopped = false;
                let result = if ensure(&mut session) {
                    let s = session.as_mut().unwrap();
                    let mut err = None;
                    let mut made = false;
                    for (k, sql) in make.iter().chain(rows.iter()).enumerate() {
                        if stop.load(Ordering::Relaxed) {
                            stopped = true;
                            break;
                        }
                        match s.run(sql, &mut Fetch::limit(0), &mut |_| {}) {
                            Ok(_) => {
                                made |= k == 0;
                                if k >= make.len() {
                                    tell(What::ImportStep { key: key.clone(), done: k + 1 - make.len(), total });
                                }
                            }
                            Err(f) => {
                                stopped = stop.load(Ordering::Relaxed);
                                err = Some(format!("{}\n\n{}", f.message, crate::app::head(sql, 300)));
                                break;
                            }
                        }
                    }
                    let failed = err.is_some() || stopped;
                    if spec.kind.transactions() && !auto.get() {
                        s.finish(!failed).ok();
                    }
                    // Not all of it: none of it.
                    if failed && made {
                        s.run(&undo, &mut Fetch::limit(0), &mut |_| {}).ok();
                        if spec.kind.transactions() && !auto.get() {
                            s.finish(true).ok();
                        }
                    }
                    match err {
                        Some(e) if !stopped => Err(e),
                        _ if stopped => Err(String::new()),
                        _ => Ok(total),
                    }
                } else {
                    Err("no connection".to_string())
                };
                tell(What::Imported { key, result, stopped });
            }
            Req::Relations { schemas } => {
                if ensure(&mut session) {
                    let r = session.as_mut().unwrap().relations(&schemas);
                    tell(What::Relations(r.map_err(|f| f.message)));
                    // (Not there: no hints for calls, nothing more.)
                    if let Ok(r) = session.as_mut().unwrap().routines() {
                        tell(What::Routines(r));
                    }
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

/// NLS_LANG taken apart: LANGUAGE_TERRITORY.CHARSET, each part optional.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nls {
    pub language: String,
    pub territory: String,
    pub charset: String,
}

impl Nls {
    /// None: empty (the default) or not of that shape.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_matches('"').trim();
        if s.is_empty() {
            return None;
        }
        let (lt, charset) = match s.split_once('.') {
            Some((a, b)) => (a, b.trim()),
            None => (s, ""),
        };
        let (language, territory) = match lt.split_once('_') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (lt.trim(), ""),
        };
        let ok = |p: &str| p.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-');
        if !(ok(language) && ok(territory) && charset.chars().all(|c| c.is_ascii_alphanumeric())) || (language.is_empty() && territory.is_empty() && charset.is_empty()) {
            return None;
        }
        Some(Nls { language: language.to_uppercase(), territory: territory.to_uppercase(), charset: charset.to_uppercase() })
    }

    /// What the session's set with.
    pub fn session(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.language.is_empty() {
            out.push(format!("ALTER SESSION SET NLS_LANGUAGE = '{}'", self.language));
        }
        if !self.territory.is_empty() {
            out.push(format!("ALTER SESSION SET NLS_TERRITORY = '{}'", self.territory));
        }
        out
    }

    /// The charset's UTF-8 (or not said).
    pub fn utf8(&self) -> bool {
        self.charset.is_empty() || matches!(self.charset.as_str(), "AL32UTF8" | "UTF8")
    }
}
