//! A database's error in plain words: what's wrong ("no column «totl»"),
//! the name it's about (to be underlined where it is in the code), a
//! guess at what was meant, and the error's code — the server's whole
//! text kept for whoever wants it.

use std::ops::Range;

use crate::i18n::t;
use crate::sql::Dialect;
use crate::sql::lex::{self, Tk, Tok};

/// What a name in an error is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thing {
    Column,
    Table,
    Function,
    /// A value in the code (a string that's not a number, a date…).
    Value,
    /// Something else (a keyword, a schema, a constraint…).
    Other,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plain {
    /// "42703", "ORA-00904", "47 UNKNOWN_IDENTIFIER".
    pub code: Option<String>,
    /// What's wrong, in a sentence.
    pub text: String,
    /// The name (or value) it's about, as the server wrote it.
    pub name: Option<(String, Thing)>,
    /// What was meant, maybe; what to do.
    pub hint: Option<String>,
    /// The statement ends too early.
    pub at_end: bool,
}

/// The server's text made plain.
pub fn explain(raw: &str, d: Dialect) -> Plain {
    let raw = raw.trim();
    let p = match d {
        Dialect::Postgres => pg(raw),
        Dialect::Oracle => ora(raw),
        Dialect::ClickHouse => ch(raw),
        Dialect::Sqlite => lite(raw),
        Dialect::Generic => None,
    }
    .or_else(|| {
        // Another database's style (Generic tabs): try each.
        if raw.contains("ORA-") || raw.contains("PLS-") {
            ora(raw)
        } else if raw.starts_with("Code: ") {
            ch(raw)
        } else {
            None
        }
    });
    let mut p = p.unwrap_or_else(|| Plain { text: first_line(raw), ..Default::default() });
    if p.text.is_empty() {
        p.text = first_line(raw);
    }
    p
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// The first name quoted any way: "x", `x`, 'x', «x».
fn quoted(s: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (o, c) in [('"', '"'), ('`', '`'), ('\'', '\''), ('«', '»')] {
        if let Some(a) = s.find(o) {
            let from = a + o.len_utf8();
            if let Some(b) = s[from..].find(c) {
                if best.as_ref().is_none_or(|(x, _)| a < *x) {
                    best = Some((a, s[from..from + b].to_string()));
                }
            }
        }
    }
    best.map(|b| b.1).filter(|n| !n.is_empty())
}

/// "\"S\".\"T\"" or "S.T" → "T".
fn last_part(n: &str) -> String {
    n.rsplit('.').next().unwrap_or(n).trim_matches('"').to_string()
}

fn msg(ru: &str, en: &str, name: &str) -> String {
    t_owned(ru, en).replace("{}", name)
}

fn t_owned(ru: &str, en: &str) -> String {
    if crate::i18n::is_ru() { ru.to_string() } else { en.to_string() }
}

fn guess(names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| {
        let list = names.iter().map(|n| format!("«{n}»")).collect::<Vec<_>>().join(", ");
        format!("{} {list}?", t("Может быть,", "Maybe"))
    })
}

// ---------------------------------------------------------------- PostgreSQL

fn pg(raw: &str) -> Option<Plain> {
    let (code, rest) = raw.split_once(": ")?;
    if code.len() != 5 || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let mut lines = rest.lines();
    let head = lines.next().unwrap_or("").trim();
    let detail: Vec<&str> = lines.filter(|l| !l.starts_with("HINT: ")).collect();
    let server_hint = rest.lines().find_map(|l| l.strip_prefix("HINT: "));
    let q = quoted(head);
    let mut p = Plain { code: Some(code.into()), ..Default::default() };
    // "Perhaps you meant … "zz.total"" → total.
    let meant = server_hint.and_then(quoted).map(|n| last_part(&n));
    let named = |p: &mut Plain, ru: &str, en: &str, thing: Thing, n: Option<String>| {
        if let Some(n) = n {
            p.text = msg(ru, en, &n);
            p.name = Some((n, thing));
        }
    };
    match code {
        "42703" => {
            // column "x" does not exist / column z.x does not exist
            let n = q.clone().or_else(|| head.split_whitespace().find(|w| w.contains('.')).map(str::to_string));
            named(&mut p, "Нет такого поля «{}»", "No column «{}»", Thing::Column, n);
            p.hint = meant.map(|m| guess(&[m]).unwrap());
        }
        "42P01" => named(&mut p, "Нет такой таблицы «{}»", "No table «{}»", Thing::Table, q),
        "42883" => {
            // function lenght(text) does not exist; operator does not exist: …
            let f = head.split_whitespace().find(|w| w.contains('(')).map(|w| w.split('(').next().unwrap_or(w).to_string());
            if head.contains('(') && f.as_ref().is_some_and(|f| !f.is_empty()) {
                named(&mut p, "Нет такой функции «{}» (или для таких типов аргументов)", "No function «{}» (or none for such argument types)", Thing::Function, f);
                p.hint = Some(t("Проверьте имя и типы аргументов — может понадобиться приведение типа (x::int)", "Check the name and the argument types — a cast (x::int) may be needed").into());
            } else {
                p.text = format!("{} {}", t("Нет такого оператора для этих типов:", "No such operator for these types:"), head);
                p.hint = Some(t("Может понадобиться приведение типа (x::int)", "A cast (x::int) may be needed").into());
            }
        }
        "42601" => {
            if let Some(n) = q {
                named(&mut p, "Синтаксическая ошибка у «{}»", "Syntax error at «{}»", Thing::Other, Some(n));
            } else if head.contains("конце") || head.contains("end of input") {
                p.text = t("Запрос оборвался: после этого места чего-то не хватает", "The statement ends too early: something's missing after this").into();
                p.at_end = true;
            } else {
                p.text = head.into();
            }
        }
        "42702" => {
            named(&mut p, "Неоднозначное поле «{}»: оно есть в нескольких таблицах", "Ambiguous column «{}»: more than one table has it", Thing::Column, q);
            p.hint = Some(t("Укажите таблицу: t.поле", "Say which table: t.column").into());
        }
        "42803" => {
            let n = q.map(|n| last_part(&n));
            named(&mut p, "Поле «{}» должно быть в GROUP BY или внутри агрегатной функции", "Column «{}» must be in GROUP BY or inside an aggregate", Thing::Column, n);
            p.hint = Some(t("Добавьте его в GROUP BY или оберните: max(…), count(…), sum(…)", "Add it to GROUP BY or wrap it: max(…), count(…), sum(…)").into());
        }
        "42P07" | "42710" | "42723" => named(&mut p, "«{}» уже существует", "«{}» already exists", Thing::Other, q),
        "3F000" => named(&mut p, "Нет такой схемы «{}»", "No schema «{}»", Thing::Other, q),
        "3D000" => named(&mut p, "Нет такой базы данных «{}»", "No database «{}»", Thing::Other, q),
        "42704" => named(&mut p, "Нет такого объекта «{}»", "No object «{}»", Thing::Other, q),
        "23505" => {
            // Key "(id)=(1)" already exists.
            let key = detail.iter().find_map(|l| key_value(l));
            p.text = match &key {
                Some((c, v)) => format!("{} {c} = {v}", t("Такое значение уже есть (должно быть уникальным):", "This value already exists (must be unique):")),
                None => t("Повтор значения, которое должно быть уникальным", "A value that must be unique is repeated").into(),
            };
            if let Some(c) = q {
                p.hint = Some(format!("{} {c}", t("Ограничение:", "Constraint:")));
            }
        }
        "23502" => named(&mut p, "Поле «{}» не может быть пустым (NULL)", "Column «{}» can't be empty (NULL)", Thing::Column, q),
        "23503" => {
            let key = detail.iter().find_map(|l| key_value(l));
            let still = head.contains("update or delete") || head.contains("UPDATE или DELETE") || detail.iter().any(|l| l.contains("referenced") || l.contains("ссылки"));
            p.text = match (still, &key) {
                (true, Some((c, v))) => format!("{} {c} = {v}", t("На эту запись ссылаются другие таблицы:", "Other tables refer to this row:")),
                (true, None) => t("На эту запись ссылаются другие таблицы", "Other tables refer to this row").into(),
                (false, Some((c, v))) => format!("{} {c} = {v}", t("Нет связанной записи в родительской таблице:", "No such row in the parent table:")),
                (false, None) => t("Нет связанной записи в родительской таблице", "No such row in the parent table").into(),
            };
        }
        "23514" => named(&mut p, "Значение не проходит проверку «{}»", "The value fails check «{}»", Thing::Other, q.or_else(|| quoted(head))),
        "22P02" | "22007" | "22008" => {
            named(&mut p, "Значение «{}» не подходит по типу", "Value «{}» doesn't fit the type", Thing::Value, q);
            if let Some(ty) = head.split("type ").nth(1).or_else(|| head.split("типа ").nth(1)).and_then(|s| s.split(':').next()) {
                p.text = format!("{} ({ty})", p.text);
            }
        }
        "22001" => p.text = t("Значение слишком длинное для поля", "The value is too long for the column").into(),
        "22003" => p.text = t("Число вне допустимого диапазона типа", "The number is out of the type's range").into(),
        "22012" => p.text = t("Деление на ноль", "Division by zero").into(),
        "42804" => p.text = format!("{} {head}", t("Не тот тип данных:", "Wrong data type:")),
        "42501" => p.text = format!("{} {head}", t("Нет прав:", "No permission:")),
        "25P02" => {
            p.text = t("Транзакция прервана предыдущей ошибкой", "The transaction is aborted by an earlier error").into();
            p.hint = Some(t("Сделайте ROLLBACK (кнопка «Откатить»)", "ROLLBACK it (the Roll back button)").into());
        }
        "25006" => p.text = t("Транзакция только для чтения: изменять нельзя", "Read-only transaction: no changes allowed").into(),
        "57014" => p.text = t("Запрос отменён", "The statement was cancelled").into(),
        "40P01" => {
            p.text = t("Взаимная блокировка (deadlock): запрос прерван", "Deadlock: the statement was stopped").into();
            p.hint = Some(t("Повторите его", "Run it again").into());
        }
        "55P03" => p.text = t("Строка или таблица заблокирована другим сеансом", "A row or table is locked by another session").into(),
        "28P01" | "28000" => p.text = t("Неверный логин или пароль", "Wrong user name or password").into(),
        _ => {}
    }
    if p.text.is_empty() {
        p.text = head.to_string();
        if p.hint.is_none() {
            p.hint = server_hint.map(str::to_string);
        }
    }
    Some(p)
}

/// `Key "(id)=(1)" already exists.` → ("id", "1").
fn key_value(l: &str) -> Option<(String, String)> {
    let a = l.find('(')?;
    let rest = &l[a + 1..];
    let (c, rest) = rest.split_once(")=(")?;
    let v = rest.rsplit_once(')')?.0;
    Some((c.to_string(), v.to_string()))
}

// --------------------------------------------------------------------- Oracle

fn ora(raw: &str) -> Option<Plain> {
    let s = raw.strip_prefix("OCI Error: ").unwrap_or(raw);
    let s = s.strip_prefix("DPI Error: ").unwrap_or(s);
    let lines: Vec<&str> = s.lines().filter(|l| !l.starts_with("Help: ")).collect();
    let first = lines.iter().find(|l| l.starts_with("ORA-") || l.starts_with("PLS-") || l.starts_with("DPI-"))?;
    let (code, head) = first.split_once(": ").unwrap_or((first, ""));
    let head = head.trim();
    let q = quoted(head);
    let mut p = Plain { code: Some(code.into()), ..Default::default() };
    let named = |p: &mut Plain, ru: &str, en: &str, thing: Thing, n: Option<String>| {
        if let Some(n) = n {
            p.text = msg(ru, en, &n);
            p.name = Some((n, thing));
        }
    };
    match code {
        "ORA-00904" => {
            // "Z"."TOTL": invalid identifier
            let full = head.split(": ").next().unwrap_or(head).replace('"', "");
            let n = (!full.is_empty()).then(|| last_part(&full));
            named(&mut p, "Нет такого поля «{}»", "No column «{}»", Thing::Column, n);
            p.hint = Some(t("Или нет такой функции, или поле не той таблицы — проверьте имя", "Or no such function, or the column's from another table — check the name").into());
        }
        "ORA-00942" => {
            // table or view "S"."T" does not exist (older: no name)
            let n = head.contains('"').then(|| last_part(&head.split_whitespace().find(|w| w.contains('"')).unwrap_or("").replace('"', "")));
            if n.is_some() {
                named(&mut p, "Нет таблицы или представления «{}»", "No table or view «{}»", Thing::Table, n);
            } else {
                p.text = t("Нет такой таблицы или представления", "No such table or view").into();
            }
            p.hint = Some(t("Или к ней нет доступа: проверьте схему и права", "Or there's no access to it: check the schema and grants").into());
        }
        "ORA-00936" => p.text = t("Не хватает выражения (поля, значения…) в этом месте", "An expression (a column, a value…) is missing here").into(),
        "ORA-00933" => p.text = t("Команда SQL не закончена как надо: лишнее в этом месте", "The SQL command isn't properly ended: something extra here").into(),
        "ORA-00923" => p.text = t("Ожидалось FROM в этом месте", "FROM expected here").into(),
        "ORA-00905" => p.text = t("Не хватает ключевого слова", "A keyword is missing").into(),
        "ORA-00906" => p.text = t("Не хватает открывающей скобки", "An opening bracket is missing").into(),
        "ORA-00907" => p.text = t("Не хватает закрывающей скобки", "A closing bracket is missing").into(),
        "ORA-00917" => p.text = t("Не хватает запятой", "A comma is missing").into(),
        "ORA-00920" => p.text = t("Неверный оператор сравнения", "Wrong comparison operator").into(),
        "ORA-00911" => p.text = t("Недопустимый символ (например, «;» в конце запроса)", "Invalid character (e.g. «;» at the statement's end)").into(),
        "ORA-01756" => p.text = t("Не закрыта кавычка строки", "A string's quote isn't closed").into(),
        "ORA-03049" => named(&mut p, "Ключевое слово «{}» здесь не к месту", "Keyword «{}» doesn't belong here", Thing::Other, q),
        "ORA-00937" => {
            p.text = t("Агрегатная функция вместе с полями не из GROUP BY", "An aggregate together with columns not in GROUP BY").into();
            p.hint = Some(t("Добавьте остальные поля в GROUP BY", "Add the other columns to GROUP BY").into());
        }
        "ORA-00979" => p.text = t("Выражение не входит в GROUP BY", "The expression isn't in GROUP BY").into(),
        "ORA-00918" => {
            p.text = t("Неоднозначное поле: оно есть в нескольких таблицах", "Ambiguous column: more than one table has it").into();
            p.hint = Some(t("Укажите таблицу: t.поле", "Say which table: t.column").into());
        }
        "ORA-01722" => {
            // ORA-03302: (ORA-01722 details) invalid string value: abc
            let v = lines.iter().find_map(|l| l.split("invalid string value: ").nth(1)).map(|v| v.trim().to_string());
            match v {
                Some(v) => named(&mut p, "Строку «{}» не перевести в число", "String «{}» isn't a number", Thing::Value, Some(v)),
                None => p.text = t("Строку не перевести в число", "A string isn't a number").into(),
            }
        }
        "ORA-01476" => p.text = t("Деление на ноль", "Division by zero").into(),
        "ORA-01861" | "ORA-01858" | "ORA-01843" | "ORA-01839" | "ORA-01847" => p.text = format!("{} ({head})", t("Дата не совпадает с форматом", "The date doesn't match the format")),
        "ORA-00001" => {
            p.text = t("Такое значение уже есть (должно быть уникальным)", "This value already exists (must be unique)").into();
            if let Some(c) = head.split('(').nth(1).and_then(|r| r.split(')').next()) {
                p.hint = Some(format!("{} {c}", t("Ограничение:", "Constraint:")));
            }
        }
        "ORA-01400" => {
            let n = head.split('(').nth(1).map(|r| last_part(&r.trim_end_matches(')').replace('"', "")));
            named(&mut p, "Поле «{}» не может быть пустым (NULL)", "Column «{}» can't be empty (NULL)", Thing::Column, n);
        }
        "ORA-12899" => {
            let col = head.split_whitespace().find(|w| w.contains('"')).map(|w| last_part(&w.replace('"', "")));
            let size = head.split('(').nth(1).map(|r| r.trim_end_matches(')').to_string());
            match col {
                Some(c) => named(&mut p, "Значение слишком длинное для поля «{}»", "The value is too long for column «{}»", Thing::Column, Some(c)),
                None => p.text = t("Значение слишком длинное для поля", "The value is too long for the column").into(),
            }
            p.hint = size;
        }
        "ORA-01438" => p.text = t("Число больше, чем позволяет точность поля", "The number is larger than the column's precision").into(),
        "ORA-02291" => p.text = t("Нет связанной записи в родительской таблице", "No such row in the parent table").into(),
        "ORA-02292" => p.text = t("На эту запись ссылаются другие таблицы", "Other tables refer to this row").into(),
        "ORA-00955" => p.text = t("Имя уже занято другим объектом", "The name is already used by another object").into(),
        "ORA-01031" => p.text = t("Недостаточно прав", "Insufficient privileges").into(),
        "ORA-01017" => p.text = t("Неверный логин или пароль", "Wrong user name or password").into(),
        "ORA-12541" => p.text = t("Нет слушателя (listener) по этому адресу", "No listener at this address").into(),
        "ORA-12514" => p.text = t("Сервис с таким именем не найден", "No service of that name").into(),
        "ORA-01403" => p.text = t("Запрос не вернул данных (NO_DATA_FOUND)", "No data found").into(),
        "ORA-01422" => p.text = t("SELECT INTO вернул больше одной строки", "SELECT INTO returned more than one row").into(),
        "ORA-06502" => p.text = format!("{} ({head})", t("Ошибка числа или значения", "Numeric or value error")),
        "ORA-00932" => p.text = format!("{} ({head})", t("Несовместимые типы данных", "Inconsistent data types")),
        "ORA-00054" => p.text = t("Ресурс занят другим сеансом", "The resource is busy (another session)").into(),
        "ORA-01013" => p.text = t("Запрос отменён", "The statement was cancelled").into(),
        "ORA-06550" => {
            // PL/SQL: the PLS- line says what.
            if let Some(pls) = lines.iter().find(|l| l.starts_with("PLS-")) {
                let (pc, ph) = pls.split_once(": ").unwrap_or((pls, ""));
                p.code = Some(format!("{code} {pc}"));
                let pq = quoted(ph);
                match pc {
                    "PLS-00201" => named(&mut p, "Не объявлено: «{}»", "Not declared: «{}»", Thing::Function, pq),
                    "PLS-00103" => named(&mut p, "Синтаксическая ошибка PL/SQL у «{}»", "PL/SQL syntax error at «{}»", Thing::Other, pq),
                    "PLS-00302" => named(&mut p, "Нет такого элемента «{}»", "No member «{}»", Thing::Function, pq),
                    "PLS-00306" => named(&mut p, "Не то число или типы аргументов у «{}»", "Wrong number or types of arguments in «{}»", Thing::Function, pq),
                    "PLS-00905" => named(&mut p, "Объект «{}» с ошибками", "Object «{}» is invalid", Thing::Function, pq),
                    _ => {}
                }
                if p.text.is_empty() {
                    p.text = format!("PL/SQL: {}", ph.trim());
                }
            }
        }
        _ => {}
    }
    if p.text.is_empty() {
        p.text = head.to_string();
    }
    Some(p)
}

// ----------------------------------------------------------------- ClickHouse

fn ch(raw: &str) -> Option<Plain> {
    let rest = raw.strip_prefix("Code: ")?;
    let (num, rest) = rest.split_once(". ")?;
    let body = rest.strip_prefix("DB::Exception: ").unwrap_or(rest);
    // "… (UNKNOWN_IDENTIFIER) (version 24.x)": the name in the last brackets
    // of capitals.
    let name = body
        .match_indices('(')
        .filter_map(|(i, _)| {
            let e = body[i..].find(')')? + i;
            let n = &body[i + 1..e];
            (n.len() > 2 && n.chars().all(|c| c.is_ascii_uppercase() || c == '_')).then_some(n)
        })
        .last()
        .unwrap_or("");
    // The sentence itself: before "In scope", "In query", the stack…
    let mut s = body;
    let tail = format!(" ({name})");
    let mut cuts = vec![" In scope", " In query", ". Stack trace", "\nStack trace", ": While processing", " while processing"];
    if !name.is_empty() {
        cuts.push(&tail);
    }
    for cut in cuts {
        if let Some(i) = s.find(cut) {
            s = &s[..i];
        }
    }
    let s = s.trim().trim_end_matches([':', '.', ' ']);
    // "Maybe you meant: ['total']" / "Maybe you meant default.zz?"
    let (s, meant) = match s.find(". Maybe you meant") {
        Some(i) => {
            let m = &s[i + 17..];
            let names: Vec<String> = m.split(['\'', '[', ']', ',', ' ', ':', '?']).filter(|x| !x.is_empty()).map(|x| x.to_string()).collect();
            (&s[..i], names)
        }
        None => (s, Vec::new()),
    };
    let q = quoted(s);
    let mut p = Plain { code: Some(if name.is_empty() { num.to_string() } else { format!("{num} {name}") }), ..Default::default() };
    p.hint = guess(&meant);
    let named = |p: &mut Plain, ru: &str, en: &str, thing: Thing, n: Option<String>| {
        if let Some(n) = n {
            p.text = msg(ru, en, &n);
            p.name = Some((n, thing));
        }
    };
    match name {
        "UNKNOWN_IDENTIFIER" | "UNKNOWN_EXPRESSION_IDENTIFIER" => named(&mut p, "Нет такого поля «{}»", "No column «{}»", Thing::Column, q),
        "UNKNOWN_TABLE" => {
            named(&mut p, "Нет такой таблицы «{}»", "No table «{}»", Thing::Table, q.or_else(|| s.split_whitespace().nth(1).map(str::to_string)));
        }
        "UNKNOWN_FUNCTION" => {
            named(&mut p, "Нет такой функции «{}»", "No function «{}»", Thing::Function, q.clone());
            if let (Some(n), Some(m)) = (&q, meant.first()) {
                if m.eq_ignore_ascii_case(n) {
                    p.hint = Some(format!("{} «{m}»", t("Имена функций в ClickHouse чувствительны к регистру:", "Function names are case-sensitive in ClickHouse:")));
                }
            }
        }
        "UNKNOWN_DATABASE" => {
            let n = s.strip_prefix("Database ").and_then(|r| r.split_whitespace().next()).map(|n| n.trim_matches('`').to_string()).or(q);
            named(&mut p, "Нет такой базы данных «{}»", "No database «{}»", Thing::Other, n);
        }
        "SYNTAX_ERROR" => {
            // failed at position 26 (FORMAT…) (line 2, col 8): … Expected one of: …
            let tok = s.split("failed at position ").nth(1).and_then(|r| r.split_once('(').and_then(|(_, b)| b.split(')').next().map(str::to_string)));
            let ours = tok.as_ref().is_some_and(|t| t.starts_with("JSONCompact") || t.starts_with("FORMAT") || t == "end of query");
            if ours || tok.is_none() {
                p.text = t("Запрос оборвался: после этого места чего-то не хватает", "The statement ends too early: something's missing after this").into();
                p.at_end = true;
            } else {
                named(&mut p, "Синтаксическая ошибка у «{}»", "Syntax error at «{}»", Thing::Other, tok);
            }
            if let Some(exp) = body.split("Expected one of: ").nth(1).or_else(|| body.split("Expected ").nth(1)) {
                let exp = exp.split(". (").next().unwrap_or(exp);
                let items: Vec<&str> = exp.split(", ").map(str::trim).filter(|x| !x.is_empty() && !x.contains("token") && x.chars().next().is_some_and(|c| !c.is_lowercase() || *x == "alias")).filter(|x| !x.chars().any(|c| c.is_lowercase()) || *x == "alias").take(8).collect();
                if !items.is_empty() && !p.at_end {
                    p.hint = Some(format!("{} {}", t("Ожидалось:", "Expected:"), items.join(", ")));
                }
            }
        }
        "NOT_AN_AGGREGATE" => {
            named(&mut p, "Поле «{}» не в GROUP BY и не внутри агрегатной функции", "Column «{}» isn't in GROUP BY or inside an aggregate", Thing::Column, q.map(|n| last_part(&n)));
            p.hint = Some(t("Добавьте его в GROUP BY или оберните: any(…), max(…), count()", "Add it to GROUP BY or wrap it: any(…), max(…), count()").into());
        }
        "NUMBER_OF_ARGUMENTS_DOESNT_MATCH" => {
            let f = s.split("for function ").nth(1).and_then(|r| r.split_whitespace().next()).map(str::to_string);
            let counts = s.split("passed ").nth(1).map(|r| r.to_string());
            named(&mut p, "Не то число аргументов у функции «{}»", "Wrong number of arguments for «{}»", Thing::Function, f);
            if let Some(c) = counts {
                p.hint = Some(format!("{} {c}", t("передано", "passed")).replace("should be", t("нужно", "should be")));
            }
        }
        "ILLEGAL_TYPE_OF_ARGUMENT" | "NO_COMMON_TYPE" | "TYPE_MISMATCH" => {
            let f = s.split("of function ").nth(1).and_then(|r| r.split([' ', ':']).next()).map(|f| f.trim_matches('`').to_string());
            p.text = format!("{}: {s}", t("Неподходящие типы", "Wrong types"));
            if let Some(f) = f {
                p.name = Some((f, Thing::Function));
            }
        }
        "AMBIGUOUS_IDENTIFIER" | "AMBIGUOUS_COLUMN_NAME" => {
            named(&mut p, "Неоднозначное поле «{}»: оно есть в нескольких таблицах", "Ambiguous column «{}»: more than one table has it", Thing::Column, q);
            p.hint = Some(t("Укажите таблицу: t.поле", "Say which table: t.column").into());
        }
        "TABLE_ALREADY_EXISTS" => {
            let n = s.strip_prefix("Table ").and_then(|r| r.split_whitespace().next()).map(str::to_string).or(q);
            named(&mut p, "Таблица «{}» уже существует", "Table «{}» already exists", Thing::Table, n);
        }
        "MEMORY_LIMIT_EXCEEDED" => p.text = t("Запросу не хватило памяти (лимит сервера)", "The query ran out of memory (the server's limit)").into(),
        "TIMEOUT_EXCEEDED" => p.text = t("Превышено время выполнения", "Time limit exceeded").into(),
        "QUERY_WAS_CANCELLED" => p.text = t("Запрос отменён", "The query was cancelled").into(),
        "ACCESS_DENIED" => p.text = format!("{} {s}", t("Нет прав:", "No permission:")),
        "AUTHENTICATION_FAILED" | "REQUIRED_PASSWORD" => p.text = t("Неверный логин или пароль", "Wrong user name or password").into(),
        "UNKNOWN_SETTING" => named(&mut p, "Нет такой настройки «{}»", "No setting «{}»", Thing::Other, q),
        "TOO_MANY_ROWS_OR_BYTES" => p.text = t("Превышен лимит строк или байт результата", "Too many rows or bytes in the result").into(),
        _ if name.starts_with("CANNOT_PARSE") => {
            let v = quoted(s);
            p.text = format!("{}: {}", t("Не разобрать значение", "Can't parse the value"), s.split(": ").next().unwrap_or(s));
            if let Some(v) = v {
                p.name = Some((v, Thing::Value));
            }
        }
        _ => {}
    }
    if p.text.is_empty() {
        p.text = s.to_string();
    }
    Some(p)
}

// --------------------------------------------------------------------- SQLite

fn lite(raw: &str) -> Option<Plain> {
    // rusqlite: "no such column: totl in select … at offset 7"
    // (The statement may run over lines: cut before it, then one line.)
    let head = match raw.rfind(" at offset ") {
        Some(i) => raw[..i].split_once(" in ").map_or(&raw[..i], |(a, _)| a),
        None => raw,
    };
    let head = head.lines().next()?.trim();
    let after = |pre: &str| head.strip_prefix(pre).map(|r| r.trim().to_string());
    let code = raw.lines().last().and_then(|l| l.trim().strip_prefix("[SQLITE")).and_then(|r| r.strip_suffix(']')).map(|r| format!("SQLITE{r}"));
    let mut p = Plain { code, ..Default::default() };
    let named = |p: &mut Plain, ru: &str, en: &str, thing: Thing, n: Option<String>| {
        if let Some(n) = n {
            p.text = msg(ru, en, &n);
            p.name = Some((n, thing));
        }
    };
    if let Some(n) = after("no such column:") {
        named(&mut p, "Нет такого поля «{}»", "No column «{}»", Thing::Column, Some(n));
    } else if let Some(n) = after("no such table:") {
        named(&mut p, "Нет такой таблицы «{}»", "No table «{}»", Thing::Table, Some(n));
    } else if let Some(n) = after("no such function:") {
        named(&mut p, "Нет такой функции «{}»", "No function «{}»", Thing::Function, Some(n));
    } else if let Some(n) = after("ambiguous column name:") {
        named(&mut p, "Неоднозначное поле «{}»: оно есть в нескольких таблицах", "Ambiguous column «{}»: more than one table has it", Thing::Column, Some(n));
        p.hint = Some(t("Укажите таблицу: t.поле", "Say which table: t.column").into());
    } else if head.starts_with("near ") && head.ends_with("syntax error") {
        named(&mut p, "Синтаксическая ошибка у «{}»", "Syntax error at «{}»", Thing::Other, quoted(head));
    } else if head.starts_with("incomplete input") {
        p.text = t("Запрос оборвался: после этого места чего-то не хватает", "The statement ends too early: something's missing after this").into();
        p.at_end = true;
    } else if let Some(c) = after("UNIQUE constraint failed:") {
        p.text = format!("{} {c}", t("Такое значение уже есть (должно быть уникальным):", "This value already exists (must be unique):"));
        p.name = Some((last_part(c.split(", ").next().unwrap_or(&c)), Thing::Column));
    } else if let Some(c) = after("NOT NULL constraint failed:") {
        named(&mut p, "Поле «{}» не может быть пустым (NULL)", "Column «{}» can't be empty (NULL)", Thing::Column, Some(last_part(&c)));
    } else if head.starts_with("FOREIGN KEY constraint failed") {
        p.text = t("Нарушена связь между таблицами (внешний ключ)", "A foreign key is broken").into();
    } else if let Some(c) = after("CHECK constraint failed:") {
        p.text = format!("{} {c}", t("Значение не проходит проверку", "The value fails check"));
    } else if let Some(r) = head.strip_prefix("table ").and_then(|r| r.strip_suffix(" already exists")) {
        named(&mut p, "Таблица «{}» уже существует", "Table «{}» already exists", Thing::Table, Some(r.trim_matches('"').to_string()));
    } else if let Some(r) = head.strip_prefix("wrong number of arguments to function ") {
        named(&mut p, "Не то число аргументов у функции «{}»", "Wrong number of arguments for «{}»", Thing::Function, Some(r.trim_end_matches("()").to_string()));
    } else if let Some(r) = head.strip_prefix("misuse of aggregate: ").or_else(|| head.strip_prefix("misuse of aggregate function ")) {
        named(&mut p, "Агрегатная функция «{}» здесь нельзя", "Aggregate «{}» can't be used here", Thing::Function, Some(r.trim_end_matches("()").to_string()));
    } else if head.contains("database is locked") {
        p.text = t("База занята другим процессом", "The database is locked by another process").into();
    } else if head.contains("readonly database") {
        p.text = t("База только для чтения", "The database is read-only").into();
    } else if head.contains("datatype mismatch") {
        p.text = t("Не тот тип данных", "Data type mismatch").into();
    } else {
        p.text = head.to_string();
    }
    Some(p)
}

// ------------------------------------------------------------------ The place

fn ident(src: &str, t: &Tok) -> Option<String> {
    match t.kind {
        Tk::Word | Tk::Keyword => Some(t.text(src).to_string()),
        Tk::Quoted => Some(crate::sql::unquote(t.text(src)).to_string()),
        _ => None,
    }
}

/// Where in `sql` the error is: the name it's about (near the server's
/// position, if it gave one), else the word at that position, else the
/// statement's end when it ends too early.
pub fn place(sql: &str, at: Option<usize>, p: &Plain, d: Dialect) -> Option<Range<usize>> {
    let toks: Vec<Tok> = lex::lex(sql, d).into_iter().filter(|t| !t.trivia()).collect();
    if toks.is_empty() {
        return None;
    }
    let last = toks.last().unwrap();
    // A qualified name's whole run: z.totl.
    let dotted = |k: usize| -> Range<usize> {
        let mut a = k;
        while a >= 2 && toks[a - 1].kind == Tk::Dot && ident(sql, &toks[a - 2]).is_some() {
            a -= 2;
        }
        let mut b = k;
        while b + 2 < toks.len() && toks[b + 1].kind == Tk::Dot && ident(sql, &toks[b + 2]).is_some() {
            b += 2;
        }
        toks[a].start..toks[b].end
    };
    let hits: Vec<usize> = match &p.name {
        Some((n, thing)) => {
            let n = n.trim_matches(['"', '`']);
            let part = n.rsplit('.').next().unwrap_or(n);
            if *thing == Thing::Value {
                toks.iter().enumerate().filter(|(_, t)| matches!(t.kind, Tk::Str | Tk::Number) && t.text(sql).contains(part)).map(|(k, _)| k).collect()
            } else {
                toks.iter().enumerate().filter(|(_, t)| ident(sql, t).is_some_and(|w| w.eq_ignore_ascii_case(part))).map(|(k, _)| k).collect()
            }
        }
        None => Vec::new(),
    };
    let tok_at = |pos: usize| toks.iter().position(|t| t.start <= pos && pos < t.end).or_else(|| toks.iter().position(|t| t.start >= pos));
    let pos = at.filter(|&a| a < sql.trim_end().len());
    if p.at_end || (at.is_some() && pos.is_none()) {
        return Some(last.start..last.end);
    }
    let whole = |k: usize| if p.name.as_ref().is_some_and(|(n, _)| n.contains('.')) || ident(sql, &toks[k]).is_some() { dotted(k) } else { toks[k].start..toks[k].end };
    if !hits.is_empty() {
        // The one nearest the server's position (at or after it first).
        let k = match pos {
            Some(a) => *hits.iter().find(|&&k| toks[k].end > a).unwrap_or(hits.last().unwrap()),
            None => hits[0],
        };
        // Only the name itself, not its qualifier (z.totl → totl), unless
        // the whole was named.
        let r = if p.name.as_ref().is_some_and(|(n, _)| n.contains('.')) { dotted(k) } else { toks[k].start..toks[k].end };
        return Some(r);
    }
    let k = tok_at(pos?)?;
    Some(whole(k))
}

/// Names alike `name` among `known` (typos, case): the nearest few.
pub fn alike(name: &str, known: &[String]) -> Vec<String> {
    let n = name.to_lowercase();
    let len = n.chars().count();
    if len < 2 {
        return Vec::new();
    }
    let max = (len / 3).clamp(1, 3);
    let mut scored: Vec<(usize, &String)> = known
        .iter()
        .filter(|k| !k.eq_ignore_ascii_case(name) || *k != name)
        .filter_map(|k| {
            let d = distance(&n, &k.to_lowercase());
            (d <= max).then_some((d, k))
        })
        .collect();
    scored.sort_by_key(|(d, k)| (*d, k.len()));
    let mut out: Vec<String> = Vec::new();
    for (_, k) in scored {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(k)) {
            out.push(k.clone());
        }
        if out.len() == 3 {
            break;
        }
    }
    out
}

/// Edits to make `a` of `b` (with swaps of neighbours).
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// A guess of our own when the server gave none: names the database has
/// (from what's known of its tables and functions) alike the one it
/// doesn't know.
pub fn add_guess(p: &mut Plain, sql: &str, rels: &[super::Relation], d: Dialect) {
    if p.hint.is_some() {
        return;
    }
    let Some((n, thing)) = &p.name else { return };
    let part = n.rsplit('.').next().unwrap_or(n).trim_matches('"');
    let known: Vec<String> = match thing {
        Thing::Column => {
            // Columns of the statement's tables first, else any.
            let words: Vec<String> = lex::lex(sql, d).iter().filter_map(|t| ident(sql, t)).map(|w| w.to_lowercase()).collect();
            let mine: Vec<String> = rels.iter().filter(|r| words.contains(&r.name.to_lowercase())).flat_map(|r| r.cols.iter().map(|c| c.0.clone())).collect();
            if mine.is_empty() { rels.iter().flat_map(|r| r.cols.iter().map(|c| c.0.clone())).collect() } else { mine }
        }
        Thing::Table => rels.iter().map(|r| r.name.clone()).collect(),
        Thing::Function => crate::sql::functions(d).into_iter().map(str::to_string).collect(),
        _ => return,
    };
    p.hint = guess(&alike(part, &known));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(sql: &str, s: &str) -> Range<usize> {
        let a = sql.find(s).unwrap();
        a..a + s.len()
    }

    #[test]
    fn postgres() {
        crate::i18n::set_ru(true);
        let sql = "select totl from zz";
        let p = explain("42703: столбец \"totl\" не существует\nHINT: Возможно, предполагалась ссылка на столбец \"zz.total\".", Dialect::Postgres);
        assert_eq!(p.code.as_deref(), Some("42703"));
        assert_eq!(p.text, "Нет такого поля «totl»");
        assert_eq!(p.hint.as_deref(), Some("Может быть, «total»?"));
        assert_eq!(place(sql, Some(7), &p, Dialect::Postgres), Some(r(sql, "totl")));
        let sql = "select z.totl from zz z";
        let p = explain("42703: столбец z.totl не существует\nHINT: Возможно, предполагалась ссылка на столбец \"z.total\".", Dialect::Postgres);
        assert_eq!(p.text, "Нет такого поля «z.totl»");
        assert_eq!(place(sql, Some(7), &p, Dialect::Postgres), Some(r(sql, "z.totl")));
        let p = explain("42883: функция lenght(text) не существует\nHINT: Функция с данными именем…", Dialect::Postgres);
        assert!(p.text.starts_with("Нет такой функции «lenght»"), "{}", p.text);
        let sql = "select * from zz where";
        let p = explain("42601: ошибка синтаксиса в конце", Dialect::Postgres);
        assert!(p.at_end);
        assert_eq!(place(sql, None, &p, Dialect::Postgres), Some(r(sql, "where")));
        let p = explain("23505: повторяющееся значение ключа нарушает ограничение уникальности \"zz_pkey\"\nКлюч \"(id)=(1)\" уже существует.", Dialect::Postgres);
        assert_eq!(p.text, "Такое значение уже есть (должно быть уникальным): id = 1");
        let sql = "select 'abc'::int";
        let p = explain("22P02: неверный синтаксис для типа integer: \"abc\"", Dialect::Postgres);
        assert_eq!(place(sql, Some(7), &p, Dialect::Postgres), Some(r(sql, "'abc'")));
    }

    #[test]
    fn clickhouse() {
        crate::i18n::set_ru(true);
        let p = explain("Code: 47. DB::Exception: Unknown expression identifier `totl`. Maybe you meant: ['total']. In scope SELECT totl FROM zz. (UNKNOWN_IDENTIFIER)", Dialect::ClickHouse);
        assert_eq!((p.code.as_deref(), p.text.as_str(), p.hint.as_deref()), (Some("47 UNKNOWN_IDENTIFIER"), "Нет такого поля «totl»", Some("Может быть, «total»?")));
        let p = explain("Code: 46. DB::Exception: Function with name `lenght` does not exist. Maybe you meant: ['length']. In scope SELECT lenght(name) FROM zz. (UNKNOWN_FUNCTION)", Dialect::ClickHouse);
        assert_eq!(p.text, "Нет такой функции «lenght»");
        let p = explain("Code: 60. DB::Exception: Unknown table expression identifier 'zzz'. Maybe you meant default.zz? In scope SELECT * FROM zzz. (UNKNOWN_TABLE)", Dialect::ClickHouse);
        assert_eq!((p.text.as_str(), p.hint.as_deref()), ("Нет такой таблицы «zzz»", Some("Может быть, «default.zz»?")));
        let sql = "select from where";
        let p = explain("Code: 62. DB::Exception: Syntax error: failed at position 26 (JSONCompactEachRowWithNamesAndTypes) (line 2, col 8): JSONCompactEachRowWithNamesAndTypes. Expected one of: token sequence, Dot, OR, AND, end of query. (SYNTAX_ERROR)", Dialect::ClickHouse);
        assert!(p.at_end);
        assert_eq!(place(sql, Some(25), &p, Dialect::ClickHouse), Some(r(sql, "where")));
        let p = explain("Code: 62. DB::Exception: Syntax error: failed at position 8 (fro) (line 1, col 8): fro x. Expected one of: FROM, WHERE, alias, token. (SYNTAX_ERROR) (version 25.1)", Dialect::ClickHouse);
        assert_eq!((p.text.as_str(), p.hint.as_deref()), ("Синтаксическая ошибка у «fro»", Some("Ожидалось: FROM, WHERE, alias")));
        let p = explain("Code: 215. DB::Exception: Column 'default.zz.total' is not under aggregate function and not in GROUP BY keys. In query SELECT total, count() FROM zz. (NOT_AN_AGGREGATE)", Dialect::ClickHouse);
        assert_eq!(p.name, Some(("total".into(), Thing::Column)));
        let p = explain("Code: 81. DB::Exception: Database nodb does not exist. (UNKNOWN_DATABASE)", Dialect::ClickHouse);
        assert_eq!(p.text, "Нет такой базы данных «nodb»");
        let p = explain("Code: 42. DB::Exception: Number of arguments for function plus doesn't match: passed 1, should be 2: In scope SELECT plus(1). (NUMBER_OF_ARGUMENTS_DOESNT_MATCH)", Dialect::ClickHouse);
        assert_eq!((p.text.as_str(), p.hint.as_deref()), ("Не то число аргументов у функции «plus»", Some("передано 1, нужно 2")));
    }

    #[test]
    fn oracle() {
        crate::i18n::set_ru(true);
        let sql = "select z.totl from dual z";
        let p = explain("OCI Error: ORA-00904: \"Z\".\"TOTL\": invalid identifier\nHelp: https://docs.oracle.com/error-help/db/ora-00904/", Dialect::Oracle);
        assert_eq!((p.code.as_deref(), p.text.as_str()), (Some("ORA-00904"), "Нет такого поля «TOTL»"));
        assert_eq!(place(sql, Some(7), &p, Dialect::Oracle), Some(r(sql, "totl")));
        let p = explain("OCI Error: ORA-00942: table or view \"ZAPROS\".\"ZZZ\" does not exist\nHelp: …", Dialect::Oracle);
        assert_eq!(p.text, "Нет таблицы или представления «ZZZ»");
        let p = explain("OCI Error: ORA-01722: unable to convert string value containing 'a' to a number: \nORA-03302: (ORA-01722 details) invalid string value: abc\nHelp: …", Dialect::Oracle);
        assert_eq!(p.text, "Строку «abc» не перевести в число");
        let p = explain("OCI Error: ORA-06550: line 1, column 7:\nPLS-00201: identifier 'NOSUCH' must be declared\nORA-06550: line 1, column 7:\nPL/SQL: Statement ignored", Dialect::Oracle);
        assert_eq!((p.code.as_deref(), p.text.as_str()), (Some("ORA-06550 PLS-00201"), "Не объявлено: «NOSUCH»"));
    }

    #[test]
    fn sqlite_and_guesses() {
        crate::i18n::set_ru(true);
        let sql = "select totl from orders";
        let mut p = explain("no such column: totl in select totl from orders at offset 7", Dialect::Sqlite);
        assert_eq!(p.text, "Нет такого поля «totl»");
        let u = explain("UNIQUE constraint failed: orders.id\n[SQLITE_CONSTRAINT_PRIMARYKEY]", Dialect::Sqlite);
        assert_eq!((u.code.as_deref(), u.text.as_str()), (Some("SQLITE_CONSTRAINT_PRIMARYKEY"), "Такое значение уже есть (должно быть уникальным): orders.id"));
        let rels = vec![super::super::Relation { schema: "main".into(), name: "orders".into(), cols: vec![("total".into(), "int".into()), ("id".into(), "int".into())], qualify: false }];
        add_guess(&mut p, sql, &rels, Dialect::Sqlite);
        assert_eq!(p.hint.as_deref(), Some("Может быть, «total»?"));
        assert_eq!(place(sql, Some(7), &p, Dialect::Sqlite), Some(r(sql, "totl")));
        let p = explain("near \"form\": syntax error in select 1 form t at offset 9", Dialect::Sqlite);
        assert_eq!(p.text, "Синтаксическая ошибка у «form»");
        assert_eq!(alike("lenght", &["length".into(), "left".into(), "upper".into()]), vec!["length".to_string()]);
    }
}
