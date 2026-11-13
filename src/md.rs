//! Markdown made from a menu (a text cell's right click while editing):
//! marks round the picked text (toggled), prefixes on its lines, blocks.

use crate::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Heading(u8),
    Bold,
    Italic,
    Strike,
    Code,
    Link,
    Bullet,
    Numbered,
    Check,
    Quote,
    CodeBlock,
    Table,
    Rule,
}

impl Op {
    pub fn label(self) -> String {
        match self {
            Op::Heading(n) => format!("{} {n}", t("Заголовок", "Heading")),
            Op::Bold => t("Жирный", "Bold").into(),
            Op::Italic => t("Курсив", "Italic").into(),
            Op::Strike => t("Зачёркнутый", "Strikethrough").into(),
            Op::Code => t("Код в строке", "Inline code").into(),
            Op::Link => t("Ссылка", "Link").into(),
            Op::Bullet => t("Список", "Bullet list").into(),
            Op::Numbered => t("Нумерованный список", "Numbered list").into(),
            Op::Check => t("Чек-лист", "Checklist").into(),
            Op::Quote => t("Цитата", "Quote").into(),
            Op::CodeBlock => t("Блок кода", "Code block").into(),
            Op::Table => t("Таблица", "Table").into(),
            Op::Rule => t("Разделитель", "Divider").into(),
        }
    }

    /// What it looks like written.
    pub fn hint(self) -> &'static str {
        match self {
            Op::Heading(1) => "#",
            Op::Heading(2) => "##",
            Op::Heading(_) => "###",
            Op::Bold => "**…**",
            Op::Italic => "*…*",
            Op::Strike => "~~…~~",
            Op::Code => "`…`",
            Op::Link => "[…](url)",
            Op::Bullet => "- ",
            Op::Numbered => "1. ",
            Op::Check => "- [ ] ",
            Op::Quote => "> ",
            Op::CodeBlock => "```",
            Op::Table => "| | |",
            Op::Rule => "---",
        }
    }
}

fn line_start(s: &str, at: usize) -> usize {
    s[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(s: &str, at: usize) -> usize {
    s[at..].find('\n').map_or(s.len(), |i| at + i)
}

/// A line's list / quote / heading mark (its length), if any.
fn mark_len(l: &str) -> usize {
    let t = l.trim_start();
    let lead = l.len() - t.len();
    for m in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "> "] {
        if t.starts_with(m) {
            return lead + m.len();
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && t[digits..].starts_with(". ") {
        return lead + digits + 2;
    }
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
        return lead + hashes + 1;
    }
    0
}

/// `op` on `text` with `a..b` picked: the new text and what's picked after.
pub fn apply(text: &str, a: usize, b: usize, op: Op) -> (String, usize, usize) {
    let (a, b) = (a.min(b).min(text.len()), a.max(b).min(text.len()));
    let wrap = |l: &str, r: &str, placeholder: &str| -> (String, usize, usize) {
        // Already wrapped: unwrapped.
        if a >= l.len() && text[..a].ends_with(l) && text[b..].starts_with(r) {
            let mut s = text.to_string();
            s.replace_range(b..b + r.len(), "");
            s.replace_range(a - l.len()..a, "");
            return (s, a - l.len(), b - l.len());
        }
        let inner = if a == b { placeholder } else { &text[a..b] };
        let s = format!("{}{l}{inner}{r}{}", &text[..a], &text[b..]);
        (s, a + l.len(), a + l.len() + inner.len())
    };
    match op {
        Op::Bold => wrap("**", "**", t("текст", "text")),
        Op::Italic => wrap("*", "*", t("текст", "text")),
        Op::Strike => wrap("~~", "~~", t("текст", "text")),
        Op::Code => wrap("`", "`", "code"),
        Op::Link => {
            let inner = if a == b { t("текст", "text").to_string() } else { text[a..b].to_string() };
            let s = format!("{}[{inner}](https://){}", &text[..a], &text[b..]);
            let url = a + 1 + inner.len() + 2;
            (s, url, url + "https://".len())
        }
        Op::Heading(_) | Op::Bullet | Op::Numbered | Op::Check | Op::Quote => {
            let (ls, le) = (line_start(text, a), line_end(text, if b > a && text[..b].ends_with('\n') { b - 1 } else { b }));
            let lines: Vec<&str> = text[ls..le].split('\n').collect();
            let want = |i: usize| match op {
                Op::Heading(n) => format!("{} ", "#".repeat(n as usize)),
                Op::Bullet => "- ".into(),
                Op::Numbered => format!("{}. ", i + 1),
                Op::Check => "- [ ] ".into(),
                _ => "> ".into(),
            };
            // All already so (the same kind): marks off; else on (another
            // kind's mark replaced).
            let same = |l: &str, i: usize| {
                let m = &l[..mark_len(l)];
                let m = m.trim_start();
                match op {
                    Op::Numbered => m.ends_with(". ") && m[..m.len() - 2].chars().all(|c| c.is_ascii_digit()) && !m.is_empty(),
                    _ => m == want(i),
                }
            };
            let off = lines.iter().enumerate().all(|(i, l)| l.trim().is_empty() || same(l, i));
            let mut k = 0;
            let out: Vec<String> = lines
                .iter()
                .map(|l| {
                    if l.trim().is_empty() && lines.len() > 1 {
                        return l.to_string();
                    }
                    let rest = &l[mark_len(l)..];
                    let lead = &l[..l.len() - l.trim_start().len()];
                    let s = if off { format!("{lead}{rest}") } else { format!("{lead}{}{rest}", want(k)) };
                    k += 1;
                    s
                })
                .collect();
            let body = out.join("\n");
            let s = format!("{}{body}{}", &text[..ls], &text[le..]);
            (s, ls + body.len(), ls + body.len())
        }
        Op::CodeBlock => {
            let (ls, le) = (line_start(text, a), line_end(text, b));
            let inner = &text[ls..le];
            let s = format!("{}```sql\n{inner}\n```{}", &text[..ls], &text[le..]);
            let at = ls + 7;
            (s, at, at + inner.len())
        }
        Op::Table => {
            let le = line_end(text, b);
            let block = format!("\n\n| {c1} | {c2} |\n| --- | --- |\n|  |  |\n", c1 = t("Колонка 1", "Column 1"), c2 = t("Колонка 2", "Column 2"));
            let s = format!("{}{block}{}", &text[..le], &text[le..]);
            let at = le + 4;
            (s, at, at + t("Колонка 1", "Column 1").len())
        }
        Op::Rule => {
            let le = line_end(text, b);
            let s = format!("{}\n\n---\n{}", &text[..le], &text[le..]);
            (s, le + 6, le + 6)
        }
    }
}

pub const MENU: &[&[Op]] = &[
    &[Op::Bold, Op::Italic, Op::Strike, Op::Code, Op::Link],
    &[Op::Heading(1), Op::Heading(2), Op::Heading(3)],
    &[Op::Bullet, Op::Numbered, Op::Check, Op::Quote],
    &[Op::CodeBlock, Op::Table, Op::Rule],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_and_lines() {
        let s = "total by day";
        assert_eq!(apply(s, 0, 5, Op::Bold), ("**total** by day".into(), 2, 7));
        // Again: off.
        assert_eq!(apply("**total** by day", 2, 7, Op::Bold).0, "total by day");
        assert_eq!(apply(s, 3, 3, Op::Code), ("totcodeal by day".replace("totcodeal", "tot`code`al"), 4, 8));
        let l = "one\ntwo\nthree";
        assert_eq!(apply(l, 0, l.len(), Op::Numbered).0, "1. one\n2. two\n3. three");
        assert_eq!(apply("1. one\n2. two", 0, 13, Op::Numbered).0, "one\ntwo");
        // Another kind's mark: replaced.
        assert_eq!(apply("- one\n- two", 0, 11, Op::Check).0, "- [ ] one\n- [ ] two");
        assert_eq!(apply("## Old", 3, 3, Op::Heading(1)).0, "# Old");
        assert_eq!(apply("# Old", 3, 3, Op::Heading(1)).0, "Old");
        assert_eq!(apply("select 1", 0, 8, Op::CodeBlock).0, "```sql\nselect 1\n```");
        assert!(apply("x", 1, 1, Op::Table).0.starts_with(&format!("x\n\n| {} | {} |\n| --- | --- |", t("Колонка 1", "Column 1"), t("Колонка 2", "Column 2"))));
        let w = t("текст", "text");
        assert_eq!(apply("a", 1, 1, Op::Link), (format!("a[{w}](https://)"), 4 + w.len(), 12 + w.len()));
    }
}
