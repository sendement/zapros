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
        let busy = st.rt.values().any(|r| r.run.is_some() || r.check_due.is_some()) || st.live.values().any(|l| l.state == ConnState::Connecting || l.rels_loading) || st.tree.values().any(|n| matches!(n, Node::Loading));
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
    for page in 0..6usize {
        h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-page"), page));
        settle(&mut h);
        sizes.push(h.ctx.memory(|m| m.area_rect(egui::Id::new("settings-window"))).map(|r| r.size()));
    }
    assert!(sizes[0].is_some() && sizes.iter().all(|s| *s == sizes[0]), "{sizes:?}");
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
    // The panel's icons centred on it.
    let w = h.state().session.side_width;
    let first = h.get_by_label("Подключения и объекты баз").rect();
    let last = h.get_by_label("Клавиши и о программе").rect();
    let (l, r) = (first.left() as f32, last.right() as f32);
    assert!(((l - 6.0) - (w - 6.0 - r)).abs() < 3.0, "centred: {l} … {r} in {w}");
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
