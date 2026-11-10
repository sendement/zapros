//! Against real servers, when told where they are:
//! ZAPROS_TEST_PG=host:port:db:user:password, ZAPROS_TEST_CH=…, ZAPROS_TEST_ORA=host:port:service:user:password.

use std::time::{Duration, Instant};

use super::*;

fn spec(var: &str, kind: Kind) -> Option<(Spec, String)> {
    let v = std::env::var(var).ok()?;
    let p: Vec<&str> = v.split(':').collect();
    let s = Spec { kind, host: p[0].into(), port: p[1].parse().unwrap(), database: p[2].into(), user: p[3].into(), save_password: false, tls: Tls::Disable, ..Default::default() };
    Some((s, p[4].into()))
}

fn rows(o: Outcome) -> Table {
    match o {
        Outcome::Rows(t) => t,
        other => panic!("{other:?}"),
    }
}

fn common(s: &mut dyn Session, big: &str, slow: &str, bad: &str) {
    let mut notes = Vec::new();
    let t = rows(s.run_limit(big, 100, &mut |n| notes.push(n)).unwrap());
    assert_eq!(t.rows.len(), 100);
    assert!(t.more);
    // The session is fine after a cut-short query.
    let t = rows(s.run_limit("select 1 as x", 10, &mut |_| {}).unwrap());
    assert_eq!(t.rows[0][0].as_deref(), Some("1"));
    assert!(t.cols[0].numeric);
    let e = s.run_limit(bad, 10, &mut |_| {}).unwrap_err();
    assert!(e.at.is_some(), "{e:?}");
    assert!(!e.lost);
    // Cancel from another thread.
    let c = s.canceller();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        c();
    });
    let t0 = Instant::now();
    let r = s.run_limit(slow, 10, &mut |_| {});
    assert!(t0.elapsed() < Duration::from_secs(8), "{:?}", t0.elapsed());
    assert!(r.is_err(), "{r:?}");
    assert!(s.alive());
    assert!(s.run_limit("select 2", 10, &mut |_| {}).is_ok());
    let root = s.children(&[]).unwrap();
    assert!(!root.is_empty());
}

#[test]
fn postgres() {
    let Some((sp, pw)) = spec("ZAPROS_TEST_PG", Kind::Postgres) else { return };
    let mut s = open(&sp, &pw).unwrap();
    assert!(s.version().starts_with("PostgreSQL"));
    common(&mut *s, "select g from generate_series(1, 100000) g", "select pg_sleep(30)", "select * from customers where idd = 1");
    let mut notes = Vec::new();
    rows(s.run_limit("select vip(1)", 10, &mut |n| notes.push(n)).unwrap());
    let r = s.run_limit("do $$ begin raise notice 'hello %', 42; end $$", 10, &mut |n| notes.push(n)).unwrap();
    assert!(matches!(r, Outcome::Done(_)));
    assert!(notes.iter().any(|n| n.contains("hello 42")), "{notes:?}");
    let t = rows(s.run_limit("select id, name, total from big_orders limit 3", 10, &mut |_| {}).unwrap());
    assert_eq!(t.cols[2].ty, "numeric");
    assert!(matches!(s.run_limit("update customers set active = true where id = 1", 10, &mut |_| {}).unwrap(), Outcome::Done(Some(1))));
    let schemas = s.children(&[]).unwrap();
    assert_eq!(schemas[0].name, "public");
    let p = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let tables = s.children(&p(&["public", "tables"])).unwrap();
    assert!(tables.iter().any(|t| t.name == "orders"));
    let fns = s.children(&p(&["public", "functions"])).unwrap();
    assert!(fns.iter().any(|f| f.name == "vip(p_id integer)"), "{fns:?}");
    assert!(s.children(&p(&["public", "procedures"])).unwrap().iter().any(|f| f.name.starts_with("touch_customer")));
    assert!(s.children(&p(&["public", "tables", "orders", "indexes"])).unwrap().iter().any(|i| i.name == "orders_customer"));
    assert_eq!(s.children(&p(&["public", "views", "big_orders"])).unwrap().len(), 6);
    let ddl = s.source(&p(&["public", "tables", "orders"]), ObjKind::Table).unwrap();
    assert!(ddl.contains("CREATE TABLE public.orders") && ddl.contains("FOREIGN KEY") && ddl.contains("CREATE INDEX orders_customer"), "{ddl}");
    assert!(s.source(&p(&["public", "functions", "vip(p_id integer)"]), ObjKind::Function).unwrap().contains("raise notice"));
    assert!(s.source(&p(&["public", "views", "big_orders"]), ObjKind::View).unwrap().contains("CREATE OR REPLACE VIEW"));
    assert!(s.source(&p(&["public", "types", "order_state"]), ObjKind::Type).unwrap().contains("ENUM ('new'"));
    assert!(s.source(&p(&["public", "sequences", "invoice_no"]), ObjKind::Sequence).unwrap().contains("START WITH 1000"));
    let rels = s.relations(&[]).unwrap();
    assert!(rels.iter().any(|r| r.name == "customers" && r.cols.iter().any(|c| c.0 == "email")));
    // Wrong password: an error, quickly, not a hang.
    let e = open(&sp, "nope").err().unwrap();
    assert!(e.message.contains("28P01") || e.message.to_lowercase().contains("password"), "{}", e.message);
}

#[test]
fn clickhouse() {
    let Some((sp, pw)) = spec("ZAPROS_TEST_CH", Kind::ClickHouse) else { return };
    let mut s = open(&sp, &pw).unwrap();
    assert!(s.version().starts_with("ClickHouse"));
    common(&mut *s, "select number from numbers(100000000)", "select sleepEachRow(1) from numbers(30) settings max_block_size = 1", "select * from events where kindd = 1");
    let t = rows(s.run_limit("select kind, count() c from events group by kind order by kind", 10, &mut |_| {}).unwrap());
    assert_eq!(t.rows.len(), 3);
    assert_eq!(t.cols[1].ty, "UInt64");
    // A session: SET and temporary tables live on.
    s.run_limit("create temporary table tmp (x UInt8)", 10, &mut |_| {}).unwrap();
    s.run_limit("insert into tmp values (1), (2)", 10, &mut |_| {}).unwrap();
    assert_eq!(rows(s.run_limit("select sum(x) from tmp", 10, &mut |_| {}).unwrap()).rows[0][0].as_deref(), Some("3"));
    let p = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert!(s.children(&p(&["default", "tables"])).unwrap().iter().any(|t| t.name == "events"));
    assert!(s.children(&p(&["default", "views"])).unwrap().iter().any(|t| t.name == "buys"));
    assert!(s.source(&p(&["default", "tables", "events"]), ObjKind::Table).unwrap().contains("MergeTree"));
    assert!(s.relations(&[]).unwrap().iter().any(|r| r.name == "events"));
    // Own FORMAT: text as it came.
    let t = rows(s.run_limit("select 1 format TSV", 10, &mut |_| {}).unwrap());
    assert_eq!(t.rows[0][0].as_deref(), Some("1"));
    let e = open(&sp, "nope").err().unwrap();
    assert!(e.message.contains("516") || e.message.contains("Authentication"), "{}", e.message);
}

#[cfg(feature = "oracle")]
#[test]
fn oracle() {
    let Some((sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) else { return };
    let mut s = open(&sp, &pw).unwrap();
    common(&mut *s, "select level from dual connect by level <= 100000", "select count(*) from all_objects a, all_objects b, all_objects c", "select dummyy from dual");
    s.run_limit("begin execute immediate 'drop table people'; exception when others then null; end;", 10, &mut |_| {}).unwrap();
    s.run_limit("create table people (id number primary key, name varchar2(100))", 10, &mut |_| {}).unwrap();
    assert!(matches!(s.run_limit("insert into people select level, 'p' || level from dual connect by level <= 5", 10, &mut |_| {}).unwrap(), Outcome::Done(Some(5))));
    let mut notes = Vec::new();
    s.run_limit("begin dbms_output.put_line('hi from plsql'); end;", 10, &mut |n| notes.push(n)).unwrap();
    assert_eq!(notes, vec!["hi from plsql"]);
    let e = s.run_limit("create or replace procedure broken is begin nul; end;", 10, &mut |_| {}).unwrap_err();
    assert!(e.message.contains("PLS-") || e.message.contains("errors"), "{}", e.message);
    s.run_limit("create or replace procedure fine(p number) is begin null; end;", 10, &mut |_| {}).unwrap();
    let user = sp.user.to_uppercase();
    let p = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert!(s.children(&p(&[&user, "tables"])).unwrap().iter().any(|t| t.name == "PEOPLE"));
    assert!(s.children(&p(&[&user, "procedures"])).unwrap().iter().any(|t| t.name == "FINE"));
    assert!(s.source(&p(&[&user, "tables", "PEOPLE"]), ObjKind::Table).unwrap().contains("CREATE TABLE"));
    assert!(s.source(&p(&[&user, "procedures", "FINE"]), ObjKind::Procedure).unwrap().contains("PROCEDURE"));
    assert!(s.relations(&[]).unwrap().iter().any(|r| r.name == "PEOPLE"));
}

#[test]
fn sqlite() {
    let dir = std::env::temp_dir().join(format!("zapros-test-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sp = Spec { kind: Kind::Sqlite, database: dir.join("t.db").to_string_lossy().into_owned(), ..Default::default() };
    let mut s = open(&sp, "").unwrap();
    s.run_limit("create table t (id integer primary key, name text, v real)", 10, &mut |_| {}).unwrap();
    s.run_limit("create view tv as select * from t", 10, &mut |_| {}).unwrap();
    assert!(matches!(s.run_limit("insert into t (name, v) values ('a', 1.5), ('b', null)", 10, &mut |_| {}).unwrap(), Outcome::Done(Some(2))));
    let t = rows(s.run_limit("select * from t", 1, &mut |_| {}).unwrap());
    assert!(t.more && t.rows.len() == 1 && t.cols[2].numeric);
    let big = "with recursive c(x) as (select 1 union all select x + 1 from c) select x from c";
    common(&mut *s, big, "with recursive c(x) as (select 1 union all select x + 1 from c) select count(*) from c", "select idd from t");
    assert_eq!(s.children(&["tables".into(), "t".into()]).unwrap().len(), 3);
    let src = s.source(&["views".into(), "tv".into()], ObjKind::View).unwrap();
    assert!(src.to_lowercase().contains("create view tv"), "{src:?}");
    std::fs::remove_dir_all(dir).ok();
}

/// One statement as the worker runs it by hand (before / run / after).
fn step(s: &mut dyn Session, sql: &str) -> Res<Outcome> {
    let r = s.tx_before(sql).and_then(|_| s.run_limit(sql, 100, &mut |_| {}));
    s.tx_after(sql, r.is_ok());
    r
}

fn count(s: &mut dyn Session, sql: &str) -> String {
    rows(s.run_limit(sql, 10, &mut |_| {}).unwrap()).rows[0][0].clone().unwrap_or_default()
}

/// Manual mode: changes wait for commit, roll back on request, a failed
/// statement doesn't spoil the rest; another session sees only commits.
fn by_hand(sp: &Spec, pw: &str, table: &str, create: &str, drop: &str) {
    let mut s = open(sp, pw).unwrap();
    let mut other = open(sp, pw).unwrap();
    s.run_limit(drop, 10, &mut |_| {}).ok();
    s.run_limit(create, 10, &mut |_| {}).unwrap();
    s.set_auto(false);
    assert!(!s.pending());
    step(&mut *s, &format!("insert into {table} values (1)")).unwrap();
    assert!(s.pending());
    assert_eq!(count(&mut *other, &format!("select count(*) from {table}")), "0");
    s.finish(false).unwrap();
    assert!(!s.pending());
    assert_eq!(count(&mut *s, &format!("select count(*) from {table}")), "0");
    step(&mut *s, &format!("insert into {table} values (2)")).unwrap();
    assert!(step(&mut *s, &format!("insert into {table} values ('not a number')")).is_err());
    step(&mut *s, &format!("insert into {table} values (3)")).unwrap();
    assert!(s.pending());
    s.finish(true).unwrap();
    assert!(!s.pending());
    assert_eq!(count(&mut *other, &format!("select count(*) from {table}")), "2");
    // Back to auto: each statement is seen at once.
    s.set_auto(true);
    s.run_limit(&format!("insert into {table} values (4)"), 10, &mut |_| {}).unwrap();
    assert_eq!(count(&mut *other, &format!("select count(*) from {table}")), "3");
    s.run_limit(drop, 10, &mut |_| {}).ok();
}

#[test]
fn postgres_transactions() {
    let Some((sp, pw)) = spec("ZAPROS_TEST_PG", Kind::Postgres) else { return };
    by_hand(&sp, &pw, "zapros_tx", "create table zapros_tx (n int)", "drop table if exists zapros_tx");
}

#[test]
fn oracle_transactions() {
    let Some((sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) else { return };
    by_hand(&sp, &pw, "zapros_tx", "create table zapros_tx (n number)", "drop table zapros_tx");
}

#[test]
fn sqlite_transactions() {
    let d = std::env::temp_dir().join(format!("zapros-tx-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&d);
    let sp = Spec { kind: Kind::Sqlite, database: d.to_string_lossy().into_owned(), ..Default::default() };
    // (SQLite takes 'not a number' into an int column: its own check.)
    by_hand(&sp, "", "zapros_tx", "create table zapros_tx (n int check (typeof(n) = 'integer'))", "drop table if exists zapros_tx");
    let _ = std::fs::remove_file(&d);
}

/// Every connection saved in the user's config opens with its saved
/// password and shows its tree: cargo test saved_connections -- --ignored
#[test]
#[ignore]
fn saved_connections() {
    let list: crate::config::Connections = crate::config::load(&crate::config::config_dir().join("connections.json"));
    assert!(!list.list.is_empty());
    for sp in &list.list {
        let pw = crate::secrets::get(&sp.id).unwrap_or_default();
        let mut s = open(sp, &pw).unwrap_or_else(|e| panic!("{}: {}", sp.title(), e.message));
        let root: Vec<String> = s.children(&[]).unwrap().into_iter().map(|o| o.name).collect();
        let rels = s.relations(&[]).unwrap().len();
        eprintln!("{} — {} — {root:?}, {rels} tables in hints", sp.title(), s.version());
    }
}

/// Waits for an event the test wants (others pass by).
fn wait_for<T>(rx: &Receiver<Event>, mut want: impl FnMut(What) -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        let ev = rx.recv_timeout(until.saturating_duration_since(Instant::now())).expect("an event in time");
        if let What::ConnectFailed(m) = &ev.what {
            panic!("{m}");
        }
        if let Some(t) = want(ev.what) {
            return t;
        }
    }
}

/// A result comes a batch at a time: the first, then more on demand, then
/// all the rest; anything else asked closes the cursor first.
fn batches(sp: Spec, pw: &str, sql: &str) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let w = Worker::spawn(sp, pw.into(), tx, egui::Context::default());
    let stmt = |s: &str| vec![Stmt { sql: s.into(), at: 0 }];
    let page = |rx: &Receiver<Event>| wait_for(rx, |w| if let What::Page { table, .. } = w { Some(table) } else { None });
    let rows = |rx: &Receiver<Event>| wait_for(rx, |w| if let What::Rows { rows, more, open, error, .. } = w { Some((rows.len(), more, open, error)) } else { None });
    w.send(Req::Run { run: 1, stmts: stmt(sql), limit: 500, go_on: false });
    let first = page(&rx);
    assert_eq!(first.rows.len(), 500);
    assert!(first.more);
    wait_for(&rx, |w| matches!(w, What::Finished { run: 1, .. }).then_some(()));
    w.send(Req::More { run: 1, n: 500, all: false });
    assert_eq!(rows(&rx), (500, true, true, None));
    w.send(Req::More { run: 1, n: 500, all: true });
    let mut got = 0;
    loop {
        let (n, more, open, err) = rows(&rx);
        assert!(err.is_none(), "{err:?}");
        got += n;
        if !open {
            assert!(!more);
            break;
        }
    }
    assert_eq!(got, 200);
    // Stopped: the cursor closes, the rest is left.
    w.send(Req::Run { run: 2, stmts: stmt(sql), limit: 500, go_on: false });
    page(&rx);
    w.send(Req::StopFetch);
    assert_eq!(rows(&rx), (0, true, false, None));
    // Another run while a cursor's open: it closes, the run goes on.
    w.send(Req::Run { run: 3, stmts: stmt(sql), limit: 500, go_on: false });
    page(&rx);
    w.send(Req::Run { run: 4, stmts: stmt("select 7"), limit: 500, go_on: false });
    assert_eq!(rows(&rx), (0, true, false, None));
    let t = wait_for(&rx, |w| if let What::Result { run: 4, outcome, .. } = w { Some(outcome) } else { None });
    assert_eq!(super::tests::rows(t.unwrap()).rows[0][0].as_deref(), Some("7"));
}

#[test]
fn sqlite_batches() {
    let dir = std::env::temp_dir().join(format!("zapros-test-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sp = Spec { kind: Kind::Sqlite, database: dir.join("t.db").to_string_lossy().into_owned(), ..Default::default() };
    batches(sp, "", "with recursive c(x) as (select 1 union all select x + 1 from c where x < 1200) select x from c");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn postgres_batches() {
    let Some((sp, pw)) = spec("ZAPROS_TEST_PG", Kind::Postgres) else { return };
    batches(sp, &pw, "select x from generate_series(1, 1200) x");
}

#[test]
fn clickhouse_batches() {
    let Some((sp, pw)) = spec("ZAPROS_TEST_CH", Kind::ClickHouse) else { return };
    batches(sp, &pw, "select number from numbers(1200)");
}

#[cfg(feature = "oracle")]
#[test]
fn oracle_batches() {
    let Some((sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) else { return };
    batches(sp, &pw, "select level from dual connect by level <= 1200");
}
