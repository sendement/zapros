//! Objects as the designers show them (a table's columns, a job), and the
//! SQL that makes what was changed by hand — per database.

use crate::db::Kind;
use crate::sql::Dialect;
use crate::sql::complete::quote;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Column {
    pub name: String,
    pub ty: String,
    pub nullable: bool,
    /// The default's expression, as written in SQL ("" — none).
    pub default: String,
    pub comment: String,
    pub pk: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableDef {
    /// Schema / database ("" in SQLite).
    pub schema: String,
    pub name: String,
    pub cols: Vec<Column>,
    pub comment: String,
    /// Other facts shown as they are (engine, keys…).
    pub info: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobDef {
    /// pg_cron's jobid ("" — a new one).
    pub id: String,
    /// Oracle: the schema it's in.
    pub owner: String,
    pub name: String,
    /// cron (pg_cron) or the calendar string (Oracle).
    pub schedule: String,
    /// What it runs: SQL (pg_cron), a PL/SQL block or a procedure's name.
    pub action: String,
    /// Oracle's job_type (PLSQL_BLOCK, STORED_PROCEDURE…).
    pub action_type: String,
    pub enabled: bool,
    pub comment: String,
    /// pg_cron: the database it runs in.
    pub database: String,
    pub info: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ObjDef {
    Table(TableDef),
    Job(JobDef),
}

/// A column as edited: where it came from (its index in the table as
/// loaded; None — added), what it is now, dropped or not.
#[derive(Clone, Debug, PartialEq)]
pub struct ColEdit {
    pub from: Option<usize>,
    pub col: Column,
    pub dropped: bool,
}

pub fn edits_of(t: &TableDef) -> Vec<ColEdit> {
    t.cols.iter().enumerate().map(|(i, c)| ColEdit { from: Some(i), col: c.clone(), dropped: false }).collect()
}

fn str_lit(kind: Kind, s: &str) -> String {
    match kind {
        Kind::ClickHouse => crate::db::ch::literal(s),
        _ => format!("'{}'", s.replace('\'', "''")),
    }
}

pub fn table_name(kind: Kind, t: &TableDef) -> String {
    let d = kind.dialect();
    if t.schema.is_empty() || kind == Kind::Sqlite { quote(&t.name, d) } else { format!("{}.{}", quote(&t.schema, d), quote(&t.name, d)) }
}

/// ClickHouse: the type with Nullable(…) put on or taken off.
fn ch_nullable(ty: &str, on: bool) -> String {
    let inner = ty.strip_prefix("Nullable(").and_then(|s| s.strip_suffix(')')).unwrap_or(ty);
    if on { format!("Nullable({inner})") } else { inner.to_string() }
}

/// A column as written in ADD COLUMN.
fn col_spec(kind: Kind, c: &Column) -> String {
    let d = kind.dialect();
    let name = quote(&c.name, d);
    let def = if c.default.trim().is_empty() { String::new() } else { format!(" DEFAULT {}", c.default.trim()) };
    match kind {
        Kind::ClickHouse => {
            let com = if c.comment.is_empty() { String::new() } else { format!(" COMMENT {}", str_lit(kind, &c.comment)) };
            format!("{name} {}{def}{com}", ch_nullable(&c.ty, c.nullable))
        }
        // Oracle: DEFAULT before NOT NULL.
        _ => format!("{name} {}{def}{}", c.ty.trim(), if c.nullable { "" } else { " NOT NULL" }),
    }
}

/// The statements that make the table what was edited (in order: drops,
/// renames, changes, adds, comments). Each without its ";".
pub fn table_changes(kind: Kind, t: &TableDef, rows: &[ColEdit], comment: &str) -> Vec<String> {
    let d = kind.dialect();
    let tn = table_name(kind, t);
    let mut out = Vec::new();
    let mut comments = Vec::new();
    let col_comment = |c: &Column, out: &mut Vec<String>| match kind {
        Kind::Postgres | Kind::Oracle => out.push(format!("COMMENT ON COLUMN {tn}.{} IS {}", quote(&c.name, d), str_lit(kind, &c.comment))),
        Kind::ClickHouse => out.push(format!("ALTER TABLE {tn} COMMENT COLUMN {} {}", quote(&c.name, d), str_lit(kind, &c.comment))),
        Kind::Sqlite => {}
    };
    for r in rows.iter().filter(|r| r.dropped && r.from.is_some()) {
        let o = &t.cols[r.from.unwrap()];
        out.push(format!("ALTER TABLE {tn} DROP COLUMN {}", quote(&o.name, d)));
    }
    for r in rows.iter().filter(|r| !r.dropped) {
        let Some(i) = r.from else { continue };
        let o = &t.cols[i];
        let c = &r.col;
        let name = quote(&c.name, d);
        if c.name != o.name && !c.name.trim().is_empty() {
            out.push(format!("ALTER TABLE {tn} RENAME COLUMN {} TO {name}", quote(&o.name, d)));
        }
        let ty_changed = c.ty.trim() != o.ty.trim();
        let null_changed = c.nullable != o.nullable;
        let def_changed = c.default.trim() != o.default.trim();
        match kind {
            Kind::Postgres => {
                if ty_changed {
                    out.push(format!("ALTER TABLE {tn} ALTER COLUMN {name} TYPE {}", c.ty.trim()));
                }
                if null_changed {
                    out.push(format!("ALTER TABLE {tn} ALTER COLUMN {name} {} NOT NULL", if c.nullable { "DROP" } else { "SET" }));
                }
                if def_changed {
                    out.push(if c.default.trim().is_empty() { format!("ALTER TABLE {tn} ALTER COLUMN {name} DROP DEFAULT") } else { format!("ALTER TABLE {tn} ALTER COLUMN {name} SET DEFAULT {}", c.default.trim()) });
                }
            }
            Kind::Oracle => {
                let mut m = Vec::new();
                if ty_changed {
                    m.push(c.ty.trim().to_string());
                }
                if def_changed {
                    m.push(format!("DEFAULT {}", if c.default.trim().is_empty() { "NULL" } else { c.default.trim() }));
                }
                if null_changed {
                    m.push(if c.nullable { "NULL".into() } else { "NOT NULL".into() });
                }
                if !m.is_empty() {
                    out.push(format!("ALTER TABLE {tn} MODIFY ({name} {})", m.join(" ")));
                }
            }
            Kind::ClickHouse => {
                if ty_changed || null_changed {
                    out.push(format!("ALTER TABLE {tn} MODIFY COLUMN {name} {}", ch_nullable(&c.ty, c.nullable)));
                }
                if def_changed {
                    out.push(if c.default.trim().is_empty() { format!("ALTER TABLE {tn} MODIFY COLUMN {name} REMOVE DEFAULT") } else { format!("ALTER TABLE {tn} MODIFY COLUMN {name} DEFAULT {}", c.default.trim()) });
                }
            }
            Kind::Sqlite => {
                if ty_changed || null_changed || def_changed {
                    out.push(format!("-- SQLite: {name} — {}", crate::i18n::t("тип, NULL и значение по умолчанию меняются только пересозданием таблицы", "type, NULL and default change only by making the table anew")));
                }
            }
        }
        if c.comment != o.comment {
            col_comment(c, &mut comments);
        }
    }
    for r in rows.iter().filter(|r| !r.dropped && r.from.is_none() && !r.col.name.trim().is_empty() && !r.col.ty.trim().is_empty()) {
        let c = &r.col;
        out.push(match kind {
            Kind::Oracle => format!("ALTER TABLE {tn} ADD ({})", col_spec(kind, c)),
            _ => format!("ALTER TABLE {tn} ADD COLUMN {}", col_spec(kind, c)),
        });
        if !c.comment.is_empty() && kind != Kind::ClickHouse {
            col_comment(c, &mut comments);
        }
    }
    out.extend(comments);
    if comment != t.comment {
        match kind {
            Kind::Postgres | Kind::Oracle => out.push(format!("COMMENT ON TABLE {tn} IS {}", str_lit(kind, comment))),
            Kind::ClickHouse => out.push(format!("ALTER TABLE {tn} MODIFY COMMENT {}", str_lit(kind, comment))),
            Kind::Sqlite => {}
        }
    }
    out
}

/// The statements as a script.
pub fn script(kind: Kind, stmts: &[String]) -> String {
    let _ = kind;
    stmts.iter().map(|s| if s.starts_with("--") { format!("{s}\n") } else { format!("{s};\n") }).collect()
}

/// pg_cron's dollar quotes: a tag the text doesn't hold.
fn dollar(s: &str) -> String {
    let mut tag = "$job$".to_string();
    let mut n = 0;
    while s.contains(&tag) {
        n += 1;
        tag = format!("$job{n}$");
    }
    format!("{tag}\n{}\n{tag}", s.trim())
}

fn ora_name(j: &JobDef) -> String {
    let d = Dialect::Oracle;
    if j.owner.is_empty() { quote(&j.name, d) } else { format!("{}.{}", quote(&j.owner, d), quote(&j.name, d)) }
}

/// The script that makes the job (`orig` None) or changes it to `cur`.
pub fn job_sql(kind: Kind, orig: Option<&JobDef>, cur: &JobDef) -> String {
    let lit = |s: &str| str_lit(kind, s);
    match kind {
        Kind::Postgres => match orig {
            None => {
                let mut s = format!("SELECT cron.schedule({}, {}, {})", lit(&cur.name), lit(&cur.schedule), dollar(&cur.action));
                if !cur.database.is_empty() {
                    s = format!("SELECT cron.schedule_in_database({}, {}, {}, {})", lit(&cur.name), lit(&cur.schedule), dollar(&cur.action), lit(&cur.database));
                }
                if !cur.enabled {
                    s += &format!(";\nSELECT cron.alter_job(jobid, active := false) FROM cron.job WHERE jobname = {}", lit(&cur.name));
                }
                s + ";\n"
            }
            Some(o) => {
                let mut args = Vec::new();
                if cur.schedule != o.schedule {
                    args.push(format!("schedule := {}", lit(&cur.schedule)));
                }
                if cur.action.trim() != o.action.trim() {
                    args.push(format!("command := {}", dollar(&cur.action)));
                }
                if cur.database != o.database {
                    args.push(format!("database := {}", lit(&cur.database)));
                }
                if cur.enabled != o.enabled {
                    args.push(format!("active := {}", cur.enabled));
                }
                if args.is_empty() {
                    return String::new();
                }
                format!("SELECT cron.alter_job(job_id := {}, {});\n", o.id, args.join(", "))
            }
        },
        Kind::Oracle => {
            let name = lit(&ora_name(cur));
            let mut body = Vec::new();
            match orig {
                None => {
                    let mut a = vec![
                        format!("job_name        => {name}"),
                        format!("job_type        => {}", lit(if cur.action_type.is_empty() { "PLSQL_BLOCK" } else { &cur.action_type })),
                        format!("job_action      => {}", lit(cur.action.trim())),
                    ];
                    if !cur.schedule.trim().is_empty() {
                        a.push(format!("repeat_interval => {}", lit(cur.schedule.trim())));
                    }
                    a.push(format!("enabled         => {}", if cur.enabled { "TRUE" } else { "FALSE" }));
                    if !cur.comment.is_empty() {
                        a.push(format!("comments        => {}", lit(&cur.comment)));
                    }
                    body.push(format!("  DBMS_SCHEDULER.CREATE_JOB(\n    {});", a.join(",\n    ")));
                }
                Some(o) => {
                    let set = |attr: &str, v: &str| format!("  DBMS_SCHEDULER.SET_ATTRIBUTE({name}, '{attr}', {});", lit(v));
                    if cur.schedule.trim() != o.schedule.trim() {
                        body.push(if cur.schedule.trim().is_empty() { format!("  DBMS_SCHEDULER.SET_ATTRIBUTE_NULL({name}, 'repeat_interval');") } else { set("repeat_interval", cur.schedule.trim()) });
                    }
                    if cur.action_type != o.action_type {
                        body.push(set("job_type", &cur.action_type));
                    }
                    if cur.action.trim() != o.action.trim() {
                        body.push(set("job_action", cur.action.trim()));
                    }
                    if cur.comment != o.comment {
                        body.push(set("comments", &cur.comment));
                    }
                    if cur.enabled != o.enabled {
                        body.push(format!("  DBMS_SCHEDULER.{}({name});", if cur.enabled { "ENABLE" } else { "DISABLE" }));
                    }
                }
            }
            if body.is_empty() {
                return String::new();
            }
            format!("BEGIN\n{}\nEND;\n/\n", body.join("\n"))
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, nullable: bool) -> Column {
        Column { name: name.into(), ty: ty.into(), nullable, ..Default::default() }
    }

    fn table() -> TableDef {
        TableDef { schema: "shop".into(), name: "orders".into(), cols: vec![col("id", "integer", false), col("note", "text", true), col("total", "numeric(10,2)", true)], ..Default::default() }
    }

    #[test]
    fn table_changes_per_database() {
        let t = table();
        let mut rows = edits_of(&t);
        rows[1].col.name = "remark".into();
        rows[1].col.comment = "free text".into();
        rows[2].col.nullable = false;
        rows[2].col.default = "0".into();
        rows.push(ColEdit { from: None, col: Column { default: "now()".into(), ..col("created", "timestamp", false) }, dropped: false });
        let pg = table_changes(Kind::Postgres, &t, &rows, "Orders");
        assert_eq!(
            pg,
            vec![
                "ALTER TABLE shop.orders RENAME COLUMN note TO remark",
                "ALTER TABLE shop.orders ALTER COLUMN total SET NOT NULL",
                "ALTER TABLE shop.orders ALTER COLUMN total SET DEFAULT 0",
                "ALTER TABLE shop.orders ADD COLUMN created timestamp DEFAULT now() NOT NULL",
                "COMMENT ON COLUMN shop.orders.remark IS 'free text'",
                "COMMENT ON TABLE shop.orders IS 'Orders'",
            ]
        );
        let ora = table_changes(Kind::Oracle, &t, &rows, "");
        // (Oracle: names in lower case are quoted to stay so.)
        assert!(ora.contains(&"ALTER TABLE \"shop\".\"orders\" MODIFY (\"total\" DEFAULT 0 NOT NULL)".to_string()), "{ora:?}");
        assert!(ora.contains(&"ALTER TABLE \"shop\".\"orders\" ADD (\"created\" timestamp DEFAULT now() NOT NULL)".to_string()), "{ora:?}");
        let cht = TableDef { cols: vec![col("id", "UInt64", false), col("note", "Nullable(String)", true), col("total", "Float64", false)], ..t.clone() };
        let mut rows = edits_of(&cht);
        rows[0].dropped = true;
        rows[1].col.nullable = false;
        let ch = table_changes(Kind::ClickHouse, &cht, &rows, "");
        assert_eq!(ch, vec!["ALTER TABLE shop.orders DROP COLUMN id", "ALTER TABLE shop.orders MODIFY COLUMN note String"]);
        // Nothing changed: nothing to do.
        assert!(table_changes(Kind::Postgres, &t, &edits_of(&t), "").is_empty());
    }

    #[test]
    fn job_scripts() {
        let o = JobDef { id: "7".into(), name: "nightly".into(), schedule: "0 3 * * *".into(), action: "vacuum".into(), enabled: true, ..Default::default() };
        let mut c = o.clone();
        c.schedule = "30 4 * * 1".into();
        c.enabled = false;
        assert_eq!(job_sql(Kind::Postgres, Some(&o), &c), "SELECT cron.alter_job(job_id := 7, schedule := '30 4 * * 1', active := false);\n");
        assert!(job_sql(Kind::Postgres, None, &o).starts_with("SELECT cron.schedule('nightly', '0 3 * * *', $job$\nvacuum\n$job$)"));
        let o = JobDef { owner: "SHOP".into(), name: "REFRESH_JOB".into(), schedule: "FREQ=HOURLY".into(), action: "SHOP_PKG.REFRESH".into(), action_type: "STORED_PROCEDURE".into(), enabled: true, ..Default::default() };
        let mut c = o.clone();
        c.schedule = "FREQ=DAILY;BYHOUR=3;BYMINUTE=0;BYSECOND=0".into();
        let s = job_sql(Kind::Oracle, Some(&o), &c);
        assert_eq!(s, "BEGIN\n  DBMS_SCHEDULER.SET_ATTRIBUTE('SHOP.REFRESH_JOB', 'repeat_interval', 'FREQ=DAILY;BYHOUR=3;BYMINUTE=0;BYSECOND=0');\nEND;\n/\n");
        assert!(job_sql(Kind::Oracle, None, &c).contains("DBMS_SCHEDULER.CREATE_JOB(\n    job_name        => 'SHOP.REFRESH_JOB'"));
        assert_eq!(job_sql(Kind::Oracle, Some(&o), &o), "");
    }
}
