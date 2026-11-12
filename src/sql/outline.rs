//! What a script is made of, for the structure panel: its statements and
//! the objects they create; inside a package (or type body), its functions,
//! procedures, types, cursors, exceptions, constants and variables.
//! Also: a package's specification and body told apart.

use std::ops::Range;

use super::lex::{self, Tk, Tok};
use super::{Dialect, split};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ent {
    Package,
    PackageBody,
    Function,
    Procedure,
    Type,
    TypeBody,
    Subtype,
    Cursor,
    Exception,
    Constant,
    Variable,
    Init,
    Table,
    View,
    MatView,
    Index,
    Sequence,
    Trigger,
    Synonym,
    Block,
    Statement,
}

impl Ent {
    pub fn icon(self) -> &'static str {
        match self {
            Ent::Package | Ent::PackageBody => "📦",
            Ent::Function => "ƒ",
            Ent::Procedure => "⚙",
            Ent::Type | Ent::TypeBody | Ent::Subtype => "T",
            Ent::Cursor => "⊙",
            Ent::Exception => "⚠",
            Ent::Constant => "π",
            Ent::Variable => "x",
            Ent::Init => "▶",
            Ent::Table => "▦",
            Ent::View => "👁",
            Ent::MatView => "◈",
            Ent::Index => "⊞",
            Ent::Sequence => "#",
            Ent::Trigger => "⚡",
            Ent::Synonym => "↪",
            Ent::Block => "◇",
            Ent::Statement => "▸",
        }
    }

    pub fn name(self) -> &'static str {
        use crate::i18n::t;
        match self {
            Ent::Package => t("пакет", "package"),
            Ent::PackageBody => t("тело пакета", "package body"),
            Ent::Function => t("функция", "function"),
            Ent::Procedure => t("процедура", "procedure"),
            Ent::Type => t("тип", "type"),
            Ent::TypeBody => t("тело типа", "type body"),
            Ent::Subtype => t("подтип", "subtype"),
            Ent::Cursor => t("курсор", "cursor"),
            Ent::Exception => t("исключение", "exception"),
            Ent::Constant => t("константа", "constant"),
            Ent::Variable => t("переменная", "variable"),
            Ent::Init => t("инициализация", "initialization"),
            Ent::Table => t("таблица", "table"),
            Ent::View => t("представление", "view"),
            Ent::MatView => t("мат. представление", "materialized view"),
            Ent::Index => t("индекс", "index"),
            Ent::Sequence => t("последовательность", "sequence"),
            Ent::Trigger => t("триггер", "trigger"),
            Ent::Synonym => t("синоним", "synonym"),
            Ent::Block => t("блок", "block"),
            Ent::Statement => t("запрос", "statement"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub kind: Ent,
    pub name: String,
    /// Its signature or what it is (a type, a table…).
    pub detail: String,
    /// Where it is (bytes): the whole of it, and its name.
    pub range: Range<usize>,
    pub at: usize,
    /// 0: a statement; 1: inside a package.
    pub depth: u8,
    /// Declared only (a package spec's function).
    pub decl: bool,
}

fn word(src: &str, t: &Tok) -> String {
    t.text(src).to_ascii_lowercase()
}

/// A name as written: "Quoted" kept, else as is; a.b joined.
fn name_at(src: &str, nt: &[&Tok], mut k: usize) -> (String, usize, usize) {
    let mut s = String::new();
    let at = nt.get(k).map_or(0, |t| t.start);
    while let Some(t) = nt.get(k) {
        if !matches!(t.kind, Tk::Word | Tk::Keyword | Tk::Quoted) {
            break;
        }
        s += t.text(src);
        if nt.get(k + 1).is_some_and(|x| x.kind == Tk::Dot) {
            s.push('.');
            k += 2;
        } else {
            k += 1;
            break;
        }
    }
    (s, at, k)
}

/// The text from `a` to `b` on one line, cut short.
fn squeeze(src: &str, a: usize, b: usize, max: usize) -> String {
    let one: String = src[a..b].split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max { format!("{}…", one.chars().take(max).collect::<String>()) } else { one }
}

/// What a CREATE makes, from the words after CREATE [OR REPLACE …].
fn created(w: &[String]) -> Option<(Ent, usize)> {
    let mut i = 0;
    while matches!(w.get(i).map(String::as_str), Some("or" | "replace" | "editionable" | "noneditionable" | "force" | "noforce" | "unique" | "temporary" | "temp" | "global" | "private" | "public" | "unlogged" | "if" | "not" | "exists" | "and" | "compile" | "resolve" | "noresolve" | "java" | "source")) {
        i += 1;
    }
    let one = |e: Ent| Some((e, i + 1));
    match w.get(i).map(String::as_str)? {
        "package" if w.get(i + 1).is_some_and(|x| x == "body") => Some((Ent::PackageBody, i + 2)),
        "package" => one(Ent::Package),
        "type" if w.get(i + 1).is_some_and(|x| x == "body") => Some((Ent::TypeBody, i + 2)),
        "type" => one(Ent::Type),
        "function" => one(Ent::Function),
        "procedure" => one(Ent::Procedure),
        "table" => one(Ent::Table),
        "view" => one(Ent::View),
        "materialized" => Some((Ent::MatView, i + 2)),
        "index" => one(Ent::Index),
        "sequence" => one(Ent::Sequence),
        "trigger" => one(Ent::Trigger),
        "synonym" => one(Ent::Synonym),
        "dictionary" => one(Ent::Table),
        _ => None,
    }
}

/// The script's statements, and inside packages what they hold.
pub fn outline(src: &str, d: Dialect) -> Vec<Entity> {
    let mut out = Vec::new();
    for r in split(src, d, false) {
        let toks = lex::lex(&src[r.clone()], d);
        let nt: Vec<Tok> = toks.iter().filter(|t| !t.trivia()).map(|t| Tok { start: t.start + r.start, end: t.end + r.start, ..*t }).collect();
        let nt: Vec<&Tok> = nt.iter().collect();
        if nt.is_empty() {
            continue;
        }
        let words: Vec<String> = nt.iter().take(12).map(|t| word(src, t)).collect();
        let first = words[0].as_str();
        if first == "create" || first == "alter" && words.get(1).is_some_and(|w| w == "package") {
            if let Some((kind, k)) = created(&words[1..]) {
                let mut k = k + 1;
                // IF NOT EXISTS after the kind (PostgreSQL).
                if words.get(k).is_some_and(|w| w == "if") {
                    k += 3;
                }
                let (name, at, after) = name_at(src, &nt, k);
                let detail = match kind {
                    Ent::Function | Ent::Procedure => signature(src, &nt, after),
                    _ => String::new(),
                };
                out.push(Entity { kind, name, detail, range: r.clone(), at, depth: 0, decl: false });
                if matches!(kind, Ent::Package | Ent::PackageBody | Ent::TypeBody) {
                    members(src, &nt[after..], kind != Ent::Package, &mut out);
                }
                continue;
            }
        }
        let (kind, name) = match first {
            "begin" | "declare" | "do" => (Ent::Block, String::new()),
            "select" | "with" => (Ent::Statement, after_word(src, &nt, "from").map(|t| format!("SELECT … FROM {t}")).unwrap_or_else(|| "SELECT".into())),
            "insert" | "merge" => (Ent::Statement, after_word(src, &nt, "into").map(|t| format!("{} INTO {t}", first.to_uppercase())).unwrap_or_else(|| first.to_uppercase())),
            "update" => (Ent::Statement, format!("UPDATE {}", name_at(src, &nt, 1).0)),
            "delete" => (Ent::Statement, after_word(src, &nt, "from").map(|t| format!("DELETE FROM {t}")).unwrap_or_else(|| "DELETE".into())),
            _ => (Ent::Statement, squeeze(src, nt[0].start, nt[nt.len().min(4) - 1].end, 40).to_uppercase()),
        };
        let detail = squeeze(src, r.start, r.end, 60);
        out.push(Entity { kind, name, detail, range: r.clone(), at: nt[0].start, depth: 0, decl: false });
    }
    out
}

fn after_word(src: &str, nt: &[&Tok], w: &str) -> Option<String> {
    let k = nt.iter().position(|t| t.is_word(src, w))?;
    let (n, _, _) = name_at(src, nt, k + 1);
    (!n.is_empty()).then_some(n)
}

/// (args) RETURN type, up to IS / AS / ; / $$.
fn signature(src: &str, nt: &[&Tok], from: usize) -> String {
    let mut depth = 0;
    let mut end = from;
    for (j, t) in nt.iter().enumerate().skip(from) {
        match t.kind {
            Tk::Open => depth += 1,
            Tk::Close => depth -= 1,
            Tk::Semi | Tk::Str if depth == 0 => break,
            _ if depth == 0 && matches!(word(src, t).as_str(), "is" | "as" | "language" | "deterministic" | "pipelined" | "parallel_enable" | "result_cache" | "authid") => break,
            _ => {}
        }
        end = j + 1;
    }
    if end <= from {
        return String::new();
    }
    squeeze(src, nt[from].start, nt[end - 1].end, 90)
}

/// A package's (or type body's) members, from after its name. `body`:
/// subprograms have bodies to step over.
fn members(src: &str, nt: &[&Tok], body: bool, out: &mut Vec<Entity>) {
    // Past IS / AS.
    let Some(mut k) = nt.iter().position(|t| matches!(word(src, t).as_str(), "is" | "as")).map(|k| k + 1) else { return };
    let w = |k: usize| nt.get(k).map(|t| word(src, t)).unwrap_or_default();
    // To the ; ending a declaration (brackets aside).
    let to_semi = |mut k: usize| {
        let mut depth = 0;
        while let Some(t) = nt.get(k) {
            match t.kind {
                Tk::Open => depth += 1,
                Tk::Close => depth -= 1,
                Tk::Semi if depth == 0 => return k,
                _ => {}
            }
            k += 1;
        }
        k
    };
    while k < nt.len() {
        let here = w(k);
        match here.as_str() {
            "end" => break,
            "begin" => {
                out.push(Entity { kind: Ent::Init, name: crate::i18n::t("инициализация", "initialization").into(), detail: String::new(), range: nt[k].start..nt.last().unwrap().end, at: nt[k].start, depth: 1, decl: false });
                break;
            }
            "function" | "procedure" | "member" | "static" | "constructor" | "map" | "order" | "overriding" | "final" | "instantiable" | "not" => {
                // (Type body methods: MEMBER FUNCTION …)
                let mut j = k;
                while !matches!(w(j).as_str(), "function" | "procedure" | "") {
                    j += 1;
                }
                let kind = if w(j) == "function" { Ent::Function } else { Ent::Procedure };
                let (name, at, after) = name_at(src, nt, j + 1);
                let detail = signature(src, nt, after);
                // A declaration (;) or a body (IS / AS … END name;).
                let mut e = after;
                let mut depth = 0;
                let mut is_body = false;
                while let Some(t) = nt.get(e) {
                    match t.kind {
                        Tk::Open => depth += 1,
                        Tk::Close => depth -= 1,
                        Tk::Semi if depth == 0 => break,
                        _ if depth == 0 && matches!(word(src, t).as_str(), "is" | "as") => {
                            is_body = body;
                            break;
                        }
                        _ => {}
                    }
                    e += 1;
                }
                let end = if is_body { skip_body(src, nt, e + 1) } else { e };
                let end_at = nt.get(end).map_or(nt.last().unwrap().end, |t| t.end);
                out.push(Entity { kind, name, detail, range: nt[k].start..end_at, at, depth: 1, decl: !is_body });
                k = end + 1;
            }
            "type" | "subtype" => {
                let (name, at, after) = name_at(src, nt, k + 1);
                let e = to_semi(after);
                let detail = squeeze(src, nt.get(after).map_or(at, |t| t.start), nt.get(e).map_or(at, |t| t.start), 60);
                let kind = if here == "type" { Ent::Type } else { Ent::Subtype };
                out.push(Entity { kind, name, detail, range: nt[k].start..nt.get(e).map_or(src.len(), |t| t.end), at, depth: 1, decl: true });
                k = e + 1;
            }
            "cursor" => {
                let (name, at, after) = name_at(src, nt, k + 1);
                let e = to_semi(after);
                let detail = squeeze(src, nt.get(after).map_or(at, |t| t.start), nt.get(e).map_or(at, |t| t.start), 60);
                out.push(Entity { kind: Ent::Cursor, name, detail, range: nt[k].start..nt.get(e).map_or(src.len(), |t| t.end), at, depth: 1, decl: true });
                k = e + 1;
            }
            "pragma" => k = to_semi(k) + 1,
            _ => {
                let e = to_semi(k);
                let t = nt[k];
                if matches!(t.kind, Tk::Word | Tk::Quoted | Tk::Keyword) && e > k + 1 {
                    let next = w(k + 1);
                    let (kind, from) = match next.as_str() {
                        "exception" => (Ent::Exception, k + 2),
                        "constant" => (Ent::Constant, k + 2),
                        _ => (Ent::Variable, k + 1),
                    };
                    let detail = if from < e { squeeze(src, nt[from].start, nt[e].start, 60) } else { String::new() };
                    out.push(Entity { kind, name: t.text(src).to_string(), detail, range: t.start..nt.get(e).map_or(src.len(), |x| x.end), at: t.start, depth: 1, decl: true });
                }
                k = e + 1;
            }
        }
    }
}

/// From just after a subprogram's IS / AS: the index of the ; after its
/// END. Nested subprograms and blocks counted; END IF / LOOP / CASE too.
fn skip_body(src: &str, nt: &[&Tok], mut k: usize) -> usize {
    let w = |k: usize| nt.get(k).map(|t| word(src, t)).unwrap_or_default();
    // Open units; each waits for its BEGIN (true) or is in it (false).
    let mut stack: Vec<bool> = vec![true];
    while k < nt.len() {
        match w(k).as_str() {
            "function" | "procedure" => {
                // A nested one with a body: IS / AS before its ;.
                let mut j = k + 1;
                let mut depth = 0;
                while let Some(t) = nt.get(j) {
                    match t.kind {
                        Tk::Open => depth += 1,
                        Tk::Close => depth -= 1,
                        Tk::Semi if depth == 0 => break,
                        _ if depth == 0 && matches!(word(src, t).as_str(), "is" | "as") => {
                            stack.push(true);
                            break;
                        }
                        _ => {}
                    }
                    j += 1;
                }
                k = j;
            }
            "declare" => stack.push(true),
            "begin" => match stack.last_mut() {
                Some(waiting) if *waiting => *waiting = false,
                _ => stack.push(false),
            },
            "case" => stack.push(false),
            "end" => {
                // END IF / END LOOP close nothing counted; END CASE does.
                match w(k + 1).as_str() {
                    "if" | "loop" => {
                        k += 2;
                        continue;
                    }
                    "case" => k += 1,
                    _ => {}
                }
                stack.pop();
                if stack.is_empty() {
                    // To the ; after END [name].
                    while k < nt.len() && nt[k].kind != Tk::Semi {
                        k += 1;
                    }
                    return k;
                }
            }
            _ => {}
        }
        k += 1;
    }
    nt.len().saturating_sub(1)
}

/// The entity the cursor is in (the innermost).
pub fn current(ents: &[Entity], at: usize) -> Option<usize> {
    ents.iter().enumerate().filter(|(_, e)| e.range.start <= at && at <= e.range.end).max_by_key(|(_, e)| e.depth).map(|(i, _)| i)
}

/// A package's specification and body, when the text is just those two
/// (as the database gives a package): (spec, body), each ending with "/".
pub fn split_package(src: &str) -> Option<(String, String)> {
    let d = Dialect::Oracle;
    let ents: Vec<Entity> = outline(src, d).into_iter().filter(|e| e.depth == 0).collect();
    let [a, b] = ents.as_slice() else { return None };
    let (spec, body) = match (a.kind, b.kind) {
        (Ent::Package, Ent::PackageBody) => (a, b),
        (Ent::PackageBody, Ent::Package) => (b, a),
        _ => return None,
    };
    if !spec.name.eq_ignore_ascii_case(&body.name) {
        return None;
    }
    // (Shown without the script's "/": it's put back when they're joined.)
    let piece = |e: &Entity| format!("{}\n", src[e.range.clone()].trim());
    Some((piece(spec), piece(body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PKG: &str = "CREATE OR REPLACE PACKAGE shop.shop_pkg AS\n  c_vat CONSTANT NUMBER := 0.2;\n  e_closed EXCEPTION;\n  g_count PLS_INTEGER;\n  TYPE t_ids IS TABLE OF NUMBER;\n  SUBTYPE money IS NUMBER(12,2);\n  CURSOR c_new IS SELECT id FROM orders WHERE status = 'new';\n  FUNCTION customer_ltv(p_customer NUMBER) RETURN NUMBER;\n  PROCEDURE close_stale_orders(p_days NUMBER DEFAULT 30);\nEND shop_pkg;\n/\nCREATE OR REPLACE PACKAGE BODY shop.shop_pkg AS\n  FUNCTION helper(x NUMBER) RETURN NUMBER IS\n    FUNCTION inner_one RETURN NUMBER IS BEGIN RETURN 1; END;\n  BEGIN\n    IF x > 0 THEN RETURN x; END IF;\n    FOR r IN (SELECT 1 FROM dual) LOOP NULL; END LOOP;\n    RETURN CASE WHEN x = 0 THEN 0 ELSE -x END;\n  END helper;\n\n  FUNCTION customer_ltv(p_customer NUMBER) RETURN NUMBER IS\n    v NUMBER;\n  BEGIN\n    BEGIN\n      SELECT 1 INTO v FROM dual;\n    EXCEPTION WHEN OTHERS THEN v := 0;\n    END;\n    CASE v WHEN 1 THEN NULL; ELSE NULL; END CASE;\n    RETURN v;\n  END;\n\n  PROCEDURE close_stale_orders(p_days NUMBER DEFAULT 30) IS\n  BEGIN\n    NULL;\n  END close_stale_orders;\nBEGIN\n  g_count := 0;\nEND shop_pkg;\n/\n";

    fn names(src: &str) -> Vec<(Ent, String, u8, bool)> {
        outline(src, Dialect::Oracle).into_iter().map(|e| (e.kind, e.name, e.depth, e.decl)).collect()
    }

    #[test]
    fn package_members() {
        let got = names(PKG);
        let want = vec![
            (Ent::Package, "shop.shop_pkg".to_string(), 0, false),
            (Ent::Constant, "c_vat".into(), 1, true),
            (Ent::Exception, "e_closed".into(), 1, true),
            (Ent::Variable, "g_count".into(), 1, true),
            (Ent::Type, "t_ids".into(), 1, true),
            (Ent::Subtype, "money".into(), 1, true),
            (Ent::Cursor, "c_new".into(), 1, true),
            (Ent::Function, "customer_ltv".into(), 1, true),
            (Ent::Procedure, "close_stale_orders".into(), 1, true),
            (Ent::PackageBody, "shop.shop_pkg".into(), 0, false),
            (Ent::Function, "helper".into(), 1, false),
            (Ent::Function, "customer_ltv".into(), 1, false),
            (Ent::Procedure, "close_stale_orders".into(), 1, false),
            (Ent::Init, "инициализация".into(), 1, false),
        ];
        crate::i18n::set_ru(true);
        assert_eq!(got.len(), want.len(), "{got:#?}");
        for (g, w) in got.iter().zip(&want) {
            if w.0 == Ent::Init {
                assert_eq!(g.0, Ent::Init);
            } else {
                assert_eq!(g, w);
            }
        }
        let ents = outline(PKG, Dialect::Oracle);
        let f = ents.iter().find(|e| e.name == "customer_ltv").unwrap();
        assert_eq!(f.detail, "(p_customer NUMBER) RETURN NUMBER");
        // The cursor inside the helper's IF: the helper.
        let at = PKG.find("RETURN x").unwrap();
        assert_eq!(ents[current(&ents, at).unwrap()].name, "helper");
    }

    #[test]
    fn statements_and_objects() {
        let src = "create table t (id int);\nselect * from sales.orders o where o.id = 1;\nupdate t set id = 2;\ncreate or replace function f(a int) returns int language sql as $$ select a $$;\ndo $$ begin null; end $$;\n";
        let got: Vec<(Ent, String)> = outline(src, Dialect::Postgres).into_iter().map(|e| (e.kind, e.name)).collect();
        assert_eq!(
            got,
            vec![
                (Ent::Table, "t".into()),
                (Ent::Statement, "SELECT … FROM sales.orders".into()),
                (Ent::Statement, "UPDATE t".into()),
                (Ent::Function, "f".into()),
                (Ent::Block, String::new()),
            ]
        );
    }

    #[test]
    fn splits_a_package() {
        let (spec, body) = split_package(PKG).unwrap();
        assert!(spec.starts_with("CREATE OR REPLACE PACKAGE shop.shop_pkg") && spec.ends_with("END shop_pkg;\n"), "{spec}");
        assert!(body.starts_with("CREATE OR REPLACE PACKAGE BODY") && body.ends_with("END shop_pkg;\n"), "{body}");
        assert!(split_package("select 1 from dual;").is_none());
    }
}
