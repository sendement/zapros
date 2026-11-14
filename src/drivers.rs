//! Database drivers. PostgreSQL, ClickHouse and SQLite are built in; Oracle
//! needs Oracle's Instant Client (libclntsh). It can be the system's, or
//! one downloaded here (Oracle's site, any version listed there, Basic or
//! Basic Light) into zapros' own folder and loaded from it — no system
//! install, no LD_LIBRARY_PATH.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::i18n::t;

const PAGE: &str = "https://www.oracle.com/database/technologies/instant-client/linux-x86-64-downloads.html";

/// An Instant Client one can download.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// Basic Light (English messages, a few character sets, smaller).
    pub lite: bool,
    pub url: String,
}

impl Release {
    pub fn label(&self) -> String {
        format!("{} · {}", self.version, if self.lite { "Basic Light" } else { "Basic" })
    }

    /// Its folder's name here.
    fn dir_name(&self) -> String {
        format!("{}{}", self.version, if self.lite { "-lite" } else { "" })
    }
}

/// The releases in Oracle's download page's links.
pub fn parse_releases(page: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    for (i, _) in page.match_indices("https://download.oracle.com/otn_software/linux/instantclient/") {
        let end = page[i..].find(['"', '\'', ' ', '<', '>']).map_or(page.len(), |e| i + e);
        let url = &page[i..end];
        let Some(file) = url.rsplit('/').next() else { continue };
        let lite = file.starts_with("instantclient-basiclite-linux.x64-");
        let Some(v) = file.strip_prefix(if lite { "instantclient-basiclite-linux.x64-" } else { "instantclient-basic-linux.x64-" }).and_then(|v| v.strip_suffix(".zip")) else { continue };
        let version = v.trim_end_matches("dbru").to_string();
        if !out.iter().any(|r| r.url == url) {
            out.push(Release { version, lite, url: url.to_string() });
        }
    }
    // Newest first, Basic before Basic Light.
    let key = |r: &Release| r.version.split('.').map(|x| x.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    out.sort_by(|a, b| key(b).cmp(&key(a)).then(a.lite.cmp(&b.lite)));
    out
}

/// Where downloaded clients live (a folder each).
pub fn root() -> PathBuf {
    crate::config::data_dir().join("drivers/oracle")
}

/// A client downloaded here.
#[derive(Clone, Debug)]
pub struct Installed {
    pub name: String,
    /// The folder holding libclntsh.so.
    pub lib_dir: PathBuf,
    pub size: u64,
    /// Libraries it needs that the system lacks (libaio…).
    pub missing: Vec<String>,
}

fn dir_size(p: &Path) -> u64 {
    std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| if e.file_type().is_ok_and(|t| t.is_dir()) { dir_size(&e.path()) } else { e.metadata().map_or(0, |m| m.len()) }).sum()).unwrap_or(0)
}

/// The folder under `p` holding libclntsh.so (the zip has one inside).
fn lib_dir_in(p: &Path) -> Option<PathBuf> {
    if p.join("libclntsh.so").exists() || std::fs::read_dir(p).ok()?.flatten().any(|e| e.file_name().to_string_lossy().starts_with("libclntsh.so")) {
        return Some(p.to_path_buf());
    }
    std::fs::read_dir(p).ok()?.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).find_map(|e| lib_dir_in(&e.path()))
}

/// What `ldd` says the library can't find.
fn missing_libs(lib_dir: &Path) -> Vec<String> {
    let lib = std::fs::read_dir(lib_dir).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("libclntsh.so."))));
    let Some(lib) = lib.or_else(|| Some(lib_dir.join("libclntsh.so"))) else { return Vec::new() };
    // The folder before the path already set (Flatpak's /app/lib: libaio).
    let mut path = lib_dir.as_os_str().to_owned();
    if let Some(old) = std::env::var_os("LD_LIBRARY_PATH").filter(|v| !v.is_empty()) {
        path.push(":");
        path.push(old);
    }
    let Ok(out) = std::process::Command::new("ldd").arg(&lib).env("LD_LIBRARY_PATH", path).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains("not found")).filter_map(|l| l.split_whitespace().next().map(str::to_string)).collect()
}

pub fn installed() -> Vec<Installed> {
    let mut v: Vec<Installed> = std::fs::read_dir(root())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| {
                    let lib_dir = lib_dir_in(&e.path())?;
                    Some(Installed { name: e.file_name().to_string_lossy().into_owned(), size: dir_size(&e.path()), missing: missing_libs(&lib_dir), lib_dir })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.name.cmp(&a.name));
    v
}

/// What's going on in the background.
#[derive(Clone, Debug, Default)]
pub enum Job {
    #[default]
    Idle,
    Listing,
    Downloading { label: String, got: u64, total: u64 },
    Unpacking { label: String },
    Failed(String),
    Done(String),
}

/// Known releases, until Oracle's page is read.
pub fn known() -> Vec<Release> {
    let mut v = Vec::new();
    for (dir, ver) in [("2326400", "23.26.4.0.0"), ("2124000", "21.24.0.0.0dbru"), ("1933000", "19.33.0.0.0dbru")] {
        for lite in [false, true] {
            let kind = if lite { "basiclite" } else { "basic" };
            v.push(Release { version: ver.trim_end_matches("dbru").into(), lite, url: format!("https://download.oracle.com/otn_software/linux/instantclient/{dir}/instantclient-{kind}-linux.x64-{ver}.zip") });
        }
    }
    v
}

pub struct State {
    pub releases: Vec<Release>,
    pub job: Job,
    /// The installed list is to be read again.
    pub changed: bool,
}

pub fn state() -> Arc<Mutex<State>> {
    static S: OnceLock<Arc<Mutex<State>>> = OnceLock::new();
    S.get_or_init(|| Arc::new(Mutex::new(State { releases: known(), job: Job::Idle, changed: false }))).clone()
}

/// Oracle's page read for its releases (in the background).
pub fn list(ctx: &egui::Context) {
    let st = state();
    st.lock().unwrap().job = Job::Listing;
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let got = ureq::get(PAGE).call().map_err(|e| e.to_string()).and_then(|mut r| r.body_mut().read_to_string().map_err(|e| e.to_string()));
        let mut s = st.lock().unwrap();
        match got {
            Ok(page) => {
                let found = parse_releases(&page);
                s.job = if found.is_empty() { Job::Failed(t("На странице Oracle не нашлось ссылок на Instant Client", "No Instant Client links on Oracle's page").into()) } else {
                    s.releases = found;
                    Job::Idle
                };
            }
            Err(e) => s.job = Job::Failed(e),
        }
        ctx.request_repaint();
    });
}

/// A release downloaded and unpacked here (in the background).
pub fn download(ctx: &egui::Context, r: Release) {
    let st = state();
    let label = r.label();
    st.lock().unwrap().job = Job::Downloading { label: label.clone(), got: 0, total: 0 };
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let res = (|| -> Result<String, String> {
            let dest = root().join(r.dir_name());
            std::fs::create_dir_all(root()).map_err(|e| e.to_string())?;
            let tmp = root().join(format!(".{}.zip", r.dir_name()));
            let mut resp = ureq::get(&r.url).call().map_err(|e| e.to_string())?;
            let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
            let mut body = resp.body_mut().with_config().limit(1 << 31).reader();
            let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 1 << 16];
            let mut got = 0u64;
            loop {
                let n = body.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| e.to_string())?;
                got += n as u64;
                if let Ok(mut s) = st.lock() {
                    s.job = Job::Downloading { label: label.clone(), got, total };
                }
                ctx.request_repaint();
            }
            drop(file);
            st.lock().unwrap().job = Job::Unpacking { label: label.clone() };
            ctx.request_repaint();
            let _ = std::fs::remove_dir_all(&dest);
            let f = std::fs::File::open(&tmp).map_err(|e| e.to_string())?;
            let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
            zip.extract(&dest).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&tmp);
            let lib = lib_dir_in(&dest).ok_or_else(|| t("В архиве нет libclntsh.so", "No libclntsh.so in the archive").to_string())?;
            // (The zip may hold the .so.NN only: the plain name ODPI-C opens.)
            if !lib.join("libclntsh.so").exists() {
                if let Some(real) = std::fs::read_dir(&lib).ok().and_then(|rd| rd.flatten().map(|e| e.file_name()).find(|n| n.to_string_lossy().starts_with("libclntsh.so."))) {
                    let _ = std::os::unix::fs::symlink(real, lib.join("libclntsh.so"));
                }
            }
            Ok(label.clone())
        })();
        let mut s = st.lock().unwrap();
        s.job = match res {
            Ok(l) => Job::Done(l),
            Err(e) => Job::Failed(e),
        };
        s.changed = true;
        ctx.request_repaint();
    });
}

/// The client folder to load Oracle from (None: the system's), as set
/// at start; it can't change once the library is loaded.
static CHOSEN: OnceLock<Option<PathBuf>> = OnceLock::new();

pub fn choose_at_start(dir: &str) {
    let _ = CHOSEN.set((!dir.is_empty()).then(|| PathBuf::from(dir)));
}

const MARK: &str = "ZAPROS_ORACLE_LIB";
const SAVED: &str = "ZAPROS_SAVED_LD_PATH";

/// At the very start (no threads yet): with a client downloaded here
/// chosen, the program starts itself again with its folder on
/// LD_LIBRARY_PATH — libclntsh 19/21 look for their own libraries
/// (libnnz19.so has no soname to be preloaded by) only along it, and the
/// loader reads it at a process's start only. Then the variable is put
/// back as it was, so programs started from here don't get it (the
/// loader keeps its own copy).
pub fn prepare(dir: &str) {
    choose_at_start(dir);
    let Some(d) = chosen_at_start() else { return };
    if std::env::var_os(MARK).is_some_and(|m| m == d.as_os_str()) {
        // SAFETY: at the start of main, before any thread.
        unsafe {
            match std::env::var_os(SAVED) {
                Some(v) if !v.is_empty() => std::env::set_var("LD_LIBRARY_PATH", v),
                _ => std::env::remove_var("LD_LIBRARY_PATH"),
            }
            std::env::remove_var(SAVED);
            std::env::remove_var(MARK);
        }
        return;
    }
    if !d.join("libclntsh.so").exists() {
        return;
    }
    let old = std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default();
    let mut path = d.clone().into_os_string();
    if !old.is_empty() {
        path.push(":");
        path.push(&old);
    }
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os();
    let argv0 = args.next().unwrap_or_default();
    // Returns only if it couldn't start (then Oracle may just not load).
    let e = std::process::Command::new("/proc/self/exe").arg0(argv0).args(args).env("LD_LIBRARY_PATH", path).env(SAVED, old).env(MARK, &d).exec();
    eprintln!("zapros: restarting with Oracle's client: {e}");
}

/// The program started anew in place of this one (all saved first).
pub fn restart() {
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os();
    let argv0 = args.next().unwrap_or_default();
    let e = std::process::Command::new("/proc/self/exe").arg0(argv0).args(args).exec();
    eprintln!("zapros: couldn't restart: {e}");
}

pub fn chosen_at_start() -> Option<PathBuf> {
    CHOSEN.get().cloned().flatten()
}

/// Before Oracle's first use: its client from the folder chosen.
#[cfg(feature = "oracle")]
pub fn init_oracle() -> Result<(), String> {
    static DONE: OnceLock<Result<(), String>> = OnceLock::new();
    DONE.get_or_init(|| {
        if oracle::InitParams::is_initialized() {
            return Ok(());
        }
        let mut p = oracle::InitParams::new();
        if let Some(dir) = chosen_at_start() {
            p.oracle_client_lib_dir(dir.to_string_lossy().as_ref()).map_err(|e| e.to_string())?;
        }
        p.init().map(|_| ()).map_err(|e| e.to_string())
    })
    .clone()
}

/// The client loaded, if it is ("23.26.4.0.0").
#[cfg(feature = "oracle")]
pub fn loaded_version() -> Option<String> {
    oracle::InitParams::is_initialized().then(|| oracle::Version::client().ok().map(|v| v.to_string())).flatten()
}

#[cfg(not(feature = "oracle"))]
pub fn loaded_version() -> Option<String> {
    None
}

/// What to do when Oracle's client can't be loaded.
pub fn hint() -> &'static str {
    t(
        "Нужен Oracle Instant Client (libclntsh.so). Его можно скачать в Настройки → Драйверы — ставить в систему не нужно.",
        "Oracle Instant Client (libclntsh.so) is needed. Download it in Settings → Drivers — no system install needed.",
    )
}

/// The system's libclntsh, if ldconfig knows one.
pub fn system_client() -> Option<String> {
    let out = std::process::Command::new("ldconfig").arg("-p").output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find(|l| l.trim_start().starts_with("libclntsh.so")).and_then(|l| l.split("=>").nth(1)).map(|p| p.trim().to_string())
}

pub fn human(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1.0 { format!("{mb:.0} MB") } else { format!("{} KB", bytes / 1024) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_from_the_page() {
        let page = r#"<a href="https://download.oracle.com/otn_software/linux/instantclient/1933000/instantclient-basic-linux.x64-19.33.0.0.0dbru.zip">x</a>
            <a href='https://download.oracle.com/otn_software/linux/instantclient/2326400/instantclient-basiclite-linux.x64-23.26.4.0.0.zip'>
            <a href="https://download.oracle.com/otn_software/linux/instantclient/2326400/instantclient-basic-linux.x64-23.26.4.0.0.zip">
            <a href="https://download.oracle.com/otn_software/linux/instantclient/2326400/instantclient-sqlplus-linux.x64-23.26.4.0.0.zip">"#;
        let r = parse_releases(page);
        assert_eq!(r.iter().map(|r| r.label()).collect::<Vec<_>>(), ["23.26.4.0.0 · Basic", "23.26.4.0.0 · Basic Light", "19.33.0.0.0 · Basic"]);
    }

    /// Downloads Instant Client 19 (the system has 23), loads it and
    /// connects: the client in use is the one downloaded.
    ///   ZAPROS_TEST_ORA=… cargo test drivers::tests::downloaded_client -- --ignored
    #[cfg(feature = "oracle")]
    #[test]
    #[ignore]
    fn downloaded_client() {
        if std::env::var("ZAPROS_TEST_ORA").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("zapros-drivers-{}", std::process::id()));
        // SAFETY: the only test in its process touching the environment.
        unsafe { std::env::set_var("ZAPROS_DATA", &dir) };
        let r = known().into_iter().find(|r| r.version.starts_with("19.") && r.lite).unwrap();
        download(&egui::Context::default(), r);
        loop {
            match state().lock().unwrap().job.clone() {
                Job::Done(_) => break,
                Job::Failed(e) => panic!("{e}"),
                _ => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let got = installed();
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].missing.is_empty(), "{:?}", got[0].missing);
        // Connecting in a process of its own, started as the program is.
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "drivers::tests::connect_with_downloaded", "--ignored", "--nocapture"])
            .env("ZAPROS_TEST_CLIENT", &got[0].lib_dir)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        assert!(text.contains("1 passed"), "{text}");
        assert!(text.contains("client 19."), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (The downloaded_client test's other half.)
    #[cfg(feature = "oracle")]
    #[test]
    #[ignore]
    fn connect_with_downloaded() {
        let (Ok(v), Some(dir)) = (std::env::var("ZAPROS_TEST_ORA"), std::env::var_os("ZAPROS_TEST_CLIENT")) else { return };
        prepare(dir.to_str().unwrap());
        // Started again: the variable is back as it was.
        assert!(std::env::var_os(MARK).is_none());
        assert!(!std::env::var("LD_LIBRARY_PATH").unwrap_or_default().contains(dir.to_str().unwrap()));
        let p: Vec<&str> = v.split(':').collect();
        let spec = crate::db::Spec { kind: crate::db::Kind::Oracle, host: p[0].into(), port: p[1].parse().unwrap(), database: p[2].into(), user: p[3].into(), ..Default::default() };
        let mut s = crate::db::open(&spec, p[4]).unwrap();
        assert!(s.run_limit("select 1 from dual", 10, &mut |_| {}).is_ok());
        let ver = loaded_version().unwrap();
        println!("client {ver}");
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        assert!(maps.lines().any(|l| l.contains("libclntsh") && l.contains(dir.to_str().unwrap())));
        assert!(!maps.contains("/usr/lib/libclntsh"));
    }
}
