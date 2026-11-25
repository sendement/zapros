//! Block (column) selection: a rectangle of lines × columns, picked with
//! Alt and the mouse; typing, Backspace / Delete and pasting act on each of
//! its lines at once.

/// Lines and columns (in characters; a column may lie past a line's end).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub anchor: (usize, usize),
    pub head: (usize, usize),
    /// Stuck to the lines' ends: typing goes right after each (not blank)
    /// line's text, its trailing spaces gone.
    pub tail: bool,
}

/// What's done to every line of the block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Put in (in place of the block's columns): one piece for every line,
    /// or the same one on all.
    Insert(Vec<String>),
    Backspace,
    Delete,
}

impl Block {
    pub fn at(line: usize, col: usize) -> Self {
        Self { anchor: (line, col), head: (line, col), tail: false }
    }

    /// First and last line.
    pub fn lines(&self) -> (usize, usize) {
        (self.anchor.0.min(self.head.0), self.anchor.0.max(self.head.0))
    }

    /// Left and right column.
    pub fn cols(&self) -> (usize, usize) {
        (self.anchor.1.min(self.head.1), self.anchor.1.max(self.head.1))
    }

    fn set_cols(&mut self, c: usize) {
        self.anchor.1 = c;
        self.head.1 = c;
    }
}

/// Every line's byte range (without its `\n`).
pub fn spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut s = 0;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            out.push((s, i));
            s = i + 1;
        }
    }
    out.push((s, text.len()));
    out
}

/// The byte (in `line`) of character column `col`, the end when past it.
pub fn byte_at(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(b, _)| b)
}

/// The block's text: its columns of every line, a line each.
pub fn text_of(text: &str, b: &Block) -> String {
    let sp = spans(text);
    let (l0, l1) = b.lines();
    let (c0, c1) = b.cols();
    (l0..=l1.min(sp.len() - 1))
        .map(|l| {
            let line = &text[sp[l].0..sp[l].1];
            line[byte_at(line, c0)..byte_at(line, c1)].to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The block lies past its lines' text (all but the blank ones), and
/// sticking to their ends would change something: spaces padded or
/// left behind.
pub fn past_ends(text: &str, b: &Block) -> bool {
    if b.tail {
        return false;
    }
    let sp = spans(text);
    let (l0, l1) = b.lines();
    let c0 = b.cols().0;
    let mut any = false;
    for l in l0..=l1.min(sp.len() - 1) {
        let line = &text[sp[l].0..sp[l].1];
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        let n = t.chars().count();
        if c0 < n {
            return false;
        }
        any |= c0 > n || t.len() < line.len();
    }
    any
}

/// The block's lines that typing goes to when stuck to their ends.
fn filled(text: &str, l: usize, sp: &[(usize, usize)]) -> bool {
    !text[sp[l].0..sp[l].1].trim().is_empty()
}

/// Where the text's caret goes: the block's head (in a stuck block: its
/// line's end).
pub fn caret(text: &str, b: &Block) -> usize {
    let sp = spans(text);
    let hl = b.head.0.min(sp.len() - 1);
    let line = &text[sp[hl].0..sp[hl].1];
    if b.tail { sp[hl].0 + line.trim_end().len() } else { sp[hl].0 + byte_at(line, b.head.1) }
}

/// The caret's spot on each line of a stuck block: (line, column).
pub fn tail_spots(text: &str, b: &Block) -> Vec<(usize, usize)> {
    let sp = spans(text);
    let (l0, l1) = b.lines();
    (l0..=l1.min(sp.len() - 1)).filter(|&l| filled(text, l, &sp)).map(|l| (l, text[sp[l].0..sp[l].1].trim_end().chars().count())).collect()
}

/// Does `op` to every line of the block; the block after it (a caret
/// as tall as the block).
pub fn edit(text: &mut String, b: &mut Block, op: &Op) {
    let sp = spans(text);
    if b.tail {
        let (l0, l1) = b.lines();
        let l1 = l1.min(sp.len() - 1);
        for l in (l0..=l1).rev() {
            if !filled(text, l, &sp) {
                continue;
            }
            let (s, e) = sp[l];
            let end = s + text[s..e].trim_end().len();
            match op {
                Op::Insert(pieces) => {
                    let piece = if pieces.len() == l1 - l0 + 1 { &pieces[l - l0] } else { &pieces[0] };
                    text.replace_range(end..e, piece);
                }
                Op::Backspace => {
                    let p = text[s..end].char_indices().next_back().map_or(end, |(i, _)| s + i);
                    text.replace_range(p..e, "");
                }
                Op::Delete => {}
            }
        }
        return;
    }
    let (l0, l1) = b.lines();
    let l1 = l1.min(sp.len() - 1);
    let (c0, c1) = b.cols();
    let wide = c0 < c1;
    // From the last line up: earlier byte ranges stay right.
    for l in (l0..=l1).rev() {
        let (s, e) = sp[l];
        let line = &text[s..e];
        let n = line.chars().count();
        let (a, z) = (s + byte_at(line, c0), s + byte_at(line, c1));
        match op {
            Op::Insert(pieces) => {
                let piece = if pieces.len() == l1 - l0 + 1 { &pieces[l - l0] } else { &pieces[0] };
                let pad = " ".repeat(c0.saturating_sub(n));
                text.replace_range(a..z, &format!("{pad}{piece}"));
            }
            Op::Backspace if wide => text.replace_range(a..z, ""),
            Op::Backspace if c0 > 0 && n >= c0 => {
                let p = s + byte_at(line, c0 - 1);
                text.replace_range(p..a, "");
            }
            Op::Delete if wide => text.replace_range(a..z, ""),
            Op::Delete if n > c0 => {
                let q = s + byte_at(line, c0 + 1);
                text.replace_range(a..q, "");
            }
            _ => {}
        }
    }
    let to = match op {
        Op::Insert(pieces) => c0 + pieces.first().map_or(0, |p| p.chars().count()),
        Op::Backspace if !wide => c0.saturating_sub(1),
        _ => c0,
    };
    b.set_cols(to);
}

/// What a paste puts on each line: as many lines as the block, one each;
/// else the text (as one line) on all.
pub fn pieces(paste: &str, b: &Block) -> Vec<String> {
    let (l0, l1) = b.lines();
    let lines: Vec<&str> = paste.trim_end_matches('\n').split('\n').map(|l| l.trim_end_matches('\r')).collect();
    if lines.len() == l1 - l0 + 1 {
        lines.into_iter().map(String::from).collect()
    } else {
        vec![lines.join(" ")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "select a,\n       bb,\n       c\n  from t";

    #[test]
    fn typing_on_every_line() {
        let mut t = T.to_string();
        let mut b = Block { anchor: (0, 7), head: (2, 7), tail: false };
        edit(&mut t, &mut b, &Op::Insert(vec!["x.".into()]));
        assert_eq!(t, "select x.a,\n       x.bb,\n       x.c\n  from t");
        assert_eq!(b.cols(), (9, 9));
        // Backspace with a caret: the char before it, on each line.
        edit(&mut t, &mut b, &Op::Backspace);
        assert_eq!(t, "select xa,\n       xbb,\n       xc\n  from t");
        assert_eq!(b.cols(), (8, 8));
    }

    #[test]
    fn a_rectangle_deleted_copied_replaced() {
        let mut t = T.to_string();
        let mut b = Block { anchor: (1, 7), head: (0, 9), tail: false };
        assert_eq!(text_of(&t, &b), "a,\nbb");
        edit(&mut t, &mut b, &Op::Delete);
        assert_eq!(t, "select \n       ,\n       c\n  from t");
        assert_eq!(b.cols(), (7, 7));
        let mut b = Block { anchor: (0, 0), head: (1, 6), tail: false };
        let mut t = T.to_string();
        let p = pieces("SELECT\n------", &b);
        edit(&mut t, &mut b, &Op::Insert(p));
        assert_eq!(t, "SELECT a,\n------ bb,\n       c\n  from t");
    }

    #[test]
    fn stuck_to_line_ends() {
        let t = "select 'a,\n       'bb  ,\n\n       'c\n  from t";
        let mut b = Block { anchor: (0, 14), head: (3, 14), tail: false };
        // Line 1 has text up to col 13 ("'bb  ,"): past all of them.
        assert!(past_ends(t, &b));
        b.tail = true;
        let mut s = t.to_string();
        edit(&mut s, &mut b, &Op::Insert(vec!["'".into()]));
        edit(&mut s, &mut b, &Op::Insert(vec![")".into()]));
        assert_eq!(s, "select 'a,')\n       'bb  ,')\n\n       'c')\n  from t");
        edit(&mut s, &mut b, &Op::Backspace);
        assert_eq!(s, "select 'a,'\n       'bb  ,'\n\n       'c'\n  from t");
        assert_eq!(tail_spots(&s, &b), [(0, 11), (1, 14), (3, 10)]);
        assert_eq!(caret(&s, &b), s.find("c'").unwrap() + 2);
        // Inside a line's text: no sticking.
        assert!(!past_ends(t, &Block { anchor: (0, 9), head: (3, 9), tail: false }));
        // Already right at the ends: nothing to stick.
        assert!(!past_ends("ab\ncd", &Block { anchor: (0, 2), head: (1, 2), tail: false }));
    }

    #[test]
    fn short_lines_padded() {
        let mut t = "abcdef\nab\nabcd".to_string();
        let mut b = Block { anchor: (0, 4), head: (2, 4), tail: false };
        edit(&mut t, &mut b, &Op::Insert(vec!["|".into()]));
        assert_eq!(t, "abcd|ef\nab  |\nabcd|");
        // Delete past a short line's end does nothing there.
        let mut t = "abcdef\nab".to_string();
        let mut b = Block { anchor: (0, 3), head: (1, 3), tail: false };
        edit(&mut t, &mut b, &Op::Delete);
        assert_eq!(t, "abcef\nab");
    }
}
