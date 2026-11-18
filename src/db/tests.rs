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

/// The designer's view of a table, and a change made and undone through it.
fn designer_round_trip(s: &mut dyn Session, kind: Kind, path: &[&str], first_col: &str) {
    use crate::design::{ObjDef, edits_of, table_changes};
    let path: Vec<String> = path.iter().map(|x| x.to_string()).collect();
    let ObjDef::Table(t) = s.describe(&path, ObjKind::Table).unwrap() else { panic!() };
    assert_eq!(t.cols[0].name, first_col, "{t:?}");
    let mut rows = edits_of(&t);
    let k = rows.len() - 1;
    rows[k].col.comment = "через конструктор".into();
    rows.push(crate::design::ColEdit { from: None, col: crate::design::Column { name: "zz_added".into(), ty: if kind == Kind::ClickHouse { "Nullable(String)".into() } else if kind == Kind::Oracle { "VARCHAR2(10)".into() } else { "text".into() }, nullable: true, ..Default::default() }, dropped: false });
    for sql in table_changes(kind, &t, &rows, &t.comment, &[]) {
        s.run(&sql, &mut Fetch::limit(0), &mut |_| {}).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    let ObjDef::Table(t2) = s.describe(&path, ObjKind::Table).unwrap() else { panic!() };
    assert_eq!(t2.cols.len(), t.cols.len() + 1);
    assert_eq!(t2.cols[k].comment, "через конструктор");
    // Back as it was.
    let mut rows = edits_of(&t2);
    rows[k].col.comment = t.cols[k].comment.clone();
    rows.last_mut().unwrap().dropped = true;
    for sql in table_changes(kind, &t2, &rows, &t2.comment, &[]) {
        s.run(&sql, &mut Fetch::limit(0), &mut |_| {}).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    let ObjDef::Table(t3) = s.describe(&path, ObjKind::Table).unwrap() else { panic!() };
    assert_eq!(t3, t);
}

#[test]
fn designer_on_real_servers() {
    if let Some((sp, pw)) = spec("ZAPROS_TEST_PG", Kind::Postgres) {
        let mut s = open(&sp, &pw).unwrap();
        designer_round_trip(s.as_mut(), Kind::Postgres, &["public", "tables", "orders"], "id");
    }
    if let Some((sp, pw)) = spec("ZAPROS_TEST_CH", Kind::ClickHouse) {
        let mut s = open(&sp, &pw).unwrap();
        let c = rows(s.run_limit("select name from system.columns where database = 'default' and table = 'events' order by position limit 1", 1, &mut |_| {}).unwrap()).rows[0][0].clone().unwrap();
        designer_round_trip(s.as_mut(), Kind::ClickHouse, &["default", "tables", "events"], &c);
    }
    if let Some((sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) {
        let mut s = open(&sp, &pw).unwrap();
        let owner = sp.user.to_uppercase();
        let t = rows(s.run_limit("select table_name from user_tables order by table_name", 1, &mut |_| {}).unwrap()).rows[0][0].clone().unwrap();
        let c = rows(s.run_limit(&format!("select column_name from user_tab_columns where table_name = '{t}' order by column_id"), 1, &mut |_| {}).unwrap()).rows[0][0].clone().unwrap();
        designer_round_trip(s.as_mut(), Kind::Oracle, &[&owner, "tables", &t], &c);
    }
}

#[test]
fn searches_on_real_servers() {
    if let Some((sp, pw)) = spec("ZAPROS_TEST_PG", Kind::Postgres) {
        let mut s = open(&sp, &pw).unwrap();
        let f = s.find_objects("ORDER").unwrap();
        assert!(f.iter().any(|x| x.name == "orders" && x.kind == ObjKind::Table && x.path == ["public", "tables", "orders"]), "{f:?}");
        let c = s.search_code("orders").unwrap();
        assert!(c.iter().any(|x| x.kind == ObjKind::Function && !x.lines.is_empty()), "{c:?}");
        println!("pg code: {:?}", c.iter().map(|x| (&x.name, x.kind)).collect::<Vec<_>>());
    }
    if let Some((sp, pw)) = spec("ZAPROS_TEST_CH", Kind::ClickHouse) {
        let mut s = open(&sp, &pw).unwrap();
        let f = s.find_objects("even").unwrap();
        assert!(f.iter().any(|x| x.name == "events"), "{f:?}");
        let c = s.search_code("events").unwrap();
        println!("ch code: {:?}", c.iter().map(|x| (&x.name, x.kind, &x.lines)).collect::<Vec<_>>());
    }
    if let Some((sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) {
        let mut s = open(&sp, &pw).unwrap();
        // A package of the test user's own (it sees no other schema's).
        s.run("CREATE OR REPLACE PACKAGE zapros_search_pkg AS\n  -- counts the orders\n  FUNCTION n RETURN NUMBER;\nEND zapros_search_pkg;", &mut Fetch::limit(0), &mut |_| {}).unwrap();
        let f = s.find_objects("search_pkg").unwrap();
        assert!(f.iter().any(|x| x.kind == ObjKind::Package && x.path[1] == "packages" && x.name == "ZAPROS_SEARCH_PKG"), "{f:?}");
        let c = s.search_code("counts the ORDERS").unwrap();
        assert!(c.iter().any(|x| x.kind == ObjKind::Package && x.lines.first().is_some_and(|l| l.0 == 2 && l.1.contains("counts the orders"))), "{c:?}");
        s.run("DROP PACKAGE zapros_search_pkg", &mut Fetch::limit(0), &mut |_| {}).unwrap();
        // A unit's code: no "/" at its end.
        s.run("CREATE OR REPLACE FUNCTION zapros_slash_fn RETURN NUMBER IS BEGIN RETURN 1; END;", &mut Fetch::limit(0), &mut |_| {}).unwrap();
        let owner = sp.user.to_uppercase();
        let src = s.source(&[owner, "functions".into(), "ZAPROS_SLASH_FN".into()], ObjKind::Function).unwrap();
        assert!(!src.trim_end().ends_with('/') && src.contains("RETURN 1"), "{src}");
        s.run("DROP FUNCTION zapros_slash_fn", &mut Fetch::limit(0), &mut |_| {}).unwrap();
    }
}


/// Real servers' errors in plain words, each with the place it's about.
#[test]
fn plain_errors_on_real_servers() {
    use crate::db::errors::{explain, place};
    use crate::sql::Dialect;
    crate::i18n::set_ru(true);
    // (statement, plain text, underlined, guess)
    let cases: &[(&str, Kind, Dialect, &[&str], &[(&str, &str, &str, Option<&str>)])] = &[
        ("ZAPROS_TEST_PG", Kind::Postgres, Dialect::Postgres, &["create temp table zz(id int primary key, total int not null, name text)", "insert into zz values (1,1,'a')"], &[
            ("select totl from zz", "Нет такого поля «totl»", "totl", Some("MEANT:total")),
            ("select z.totl from zz z", "Нет такого поля «z.totl»", "z.totl", Some("MEANT:total")),
            ("select * from zzz", "Нет такой таблицы «zzz»", "zzz", None),
            ("select lenght(name) from zz", "Нет такой функции «lenght» (или для таких типов аргументов)", "lenght", None),
            ("select from where", "Синтаксическая ошибка у «where»", "where", None),
            ("select * from zz where", "Запрос оборвался: после этого места чего-то не хватает", "where", None),
            ("select id from zz a join zz b on true", "Неоднозначное поле «id»: оно есть в нескольких таблицах", "id", None),
            ("insert into zz values (1,1,'a')", "Такое значение уже есть (должно быть уникальным): id = 1", "", None),
            ("select 'abc'::int", "Значение «abc» не подходит по типу (integer)", "'abc'", None),
            ("select total, count(*) from zz", "Поле «total» должно быть в GROUP BY или внутри агрегатной функции", "total", None),
        ]),
        ("ZAPROS_TEST_CH", Kind::ClickHouse, Dialect::ClickHouse, &["create table if not exists zz(id UInt32, total UInt32, name String) engine=Memory"], &[
            ("select totl from zz", "Нет такого поля «totl»", "totl", Some("MEANT:total")),
            ("select z.totl from zz z", "Нет такого поля «z.totl»", "z.totl", Some("MEANT:z.total")),
            ("select * from zzz", "Нет такой таблицы «zzz»", "zzz", Some("MEANT:default.zz")),
            ("select lenght(name) from zz", "Нет такой функции «lenght»", "lenght", Some("MEANT:length")),
            ("select from where", "Запрос оборвался: после этого места чего-то не хватает", "where", None),
            ("select total, count() from zz", "Поле «total» не в GROUP BY и не внутри агрегатной функции", "total", None),
            ("select * from nodb.t", "Нет такой базы данных «nodb»", "nodb", None),
            ("select plus(1)", "Не то число аргументов у функции «plus»", "plus", Some("передано 1, нужно 2")),
        ]),
        ("ZAPROS_TEST_ORA", Kind::Oracle, Dialect::Oracle, &[], &[
            ("select totl from dual", "Нет такого поля «TOTL»", "totl", None),
            ("select z.totl from dual z", "Нет такого поля «TOTL»", "totl", None),
            ("select * from zzz", "Нет таблицы или представления «ZZZ»", "zzz", None),
            ("select from where", "Не хватает выражения (поля, значения…) в этом месте", "from", None),
            ("select to_number('abc') from dual", "Строку «abc» не перевести в число", "'abc'", None),
            ("select 1/0 from dual", "Деление на ноль", "/", None),
            ("begin nosuch(1); end;", "Не объявлено: «NOSUCH»", "nosuch", None),
            ("select (1 from dual", "Не хватает закрывающей скобки", "from", None),
        ]),
    ];
    for (var, kind, d, setup, list) in cases {
        let Some((sp, pw)) = spec(var, *kind) else { continue };
        let mut s = open(&sp, &pw).unwrap();
        for q in *setup {
            s.run_limit(q, 10, &mut |_| {}).unwrap();
        }
        eprintln!("plain errors: {kind:?}, {} cases", list.len());
        for (q, text, under, hint) in *list {
            let f = s.run_limit(q, 10, &mut |_| {}).unwrap_err();
            let p = explain(&f.message, *d);
            assert_eq!(p.text, *text, "{kind:?} {q}: {}", f.message);
            if let Some(h) = hint {
                match h.strip_prefix("MEANT:") {
                    Some(m) => assert_eq!(p.meant.join(", "), m, "{kind:?} {q}: {}", f.message),
                    None => assert_eq!(p.hint.as_deref(), Some(*h), "{kind:?} {q}: {}", f.message),
                }
            }
            assert!(p.code.is_some(), "{kind:?} {q}");
            let got = place(q, f.at, &p, *d).map(|r| &q[r]);
            if under.is_empty() {
                continue;
            }
            assert_eq!(got, Some(*under), "{kind:?} {q}: at {:?}", f.at);
        }
        if *kind == Kind::ClickHouse {
            let _ = s.run_limit("drop table zz", 10, &mut |_| {});
        }
    }
}

/// Real servers' unknown names read in their context: a wrong alias, a
/// column not of the alias' table, a typo, a call, a table.
#[test]
fn names_in_context_on_real_servers() {
    use crate::db::errors::understand;
    use crate::sql::Dialect;
    crate::i18n::set_ru(true);
    let two = "select x.total, c.name from zz_orders o join zz_customers c on c.id = o.customer_id";
    let cases: &[(&str, Kind, Dialect, &str, &[&str], &[&str])] = &[
        ("ZAPROS_TEST_PG", Kind::Postgres, Dialect::Postgres, "zzerr", &["drop schema if exists zzerr cascade", "create schema zzerr", "set search_path = zzerr, public", "create table zz_orders(id int, customer_id int, total int, note text)", "create table zz_customers(id int, name text)"], &["drop schema zzerr cascade"]),
        ("ZAPROS_TEST_CH", Kind::ClickHouse, Dialect::ClickHouse, "default", &["drop table if exists zz_orders", "drop table if exists zz_customers", "create table zz_orders(id UInt32, customer_id UInt32, total UInt32, note String) engine=Memory", "create table zz_customers(id UInt32, name String) engine=Memory"], &["drop table zz_orders", "drop table zz_customers"]),
        ("ZAPROS_TEST_ORA", Kind::Oracle, Dialect::Oracle, "ZAPROS", &["begin execute immediate 'drop table zz_orders'; exception when others then null; end;", "begin execute immediate 'drop table zz_customers'; exception when others then null; end;", "create table zz_orders(id number, customer_id number, total number, note varchar2(100))", "create table zz_customers(id number, name varchar2(100))"], &["drop table zz_orders", "drop table zz_customers"]),
    ];
    for (var, kind, d, schema, setup, down) in cases {
        let Some((sp, pw)) = spec(var, *kind) else { continue };
        let mut s = open(&sp, &pw).unwrap();
        for q in *setup {
            s.run_limit(q, 10, &mut |_| {}).unwrap_or_else(|e| panic!("{q}: {}", e.message));
            if *kind == Kind::Oracle {
                let _ = s.run_limit("commit", 10, &mut |_| {});
            }
        }
        let rels = s.relations(&[schema.to_string()]).unwrap();
        assert!(rels.iter().any(|r| r.name.eq_ignore_ascii_case("zz_orders")), "{kind:?}: {:?}", rels.iter().map(|r| &r.name).collect::<Vec<_>>());
        let list: &[(&str, &str, &str)] = &[
            (two, "В запросе нет таблицы или алиаса «x» — может быть, «o.total»?", "x"),
            ("select x.* from zz_orders o", "В запросе нет таблицы или алиаса «x»", "x"),
            ("select o.totl from zz_orders o", "У таблицы «zz_orders» (o) нет поля «totl» — может быть, «o.total»?", "totl"),
            ("select o.name from zz_orders o join zz_customers c on c.id = o.customer_id", "У таблицы «zz_orders» (o) нет поля «name»", "name"),
            ("select totl from zz_orders", "Нет такого поля «totl» — может быть, «total»?", "totl"),
            ("select customer from zz_orders", "Нет такого поля «customer» — может быть, «customer_id»?", "customer"),
            ("select lenght(note) from zz_orders", "Нет такой функции «lenght» — может быть, «length»?", "lenght"),
            ("select * from zz_ordrs", "Нет такой таблицы «zz_ordrs» — может быть, «zz_orders»?", "zz_ordrs"),
        ];
        for (q, sentence, under) in list {
            let f = s.run_limit(q, 10, &mut |_| {}).unwrap_err();
            let (p, r) = understand(&f.message, f.at, q, &rels, *d);
            eprintln!("{kind:?} {q}\n  {}\n  {}\n  {:?} → {:?}", f.message.lines().next().unwrap_or(""), p.sentence(), p.hint, r.clone().map(|r| &q[r]));
            assert_eq!(p.sentence(), *sentence, "{kind:?} {q}: {}", f.message);
            assert_eq!(r.map(|r| &q[r]), Some(*under), "{kind:?} {q}: {}", f.message);
        }
        for q in *down {
            let _ = s.run_limit(q, 10, &mut |_| {});
        }
    }
}


/// Ctrl+click's lookup: a table by its bare name, with its schema, a
/// package's member, and nothing for a name that isn't there.
#[test]
fn locate_on_real_servers() {
    if let Some((sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) {
        let mut s = open(&sp, &pw).unwrap();
        let _ = s.run_limit("drop table zz_loc", 0, &mut |_| {});
        s.run_limit("create table zz_loc (id number)", 0, &mut |_| {}).unwrap();
        let user = sp.user.to_uppercase();
        let (path, kind) = s.locate(None, "ZZ_LOC", false).unwrap().expect("bare name");
        assert_eq!((path, kind), (vec![user.clone(), "tables".into(), "ZZ_LOC".into()], ObjKind::Table));
        assert!(s.locate(Some(&user), "ZZ_LOC", false).unwrap().is_some(), "schema.table");
        assert!(s.locate(None, "ZZ_NOT_THERE", false).unwrap().is_none());
        s.run_limit("drop table zz_loc", 0, &mut |_| {}).unwrap();
    }
    if let Some((sp, pw)) = spec("ZAPROS_TEST_PG", Kind::Postgres) {
        let mut s = open(&sp, &pw).unwrap();
        assert!(s.locate(None, "customers", false).unwrap().is_some());
    }
}

/// A session's id as the server knows it, and ending it from another.
#[test]
fn server_sessions_ended_from_outside() {
    for (var, kind) in [("ZAPROS_TEST_PG", Kind::Postgres), ("ZAPROS_TEST_ORA", Kind::Oracle)] {
        let Some((sp, pw)) = spec(var, kind) else { continue };
        let mut a = open(&sp, &pw).unwrap();
        let mut b = open(&sp, &pw).unwrap();
        let id = a.server_id().expect("server id");
        match b.kill(&id) {
            Ok(()) => assert!(!a.alive() || a.run_limit("select 1 from dual", 1, &mut |_| {}).is_err() || kind == Kind::Postgres && a.run_limit("select 1", 1, &mut |_| {}).is_err(), "{var}: still alive"),
            // (Oracle: ALTER SYSTEM takes a privilege the user may lack.)
            Err(e) => assert!(kind == Kind::Oracle && e.message.contains("ORA-01031"), "{var}: {e:?}"),
        }
    }
}

/// Another schema's objects, seen through their own grants only (no
/// SELECT_CATALOG_ROLE): their code still shows (ORA-31603 otherwise).
/// ZAPROS_TEST_ORA_OTHER=user:password of a second schema.
#[test]
fn oracle_other_schema_code() {
    let (Some((sp, pw)), Ok(other)) = (spec("ZAPROS_TEST_ORA", Kind::Oracle), std::env::var("ZAPROS_TEST_ORA_OTHER")) else { return };
    let (ou, op) = other.split_once(':').unwrap();
    let mut o = open(&Spec { user: ou.into(), ..sp.clone() }, op).unwrap();
    let me = sp.user.to_uppercase();
    for sql in [
        "create or replace package zz_pkg as function two return number; end zz_pkg;",
        "create or replace package body zz_pkg as function two return number is begin return 2; end; end zz_pkg;",
        "create or replace view zz_v as select 1 as one from dual",
        "begin execute immediate 'drop table zz_t'; exception when others then null; end;",
        "create table zz_t (id number(10) primary key, name varchar2(40) default 'x' not null)",
        "begin execute immediate 'drop sequence zz_s'; exception when others then null; end;",
        "create sequence zz_s start with 5",
    ] {
        o.run_limit(sql, 0, &mut |_| {}).unwrap();
    }
    for g in ["grant execute on zz_pkg to ", "grant select on zz_v to ", "grant select, update on zz_t to ", "grant select on zz_s to "] {
        o.run_limit(&format!("{g}{me}"), 0, &mut |_| {}).unwrap();
    }
    let owner = ou.to_uppercase();
    let mut s = open(&sp, &pw).unwrap();
    let src = |s: &mut Box<dyn Session>, folder: &str, name: &str, kind| s.source(&[owner.clone(), folder.into(), name.into()], kind);
    let pkg = src(&mut s, "packages", "ZZ_PKG", ObjKind::Package).unwrap();
    assert!(pkg.contains(&format!("CREATE OR REPLACE package \"{owner}\".zz_pkg as")) || pkg.contains(&format!("\"{owner}\".\"ZZ_PKG\"")), "{pkg}");
    assert!(pkg.contains("function two return number"), "{pkg}");
    let v = src(&mut s, "views", "ZZ_V", ObjKind::View).unwrap();
    assert!(v.contains("select 1 as one from dual"), "{v}");
    let t = src(&mut s, "tables", "ZZ_T", ObjKind::Table).unwrap();
    assert!(t.contains("NUMBER(10)") && t.contains("NOT NULL") && t.contains("PRIMARY KEY"), "{t}");
    let q = src(&mut s, "sequences", "ZZ_S", ObjKind::Sequence).unwrap();
    assert!(q.contains("SEQUENCE") && q.contains("ZZ_S"), "{q}");
    eprintln!("{pkg}\n{v}\n{t}\n{q}");
}

/// The table designer's indexes: read, then renamed, remade, dropped and
/// added by its statements, and read again — on every database at hand.
#[test]
fn index_designer_on_real_servers() {
    use crate::design::{IndexDef, IndexEdit, edits_of, index_edits_of, table_changes};
    let dir = std::env::temp_dir().join(format!("zapros-ix-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lite = Spec { kind: Kind::Sqlite, database: dir.join("ix.db").to_string_lossy().into_owned(), ..Default::default() };
    let mut cases: Vec<(Spec, String)> = vec![(lite, String::new())];
    for (v, k) in [("ZAPROS_TEST_PG", Kind::Postgres), ("ZAPROS_TEST_ORA", Kind::Oracle), ("ZAPROS_TEST_CH", Kind::ClickHouse)] {
        if let Some(c) = spec(v, k) {
            cases.push(c);
        }
    }
    for (sp, pw) in cases {
        let kind = sp.kind;
        let mut s = open(&sp, &pw).unwrap();
        let run = |s: &mut Box<dyn Session>, sql: &str| s.run_limit(sql, 0, &mut |_| {}).map(|_| ()).map_err(|e| format!("{kind:?} {sql}: {}", e.message));
        let (schema, table, path): (String, String, Vec<String>) = match kind {
            Kind::Postgres => ("public".into(), "zz_ix".into(), vec!["public".into(), "tables".into(), "zz_ix".into()]),
            Kind::Oracle => (sp.user.to_uppercase(), "ZZ_IX".into(), vec![sp.user.to_uppercase(), "tables".into(), "ZZ_IX".into()]),
            Kind::ClickHouse => ("default".into(), "zz_ix".into(), vec!["default".into(), "tables".into(), "zz_ix".into()]),
            Kind::Sqlite => (String::new(), "zz_ix".into(), vec!["tables".into(), "zz_ix".into()]),
        };
        let _ = run(&mut s, "drop table zz_ix");
        match kind {
            Kind::ClickHouse => {
                run(&mut s, "create table zz_ix (id UInt64, a Int32, b String, INDEX ix_a a TYPE minmax GRANULARITY 2) engine = MergeTree order by id").unwrap();
            }
            _ => {
                run(&mut s, "create table zz_ix (id integer primary key, a integer, b varchar(20))").unwrap();
                run(&mut s, "create index zz_ix_a on zz_ix (a)").unwrap();
            }
        }
        let def = match s.describe(&path, ObjKind::Table).unwrap() {
            crate::design::ObjDef::Table(t) => t,
            o => panic!("{o:?}"),
        };
        let a_name = if kind == Kind::ClickHouse { "ix_a" } else if kind == Kind::Oracle { "ZZ_IX_A" } else { "zz_ix_a" };
        let a = def.indexes.iter().find(|x| x.name == a_name).unwrap_or_else(|| panic!("{kind:?}: {:?}", def.indexes));
        assert!(a.cols.to_lowercase().contains('a') && !a.constraint, "{kind:?}: {a:?}");
        // (SQLite's INTEGER PRIMARY KEY is the rowid: no index of its own.)
        if !matches!(kind, Kind::ClickHouse | Kind::Sqlite) {
            assert!(def.indexes.iter().any(|x| x.constraint && x.unique), "{kind:?}: the primary key's: {:?}", def.indexes);
        }
        // Renamed; a new one on b (descending, and partial where it can be).
        let mut rows = index_edits_of(&def);
        let i = rows.iter().position(|r| r.idx.name == a_name).unwrap();
        rows[i].idx.name = if kind == Kind::Oracle { "ZZ_IX_A2".into() } else { "zz_ix_a2".into() };
        let b = match kind {
            Kind::ClickHouse => IndexDef { name: "ix_b".into(), cols: "b".into(), method: "bloom_filter(0.01)".into(), granularity: "1".into(), ..Default::default() },
            Kind::Postgres | Kind::Sqlite => IndexDef { name: "zz_ix_b".into(), cols: "b DESC".into(), unique: true, filter: "b IS NOT NULL".into(), ..Default::default() },
            Kind::Oracle => IndexDef { name: "zz_ix_b".into(), cols: "b DESC".into(), method: "NORMAL".into(), ..Default::default() },
        };
        rows.push(IndexEdit { from: None, idx: b, dropped: false });
        let stmts = table_changes(kind, &def, &edits_of(&def), &def.comment, &rows);
        for st in &stmts {
            run(&mut s, st).unwrap();
        }
        let def2 = match s.describe(&path, ObjKind::Table).unwrap() {
            crate::design::ObjDef::Table(t) => t,
            o => panic!("{o:?}"),
        };
        let names: Vec<String> = def2.indexes.iter().map(|x| x.name.to_lowercase()).collect();
        assert!(names.contains(&"zz_ix_a2".to_string()) || names.contains(&"ix_a".to_string()) == false && names.iter().any(|n| n == "zz_ix_a2"), "{kind:?}: {names:?} after {stmts:?}");
        let b_name = if kind == Kind::ClickHouse { "ix_b" } else { "zz_ix_b" };
        let b = def2.indexes.iter().find(|x| x.name.to_lowercase() == b_name).unwrap_or_else(|| panic!("{kind:?}: {names:?} after {stmts:?}"));
        match kind {
            Kind::Postgres | Kind::Sqlite => assert!(b.unique && b.cols.to_lowercase().contains("desc") && b.filter.to_lowercase().contains("not null"), "{kind:?}: {b:?}"),
            Kind::Oracle => assert!(b.cols.to_uppercase().contains("DESC"), "{b:?}"),
            Kind::ClickHouse => assert!(b.method.starts_with("bloom_filter"), "{b:?}"),
            _ => {}
        }
        // Nothing changed: no statements; then the renamed one dropped.
        assert!(table_changes(kind, &def2, &edits_of(&def2), &def2.comment, &index_edits_of(&def2)).is_empty(), "{kind:?}");
        let mut rows = index_edits_of(&def2);
        for r in rows.iter_mut().filter(|r| r.idx.name.to_lowercase() == "zz_ix_a2") {
            r.dropped = true;
        }
        for st in table_changes(kind, &def2, &edits_of(&def2), &def2.comment, &rows) {
            run(&mut s, &st).unwrap();
        }
        let def3 = match s.describe(&path, ObjKind::Table).unwrap() {
            crate::design::ObjDef::Table(t) => t,
            o => panic!("{o:?}"),
        };
        assert!(!def3.indexes.iter().any(|x| x.name.to_lowercase() == "zz_ix_a2"), "{kind:?}: {:?}", def3.indexes);
        run(&mut s, "drop table zz_ix").unwrap();
        let _ = (schema, table);
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// An object's DDL with its needless quotes taken off still runs.
#[test]
fn bare_names_ddl_still_runs() {
    use crate::sql::complete::bare_names;
    if let Some((sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) {
        let mut s = open(&sp, &pw).unwrap();
        for t in ["ZZ_BARE", "ZZ_BARE2"] {
            let _ = s.run_limit(&format!("drop table {t}"), 0, &mut |_| {});
        }
        s.run_limit("create table zz_bare (id number primary key, \"lower\" varchar2(5), \"DATE\" date, \"TWO WORDS\" int, v$x number, \"LEVEL\" int)", 0, &mut |_| {}).unwrap();
        let me = sp.user.to_uppercase();
        let ddl = s.source(&[me.clone(), "tables".into(), "ZZ_BARE".into()], ObjKind::Table).unwrap();
        let bare = bare_names(&ddl, crate::sql::Dialect::Oracle);
        assert!(bare.contains(&format!("{me}.ZZ_BARE")) && bare.contains("\"lower\"") && bare.contains("\"DATE\"") && bare.contains("\"LEVEL\"") && bare.contains("V$X"), "{bare}");
        let again = bare.replace("ZZ_BARE", "ZZ_BARE2");
        let first = crate::sql::split(&again, crate::sql::Dialect::Oracle, false).into_iter().next().unwrap();
        s.run_limit(again[first].trim().trim_end_matches(';'), 0, &mut |_| {}).unwrap_or_else(|e| panic!("{}\n{again}", e.message));
        for t in ["ZZ_BARE", "ZZ_BARE2"] {
            s.run_limit(&format!("drop table {t}"), 0, &mut |_| {}).unwrap();
        }
    }
    if let Some((sp, pw)) = spec("ZAPROS_TEST_CH", Kind::ClickHouse) {
        let mut s = open(&sp, &pw).unwrap();
        for t in ["zz_bare", "zz_bare2"] {
            let _ = s.run_limit(&format!("drop table if exists {t}"), 0, &mut |_| {});
        }
        s.run_limit("create table zz_bare (id UInt64, `my col` String, `default` Int8, total Float64) engine = MergeTree order by id", 0, &mut |_| {}).unwrap();
        let ddl = s.source(&["default".into(), "tables".into(), "zz_bare".into()], ObjKind::Table).unwrap();
        let bare = bare_names(&ddl, crate::sql::Dialect::ClickHouse);
        assert!(bare.contains("`my col`") && bare.contains("`default`") && !bare.contains("`total`"), "{bare}");
        s.run_limit(bare.replace("zz_bare", "zz_bare2").trim().trim_end_matches(';'), 0, &mut |_| {}).unwrap_or_else(|e| panic!("{}\n{bare}", e.message));
        for t in ["zz_bare", "zz_bare2"] {
            s.run_limit(&format!("drop table {t}"), 0, &mut |_| {}).unwrap();
        }
    }
}

/// A table made from a file: its CREATE (key, an index), its rows in
/// batches with every kind of value, then read back — on every database
/// at hand.
#[test]
fn import_on_real_servers() {
    use crate::import::{self, ReadOpts};
    let dir = std::env::temp_dir().join(format!("zapros-imp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut csv = String::from("Код;Имя клиента;Сумма;Оплачен;Дата;Когда\n");
    for i in 1..=1203 {
        let name = if i % 7 == 0 { String::new() } else { format!("Клиент \"{i}\"'s") };
        csv += &format!("{i};{name};{},{};{};{:02}.03.2024;2024-03-{:02} 10:{:02}:05\n", i * 3, i % 10, if i % 2 == 0 { "true" } else { "false" }, i % 28 + 1, i % 28 + 1, i % 60);
    }
    let file = dir.join("клиенты.csv");
    std::fs::write(&file, csv).unwrap();
    let data = import::read(&file, &ReadOpts::default()).unwrap();
    assert_eq!(data.rows.len(), 1203);
    let lite = Spec { kind: Kind::Sqlite, database: dir.join("imp.db").to_string_lossy().into_owned(), ..Default::default() };
    let mut cases: Vec<(Spec, String)> = vec![(lite, String::new())];
    for (v, k) in [("ZAPROS_TEST_PG", Kind::Postgres), ("ZAPROS_TEST_ORA", Kind::Oracle), ("ZAPROS_TEST_CH", Kind::ClickHouse)] {
        if let Some(c) = spec(v, k) {
            cases.push(c);
        }
    }
    for (sp, pw) in cases {
        let kind = sp.kind;
        let mut s = open(&sp, &pw).unwrap();
        let schema = match kind {
            Kind::Postgres => "public".to_string(),
            Kind::Oracle => sp.user.to_uppercase(),
            Kind::ClickHouse => "default".to_string(),
            Kind::Sqlite => String::new(),
        };
        let mut cols = import::columns(kind, &data);
        let types: Vec<String> = cols.iter().map(|c| c.ty.clone()).collect();
        cols[0].pk = true;
        let table = if kind == Kind::Oracle { "ZZ_IMPORT" } else { "zz_import" };
        let name_col = cols[1].name.clone();
        let idx = vec![crate::design::IndexDef { name: if kind == Kind::Oracle { "ZZ_IMPORT_IX1".into() } else { "zz_import_ix1".into() }, cols: crate::sql::complete::quote(&name_col, kind.dialect()), ..Default::default() }];
        let t = crate::design::TableDef { schema: schema.clone(), name: table.into(), ..Default::default() };
        let tn = crate::design::table_name(kind, &t);
        let _ = s.run_limit(&format!("drop table {tn}"), 0, &mut |_| {});
        for sql in import::ddl(kind, &schema, table, &cols, &idx).iter().chain(import::inserts(kind, &schema, table, &cols, &data.rows, import::batch(kind)).iter()) {
            s.run_limit(sql, 0, &mut |_| {}).unwrap_or_else(|e| panic!("{kind:?} {types:?}: {}\n{}", e.message, crate::app::head(sql, 600)));
        }
        if kind.transactions() {
            s.finish(true).ok();
        }
        let q = |s: &mut Box<dyn Session>, sql: &str| rows(s.run_limit(sql, 10, &mut |_| {}).unwrap_or_else(|e| panic!("{kind:?} {sql}: {}", e.message))).rows;
        let d = kind.dialect();
        let c = |k: usize| crate::sql::complete::quote(&cols[k].name, d);
        let n = q(&mut s, &format!("select count(*), count({}), sum({}) from {tn}", c(1), c(2)));
        assert_eq!(n[0][0].as_deref(), Some("1203"), "{kind:?}");
        assert_eq!(n[0][1].as_deref(), Some((1203 - 1203 / 7).to_string().as_str()), "{kind:?}: empty names as NULL");
        let sum: f64 = n[0][2].as_deref().unwrap().parse().unwrap();
        let want: f64 = (1..=1203).map(|i| (i * 3) as f64 + (i % 10) as f64 / 10.0).sum();
        assert!((sum - want).abs() < 0.01, "{kind:?}: {sum} vs {want}");
        let r = q(&mut s, &format!("select {}, {}, {} from {tn} where {} = 14", c(1), c(4), c(5), c(0)));
        assert_eq!(r[0][0], None, "{kind:?}");
        assert!(r[0][1].as_deref().unwrap().starts_with("2024-03-15") || r[0][1].as_deref().unwrap().contains("15"), "{kind:?}: {:?}", r[0]);
        let r = q(&mut s, &format!("select {} from {tn} where {} = 13", c(1), c(0)));
        assert_eq!(r[0][0].as_deref(), Some("Клиент \"13\"'s"), "{kind:?}");
        // The key and the index are there.
        let path: Vec<String> = if schema.is_empty() { vec!["tables".into(), table.into()] } else { vec![schema.clone(), "tables".into(), table.into()] };
        let crate::design::ObjDef::Table(def) = s.describe(&path, ObjKind::Table).unwrap() else { panic!() };
        assert!(def.indexes.iter().any(|x| x.name.eq_ignore_ascii_case(&idx[0].name)), "{kind:?}: {:?}", def.indexes);
        assert!(!def.cols[0].nullable, "{kind:?}: the key's column");
        s.run_limit(&format!("drop table {tn}"), 0, &mut |_| {}).unwrap();
        if kind.transactions() {
            s.finish(true).ok();
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// Functions and procedures with their parameters, for the hints in a
/// call: a function with a default and an OUT, a package's procedure.
#[test]
fn routines_on_real_servers() {
    if let Some((sp, pw)) = spec("ZAPROS_TEST_PG", Kind::Postgres) {
        let mut s = open(&sp, &pw).unwrap();
        s.run_limit("create or replace function zz_add(p_user integer, p_total numeric default 0, out p_id bigint) as $$ select 1::bigint $$ language sql", 0, &mut |_| {}).unwrap();
        s.finish(true).ok();
        let r = s.routines().unwrap();
        let f = r.iter().find(|r| r.name == "zz_add").expect("zz_add");
        assert_eq!(f.args.iter().map(|a| (a.name.as_str(), a.ty.as_str(), a.mode.as_str(), a.default)).collect::<Vec<_>>(), [("p_user", "integer", "", false), ("p_total", "numeric", "", true), ("p_id", "bigint", "OUT", false)]);
        s.run_limit("drop function zz_add", 0, &mut |_| {}).unwrap();
        s.finish(true).ok();
    }
    if let Some((sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) {
        let mut s = open(&sp, &pw).unwrap();
        s.run_limit("create or replace package zz_pkg as procedure add_order(p_user in number, p_note in varchar2 default null, p_id out number); end;", 0, &mut |_| {}).unwrap();
        let r = s.routines().unwrap();
        let p = r.iter().find(|r| r.package == "ZZ_PKG" && r.name == "ADD_ORDER").expect("ZZ_PKG.ADD_ORDER");
        assert_eq!(p.args.iter().map(|a| (a.name.as_str(), a.mode.as_str(), a.default)).collect::<Vec<_>>(), [("P_USER", "IN", false), ("P_NOTE", "IN", true), ("P_ID", "OUT", false)]);
        let hits = crate::sql::complete::routines_named(&r, Some("zz_pkg"), "add_order");
        assert_eq!(hits.len(), 1);
        s.run_limit("drop package zz_pkg", 0, &mut |_| {}).unwrap();
    }
}

/// Cyrillic through Oracle whatever NLS_LANG says (run this one with
/// NLS_LANG set to a single-byte charset too): code compiled with it in
/// comments, strings, a quoted name and an error message; read back from
/// ALL_SOURCE, from the code's own output, from the error raised and from
/// the DDL the tree shows.
#[test]
fn oracle_keeps_cyrillic() {
    let Some((sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) else { return };
    let mut s = open(&sp, &pw).unwrap();
    let cs = rows(s.run_limit("select value from nls_database_parameters where parameter = 'NLS_CHARACTERSET'", 1, &mut |_| {}).unwrap()).rows;
    eprintln!("database charset: {:?}; NLS_LANG={:?}", cs[0][0], std::env::var("NLS_LANG").ok());
    let text = "Привет, мир — ёлка «Ё» №1";
    let spec_sql = "create or replace package zz_кирилл as\n  -- Комментарий: проверка «кириллицы» и ё\n  function привет return varchar2;\n  procedure ошибка;\nend;";
    let body_sql = format!("create or replace package body zz_кирилл as\n  function привет return varchar2 is\n  begin\n    return '{text}'; -- строка с ё\n  end;\n  procedure ошибка is\n  begin\n    raise_application_error(-20001, 'Нет такого заказа: №5');\n  end;\nend;");
    for sql in [spec_sql.to_string(), body_sql] {
        s.run_limit(&sql, 0, &mut |_| {}).unwrap_or_else(|e| panic!("{}", e.message));
    }
    // Compiled clean.
    let errs = rows(s.run_limit("select count(*) from user_errors where name = 'ZZ_КИРИЛЛ'", 1, &mut |_| {}).unwrap()).rows;
    assert_eq!(errs[0][0].as_deref(), Some("0"), "compiled without errors");
    // Its output.
    let got = rows(s.run_limit("select zz_кирилл.привет from dual", 1, &mut |_| {}).unwrap()).rows;
    assert_eq!(got[0][0].as_deref(), Some(text));
    // Its code as kept.
    let src = rows(s.run_limit("select text from user_source where name = 'ZZ_КИРИЛЛ' and type = 'PACKAGE' and line = 2", 1, &mut |_| {}).unwrap()).rows;
    assert_eq!(src[0][0].as_deref().map(str::trim_end), Some("  -- Комментарий: проверка «кириллицы» и ё"));
    // Its error's message.
    let e = s.run_limit("begin zz_кирилл.ошибка; end;", 0, &mut |_| {}).err().expect("an error");
    assert!(e.message.contains("Нет такого заказа: №5"), "{}", e.message);
    // The DDL the tree opens.
    let ddl = s.source(&[sp.user.to_uppercase(), "packages".into(), "ZZ_КИРИЛЛ".into()], ObjKind::Package).unwrap();
    assert!(ddl.contains("Комментарий: проверка «кириллицы» и ё") && ddl.contains(text), "{ddl}");
    // A table and data with it.
    let _ = s.run_limit("drop table \"Заказы_тест\"", 0, &mut |_| {});
    s.run_limit("create table \"Заказы_тест\" (\"Имя\" varchar2(50 char), примечание nvarchar2(50))", 0, &mut |_| {}).unwrap();
    s.run_limit("insert into \"Заказы_тест\" values ('Ёжик', N'Щука')", 0, &mut |_| {}).unwrap();
    let r = rows(s.run_limit("select \"Имя\", примечание from \"Заказы_тест\"", 1, &mut |_| {}).unwrap()).rows;
    assert_eq!((r[0][0].as_deref(), r[0][1].as_deref()), (Some("Ёжик"), Some("Щука")));
    s.run_limit("drop table \"Заказы_тест\"", 0, &mut |_| {}).unwrap();
    s.run_limit("drop package zz_кирилл", 0, &mut |_| {}).unwrap();
}

/// A connection's own NLS_LANG: its language and territory set for the
/// session, the text still whole; a wrong one said plainly.
#[test]
fn oracle_nls_lang_of_a_connection() {
    assert_eq!(super::Nls::parse("AMERICAN_AMERICA.AL32UTF8"), Some(super::Nls { language: "AMERICAN".into(), territory: "AMERICA".into(), charset: "AL32UTF8".into() }));
    assert_eq!(super::Nls::parse("english_united kingdom.we8mswin1252").map(|n| (n.territory.clone(), n.utf8())), Some(("UNITED KINGDOM".into(), false)));
    assert_eq!(super::Nls::parse(".AL32UTF8").map(|n| n.session().len()), Some(0));
    assert!(super::Nls::parse("").is_none() && super::Nls::parse("RU'; drop").is_none());
    let Some((mut sp, pw)) = spec("ZAPROS_TEST_ORA", Kind::Oracle) else { return };
    let param = |s: &mut Box<dyn Session>, p: &str| rows(s.run_limit(&format!("select value from nls_session_parameters where parameter = '{p}'"), 1, &mut |_| {}).unwrap()).rows[0][0].clone().unwrap();
    for (nls, lang, terr, dec) in [("AMERICAN_AMERICA.AL32UTF8", "AMERICAN", "AMERICA", ".,"), ("RUSSIAN_RUSSIA.CL8MSWIN1251", "RUSSIAN", "RUSSIA", ", "), ("GERMAN_GERMANY", "GERMAN", "GERMANY", ",.")] {
        sp.nls_lang = nls.into();
        let mut s = open(&sp, &pw).unwrap_or_else(|e| panic!("{nls}: {}", e.message));
        assert_eq!((param(&mut s, "NLS_LANGUAGE"), param(&mut s, "NLS_TERRITORY")), (lang.to_string(), terr.to_string()), "{nls}");
        assert_eq!(param(&mut s, "NLS_NUMERIC_CHARACTERS").replace('\u{a0}', " "), dec, "{nls}");
        let r = rows(s.run_limit("select 'Ёжик — №1' from dual", 1, &mut |_| {}).unwrap()).rows;
        assert_eq!(r[0][0].as_deref(), Some("Ёжик — №1"), "{nls}");
    }
    sp.nls_lang = "KLINGON_QONOS.AL32UTF8".into();
    let e = open(&sp, &pw).err().expect("a language Oracle doesn't know");
    assert!(e.message.starts_with("NLS_LANG «KLINGON_QONOS.AL32UTF8»") && e.message.contains("ORA-"), "{}", e.message);
}
