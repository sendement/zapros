//! Several windows (processes) of zapros at once: they share the session
//! file. Each writes its changes there (under a lock, its own merged over
//! what the others wrote since) and picks up the others' within a moment.
//! A tab being typed in is held by its window: the others show it
//! read-only, its text following, until it's let go (a few seconds idle)
//! or taken over.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Session, Tab, Workspace};

/// A tab's hold by a window: until when (ms since 1970).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    pub owner: String,
    pub until: u64,
}

/// The session file as written: the session, a revision counted up by
/// every write, and the holds.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct OnDisk {
    #[serde(flatten)]
    session: Session,
    rev: u64,
    writer: String,
    leases: BTreeMap<String, Lease>,
}

/// How long typing holds a tab after the last key.
pub const HOLD: Duration = Duration::from_secs(5);
/// A window counts as alive this long after its last sign.
const ALIVE: Duration = Duration::from_secs(8);

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub struct Sync {
    pub me: String,
    path: PathBuf,
    /// The session as last written or read: what both sides started from.
    base: Session,
    rev: u64,
    seen: Option<SystemTime>,
    /// Every window's holds, as the file has them (ours put in on write).
    pub leases: BTreeMap<String, Lease>,
    /// Ours: tab → until.
    mine: BTreeMap<String, u64>,
    /// Taken over from another window: ours whatever the file says.
    taken: std::collections::BTreeSet<String>,
    beat: Instant,
    /// Other windows alive (seen by their signs).
    pub others: usize,
    /// Tabs both sides changed: theirs kept as copies (titles, for a note).
    pub conflicts: Vec<String>,
}

fn same<T: Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

fn value<T: Serialize>(a: &T) -> Value {
    serde_json::to_value(a).unwrap_or(Value::Null)
}

impl Sync {
    /// The session from the file, and the sync state around it.
    pub fn open(path: &Path) -> (Session, Sync) {
        let disk: OnDisk = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let session = if disk.session.workspaces.is_empty() { Session::default() } else { disk.session };
        let seen = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let me = crate::db::new_id();
        let s = Sync {
            me,
            path: path.to_path_buf(),
            base: session.clone(),
            rev: disk.rev,
            seen,
            leases: disk.leases,
            mine: BTreeMap::new(),
            taken: Default::default(),
            beat: Instant::now() - Duration::from_secs(60),
            others: 0,
            conflicts: Vec::new(),
        };
        (session, s)
    }

    fn lock(&self) -> Option<std::fs::File> {
        std::fs::create_dir_all(self.path.parent()?).ok()?;
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.path.with_extension("lock")).ok()?;
        f.lock().ok()?;
        Some(f)
    }

    fn read(&self) -> Option<OnDisk> {
        serde_json::from_str(&std::fs::read_to_string(&self.path).ok()?).ok()
    }

    /// Another window wrote since: true when the file's changed (cheap: its
    /// time only).
    pub fn changed(&self) -> bool {
        std::fs::metadata(&self.path).and_then(|m| m.modified()).ok() != self.seen
    }

    /// Takes in what the other windows wrote since (if anything): merged
    /// into `ours`. True when `ours` changed.
    pub fn pull(&mut self, ours: &mut Session) -> bool {
        if !self.changed() {
            return false;
        }
        self.seen = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        let Some(disk) = self.read() else { return false };
        self.leases = disk.leases;
        self.yield_taken();
        if disk.rev == self.rev {
            return false;
        }
        self.rev = disk.rev;
        let before = value(ours);
        let merged = merge(&self.base, ours, &disk.session, &mut self.conflicts);
        self.base = disk.session;
        *ours = merged;
        value(ours) != before
    }

    /// Writes `ours` (the others' changes since merged in first).
    pub fn push(&mut self, ours: &mut Session) -> std::io::Result<()> {
        let _lock = self.lock();
        let disk = self.read().unwrap_or_default();
        if disk.rev != self.rev && !disk.session.workspaces.is_empty() {
            *ours = merge(&self.base, ours, &disk.session, &mut self.conflicts);
        }
        let now = now_ms();
        let taken = std::mem::take(&mut self.taken);
        let mut leases: BTreeMap<String, Lease> = disk.leases.into_iter().filter(|(k, l)| l.until > now && l.owner != self.me && !taken.contains(k)).collect();
        // Held by another window now (taken over): let go.
        self.mine.retain(|tab, until| *until > now && !leases.contains_key(tab));
        for (tab, until) in &self.mine {
            leases.insert(tab.clone(), Lease { owner: self.me.clone(), until: *until });
        }
        let out = OnDisk { session: ours.clone(), rev: disk.rev.max(self.rev) + 1, writer: self.me.clone(), leases: leases.clone() };
        crate::config::save(&self.path, &out)?;
        self.rev = out.rev;
        self.base = ours.clone();
        self.leases = leases;
        self.seen = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        Ok(())
    }

    /// Tabs another window has taken over: no longer ours.
    fn yield_taken(&mut self) {
        let now = now_ms();
        let me = self.me.clone();
        let taken: Vec<String> = self.leases.iter().filter(|(_, l)| l.owner != me && l.until > now).map(|(k, _)| k.clone()).collect();
        for k in taken {
            self.mine.remove(&k);
        }
    }

    /// Typing in `tab`: it's ours for a while. True when that's new (worth
    /// writing at once, for the others to see).
    pub fn hold(&mut self, tab: &str) -> bool {
        let until = now_ms() + HOLD.as_millis() as u64;
        let fresh = self.mine.get(tab).is_none_or(|u| *u < until.saturating_sub(2000));
        self.mine.insert(tab.to_string(), until);
        fresh
    }

    /// Another window holds `tab` (it's typed in there).
    pub fn held_elsewhere(&self, tab: &str) -> bool {
        self.leases.get(tab).is_some_and(|l| l.owner != self.me && l.until > now_ms())
    }

    /// Takes `tab` over from the window holding it.
    pub fn take_over(&mut self, tab: &str) {
        self.leases.remove(tab);
        self.taken.insert(tab.to_string());
        self.hold(tab);
    }

    /// Our sign of life, every couple of seconds; counts the others'.
    pub fn heartbeat(&mut self) {
        if self.beat.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.beat = Instant::now();
        let Some(dir) = self.path.parent().map(|p| p.join("windows")) else { return };
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join(&self.me), b"").ok();
        let mut others = 0;
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let age = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).unwrap_or(Duration::MAX);
                if e.file_name().to_string_lossy() == self.me {
                    continue;
                }
                if age < ALIVE {
                    others += 1;
                } else if age > Duration::from_secs(600) {
                    std::fs::remove_file(e.path()).ok();
                }
            }
        }
        self.others = others;
    }

    /// Gone: the sign taken away, the holds let go.
    pub fn leave(&mut self, ours: &mut Session) {
        self.mine.clear();
        self.push(ours).ok();
        if let Some(dir) = self.path.parent() {
            std::fs::remove_file(dir.join("windows").join(&self.me)).ok();
        }
    }
}

/// Three-way: what changed on each side since `base`, both put together.
/// The window's own place (active workspace, tabs, panel sizes) stays ours.
pub fn merge(base: &Session, ours: &Session, theirs: &Session, conflicts: &mut Vec<String>) -> Session {
    let mut out = ours.clone();
    out.workspaces = merge_list(&base.workspaces, &ours.workspaces, &theirs.workspaces, |w| w.id.clone(), &mut |b, o, t| vec![merge_ws(b, o, t, conflicts)]);
    if same(&ours.project, &base.project) {
        out.project = theirs.project.clone();
    }
    if same(&ours.schema_filters, &base.schema_filters) {
        out.schema_filters = theirs.schema_filters.clone();
    }
    if out.workspaces.is_empty() {
        out.workspaces = vec![Workspace::default()];
    }
    // The workspace shown, by its id.
    let shown = ours.workspaces.get(ours.active).map(|w| w.id.clone());
    out.active = shown.and_then(|id| out.workspaces.iter().position(|w| w.id == id)).unwrap_or(ours.active.min(out.workspaces.len() - 1));
    out
}

fn merge_ws(base: Option<&Workspace>, o: &Workspace, t: &Workspace, conflicts: &mut Vec<String>) -> Workspace {
    let empty = Workspace { tabs: Vec::new(), ..o.clone() };
    let b = base.unwrap_or(&empty);
    let mut w = o.clone();
    if o.name == b.name {
        w.name = t.name.clone();
    }
    if o.color == b.color {
        w.color = t.color;
    }
    if o.conn == b.conn {
        w.conn = t.conn.clone();
    }
    w.tabs = merge_list(&b.tabs, &o.tabs, &t.tabs, |x| x.id.clone(), &mut |bt, ot, tt| merge_tab(bt, ot, tt, conflicts));
    if w.tabs.is_empty() {
        w.tabs.push(Tab::default());
    }
    let shown = o.tabs.get(o.active).map(|x| x.id.clone());
    w.active = shown.and_then(|id| w.tabs.iter().position(|x| x.id == id)).unwrap_or(o.active.min(w.tabs.len() - 1));
    w
}

fn merge_tab(base: Option<&Tab>, o: &Tab, t: &Tab, conflicts: &mut Vec<String>) -> Vec<Tab> {
    let Some(b) = base else {
        // Made on both sides with one id: can't be; ours.
        return vec![o.clone()];
    };
    let (vo, vt, vb) = (value(o), value(t), value(b));
    if vo == vb || vo == vt {
        return vec![t.clone()];
    }
    if vt == vb {
        return vec![o.clone()];
    }
    // Both changed it: ours stays, theirs beside it as a copy.
    conflicts.push(o.title.clone());
    let mut copy = t.clone();
    copy.id = crate::db::new_id();
    copy.title = format!("{} ({})", t.title, crate::i18n::t("другое окно", "other window"));
    vec![o.clone(), copy]
}

/// A list merged by its items' ids: added, dropped and changed on either
/// side; the order theirs when ours kept the old one.
fn merge_list<T: Clone + Serialize>(base: &[T], ours: &[T], theirs: &[T], id: impl Fn(&T) -> String, item: &mut dyn FnMut(Option<&T>, &T, &T) -> Vec<T>) -> Vec<T> {
    let ids = |v: &[T]| v.iter().map(&id).collect::<Vec<_>>();
    let (bi, oi, ti) = (ids(base), ids(ours), ids(theirs));
    let find = |v: &'_ [T], k: &str| v.iter().find(|x| id(x) == k).cloned();
    // Order: theirs when ours is still the old one, else ours; what the
    // other side added after.
    let mut order: Vec<String> = if oi == bi { ti.clone() } else { oi.clone() };
    for k in ti.iter().chain(oi.iter()) {
        if !order.contains(k) {
            order.push(k.clone());
        }
    }
    let mut out = Vec::new();
    for k in order {
        let (b, o, t) = (find(base, &k), find(ours, &k), find(theirs, &k));
        match (o, t) {
            (Some(o), Some(t)) => out.extend(item(b.as_ref(), &o, &t)),
            // Gone from theirs: dropped there (kept if we changed it since).
            (Some(o), None) => {
                if b.as_ref().is_none_or(|b| !same(b, &o)) {
                    out.push(o);
                }
            }
            // Gone from ours: dropped here (theirs kept if changed there since).
            (None, Some(t)) => {
                if b.as_ref().is_none_or(|b| !same(b, &t)) {
                    out.push(t);
                }
            }
            (None, None) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str, text: &str) -> Tab {
        Tab { id: id.into(), title: id.into(), text: text.into(), ..Default::default() }
    }

    fn session(tabs: Vec<Tab>) -> Session {
        Session { workspaces: vec![Workspace { id: "w".into(), name: "W".into(), tabs, active: 0, ..Default::default() }], ..Default::default() }
    }

    fn texts(s: &Session) -> Vec<(String, String)> {
        s.workspaces[0].tabs.iter().map(|t| (t.title.clone(), t.text.clone())).collect()
    }

    #[test]
    fn three_way() {
        let base = session(vec![tab("a", "1"), tab("b", "2"), tab("c", "3")]);
        // Ours: a edited, c closed. Theirs: b edited, d added.
        let ours = session(vec![tab("a", "1 ours"), tab("b", "2")]);
        let theirs = session(vec![tab("a", "1"), tab("b", "2 theirs"), tab("c", "3"), tab("d", "new")]);
        let mut c = Vec::new();
        let m = merge(&base, &ours, &theirs, &mut c);
        assert_eq!(texts(&m), [("a".into(), "1 ours".into()), ("b".into(), "2 theirs".into()), ("d".into(), "new".into())]);
        assert!(c.is_empty());
        // Both edited a: ours stays, theirs as a copy.
        let theirs = session(vec![tab("a", "1 theirs"), tab("b", "2"), tab("c", "3")]);
        let m = merge(&base, &ours, &theirs, &mut c);
        assert_eq!(m.workspaces[0].tabs.len(), 3);
        assert_eq!(m.workspaces[0].tabs[1].text, "1 theirs");
        assert!(m.workspaces[0].tabs[1].title.starts_with("a ("));
        assert_eq!(c, ["a"]);
        // Closed there, edited here: kept.
        let theirs = session(vec![tab("b", "2"), tab("c", "3")]);
        let m = merge(&base, &ours, &theirs, &mut Vec::new());
        assert_eq!(texts(&m)[0], ("a".into(), "1 ours".into()));
        // The order changed there, not here: theirs.
        let theirs = session(vec![tab("c", "3"), tab("a", "1"), tab("b", "2")]);
        let m = merge(&base, &base, &theirs, &mut Vec::new());
        assert_eq!(m.workspaces[0].tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["c", "a", "b"]);
    }

    #[test]
    fn two_windows_through_the_file() {
        let dir = std::env::temp_dir().join(format!("zapros-sync-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        crate::config::save(&path, &session(vec![tab("a", "1"), tab("b", "2")])).unwrap();
        let (mut s1, mut w1) = Sync::open(&path);
        let (mut s2, mut w2) = Sync::open(&path);
        // One types in a: holds it; the other sees the text and the hold.
        s1.workspaces[0].tabs[0].text = "1 typed".into();
        w1.hold("a");
        w1.push(&mut s1).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(w2.pull(&mut s2));
        assert_eq!(s2.workspaces[0].tabs[0].text, "1 typed");
        assert!(w2.held_elsewhere("a") && !w1.held_elsewhere("a"));
        // The other edits b meanwhile and writes: both changes there.
        s2.workspaces[0].tabs[1].text = "2 edited".into();
        w2.push(&mut s2).unwrap();
        s1.workspaces[0].tabs[0].text = "1 typed more".into();
        w1.push(&mut s1).unwrap();
        assert_eq!(texts(&s1), [("a".into(), "1 typed more".into()), ("b".into(), "2 edited".into())]);
        std::thread::sleep(Duration::from_millis(20));
        w2.pull(&mut s2);
        assert_eq!(texts(&s2), texts(&s1));
        // Taken over: the first one sees it held elsewhere.
        w2.take_over("a");
        w2.push(&mut s2).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        w1.pull(&mut s1);
        assert!(w1.held_elsewhere("a"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
