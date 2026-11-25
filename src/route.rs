//! Links routed flat: only along x and y, round what's in the way. A
//! sparse grid of the lines just off each obstacle (and halfway between
//! them) is searched for the path of least length plus a price per turn
//! (A*, a turn is part of the state). Paths are kept until anything they
//! depend on moves.

use std::collections::{BinaryHeap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use egui::{Pos2, Rect, Vec2};

/// The room kept round an obstacle.
pub const MARGIN: f32 = 12.0;
/// How far a link leaves its end straight out before it may turn.
pub const STUB: f32 = 24.0;
/// A turn costs as much as this much length.
const BEND: f32 = 60.0;

/// One way a link may leave (or reach) its end: the point on the edge and
/// the way out (a unit along x or y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Port {
    pub at: Pos2,
    pub out: Vec2,
}

impl Port {
    fn stub(&self) -> Pos2 {
        self.at + self.out * STUB
    }
}

/// The ways out of a rectangle: a field row's two sides, or the middle of
/// each of its four sides.
pub fn ports(r: Rect, row: Option<f32>) -> Vec<Port> {
    match row {
        Some(y) => vec![Port { at: Pos2::new(r.right(), y), out: Vec2::X }, Port { at: Pos2::new(r.left(), y), out: -Vec2::X }],
        None => vec![
            Port { at: Pos2::new(r.right(), r.center().y), out: Vec2::X },
            Port { at: Pos2::new(r.left(), r.center().y), out: -Vec2::X },
            Port { at: Pos2::new(r.center().x, r.bottom()), out: Vec2::Y },
            Port { at: Pos2::new(r.center().x, r.top()), out: -Vec2::Y },
        ],
    }
}

static CACHE: Mutex<Option<HashMap<u64, Vec<Pos2>>>> = Mutex::new(None);

/// The path from one of `from` to one of `to` round `obstacles`: kept
/// while none of them moves.
pub fn route(from: &[Port], to: &[Port], obstacles: &[Rect]) -> Vec<Pos2> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in from.iter().chain(to) {
        [p.at.x, p.at.y, p.out.x, p.out.y].iter().for_each(|f| f.to_bits().hash(&mut h));
    }
    from.len().hash(&mut h);
    for r in obstacles {
        [r.min.x, r.min.y, r.max.x, r.max.y].iter().for_each(|f| f.to_bits().hash(&mut h));
    }
    let key = h.finish();
    if let Some(v) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(&key)) {
        return v.clone();
    }
    let v = search(from, to, obstacles).unwrap_or_else(|| plain(from, to));
    let mut c = CACHE.lock().unwrap();
    let c = c.get_or_insert_with(HashMap::new);
    if c.len() > 4000 {
        c.clear();
    }
    c.insert(key, v.clone());
    v
}

/// No way round found: the nearest pair of ends joined by a Z.
fn plain(from: &[Port], to: &[Port]) -> Vec<Pos2> {
    let mut best: Option<(f32, Vec<Pos2>)> = None;
    for a in from {
        for b in to {
            let (qa, qb) = (a.stub(), b.stub());
            let mid = if a.out.x != 0.0 { vec![Pos2::new((qa.x + qb.x) / 2.0, qa.y), Pos2::new((qa.x + qb.x) / 2.0, qb.y)] } else { vec![Pos2::new(qa.x, (qa.y + qb.y) / 2.0), Pos2::new(qb.x, (qa.y + qb.y) / 2.0)] };
            let d = (qa - qb).abs();
            let len = d.x + d.y;
            if best.as_ref().is_none_or(|(l, _)| len < *l) {
                let mut v = vec![a.at, qa];
                v.extend(mid);
                v.extend([qb, b.at]);
                best = Some((len, simplify(v)));
            }
        }
    }
    best.map(|b| b.1).unwrap_or_default()
}

/// Corners only: points on a straight run between two others dropped.
fn simplify(v: Vec<Pos2>) -> Vec<Pos2> {
    let mut out: Vec<Pos2> = Vec::with_capacity(v.len());
    for p in v {
        if out.last().is_some_and(|l| (*l - p).length() < 0.01) {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let straight = ((a.x - b.x).abs() < 0.01 && (b.x - p.x).abs() < 0.01) || ((a.y - b.y).abs() < 0.01 && (b.y - p.y).abs() < 0.01);
            if straight {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// The directions, as indices: right, left, down, up.
const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

fn dir_of(v: Vec2) -> usize {
    if v.x > 0.5 {
        0
    } else if v.x < -0.5 {
        1
    } else if v.y > 0.5 {
        2
    } else {
        3
    }
}

fn opposite(d: usize) -> usize {
    d ^ 1
}

/// A cost ordered for the heap (lowest first).
#[derive(PartialEq)]
struct Open(f32, u32);

impl Eq for Open {}

impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Open {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.0.partial_cmp(&self.0).unwrap_or(std::cmp::Ordering::Equal).then(o.1.cmp(&self.1))
    }
}

fn search(from: &[Port], to: &[Port], obstacles: &[Rect]) -> Option<Vec<Pos2>> {
    if from.is_empty() || to.is_empty() {
        return None;
    }
    let walls: Vec<Rect> = obstacles.iter().map(|r| r.expand(MARGIN)).collect();
    let inside = |p: Pos2| walls.iter().any(|w| p.x > w.min.x + 0.01 && p.x < w.max.x - 0.01 && p.y > w.min.y + 0.01 && p.y < w.max.y - 0.01);
    // The grid's lines: just off each obstacle, through each stub, round
    // everything, and halfway between those.
    let mut xs: Vec<f32> = Vec::new();
    let mut ys: Vec<f32> = Vec::new();
    for w in &walls {
        xs.extend([w.min.x, w.max.x]);
        ys.extend([w.min.y, w.max.y]);
    }
    for p in from.iter().chain(to) {
        let q = p.stub();
        xs.push(q.x);
        ys.push(q.y);
    }
    let all = walls.iter().copied().chain(from.iter().chain(to).map(|p| Rect::from_two_pos(p.at, p.stub()))).reduce(|a, b| a.union(b))?;
    xs.extend([all.min.x - MARGIN * 2.0, all.max.x + MARGIN * 2.0]);
    ys.extend([all.min.y - MARGIN * 2.0, all.max.y + MARGIN * 2.0]);
    let lines = |mut v: Vec<f32>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        let mids: Vec<f32> = v.windows(2).filter(|w| w[1] - w[0] > MARGIN).map(|w| (w[0] + w[1]) / 2.0).collect();
        v.extend(mids);
        v.sort_by(|a, b| a.total_cmp(b));
        v
    };
    let (xs, ys) = (lines(xs), lines(ys));
    let (nx, ny) = (xs.len(), ys.len());
    let find = |v: &[f32], x: f32| v.iter().enumerate().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map(|(i, _)| i).unwrap_or(0);
    let node = |p: Pos2| (find(&xs, p.x), find(&ys, p.y));
    let pos = |i: usize, j: usize| Pos2::new(xs[i], ys[j]);
    // Ends are free even when close to something else.
    let mut free = vec![false; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            free[j * nx + i] = !inside(pos(i, j));
        }
    }
    let mut goal: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, p) in to.iter().enumerate() {
        let (i, j) = node(p.stub());
        free[j * nx + i] = true;
        goal.entry(j * nx + i).or_default().push(k);
    }
    let targets: Vec<Pos2> = to.iter().map(Port::stub).collect();
    let guess = |p: Pos2| targets.iter().map(|t| (t.x - p.x).abs() + (t.y - p.y).abs()).fold(f32::INFINITY, f32::min);
    // States: node × the way it was entered.
    let state = |n: usize, d: usize| (n * 4 + d) as u32;
    let mut cost = vec![f32::INFINITY; nx * ny * 4];
    let mut back: Vec<u32> = vec![u32::MAX; nx * ny * 4];
    let mut start_of: HashMap<u32, usize> = HashMap::new();
    let mut heap = BinaryHeap::new();
    for (k, p) in from.iter().enumerate() {
        let (i, j) = node(p.stub());
        let n = j * nx + i;
        free[n] = true;
        let s = state(n, dir_of(p.out));
        if cost[s as usize] > 0.0 {
            cost[s as usize] = 0.0;
            start_of.insert(s, k);
            heap.push(Open(guess(pos(i, j)), s));
        }
    }
    let mut best: Option<(f32, u32, usize)> = None;
    while let Some(Open(f, s)) = heap.pop() {
        if best.as_ref().is_some_and(|b| f >= b.0) {
            break;
        }
        let (n, d) = (s as usize / 4, s as usize % 4);
        let g = cost[s as usize];
        if f > g + guess(pos(n % nx, n / nx)) + 0.01 {
            continue;
        }
        if let Some(ks) = goal.get(&n) {
            for &k in ks {
                // In along the last stub: a turn there unless coming the right way.
                let want = opposite(dir_of(to[k].out));
                let total = g + if d == want { 0.0 } else if d == opposite(want) { BEND * 2.0 } else { BEND };
                if best.as_ref().is_none_or(|b| total < b.0) {
                    best = Some((total, s, k));
                }
            }
        }
        let (i, j) = (n % nx, n / nx);
        for (nd, (dx, dy)) in DIRS.iter().enumerate() {
            if nd == opposite(d) {
                continue;
            }
            let (ni, nj) = (i as i32 + dx, j as i32 + dy);
            if ni < 0 || nj < 0 || ni as usize >= nx || nj as usize >= ny {
                continue;
            }
            let (ni, nj) = (ni as usize, nj as usize);
            let m = nj * nx + ni;
            if !free[m] {
                continue;
            }
            let (a, b) = (pos(i, j), pos(ni, nj));
            if inside(a + (b - a) / 2.0) {
                continue;
            }
            let ng = g + (b - a).length() + if nd == d { 0.0 } else { BEND };
            let t = state(m, nd);
            if ng + 0.001 < cost[t as usize] {
                cost[t as usize] = ng;
                back[t as usize] = s;
                heap.push(Open(ng + guess(b), t));
            }
        }
    }
    let (_, mut s, k) = best?;
    let mut pts = vec![to[k].at];
    loop {
        let n = s as usize / 4;
        pts.push(pos(n % nx, n / nx));
        if let Some(&ks) = start_of.get(&s) {
            pts.push(from[ks].at);
            break;
        }
        s = back[s as usize];
        if s == u32::MAX {
            return None;
        }
    }
    pts.reverse();
    Some(simplify(pts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(v: &[Pos2]) -> bool {
        v.windows(2).all(|w| (w[0].x - w[1].x).abs() < 0.01 || (w[0].y - w[1].y).abs() < 0.01)
    }

    fn crosses(v: &[Pos2], r: Rect) -> bool {
        v.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            (0..=20).any(|k| {
                let p = a + (b - a) * (k as f32 / 20.0);
                p.x > r.min.x + 0.5 && p.x < r.max.x - 0.5 && p.y > r.min.y + 0.5 && p.y < r.max.y - 0.5
            })
        })
    }

    #[test]
    fn straight_when_nothing_is_in_the_way() {
        let a = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(100.0, 60.0));
        let b = Rect::from_min_size(Pos2::new(300.0, 0.0), Vec2::new(100.0, 60.0));
        let v = route(&ports(a, None), &ports(b, None), &[a, b]);
        assert_eq!(v, vec![Pos2::new(100.0, 30.0), Pos2::new(300.0, 30.0)]);
    }

    #[test]
    fn round_what_is_between_on_x_and_y_only() {
        let a = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(100.0, 60.0));
        let b = Rect::from_min_size(Pos2::new(400.0, 0.0), Vec2::new(100.0, 60.0));
        let wall = Rect::from_min_size(Pos2::new(180.0, -100.0), Vec2::new(120.0, 400.0));
        let v = route(&ports(a, Some(20.0)), &ports(b, Some(40.0)), &[a, b, wall]);
        assert!(flat(&v), "{v:?}");
        assert!(!crosses(&v, wall) && !crosses(&v, a) && !crosses(&v, b), "{v:?}");
        assert_eq!(v.first(), Some(&Pos2::new(100.0, 20.0)));
        assert_eq!(v.last(), Some(&Pos2::new(400.0, 40.0)));
        // As few turns as it takes: up (or down) round the wall and back.
        assert!(v.len() <= 6, "{v:?}");
    }

    #[test]
    fn from_a_field_to_one_beside_it_on_the_same_table() {
        let t = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(200.0, 300.0));
        let v = route(&ports(t, Some(50.0)), &ports(t, Some(150.0)), &[t]);
        assert!(flat(&v) && !crosses(&v, t), "{v:?}");
        assert_eq!(v.len(), 4, "out, along, in: {v:?}");
    }

    #[test]
    fn walled_in_still_joined() {
        let a = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(100.0, 60.0));
        let b = Rect::from_min_size(Pos2::new(300.0, 0.0), Vec2::new(100.0, 60.0));
        let all = Rect::from_min_size(Pos2::new(-50.0, -50.0), Vec2::new(500.0, 160.0));
        let v = route(&ports(a, None), &ports(b, None), &[a, b, all]);
        assert!(v.len() >= 2 && flat(&v), "{v:?}");
    }
}
