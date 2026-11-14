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
use crate::i18n::t as tr;
use crate::sql::check::{Issue, Sev};
use crate::sql::complete::{self, Hints};
use crate::sql::lex::{self, Tk};
use crate::sql::{Dialect, statement_at};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    RunStatement,
    RunScript,
    Explain,
    Format,
    Cancel,
    Save,
    Commit,
    Rollback,
    /// Ctrl+click on a name of the database's: open it (a table's
    /// designer, a function's code…).
    Open { schema: Option<String>, name: String, call: bool },
}

pub struct Popup {
    pub hints: Hints,
    /// The row picked; with `typed_first`, row 0 is the word as typed and
    /// the hints start at 1.
    pub sel: usize,
    pub forced: bool,
    pub typed_first: bool,
}

impl Popup {
    /// Rows shown.
    fn rows(&self) -> usize {
        self.hints.items.len() + self.typed_first as usize
    }

    /// The hint a row is (None: the word as typed).
    fn item(&self, row: usize) -> Option<usize> {
        if self.typed_first { row.checked_sub(1) } else { Some(row) }
    }
}

/// What an editor keeps between frames.
#[derive(Default)]
pub struct EditorState {
    pub popup: Option<Popup>,
    job: Option<(u64, LayoutJob)>,
    /// The text's brackets, CASEs and blocks, as of `job`.
    blocks: Vec<crate::sql::bands::Block>,
    /// The rainbow bands as of `job` (painted under the text).
    bands: Vec<(Range<usize>, Color32)>,
    /// (layout key, block start) → the x its lines' text starts at.
    frame_x: std::collections::HashMap<(u64, usize), f32>,
    /// What can be folded: (its first line's start, its end), as of `job`.
    foldable: Vec<(usize, usize)>,
    /// Folded: the starts of the lines whose blocks are folded.
    pub folds: Vec<usize>,
    /// The caret: where, and since when (for the same-word marks).
    caret_since: Option<(usize, std::time::Instant)>,
    /// (layout key, word) → where the word is (bytes).
    words: Option<(u64, String, Vec<usize>)>,
    /// (layout key, byte under the pointer) → its bracket pair.
    hover_pair: Option<(u64, usize, Option<(usize, usize)>)>,
    /// Cursor (bytes) as of the last frame: (primary, secondary).
    pub cursor: Option<(usize, usize)>,
    /// Put the cursor here (bytes) next frame and focus.
    pub goto: Option<usize>,
    /// Pick this (bytes) next frame, focus, and scroll it into sight.
    pub select: Option<(usize, usize)>,
    /// A pick waiting to be scrolled to (once laid out).
    scroll_to: Option<(usize, usize)>,
    /// What a search found here (lit under the text), and the one it's on.
    pub marks: Vec<Range<usize>>,
    pub mark_now: Option<Range<usize>>,
    pub focus: bool,
    /// A block (column) selection, made with Alt and the mouse.
    pub block: Option<crate::block::Block>,
    /// The mouse is making the block now.
    block_drag: bool,
    /// Where a Ctrl+click led (lit a moment).
    flash: Option<(Range<usize>, std::time::Instant)>,
    /// The text's rect, as of the last frame (where an Alt press lands).
    pub text_rect: Option<egui::Rect>,
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
    /// Rows it takes at least (a notebook's cell: as many as its text has).
    pub rows: usize,
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

/// A byte offset moved back onto a character's start (offsets kept from
/// before an edit may land inside a letter).
pub fn floor(text: &str, b: usize) -> usize {
    let mut b = b.min(text.len());
    while !text.is_char_boundary(b) {
        b -= 1;
    }
    b
}

fn char_of(text: &str, b: usize) -> usize {
    text[..floor(text, b)].chars().count()
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

/// The bands' hues, as in listok (fixed: a desktop scheme may be one hue).
const BAND_HUES: [f32; 6] = [45.0, 285.0, 205.0, 120.0, 330.0, 170.0];

const RAINBOW_DARK: [Color32; 4] = [Color32::from_rgb(255, 215, 0), Color32::from_rgb(218, 112, 214), Color32::from_rgb(23, 159, 255), Color32::from_rgb(120, 200, 120)];
const RAINBOW_LIGHT: [Color32; 4] = [Color32::from_rgb(175, 130, 0), Color32::from_rgb(160, 50, 160), Color32::from_rgb(0, 100, 200), Color32::from_rgb(30, 130, 60)];

pub fn error_bg(dark: bool) -> Color32 {
    if dark { Color32::from_rgba_unmultiplied(220, 40, 40, 90) } else { Color32::from_rgba_unmultiplied(255, 60, 60, 70) }
}

fn warn_color(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(230, 170, 40) } else { Color32::from_rgb(200, 120, 0) }
}

/// The bands' colours, full strength.
fn band_hues(dark: bool) -> Vec<Color32> {
    let (s, v) = if dark { (0.5, 0.97) } else { (0.75, 0.62) };
    BAND_HUES.iter().map(|&h| Color32::from(egui::ecolor::Hsva::new(h / 360.0, s, v, 1.0))).collect()
}

/// The bands on the rows in sight, as one mesh: every edge on a whole
/// pixel, the same one for the pieces either side of it (no seams, no
/// overlaps), each row down to the next one's top.
fn band_mesh(text: &str, bands: &[(Range<usize>, Color32)], galley: &egui::Galley, at: egui::Pos2, clip: egui::Rect, ppp: f32) -> egui::Mesh {
    let px = |v: f32| (v * ppp).round() / ppp;
    let mut mesh = egui::Mesh::default();
    let rows = &galley.rows;
    if rows.is_empty() {
        return mesh;
    }
    // The rows in sight (one per line: nothing wraps).
    let first = rows.partition_point(|r| at.y + r.max_y() < clip.top());
    let last = rows.partition_point(|r| at.y + r.min_y() <= clip.bottom());
    if first >= last {
        return mesh;
    }
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    if starts.len() != rows.len() {
        return mesh;
    }
    let lo = starts[first];
    let hi = starts.get(last).copied().unwrap_or(text.len());
    let mut i = bands.partition_point(|(r, _)| r.end <= lo);
    while let Some((r, c)) = bands.get(i) {
        if r.start >= hi {
            break;
        }
        i += 1;
        let mut a = r.start.max(lo);
        while a < r.end.min(hi) {
            let l = starts.partition_point(|&s| s <= a) - 1;
            let ls = starts[l];
            let le = text[ls..].find('\n').map_or(text.len(), |k| ls + k);
            let b = r.end.min(le);
            let row = &rows[l];
            if b > a && row.row.size.y > 0.0 && text.is_char_boundary(a) && text.is_char_boundary(b) {
                let c0 = text[ls..a].chars().count();
                let c1 = c0 + text[a..b].chars().count();
                let x = |c: usize| px(at.x + row.pos.x + row.row.x_offset(egui::text::CharIndex(c)));
                let top = px(at.y + row.min_y());
                let bottom = px(at.y + rows.get(l + 1).map_or(row.max_y(), |n| n.min_y().max(row.max_y())));
                let rect = egui::Rect::from_min_max(egui::pos2(x(c0), top), egui::pos2(x(c1), bottom));
                if rect.width() > 0.0 {
                    mesh.add_colored_rect(rect, *c);
                }
            }
            a = le + 1;
        }
    }
    mesh
}

/// The text laid out with colours.
pub fn layout(text: &str, cx: &Ctx, pair: Option<(usize, usize)>, font: f32, mono_bold: bool) -> LayoutJob {
    layout_blocks(text, cx, pair, font, mono_bold, &[], false).0
}

/// What a text lays out with besides its colours: its blocks (for the
/// frames) and what can be folded (byte ranges over more than a line).
#[derive(Default)]
struct Found {
    blocks: Vec<crate::sql::bands::Block>,
    /// (the start of its first line, its end).
    foldable: Vec<(usize, usize)>,
    /// The rainbow bands, when the editor paints them itself.
    bands: Vec<(Range<usize>, Color32)>,
}

/// The text laid out with colours, and its blocks (for the frames).
/// `own_bands`: the bands come back to be painted by the caller (as text
/// backgrounds egui rounds each piece's edges on its own: hairline gaps
/// between neighbours at most scales).
fn layout_blocks(text: &str, cx: &Ctx, pair: Option<(usize, usize)>, font: f32, mono_bold: bool, hidden: &[Range<usize>], own_bands: bool) -> (LayoutJob, Found) {
    use crate::sql::embed::Class;
    let pal: &Palette = if cx.dark { &cx.settings.dark } else { &cx.settings.light };
    // Every byte's class; SQL inside strings (dynamic SQL, $$ bodies) too.
    let spans = crate::sql::embed::classify(text, cx.dialect, cx.settings.embedded_sql);
    let mono = FontId::monospace(font);
    // Extra room between lines, as set.
    let spacing = cx.settings.fonts.code_spacing.clamp(0.0, 12.0);
    let bold = if mono_bold && pal.bold_keywords { FontId::new(font, FontFamily::Name("mono-bold".into())) } else { mono.clone() };
    let rainbow = if cx.dark { RAINBOW_DARK } else { RAINBOW_LIGHT };
    // SQL in a string: a faint wash of the string colour, deeper ones more.
    let wash = |level: u8| {
        let c = rgb(pal.string);
        let a = (if cx.dark { 16 } else { 22 }) * level.min(3);
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
    };
    let fmt_of = |sp: &crate::sql::embed::Span| -> TextFormat {
        let mut f = TextFormat::simple(mono.clone(), rgb(pal.text));
        match sp.class {
            Class::Keyword => {
                f.color = rgb(pal.keyword);
                f.font_id = bold.clone();
            }
            Class::Function => f.color = rgb(pal.function),
            Class::Type => f.color = rgb(pal.type_),
            Class::Str => f.color = rgb(pal.string),
            Class::Number => f.color = rgb(pal.number),
            Class::Comment => {
                f.color = rgb(pal.comment);
                f.italics = pal.italic_comments;
            }
            Class::Quoted => f.color = rgb(pal.quoted),
            Class::Param => f.color = rgb(pal.param),
            Class::Op => f.color = rgb(pal.operator),
            Class::Bracket(depth) => f.color = if pal.rainbow { rainbow[depth as usize % 4] } else { rgb(pal.operator) },
            Class::TableAlias => f.color = rgb(pal.table_alias),
            Class::ColumnAlias => f.color = rgb(pal.column_alias),
            Class::Schema => f.color = rgb(pal.schema),
            Class::Text => {}
        }
        if sp.level > 0 {
            f.background = wash(sp.level);
        }
        // Backgrounds that meet edge to edge: egui's 1px grow would make
        // them overlap (darker seams, at 1.2× too).
        f.expand_bg = 0.0;
        f
    };
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
    // Rainbow bands under the indents (listok's hues).
    let (found, blocks) = if (pal.bands || pal.frames) && text.len() < 2_000_000 { crate::sql::bands::analyse(text, cx.dialect) } else { Default::default() };
    let bands: Vec<(Range<usize>, Color32)> = if pal.bands {
        let alpha = if cx.dark { 0.24 } else { 0.30 };
        let hues: Vec<Color32> = band_hues(cx.dark).into_iter().map(|c| c.gamma_multiply(alpha)).collect();
        found.into_iter().map(|b| (b.range, hues[b.level % hues.len()])).collect()
    } else {
        Vec::new()
    };
    let mut cuts: Vec<usize> = spans.iter().map(|t| t.range.start).collect();
    for (r, _) in &bands {
        cuts.push(r.start);
        cuts.push(r.end);
    }
    for (r, _) in &over {
        // (Ranges found before the last edit may end inside a letter.)
        cuts.push(floor(text, r.start));
        cuts.push(floor(text, r.end));
    }
    for r in hidden {
        cuts.push(r.start.min(text.len()));
        cuts.push(r.end.min(text.len()));
    }
    cuts.push(text.len());
    cuts.sort_unstable();
    cuts.dedup();
    // Folded away: laid out as nothing (no width, no height).
    let mut gone = TextFormat::simple(FontId::monospace(0.5), Color32::TRANSPARENT);
    gone.line_height = Some(0.0);
    gone.expand_bg = 0.0;
    let mut hi = 0;
    let mut job = LayoutJob { text: text.to_string(), ..Default::default() };
    job.wrap.max_width = f32::INFINITY;
    let pair_bg = if cx.dark { Color32::from_rgba_unmultiplied(120, 160, 255, 70) } else { Color32::from_rgba_unmultiplied(80, 120, 255, 60) };
    let mut ti = 0;
    let mut bi = 0;
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b {
            continue;
        }
        while ti + 1 < spans.len() && spans[ti + 1].range.start <= a {
            ti += 1;
        }
        while hi < hidden.len() && hidden[hi].end <= a {
            hi += 1;
        }
        if hidden.get(hi).is_some_and(|r| r.start <= a && b <= r.end) {
            job.sections.push(egui::text::LayoutSection { leading_space: 0.0, byte_range: egui::text::ByteIndex(a)..egui::text::ByteIndex(b), format: gone.clone() });
            continue;
        }
        let mut f = spans.get(ti).map(&fmt_of).unwrap_or_else(|| TextFormat::simple(mono.clone(), rgb(pal.text)));
        f.expand_bg = 0.0;
        while bi < bands.len() && bands[bi].0.end <= a {
            bi += 1;
        }
        if let Some((_, c)) = bands.get(bi).filter(|(r, _)| !own_bands && r.start <= a && b <= r.end) {
            f.background = *c;
        }
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
        if spacing > 0.0 {
            f.line_height = Some((font * 1.2 + spacing).round());
        }
        job.sections.push(egui::text::LayoutSection { leading_space: 0.0, byte_range: egui::text::ByteIndex(a)..egui::text::ByteIndex(b), format: f });
    }
    // Foldable: blocks and statements over more than a line.
    let mut spans_: Vec<Range<usize>> = if text.len() < 2_000_000 { crate::sql::split(text, cx.dialect, cx.settings.blank_line_splits).into_iter().map(|r| r.start..r.start + text[r.clone()].trim_end().len()).filter(|r| text[r.clone()].contains('\n')).collect() } else { Vec::new() };
    spans_.extend(blocks.iter().filter(|b| b.range.end <= text.len() && text[b.range.clone()].contains('\n')).map(|b| b.range.clone()));
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    // One per first line: the longest.
    let mut foldable: Vec<(usize, usize)> = spans_.into_iter().map(|r| (line_starts[line_starts.partition_point(|&x| x <= r.start) - 1], r.end)).collect();
    foldable.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    foldable.dedup_by_key(|f| f.0);
    (job, Found { blocks: if pal.frames { blocks } else { Vec::new() }, foldable, bands: if own_bands { bands } else { Vec::new() } })
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

/// The block selection: made by the mouse (Alt held), the text's caret
/// kept at its head, its columns painted under the text, a tall caret when
/// it's thin.
#[allow(clippy::too_many_arguments)]
fn block_mouse_and_paint(ui: &egui::Ui, ctx: &egui::Context, id: egui::Id, text: &str, st: &mut EditorState, output: &egui::text_edit::TextEditOutput, alt_press: bool, under: egui::layers::ShapeIdx, char_w: f32) {
    use crate::block::{self, Block};
    let gp = output.galley_pos;
    let to_lc = |pos: egui::Pos2| {
        let c = output.galley.cursor_from_pos(pos - gp);
        let line = text[..byte_of(text, c.index.0)].matches('\n').count();
        (line, ((pos.x - gp.x) / char_w).round().max(0.0) as usize)
    };
    if st.block_drag {
        if alt_press {
            if let Some(o) = ctx.input(|i| i.pointer.press_origin()) {
                let (l, c) = to_lc(o);
                st.block = Some(Block::at(l, c));
            }
        }
        if let (Some(b), Some(p)) = (st.block.as_mut(), ctx.input(|i| i.pointer.interact_pos())) {
            b.head = to_lc(p);
        }
        if !ctx.input(|i| i.pointer.primary_down()) {
            st.block_drag = false;
        }
    }
    let Some(b) = st.block else { return };
    let sp = block::spans(text);
    let (l0, l1) = b.lines();
    let (c0, c1) = b.cols();
    // The text's caret at the block's head (no selection of its own).
    let hl = b.head.0.min(sp.len() - 1);
    let at = sp[hl].0 + block::byte_at(&text[sp[hl].0..sp[hl].1], b.head.1);
    if output.cursor_range.is_none_or(|r| r.primary != r.secondary || byte_of(text, r.primary.index.0) != at) {
        set_cursor(ctx, id, text, at, at);
        ctx.request_repaint();
    }
    let row = |l: usize| {
        let ci = char_of(text, sp[l.min(sp.len() - 1)].0);
        output.galley.pos_from_cursor(egui::text::CCursor::new(ci)).translate(gp.to_vec2())
    };
    let x = |c: usize| gp.x + c as f32 * char_w;
    let fill = ui.visuals().selection.bg_fill;
    let mut shapes = Vec::new();
    if c0 < c1 {
        for l in l0..=l1.min(sp.len() - 1) {
            let r = row(l);
            if r.height() > 0.5 {
                shapes.push(egui::Shape::rect_filled(egui::Rect::from_x_y_ranges(x(c0)..=x(c1), r.y_range()), 0.0, fill));
            }
        }
    }
    ui.painter().set(under, egui::Shape::Vec(shapes));
    // A caret as tall as the block, where typing goes.
    let (top, bottom) = (row(l0).top(), row(l1).bottom());
    let stroke = ui.visuals().text_cursor.stroke;
    let cx = if b.head.1 == c1 { x(c1) } else { x(c0) };
    ui.painter().line_segment([egui::pos2(cx, top), egui::pos2(cx, bottom)], egui::Stroke::new(stroke.width.max(1.5), stroke.color));
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

/// A selection's rows on screen (as egui lays them: a selected line end
/// shows as half a row's height).
fn selection_rows(g: &egui::Galley, at: egui::Pos2, cr: &egui::text_selection::CCursorRange) -> Vec<egui::Rect> {
    let [a, b] = cr.sorted_cursors();
    if a == b || g.rows.is_empty() {
        return Vec::new();
    }
    let (la, lb) = (g.layout_from_cursor(a), g.layout_from_cursor(b));
    let mut out = Vec::new();
    for ri in la.row..=lb.row.min(g.rows.len() - 1) {
        let pr = &g.rows[ri];
        let left = if ri == la.row { pr.row.x_offset(la.column) } else { 0.0 };
        let right = if ri == lb.row {
            pr.row.x_offset(lb.column)
        } else {
            pr.row.size.x + if pr.ends_with_newline { pr.row.height() / 2.0 } else { 0.0 }
        };
        if right - left < 0.5 {
            continue;
        }
        let o = at + pr.pos.to_vec2();
        out.push(egui::Rect::from_min_max(egui::pos2(o.x + left, o.y), egui::pos2(o.x + right, o.y + pr.row.size.y)));
    }
    out
}

/// Rows stacked into one shape: its outer corners rounded, and where a
/// row sticks out past the next, the inner corner filled round too.
fn rounded_rows(rows: &[egui::Rect], radius: f32, color: Color32) -> egui::Mesh {
    use std::f32::consts::FRAC_PI_2;
    const STEPS: usize = 6;
    let mut mesh = egui::Mesh::default();
    // A fan from `c` through `pts`.
    let fan = |mesh: &mut egui::Mesh, c: egui::Pos2, pts: &[egui::Pos2], closed: bool| {
        let i0 = mesh.vertices.len() as u32;
        mesh.colored_vertex(c, color);
        for p in pts {
            mesh.colored_vertex(*p, color);
        }
        let n = pts.len() as u32;
        for k in 0..n {
            if k + 1 == n && !closed {
                break;
            }
            mesh.add_triangle(i0, i0 + 1 + k, i0 + 1 + (k + 1) % n);
        }
    };
    // Touching and overlapping: the same shape.
    let joined = |a: &egui::Rect, b: &egui::Rect| (a.bottom() - b.top()).abs() < 1.0 && a.right() > b.left() + 0.5 && a.left() < b.right() - 0.5;
    for (i, r) in rows.iter().enumerate() {
        let up = i.checked_sub(1).map(|j| rows[j]).filter(|u| joined(u, r));
        let down = rows.get(i + 1).copied().filter(|d| joined(r, d));
        let h = r.height();
        let lim = |d: f32| radius.min(h / 2.0).min(r.width() / 2.0).min(d / 2.0);
        // Outer radius of a corner, given the neighbour's edge there
        // (None: no neighbour) and on which side being further out rounds it.
        let outer = |n: Option<f32>, mine: f32, out_is_less: bool| match n {
            None => lim(f32::INFINITY),
            Some(x) if (out_is_less && x > mine + 0.5) || (!out_is_less && x < mine - 0.5) => lim((x - mine).abs()),
            _ => 0.0,
        };
        let nw = outer(up.map(|u| u.left()), r.left(), true);
        let ne = outer(up.map(|u| u.right()), r.right(), false);
        let sw = outer(down.map(|d| d.left()), r.left(), true);
        let se = outer(down.map(|d| d.right()), r.right(), false);
        // The row: a convex outline, clockwise.
        let mut pts: Vec<egui::Pos2> = Vec::new();
        let mut corner = |c: egui::Pos2, ix: f32, iy: f32, rr: f32, rev: bool| {
            if rr <= 0.1 {
                pts.push(c);
                return;
            }
            let center = c + egui::vec2(ix * rr, iy * rr);
            let mut arc: Vec<egui::Pos2> = (0..=STEPS)
                .map(|k| {
                    let f = k as f32 / STEPS as f32 * FRAC_PI_2;
                    center + egui::vec2(-ix * f.cos(), -iy * f.sin()) * rr
                })
                .collect();
            if rev {
                arc.reverse();
            }
            pts.extend(arc);
        };
        corner(r.left_top(), 1.0, 1.0, nw, false);
        corner(r.right_top(), -1.0, 1.0, ne, true);
        corner(r.right_bottom(), -1.0, -1.0, se, false);
        corner(r.left_bottom(), 1.0, -1.0, sw, true);
        fan(&mut mesh, r.center(), &pts, true);
        // Inner corners: where the neighbour sticks out past this row, the
        // angle between them filled round.
        let mut inner = |c: egui::Pos2, dx: f32, dy: f32, d: f32| {
            let rr = lim(d);
            if rr <= 0.1 {
                return;
            }
            let center = c + egui::vec2(dx * rr, dy * rr);
            let arc: Vec<egui::Pos2> = (0..=STEPS)
                .map(|k| {
                    let f = k as f32 / STEPS as f32 * FRAC_PI_2;
                    center + egui::vec2(-dx * f.sin(), -dy * f.cos()) * rr
                })
                .collect();
            fan(&mut mesh, c, &arc, false);
        };
        if let Some(u) = up {
            if u.left() < r.left() - 0.5 {
                inner(r.left_top(), -1.0, 1.0, r.left() - u.left());
            }
            if u.right() > r.right() + 0.5 {
                inner(r.right_top(), 1.0, 1.0, u.right() - r.right());
            }
        }
        if let Some(d) = down {
            if d.left() < r.left() - 0.5 {
                inner(r.left_bottom(), -1.0, -1.0, r.left() - d.left());
            }
            if d.right() > r.right() + 0.5 {
                inner(r.right_bottom(), 1.0, -1.0, d.right() - r.right());
            }
        }
    }
    mesh
}

/// A rounded frame a whole screen pixel wide, on the pixel grid (at any
/// scale); its corners gone over again, faintly, so their smoothed pixels
/// don't come out paler than the sides.
pub fn crisp_frame(painter: &egui::Painter, rect: egui::Rect, radius: f32, color: Color32, ppp: f32) {
    let px = 1.0 / ppp;
    let w = px * ppp.round().max(1.0);
    let snap = |v: f32| (v * ppp).round() / ppp;
    let r = egui::Rect::from_min_max(egui::pos2(snap(rect.left()) + w / 2.0, snap(rect.top()) + w / 2.0), egui::pos2(snap(rect.right()) - w / 2.0, snap(rect.bottom()) - w / 2.0));
    painter.rect_stroke(r, radius, Stroke::new(w, color), egui::StrokeKind::Middle);
    // The corners once more, faintly: their smoothed pixels as strong as
    // the sides'.
    let k = radius + w;
    let faint = color.gamma_multiply(0.5);
    for (cx, cy) in [(r.left(), r.top()), (r.right(), r.top()), (r.left(), r.bottom()), (r.right(), r.bottom())] {
        let clip = egui::Rect::from_center_size(egui::pos2(cx, cy), egui::vec2(k * 2.0, k * 2.0));
        painter.with_clip_rect(clip.intersect(painter.clip_rect())).rect_stroke(r, radius, Stroke::new(w, faint), egui::StrokeKind::Middle);
    }
}

/// The name at `at` (letters, digits, _ $ #, or a quoted one).
fn name_at(text: &str, at: usize) -> Option<Range<usize>> {
    let at = floor(text, at.min(text.len()));
    let is = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '#';
    let a = text[..at].char_indices().rev().take_while(|(_, c)| is(*c)).last().map_or(at, |(i, _)| i);
    let b = text[at..].char_indices().find(|(_, c)| !is(*c)).map_or(text.len(), |(i, _)| at + i);
    (b > a).then_some(a..b)
}

/// Ctrl held: the name under the pointer underlined, a hand; a click goes
/// where it leads — in the text (an alias's table, a CTE, the script's own
/// function: picked and lit), or to the database (an action).
fn follow_links(ui: &mut egui::Ui, ctx: &egui::Context, id: egui::Id, text: &str, st: &mut EditorState, output: &egui::text_edit::TextEditOutput, cx: &Ctx, out: &mut Out) {
    let gp = output.galley_pos;
    let rect_of = |r: &Range<usize>| {
        let a = output.galley.pos_from_cursor(CCursor::new(char_of(text, r.start))).translate(gp.to_vec2());
        let b = output.galley.pos_from_cursor(CCursor::new(char_of(text, r.end))).translate(gp.to_vec2());
        egui::Rect::from_min_max(a.min, egui::pos2(b.max.x.max(a.min.x + 2.0), a.max.y))
    };
    // The place led to, lit a moment.
    if let Some((r, since)) = st.flash.clone() {
        let t = since.elapsed().as_secs_f32();
        if t > 1.4 || r.end > text.len() {
            st.flash = None;
        } else {
            let k = (1.0 - t / 1.4).clamp(0.0, 1.0);
            let c = ui.visuals().selection.stroke.color;
            let rect = rect_of(&r).expand2(egui::vec2(2.0, 1.0));
            ui.painter().rect_filled(rect, 3.0, ui.visuals().selection.bg_fill.gamma_multiply(0.6 * k));
            ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.5, c.gamma_multiply(k)), egui::StrokeKind::Outside);
            ctx.request_repaint();
        }
    }
    if !ctx.input(|i| i.modifiers.command) {
        return;
    }
    let Some(pos) = ctx.pointer_hover_pos().filter(|p| output.response.rect.contains(*p)) else { return };
    let at = byte_of(text, output.galley.cursor_from_pos(pos - gp).index.0);
    let Some(word) = name_at(text, at) else { return };
    let wr = rect_of(&word);
    if !wr.expand(2.0).contains(pos) {
        return;
    }
    let link = ui.visuals().hyperlink_color;
    ui.painter().hline(wr.x_range(), wr.bottom() - 1.0, egui::Stroke::new(1.2, link));
    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    if !output.response.clicked() {
        return;
    }
    let has_col = |t: &str, c: &str| cx.rels.iter().any(|r| r.name.eq_ignore_ascii_case(t) && r.cols.iter().any(|(n, _)| n.eq_ignore_ascii_case(c)));
    match crate::sql::follow::follow(text, word.start, cx.dialect, &has_col).map(|x| x.1) {
        Some(crate::sql::follow::Target::Here(r)) => {
            set_cursor(ctx, id, text, r.start, r.end);
            ui.scroll_to_rect(rect_of(&r), Some(egui::Align::Center));
            st.flash = Some((r, std::time::Instant::now()));
        }
        Some(crate::sql::follow::Target::Object { schema, name, call }) => out.actions.push(Action::Open { schema, name, call }),
        None => {}
    }
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
    if let Some((a, b)) = st.select.take() {
        let (a, b) = (floor(text, a.min(text.len())), floor(text, b.min(text.len())));
        set_cursor(&ctx, id, text, a, b);
        ctx.memory_mut(|m| m.request_focus(id));
        st.scroll_to = Some((a, b));
    }
    if std::mem::take(&mut st.focus) {
        ctx.memory_mut(|m| m.request_focus(id));
    }
    // Focus gone (Esc, a click elsewhere): no block.
    if !focused && !st.block_drag {
        st.block = None;
    }
    // A block selection takes the typing, Backspace / Delete, copy, cut and
    // paste for all its lines; a move, Esc, Enter or undo ends it.
    if focused && st.block.is_some() {
        use crate::block::{self, Op};
        let mut ops: Vec<Op> = Vec::new();
        let (mut copy, mut end) = (false, false);
        ui.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Text(t) => {
                    match ops.last_mut() {
                        Some(Op::Insert(p)) if p.len() == 1 => p[0].push_str(t),
                        _ => ops.push(Op::Insert(vec![t.clone()])),
                    }
                    false
                }
                egui::Event::Paste(t) => {
                    ops.push(Op::Insert(st.block.as_ref().map_or_else(|| vec![t.clone()], |b| block::pieces(t, b))));
                    false
                }
                egui::Event::Copy => {
                    copy = true;
                    false
                }
                egui::Event::Cut => {
                    copy = true;
                    ops.push(Op::Delete);
                    false
                }
                egui::Event::Key { key: Key::Backspace, pressed, .. } => {
                    if *pressed {
                        ops.push(Op::Backspace);
                    }
                    false
                }
                egui::Event::Key { key: Key::Delete, pressed, .. } => {
                    if *pressed {
                        ops.push(Op::Delete);
                    }
                    false
                }
                egui::Event::Key { key: Key::Escape, pressed: true, .. } => {
                    end = true;
                    false
                }
                egui::Event::Key { key, pressed: true, modifiers, .. }
                    if matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End | Key::PageUp | Key::PageDown | Key::Enter | Key::Tab) || (modifiers.command && matches!(key, Key::Z | Key::Y | Key::A)) =>
                {
                    end = true;
                    true
                }
                _ => true,
            });
        });
        if let Some(b) = st.block.as_mut() {
            if copy {
                ctx.copy_text(block::text_of(text, b));
            }
            // (Cut of a caret-thin block deletes nothing.)
            let thin = b.cols().0 == b.cols().1;
            for op in &ops {
                if copy && thin && *op == Op::Delete {
                    continue;
                }
                block::edit(text, b, op);
                out.changed = true;
            }
        }
        st.popup = None;
        if end {
            st.block = None;
        }
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
                let n = pop.rows();
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
                let as_typed = pop.item(pop.sel).is_none();
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    st.popup = None;
                } else if as_typed && (i.key_pressed(Key::Enter) || i.key_pressed(Key::Tab)) {
                    // The word as typed: nothing put in; Enter / Tab do
                    // what they'd do without hints (a new line, an indent).
                    st.popup = None;
                } else if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab) {
                    if let Some(k) = pop.item(pop.sel) {
                        let c = accept(text, pop, k);
                        new_sel = Some((c, c));
                        out.changed = true;
                    }
                    st.popup = None;
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
                // (Bytes around the cursor: next to a letter of two bytes
                // they're no pair, and the slice would cut it.)
                let pair = text.get(a - 1..a + 1).unwrap_or("");
                if matches!(pair, "()" | "[]" | "''" | "\"\"") && i.consume_key(Modifiers::NONE, Key::Backspace) {
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
    // (Room for the fold marks too.)
    let gutter = digits * char_w + 30.0;
    let cursor_now = egui::text_edit::TextEditState::load(&ctx, id).and_then(|s| s.cursor.char_range()).map(|r| byte_of(text, r.primary.index.0));
    let mono_bold = ctx.fonts(|f| f.families().iter().any(|fam| *fam == FontFamily::Name("mono-bold".into())));
    let (pair, _) = around(st, text, cursor_now, cx);
    let text_len = text.len();
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (cx.dialect, cx.dark, cx.issues, cx.server_err, pair, font.to_bits(), mono_bold, cx.settings.fonts.code_spacing.to_bits()).hash(&mut h);
        format!("{:?}{:?}", cx.settings.dark, cx.settings.light).hash(&mut h);
        h.finish()
    };
    // Folded blocks: from their first line's end to their last line's end.
    let eol = |text: &str, x: usize| text[x.min(text.len())..].find('\n').map_or(text.len(), |i| x.min(text.len()) + i);
    let hidden: Vec<Range<usize>> = {
        let mut v: Vec<Range<usize>> = st
            .folds
            .iter()
            .filter_map(|&f| {
                let i = st.foldable.binary_search_by_key(&f, |x| x.0).ok()?;
                let end = st.foldable[i].1.min(text.len());
                let a = eol(text, f);
                let b = eol(text, end.saturating_sub(1));
                (a < b).then_some(a..b)
            })
            .collect();
        v.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for r in v {
            match merged.last_mut() {
                Some(m) if r.start <= m.end => m.end = m.end.max(r.end),
                _ => merged.push(r),
            }
        }
        merged
    };
    // The caret inside a fold: it opens.
    if let Some(c) = cursor_now {
        if let Some(r) = hidden.iter().find(|r| r.start < c && c <= r.end) {
            let f = text[..r.start].rfind('\n').map_or(0, |i| i + 1);
            st.folds.retain(|&x| x != f);
            ctx.request_repaint();
        }
    }
    // The text as laid out (egui's own copy has the selection's glyphs
    // recoloured): the selected part is drawn again from it.
    let mut plain: Option<Arc<egui::Galley>> = None;
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _w: f32| -> Arc<egui::Galley> {
        let s = buf.as_str();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // (Folds made for another text don't fit an edited one.)
        let hidden: Vec<Range<usize>> = if s.len() == text_len { hidden.clone() } else { Vec::new() };
        (key, s, &hidden).hash(&mut h);
        let k = h.finish();
        let job = match &st.job {
            Some((kk, j)) if *kk == k => j.clone(),
            _ => {
                let (j, found) = layout_blocks(s, cx, pair, font, mono_bold, &hidden, true);
                st.job = Some((k, j.clone()));
                st.blocks = found.blocks;
                st.foldable = found.foldable;
                st.bands = found.bands;
                st.frame_x.clear();
                j
            }
        };
        let g = ui.fonts_mut(|f| f.layout_job(job));
        plain = Some(g.clone());
        g
    };
    let before = text.clone();
    // Under the text: the caret's line, the same words, a bracket pair.
    let under = ui.painter().add(egui::Shape::Noop);
    let mut sel_rows: Vec<egui::Rect> = Vec::new();
    // (egui moves the caret on any button's press: a right click inside the
    // selection must keep it — the selection as of before the press.)
    let right_press = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
    let sel_before = st.cursor;
    // Alt and a press on the text: a block selection starts; a press
    // without Alt ends one.
    let (alt_press, plain_press) = ctx.input(|i| {
        let on = i.pointer.interact_pos().is_some_and(|p| st.text_rect.is_some_and(|r| r.contains(p)));
        let pressed = on && i.pointer.primary_pressed();
        (pressed && i.modifiers.alt, pressed && !i.modifiers.alt)
    });
    if alt_press {
        st.block_drag = true;
    }
    if plain_press {
        st.block = None;
        st.block_drag = false;
    }
    let blocky = st.block.is_some() || st.block_drag;
    // Under the text: the block's columns.
    let under_block = ui.painter().add(egui::Shape::Noop);
    let output = ui
        .horizontal_top(|ui| {
            ui.add_space(gutter);
            // egui's selection hidden: drawn here instead (rounded, under
            // the text, the text keeping its colours).
            ui.visuals_mut().selection.bg_fill = Color32::TRANSPARENT;
            ui.visuals_mut().selection.stroke.color = Color32::TRANSPARENT;
            egui::TextEdit::multiline(text)
        .id(id)
        .font(FontId::monospace(font))
        .code_editor()
        .lock_focus(true)
        .desired_width(f32::INFINITY)
        .desired_rows(cx.rows)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin { left: 0, right: 8, top: 4, bottom: if cx.rows < 30 { 4 } else { 40 } })
        .layouter(&mut layouter)
        .show(ui)
        })
        .inner;
    if *text != before {
        out.changed = true;
        // One char typed after a word: a keyword takes the style's case
        // (same length: the caret stays where it is).
        let fo = cx.settings.format;
        if fo.case_while_typing && text.len() == before.len() + 1 && st.block.is_none() {
            let p = before.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
            if text.is_char_boundary(p) && text.is_char_boundary(p + 1) && before.is_char_boundary(p) && text[p + 1..] == before[p..] {
                if let Some((r, w)) = crate::sql::autocase::on_typed(text, p, cx.dialect, fo.keywords) {
                    text.replace_range(r, &w);
                }
            }
        }
        // Folds after the change move with the text.
        let p = before.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
        let delta = text.len() as isize - before.len() as isize;
        for f in st.folds.iter_mut() {
            if *f > p {
                *f = (*f as isize + delta).max(0) as usize;
            }
        }
        st.folds.retain(|&f| f <= text.len() && (f == 0 || text.as_bytes().get(f - 1) == Some(&b'\n')));
    }
    st.text_rect = Some(output.response.rect);
    // The wheel while the mouse is held (a selection being dragged): egui's
    // scroll areas ignore it then, so it's passed on by hand.
    if output.response.dragged() || st.block_drag {
        let d = ctx.input(|i| i.smooth_scroll_delta());
        if d != egui::Vec2::ZERO {
            ui.scroll_with_delta(d);
        }
    }
    block_mouse_and_paint(ui, &ctx, id, text, st, &output, alt_press, under_block, char_w);
    follow_links(ui, &ctx, id, text, st, &output, cx, &mut out);

    // Line numbers, error dots.
    let painter = ui.painter().clone();
    let weak = ui.visuals().weak_text_color();
    let strong = ui.visuals().strong_text_color();
    let cursor_b = output.cursor_range.map(|r| byte_of(text, r.primary.index.0)).or(cursor_now);
    let cur_line = cursor_b.map(|c| text[..floor(text, c)].matches('\n').count());
    let err_lines: std::collections::HashSet<usize> = cx.issues.iter().filter(|i| i.sev == Sev::Error).map(|i| text[..floor(text, i.range.start)].matches('\n').count()).collect();
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
        // (A folded line's row has no height: no number.)
        let gone = output.galley.rows.len() > 1 && row.rect().height() < 1.0;
        if new_line && !gone && r.bottom() >= clip.top() && r.top() <= clip.bottom() {
            let color = if Some(line) == cur_line { strong } else { weak };
            let x = output.galley_pos.x - 18.0;
            painter.text(egui::pos2(x, r.center().y), egui::Align2::RIGHT_CENTER, (line + 1).to_string(), FontId::monospace(font * 0.85), color);
            if err_lines.contains(&line) {
                painter.circle_filled(egui::pos2(output.galley_pos.x - gutter + 7.0, r.center().y), 3.0, Color32::from_rgb(230, 70, 70));
            }
        }
        new_line = row.ends_with_newline;
        if new_line {
            line += 1;
        }
    }

    // Fold marks in the gutter (▾ under the pointer, ▸ when folded), and
    // a "⋯ n" after a folded line (a click opens it).
    if !st.foldable.is_empty() {
        let gutter_hot = ui.rect_contains_pointer(egui::Rect::from_min_max(egui::pos2(output.galley_pos.x - gutter, clip.top()), egui::pos2(output.galley_pos.x, clip.bottom())));
        let mut toggle: Option<usize> = None;
        let mut lb = 0usize;
        let mut new_line = true;
        let mut fi = 0;
        for row in output.galley.rows.iter() {
            let r = row.rect().translate(output.galley_pos.to_vec2());
            let gone = output.galley.rows.len() > 1 && row.rect().height() < 1.0;
            if new_line && !gone && r.bottom() >= clip.top() && r.top() <= clip.bottom() {
                while fi < st.foldable.len() && st.foldable[fi].0 < lb {
                    fi += 1;
                }
                if st.foldable.get(fi).is_some_and(|f| f.0 == lb) {
                    let folded = st.folds.contains(&lb);
                    let c = egui::pos2(output.galley_pos.x - 9.0, r.center().y);
                    let hit = egui::Rect::from_center_size(c, egui::vec2(14.0, r.height().max(row_h)));
                    let resp = ui.interact(hit, id.with(("fold", lb)), egui::Sense::click());
                    if folded || gutter_hot {
                        let color = if resp.hovered() || folded { strong } else { weak };
                        crate::ui::paint_chevron(&painter, c, font * 0.75, if folded { 0.0 } else { 1.0 }, color);
                    }
                    if resp.clicked() {
                        toggle = Some(lb);
                    }
                    if folded {
                        let end = st.foldable[fi].1.min(text.len());
                        let n = text[lb..end].matches('\n').count();
                        let label = format!("⋯ {n}");
                        let g = painter.layout_no_wrap(label, FontId::monospace(font * 0.8), weak);
                        let pill = egui::Rect::from_min_size(egui::pos2(r.right() + 8.0, r.center().y - g.size().y / 2.0 - 1.0), g.size() + egui::vec2(10.0, 2.0));
                        let pr = ui.interact(pill, id.with(("unfold", lb)), egui::Sense::click()).on_hover_text(crate::i18n::t("Свёрнуто — щелчок развернёт", "Folded: click to open"));
                        painter.rect(pill, 3, if pr.hovered() { ui.visuals().widgets.hovered.weak_bg_fill } else { ui.visuals().widgets.inactive.weak_bg_fill }, Stroke::NONE, egui::StrokeKind::Inside);
                        painter.galley(pill.min + egui::vec2(5.0, 1.0), g, weak);
                        if pr.clicked() {
                            toggle = Some(lb);
                        }
                    }
                }
            }
            // The next line's start.
            if row.ends_with_newline {
                lb = text[lb..].find('\n').map_or(text.len(), |i| lb + i + 1);
            }
            new_line = row.ends_with_newline;
        }
        if let Some(f) = toggle {
            if let Some(i) = st.folds.iter().position(|&x| x == f) {
                st.folds.remove(i);
            } else {
                st.folds.push(f);
                // The caret inside goes to the fold's line.
                if cursor_b.is_some_and(|c| c > eol(text, f)) {
                    set_cursor(&ctx, id, text, eol(text, f), eol(text, f));
                }
            }
            ctx.request_repaint();
        }
    }

    // Under the text: the caret's line (faint; the bands only change hue a
    // little), the words like the one picked or under a resting caret, and
    // the brackets under the pointer (or picked) with their pairs.
    {
        let mut shapes: Vec<egui::Shape> = Vec::new();
        let gp = output.galley_pos.to_vec2();
        if !st.bands.is_empty() {
            shapes.push(egui::Shape::mesh(band_mesh(text, &st.bands, &output.galley, output.galley_pos, clip, ctx.pixels_per_point())));
        }
        // The selection: over the bands (they show through, another shade),
        // its corners rounded, outside and in.
        if ctx.memory(|m| m.has_focus(id)) && !blocky {
            if let Some(cr) = &output.cursor_range {
                let at = output.galley_pos - egui::vec2(output.galley.rect.left(), 0.0);
                sel_rows = selection_rows(&output.galley, at, cr);
                let base = ui.visuals().selection.bg_fill;
                let fill = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), if cx.dark { 120 } else { 100 });
                let r = (font * 0.32).clamp(2.0, 6.0);
                shapes.push(egui::Shape::mesh(rounded_rows(&sel_rows, r, fill)));
            }
        }
        // What a search found: lit (the one it's on, more).
        if !st.marks.is_empty() {
            let rect_of = |r: &Range<usize>| {
                let a = output.galley.pos_from_cursor(CCursor::new(char_of(text, r.start.min(text.len())))).translate(gp);
                let b = output.galley.pos_from_cursor(CCursor::new(char_of(text, r.end.min(text.len())))).translate(gp);
                egui::Rect::from_min_max(a.min, egui::pos2(b.max.x.max(a.min.x + 2.0), a.max.y))
            };
            let base = Color32::from_rgb(250, 200, 60);
            for r in &st.marks {
                if r.end > text.len() || r.start >= r.end {
                    continue;
                }
                let rect = rect_of(r);
                if !rect.intersects(clip) {
                    continue;
                }
                let now = st.mark_now.as_ref() == Some(r);
                shapes.push(egui::Shape::rect_filled(rect.expand2(egui::vec2(1.0, 0.0)), 2.0, base.gamma_multiply(if now { 0.55 } else { 0.22 })));
                if now {
                    shapes.push(egui::Shape::rect_stroke(rect.expand2(egui::vec2(1.0, 0.0)), 2.0, Stroke::new(1.0, base), egui::StrokeKind::Outside));
                }
            }
        }
        // A pick asked for: scrolled into sight.
        if let Some((a, b)) = st.scroll_to.take() {
            let p = output.galley.pos_from_cursor(CCursor::new(char_of(text, a))).translate(gp);
            let q = output.galley.pos_from_cursor(CCursor::new(char_of(text, b))).translate(gp);
            ui.scroll_to_rect(p.union(q).expand(20.0), Some(egui::Align::Center));
        }
        let has_focus = ctx.memory(|m| m.has_focus(id));
        let sel = output.cursor_range.map(|r| (byte_of(text, r.primary.index.0), byte_of(text, r.secondary.index.0)));
        if has_focus {
            if let Some(cr) = output.cursor_range {
                let at = output.galley.pos_from_cursor(cr.primary).translate(gp);
                let line = egui::Rect::from_min_max(egui::pos2(output.galley_pos.x - 2.0, at.top()), egui::pos2(clip.right(), at.bottom()));
                let c = if cx.dark { Color32::from_rgba_unmultiplied(255, 255, 255, 10) } else { Color32::from_rgba_unmultiplied(0, 0, 0, 9) };
                shapes.push(egui::Shape::rect_filled(line, 0.0, c));
            }
        }
        let k = st.job.as_ref().map_or(0, |j| j.0);
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '#';
        // The word: picked whole, or under a caret resting a while.
        let mut word: Option<String> = None;
        if let Some((p, q)) = sel {
            let (a, b) = (p.min(q).min(text.len()), p.max(q).min(text.len()));
            if a < b && b - a <= 128 && text.is_char_boundary(a) && text.is_char_boundary(b) {
                let w = &text[a..b];
                if w.chars().count() >= 2 && w.chars().all(is_word) {
                    word = Some(w.to_string());
                }
            } else if a == b {
                match st.caret_since {
                    Some((x, since)) if x == a => {
                        let wait = std::time::Duration::from_millis(600);
                        if since.elapsed() >= wait {
                            let s0 = text[..a].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(a, |(i, _)| i);
                            let s1 = text[a..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| a + i);
                            if s1 > s0 && text[s0..s1].chars().count() >= 2 && !text[s0..s1].chars().all(|c| c.is_ascii_digit()) {
                                word = Some(text[s0..s1].to_string());
                            }
                        } else if has_focus {
                            ctx.request_repaint_after(wait - since.elapsed());
                        }
                    }
                    _ => st.caret_since = Some((a, std::time::Instant::now())),
                }
            }
            if a != b {
                st.caret_since = None;
            }
        }
        // The rows on screen, as bytes.
        let (mut c0, mut c1, mut ci) = (usize::MAX, 0usize, 0usize);
        for row in &output.galley.rows {
            let r = row.rect().translate(gp);
            let n = row.char_count_including_newline().0;
            if r.bottom() >= clip.top() && r.top() <= clip.bottom() {
                c0 = c0.min(ci);
                c1 = ci + n;
            }
            ci += n;
        }
        let visible = (c0 != usize::MAX).then(|| (byte_of(text, c0), byte_of(text, c1)));
        let char_rect = |b: usize, chars: usize| -> egui::Rect {
            let ci = char_of(text, b);
            let a = output.galley.pos_from_cursor(CCursor::new(ci));
            let z = output.galley.pos_from_cursor(CCursor::new(ci + chars));
            egui::Rect::from_min_max(egui::pos2(a.left(), a.top()), egui::pos2(z.left().max(a.left() + 2.0), a.bottom())).translate(gp)
        };
        if let (Some(w), Some((v0, v1))) = (&word, visible) {
            let lw = w.to_lowercase();
            if !st.words.as_ref().is_some_and(|(kk, ww, _)| *kk == k && *ww == lw) {
                let low = text.to_lowercase();
                let found: Vec<usize> = if low.len() == text.len() {
                    low.match_indices(&lw)
                        .map(|(i, _)| i)
                        .filter(|&i| !text[..i].chars().next_back().is_some_and(is_word) && !text[i + lw.len()..].chars().next().is_some_and(is_word))
                        .take(20_000)
                        .collect()
                } else {
                    Vec::new()
                };
                st.words = Some((k, lw.clone(), found));
            }
            let found = &st.words.as_ref().unwrap().2;
            if found.len() > 1 {
                let c = if cx.dark { Color32::from_rgba_unmultiplied(255, 235, 140, 34) } else { Color32::from_rgba_unmultiplied(200, 150, 0, 40) };
                let n = w.chars().count();
                for &b in found.iter().filter(|&&b| b >= v0 && b <= v1) {
                    shapes.push(egui::Shape::rect_filled(char_rect(b, n).expand2(egui::vec2(0.5, 0.0)), 2.0, c));
                }
            }
        }
        // Brackets: under the pointer, or a picked one.
        let mut at: Option<usize> = None;
        if let Some(pos) = output.response.hover_pos() {
            let b = byte_of(text, output.galley.cursor_from_pos(pos - output.galley_pos).index.0);
            let is_br = |x: usize| text.get(x..).and_then(|s| s.chars().next()).is_some_and(|c| "()[]{}".contains(c));
            at = if is_br(b) { Some(b) } else if b > 0 && is_br(b - 1) { Some(b - 1) } else { None };
        }
        if let Some((p, q)) = sel {
            let (a, b) = (p.min(q), p.max(q));
            if b == a + 1 && text.get(a..b).is_some_and(|s| "()[]{}".contains(s)) {
                at = Some(a);
            }
        }
        if let Some(b) = at {
            let pr = match st.hover_pair {
                Some((kk, bb, pr)) if kk == k && bb == b => pr,
                _ => {
                    let pr = bracket_pair(text, b, cx.dialect);
                    st.hover_pair = Some((k, b, pr));
                    pr
                }
            };
            if let Some((x, y)) = pr {
                let c = if cx.dark { Color32::from_rgba_unmultiplied(120, 170, 255, 60) } else { Color32::from_rgba_unmultiplied(70, 120, 255, 50) };
                let line = Stroke::new(1.0, c.gamma_multiply(2.0));
                for z in [x, y] {
                    let r = char_rect(z, 1).expand2(egui::vec2(1.0, 0.0));
                    shapes.push(egui::Shape::rect_filled(r, 2.0, c));
                    shapes.push(egui::Shape::rect_stroke(r, 2.0, line, egui::StrokeKind::Inside));
                }
            }
        }
        painter.set(under, egui::Shape::Vec(shapes));
    }
    // The selected text again over egui's (hidden) glyphs, in its colours.
    if let Some(g) = &plain {
        let at = output.galley_pos - egui::vec2(g.rect.left(), 0.0);
        let color = ui.visuals().text_color();
        for r in &sel_rows {
            let c = r.intersect(clip);
            if c.is_positive() {
                ui.painter().with_clip_rect(c).galley(at, g.clone(), color);
            }
        }
    }

    // Frames round the block at the caret and the one under the pointer.
    if !st.blocks.is_empty() {
        let hover = output.response.hover_pos().map(|pos| byte_of(text, output.galley.cursor_from_pos(pos - output.galley_pos).index.0));
        let line_of = |b: usize| text.as_bytes()[..b].iter().filter(|&&c| c == b'\n').count();
        // The innermost one over lines that holds `at`.
        let inner = |at: usize| {
            st.blocks.iter().filter(|b| b.range.start <= at && at <= b.range.end && text.get(b.range.clone()).is_some_and(|t| t.contains('\n'))).max_by_key(|b| b.range.start)
        };
        let caret = cursor_b.and_then(|c| inner(c));
        let under = hover.and_then(|h| inner(h)).filter(|b| Some(*b) != caret);
        let hues = band_hues(cx.dark);
        let rows = &output.galley.rows;
        for (b, strong) in [(under, false), (caret, true)] {
            let Some(b) = b else { continue };
            let (l0, l1) = (line_of(b.range.start), line_of(b.range.end));
            if l1 >= rows.len() {
                continue;
            }
            let at = output.galley_pos.to_vec2();
            // Where its column is on its first line, as laid out (bold
            // keywords, comments, other scripts may be wider).
            let ls = text[..b.range.start].rfind('\n').map_or(0, |i| i + 1);
            let line = &text[ls..];
            let mut w = 0;
            let mut byte = line.find('\n').unwrap_or(line.len());
            for (i, ch) in line.char_indices() {
                if w >= b.col || ch == '\n' {
                    byte = i;
                    break;
                }
                w += if ch == '\t' { 4 } else { 1 };
            }
            let mut x = output.galley.pos_from_cursor(CCursor::new(char_of(text, ls + byte))).min.x;
            // Lines inside further left than that (its text under "IS ("):
            // the frame takes them in. (Once per text and block.)
            let key = (st.job.as_ref().map_or(0, |j| j.0), b.range.start);
            let inside = *st.frame_x.entry(key).or_insert_with(|| {
                rows[l0 + 1..=l1].iter().filter_map(|r| r.glyphs.iter().find(|g| !g.chr.is_whitespace()).map(|g| g.pos.x)).fold(f32::INFINITY, f32::min)
            });
            x = x.min(inside);
            let left = output.galley_pos.x + x - 3.0;
            let right = rows[l0..=l1].iter().map(|r| r.rect().right()).fold(0.0f32, f32::max) + at.x + 4.0;
            let top = rows[l0].rect().top() + at.y;
            let mut bottom = rows[l1].rect().bottom() + at.y;
            if bottom - rows[l1].rect().top() - at.y < 1.0 {
                bottom += row_h;
            }
            let rect = egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right.max(left + 4.0 * char_w), bottom));
            if !rect.intersects(clip) {
                continue;
            }
            let c = hues[b.level % hues.len()];
            let c = if strong { c.gamma_multiply(0.95) } else { c.gamma_multiply(0.6) };
            crisp_frame(&painter, rect, 3.0, c, ctx.pixels_per_point());
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

    // Right click: the caret goes there (unless inside the selection),
    // then a menu of what can be done there.
    let menu_sel = id.with("menu-sel");
    if right_press && output.response.hovered() {
        let (p, q) = sel_before.map(|(p, q)| (floor(text, p), floor(text, q))).unwrap_or((text.len(), text.len()));
        let (lo, hi) = (p.min(q), p.max(q));
        let at = ctx.input(|i| i.pointer.interact_pos()).map(|pos| byte_of(text, output.galley.cursor_from_pos(pos - output.galley_pos).index.0));
        let keep = match at {
            Some(at) if lo < hi && lo <= at && at <= hi => {
                set_cursor(&ctx, id, text, p, q);
                (lo, hi)
            }
            Some(at) => {
                set_cursor(&ctx, id, text, at, at);
                (at, at)
            }
            None => (lo, hi),
        };
        ctx.data_mut(|d| d.insert_temp(menu_sel, keep));
        ctx.memory_mut(|m| m.request_focus(id));
    }
    output.response.context_menu(|ui| {
        ui.set_min_width(260.0);
        let (a, b) = ctx.data(|d| d.get_temp::<(usize, usize)>(menu_sel)).unwrap_or((text.len(), text.len()));
        let (a, b) = (floor(text, a), floor(text, b));
        let has_sel = a < b;
        let item = |ui: &mut egui::Ui, label: &str, keys: &str, enabled: bool| -> bool {
            let r = ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(keys));
            if r.clicked() {
                ui.close();
            }
            r.clicked()
        };
        if item(ui, &format!("▶  {}", tr("Выполнить запрос", "Run statement")), "Ctrl+Enter", true) {
            out.actions.push(Action::RunStatement);
        }
        if item(ui, &format!("⏩  {}", tr("Выполнить скрипт", "Run script")), "F5", true) {
            out.actions.push(Action::RunScript);
        }
        if item(ui, &format!("🔍  {}", tr("План запроса", "Explain plan")), "Ctrl+E", true) {
            out.actions.push(Action::Explain);
        }
        ui.separator();
        if item(ui, tr("Вырезать", "Cut"), "Ctrl+X", has_sel) {
            ctx.copy_text(text[a..b].to_string());
            text.replace_range(a..b, "");
            set_cursor(&ctx, id, text, a, a);
            out.changed = true;
        }
        if item(ui, tr("Копировать", "Copy"), "Ctrl+C", has_sel) {
            ctx.copy_text(text[a..b].to_string());
        }
        if item(ui, tr("Вставить", "Paste"), "Ctrl+V", true) {
            // The system's clipboard comes in as a paste to the editor.
            ctx.memory_mut(|m| m.request_focus(id));
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
        if item(ui, tr("Выделить всё", "Select all"), "Ctrl+A", !text.is_empty()) {
            set_cursor(&ctx, id, text, text.len(), 0);
        }
        ui.separator();
        if item(ui, &format!("✨  {}", tr("Форматировать", "Format")), "Ctrl+Shift+F", true) {
            out.actions.push(Action::Format);
        }
        if item(ui, tr("Закомментировать строки", "Comment lines"), "Ctrl+/", true) {
            let (s, e) = toggle_comment(text, a, b);
            set_cursor(&ctx, id, text, e, s);
            out.changed = true;
        }
        if item(ui, tr("Дублировать строку", "Duplicate line"), "Ctrl+D", true) {
            let (s, e) = lines_of(text, a, b);
            let block = text[s..e].to_string();
            text.insert_str(e, &format!("\n{block}"));
            let c = b + block.len() + 1;
            set_cursor(&ctx, id, text, c, c);
            out.changed = true;
        }
        ui.separator();
        for (label, upper) in [(tr("ВЕРХНИЙ РЕГИСТР", "UPPER CASE"), true), (tr("нижний регистр", "lower case"), false)] {
            if item(ui, label, "", has_sel) {
                let s = if upper { text[a..b].to_uppercase() } else { text[a..b].to_lowercase() };
                text.replace_range(a..b, &s);
                set_cursor(&ctx, id, text, a + s.len(), a);
                out.changed = true;
            }
        }
    });

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
            match complete::hints(text, c, cx.dialect, cx.rels, cx.settings.hint_case, forced, cx.settings.hint_quote_schemas) {
                Some(h) => {
                    let typed_first = cx.settings.hints_typed_first;
                    // The same hint stays picked as the list changes (with
                    // the word as typed first: that, unless a hint was).
                    let was = st.popup.as_ref().and_then(|p| p.item(p.sel).and_then(|k| p.hints.items.get(k)));
                    let keep = was.and_then(|it| h.items.iter().position(|x| x == it)).map_or(0, |k| k + typed_first as usize);
                    st.popup = Some(Popup { hints: h, sel: keep, forced, typed_first });
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
                    if pop.typed_first {
                        let typed = text.get(pop.hints.range.clone()).unwrap_or("");
                        let sel = pop.sel == 0;
                        let r = ui
                            .horizontal(|ui| {
                                let resp = ui.add(egui::Button::selectable(sel, egui::RichText::new(format!("↵  {typed}")).monospace()).frame_when_inactive(sel));
                                ui.label(egui::RichText::new(t("как набрано", "as typed")).weak().small());
                                resp
                            })
                            .inner;
                        if sel {
                            r.scroll_to_me(None);
                        }
                        if r.clicked() {
                            picked = Some(usize::MAX);
                        }
                    }
                    for (k, it) in pop.hints.items.iter().enumerate() {
                        let sel = k + pop.typed_first as usize == pop.sel;
                        let r = ui
                            .horizontal(|ui| {
                                let resp = ui.add(egui::Button::selectable(sel, egui::RichText::new(format!("{}  {}", it.kind.icon(), it.label)).monospace()).frame_when_inactive(sel));
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
                let help = if pop.typed_first { t("↑↓ выбор · Enter/Tab вставить (на «как набрано» — перенос) · Esc закрыть", "↑↓ choose · Enter/Tab insert (on \"as typed\": a new line) · Esc close") } else { t("↑↓ выбор · Enter/Tab вставить · Esc закрыть", "↑↓ choose · Enter/Tab insert · Esc close") };
                ui.label(egui::RichText::new(help).weak().small());
            });
        });
    }
    if let Some(k) = picked {
        let pop = st.popup.take().unwrap();
        // (The word as typed: just closed.)
        if k != usize::MAX {
            let c = accept(text, &pop, k);
            set_cursor(&ctx, id, text, c, c);
            out.changed = true;
        }
        ctx.memory_mut(|m| m.request_focus(id));
    }
    out
}

/// The selection (bytes), if any; else None.
pub fn selection(st: &EditorState) -> Option<Range<usize>> {
    st.cursor.filter(|(a, b)| a != b).map(|(a, b)| a.min(b)..a.max(b))
}
