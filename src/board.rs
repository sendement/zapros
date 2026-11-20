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
use crate::db::ObjKind;

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
}

impl Default for Board {
    fn default() -> Self {
        Self { id: crate::db::new_id(), name: String::new(), items: Vec::new(), links: Vec::new(), pan: [0.0, 0.0], zoom: 1.0, next: 1 }
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    /// A frame (a rectangle, an ellipse, a diamond) with text inside.
    Frame { shape: Shape, text: String, fill: bool },
    Text { text: String, size: f32 },
    /// A brush stroke: points in the board's units.
    Stroke { points: Vec<[f32; 2]>, width: f32 },
    /// A table sketched by hand: its name, fields, a note.
    Entity { name: String, fields: Vec<Field>, note: String },
    /// A notebook from a tab: its cells as they were when put here (or
    /// last refreshed), as a card or a flowchart of cells.
    Notebook { tab: String, title: String, cells: Vec<NbCell>, flow: bool, note: String },
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NbCell {
    pub md: bool,
    pub text: String,
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
}

/// Heights of an item's parts (board units).
pub const HEAD_H: f32 = 28.0;
pub const ROW_H: f32 = 20.0;
pub const NOTE_LINE_H: f32 = 17.0;
/// A notebook's cell in a flowchart: its box and the gap (the arrow) below.
pub const FLOW_BOX_H: f32 = 58.0;
pub const FLOW_GAP: f32 = 22.0;
/// How far a field's link goes out sideways before it turns.
pub const LINK_STUB: f32 = 24.0;
/// Lines of a cell a notebook's card shows.
pub const CARD_LINES: usize = 3;

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
            _ => 0,
        }
    }

    /// The text a note line count is taken from.
    pub fn note(&self) -> &str {
        match &self.kind {
            Kind::Entity { note, .. } | Kind::Notebook { note, .. } | Kind::Object { note, .. } => note,
            _ => "",
        }
    }

    /// How tall it must be to show all it holds (None: as the user made it).
    pub fn natural_h(&self) -> Option<f32> {
        let notes = |n: &str| if n.trim().is_empty() { 0.0 } else { 8.0 + NOTE_LINE_H * n.lines().count().max(1) as f32 };
        match &self.kind {
            Kind::Entity { .. } | Kind::Object { .. } => Some(HEAD_H + ROW_H * self.row_count().max(1) as f32 + 6.0 + notes(self.note())),
            Kind::Notebook { cells, flow, .. } => {
                let body = if *flow {
                    cells.len() as f32 * (FLOW_BOX_H + FLOW_GAP) - FLOW_GAP + 16.0
                } else {
                    cells.iter().map(|c| 10.0 + NOTE_LINE_H * c.text.lines().count().clamp(1, CARD_LINES) as f32).sum::<f32>() + 8.0
                };
                Some(HEAD_H + body.max(ROW_H) + notes(self.note()))
            }
            _ => None,
        }
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
        if let Kind::Stroke { points, .. } = &mut self.kind {
            for p in points {
                p[0] += d.x;
                p[1] += d.y;
            }
        }
    }

    /// Resized to `to` (a stroke's points scaled along).
    pub fn resize(&mut self, to: Rect) {
        let from = self.rect();
        if let Kind::Stroke { points, .. } = &mut self.kind {
            let sx = if from.width() > 0.0 { to.width() / from.width() } else { 1.0 };
            let sy = if from.height() > 0.0 { to.height() / from.height() } else { 1.0 };
            for p in points {
                p[0] = to.min.x + (p[0] - from.min.x) * sx;
                p[1] = to.min.y + (p[1] - from.min.y) * sy;
            }
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
        self.items.push(it);
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
        b.links.push(Link { id, from: c, to: a, from_field: Some(1), to_field: Some(0), label: String::new(), color: [0, 0, 0] });
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
