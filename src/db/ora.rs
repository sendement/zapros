//! Oracle through ODPI-C (the `oracle` crate): needs Oracle Instant Client
//! (libclntsh.so) at run time only. DBMS_OUTPUT is on and read after each
//! statement; break_execution stops a running one.

use std::sync::Arc;

use oracle::{Connection, Connector, StatementType};

use super::*;

pub struct Ora {
    conn: Arc<Connection>,
    meta_ready: bool,
    /// Commit after each statement (ODPI's own autocommit stays off: the
    /// connection is shared with the canceller and can't be changed).
    auto: bool,
}

fn fail(e: oracle::Error) -> Failure {
    let message = e.to_string();
    let at = e.db_error().map(|d| d.offset() as usize).filter(|&o| o > 0);
    let lost = ["ORA-03113", "ORA-03114", "ORA-03135", "ORA-02396", "ORA-01012", "ORA-12170", "ORA-12541", "ORA-28547", "DPI-1010", "DPI-1080"].iter().any(|c| message.contains(c));
    Failure { message, at, lost }
}

/// "//host:port/service", or the field as written when there's no host
/// (a TNS alias or a full descriptor).
fn connect_string(spec: &Spec) -> String {
    if spec.host.trim().is_empty() {
        spec.database.clone()
    } else {
        format!("//{}:{}/{}", spec.host.trim(), if spec.port == 0 { 1521 } else { spec.port }, spec.database.trim())
    }
}

/// A script's text without the "/" (and blank lines) at its end.
pub fn strip_slash(text: &str) -> String {
    let mut t = text.trim_end();
    while let Some(rest) = t.strip_suffix('/').filter(|r| r.is_empty() || r.ends_with('\n')) {
        t = rest.trim_end();
    }
    t.to_string()
}

fn kind_folder(f: &str) -> Option<(&'static str, ObjKind)> {
    Some(match f {
        "functions" => ("FUNCTION", ObjKind::Function),
        "procedures" => ("PROCEDURE", ObjKind::Procedure),
        "packages" => ("PACKAGE", ObjKind::Package),
        "sequences" => ("SEQUENCE", ObjKind::Sequence),
        "triggers" => ("TRIGGER", ObjKind::Trigger),
        "types" => ("TYPE", ObjKind::Type),
        "synonyms" => ("SYNONYM", ObjKind::Synonym),
        _ => return None,
    })
}

impl Ora {

    /// An object's DDL from the ALL_ views (what DBMS_METADATA won't give
    /// for another schema without SELECT_CATALOG_ROLE); None: nothing seen.
    fn source_by_hand(&mut self, schema: &str, name: &str, kind: ObjKind) -> Res<Option<String>> {
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let one = |rows: Vec<Vec<String>>| rows.into_iter().next();
        let full = format!("{}.{}", q(schema), q(name));
        Ok(match kind {
            ObjKind::Function | ObjKind::Procedure | ObjKind::Package | ObjKind::Type | ObjKind::Trigger => {
                let rows = self.rows("SELECT type, text FROM all_source WHERE owner = :1 AND name = :2 ORDER BY DECODE(type, 'PACKAGE BODY', 1, 'TYPE BODY', 1, 0), line", &[&schema, &name])?;
                if rows.is_empty() {
                    return Ok(None);
                }
                // Each unit (a spec, a body) on its own, "/" between them.
                let mut units: Vec<(String, String)> = Vec::new();
                for r in rows {
                    match units.last_mut() {
                        Some(u) if u.0 == r[0] => u.1.push_str(&r[1]),
                        _ => units.push((r[0].clone(), r[1].clone())),
                    }
                }
                let parts: Vec<String> = units.into_iter().map(|(ty, text)| format!("CREATE OR REPLACE {}", qualify_unit(text.trim_end(), &ty, schema))).collect();
                Some(parts.join("\n/\n\n"))
            }
            ObjKind::View => one(self.rows("SELECT text FROM all_views WHERE owner = :1 AND view_name = :2", &[&schema, &name])?).map(|r| format!("CREATE OR REPLACE VIEW {full} AS\n{};", r[0].trim_end())),
            ObjKind::MatView => one(self.rows("SELECT query FROM all_mviews WHERE owner = :1 AND mview_name = :2", &[&schema, &name])?).map(|r| format!("CREATE MATERIALIZED VIEW {full} AS\n{};", r[0].trim_end())),
            ObjKind::Sequence => one(self.rows(
                "SELECT min_value, max_value, increment_by, last_number, cache_size, cycle_flag, order_flag FROM all_sequences WHERE sequence_owner = :1 AND sequence_name = :2",
                &[&schema, &name],
            )?)
            .map(|r| {
                let cache = if r[4] == "0" { "NOCACHE".to_string() } else { format!("CACHE {}", r[4]) };
                format!(
                    "CREATE SEQUENCE {full}\n  MINVALUE {} MAXVALUE {}\n  INCREMENT BY {} START WITH {}\n  {cache} {} {};",
                    r[0], r[1], r[2], r[3], if r[5] == "Y" { "CYCLE" } else { "NOCYCLE" }, if r[6] == "Y" { "ORDER" } else { "NOORDER" }
                )
            }),
            ObjKind::Synonym => one(self.rows("SELECT table_owner, table_name, db_link FROM all_synonyms WHERE owner = :1 AND synonym_name = :2", &[&schema, &name])?).map(|r| {
                let link = if r[2].is_empty() { String::new() } else { format!("@{}", r[2]) };
                let owner = if schema == "PUBLIC" { "PUBLIC SYNONYM ".to_string() } else { format!("SYNONYM {full}") };
                let owner = if schema == "PUBLIC" { format!("{owner}{}", q(name)) } else { owner };
                format!("CREATE OR REPLACE {owner} FOR {}.{}{link};", q(&r[0]), q(&r[1]))
            }),
            ObjKind::Table => {
                let def = match self.describe(&[schema.to_string(), "tables".into(), name.to_string()], ObjKind::Table)? {
                    crate::design::ObjDef::Table(t) => t,
                    _ => return Ok(None),
                };
                if def.cols.is_empty() {
                    return Ok(None);
                }
                let width = def.cols.iter().map(|c| q(&c.name).len()).max().unwrap_or(0);
                let mut lines: Vec<String> = def
                    .cols
                    .iter()
                    .map(|c| {
                        let mut l = format!("  {:width$} {}", q(&c.name), c.ty);
                        if !c.default.is_empty() {
                            l += &format!(" DEFAULT {}", c.default);
                        }
                        if !c.nullable {
                            l += " NOT NULL";
                        }
                        l
                    })
                    .collect();
                let pk: Vec<String> = def.cols.iter().filter(|c| c.pk).map(|c| q(&c.name)).collect();
                if !pk.is_empty() {
                    lines.push(format!("  PRIMARY KEY ({})", pk.join(", ")));
                }
                let mut out = format!("CREATE TABLE {full} (\n{}\n);", lines.join(",\n"));
                let lit = |s: &str| format!("'{}'", s.replace('\'', "''"));
                if !def.comment.is_empty() {
                    out += &format!("\n\nCOMMENT ON TABLE {full} IS {};", lit(&def.comment));
                }
                for c in def.cols.iter().filter(|c| !c.comment.is_empty()) {
                    out += &format!("\nCOMMENT ON COLUMN {full}.{} IS {};", q(&c.name), lit(&c.comment));
                }
                Some(out)
            }
            _ => None,
        })
    }
    pub fn open(spec: &Spec, password: &str) -> Res<Self> {
        if let Err(message) = crate::drivers::init_oracle() {
            return Err(Failure { message: format!("{message}\n\n{}", crate::drivers::hint()), at: None, lost: false });
        }
        let mut c = Connector::new(spec.user.as_str(), password, connect_string(spec));
        let mut user = spec.user.trim().to_string();
        // "sys as sysdba"
        let up = user.to_uppercase();
        if let Some(name) = up.strip_suffix(" AS SYSDBA") {
            user = spec.user[..name.len()].to_string();
            c = Connector::new(user.as_str(), password, connect_string(spec));
            c.privilege(oracle::Privilege::Sysdba);
        }
        let mut conn = c.connect().map_err(|e| {
            let mut f = fail(e);
            if f.message.contains("DPI-1047") {
                f.message = format!("{}\n\n{}", f.message, crate::drivers::hint());
            }
            f
        })?;
        conn.set_autocommit(false);
        conn.execute("BEGIN DBMS_OUTPUT.ENABLE(NULL); END;", &[]).ok();
        if spec.read_only {
            conn.execute("SET TRANSACTION READ ONLY", &[]).ok();
        }
        Ok(Self { conn: Arc::new(conn), meta_ready: false, auto: true })
    }

    fn rows(&self, sql: &str, params: &[&dyn oracle::sql_type::ToSql]) -> Res<Vec<Vec<String>>> {
        let rs = self.conn.query(sql, params).map_err(fail)?;
        let mut out = Vec::new();
        for r in rs {
            let r = r.map_err(fail)?;
            out.push((0..r.sql_values().len()).map(|i| r.get::<usize, Option<String>>(i).ok().flatten().unwrap_or_default()).collect());
        }
        Ok(out)
    }

    fn run_one(&mut self, sql: &str, fetch: &mut Fetch, notes: &mut dyn FnMut(String)) -> Res<Outcome> {
        let sql = tidy(sql);
        let mut st = self.conn.statement(&sql).fetch_array_size(500).build().map_err(fail)?;
        let r = if st.is_query() {
            let rs = st.query(&[]).map_err(fail)?;
            let cols: Vec<Col> = rs
                .column_info()
                .iter()
                .map(|c| {
                    let ty = c.oracle_type().to_string();
                    Col { name: c.name().to_string(), numeric: is_numeric_type(&ty), ty }
                })
                .collect();
            let mut t = Table { cols, ..Default::default() };
            for row in rs {
                if !fetch.room(&mut t) {
                    break;
                }
                let row = row.map_err(fail)?;
                t.rows.push(
                    (0..row.sql_values().len())
                        .map(|i| match row.get::<usize, Option<String>>(i) {
                            Ok(v) => v,
                            Err(_) => Some(format!("<{}>", row.column_info()[i].oracle_type())),
                        })
                        .collect(),
                );
            }
            Ok(Outcome::Rows(t))
        } else {
            st.execute(&[]).map_err(fail)?;
            let n = match st.statement_type() {
                StatementType::Insert | StatementType::Update | StatementType::Delete | StatementType::Merge => st.row_count().ok(),
                _ => None,
            };
            Ok(Outcome::Done(n))
        };
        self.output(notes);
        // CREATE PROCEDURE … with errors: Oracle only warns; show them.
        if r.is_ok() && matches!(st.statement_type(), StatementType::Create | StatementType::Alter) {
            if let Some((owner, ty, name)) = created_unit(&sql) {
                let q = "SELECT line || ':' || position || ' ' || text FROM all_errors WHERE attribute = 'ERROR' AND owner = NVL(:1, USER) AND type = :2 AND name = :3 ORDER BY sequence";
                if let Ok(errs) = self.rows(q, &[&owner, &ty, &name]) {
                    if !errs.is_empty() {
                        return Err(Failure::new(format!("{ty} {name}: compiled with errors\n{}", errs.into_iter().map(|e| e.join(" ")).collect::<Vec<_>>().join("\n"))));
                    }
                }
            }
        }
        r
    }

    fn output(&self, notes: &mut dyn FnMut(String)) {
        let Ok(mut st) = self.conn.statement("BEGIN DBMS_OUTPUT.GET_LINE(:1, :2); END;").build() else { return };
        for _ in 0..100_000 {
            if st.execute(&[&oracle::sql_type::OracleType::Varchar2(32767), &oracle::sql_type::OracleType::Number(0, 0)]).is_err() {
                return;
            }
            let status: i32 = st.bind_value(2).unwrap_or(1);
            if status != 0 {
                return;
            }
            let line: Option<String> = st.bind_value(1).unwrap_or(None);
            notes(line.unwrap_or_default());
        }
    }
}

/// What a CREATE makes: (owner, type as in ALL_ERRORS, name).
fn created_unit(sql: &str) -> Option<(Option<String>, String, String)> {
    use crate::sql::lex::{Tk, lex};
    let toks: Vec<_> = lex(sql, crate::sql::Dialect::Oracle).into_iter().filter(|t| !t.trivia()).collect();
    let w = |i: usize| toks.get(i).map(|t| t.text(sql).to_ascii_uppercase()).unwrap_or_default();
    let mut i = 0;
    if !matches!(w(0).as_str(), "CREATE" | "ALTER") {
        return None;
    }
    i += 1;
    while matches!(w(i).as_str(), "OR" | "REPLACE" | "EDITIONABLE" | "NONEDITIONABLE") {
        i += 1;
    }
    let mut ty = w(i);
    if !matches!(ty.as_str(), "PROCEDURE" | "FUNCTION" | "PACKAGE" | "TRIGGER" | "TYPE" | "VIEW") {
        return None;
    }
    i += 1;
    if w(i) == "BODY" {
        ty += " BODY";
        i += 1;
    }
    let name = |t: &crate::sql::lex::Tok| if t.kind == Tk::Quoted { t.text(sql).trim_matches('"').to_string() } else { t.text(sql).to_ascii_uppercase() };
    let first = toks.get(i)?;
    if toks.get(i + 1).is_some_and(|t| t.kind == Tk::Dot) {
        Some((Some(name(first)), ty, name(toks.get(i + 2)?)))
    } else {
        Some((None, ty, name(first)))
    }
}

/// SQL ends without ";", PL/SQL keeps its END;.
fn tidy(sql: &str) -> String {
    let s = sql.trim();
    let s = s.strip_suffix('/').map(str::trim_end).unwrap_or(s);
    if crate::sql::is_plsql_block(s, crate::sql::Dialect::Oracle) { s.to_string() } else { s.trim_end_matches(';').trim_end().to_string() }
}

impl Session for Ora {
    fn run(&mut self, sql: &str, fetch: &mut Fetch, notes: &mut dyn FnMut(String)) -> Res<Outcome> {
        let r = self.run_one(sql, fetch, notes);
        if self.auto && r.is_ok() {
            self.conn.commit().map_err(fail)?;
        }
        r
    }

    fn set_auto(&mut self, on: bool) {
        if on && !self.auto {
            self.conn.commit().ok();
        }
        self.auto = on;
    }

    fn finish(&mut self, commit: bool) -> Res<()> {
        if commit { self.conn.commit() } else { self.conn.rollback() }.map_err(fail)
    }

    fn pending(&mut self) -> bool {
        self.rows("select dbms_transaction.local_transaction_id from dual", &[]).is_ok_and(|r| r.first().and_then(|r| r.first()).is_some_and(|v| !v.is_empty()))
    }

    fn children(&mut self, path: &[String]) -> Res<Vec<Obj>> {
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        let list = |rows: Vec<Vec<String>>, kind: ObjKind, leaf: bool| rows.into_iter().map(|r| obj(r[0].clone(), kind, r.get(1).cloned().unwrap_or_default(), leaf)).collect::<Vec<_>>();
        Ok(match p.as_slice() {
            [] => list(
                self.rows("SELECT username, '' FROM all_users ORDER BY CASE WHEN username = USER THEN 0 ELSE 1 END, CASE WHEN oracle_maintained = 'Y' THEN 1 ELSE 0 END, username", &[])
                    .or_else(|_| self.rows("SELECT username, '' FROM all_users ORDER BY CASE WHEN username = USER THEN 0 ELSE 1 END, username", &[]))?,
                ObjKind::Schema,
                false,
            ),
            [_] => ["tables", "views", "matviews", "functions", "procedures", "packages", "sequences", "triggers", "types", "synonyms", "jobs"].iter().map(|f| folder(f)).collect(),
            [s, "tables"] => list(
                self.rows(
                    "SELECT t.table_name, NVL(c.comments, CASE WHEN t.num_rows IS NOT NULL THEN '~' || t.num_rows || ' rows' END) FROM all_tables t
                       LEFT JOIN all_tab_comments c ON c.owner = t.owner AND c.table_name = t.table_name
                      WHERE t.owner = :1 AND t.nested = 'NO' AND t.secondary = 'N' AND t.table_name NOT LIKE 'BIN$%' ORDER BY 1",
                    &[s],
                )?,
                ObjKind::Table,
                false,
            ),
            [s, "views"] => list(self.rows("SELECT view_name, '' FROM all_views WHERE owner = :1 ORDER BY 1", &[s])?, ObjKind::View, false),
            [s, "matviews"] => list(self.rows("SELECT mview_name, refresh_method || ' ' || refresh_mode FROM all_mviews WHERE owner = :1 ORDER BY 1", &[s])?, ObjKind::MatView, false),
            [s, "jobs"] => list(self.rows("SELECT job_name, enabled || ' · ' || state || ' · ' || repeat_interval FROM all_scheduler_jobs WHERE owner = :1 ORDER BY 1", &[s])?, ObjKind::Job, true),
            [s, f] => match kind_folder(f) {
                Some((ty, kind)) => list(self.rows("SELECT object_name, CASE WHEN status <> 'VALID' THEN status END FROM all_objects WHERE owner = :1 AND object_type = :2 ORDER BY 1", &[s, &ty])?, kind, true),
                None => Vec::new(),
            },
            [_, "tables", _] => ["columns", "keys", "indexes", "triggers"].iter().map(|f| folder(f)).collect(),
            [s, _, t] | [s, _, t, "columns"] => list(
                self.rows(
                    "SELECT column_name, data_type || CASE WHEN data_type IN ('VARCHAR2', 'CHAR', 'NVARCHAR2', 'RAW') THEN '(' || data_length || ')'
                            WHEN data_type = 'NUMBER' AND data_precision IS NOT NULL THEN '(' || data_precision || ',' || data_scale || ')' END
                            || CASE WHEN nullable = 'N' THEN ' not null' END
                       FROM all_tab_columns WHERE owner = :1 AND table_name = :2 ORDER BY column_id",
                    &[s, t],
                )?,
                ObjKind::Column,
                true,
            ),
            [s, _, t, "keys"] => list(
                self.rows("SELECT constraint_name, DECODE(constraint_type, 'P', 'primary key', 'U', 'unique', 'R', 'foreign key', 'C', 'check', constraint_type) FROM all_constraints WHERE owner = :1 AND table_name = :2 AND constraint_type IN ('P', 'U', 'R', 'C') AND NOT (constraint_type = 'C' AND constraint_name LIKE 'SYS_C%') ORDER BY 1", &[s, t])?,
                ObjKind::Key,
                true,
            ),
            [s, _, t, "indexes"] => list(self.rows("SELECT index_name, index_type || CASE WHEN uniqueness = 'UNIQUE' THEN ' unique' END FROM all_indexes WHERE table_owner = :1 AND table_name = :2 ORDER BY 1", &[s, t])?, ObjKind::Index, true),
            [s, _, t, "triggers"] => list(self.rows("SELECT trigger_name, trigger_type || ' ' || triggering_event FROM all_triggers WHERE table_owner = :1 AND table_name = :2 ORDER BY 1", &[s, t])?, ObjKind::Trigger, true),
            _ => Vec::new(),
        })
    }

    fn source(&mut self, path: &[String], kind: ObjKind) -> Res<String> {
        if !self.meta_ready {
            self.conn
                .execute(
                    "BEGIN
                       DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'STORAGE', FALSE);
                       DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'SEGMENT_ATTRIBUTES', FALSE);
                       DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'SQLTERMINATOR', TRUE);
                       DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'PRETTY', TRUE);
                     END;",
                    &[],
                )
                .ok();
            self.meta_ready = true;
        }
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        let (schema, name) = match p.as_slice() {
            [s, _, n] => (*s, *n),
            [s, _, _, _, n] => (*s, *n),
            _ => return Err(Failure::new("nothing to show")),
        };
        let ty = match kind {
            ObjKind::Table => "TABLE",
            ObjKind::View => "VIEW",
            ObjKind::MatView => "MATERIALIZED_VIEW",
            ObjKind::Function => "FUNCTION",
            ObjKind::Procedure => "PROCEDURE",
            ObjKind::Package => "PACKAGE",
            ObjKind::Sequence => "SEQUENCE",
            ObjKind::Trigger => "TRIGGER",
            ObjKind::Type => "TYPE",
            ObjKind::Synonym => "SYNONYM",
            ObjKind::Index => "INDEX",
            ObjKind::Job => "PROCOBJ",
            _ => return Err(Failure::new("nothing to show")),
        };
        let mut text = match self.rows("SELECT DBMS_METADATA.GET_DDL(:1, :2, :3) FROM dual", &[&ty, &name, &schema]) {
            Ok(r) => r.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or_default().trim().to_string(),
            // Another schema's object without SELECT_CATALOG_ROLE (ORA-31603
            // though the object's own grants let it be seen): put together
            // from what the dictionary's ALL_ views show.
            Err(e) => match self.source_by_hand(schema, name, kind) {
                Ok(Some(t)) => t,
                Ok(None) => return Err(e),
                Err(_) => return Err(e),
            },
        };
        // Two PL/SQL units (a spec and its body): a "/" between them, for
        // running them as a script; none at the end (the last one runs to
        // the text's end).
        if matches!(kind, ObjKind::Function | ObjKind::Procedure | ObjKind::Package | ObjKind::Trigger | ObjKind::Type) {
            text = text.replace("\n  CREATE OR REPLACE", "\n/\n\nCREATE OR REPLACE");
            while text.contains("/\n/\n") {
                text = text.replace("/\n/\n", "/\n");
            }
        }
        Ok(strip_slash(&text) + "\n")
    }

    fn find_objects(&mut self, q: &str) -> Res<Vec<Found>> {
        let rows = self.rows(
            "SELECT owner, object_name, object_type FROM all_objects
              WHERE INSTR(UPPER(object_name), UPPER(:1)) > 0
                AND object_type IN ('TABLE', 'VIEW', 'MATERIALIZED VIEW', 'FUNCTION', 'PROCEDURE', 'PACKAGE', 'SEQUENCE', 'TRIGGER', 'TYPE', 'JOB', 'SYNONYM')
                AND owner IN (SELECT username FROM all_users WHERE oracle_maintained = 'N')
                AND ROWNUM <= 500
              ORDER BY owner, object_name",
            &[&q],
        )?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let (folder, kind) = if r[2] == "SYNONYM" { ("synonyms", ObjKind::Synonym) } else { folder_of(&r[2])? };
                Some(Found { path: vec![r[0].clone(), folder.into(), r[1].clone()], kind, schema: r[0].clone(), name: r[1].clone(), lines: Vec::new() })
            })
            .collect())
    }

    fn search_code(&mut self, q: &str) -> Res<Vec<Found>> {
        let mut out: Vec<Found> = Vec::new();
        // PL/SQL: line by line in ALL_SOURCE (each unit once, its lines).
        let rows = self.rows(
            "SELECT owner, name, type, line, text FROM all_source
              WHERE INSTR(UPPER(text), UPPER(:1)) > 0
                AND owner IN (SELECT username FROM all_users WHERE oracle_maintained = 'N')
                AND ROWNUM <= 3000
              ORDER BY owner, name, type, line",
            &[&q],
        )?;
        for r in rows {
            let Some((folder, kind)) = folder_of(&r[2]) else { continue };
            let line = (r[3].parse().unwrap_or(0), around(r[4].trim(), q));
            match out.iter_mut().find(|f| f.schema == r[0] && f.name == r[1] && f.kind == kind) {
                Some(f) if f.lines.len() < 3 => f.lines.push(line),
                Some(_) => {}
                None => out.push(Found { path: vec![r[0].clone(), folder.into(), r[1].clone()], kind, schema: r[0].clone(), name: r[1].clone(), lines: vec![line] }),
            }
        }
        // Views (their first 4000 chars), jobs' actions.
        for r in self.rows("SELECT owner, view_name, text_vc FROM all_views WHERE INSTR(UPPER(text_vc), UPPER(:1)) > 0 AND owner IN (SELECT username FROM all_users WHERE oracle_maintained = 'N') AND ROWNUM <= 300", &[&q])? {
            out.push(Found { path: vec![r[0].clone(), "views".into(), r[1].clone()], kind: ObjKind::View, schema: r[0].clone(), name: r[1].clone(), lines: lines_with(&r[2], q, 3) });
        }
        for r in self.rows("SELECT owner, job_name, job_action FROM all_scheduler_jobs WHERE INSTR(UPPER(job_action), UPPER(:1)) > 0 AND owner IN (SELECT username FROM all_users WHERE oracle_maintained = 'N') AND ROWNUM <= 200", &[&q])? {
            out.push(Found { path: vec![r[0].clone(), "jobs".into(), r[1].clone()], kind: ObjKind::Job, schema: r[0].clone(), name: r[1].clone(), lines: lines_with(&r[2], q, 3) });
        }
        Ok(out)
    }

    fn locate(&mut self, schema: Option<&str>, name: &str, _call: bool) -> Res<Option<(Vec<String>, ObjKind)>> {
        let pick = |rows: Vec<Vec<String>>| -> Option<(Vec<String>, ObjKind)> {
            let r = rows.into_iter().next()?;
            let (folder, kind) = match r[2].as_str() {
                "TABLE" => ("tables", ObjKind::Table),
                "VIEW" => ("views", ObjKind::View),
                "MATERIALIZED VIEW" => ("matviews", ObjKind::MatView),
                "FUNCTION" => ("functions", ObjKind::Function),
                "PROCEDURE" => ("procedures", ObjKind::Procedure),
                "PACKAGE" | "PACKAGE BODY" => ("packages", ObjKind::Package),
                "SEQUENCE" => ("sequences", ObjKind::Sequence),
                "TRIGGER" => ("triggers", ObjKind::Trigger),
                "TYPE" | "TYPE BODY" => ("types", ObjKind::Type),
                "JOB" => ("jobs", ObjKind::Job),
                _ => return None,
            };
            Some((vec![r[0].clone(), folder.into(), r[1].clone()], kind))
        };
        let q = "SELECT owner, object_name, object_type FROM all_objects WHERE owner = NVL(:1, USER) AND object_name = :2
                  AND object_type IN ('TABLE', 'VIEW', 'MATERIALIZED VIEW', 'FUNCTION', 'PROCEDURE', 'PACKAGE', 'SEQUENCE', 'TRIGGER', 'TYPE', 'JOB')
                  ORDER BY DECODE(object_type, 'TABLE', 0, 'MATERIALIZED VIEW', 1, 2)";
        let none: Option<&str> = None;
        if let Some(s) = schema {
            if let Some(f) = pick(self.rows(q, &[&s, &name])?) {
                return Ok(Some(f));
            }
            // pkg.member: the package (in the session's schema, or by a synonym).
            if let Some(f) = pick(self.rows(q, &[&none, &s])?) {
                return Ok(Some(f));
            }
            let syn = self.rows("SELECT table_owner, table_name FROM all_synonyms WHERE synonym_name = :1 AND owner IN (USER, 'PUBLIC') ORDER BY DECODE(owner, 'PUBLIC', 1, 0)", &[&s])?;
            if let Some(r) = syn.into_iter().next() {
                return Ok(pick(self.rows(q, &[&r[0], &r[1]])?));
            }
            return Ok(None);
        }
        if let Some(f) = pick(self.rows(q, &[&none, &name])?) {
            return Ok(Some(f));
        }
        let syn = self.rows("SELECT table_owner, table_name FROM all_synonyms WHERE synonym_name = :1 AND owner IN (USER, 'PUBLIC') ORDER BY DECODE(owner, 'PUBLIC', 1, 0)", &[&name])?;
        match syn.into_iter().next() {
            Some(r) => Ok(pick(self.rows(q, &[&r[0], &r[1]])?)),
            None => Ok(None),
        }
    }

    fn describe(&mut self, path: &[String], kind: ObjKind) -> Res<crate::design::ObjDef> {
        use crate::design::{Column, JobDef, ObjDef, TableDef};
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        match (p.as_slice(), kind) {
            ([s, "jobs", name], ObjKind::Job) => {
                let r = self.rows(
                    "SELECT job_name, NVL(repeat_interval, ''), NVL(job_action, NVL(program_name, '')), NVL(job_type, ''), enabled, NVL(comments, ''), state,
                            TO_CHAR(next_run_date, 'YYYY-MM-DD HH24:MI:SS'), TO_CHAR(last_start_date, 'YYYY-MM-DD HH24:MI:SS'), run_count, failure_count, NVL(schedule_name, '')
                       FROM all_scheduler_jobs WHERE owner = :1 AND job_name = :2",
                    &[s, name],
                )?;
                let r = r.into_iter().next().ok_or_else(|| Failure::new("no such job"))?;
                let t = crate::i18n::t;
                let mut info = vec![(t("состояние", "state").to_string(), r[6].clone()), (t("следующий запуск", "next run").to_string(), r[7].clone()), (t("последний запуск", "last run").to_string(), r[8].clone()), (t("запусков / сбоев", "runs / failures").to_string(), format!("{} / {}", r[9], r[10]))];
                if !r[11].is_empty() {
                    info.push((t("расписание (объект)", "schedule (object)").to_string(), r[11].clone()));
                }
                Ok(ObjDef::Job(JobDef { owner: s.to_string(), name: r[0].clone(), schedule: r[1].clone(), action: r[2].clone(), action_type: r[3].clone(), enabled: r[4] == "TRUE", comment: r[5].clone(), info, ..Default::default() }))
            }
            ([s, "tables", name], ObjKind::Table) => {
                let rows = self.rows(
                    "SELECT c.column_name,
                            CASE WHEN c.data_type IN ('VARCHAR2', 'NVARCHAR2', 'CHAR', 'NCHAR') THEN c.data_type || '(' || c.char_length || CASE WHEN c.char_used = 'C' AND c.data_type IN ('VARCHAR2', 'CHAR') THEN ' CHAR' END || ')'
                                 WHEN c.data_type = 'RAW' THEN 'RAW(' || c.data_length || ')'
                                 WHEN c.data_type = 'NUMBER' AND c.data_precision IS NOT NULL THEN 'NUMBER(' || c.data_precision || CASE WHEN c.data_scale <> 0 THEN ',' || c.data_scale END || ')'
                                 WHEN c.data_type = 'NUMBER' AND c.data_scale = 0 THEN 'INTEGER'
                                 ELSE c.data_type END,
                            c.nullable, c.data_default, NVL(m.comments, ''),
                            CASE WHEN EXISTS (SELECT 1 FROM all_constraints k JOIN all_cons_columns kc ON kc.owner = k.owner AND kc.constraint_name = k.constraint_name
                                               WHERE k.owner = c.owner AND k.table_name = c.table_name AND k.constraint_type = 'P' AND kc.column_name = c.column_name) THEN 'Y' ELSE 'N' END
                       FROM all_tab_columns c LEFT JOIN all_col_comments m ON m.owner = c.owner AND m.table_name = c.table_name AND m.column_name = c.column_name
                      WHERE c.owner = :1 AND c.table_name = :2 ORDER BY c.column_id",
                    &[s, name],
                )?;
                let cols = rows.into_iter().map(|r| Column { name: r[0].clone(), ty: r[1].clone(), nullable: r[2] == "Y", default: r[3].trim().to_string(), comment: r[4].clone(), pk: r[5] == "Y" }).collect();
                let comment = self.rows("SELECT NVL(comments, '') FROM all_tab_comments WHERE owner = :1 AND table_name = :2", &[s, name])?.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or_default();
                let info = self
                    .rows(
                        "SELECT constraint_name, DECODE(constraint_type, 'P', 'PRIMARY KEY', 'U', 'UNIQUE', 'R', 'FOREIGN KEY', 'C', 'CHECK', constraint_type) FROM all_constraints
                          WHERE owner = :1 AND table_name = :2 AND constraint_type IN ('P', 'U', 'R') ORDER BY DECODE(constraint_type, 'P', 0, 1), 1",
                        &[s, name],
                    )?
                    .into_iter()
                    .map(|r| (r[0].clone(), r[1].clone()))
                    .collect();
                let list = self.rows(
                    "SELECT i.index_name, i.uniqueness, i.index_type,
                            CASE WHEN EXISTS (SELECT 1 FROM all_constraints k WHERE k.owner = i.table_owner AND k.table_name = i.table_name AND k.index_name = i.index_name AND k.constraint_type IN ('P', 'U')) THEN 'Y' ELSE 'N' END
                       FROM all_indexes i WHERE i.table_owner = :1 AND i.table_name = :2 AND i.index_type NOT IN ('LOB', 'IOT - TOP', 'CLUSTER')
                      ORDER BY 4 DESC, 1",
                    &[s, name],
                )?;
                let ixcols = self.rows("SELECT index_name, column_name, descend, column_position FROM all_ind_columns WHERE table_owner = :1 AND table_name = :2 ORDER BY index_name, column_position", &[s, name])?;
                let exprs = self.rows("SELECT index_name, column_position, column_expression FROM all_ind_expressions WHERE table_owner = :1 AND table_name = :2", &[s, name])?;
                let indexes = list
                    .into_iter()
                    .map(|r| {
                        let on: Vec<String> = ixcols
                            .iter()
                            .filter(|c| c[0] == r[0])
                            .map(|c| {
                                let e = exprs.iter().find(|e| e[0] == r[0] && e[1] == c[3]).map(|e| e[2].trim().to_string());
                                let what = e.unwrap_or_else(|| crate::sql::complete::quote(&c[1], crate::sql::Dialect::Oracle));
                                if c[2] == "DESC" { format!("{what} DESC") } else { what }
                            })
                            .collect();
                        let method = if r[2].contains("BITMAP") { "BITMAP" } else { "NORMAL" };
                        crate::design::IndexDef { name: r[0].clone(), cols: on.join(", "), unique: r[1] == "UNIQUE", method: method.into(), constraint: r[3] == "Y", ..Default::default() }
                    })
                    .collect();
                Ok(ObjDef::Table(TableDef { schema: s.to_string(), name: name.to_string(), cols, comment, info, indexes }))
            }
            _ => Err(Failure::new(crate::i18n::t("Конструктор для этого объекта здесь не поддерживается", "No designer for this object here"))),
        }
    }

    fn relations(&mut self, schemas: &[String]) -> Res<Vec<Relation>> {
        let mut out = Vec::new();
        let me = self.rows("SELECT USER FROM dual", &[])?.remove(0).remove(0);
        // None asked for: the user's own and what others (not Oracle's own
        // schemas) let it see.
        let list: Vec<String> = if schemas.is_empty() {
            let mut l = vec![me.clone()];
            let others = self.rows(
                "SELECT DISTINCT t.owner FROM all_tables t JOIN all_users u ON u.username = t.owner
                  WHERE u.oracle_maintained = 'N' AND t.owner <> USER ORDER BY 1",
                &[],
            )?;
            l.extend(others.into_iter().filter_map(|r| r.into_iter().next()));
            l
        } else {
            schemas.to_vec()
        };
        for s in list {
            let rows = self.rows("SELECT owner, table_name, column_name, data_type FROM all_tab_columns WHERE owner = :1 AND table_name NOT LIKE 'BIN$%' ORDER BY table_name, column_id", &[&s])?;
            out.extend(super::pg::group_relations(rows));
        }
        Ok(qualify(out, &[me]))
    }

    fn server_id(&mut self) -> Option<String> {
        // (DBMS_DEBUG_JDWP's are open to every user; v$session often isn't.)
        self.rows("SELECT DBMS_DEBUG_JDWP.CURRENT_SESSION_ID || ',' || DBMS_DEBUG_JDWP.CURRENT_SESSION_SERIAL FROM dual", &[]).ok()?.into_iter().next()?.into_iter().next().filter(|s| s.len() > 2)
    }

    fn kill(&mut self, id: &str) -> Res<()> {
        if !id.chars().all(|c| c.is_ascii_digit() || c == ',') {
            return Err(Failure::new("bad session id"));
        }
        self.conn.execute(&format!("ALTER SYSTEM KILL SESSION '{id}' IMMEDIATE"), &[]).map_err(fail)?;
        Ok(())
    }

    fn canceller(&self) -> Canceller {
        let c = self.conn.clone();
        Arc::new(move || {
            c.break_execution().ok();
        })
    }

    fn version(&mut self) -> String {
        self.conn.server_version().map(|(v, banner)| banner.lines().next().map(str::to_string).unwrap_or_else(|| format!("Oracle {v}"))).unwrap_or_default()
    }

    fn alive(&mut self) -> bool {
        self.conn.status().is_ok_and(|s| s == oracle::ConnStatus::Normal)
    }
}

#[cfg(test)]
mod slash_tests {
    #[test]
    fn no_slash_at_the_end() {
        assert_eq!(super::strip_slash("create function f return number is begin return 1; end;\n/\n"), "create function f return number is begin return 1; end;");
        assert_eq!(super::strip_slash("begin null; end;\n/\n/\n"), "begin null; end;");
        assert_eq!(super::strip_slash("select 4/2 from dual"), "select 4/2 from dual");
    }
}

/// A unit's source as ALL_SOURCE keeps it ("PACKAGE BODY pkg AS …"), its
/// name given the schema: "PACKAGE BODY "S"."PKG" AS …".
fn qualify_unit(text: &str, ty: &str, schema: &str) -> String {
    let lead = text.len() - text.trim_start().len();
    let rest = &text[lead..];
    // The unit's keywords ("PACKAGE BODY", "TYPE", …), as many words as `ty`.
    let mut at = 0;
    for _ in ty.split_whitespace() {
        let r = &rest[at..];
        let skip = r.len() - r.trim_start().len();
        let w = r[skip..].find(|c: char| c.is_whitespace()).unwrap_or(r.len() - skip);
        at += skip + w;
    }
    let after = &rest[at..];
    let gap = after.len() - after.trim_start().len();
    let name_at = at + gap;
    let tail = &rest[name_at..];
    // Already qualified ("S".pkg / s.pkg): as it is.
    let end = if tail.starts_with('"') { tail[1..].find('"').map_or(tail.len(), |i| i + 2) } else { tail.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$' || c == '#')).unwrap_or(tail.len()) };
    if tail[end..].starts_with('.') || end == 0 {
        return rest.to_string();
    }
    format!("{}\"{}\".{}", &rest[..name_at], schema.replace('"', "\"\""), tail)
}
