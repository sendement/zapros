//! ClickHouse over its HTTP interface. A session id keeps SET / USE /
//! temporary tables between statements; rows stream in as JSON arrays, one
//! a line, and reading stops at the limit (the query is killed then).

use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;
use std::time::Duration;

use super::*;

/// How long a busy session is waited for before going on in a new one.
const SESSION_WAIT: Duration = Duration::from_secs(4);

pub struct Ch {
    agent: ureq::Agent,
    base: String,
    user: String,
    password: String,
    database: String,
    session: String,
    readonly: bool,
    /// The query running now (for KILL QUERY).
    running: Arc<Mutex<Option<String>>>,
}

pub fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

pub fn ident(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !s.starts_with(|c: char| c.is_ascii_digit()) {
        s.to_string()
    } else {
        format!("`{}`", s.replace('`', "\\`"))
    }
}

fn value(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

/// "Code: 62. DB::Exception: Syntax error: failed at position 8 (…)"; with
/// no position, where the name it complains about (`x` or 'x') is.
fn fail(body: &str) -> Failure {
    fail_in(body, "")
}

fn fail_in(body: &str, sql: &str) -> Failure {
    let message = body.trim().to_string();
    let mut at = message.find("position ").and_then(|i| message[i + 9..].split(|c: char| !c.is_ascii_digit()).next()?.parse::<usize>().ok()).map(|p| p.saturating_sub(1));
    if at.is_none() && !sql.is_empty() {
        let head = message.split(" In scope").next().unwrap_or(&message);
        let name = ['`', '\''].iter().find_map(|q| {
            let a = head.find(*q)? + 1;
            let b = head[a..].find(*q)? + a;
            Some(head[a..b].to_string())
        });
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            let n = n.rsplit('.').next().unwrap_or(&n).to_string();
            at = sql.match_indices(&n).map(|(i, _)| i).find(|&i| {
                let before = sql[..i].chars().next_back();
                let after = sql[i + n.len()..].chars().next();
                before.is_none_or(|c| !c.is_alphanumeric() && c != '_') && after.is_none_or(|c| !c.is_alphanumeric() && c != '_')
            });
        }
    }
    Failure { message, at, lost: false }
}

/// Statements that give rows back (FORMAT is added to them).
fn returns_rows(sql: &str) -> bool {
    let w = crate::sql::first_word(sql).to_lowercase();
    matches!(w.as_str(), "select" | "with" | "show" | "describe" | "desc" | "exists" | "explain" | "check")
}

fn has_format(sql: &str) -> bool {
    let toks = crate::sql::lex::lex(sql, crate::sql::Dialect::ClickHouse);
    toks.iter().any(|t| t.kind == crate::sql::lex::Tk::Keyword && t.text(sql).eq_ignore_ascii_case("format"))
}

impl Ch {
    pub fn open(spec: &Spec, password: &str) -> Res<Self> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(spec.connect_timeout.max(1))))
            .http_status_as_error(false)
            .build()
            .into();
        let scheme = if spec.tls == Tls::Require { "https" } else { "http" };
        let port = if spec.port == 0 { 8123 } else { spec.port };
        let mut s = Self {
            agent,
            base: format!("{scheme}://{}:{port}/", if spec.host.is_empty() { "localhost" } else { &spec.host }),
            user: if spec.user.is_empty() { "default".into() } else { spec.user.clone() },
            password: password.to_string(),
            database: spec.database.clone(),
            session: format!("zapros-{}", new_id()),
            readonly: spec.read_only,
            running: Arc::default(),
        };
        s.rows("SELECT 1")?;
        Ok(s)
    }

    fn post(&self, sql: &str, query_id: Option<&str>, session: bool) -> Res<ureq::http::Response<ureq::Body>> {
        let mut r = self.agent.post(&self.base).header("X-ClickHouse-User", &self.user).header("X-ClickHouse-Key", &self.password);
        if !self.database.is_empty() {
            r = r.query("database", &self.database);
        }
        if session {
            r = r.query("session_id", &self.session).query("session_timeout", "3600");
        }
        if let Some(id) = query_id {
            r = r.query("query_id", id);
        }
        if self.readonly {
            r = r.query("readonly", "2");
        }
        r = r.query("output_format_json_quote_64bit_integers", "0").query("send_progress_in_http_headers", "0").query("cancel_http_readonly_queries_on_client_close", "1");
        let mut resp = r.send(sql).map_err(|e| Failure::lost(e.to_string()))?;
        if !resp.status().is_success() {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(fail(&body));
        }
        Ok(resp)
    }

    /// Like post, but waits a little while the session is still busy with
    /// a query that was cut short or killed; still busy after that (a query
    /// the server won't let go of), it goes on in a new session — and says
    /// so, as SET / USE / temporary tables stay behind in the old one.
    fn post_retry(&mut self, sql: &str, query_id: Option<&str>, notes: &mut dyn FnMut(String)) -> Res<ureq::http::Response<ureq::Body>> {
        let start = std::time::Instant::now();
        let mut renewed = false;
        loop {
            match self.post(sql, query_id, true) {
                Err(f) if f.message.contains("SESSION_IS_LOCKED") && start.elapsed() < SESSION_WAIT => std::thread::sleep(Duration::from_millis(100)),
                Err(f) if f.message.contains("SESSION_IS_LOCKED") && !renewed => {
                    renewed = true;
                    self.session = format!("zapros-{}", new_id());
                    notes(crate::i18n::t(
                        "Сессия ClickHouse занята другим запросом — открыта новая (SET, USE и временные таблицы прежней здесь не действуют)",
                        "The ClickHouse session is busy with another query — a new one is opened (the old one's SET, USE and temporary tables don't apply here)",
                    ).into());
                }
                r => return r,
            }
        }
    }

    /// Rows as text of a query of our own.
    fn rows(&mut self, sql: &str) -> Res<Vec<Vec<String>>> {
        let mut resp = self.post_retry(&format!("{sql} FORMAT JSONCompactEachRow"), None, &mut |_| {})?;
        let body = resp.body_mut().read_to_string().map_err(|e| Failure::lost(e.to_string()))?;
        let mut out = Vec::new();
        for line in body.lines().filter(|l| !l.trim().is_empty()) {
            let v: Vec<serde_json::Value> = serde_json::from_str(line).map_err(|_| fail(line))?;
            out.push(v.iter().map(|x| value(x).unwrap_or_default()).collect());
        }
        Ok(out)
    }

    fn one(&mut self, sql: &str) -> Res<String> {
        Ok(self.rows(sql)?.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or_default())
    }
}

impl Session for Ch {
    fn run(&mut self, sql: &str, fetch: &mut Fetch, notes: &mut dyn FnMut(String)) -> Res<Outcome> {
        let sql = sql.trim().trim_end_matches(';').trim_end();
        let qid = format!("zapros-{}", new_id());
        let ours = returns_rows(sql) && !has_format(sql);
        let text = if ours { format!("{sql}\nFORMAT JSONCompactEachRowWithNamesAndTypes") } else { sql.to_string() };
        *self.running.lock().unwrap() = Some(qid.clone());
        let resp = self.post_retry(&text, Some(&qid), notes).map_err(|f| if f.lost || f.at.is_some() { f } else { fail_in(&f.message, sql) });
        let result = (|| {
            let mut resp = resp?;
            let summary = resp.headers().get("X-ClickHouse-Summary").and_then(|v| v.to_str().ok()).map(str::to_string);
            let reader = resp.body_mut().as_reader();
            if !ours {
                let mut body = String::new();
                BufReader::new(reader).take(64 << 20).read_to_string(&mut body).map_err(|e| Failure::lost(e.to_string()))?;
                if body.contains("DB::Exception") {
                    return Err(fail_in(&body, sql));
                }
                if body.trim().is_empty() {
                    let written = summary.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v["written_rows"].as_str().and_then(|n| n.parse().ok()));
                    return Ok(Outcome::Done(written));
                }
                // Its own FORMAT: the text as it came, a line to a row.
                let t = Table { cols: vec![Col { name: "result".into(), ..Default::default() }], rows: body.lines().map(|l| vec![Some(l.to_string())]).collect(), more: false };
                return Ok(Outcome::Rows(t));
            }
            let mut lines = BufReader::new(reader).lines();
            let mut next = || -> Res<Option<Vec<serde_json::Value>>> {
                match lines.next() {
                    None => Ok(None),
                    Some(Err(e)) => Err(Failure::lost(e.to_string())),
                    Some(Ok(l)) => serde_json::from_str(&l).map(Some).map_err(|_| fail(&l)),
                }
            };
            let names = next()?.unwrap_or_default();
            let types = next()?.unwrap_or_default();
            let mut t = Table {
                cols: names.iter().zip(types.iter().chain(std::iter::repeat(&serde_json::Value::Null))).map(|(n, ty)| {
                    let ty = value(ty).unwrap_or_default();
                    Col { name: value(n).unwrap_or_default(), numeric: is_numeric_type(&ty) && !ty.starts_with("Array"), ty }
                }).collect(),
                ..Default::default()
            };
            while let Some(r) = next()? {
                if !fetch.room(&mut t) {
                    break;
                }
                t.rows.push(r.iter().map(value).collect());
            }
            Ok(Outcome::Rows(t))
        })();
        // Cut short: the server stops it too.
        if matches!(&result, Ok(Outcome::Rows(t)) if t.more) {
            self.canceller()();
        } else {
            *self.running.lock().unwrap() = None;
        }
        result
    }

    fn children(&mut self, path: &[String]) -> Res<Vec<Obj>> {
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        let list = |rows: Vec<Vec<String>>, kind: ObjKind, leaf: bool| rows.into_iter().map(|r| obj(r[0].clone(), kind, r.get(1).cloned().unwrap_or_default(), leaf)).collect::<Vec<_>>();
        Ok(match p.as_slice() {
            [] => {
                let mut v = list(self.rows("SELECT name, comment FROM system.databases ORDER BY name IN ('system', 'INFORMATION_SCHEMA', 'information_schema'), name = currentDatabase() DESC, name")?, ObjKind::Database, false);
                v.push(folder("functions"));
                v
            }
            ["functions"] => list(self.rows("SELECT name, '' FROM system.functions WHERE origin = 'SQLUserDefined' ORDER BY name")?, ObjKind::Function, true),
            [_] => ["tables", "views", "matviews", "dictionaries"].iter().map(|f| folder(f)).collect(),
            [db, f @ ("tables" | "views" | "matviews" | "dictionaries")] => {
                let cond = match *f {
                    "tables" => "engine NOT IN ('View', 'MaterializedView', 'Dictionary', 'LiveView', 'WindowView')",
                    "views" => "engine = 'View'",
                    "matviews" => "engine = 'MaterializedView'",
                    _ => "engine = 'Dictionary'",
                };
                let kind = match *f {
                    "tables" => ObjKind::Table,
                    "views" => ObjKind::View,
                    "matviews" => ObjKind::MatView,
                    _ => ObjKind::Dictionary,
                };
                list(
                    self.rows(&format!(
                        "SELECT name, if(comment != '', comment, concat(engine, if(total_rows IS NULL, '', concat(' · ', toString(total_rows), ' rows')))) FROM system.tables WHERE database = {} AND {cond} AND NOT is_temporary ORDER BY name",
                        literal(db)
                    ))?,
                    kind,
                    false,
                )
            }
            [db, _, t] => list(
                self.rows(&format!(
                    "SELECT name, concat(type, if(is_in_primary_key, ' · PK', ''), if(comment != '', concat(' · ', comment), '')) FROM system.columns WHERE database = {} AND table = {} ORDER BY position",
                    literal(db),
                    literal(t)
                ))?,
                ObjKind::Column,
                true,
            ),
            _ => Vec::new(),
        })
    }

    fn source(&mut self, path: &[String], kind: ObjKind) -> Res<String> {
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        match (p.as_slice(), kind) {
            (["functions", f], _) => self.one(&format!("SELECT create_query FROM system.functions WHERE name = {}", literal(f))).map(|s| s + ";\n"),
            ([db, _, t], ObjKind::Dictionary) => self.one(&format!("SHOW CREATE DICTIONARY {}.{}", ident(db), ident(t))).map(|s| s + ";\n"),
            ([db, _, t], _) => self.one(&format!("SHOW CREATE TABLE {}.{}", ident(db), ident(t))).map(|s| s + ";\n"),
            _ => Err(Failure::new("nothing to show")),
        }
    }

    fn find_objects(&mut self, q: &str) -> Res<Vec<Found>> {
        let mut out = Vec::new();
        for r in self.rows(&format!("SELECT database, name, engine FROM system.tables WHERE positionCaseInsensitive(name, {}) > 0 AND database NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema') AND NOT is_temporary ORDER BY database, name LIMIT 500", literal(q)))? {
            let (folder, kind) = match r[2].as_str() {
                "View" => ("views", ObjKind::View),
                "MaterializedView" => ("matviews", ObjKind::MatView),
                "Dictionary" => ("dictionaries", ObjKind::Dictionary),
                _ => ("tables", ObjKind::Table),
            };
            out.push(Found { path: vec![r[0].clone(), folder.into(), r[1].clone()], kind, schema: r[0].clone(), name: r[1].clone(), lines: Vec::new() });
        }
        for r in self.rows(&format!("SELECT name FROM system.functions WHERE origin = 'SQLUserDefined' AND positionCaseInsensitive(name, {}) > 0", literal(q)))? {
            out.push(Found { path: vec!["functions".into(), r[0].clone()], kind: ObjKind::Function, schema: String::new(), name: r[0].clone(), lines: Vec::new() });
        }
        Ok(out)
    }

    fn search_code(&mut self, q: &str) -> Res<Vec<Found>> {
        let mut out = Vec::new();
        for r in self.rows(&format!(
            "SELECT database, name, engine, create_table_query FROM system.tables WHERE engine IN ('View', 'MaterializedView', 'Dictionary', 'LiveView', 'WindowView') AND positionCaseInsensitive(create_table_query, {}) > 0 AND database NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema') ORDER BY database, name LIMIT 300",
            literal(q)
        ))? {
            let (folder, kind) = match r[2].as_str() {
                "MaterializedView" => ("matviews", ObjKind::MatView),
                "Dictionary" => ("dictionaries", ObjKind::Dictionary),
                _ => ("views", ObjKind::View),
            };
            // (One line as stored: laid out for the snippet.)
            let text = crate::sql::fmt::format(&r[3], crate::sql::Dialect::ClickHouse, crate::sql::fmt::Options { one_line: 0, ..Default::default() });
            out.push(Found { path: vec![r[0].clone(), folder.into(), r[1].clone()], kind, schema: r[0].clone(), name: r[1].clone(), lines: lines_with(&text, q, 3) });
        }
        for r in self.rows(&format!("SELECT name, create_query FROM system.functions WHERE origin = 'SQLUserDefined' AND positionCaseInsensitive(create_query, {}) > 0", literal(q)))? {
            out.push(Found { path: vec!["functions".into(), r[0].clone()], kind: ObjKind::Function, schema: String::new(), name: r[0].clone(), lines: lines_with(&r[1], q, 3) });
        }
        Ok(out)
    }

    fn locate(&mut self, schema: Option<&str>, name: &str, call: bool) -> Res<Option<(Vec<String>, ObjKind)>> {
        if call {
            let f = self.rows(&format!("SELECT name FROM system.functions WHERE origin = 'SQLUserDefined' AND name = {}", literal(name)))?;
            return Ok(f.into_iter().next().map(|r| (vec!["functions".to_string(), r[0].clone()], ObjKind::Function)));
        }
        let db = schema.map_or("currentDatabase()".to_string(), literal);
        let r = self.rows(&format!("SELECT database, name, engine FROM system.tables WHERE database = {db} AND name = {} LIMIT 1", literal(name)))?;
        Ok(r.into_iter().next().map(|r| {
            let (folder, kind) = match r[2].as_str() {
                "View" => ("views", ObjKind::View),
                "MaterializedView" => ("matviews", ObjKind::MatView),
                "Dictionary" => ("dictionaries", ObjKind::Dictionary),
                _ => ("tables", ObjKind::Table),
            };
            (vec![r[0].clone(), folder.into(), r[1].clone()], kind)
        }))
    }

    fn describe(&mut self, path: &[String], kind: ObjKind) -> Res<crate::design::ObjDef> {
        use crate::design::{Column, ObjDef, TableDef};
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        let ([db, "tables", t], ObjKind::Table) = (p.as_slice(), kind) else {
            return Err(Failure::new(crate::i18n::t("Конструктор для этого объекта здесь не поддерживается", "No designer for this object here")));
        };
        let cols = self
            .rows(&format!(
                "SELECT name, type, if(default_kind IN ('', 'DEFAULT'), default_expression, concat(default_kind, ' ', default_expression)), comment, toString(is_in_primary_key) FROM system.columns WHERE database = {} AND table = {} ORDER BY position",
                literal(db),
                literal(t)
            ))?
            .into_iter()
            .map(|r| Column { name: r[0].clone(), nullable: r[1].starts_with("Nullable("), ty: r[1].clone(), default: r[2].clone(), comment: r[3].clone(), pk: r[4] == "1" })
            .collect();
        let r = self.rows(&format!("SELECT comment, engine, sorting_key, partition_key, primary_key FROM system.tables WHERE database = {} AND name = {}", literal(db), literal(t)))?;
        let r = r.into_iter().next().unwrap_or_default();
        let mut info = Vec::new();
        for (k, i) in [("ENGINE", 1), ("ORDER BY", 2), ("PARTITION BY", 3), ("PRIMARY KEY", 4)] {
            if let Some(v) = r.get(i).filter(|v| !v.is_empty()) {
                info.push((k.to_string(), v.clone()));
            }
        }
        let indexes = self
            .rows(&format!("SELECT name, expr, type_full, toString(granularity) FROM system.data_skipping_indices WHERE database = {} AND table = {} ORDER BY name", literal(db), literal(t)))
            .unwrap_or_default()
            .into_iter()
            .map(|r| crate::design::IndexDef { name: r[0].clone(), cols: r[1].clone(), method: r[2].clone(), granularity: r[3].clone(), ..Default::default() })
            .collect();
        Ok(ObjDef::Table(TableDef { schema: db.to_string(), name: t.to_string(), cols, comment: r.first().cloned().unwrap_or_default(), info, indexes }))
    }

    fn relations(&mut self, schemas: &[String]) -> Res<Vec<Relation>> {
        // None asked for: every database but the system's.
        let cond = if schemas.is_empty() {
            "database NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema')".to_string()
        } else {
            format!("database IN ({})", schemas.iter().map(|s| literal(s)).collect::<Vec<_>>().join(", "))
        };
        let rows = self.rows(&format!("SELECT database, table, name, type FROM system.columns WHERE {cond} ORDER BY database, table, position"))?;
        let current = self.rows("SELECT currentDatabase()")?.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or_default();
        Ok(qualify(super::pg::group_relations(rows), &[current]))
    }

    fn canceller(&self) -> Canceller {
        let (agent, base, user, key, running) = (self.agent.clone(), self.base.clone(), self.user.clone(), self.password.clone(), self.running.clone());
        Arc::new(move || {
            let Some(id) = running.lock().unwrap().take() else { return };
            agent.post(&base).header("X-ClickHouse-User", &user).header("X-ClickHouse-Key", &key).send(format!("KILL QUERY WHERE query_id = {} ASYNC", literal(&id))).ok();
        })
    }

    fn version(&mut self) -> String {
        self.one("SELECT version()").map(|v| format!("ClickHouse {v}")).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session held by a query that won't end soon: the next statement
    /// goes on in a new session (and says so) instead of failing.
    #[test]
    fn busy_session_goes_on_in_a_new_one() {
        let Ok(v) = std::env::var("ZAPROS_TEST_CH") else { return };
        let p: Vec<&str> = v.split(':').collect();
        let spec = Spec { kind: Kind::ClickHouse, host: p[0].into(), port: p[1].parse().unwrap(), database: p[2].into(), user: p[3].into(), ..Default::default() };
        let mut ch = Ch::open(&spec, p[4]).unwrap();
        let (agent, base, user, key, session) = (ch.agent.clone(), ch.base.clone(), ch.user.clone(), ch.password.clone(), ch.session.clone());
        let holder = std::thread::spawn(move || {
            let sql = "SELECT sleepEachRow(1) FROM numbers(9) SETTINGS max_block_size = 1";
            agent.post(&base).header("X-ClickHouse-User", &user).header("X-ClickHouse-Key", &key).query("session_id", &session).send(sql).map(|mut r| r.body_mut().read_to_string().ok()).ok();
        });
        std::thread::sleep(Duration::from_millis(500));
        let old = ch.session.clone();
        let mut said = Vec::new();
        let t0 = std::time::Instant::now();
        let out = ch.run("select 7", &mut Fetch::new(10, |_| 0), &mut |m| said.push(m)).unwrap();
        let Outcome::Rows(t) = out else { panic!("{out:?}") };
        assert_eq!(t.rows[0][0].as_deref(), Some("7"));
        assert_ne!(ch.session, old);
        assert_eq!(said.len(), 1, "{said:?}");
        let took = t0.elapsed();
        assert!(took >= SESSION_WAIT && took < SESSION_WAIT + Duration::from_secs(2), "{took:?}");
        holder.join().ok();
    }
}
