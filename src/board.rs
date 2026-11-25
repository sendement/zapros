//! Boards: a canvas of notes beside the scripts — frames, shapes, brush
//! strokes and text; tables sketched as entities with their fields;
//! notebooks from the tabs and the databases' tables, views and routines
//! put on it to write notes by; arrows linking them (a field to a field).
//! Each board is a JSON file of its own in the data folder.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::config::Rgb;
use crate::db::{ObjKind, Relation};
use crate::sql::Dialect;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Board {
    pub id: String,
    pub name: String,
    pub items: Vec<Item>,
    pub links: Vec<Link>,
    /// The world point at the canvas' top-left corner, and the zoom.
    pub pan: [f32; 2],
    pub zoom: f32,
    /// The next item's or link's id.
    pub next: u64,
    pub backdrop: Backdrop,
}

impl Default for Board {
    fn default() -> Self {
        Self { id: crate::db::new_id(), name: String::new(), items: Vec::new(), links: Vec::new(), pan: [0.0, 0.0], zoom: 1.0, next: 1, backdrop: Backdrop::Dots }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    /// x, y, w, h in the board's units.
    pub rect: [f32; 4],
    pub color: Rgb,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    #[default]
    Rect,
    Ellipse,
    Diamond,
    /// A flowchart's start or end.
    Pill,
    /// A flowchart's input or output.
    Parallelogram,
    /// A database.
    Cylinder,
    /// A preparation step.
    Hexagon,
    /// A sticky note.
    Note,
}

impl Shape {
    pub const ALL: [Shape; 8] = [Shape::Rect, Shape::Ellipse, Shape::Diamond, Shape::Pill, Shape::Parallelogram, Shape::Cylinder, Shape::Hexagon, Shape::Note];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dash {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// How a link or a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LineStyle {
    pub dash: Dash,
    /// Arrowheads at its start, at its end.
    pub start: bool,
    pub end: bool,
    pub width: f32,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self { dash: Dash::Solid, start: false, end: true, width: 1.6 }
    }
}

/// A zone's (a background's) look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pattern {
    #[default]
    Plain,
    Dots,
    Grid,
    Stripes,
}

/// The board's own background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backdrop {
    #[default]
    Dots,
    Grid,
    Lines,
    Plain,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    /// A frame (a rectangle, an ellipse, a diamond…) with text inside.
    Frame { shape: Shape, text: String, fill: bool },
    Text { text: String, size: f32 },
    /// A brush stroke: points in the board's units.
    Stroke { points: Vec<[f32; 2]>, width: f32 },
    /// A line on its own (not between items): its two ends.
    Line { a: [f32; 2], b: [f32; 2], style: LineStyle, label: String },
    /// A background zone with a title, under the rest.
    Zone { title: String, pattern: Pattern },
    /// A table sketched by hand: its name, fields, a note.
    Entity { name: String, fields: Vec<Field>, note: String },
    /// A tab's script or notebook (or one cell of it) as it was when put
    /// here (or last refreshed): its statements drawn as tables and
    /// schemes.
    Notebook { snap: Snap, note: String },
    /// A workspace: a frame round its tabs' frames.
    Workspace { ws: String, name: String, tabs: Vec<Snap>, note: String },
    /// A database's table, view, function, procedure…: its columns (or
    /// parameters) as last seen, and a note.
    Object { conn: String, path: Vec<String>, obj: ObjKind, name: String, cols: Vec<(String, String)>, note: String },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub key: bool,
    pub note: String,
}

/// A tab's statements as last taken.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snap {
    pub tab: String,
    pub title: String,
    /// Just one cell of a notebook: its number.
    pub cell: Option<usize>,
    pub parts: Vec<Part>,
}

/// A statement (or a markdown cell) and how it's drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    /// The notebook's cell it's in.
    pub cell: usize,
    pub sql: String,
    pub what: What,
    /// A block of code shown by its first line only.
    #[serde(default)]
    pub compact: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum What {
    /// A query: a table of the columns it gives.
    Query { title: String, cols: Vec<(String, String)> },
    /// Rows put into a table (INSERT, CREATE TABLE … AS): where from (a
    /// query's columns, or VALUES' rows), into which, its columns.
    Insert { verb: String, target: String, cols: Vec<(String, String)>, source: Option<(String, Vec<(String, String)>)>, values: usize },
    /// A table changed (UPDATE, DELETE, MERGE, CREATE TABLE, DROP…).
    Change { verb: String, target: String, cols: Vec<(String, String)> },
    /// A block (DECLARE / BEGIN, a function's or procedure's code): all of it.
    Code { title: String },
    /// A markdown cell.
    Note,
    Other { title: String },
}

/// An arrow from one item to another (from a field's row to a field's
/// row, for entities and tables).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub id: u64,
    pub from: u64,
    pub to: u64,
    #[serde(default)]
    pub from_field: Option<usize>,
    #[serde(default)]
    pub to_field: Option<usize>,
    #[serde(default)]
    pub label: String,
    pub color: Rgb,
    #[serde(default)]
    pub style: LineStyle,
}

/// Heights of an item's parts (board units).
pub const HEAD_H: f32 = 28.0;
pub const ROW_H: f32 = 20.0;
pub const NOTE_LINE_H: f32 = 17.0;
/// How far a field's link goes out sideways before it turns.
pub const LINK_STUB: f32 = 24.0;
/// A statement's table; the room round and between what a frame holds.
pub const PART_W: f32 = 210.0;
pub const PAD: f32 = 12.0;
pub const GAP: f32 = 18.0;
/// A line of code; a character's width (at 11 units).
pub const CODE_LINE_H: f32 = 15.0;
pub const CODE_CHAR_W: f32 = 6.7;
/// A scheme's arrow (INSERT, UPDATE…) between its boxes.
pub const ARROW_W: f32 = 84.0;
/// A tab's frame in a workspace: its title's strip.
pub const SUB_HEAD: f32 = 24.0;
/// Most rows a statement's table shows (the rest: "… N more").
pub const MAX_ROWS: usize = 24;

/// A table's height for `n` rows (one at least).
pub fn table_h(n: usize) -> f32 {
    HEAD_H + ROW_H * n.clamp(1, MAX_ROWS + 1) as f32 + 6.0
}

impl Part {
    /// Its size on the board.
    pub fn size(&self) -> Vec2 {
        let lines = self.sql.lines().count().max(1);
        match &self.what {
            What::Query { cols, .. } => Vec2::new(PART_W, table_h(cols.len())),
            What::Insert { cols, source, .. } => {
                let from = source.as_ref().map_or(HEAD_H + ROW_H + 6.0, |(_, c)| table_h(c.len()));
                Vec2::new(PAD * 2.0 + PART_W * 2.0 + ARROW_W, SUB_HEAD + PAD + from.max(table_h(cols.len())) + PAD)
            }
            What::Change { cols, .. } => Vec2::new(PAD * 2.0 + PART_W * 0.6 + ARROW_W + PART_W, SUB_HEAD + PAD + table_h(cols.len()).max(56.0) + PAD),
            What::Code { .. } => {
                let widest = self.sql.lines().map(|l| l.chars().count()).max().unwrap_or(10) as f32;
                let w = (widest * CODE_CHAR_W + 24.0).clamp(260.0, 680.0);
                let n = if self.compact { 2 } else { lines };
                Vec2::new(w, HEAD_H + 10.0 + CODE_LINE_H * n as f32)
            }
            What::Note => {
                let n: usize = self.sql.lines().map(|l| 1 + l.chars().count() / 40).sum::<usize>().max(1);
                Vec2::new(280.0, 12.0 + NOTE_LINE_H * n as f32)
            }
            What::Other { .. } => Vec2::new(280.0, HEAD_H + 10.0 + CODE_LINE_H * lines.min(4) as f32),
        }
    }
}

/// Boxes of these sizes laid left to right, a new row when `width` is
/// filled: their places (from 0, 0) and the height they take.
pub fn flow(sizes: &[Vec2], width: f32) -> (Vec<Pos2>, f32) {
    let mut out = Vec::with_capacity(sizes.len());
    let (mut x, mut y, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
    for s in sizes {
        if x > 0.0 && x + s.x > width {
            x = 0.0;
            y += row_h + GAP;
            row_h = 0.0;
        }
        out.push(Pos2::new(x, y));
        x += s.x + GAP;
        row_h = row_h.max(s.y);
    }
    (out, y + row_h)
}

/// How wide a frame round these parts likes to be (inside).
pub fn parts_width(parts: &[Part]) -> f32 {
    let sizes: Vec<Vec2> = parts.iter().map(Part::size).collect();
    let widest = sizes.iter().map(|s| s.x).fold(0.0, f32::max);
    let all = sizes.iter().map(|s| s.x).sum::<f32>() + GAP * sizes.len().saturating_sub(1) as f32;
    all.min(760.0f32.max(widest)).max(widest).max(PART_W)
}

impl Snap {
    /// Drawn as its one part, no frame round it (a query, a block).
    pub fn alone(&self) -> bool {
        self.parts.len() == 1 && matches!(self.parts[0].what, What::Query { .. } | What::Code { .. })
    }

    /// Its frame's size in a workspace (its title strip and all).
    pub fn sub_size(&self) -> Vec2 {
        let w = parts_width(&self.parts);
        let sizes: Vec<Vec2> = self.parts.iter().map(Part::size).collect();
        let (_, h) = flow(&sizes, w);
        Vec2::new(w + PAD * 2.0, SUB_HEAD + PAD + h.max(ROW_H) + PAD)
    }
}

/// Where an item's insides go (board units): the frames of a workspace's
/// tabs (tab index, rect) and every part (tab index, part index, rect).
#[derive(Default)]
pub struct Inside {
    pub subs: Vec<(usize, Rect)>,
    pub parts: Vec<(usize, usize, Rect)>,
}

impl Item {
    pub fn rect(&self) -> Rect {
        Rect::from_min_size(Pos2::new(self.rect[0], self.rect[1]), Vec2::new(self.rect[2], self.rect[3]))
    }

    pub fn set_rect(&mut self, r: Rect) {
        self.rect = [r.min.x, r.min.y, r.width(), r.height()];
    }

    pub fn row_count(&self) -> usize {
        match &self.kind {
            Kind::Entity { fields, .. } => fields.len(),
            Kind::Object { cols, .. } => cols.len(),
            Kind::Notebook { snap, .. } if snap.alone() => match &snap.parts[0].what {
                What::Query { cols, .. } => cols.len().min(MAX_ROWS),
                _ => 0,
            },
            _ => 0,
        }
    }

    /// The text a note line count is taken from.
    pub fn note(&self) -> &str {
        match &self.kind {
            Kind::Entity { note, .. } | Kind::Notebook { note, .. } | Kind::Object { note, .. } | Kind::Workspace { note, .. } => note,
            _ => "",
        }
    }

    /// How tall it must be to show all it holds (None: as the user made it).
    pub fn natural_h(&self) -> Option<f32> {
        let notes = |n: &str| if n.trim().is_empty() { 0.0 } else { 8.0 + NOTE_LINE_H * n.lines().count().max(1) as f32 };
        match &self.kind {
            Kind::Entity { .. } | Kind::Object { .. } => Some(HEAD_H + ROW_H * self.row_count().max(1) as f32 + 6.0 + notes(self.note())),
            Kind::Notebook { snap, .. } if snap.alone() => Some(snap.parts[0].size().y + notes(self.note())),
            Kind::Notebook { .. } | Kind::Workspace { .. } => {
                let inside = self.inside();
                let bottom = inside.subs.iter().map(|s| s.1.bottom()).chain(inside.parts.iter().map(|p| p.2.bottom())).fold(self.rect[1] + HEAD_H + PAD + ROW_H, f32::max);
                Some(bottom - self.rect[1] + PAD + notes(self.note()))
            }
            _ => None,
        }
    }

    /// The width it's put on with.
    pub fn natural_w(&self) -> f32 {
        match &self.kind {
            Kind::Notebook { snap, .. } if snap.alone() => snap.parts[0].size().x,
            Kind::Notebook { snap, .. } => parts_width(&snap.parts) + PAD * 2.0,
            Kind::Workspace { tabs, .. } => {
                let sizes: Vec<Vec2> = tabs.iter().map(Snap::sub_size).collect();
                let widest = sizes.iter().map(|s| s.x).fold(PART_W, f32::max);
                let all = sizes.iter().map(|s| s.x).sum::<f32>() + GAP * sizes.len().saturating_sub(1) as f32;
                all.min(1500.0f32.max(widest)).max(widest) + PAD * 2.0
            }
            _ => self.rect[2],
        }
    }

    /// Where its insides go (a notebook's parts, a workspace's tabs).
    pub fn inside(&self) -> Inside {
        let r = self.rect();
        let mut out = Inside::default();
        let top = Pos2::new(r.left() + PAD, r.top() + HEAD_H + PAD);
        let width = (r.width() - PAD * 2.0).max(PART_W);
        let place = |snap: &Snap, k: usize, at: Pos2, width: f32, out: &mut Inside| {
            let sizes: Vec<Vec2> = snap.parts.iter().map(Part::size).collect();
            let (pos, _) = flow(&sizes, width);
            for (j, (p, s)) in pos.iter().zip(&sizes).enumerate() {
                out.parts.push((k, j, Rect::from_min_size(at + p.to_vec2(), *s)));
            }
        };
        match &self.kind {
            Kind::Notebook { snap, .. } if snap.alone() => out.parts.push((0, 0, Rect::from_min_size(r.min, snap.parts[0].size()))),
            Kind::Notebook { snap, .. } => place(snap, 0, top, width, &mut out),
            Kind::Workspace { tabs, .. } => {
                let sizes: Vec<Vec2> = tabs.iter().map(|t| {
                    let s = t.sub_size();
                    Vec2::new(s.x.min(width), s.y)
                }).collect();
                let (pos, _) = flow(&sizes, width);
                for (k, (p, s)) in pos.iter().zip(&sizes).enumerate() {
                    let sub = Rect::from_min_size(top + p.to_vec2(), *s);
                    // (Narrowed to the workspace: its parts laid anew.)
                    let at = sub.min + Vec2::new(PAD, SUB_HEAD + PAD);
                    let sizes: Vec<Vec2> = tabs[k].parts.iter().map(Part::size).collect();
                    let (pp, h) = flow(&sizes, sub.width() - PAD * 2.0);
                    let sub = Rect::from_min_size(sub.min, Vec2::new(sub.width(), SUB_HEAD + PAD + h.max(ROW_H) + PAD));
                    out.subs.push((k, sub));
                    for (j, (q, sz)) in pp.iter().zip(&sizes).enumerate() {
                        out.parts.push((k, j, Rect::from_min_size(at + q.to_vec2(), *sz)));
                    }
                }
                // (Rows of tab frames grown as their parts were laid: the
                // ones below moved down to clear them.)
                let mut fixed: Vec<(usize, Rect)> = Vec::new();
                for (k, sub) in out.subs.iter().copied() {
                    let mut dy: f32 = 0.0;
                    for (_, f) in &fixed {
                        if f.x_range().intersects(sub.x_range()) && f.top() < sub.top() && f.bottom() + GAP > sub.top() + dy {
                            dy = dy.max(f.bottom() + GAP - sub.top());
                        }
                    }
                    let moved = sub.translate(Vec2::new(0.0, dy));
                    fixed.push((k, moved));
                    for p in out.parts.iter_mut().filter(|p| p.0 == k) {
                        p.2 = p.2.translate(Vec2::new(0.0, dy));
                    }
                }
                out.subs = fixed;
            }
            _ => {}
        }
        out
    }

    /// A field row's centre height (board units), for links.
    pub fn row_y(&self, i: usize) -> f32 {
        self.rect[1] + HEAD_H + ROW_H * (i as f32 + 0.5)
    }

    /// The field row at height `y`, if it's on one.
    pub fn row_at(&self, y: f32) -> Option<usize> {
        let k = ((y - self.rect[1] - HEAD_H) / ROW_H).floor();
        (k >= 0.0 && (k as usize) < self.row_count()).then_some(k as usize)
    }

    /// Hit by a point (strokes: near their line).
    pub fn hit(&self, p: Pos2, slack: f32) -> bool {
        match &self.kind {
            Kind::Line { a, b, style, .. } => dist_to_segment(p, Pos2::new(a[0], a[1]), Pos2::new(b[0], b[1])) <= style.width / 2.0 + slack + 3.0,
            // A zone: by its title's strip or its edge (inside, the board's
            // as usual: panned, picked by a box).
            Kind::Zone { .. } => {
                let r = self.rect();
                r.expand(slack).contains(p) && (p.y <= r.top() + HEAD_H || !r.shrink(8.0 + slack).contains(p))
            }
            Kind::Stroke { points, width } => {
                let near = width / 2.0 + slack;
                points.windows(2).any(|w| dist_to_segment(p, Pos2::new(w[0][0], w[0][1]), Pos2::new(w[1][0], w[1][1])) <= near) || (points.len() == 1 && Pos2::new(points[0][0], points[0][1]).distance(p) <= near)
            }
            Kind::Frame { shape: Shape::Ellipse, fill: false, .. } | Kind::Frame { shape: Shape::Ellipse, fill: true, .. } => {
                let r = self.rect().expand(slack);
                let c = r.center();
                let (a, b) = (r.width() / 2.0, r.height() / 2.0);
                a > 0.0 && b > 0.0 && ((p.x - c.x) / a).powi(2) + ((p.y - c.y) / b).powi(2) <= 1.0
            }
            _ => self.rect().expand(slack).contains(p),
        }
    }

    /// Moved by `d`.
    pub fn shift(&mut self, d: Vec2) {
        self.rect[0] += d.x;
        self.rect[1] += d.y;
        match &mut self.kind {
            Kind::Stroke { points, .. } => {
                for p in points {
                    p[0] += d.x;
                    p[1] += d.y;
                }
            }
            Kind::Line { a, b, .. } => {
                for p in [a, b] {
                    p[0] += d.x;
                    p[1] += d.y;
                }
            }
            _ => {}
        }
    }

    /// A line's end moved to `to` (its rect follows).
    pub fn set_end(&mut self, end_b: bool, to: Pos2) {
        if let Kind::Line { a, b, .. } = &mut self.kind {
            if end_b {
                *b = [to.x, to.y];
            } else {
                *a = [to.x, to.y];
            }
            let r = Rect::from_two_pos(Pos2::new(a[0], a[1]), Pos2::new(b[0], b[1]));
            self.set_rect(r);
        }
    }

    /// Resized to `to` (a stroke's points scaled along).
    pub fn resize(&mut self, to: Rect) {
        let from = self.rect();
        let sx = if from.width() > 0.0 { to.width() / from.width() } else { 1.0 };
        let sy = if from.height() > 0.0 { to.height() / from.height() } else { 1.0 };
        let scale = |p: &mut [f32; 2]| {
            p[0] = to.min.x + (p[0] - from.min.x) * sx;
            p[1] = to.min.y + (p[1] - from.min.y) * sy;
        };
        match &mut self.kind {
            Kind::Stroke { points, .. } => points.iter_mut().for_each(scale),
            Kind::Line { a, b, .. } => {
                scale(a);
                scale(b);
            }
            _ => {}
        }
        self.set_rect(to);
    }
}

pub fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Where a line from a rect's centre towards `to` leaves the rect.
pub fn edge_point(r: Rect, to: Pos2) -> Pos2 {
    let c = r.center();
    let d = to - c;
    if d.length_sq() < 1e-6 {
        return c;
    }
    let sx = if d.x.abs() > 1e-6 { (r.width() / 2.0) / d.x.abs() } else { f32::INFINITY };
    let sy = if d.y.abs() > 1e-6 { (r.height() / 2.0) / d.y.abs() } else { f32::INFINITY };
    c + d * sx.min(sy).min(1.0)
}

impl Board {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), ..Default::default() }
    }

    pub fn take_id(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    pub fn item(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: u64) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// Adds an item (its height as it needs), on top; its id.
    pub fn add(&mut self, rect: Rect, color: Rgb, kind: Kind) -> u64 {
        let id = self.take_id();
        let mut it = Item { id, rect: [rect.min.x, rect.min.y, rect.width(), rect.height()], color, kind };
        if let Some(h) = it.natural_h() {
            it.rect[3] = h;
        }
        // (Zones go under everything.)
        if matches!(it.kind, Kind::Zone { .. }) {
            let k = self.items.iter().take_while(|i| matches!(i.kind, Kind::Zone { .. })).count();
            self.items.insert(k, it);
        } else {
            self.items.push(it);
        }
        id
    }

    /// The items hit by a point, topmost first.
    pub fn hit(&self, p: Pos2, slack: f32) -> Option<u64> {
        self.items.iter().rev().find(|i| i.hit(p, slack)).map(|i| i.id)
    }

    /// Takes out items, and the links to or from them.
    pub fn remove(&mut self, ids: &[u64]) {
        self.items.retain(|i| !ids.contains(&i.id));
        self.links.retain(|l| !ids.contains(&l.from) && !ids.contains(&l.to) && !ids.contains(&l.id));
    }

    /// Items fit to their contents' height.
    pub fn fit_heights(&mut self) {
        for it in &mut self.items {
            if let Some(h) = it.natural_h() {
                it.rect[3] = h;
            }
        }
    }

    /// The link's way (board units), first point to last: a field's end
    /// leaves its row sideways (a short stub out, at the side facing the
    /// other item, or both at the right when they stand one under the
    /// other); else straight from where the line between the centres
    /// leaves each.
    pub fn link_path(&self, l: &Link) -> Option<Vec<Pos2>> {
        let a = self.item(l.from)?;
        let b = self.item(l.to)?;
        let (ra, rb) = (a.rect(), b.rect());
        let stacked = (ra.center().x - rb.center().x).abs() < (ra.width() + rb.width()) / 4.0;
        // Out to the right (+1) or the left (-1).
        let side = |r: Rect, other: Rect| if stacked || other.center().x >= r.center().x { 1.0 } else { -1.0 };
        let fa = l.from_field.filter(|&f| f < a.row_count());
        let fb = l.to_field.filter(|&f| f < b.row_count());
        let end = |it: &Item, r: Rect, f: Option<usize>, other: Rect, toward: Pos2| -> (Pos2, Option<f32>) {
            match f {
                Some(f) => {
                    let s = side(r, other);
                    (Pos2::new(if s > 0.0 { r.right() } else { r.left() }, it.row_y(f)), Some(s))
                }
                None => (edge_point(r, toward), None),
            }
        };
        let (pa, sa) = end(a, ra, fa, rb, rb.center());
        let (pb, sb) = end(b, rb, fb, ra, pa);
        let (pa, sa) = if fa.is_none() { end(a, ra, None, rb, pb) } else { (pa, sa) };
        let mut v = vec![pa];
        if let Some(s) = sa {
            v.push(pa + Vec2::new(LINK_STUB * s, 0.0));
        }
        if let (Some(s1), Some(s2)) = (sa, sb) {
            // Both at the same side: out to the farther edge.
            if s1 == s2 {
                let x = if s1 > 0.0 { pa.x.max(pb.x) + LINK_STUB } else { pa.x.min(pb.x) - LINK_STUB };
                v[1].x = x;
                v.push(Pos2::new(x, pb.y));
                v.push(pb);
                return Some(v);
            }
        }
        if let Some(s) = sb {
            v.push(pb + Vec2::new(LINK_STUB * s, 0.0));
        }
        v.push(pb);
        Some(v)
    }

    /// A point's distance from a link's way.
    pub fn link_dist(&self, l: &Link, p: Pos2) -> f32 {
        self.link_path(l).map_or(f32::INFINITY, |v| v.windows(2).map(|w| dist_to_segment(p, w[0], w[1])).fold(f32::INFINITY, f32::min))
    }

    /// A place for something `size` big near `at` (its centre) that
    /// covers nothing: rings of tries around it, else `at`.
    pub fn free_spot(&self, at: Pos2, size: Vec2) -> Pos2 {
        let fits = |c: Pos2| {
            let r = Rect::from_center_size(c, size).expand(16.0);
            !self.items.iter().any(|i| i.rect().intersects(r))
        };
        let step = Vec2::new(size.x + 40.0, size.y + 40.0) / 2.0;
        for ring in 0..8i32 {
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs() != ring && dy.abs() != ring {
                        continue;
                    }
                    let c = at + Vec2::new(dx as f32 * step.x, dy as f32 * step.y);
                    if fits(c) {
                        return c;
                    }
                }
            }
        }
        at
    }

    /// Everything's bounds (None: empty).
    pub fn bounds(&self) -> Option<Rect> {
        self.items.iter().map(|i| i.rect()).reduce(|a, b| a.union(b))
    }
}

/// Where boards are kept.
pub fn dir() -> PathBuf {
    crate::config::data_dir().join("boards")
}

fn path_of(id: &str) -> PathBuf {
    dir().join(format!("{id}.json"))
}

pub fn save(b: &Board) -> std::io::Result<()> {
    std::fs::create_dir_all(dir())?;
    let tmp = dir().join(format!("{}.json.tmp", b.id));
    std::fs::write(&tmp, serde_json::to_vec_pretty(b).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, path_of(&b.id))
}

pub fn load(id: &str) -> Option<Board> {
    let s = std::fs::read_to_string(path_of(id)).ok()?;
    serde_json::from_str(&s).ok()
}

/// The boards there are: (id, name), by name.
pub fn list() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "json").then_some(())?;
            let b: Board = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
            Some((b.id, b.name))
        })
        .collect();
    v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    v
}

pub fn delete(id: &str) {
    let _ = std::fs::remove_file(path_of(id));
}

/// An open board: what's on screen, what was last saved and the steps
/// back and forth.
#[derive(Default)]
pub struct Open {
    pub board: Board,
    /// As last committed (a step's start).
    pub committed: Board,
    pub undo: Vec<Board>,
    pub redo: Vec<Board>,
}

impl Open {
    pub fn new(b: Board) -> Self {
        Self { committed: b.clone(), board: b, undo: Vec::new(), redo: Vec::new() }
    }

    /// The board changed since the last step: that's a step (true: to be
    /// saved). Pan and zoom aren't steps, but are saved.
    pub fn commit(&mut self) -> bool {
        if self.board == self.committed {
            return false;
        }
        let only_view = Board { pan: self.committed.pan, zoom: self.committed.zoom, ..self.board.clone() } == self.committed;
        if !only_view {
            self.undo.push(self.committed.clone());
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.committed = self.board.clone();
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(mut b) = self.undo.pop() else { return false };
        (b.pan, b.zoom) = (self.board.pan, self.board.zoom);
        self.redo.push(std::mem::replace(&mut self.board, b));
        self.committed = self.board.clone();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut b) = self.redo.pop() else { return false };
        (b.pan, b.zoom) = (self.board.pan, self.board.zoom);
        self.undo.push(std::mem::replace(&mut self.board, b));
        self.committed = self.board.clone();
        true
    }
}

/// The boards: their list and those open.
#[derive(Default)]
pub struct Boards {
    pub list: Vec<(String, String)>,
    pub open: HashMap<String, Open>,
    /// The board things go to from the tree's menu (the last one shown).
    pub last: Option<String>,
}

impl Boards {
    pub fn load() -> Self {
        Self { list: list(), ..Default::default() }
    }

    pub fn get(&mut self, id: &str) -> Option<&mut Open> {
        if !self.open.contains_key(id) {
            let b = load(id)?;
            self.open.insert(id.to_string(), Open::new(b));
        }
        self.open.get_mut(id)
    }

    pub fn create(&mut self, name: &str) -> String {
        let b = Board::new(name);
        let id = b.id.clone();
        let _ = save(&b);
        self.open.insert(id.clone(), Open::new(b));
        self.list = list();
        id
    }

    pub fn rename(&mut self, id: &str, name: &str) {
        if let Some(o) = self.get(id) {
            o.board.name = name.to_string();
            o.committed.name = name.to_string();
            let _ = save(&o.board);
        }
        self.list = list();
    }

    pub fn remove(&mut self, id: &str) {
        self.open.remove(id);
        delete(id);
        if self.last.as_deref() == Some(id) {
            self.last = None;
        }
        self.list = list();
    }

    pub fn name(&self, id: &str) -> String {
        self.list.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone()).unwrap_or_default()
    }
}

/// What can be dropped on a board from elsewhere (the database tree).
#[derive(Clone, Debug)]
pub struct Drop {
    pub conn: String,
    pub path: Vec<String>,
    pub obj: ObjKind,
    pub name: String,
}

/// What's dragged from the board's own library.
#[derive(Clone, Debug)]
pub enum Lib {
    Shape(Shape),
    Text,
    Entity,
    Line(LineStyle),
    Zone(Pattern),
    /// A workspace, a tab, a notebook's cell (by ids, the cell's number).
    Workspace(String),
    Tab(String),
    Cell(String, usize),
}

/// A tab's cells (a script's: one, its text): markdown or not, text.
pub fn parts_of(cells: &[(bool, String)], d: Dialect, rels: &[Relation]) -> Vec<Part> {
    let mut out = Vec::new();
    for (k, (md, text)) in cells.iter().enumerate() {
        if *md {
            if !text.trim().is_empty() {
                out.push(Part { cell: k, sql: text.trim().to_string(), what: What::Note, compact: false });
            }
            continue;
        }
        for r in crate::sql::split(text, d, true) {
            let sql = text[r].trim();
            if sql.is_empty() {
                continue;
            }
            out.push(Part { cell: k, sql: sql.to_string(), what: what_of(sql, d, rels), compact: false });
        }
    }
    out
}

/// A name without its quotes, if it has them.
fn unq(s: &str) -> String {
    crate::sql::complete::col_label(s)
}

/// What a statement is, for drawing.
pub fn what_of(sql: &str, d: Dialect, rels: &[Relation]) -> What {
    use crate::sql::complete::{query_columns, scope};
    use crate::sql::lex::{self, Tk};
    let toks: Vec<lex::Tok> = lex::lex(sql, d).into_iter().filter(|t| !t.trivia()).collect();
    let up = |k: usize| toks.get(k).map_or(String::new(), |t| t.text(sql).to_ascii_uppercase());
    let first_line = || {
        let l = sql.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        if l.chars().count() > 48 { format!("{}…", l.chars().take(47).collect::<String>()) } else { l.to_string() }
    };
    // A dotted name from token k: its text, the token after it.
    let name_at = |mut k: usize| -> (String, usize) {
        let mut s = String::new();
        while let Some(t) = toks.get(k) {
            if !matches!(t.kind, Tk::Word | Tk::Quoted | Tk::Keyword) {
                break;
            }
            s.push_str(t.text(sql));
            k += 1;
            if toks.get(k).is_some_and(|t| t.kind == Tk::Dot) {
                s.push('.');
                k += 1;
            } else {
                break;
            }
        }
        (s, k)
    };
    let close_of = |open: usize| {
        let mut depth = 0;
        for (j, t) in toks.iter().enumerate().skip(open) {
            match t.kind {
                Tk::Open => depth += 1,
                Tk::Close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
        }
        None
    };
    // The first word at the top level among these (after WITH's CTEs).
    let top_word = |words: &[&str]| {
        let mut depth = 0;
        for (j, t) in toks.iter().enumerate() {
            match t.kind {
                Tk::Open => depth += 1,
                Tk::Close => depth -= 1,
                _ if depth == 0 && words.contains(&t.text(sql).to_ascii_uppercase().as_str()) => return Some(j),
                _ => {}
            }
        }
        None
    };
    let rel_cols = |name: &str| -> Vec<(String, String)> {
        let bare = unq(name.rsplit('.').next().unwrap_or(name));
        let schema = name.rsplit_once('.').map(|(s, _)| unq(s.rsplit('.').next().unwrap_or(s)));
        rels.iter()
            .find(|r| r.name.eq_ignore_ascii_case(&bare) && schema.as_deref().is_none_or(|s| r.schema.eq_ignore_ascii_case(s)))
            .map(|r| r.cols.clone())
            .unwrap_or_default()
    };
    let tables_of = |q: &str| {
        let refs = scope(q, d);
        let mut names: Vec<String> = Vec::new();
        for r in refs {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&r.table)) {
                names.push(r.table);
            }
        }
        match names.len() {
            0 => "SELECT".to_string(),
            1 | 2 => names.join(", "),
            n => format!("{}, {} +{}", names[0], names[1], n - 2),
        }
    };
    if crate::sql::is_plsql_block(sql, d) || matches!(up(0).as_str(), "DECLARE" | "BEGIN" | "DO") || (up(0) == "CREATE" && (1..6).any(|k| matches!(up(k).as_str(), "FUNCTION" | "PROCEDURE" | "PACKAGE" | "TRIGGER"))) {
        return What::Code { title: first_line() };
    }
    let main = top_word(&["SELECT", "INSERT", "UPDATE", "DELETE", "MERGE"]).map(|k| up(k));
    match up(0).as_str() {
        "SELECT" | "WITH" | "(" | "VALUES" | "TABLE" if !matches!(main.as_deref(), Some("INSERT" | "UPDATE" | "DELETE" | "MERGE")) => {
            What::Query { title: tables_of(sql), cols: query_columns(sql, d, rels) }
        }
        _ => {
            let verb = main.clone().filter(|_| up(0) == "WITH").unwrap_or_else(|| up(0));
            let at = toks.iter().position(|t| t.text(sql).eq_ignore_ascii_case(&verb)).unwrap_or(0);
            match verb.as_str() {
                "INSERT" | "REPLACE" | "UPSERT" => {
                    let mut k = at + 1;
                    while matches!(up(k).as_str(), "INTO" | "TABLE" | "OVERWRITE" | "IGNORE" | "OR") {
                        k += 1;
                    }
                    let (target, mut k) = name_at(k);
                    let mut cols: Vec<(String, String)> = Vec::new();
                    let known = rel_cols(&target);
                    if toks.get(k).is_some_and(|t| t.kind == Tk::Open) {
                        if let Some(c) = close_of(k) {
                            cols = toks[k + 1..c]
                                .iter()
                                .filter(|t| matches!(t.kind, Tk::Word | Tk::Quoted | Tk::Keyword))
                                .map(|t| {
                                    let n = t.text(sql).to_string();
                                    let ty = known.iter().find(|(c, _)| unq(c).eq_ignore_ascii_case(&unq(&n))).map(|x| x.1.clone()).unwrap_or_default();
                                    (n, ty)
                                })
                                .collect();
                            k = c + 1;
                        }
                    }
                    if cols.is_empty() {
                        cols = known;
                    }
                    let rest = toks.get(k).map_or("", |t| &sql[t.start..]);
                    let rest_up = up(k);
                    if matches!(rest_up.as_str(), "SELECT" | "WITH" | "(") {
                        What::Insert { verb: "INSERT".into(), target, cols, source: Some((tables_of(rest), query_columns(rest, d, rels))), values: 0 }
                    } else {
                        // VALUES (…), (…): its rows.
                        let mut n = 0;
                        let mut depth = 0;
                        for t in toks.iter().skip(k + 1) {
                            match t.kind {
                                Tk::Open => {
                                    if depth == 0 {
                                        n += 1;
                                    }
                                    depth += 1;
                                }
                                Tk::Close => depth -= 1,
                                _ => {}
                            }
                        }
                        What::Insert { verb: "INSERT".into(), target, cols, source: None, values: n }
                    }
                }
                "CREATE" => {
                    // CREATE [OR REPLACE] [TEMP…] TABLE / VIEW name …
                    let what = (1..8).find(|&k| matches!(up(k).as_str(), "TABLE" | "VIEW"));
                    let Some(w) = what else { return What::Other { title: first_line() } };
                    let mut k = w + 1;
                    if up(k) == "IF" {
                        k += 3;
                    }
                    let (target, k) = name_at(k);
                    let kind = format!("CREATE {}", up(w));
                    if let Some(a) = top_word(&["AS"]).filter(|&a| a >= k) {
                        let q = toks.get(a + 1).map_or("", |t| &sql[t.start..]);
                        let src = query_columns(q, d, rels);
                        return What::Insert { verb: kind, target, cols: src.clone(), source: Some((tables_of(q), src)), values: 0 };
                    }
                    // Its columns as defined.
                    let mut cols = Vec::new();
                    if toks.get(k).is_some_and(|t| t.kind == Tk::Open) {
                        if let Some(c) = close_of(k) {
                            let mut item: Vec<&lex::Tok> = Vec::new();
                            let mut depth = 0;
                            let mut push = |item: &mut Vec<&lex::Tok>| {
                                let first = item.first().map(|t| t.text(sql).to_ascii_uppercase()).unwrap_or_default();
                                if !item.is_empty() && !matches!(first.as_str(), "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "FOREIGN" | "CHECK" | "INDEX" | "KEY" | "PROJECTION" | "EXCLUDE" | "LIKE") {
                                    let ty = item.get(1).map_or(String::new(), |t| t.text(sql).to_string());
                                    cols.push((item[0].text(sql).to_string(), ty));
                                }
                                item.clear();
                            };
                            for t in &toks[k + 1..c] {
                                match t.kind {
                                    Tk::Open => depth += 1,
                                    Tk::Close => depth -= 1,
                                    Tk::Comma if depth == 0 => {
                                        push(&mut item);
                                        continue;
                                    }
                                    _ => {}
                                }
                                item.push(t);
                            }
                            push(&mut item);
                        }
                    }
                    What::Change { verb: kind, target, cols }
                }
                "UPDATE" => {
                    let (target, k) = name_at(at + 1);
                    // SET a = …, b = …: the columns set.
                    let mut cols = Vec::new();
                    if let Some(s) = (k..toks.len()).find(|&j| up(j) == "SET") {
                        let mut depth = 0;
                        for j in s + 1..toks.len() {
                            match toks[j].kind {
                                Tk::Open => depth += 1,
                                Tk::Close => depth -= 1,
                                _ if depth == 0 && matches!(up(j).as_str(), "WHERE" | "FROM" | "RETURNING") => break,
                                Tk::Op if depth == 0 && toks[j].text(sql) == "=" => {
                                    if let Some(n) = toks.get(j - 1).filter(|t| matches!(t.kind, Tk::Word | Tk::Quoted | Tk::Keyword)) {
                                        let n = n.text(sql).to_string();
                                        let ty = rel_cols(&target).into_iter().find(|(c, _)| unq(c).eq_ignore_ascii_case(&unq(&n))).map(|x| x.1).unwrap_or_default();
                                        cols.push((n, ty));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    What::Change { verb: "UPDATE".into(), target, cols }
                }
                "DELETE" | "MERGE" | "TRUNCATE" | "DROP" | "ALTER" => {
                    let mut k = at + 1;
                    while matches!(up(k).as_str(), "FROM" | "INTO" | "TABLE" | "VIEW" | "IF" | "EXISTS" | "ONLY") {
                        k += 1;
                    }
                    let (target, _) = name_at(k);
                    if target.is_empty() {
                        return What::Other { title: first_line() };
                    }
                    What::Change { verb: verb.clone(), target, cols: Vec::new() }
                }
                _ => What::Other { title: first_line() },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(b: &mut Board, x: f32, fields: &[&str]) -> u64 {
        let fields = fields.iter().map(|f| Field { name: f.to_string(), ..Default::default() }).collect();
        b.add(Rect::from_min_size(Pos2::new(x, 0.0), Vec2::new(160.0, 10.0)), [92, 140, 230], Kind::Entity { name: "t".into(), fields, note: String::new() })
    }

    #[test]
    fn items_links_and_steps() {
        let mut b = Board::new("b");
        let a = entity(&mut b, 0.0, &["id", "name"]);
        let c = entity(&mut b, 400.0, &["id", "a_id"]);
        // As tall as its rows.
        assert_eq!(b.item(a).unwrap().rect[3], HEAD_H + 2.0 * ROW_H + 6.0);
        // A field's row by its height; a link from it leaves at the side facing the other.
        let it = b.item(c).unwrap();
        assert_eq!(it.row_at(it.row_y(1)), Some(1));
        assert_eq!(it.row_at(it.rect[1] + 2.0), None);
        let id = b.take_id();
        b.links.push(Link { id, from: c, to: a, from_field: Some(1), to_field: Some(0), label: String::new(), color: [0, 0, 0], style: LineStyle::default() });
        let v = b.link_path(&b.links[0]).unwrap();
        let (p, q) = (v[0], *v.last().unwrap());
        assert_eq!(p.x, 400.0);
        assert_eq!(q.x, 160.0);
        assert_eq!(p.y, b.item(c).unwrap().row_y(1));
        // Out sideways from each row first.
        assert_eq!(v[1], p - Vec2::new(LINK_STUB, 0.0));
        assert_eq!(v[2], q + Vec2::new(LINK_STUB, 0.0));
        // One under the other: both out at the right, round the farther edge.
        b.item_mut(c).unwrap().rect[0] = 0.0;
        b.item_mut(c).unwrap().rect[1] = 300.0;
        let v = b.link_path(&b.links[0]).unwrap();
        assert_eq!(v.len(), 4, "{v:?}");
        assert!(v[1].x > 160.0 && v[1].x == v[2].x);
        assert_eq!(b.link_dist(&b.links[0], Pos2::new(v[1].x, 200.0)), 0.0);
        b.item_mut(c).unwrap().rect[0] = 400.0;
        b.item_mut(c).unwrap().rect[1] = 0.0;
        // The top one is hit; taking it out takes its links.
        assert_eq!(b.hit(Pos2::new(410.0, 10.0), 0.0), Some(c));
        b.remove(&[c]);
        assert!(b.links.is_empty());
        // A stroke: hit near its line only; moved with its points.
        let s = b.add(Rect::from_min_max(Pos2::new(0.0, 200.0), Pos2::new(100.0, 300.0)), [0, 0, 0], Kind::Stroke { points: vec![[0.0, 200.0], [100.0, 300.0]], width: 2.0 });
        assert_eq!(b.hit(Pos2::new(50.0, 250.0), 2.0), Some(s));
        assert_eq!(b.hit(Pos2::new(90.0, 210.0), 2.0), None);
        b.item_mut(s).unwrap().shift(Vec2::new(10.0, 0.0));
        assert!(matches!(&b.item(s).unwrap().kind, Kind::Stroke { points, .. } if points[0] == [10.0, 200.0]));
    }

    fn what(sql: &str, d: Dialect) -> What {
        let rels = vec![Relation { schema: "public".into(), name: "orders".into(), cols: vec![("id".into(), "int".into()), ("total".into(), "numeric".into()), ("note".into(), "text".into())], ..Default::default() }];
        what_of(sql, d, &rels)
    }

    #[test]
    fn statements_drawn_by_what_they_are() {
        let pg = Dialect::Postgres;
        // A query: its columns, * opened up, an expression as written.
        match what("with t as (select id, total from orders) select t.*, count(*), 1 + 2 as three from t", pg) {
            What::Query { cols, .. } => assert_eq!(cols.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["id", "total", "count(*)", "three"]),
            w => panic!("{w:?}"),
        }
        // INSERT … SELECT: from the query into the table.
        match what("insert into archive (id, sum) select id, total from orders where total > 0", pg) {
            What::Insert { target, cols, source: Some((from, src)), .. } => {
                assert_eq!(target, "archive");
                assert_eq!(cols.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["id", "sum"]);
                assert_eq!(from, "orders");
                assert_eq!(src.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["id", "total"]);
            }
            w => panic!("{w:?}"),
        }
        // VALUES: its rows counted; the table's own columns when none listed.
        match what("insert into orders values (1, 2, 'a'), (2, 3, 'b')", pg) {
            What::Insert { source: None, values: 2, cols, target, .. } => assert_eq!(cols.len(), 3, "{target:?}"),
            w => panic!("{w:?}"),
        }
        assert!(matches!(what("create table big as select * from orders", pg), What::Insert { verb, cols, .. } if verb == "CREATE TABLE" && cols.len() == 3));
        assert!(matches!(what("create table t (id int primary key, name text, constraint c check (id > 0))", pg), What::Change { cols, .. } if cols == vec![("id".to_string(), "int".to_string()), ("name".to_string(), "text".to_string())]));
        assert!(matches!(what("update shop.orders o set note = 'x', total = total * 2 where id = 1", pg), What::Change { verb, target, cols } if verb == "UPDATE" && target == "shop.orders" && cols.len() == 2));
        assert!(matches!(what("delete from orders where id = 1", pg), What::Change { verb, .. } if verb == "DELETE"));
        assert!(matches!(what("declare\n  n number;\nbegin\n  null;\nend;", Dialect::Oracle), What::Code { .. }));
        assert!(matches!(what("do $$ begin perform 1; end $$", pg), What::Code { .. }));
        // A notebook's cells: markdown as notes, statements split.
        let parts = parts_of(&[(true, "# Hi".into()), (false, "select 1 a;\nselect 2 b;".into())], pg, &[]);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts.iter().map(|p| p.cell).collect::<Vec<_>>(), [0, 1, 1]);
    }

    #[test]
    fn notebooks_and_workspaces_laid_out() {
        let parts = parts_of(&[(false, "select 1 a;\nselect 1 a, 2 b;\ninsert into t select 1 x;".into())], Dialect::Postgres, &[]);
        let snap = Snap { tab: "t".into(), title: "s".into(), cell: None, parts };
        let mut b = Board::new("b");
        let mut it = Item { id: 0, rect: [0.0, 0.0, 10.0, 10.0], color: [0, 0, 0], kind: Kind::Notebook { snap: snap.clone(), note: String::new() } };
        it.rect[2] = it.natural_w();
        let id = b.add(it.rect(), [0, 0, 0], it.kind.clone());
        let it = b.item(id).unwrap();
        let inside = it.inside();
        assert_eq!(inside.parts.len(), 3);
        // Inside the frame, none over another.
        for (i, (_, _, r)) in inside.parts.iter().enumerate() {
            assert!(it.rect().contains_rect(*r), "{r:?} in {:?}", it.rect());
            for (_, _, q) in &inside.parts[i + 1..] {
                assert!(!r.intersects(*q), "{r:?} {q:?}");
            }
        }
        // A workspace of three such tabs: their frames apart, inside it.
        let ws = Kind::Workspace { ws: "w".into(), name: "W".into(), tabs: vec![snap.clone(), snap.clone(), snap], note: String::new() };
        let mut it = Item { id: 0, rect: [0.0, 0.0, 10.0, 10.0], color: [0, 0, 0], kind: ws };
        it.rect[2] = it.natural_w().min(900.0);
        it.rect[3] = it.natural_h().unwrap();
        let inside = it.inside();
        assert_eq!(inside.subs.len(), 3);
        for (i, (_, r)) in inside.subs.iter().enumerate() {
            assert!(it.rect().contains_rect(*r), "{r:?} in {:?}", it.rect());
            for (_, q) in &inside.subs[i + 1..] {
                assert!(!r.intersects(*q), "{r:?} {q:?}");
            }
        }
        // A query alone: just its table, its columns linkable rows.
        let one = Snap { tab: "t".into(), title: "q".into(), cell: None, parts: parts_of(&[(false, "select 1 a, 2 b".into())], Dialect::Postgres, &[]) };
        let it = Item { id: 0, rect: [0.0, 0.0, PART_W, 10.0], color: [0, 0, 0], kind: Kind::Notebook { snap: one, note: String::new() } };
        assert_eq!(it.row_count(), 2);
        assert_eq!(it.natural_h(), Some(table_h(2)));
    }

    #[test]
    fn lines_and_zones() {
        let mut b = Board::new("b");
        let l = b.add(Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 0.0)), [0, 0, 0], Kind::Line { a: [0.0, 0.0], b: [100.0, 0.0], style: LineStyle::default(), label: String::new() });
        assert_eq!(b.hit(Pos2::new(50.0, 2.0), 1.0), Some(l));
        b.item_mut(l).unwrap().set_end(true, Pos2::new(100.0, 80.0));
        assert_eq!(b.item(l).unwrap().rect, [0.0, 0.0, 100.0, 80.0]);
        // A zone goes under the rest; hit by its title or edge only.
        let z = b.add(Rect::from_min_size(Pos2::new(200.0, 0.0), Vec2::new(300.0, 300.0)), [0, 0, 0], Kind::Zone { title: "z".into(), pattern: Pattern::Dots });
        assert_eq!(b.items[0].id, z);
        assert_eq!(b.hit(Pos2::new(350.0, 10.0), 0.0), Some(z));
        assert_eq!(b.hit(Pos2::new(350.0, 150.0), 0.0), None);
        assert_eq!(b.hit(Pos2::new(203.0, 150.0), 0.0), Some(z));
    }

    #[test]
    fn undo_redo_and_view() {
        let mut o = Open::new(Board::new("b"));
        o.board.add(Rect::from_min_size(Pos2::ZERO, Vec2::splat(50.0)), [1, 2, 3], Kind::Text { text: "x".into(), size: 16.0 });
        let _ = o.commit();
        assert_eq!(o.undo.len(), 1);
        // Pan and zoom: kept, but not a step.
        o.board.zoom = 2.0;
        let _ = o.commit();
        assert_eq!(o.undo.len(), 1);
        assert!(o.undo());
        assert!(o.board.items.is_empty());
        assert_eq!(o.board.zoom, 2.0, "the view stays");
        assert!(o.redo());
        assert_eq!(o.board.items.len(), 1);
        // A board as JSON and back.
        let s = serde_json::to_string(&o.board).unwrap();
        assert_eq!(serde_json::from_str::<Board>(&s).unwrap(), o.board);
    }
}
