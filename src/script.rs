//! Script directives (docs/directives.md): variables, lists, dictionaries,
//! loops, conditions and output, written in `-- @…` comments of an SQL
//! script. The script is read through first (`parse`) into a program of SQL
//! statements and directives; the connection's worker then runs it (`run`)
//! in its session, putting the variables into the SQL (`:x` a value, `&x`
//! text as it is) statement by statement.

use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use chrono::{Datelike, Months, NaiveDate, NaiveDateTime, Timelike};

use crate::db::{Stmt, Table};
use crate::sql::Dialect;
use crate::sql::lex::{self, Tk};

// ───────────────────────────── values ─────────────────────────────

/// How fine a date is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prec {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    /// A number written with leading zeros: keeps its width when put in.
    Pad(i64, u8),
    Num(f64),
    Str(String),
    Date(NaiveDateTime, Prec),
    List(Vec<Value>),
    Dict(Vec<(String, Value)>),
}

impl Value {
    fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Int(_) | Value::Pad(..) | Value::Num(_) => "number",
            Value::Str(_) => "string",
            Value::Date(..) => "date",
            Value::List(_) => "list",
            Value::Dict(_) => "dictionary",
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(n) | Value::Pad(n, _) => *n != 0,
            Value::Num(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Date(..) => true,
            Value::List(l) => !l.is_empty(),
            Value::Dict(d) => !d.is_empty(),
        }
    }

    fn num(&self) -> Option<f64> {
        match self {
            Value::Int(n) | Value::Pad(n, _) => Some(*n as f64),
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(*b as i64 as f64),
            Value::Str(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    }

    fn int(&self) -> Option<i64> {
        match self {
            Value::Int(n) | Value::Pad(n, _) => Some(*n),
            Value::Num(n) if n.fract() == 0.0 => Some(*n as i64),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    /// As text, the way `&x` and `@print` put it.
    pub fn text(&self) -> Result<String, String> {
        Ok(match self {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Pad(n, w) => pad(*n, *w),
            Value::Num(n) => num_text(*n),
            Value::Str(s) => s.clone(),
            Value::Date(d, p) => date_text(d, *p),
            Value::List(l) => l.iter().map(Value::text).collect::<Result<Vec<_>, _>>()?.join(","),
            Value::Dict(_) => return Err("a dictionary can't go into SQL as it is: take a field of it".into()),
        })
    }

    /// As a value in SQL, the way `:x` puts it.
    pub fn sql(&self, d: Dialect) -> Result<String, String> {
        Ok(match self {
            Value::Null => "NULL".into(),
            Value::Bool(b) if d == Dialect::Oracle => (if *b { "1" } else { "0" }).into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(_) | Value::Pad(..) | Value::Num(_) => self.text()?,
            Value::Str(s) => crate::sql::params::value(s, crate::sql::params::As::Text, d),
            Value::Date(dt, p) => {
                let s = date_text(dt, *p);
                match d {
                    Dialect::Oracle if *p >= Prec::Day => format!("DATE '{s}'"),
                    Dialect::Oracle => format!("TIMESTAMP '{s}'"),
                    _ => format!("'{s}'"),
                }
            }
            Value::List(l) => l.iter().map(|v| v.sql(d)).collect::<Result<Vec<_>, _>>()?.join(", "),
            Value::Dict(_) => return Err("a dictionary can't go into SQL as it is: take a field of it".into()),
        })
    }

    /// As `@print` shows it inside a list or dictionary.
    fn shown_inner(&self) -> String {
        match self {
            Value::Str(s) => format!("\"{}\"", s.replace('"', "\\\"")),
            Value::Date(..) => format!("\"{}\"", self.shown()),
            _ => self.shown(),
        }
    }

    /// As `@print` shows it.
    pub fn shown(&self) -> String {
        match self {
            Value::Null => "null".into(),
            Value::List(l) => format!("[{}]", l.iter().map(Value::shown_inner).collect::<Vec<_>>().join(", ")),
            Value::Dict(d) => format!("{{{}}}", d.iter().map(|(k, v)| format!("\"{k}\": {}", v.shown_inner())).collect::<Vec<_>>().join(", ")),
            v => v.text().unwrap_or_default(),
        }
    }

    fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v),
            _ => None,
        }
    }
}

fn pad(n: i64, w: u8) -> String {
    if n < 0 { format!("-{:0w$}", -n, w = w as usize) } else { format!("{:0w$}", n, w = w as usize) }
}

fn num_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

fn date_text(d: &NaiveDateTime, p: Prec) -> String {
    match p {
        Prec::Second => d.format("%Y-%m-%d %H:%M:%S").to_string(),
        Prec::Minute | Prec::Hour => d.format("%Y-%m-%d %H:%M:00").to_string(),
        _ => d.format("%Y-%m-%d").to_string(),
    }
}

// ───────────────────────────── dates ─────────────────────────────

/// A date format's pieces (Oracle style, any case).
#[derive(Clone, Debug, PartialEq)]
enum Fmt {
    Yyyy,
    Yy,
    Mm,
    Dd,
    Ddd,
    Hh,
    Mi,
    Ss,
    Q,
    Lit(char),
}

fn fmt_tokens(f: &str) -> Vec<Fmt> {
    let low = f.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while i < low.len() {
        let r = &low[i..];
        let (t, n) = if r.starts_with("yyyy") {
            (Fmt::Yyyy, 4)
        } else if r.starts_with("hh24") {
            (Fmt::Hh, 4)
        } else if r.starts_with("ddd") {
            (Fmt::Ddd, 3)
        } else if r.starts_with("yy") {
            (Fmt::Yy, 2)
        } else if r.starts_with("mm") {
            (Fmt::Mm, 2)
        } else if r.starts_with("dd") {
            (Fmt::Dd, 2)
        } else if r.starts_with("hh") {
            (Fmt::Hh, 2)
        } else if r.starts_with("mi") {
            (Fmt::Mi, 2)
        } else if r.starts_with("ss") {
            (Fmt::Ss, 2)
        } else if r.starts_with('q') {
            (Fmt::Q, 1)
        } else {
            let c = f[i..].chars().next().unwrap();
            (Fmt::Lit(c), c.len_utf8())
        };
        out.push(t);
        i += n;
    }
    out
}

/// A date read with a format; its precision, the finest piece there.
fn date_by(s: &str, f: &str) -> Result<(NaiveDateTime, Prec), String> {
    let bad = || format!("'{s}' doesn't read as a date by '{f}'");
    let (mut y, mut mo, mut d, mut h, mut mi, mut se) = (1970i32, 1u32, 1u32, 0u32, 0u32, 0u32);
    let mut doy: Option<u32> = None;
    let mut prec = Prec::Year;
    let mut rest = s.trim();
    let take = |n: usize, rest: &mut &str| -> Result<u32, String> {
        let digits: String = rest.chars().take(n).take_while(char::is_ascii_digit).collect();
        if digits.len() != n {
            return Err(bad());
        }
        *rest = &rest[n..];
        digits.parse().map_err(|_| bad())
    };
    for t in fmt_tokens(f) {
        match t {
            Fmt::Yyyy => y = take(4, &mut rest)? as i32,
            Fmt::Yy => y = 2000 + take(2, &mut rest)? as i32,
            Fmt::Q => {
                mo = (take(1, &mut rest)? - 1) * 3 + 1;
                prec = prec.min(Prec::Quarter);
            }
            Fmt::Mm => {
                mo = take(2, &mut rest)?;
                prec = prec.min(Prec::Month);
            }
            Fmt::Dd => {
                d = take(2, &mut rest)?;
                prec = prec.min(Prec::Day);
            }
            Fmt::Ddd => {
                doy = Some(take(3, &mut rest)?);
                prec = prec.min(Prec::Day);
            }
            Fmt::Hh => {
                h = take(2, &mut rest)?;
                prec = prec.min(Prec::Hour);
            }
            Fmt::Mi => {
                mi = take(2, &mut rest)?;
                prec = prec.min(Prec::Minute);
            }
            Fmt::Ss => {
                se = take(2, &mut rest)?;
                prec = prec.min(Prec::Second);
            }
            Fmt::Lit(c) => {
                if !rest.starts_with(c) {
                    return Err(bad());
                }
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if !rest.is_empty() {
        return Err(bad());
    }
    let date = match doy {
        Some(n) => NaiveDate::from_yo_opt(y, n),
        None => NaiveDate::from_ymd_opt(y, mo, d),
    }
    .ok_or_else(bad)?;
    let dt = date.and_hms_opt(h, mi, se).ok_or_else(bad)?;
    Ok((dt, prec))
}

/// A date written the usual way: 2026-11-01, 2026-11-01 10:30[:00], 2026-11.
fn date_iso(s: &str) -> Result<(NaiveDateTime, Prec), String> {
    let s = s.trim();
    // A year alone: the only number read as a date by itself.
    if let Some(d) = year(s) {
        return Ok((d, Prec::Year));
    }
    for (f, p) in [("%Y-%m-%d %H:%M:%S", Prec::Second), ("%Y-%m-%dT%H:%M:%S", Prec::Second), ("%Y-%m-%d %H:%M", Prec::Minute)] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
            return Ok((d, p));
        }
    }
    // Fractions of a second (from a server): dropped.
    if let Some(head) = s.split('.').next()
        && head.len() < s.len()
        && let Ok(d) = NaiveDateTime::parse_from_str(head, "%Y-%m-%d %H:%M:%S")
    {
        return Ok((d, Prec::Second));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok((d.and_hms_opt(0, 0, 0).unwrap(), Prec::Day));
    }
    if let Ok(d) = NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d") {
        return Ok((d.and_hms_opt(0, 0, 0).unwrap(), Prec::Month));
    }
    Err(format!("'{s}' doesn't read as a date (2026-11-01, 2026-11-01 10:30, 2026-11; or give a format: | date yyyymmdd)"))
}

/// Four digits, a year: its first day.
fn year(s: &str) -> Option<NaiveDateTime> {
    if s.len() != 4 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    NaiveDate::from_ymd_opt(s.parse().ok()?, 1, 1)?.and_hms_opt(0, 0, 0)
}

/// A value as a date for `| range`: a date, a year (2026) or a date's text
/// written the usual way; other numbers aren't guessed.
fn as_period(v: &Value) -> Result<(NaiveDateTime, Prec), String> {
    match v {
        Value::Date(d, p) => Ok((*d, *p)),
        Value::Str(t) => date_iso(t).map_err(|_| format!("| range: '{t}' isn't a date (2026, 2026-11, 2026-11-05; or read it first: | date yyyymm)")),
        v => {
            let t = v.text()?;
            match v {
                Value::Int(_) => year(&t).map(|d| (d, Prec::Year)),
                _ => None,
            }
            .ok_or_else(|| format!("| range: the number {t} is taken for a date only when it's a year (2026); read it first: {t} | date {}", guess_fmt(&t)))
        }
    }
}

/// A format the digits likely are in (for a hint).
fn guess_fmt(t: &str) -> &'static str {
    match t.len() {
        6 => "yyyymm",
        8 => "yyyymmdd",
        10 => "yyyymmddhh24",
        _ => "yyyymm",
    }
}

fn date_fmt(d: &NaiveDateTime, f: &str) -> String {
    let mut out = String::new();
    for t in fmt_tokens(f) {
        match t {
            Fmt::Yyyy => out += &format!("{:04}", d.year()),
            Fmt::Yy => out += &format!("{:02}", d.year() % 100),
            Fmt::Mm => out += &format!("{:02}", d.month()),
            Fmt::Dd => out += &format!("{:02}", d.day()),
            Fmt::Ddd => out += &format!("{:03}", d.ordinal()),
            Fmt::Hh => out += &format!("{:02}", d.hour()),
            Fmt::Mi => out += &format!("{:02}", d.minute()),
            Fmt::Ss => out += &format!("{:02}", d.second()),
            Fmt::Q => out += &((d.month() - 1) / 3 + 1).to_string(),
            Fmt::Lit(c) => out.push(c),
        }
    }
    out
}

/// A date moved by `n` units.
fn date_add(d: NaiveDateTime, unit: Prec, n: i64) -> Option<NaiveDateTime> {
    let months = |m: i64| if m >= 0 { d.checked_add_months(Months::new(m as u32)) } else { d.checked_sub_months(Months::new((-m) as u32)) };
    match unit {
        Prec::Second => d.checked_add_signed(chrono::Duration::seconds(n)),
        Prec::Minute => d.checked_add_signed(chrono::Duration::minutes(n)),
        Prec::Hour => d.checked_add_signed(chrono::Duration::hours(n)),
        Prec::Day => d.checked_add_signed(chrono::Duration::days(n)),
        Prec::Week => d.checked_add_signed(chrono::Duration::weeks(n)),
        Prec::Month => months(n),
        Prec::Quarter => months(n * 3),
        Prec::Year => months(n * 12),
    }
}

// ───────────────────────────── expressions ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Conv {
    Number,
    /// To text; with a format, a date's (or a date's text) in it.
    Str(Option<String>),
    List,
    Date(Option<String>),
    /// A collection's: an item added (at its place: 0 the first, -1 the
    /// last), one taken off (the last; by its place; a dictionary's by its
    /// key), ones equal to a value taken off.
    Push(Box<Expr>, Option<Box<Expr>>),
    Pop(Option<Box<Expr>>),
    Remove(Box<Expr>),
    /// Each item (as `it`, or the name before "=>") worked out; kept when
    /// it holds.
    Map(String, Box<Expr>),
    Filter(String, Box<Expr>),
    /// By the items, or by what each gives.
    Sort { desc: bool, by: Option<(String, Box<Expr>)> },
    Reverse,
    Unique,
    /// The items as text, between them ", " or the given text.
    Join(Option<Box<Expr>>),
    /// A date's period (a list's: each item's) cut by so many units:
    /// `2026 | range month`.
    Range(Option<Box<Expr>>, Prec),
}

impl Conv {
    /// One that reads a collection (not a value's kind).
    fn on_items(&self) -> bool {
        !matches!(self, Conv::Number | Conv::Str(_) | Conv::List | Conv::Date(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Lit(Value),
    Var(String),
    List(Vec<Expr>),
    Dict(Vec<(String, Expr)>),
    Template(Vec<Piece>),
    /// A query: its text (variables put in when it runs).
    Query(String),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Unary(&'static str, Box<Expr>),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    Pipe(Box<Expr>, Conv),
    /// a .. b [| conv] [step …] [| …]: a list.
    Range(Box<Steps>),
}

#[derive(Clone, Debug, PartialEq)]
enum T {
    Num(String),
    Str(String),
    Tpl(String),
    Name(String),
    Query(String),
    Op(&'static str),
}

const OPS: [&str; 29] = ["..", "=>", "|=", "==", "!=", "<>", "<=", ">=", "+=", "-=", "*=", "/=", "++", "--", "=", "<", ">", "+", "-", "*", "/", "%", "|", "(", ")", "[", "]", ",", "."];

fn tokens(src: &str) -> Result<Vec<T>, String> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = src[i..].chars().next().unwrap();
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            // A fraction, not a range's "..".
            if j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
                j += 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            out.push(T::Num(src[i..j].to_string()));
            i = j;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < src.len() && src[j..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                j += src[j..].chars().next().unwrap().len_utf8();
            }
            out.push(T::Name(src[i..j].to_string()));
            i = j;
            continue;
        }
        if c == '\'' || c == '"' {
            let mut s = String::new();
            let mut j = i + 1;
            loop {
                let Some(ch) = src[j..].chars().next() else { return Err("a string isn't closed".into()) };
                j += ch.len_utf8();
                if ch == '\\' {
                    if let Some(n) = src[j..].chars().next() {
                        s.push(n);
                        j += n.len_utf8();
                    }
                } else if ch == c {
                    // '' inside '…': one quote.
                    if src[j..].starts_with(c) {
                        s.push(c);
                        j += 1;
                    } else {
                        break;
                    }
                } else {
                    s.push(ch);
                }
            }
            out.push(T::Str(s));
            i = j;
            continue;
        }
        if c == '`' {
            let Some(end) = src[i + 1..].find('`') else { return Err("a ` string isn't closed".into()) };
            out.push(T::Tpl(src[i + 1..i + 1 + end].to_string()));
            i += end + 2;
            continue;
        }
        if c == '{' || c == '}' || c == ':' {
            out.push(T::Op(match c {
                '{' => "{",
                '}' => "}",
                _ => ":",
            }));
            i += 1;
            continue;
        }
        if c == '(' {
            // A query: its text to the matching ")".
            let after = src[i + 1..].trim_start();
            let word: String = after.chars().take_while(|c| c.is_alphabetic()).collect::<String>().to_ascii_lowercase();
            if word == "select" || word == "with" {
                let end = closing(src, i).ok_or("a query's \"(\" isn't closed")?;
                out.push(T::Query(src[i + 1..end].trim().to_string()));
                i = end + 1;
                continue;
            }
        }
        let Some(op) = OPS.iter().find(|o| src[i..].starts_with(**o)) else { return Err(format!("\"{c}\" isn't understood here")) };
        out.push(T::Op(op));
        i += op.len();
    }
    Ok(out)
}

/// The ")" matching the "(" at `open` (strings skipped).
fn closing(src: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    for (k, c) in src[open..].char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' | '`' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(open + k);
                    }
                }
                _ => {}
            },
        }
    }
    None
}

struct P {
    t: Vec<T>,
    i: usize,
    /// Inside a collection operation's argument: "|" ends it (the next
    /// operation's), unless in brackets.
    no_pipe: u32,
}

impl P {
    fn new(src: &str) -> Result<P, String> {
        Ok(P { t: tokens(src)?, i: 0, no_pipe: 0 })
    }

    fn peek(&self) -> Option<&T> {
        self.t.get(self.i)
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Some(T::Op(o)) if *o == op)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Some(T::Name(n)) if n.eq_ignore_ascii_case(w))
    }

    fn eat_op(&mut self, op: &str) -> bool {
        let y = self.is_op(op);
        if y {
            self.i += 1;
        }
        y
    }

    fn eat_word(&mut self, w: &str) -> bool {
        let y = self.is_word(w);
        if y {
            self.i += 1;
        }
        y
    }

    fn want_op(&mut self, op: &str) -> Result<(), String> {
        if self.eat_op(op) { Ok(()) } else { Err(format!("\"{op}\" expected{}", self.here())) }
    }

    fn here(&self) -> String {
        match self.peek() {
            None => " at the end".into(),
            Some(T::Op(o)) => format!(" before \"{o}\""),
            Some(T::Name(n)) | Some(T::Num(n)) => format!(" before \"{n}\""),
            Some(T::Str(s)) => format!(" before '{s}'"),
            Some(_) => String::new(),
        }
    }

    fn name(&mut self) -> Result<String, String> {
        match self.peek() {
            Some(T::Name(n)) => {
                let n = n.clone();
                self.i += 1;
                Ok(n)
            }
            _ => Err(format!("a name expected{}", self.here())),
        }
    }

    fn done(&self) -> Result<(), String> {
        if self.i < self.t.len() { Err(format!("more than expected{}", self.here())) } else { Ok(()) }
    }

    /// An expression, or bare items with commas: a list.
    fn list_or_expr(&mut self) -> Result<Expr, String> {
        let first = self.expr()?;
        if self.eat_op("..") {
            return Ok(Expr::Range(Box::new(steps(self, first)?)));
        }
        if !self.is_op(",") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_op(",") {
            items.push(self.expr()?);
        }
        Ok(Expr::List(items))
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while self.eat_word("or") {
            l = Expr::Bin("or", Box::new(l), Box::new(self.and()?));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.not()?;
        while self.eat_word("and") {
            l = Expr::Bin("and", Box::new(l), Box::new(self.not()?));
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.eat_word("not") {
            return Ok(Expr::Unary("not", Box::new(self.not()?)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let l = self.add()?;
        for op in ["==", "!=", "<>", "<=", ">=", "=", "<", ">"] {
            if self.eat_op(op) {
                let op = match op {
                    "==" => "=",
                    "<>" => "!=",
                    o => o,
                };
                let op: &'static str = ["=", "!=", "<=", ">=", "<", ">"].into_iter().find(|o| *o == op).unwrap();
                return Ok(Expr::Bin(op, Box::new(l), Box::new(self.add()?)));
            }
        }
        if self.eat_word("in") {
            return Ok(Expr::Bin("in", Box::new(l), Box::new(self.add()?)));
        }
        if self.is_word("not") && matches!(self.t.get(self.i + 1), Some(T::Name(n)) if n.eq_ignore_ascii_case("in")) {
            self.i += 2;
            return Ok(Expr::Unary("not", Box::new(Expr::Bin("in", Box::new(l), Box::new(self.add()?)))));
        }
        Ok(l)
    }

    fn add(&mut self) -> Result<Expr, String> {
        let mut l = self.mul()?;
        loop {
            let op = if self.is_op("+") {
                "+"
            } else if self.is_op("-") {
                "-"
            } else {
                break;
            };
            self.i += 1;
            l = Expr::Bin(op, Box::new(l), Box::new(self.mul()?));
        }
        Ok(l)
    }

    fn mul(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        loop {
            let op = ["*", "/", "%"].into_iter().find(|o| self.is_op(o));
            let Some(op) = op else { break };
            self.i += 1;
            l = Expr::Bin(op, Box::new(l), Box::new(self.unary()?));
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat_op("-") {
            return Ok(Expr::Unary("-", Box::new(self.unary()?)));
        }
        let mut e = self.post()?;
        while self.no_pipe == 0 && self.eat_op("|") {
            e = Expr::Pipe(Box::new(e), self.conv()?);
        }
        Ok(e)
    }

    /// An operation's argument: up to the next "|".
    fn arg(&mut self) -> Result<Expr, String> {
        self.no_pipe += 1;
        let e = self.expr();
        self.no_pipe -= 1;
        e
    }

    /// An optional argument is there.
    fn arg_follows(&self) -> bool {
        match self.peek() {
            None => false,
            Some(T::Op(o)) => matches!(*o, "(" | "[" | "{" | "-"),
            Some(T::Name(n)) => !["and", "or", "step", "desc", "asc", "at", "by", "in"].contains(&n.to_ascii_lowercase().as_str()),
            Some(_) => true,
        }
    }

    /// "x => …" or "…" (with `it`).
    fn lambda(&mut self) -> Result<(String, Box<Expr>), String> {
        let name = match (self.peek(), self.t.get(self.i + 1)) {
            (Some(T::Name(n)), Some(T::Op("=>"))) => {
                let n = n.to_ascii_lowercase();
                self.i += 2;
                n
            }
            _ => "it".to_string(),
        };
        Ok((name, Box::new(self.arg()?)))
    }

    /// A loop variable's "as …": `as yyyymm` (text by a mask), `as date
    /// yyyymm`, `as string …`, `as number`.
    fn loop_as(&mut self) -> Result<Option<Conv>, String> {
        if !self.eat_word("as") {
            return Ok(None);
        }
        let word = match self.peek() {
            Some(T::Name(w)) => w.to_ascii_lowercase(),
            _ => return Err(format!("as what: a mask (as yyyymm), as date yyyymm, as number{}", self.here())),
        };
        Ok(Some(match word.as_str() {
            "date" | "string" | "str" | "text" | "number" | "num" | "int" => self.conv()?,
            _ => Conv::Str(Some(self.format(true).ok_or_else(|| format!("\"{word}\" isn't a mask (yyyy, mm, dd, hh24, mi, ss, q)"))?)),
        }))
    }

    fn conv(&mut self) -> Result<Conv, String> {
        let n = self.name()?.to_ascii_lowercase();
        Ok(match n.as_str() {
            "number" | "num" | "int" => Conv::Number,
            // A date (or a date's text) in a format: | string mmyyyy.
            "string" | "str" | "text" => Conv::Str(self.format(true)),
            "list" => Conv::List,
            "date" => Conv::Date(self.format(false)),
            "push" | "append" | "add" => {
                let x = self.arg()?;
                let at = if self.eat_word("at") { Some(Box::new(self.arg()?)) } else { None };
                Conv::Push(Box::new(x), at)
            }
            "pop" => Conv::Pop(if self.arg_follows() { Some(Box::new(self.arg()?)) } else { None }),
            "remove" => Conv::Remove(Box::new(self.arg()?)),
            "map" => {
                let (n, e) = self.lambda()?;
                Conv::Map(n, e)
            }
            "filter" | "where" => {
                let (n, e) = self.lambda()?;
                Conv::Filter(n, e)
            }
            "sort" => {
                let by = if self.eat_word("by") { Some(self.lambda()?) } else { None };
                let desc = self.eat_word("desc");
                if !desc {
                    self.eat_word("asc");
                }
                Conv::Sort { desc, by }
            }
            "reverse" => Conv::Reverse,
            "unique" | "distinct" => Conv::Unique,
            "join" => Conv::Join(if self.arg_follows() { Some(Box::new(self.arg()?)) } else { None }),
            "range" => {
                let is_unit = |p: &Self| matches!(p.peek(), Some(T::Name(w)) if unit(w).is_some());
                let n = if is_unit(self) || !self.arg_follows() { None } else { Some(Box::new(self.unary()?)) };
                let u = match self.peek() {
                    Some(T::Name(w)) if unit(w).is_some() => unit(w).unwrap(),
                    _ => return Err(format!("| range by what: month, day, hour… (| range month){}", self.here())),
                };
                self.i += 1;
                Conv::Range(n, u)
            }
            other => return Err(format!("no \"| {other}\" (number, string, list, date, push, pop, remove, map, filter, sort, reverse, unique, join, range)")),
        })
    }

    /// A date format after "| date" / "| string": the words, digits and
    /// marks up to what can't be one (an operator, "step", "..", the end).
    /// `strict`: only words made of format tokens (yyyy, mm…).
    fn format(&mut self, strict: bool) -> Option<String> {
        let mut f = String::new();
        if let Some(T::Str(s)) = self.peek() {
            f = s.clone();
            self.i += 1;
        } else {
            while let Some(T::Name(w)) = self.peek() {
                if ["step", "and", "or", "in", "not", "at", "by", "desc", "asc"].iter().any(|x| w.eq_ignore_ascii_case(x)) {
                    break;
                }
                if strict && fmt_tokens(w).iter().any(|t| matches!(t, Fmt::Lit(_))) {
                    break;
                }
                f += w;
                self.i += 1;
                // yyyy-mm-dd, dd.mm.yyyy, hh24:mi: marks glued between
                // format words.
                let mark = ["-", ".", "/", ":"].into_iter().find(|m| self.is_op(m));
                match mark {
                    Some(m) if self.fmt_glued() => {
                        f += m;
                        self.i += 1;
                    }
                    _ => break,
                }
            }
        }
        (!f.is_empty()).then_some(f)
    }

    /// The word after a "-" is a date format's piece.
    fn fmt_glued(&self) -> bool {
        matches!(self.t.get(self.i + 1), Some(T::Name(w)) if fmt_tokens(w).iter().all(|t| !matches!(t, Fmt::Lit(_))))
    }

    fn post(&mut self) -> Result<Expr, String> {
        let mut e = self.atom()?;
        loop {
            if self.is_op(".") && matches!(self.t.get(self.i + 1), Some(T::Name(_))) {
                self.i += 1;
                let n = self.name()?;
                e = Expr::Field(Box::new(e), n);
            } else if self.eat_op("[") {
                let k = self.expr()?;
                self.want_op("]")?;
                e = Expr::Index(Box::new(e), Box::new(k));
            } else {
                return Ok(e);
            }
        }
    }

    fn atom(&mut self) -> Result<Expr, String> {
        // In brackets (a call's too) pipes are its own again.
        let saved = self.no_pipe;
        if matches!(self.peek(), Some(T::Op("(" | "[" | "{"))) || matches!((self.peek(), self.t.get(self.i + 1)), (Some(T::Name(_)), Some(T::Op("(")))) {
            self.no_pipe = 0;
        }
        let r = self.atom_in();
        self.no_pipe = saved;
        r
    }

    fn atom_in(&mut self) -> Result<Expr, String> {
        let Some(t) = self.peek().cloned() else { return Err("a value expected at the end".into()) };
        self.i += 1;
        Ok(match t {
            T::Num(n) => {
                if n.contains('.') {
                    Expr::Lit(Value::Num(n.parse().map_err(|_| format!("bad number {n}"))?))
                } else {
                    let v: i64 = n.parse().map_err(|_| format!("bad number {n}"))?;
                    if n.len() > 1 && n.starts_with('0') { Expr::Lit(Value::Pad(v, n.len() as u8)) } else { Expr::Lit(Value::Int(v)) }
                }
            }
            T::Str(s) => Expr::Lit(Value::Str(s)),
            T::Tpl(s) => Expr::Template(template(&s)?),
            T::Query(q) => Expr::Query(q),
            T::Name(n) => match n.to_ascii_lowercase().as_str() {
                "true" => Expr::Lit(Value::Bool(true)),
                "false" => Expr::Lit(Value::Bool(false)),
                "null" => Expr::Lit(Value::Null),
                _ if self.is_op("(") => {
                    self.i += 1;
                    let mut args = Vec::new();
                    if !self.eat_op(")") {
                        loop {
                            args.push(self.expr()?);
                            if self.eat_op(")") {
                                break;
                            }
                            self.want_op(",")?;
                        }
                    }
                    Expr::Call(n.to_ascii_lowercase(), args)
                }
                _ => Expr::Var(n.to_ascii_lowercase()),
            },
            T::Op("(") => {
                let e = self.expr()?;
                if self.eat_op("..") {
                    let r = Expr::Range(Box::new(steps(self, e)?));
                    self.want_op(")")?;
                    return Ok(r);
                }
                if self.eat_op(",") {
                    let mut items = vec![e];
                    if !self.is_op(")") {
                        loop {
                            items.push(self.expr()?);
                            if !self.eat_op(",") || self.is_op(")") {
                                break;
                            }
                        }
                    }
                    self.want_op(")")?;
                    Expr::List(items)
                } else {
                    self.want_op(")")?;
                    e
                }
            }
            T::Op("[") => Expr::List(self.items("]")?),
            T::Op("{") => {
                if self.eat_op("}") {
                    return Ok(Expr::Dict(Vec::new()));
                }
                // {k: v, …} a dictionary, {a, b} a list.
                let is_dict = matches!(self.t.get(self.i + 1), Some(T::Op(":")));
                if !is_dict {
                    return Ok(Expr::List(self.items("}")?));
                }
                let mut pairs = Vec::new();
                loop {
                    let k = match self.peek().cloned() {
                        Some(T::Str(s)) | Some(T::Name(s)) => s,
                        Some(T::Num(s)) => s,
                        _ => return Err(format!("a key expected{}", self.here())),
                    };
                    self.i += 1;
                    self.want_op(":")?;
                    pairs.push((k, self.expr()?));
                    if self.eat_op("}") {
                        break;
                    }
                    self.want_op(",")?;
                    if self.eat_op("}") {
                        break;
                    }
                }
                Expr::Dict(pairs)
            }
            T::Op(o) => return Err(format!("\"{o}\" isn't expected here")),
        })
    }

    fn items(&mut self, close: &str) -> Result<Vec<Expr>, String> {
        let mut items = Vec::new();
        if self.eat_op(close) {
            return Ok(items);
        }
        loop {
            items.push(self.expr()?);
            if self.eat_op(close) {
                return Ok(items);
            }
            self.want_op(",")?;
            if self.eat_op(close) {
                return Ok(items);
            }
        }
    }
}

/// A `…${x}…` string's pieces.
fn template(s: &str) -> Result<Vec<Piece>, String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(k) = rest.find("${") {
        if k > 0 {
            out.push(Piece::Text(rest[..k].to_string()));
        }
        let end = rest[k..].find('}').ok_or("\"${\" isn't closed with \"}\"")? + k;
        out.push(Piece::Expr(parse_expr(&rest[k + 2..end])?));
        rest = &rest[end + 1..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest.to_string()));
    }
    Ok(out)
}

/// One whole expression.
pub fn parse_expr(src: &str) -> Result<Expr, String> {
    let mut p = P::new(src)?;
    let e = p.expr()?;
    p.done()?;
    Ok(e)
}

// ───────────────────────────── the program ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Steps {
    pub from: Expr,
    pub to: Expr,
    pub conv: Option<Conv>,
    /// How many, of what (None: by the ends' kind).
    pub step: Option<(Option<Expr>, Option<Prec>)>,
    /// Operations on the range's list (| filter …, | map …).
    pub after: Vec<Conv>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Range(Steps),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Case {
    Range(Steps),
    Values(Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Sql(Stmt),
    Set { line: usize, name: String, op: &'static str, value: Option<Expr> },
    For { line: usize, names: Vec<String>, source: Source, body: Vec<Node> },
    While { line: usize, cond: Expr, body: Vec<Node> },
    If { line: usize, arms: Vec<(Expr, Vec<Node>)>, other: Vec<Node> },
    Match { line: usize, subject: Expr, cases: Vec<(Case, Vec<Node>)>, other: Vec<Node> },
    Break(usize),
    Continue(usize),
    Print { line: usize, args: Vec<Expr> },
    Pause(usize, Duration),
    OnError(bool),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    pub nodes: Vec<Node>,
}

/// A directive line, as read.
#[derive(Debug)]
enum Line {
    Node(Node),
    For(Vec<String>, Source),
    While(Expr),
    If(Expr),
    Elif(Expr),
    Else,
    Match(Expr),
    Case(Case),
    Default,
    End(Option<&'static str>),
}

const WORDS: [&str; 18] = ["set", "var", "for", "while", "if", "elif", "else", "match", "case", "default", "end", "break", "continue", "print", "pause", "on", "echo", "endfor"];

/// The directive in a comment's text (`-- @…`), if it is one: its text
/// after the "@" (the trailing " -- comment" cut).
fn directive_text(comment: &str) -> Option<&str> {
    let body = comment.strip_prefix("--")?.trim_start();
    let body = body.strip_prefix('@')?;
    let word: String = body.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    if word.is_empty() {
        return None;
    }
    let low = word.to_ascii_lowercase();
    let after = body[word.len()..].trim_start();
    let known = WORDS.contains(&low.as_str()) || ["=", "+=", "-=", "*=", "/=", "|=", "++", "--"].iter().any(|o| after.starts_with(o));
    if !known {
        return None;
    }
    Some(cut_comment(body).trim())
}

/// The text up to a " -- " outside strings.
fn cut_comment(s: &str) -> &str {
    let mut quote: Option<char> = None;
    let b = s.as_bytes();
    for (k, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' || c == '`' => quote = Some(c),
            None if c == '-' && b.get(k + 1) == Some(&b'-') && (k == 0 || b[k - 1] == b' ' || b[k - 1] == b'\t') => {
                // "@x--" is a decrement, not a comment.
                let rest = s[k + 2..].trim();
                if !(rest.is_empty() && k > 0 && b[k - 1] != b' ') {
                    return &s[..k];
                }
            }
            _ => {}
        }
    }
    s
}

/// The directives in a text: (the comment's range, the directive's text).
fn directives(text: &str, d: Dialect) -> Vec<(Range<usize>, String)> {
    lex::lex(text, d)
        .into_iter()
        .filter(|t| t.kind == Tk::LineComment)
        .filter(|t| {
            // Alone on its line.
            let line_start = text[..t.start].rfind('\n').map_or(0, |k| k + 1);
            text[line_start..t.start].trim().is_empty()
        })
        .filter_map(|t| directive_text(t.text(text)).map(|s| (t.start..t.end, s.to_string())))
        .collect()
}

/// The text holds directives (then it's run as a script).
pub fn has_directives(text: &str, d: Dialect) -> bool {
    text.contains('@') && !directives(text, d).is_empty()
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at].matches('\n').count() + 1
}

// ───────────────────────────── colours ─────────────────────────────

/// What a piece of a directive line is, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    /// The "--" that keeps it a comment for the database.
    Mark,
    /// The directive's own words, by what it does: loops, conditions,
    /// variables, the rest (print, pause, on error).
    Loop,
    Cond,
    Var,
    Misc,
    /// A variable.
    Name,
    Func,
    Str,
    Num,
    Op,
    /// A query's SQL (inside its brackets).
    Query,
    /// " -- what it's for" after it.
    Comment,
}

/// Functions a directive's expression may call.
pub const FUNCTIONS: [&str; 5] = ["len", "upper", "lower", "keys", "values"];

/// The colour a directive's word gives its line (and an "@end"'s word).
fn kind_of(word: &str) -> Paint {
    match word {
        "for" | "while" | "endfor" | "break" | "continue" => Paint::Loop,
        "if" | "elif" | "else" | "match" | "case" | "default" => Paint::Cond,
        "set" | "var" => Paint::Var,
        "print" | "echo" | "pause" | "on" => Paint::Misc,
        _ => Paint::Var,
    }
}

/// Whether the comment `t` of `text` is alone on its line.
pub fn alone(text: &str, at: usize) -> bool {
    let ls = text[..at].rfind('\n').map_or(0, |k| k + 1);
    text[ls..at].trim().is_empty()
}

/// The pieces of a line comment that is a directive (`comment`: its text,
/// alone on its line), by their bytes in it; None when it isn't one.
/// `open`: the blocks open above it (a bare "@end" takes its block's
/// colour), kept up to date.
pub fn paint(comment: &str, open: &mut Vec<Paint>) -> Option<Vec<(Range<usize>, Paint)>> {
    directive_text(comment)?;
    let at = comment.find('@')?;
    let body = &comment[at + 1..];
    let word: String = body.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    let low = word.to_ascii_lowercase();
    let mut out = vec![(0..2, Paint::Mark)];
    let w_end = at + 1 + word.len();
    let known = WORDS.contains(&low.as_str());
    let mut kind = kind_of(&low);
    // The "@word" (and "@end for", "@else if"): in the block's colour.
    let mut head_end = w_end;
    let next: String = comment[w_end..].trim_start().chars().take_while(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let take_next = |head_end: &mut usize| *head_end = w_end + (comment[w_end..].len() - comment[w_end..].trim_start().len()) + next.len();
    match low.as_str() {
        "end" => {
            let top = open.pop();
            kind = match next.as_str() {
                "for" | "while" => Paint::Loop,
                "if" | "match" => Paint::Cond,
                _ => top.unwrap_or(Paint::Misc),
            };
            if matches!(next.as_str(), "for" | "while" | "if" | "match") {
                take_next(&mut head_end);
            }
        }
        "endfor" => {
            open.pop();
        }
        "else" if next == "if" => take_next(&mut head_end),
        "on" if next == "error" => take_next(&mut head_end),
        "for" | "while" => open.push(Paint::Loop),
        "if" | "match" => open.push(Paint::Cond),
        _ => {}
    }
    if known {
        out.push((at..head_end, kind));
    } else {
        // name = …: the "@" in the variables' colour, the name a variable.
        out.push((at..at + 1, Paint::Var));
        out.push((at + 1..w_end, Paint::Name));
    }
    // The rest, up to a " -- comment".
    let rest_end = at + 1 + cut_comment(body).len();
    let rest_end = rest_end.max(head_end);
    let s = comment;
    let b = s.as_bytes();
    let mut i = head_end;
    // After "| date": its format, up to a space.
    let mut fmt_next = false;
    let mut prev_pipe = false;
    while i < rest_end {
        let c = s[i..].chars().next().unwrap();
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let start = i;
        let next_word: String = s[i..rest_end].chars().take_while(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if fmt_next && c != '\'' && c != '"' && !["step", "and", "or", "in", "not", "at", "by", "desc", "asc"].contains(&next_word.as_str()) {
            while i < rest_end && !s[i..].starts_with(char::is_whitespace) {
                i += s[i..].chars().next().unwrap().len_utf8();
            }
            out.push((start..i, Paint::Str));
            fmt_next = false;
            continue;
        }
        fmt_next = false;
        if low == "pause" {
            out.push((start..rest_end, Paint::Num));
            break;
        }
        if c.is_ascii_digit() {
            while i < rest_end && (b[i].is_ascii_alphanumeric() || b[i] == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
                i += 1;
            }
            out.push((start..i, Paint::Num));
        } else if c.is_alphabetic() || c == '_' {
            while i < rest_end && s[i..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                i += s[i..].chars().next().unwrap().len_utf8();
            }
            let w = s[start..i].to_ascii_lowercase();
            let call = s[i..rest_end].trim_start().starts_with('(');
            let p = if prev_pipe {
                fmt_next = matches!(w.as_str(), "date" | "string" | "str" | "text");
                kind
            } else if call && FUNCTIONS.contains(&w.as_str()) {
                Paint::Func
            } else if w == "as" && low == "for" {
                // "as yyyymm": the mask; "as date yyyymm": as after a pipe.
                let nxt: String = s[i..rest_end].trim_start().chars().take_while(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase();
                if matches!(nxt.as_str(), "date" | "string" | "str" | "text" | "number" | "num" | "int") {
                    out.push((start..i, kind));
                    prev_pipe = true;
                    continue;
                }
                fmt_next = true;
                kind
            } else if matches!(w.as_str(), "in" | "step" | "and" | "or" | "not" | "true" | "false" | "null" | "error" | "continue" | "stop" | "at" | "by" | "desc" | "asc") || (unit(&w).is_some() && out.iter().any(|(r, _)| s[r.clone()].eq_ignore_ascii_case("step") || s[r.clone()].eq_ignore_ascii_case("range"))) {
                kind
            } else {
                Paint::Name
            };
            out.push((start..i, p));
        } else if c == '\'' || c == '"' || c == '`' {
            i += 1;
            while i < rest_end {
                let ch = s[i..].chars().next().unwrap();
                i += ch.len_utf8();
                if ch == '\\' {
                    i = (i + s[i..].chars().next().map_or(0, char::len_utf8)).min(rest_end);
                } else if ch == c {
                    break;
                }
            }
            out.push((start..i, Paint::Str));
        } else if c == '(' && {
            let w: String = s[i + 1..].trim_start().chars().take_while(|c| c.is_alphabetic()).collect::<String>().to_ascii_lowercase();
            w == "select" || w == "with"
        } {
            let end = closing(&s[..rest_end], i).unwrap_or(rest_end);
            out.push((i..i + 1, Paint::Op));
            out.push((i + 1..end, Paint::Query));
            if end < rest_end {
                out.push((end..end + 1, Paint::Op));
            }
            i = (end + 1).min(rest_end);
        } else {
            let op = OPS.iter().chain(["{", "}", ":", "$"].iter()).find(|o| s[i..].starts_with(**o)).map_or(c.len_utf8(), |o| o.len());
            i += op;
            out.push((start..i, Paint::Op));
            prev_pipe = matches!(&s[start..i], "|" | "|=");
            continue;
        }
        prev_pipe = false;
    }
    if rest_end < s.len() {
        let c = rest_end + (s[rest_end..].len() - s[rest_end..].trim_start().len());
        if c < s.len() {
            out.push((c..s.len(), Paint::Comment));
        }
    }
    Some(out)
}

/// How a directive line shapes the text's blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// @for, @while, @if, @match: a block starts.
    Open,
    /// @elif, @else, @case, @default: a block's next part.
    Mid,
    /// @end…: the block ends.
    Close,
    Plain,
}

/// The shape of a line comment's directive (None: not one).
pub fn shape(comment: &str) -> Option<Shape> {
    let body = directive_text(comment)?;
    let word: String = body.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<String>().to_ascii_lowercase();
    Some(match word.as_str() {
        "for" | "while" | "if" | "match" => Shape::Open,
        "elif" | "else" | "case" | "default" => Shape::Mid,
        "end" | "endfor" => Shape::Close,
        _ => Shape::Plain,
    })
}

/// The variables a script's directives make (@set, @var, @for, name = …),
/// for hints.
pub fn var_names(text: &str, d: Dialect) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, s) in directives(text, d) {
        let words: Vec<&str> = s.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
        let Some(first) = words.first() else { continue };
        let low = first.to_ascii_lowercase();
        let names: Vec<&str> = match low.as_str() {
            "set" | "var" => words.get(1).into_iter().copied().collect(),
            "for" => {
                let low = s.to_ascii_lowercase();
                let before_in = [low.find(" in "), low.find(" as ")].into_iter().flatten().min().unwrap_or(s.len());
                s[3.min(s.len())..before_in].split(',').map(str::trim).filter(|w| !w.is_empty()).collect()
            }
            w if !WORDS.contains(&w) => vec![*first],
            _ => Vec::new(),
        };
        for n in names {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// Where `&x`, `&{…}` and `:{…}` stand in a script's SQL (not in
/// comments: `in_comment` says where those are), to be coloured as
/// parameters (`:x` the lexer finds itself).
pub fn marks(sql: &str, in_comment: &dyn Fn(usize) -> bool) -> Vec<Range<usize>> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if (c == b'&' || c == b':') && !in_comment(i) {
            if b.get(i + 1) == Some(&b'{') && !(c == b':' && i > 0 && b[i - 1] == b':') {
                if let Some(end) = sql[i + 1..].find('}').map(|k| i + 1 + k + 1) {
                    if !sql[i..end].contains('\n') {
                        out.push(i..end);
                        i = end;
                        continue;
                    }
                }
            } else if c == b'&' && b.get(i + 1).is_some_and(|x| x.is_ascii_alphabetic() || *x == b'_') {
                let mut j = i + 1;
                loop {
                    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                        j += 1;
                    }
                    // .field, or the "." that ends the name.
                    if b.get(j) == Some(&b'.') {
                        j += 1;
                        if b.get(j).is_some_and(|x| x.is_ascii_alphabetic() || *x == b'_') {
                            continue;
                        }
                    }
                    break;
                }
                out.push(i..j);
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn steps(p: &mut P, from: Expr) -> Result<Steps, String> {
    let mut to = p.unary()?;
    // "a .. b | filter …": on the list; peeled off the end.
    let mut after = Vec::new();
    while let Expr::Pipe(t, c) = to {
        if c.on_items() {
            after.insert(0, c);
            to = *t;
        } else {
            to = Expr::Pipe(t, c);
            break;
        }
    }
    let mut conv = None;
    let (from, to) = match (from, to) {
        // "a .. b | date f": the pipe reads both ends.
        (f, Expr::Pipe(t, c)) if !matches!(f, Expr::Pipe(..)) => {
            conv = Some(c);
            (f, *t)
        }
        (f, t) => (f, t),
    };
    let mut step = None;
    if p.eat_word("step") {
        let n = if matches!(p.peek(), Some(T::Name(w)) if unit(w).is_some()) { None } else { Some(p.unary()?) };
        let u = match p.peek() {
            Some(T::Name(w)) if unit(w).is_some() => {
                let u = unit(w);
                p.i += 1;
                u
            }
            _ => None,
        };
        step = Some((n, u));
    }
    while p.no_pipe == 0 && p.eat_op("|") {
        after.push(p.conv()?);
    }
    Ok(Steps { from, to, conv, step, after })
}

/// A conversion done on each item: `| map (it | conv)`.
fn as_map(c: Conv) -> Conv {
    Conv::Map("it".into(), Box::new(Expr::Pipe(Box::new(Expr::Var("it".into())), c)))
}

fn unit(w: &str) -> Option<Prec> {
    Some(match w.to_ascii_lowercase().trim_end_matches('s') {
        "second" | "sec" => Prec::Second,
        "minute" | "min" => Prec::Minute,
        "hour" => Prec::Hour,
        "day" => Prec::Day,
        "week" => Prec::Week,
        "month" => Prec::Month,
        "quarter" => Prec::Quarter,
        "year" => Prec::Year,
        _ => return None,
    })
}

fn duration(s: &str) -> Result<Duration, String> {
    let s = s.trim().to_ascii_lowercase();
    let bad = || format!("\"{s}\": a pause is like 10s, 500ms, 2m, 1h");
    let (n, k) = if let Some(n) = s.strip_suffix("ms") {
        (n, 0.001)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1.0)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60.0)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3600.0)
    } else {
        (s.as_str(), 1.0)
    };
    let n: f64 = n.trim().parse().map_err(|_| bad())?;
    if !(n >= 0.0) {
        return Err(bad());
    }
    Ok(Duration::from_secs_f64(n * k))
}

/// One directive's text read.
fn read_line(src: &str, line: usize) -> Result<Line, String> {
    let mut p = P::new(src)?;
    let word = p.name()?;
    let low = word.to_ascii_lowercase();
    let out = match low.as_str() {
        "set" | "var" => {
            let name = p.name()?.to_ascii_lowercase();
            p.want_op("=")?;
            let value = p.list_or_expr()?;
            Line::Node(Node::Set { line, name, op: "=", value: Some(value) })
        }
        "for" => {
            let mut names = vec![p.name()?.to_ascii_lowercase()];
            if p.eat_op(",") {
                names.push(p.name()?.to_ascii_lowercase());
            }
            let each = p.loop_as()?;
            if !p.eat_word("in") {
                return Err(format!("\"in\" expected{}", p.here()));
            }
            let first = p.unary()?;
            let source = if p.eat_op("..") {
                Source::Range(steps(&mut p, first)?)
            } else if p.is_op(",") {
                let mut items = vec![first];
                while p.eat_op(",") {
                    items.push(p.expr()?);
                }
                Source::Expr(Expr::List(items))
            } else {
                // The rest of an expression after its first part.
                p.i = 0;
                p.name()?;
                p.name()?;
                if p.eat_op(",") {
                    p.name()?;
                }
                p.loop_as()?;
                p.eat_word("in");
                Source::Expr(p.list_or_expr()?)
            };
            // "as …": each item converted (as a map after the list).
            let source = match (source, each) {
                (s, None) => s,
                (Source::Range(mut st), Some(c)) => {
                    st.after.push(as_map(c));
                    Source::Range(st)
                }
                (Source::Expr(e), Some(c)) => Source::Expr(Expr::Pipe(Box::new(e), as_map(c))),
            };
            Line::For(names, source)
        }
        "while" => Line::While(p.expr()?),
        "if" => Line::If(p.expr()?),
        "elif" => Line::Elif(p.expr()?),
        "else" if p.eat_word("if") => Line::Elif(p.expr()?),
        "else" => Line::Else,
        "match" => Line::Match(p.expr()?),
        "case" => {
            let first = p.unary()?;
            if p.eat_op("..") {
                Line::Case(Case::Range(steps(&mut p, first)?))
            } else {
                let mut items = vec![first];
                while p.eat_op(",") {
                    items.push(p.expr()?);
                }
                Line::Case(Case::Values(items))
            }
        }
        "default" => Line::Default,
        "end" | "endfor" => {
            let what = if low == "endfor" {
                Some("for")
            } else {
                match p.peek() {
                    Some(T::Name(w)) => {
                        let w = w.to_ascii_lowercase();
                        let k = ["for", "while", "if", "match"].into_iter().find(|k| *k == w).ok_or(format!("\"@end {w}\": for, while, if or match"))?;
                        p.i += 1;
                        Some(k)
                    }
                    _ => None,
                }
            };
            Line::End(what)
        }
        "break" => Line::Node(Node::Break(line)),
        "continue" => Line::Node(Node::Continue(line)),
        "print" | "echo" => {
            let args = if p.eat_op("(") {
                p.items(")")?
            } else if p.peek().is_none() {
                Vec::new()
            } else {
                match p.list_or_expr()? {
                    Expr::List(items) => items,
                    e => vec![e],
                }
            };
            Line::Node(Node::Print { line, args })
        }
        "pause" => {
            let rest = src.trim_start()[word.len()..].trim();
            p.i = p.t.len();
            Line::Node(Node::Pause(line, duration(rest)?))
        }
        "on" => {
            if !p.eat_word("error") {
                return Err("\"@on error stop\" or \"@on error continue\"".into());
            }
            let go_on = if p.eat_word("continue") {
                true
            } else if p.eat_word("stop") {
                false
            } else {
                return Err("\"@on error stop\" or \"@on error continue\"".into());
            };
            Line::Node(Node::OnError(go_on))
        }
        _ => {
            // name = …, name += …, name++, name--.
            let name = low;
            let op = ["=", "+=", "-=", "*=", "/=", "|=", "++", "--"].into_iter().find(|o| p.is_op(o)).ok_or(format!("no directive \"@{word}\""))?;
            p.i += 1;
            if op == "|=" {
                // x |= pop: x = x | pop.
                let mut e = Expr::Pipe(Box::new(Expr::Var(name.clone())), p.conv()?);
                while p.eat_op("|") {
                    e = Expr::Pipe(Box::new(e), p.conv()?);
                }
                Line::Node(Node::Set { line, name, op: "=", value: Some(e) })
            } else {
                let value = if op == "++" || op == "--" { None } else { Some(p.list_or_expr()?) };
                Line::Node(Node::Set { line, name, op, value })
            }
        }
    };
    p.done()?;
    Ok(out)
}

/// An open block while reading.
enum Open {
    For { line: usize, names: Vec<String>, source: Source },
    While { line: usize, cond: Expr },
    If { line: usize, arms: Vec<(Expr, Vec<Node>)>, cond: Option<Expr> },
    Match { line: usize, subject: Expr, cases: Vec<(Case, Vec<Node>)>, case: Option<Case>, default: bool },
}

impl Open {
    fn word(&self) -> &'static str {
        match self {
            Open::For { .. } => "for",
            Open::While { .. } => "while",
            Open::If { .. } => "if",
            Open::Match { .. } => "match",
        }
    }

    fn line(&self) -> usize {
        match self {
            Open::For { line, .. } | Open::While { line, .. } | Open::If { line, .. } | Open::Match { line, .. } => *line,
        }
    }
}

/// A mistake in a script: its line (in the editor's text) and what.
#[derive(Clone, Debug, PartialEq)]
pub struct Mistake {
    pub line: usize,
    pub msg: String,
}

/// The script read through: its SQL (split as a script is) and directives.
/// `base`: where `text` starts in the editor's text (`full`).
pub fn parse(full: &str, base: usize, text: &str, d: Dialect, blank_lines: bool) -> Result<Program, Mistake> {
    let found = directives(text, d);
    // Each level: (the open block, nodes so far).
    let mut stack: Vec<(Option<Open>, Vec<Node>)> = vec![(None, Vec::new())];
    let mut from = 0;
    let sql_part = |stack: &mut Vec<(Option<Open>, Vec<Node>)>, a: usize, b: usize| {
        let part = &text[a..b];
        for r in crate::sql::split(part, d, blank_lines) {
            let sql = crate::sql::runnable(&part[r.clone()], d);
            if !sql.is_empty() {
                stack.last_mut().unwrap().1.push(Node::Sql(Stmt { sql, at: base + a + r.start }));
            }
        }
    };
    for (r, src) in found {
        sql_part(&mut stack, from, r.start);
        from = r.end;
        let line = line_of(full, base + r.start);
        let wrong = |msg: String| Mistake { line, msg };
        match read_line(&src, line).map_err(|m| wrong(format!("@{src}: {m}")))? {
            Line::Node(n) => stack.last_mut().unwrap().1.push(n),
            Line::For(names, source) => stack.push((Some(Open::For { line, names, source }), Vec::new())),
            Line::While(cond) => stack.push((Some(Open::While { line, cond }), Vec::new())),
            Line::If(cond) => stack.push((Some(Open::If { line, arms: Vec::new(), cond: Some(cond) }), Vec::new())),
            Line::Match(subject) => stack.push((Some(Open::Match { line, subject, cases: Vec::new(), case: None, default: false }), Vec::new())),
            Line::Elif(_) | Line::Else if !matches!(stack.last().unwrap().0, Some(Open::If { .. })) => {
                return Err(wrong("@elif / @else outside @if".into()));
            }
            Line::Elif(c) => {
                let (open, body) = stack.last_mut().unwrap();
                let Some(Open::If { arms, cond, .. }) = open else { unreachable!() };
                let Some(prev) = cond.take() else { return Err(wrong("@elif after @else".into())) };
                arms.push((prev, std::mem::take(body)));
                *cond = Some(c);
            }
            Line::Else => {
                let (open, body) = stack.last_mut().unwrap();
                let Some(Open::If { arms, cond, .. }) = open else { unreachable!() };
                let Some(prev) = cond.take() else { return Err(wrong("a second @else".into())) };
                arms.push((prev, std::mem::take(body)));
            }
            Line::Case(c) => {
                let (open, body) = stack.last_mut().unwrap();
                let Some(Open::Match { cases, case, default, .. }) = open else { return Err(wrong("@case outside @match".into())) };
                if *default {
                    return Err(wrong("@case after @default".into()));
                }
                if let Some(prev) = case.take() {
                    cases.push((prev, std::mem::take(body)));
                } else if !body.is_empty() {
                    return Err(wrong("between @match and its first @case there's nothing to put".into()));
                }
                *case = Some(c);
            }
            Line::Default => {
                let (open, body) = stack.last_mut().unwrap();
                let Some(Open::Match { cases, case, default, .. }) = open else { return Err(wrong("@default outside @match".into())) };
                if *default {
                    return Err(wrong("a second @default".into()));
                }
                if let Some(prev) = case.take() {
                    cases.push((prev, std::mem::take(body)));
                }
                *default = true;
            }
            Line::End(what) => {
                if stack.len() == 1 {
                    return Err(wrong("@end with nothing open".into()));
                }
                let (open, body) = stack.pop().unwrap();
                let open = open.unwrap();
                if let Some(w) = what
                    && w != open.word()
                {
                    return Err(wrong(format!("@end {w} closes the @{} of line {}", open.word(), open.line())));
                }
                let node = match open {
                    Open::For { line, names, source } => Node::For { line, names, source, body },
                    Open::While { line, cond } => Node::While { line, cond, body },
                    Open::If { line, mut arms, cond } => {
                        let other = match cond {
                            Some(c) => {
                                arms.push((c, body));
                                Vec::new()
                            }
                            None => body,
                        };
                        Node::If { line, arms, other }
                    }
                    Open::Match { line, subject, mut cases, case, default } => {
                        let other = if default {
                            body
                        } else {
                            if let Some(c) = case {
                                cases.push((c, body));
                            }
                            Vec::new()
                        };
                        Node::Match { line, subject, cases, other }
                    }
                };
                stack.last_mut().unwrap().1.push(node);
            }
        }
    }
    sql_part(&mut stack, from, text.len());
    if stack.len() > 1 {
        let (open, _) = stack.pop().unwrap();
        let open = open.unwrap();
        return Err(Mistake { line: open.line(), msg: format!("@{} isn't closed with @end", open.word()) });
    }
    Ok(Program { nodes: stack.pop().unwrap().1 })
}

impl Program {
    /// The names its directives give values to (they aren't asked for).
    pub fn names(&self) -> Vec<String> {
        fn walk(nodes: &[Node], out: &mut Vec<String>) {
            for n in nodes {
                match n {
                    Node::Set { name, .. } => out.push(name.clone()),
                    Node::For { names, body, .. } => {
                        out.extend(names.iter().cloned());
                        walk(body, out);
                    }
                    Node::While { body, .. } => walk(body, out),
                    Node::If { arms, other, .. } => {
                        arms.iter().for_each(|(_, b)| walk(b, out));
                        walk(other, out);
                    }
                    Node::Match { cases, other, .. } => {
                        cases.iter().for_each(|(_, b)| walk(b, out));
                        walk(other, out);
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, &mut out);
        out.sort();
        out.dedup();
        out
    }

    /// Every SQL statement in it, in order (to look over or change).
    pub fn sql_mut(&mut self) -> Vec<&mut Stmt> {
        fn walk<'a>(nodes: &'a mut [Node], out: &mut Vec<&'a mut Stmt>) {
            for n in nodes {
                match n {
                    Node::Sql(s) => out.push(s),
                    Node::For { body, .. } | Node::While { body, .. } => walk(body, out),
                    Node::If { arms, other, .. } => {
                        for (_, b) in arms.iter_mut() {
                            walk(b, out);
                        }
                        walk(other, out);
                    }
                    Node::Match { cases, other, .. } => {
                        for (_, b) in cases.iter_mut() {
                            walk(b, out);
                        }
                        walk(other, out);
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&mut self.nodes, &mut out);
        out
    }
}

// ───────────────────────────── running ─────────────────────────────

/// What a statement came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlEnd {
    Ok,
    Failed,
    /// Stopped, or the session's gone: the run ends.
    Over,
}

/// Where the script's statements and queries run.
pub trait Host {
    fn sql(&mut self, st: Stmt) -> SqlEnd;
    /// A query's whole answer (for a value).
    fn query(&mut self, sql: &str) -> Result<Table, String>;
    fn print(&mut self, text: String);
    /// Where it's got to ("7/16 · i = 06").
    fn progress(&mut self, text: String);
    fn stopped(&self) -> bool;
}

/// The variables: the script's own and, inside loops, the loops'.
#[derive(Clone, Debug, Default)]
pub struct Vars {
    pub globals: HashMap<String, Value>,
    loops: Vec<(String, Value)>,
}

impl Vars {
    pub fn get(&self, name: &str) -> Option<&Value> {
        let name = name.to_ascii_lowercase();
        self.loops.iter().rev().find(|(n, _)| *n == name).map(|(_, v)| v).or_else(|| self.globals.get(&name))
    }

    fn set(&mut self, name: &str, v: Value) {
        if let Some(slot) = self.loops.iter_mut().rev().find(|(n, _)| n == name) {
            slot.1 = v;
        } else {
            self.globals.insert(name.to_string(), v);
        }
    }
}

/// How a run ended (Err: a mistake in a directive, at its line).
#[derive(Debug, PartialEq)]
pub enum End {
    Done,
    /// A statement failed and the script stopped there.
    Failed,
    Over,
}

enum Flow {
    Next,
    Break,
    Continue,
    Stop(End),
}

/// Passes without any SQL before a loop's taken for runaway.
const RUNAWAY: u64 = 100_000;

struct Run<'h> {
    host: &'h mut dyn Host,
    d: Dialect,
    go_on: bool,
    idle: u64,
}

pub fn run(prog: &Program, vars: &mut Vars, d: Dialect, go_on: bool, host: &mut dyn Host) -> Result<End, Mistake> {
    let mut r = Run { host, d, go_on, idle: 0 };
    match r.block(&prog.nodes, vars)? {
        Flow::Stop(e) => Ok(e),
        _ => Ok(End::Done),
    }
}

impl Run<'_> {
    fn block(&mut self, nodes: &[Node], vars: &mut Vars) -> Result<Flow, Mistake> {
        for n in nodes {
            if self.host.stopped() {
                return Ok(Flow::Stop(End::Over));
            }
            match self.node(n, vars)? {
                Flow::Next => {}
                f => return Ok(f),
            }
        }
        Ok(Flow::Next)
    }

    fn eval(&mut self, e: &Expr, vars: &Vars, line: usize) -> Result<Value, Mistake> {
        eval(e, vars, self.d, &mut *self.host).map_err(|msg| Mistake { line, msg })
    }

    fn node(&mut self, n: &Node, vars: &mut Vars) -> Result<Flow, Mistake> {
        match n {
            Node::Sql(st) => {
                self.idle = 0;
                let sql = put(&st.sql, vars, self.d).map_err(|msg| Mistake { line: 0, msg })?;
                match self.host.sql(Stmt { sql, at: st.at }) {
                    SqlEnd::Ok => {}
                    SqlEnd::Failed if self.go_on => {}
                    SqlEnd::Failed => return Ok(Flow::Stop(End::Failed)),
                    SqlEnd::Over => return Ok(Flow::Stop(End::Over)),
                }
            }
            Node::Set { line, name, op, value } => {
                let v = match value {
                    Some(e) => self.eval(e, vars, *line)?,
                    None => Value::Int(1),
                };
                let v = if *op == "=" {
                    v
                } else {
                    let old = vars.get(name).cloned().ok_or(Mistake { line: *line, msg: format!("\"{name}\" has no value yet") })?;
                    let bin = match *op {
                        "+=" | "++" => "+",
                        "-=" | "--" => "-",
                        "*=" => "*",
                        _ => "/",
                    };
                    binary(bin, old, v).map_err(|msg| Mistake { line: *line, msg })?
                };
                vars.set(name, v);
            }
            Node::Print { line, args } => {
                let mut s = String::new();
                for a in args {
                    s += &self.eval(a, vars, *line)?.shown();
                }
                self.host.print(s);
            }
            Node::Pause(_, d) => {
                let until = std::time::Instant::now() + *d;
                while std::time::Instant::now() < until {
                    if self.host.stopped() {
                        return Ok(Flow::Stop(End::Over));
                    }
                    std::thread::sleep((until - std::time::Instant::now()).min(Duration::from_millis(50)));
                }
            }
            Node::OnError(go_on) => self.go_on = *go_on,
            Node::Break(_) => return Ok(Flow::Break),
            Node::Continue(_) => return Ok(Flow::Continue),
            Node::If { line, arms, other } => {
                for (c, body) in arms {
                    if self.eval(c, vars, *line)?.truthy() {
                        return self.block(body, vars);
                    }
                }
                return self.block(other, vars);
            }
            Node::Match { line, subject, cases, other } => {
                let v = self.eval(subject, vars, *line)?;
                for (case, body) in cases {
                    let fits = match case {
                        Case::Values(items) => {
                            let mut any = false;
                            for e in items {
                                let w = self.eval(e, vars, *line)?;
                                any |= match &w {
                                    Value::List(l) => l.iter().any(|x| equal(&v, x)),
                                    w => equal(&v, w),
                                };
                                if any {
                                    break;
                                }
                            }
                            any
                        }
                        Case::Range(s) if !s.after.is_empty() => {
                            let Value::List(l) = self.eval(&Expr::Range(Box::new(s.clone())), vars, *line)? else { unreachable!() };
                            l.iter().any(|w| equal(&v, w))
                        }
                        Case::Range(s) => {
                            let (a, b, _) = self.ends(s, vars, *line)?;
                            let lo = if compare(&a, &b).is_some_and(|o| o.is_le()) { (a, b) } else { (b, a) };
                            compare(&v, &lo.0).is_some_and(|o| o.is_ge()) && le_end(&v, &lo.1)
                        }
                    };
                    if fits {
                        return self.block(body, vars);
                    }
                }
                return self.block(other, vars);
            }
            Node::While { line, cond, body } => {
                let mut pass = 0u64;
                while self.eval(cond, vars, *line)?.truthy() {
                    pass += 1;
                    self.host.progress(format!("while · {pass}"));
                    if let Some(f) = self.pass(body, vars, *line)? {
                        return Ok(f);
                    }
                }
            }
            Node::For { line, names, source, body } => {
                let items = self.items(source, vars, *line)?;
                let total = items.len();
                let shown = names.last().cloned().unwrap_or_default();
                for (k, (key, v)) in items.into_iter().enumerate() {
                    self.host.progress(format!("{}/{total} · {shown} = {}", k + 1, v.shown()));
                    let mark = vars.loops.len();
                    if names.len() == 2 {
                        vars.loops.push((names[0].clone(), key));
                        vars.loops.push((names[1].clone(), v));
                    } else {
                        vars.loops.push((names[0].clone(), v));
                    }
                    let f = self.pass(body, vars, *line);
                    vars.loops.truncate(mark);
                    if let Some(f) = f? {
                        return Ok(f);
                    }
                }
            }
        }
        Ok(Flow::Next)
    }

    /// A loop's pass: Some — the loop's over (stopped, broken off).
    fn pass(&mut self, body: &[Node], vars: &mut Vars, line: usize) -> Result<Option<Flow>, Mistake> {
        self.idle += 1;
        if self.idle > RUNAWAY {
            return Err(Mistake { line, msg: format!("{RUNAWAY} passes without any SQL: the loop's taken for endless") });
        }
        match self.block(body, vars)? {
            Flow::Next | Flow::Continue => Ok(None),
            Flow::Break => Ok(Some(Flow::Next)),
            f @ Flow::Stop(_) => Ok(Some(f)),
        }
    }

    /// A range's ends worked out (read by its pipe), and its step.
    fn ends(&mut self, s: &Steps, vars: &Vars, line: usize) -> Result<(Value, Value, Option<(Option<i64>, Option<Prec>)>), Mistake> {
        let mut a = self.eval(&s.from, vars, line)?;
        let mut b = self.eval(&s.to, vars, line)?;
        if let Some(c) = &s.conv {
            a = convert(a, c).map_err(|msg| Mistake { line, msg })?;
            b = convert(b, c).map_err(|msg| Mistake { line, msg })?;
        }
        let step = match &s.step {
            None => None,
            Some((n, u)) => {
                let n = match n {
                    Some(e) => Some(self.eval(e, vars, line)?.int().ok_or(Mistake { line, msg: "a step is a whole number".into() })?),
                    None => None,
                };
                Some((n, *u))
            }
        };
        Ok((a, b, step))
    }

    /// What a loop goes over: (key or pass number, value) each.
    fn items(&mut self, src: &Source, vars: &Vars, line: usize) -> Result<Vec<(Value, Value)>, Mistake> {
        let wrong = |msg: String| Mistake { line, msg };
        let numbered = |l: Vec<Value>| l.into_iter().enumerate().map(|(k, v)| (Value::Int(k as i64), v)).collect();
        match src {
            Source::Expr(e) => Ok(match self.eval(e, vars, line)? {
                Value::List(l) => numbered(l),
                Value::Dict(d) => d.into_iter().map(|(k, v)| (Value::Str(k), v)).collect(),
                Value::Null => Vec::new(),
                v => numbered(vec![v]),
            }),
            Source::Range(s) => {
                let (a, b, step) = self.ends(s, vars, line)?;
                let mut list = Value::List(range(&a, &b, step).map_err(wrong)?);
                for c in &s.after {
                    list = on_items(list, c, vars, self.d, self.host).map_err(wrong)?;
                }
                Ok(match list {
                    Value::List(l) => numbered(l),
                    Value::Dict(d) => d.into_iter().map(|(k, v)| (Value::Str(k), v)).collect(),
                    Value::Null => Vec::new(),
                    v => numbered(vec![v]),
                })
            }
        }
    }
}

/// A date's `.start`, `.end` (the next period's start), `.year`…
fn date_field(dt: &NaiveDateTime, p: Prec, name: &str) -> Result<Value, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "start" => Value::Date(*dt, p),
        "end" => Value::Date(date_add(*dt, p, 1).ok_or("a date out of range")?, p),
        "year" => Value::Int(dt.year() as i64),
        "quarter" => Value::Int(((dt.month() - 1) / 3 + 1) as i64),
        "month" => Value::Pad(dt.month() as i64, 2),
        "day" => Value::Pad(dt.day() as i64, 2),
        "hour" => Value::Pad(dt.hour() as i64, 2),
        _ => return Err(format!("a date has no \".{name}\" (start, end, year, quarter, month, day, hour)")),
    })
}

/// `v` is at or before a range's end (a date's end: to the end of its
/// precision).
fn le_end(v: &Value, end: &Value) -> bool {
    match (v, end) {
        (Value::Date(x, _), Value::Date(e, p)) => date_add(*e, *p, 1).is_some_and(|lim| x < &lim),
        _ => compare(v, end).is_some_and(|o| o.is_le()),
    }
}

/// The values of a range, both ends in.
fn range(a: &Value, b: &Value, step: Option<(Option<i64>, Option<Prec>)>) -> Result<Vec<Value>, String> {
    const MOST: usize = 1_000_000;
    let n = step.and_then(|s| s.0);
    if n == Some(0) {
        return Err("a step of 0".into());
    }
    match (a, b) {
        (Value::Date(x, px), Value::Date(y, py)) => {
            let unit = step.and_then(|s| s.1).unwrap_or(*px.min(py));
            let n = n.unwrap_or(1);
            let mut out = Vec::new();
            let mut cur = *x;
            if x <= y {
                if n < 0 {
                    return Err("the range goes up, the step down".into());
                }
                // The end's in up to the end of its precision.
                let lim = date_add(*y, *py, 1).ok_or("a date out of range")?;
                while cur < lim {
                    out.push(Value::Date(cur, unit));
                    cur = date_add(cur, unit, n).ok_or("a date out of range")?;
                    if out.len() > MOST {
                        return Err("more than a million passes".into());
                    }
                }
            } else {
                let n = if n > 0 { -n } else { n };
                while cur >= *y {
                    out.push(Value::Date(cur, unit));
                    cur = date_add(cur, unit, n).ok_or("a date out of range")?;
                    if out.len() > MOST {
                        return Err("more than a million passes".into());
                    }
                }
            }
            Ok(out)
        }
        (Value::Str(x), Value::Str(y)) if x.chars().count() == 1 && y.chars().count() == 1 => {
            let (x, y) = (x.chars().next().unwrap() as i64, y.chars().next().unwrap() as i64);
            Ok(int_range(x, y, n)?.into_iter().filter_map(|c| char::from_u32(c as u32)).map(|c| Value::Str(c.to_string())).collect())
        }
        (x, y) if x.int().is_some() && y.int().is_some() && !matches!(x, Value::Str(_)) => {
            if let Some(u) = step.and_then(|s| s.1) {
                // Years are dates by themselves; other numbers aren't guessed.
                let (tx, ty) = (x.text()?, y.text()?);
                if let (Value::Int(_), Value::Int(_), Some(dx), Some(dy)) = (x, y, year(&tx), year(&ty)) {
                    return range(&Value::Date(dx, Prec::Year), &Value::Date(dy, Prec::Year), Some((n, Some(u))));
                }
                return Err(format!("a step by a date's unit over numbers: read them as dates first ({tx} .. {ty} | date {})", guess_fmt(&tx)));
            }
            let w = match (x, y) {
                (Value::Pad(_, w), _) | (_, Value::Pad(_, w)) => Some(*w),
                _ => None,
            };
            let list = int_range(x.int().unwrap(), y.int().unwrap(), n)?;
            if list.len() > MOST {
                return Err("more than a million passes".into());
            }
            Ok(list.into_iter().map(|i| match w {
                Some(w) => Value::Pad(i, w),
                None => Value::Int(i),
            }).collect())
        }
        (x, y) => Err(format!("a range from a {} to a {}: numbers, dates or letters", x.kind(), y.kind())),
    }
}

fn int_range(a: i64, b: i64, n: Option<i64>) -> Result<Vec<i64>, String> {
    let n = n.unwrap_or(if a <= b { 1 } else { -1 });
    if (a < b && n < 0) || (a > b && n > 0) {
        return Err("the step goes the other way from the range".into());
    }
    if ((b - a) / n) as u64 > 1_000_000 {
        return Err("more than a million passes".into());
    }
    let mut out = Vec::new();
    let mut i = a;
    while (n > 0 && i <= b) || (n < 0 && i >= b) {
        out.push(i);
        i += n;
    }
    Ok(out)
}

fn convert(v: Value, c: &Conv) -> Result<Value, String> {
    Ok(match c {
        Conv::Number => match &v {
            Value::Int(_) | Value::Num(_) => v,
            Value::Pad(n, _) => Value::Int(*n),
            Value::Null => Value::Null,
            _ => {
                let s = v.text()?;
                let t = s.trim();
                if let Ok(n) = t.parse::<i64>() {
                    Value::Int(n)
                } else {
                    Value::Num(t.parse::<f64>().map_err(|_| format!("'{s}' isn't a number"))?)
                }
            }
        },
        Conv::Str(None) => match v {
            Value::Null => Value::Null,
            v => Value::Str(v.text()?),
        },
        Conv::Str(Some(f)) => match v {
            Value::Null => Value::Null,
            Value::Date(d, _) => Value::Str(date_fmt(&d, f)),
            v => {
                let t = v.text()?;
                let (d, _) = date_iso(&t).map_err(|_| format!("| string {f}: '{t}' isn't a date (read it first: | date yyyymm)"))?;
                Value::Str(date_fmt(&d, f))
            }
        },
        Conv::List => match v {
            Value::List(_) => v,
            Value::Null => Value::List(Vec::new()),
            v => Value::List(vec![v]),
        },
        Conv::Date(f) => match (v, f) {
            // A date given a format: text in that format.
            (Value::Date(d, _), Some(f)) => Value::Str(date_fmt(&d, f)),
            (v @ Value::Date(..), None) => v,
            (Value::Null, _) => Value::Null,
            (v, Some(f)) => {
                let (d, p) = date_by(&v.text()?, f)?;
                Value::Date(d, p)
            }
            (v, None) => {
                let (d, p) = date_iso(&v.text()?)?;
                Value::Date(d, p)
            }
        },

        // (Those go through on_items.)
        _ => return Err("not a conversion".into()),
    })
}

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::List(x), Value::List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| equal(a, b)),
        (Value::Dict(x), Value::Dict(y)) => x.len() == y.len() && x.iter().all(|(k, v)| b.field(k).is_some_and(|w| equal(v, w))),
        _ => compare(a, b) == Some(std::cmp::Ordering::Equal),
    }
}

fn compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Date(x, _), Value::Date(y, _)) => Some(x.cmp(y)),
        (Value::Date(x, _), Value::Str(s)) => date_iso(s).ok().map(|(y, _)| x.cmp(&y)),
        (Value::Str(s), Value::Date(y, _)) => date_iso(s).ok().map(|(x, _)| x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (x, y) => match (x.num(), y.num()) {
            (Some(p), Some(q)) => p.partial_cmp(&q),
            _ => None,
        },
    }
}

fn arith(op: &str, a: &Value, b: &Value) -> Result<Value, String> {
    if let (Some(x), Some(y)) = (a.int().filter(|_| !matches!(a, Value::Str(_) | Value::Num(_))), b.int().filter(|_| !matches!(b, Value::Str(_) | Value::Num(_)))) {
        return Ok(match op {
            "+" => Value::Int(x.wrapping_add(y)),
            "-" => Value::Int(x.wrapping_sub(y)),
            "*" => Value::Int(x.wrapping_mul(y)),
            "/" if y == 0 => return Err("division by zero".into()),
            "/" if x % y == 0 => Value::Int(x / y),
            "/" => Value::Num(x as f64 / y as f64),
            "%" if y == 0 => return Err("division by zero".into()),
            _ => Value::Int(x % y),
        });
    }
    match (a.num(), b.num()) {
        (Some(x), Some(y)) => Ok(Value::Num(match op {
            "+" => x + y,
            "-" => x - y,
            "*" => x * y,
            "/" if y == 0.0 => return Err("division by zero".into()),
            "/" => x / y,
            _ => x % y,
        })),
        _ => Err(format!("{} {op} {}: numbers wanted", a.kind(), b.kind())),
    }
}

fn binary(op: &str, a: Value, b: Value) -> Result<Value, String> {
    match op {
        "+" => match (a, b) {
            (Value::List(mut x), Value::List(y)) => {
                x.extend(y);
                Ok(Value::List(x))
            }
            (Value::Date(d, p), n) | (n, Value::Date(d, p)) if n.int().is_some() && !matches!(n, Value::Str(_)) => Ok(Value::Date(date_add(d, p, n.int().unwrap()).ok_or("a date out of range")?, p)),
            (a @ Value::Str(_), b) | (a, b @ Value::Str(_)) => Ok(Value::Str(a.text()? + &b.text()?)),
            (a, b) => arith("+", &a, &b),
        },
        "-" => match (a, b) {
            (Value::Date(d, p), n) if n.int().is_some() && !matches!(n, Value::Str(_)) => Ok(Value::Date(date_add(d, p, -n.int().unwrap()).ok_or("a date out of range")?, p)),
            (a, b) => arith("-", &a, &b),
        },
        "*" | "/" | "%" => arith(op, &a, &b),
        "=" => Ok(Value::Bool(equal(&a, &b))),
        "!=" => Ok(Value::Bool(!equal(&a, &b))),
        "<" | "<=" | ">" | ">=" => {
            let o = compare(&a, &b).ok_or(format!("a {} and a {} don't compare", a.kind(), b.kind()))?;
            Ok(Value::Bool(match op {
                "<" => o.is_lt(),
                "<=" => o.is_le(),
                ">" => o.is_gt(),
                _ => o.is_ge(),
            }))
        }
        "in" => Ok(Value::Bool(match &b {
            Value::List(l) => l.iter().any(|x| equal(&a, x)),
            Value::Dict(_) => b.field(&a.text()?).is_some(),
            Value::Str(s) => s.contains(&a.text()?),
            Value::Null => false,
            _ => return Err(format!("\"in\" a {}: a list, dictionary or string wanted", b.kind())),
        })),
        _ => Err(format!("no operator {op}")),
    }
}

/// A query's answer as a value: one cell, a value; one column, a list;
/// more, a list of rows (dictionaries by column).
pub fn from_table(t: &Table) -> Value {
    let cell = |k: usize, v: &Option<String>| -> Value {
        let Some(s) = v else { return Value::Null };
        let col = &t.cols[k];
        let ty = col.ty.to_ascii_lowercase();
        if col.numeric {
            if let Ok(n) = s.trim().parse::<i64>() {
                return Value::Int(n);
            }
            if let Ok(n) = s.trim().parse::<f64>() {
                return Value::Num(n);
            }
        }
        if (ty.contains("date") || ty.contains("time")) && let Ok((d, p)) = date_iso(s) {
            return Value::Date(d, p);
        }
        Value::Str(s.clone())
    };
    match (t.cols.len(), t.rows.len()) {
        (1, 1) => cell(0, &t.rows[0][0]),
        (1, 0) => Value::Null,
        (1, _) => Value::List(t.rows.iter().map(|r| cell(0, &r[0])).collect()),
        _ => Value::List(t.rows.iter().map(|r| Value::Dict(t.cols.iter().enumerate().map(|(k, c)| (c.name.clone(), cell(k, &r[k]))).collect())).collect()),
    }
}

fn eval(e: &Expr, vars: &Vars, d: Dialect, host: &mut dyn Host) -> Result<Value, String> {
    Ok(match e {
        Expr::Lit(v) => v.clone(),
        Expr::Var(n) => vars.get(n).cloned().ok_or(format!("\"{n}\" has no value"))?,
        Expr::List(items) => Value::List(items.iter().map(|x| eval(x, vars, d, host)).collect::<Result<_, _>>()?),
        Expr::Dict(pairs) => Value::Dict(pairs.iter().map(|(k, x)| Ok((k.clone(), eval(x, vars, d, host)?))).collect::<Result<_, String>>()?),
        Expr::Template(pieces) => {
            let mut s = String::new();
            for p in pieces {
                match p {
                    Piece::Text(t) => s += t,
                    Piece::Expr(x) => s += &eval(x, vars, d, host)?.shown(),
                }
            }
            Value::Str(s)
        }
        Expr::Query(q) => {
            let sql = put(q, vars, d)?;
            from_table(&host.query(&sql)?)
        }
        Expr::Field(x, name) => {
            let v = eval(x, vars, d, host)?;
            match &v {
                Value::Dict(_) => v.field(name).cloned().ok_or(format!("no \"{name}\" in it"))?,
                Value::Date(dt, p) => date_field(dt, *p, name)?,
                other => return Err(format!("\".{name}\" of a {}: a dictionary or a row wanted", other.kind())),
            }
        }
        Expr::Index(x, k) => {
            let v = eval(x, vars, d, host)?;
            let k = eval(k, vars, d, host)?;
            match (&v, &k) {
                (Value::List(l), k) => {
                    let i = k.int().ok_or("a list's index is a whole number")?;
                    let i = if i < 0 { l.len() as i64 + i } else { i };
                    l.get(i as usize).cloned().ok_or(format!("no element {i}: the list has {}", l.len()))?
                }
                (Value::Dict(_), k) => v.field(&k.text()?).cloned().ok_or(format!("no \"{}\" in it", k.text()?))?,
                (Value::Str(s), k) => {
                    let i = k.int().ok_or("a string's index is a whole number")?;
                    let n = s.chars().count() as i64;
                    let i = if i < 0 { n + i } else { i };
                    s.chars().nth(i as usize).map(|c| Value::Str(c.to_string())).ok_or("past the string's end")?
                }
                (other, _) => return Err(format!("[…] of a {}", other.kind())),
            }
        }
        Expr::Call(f, args) => {
            let a: Vec<Value> = args.iter().map(|x| eval(x, vars, d, host)).collect::<Result<_, _>>()?;
            match (f.as_str(), a.as_slice()) {
                ("len", [v]) => Value::Int(match v {
                    Value::List(l) => l.len(),
                    Value::Dict(x) => x.len(),
                    Value::Null => 0,
                    v => v.text()?.chars().count(),
                } as i64),
                ("upper", [v]) => Value::Str(v.text()?.to_uppercase()),
                ("lower", [v]) => Value::Str(v.text()?.to_lowercase()),
                ("keys", [Value::Dict(x)]) => Value::List(x.iter().map(|(k, _)| Value::Str(k.clone())).collect()),
                ("values", [Value::Dict(x)]) => Value::List(x.iter().map(|(_, v)| v.clone()).collect()),
                _ => return Err(format!("no function {f}({} arguments): len, upper, lower, keys, values", a.len())),
            }
        }
        Expr::Unary(op, x) => {
            let v = eval(x, vars, d, host)?;
            match *op {
                "not" => Value::Bool(!v.truthy()),
                _ => arith("-", &Value::Int(0), &v)?,
            }
        }
        Expr::Bin(op, a, b) => match *op {
            "and" => {
                let l = eval(a, vars, d, host)?;
                if !l.truthy() { l } else { eval(b, vars, d, host)? }
            }
            "or" => {
                let l = eval(a, vars, d, host)?;
                if l.truthy() { l } else { eval(b, vars, d, host)? }
            }
            op => binary(op, eval(a, vars, d, host)?, eval(b, vars, d, host)?)?,
        },
        Expr::Pipe(x, c) if c.on_items() => on_items(eval(x, vars, d, host)?, c, vars, d, host)?,
        Expr::Pipe(x, c) => convert(eval(x, vars, d, host)?, c)?,
        Expr::Range(s) => {
            let mut a = eval(&s.from, vars, d, host)?;
            let mut b = eval(&s.to, vars, d, host)?;
            if let Some(c) = &s.conv {
                a = convert(a, c)?;
                b = convert(b, c)?;
            }
            let step = match &s.step {
                None => None,
                Some((n, u)) => Some((
                    match n {
                        Some(e) => Some(eval(e, vars, d, host)?.int().ok_or("a step is a whole number")?),
                        None => None,
                    },
                    *u,
                )),
            };
            let mut v = Value::List(range(&a, &b, step)?);
            for c in &s.after {
                v = on_items(v, c, vars, d, host)?;
            }
            v
        }
    })
}

/// A collection's operation (| push, | pop, | map …).
fn on_items(v: Value, c: &Conv, vars: &Vars, d: Dialect, host: &mut dyn Host) -> Result<Value, String> {
    // Each item as `name`, in a copy of the variables.
    let each = |name: &str| {
        let mut local = vars.clone();
        local.loops.push((name.to_string(), Value::Null));
        local
    };
    let set = |local: &mut Vars, x: Value| local.loops.last_mut().unwrap().1 = x;
    // A dictionary's items as {key, value}.
    let entry = |k: &str, x: &Value| Value::Dict(vec![("key".into(), Value::Str(k.to_string())), ("value".into(), x.clone())]);
    let place = |i: i64, n: usize, end: bool| -> Result<usize, String> {
        let n = n as i64;
        let k = if i < 0 { n + i + end as i64 } else { i };
        if k < 0 || k > n || (!end && k == n) { Err(format!("no place {i}: {n} items")) } else { Ok(k as usize) }
    };
    Ok(match (v, c) {
        (v, c) if !c.on_items() => convert(v, c)?,
        (Value::Null, Conv::Push(..)) => return on_items(Value::List(Vec::new()), c, vars, d, host),
        (Value::Null, _) => Value::Null,
        (Value::List(mut l), Conv::Push(x, at)) => {
            let x = eval(x, vars, d, host)?;
            let k = match at {
                Some(e) => place(eval(e, vars, d, host)?.int().ok_or("a place is a whole number")?, l.len(), true)?,
                None => l.len(),
            };
            l.insert(k, x);
            Value::List(l)
        }
        (Value::Dict(mut m), Conv::Push(x, _)) => match eval(x, vars, d, host)? {
            Value::Dict(more) => {
                for (k, x) in more {
                    match m.iter_mut().find(|(n, _)| *n == k) {
                        Some(slot) => slot.1 = x,
                        None => m.push((k, x)),
                    }
                }
                Value::Dict(m)
            }
            other => return Err(format!("a dictionary takes a dictionary ({{key: value}}), not a {}", other.kind())),
        },
        (Value::List(mut l), Conv::Pop(at)) => {
            if l.is_empty() {
                return Err("| pop: the list is empty".into());
            }
            let k = match at {
                Some(e) => place(eval(e, vars, d, host)?.int().ok_or("a place is a whole number")?, l.len(), false)?,
                None => l.len() - 1,
            };
            l.remove(k);
            Value::List(l)
        }
        (Value::Dict(mut m), Conv::Pop(at)) => {
            match at {
                Some(e) => {
                    let k = eval(e, vars, d, host)?.text()?;
                    let n = m.iter().position(|(n, _)| *n == k).ok_or(format!("no \"{k}\" in it"))?;
                    m.remove(n);
                }
                None => {
                    m.pop().ok_or("| pop: the dictionary is empty")?;
                }
            }
            Value::Dict(m)
        }
        (Value::List(mut l), Conv::Remove(x)) => {
            let x = eval(x, vars, d, host)?;
            l.retain(|y| !equal(y, &x));
            Value::List(l)
        }
        (Value::Dict(mut m), Conv::Remove(x)) => {
            let k = eval(x, vars, d, host)?.text()?;
            m.retain(|(n, _)| *n != k);
            Value::Dict(m)
        }
        (Value::List(l), Conv::Map(n, e)) => {
            let mut local = each(n);
            let mut out = Vec::with_capacity(l.len());
            for x in l {
                set(&mut local, x);
                out.push(eval(e, &local, d, host)?);
            }
            Value::List(out)
        }
        (Value::Dict(m), Conv::Map(n, e)) => {
            let mut local = each(n);
            let mut out = Vec::with_capacity(m.len());
            for (k, x) in &m {
                set(&mut local, entry(k, x));
                out.push(eval(e, &local, d, host)?);
            }
            Value::List(out)
        }
        (Value::List(l), Conv::Filter(n, e)) => {
            let mut local = each(n);
            let mut out = Vec::new();
            for x in l {
                set(&mut local, x.clone());
                if eval(e, &local, d, host)?.truthy() {
                    out.push(x);
                }
            }
            Value::List(out)
        }
        (Value::Dict(m), Conv::Filter(n, e)) => {
            let mut local = each(n);
            let mut out = Vec::new();
            for (k, x) in m {
                set(&mut local, entry(&k, &x));
                if eval(e, &local, d, host)?.truthy() {
                    out.push((k, x));
                }
            }
            Value::Dict(out)
        }
        (v @ (Value::List(_) | Value::Dict(_)), Conv::Sort { desc, by }) => {
            let dict = matches!(v, Value::Dict(_));
            let items: Vec<(Value, Value)> = match v {
                Value::List(l) => l.into_iter().map(|x| (x.clone(), x)).collect(),
                Value::Dict(m) => m.into_iter().map(|(k, x)| (Value::Str(k), x)).collect(),
                _ => unreachable!(),
            };
            // (Each one's key worked out once.)
            let mut keyed: Vec<(Value, (Value, Value))> = Vec::with_capacity(items.len());
            let mut local = by.as_ref().map(|(n, _)| each(n));
            for (k, x) in items {
                let key = match (by, local.as_mut()) {
                    (Some((_, e)), Some(lv)) => {
                        set(lv, if dict { entry(&k.text()?, &x) } else { x.clone() });
                        eval(e, lv, d, host)?
                    }
                    _ => k.clone(),
                };
                keyed.push((key, (k, x)));
            }
            keyed.sort_by(|a, b| {
                let o = match (&a.0, &b.0) {
                    (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
                    (Value::Null, _) => std::cmp::Ordering::Less,
                    (_, Value::Null) => std::cmp::Ordering::Greater,
                    (x, y) => compare(x, y).unwrap_or_else(|| x.shown().cmp(&y.shown())),
                };
                if *desc { o.reverse() } else { o }
            });
            if dict {
                Value::Dict(keyed.into_iter().map(|(_, (k, x))| (k.text().unwrap_or_default(), x)).collect())
            } else {
                Value::List(keyed.into_iter().map(|(_, (_, x))| x).collect())
            }
        }
        (Value::List(mut l), Conv::Reverse) => {
            l.reverse();
            Value::List(l)
        }
        (Value::Dict(mut m), Conv::Reverse) => {
            m.reverse();
            Value::Dict(m)
        }
        (Value::Str(t), Conv::Reverse) => Value::Str(t.chars().rev().collect()),
        (Value::List(l), Conv::Unique) => {
            let mut out: Vec<Value> = Vec::new();
            for x in l {
                if !out.iter().any(|y| equal(y, &x)) {
                    out.push(x);
                }
            }
            Value::List(out)
        }
        (Value::List(l), Conv::Range(..)) => {
            let mut out = Vec::new();
            for x in l {
                match on_items(x, c, vars, d, host)? {
                    Value::List(more) => out.extend(more),
                    other => out.push(other),
                }
            }
            Value::List(out)
        }
        (v, Conv::Range(n, u)) => {
            let (dt, p) = as_period(&v)?;
            let n = match n {
                Some(e) => Some(eval(e, vars, d, host)?.int().ok_or("| range: a count is a whole number")?),
                None => None,
            };
            if n.is_some_and(|n| n <= 0) {
                return Err("| range: a count above 0".into());
            }
            let at = Value::Date(dt, p);
            Value::List(range(&at, &at, Some((n, Some(*u))))?)
        }
        (Value::List(l), Conv::Join(sep)) => {
            let sep = match sep {
                Some(e) => eval(e, vars, d, host)?.text()?,
                None => ", ".into(),
            };
            Value::Str(l.iter().map(Value::text).collect::<Result<Vec<_>, _>>()?.join(&sep))
        }
        (v, c) => {
            let what = match c {
                Conv::Push(..) => "push",
                Conv::Pop(_) => "pop",
                Conv::Remove(_) => "remove",
                Conv::Map(..) => "map",
                Conv::Filter(..) => "filter",
                Conv::Sort { .. } => "sort",
                Conv::Reverse => "reverse",
                Conv::Unique => "unique",
                Conv::Join(_) => "join",
                Conv::Range(..) => "range",
                _ => "",
            };
            return Err(format!("| {what} of a {}: a list (or a dictionary) wanted", v.kind()));
        }
    })
}

/// A value worked out apart from a run (no queries): for tests and hover.
/// A run's host that has no database.
struct NoHost;

impl Host for NoHost {
    fn sql(&mut self, _: Stmt) -> SqlEnd {
        SqlEnd::Over
    }
    fn query(&mut self, _: &str) -> Result<Table, String> {
        Err("no queries here".into())
    }
    fn print(&mut self, _: String) {}
    fn progress(&mut self, _: String) {}
    fn stopped(&self) -> bool {
        false
    }
}

/// The variables a script's directives above `upto` give, as far as can
/// be told without running it: plain values (not queries), a loop's
/// first pass. What can't be told is left out.
pub fn static_vars(text: &str, upto: usize, d: Dialect) -> Vars {
    let mut vars = Vars::default();
    for (r, src) in directives(text, d) {
        if r.start >= upto {
            break;
        }
        let Ok(line) = read_line(&src, 0) else { continue };
        let ev = |e: &Expr, vars: &Vars| eval(e, vars, d, &mut NoHost).ok();
        match line {
            Line::Node(Node::Set { name, op, value, .. }) => {
                let v = match &value {
                    Some(e) => ev(e, &vars),
                    None => Some(Value::Int(1)),
                };
                let Some(v) = v else {
                    vars.globals.remove(&name);
                    continue;
                };
                let v = if op == "=" {
                    Some(v)
                } else {
                    let bin = match op {
                        "+=" | "++" => "+",
                        "-=" | "--" => "-",
                        "*=" => "*",
                        _ => "/",
                    };
                    vars.get(&name).cloned().and_then(|old| binary(bin, old, v).ok())
                };
                match v {
                    Some(v) => vars.set(&name, v),
                    None => {
                        vars.globals.remove(&name);
                    }
                }
            }
            Line::For(names, source) => {
                let all = match source {
                    Source::Expr(e) => ev(&e, &vars),
                    Source::Range(st) => ev(&Expr::Range(Box::new(st)), &vars),
                };
                let first = match all {
                    Some(Value::List(l)) => l.into_iter().next().map(|v| (Value::Int(0), v)),
                    Some(Value::Dict(m)) => m.into_iter().next().map(|(k, v)| (Value::Str(k), v)),
                    Some(Value::Null) | None => None,
                    Some(v) => Some((Value::Int(0), v)),
                };
                if let Some((k, v)) = first {
                    if names.len() == 2 {
                        vars.globals.insert(names[0].clone(), k);
                        vars.globals.insert(names[1].clone(), v);
                    } else {
                        vars.globals.insert(names[0].clone(), v);
                    }
                }
            }
            _ => {}
        }
    }
    vars
}

/// A script's text with its &x put in as far as `static_vars` tells (for
/// hints: "from bis.calls_&m" is a table then), and where each went: (its
/// range in `text`, the length of what it became). None: nothing put in.
pub fn resolved(text: &str, upto: usize, d: Dialect) -> Option<(String, Vec<(Range<usize>, usize)>)> {
    if !text.contains('&') || !has_directives(text, d) {
        return None;
    }
    let vars = static_vars(text, upto, d);
    let toks = lex::lex(text, d);
    let comment = |i: usize| {
        let k = toks.partition_point(|t| t.end <= i);
        toks.get(k).is_some_and(|t| matches!(t.kind, Tk::LineComment | Tk::BlockComment))
    };
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::new();
    let mut prev = 0;
    for m in marks(text, &comment) {
        if !text[m.clone()].starts_with('&') {
            continue;
        }
        let Ok(put) = put(&text[m.clone()], &vars, d) else { continue };
        if put == text[m.clone()] {
            continue;
        }
        out += &text[prev..m.start];
        out += &put;
        map.push((m.clone(), put.len()));
        prev = m.end;
    }
    if map.is_empty() {
        return None;
    }
    out += &text[prev..];
    Some((out, map))
}

pub fn eval_plain(src: &str, vars: &Vars, d: Dialect) -> Result<Value, String> {
    let mut p = P::new(src)?;
    let e = p.list_or_expr()?;
    p.done()?;
    eval(&e, vars, d, &mut NoHost)
}

// ───────────────────────────── into SQL ─────────────────────────────

/// The variables put into a statement: `:x` a value, `&x` text as it is,
/// `:x.f` / `&x.f` a field, `:{…}` / `&{…}` an expression, `&x.` the dot
/// ending the name. Names with no value stay as they are (strings and
/// comments too).
pub fn put(sql: &str, vars: &Vars, d: Dialect) -> Result<String, String> {
    if !sql.contains([':', '&']) {
        return Ok(sql.to_string());
    }
    let toks = lex::lex(sql, d);
    let dead: Vec<Range<usize>> = toks.iter().filter(|t| matches!(t.kind, Tk::Str | Tk::LineComment | Tk::BlockComment | Tk::Quoted)).map(|t| t.start..t.end).collect();
    let comments: Vec<Range<usize>> = toks.iter().filter(|t| matches!(t.kind, Tk::LineComment | Tk::BlockComment)).map(|t| t.start..t.end).collect();
    let b = sql.as_bytes();
    let ident_at = |k: usize| sql[k..].chars().next().is_some_and(|c| c.is_alphabetic() || c == '_');
    let name_end = |mut k: usize| {
        while k < sql.len() && sql[k..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            k += sql[k..].chars().next().unwrap().len_utf8();
        }
        k
    };
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        let c = b[i];
        let amp = c == b'&';
        // `&` goes in strings and quoted names too (as SQL*Plus has it),
        // `:` doesn't; neither in comments.
        let skip = |r: &&Range<usize>| r.contains(&i);
        if (c != b':' && !amp) || (amp && dead.iter().filter(|r| comments.iter().any(|c| c == *r)).any(|r| skip(&r))) || (!amp && dead.iter().any(|r| skip(&r))) {
            i += 1;
            continue;
        }
        let prev = if i > 0 { b[i - 1] } else { b' ' };
        // a::type, x := 1, {p:T}; `&` after a name is fine (events_&i).
        if !amp && (prev.is_ascii_alphanumeric() || prev == b'_' || prev == b':') {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if amp && b.get(j) == Some(&b'&') {
            j += 1;
        }
        let (value, mut end) = if b.get(j) == Some(&b'{') {
            let Some(close) = sql[j..].find('}') else {
                i += 1;
                continue;
            };
            let v = eval_plain(&sql[j + 1..j + close], vars, d).map_err(|m| format!("{}: {m}", &sql[i..j + close + 1]))?;
            (v, j + close + 1)
        } else if ident_at(j) {
            let e = name_end(j);
            let Some(v) = vars.get(&sql[j..e]) else {
                i = e;
                continue;
            };
            let mut v = v.clone();
            let mut e = e;
            // Fields: .name while it's a row / dictionary that has it.
            while b.get(e) == Some(&b'.') && ident_at(e + 1) {
                let fe = name_end(e + 1);
                match v.field(&sql[e + 1..fe]) {
                    Some(f) => {
                        v = f.clone();
                        e = fe;
                    }
                    None => break,
                }
            }
            (v, e)
        } else {
            i += 1;
            continue;
        };
        // "&x." — the dot ends the name.
        if amp && b.get(end) == Some(&b'.') {
            end += 1;
        }
        let text = if amp { value.text() } else { value.sql(d) }.map_err(|m| format!("{}: {m}", &sql[i..end]))?;
        out += &sql[copied..i];
        out += &text;
        copied = end;
        i = end;
    }
    out += &sql[copied..];
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(src: &str) -> Value {
        eval_plain(src, &Vars::default(), Dialect::ClickHouse).unwrap()
    }

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        let mut out = Vars::default();
        for (n, e) in pairs {
            let val = eval_plain(e, &out, Dialect::ClickHouse).unwrap();
            out.globals.insert(n.to_string(), val);
        }
        out
    }

    #[test]
    fn values_and_expressions() {
        assert_eq!(v("1 + 2 * 3"), Value::Int(7));
        assert_eq!(v("7 / 2"), Value::Num(3.5));
        assert_eq!(v("'a' + 1"), Value::Str("a1".into()));
        assert_eq!(v("\"123\" | number"), Value::Int(123));
        assert_eq!(v("1 | string"), Value::Str("1".into()));
        for l in ["1,2,3", "[1,2,3]", "(1,2,3)", "{1,2,3}"] {
            assert_eq!(v(l), Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]), "{l}");
        }
        let dk = v("{\"key1\":1,\"key2\":2}");
        assert_eq!(dk.field("KEY2"), Some(&Value::Int(2)));
        assert_eq!(v("len([1,2]) + len('abc')"), Value::Int(5));
        assert_eq!(v("2 in [1,2] and not 5 in [1,2] and 'k' in {k: 1}"), Value::Bool(true));
        assert_eq!(v("[1,2][-1]"), Value::Int(2));
        assert_eq!(v("`a ${1+1} b`"), Value::Str("a 2 b".into()));
        assert_eq!(v("202611 | date yyyymm").shown(), "2026-11-01");
        assert_eq!(v("202611 | date yyyymm | date yyyy-mm"), Value::Str("2026-11".into()));
        assert_eq!(v("'2026-11-01' | date + 1").shown(), "2026-11-02");
        assert_eq!(v("'2026-11-01 10:30' | date | date yyyymmdd_hh24mi"), Value::Str("20261101_1030".into()));
        assert_eq!(v("'2026-11-01 10:30' | date | date dd.mm.yyyy"), Value::Str("01.11.2026".into()));
        assert!(eval_plain("'2026-11-01' | date | date dd.mm.yyyy hh24:mi", &Vars::default(), Dialect::ClickHouse).is_err(), "a space in a format: quoted");
        assert_eq!(v("'2026-11-01 10:30' | date | date 'dd.mm.yyyy hh24:mi'"), Value::Str("01.11.2026 10:30".into()));
        assert_eq!(v("'01/11/2026' | date dd/mm/yyyy").shown(), "2026-11-01");
        // A date to text by a mask.
        assert_eq!(v("202611 | date yyyymm | string mmyyyy"), Value::Str("112026".into()));
        assert_eq!(v("'2026-11-05' | string dd.mm.yyyy"), Value::Str("05.11.2026".into()));
        assert_eq!(v("'2026-11-05 10:30' | date | string 'dd.mm.yyyy hh24:mi'"), Value::Str("05.11.2026 10:30".into()));
        assert_eq!(v("202611 | string"), Value::Str("202611".into()));
        assert_eq!(v("1 | string in ['1']"), Value::Bool(true));
        assert!(eval_plain("'abc' | string yyyy", &Vars::default(), Dialect::ClickHouse).unwrap_err().contains("isn't a date"));
        assert_eq!(v("[1, 'a', {x: null}]").shown(), "[1, \"a\", {\"x\": null}]");
        assert!(eval_plain("1 +", &Vars::default(), Dialect::ClickHouse).is_err());
        assert!(eval_plain("nope", &Vars::default(), Dialect::ClickHouse).unwrap_err().contains("nope"));
    }

    #[test]
    fn ranges() {
        let r = |a: &str, b: &str, s: Option<(Option<i64>, Option<Prec>)>| range(&v(a), &v(b), s).unwrap().iter().map(|x| x.text().unwrap()).collect::<Vec<_>>().join(" ");
        assert_eq!(r("1", "4", None), "1 2 3 4");
        assert_eq!(r("4", "1", None), "4 3 2 1");
        assert_eq!(r("0", "100", Some((Some(25), None))), "0 25 50 75 100");
        assert_eq!(r("08", "11", None), "08 09 10 11");
        assert_eq!(r("'a'", "'d'", None), "a b c d");
        // The end month in whole, by days.
        let days = range(&v("202611 | date yyyymm"), &v("202612 | date yyyymm"), Some((None, Some(Prec::Day)))).unwrap();
        assert_eq!(days.len(), 61);
        assert_eq!(days.last().unwrap().text().unwrap(), "2026-12-31");
        assert_eq!(r("202611 | date yyyymm", "202702 | date yyyymm", None), "2026-11-01 2026-12-01 2027-01-01 2027-02-01");
        assert_eq!(r("'2026-11-01 22:00' | date", "'2026-11-02 01:00' | date", Some((None, Some(Prec::Hour)))), "2026-11-01 22:00:00 2026-11-01 23:00:00 2026-11-02 00:00:00 2026-11-02 01:00:00");
        assert!(range(&v("1"), &v("5"), Some((Some(-1), None))).is_err());
    }

    #[test]
    fn put_into_sql() {
        let vs = vars(&[("i", "7"), ("p", "06"), ("s", "\"O'Neil\""), ("ls", "1,2,3"), ("names", "['a','b']"), ("t", "{database: 'db', name: 'ev_1'}"), ("d", "'2026-11-01' | date")]);
        let ch = Dialect::ClickHouse;
        assert_eq!(put("select * from events_&i where n = :i", &vs, ch).unwrap(), "select * from events_7 where n = 7");
        assert_eq!(put("from events_&i._old, t_&p", &vs, ch).unwrap(), "from events_7_old, t_06");
        assert_eq!(put("from &t.database..&t.name", &vs, ch).unwrap(), "from db.ev_1");
        assert_eq!(put("where x in (:ls) and y in (:names) and n = :s", &vs, ch).unwrap(), "where x in (1, 2, 3) and y in ('a', 'b') and n = 'O\\'Neil'");
        assert_eq!(put("where day = :d and t like 'x_&{d | date yyyymmdd}%' and s = ':d'", &vs, ch).unwrap(), "where day = '2026-11-01' and t like 'x_20261101%' and s = ':d'");
        assert_eq!(put("from e_&{d | date yyyymmdd} where n > :{i * 2}", &vs, ch).unwrap(), "from e_20261101 where n > 14");
        assert_eq!(put("where day = :d", &vs, Dialect::Oracle).unwrap(), "where day = DATE '2026-11-01'");
        // Left alone: casts, :=, unknown names, `:` in strings, comments.
        let keep = "select a::text, x := 1, :unknown, &unknown, ':i', a & b -- :i &i\nfrom {p:String}";
        assert_eq!(put(keep, &vs, Dialect::Postgres).unwrap(), keep);
        assert_eq!(put("select \"&i\", 'ev_&i'", &vs, Dialect::Postgres).unwrap(), "select \"7\", 'ev_7'");
        assert!(put("select :t", &vs, ch).unwrap_err().contains("dictionary"));
    }

    fn prog(src: &str) -> Result<Program, Mistake> {
        parse(src, 0, src, Dialect::ClickHouse, false)
    }

    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
        fail_on: Option<String>,
        answers: HashMap<String, Table>,
    }

    impl Host for Fake {
        fn sql(&mut self, st: Stmt) -> SqlEnd {
            self.log.push(st.sql.clone());
            if self.fail_on.as_ref().is_some_and(|f| st.sql.contains(f.as_str())) { SqlEnd::Failed } else { SqlEnd::Ok }
        }
        fn query(&mut self, sql: &str) -> Result<Table, String> {
            self.log.push(format!("? {sql}"));
            self.answers.get(sql).cloned().ok_or(format!("no answer for {sql}"))
        }
        fn print(&mut self, text: String) {
            self.log.push(format!("> {text}"));
        }
        fn progress(&mut self, _: String) {}
        fn stopped(&self) -> bool {
            false
        }
    }

    fn table(cols: &[&str], rows: &[&[&str]]) -> Table {
        Table {
            cols: cols.iter().map(|c| crate::db::Col { name: c.to_string(), ty: "String".into(), numeric: false }).collect(),
            rows: rows.iter().map(|r| r.iter().map(|c| Some(c.to_string())).collect()).collect(),
            more: false,
        }
    }

    fn go(src: &str, host: &mut Fake) -> Result<End, Mistake> {
        let p = prog(src)?;
        run(&p, &mut Vars::default(), Dialect::ClickHouse, false, host)
    }

    #[test]
    fn loops_and_conditions() {
        let src = "-- @set day = '2026-11-01'\n\
            -- @for i in 0 .. 2\n\
            insert into agg select * from events_&i where d = :day;\n\
            -- @if i = 1\n\
            -- @print('one ', i)\n\
            -- @elif i > 1\n\
            -- @print(`more: ${i}`)\n\
            -- @else\n\
            optimize table events_&i final;\n\
            -- @end if\n\
            -- @end for\n\
            -- @set x = 3\n\
            -- @while x > 0\n\
            -- @x--\n\
            -- @if x = 1\n\
            -- @break\n\
            -- @end\n\
            -- @end while\n\
            -- @print(x)\n";
        let mut h = Fake::default();
        assert_eq!(go(src, &mut h), Ok(End::Done));
        assert_eq!(
            h.log,
            [
                "insert into agg select * from events_0 where d = '2026-11-01'",
                "optimize table events_0 final",
                "insert into agg select * from events_1 where d = '2026-11-01'",
                "> one 1",
                "insert into agg select * from events_2 where d = '2026-11-01'",
                "> more: 2",
                "> 1",
            ]
        );
    }

    #[test]
    fn periods_cut_by_range() {
        let txt = |e: &str| match v(e) {
            Value::List(l) => l.iter().map(|x| x.text().unwrap()).collect::<Vec<_>>().join(" "),
            other => panic!("{other:?}"),
        };
        let months = txt("2026 | range month");
        assert!(months.starts_with("2026-01-01 2026-02-01") && months.ends_with("2026-12-01"), "{months}");
        assert_eq!(match v("2026 | range day") { Value::List(l) => l.len(), _ => 0 }, 365);
        assert_eq!(txt("'2026-11' | range 2 week").split(' ').count(), 3);
        assert_eq!(txt("202611 | date yyyymm | range day").split(' ').count(), 30);
        assert_eq!(txt("20261105 | date yyyymmdd | range 6 hour"), "2026-11-05 00:00:00 2026-11-05 06:00:00 2026-11-05 12:00:00 2026-11-05 18:00:00");
        // A list: each item cut, all in one list.
        assert_eq!(txt("[2025, 2026] | range year"), "2025-01-01 2026-01-01");
        assert_eq!(txt("202601 .. 202602 | date yyyymm | range day").split(' ').count(), 59);
        // Other numbers aren't guessed to be dates.
        let e = eval_plain("202611 | range day", &Vars::default(), Dialect::ClickHouse).unwrap_err();
        assert!(e.contains("| date yyyymm"), "{e}");
        let e = eval_plain("202601 .. 202612 step month", &Vars::default(), Dialect::ClickHouse).unwrap_err();
        assert!(e.contains("read them as dates"), "{e}");
        assert_eq!(txt("2025 .. 2026 step quarter").split(' ').count(), 8);
        assert!(eval_plain("2026 | range", &Vars::default(), Dialect::ClickHouse).is_err());
        // A period's start, end and parts.
        let vs = vars(&[("m", "202611 | date yyyymm")]);
        let f = |e: &str| eval_plain(e, &vs, Dialect::ClickHouse).unwrap().text().unwrap();
        assert_eq!((f("m.start"), f("m.end"), f("m.year"), f("m.month"), f("m.quarter")), ("2026-11-01".into(), "2026-12-01".into(), "2026".into(), "11".into(), "4".into()));
        assert_eq!(put("where dt >= :{m.start} and dt < :{m.end}", &vs, Dialect::Oracle).unwrap(), "where dt >= DATE '2026-11-01' and dt < DATE '2026-12-01'");
        // A loop's items converted as they're named.
        let mut h = Fake::default();
        go("-- @for m as yyyymm in 2026 | range month\nselect &m;\n-- @end for", &mut h).unwrap();
        assert_eq!(h.log.len(), 12);
        assert_eq!((h.log[0].as_str(), h.log[11].as_str()), ("select 202601", "select 202612"));
        let mut h = Fake::default();
        go("-- @for d as date yyyymm in 202611 .. 202612\nselect :{d.end};\n-- @end for", &mut h).unwrap();
        assert_eq!(h.log, ["select '2026-12-01'", "select '2027-01-01'"]);
        let mut h = Fake::default();
        go("-- @for k, x as string in [1, 2]\nselect '&k-&x';\n-- @end for", &mut h).unwrap();
        assert_eq!(h.log, ["select '0-1'", "select '1-2'"]);
        assert!(prog("-- @for m as in 1..2\n-- @end for").is_err());
        assert_eq!(var_names("-- @for m as yyyymm in 2026 | range month\n", Dialect::Generic), ["m"]);
        let mut open = Vec::new();
        let show = |c: &str, open: &mut Vec<Paint>| paint(c, open).unwrap().into_iter().map(|(r, p)| format!("{}:{p:?}", &c[r])).collect::<Vec<_>>().join(" ");
        assert_eq!(show("-- @for m as yyyymm in y | range 2 month", &mut open), "--:Mark @for:Loop m:Name as:Loop yyyymm:Str in:Loop y:Name |:Op range:Loop 2:Num month:Loop");
        assert_eq!(show("-- @for m as date yyyymm in 1..2", &mut open), "--:Mark @for:Loop m:Name as:Loop date:Loop yyyymm:Str in:Loop 1:Num ..:Op 2:Num");
    }

    #[test]
    fn collections_changed() {
        let l = |s: &str| v(s).shown();
        assert_eq!(l("[1, 2, 3] | pop"), "[1, 2]");
        assert_eq!(l("[1, 2, 3] | pop 0"), "[2, 3]");
        assert_eq!(l("[1, 2, 3] | pop -2"), "[1, 3]");
        assert_eq!(l("[1, 2] | push 3"), "[1, 2, 3]");
        assert_eq!(l("[1, 2] | push 0 at 0 | push 9 at -1"), "[0, 1, 2, 9]");
        assert_eq!(l("[1, 2, 1, 3] | remove 1"), "[2, 3]");
        assert_eq!(l("{a: 1, b: 2} | pop 'a' | push {c: 3}"), "{\"b\": 2, \"c\": 3}");
        assert_eq!(l("[1, 2, 3, 4] | map it * 10 | filter it > 15"), "[20, 30, 40]");
        assert_eq!(l("[1, 2, 3] | map x => x + 1 | join '-'"), "2-3-4");
        assert_eq!(l("[3, 1, 2] | sort"), "[1, 2, 3]");
        assert_eq!(l("[3, 1, 2] | sort desc"), "[3, 2, 1]");
        assert_eq!(l("[{n: 'b', k: 2}, {n: 'a', k: 1}] | sort by it.k | map it.n"), "[\"a\", \"b\"]");
        assert_eq!(l("[{n: 'b', k: 2}, {n: 'a', k: 1}] | sort by r => r.n desc | map r => r.k"), "[2, 1]");
        assert_eq!(l("{b: 2, a: 1, c: 3} | sort | filter it.value > 1"), "{\"b\": 2, \"c\": 3}");
        assert_eq!(l("{a: 1, b: 2} | map `${it.key}=${it.value}` | join"), "a=1, b=2");
        assert_eq!(l("[1, 2, 2, 3, 1] | unique | reverse"), "[3, 2, 1]");
        assert_eq!(l("[1, 2] | map (it | string) + '!'"), "[\"1!\", \"2!\"]");
        // Ranges are lists; an operation after one takes the list.
        assert_eq!(l("01..05 | filter it % 2 == 1"), "[01, 03, 05]");
        assert_eq!(l("(1..3) | map it * 2"), "[2, 4, 6]");
        assert_eq!(l("202611..202612 | date yyyymm step month | map (it | date yyyymm)"), "[\"202611\", \"202612\"]");
        assert!(eval_plain("[] | pop", &Vars::default(), Dialect::ClickHouse).unwrap_err().contains("empty"));
        assert!(eval_plain("5 | push 1", &Vars::default(), Dialect::ClickHouse).unwrap_err().contains("list"));
        // In place: x |= …; over a range in a loop.
        let src = "-- @set ls = [1, 2, 3]\n-- @ls |= pop 0 | push 7\n-- @ls += [8]\n-- @print(ls)\n-- @for i in 1..6 | filter it % 3 == 0\n-- @print(i)\n-- @end for\n-- @match 4\n-- @case 1..6 | filter it % 2 == 0\n-- @print('even')\n-- @end match";
        let mut h = Fake::default();
        assert_eq!(go(src, &mut h), Ok(End::Done));
        assert_eq!(h.log, ["> [2, 3, 7, 8]", "> 3", "> 6", "> even"]);
    }

    #[test]
    fn match_dicts_and_queries() {
        let src = "-- @set dk = {\"eu\": 1, \"us\": 7}\n\
            -- @for k, v in dk\n\
            -- @match v\n\
            -- @case 'x', 1\n\
            select '&k' as first;\n\
            -- @case 5 .. 10\n\
            select '&k' as mid;\n\
            -- @default\n\
            select 'other';\n\
            -- @end match\n\
            -- @end for\n\
            -- @for t in (select database, name from system.tables)\n\
            optimize table &t.database..&t.name final;\n\
            -- @end for\n\
            -- @set n = (select count() from x)\n\
            -- @print(n + 1)";
        let mut h = Fake::default();
        h.answers.insert("select database, name from system.tables".into(), table(&["database", "name"], &[&["db", "a"], &["db", "b"]]));
        let mut n = table(&["c"], &[&["41"]]);
        n.cols[0].numeric = true;
        h.answers.insert("select count() from x".into(), n);
        assert_eq!(go(src, &mut h), Ok(End::Done));
        assert_eq!(
            h.log,
            [
                "select 'eu' as first",
                "select 'us' as mid",
                "? select database, name from system.tables",
                "optimize table db.a final",
                "optimize table db.b final",
                "? select count() from x",
                "> 42",
            ]
        );
    }

    #[test]
    fn errors_stop_or_go_on() {
        let src = "-- @for i in 1 .. 3\nselect &i;\n-- @end for";
        let mut h = Fake { fail_on: Some("2".into()), ..Default::default() };
        assert_eq!(go(src, &mut h), Ok(End::Failed));
        assert_eq!(h.log.len(), 2);
        let mut h = Fake { fail_on: Some("2".into()), ..Default::default() };
        assert_eq!(go(&format!("-- @on error continue\n{src}"), &mut h), Ok(End::Done));
        assert_eq!(h.log.len(), 3);
    }

    #[test]
    fn mistakes_found_first() {
        let line = |src: &str| prog(src).unwrap_err();
        assert_eq!(line("select 1;\n-- @for i in 1 .. 3\nselect 2;").line, 2);
        let m = line("-- @for i in 1..2\n-- @end while");
        assert!(m.msg.contains("closes the @for"), "{m:?}");
        assert_eq!(line("select 1;\n\n-- @set x = = 2").line, 3);
        assert!(line("-- @else").msg.contains("outside"));
        // Not directives: plain comments stay comments.
        let p = prog("-- @author someone\n-- a @ b\nselect 1; -- @set x = 1\n").unwrap();
        assert_eq!(p.nodes.len(), 1);
        // A runaway loop.
        let mut h = Fake::default();
        assert!(go("-- @while true\n-- @end while", &mut h).unwrap_err().msg.contains("endless"));
        // Names given values aren't asked for.
        let p = prog("-- @set a = 1\n-- @for i, x in [1]\n-- @y++\n-- @end for").unwrap();
        assert_eq!(p.names(), ["a", "i", "x", "y"]);
    }

    #[test]
    fn painted() {
        let show = |c: &str, open: &mut Vec<Paint>| paint(c, open).unwrap().into_iter().map(|(r, p)| format!("{}:{p:?}", &c[r])).collect::<Vec<_>>().join(" ");
        let mut open = Vec::new();
        assert_eq!(show("-- @for i in 1..3 step 1 day -- shards", &mut open), "--:Mark @for:Loop i:Name in:Loop 1:Num ..:Op 3:Num step:Loop 1:Num day:Loop -- shards:Comment");
        assert_eq!(show("--@if len(xs) > 0 and x == 'a'", &mut open), "--:Mark @if:Cond len:Func (:Op xs:Name ):Op >:Op 0:Num and:Cond x:Name ==:Op 'a':Str");
        assert_eq!(show("-- @end", &mut open), "--:Mark @end:Cond");
        assert_eq!(show("-- @end for", &mut open), "--:Mark @end for:Loop");
        assert_eq!(show("-- @d = (select max(d) from t) | date yyyy-mm", &mut open), "--:Mark @:Var d:Name =:Op (:Op select max(d) from t:Query ):Op |:Op date:Var yyyy-mm:Str");
        assert_eq!(show("-- @v = d | string mmyyyy", &mut open), "--:Mark @:Var v:Name =:Op d:Name |:Op string:Var mmyyyy:Str");
        assert_eq!(show("-- @pause 10s", &mut open), "--:Mark @pause:Misc 10s:Num");
        assert!(paint("-- @author me", &mut open).is_none());
        assert_eq!(shape("-- @else if x"), Some(Shape::Mid));
        assert_eq!(shape("-- @endfor"), Some(Shape::Close));
        assert_eq!(var_names("-- @set a = 1\n-- @for k, v in d\n-- @n++\n", Dialect::Generic), ["a", "k", "v", "n"]);
        // &x as far as can be told before a run.
        let src = "-- @var m = 202606 | date yyyymm | string mmyyyy\n-- @set n = (select 1)\n-- @for s in 01..03\nselect * from bis.calls_&m c join ev_&s e on 1 = 1 join t_&n x;\n-- @end for";
        let at = src.find("select *").unwrap();
        let (text, map) = resolved(src, at, Dialect::Oracle).unwrap();
        assert!(text.contains("from bis.calls_062026 c join ev_01 e on 1 = 1 join t_&n x;"), "{text}");
        assert_eq!(map.len(), 2);
        let none = |_| false;
        let m = |s: &str| marks(s, &none).into_iter().map(|r| s[r].to_string()).collect::<Vec<_>>();
        assert_eq!(m("select * from ev_&i where a = :{i+1} and b = &t.db..&t.name and c::int = 1 and d = &x."), ["&i", ":{i+1}", "&t.db.", "&t.name", "&x."]);
    }
}
