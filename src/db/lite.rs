//! SQLite files (rusqlite, the library built in).

use std::sync::Arc;

use rusqlite::{Connection, OpenFlags, types::ValueRef};

use super::*;

pub struct Lite {
    conn: Connection,
    /// The last statement only read.
    read_only: bool,
    /// The open transaction changed something.
    dirty: bool,
}

fn fail(e: rusqlite::Error) -> Failure {
    let at = match &e {
        rusqlite::Error::SqlInputError { offset, .. } if *offset >= 0 => Some(*offset as usize),
        _ => None,
    };
    Failure { message: e.to_string(), at, lost: false }
}

fn text(v: ValueRef) -> Option<String> {
    match v {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i.to_string()),
        ValueRef::Real(f) => Some(fmt_float(f)),
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Some(format!("<{} bytes>", b.len())),
    }
}

impl Lite {
    pub fn open(spec: &Spec) -> Res<Self> {
        let path = crate::config::expand(&spec.database);
        let flags = if spec.read_only { OpenFlags::SQLITE_OPEN_READ_ONLY } else { OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE } | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(fail)?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).ok();
        Ok(Self { conn, read_only: true, dirty: false })
    }

    fn rows(&self, sql: &str, params: &[&str]) -> Res<Vec<Vec<String>>> {
        let mut st = self.conn.prepare(sql).map_err(fail)?;
        let n = st.column_count();
        let mut rows = st.query(rusqlite::params_from_iter(params.iter())).map_err(fail)?;
        let mut out = Vec::new();
        while let Some(r) = rows.next().map_err(fail)? {
            out.push((0..n).map(|i| r.get_ref(i).ok().and_then(text).unwrap_or_default()).collect());
        }
        Ok(out)
    }
}

impl Lite {
    /// Objects of sqlite_master matching `cond` (?1: what's looked for).
    fn master_hits(&self, cond: &str, q: &str) -> Res<Vec<Found>> {
        Ok(self
            .rows(&format!("SELECT name, type FROM sqlite_master WHERE {cond} AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 500"), &[q])?
            .into_iter()
            .filter_map(|r| {
                let (folder, kind) = match r[1].as_str() {
                    "table" => ("tables", ObjKind::Table),
                    "view" => ("views", ObjKind::View),
                    "index" => ("indexes", ObjKind::Index),
                    "trigger" => ("triggers", ObjKind::Trigger),
                    _ => return None,
                };
                Some(Found { path: vec![folder.into(), r[0].clone()], kind, schema: String::new(), name: r[0].clone(), lines: Vec::new() })
            })
            .collect())
    }
}

impl Session for Lite {
    fn run(&mut self, sql: &str, fetch: &mut Fetch, _notes: &mut dyn FnMut(String)) -> Res<Outcome> {
        let mut st = self.conn.prepare(sql).map_err(fail)?;
        self.read_only = st.readonly();
        let n = st.column_count();
        if n == 0 {
            let changed = st.execute([]).map_err(fail)?;
            let w = crate::sql::first_word(sql).to_lowercase();
            return Ok(Outcome::Done(matches!(w.as_str(), "insert" | "update" | "delete" | "replace").then_some(changed as u64)));
        }
        let cols: Vec<Col> = st.columns().iter().map(|c| Col { name: c.name().to_string(), ty: c.decl_type().unwrap_or("").to_string(), numeric: false }).collect();
        let mut t = Table { cols, ..Default::default() };
        let mut rows = st.query([]).map_err(fail)?;
        while let Some(r) = rows.next().map_err(fail)? {
            if !fetch.room(&mut t) {
                break;
            }
            t.rows.push((0..n).map(|i| r.get_ref(i).ok().and_then(text)).collect());
        }
        for c in t.cols.iter_mut() {
            c.numeric = is_numeric_type(&c.ty);
        }
        t.guess_numeric();
        Ok(Outcome::Rows(t))
    }

    fn children(&mut self, path: &[String]) -> Res<Vec<Obj>> {
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        let master = |this: &Self, ty: &str, kind: ObjKind, leaf: bool| -> Res<Vec<Obj>> {
            Ok(this.rows("SELECT name, coalesce(tbl_name, '') FROM sqlite_master WHERE type = ?1 AND name NOT LIKE 'sqlite_%' ORDER BY name", &[ty])?.into_iter().map(|r| obj(r[0].clone(), kind, if ty == "table" { String::new() } else { r[1].clone() }, leaf)).collect())
        };
        Ok(match p.as_slice() {
            [] => ["tables", "views", "indexes", "triggers"].iter().map(|f| folder(f)).collect(),
            ["tables"] => master(self, "table", ObjKind::Table, false)?,
            ["views"] => master(self, "view", ObjKind::View, false)?,
            ["indexes"] => master(self, "index", ObjKind::Index, true)?,
            ["triggers"] => master(self, "trigger", ObjKind::Trigger, true)?,
            [_, t] => self
                .rows(&format!("SELECT name, type || CASE WHEN \"notnull\" THEN ' not null' ELSE '' END || CASE WHEN pk > 0 THEN ' · PK' ELSE '' END FROM pragma_table_info(?1)"), &[t])?
                .into_iter()
                .map(|r| obj(r[0].clone(), ObjKind::Column, r[1].clone(), true))
                .collect(),
            _ => Vec::new(),
        })
    }

    fn source(&mut self, path: &[String], _kind: ObjKind) -> Res<String> {
        let name = path.last().ok_or_else(|| Failure::new("nothing to show"))?;
        let r = self.rows("SELECT sql FROM sqlite_master WHERE name = ?1", &[name])?;
        Ok(r.into_iter().next().and_then(|r| r.into_iter().next()).map(|s| s + ";\n").unwrap_or_default())
    }

    fn find_objects(&mut self, q: &str) -> Res<Vec<Found>> {
        self.master_hits("instr(lower(name), lower(?1)) > 0", q)
    }

    fn search_code(&mut self, q: &str) -> Res<Vec<Found>> {
        let mut v = self.master_hits("type IN ('view', 'trigger') AND instr(lower(sql), lower(?1)) > 0", q)?;
        for f in v.iter_mut() {
            let sql = self.rows("SELECT sql FROM sqlite_master WHERE name = ?1", &[&f.name])?.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or_default();
            f.lines = lines_with(&sql, q, 3);
        }
        Ok(v)
    }

    fn locate(&mut self, _schema: Option<&str>, name: &str, _call: bool) -> Res<Option<(Vec<String>, ObjKind)>> {
        let r = self.rows("SELECT name, type FROM sqlite_master WHERE name = ?1 COLLATE NOCASE AND name NOT LIKE 'sqlite_%' LIMIT 1", &[name])?;
        Ok(r.into_iter().next().and_then(|r| {
            let (folder, kind) = match r[1].as_str() {
                "table" => ("tables", ObjKind::Table),
                "view" => ("views", ObjKind::View),
                "index" => ("indexes", ObjKind::Index),
                "trigger" => ("triggers", ObjKind::Trigger),
                _ => return None,
            };
            Some((vec![folder.to_string(), r[0].clone()], kind))
        }))
    }

    fn describe(&mut self, path: &[String], kind: ObjKind) -> Res<crate::design::ObjDef> {
        use crate::design::{Column, ObjDef, TableDef};
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        let (["tables", t], ObjKind::Table) = (p.as_slice(), kind) else {
            return Err(Failure::new(crate::i18n::t("Конструктор для этого объекта здесь не поддерживается", "No designer for this object here")));
        };
        let cols = self
            .rows("SELECT name, type, CASE WHEN \"notnull\" THEN 'N' ELSE 'Y' END, coalesce(dflt_value, ''), CASE WHEN pk > 0 THEN 'Y' ELSE 'N' END FROM pragma_table_info(?1) ORDER BY cid", &[t])?
            .into_iter()
            .map(|r| Column { name: r[0].clone(), ty: r[1].clone(), nullable: r[2] == "Y", default: r[3].clone(), comment: String::new(), pk: r[4] == "Y" })
            .collect();
        let info = self.rows("SELECT name, CASE WHEN \"unique\" THEN 'UNIQUE' ELSE '' END || ' INDEX' FROM pragma_index_list(?1) ORDER BY 1", &[t])?.into_iter().map(|r| (r[0].clone(), r[1].trim().to_string())).collect();
        Ok(ObjDef::Table(TableDef { schema: String::new(), name: t.to_string(), cols, comment: String::new(), info }))
    }

    fn relations(&mut self, _schemas: &[String]) -> Res<Vec<Relation>> {
        let rows = self.rows(
            "SELECT 'main', m.name, p.name, p.type FROM sqlite_master m, pragma_table_info(m.name) p WHERE m.type IN ('table', 'view') AND m.name NOT LIKE 'sqlite_%' ORDER BY m.name, p.cid",
            &[],
        )?;
        Ok(super::pg::group_relations(rows))
    }

    fn canceller(&self) -> Canceller {
        let h = self.conn.get_interrupt_handle();
        Arc::new(move || h.interrupt())
    }

    fn version(&mut self) -> String {
        format!("SQLite {}", rusqlite::version())
    }

    fn tx_before(&mut self, sql: &str) -> Res<()> {
        let plain = tx_stmt(sql) == TxStmt::Other && !matches!(verb(sql).as_str(), "vacuum" | "attach" | "detach" | "pragma");
        if plain && self.conn.is_autocommit() {
            self.conn.execute_batch("BEGIN").map_err(fail)?;
        }
        Ok(())
    }

    fn tx_after(&mut self, sql: &str, ok: bool) {
        if tx_stmt(sql) == TxStmt::End {
            self.dirty = false;
        }
        // Changed something, or a BEGIN of the script's own: kept open.
        if ok && (!self.read_only && tx_stmt(sql) == TxStmt::Other || tx_stmt(sql) == TxStmt::Begin) {
            self.dirty = true;
        }
        // Only read: the transaction ends (no lock held on the file).
        if !self.dirty && !self.conn.is_autocommit() {
            self.conn.execute_batch("COMMIT").ok();
        }
    }

    fn set_auto(&mut self, on: bool) {
        if on && !self.conn.is_autocommit() {
            self.conn.execute_batch("COMMIT").ok();
        }
    }

    fn finish(&mut self, commit: bool) -> Res<()> {
        if !self.conn.is_autocommit() {
            self.conn.execute_batch(if commit { "COMMIT" } else { "ROLLBACK" }).map_err(fail)?;
        }
        self.dirty = false;
        Ok(())
    }

    fn pending(&mut self) -> bool {
        !self.conn.is_autocommit() && self.dirty
    }
}

