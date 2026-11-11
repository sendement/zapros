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
    panic!("still busy");
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
    h.state_mut().settings_open = false;
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
