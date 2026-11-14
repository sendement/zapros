//! The reference of functions, operators and keywords (help.txt): what
//! Ctrl+click on one of them shows — how it's written, what it does, an
//! example.

use std::sync::OnceLock;

use super::Dialect;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub names: Vec<String>,
    /// "pg", "ch", "ora", "lite"; empty: every database.
    pub dialects: Vec<String>,
    /// 'f' function, 'o' operator, 'k' keyword.
    pub kind: char,
    pub sig: String,
    pub ru: Vec<String>,
    pub en: Vec<String>,
    pub example: Vec<String>,
}

impl Entry {
    /// What it does, in the program's language.
    pub fn about(&self) -> String {
        let v = if crate::i18n::is_ru() && !self.ru.is_empty() { &self.ru } else { &self.en };
        v.join("\n")
    }

    pub fn title(&self) -> String {
        self.names.join(", ")
    }

    pub fn example(&self) -> String {
        self.example.join("\n")
    }

    fn fits(&self, d: Dialect) -> bool {
        self.dialects.is_empty() || d == Dialect::Generic || self.dialects.iter().any(|x| x == tag(d))
    }
}

fn tag(d: Dialect) -> &'static str {
    match d {
        Dialect::Postgres => "pg",
        Dialect::ClickHouse => "ch",
        Dialect::Oracle => "ora",
        Dialect::Sqlite => "lite",
        Dialect::Generic => "",
    }
}

/// help.txt read into entries.
pub fn parse(src: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for line in src.lines() {
        let line = line.trim_end();
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(head) = line.strip_prefix("@ ") {
            let parts: Vec<&str> = head.split(" | ").collect();
            let names = parts[0].split(", ").map(|s| s.trim().to_string()).collect();
            let dialects = parts.get(1).map_or(Vec::new(), |d| if d.trim() == "*" { Vec::new() } else { d.split_whitespace().map(str::to_string).collect() });
            let kind = parts.get(2).and_then(|k| k.trim().chars().next()).unwrap_or('f');
            out.push(Entry { names, dialects, kind, sig: String::new(), ru: Vec::new(), en: Vec::new(), example: Vec::new() });
            continue;
        }
        let Some(e) = out.last_mut() else { continue };
        let (key, val) = line.split_once(": ").unwrap_or((line, ""));
        match key {
            "sig" => e.sig = val.to_string(),
            "ru" => e.ru.push(val.to_string()),
            "en" => e.en.push(val.to_string()),
            // (An example keeps its own indent after "ex: ".)
            "ex" => e.example.push(val.to_string()),
            _ => {}
        }
    }
    out
}

pub fn entries() -> &'static [Entry] {
    static E: OnceLock<Vec<Entry>> = OnceLock::new();
    E.get_or_init(|| parse(include_str!("help.txt")))
}

/// The entry for `name` (a word or an operator) in dialect `d`; `call`:
/// written as a call (a function before a keyword then). The dialect's
/// own entry before one for every database.
pub fn lookup(name: &str, d: Dialect, call: bool) -> Option<&'static Entry> {
    let all = entries();
    let named: Vec<&Entry> = all.iter().filter(|e| e.fits(d) && e.names.iter().any(|n| n.eq_ignore_ascii_case(name))).collect();
    let want = |e: &&Entry| if call { e.kind == 'f' } else { e.kind != 'f' };
    let own = |e: &&Entry| !e.dialects.is_empty();
    named
        .iter()
        .find(|e| want(e) && own(e))
        .or_else(|| named.iter().find(|e| want(e)))
        .or_else(|| named.iter().find(|e| own(e)))
        .or_else(|| named.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_reads_and_finds() {
        let all = entries();
        assert!(all.len() > 120, "{}", all.len());
        for e in all {
            assert!(!e.sig.is_empty() && !e.ru.is_empty() && !e.en.is_empty() && !e.example.is_empty(), "{:?}", e.names);
        }
        // The dialect's own first: length counts bytes in ClickHouse.
        assert!(lookup("length", Dialect::ClickHouse, true).unwrap().about().contains("БАЙТАХ") || lookup("length", Dialect::ClickHouse, true).unwrap().en[0].contains("BYTES"));
        assert_eq!(lookup("length", Dialect::Postgres, true).unwrap().dialects, ["pg", "ora", "lite"]);
        // A call or a keyword: LEFT JOIN or left(s, n).
        assert_eq!(lookup("left", Dialect::Postgres, false).unwrap().kind, 'k');
        assert_eq!(lookup("LEFT", Dialect::Postgres, true).unwrap().kind, 'f');
        // Operators; ones a database lacks aren't found.
        assert!(lookup("::", Dialect::Postgres, false).is_some());
        assert!(lookup("::", Dialect::Oracle, false).is_none());
        assert!(lookup("coalesce", Dialect::Oracle, true).is_some());
        assert!(lookup("nvl", Dialect::Postgres, true).is_none());
        // An example keeps its lines.
        assert!(lookup("over", Dialect::Postgres, false).unwrap().example().lines().count() > 2);
    }
}
