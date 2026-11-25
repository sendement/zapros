//! The window drawn offscreen (wgpu) against a SQLite file (and the local
//! PostgreSQL when ZAPROS_TEST_PG is set); pictures land in
//! ZAPROS_SHOT_DIR when it's set.

use std::path::PathBuf;
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use crate::app::{App, ConnState, Node};
use crate::config::{self, Connections, Session, Tab, Workspace};
use crate::db::{Kind, Spec, Tls};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zapros-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("project/reports")).unwrap();
    d
}

/// A SQLite file with a couple of tables.
fn sqlite_db(path: &std::path::Path) {
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch(
        "create table customers (id integer primary key, name text not null, email text, active int default 1);
         create table orders (id integer primary key, customer_id int references customers(id), total real, note text);
         create view big_orders as select * from orders where total > 100;
         create index orders_customer on orders(customer_id);
         with recursive n(x) as (select 1 union all select x + 1 from n where x < 300)
         insert into customers (name, email) select 'Customer ' || x, 'c' || x || '@example.com' from n;
         with recursive n(x) as (select 1 union all select x + 1 from n where x < 3000)
         insert into orders (customer_id, total, note) select 1 + x % 300, (x * 37) % 1000 + 0.5, case when x % 5 = 0 then 'gift' end from n;",
    )
    .unwrap();
}

pub const SCRIPT: &str = "-- Top customers\nselect c.name, count(o.id) as orders, sum(o.total) total,\n       case when sum(o.total) > 5000 then 'vip' else 'regular' end as kind\n  from customers c\n  join orders o on o.customer_id = c.id\n where c.active = 1\n group by c.name\n order by total desc;\n\nselect case when total > 100 then 'big'\n  from orders o where (o.total > 10;\n\nupdate orders set note = 'x';\n";

/// One window at a time: they share the process's environment.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn harness(name: &str, script: &str, pg: bool) -> (Harness<'static, App>, std::sync::MutexGuard<'static, ()>) {
    harness_at(name, script, pg, 1.0)
}

fn harness_at(name: &str, script: &str, pg: bool, scale: f32) -> (Harness<'static, App>, std::sync::MutexGuard<'static, ()>) {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = dir(name);
    unsafe {
        std::env::set_var("ZAPROS_CONFIG", d.join("config"));
        std::env::set_var("ZAPROS_DATA", d.join("data"));
        std::env::set_var("ZAPROS_NO_KEYRING", "1");
    }
    let db = d.join("shop.db");
    sqlite_db(&db);
    std::fs::write(d.join("project/top.sql"), "select 1;\n").unwrap();
    std::fs::write(d.join("project/reports/daily.sql"), "select 2;\n").unwrap();
    let lite = Spec { id: "lite".into(), name: "shop.db".into(), kind: Kind::Sqlite, database: db.to_string_lossy().into_owned(), color: Some([76, 175, 80]), ..Default::default() };
    let mut list = vec![lite];
    if pg {
        if let Ok(v) = std::env::var("ZAPROS_TEST_PG") {
            let p: Vec<&str> = v.split(':').collect();
            list.push(Spec { id: "pg".into(), name: "demo (local)".into(), kind: Kind::Postgres, host: p[0].into(), port: p[1].parse().unwrap(), database: p[2].into(), user: p[3].into(), save_password: true, tls: Tls::Disable, color: Some([92, 140, 230]), ..Default::default() });
            unsafe { std::env::set_var("ZAPROS_DATA", d.join("data")) };
            crate::secrets::set("pg", p[4]).unwrap();
        }
    }
    config::save(&d.join("config/connections.json"), &Connections { list }).unwrap();
    let ws = |name: &str, color, conn: &str, tabs: Vec<Tab>| Workspace { name: name.into(), color, conn: Some(conn.into()), tabs, active: 0, ..Default::default() };
    let session = Session {
        workspaces: vec![
            ws("Отчёты", [92, 140, 230], "lite", vec![Tab { title: "Клиенты".into(), text: script.into(), ..Default::default() }, Tab { title: "Черновик".into(), ..Default::default() }]),
            ws("Миграции", [239, 108, 0], "lite", vec![Tab { title: "v42".into(), ..Default::default() }]),
            ws("Прод", [229, 57, 53], "lite", vec![Tab { title: "hotfix".into(), ..Default::default() }]),
        ],
        project: Some(d.join("project")),
        ..Default::default()
    };
    config::save(&d.join("data/session.json"), &session).unwrap();
    let mut h = Harness::builder().with_size([1280.0, 800.0]).with_pixels_per_point(scale).with_step_dt(1.0 / 60.0).wgpu().build_eframe(|cc| {
        let mut app = App::new(cc);
        app.settings.russian = Some(true);
        crate::i18n::set_ru(true);
        app.settings.theme = crate::config::ThemePref::Dark;
        app
    });
    settle(&mut h);
    (h, guard)
}

/// Frames until no connection, query or tree node is pending.
fn settle(h: &mut Harness<'static, App>) {
    let mut quiet = 0;
    for _ in 0..1500 {
        h.step();
        let st = h.state();
        let busy = st.rt.values().any(|r| r.run.is_some() || r.check_due.is_some()) || st.live.values().any(|l| l.state == ConnState::Connecting || l.rels_loading) || st.tree.values().any(|n| matches!(n, Node::Loading)) || !st.tree_refreshing.is_empty();
        quiet = if busy { 0 } else { quiet + 1 };
        if quiet >= 10 {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let st = h.state();
    let runs: Vec<_> = st.rt.iter().filter(|(_, r)| r.run.is_some() || r.check_due.is_some()).map(|(k, r)| (k.clone(), r.run.is_some())).collect();
    let tree: Vec<_> = st.tree.iter().filter(|(_, n)| matches!(n, Node::Loading)).map(|(k, _)| k.clone()).collect();
    let live: Vec<_> = st.live.iter().map(|(k, l)| (k.clone(), l.state == ConnState::Connecting, l.rels_loading)).collect();
    panic!("still busy: {runs:?} {tree:?} {live:?}");
}

fn shot(h: &mut Harness<'static, App>, name: &str) {
    let img = h.render().expect("render");
    if let Some(dir) = std::env::var_os("ZAPROS_SHOT_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
}

fn key(h: &mut Harness<'static, App>, key: egui::Key, modifiers: egui::Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        h.step();
    }
}

fn type_text(h: &mut Harness<'static, App>, s: &str) {
    for ch in s.chars() {
        h.input_mut().events.push(egui::Event::Text(ch.to_string()));
        h.step();
    }
}

fn focus_editor(h: &mut Harness<'static, App>, at: usize) {
    let id = h.state().tab().id.clone();
    h.state_mut().rt.entry(id).or_default().ed.goto = Some(at);
    h.step();
    h.step();
}

#[test]
fn runs_a_statement_and_marks_problems() {
    let (mut h, _g) = harness("run", SCRIPT, false);
    // The checker found the open CASE, the open bracket and the UPDATE without WHERE.
    let issues = h.state().rt.get(&h.state().tab().id).map(|r| r.issues.clone()).unwrap_or_default();
    let text = h.state().tab().text.clone();
    let found: Vec<&str> = issues.iter().map(|i| &text[i.range.start..(i.range.start + 6).min(i.range.end)]).collect();
    assert!(found.iter().any(|s| s.starts_with("case")), "{found:?}");
    assert!(found.iter().any(|s| s.starts_with("(o.tot")), "{found:?}");
    assert!(found.iter().any(|s| s.starts_with("update")), "{found:?}");
    // Run the first statement (cursor in it).
    focus_editor(&mut h, 30);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    let rt = h.state().rt.get(&h.state().tab().id).unwrap();
    assert_eq!(rt.results.len(), 1, "{:?}", rt.log.iter().map(|l| l.text.clone()).collect::<Vec<_>>());
    assert_eq!(rt.results[0].table.rows.len(), 300);
    assert_eq!(rt.results[0].table.cols[3].name, "kind");
    // The tree shows the file's objects.
    h.state_mut().open_nodes.insert(("lite".into(), vec!["tables".into()]));
    settle(&mut h);
    h.state_mut().open_nodes.insert(("lite".into(), vec!["tables".into(), "orders".into()]));
    settle(&mut h);
    shot(&mut h, "main");
}

#[test]
fn hints_while_typing() {
    let (mut h, _g) = harness("hints", "", false);
    settle(&mut h);
    focus_editor(&mut h, 0);
    type_text(&mut h, "select o.");
    settle(&mut h);
    type_text(&mut h, "to");
    h.step();
    let st = h.state();
    let id = st.tab().id.clone();
    // No FROM yet: o is unknown; type the rest and come back.
    assert!(st.rt[&id].ed.popup.is_none() || st.rt[&id].ed.popup.as_ref().unwrap().hints.items.iter().all(|i| i.label != "total"));
    let text = "select o.\n  from orders o\n  join customers c on ";
    h.state_mut().tab_mut().text = text.into();
    h.step();
    focus_editor(&mut h, 9);
    key(&mut h, egui::Key::Space, egui::Modifiers::COMMAND);
    h.step();
    let labels: Vec<String> = h.state().rt[&id].ed.popup.as_ref().map(|p| p.hints.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default();
    assert!(labels.contains(&"total".to_string()), "{labels:?}");
    shot(&mut h, "hints-columns");
    // At the end, after ON: the join condition first.
    key(&mut h, egui::Key::Escape, egui::Modifiers::NONE);
    focus_editor(&mut h, text.len());
    key(&mut h, egui::Key::Space, egui::Modifiers::COMMAND);
    h.step();
    let first = h.state().rt[&id].ed.popup.as_ref().map(|p| p.hints.items[0].insert.clone());
    assert_eq!(first.as_deref(), Some("c.id = o.customer_id"));
    key(&mut h, egui::Key::Enter, egui::Modifiers::NONE);
    h.step();
    assert!(h.state().tab().text.ends_with("on c.id = o.customer_id"), "{}", h.state().tab().text);
    shot(&mut h, "hints-join");
    // Not connected (after a restart too): the hints go on with what was
    // last loaded, kept on the disk.
    h.state_mut().disconnect("lite");
    settle(&mut h);
    assert!(!h.state().live.contains_key("lite"));
    h.state_mut().hint_cache.clear();
    h.state_mut().load_hints();
    h.state_mut().tab_mut().text = text.into();
    h.step();
    focus_editor(&mut h, 9);
    key(&mut h, egui::Key::Space, egui::Modifiers::COMMAND);
    h.step();
    let labels: Vec<String> = h.state().rt[&id].ed.popup.as_ref().map(|p| p.hints.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default();
    assert!(labels.contains(&"total".to_string()), "offline: {labels:?}");
}

#[test]
fn formats_and_pairs() {
    let (mut h, _g) = harness("fmt", "select a,b from t where x=1 and y in (1,2)", false);
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::F, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    h.step();
    assert_eq!(h.state().tab().text, "SELECT a\n     , b\n  FROM t\n WHERE x = 1\n   AND y IN (1, 2)");
    // Brackets and quotes typed in pairs.
    h.state_mut().tab_mut().text = String::new();
    focus_editor(&mut h, 0);
    type_text(&mut h, "f('x");
    h.step();
    assert_eq!(h.state().tab().text, "f('x')");
}

#[test]
fn settings_and_workspace_windows() {
    let (mut h, _g) = harness("settings", SCRIPT, false);
    h.state_mut().settings_open = true;
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), 4usize));
    settle(&mut h);
    shot(&mut h, "settings-colours");
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), 3usize));
    settle(&mut h);
    shot(&mut h, "settings-formatter");
    // One size on every page.
    let mut sizes = Vec::new();
    for page in 0..7usize {
        h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), page));
        settle(&mut h);
        sizes.push(h.ctx.memory(|m| m.area_rect(egui::Id::new("settings-window"))).map(|r| r.size()));
    }
    assert!(sizes[0].is_some() && sizes.iter().all(|s| *s == sizes[0]), "{sizes:?}");
    // Drivers: a client downloaded earlier is listed and can be picked;
    // the pick waits for a restart.
    let lib = crate::drivers::root().join("23.26.4.0.0-lite/instantclient_23_26");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("libclntsh.so"), b"").unwrap();
    h.ctx.data_mut(|d| d.remove_temp::<std::sync::Arc<Vec<crate::drivers::Installed>>>(egui::Id::new("drivers-installed")));
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), 6usize));
    settle(&mut h);
    h.get_by_label("23.26.4.0.0-lite").click();
    settle(&mut h);
    assert_eq!(h.state().settings.oracle_client, lib.to_string_lossy());
    let _ = h.get_by_label_contains("после перезапуска");
    shot(&mut h, "settings-drivers");
    h.state_mut().settings.oracle_client.clear();
    // A new scale waits while the settings are open, comes once closed.
    h.state_mut().settings.ui_scale = 1.25;
    settle(&mut h);
    assert_eq!(h.ctx.zoom_factor(), 1.0);
    h.state_mut().settings_open = false;
    settle(&mut h);
    assert_eq!(h.ctx.zoom_factor(), 1.25);
    h.state_mut().settings.ui_scale = 1.0;
    settle(&mut h);
    h.state_mut().ws_edit = Some(1);
    settle(&mut h);
    shot(&mut h, "workspace");
    h.state_mut().ws_edit = None;
    h.state_mut().conn_edit = Some(crate::dialogs::ConnEdit::new(None));
    settle(&mut h);
    shot(&mut h, "connection");
}

#[test]
fn postgres_tree_and_notices() {
    if std::env::var("ZAPROS_TEST_PG").is_err() {
        return;
    }
    let script = "select vip(1);\n\ndo $$\nbegin\n  if true then\n    raise notice 'hello from plpgsql';\n  end if;\nend $$;\n\nselect * from orders o join customers c on c.id = o.customer_id limit 50;\n";
    let (mut h, _g) = harness("pg", script, true);
    {
        let st = h.state_mut();
        st.ws_mut().conn = Some("pg".into());
    }
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "pg", None);
    settle(&mut h);
    for p in [vec![], vec!["public".to_string()], vec!["public".into(), "tables".into()], vec!["public".into(), "functions".into()]] {
        h.state_mut().open_nodes.insert(("pg".into(), p));
        settle(&mut h);
    }
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    settle(&mut h);
    let rt = h.state().rt.get(&h.state().tab().id).unwrap();
    let log: Vec<String> = rt.log.iter().map(|l| l.text.clone()).collect();
    // The NOTICE in the output tab (not among the messages).
    assert!(rt.output.iter().any(|l| l.text.contains("hello from plpgsql")), "{:?}", rt.output.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert!(!log.iter().any(|l| l.starts_with("NOTICE") || l == "hello from plpgsql"), "{log:?}");
    assert_eq!(rt.results.len(), 2, "{log:?}");
    shot(&mut h, "pg");
    let id0 = h.state().tab().id.clone();
    h.state_mut().rt.get_mut(&id0).unwrap().shown = crate::app::Pane::Output;
    settle(&mut h);
    shot(&mut h, "pg-output");
    let id = h.state().tab().id.clone();
    h.state_mut().rt.get_mut(&id).unwrap().shown = crate::app::Pane::Messages;
    settle(&mut h);
    shot(&mut h, "pg-messages");
}

#[test]
fn big_script_stays_quick() {
    let one = "select c.name, count(o.id) from customers c join orders o on o.customer_id = c.id where c.active = 1 group by c.name;\n";
    let script = one.repeat(5000);
    let (mut h, _g) = harness("big", &script, false);
    focus_editor(&mut h, 100);
    let t = std::time::Instant::now();
    for _ in 0..20 {
        h.input_mut().events.push(egui::Event::Text("x".into()));
        h.step();
    }
    let per = t.elapsed() / 20;
    eprintln!("per typed frame: {per:?} ({} KB)", script.len() / 1024);
    assert!(per < std::time::Duration::from_millis(400), "{per:?}");
}

#[test]
fn transactions_by_hand() {
    let script = "update orders set note = 'kept' where id = 1;\nselect note from orders where id = 1;\n";
    let (mut h, _g) = harness("tx", script, false);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "lite", None);
    settle(&mut h);
    h.state_mut().tx("lite", crate::db::TxOp::Auto(false));
    settle(&mut h);
    focus_editor(&mut h, 3);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    let l = &h.state().live["lite"];
    assert!(!l.auto && l.pending, "auto {} pending {}", l.auto, l.pending);
    shot(&mut h, "tx-pending");
    // Rolled back: the change is gone, nothing pending.
    key(&mut h, egui::Key::Backspace, egui::Modifiers::COMMAND | egui::Modifiers::ALT);
    settle(&mut h);
    assert!(!h.state().live["lite"].pending);
    focus_editor(&mut h, 50);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    let rt = h.state().rt.get(&h.state().tab().id).unwrap();
    assert_eq!(rt.results.last().unwrap().table.rows[0][0], None);
    // Committed this time; switching auto-commit back on asks nothing.
    focus_editor(&mut h, 3);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND | egui::Modifiers::ALT);
    settle(&mut h);
    assert!(!h.state().live["lite"].pending);
    // Pending again: turning auto-commit on asks first.
    focus_editor(&mut h, 3);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    h.state_mut().tx("lite", crate::db::TxOp::Auto(true));
    h.step();
    assert!(h.state().tx_ask.is_some());
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "tx-ask");
    let ctx = h.ctx.clone();
    h.state_mut().tx_ask = None;
    h.state_mut().tx_answer(&ctx, vec!["lite".into()], crate::app::TxThen::AutoOn, true);
    settle(&mut h);
    let l = &h.state().live["lite"];
    assert!(l.auto && !l.pending);
    let db = rusqlite::Connection::open(&l.spec.database).unwrap();
    let note: Option<String> = db.query_row("select note from orders where id = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(note.as_deref(), Some("kept"));
}

#[test]
fn light_theme_sheet() {
    let (mut h, _g) = harness("light", SCRIPT, false);
    if std::env::var_os("ZAPROS_DARK").is_none() {
        h.state_mut().settings.theme = crate::config::ThemePref::Light;
    }
    h.state_mut().session.active = 1;
    settle(&mut h);
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "light");
}

#[test]
fn typing_cyrillic() {
    let (mut h, _g) = harness("cyr", SCRIPT, false);
    for at in [0usize, 30, 120, SCRIPT.len()] {
        focus_editor(&mut h, at);
        type_text(&mut h, "привет мир ёжик ЩУКА ");
        type_text(&mut h, "(кейс 'строка' \"ид\" -- коммент\n");
        key(&mut h, egui::Key::Enter, egui::Modifiers::NONE);
        type_text(&mut h, "фыва.пролд ");
        key(&mut h, egui::Key::Backspace, egui::Modifiers::NONE);
        key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
        key(&mut h, egui::Key::Space, egui::Modifiers::COMMAND);
        type_text(&mut h, "ж");
        key(&mut h, egui::Key::Tab, egui::Modifiers::NONE);
        key(&mut h, egui::Key::Slash, egui::Modifiers::COMMAND);
        key(&mut h, egui::Key::D, egui::Modifiers::COMMAND);
        settle(&mut h);
    }
    key(&mut h, egui::Key::F, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    settle(&mut h);
}

/// Random typing (Cyrillic, brackets, quotes, keys, hints) anywhere in the
/// text: nothing may cut a letter in half.
#[test]
fn random_typing_never_panics() {
    let (mut h, _g) = harness("fuzz", "select имя, (цена * 2) from товары т where т.код = 'щ';\n", false);
    let mut seed: u64 = std::env::var("ZAPROS_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(7);
    let mut rnd = |n: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n
    };
    let bits = ["ж", "Щука", "ёлка ", "(", ")", "'", "\"", "[", "]", ".", " ", ",", ";", "case ", "end", "select ", "from ", "т.", "ы", "--", "$$", "€", "日本", "😀"];
    let keys = [egui::Key::Backspace, egui::Key::Delete, egui::Key::Enter, egui::Key::ArrowLeft, egui::Key::ArrowRight, egui::Key::ArrowUp, egui::Key::Tab, egui::Key::Home, egui::Key::End];
    let steps: usize = std::env::var("ZAPROS_FUZZ_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
    let start = h.state().tab().text.clone();
    focus_editor(&mut h, 10);
    for n in 0..steps {
        match rnd(10) {
            0 => {
                let len = h.state().tab().text.len();
                let at = rnd(len + 1);
                focus_editor(&mut h, at);
            }
            1..=5 => type_text(&mut h, bits[rnd(bits.len())]),
            6 | 7 => {
                let m = if rnd(4) == 0 { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
                key(&mut h, keys[rnd(keys.len())], m);
            }
            8 => {
                let k = [egui::Key::Space, egui::Key::Slash, egui::Key::D][rnd(3)];
                key(&mut h, k, egui::Modifiers::COMMAND);
            }
            _ => {
                let p = egui::pos2(300.0 + rnd(900) as f32, 100.0 + rnd(380) as f32);
                h.input_mut().events.push(egui::Event::PointerMoved(p));
                h.step();
            }
        }
        if n % 150 == 149 {
            key(&mut h, egui::Key::F, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
            settle(&mut h);
        }
    }
    settle(&mut h);
    let end = h.state().tab().text.clone();
    eprintln!("fuzz text ({} → {} bytes):\n{end}", start.len(), end.len());
    assert!(end != start && end.chars().filter(|c| !c.is_ascii()).count() > 20, "typing didn't reach the editor");
}

pub const ORA_DYNAMIC: &str = "create or replace procedure purge_orders(p_status varchar2, p_days number) is\n  v_sql varchar2(4000);\n  v_cnt number;\nbegin\n  v_sql := 'select count(*) from orders o join customers c on c.id = o.customer_id where o.status = :1 and o.created_at < sysdate - ' || p_days;\n  if p_status = 'new' then\n    v_sql := v_sql || ' and o.note is null and c.segment in (''retail'', ''vip'')';\n  end if;\n  execute immediate v_sql into v_cnt using p_status;\n  execute immediate q'[delete from order_audit where changed_at < add_months(sysdate, -12) and new_status = 'cancelled']';\n  dbms_output.put_line('Удалено: ' || v_cnt);\nend;\n/\n";

pub const PG_DYNAMIC: &str = "create or replace function sales.archive(p_table text, p_before date) returns bigint\nlanguage plpgsql as $$\ndeclare\n  n bigint;\nbegin\n  execute format('insert into archive.%I select * from sales.%I where created_at < %L and status in (''delivered'', ''cancelled'')', p_table, p_table, p_before);\n  get diagnostics n = row_count;\n  execute 'delete from sales.' || quote_ident(p_table) || ' where created_at < $1' using p_before;\n  return n;\nend $$;\n";

#[test]
fn dynamic_sql_coloured_and_formatted() {
    let (mut h, _g) = harness("dyn", ORA_DYNAMIC, false);
    // An Oracle connection that never answers: only its dialect is used.
    let ora = Spec { id: "ora".into(), name: "ora".into(), kind: Kind::Oracle, host: "127.0.0.1".into(), port: 1, save_password: false, ..Default::default() };
    let pg = Spec { id: "pgd".into(), name: "pg".into(), kind: Kind::Postgres, host: "127.0.0.1".into(), port: 1, save_password: false, ..Default::default() };
    h.state_mut().conns.list.extend([ora, pg]);
    h.state_mut().tab_mut().conn = Some("ora".into());
    h.state_mut().ask = None;
    settle(&mut h);
    h.state_mut().ask = None;
    shot(&mut h, "dyn-ora");
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::F, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    h.state_mut().ask = None;
    settle(&mut h);
    let text = h.state().tab().text.clone();
    assert!(text.contains("'SELECT COUNT(*)'"), "{text}");
    assert!(text.contains("q'[DELETE"), "{text}");
    assert!(text.contains("' AND o.note IS NULL"), "{text}");
    shot(&mut h, "dyn-ora-formatted");
    h.state_mut().new_tab(Some("pg".into()), PG_DYNAMIC.into(), Some("pgd".into()));
    h.state_mut().ask = None;
    settle(&mut h);
    h.state_mut().ask = None;
    shot(&mut h, "dyn-pg");
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::F, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    h.state_mut().ask = None;
    settle(&mut h);
    let text = h.state().tab().text.clone();
    assert!(text.contains("%I") && text.contains("%L"), "{text}");
    shot(&mut h, "dyn-pg-formatted");
}

#[test]
fn package_parts_and_structure() {
    let (mut h, _g) = harness("pkg", "", false);
    let ora = Spec { id: "ora".into(), name: "ora".into(), kind: Kind::Oracle, host: "127.0.0.1".into(), port: 1, save_password: false, ..Default::default() };
    h.state_mut().conns.list.push(ora);
    let pkg = include_str!("../examples/seed/ora.sql");
    let a = pkg.find("create or replace package shop_pkg").unwrap();
    let b = pkg.find("create or replace function order_vat").unwrap();
    // As the database gives it: the specification, then the body.
    h.state_mut().new_tab(Some("SHOP_PKG".into()), pkg[a..b].to_string(), Some("ora".into()));
    h.state_mut().tab_mut().split_package();
    h.state_mut().ask = None;
    settle(&mut h);
    h.state_mut().ask = None;
    assert!(h.state().tab().text.starts_with("create or replace package shop_pkg as"));
    // The body's function in the structure: a click shows the body there.
    let at_body = h.state().tab().other.as_ref().unwrap().find("function customer_ltv").unwrap();
    focus_editor(&mut h, 10);
    shot(&mut h, "pkg-spec");
    key(&mut h, egui::Key::Num2, egui::Modifiers::ALT);
    focus_editor(&mut h, at_body + 12);
    settle(&mut h);
    assert!(h.state().tab().on_body);
    assert!(h.state().tab().text.starts_with("create or replace package body shop_pkg"));
    shot(&mut h, "pkg-body");
    // Saved whole: specification first.
    let full = h.state().tab().full_text();
    assert!(full.find("package shop_pkg").unwrap() < full.find("package body shop_pkg").unwrap());
    // Each part shown without its "/", the whole script with both.
    assert!(!h.state().tab().text.trim_end().ends_with('/') && !h.state().tab().other.as_ref().unwrap().trim_end().ends_with('/'));
    assert_eq!(full.matches("\n/\n").count(), 2, "{full}");
    h.state_mut().tab_mut().join_parts();
    assert!(h.state().tab().other.is_none() && h.state().tab().text == full);
}

/// Presses and lets go a mouse button at `p`.
fn click_at(h: &mut Harness<'static, App>, p: egui::Pos2, button: egui::PointerButton) {
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: p, button, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
}

#[test]
fn bands_and_context_menu() {
    let (mut h, _g) = harness("menu", ORA_DYNAMIC, false);
    let ora = Spec { id: "ora".into(), name: "ora".into(), kind: Kind::Oracle, host: "127.0.0.1".into(), port: 1, save_password: false, ..Default::default() };
    h.state_mut().conns.list.push(ora);
    h.state_mut().tab_mut().conn = Some("ora".into());
    h.state_mut().ask = None;
    settle(&mut h);
    h.state_mut().ask = None;
    // Laid out, so the bands show under the blocks.
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::F, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    h.state_mut().ask = None;
    settle(&mut h);
    shot(&mut h, "bands");
    // Select all, right click in the text: the menu; UPPER CASE.
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::A, egui::Modifiers::COMMAND);
    h.step();
    click_at(&mut h, egui::pos2(500.0, 160.0), egui::PointerButton::Secondary);
    let (p, q) = h.state().rt.get(&h.state().tab().id).unwrap().ed.cursor.unwrap();
    assert!(p.max(q) - p.min(q) > 500, "the selection stays: {p}..{q}");
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "context-menu");
    h.get_by_label_contains("ВЕРХНИЙ РЕГИСТР").click();
    for _ in 0..3 {
        h.step();
    }
    let text = h.state().tab().text.clone();
    assert!(text.contains("CREATE OR REPLACE PROCEDURE PURGE_ORDERS"), "{text}");
    // Right click elsewhere (no selection there): the caret moves there.
    click_at(&mut h, egui::pos2(420.0, 127.0), egui::PointerButton::Primary);
    click_at(&mut h, egui::pos2(700.0, 241.0), egui::PointerButton::Secondary);
    h.step();
    let rt = h.state().rt.get(&h.state().tab().id).unwrap();
    let (p, q) = rt.ed.cursor.unwrap();
    assert_eq!(p, q, "no selection after the right click");
    assert!(crate::app::line_of(&text, p) > 3, "caret went to the clicked line: {p}");
}

pub const ORA_STRINGS: &str = "create or replace procedure report(p_days number) is\n  v_sql varchar2(4000);\nbegin\n  for r in (select id\n              from orders\n             where status = 'new') loop\n    if r.id > 0 then\n      execute immediate 'select count(*)\n                           from orders o\n                           join customers c\n                             on c.id = o.customer_id\n                          where o.id = :1\n                            and c.segment in (select s\n                                                from segments)'\n        using r.id;\n    end if;\n  end loop;\n  v_sql := 'select o.id '\n        || '  from orders o '\n        || ' where o.created_at < sysdate - ' || p_days\n        || '   and o.status = ''new''';\nend;\n/\n";

#[test]
fn bands_at_scale_and_in_strings() {
    let (mut h, _g) = harness_at("bands12", ORA_STRINGS, false, 1.2);
    let ora = Spec { id: "ora".into(), name: "ora".into(), kind: Kind::Oracle, host: "127.0.0.1".into(), port: 1, save_password: false, ..Default::default() };
    h.state_mut().conns.list.push(ora);
    h.state_mut().tab_mut().conn = Some("ora".into());
    h.state_mut().ask = None;
    settle(&mut h);
    h.state_mut().ask = None;
    shot(&mut h, "bands-1.2");
    let bands = crate::sql::bands::bands(ORA_STRINGS, crate::sql::Dialect::Oracle);
    let line = |b: &crate::sql::bands::Band| ORA_STRINGS[..b.range.start].matches('\n').count();
    // The string's lines (8…13) have the inner SELECT's bands.
    assert!(bands.iter().any(|b| line(b) == 9 && b.level > 1), "{bands:?}");
    // Frames: the caret in the IF, the pointer over the FOR loop's bracket.
    let id = h.state().tab().id.clone();
    h.state_mut().rt.get_mut(&id).unwrap().ed.goto = Some(ORA_STRINGS.find("using").unwrap());
    for _ in 0..3 {
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(560.0 / 1.2, 255.0 / 1.2)));
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "frames");
    // The tab's ✕ shows on hover (the tab grows to it).
    let crosses = |h: &Harness<'static, App>| h.query_all_by_label("🗙").count();
    let before = crosses(&h);
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(325.0, 62.0)));
    for _ in 0..4 {
        h.step();
    }
    shot(&mut h, "tab-hover");
    assert_eq!(crosses(&h), before + 1, "the ✕ under the pointer");
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(900.0, 700.0)));
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(crosses(&h), before, "no ✕ away from it");
}

#[test]
fn frame_follows_the_text() {
    let src = "select /*+ полный скан, parallel(4) */ *\n  from orders o\n where o.id in (select /*+ хинт */ id\n                  from customers\n                 where active = 1)";
    let (mut h, _g) = harness_at("frame-hint", src, false, 1.2);
    h.state_mut().ask = None;
    settle(&mut h);
    let id = h.state().tab().id.clone();
    h.state_mut().rt.get_mut(&id).unwrap().ed.goto = Some(src.find("customers").unwrap());
    for _ in 0..4 {
        h.step();
    }
    shot(&mut h, "frame-hint");
}

#[test]
fn results_in_batches_export_and_totals() {
    let (mut h, _g) = harness("batches", "select id, total, note from orders order by id;\n", false);
    focus_editor(&mut h, 5);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    let id = h.state().tab().id.clone();
    let loaded = |h: &Harness<'static, App>| h.state().rt[&id].results[0].table.rows.len();
    assert_eq!(loaded(&h), 500);
    assert!(h.state().rt[&id].results[0].cursor.is_some(), "the cursor stays open");
    shot(&mut h, "batches");
    // One more batch, then all the rest.
    h.get_by_label_contains("Ещё").click();
    let wait = |h: &mut Harness<'static, App>, n: usize| {
        for _ in 0..500 {
            h.step();
            if h.state().rt[&id].results[0].table.rows.len() == n && h.state().rt[&id].results[0].loading.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("rows: {}", h.state().rt[&id].results[0].table.rows.len());
    };
    wait(&mut h, 1000);
    h.get_by_label_contains("Все").click();
    wait(&mut h, 3000);
    for _ in 0..5 {
        h.step();
    }
    assert!(h.state().rt[&id].results[0].cursor.is_none(), "all in: the cursor's closed");
    // Totals only when asked: the header's menu has the actions, the value
    // shows in a row under the table.
    h.get_by_label("total").click_secondary();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.query_by_label_contains("Сумма:").is_none(), "no values in the menu");
    h.get_by_label_contains("Итоги").hover();
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "totals-menu");
    h.get_by_label("Сумма").click();
    for _ in 0..3 {
        h.step();
    }
    let total: f64 = (1..=3000).map(|x: u64| ((x * 37) % 1000) as f64 + 0.5).sum();
    let got = h.state().rt[&id].results[0].totals.get(&1).map(|t| t.1.clone());
    assert_eq!(got.and_then(|v| v.parse::<f64>().ok()), Some(total));
    // Sorted by note, then by total going down.
    {
        let v = &mut h.state_mut().rt.get_mut(&id).unwrap().results[0];
        v.sort = vec![(2, true), (1, false)];
        crate::results::reorder(v);
    }
    let v = &h.state().rt[&id].results[0];
    let first: Vec<(Option<String>, f64)> = v.order.iter().take(3).map(|&r| (v.table.rows[r][2].clone(), v.table.rows[r][1].as_deref().unwrap().parse().unwrap())).collect();
    assert_eq!(first[0].0.as_deref(), Some("gift"));
    assert!(first[0].1 >= first[1].1 && first[1].1 >= first[2].1, "{first:?}");
    // Two columns picked and moved to the front.
    h.get_by_label("note").click();
    h.step();
    h.get_by_label("total").click_modifiers(egui::Modifiers::COMMAND);
    h.step();
    assert_eq!(h.state().rt[&id].results[0].picked.len(), 2);
    h.get_by_label("note").click_secondary();
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label_contains("В начало").click();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.state().rt[&id].results[0].cols, vec![1, 2, 0]);
    shot(&mut h, "columns");
    // Run again: same columns, same order.
    focus_editor(&mut h, 5);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    assert_eq!(h.state().rt[&id].results[0].cols, vec![1, 2, 0], "the order's kept for the same columns");
    h.state_mut().rt.get_mut(&id).unwrap().results[0].sort.clear();
    // Saved as CSV, 1000 rows a file.
    h.get_by_label_contains("Экспорт").click();
    for _ in 0..3 {
        h.step();
    }
    let dir = std::env::temp_dir().join(format!("zapros-export-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    if let Some((_, _, e)) = h.state_mut().export.as_mut() {
        e.path = dir.join("orders.csv");
        e.split = 1000;
    }
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "export");
    h.get_by_label_contains("Сохранить").click();
    // (The rows load first, the file comes after.)
    for _ in 0..500 {
        h.step();
        if dir.join("orders_3.csv").exists() && h.state().rt[&id].results[0].cursor.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
    let third = std::fs::read_to_string(dir.join("orders_3.csv")).unwrap();
    assert_eq!(third.lines().count(), 1001, "header + 1000 rows");
    // The columns as shown (moved above).
    assert!(third.starts_with("total,note,id\n37.5,,2001\n"), "{}", &third[..40]);
    // The rest went to the file only: the table kept its first batch.
    assert_eq!(loaded(&h), 500, "rows past the first batch not loaded into the table");
    assert!(h.state().rt[&id].results[0].exporting.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn big_result_streamed_to_a_file() {
    // 300 000 rows: written a batch at a time, none kept but the first.
    let sql = "with recursive n(i) as (select 1 union all select i + 1 from n where i < 300000)\nselect i, 'row ' || i as name, i * 1.5 as v from n;\n";
    let (mut h, _g) = harness("big-export", sql, false);
    focus_editor(&mut h, 5);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    let id = h.state().tab().id.clone();
    assert_eq!(h.state().rt[&id].results[0].table.rows.len(), 500);
    let dir = std::env::temp_dir().join(format!("zapros-big-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (fmt, name) in [(crate::export::Format::Csv, "big.csv"), (crate::export::Format::Xlsx, "big.xlsx")] {
        if fmt == crate::export::Format::Xlsx {
            // Run again: the cursor's open anew.
            focus_editor(&mut h, 5);
            key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
            settle(&mut h);
        }
        h.get_by_label_contains("Экспорт").click();
        for _ in 0..3 {
            h.step();
        }
        if let Some((_, _, e)) = h.state_mut().export.as_mut() {
            e.format = fmt;
            e.path = dir.join(name);
            assert!(e.load_all);
        }
        h.step();
        shot(&mut h, &format!("big-export-{name}"));
        // (Clear of the clicks before: else it counts as their double click.)
        for _ in 0..50 {
            h.step();
        }
        h.get_by_label_contains("Сохранить").click();
        h.step();
        let mut seen_progress = false;
        for _ in 0..6000 {
            h.step();
            let v = &h.state().rt[&id].results[0];
            seen_progress |= v.exporting.is_some_and(|n| n > 500);
            if v.cursor.is_none() && v.exporting.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let v = &h.state().rt[&id].results[0];
        assert!(v.cursor.is_none() && v.exporting.is_none(), "done");
        assert_eq!(v.table.rows.len(), 500, "the table kept its first batch only");
        assert!(seen_progress, "progress shown while writing");
        if fmt == crate::export::Format::Csv {
            let text = std::fs::read_to_string(dir.join(name)).unwrap();
            assert_eq!(text.lines().count(), 300_001);
            assert!(text.ends_with("300000,row 300000,450000\n"), "{}", &text[text.len() - 60..]);
        } else {
            assert!(std::fs::metadata(dir.join(name)).unwrap().len() > 1_000_000);
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn notebook_cells_with_and_export() {
    let text = "-- %% [markdown]\n-- # Крупные заказы\n--\n-- Заказы **дороже 900** и *три* самых больших.\n-- %%\nwith big as (select * from orders where total > 900)\n-- %%\nselect count(*) as n from big;\n-- %%\nwith top as (select * from big order by total desc limit 3)\nselect id, total from top;\n";
    let (mut h, _g) = harness("notebook", text, false);
    h.state_mut().toggle_notebook();
    h.step();
    let cells: Vec<String> = h.state().tab().notebook.as_ref().unwrap().cells.iter().map(|c| c.id.clone()).collect();
    assert_eq!(cells.len(), 4);
    let tab = h.state().tab().id.clone();
    // Everything, in order: the WITH cell only lends its CTE.
    let ctx = h.ctx.clone();
    h.state_mut().run_cells(&ctx, None);
    settle(&mut h);
    let key = |i: usize| crate::app::App::cell_key(&tab, &cells[i]);
    let want = (1..=3000u64).filter(|x| ((x * 37) % 1000) as f64 + 0.5 > 900.0).count().to_string();
    let rt2 = &h.state().rt[&key(2)];
    assert_eq!(rt2.results[0].table.rows[0][0].as_deref(), Some(want.as_str()), "{:?}", rt2.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    let rt3 = &h.state().rt[&key(3)];
    assert_eq!(rt3.results[0].table.rows.len(), 3, "{:?}", rt3.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert!(h.state().rt.get(&key(1)).is_none_or(|r| r.results.is_empty()), "a WITH cell gives nothing in a run of all");
    // Run alone, the WITH cell shows its CTE.
    h.state_mut().run_cells(&ctx, Some(cells[1].clone()));
    settle(&mut h);
    assert!(!h.state().rt[&key(1)].results.is_empty());
    shot(&mut h, "notebook");
    // Kept on disk only when pinned.
    assert!(h.state().tab().notebook.as_ref().unwrap().cells[2].saved.is_none());
    h.state_mut().pin_cell(&cells[2]);
    let saved = h.state().tab().notebook.as_ref().unwrap().cells[2].saved.clone().unwrap();
    assert_eq!(saved.tables[0].rows[0][0].as_deref(), Some(want.as_str()));
    // As one text and back: the marks, the kept output stays.
    h.state_mut().toggle_notebook();
    h.step();
    assert!(h.state().tab().text.contains("-- %%\nselect count(*) as n from big;"), "{}", h.state().tab().text);
    h.state_mut().toggle_notebook();
    h.step();
    assert!(h.state().tab().notebook.as_ref().unwrap().cells[2].saved.is_some(), "the kept output survives the round trip");
    // Every cell folded to its first line, then open again.
    h.get_by_label_contains("Свернуть все").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().tab().notebook.as_ref().unwrap().cells.iter().all(|c| c.collapsed));
    shot(&mut h, "notebook-folded");
    h.get_by_label_contains("Развернуть все").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().tab().notebook.as_ref().unwrap().cells.iter().all(|c| !c.collapsed));
    // Plain SQL: each statement with its CTEs.
    h.state_mut().export_notebook(true);
    h.step();
    let sql = h.state().tab().text.clone();
    assert!(sql.starts_with("-- # Крупные заказы"), "{sql}");
    assert!(sql.contains("WITH big as (select * from orders where total > 900)\nselect count(*) as n from big;"), "{sql}");
    assert!(sql.contains("WITH big as (select * from orders where total > 900),\ntop as ("), "{sql}");
}

#[test]
fn folds_and_marks() {
    let (mut h, _g) = harness_at("folds", ORA_STRINGS, false, 1.0);
    let ora = Spec { id: "ora".into(), name: "ora".into(), kind: Kind::Oracle, host: "127.0.0.1".into(), port: 1, save_password: false, ..Default::default() };
    h.state_mut().conns.list.push(ora);
    h.state_mut().tab_mut().conn = Some("ora".into());
    h.state_mut().ask = None;
    settle(&mut h);
    h.state_mut().ask = None;
    let id = h.state().tab().id.clone();
    // The FOR loop folded (its first line stays, "⋯ n" after it).
    let at = ORA_STRINGS.find("  for r in").unwrap();
    h.state_mut().rt.get_mut(&id).unwrap().ed.folds.push(at);
    // "orders" picked: the other "orders" marked too.
    let o = ORA_STRINGS.rfind("orders").unwrap();
    focus_editor(&mut h, o);
    {
        let tid = egui::Id::new(("editor", &id, false));
        let ctx = h.ctx.clone();
        let mut st = egui::text_edit::TextEditState::load(&ctx, tid).unwrap_or_default();
        let ci = ORA_STRINGS[..o].chars().count();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(ci), egui::text::CCursor::new(ci + 6))));
        st.store(&ctx, tid);
    }
    for _ in 0..4 {
        h.step();
    }
    shot(&mut h, "folds");
    assert!(h.state().rt[&id].ed.folds.contains(&at), "the fold stays while the caret's outside it");
    // A click on "⋯" opens it.
    click_at(&mut h, egui::pos2(548.0, 178.0), egui::PointerButton::Primary);
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().rt[&id].ed.folds.is_empty(), "the ⋯ opened it");
    // A bracket under the pointer: it and its pair marked.
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(437.0, 178.0)));
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "bracket-hover");
}

#[test]
fn wide_table_scrolls_sideways() {
    let cols: Vec<String> = (0..14).map(|k| format!("note || '-{k}-' || total as wide_column_{k}")).collect();
    let script = format!("select id, {} from orders;\n", cols.join(", "));
    let (mut h, _g) = harness("wide", &script, false);
    focus_editor(&mut h, 3);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    let id = h.state().tab().id.clone();
    assert_eq!(h.state().rt[&id].results[0].table.cols.len(), 15);
    {
        let v = &mut h.state_mut().rt.get_mut(&id).unwrap().results[0];
        v.sel = Some((4, 2));
        v.picked = [2usize, 3].into();
        v.rows_picked = [1usize, 2].into();
    }
    for _ in 0..2 {
        h.step();
    }
    shot(&mut h, "wide");
    h.state_mut().settings.theme = crate::config::ThemePref::Light;
    for _ in 0..4 {
        h.step();
    }
    shot(&mut h, "wide-light");
    h.state_mut().settings.theme = crate::config::ThemePref::Dark;
    for _ in 0..4 {
        h.step();
    }
    // Scrolled to the right: the last columns come into view.
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(700.0, 650.0)));
    h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(-3000.0, 0.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
    for _ in 0..6 {
        h.step();
    }
    shot(&mut h, "wide-right");
    // The numbers stay at the left edge: a click there picks the row.
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(298.0, 637.0)));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: egui::pos2(298.0, 637.0), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.step();
    let v = &h.state().rt[&id].results[0];
    assert_eq!(v.rows_picked, [0usize].into(), "a click on the pinned number picks its row");
    // A cell: chosen by a click.
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(420.0, 685.0)));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: egui::pos2(420.0, 685.0), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.step();
    assert_eq!(h.state().rt[&id].results[0].sel.map(|s| s.0), Some(2), "a click on a cell chooses it");
    shot(&mut h, "cell-clicked");
    // The sort square opens under the mouse: ▲ ▼; ▼ sorts descending.
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(587.0, 612.0)));
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "sort-open");
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(589.0, 612.0)));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: egui::pos2(589.0, 612.0), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.step();
    assert_eq!(h.state().rt[&id].results[0].sort, vec![(10usize, false)], "▼ in the opened square");
    // The header picks by a click anywhere on it, not just its name.
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(725.0, 618.0)));
    for _ in 0..3 {
        h.step();
    }
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: egui::pos2(725.0, 618.0), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.step();
    assert_eq!(h.state().rt[&id].results[0].picked, [11usize].into(), "a click past the name picks the column");
    // The rows' own scroll bar, at the right of the room even when the
    // table is wider: dragged to the bottom, the last rows show.
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(1261.0, 640.0)));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: egui::pos2(1261.0, 640.0), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    for y in [680.0, 720.0, 790.0] {
        h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(1261.0, y)));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: egui::pos2(1261.0, 790.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "vbar-bottom");
}

#[test]
fn notebook_output_pulled_up_and_hand_cursor() {
    let script = "-- %% [markdown]\n-- # Заголовок\n--\n-- текст\n-- %%\nselect id, note, total from orders;\n-- %%\nselect 1;\n";
    let (mut h, _g) = harness("nb-pull", script, false);
    h.state_mut().toggle_notebook();
    h.step();
    let ctx = h.ctx.clone();
    h.state_mut().run_cells(&ctx, None);
    settle(&mut h);
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "nb-pull-before");
    let label = h.get_by_label_contains("(500+)").rect();
    // The edge sits just over the output's tabs.
    let edge = egui::pos2(label.center().x + 300.0, label.top() - 6.0);
    let drag = |h: &mut Harness<'static, App>, from: egui::Pos2, to: egui::Pos2| {
        h.input_mut().events.push(egui::Event::PointerMoved(from));
        h.step();
        h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.step();
        for k in 1..=5 {
            h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (k as f32 / 5.0)));
            h.step();
        }
        h.input_mut().events.push(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.step();
        h.step();
    };
    drag(&mut h, edge, edge - egui::vec2(0.0, 100.0));
    let out_h = h.state().tab().notebook.as_ref().unwrap().cells[1].out_h;
    // The edge went up with the mouse (the notebook scrolled along).
    let moved = h.get_by_label_contains("(500+)").rect();
    assert!((moved.top() - (label.top() - 100.0)).abs() < 12.0, "{moved:?} vs {label:?}");
    assert!((out_h - 400.0).abs() < 12.0, "pulled up by 100 from 300: {out_h}");
    shot(&mut h, "nb-pull-after");
    // A hand over buttons, the arrow over the table's cells.
    let cursor = |h: &mut Harness<'static, App>, at: egui::Pos2| {
        h.input_mut().events.push(egui::Event::PointerMoved(at));
        h.step();
        h.step();
        h.output().platform_output.cursor_icon
    };
    let run_all = h.get_by_label_contains("Выполнить все").rect().center();
    assert_eq!(cursor(&mut h, run_all), egui::CursorIcon::PointingHand);
    let num = h.get_by_label_contains("(500+)").rect().center();
    assert_eq!(cursor(&mut h, num), egui::CursorIcon::PointingHand, "the output's tab");
    let cell = h.get_by_label_contains("(500+)").rect().center() + egui::vec2(0.0, 150.0);
    assert_eq!(cursor(&mut h, cell), egui::CursorIcon::Default, "a table cell");
}

#[test]
fn notebook_scrolled_keeps_shades_inside() {
    let script = "-- %%\nselect id, note, total from orders;\n-- %%\nselect id, total from orders;\n";
    let (mut h, _g) = harness("nb-shades", script, false);
    h.state_mut().toggle_notebook();
    h.step();
    let ctx = h.ctx.clone();
    h.state_mut().run_cells(&ctx, None);
    settle(&mut h);
    let tab = h.state().tab().id.clone();
    let cells: Vec<String> = h.state().tab().notebook.as_ref().unwrap().cells.iter().map(|c| c.id.clone()).collect();
    {
        let v = &mut h.state_mut().rt.get_mut(&crate::app::App::cell_key(&tab, &cells[0])).unwrap().results[0];
        v.sel = Some((0, 1));
        v.picked = [2usize].into();
    }
    for _ in 0..3 {
        h.step();
    }
    let before = h.render().expect("render");
    // The lower output's top edge pulled up: the notebook scrolls the upper
    // table out over the top.
    let labels: Vec<egui::Rect> = h.query_all_by_label_contains("(500+)").map(|n| n.rect()).collect();
    let lower = labels.iter().copied().max_by(|a, b| a.top().total_cmp(&b.top())).unwrap();
    let from = egui::pos2(lower.center().x + 300.0, lower.top() - 6.0);
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    for k in 1..=8 {
        h.input_mut().events.push(egui::Event::PointerMoved(from - egui::vec2(0.0, 40.0 * k as f32)));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: from - egui::vec2(0.0, 320.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "nb-shades");
    let after = h.render().expect("render");
    // Over the notebook (the toolbars, the tabs) nothing changed.
    let bar = h.get_by_label_contains("Выполнить все").rect();
    let ppp = after.width() as f32 / 1280.0;
    let bottom = ((bar.bottom() + 2.0) * ppp) as u32;
    let x0 = (bar.left() * ppp) as u32;
    let mut differ = 0;
    for y in 0..bottom {
        for x in x0..after.width() {
            if before.get_pixel(x, y) != after.get_pixel(x, y) {
                differ += 1;
            }
        }
    }
    assert!(differ < 50, "{differ} pixels changed over the notebook");
}

#[test]
fn notebook_markdown_view_edit_switch() {
    let script = "-- %% [markdown]\n-- # Отчёт\n--\n-- первый **жирный** абзац\n-- %%\nselect 1;\n";
    let (mut h, _g) = harness("nb-md", script, false);
    h.state_mut().toggle_notebook();
    for _ in 0..2 {
        h.step();
    }
    let tab = h.state().tab().id.clone();
    let cell = h.state().tab().notebook.as_ref().unwrap().cells[0].id.clone();
    let editing = |h: &Harness<'static, App>| h.state().rt.get(&tab).is_some_and(|r| r.md_edit.contains(&cell));
    assert!(!editing(&h));
    h.get_by_label("Правка").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(editing(&h), "the switch opens the text");
    shot(&mut h, "nb-md-edit");
    // Typed into (the caret went in by itself).
    key(&mut h, egui::Key::End, egui::Modifiers::COMMAND);
    type_text(&mut h, "\nещё строка");
    for _ in 0..2 {
        h.step();
    }
    assert!(h.state().tab().notebook.as_ref().unwrap().cells[0].text.ends_with("ещё строка"), "{}", h.state().tab().notebook.as_ref().unwrap().cells[0].text);
    // A click elsewhere doesn't close it; the switch does.
    h.get_by_label_contains("Выполнить все").hover();
    h.step();
    let away = egui::pos2(900.0, 700.0);
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: away, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    assert!(editing(&h), "still edited after a click away");
    h.get_by_label("Просмотр").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(!editing(&h), "back to the formatted view");
    shot(&mut h, "nb-md-view");
}

#[test]
fn block_selection_with_alt() {
    let (mut h, _g) = harness("block", "select a,\n       bb,\n       c\n  from orders;\n", false);
    focus_editor(&mut h, 0);
    let id = h.state().tab().id.clone();
    let r = h.state().rt[&id].ed.text_rect.unwrap();
    let font = egui::FontId::monospace(h.state().settings.font_size);
    let (cw, rh) = h.ctx.fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    let at = |line: f32, col: f32| egui::pos2(r.left() + col * cw, r.top() + 4.0 + (line + 0.5) * rh);
    // Alt held: from line 1 col 7 down to line 3.
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
    h.input_mut().events.push(egui::Event::PointerMoved(at(0.0, 7.0)));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: at(0.0, 7.0), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::ALT });
    h.step();
    for l in [0.7, 1.4, 2.0] {
        h.input_mut().events.push(egui::Event::PointerMoved(at(l, 7.0)));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: at(2.0, 7.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::ALT });
    h.step();
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.step();
    assert_eq!(h.state().rt[&id].ed.block.map(|b| (b.lines(), b.cols())), Some(((0, 2), (7, 7))));
    // Typed on every line at once, then a Backspace on every line.
    type_text(&mut h, "o.");
    assert_eq!(h.state().tab().text, "select o.a,\n       o.bb,\n       o.c\n  from orders;\n");
    key(&mut h, egui::Key::Backspace, egui::Modifiers::NONE);
    assert_eq!(h.state().tab().text, "select oa,\n       obb,\n       oc\n  from orders;\n");
    h.step();
    let caret = h.state().rt[&id].ed.cursor;
    assert_eq!(caret, Some((31, 31)), "the text's caret at the block's head");
    shot(&mut h, "block");
    // Esc: back to one caret, typing goes to one place.
    key(&mut h, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(h.state().rt[&id].ed.block.is_none());
}

#[test]
fn wheel_scrolls_while_selecting() {
    let script: String = (1..=120).map(|i| format!("select {i} as n;\n")).collect();
    let (mut h, _g) = harness("wheel-sel", &script, false);
    focus_editor(&mut h, 0);
    let id = h.state().tab().id.clone();
    let r = h.state().rt[&id].ed.text_rect.unwrap();
    let p = egui::pos2(r.left() + 40.0, r.top() + 12.0);
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    // A small move makes it a drag; then the wheel, the pointer still.
    h.input_mut().events.push(egui::Event::PointerMoved(p + egui::vec2(0.0, 30.0)));
    h.step();
    for _ in 0..4 {
        h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -300.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
        h.step();
    }
    for _ in 0..20 {
        h.step();
    }
    let (a, b) = h.state().rt[&id].ed.cursor.unwrap();
    let text = h.state().tab().text.clone();
    let line = |x: usize| text[..x].matches('\n').count();
    assert!(line(a.max(b)) > 40, "the selection reached line {}", line(a.max(b)));
    assert_eq!(line(a.min(b)), 0);
    h.input_mut().events.push(egui::Event::PointerButton { pos: p + egui::vec2(0.0, 30.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.step();
}

#[test]
fn block_rectangle_cut() {
    let (mut h, _g) = harness("block-rect", "select id,\n       note,\n       total\n  from orders;\n", false);
    focus_editor(&mut h, 0);
    let id = h.state().tab().id.clone();
    let r = h.state().rt[&id].ed.text_rect.unwrap();
    let font = egui::FontId::monospace(h.state().settings.font_size);
    let (cw, rh) = h.ctx.fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    let at = |line: f32, col: f32| egui::pos2(r.left() + col * cw, r.top() + 4.0 + (line + 0.5) * rh);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
    h.input_mut().events.push(egui::Event::PointerMoved(at(0.0, 0.0)));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: at(0.0, 0.0), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::ALT });
    h.step();
    for (l, c) in [(0.5, 3.0), (1.5, 6.0), (2.0, 7.0)] {
        h.input_mut().events.push(egui::Event::PointerMoved(at(l, c)));
        h.step();
    }
    shot(&mut h, "block-rect");
    h.input_mut().events.push(egui::Event::PointerButton { pos: at(2.0, 7.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::ALT });
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.step();
    h.step();
    h.input_mut().events.push(egui::Event::Cut);
    h.step();
    h.step();
    assert_eq!(h.state().tab().text, "id,\nnote,\ntotal\n  from orders;\n");
}

pub const DEEP: &str = "begin\n  for a in 1..2 loop\n    for b in 1..2 loop\n      if a > b then\n        for c in (select x\n                    from (select y as x\n                            from (select z as y\n                                    from t\n                                   where z in (select q\n                                                 from u\n                                                where q > 0))))\n        loop\n          if c.x > 0 then\n            null;\n          end if;\n        end loop;\n      end if;\n    end loop;\n  end loop;\nend;\n";

/// Columns of the sheet's colour between two bands' colours, `rows` tall
/// or more (a hairline gap), in the picture's rect `area` (points).
fn band_seams(img: &image::RgbaImage, k: f32, area: egui::Rect, rows: usize) -> Vec<(u32, usize)> {
    let bg = *img.get_pixel((area.right() * k) as u32 - 2, (area.bottom() * k) as u32 - 2);
    let band = |p: &image::Rgba<u8>| *p != bg && p.0[..3].iter().max().unwrap() < &140 && p.0[..3].iter().max().unwrap() - p.0[..3].iter().min().unwrap() < 40 && p.0[..3].iter().map(|&v| v as u32).sum::<u32>() > 150;
    let mut hits = std::collections::HashMap::<u32, usize>::new();
    for y in (area.top() * k) as u32..(area.bottom() * k) as u32 {
        for x in (area.left() * k) as u32 + 1..(area.right() * k) as u32 - 1 {
            if *img.get_pixel(x, y) == bg && band(img.get_pixel(x - 1, y)) && band(img.get_pixel(x + 1, y)) {
                *hits.entry(x).or_default() += 1;
            }
        }
    }
    let mut v: Vec<(u32, usize)> = hits.into_iter().filter(|&(_, n)| n >= rows).collect();
    v.sort_unstable();
    v
}

#[test]
fn deep_bands_have_no_gaps_at_any_scale() {
    for k in [1.0f32, 1.25, 1.5] {
        let (mut h, _g) = harness_at("deep", DEEP, false, k);
        let ora = Spec { id: "ora".into(), name: "ora".into(), kind: Kind::Oracle, host: "127.0.0.1".into(), port: 1, save_password: false, ..Default::default() };
        h.state_mut().conns.list.push(ora);
        h.state_mut().tab_mut().conn = Some("ora".into());
        h.state_mut().ask = None;
        settle(&mut h);
        h.state_mut().ask = None;
        for _ in 0..3 {
            h.step();
        }
        shot(&mut h, &format!("deep-bands-{k}"));
        let id = h.state().tab().id.clone();
        let r = h.state().rt[&id].ed.text_rect.unwrap();
        let img = h.render().unwrap();
        let area = egui::Rect::from_min_max(r.min, egui::pos2(r.left() + 420.0, r.top() + 320.0));
        let seams = band_seams(&img, k, area, 12);
        assert!(seams.is_empty(), "scale {k}: gaps between bands at {seams:?}");
    }
}

/// Ctrl+click on a node of the tree (the label left-most of that name).
fn ctrl_click_tree(h: &mut Harness<'static, App>, name: &str) {
    let p = h.get_all_by_label(name).map(|n| n.rect().center()).min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    let p = egui::pos2(p.x as f32, p.y as f32);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::COMMAND });
        h.step();
    }
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.step();
}

#[test]
fn table_designer_beside_on_ctrl_click() {
    let (mut h, _g) = harness("designer", SCRIPT, false);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "lite", None);
    for p in [vec![], vec!["tables".to_string()]] {
        h.state_mut().open_nodes.insert(("lite".into(), p));
        settle(&mut h);
    }
    ctrl_click_tree(&mut h, "orders");
    settle(&mut h);
    // Beside the first tab, the designer with the table's columns.
    let titles: Vec<String> = h.state().ws().tabs.iter().map(|t| t.title.clone()).collect();
    assert_eq!(titles[1], "▦ orders", "{titles:?}");
    assert_eq!(h.state().ws().active, 1);
    let id = h.state().tab().id.clone();
    let cols = |h: &Harness<'static, App>| match h.state().rt[&id].design.as_ref().map(|d| &d.loaded) {
        Some(crate::design_ui::Loaded::Table(te)) => te.rows.iter().map(|r| r.col.name.clone()).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    assert_eq!(cols(&h), ["id", "customer_id", "total", "note"]);
    shot(&mut h, "designer-table");
    // A column added by hand: its ALTER, applied.
    h.get_by_label_contains("Колонка").click();
    h.step();
    if let Some(crate::design_ui::Loaded::Table(te)) = h.state_mut().rt.get_mut(&id).unwrap().design.as_mut().map(|d| &mut d.loaded) {
        te.rows.last_mut().unwrap().col.name = "shipped".into();
    }
    settle(&mut h);
    let script = h.state().rt[&id].design.as_ref().unwrap().script();
    assert_eq!(script, "ALTER TABLE orders ADD COLUMN shipped INTEGER;\n");
    shot(&mut h, "designer-table-changed");
    h.get_by_label_contains("Применить").click();
    for _ in 0..50 {
        settle(&mut h);
        if cols(&h).len() == 5 && !h.state().rt[&id].design.as_ref().unwrap().busy {
            break;
        }
    }
    assert_eq!(cols(&h), ["id", "customer_id", "total", "note", "shipped"]);
    assert!(h.state().rt[&id].design.as_ref().unwrap().script().is_empty());
    // Its code a switch away.
    h.get_by_label_contains("Код").click();
    settle(&mut h);
    assert!(h.state().tab().text.contains("CREATE TABLE orders") || h.state().tab().text.contains("create table orders"), "{}", h.state().tab().text);
    assert!(h.state().tab().text.contains("shipped"), "{}", h.state().tab().text);
    shot(&mut h, "designer-table-code");
    // Ctrl+click on it again: that tab, not a new one.
    h.state_mut().ws_mut().active = 0;
    ctrl_click_tree(&mut h, "orders");
    settle(&mut h);
    assert_eq!(h.state().ws().tabs.len(), 3);
    assert_eq!(h.state().ws().active, 1);
    // Another kind of object: its code, in a tab beside.
    h.state_mut().open_nodes.insert(("lite".into(), vec!["views".into()]));
    settle(&mut h);
    ctrl_click_tree(&mut h, "big_orders");
    settle(&mut h);
    assert_eq!(h.state().ws().active, 2);
    assert_eq!(h.state().tab().title, "big_orders");
    assert!(h.state().tab().object.is_none());
    assert!(h.state().tab().text.to_lowercase().contains("create view big_orders"), "{}", h.state().tab().text);
}

#[test]
fn oracle_job_in_a_tab() {
    let Ok(v) = std::env::var("ZAPROS_TEST_ORA") else { return };
    let p: Vec<String> = v.split(':').map(String::from).collect();
    let (mut h, _g) = harness("orajob", "", false);
    let ora = Spec { id: "ora".into(), name: "ora".into(), kind: Kind::Oracle, host: p[0].clone(), port: p[1].parse().unwrap(), database: p[2].clone(), user: p[3].clone(), save_password: true, ..Default::default() };
    crate::secrets::set("ora", &p[4]).unwrap();
    h.state_mut().conns.list.push(ora);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "ora", None);
    settle(&mut h);
    let owner = p[3].to_uppercase();
    // (One left over from a run before goes first.)
    if let Some(l) = h.state().live.get("ora") {
        l.meta.send(crate::db::Req::Apply { key: "none".into(), stmts: vec!["BEGIN DBMS_SCHEDULER.DROP_JOB('ZAPROS_TEST_JOB'); EXCEPTION WHEN OTHERS THEN NULL; END;".into()] });
    }
    // A new job, in a tab beside.
    h.state_mut().new_job("ora", vec![owner.clone(), "jobs".into()]);
    settle(&mut h);
    let id = h.state().tab().id.clone();
    fn job<'a>(h: &'a mut Harness<'static, App>, id: &str) -> &'a mut crate::design_ui::JobEdit {
        match h.state_mut().rt.get_mut(id).and_then(|r| r.design.as_mut()).map(|d| &mut d.loaded) {
            Some(crate::design_ui::Loaded::Job(je)) => je,
            _ => panic!("no job designer"),
        }
    }
    job(&mut h, &id).cur.name = "ZAPROS_TEST_JOB".into();
    job(&mut h, &id).cur.enabled = false;
    settle(&mut h);
    shot(&mut h, "job-new");
    let wait = |h: &mut Harness<'static, App>| {
        for _ in 0..100 {
            settle(h);
            let d = h.state().rt[&id].design.as_ref().unwrap();
            if !d.busy && matches!(&d.loaded, crate::design_ui::Loaded::Job(je) if je.orig.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    h.get_by_label_contains("Применить").click();
    wait(&mut h);
    let d = h.state().rt[&id].design.as_ref().unwrap();
    assert!(matches!(d.status, Some(Ok(_))), "{:?}", d.status);
    assert_eq!(h.state().tab().title, "⏱ ZAPROS_TEST_JOB");
    assert_eq!(h.state().tab().object.as_ref().unwrap().path, vec![owner.clone(), "jobs".into(), "ZAPROS_TEST_JOB".into()]);
    assert_eq!(job(&mut h, &id).orig.as_ref().unwrap().schedule, "FREQ=DAILY;BYHOUR=3;BYMINUTE=0;BYSECOND=0");
    // Weekly on Monday and Friday at 09:30, by the designer.
    {
        let je = job(&mut h, &id);
        let p = je.plan.as_mut().unwrap();
        p.freq = crate::sched::Freq::Weekly;
        p.days = [true, false, false, false, true, false, false];
        p.hours = vec![9];
        p.minutes = vec![30];
        je.cur.schedule = p.oracle();
    }
    settle(&mut h);
    shot(&mut h, "job-weekly");
    let sql = h.state().rt[&id].design.as_ref().unwrap().script();
    assert!(sql.contains("'repeat_interval', 'FREQ=WEEKLY;BYDAY=MON,FRI;BYHOUR=9;BYMINUTE=30;BYSECOND=0')"), "{sql}");
    h.get_by_label_contains("Применить").click();
    for _ in 0..100 {
        settle(&mut h);
        if job(&mut h, &id).orig.as_ref().unwrap().schedule.starts_with("FREQ=WEEKLY") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = format!("{:?}", h.state().rt[&id].design.as_ref().unwrap().status);
    assert_eq!(job(&mut h, &id).orig.as_ref().unwrap().schedule, "FREQ=WEEKLY;BYDAY=MON,FRI;BYHOUR=9;BYMINUTE=30;BYSECOND=0", "{status}");
    // Its code a switch away.
    h.get_by_label("{ } Код").click();
    settle(&mut h);
    assert!(h.state().tab().text.to_uppercase().contains("DBMS_SCHEDULER.CREATE_JOB"), "{}", h.state().tab().text);
    shot(&mut h, "job-code");
    if let Some(l) = h.state().live.get("ora") {
        l.meta.send(crate::db::Req::Apply { key: "none".into(), stmts: vec!["BEGIN DBMS_SCHEDULER.DROP_JOB('ZAPROS_TEST_JOB'); END;".into()] });
    }
    settle(&mut h);
}

#[test]
fn schemes_and_backdrop_gradient() {
    let (mut h, _g) = harness("look", SCRIPT, false);
    h.state_mut().settings_open = true;
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), 5usize));
    settle(&mut h);
    shot(&mut h, "look-settings");
    h.state_mut().settings_open = false;
    for (id, grad) in [("dracula", Some(0usize)), ("win11-light", None), ("ubuntu", Some(4)), ("nord", Some(2))] {
        {
            let s = &mut h.state_mut().settings;
            s.scheme = id.into();
            let sc = crate::themes::find(id).unwrap();
            if sc.dark { s.dark = sc.palette() } else { s.light = sc.palette() }
            s.backdrop = grad.map_or_else(crate::themes::Backdrop::default, crate::themes::preset);
        }
        settle(&mut h);
        assert_eq!(h.ctx.global_style().visuals.panel_fill, egui::Color32::from_rgb(crate::themes::find(id).unwrap().bg[0], crate::themes::find(id).unwrap().bg[1], crate::themes::find(id).unwrap().bg[2]));
        shot(&mut h, &format!("look-{id}"));
        // The gradient shows round the sheet: the top left differs from the bottom left.
        let img = h.render().unwrap();
        let (a, b) = (*img.get_pixel(3, 790), *img.get_pixel(1276, 3));
        assert_eq!(a != b, grad.is_some(), "{id}: {a:?} {b:?}");
    }
}

/// Ctrl+click at a line / column of the editor.
fn ctrl_click_text(h: &mut Harness<'static, App>, line: usize, col: usize) {
    let id = h.state().tab().id.clone();
    let r = h.state().rt[&id].ed.text_rect.unwrap();
    let font = egui::FontId::monospace(h.state().settings.font_size);
    let (cw, rh) = h.ctx.fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    let p = egui::pos2(r.left() + (col as f32 + 0.5) * cw, r.top() + 4.0 + (line as f32 + 0.5) * rh);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    h.step();
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::COMMAND });
        h.step();
    }
}

#[test]
fn ctrl_click_in_the_code() {
    let src = "select o.total, c.name\n  from orders o\n  join customers c on c.id = o.customer_id;\n";
    let (mut h, _g) = harness("follow", src, false);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "lite", None);
    settle(&mut h);
    focus_editor(&mut h, 0);
    // The alias `c` in `c.name` → `customers` picked.
    ctrl_click_text(&mut h, 0, 16);
    h.step();
    shot(&mut h, "follow-alias");
    let id = h.state().tab().id.clone();
    let cur = h.state().rt[&id].ed.cursor.unwrap();
    let (a, b) = (cur.0.min(cur.1), cur.0.max(cur.1));
    assert_eq!(&src[a..b], "customers");
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    settle(&mut h);
    // The table's name → its designer in a tab beside.
    ctrl_click_text(&mut h, 1, 9);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    settle(&mut h);
    assert_eq!(h.state().tab().title, "▦ orders");
    assert!(h.state().tab().object.is_some());
    // A name the database doesn't have: said so.
    h.state_mut().ws_mut().active = 0;
    h.state_mut().tab_mut().text = "select nosuch(1);\n".into();
    settle(&mut h);
    ctrl_click_text(&mut h, 0, 8);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    settle(&mut h);
    assert!(h.state().toast.as_ref().is_some_and(|(m, _)| m.contains("nosuch")), "{:?}", h.state().toast);
}

#[test]
fn ctrl_click_opens_a_function() {
    if std::env::var("ZAPROS_TEST_PG").is_err() {
        return;
    }
    let (mut h, _g) = harness("follow-pg", "select vip(1);\n", true);
    h.state_mut().ws_mut().conn = Some("pg".into());
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "pg", None);
    settle(&mut h);
    focus_editor(&mut h, 0);
    ctrl_click_text(&mut h, 0, 8);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    for _ in 0..20 {
        settle(&mut h);
        if h.state().ws().tabs.len() > 2 {
            break;
        }
    }
    assert_eq!(h.state().tab().title, "vip");
    assert!(h.state().tab().text.to_lowercase().contains("function public.vip"), "{}", h.state().tab().text);
    shot(&mut h, "follow-function");
}

#[test]
fn icon_packs_in_the_tree_and_toolbar() {
    let (mut h, _g) = harness("icons", SCRIPT, false);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "lite", None);
    for p in [vec![], vec!["tables".to_string()], vec!["tables".into(), "orders".into()], vec!["views".into()], vec!["indexes".into()], vec!["triggers".into()]] {
        h.state_mut().open_nodes.insert(("lite".into(), p));
        settle(&mut h);
    }
    h.state_mut().settings_open = true;
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), 5usize));
    settle(&mut h);
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(300.0, 300.0)));
    h.step();
    for _ in 0..4 {
        h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -120.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
        h.step();
    }
    settle(&mut h);
    shot(&mut h, "icons-settings");
    h.state_mut().settings_open = false;
    for (id, ..) in crate::icons_data::PACKS {
        h.state_mut().settings.icons = id.to_string();
        settle(&mut h);
        shot(&mut h, &format!("icons-{id}"));
    }
    // No line at the side panel's edge under the pointer: the edge looks
    // the same with the pointer there as without.
    let x = h.state().session.side_width;
    let row = |img: &image::RgbaImage| (x as u32 - 6..x as u32 + 6).map(|px| *img.get_pixel(px, 600)).collect::<Vec<_>>();
    let before = row(&h.render().unwrap());
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(x, 400.0)));
    for _ in 0..4 {
        h.step();
    }
    shot(&mut h, "edge-hover");
    assert_eq!(row(&h.render().unwrap()), before, "a line at the panel's edge");
    // The symbols as before.
    h.state_mut().settings.icons.clear();
    settle(&mut h);
    shot(&mut h, "icons-none");
}

#[test]
fn keywords_take_the_case_while_typing() {
    let (mut h, _g) = harness("autocase", "", false);
    focus_editor(&mut h, 0);
    type_text(&mut h, "select id, if from orders o where o.from in (1) ");
    // Keywords up; names, x.word and the like as typed.
    assert_eq!(h.state().tab().text, "SELECT id, if FROM orders o WHERE o.from IN (1) ");
    // Off: as typed.
    h.state_mut().settings.format.case_while_typing = false;
    h.state_mut().tab_mut().text.clear();
    focus_editor(&mut h, 0);
    type_text(&mut h, "select 1 ");
    assert_eq!(h.state().tab().text, "select 1 ");
    // The style page.
    h.state_mut().settings_open = true;
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), 3usize));
    h.state_mut().settings.format.river = false;
    h.state_mut().settings.format.leading_commas = false;
    h.state_mut().settings.format.indent = 4;
    settle(&mut h);
    shot(&mut h, "code-style");
}

#[test]
fn fonts_for_each_place() {
    let (mut h, _g) = harness("fonts", SCRIPT, false);
    assert!(crate::fonts::families().iter().any(|f| f.1), "monospaced families found");
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    {
        let f = &mut h.state_mut().settings.fonts;
        f.ui = "DejaVu Serif".into();
        f.ui_size = 14.0;
        f.code = "Liberation Mono".into();
        f.table = "DejaVu Sans".into();
        f.table_size = 13.0;
        f.code_spacing = 4.0;
    }
    settle(&mut h);
    shot(&mut h, "fonts-applied");
    assert_eq!(h.ctx.global_style().text_styles[&egui::TextStyle::Body].size, 14.0);
    // A code font without the interface's shapes (as a Nerd Font may be):
    // they come from the fallbacks, never an empty square.
    h.state_mut().settings.fonts.code = "JetBrainsMono Nerd Font Mono".into();
    h.state_mut().settings.fonts.ui = String::new();
    settle(&mut h);
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace, egui::FontFamily::Name("bold".into()), egui::FontFamily::Name("table".into())] {
        // (A char no font has comes out as the replacement square: a shape
        // drawn differently from that is really there.)
        let uv = |h: &Harness<'static, App>, c: char| {
            let g = h.ctx.fonts_mut(|f| f.layout_no_wrap(c.to_string(), egui::FontId::new(13.0, fam.clone()), egui::Color32::WHITE));
            g.rows[0].glyphs[0].uv_rect
        };
        let missing = uv(&h, '\u{10FFFD}');
        for ch in ['▦', '↵', '●', '▸', 'ж'] {
            assert_ne!(uv(&h, ch), missing, "{ch} in {fam:?} is the empty square");
        }
    }
    h.state_mut().settings_open = true;
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), 5usize));
    settle(&mut h);
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(300.0, 300.0)));
    h.step();
    for _ in 0..7 {
        h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -120.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
        h.step();
    }
    settle(&mut h);
    shot(&mut h, "fonts-settings");
}

#[test]
fn stopwatch_by_the_spinner() {
    assert_eq!(crate::app::stopwatch(Duration::from_millis(3_450)), "0:03.4");
    assert_eq!(crate::app::stopwatch(Duration::from_millis(725_000)), "12:05.0");
    assert_eq!(crate::app::stopwatch(Duration::from_secs(3_723)), "1:02:03");
    if std::env::var("ZAPROS_TEST_PG").is_err() {
        return;
    }
    let (mut h, _g) = harness("watch", "select pg_sleep(2);\n", true);
    h.state_mut().ws_mut().conn = Some("pg".into());
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "pg", None);
    settle(&mut h);
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    for _ in 0..40 {
        h.step();
        std::thread::sleep(Duration::from_millis(30));
    }
    // On the toolbar and on the tab.
    let n = h.query_all_by_label_contains("0:01.").count() + h.query_all_by_label_contains("0:00.").count();
    assert_eq!(n, 2, "stopwatches running");
    shot(&mut h, "stopwatch");
    settle(&mut h);
}

#[test]
fn markdown_menu_and_new_marks() {
    let (mut h, _g) = harness("mdmenu", "", false);
    h.state_mut().tab_mut().text = "-- %% [markdown]\n-- ## Plan\n-- - [ ] first\n-- - [x] second\n--\n-- ~~old~~ and **new**\n--\n-- | name | rows |\n-- | --- | --- |\n-- | orders | 3000 |\n-- %%\nselect 1;\n".into();
    let d = crate::sql::Dialect::Sqlite;
    h.state_mut().tab_mut().set_notebook(true, d);
    settle(&mut h);
    shot(&mut h, "md-marks");
    // Editing: a right click on the text, the menu.
    let cell = h.state().tab().notebook.as_ref().unwrap().cells.iter().find(|c| c.kind == crate::config::CellKind::Markdown).map(|c| c.id.clone());
    let cell = cell.expect("a Markdown cell");
    let id = h.state().tab().id.clone();
    h.state_mut().rt.entry(id).or_default().md_edit.insert(cell.clone());
    settle(&mut h);
    let r = h.ctx.read_response(egui::Id::new(("md", &cell))).expect("the text being edited").rect;
    click_at(&mut h, r.left_top() + egui::vec2(40.0, 10.0), egui::PointerButton::Secondary);
    settle(&mut h);
    shot(&mut h, "md-menu");
    assert!(h.query_by_label("Таблица").is_some() && h.query_by_label("Чек-лист").is_some());
}

/// Started on a session whose first frame draws bold Markdown, a table's
/// cells, keywords in bold: every font family is there from the start.
#[test]
fn starts_on_a_notebook_with_bold_markdown() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = dir("start-bold");
    unsafe {
        std::env::set_var("ZAPROS_CONFIG", d.join("config"));
        std::env::set_var("ZAPROS_DATA", d.join("data"));
        std::env::set_var("ZAPROS_NO_KEYRING", "1");
    }
    let nb = config::Notebook {
        on: true,
        cells: vec![
            config::Cell { id: "m".into(), kind: config::CellKind::Markdown, text: "# Report\n**bold** and *italic*\n\n| a | b |\n| --- | --- |\n| **x** | 1 |".into(), ..Default::default() },
            config::Cell { id: "s".into(), kind: config::CellKind::Sql, text: "select 1".into(), ..Default::default() },
        ],
    };
    let session = Session { workspaces: vec![Workspace { name: "w".into(), tabs: vec![Tab { title: "nb".into(), notebook: Some(nb), ..Default::default() }], ..Default::default() }], ..Default::default() };
    config::save(&d.join("data/session.json"), &session).unwrap();
    let mut h = Harness::builder().with_size([1280.0, 800.0]).wgpu().build_eframe(|cc| App::new(cc));
    for _ in 0..5 {
        h.step();
    }
    assert!(h.state().tab().is_notebook());
}

#[test]
fn hints_with_the_word_as_typed_first() {
    let (mut h, _g) = harness("typed-first", "", false);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "lite", None);
    settle(&mut h);
    h.state_mut().settings.hints_typed_first = true;
    h.state_mut().settings.format.case_while_typing = false;
    focus_editor(&mut h, 0);
    // "o" opens hints (orders…); Enter: a new line, the word as typed.
    type_text(&mut h, "select * from o");
    settle(&mut h);
    let id = h.state().tab().id.clone();
    assert!(h.state().rt[&id].ed.popup.is_some(), "hints open");
    shot(&mut h, "typed-first");
    key(&mut h, egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(h.state().tab().text, "select * from o\n");
    assert!(h.state().rt[&id].ed.popup.is_none());
    // ↓ then Enter: the hint.
    type_text(&mut h, "from ord");
    settle(&mut h);
    key(&mut h, egui::Key::ArrowDown, egui::Modifiers::NONE);
    key(&mut h, egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(h.state().tab().text, "select * from o\nfrom orders");
    // Off: Enter takes the first hint, as before.
    h.state_mut().settings.hints_typed_first = false;
    type_text(&mut h, " join cust");
    settle(&mut h);
    key(&mut h, egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(h.state().tab().text, "select * from o\nfrom orders join customers");
    // The panel's icons centred on it, past its button at the corner.
    let w = h.state().session.side_width;
    let from = h.get_by_label("Скрыть боковую панель").rect().right() + 4.0;
    let first = h.get_by_label("Подключения и объекты баз").rect();
    let last = h.get_by_label("Клавиши и о программе").rect();
    let (l, r) = (first.left() as f32, last.right() as f32);
    assert!(((l - from) - (w - 6.0 - r)).abs() < 3.0, "centred: {l} … {r} in {from} … {w}");
}

#[test]
fn notebook_cells_dragged_formatted_copied_timed() {
    let (mut h, _g) = harness("nbdrag", "-- %%\nselect 1 as a\n-- %%\nselect 2 as b\n-- %%\nselect 3 as c\n", false);
    h.state_mut().tab_mut().set_notebook(true, crate::sql::Dialect::Sqlite);
    settle(&mut h);
    let texts = |h: &Harness<'static, App>| h.state().tab().notebook.as_ref().unwrap().cells.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(&h), ["select 1 as a", "select 2 as b", "select 3 as c"]);
    // The first cell's grip dragged below the last cell.
    let grips: Vec<egui::Rect> = h.query_all_by_label("⠿").map(|n| { let r = n.rect(); egui::Rect::from_min_max(egui::pos2(r.min.x as f32, r.min.y as f32), egui::pos2(r.max.x as f32, r.max.y as f32)) }).collect();
    assert_eq!(grips.len(), 3);
    let from = grips[0].center();
    let to = grips[2].center() + egui::vec2(0.0, 80.0);
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    for k in 1..=10 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (k as f32 / 10.0)));
        h.step();
    }
    shot(&mut h, "nb-dragging");
    // Carried: folded to its header and first line meanwhile.
    assert!(h.query_by_label("select 1 as a").is_some(), "the dragged cell folded");
    assert!(h.query_by_label("select 2 as b").is_none(), "the others not");
    h.input_mut().events.push(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.step();
    h.step();
    assert_eq!(texts(&h), ["select 2 as b", "select 3 as c", "select 1 as a"]);
    // The top bar's format: every SQL cell.
    {
        let nb = h.state_mut().tab_mut().notebook.as_mut().unwrap();
        nb.cells[0].text = "select x, y from t where x=1".into();
        nb.cells[1].text = "select z from u".into();
    }
    h.state_mut().format_all_cells();
    let t = texts(&h);
    assert!(t[0].starts_with("SELECT x\n") && t[1].starts_with("SELECT z\n"), "{t:?}");
    // A run: its time stays after it.
    focus_editor(&mut h, 0);
    h.get_all_by_label_contains("Выполнить").next().unwrap().click();
    settle(&mut h);
    let ran = h.state().rt.iter().any(|(_, r)| r.last_run.is_some());
    assert!(ran, "the last run's time kept");
    shot(&mut h, "nb-timed");
}

#[test]
fn workspace_tabs_dragged_and_own_title_bar() {
    let (mut h, _g) = harness("wsdrag", SCRIPT, false);
    let names = |h: &Harness<'static, App>| h.state().session.workspaces.iter().map(|w| w.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&h), ["Отчёты", "Миграции", "Прод"]);
    let at = |h: &Harness<'static, App>, n: &str| {
        let r = h.get_by_label(n).rect();
        egui::pos2(((r.min.x + r.max.x) / 2.0) as f32, ((r.min.y + r.max.y) / 2.0) as f32)
    };
    let from = at(&h, "Отчёты");
    let to = at(&h, "Прод") + egui::vec2(20.0, 0.0);
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    for k in 1..=12 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (k as f32 / 12.0)));
        h.step();
    }
    shot(&mut h, "ws-dragging");
    h.input_mut().events.push(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.step();
    assert_eq!(names(&h), ["Миграции", "Прод", "Отчёты"]);
    assert_eq!(h.state().ws().name, "Отчёты", "the dragged one stays the chosen one");
    // The window's own title bar: its buttons at the row's end.
    h.state_mut().settings.system_titlebar = false;
    h.state_mut().settings.win_style = crate::titlebar::Style::Macos;
    settle(&mut h);
    assert!(h.query_by_label("Закрыть").is_some() || h.query_all_by_label_contains("Закрыть").count() > 0);
    shot(&mut h, "own-titlebar");
    // Windows 11's: the button is its ground — as far from the top as from
    // the right edge.
    h.state_mut().settings.win_style = crate::titlebar::Style::Win11;
    settle(&mut h);
    let c = h.get_by_label("Закрыть").rect();
    assert!((c.min.y - 6.0).abs() < 0.6 && (1280.0 - c.max.x - 6.0).abs() < 0.6, "{c:?}");
    let p = egui::pos2(((c.min.x + c.max.x) / 2.0) as f32, ((c.min.y + c.max.y) / 2.0) as f32);
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "own-titlebar-win11");
    // The side panel's icons: their squares 6 from the top, 6 from the card.
    let i = h.get_by_label("Настройки").rect();
    assert!((i.min.y - 6.0).abs() < 0.6 && (38.0 - i.max.y - 6.0).abs() < 0.6, "{i:?}");
    h.state_mut().settings_open = true;
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), 5usize));
    settle(&mut h);
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(300.0, 300.0)));
    h.step();
    for _ in 0..4 {
        h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -120.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
        h.step();
    }
    settle(&mut h);
    shot(&mut h, "titlebar-settings");
}

#[test]
fn find_and_replace_across_tabs() {
    let (mut h, _g) = harness("find", "select total from orders where total > 1;\n", false);
    h.state_mut().session.workspaces[1].tabs[0].text = "select total from t;\n".into();
    settle(&mut h);
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::F, egui::Modifiers::COMMAND);
    assert!(h.state().find.open);
    type_text(&mut h, "total");
    settle(&mut h);
    shot(&mut h, "find-bar");
    let id = h.state().tab().id.clone();
    assert_eq!(h.state().rt[&id].ed.marks.len(), 2, "both lit");
    // Every tab: three; the one in another workspace shown on Enter.
    h.state_mut().find.scope = crate::search::Scope::All;
    settle(&mut h);
    assert_eq!(crate::search::hits(h.state()).len(), 3);
    let hs = crate::search::hits(h.state());
    crate::search::go(h.state_mut(), &hs[2]);
    settle(&mut h);
    assert_eq!(h.state().session.active, 1);
    shot(&mut h, "find-all");
    // Replace all of them.
    h.state_mut().find.replace = "amount".into();
    h.state_mut().find.show_replace = true;
    settle(&mut h);
    h.get_by_label_contains("Заменить все").click();
    settle(&mut h);
    assert_eq!(h.state().session.workspaces[0].tabs[0].text, "select amount from orders where amount > 1;\n");
    assert_eq!(h.state().session.workspaces[1].tabs[0].text, "select amount from t;\n");
}

#[test]
fn database_code_and_tree_search() {
    let (mut h, _g) = harness("dbsearch", "select 1;\n", false);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "lite", None);
    settle(&mut h);
    // The tree: objects by name, the whole database.
    h.state_mut().tree_filter = "order".into();
    for _ in 0..60 {
        settle(&mut h);
        if matches!(h.state().tree_search.hits.get("lite"), Some(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let names: Vec<String> = match h.state().tree_search.hits.get("lite") {
        Some(Some(Ok(v))) => v.iter().map(|f| f.name.clone()).collect(),
        other => panic!("{other:?}"),
    };
    assert!(names.contains(&"orders".to_string()) && names.contains(&"big_orders".to_string()) && names.contains(&"orders_customer".to_string()), "{names:?}");
    shot(&mut h, "tree-search");
    h.state_mut().tree_filter.clear();
    // The code of the database's objects: the view holding "total > 100".
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::F, egui::Modifiers::COMMAND);
    h.state_mut().find.scope = crate::search::Scope::Database;
    type_text(&mut h, "total >");
    for _ in 0..60 {
        settle(&mut h);
        if matches!(h.state().find.db_hits, Some(_)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let hits = h.state().find.db_hits.clone().unwrap().unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].name, "big_orders");
    assert!(hits[0].lines[0].1.contains("total > 100"));
    settle(&mut h);
    shot(&mut h, "db-search");
}

#[test]
fn notebook_labels_and_filter() {
    let (mut h, _g) = harness("nblabels", "-- %%\nselect 1 as a\n-- %%\nselect 2 as b\n-- %%\nselect 3 as c\n", false);
    h.state_mut().tab_mut().set_notebook(true, crate::sql::Dialect::Sqlite);
    settle(&mut h);
    {
        let nb = h.state_mut().tab_mut().notebook.as_mut().unwrap();
        assert!(nb.cells.iter().all(|c| !c.created.is_empty()), "made with a time");
        nb.cells[0].labels = vec![0];
        nb.cells[2].labels = vec![0, 3];
        nb.cells[1].collapsed = true;
    }
    let id = h.state().tab().id.clone();
    settle(&mut h);
    shot(&mut h, "nb-labels");
    let shown = |h: &Harness<'static, App>| h.query_all_by_label("⠿").count();
    assert_eq!(shown(&h), 3);
    // By label: two shown.
    h.state_mut().rt.get_mut(&id).unwrap().nb_labels = vec![0];
    settle(&mut h);
    assert_eq!(shown(&h), 2);
    // By text: the folded one opens.
    h.state_mut().rt.get_mut(&id).unwrap().nb_labels.clear();
    h.state_mut().rt.get_mut(&id).unwrap().nb_query = "2 as".into();
    settle(&mut h);
    assert_eq!(shown(&h), 1);
    assert!(h.query_by_label_contains("⋯").is_none(), "shown open");
    shot(&mut h, "nb-filter");
    // Left out: label 0 → only the middle cell.
    {
        let rt = h.state_mut().rt.get_mut(&id).unwrap();
        rt.nb_query.clear();
        rt.nb_exclude = vec![0];
    }
    settle(&mut h);
    assert_eq!(shown(&h), 1);
    // All of the wanted labels: 0 and 3 → the last one alone.
    {
        let rt = h.state_mut().rt.get_mut(&id).unwrap();
        rt.nb_exclude.clear();
        rt.nb_labels = vec![0, 3];
        rt.nb_all = true;
    }
    settle(&mut h);
    assert_eq!(shown(&h), 1);
    // Made today: all; before last year: none.
    {
        let rt = h.state_mut().rt.get_mut(&id).unwrap();
        rt.nb_labels.clear();
        rt.nb_date = 1;
    }
    settle(&mut h);
    assert_eq!(shown(&h), 3);
    {
        let rt = h.state_mut().rt.get_mut(&id).unwrap();
        rt.nb_date = 4;
        rt.nb_to = "2020-01-01".into();
    }
    settle(&mut h);
    assert_eq!(shown(&h), 0);
    h.state_mut().rt.get_mut(&id).unwrap().nb_date = 0;
    settle(&mut h);
    // A cell's labels: a click on them, the menu (named as set).
    h.state_mut().settings.labels[0].name = "Срочно".into();
    h.state_mut().settings.labels[0].color = [10, 200, 120];
    settle(&mut h);
    h.get_all_by_label("labels").next().unwrap().click();
    settle(&mut h);
    assert!(h.query_by_label("Срочно").is_some(), "the menu, the label renamed");
    shot(&mut h, "nb-label-menu");
}

#[test]
fn schema_filter_in_the_tree() {
    if std::env::var("ZAPROS_TEST_PG").is_err() {
        return;
    }
    let (mut h, _g) = harness("schemas", "select 1;\n", true);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "pg", None);
    settle(&mut h);
    h.state_mut().open_nodes.insert(("pg".into(), vec![]));
    settle(&mut h);
    assert!(h.query_by_label("information_schema").is_some());
    // Only public: the rest hidden.
    h.state_mut().session.schema_filters.insert("pg".into(), crate::config::SchemaFilter { only: true, names: vec!["public".into()] });
    settle(&mut h);
    assert!(h.query_by_label("information_schema").is_none() && h.query_by_label("pg_catalog").is_none());
    assert!(h.query_by_label("public").is_some());
    // Hiding: just that one gone.
    h.state_mut().session.schema_filters.insert("pg".into(), crate::config::SchemaFilter { only: false, names: vec!["pg_catalog".into()] });
    settle(&mut h);
    assert!(h.query_by_label("information_schema").is_some() && h.query_by_label("pg_catalog").is_none());
    // The popup.
    h.get_by_label("Какие схемы / базы показывать").click();
    settle(&mut h);
    shot(&mut h, "schema-filter");
}

/// At any scale the window's own buttons stand as far from the top as
/// from the side, and the side panel's icons as far from the card below as
/// from the top — in screen pixels.
#[test]
fn margins_even_at_every_scale() {
    for z in [1.0f32, 1.2, 1.25, 1.5] {
        let (mut h, _g) = harness("margins", SCRIPT, false);
        h.state_mut().settings.ui_scale = z;
        h.state_mut().settings.system_titlebar = false;
        h.state_mut().settings.win_style = crate::titlebar::Style::Win11;
        settle(&mut h);
        let c = h.get_by_label("Закрыть").rect();
        let p = egui::pos2(((c.min.x + c.max.x) / 2.0) as f32, ((c.min.y + c.max.y) / 2.0) as f32);
        h.input_mut().events.push(egui::Event::PointerMoved(p));
        for _ in 0..3 {
            h.step();
        }
        let img = h.render().unwrap();
        let (w, _) = img.dimensions();
        let px = |x: u32, y: u32| img.get_pixel(x, y).0;
        // The red of the close button lit.
        let red: Vec<(u32, u32)> = (0..90).flat_map(|y| (w - 140..w).map(move |x| (x, y))).filter(|&(x, y)| { let p = px(x, y); p[0] > 150 && p[1] < 80 && p[2] < 60 }).collect();
        let top = red.iter().map(|p| p.1).min().unwrap();
        let right = w - 1 - red.iter().map(|p| p.0).max().unwrap();
        assert_eq!(top, right, "scale {z}: the close button {top} from the top, {right} from the side");
        // The side panel's chosen icon (blue-ish) and the card under it.
        let bg = px(2, 2);
        let k = z;
        let card = (((36.0 * k) as u32)..200).find(|&y| px((140.0 * k) as u32, y) != bg).unwrap();
        let ys: Vec<u32> = (0..card).filter(|&y| (0..(280.0 * k) as u32).any(|x| { let p = px(x, y); p[2] as i32 > p[0] as i32 + 15 })).collect();
        let (above, below) = (*ys.first().unwrap(), card - 1 - ys.last().unwrap());
        assert_eq!(above, below, "scale {z}: the icons {above} under the top, {below} over the card");
    }
}

#[test]
fn label_dots_unfold_under_the_pointer() {
    let (mut h, _g) = harness("chips", "-- %%\nselect 1\n", false);
    h.state_mut().tab_mut().set_notebook(true, crate::sql::Dialect::Sqlite);
    settle(&mut h);
    shot(&mut h, "chips-folded");
    // The stack: right of the filter field, left of the filter button.
    let fb = h.query_all_by_label_contains("фильтр блоков").next().map(|n| n.rect());
    let p = match fb {
        Some(r) => egui::pos2(r.max.x as f32 + 20.0, ((r.min.y + r.max.y) / 2.0) as f32),
        None => egui::pos2(1200.0, 124.0),
    };
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    for _ in 0..4 {
        h.step();
    }
    shot(&mut h, "chips-open");
}

#[test]
fn empty_cells_stay_apart() {
    let (mut h, _g) = harness("twins", "-- %%\nselect 1\n", false);
    h.state_mut().tab_mut().set_notebook(true, crate::sql::Dialect::Sqlite);
    settle(&mut h);
    // Two empty Markdown cells, then the notebook off and on again.
    for _ in 0..2 {
        h.get_by_label("+ Markdown").click();
        settle(&mut h);
    }
    h.state_mut().toggle_notebook();
    settle(&mut h);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let cells = |h: &Harness<'static, App>| h.state().tab().notebook.as_ref().unwrap().cells.clone();
    let c = cells(&h);
    let ids: std::collections::HashSet<_> = c.iter().map(|c| c.id.clone()).collect();
    assert_eq!(ids.len(), c.len(), "an id each: {:?}", c.iter().map(|c| (&c.id, c.kind)).collect::<Vec<_>>());
    // A label and a kind on one: the other untouched.
    let empty: Vec<usize> = c.iter().enumerate().filter(|(_, c)| c.text.is_empty()).map(|(i, _)| i).collect();
    assert_eq!(empty.len(), 2);
    h.state_mut().tab_mut().notebook.as_mut().unwrap().cells[empty[0]].labels = vec![1];
    h.state_mut().tab_mut().notebook.as_mut().unwrap().cells[empty[0]].kind = crate::config::CellKind::Sql;
    settle(&mut h);
    let c = cells(&h);
    assert!(c[empty[1]].labels.is_empty() && c[empty[1]].kind == crate::config::CellKind::Markdown);
    // Twins from before the fix: told apart.
    let mut nb = crate::config::Notebook { on: true, cells: vec![c[0].clone(), c[0].clone()] };
    assert!(nb.fix_ids());
    assert_ne!(nb.cells[0].id, nb.cells[1].id);
}

#[test]
fn errors_in_plain_words() {
    let script = "select totl, name\n  from orders o\n  join customers c on c.id = o.customer_id";
    let (mut h, _g) = harness("errors", script, false);
    focus_editor(&mut h, 3);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    let rt = h.state().rt.get(&h.state().tab().id).unwrap();
    let l = rt.log.iter().find(|l| l.err.is_some()).expect("an error line");
    // A guess from the tables of the statement (with its alias: two
    // tables), the name underlined.
    assert_eq!(l.text, "Нет такого поля «totl» — может быть, «o.total»?");
    let (r, tip) = rt.server_err.clone().unwrap();
    assert_eq!(&script[r.clone()], "totl");
    assert!(tip.contains("total"), "{tip}");
    assert!(l.err.as_ref().unwrap().raw.contains("no such column"));
    // The whole log unfolds.
    let _ = h.get_by_label_contains("SQLITE_ERROR");
    h.get_by_label("Весь лог…").click();
    settle(&mut h);
    let _ = h.get_by_label_contains("no such column: totl");
    shot(&mut h, "errors");
}

#[test]
fn aliases_coloured() {
    let script = "select o.id, c.name as who, count(*) n, sum(o.total) over (partition by o.customer_id) as running\n  from orders o\n  join customers c on c.id = o.customer_id\n group by o.id, who\n order by n desc";
    let (mut h, _g) = harness("aliases", script, false);
    settle(&mut h);
    let st = h.state();
    let pal = st.settings.dark.clone();
    let spans = crate::sql::embed::classify(script, crate::sql::Dialect::Sqlite, false);
    let class_of = |s: &str, nth: usize| {
        let at = script.match_indices(s).nth(nth).unwrap().0;
        spans.iter().find(|x| x.range.start == at).map(|x| x.class)
    };
    use crate::sql::embed::Class;
    assert_eq!(class_of("o.id", 0), Some(Class::TableAlias));
    assert_eq!(class_of("who", 0), Some(Class::ColumnAlias));
    assert_eq!(class_of("who", 1), Some(Class::ColumnAlias));
    assert_eq!(class_of("n,", 0), Some(Class::ColumnAlias));
    assert_eq!(class_of("n desc", 0), Some(Class::ColumnAlias));
    assert_ne!(pal.table_alias, pal.column_alias);
    shot(&mut h, "aliases");
}

#[test]
fn selection_keeps_text_colours_rounded() {
    let script = "select o.id, sum(o.total) as total\n  from orders o\n where o.total > (select avg(total)\n                    from orders)\n group by o.id";
    let (mut h, _g) = harness("selection", script, false);
    focus_editor(&mut h, 0);
    let id = h.state().tab().id.clone();
    let a = script.find("sum").unwrap();
    let b = script.find("avg").unwrap();
    h.state_mut().rt.get_mut(&id).unwrap().ed.select = Some((a, b));
    for _ in 0..4 {
        h.step();
    }
    let img = h.render().unwrap();
    let r = h.state().rt[&id].ed.text_rect.unwrap();
    if let Some(dir) = std::env::var_os("ZAPROS_SHOT_DIR") {
        let crop = image::imageops::crop_imm(&img, r.left() as u32, r.top() as u32, 460, 110).to_image();
        let big = image::imageops::resize(&crop, 460 * 3, 110 * 3, image::imageops::FilterType::Nearest);
        big.save(PathBuf::from(dir).join("selection.png")).unwrap();
    }
    // "from" (line 2) is selected: still drawn in the keyword colour.
    let kw = h.state().settings.dark.keyword;
    let line_h = 19.0;
    let row = egui::Rect::from_min_size(egui::pos2(r.left(), r.top() + line_h), egui::vec2(120.0, line_h + 4.0));
    let mut hits = 0;
    for y in row.top() as u32..row.bottom() as u32 {
        for x in row.left() as u32..row.right() as u32 {
            let p = img.get_pixel(x, y);
            let d = (p[0] as i32 - kw[0] as i32).abs() + (p[1] as i32 - kw[1] as i32).abs() + (p[2] as i32 - kw[2] as i32).abs();
            if d < 50 {
                hits += 1;
            }
        }
    }
    assert!(hits > 15, "the selected keyword lost its colour ({hits} px)");
}

#[test]
fn notebook_cells_stop_and_explain_themselves() {
    let text = "-- %%\nselect count(*) from orders where total > 100;\n-- %%\nwith recursive n(x) as (select 1 union all select x + 1 from n) select count(*) from n;\n";
    let (mut h, _g) = harness("nb-buttons", text, false);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    // The toolbar's run / script / stop / plan stay with the plain text.
    assert!(h.query_by_label("⏩ Скрипт").is_none() && h.query_by_label("⏩").is_none());
    let plans = h.get_all_by_label_contains("План").count();
    assert_eq!(plans, 2, "a plan button per SQL cell");
    let tab = h.state().tab().id.clone();
    let cells: Vec<String> = h.state().tab().notebook.as_ref().unwrap().cells.iter().map(|c| c.id.clone()).collect();
    let key = |i: usize| crate::app::App::cell_key(&tab, &cells[i]);
    // The first cell's plan.
    h.get_all_by_label_contains("План").next().unwrap().click();
    settle(&mut h);
    let rt = &h.state().rt[&key(0)];
    assert!(rt.results.first().is_some_and(|v| v.sql.starts_with("EXPLAIN QUERY PLAN")), "{:?}", rt.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    // The second runs forever: its own stop button ends it.
    let ctx = h.ctx.clone();
    h.state_mut().run_cells(&ctx, Some(cells[1].clone()));
    for _ in 0..20 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state().rt[&key(1)].run.is_some());
    // (The results' own stop, for rows loading, is there too: disabled.)
    h.get_all_by_label("Стоп").find(|n| !format!("{n:?}").contains("disabled: true")).unwrap().click();
    settle(&mut h);
    let rt = &h.state().rt[&key(1)];
    assert!(rt.run.is_none() && rt.last_run.is_some_and(|l| l.1), "{:?}", rt.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert!(rt.log.iter().any(|l| l.text == "Запрос отменён"), "{:?}", rt.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    shot(&mut h, "notebook-buttons");
}

#[test]
fn bands_cross_blank_lines_rounded() {
    let script = "select o.id\n     , (select max(total)\n\n          from orders x\n\n         where x.customer_id = o.customer_id) as top\n  from orders o\n\n where o.total > 10";
    let (mut h, _g) = harness("bands-blank", script, false);
    focus_editor(&mut h, 0);
    for _ in 0..4 {
        h.step();
    }
    let id = h.state().tab().id.clone();
    assert!(!h.state().rt[&id].ed.blank_bands.is_empty());
    let img = h.render().unwrap();
    let r = h.state().rt[&id].ed.text_rect.unwrap();
    if let Some(dir) = std::env::var_os("ZAPROS_SHOT_DIR") {
        let crop = image::imageops::crop_imm(&img, r.left() as u32 - 4, r.top() as u32, 460, 180).to_image();
        image::imageops::resize(&crop, 460 * 3, 180 * 3, image::imageops::FilterType::Nearest).save(PathBuf::from(dir).join("bands-blank.png")).unwrap();
    }
}

#[test]
fn ctrl_click_opens_help() {
    let script = "select coalesce(note, 'нет') || ' / ' || name, count(*) over (partition by c.id)\n  from orders o\n  left join customers c on c.id = o.customer_id\n group by c.id";
    let (mut h, _g) = harness("help", script, false);
    focus_editor(&mut h, 0);
    // A function.
    ctrl_click_text(&mut h, 0, 9);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    settle(&mut h);
    let (i, _) = h.state().help.expect("help for coalesce");
    assert_eq!(crate::sql::help::entries()[i].names[0], "coalesce");
    let _ = h.get_by_label_contains("coalesce — функция");
    let _ = h.get_by_label_contains("Первое значение");
    shot(&mut h, "help-coalesce");
    // An operator.
    h.state_mut().help = None;
    let col = script[..script.find("||").unwrap()].chars().count();
    ctrl_click_text(&mut h, 0, col);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    settle(&mut h);
    let (i, _) = h.state().help.expect("help for ||");
    assert_eq!(crate::sql::help::entries()[i].names[0], "||");
    // A keyword: LEFT (JOIN), not left(s, n).
    h.state_mut().help = None;
    ctrl_click_text(&mut h, 2, 3);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    settle(&mut h);
    let (i, _) = h.state().help.expect("help for LEFT");
    assert_eq!(crate::sql::help::entries()[i].kind, 'k');
    // A table still opens as before (no help).
    h.state_mut().help = None;
    ctrl_click_text(&mut h, 1, 8);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    settle(&mut h);
    assert!(h.state().help.is_none());
}

/// The README's picture (docs/screenshot.png): `cargo test --release
/// readme_picture -- --ignored`.
#[test]
#[ignore]
fn readme_picture() {
    let script = "-- Best customers this year\nselect c.name, count(o.id) as orders, sum(o.total) total,\n       case when sum(o.total) > 5000 then 'vip' else 'regular' end as kind\n  from customers c\n  join orders o on o.customer_id = c.id\n where c.active = 1\n group by c.name\n order by total desc;\n\nselect * from big_orders where note = 'gift';\n";
    let (mut h, _g) = harness("readme", script, false);
    crate::i18n::set_ru(false);
    {
        let st = h.state_mut();
        st.settings.russian = Some(false);
        for (w, (name, tab)) in st.session.workspaces.iter_mut().zip([("Reports", "Customers"), ("Migrations", "v42"), ("Production", "hotfix")]) {
            w.name = name.into();
            w.tabs[0].title = tab.into();
        }
        st.session.workspaces[0].tabs[1].title = "Draft".into();
    }
    focus_editor(&mut h, 30);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    h.state_mut().open_nodes.insert(("lite".into(), vec!["tables".into()]));
    settle(&mut h);
    h.state_mut().open_nodes.insert(("lite".into(), vec!["tables".into(), "orders".into()]));
    settle(&mut h);
    let img = h.render().expect("render");
    img.save(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/screenshot.png")).unwrap();
}

#[test]
fn result_as_a_chart() {
    let script = "select date('2026-01-01', '+' || (id % 30) || ' days') as day, count(*) as orders, round(sum(total) / 100) as hundreds\n  from orders\n group by 1\n order by 1;\n";
    let (mut h, _g) = harness("chart", script, false);
    focus_editor(&mut h, 10);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    let id = h.state().tab().id.clone();
    assert_eq!(h.state().rt[&id].results[0].table.rows.len(), 30);
    h.get_by_label("📈 График").click();
    for _ in 0..4 {
        h.step();
    }
    let v = &h.state().rt[&id].results[0];
    assert!(v.chart.on);
    // The date along X, both numbers over it.
    assert_eq!((v.chart.x, v.chart.ys.iter().copied().collect::<Vec<_>>()), (0, vec![1, 2]));
    let img = h.render().unwrap();
    // Lines in the two series' colours (blue and green) in the lower half.
    let near = |c: egui::Color32| img.enumerate_pixels().filter(|(_, y, p)| *y > 560 && (p[0] as i32 - c.r() as i32).abs() < 30 && (p[1] as i32 - c.g() as i32).abs() < 30 && (p[2] as i32 - c.b() as i32).abs() < 30).count();
    assert!(near(crate::chart::color(0)) > 200, "{}", near(crate::chart::color(0)));
    assert!(near(crate::chart::color(1)) > 200, "{}", near(crate::chart::color(1)));
    shot(&mut h, "chart-lines");
    // Bars; one column hidden by its chip.
    h.get_by_label("Столбцы").click();
    h.get_by_label("● orders").click();
    for _ in 0..4 {
        h.step();
    }
    let v = &h.state().rt[&id].results[0];
    assert_eq!((v.chart.kind, v.chart.ys.len()), (crate::chart::Kind::Bars, 1));
    shot(&mut h, "chart-bars");
    // Back to the table.
    h.get_by_label("▦ Таблица").click();
    h.step();
    assert!(!h.state().rt[&id].results[0].chart.on);
}

#[test]
fn notebook_cells_show_how_they_ran() {
    use crate::app::Flag;
    let text = "-- %%\nselect count(*) from orders;\n-- %%\nselect totl from orders;\n-- %%\nupdate orders set note = 'x' where 1 = 0;\n-- %%\nwith recursive n(x) as (select 1 union all select x + 1 from n) select count(*) from n;\n";
    let (mut h, _g) = harness("nb-flags", text, false);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let tab = h.state().tab().id.clone();
    let cells: Vec<String> = h.state().tab().notebook.as_ref().unwrap().cells.iter().map(|c| c.id.clone()).collect();
    let key = |i: usize| crate::app::App::cell_key(&tab, &cells[i]);
    let ctx = h.ctx.clone();
    for c in &cells[..3] {
        h.state_mut().run_cells(&ctx, Some(c.clone()));
        settle(&mut h);
    }
    // The last one runs on: blue while it does.
    h.state_mut().run_cells(&ctx, Some(cells[3].clone()));
    for _ in 0..10 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    let flag = |h: &Harness<'static, App>, i: usize| h.state().rt.get(&key(i)).and_then(|r| r.flag.as_ref()).map(|f| f.kind);
    assert_eq!(flag(&h, 3), Some(Flag::Running));
    assert_eq!([flag(&h, 0), flag(&h, 1), flag(&h, 2)], [Some(Flag::Ok), Some(Flag::Error), Some(Flag::Warn)]);
    // (Folded: all four in sight.)
    for c in h.state_mut().tab_mut().notebook.as_mut().unwrap().cells.iter_mut() {
        c.collapsed = true;
    }
    h.step();
    h.step();
    shot(&mut h, "notebook-flags");
    let img = h.render().unwrap();
    // The headers tinted: green, red, yellow and blue all there.
    let has = |c: egui::Color32| img.pixels().filter(|p| (p[0] as i32 - c.r() as i32 * 85 / 100).abs() < 16 && (p[1] as i32 - c.g() as i32 * 85 / 100).abs() < 16 && (p[2] as i32 - c.b() as i32 * 85 / 100).abs() < 16).count();
    for c in [crate::ui::GREEN, crate::ui::RED, crate::notebook_ui::YELLOW, crate::ui::BLUE] {
        assert!(has(c) > 40, "{c:?}: {}", has(c));
    }
    h.get_all_by_label("Стоп").find(|n| !format!("{n:?}").contains("disabled: true")).unwrap().click();
    settle(&mut h);
    // Stopped: a warning.
    assert_eq!(flag(&h, 3), Some(Flag::Warn));
    // The focused cell keeps its colour; the others fade some time after.
    for i in 0..3 {
        if let Some(f) = h.state_mut().rt.get_mut(&key(i)).and_then(|r| r.flag.as_mut()) {
            f.quiet = Some(std::time::Instant::now() - Duration::from_secs(30));
        }
    }
    h.step();
    h.step();
    assert_eq!(h.state().rt[&key(3)].flag.as_ref().map(|f| f.kind), Some(Flag::Warn), "focused: stays");
    assert!((0..3).all(|i| flag(&h, i).is_none()));
}

#[test]
fn notebook_header_sticks_while_scrolled() {
    let long: String = (0..70).map(|i| format!("select {i} as n{i};\n")).collect();
    let text = format!("-- %%\n{long}-- %%\nselect 1;\n");
    let (mut h, _g) = harness("nb-sticky", &text, false);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let top_of = |h: &Harness<'static, App>, label: &str| h.get_all_by_label(label).map(|n| n.rect().min.y as f32).fold(f32::MAX, f32::min);
    let natural = top_of(&h, "[1]");
    // Scrolled down a good way into the first cell: its header stays on top.
    let over = egui::pos2(700.0, 400.0);
    h.input_mut().events.push(egui::Event::PointerMoved(over));
    for _ in 0..6 {
        h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -120.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
        h.step();
    }
    for _ in 0..30 {
        h.step();
    }
    let id = h.state().tab().id.clone();
    let cell0 = h.state().tab().notebook.as_ref().unwrap().cells[0].id.clone();
    let stuck = top_of(&h, "[1]");
    assert!((stuck - natural).abs() < 30.0, "header at {stuck}, was {natural}");
    shot(&mut h, "notebook-sticky");
    // Its buttons still work there: Run.
    h.get_all_by_label_contains("Выполнить").filter(|n| !format!("{n:?}").contains("Выполнить все")).min_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y)).unwrap().click();
    settle(&mut h);
    let key = crate::app::App::cell_key(&id, &cell0);
    assert!(h.state().rt.get(&key).is_some_and(|r| !r.results.is_empty()), "the stuck header's Run ran the first cell");
    shot(&mut h, "notebook-sticky-flag");
}

#[test]
fn hung_run_ended_by_force() {
    let Ok(pg) = std::env::var("ZAPROS_TEST_PG") else { return };
    let (mut h, _g) = harness("force", "select pg_sleep(60);\n", true);
    h.state_mut().ws_mut().conn = Some("pg".into());
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "pg", None);
    settle(&mut h);
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    for _ in 0..30 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    let id = h.state().tab().id.clone();
    let pid = h.state().live["pg"].query.server_id().expect("backend pid");
    // A stop the server "didn't answer": asked long ago, still running.
    h.state_mut().rt.get_mut(&id).unwrap().run.as_mut().unwrap().stop_asked = Some(std::time::Instant::now() - Duration::from_secs(10));
    h.step();
    shot(&mut h, "force-button");
    h.get_all_by_label_contains("Прервать").next().expect("the Kill button").click();
    for _ in 0..100 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    let rt = &h.state().rt[&id];
    assert!(rt.run.is_none());
    assert!(rt.log.iter().any(|l| l.text.starts_with("Запрос прерван принудительно")), "{:?}", rt.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert!(h.state().toast.as_ref().is_some_and(|t| t.0.contains("завершена на сервере")), "{:?}", h.state().toast);
    // The backend's gone from the server.
    let p: Vec<&str> = pg.split(':').collect();
    let spec = crate::db::Spec { kind: crate::db::Kind::Postgres, host: p[0].into(), port: p[1].parse().unwrap(), database: p[2].into(), user: p[3].into(), tls: crate::db::Tls::Disable, ..Default::default() };
    let mut s = crate::db::open(&spec, p[4]).unwrap();
    let left = s.run_limit(&format!("select count(*) from pg_stat_activity where pid = {pid}"), 10, &mut |_| {}).unwrap();
    match left {
        crate::db::Outcome::Rows(t) => assert_eq!(t.rows[0][0].as_deref(), Some("0")),
        o => panic!("{o:?}"),
    }
    // The new session works.
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    for _ in 0..10 {
        h.step();
    }
    h.state_mut().cancel();
    settle(&mut h);
}

/// A second window of the app on the same data (the first one's harness
/// set where that is).
fn second_window() -> Harness<'static, App> {
    let mut h = Harness::builder().with_size([1280.0, 800.0]).with_step_dt(1.0 / 60.0).wgpu().build_eframe(|cc| {
        let mut app = App::new(cc);
        app.settings.russian = Some(true);
        app.settings.theme = crate::config::ThemePref::Dark;
        app
    });
    settle(&mut h);
    h
}

#[test]
fn windows_share_their_tabs() {
    let (mut a, _g) = harness("sync-ui", "select 1;\n", false);
    let mut b = second_window();
    let id = a.state().tab().id.clone();
    assert_eq!(b.state().tab().id, id);
    // Typed in the first: the second gets the text, read-only, with a note.
    focus_editor(&mut a, 9);
    type_text(&mut a, " -- from A");
    a.state_mut().save_session();
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(15));
        b.step();
    }
    assert_eq!(b.state().tab().text, "select 1; -- from A\n");
    assert!(b.state().tab_held_elsewhere());
    assert!(b.query_by_label_contains("правят в другом окне").is_some());
    // Typing there changes nothing.
    focus_editor(&mut b, 0);
    type_text(&mut b, "zz");
    assert_eq!(b.state().tab().text, "select 1; -- from A\n");
    shot(&mut b, "sync-held");
    // "Edit here": the second takes over; the first turns read-only.
    b.get_by_label("Править здесь").click();
    b.step();
    b.step();
    assert!(!b.state().tab_held_elsewhere());
    std::thread::sleep(Duration::from_millis(15));
    a.step();
    a.step();
    assert!(a.state().tab_held_elsewhere());
    focus_editor(&mut b, 0);
    type_text(&mut b, "/* B */ ");
    b.state_mut().save_session();
    std::thread::sleep(Duration::from_millis(15));
    a.step();
    assert_eq!(a.state().tab().text, "/* B */ select 1; -- from A\n");
    // A tab made in one shows up in the other.
    a.state_mut().new_tab(Some("new one".into()), "select 2;".into(), None);
    a.state_mut().save_session();
    std::thread::sleep(Duration::from_millis(15));
    b.step();
    assert!(b.state().ws().tabs.iter().any(|t| t.title == "new one" && t.text == "select 2;"));
    // Each window stays on its own tab.
    assert_eq!(b.state().tab().id, id);
}

#[test]
fn table_designer_indexes() {
    let (mut h, _g) = harness("designer-ix", SCRIPT, false);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "lite", None);
    for p in [vec![], vec!["tables".to_string()]] {
        h.state_mut().open_nodes.insert(("lite".into(), p));
        settle(&mut h);
    }
    ctrl_click_tree(&mut h, "orders");
    settle(&mut h);
    let id = h.state().tab().id.clone();
    let indexes = |h: &Harness<'static, App>| match h.state().rt[&id].design.as_ref().map(|d| &d.loaded) {
        Some(crate::design_ui::Loaded::Table(te)) => te.indexes.iter().filter(|r| !r.dropped).map(|r| (r.idx.name.clone(), r.idx.cols.clone())).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    // The file's index is there.
    assert_eq!(indexes(&h), [("orders_customer".to_string(), "customer_id".to_string())]);
    // One added (its columns from the menu of the table's), the other renamed.
    h.get_by_label_contains("➕ Индекс").click();
    h.step();
    if let Some(crate::design_ui::Loaded::Table(te)) = h.state_mut().rt.get_mut(&id).unwrap().design.as_mut().map(|d| &mut d.loaded) {
        let new = te.indexes.last_mut().unwrap();
        new.idx.cols = "total DESC".into();
        new.idx.filter = "note IS NOT NULL".into();
        te.indexes[0].idx.name = "orders_by_customer".into();
    }
    settle(&mut h);
    let script = h.state().rt[&id].design.as_ref().unwrap().script();
    assert_eq!(script, "DROP INDEX orders_customer;\nCREATE INDEX orders_by_customer ON orders (customer_id);\nCREATE INDEX orders_ix2 ON orders (total DESC) WHERE note IS NOT NULL;\n");
    shot(&mut h, "designer-indexes");
    h.get_by_label_contains("Применить").click();
    for _ in 0..50 {
        settle(&mut h);
        if indexes(&h).len() == 2 && !h.state().rt[&id].design.as_ref().unwrap().busy && h.state().rt[&id].design.as_ref().unwrap().script().is_empty() {
            break;
        }
    }
    let mut got = indexes(&h);
    got.sort();
    assert_eq!(got, [("orders_by_customer".to_string(), "customer_id".to_string()), ("orders_ix2".to_string(), "total DESC".to_string())]);
}

#[test]
fn notebook_wheel_scrolls_while_selecting() {
    let long: String = (1..=80).map(|i| format!("select {i} as n;\n")).collect();
    let text = format!("-- %%\nselect 0;\n-- %%\n{long}-- %%\nselect 99;\n");
    let (mut h, _g) = harness("nb-wheel-sel", &text, false);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let tab = h.state().tab().id.clone();
    let cell = h.state().tab().notebook.as_ref().unwrap().cells[1].id.clone();
    // A cell below with results: its table's scroll mustn't take the wheel.
    let last = h.state().tab().notebook.as_ref().unwrap().cells[2].id.clone();
    let ctx = h.ctx.clone();
    h.state_mut().run_cells(&ctx, Some(last));
    settle(&mut h);
    let key = crate::app::App::cell_key(&tab, &cell);
    let r = h.state().rt[&key].ed.text_rect.unwrap();
    let p = egui::pos2(r.left() + 40.0, r.top() + 12.0);
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    h.input_mut().events.push(egui::Event::PointerMoved(p + egui::vec2(0.0, 30.0)));
    h.step();
    for _ in 0..6 {
        h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -200.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
        h.step();
    }
    for _ in 0..20 {
        h.step();
    }
    let (a, b) = h.state().rt[&key].ed.cursor.unwrap();
    let text = h.state().tab().notebook.as_ref().unwrap().cells[1].text.clone();
    let line = |x: usize| text[..x.min(text.len())].matches('\n').count();
    shot(&mut h, "nb-wheel-sel");
    assert!(line(a.max(b)) > 25, "the selection reached line {}", line(a.max(b)));
    h.input_mut().events.push(egui::Event::PointerButton { pos: p + egui::vec2(0.0, 30.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.step();
}

#[test]
fn notebook_last_cell_scrolls_to_the_top() {
    let text = "-- %%\nselect 1;\n-- %%\nselect 2;\n-- %%\nselect 3;\n";
    let (mut h, _g) = harness("nb-tail", text, false);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let top_of = |h: &Harness<'static, App>, label: &str| h.get_all_by_label(label).map(|n| n.rect().min.y as f32).fold(f32::MAX, f32::min);
    let first = top_of(&h, "[1]");
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(700.0, 400.0)));
    for _ in 0..30 {
        h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -200.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
        h.step();
    }
    for _ in 0..30 {
        h.step();
    }
    let last = top_of(&h, "[3]");
    shot(&mut h, "nb-tail");
    assert!((last - first).abs() < 40.0, "the last cell's header at {last}, the first's was at {first}");
}

#[test]
fn tree_refreshes_in_place_and_is_kept() {
    let (mut h, _g) = harness("tree-keep", SCRIPT, false);
    let ctx = h.ctx.clone();
    h.state_mut().ensure_live(&ctx, "lite", None);
    for p in [vec![], vec!["tables".to_string()]] {
        h.state_mut().open_nodes.insert(("lite".into(), p));
        settle(&mut h);
    }
    let key = ("lite".to_string(), vec!["tables".to_string()]);
    let names = |h: &Harness<'static, App>| match h.state().tree.get(&key) {
        Some(Node::Ready(v)) => v.iter().map(|o| o.name.clone()).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    assert_eq!(names(&h), ["customers", "orders"]);
    // A table made from outside; the folder refreshed: the old list stays
    // shown meanwhile, then the new one's in.
    let db = h.state().spec("lite").unwrap().database.clone();
    rusqlite::Connection::open(&db).unwrap().execute_batch("create table zz_new (x int)").unwrap();
    h.state_mut().mark_stale("lite", &[]);
    h.state_mut().request_children("lite", vec!["tables".into()]);
    assert_eq!(names(&h), ["customers", "orders"], "the old list while refreshing");
    settle(&mut h);
    assert_eq!(names(&h), ["customers", "orders", "zz_new"]);
    rusqlite::Connection::open(&db).unwrap().execute_batch("drop table zz_new").unwrap();
    h.state_mut().request_children("lite", vec!["tables".into()]);
    settle(&mut h);
    assert_eq!(names(&h), ["customers", "orders"]);
    // Kept on disk: the next start shows it at once, before connecting.
    h.state_mut().save_session();
    let b = Harness::builder().with_size([1280.0, 800.0]).wgpu().build_eframe(|cc| App::new(cc));
    match b.state().tree.get(&key) {
        Some(Node::Ready(v)) => assert_eq!(v.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["customers", "orders"]),
        _ => panic!("no cached tree"),
    }
}

#[test]
fn export_format_picked() {
    let (mut h, _g) = harness("export-fmt", SCRIPT, false);
    focus_editor(&mut h, 30);
    key(&mut h, egui::Key::Enter, egui::Modifiers::COMMAND);
    settle(&mut h);
    h.get_all_by_label_contains("Экспорт").next().unwrap().click();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().export.is_some());
    shot(&mut h, "export-window");
    click_at(&mut h, egui::pos2(685.0, 334.0), egui::PointerButton::Primary);
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "export-open");
    h.get_all_by_label_contains("JSON").next().expect("JSON in the list").click();
    for _ in 0..3 {
        h.step();
    }
    let f = h.state().export.as_ref().map(|e| e.2.format);
    assert_eq!(f, Some(crate::export::Format::Json));
}

#[test]
fn export_format_picked_in_a_notebook() {
    let (mut h, _g) = harness("export-fmt-nb", "-- %%\nselect 1 as a;\n-- %%\nselect 2 as b;\n", false);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let ctx = h.ctx.clone();
    h.state_mut().run_cells(&ctx, None);
    settle(&mut h);
    h.get_all_by_label_contains("Экспорт").next().unwrap().click();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().export.is_some());
    shot(&mut h, "export-nb-window");
    let combo = h.get_all_by_label_contains("Формат").next().map(|n| { let r = n.rect(); egui::pos2(r.max.x as f32 + 150.0, ((r.min.y + r.max.y) / 2.0) as f32) }).unwrap();
    click_at(&mut h, combo, egui::PointerButton::Primary);
    for _ in 0..3 {
        h.step();
    }
    shot(&mut h, "export-nb-open");
    h.get_all_by_label_contains("JSON").next().expect("JSON in the list").click();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.state().export.as_ref().map(|e| e.2.format), Some(crate::export::Format::Json));
}

#[test]
fn notebook_focused_cell_keeps_its_edges() {
    let cols: String = (0..30).map(|i| format!("{i} as column_number_{i}")).collect::<Vec<_>>().join(", ");
    let text = format!("-- %%\nselect {cols} from orders;\n-- %%\nselect * from orders;\n-- %%\nselect 1;\n");
    let (mut h, _g) = harness("nb-edges", &text, false);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let tab = h.state().tab().id.clone();
    let cells: Vec<String> = h.state().tab().notebook.as_ref().unwrap().cells.iter().map(|c| c.id.clone()).collect();
    let ctx = h.ctx.clone();
    h.state_mut().run_cells(&ctx, Some(cells[0].clone()));
    settle(&mut h);
    h.state_mut().rt.entry(tab.clone()).or_default().cell_focus = Some(cells[0].clone());
    for _ in 0..4 {
        h.step();
    }
    shot(&mut h, "nb-edges");
    h.state_mut().run_cells(&ctx, Some(cells[1].clone()));
    settle(&mut h);
    h.state_mut().rt.entry(tab.clone()).or_default().cell_focus = Some(cells[1].clone());
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(700.0, 200.0)));
    for _ in 0..3 {
        h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -150.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
        h.step();
    }
    for _ in 0..20 {
        h.step();
    }
    shot(&mut h, "nb-edges-2");
    h.set_size(egui::vec2(820.0, 800.0));
    for _ in 0..10 {
        h.step();
    }
    shot(&mut h, "nb-edges-3");
    // Narrow: the cells stay inside the view (their right edge in sight),
    // the header's right group and the results' bar wrapped.
    let rightmost = h.get_all_by_label("🗑").map(|n| n.rect().max.x).fold(0.0f32, f32::max);
    assert!(rightmost < 800.0, "a cell reaches {rightmost}");
    let filter = h.get_all_by_role(egui::accesskit::Role::TextInput).map(|n| n.rect().max.x).fold(0.0f32, f32::max);
    assert!(filter < 800.0, "the filter reaches {filter}");
}

#[test]
fn notebook_hints_see_ctes_from_cells_above() {
    let text = "-- %%\nwith big as (select id, total from orders where total > 900)\n-- %%\nselect b.\n  from big b\n";
    let (mut h, _g) = harness("nb-cte-hints", text, false);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let tab = h.state().tab().id.clone();
    let cells: Vec<String> = h.state().tab().notebook.as_ref().unwrap().cells.iter().map(|c| c.id.clone()).collect();
    let key2 = crate::app::App::cell_key(&tab, &cells[1]);
    h.state_mut().rt.entry(tab.clone()).or_default().cell_focus = Some(cells[1].clone());
    h.state_mut().rt.entry(key2.clone()).or_default().ed.goto = Some(9);
    h.step();
    h.step();
    key(&mut h, egui::Key::Space, egui::Modifiers::COMMAND);
    h.step();
    let labels: Vec<String> = h.state().rt[&key2].ed.popup.as_ref().map(|p| p.hints.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default();
    assert_eq!(labels, ["id", "total"], "the WITH cell's columns");
    shot(&mut h, "nb-cte-hints");
}

#[test]
fn table_made_from_a_file() {
    let (mut h, _g) = harness("import", "", false);
    // A file in the project folder (where the picker starts).
    let mut csv = String::from("Номер;Товар;Цена;Дата продажи\n");
    for i in 1..=250 {
        csv += &format!("{i};Товар {i};{},{};{:02}.04.2024\n", i * 10, i % 10, i % 28 + 1);
    }
    let project = h.state().session.project.clone().unwrap();
    std::fs::write(project.join("продажи.csv"), csv).unwrap();
    // The tables folder's menu.
    h.get_by_label("Таблицы").click_secondary();
    h.step();
    h.get_by_label_contains("Создать таблицу из файла").click();
    h.step();
    h.step();
    assert!(h.state().import.is_some(), "the window opens");
    h.get_by_label_contains("продажи.csv").click();
    h.step();
    h.state_mut().import.as_mut().unwrap().wait();
    h.step();
    h.step();
    {
        let d = h.state().import.as_ref().unwrap();
        assert_eq!(d.table, "продажи");
        assert_eq!(d.cols.iter().map(|c| (c.name.as_str(), c.ty.as_str())).collect::<Vec<_>>(), [("номер", "INTEGER"), ("товар", "TEXT"), ("цена", "REAL"), ("дата_продажи", "TEXT")]);
    }
    // The key on the first; an index on two fields, picked by their chips.
    {
        let d = h.state_mut().import.as_mut().unwrap();
        d.cols[0].pk = true;
    }
    h.get_by_label("+ Индекс").click();
    h.step();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "дата_продажи").click();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "товар").click();
    h.step();
    h.step();
    assert_eq!(h.state().import.as_ref().unwrap().indexes[0].cols, ["дата_продажи", "товар"]);
    shot(&mut h, "import");
    let ddl = h.state().import.as_ref().unwrap().ddl();
    assert!(ddl[1].contains("(дата_продажи, товар)"), "{ddl:?}");
    h.get_by_label_contains("Создать и загрузить").click();
    for _ in 0..300 {
        h.step();
        if matches!(h.state().import.as_ref().map(|d| &d.stage), Some(crate::import_ui::Stage::Done(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    match h.state().import.as_ref().map(|d| &d.stage) {
        Some(crate::import_ui::Stage::Done(Ok(250))) => {}
        Some(crate::import_ui::Stage::Done(r)) => panic!("{r:?}"),
        _ => panic!("not done"),
    }
    settle(&mut h);
    shot(&mut h, "import-done");
    // In the tree; opened: its rows.
    assert!(h.state().tree.iter().any(|(k, n)| k.1 == ["tables"] && matches!(n, Node::Ready(items) if items.iter().any(|o| o.name == "продажи"))), "the tree shows it");
    h.get_by_label("Открыть таблицу").click();
    settle(&mut h);
    assert!(h.state().import.is_none());
    let id = h.state().tab().id.clone();
    let rows = &h.state().rt[&id].results[0].table.rows;
    assert_eq!(rows.len(), 200);
    assert_eq!(rows[0], [Some("1".into()), Some("Товар 1".into()), Some("10.1".into()), Some("2024-04-02".into())], "numbers and dates as SQL has them");
}

#[test]
fn parameters_asked_before_running() {
    let (mut h, _g) = harness("params", "select :n + 1 as x, '&lit :no' as s, &col as c from orders where id = :n", false);
    settle(&mut h);
    focus_editor(&mut h, 0);
    let ctx = h.ctx.clone();
    h.state_mut().run(&ctx, false, false);
    h.step();
    h.step();
    assert_eq!(h.state().param_ask.len(), 1, "asked first");
    assert_eq!(h.state().param_ask[0].params.iter().map(|p| p.shown()).collect::<Vec<_>>(), [":n", "&col"]);
    // The first one's field has the focus.
    type_text(&mut h, "41");
    h.state_mut().param_values.get_mut("&col").unwrap().0 = "total".into();
    h.step();
    h.step();
    assert_eq!(h.state().param_values[":n"].0, "41");
    shot(&mut h, "params");
    h.get_by_label_contains("Выполнить").click();
    settle(&mut h);
    let id = h.state().tab().id.clone();
    let r = &h.state().rt[&id].results[0];
    assert_eq!(r.table.rows[0][0].as_deref(), Some("42"));
    assert_eq!(r.table.rows[0][1].as_deref(), Some("&lit :no"));
    // Remembered for the next run.
    h.state_mut().run(&ctx, false, false);
    h.step();
    h.step();
    assert_eq!(h.state().param_values[":n"].0, "41");
    key(&mut h, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(h.state().param_ask.is_empty(), "cancelled");
}

#[test]
fn star_opened_into_columns() {
    let (mut h, _g) = harness("star", "select * from orders o", false);
    settle(&mut h);
    let id = h.state().tab().id.clone();
    let rels = h.state().live["lite"].rels.clone();
    let (range, cols, qualify) = crate::sql::complete::star_at("select * from orders o", 7, crate::sql::Dialect::Sqlite, &rels, &[]).unwrap();
    assert_eq!(cols.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["id", "customer_id", "total", "note"]);
    let editor = egui::Id::new(("editor", &id, false));
    h.state_mut().star = Some(crate::dialogs::StarWin { on: vec![true; cols.len()], cols, range, editor, qualify, at: egui::pos2(500.0, 200.0), d: crate::sql::Dialect::Sqlite });
    h.step();
    h.step();
    // One left out.
    h.get_by_label("note").click();
    h.step();
    shot(&mut h, "star");
    h.get_by_label("Заменить * списком").click();
    h.step();
    h.step();
    assert_eq!(h.state().tab().text, "select id, customer_id, total from orders o");
    assert!(h.state().star.is_none());
}

#[test]
fn oracle_connection_nls_lang_field() {
    let (mut h, _g) = harness("nls", "", false);
    let spec = crate::db::Spec { kind: Kind::Oracle, name: "Склад".into(), host: "db.local".into(), port: 1521, database: "ORCLPDB1".into(), user: "shop".into(), nls_lang: "RUSSIAN_RUSSIA.CL8MSWIN1251".into(), ..Default::default() };
    h.state_mut().conn_edit = Some(crate::dialogs::ConnEdit::new(Some(spec)));
    h.step();
    h.step();
    assert!(h.query_by_label_contains("сервер сам переводит в CL8MSWIN1251").is_some(), "the note on its charset");
    shot(&mut h, "conn-nls");
    h.state_mut().conn_edit.as_mut().unwrap().spec.nls_lang = "nonsense.;".into();
    h.step();
    h.step();
    assert!(h.query_by_label_contains("ЯЗЫК_ТЕРРИТОРИЯ.КОДИРОВКА").is_some());
}

#[test]
fn shortcuts_changed_in_the_settings() {
    let (mut h, _g) = harness("keys", "select 1 as a;\nselect 2 as b;", false);
    h.state_mut().settings_open = true;
    h.step();
    h.step();
    shot(&mut h, "settings");
    // The window is in the middle.
    let r = h.get_all_by_label("Общие").map(|n| n.rect()).min_by(|a, b| a.min.x.total_cmp(&b.min.x)).unwrap();
    assert!(r.min.x > 150.0, "{r:?}");
    h.get_all_by_label("Горячие клавиши").min_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x)).unwrap().click();
    h.step();
    h.step();
    shot(&mut h, "settings-keys");
    // F5 of "Run the script" → F9.
    h.get_by_label("F5").click();
    h.step();
    assert!(h.state().key_capture);
    key(&mut h, egui::Key::F9, egui::Modifiers::NONE);
    h.step();
    assert!(!h.state().key_capture);
    assert_eq!(crate::keys::label(&h.state().settings.keys, crate::keys::Cmd::RunScript), "Ctrl+Shift+Enter / F9");
    // A key another command has: shown in red, said whose.
    h.get_all_by_label("+").min_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y)).unwrap().click();
    h.step();
    key(&mut h, egui::Key::E, egui::Modifiers::COMMAND);
    h.step();
    assert_eq!(crate::keys::taken_by(&h.state().settings.keys, &crate::keys::Chord::new(egui::Modifiers::COMMAND, egui::Key::E), crate::keys::Cmd::Explain), Some(crate::keys::Cmd::RunStatement), "the first row's + took Ctrl+E");
    shot(&mut h, "settings-keys-clash");
    h.state_mut().settings.keys.remove(&crate::keys::Cmd::RunStatement);
    h.state_mut().settings_open = false;
    h.step();
    // F9 runs the script now; F5 doesn't.
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::F5, egui::Modifiers::NONE);
    settle(&mut h);
    let id = h.state().tab().id.clone();
    assert!(h.state().rt[&id].results.is_empty(), "F5 does nothing now");
    key(&mut h, egui::Key::F9, egui::Modifiers::NONE);
    settle(&mut h);
    assert_eq!(h.state().rt[&id].results.len(), 2, "F9 ran both");
}

#[test]
fn ghost_of_the_likely_next_words() {
    let text = "select o.customer_id, count(*) as n\n  from orders o\n group by ";
    let (mut h, _g) = harness("ghost", text, false);
    settle(&mut h);
    focus_editor(&mut h, text.len());
    h.step();
    h.step();
    let id = h.state().tab().id.clone();
    assert_eq!(h.state().rt[&id].ed.ghost.as_ref().map(|g| g.1.as_str()), Some("o.customer_id"));
    shot(&mut h, "ghost");
    key(&mut h, egui::Key::Tab, egui::Modifiers::NONE);
    h.step();
    assert_eq!(h.state().tab().text, format!("{text}o.customer_id"));
    // Then ORDER BY: its alias next.
    type_text(&mut h, "\n order by ");
    h.step();
    h.step();
    assert_eq!(h.state().rt[&id].ed.ghost.as_ref().map(|g| g.1.as_str()), Some("o.customer_id"));
    // Something else typed: gone.
    type_text(&mut h, "zz");
    h.step();
    assert!(h.state().rt[&id].ed.ghost.is_none());
}

#[test]
fn caret_keeps_in_view() {
    let text: String = (0..200).map(|i| format!("select {i} from dual;\n")).collect();
    let (mut h, _g) = harness("caret-view", &text, false);
    focus_editor(&mut h, 0);
    for _ in 0..60 {
        key(&mut h, egui::Key::ArrowDown, egui::Modifiers::NONE);
    }
    for _ in 0..5 {
        h.step();
    }
    shot(&mut h, "caret-down");
    let id = h.state().tab().id.clone();
    let c = h.state().rt[&id].ed.cursor.map(|c| c.0).unwrap();
    assert_eq!(h.state().tab().text[..c].matches('\n').count(), 60);
    // Held down (the key repeating every frame): along too.
    h.input_mut().events.push(egui::Event::Key { key: egui::Key::ArrowDown, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
    h.step();
    for _ in 0..80 {
        h.input_mut().events.push(egui::Event::Key { key: egui::Key::ArrowDown, physical_key: None, pressed: true, repeat: true, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    shot(&mut h, "caret-held");
    h.input_mut().events.push(egui::Event::Key { key: egui::Key::ArrowDown, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE });
    h.step();
    // Alt+Shift+↓: the caret's line goes a line down, the caret with it.
    focus_editor(&mut h, 0);
    key(&mut h, egui::Key::ArrowDown, egui::Modifiers::ALT | egui::Modifiers::SHIFT);
    h.step();
    assert!(h.state().tab().text.starts_with("select 1 from dual;\nselect 0 from dual;\nselect 2"), "{}", &h.state().tab().text[..60]);
    assert_eq!(h.state().rt[&id].ed.cursor.map(|c| c.0), Some(20));
    // In a notebook too: the cell's lines past the notebook's bottom.
    let nb = format!("-- %%\n{text}-- %%\nselect 1;\n");
    h.state_mut().tab_mut().text = nb;
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let tab = h.state().tab().id.clone();
    let cells: Vec<String> = h.state().tab().notebook.as_ref().unwrap().cells.iter().map(|c| c.id.clone()).collect();
    let k = crate::app::App::cell_key(&tab, &cells[0]);
    h.state_mut().rt.entry(tab.clone()).or_default().cell_focus = Some(cells[0].clone());
    h.state_mut().rt.entry(k.clone()).or_default().ed.goto = Some(0);
    for _ in 0..3 {
        h.step();
    }
    for _ in 0..60 {
        key(&mut h, egui::Key::ArrowDown, egui::Modifiers::NONE);
    }
    for _ in 0..5 {
        h.step();
    }
    shot(&mut h, "caret-down-nb");
    // (Its scroll area: the notebook's, by its offset.)
    let down = h.state().rt[&k].ed.cursor.unwrap().0;
    assert_eq!(h.state().tab().notebook.as_ref().unwrap().cells[0].text[..down].matches('\n').count(), 60);
    let offs = |h: &Harness<'static, App>| h.ctx.data(|d| d.get_temp::<f32>(egui::Id::new(("nb-offset", &tab)))).unwrap_or(0.0);
    let low = offs(&h);
    assert!(low > 500.0, "the notebook went down after the caret: {low}");
    for _ in 0..50 {
        key(&mut h, egui::Key::ArrowUp, egui::Modifiers::NONE);
    }
    for _ in 0..5 {
        h.step();
    }
    shot(&mut h, "caret-up-nb");
    assert!(offs(&h) < low - 400.0, "and back up: {} from {low}", offs(&h));
}

#[test]
fn formatted_on_paste_and_brackets() {
    let text = "select 1;\n\n";
    let (mut h, _g) = harness("fmt-paste", text, false);
    h.state_mut().settings.format.on_paste = true;
    focus_editor(&mut h, text.len());
    h.input_mut().events.push(egui::Event::Paste("select id,total from orders where total>100".into()));
    h.step();
    h.step();
    let t = h.state().tab().text.clone();
    assert!(t.starts_with("select 1;\n\nSELECT id"), "{t}");
    assert!(t.contains("\n  FROM orders") || t.contains("\nFROM orders") || t.contains(" FROM orders\n"), "{t}");
    let id = h.state().tab().id.clone();
    assert_eq!(h.state().rt[&id].ed.cursor.map(|c| c.0), Some(t.trim_end().len()), "the caret after what was pasted: {t:?}");
    shot(&mut h, "fmt-paste");
    // A selection put in brackets: laid out too.
    let s = "select * from orders where customer_id in select id from customers;";
    h.state_mut().tab_mut().text = s.into();
    let a = s.find("select id").unwrap();
    let b = s.len() - 1;
    h.state_mut().rt.get_mut(&id).unwrap().ed.select = Some((a, b));
    h.step();
    h.step();
    type_text(&mut h, "(");
    h.step();
    let t = h.state().tab().text.clone();
    assert!(t.starts_with("SELECT *"), "{t}");
    assert!(t.contains("(SELECT id") || t.contains("(\n"), "{t}");
    shot(&mut h, "fmt-brackets");
    // Off: as pasted.
    h.state_mut().settings.format.on_paste = false;
    h.state_mut().tab_mut().text = String::new();
    focus_editor(&mut h, 0);
    h.input_mut().events.push(egui::Event::Paste("select a from b".into()));
    h.step();
    assert_eq!(h.state().tab().text, "select a from b");
}

#[test]
fn paste_closes_brackets_undone_in_steps() {
    let (mut h, _g) = harness("paste-close", "", false);
    h.state_mut().settings.format.on_paste = true;
    focus_editor(&mut h, 0);
    h.input_mut().events.push(egui::Event::Paste("select count(".into()));
    h.step();
    h.step();
    let t = h.state().tab().text.clone();
    assert!(t.to_ascii_lowercase().starts_with("select count()"), "{t:?}");
    let id = h.state().tab().id.clone();
    let caret = t.find('(').unwrap() + 1;
    assert_eq!(h.state().rt[&id].ed.cursor, Some((caret, caret)), "the caret inside the brackets");
    // Ctrl+Z: first the fix-up (the paste as it was), then the paste.
    key(&mut h, egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(h.state().tab().text, "select count(");
    key(&mut h, egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(h.state().tab().text, "");
    // And again forward.
    key(&mut h, egui::Key::Z, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    assert_eq!(h.state().tab().text, "select count(");
    key(&mut h, egui::Key::Z, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    assert_eq!(h.state().tab().text, t);
    // Closed later on in the statement: nothing added.
    h.state_mut().settings.format.on_paste = false;
    h.state_mut().tab_mut().text = "select ) from t".into();
    focus_editor(&mut h, 7);
    h.input_mut().events.push(egui::Event::Paste("f(".into()));
    h.step();
    assert_eq!(h.state().tab().text, "select f() from t");
}

#[test]
fn block_past_line_ends_sticks() {
    let text = "select a,\n       bb,\n       c\n  from orders;\n";
    let (mut h, _g) = harness("block-stick", text, false);
    focus_editor(&mut h, 0);
    let id = h.state().tab().id.clone();
    let r = h.state().rt[&id].ed.text_rect.unwrap();
    let font = egui::FontId::monospace(h.state().settings.font_size);
    let (cw, rh) = h.ctx.fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    let at = |line: f32, col: f32| egui::pos2(r.left() + col * cw, r.top() + 4.0 + (line + 0.5) * rh);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
    h.input_mut().events.push(egui::Event::PointerMoved(at(0.0, 14.0)));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: at(0.0, 14.0), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::ALT });
    h.step();
    for l in [0.7, 1.4, 2.0] {
        h.input_mut().events.push(egui::Event::PointerMoved(at(l, 14.0)));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: at(2.0, 14.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::ALT });
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.step();
    h.step();
    type_text(&mut h, "')");
    assert_eq!(h.state().tab().text, "select a,')\n       bb,')\n       c')\n  from orders;\n");
    shot(&mut h, "block-stick");
    key(&mut h, egui::Key::Backspace, egui::Modifiers::NONE);
    assert_eq!(h.state().tab().text, "select a,'\n       bb,'\n       c'\n  from orders;\n");
    // Ctrl+Z: the sticking undone (the column as padded), then the typing.
    // (Back to as it was after the first char: the sticking is next.)
    key(&mut h, egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(h.state().tab().text, "select a,     '\n       bb,    '\n       c      '\n  from orders;\n");
    key(&mut h, egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(h.state().tab().text, text);
}

#[test]
fn side_panel_put_away_and_back() {
    let (mut h, _g) = harness("side-away", SCRIPT, false);
    let w = h.state().session.side_width;
    shot(&mut h, "side-out");
    // Its button at the window's corner, staying there.
    let at = h.get_by_label("Скрыть боковую панель").rect();
    h.get_by_label("Скрыть боковую панель").click();
    for _ in 0..30 {
        h.step();
    }
    assert!(h.state().session.side_hidden);
    shot(&mut h, "side-hidden");
    assert_eq!(h.get_by_label("Показать боковую панель").rect(), at, "the button stays put");
    // The menus there now.
    assert!(h.query_by_label("Настройки").is_some());
    let ws = h.get_by_label_contains("Отчёты").rect();
    assert!(ws.left() < 300.0, "the workspaces moved left: {ws:?}");
    h.get_by_label("Показать боковую панель").click();
    for _ in 0..30 {
        h.step();
    }
    assert!(!h.state().session.side_hidden);
    assert_eq!(h.state().session.side_width, w, "as wide as it was");
    // Its edge pulled a little past the narrowest: stays; well past: away.
    let drag = |h: &mut Harness<'static, App>, to: f32, release: bool| {
        let from = egui::pos2(h.state().session.side_width, 400.0);
        h.input_mut().events.push(egui::Event::PointerMoved(from));
        h.step();
        h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.step();
        for k in 1..=10 {
            h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(from.x + (to - from.x) * k as f32 / 10.0, 400.0)));
            h.step();
        }
        if release {
            h.input_mut().events.push(egui::Event::PointerButton { pos: egui::pos2(to, 400.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        }
        for _ in 0..30 {
            h.step();
        }
    };
    drag(&mut h, 180.0, true);
    assert!(!h.state().session.side_hidden, "a little past: still there");
    assert!((h.state().session.side_width - 240.0).abs() < 2.0, "{}", h.state().session.side_width);
    shot(&mut h, "side-narrowest");
    drag(&mut h, 60.0, true);
    assert!(h.state().session.side_hidden, "well past: away");
    shot(&mut h, "side-pulled-away");
    // Dragged back out from the window's edge.
    let from = egui::pos2(1.0, 400.0);
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    for k in 1..=10 {
        h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(30.0 * k as f32, 400.0)));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: egui::pos2(300.0, 400.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    for _ in 0..30 {
        h.step();
    }
    assert!(!h.state().session.side_hidden, "pulled back out");
    shot(&mut h, "side-back");
    // Ctrl+B: away and back.
    key(&mut h, egui::Key::B, egui::Modifiers::COMMAND);
    assert!(h.state().session.side_hidden);
    key(&mut h, egui::Key::B, egui::Modifiers::COMMAND);
    assert!(!h.state().session.side_hidden);
}

#[test]
fn side_panel_slides_with_the_sheet() {
    let (mut h, _g) = harness("side-slide", SCRIPT, false);
    h.state_mut().tab_mut().set_notebook(true, crate::sql::Dialect::Sqlite);
    // (Animations are off in tests: on here, as in the app.)
    h.ctx.all_styles_mut(|s| s.animation_time = 0.2);
    settle(&mut h);
    let sheet = |h: &Harness<'static, App>| h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(crate::ui::SHEET_KEY))).unwrap().left();
    for (label, from, to) in [("Скрыть боковую панель", 280.0, 6.0), ("Показать боковую панель", 6.0, 280.0)] {
        assert!((sheet(&h) - from).abs() < 1.0, "{label}: {}", sheet(&h));
        h.get_by_label(label).click();
        let mut xs = Vec::new();
        for _ in 0..30 {
            h.step();
            xs.push(sheet(&h));
        }
        // The sheet's edge goes with the panel's, a little every frame, no
        // standing still on the way, no jumps.
        // (The click's own frame laid the sheet out before it.)
        let moving: Vec<f32> = xs.iter().copied().skip_while(|x| (x - from).abs() < 0.01).take_while(|x| (x - to).abs() > 0.5).collect();
        assert!(moving.len() >= 5, "{label}: {xs:?}");
        let mut prev = from;
        for x in &moving {
            assert!((x - prev).abs() > 0.1 && (x - prev).abs() < 80.0 && (to - x) * (to - prev) >= 0.0, "{label}: {xs:?}");
            prev = *x;
        }
        assert!((xs.last().unwrap() - to).abs() < 0.5, "{label}: {xs:?}");
    }
}

/// A mouse drag from `a` to `b` in `n` steps (the button let go at the end).
fn drag_to(h: &mut Harness<'static, App>, a: egui::Pos2, b: egui::Pos2, n: usize) {
    h.input_mut().events.push(egui::Event::PointerMoved(a));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    for k in 1..=n {
        h.input_mut().events.push(egui::Event::PointerMoved(a + (b - a) * k as f32 / n as f32));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    for _ in 0..3 {
        h.step();
    }
}

#[test]
fn board_scripts_as_schemes() {
    use crate::board::Kind as B;
    let script = "select o.id, o.total, c.name\n  from orders o join customers c on c.id = o.customer_id;\n\ninsert into orders_archive (id, total)\nselect id, total from orders where total > 100;\n\ninsert into orders (customer_id, total) values (1, 10), (2, 20);\n\nupdate orders set note = 'checked', total = total * 1.2 where id = 7;\n\ncreate trigger orders_note after insert on orders\nbegin\n  update orders set note = 'new'\n   where id = new.id;\n  insert into audit (what) values ('order');\nend;\n";
    let (mut h, _g) = harness("board-scripts", script, false);
    settle(&mut h);
    let tab = h.state().tab().id.clone();
    let id = h.state_mut().new_board();
    settle(&mut h);
    {
        let app = h.state_mut();
        let mut open = std::mem::take(app.boards.open.get_mut(&id).unwrap());
        crate::board_ui::add_notebook(app, &mut open.board, &tab, egui::pos2(0.0, 0.0));
        open.board.fit_heights();
        app.boards.open.insert(id.clone(), open);
        app.board_ui.sel.clear();
        app.board_ui.lib = false;
    }
    settle(&mut h);
    let b = h.state().boards.open[&id].board.clone();
    let it = b.items.iter().find(|i| matches!(i.kind, B::Notebook { .. })).unwrap().clone();
    let B::Notebook { snap, .. } = &it.kind else { unreachable!() };
    let kinds: Vec<&str> = snap.parts.iter().map(|p| match &p.what {
        crate::board::What::Query { .. } => "query",
        crate::board::What::Insert { source: Some(_), .. } => "insert-select",
        crate::board::What::Insert { .. } => "insert-values",
        crate::board::What::Change { .. } => "change",
        crate::board::What::Code { .. } => "code",
        _ => "other",
    }).collect();
    assert_eq!(kinds, ["query", "insert-select", "insert-values", "change", "code"], "{:?}", snap.parts);
    // Seen at full size from its top-left corner.
    {
        let o = h.state_mut().boards.open.get_mut(&id).unwrap();
        o.board.zoom = 1.0;
        o.board.pan = [it.rect[0] - 20.0, it.rect[1] - 20.0];
    }
    settle(&mut h);
    shot(&mut h, "board-scripts");
    // A workspace: its frame round each tab's.
    {
        let app = h.state_mut();
        app.ws_mut().tabs.iter_mut().find(|t| t.title == "Черновик").unwrap().text = "select id, name from customers;".into();
        let ws = app.ws().id.clone();
        let mut open = std::mem::take(app.boards.open.get_mut(&id).unwrap());
        let at = egui::pos2(it.rect[0] + it.rect[2] / 2.0, it.rect[1] + it.rect[3] + 600.0);
        crate::board_ui::put_ws(app, &mut open.board, &ws, at);
        open.board.fit_heights();
        let w = open.board.items.last().unwrap().clone();
        open.board.pan = [w.rect[0] - 20.0, w.rect[1] - 20.0];
        open.board.zoom = 0.8;
        app.boards.open.insert(id.clone(), open);
        app.board_ui.sel.clear();
        let B::Workspace { tabs, .. } = &w.kind else { panic!("{:?}", w.kind) };
        assert_eq!(tabs.len(), 2);
    }
    settle(&mut h);
    shot(&mut h, "board-ws");
    // A notebook: its markdown notes over the statements after them.
    {
        use crate::config::{Cell, CellKind, Notebook};
        let app = h.state_mut();
        let cell = |kind, text: &str| Cell { kind, text: text.into(), ..Default::default() };
        let tb = app.ws_mut().tabs.iter_mut().find(|t| t.title == "Черновик").unwrap();
        tb.notebook = Some(Notebook { on: true, cells: vec![
            cell(CellKind::Markdown, "# Заказы\nКрупные заказы и их клиенты за месяц"),
            cell(CellKind::Sql, "select id, total from orders where total > 100;"),
            cell(CellKind::Sql, "select c.name, count(*) n from customers c join orders o on o.customer_id = c.id group by c.name;"),
            cell(CellKind::Markdown, "## Архив\nСтарые заказы переносим в архив"),
            cell(CellKind::Sql, "insert into orders_archive select * from orders where id < 10;"),
        ] });
        let tid = tb.id.clone();
        let mut open = std::mem::take(app.boards.open.get_mut(&id).unwrap());
        let nid = crate::board_ui::add_notebook(app, &mut open.board, &tid, egui::pos2(5000.0, 0.0)).unwrap();
        open.board.fit_heights();
        let n = open.board.item(nid).unwrap().clone();
        open.board.pan = [n.rect[0] - 20.0, n.rect[1] - 20.0];
        open.board.zoom = 1.0;
        app.boards.open.insert(id.clone(), open);
        app.board_ui.sel.clear();
    }
    settle(&mut h);
    shot(&mut h, "board-md");
}

#[test]
fn boards_drawn_linked_and_filled() {
    use crate::board::Kind as B;
    let (mut h, _g) = harness("boards", SCRIPT, false);
    // Side panel → Boards → New board: named as it's made.
    h.get_by_label_contains("Доски: схемы").click();
    settle(&mut h);
    h.get_by_label_contains("Новая доска").click();
    settle(&mut h);
    key(&mut h, egui::Key::A, egui::Modifiers::COMMAND);
    type_text(&mut h, "Схема заказов");
    key(&mut h, egui::Key::Enter, egui::Modifiers::NONE);
    settle(&mut h);
    let id = h.state().tab().board.clone().expect("a board's tab");
    assert_eq!(h.state().boards.name(&id), "Схема заказов");
    assert_eq!(h.state().tab().title, "Схема заказов");
    let canvas: egui::Rect = h.ctx.data(|d| d.get_temp(egui::Id::new("board-canvas"))).unwrap();
    let board = |h: &Harness<'static, App>| h.state().boards.open[&id].board.clone();
    let at = |h: &Harness<'static, App>, w: egui::Pos2| {
        let b = &h.state().boards.open[&id].board;
        canvas.min + (w.to_vec2() - egui::vec2(b.pan[0], b.pan[1])) * b.zoom
    };
    // A frame drawn, its text typed in place.
    h.get_by_label_contains("Рамка (R)").click();
    settle(&mut h);
    drag_to(&mut h, canvas.min + egui::vec2(60.0, 60.0), canvas.min + egui::vec2(260.0, 150.0), 6);
    type_text(&mut h, "Начало");
    key(&mut h, egui::Key::Escape, egui::Modifiers::NONE);
    settle(&mut h);
    assert!(matches!(&board(&h).items[0].kind, B::Frame { text, .. } if text == "Начало"), "{:?}", board(&h).items);
    // Two tables: one named in the properties, the other given a field.
    h.get_by_label_contains("Таблица-сущность (E)").click();
    settle(&mut h);
    click_at(&mut h, canvas.min + egui::vec2(340.0, 60.0), egui::PointerButton::Primary);
    settle(&mut h);
    key(&mut h, egui::Key::A, egui::Modifiers::COMMAND);
    type_text(&mut h, "customers");
    settle(&mut h);
    // (Its window, opened as it was made, closed.)
    h.get_by_label("Готово").click();
    settle(&mut h);
    h.get_by_label_contains("Таблица-сущность (E)").click();
    settle(&mut h);
    click_at(&mut h, canvas.min + egui::vec2(340.0, 260.0), egui::PointerButton::Primary);
    settle(&mut h);
    h.get_by_label_contains("Поле").click();
    settle(&mut h);
    shot(&mut h, "board-entity");
    h.get_by_label("Готово").click();
    settle(&mut h);
    let b = board(&h);
    let (a, c) = (b.items[1].clone(), b.items[2].clone());
    assert!(matches!(&a.kind, B::Entity { name, .. } if name == "customers"), "{:?}", a.kind);
    assert!(matches!(&c.kind, B::Entity { fields, .. } if fields.len() == 2), "{:?}", c.kind);
    // A link from the second's second field to the first's id.
    h.get_by_label_contains("Связь (A)").click();
    settle(&mut h);
    let from = at(&h, egui::pos2(c.rect[0] + 40.0, c.row_y(1)));
    let to = at(&h, egui::pos2(a.rect[0] + 40.0, a.row_y(0)));
    drag_to(&mut h, from, to, 8);
    settle(&mut h);
    let b = board(&h);
    assert_eq!(b.links.len(), 1, "{:?}", b.links);
    assert_eq!((b.links[0].from, b.links[0].from_field, b.links[0].to, b.links[0].to_field), (c.id, Some(1), a.id, Some(0)));
    // A table dragged from the library's tree: its columns come along.
    h.state_mut().open_nodes.insert(("lite".into(), vec![]));
    h.state_mut().open_nodes.insert(("lite".into(), vec!["tables".into()]));
    settle(&mut h);
    let orders = h.get_by_label("orders").rect().center();
    drag_to(&mut h, orders, canvas.min + egui::vec2(150.0, 330.0), 12);
    settle(&mut h);
    settle(&mut h);
    let b = board(&h);
    let obj = b.items.iter().find(|i| matches!(i.kind, B::Object { .. })).expect("the table on the board");
    assert!(matches!(&obj.kind, B::Object { name, cols, .. } if name == "orders" && cols.iter().any(|c| c.0 == "customer_id")), "{:?}", obj.kind);
    // A script from a tab: a table per query, UPDATE's scheme.
    h.get_by_label_contains("Добавить").click();
    settle(&mut h);
    h.get_by_label("📜 Клиенты").click();
    settle(&mut h);
    let b = board(&h);
    let nb = b.items.iter().find(|i| matches!(i.kind, B::Notebook { .. })).expect("the script on the board").clone();
    let B::Notebook { snap, .. } = &nb.kind else { unreachable!() };
    assert_eq!(snap.parts.len(), 3, "{:?}", snap.parts);
    assert!(matches!(&snap.parts[0].what, crate::board::What::Query { cols, .. } if cols.iter().map(|c| c.0.as_str()).collect::<Vec<_>>() == ["name", "orders", "total", "kind"]), "{:?}", snap.parts[0].what);
    assert!(matches!(&snap.parts[2].what, crate::board::What::Change { verb, target, cols } if verb == "UPDATE" && target == "orders" && cols[0].0 == "note"), "{:?}", snap.parts[2].what);
    // As tall as what it holds; a click on a statement shows its script.
    assert_eq!(nb.rect[3], nb.natural_h().unwrap());
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("board-fit"), true));
    settle(&mut h);
    let inside = nb.inside();
    let upd = inside.parts[2].2;
    h.get_by_label_contains("Выбор и перемещение").click();
    settle(&mut h);
    let p = at(&h, upd.center());
    click_at(&mut h, p, egui::PointerButton::Primary);
    settle(&mut h);
    assert_eq!(h.state().board_ui.sub, Some((0, 2)));
    assert!(h.state().board_ui.win.is_none(), "a click only picks");
    // A double click: the window, the statement's script in it.
    // (Past the earlier clicks' time: else a triple click.)
    for _ in 0..50 {
        h.step();
    }
    click_at(&mut h, p, egui::PointerButton::Primary);
    click_at(&mut h, p, egui::PointerButton::Primary);
    settle(&mut h);
    assert!(h.state().board_ui.win.is_some());
    // (Its name on top, its script open under it, then the list.)
    assert!(h.query_all_by_label("UPDATE orders").count() >= 2, "the statement picked in the properties");
    h.get_by_label("Готово").click();
    settle(&mut h);
    // A link from a field of a table inside the UPDATE's scheme to the
    // orders table on the board.
    let nb = board(&h).items.iter().find(|i| i.id == nb.id).unwrap().clone();
    let obj_id = board(&h).items.iter().find(|i| matches!(i.kind, B::Object { .. })).unwrap().id;
    let deep = nb.anchors().into_iter().find(|a| a.at == crate::board::At { tab: None, part: Some(2), table: Some(1) }).expect("the UPDATE's table");
    let from = at(&h, egui::pos2(deep.rect.center().x, deep.rows[0]));
    let obj = board(&h).item(obj_id).unwrap().clone();
    let to = at(&h, egui::pos2(obj.rect[0] + 20.0, obj.row_y(0)));
    h.get_by_label_contains("Связь (A)").click();
    settle(&mut h);
    drag_to(&mut h, from, to, 10);
    settle(&mut h);
    let l = board(&h).links.last().unwrap().clone();
    assert_eq!((l.from, l.from_at, l.from_field, l.to, l.to_at, l.to_field), (nb.id, deep.at, Some(0), obj_id, crate::board::At::default(), Some(0)), "{l:?}");
    assert!(board(&h).link_path(&l).is_some());
    h.get_by_label_contains("Выбор и перемещение").click();
    settle(&mut h);
    // From the library: a whole workspace, a sticky note, a dashed line.
    let ws = h.query_all_by_label("Отчёты").map(|n| n.rect()).find(|r| r.right() < canvas.min.x && r.top() > canvas.top()).expect("the workspace in the library").center();
    drag_to(&mut h, ws, canvas.min + egui::vec2(200.0, 520.0), 12);
    settle(&mut h);
    let b = board(&h);
    let w = b.items.iter().find(|i| matches!(i.kind, B::Workspace { .. })).expect("the workspace on the board");
    assert!(matches!(&w.kind, B::Workspace { name, tabs, .. } if name == "Отчёты" && tabs.len() == 1 && tabs[0].title == "Клиенты"), "{:?}", w.kind);
    let note = h.get_by_label("Стикер").rect().center();
    drag_to(&mut h, note, canvas.min + egui::vec2(80.0, 440.0), 10);
    key(&mut h, egui::Key::Escape, egui::Modifiers::NONE);
    settle(&mut h);
    // (The databases' section folded: the lines in sight.)
    h.get_by_label("Базы данных").click();
    settle(&mut h);
    let line = h.get_by_label("Пунктирная стрелка").rect().center();
    drag_to(&mut h, line, canvas.min + egui::vec2(300.0, 440.0), 10);
    settle(&mut h);
    let b = board(&h);
    assert!(b.items.iter().any(|i| matches!(i.kind, B::Frame { shape: crate::board::Shape::Note, .. })));
    assert!(b.items.iter().any(|i| matches!(&i.kind, B::Line { style, .. } if style.dash == crate::board::Dash::Dashed)));
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("board-fit"), true));
    settle(&mut h);
    shot(&mut h, "board");
    // A step back takes the line away; forth again.
    h.input_mut().events.push(egui::Event::PointerMoved(canvas.center()));
    h.step();
    key(&mut h, egui::Key::Z, egui::Modifiers::COMMAND);
    settle(&mut h);
    assert!(!board(&h).items.iter().any(|i| matches!(i.kind, B::Line { .. })));
    key(&mut h, egui::Key::Z, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    settle(&mut h);
    assert!(board(&h).items.iter().any(|i| matches!(i.kind, B::Line { .. })));
    // On disk as on screen.
    let saved = crate::board::load(&id).expect("saved");
    assert_eq!(saved.items, board(&h).items);
    assert_eq!(saved.links, board(&h).links);
}

/// Two notebooks on one connection share its session: a stop in one ends
/// its own run only — the other's goes on.
#[test]
fn stop_in_one_notebook_spares_another() {
    let forever = "-- %%\nwith recursive n(x) as (select 1 union all select x + 1 from n) select count(*) from n;\n";
    let (mut h, _g) = harness("nb-stop-one", forever, false);
    h.state_mut().toggle_notebook();
    settle(&mut h);
    let ctx = h.ctx.clone();
    let tab_of = |h: &Harness<'static, App>| {
        let t = h.state().tab();
        crate::app::App::cell_key(&t.id, &t.notebook.as_ref().unwrap().cells[0].id)
    };
    let a = tab_of(&h);
    h.state_mut().run_cells(&ctx, None);
    for _ in 0..20 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state().rt[&a].run.is_some());
    // The second notebook: its run waits its turn behind the first.
    h.state_mut().ws_mut().active = 1;
    h.state_mut().tab_mut().text = "-- %%\nselect 42;\n".into();
    h.state_mut().toggle_notebook();
    for _ in 0..10 {
        h.step();
    }
    let b = tab_of(&h);
    h.state_mut().run_cells(&ctx, None);
    for _ in 0..10 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state().rt[&b].run.is_some(), "waits its turn");
    // Stopped there: only its own run ends.
    h.state_mut().cancel();
    for _ in 0..30 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    let rb = &h.state().rt[&b];
    assert!(rb.run.is_none() && rb.last_run.is_some_and(|l| l.1), "{:?}", rb.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    let ra = &h.state().rt[&a];
    assert!(ra.run.is_some() && ra.run.as_ref().unwrap().stop_asked.is_none(), "the first goes on: {:?}", ra.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    // Run again there, then stopped in the first: the second's goes on.
    h.state_mut().run_cells(&ctx, None);
    h.step();
    h.state_mut().ws_mut().active = 0;
    h.state_mut().cancel();
    for _ in 0..50 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    settle(&mut h);
    let ra = &h.state().rt[&a];
    assert!(ra.run.is_none() && ra.last_run.is_some_and(|l| l.1), "{:?}", ra.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    let rb = &h.state().rt[&b];
    assert!(rb.run.is_none() && rb.last_run.is_some_and(|l| !l.1), "{:?}", rb.log.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert!(rb.results.first().is_some_and(|v| v.table.rows.first().and_then(|r| r[0].as_deref()) == Some("42")), "the second ran");
}
