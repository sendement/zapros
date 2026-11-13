//! PostgreSQL over tokio-postgres, on a runtime of the worker's own. Scripts
//! go by the simple protocol (any statement, values as the server prints
//! them); rows past the limit cancel the query on the server.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio_postgres::{AsyncMessage, Client, Config, SimpleQueryMessage, config::SslMode};

use super::*;

pub struct Pg {
    rt: tokio::runtime::Runtime,
    client: Client,
    notes: std::sync::mpsc::Receiver<String>,
    tls: Option<postgres_native_tls::MakeTlsConnector>,
    /// By hand: a transaction is open (BEGIN sent, no COMMIT since).
    in_tx: bool,
    /// A savepoint before the statement running, so its failure undoes it
    /// alone and the transaction goes on.
    sp: bool,
    /// The statement was cut short by our own cancel (enough rows): its
    /// transaction is aborted, back to the savepoint.
    cut: bool,
}

/// Statements PostgreSQL won't run inside a transaction.
fn outside_tx(sql: &str) -> bool {
    let w = crate::sql::first_words(sql, 2);
    let w: Vec<&str> = w.iter().map(|s| s.as_str()).collect();
    matches!(w.as_slice(), ["vacuum", ..] | ["create" | "drop", "database" | "tablespace"] | ["alter", "system"])
        || sql.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|x| x == "concurrently")
}

fn fail(e: tokio_postgres::Error, sql: &str) -> Failure {
    let lost = e.is_closed() || e.as_db_error().is_none() && e.to_string().to_lowercase().contains("connection");
    match e.as_db_error() {
        Some(db) => {
            let mut m = format!("{}: {}", db.code().code(), db.message());
            if let Some(d) = db.detail() {
                m += &format!("\n{d}");
            }
            if let Some(h) = db.hint() {
                m += &format!("\nHINT: {h}");
            }
            if let Some(w) = db.where_() {
                m += &format!("\n{w}");
            }
            let at = match db.position() {
                Some(tokio_postgres::error::ErrorPosition::Original(p)) => sql.char_indices().nth((*p as usize).saturating_sub(1)).map(|(b, _)| b),
                _ => None,
            };
            Failure { message: m, at, lost }
        }
        None => Failure { message: e.to_string(), at: None, lost },
    }
}

impl Pg {
    pub fn open(spec: &Spec, password: &str) -> Res<Self> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| Failure::new(e.to_string()))?;
        let mut cfg = Config::new();
        cfg.host(if spec.host.is_empty() { "localhost" } else { &spec.host })
            .port(if spec.port == 0 { 5432 } else { spec.port })
            .user(&spec.user)
            .dbname(if spec.database.is_empty() { &spec.user } else { &spec.database })
            .application_name("zapros")
            .keepalives(true)
            .keepalives_idle(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(spec.connect_timeout.max(1)));
        if !password.is_empty() {
            cfg.password(password);
        }
        let (tx, notes) = std::sync::mpsc::channel();
        let (client, tls) = match spec.tls {
            Tls::Disable => {
                cfg.ssl_mode(SslMode::Disable);
                let (client, conn) = rt.block_on(cfg.connect(tokio_postgres::NoTls)).map_err(|e| fail(e, ""))?;
                rt.spawn(pump(conn, tx));
                (client, None)
            }
            mode => {
                cfg.ssl_mode(if mode == Tls::Require { SslMode::Require } else { SslMode::Prefer });
                // Like libpq's sslmode=require: encrypted, the certificate not checked.
                let tc = native_tls::TlsConnector::builder().danger_accept_invalid_certs(true).danger_accept_invalid_hostnames(true).build().map_err(|e| Failure::new(e.to_string()))?;
                let tls = postgres_native_tls::MakeTlsConnector::new(tc);
                let (client, conn) = rt.block_on(cfg.connect(tls.clone())).map_err(|e| fail(e, ""))?;
                rt.spawn(pump(conn, tx));
                (client, Some(tls))
            }
        };
        let mut s = Self { rt, client, notes, tls, in_tx: false, sp: false, cut: false };
        if spec.read_only {
            s.simple("SET default_transaction_read_only = on").ok();
        }
        Ok(s)
    }

    fn simple(&mut self, sql: &str) -> Res<Vec<SimpleQueryMessage>> {
        self.rt.block_on(self.client.simple_query(sql)).map_err(|e| fail(e, sql))
    }

    /// Rows of text (every column cast to text in the query).
    fn rows(&mut self, sql: &str, params: &[&str]) -> Res<Vec<Vec<String>>> {
        let p: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params.iter().map(|s| s as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
        let rows = self.rt.block_on(self.client.query(sql, &p)).map_err(|e| fail(e, sql))?;
        Ok(rows.iter().map(|r| (0..r.len()).map(|i| r.get::<_, Option<String>>(i).unwrap_or_default()).collect()).collect())
    }

    fn one(&mut self, sql: &str, params: &[&str]) -> Res<String> {
        Ok(self.rows(sql, params)?.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or_default())
    }

    fn table_ddl(&mut self, schema: &str, table: &str) -> Res<String> {
        let rel = format!("{}.{}", ident(schema), ident(table));
        let cols = self.rows(
            "select a.attname::text, format_type(a.atttypid, a.atttypmod), a.attnotnull::text, pg_get_expr(d.adbin, d.adrelid), col_description(a.attrelid, a.attnum), a.attidentity::text, a.attgenerated::text
               from pg_attribute a left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
              where a.attrelid = $1::text::regclass and a.attnum > 0 and not a.attisdropped order by a.attnum",
            &[&rel],
        )?;
        let mut lines: Vec<String> = Vec::new();
        for c in &cols {
            let mut l = format!("{} {}", ident(&c[0]), c[1]);
            match c[5].as_str() {
                "a" => l += " GENERATED ALWAYS AS IDENTITY",
                "d" => l += " GENERATED BY DEFAULT AS IDENTITY",
                _ if c[6] == "s" => l += &format!(" GENERATED ALWAYS AS ({}) STORED", c[3]),
                _ if !c[3].is_empty() => l += &format!(" DEFAULT {}", c[3]),
                _ => {}
            }
            if c[2] == "true" {
                l += " NOT NULL";
            }
            lines.push(l);
        }
        for k in self.rows("select conname::text, pg_get_constraintdef(oid, true) from pg_constraint where conrelid = $1::text::regclass and contype <> 'n' order by contype desc, conname", &[&rel])? {
            lines.push(format!("CONSTRAINT {} {}", ident(&k[0]), k[1]));
        }
        let mut out = format!("CREATE TABLE {rel} (\n    {}\n)", lines.join(",\n    "));
        let part = self.one("select pg_get_partkeydef($1::text::regclass)", &[&rel])?;
        if !part.is_empty() {
            out += &format!(" PARTITION BY {part}");
        }
        out += ";\n";
        for i in self.rows("select pg_get_indexdef(i.indexrelid) from pg_index i where i.indrelid = $1::text::regclass and not exists (select 1 from pg_constraint c where c.conindid = i.indexrelid) order by 1", &[&rel])? {
            out += &format!("\n{};", i[0]);
        }
        let comment = self.one("select obj_description($1::text::regclass, 'pg_class')", &[&rel])?;
        if !comment.is_empty() {
            out += &format!("\n\nCOMMENT ON TABLE {rel} IS {};", literal(&comment));
        }
        for c in cols.iter().filter(|c| !c[4].is_empty()) {
            out += &format!("\nCOMMENT ON COLUMN {rel}.{} IS {};", ident(&c[0]), literal(&c[4]));
        }
        for t in self.rows("select pg_get_triggerdef(oid, true) from pg_trigger where tgrelid = $1::text::regclass and not tgisinternal", &[&rel])? {
            out += &format!("\n\n{};", t[0]);
        }
        Ok(out)
    }
}

/// Drives the connection, passing NOTICEs on.
async fn pump<S, T>(mut conn: tokio_postgres::Connection<S, T>, tx: std::sync::mpsc::Sender<String>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut messages = futures_util::stream::poll_fn(move |cx| conn.poll_message(cx));
    while let Some(m) = messages.next().await {
        match m {
            Ok(AsyncMessage::Notice(n)) => {
                tx.send(format!("{}: {}", n.severity(), n.message())).ok();
            }
            Ok(AsyncMessage::Notification(n)) => {
                tx.send(format!("NOTIFY {}: {}", n.channel(), n.payload())).ok();
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

pub fn ident(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') && !s.starts_with(|c: char| c.is_ascii_digit()) {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}

fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn returns_rows(sql: &str) -> bool {
    let w = crate::sql::first_word(sql).to_lowercase();
    matches!(w.as_str(), "select" | "with" | "values" | "table")
}

impl Session for Pg {
    fn run(&mut self, sql: &str, fetch: &mut Fetch, notes: &mut dyn FnMut(String)) -> Res<Outcome> {
        // The columns' types, when the statement can be described.
        let types: Option<Vec<String>> = if returns_rows(sql) {
            self.rt.block_on(self.client.prepare(sql)).ok().map(|st| st.columns().iter().map(|c| c.type_().name().to_string()).collect())
        } else {
            None
        };
        let client = &self.client;
        let cancel = client.cancel_token();
        let tls = self.tls.clone();
        let cut_short = std::cell::Cell::new(false);
        let result: Result<(Option<Table>, Option<u64>), tokio_postgres::Error> = self.rt.block_on(async {
            let stream = client.simple_query_raw(sql).await?;
            futures_util::pin_mut!(stream);
            let mut table: Option<Table> = None;
            let mut count = None;
            let mut cut = false;
            while let Some(m) = stream.next().await {
                let m = match m {
                    Ok(m) => m,
                    // The cancel we sent once the limit was reached.
                    Err(e) if cut && e.code() == Some(&tokio_postgres::error::SqlState::QUERY_CANCELED) => {
                        cut_short.set(true);
                        break;
                    }
                    Err(e) => return Err(e),
                };
                match m {
                    SimpleQueryMessage::RowDescription(cols) => {
                        table = Some(Table {
                            cols: cols.iter().enumerate().map(|(i, c)| Col { name: c.name().to_string(), ty: types.as_ref().and_then(|t| t.get(i)).cloned().unwrap_or_default(), numeric: false }).collect(),
                            ..Default::default()
                        });
                    }
                    SimpleQueryMessage::Row(r) => {
                        let Some(t) = table.as_mut() else { continue };
                        if cut {
                            continue;
                        }
                        if !fetch.room(t) {
                            // Enough: the server stops it.
                            cut = true;
                            match &tls {
                                Some(tls) => cancel.cancel_query(tls.clone()).await.ok(),
                                None => cancel.cancel_query(tokio_postgres::NoTls).await.ok(),
                            };
                            continue;
                        }
                        t.rows.push((0..r.len()).map(|i| r.get(i).map(str::to_string)).collect());
                    }
                    SimpleQueryMessage::CommandComplete(n) => count = Some(n),
                    _ => {}
                }
            }
            Ok((table, count))
        });
        self.cut = cut_short.get();
        // Cut short, but the query ended before our cancel got there: the
        // cancel may still land on the next statement. A probe takes it.
        let sent = matches!(&result, Ok((Some(t), _)) if t.more);
        if sent && !self.cut {
            std::thread::sleep(std::time::Duration::from_millis(15));
            for _ in 0..3 {
                match self.rt.block_on(self.client.simple_query("SELECT 1")) {
                    Err(e) if e.code() == Some(&tokio_postgres::error::SqlState::QUERY_CANCELED) => {
                        // Taken here; inside a transaction it's aborted now.
                        self.cut = true;
                    }
                    _ => break,
                }
            }
        }
        while let Ok(n) = self.notes.try_recv() {
            notes(n);
        }
        match result {
            Ok((Some(mut t), _)) => {
                for c in t.cols.iter_mut() {
                    c.numeric = is_numeric_type(&c.ty);
                }
                t.guess_numeric();
                Ok(Outcome::Rows(t))
            }
            Ok((None, n)) => Ok(Outcome::Done(n)),
            Err(e) => {
                let mut f = fail(e, sql);
                f.lost |= self.client.is_closed();
                Err(f)
            }
        }
    }

    fn children(&mut self, path: &[String]) -> Res<Vec<Obj>> {
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        let leaf = |rows: Vec<Vec<String>>, kind: ObjKind, leaf: bool| rows.into_iter().map(|r| obj(r[0].clone(), kind, r.get(1).cloned().unwrap_or_default(), leaf)).collect::<Vec<_>>();
        Ok(match p.as_slice() {
            [] => {
                let mut v = leaf(
                    self.rows(
                        "select nspname::text, coalesce(obj_description(oid, 'pg_namespace'), '') from pg_namespace
                          where nspname !~ '^pg_(toast|temp)' order by nspname = 'public' desc, nspname like 'pg\\_%' or nspname = 'information_schema', nspname",
                        &[],
                    )?,
                    ObjKind::Schema,
                    false,
                );
                if !self.one("select to_regclass('cron.job')::text", &[])?.is_empty() {
                    v.push(folder("jobs"));
                }
                v
            }
            ["jobs"] => leaf(self.rows("select jobid::text || ' ' || coalesce(jobname, ''), schedule || '  ' || database from cron.job order by jobid", &[])?, ObjKind::Job, true),
            [_] => ["tables", "views", "matviews", "functions", "procedures", "sequences", "types"].iter().map(|f| folder(f)).collect(),
            [s, "tables"] => leaf(
                self.rows(
                    "select c.relname::text, coalesce(obj_description(c.oid, 'pg_class'), case when c.reltuples >= 0 then '~' || c.reltuples::bigint::text || ' rows' else '' end)
                       from pg_class c join pg_namespace n on n.oid = c.relnamespace
                      where n.nspname = $1 and c.relkind in ('r', 'p', 'f') and not c.relispartition order by 1",
                    &[s],
                )?,
                ObjKind::Table,
                false,
            ),
            [s, "views"] => leaf(self.rows("select c.relname::text, coalesce(obj_description(c.oid, 'pg_class'), '') from pg_class c join pg_namespace n on n.oid = c.relnamespace where n.nspname = $1 and c.relkind = 'v' order by 1", &[s])?, ObjKind::View, false),
            [s, "matviews"] => leaf(self.rows("select c.relname::text, '' from pg_class c join pg_namespace n on n.oid = c.relnamespace where n.nspname = $1 and c.relkind = 'm' order by 1", &[s])?, ObjKind::MatView, false),
            [s, f @ ("functions" | "procedures")] => {
                let k = if *f == "functions" { "f', 'a', 'w" } else { "p" };
                leaf(
                    self.rows(
                        &format!(
                            "select p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')', coalesce(pg_get_function_result(p.oid), '')
                               from pg_proc p join pg_namespace n on n.oid = p.pronamespace where n.nspname = $1 and p.prokind in ('{k}') order by 1"
                        ),
                        &[s],
                    )?,
                    if *f == "functions" { ObjKind::Function } else { ObjKind::Procedure },
                    true,
                )
            }
            [s, "sequences"] => leaf(self.rows("select sequencename::text, data_type::text from pg_sequences where schemaname = $1 order by 1", &[s])?, ObjKind::Sequence, true),
            [s, "types"] => leaf(
                self.rows(
                    "select t.typname::text, case t.typtype when 'e' then 'enum' when 'c' then 'composite' when 'd' then 'domain' when 'r' then 'range' else '' end
                       from pg_type t join pg_namespace n on n.oid = t.typnamespace
                      where n.nspname = $1 and t.typtype in ('e', 'c', 'd', 'r') and (t.typrelid = 0 or (select relkind from pg_class where oid = t.typrelid) = 'c') order by 1",
                    &[s],
                )?,
                ObjKind::Type,
                true,
            ),
            [_, "tables", _] => ["columns", "keys", "indexes", "triggers"].iter().map(|f| folder(f)).collect(),
            [s, _, t] | [s, _, t, "columns"] => {
                let rel = format!("{}.{}", ident(s), ident(t));
                leaf(
                    self.rows(
                        "select attname::text, format_type(atttypid, atttypmod) || case when attnotnull then ' not null' else '' end from pg_attribute where attrelid = $1::text::regclass and attnum > 0 and not attisdropped order by attnum",
                        &[&rel],
                    )?,
                    ObjKind::Column,
                    true,
                )
            }
            [s, _, t, "keys"] => leaf(self.rows("select conname::text, pg_get_constraintdef(oid, true) from pg_constraint where conrelid = $1::text::regclass order by 1", &[&format!("{}.{}", ident(s), ident(t))])?, ObjKind::Key, true),
            [s, _, t, "indexes"] => leaf(
                self.rows("select c.relname::text, pg_get_indexdef(i.indexrelid) from pg_index i join pg_class c on c.oid = i.indexrelid where i.indrelid = $1::text::regclass order by 1", &[&format!("{}.{}", ident(s), ident(t))])?,
                ObjKind::Index,
                true,
            ),
            [s, _, t, "triggers"] => leaf(self.rows("select tgname::text, '' from pg_trigger where tgrelid = $1::text::regclass and not tgisinternal order by 1", &[&format!("{}.{}", ident(s), ident(t))])?, ObjKind::Trigger, true),
            _ => Vec::new(),
        })
    }

    fn find_objects(&mut self, q: &str) -> Res<Vec<Found>> {
        const SYS: &str = "n.nspname not like 'pg\\_%' and n.nspname <> 'information_schema'";
        let mut out = Vec::new();
        for r in self.rows(&format!("select n.nspname::text, c.relname::text, c.relkind::text from pg_class c join pg_namespace n on n.oid = c.relnamespace where c.relkind in ('r', 'p', 'f', 'v', 'm', 'S') and not c.relispartition and strpos(lower(c.relname), lower($1)) > 0 and {SYS} order by 1, 2 limit 300"), &[q])? {
            if let Some((folder, kind)) = folder_of(&r[2]) {
                out.push(Found { path: vec![r[0].clone(), folder.into(), r[1].clone()], kind, schema: r[0].clone(), name: r[1].clone(), lines: Vec::new() });
            }
        }
        for r in self.rows(&format!("select n.nspname::text, p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')', p.prokind::text from pg_proc p join pg_namespace n on n.oid = p.pronamespace where strpos(lower(p.proname), lower($1)) > 0 and {SYS} order by 1, 2 limit 200"), &[q])? {
            let (folder, kind) = if r[2] == "p" { ("procedures", ObjKind::Procedure) } else { ("functions", ObjKind::Function) };
            out.push(Found { path: vec![r[0].clone(), folder.into(), r[1].clone()], kind, schema: r[0].clone(), name: r[1].clone(), lines: Vec::new() });
        }
        Ok(out)
    }

    fn search_code(&mut self, q: &str) -> Res<Vec<Found>> {
        const SYS: &str = "not like 'pg\\_%'";
        let mut out = Vec::new();
        for r in self.rows(&format!("select n.nspname::text, p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')', p.prokind::text, p.prosrc from pg_proc p join pg_namespace n on n.oid = p.pronamespace where strpos(lower(p.prosrc), lower($1)) > 0 and n.nspname {SYS} and n.nspname <> 'information_schema' and p.prolang <> 12 order by 1, 2 limit 200"), &[q])? {
            let (folder, kind) = if r[2] == "p" { ("procedures", ObjKind::Procedure) } else { ("functions", ObjKind::Function) };
            out.push(Found { path: vec![r[0].clone(), folder.into(), r[1].clone()], kind, schema: r[0].clone(), name: r[1].clone(), lines: lines_with(&r[3], q, 3) });
        }
        for (sql, folder, kind) in [
            (format!("select schemaname::text, viewname::text, definition from pg_views where strpos(lower(definition), lower($1)) > 0 and schemaname {SYS} and schemaname <> 'information_schema' order by 1, 2 limit 200"), "views", ObjKind::View),
            (format!("select schemaname::text, matviewname::text, definition from pg_matviews where strpos(lower(definition), lower($1)) > 0 and schemaname {SYS} order by 1, 2 limit 200"), "matviews", ObjKind::MatView),
        ] {
            for r in self.rows(&sql, &[q])? {
                out.push(Found { path: vec![r[0].clone(), folder.into(), r[1].clone()], kind, schema: r[0].clone(), name: r[1].clone(), lines: lines_with(&r[2], q, 3) });
            }
        }
        if !self.one("select to_regclass('cron.job')::text", &[])?.is_empty() {
            for r in self.rows("select jobid::text || ' ' || coalesce(jobname, ''), command from cron.job where strpos(lower(command), lower($1)) > 0 order by jobid limit 100", &[q])? {
                out.push(Found { path: vec!["jobs".into(), r[0].clone()], kind: ObjKind::Job, schema: "cron".into(), name: r[0].clone(), lines: lines_with(&r[1], q, 3) });
            }
        }
        Ok(out)
    }

    fn locate(&mut self, schema: Option<&str>, name: &str, call: bool) -> Res<Option<(Vec<String>, ObjKind)>> {
        let s = schema.unwrap_or("");
        let rel = |me: &mut Self| -> Res<Option<(Vec<String>, ObjKind)>> {
            let r = me.rows(
                "select n.nspname::text, c.relname::text, c.relkind::text from pg_class c join pg_namespace n on n.oid = c.relnamespace
                  where c.relname = $2 and c.relkind in ('r', 'p', 'f', 'v', 'm', 'S') and (n.nspname = $1 or ($1 = '' and pg_table_is_visible(c.oid)))
                  order by n.nspname = 'public' desc limit 1",
                &[s, name],
            )?;
            Ok(r.into_iter().next().map(|r| {
                let (folder, kind) = match r[2].as_str() {
                    "v" => ("views", ObjKind::View),
                    "m" => ("matviews", ObjKind::MatView),
                    "S" => ("sequences", ObjKind::Sequence),
                    _ => ("tables", ObjKind::Table),
                };
                (vec![r[0].clone(), folder.into(), r[1].clone()], kind)
            }))
        };
        let func = |me: &mut Self| -> Res<Option<(Vec<String>, ObjKind)>> {
            let r = me.rows(
                "select n.nspname::text, p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')', p.prokind::text from pg_proc p join pg_namespace n on n.oid = p.pronamespace
                  where p.proname = $2 and (n.nspname = $1 or ($1 = '' and pg_function_is_visible(p.oid))) and n.nspname not in ('pg_catalog', 'information_schema') limit 1",
                &[s, name],
            )?;
            Ok(r.into_iter().next().map(|r| if r[2] == "p" { (vec![r[0].clone(), "procedures".into(), r[1].clone()], ObjKind::Procedure) } else { (vec![r[0].clone(), "functions".into(), r[1].clone()], ObjKind::Function) }))
        };
        if call {
            if let Some(f) = func(self)? {
                return Ok(Some(f));
            }
            return rel(self);
        }
        if let Some(r) = rel(self)? {
            return Ok(Some(r));
        }
        func(self)
    }

    fn describe(&mut self, path: &[String], kind: ObjKind) -> Res<crate::design::ObjDef> {
        use crate::design::{Column, JobDef, ObjDef, TableDef};
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        match (p.as_slice(), kind) {
            (["jobs", job], ObjKind::Job) => {
                let id = job.split(' ').next().unwrap_or("0");
                let r = self.rows("select jobid::text, coalesce(jobname, ''), schedule, command, database, active::text, username from cron.job where jobid::text = $1 or jobname = $2 order by jobid::text = $1 desc limit 1", &[id, job])?;
                let r = r.into_iter().next().ok_or_else(|| Failure::new("no such job"))?;
                let last = self
                    .rows("select status || ' · ' || to_char(start_time, 'YYYY-MM-DD HH24:MI:SS') from cron.job_run_details where jobid = $1::text::bigint order by start_time desc limit 1", &[id])
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .and_then(|v| v.into_iter().next())
                    .unwrap_or_default();
                let mut info = vec![("jobid".to_string(), r[0].clone()), (crate::i18n::t("пользователь", "user").to_string(), r[6].clone())];
                if !last.is_empty() {
                    info.push((crate::i18n::t("последний запуск", "last run").to_string(), last));
                }
                Ok(ObjDef::Job(JobDef { id: r[0].clone(), name: r[1].clone(), schedule: r[2].clone(), action: r[3].clone(), database: r[4].clone(), enabled: r[5] == "true", info, ..Default::default() }))
            }
            ([s, "tables", t], ObjKind::Table) => {
                let rel = format!("{}.{}", ident(s), ident(t));
                let cols = self.rows(
                    "select a.attname::text, format_type(a.atttypid, a.atttypmod), (not a.attnotnull)::text, coalesce(pg_get_expr(d.adbin, d.adrelid), ''),
                            coalesce(col_description(a.attrelid, a.attnum), ''),
                            coalesce(a.attnum = any(select unnest(i.indkey) from pg_index i where i.indrelid = a.attrelid and i.indisprimary), false)::text
                       from pg_attribute a left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
                      where a.attrelid = $1::text::regclass and a.attnum > 0 and not a.attisdropped order by a.attnum",
                    &[&rel],
                )?;
                let cols = cols.into_iter().map(|r| Column { name: r[0].clone(), ty: r[1].clone(), nullable: r[2] == "true", default: r[3].clone(), comment: r[4].clone(), pk: r[5] == "true" }).collect();
                let comment = self.one("select coalesce(obj_description($1::text::regclass, 'pg_class'), '')", &[&rel])?;
                let info = self.rows("select conname::text, pg_get_constraintdef(oid, true) from pg_constraint where conrelid = $1::text::regclass and contype <> 'n' order by contype = 'p' desc, 1", &[&rel])?.into_iter().map(|r| (r[0].clone(), r[1].clone())).collect();
                Ok(ObjDef::Table(TableDef { schema: s.to_string(), name: t.to_string(), cols, comment, info }))
            }
            _ => Err(Failure::new(crate::i18n::t("Конструктор для этого объекта здесь не поддерживается", "No designer for this object here"))),
        }
    }

    fn source(&mut self, path: &[String], kind: ObjKind) -> Res<String> {
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        let (s, name) = match p.as_slice() {
            ["jobs", job] => {
                let id = job.split(' ').next().unwrap_or("0");
                let r = self.rows("select coalesce(jobname, ''), schedule, command, database from cron.job where jobid::text = $1 or jobname = $2 order by jobid::text = $1 desc limit 1", &[id, job])?;
                let r = r.first().ok_or_else(|| Failure::new("no such job"))?;
                return Ok(format!("-- database: {}\nSELECT cron.schedule({}, {}, $job$\n{}\n$job$);\n", r[3], literal(&r[0]), literal(&r[1]), r[2].trim()));
            }
            [s, _, name] => (*s, *name),
            [s, _, t, _, name] => {
                let rel = format!("{}.{}", ident(s), ident(t));
                return match kind {
                    ObjKind::Index => Ok(self.one("select pg_get_indexdef(c.oid) || ';' from pg_class c where c.oid = $1::text::regclass", &[&format!("{}.{}", ident(s), ident(name))])?),
                    ObjKind::Trigger => Ok(self.one("select pg_get_triggerdef(oid, true) || ';' from pg_trigger where tgrelid = $1::text::regclass and tgname = $2", &[&rel, name])?),
                    _ => self.table_ddl(s, t),
                };
            }
            _ => return Err(Failure::new("nothing to show")),
        };
        let rel = format!("{}.{}", ident(s), ident(name));
        match kind {
            ObjKind::Table => self.table_ddl(s, name),
            ObjKind::View => Ok(format!("CREATE OR REPLACE VIEW {rel} AS\n{}", self.one("select pg_get_viewdef($1::text::regclass, true)", &[&rel])?)),
            ObjKind::MatView => Ok(format!("CREATE MATERIALIZED VIEW {rel} AS\n{}", self.one("select pg_get_viewdef($1::text::regclass, true)", &[&rel])?)),
            ObjKind::Function | ObjKind::Procedure => {
                let def = self.one(
                    "select pg_get_functiondef(p.oid) from pg_proc p join pg_namespace n on n.oid = p.pronamespace
                      where n.nspname = $1 and p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')' = $2",
                    &[s, name],
                )?;
                if def.is_empty() {
                    return Err(Failure::new("not found"));
                }
                Ok(def.trim_end().to_string() + ";\n")
            }
            ObjKind::Sequence => {
                let r = self.rows("select data_type::text, start_value::text, min_value::text, max_value::text, increment_by::text, cycle::text, cache_size::text, coalesce(last_value::text, '') from pg_sequences where schemaname = $1 and sequencename = $2", &[s, name])?;
                let r = r.first().ok_or_else(|| Failure::new("no such sequence"))?;
                Ok(format!(
                    "CREATE SEQUENCE {rel}\n    AS {}\n    INCREMENT BY {}\n    MINVALUE {}\n    MAXVALUE {}\n    START WITH {}\n    CACHE {}{};\n-- last value: {}\n",
                    r[0], r[4], r[2], r[3], r[1], r[6], if r[5] == "true" { "\n    CYCLE" } else { "" }, if r[7].is_empty() { "—" } else { &r[7] }
                ))
            }
            ObjKind::Type => {
                let labels = self.one("select string_agg(quote_literal(e.enumlabel), ', ' order by e.enumsortorder) from pg_enum e where e.enumtypid = $1::text::regtype", &[&rel])?;
                if !labels.is_empty() {
                    return Ok(format!("CREATE TYPE {rel} AS ENUM ({labels});\n"));
                }
                let attrs = self.one("select string_agg(quote_ident(a.attname) || ' ' || format_type(a.atttypid, a.atttypmod), E',\\n    ' order by a.attnum) from pg_attribute a join pg_type t on t.typrelid = a.attrelid where t.oid = $1::text::regtype and a.attnum > 0", &[&rel])?;
                if !attrs.is_empty() {
                    return Ok(format!("CREATE TYPE {rel} AS (\n    {attrs}\n);\n"));
                }
                Ok(format!("-- {rel}: {}\n", self.one("select format_type(typbasetype, typtypmod) from pg_type where oid = $1::text::regtype", &[&rel])?))
            }
            _ => Err(Failure::new("nothing to show")),
        }
    }

    fn relations(&mut self, schemas: &[String]) -> Res<Vec<Relation>> {
        // None asked for: every schema of the user's (those on the search
        // path written bare, the others with their schema).
        let path: Vec<String> = self.one("select array_to_string(current_schemas(false), ',')", &[])?.split(',').map(str::to_string).collect();
        let list = if schemas.is_empty() {
            self.one(
                "select string_agg(nspname, ',') from pg_namespace
                  where nspname !~ '^pg_' and nspname <> 'information_schema' and has_schema_privilege(oid, 'USAGE')",
                &[],
            )?
        } else {
            schemas.join(",")
        };
        let rows = self.rows(
            "select n.nspname::text, c.relname::text, a.attname::text, format_type(a.atttypid, a.atttypmod)
               from pg_class c join pg_namespace n on n.oid = c.relnamespace
               join pg_attribute a on a.attrelid = c.oid and a.attnum > 0 and not a.attisdropped
              where c.relkind in ('r', 'v', 'm', 'p', 'f') and n.nspname = any(string_to_array($1, ','))
              order by 1, 2, a.attnum",
            &[&list],
        )?;
        Ok(qualify(group_relations(rows), &path))
    }

    fn canceller(&self) -> Canceller {
        let token = self.client.cancel_token();
        let tls = self.tls.clone();
        Arc::new(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            match &tls {
                Some(t) => rt.block_on(token.cancel_query(t.clone())).ok(),
                None => rt.block_on(token.cancel_query(tokio_postgres::NoTls)).ok(),
            };
        })
    }

    fn version(&mut self) -> String {
        self.one("select version()", &[]).unwrap_or_default().split(" on ").next().unwrap_or_default().to_string()
    }

    fn alive(&mut self) -> bool {
        !self.client.is_closed()
    }

    fn tx_before(&mut self, sql: &str) -> Res<()> {
        self.sp = false;
        match tx_stmt(sql) {
            TxStmt::Begin | TxStmt::End | TxStmt::Savepoint => return Ok(()),
            TxStmt::Other => {}
        }
        if outside_tx(sql) {
            return Ok(());
        }
        if !self.in_tx {
            self.simple("BEGIN")?;
            self.in_tx = true;
        }
        self.simple("SAVEPOINT zapros_stmt")?;
        self.sp = true;
        Ok(())
    }

    fn tx_after(&mut self, sql: &str, ok: bool) {
        match tx_stmt(sql) {
            TxStmt::Begin if ok => self.in_tx = true,
            TxStmt::End if ok => self.in_tx = false,
            _ => {}
        }
        if std::mem::take(&mut self.sp) {
            // Failed, or cut short by our cancel (which aborts it too).
            let undo = !ok || std::mem::take(&mut self.cut);
            if undo || self.simple("RELEASE SAVEPOINT zapros_stmt").is_err() {
                self.simple("ROLLBACK TO SAVEPOINT zapros_stmt").ok();
                self.simple("RELEASE SAVEPOINT zapros_stmt").ok();
            }
        }
        self.cut = false;
    }

    fn set_auto(&mut self, on: bool) {
        // (Asked for only with nothing uncommitted.)
        if on && self.in_tx {
            self.simple("COMMIT").ok();
            self.in_tx = false;
        }
    }

    fn finish(&mut self, commit: bool) -> Res<()> {
        self.simple(if commit { "COMMIT" } else { "ROLLBACK" })?;
        self.in_tx = false;
        Ok(())
    }

    fn pending(&mut self) -> bool {
        // Changes made: a transaction id given (a failed transaction
        // can't be asked, and it waits for a rollback all the same).
        self.in_tx && self.one("select (txid_current_if_assigned() is not null)::text", &[]).map_or(true, |v| v == "true")
    }
}

/// Rows of schema, table, column, type into relations.
pub fn group_relations(rows: Vec<Vec<String>>) -> Vec<Relation> {
    let mut out: Vec<Relation> = Vec::new();
    for r in rows {
        if out.last().is_none_or(|l| l.schema != r[0] || l.name != r[1]) {
            out.push(Relation { schema: r[0].clone(), name: r[1].clone(), cols: Vec::new(), qualify: false });
        }
        out.last_mut().unwrap().cols.push((r[2].clone(), r.get(3).cloned().unwrap_or_default()));
    }
    out
}
