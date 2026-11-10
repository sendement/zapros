//! The script editor: egui's TextEdit with our own layout (colours by
//! token, problems on a red background or underlined, the bracket pair at
//! the cursor), line numbers with the current statement marked beside
//! them, auto-indent, paired brackets and quotes, and the hints popup.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Key, Modifiers, Stroke};

use crate::config::{Palette, Rgb, Settings};
use crate::db::Relation;
use crate::i18n::t;
use crate::sql::check::{Issue, Sev};
use crate::sql::complete::{self, Hints};
use crate::sql::lex::{self, Tk};
use crate::sql::{Dialect, statement_at};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    RunStatement,
    RunScript,
    Explain,
    Format,
    Cancel,
    Save,
    Commit,
    Rollback,
}

pub struct Popup {
    pub hints: Hints,
    pub sel: usize,
    pub forced: bool,
}

/// What an editor keeps between frames.
#[derive(Default)]
pub struct EditorState {
    pub popup: Option<Popup>,
    job: Option<(u64, LayoutJob)>,
    /// Cursor (bytes) as of the last frame: (primary, secondary).
    pub cursor: Option<(usize, usize)>,
    /// Put the cursor here (bytes) next frame and focus.
    pub goto: Option<usize>,
    pub focus: bool,
    /// (text hash, cursor) → bracket pair and the statement there.
    around: Option<(u64, usize, Option<(usize, usize)>, Option<Range<usize>>)>,
}

fn text_hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// The bracket pair and the statement at the cursor, worked out once per
/// text and cursor (not every frame).
fn around(st: &mut EditorState, text: &str, at: Option<usize>, cx: &Ctx) -> (Option<(usize, usize)>, Option<Range<usize>>) {
    let Some(at) = at else { return (None, None) };
    let h = text_hash(text);
    if let Some((hh, aa, p, r)) = &st.around {
        if *hh == h && *aa == at {
            return (*p, r.clone());
        }
    }
    let pair = bracket_pair(text, at, cx.dialect);
    let stmt = statement_at(text, at, cx.dialect, cx.settings.blank_line_splits);
    st.around = Some((h, at, pair, stmt.clone()));
    (pair, stmt)
}

pub struct Ctx<'a> {
    pub dialect: Dialect,
    pub settings: &'a Settings,
    pub dark: bool,
    pub issues: &'a [Issue],
    /// Where the server said a statement failed.
    pub server_err: Option<&'a (Range<usize>, String)>,
    pub rels: &'a [Relation],
}

pub struct Out {
    pub changed: bool,
    pub actions: Vec<Action>,
}

pub fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn byte_of(text: &str, ci: usize) -> usize {
    text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b)
}

fn char_of(text: &str, b: usize) -> usize {
    text[..b.min(text.len())].chars().count()
}

fn line_start(text: &str, b: usize) -> usize {
    text[..b].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, b: usize) -> usize {
    text[b..].find('\n').map_or(text.len(), |i| b + i)
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// The bracket by the cursor and its pair.
fn bracket_pair(text: &str, at: usize, d: Dialect) -> Option<(usize, usize)> {
    let toks = lex::lex(text, d);
    let br: Vec<&lex::Tok> = toks.iter().filter(|t| matches!(t.kind, Tk::Open | Tk::Close)).collect();
    let here = br.iter().position(|t| t.start == at).or_else(|| br.iter().position(|t| t.end == at))?;
    let t = br[here];
    if t.kind == Tk::Open {
        let mut depth = 0;
        for b in &br[here..] {
            depth += if b.kind == Tk::Open { 1 } else { -1 };
            if depth == 0 {
                return Some((t.start, b.start));
            }
        }
    } else {
        let mut depth = 0;
        for b in br[..=here].iter().rev() {
            depth += if b.kind == Tk::Close { 1 } else { -1 };
            if depth == 0 {
                return Some((b.start, t.start));
            }
        }
    }
    None
}

const RAINBOW_DARK: [Color32; 4] = [Color32::from_rgb(255, 215, 0), Color32::from_rgb(218, 112, 214), Color32::from_rgb(23, 159, 255), Color32::from_rgb(120, 200, 120)];
const RAINBOW_LIGHT: [Color32; 4] = [Color32::from_rgb(175, 130, 0), Color32::from_rgb(160, 50, 160), Color32::from_rgb(0, 100, 200), Color32::from_rgb(30, 130, 60)];

pub fn error_bg(dark: bool) -> Color32 {
    if dark { Color32::from_rgba_unmultiplied(220, 40, 40, 90) } else { Color32::from_rgba_unmultiplied(255, 60, 60, 70) }
}

fn warn_color(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(230, 170, 40) } else { Color32::from_rgb(200, 120, 0) }
}

/// The text laid out with colours.
pub fn layout(text: &str, cx: &Ctx, pair: Option<(usize, usize)>, font: f32, mono_bold: bool) -> LayoutJob {
    let pal: &Palette = if cx.dark { &cx.settings.dark } else { &cx.settings.light };
    let toks = lex::lex(text, cx.dialect);
    let mono = FontId::monospace(font);
    let bold = if mono_bold && pal.bold_keywords { FontId::new(font, FontFamily::Name("mono-bold".into())) } else { mono.clone() };
    let rainbow = if cx.dark { RAINBOW_DARK } else { RAINBOW_LIGHT };
    // Each token's format.
    let mut depth = 0usize;
    let mut fmts: Vec<TextFormat> = Vec::with_capacity(toks.len());
    for (k, tk) in toks.iter().enumerate() {
        let mut f = TextFormat::simple(mono.clone(), rgb(pal.text));
        match tk.kind {
            Tk::Keyword => {
                let called = toks[k + 1..].iter().find(|x| x.kind != Tk::Space).is_some_and(|x| x.kind == Tk::Open && x.text(text) == "(");
                if called && !matches!(tk.text(text).to_ascii_uppercase().as_str(), "IN" | "EXISTS" | "AS" | "VALUES" | "ON" | "USING" | "OVER" | "AND" | "OR" | "NOT" | "INTO" | "ANY" | "ALL" | "FILTER" | "WITHIN") {
                    f.color = rgb(pal.function);
                } else {
                    f.color = rgb(pal.keyword);
                    f.font_id = bold.clone();
                }
            }
            Tk::Word => {
                let w = tk.text(text);
                let called = toks[k + 1..].iter().find(|x| x.kind != Tk::Space).is_some_and(|x| x.kind == Tk::Open && x.text(text) == "(");
                let after_dot = k > 0 && toks[k - 1].kind == Tk::Dot;
                if called && !after_dot {
                    f.color = rgb(pal.function);
                } else if crate::sql::is_type(w) && !after_dot {
                    f.color = rgb(pal.type_);
                }
            }
            Tk::Str => f.color = rgb(pal.string),
            Tk::Number => f.color = rgb(pal.number),
            Tk::LineComment | Tk::BlockComment => {
                f.color = rgb(pal.comment);
                f.italics = pal.italic_comments;
            }
            Tk::Quoted => f.color = rgb(pal.quoted),
            Tk::Param => f.color = rgb(pal.param),
            Tk::Open => {
                if pal.rainbow {
                    f.color = rainbow[depth % 4];
                }
                depth += 1;
            }
            Tk::Close => {
                depth = depth.saturating_sub(1);
                if pal.rainbow {
                    f.color = rainbow[depth % 4];
                }
            }
            Tk::Op | Tk::Comma | Tk::Semi | Tk::Dot | Tk::Slash => f.color = rgb(pal.operator),
            _ => {}
        }
        fmts.push(f);
    }
    // Overlays: problems, the server's error, the bracket pair.
    let mut over: Vec<(Range<usize>, u8)> = Vec::new();
    for i in cx.issues {
        over.push((i.range.clone(), if i.sev == Sev::Error { 1 } else { 2 }));
    }
    if let Some((r, _)) = cx.server_err {
        over.push((r.clone(), 3));
    }
    if let Some((a, b)) = pair {
        over.push((a..a + 1, 4));
        over.push((b..b + 1, 4));
    }
    let mut cuts: Vec<usize> = toks.iter().map(|t| t.start).collect();
    for (r, _) in &over {
        cuts.push(r.start.min(text.len()));
        cuts.push(r.end.min(text.len()));
    }
    cuts.push(text.len());
    cuts.sort_unstable();
    cuts.dedup();
    let mut job = LayoutJob { text: text.to_string(), ..Default::default() };
    job.wrap.max_width = f32::INFINITY;
    let pair_bg = if cx.dark { Color32::from_rgba_unmultiplied(120, 160, 255, 70) } else { Color32::from_rgba_unmultiplied(80, 120, 255, 60) };
    let mut ti = 0;
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            continue;
        }
        while ti + 1 < toks.len() && toks[ti + 1].start <= a {
            ti += 1;
        }
        let mut f = fmts.get(ti).cloned().unwrap_or_else(|| TextFormat::simple(mono.clone(), rgb(pal.text)));
        for (r, kind) in &over {
            if r.start <= a && b <= r.end.max(r.start + 1) && r.start < text.len() {
                match kind {
                    1 => f.background = error_bg(cx.dark),
                    2 => f.underline = Stroke::new(1.6, warn_color(cx.dark)),
                    3 => {
                        f.underline = Stroke::new(2.0, Color32::from_rgb(235, 60, 60));
                        if f.background == Color32::TRANSPARENT {
                            f.background = error_bg(cx.dark).gamma_multiply(0.6);
                        }
                    }
                    _ => {
                        if f.background == Color32::TRANSPARENT {
                            f.background = pair_bg;
                        }
                    }
                }
            }
        }
        job.sections.push(egui::text::LayoutSection { leading_space: 0.0, byte_range: egui::text::ByteIndex(a)..egui::text::ByteIndex(b), format: f });
    }
    job
}

/// Replaces `r` with `with` and puts the cursor after it (or at "$0").
fn edit(text: &mut String, r: Range<usize>, with: &str) -> usize {
    let (with, mark) = match with.find("$0") {
        Some(i) => (with.replacen("$0", "", 1), Some(i)),
        None => (with.to_string(), None),
    };
    text.replace_range(r.clone(), &with);
    r.start + mark.unwrap_or(with.len())
}

fn set_cursor(ctx: &egui::Context, id: egui::Id, text: &str, a: usize, b: usize) {
    let mut st = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(char_of(text, b)), CCursor::new(char_of(text, a)))));
    st.store(ctx, id);
}

fn accept(text: &mut String, p: &Popup, sel: usize) -> usize {
    let it = &p.hints.items[sel];
    edit(text, p.hints.range.clone(), &it.insert)
}

/// The lines a range touches: start of the first, end of the last.
fn lines_of(text: &str, a: usize, b: usize) -> (usize, usize) {
    let b = if b > a && text[..b].ends_with('\n') { b - 1 } else { b };
    (line_start(text, a), line_end(text, b))
}

fn toggle_comment(text: &mut String, a: usize, b: usize) -> (usize, usize) {
    let (s, e) = lines_of(text, a, b);
    let block = &text[s..e];
    let all = block.lines().filter(|l| !l.trim().is_empty()).all(|l| l.trim_start().starts_with("--"));
    let new: Vec<String> = block
        .split('\n')
        .map(|l| {
            if l.trim().is_empty() {
                l.to_string()
            } else if all {
                let ind = indent_of(l);
                let rest = &l[ind.len()..];
                format!("{ind}{}", rest.strip_prefix("-- ").or_else(|| rest.strip_prefix("--")).unwrap_or(rest))
            } else {
                format!("-- {l}")
            }
        })
        .collect();
    let joined = new.join("\n");
    text.replace_range(s..e, &joined);
    (s, s + joined.len())
}

fn shift_lines(text: &mut String, a: usize, b: usize, tab: usize, out: bool) -> (usize, usize) {
    let (s, e) = lines_of(text, a, b);
    let pad = " ".repeat(tab);
    let new: Vec<String> = text[s..e]
        .split('\n')
        .map(|l| {
            if out {
                let n = l.len() - l.trim_start_matches(' ').len();
                if l.starts_with('\t') { l[1..].to_string() } else { l[n.min(tab)..].to_string() }
            } else if l.is_empty() {
                l.to_string()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect();
    let joined = new.join("\n");
    text.replace_range(s..e, &joined);
    (s, s + joined.len())
}

/// Shortcuts that work when the editor has focus.
fn shortcuts(i: &mut egui::InputState) -> Vec<Action> {
    let mut v = Vec::new();
    let cmd = Modifiers::COMMAND;
    if i.consume_key(cmd | Modifiers::ALT, Key::Enter) {
        v.push(Action::Commit);
    }
    if i.consume_key(cmd | Modifiers::ALT, Key::Backspace) {
        v.push(Action::Rollback);
    }
    if i.consume_key(cmd | Modifiers::SHIFT, Key::Enter) || i.consume_key(Modifiers::NONE, Key::F5) {
        v.push(Action::RunScript);
    }
    if i.consume_key(cmd, Key::Enter) {
        v.push(Action::RunStatement);
    }
    if i.consume_key(cmd | Modifiers::SHIFT, Key::F) || i.consume_key(Modifiers::ALT | Modifiers::SHIFT, Key::F) {
        v.push(Action::Format);
    }
    if i.consume_key(cmd, Key::E) || i.consume_key(Modifiers::NONE, Key::F7) {
        v.push(Action::Explain);
    }
    if i.consume_key(Modifiers::SHIFT, Key::Escape) {
        v.push(Action::Cancel);
    }
    if i.consume_key(cmd, Key::S) {
        v.push(Action::Save);
    }
    v
}

pub fn show(ui: &mut egui::Ui, id: egui::Id, text: &mut String, st: &mut EditorState, cx: &Ctx) -> Out {
    let mut out = Out { changed: false, actions: Vec::new() };
    let ctx = ui.ctx().clone();
    let focused = ctx.memory(|m| m.has_focus(id));
    let tab = cx.settings.tab_width.max(1);
    if let Some(g) = st.goto.take() {
        set_cursor(&ctx, id, text, g, g);
        ctx.memory_mut(|m| m.request_focus(id));
    }
    if std::mem::take(&mut st.focus) {
        ctx.memory_mut(|m| m.request_focus(id));
    }
    let cur = egui::text_edit::TextEditState::load(&ctx, id).and_then(|s| s.cursor.char_range()).map(|r| (byte_of(text, r.primary.index.0), byte_of(text, r.secondary.index.0)));
    let mut typed: Option<String> = None;
    let mut forced = false;
    if focused {
        let (p, s) = cur.unwrap_or((text.len(), text.len()));
        let (a, b) = (p.min(s), p.max(s));
        let mut new_sel: Option<(usize, usize)> = None;
        ui.input_mut(|i| {
            out.actions = shortcuts(i);
            if let Some(pop) = st.popup.as_mut() {
                let n = pop.hints.items.len();
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    pop.sel = (pop.sel + 1) % n;
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    pop.sel = (pop.sel + n - 1) % n;
                }
                if i.consume_key(Modifiers::NONE, Key::PageDown) {
                    pop.sel = (pop.sel + 8).min(n - 1);
                }
                if i.consume_key(Modifiers::NONE, Key::PageUp) {
                    pop.sel = pop.sel.saturating_sub(8);
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    st.popup = None;
                } else if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab) {
                    let c = accept(text, pop, pop.sel);
                    new_sel = Some((c, c));
                    st.popup = None;
                    out.changed = true;
                }
            }
            if i.consume_key(Modifiers::COMMAND, Key::Space) {
                forced = true;
            }
            if i.consume_key(Modifiers::COMMAND, Key::Slash) {
                let (s, e) = toggle_comment(text, a, b);
                new_sel = Some((e, s));
                out.changed = true;
            }
            if i.consume_key(Modifiers::COMMAND, Key::D) {
                let (s, e) = lines_of(text, a, b);
                let block = text[s..e].to_string();
                text.insert_str(e, &format!("\n{block}"));
                new_sel = Some((p + block.len() + 1, p + block.len() + 1));
                out.changed = true;
            }
            if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                let (s, e) = shift_lines(text, a, b, tab, true);
                new_sel = Some((e, s));
                out.changed = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Tab) {
                if text[a..b].contains('\n') {
                    let (s, e) = shift_lines(text, a, b, tab, false);
                    new_sel = Some((e, s));
                } else {
                    let col = text[line_start(text, a)..a].chars().count();
                    let pad = " ".repeat(tab - col % tab);
                    let c = edit(text, a..b, &pad);
                    new_sel = Some((c, c));
                }
                out.changed = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Enter) {
                let ls = line_start(text, a);
                let line = &text[ls..a];
                let mut ind = indent_of(line).to_string();
                let trimmed = line.trim_end().to_ascii_uppercase();
                if trimmed.ends_with('(') || ["BEGIN", "THEN", "LOOP", "ELSE", "DECLARE", "IS", "AS"].iter().any(|w| trimmed.ends_with(w) && trimmed[..trimmed.len() - w.len()].ends_with([' ', '\t']) || trimmed == *w) {
                    ind += &" ".repeat(tab);
                }
                // Between ( and ): the ) on a line of its own.
                let closing = text[b..].starts_with(')') && line.trim_end().ends_with('(');
                let ins = if closing { format!("\n{ind}$0\n{}", indent_of(line)) } else { format!("\n{ind}") };
                let c = edit(text, a..b, &ins);
                new_sel = Some((c, c));
                out.changed = true;
            }
            // Typed text: brackets and quotes in pairs, closing ones stepped over.
            let mut k = 0;
            while k < i.events.len() {
                if let egui::Event::Text(ev) = &i.events[k] {
                    let ev = ev.clone();
                    typed = Some(ev.clone());
                    if cx.settings.auto_pairs && ev.chars().count() == 1 {
                        let ch = ev.chars().next().unwrap();
                        let (p0, s0) = new_sel.unwrap_or((p, s));
                        let (a0, b0) = (p0.min(s0), p0.max(s0));
                        let next = text[b0..].chars().next();
                        let prev = text[..a0].chars().next_back();
                        let handled = match ch {
                            ')' | ']' if next == Some(ch) && a0 == b0 => Some((a0 + 1, a0 + 1)),
                            '\'' | '"' if next == Some(ch) && a0 == b0 => Some((a0 + 1, a0 + 1)),
                            '(' | '[' => {
                                let close = if ch == '(' { ")" } else { "]" };
                                if a0 != b0 {
                                    let inner = text[a0..b0].to_string();
                                    text.replace_range(a0..b0, &format!("{ch}{inner}{close}"));
                                    Some((a0 + 1, b0 + 1))
                                } else if next.is_none_or(|n| n.is_whitespace() || ")],;".contains(n)) {
                                    text.insert_str(a0, &format!("{ch}{close}"));
                                    Some((a0 + 1, a0 + 1))
                                } else {
                                    None
                                }
                            }
                            '\'' | '"' if a0 == b0 && prev.is_none_or(|c| !c.is_alphanumeric() && c != ch) && next.is_none_or(|n| n.is_whitespace() || ")],;".contains(n)) => {
                                text.insert_str(a0, &format!("{ch}{ch}"));
                                Some((a0 + 1, a0 + 1))
                            }
                            _ => None,
                        };
                        if let Some(c) = handled {
                            i.events.remove(k);
                            new_sel = Some(c);
                            out.changed = true;
                            continue;
                        }
                    }
                }
                k += 1;
            }
            // Backspace between an empty pair takes both.
            if cx.settings.auto_pairs && a == b && a > 0 && new_sel.is_none() {
                let pair = &text[a.saturating_sub(1).min(a)..(a + 1).min(text.len())];
                if matches!(pair, "()" | "[]" | "''" | "\"\"") && text.is_char_boundary(a - 1) && i.consume_key(Modifiers::NONE, Key::Backspace) {
                    text.replace_range(a - 1..a + 1, "");
                    new_sel = Some((a - 1, a - 1));
                    out.changed = true;
                }
            }
        });
        if let Some((x, y)) = new_sel {
            set_cursor(&ctx, id, text, x, y);
        }
    }

    // Line numbers' room.
    let font = cx.settings.font_size;
    let lines = text.matches('\n').count() + 1;
    let digits = lines.to_string().len().max(3) as f32;
    let char_w = ui.fonts_mut(|f| f.glyph_width(&FontId::monospace(font), '0'));
    let gutter = digits * char_w + 18.0;
    let cursor_now = egui::text_edit::TextEditState::load(&ctx, id).and_then(|s| s.cursor.char_range()).map(|r| byte_of(text, r.primary.index.0));
    let mono_bold = ctx.fonts(|f| f.families().iter().any(|fam| *fam == FontFamily::Name("mono-bold".into())));
    let (pair, _) = around(st, text, cursor_now, cx);
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (cx.dialect, cx.dark, cx.issues, cx.server_err, pair, font.to_bits(), mono_bold).hash(&mut h);
        format!("{:?}{:?}", cx.settings.dark, cx.settings.light).hash(&mut h);
        h.finish()
    };
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _w: f32| -> Arc<egui::Galley> {
        let s = buf.as_str();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (key, s).hash(&mut h);
        let k = h.finish();
        let job = match &st.job {
            Some((kk, j)) if *kk == k => j.clone(),
            _ => {
                let j = layout(s, cx, pair, font, mono_bold);
                st.job = Some((k, j.clone()));
                j
            }
        };
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let before = text.clone();
    let output = ui
        .horizontal_top(|ui| {
            ui.add_space(gutter);
            egui::TextEdit::multiline(text)
        .id(id)
        .font(FontId::monospace(font))
        .code_editor()
        .lock_focus(true)
        .desired_width(f32::INFINITY)
        .desired_rows(30)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin { left: 0, right: 8, top: 4, bottom: 40 })
        .layouter(&mut layouter)
        .show(ui)
        })
        .inner;
    if *text != before {
        out.changed = true;
    }

    // Line numbers, the current statement's bar.
    let painter = ui.painter().clone();
    let weak = ui.visuals().weak_text_color();
    let strong = ui.visuals().strong_text_color();
    let cursor_b = output.cursor_range.map(|r| byte_of(text, r.primary.index.0)).or(cursor_now);
    let cur_line = cursor_b.map(|c| text[..c.min(text.len())].matches('\n').count());
    let (_, stmt) = around(st, text, cursor_b, cx);
    let stmt_lines = stmt.map(|r| (text[..r.start].matches('\n').count(), text[..r.end].matches('\n').count()));
    let err_lines: std::collections::HashSet<usize> = cx.issues.iter().filter(|i| i.sev == Sev::Error).map(|i| text[..i.range.start.min(text.len())].matches('\n').count()).collect();
    let clip = ui.clip_rect();
    let mut line = 0;
    let mut new_line = true;
    // (An empty text's row has no height: a line's worth.)
    let row_h = painter.layout_no_wrap("0".into(), FontId::monospace(font), weak).size().y;
    for row in &output.galley.rows {
        let mut r = row.rect().translate(output.galley_pos.to_vec2());
        if r.height() < 1.0 {
            r.max.y = r.min.y + row_h;
        }
        if new_line && r.bottom() >= clip.top() && r.top() <= clip.bottom() {
            let color = if Some(line) == cur_line { strong } else { weak };
            let x = output.galley_pos.x - 10.0;
            painter.text(egui::pos2(x, r.center().y), egui::Align2::RIGHT_CENTER, (line + 1).to_string(), FontId::monospace(font * 0.85), color);
            if stmt_lines.is_some_and(|(a, b)| line >= a && line <= b) {
                painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x + 3.0, r.top()), egui::pos2(x + 6.0, r.bottom())), 1.0, ui.visuals().selection.bg_fill);
            }
            if err_lines.contains(&line) {
                painter.circle_filled(egui::pos2(output.galley_pos.x - gutter + 7.0, r.center().y), 3.0, Color32::from_rgb(230, 70, 70));
            }
        }
        new_line = row.ends_with_newline;
        if new_line {
            line += 1;
        }
    }

    // Problems under the pointer: what they are.
    if let Some(pos) = output.response.hover_pos() {
        let c = output.galley.cursor_from_pos(pos - output.galley_pos);
        let b = byte_of(text, c.index.0);
        let msgs: Vec<String> = cx
            .issues
            .iter()
            .filter(|i| i.range.start <= b && b < i.range.end.max(i.range.start + 1))
            .map(|i| format!("{} {}", if i.sev == Sev::Error { "⛔" } else { "⚠" }, i.msg))
            .chain(cx.server_err.filter(|(r, _)| r.start <= b && b <= r.end).map(|(_, m)| format!("⚡ {m}")))
            .collect();
        if !msgs.is_empty() {
            egui::Tooltip::always_open(ctx.clone(), ui.layer_id(), id.with("issue"), egui::PopupAnchor::Pointer).show(|ui| {
                for m in msgs {
                    ui.label(m);
                }
            });
        }
    }

    // Hints.
    let cursor_b = output.cursor_range.map(|r| byte_of(text, r.primary.index.0));
    let has_focus = output.response.has_focus();
    if !has_focus {
        st.popup = None;
    }
    if let Some(c) = cursor_b.filter(|_| has_focus && cx.settings.hints) {
        let word_char = typed.as_deref().is_some_and(|s| s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.'));
        let moved = st.cursor.is_some_and(|(p, _)| p != c) && !out.changed;
        if moved && !forced {
            st.popup = None;
        }
        let want = forced || (out.changed && st.popup.is_some()) || (cx.settings.hints_auto && word_char && out.changed);
        if want {
            let forced = forced || st.popup.as_ref().is_some_and(|p| p.forced);
            match complete::hints(text, c, cx.dialect, cx.rels, cx.settings.hint_case, forced) {
                Some(h) => {
                    let keep = st.popup.as_ref().and_then(|p| p.hints.items.get(p.sel)).and_then(|it| h.items.iter().position(|x| x == it)).unwrap_or(0);
                    st.popup = Some(Popup { hints: h, sel: keep, forced });
                }
                None => st.popup = None,
            }
        }
    }
    st.cursor = output.cursor_range.map(|r| (byte_of(text, r.primary.index.0), byte_of(text, r.secondary.index.0)));
    let mut picked: Option<usize> = None;
    if let (Some(pop), Some(cr)) = (st.popup.as_ref(), output.cursor_range) {
        let at = output.galley.pos_from_cursor(cr.primary).translate(output.galley_pos.to_vec2());
        egui::Area::new(id.with("hints")).order(egui::Order::Foreground).fixed_pos(at.left_bottom() + egui::vec2(0.0, 2.0)).show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(260.0);
                ui.set_max_width(520.0);
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for (k, it) in pop.hints.items.iter().enumerate() {
                        let sel = k == pop.sel;
                        let r = ui
                            .horizontal(|ui| {
                                let resp = ui.add(egui::Button::selectable(sel, egui::RichText::new(format!("{}  {}", it.kind.icon(), it.label)).monospace()).frame_when_inactive(false));
                                if !it.detail.is_empty() {
                                    ui.label(egui::RichText::new(&it.detail).weak().small());
                                }
                                resp
                            })
                            .inner;
                        if sel {
                            r.scroll_to_me(None);
                        }
                        if r.clicked() {
                            picked = Some(k);
                        }
                    }
                });
                ui.label(egui::RichText::new(t("↑↓ выбор · Enter/Tab вставить · Esc закрыть", "↑↓ choose · Enter/Tab insert · Esc close")).weak().small());
            });
        });
    }
    if let Some(k) = picked {
        let pop = st.popup.take().unwrap();
        let c = accept(text, &pop, k);
        set_cursor(&ctx, id, text, c, c);
        ctx.memory_mut(|m| m.request_focus(id));
        out.changed = true;
    }
    out
}

/// The selection (bytes), if any; else None.
pub fn selection(st: &EditorState) -> Option<Range<usize>> {
    st.cursor.filter(|(a, b)| a != b).map(|(a, b)| a.min(b)..a.max(b))
}
