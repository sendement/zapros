//! A result as a chart: one column along X (numbers, dates or just
//! labels), the others' values as points, lines or bars over it.

use std::collections::BTreeSet;

use egui::{Color32, RichText};
use egui_plot::{Bar, BarChart, Legend, Line, Plot, PlotPoints, Points};

use crate::app::ResultView;
use crate::i18n::t;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Kind {
    #[default]
    Lines,
    Points,
    Bars,
}

/// What X holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Axis {
    Numbers,
    /// Seconds since 1970 (UTC as written).
    Dates,
    /// Each distinct value its own place, in the order they come.
    Labels(Vec<String>),
}

/// The chart's settings, kept with the result.
#[derive(Clone, Debug, Default)]
pub struct Chart {
    /// Shown instead of the table.
    pub on: bool,
    pub x: usize,
    pub ys: BTreeSet<usize>,
    pub kind: Kind,
    /// X and Y picked (else they're guessed on first show).
    pub set: bool,
    cache: Option<(Key, Data)>,
}

#[derive(Clone, Debug, PartialEq)]
struct Key {
    rows: usize,
    order: Vec<usize>,
    x: usize,
    ys: Vec<usize>,
    kind: Kind,
}

/// The points worked out.
#[derive(Clone, Debug, Default)]
pub struct Data {
    pub axis: Option<Axis>,
    /// Per Y column: its name and points (x, y).
    pub series: Vec<(usize, String, Vec<[f64; 2]>)>,
}

/// A number in a cell ("1 234,5" and "12.5%" too).
pub fn number(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('%');
    if s.is_empty() {
        return None;
    }
    s.parse::<f64>().ok().or_else(|| s.replace([' ', '\u{a0}'], "").replace(',', ".").parse().ok()).filter(|v: &f64| v.is_finite())
}

/// A date or a timestamp in a cell, as seconds.
pub fn date(s: &str) -> Option<f64> {
    use chrono::{NaiveDate, NaiveDateTime};
    let s = s.trim();
    if s.len() < 8 || !s.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    // A zone at the end ("+03", "+03:00", "Z") left out.
    let body = s.trim_end_matches('Z');
    let body = match body.rfind(['+', '-']) {
        Some(i) if i > 10 && body[i + 1..].chars().all(|c| c.is_ascii_digit() || c == ':') => &body[..i],
        _ => body,
    };
    for f in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M", "%d.%m.%Y %H:%M:%S", "%d.%m.%Y %H:%M"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(body, f) {
            return Some(d.and_utc().timestamp_millis() as f64 / 1000.0);
        }
    }
    for f in ["%Y-%m-%d", "%d.%m.%Y", "%Y/%m/%d"] {
        if let Ok(d) = NaiveDate::parse_from_str(body, f) {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp() as f64);
        }
    }
    None
}

/// What column `c` holds, over the rows shown.
pub fn axis_of(v: &ResultView, c: usize) -> Axis {
    let vals = || v.order.iter().filter_map(|&r| v.table.rows.get(r).and_then(|row| row.get(c)).and_then(|x| x.as_deref()));
    let any = vals().next().is_some();
    if any && vals().all(|s| number(s).is_some()) {
        return Axis::Numbers;
    }
    if any && vals().all(|s| date(s).is_some()) {
        return Axis::Dates;
    }
    let mut seen = std::collections::HashSet::new();
    Axis::Labels(vals().filter(|s| seen.insert(s.to_string())).map(str::to_string).collect())
}

/// Columns with numbers in them (some at least), for Y.
pub fn numeric_cols(v: &ResultView) -> Vec<usize> {
    (0..v.table.cols.len())
        .filter(|&c| {
            let mut n = 0;
            let mut all = 0;
            for &r in v.order.iter().take(500) {
                if let Some(s) = v.table.rows.get(r).and_then(|row| row.get(c)).and_then(|x| x.as_deref()) {
                    all += 1;
                    if number(s).is_some() {
                        n += 1;
                    }
                }
            }
            n > 0 && n * 2 >= all
        })
        .collect()
}

/// X and Y as a first guess: a date or text column along X (else the
/// first), every other numeric column up.
pub fn guess(v: &ResultView) -> (usize, BTreeSet<usize>) {
    let nums = numeric_cols(v);
    let shown = if v.cols.len() == v.table.cols.len() { v.cols.clone() } else { (0..v.table.cols.len()).collect() };
    let x = shown.iter().copied().find(|&c| !nums.contains(&c) || matches!(axis_of(v, c), Axis::Dates)).or_else(|| shown.first().copied()).unwrap_or(0);
    let ys = nums.into_iter().filter(|&c| c != x).take(6).collect();
    (x, ys)
}

/// The points of the chart for the rows shown (filtered, in their order;
/// numbers and dates along X sorted for lines).
pub fn data(v: &ResultView, x: usize, ys: &BTreeSet<usize>, kind: Kind) -> Data {
    let axis = axis_of(v, x);
    let cell = |r: usize, c: usize| v.table.rows.get(r).and_then(|row| row.get(c)).and_then(|s| s.as_deref());
    let place = |s: &str| -> Option<f64> {
        match &axis {
            Axis::Numbers => number(s),
            Axis::Dates => date(s),
            Axis::Labels(l) => l.iter().position(|x| x == s).map(|i| i as f64),
        }
    };
    let mut series = Vec::new();
    for &c in ys {
        let mut pts: Vec<[f64; 2]> = v.order.iter().filter_map(|&r| Some([place(cell(r, x)?)?, number(cell(r, c)?)?])).collect();
        if kind == Kind::Lines && !matches!(axis, Axis::Labels(_)) {
            pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        }
        series.push((c, v.table.cols.get(c).map_or_else(String::new, |col| col.name.clone()), pts));
    }
    Data { axis: Some(axis), series }
}

const COLORS: [Color32; 8] = [
    Color32::from_rgb(66, 133, 244),
    Color32::from_rgb(76, 175, 80),
    Color32::from_rgb(240, 160, 40),
    Color32::from_rgb(229, 80, 80),
    Color32::from_rgb(171, 71, 188),
    Color32::from_rgb(38, 166, 154),
    Color32::from_rgb(240, 98, 146),
    Color32::from_rgb(141, 110, 99),
];

pub fn color(k: usize) -> Color32 {
    COLORS[k % COLORS.len()]
}

/// A date's text for the axis, as fine as the span shown needs.
fn date_text(secs: f64, span: f64) -> String {
    let Some(d) = chrono::DateTime::from_timestamp_millis((secs * 1000.0) as i64) else { return String::new() };
    let f = if span < 120.0 {
        "%H:%M:%S"
    } else if span < 2.0 * 86400.0 {
        "%d.%m %H:%M"
    } else if span < 400.0 * 86400.0 {
        "%d.%m.%Y"
    } else {
        "%m.%Y"
    };
    d.format(f).to_string()
}

/// A value's text in the hover note.
fn x_text(axis: &Axis, x: f64, span: f64) -> String {
    match axis {
        Axis::Numbers => trim_num(x),
        Axis::Dates => date_text(x, span.min(86400.0)),
        Axis::Labels(l) => l.get(x.round().max(0.0) as usize).cloned().unwrap_or_default(),
    }
}

fn trim_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        let s = format!("{x:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// The settings row and the chart itself.
pub fn show(ui: &mut egui::Ui, id: &str, v: &mut ResultView) {
    if !v.chart.set {
        let (x, ys) = guess(v);
        v.chart.x = x;
        v.chart.ys = ys;
        v.chart.set = true;
    }
    let names: Vec<String> = v.table.cols.iter().map(|c| c.name.clone()).collect();
    let nums = numeric_cols(v);
    let mut ch = std::mem::take(&mut v.chart);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(t("Ось X:", "X axis:")).weak());
        egui::ComboBox::from_id_salt(("chart-x", id)).selected_text(names.get(ch.x).cloned().unwrap_or_default()).show_ui(ui, |ui| {
            for (c, n) in names.iter().enumerate() {
                if ui.selectable_label(ch.x == c, n).clicked() {
                    ch.x = c;
                    ch.ys.remove(&c);
                }
            }
        });
        ui.separator();
        ui.label(RichText::new(t("Значения:", "Values:")).weak());
        for (k, &c) in nums.iter().filter(|&&c| c != ch.x).enumerate() {
            let on = ch.ys.contains(&c);
            let col = color(k);
            let text = RichText::new(format!("● {}", names[c])).color(if on { col } else { ui.visuals().weak_text_color() });
            if ui.selectable_label(on, text).on_hover_text(t("Показать или скрыть этот столбец", "Show or hide this column")).clicked() {
                if on {
                    ch.ys.remove(&c);
                } else {
                    ch.ys.insert(c);
                }
            }
        }
        if nums.iter().all(|&c| c == ch.x) {
            ui.label(RichText::new(t("нет столбцов с числами", "no columns with numbers")).weak().italics());
        }
        ui.separator();
        for (k, name) in [(Kind::Lines, t("Линии", "Lines")), (Kind::Points, t("Точки", "Points")), (Kind::Bars, t("Столбцы", "Bars"))] {
            if ui.selectable_label(ch.kind == k, name).clicked() {
                ch.kind = k;
            }
        }
    });
    // The points, worked out again only when something they come from changed.
    let key = Key { rows: v.table.rows.len(), order: v.order.clone(), x: ch.x, ys: ch.ys.iter().copied().collect(), kind: ch.kind };
    if ch.cache.as_ref().is_none_or(|(k, _)| *k != key) {
        ch.cache = Some((key, data(v, ch.x, &ch.ys, ch.kind)));
    }
    let d = ch.cache.as_ref().map(|c| c.1.clone()).unwrap_or_default();
    // Each column keeps its colour from the chips above.
    let tint = |c: usize| color(nums.iter().filter(|&&n| n != ch.x).position(|&n| n == c).unwrap_or(0));
    let axis = d.axis.clone().unwrap_or(Axis::Numbers);
    let x_name = names.get(ch.x).cloned().unwrap_or_default();
    let ax = axis.clone();
    let ax2 = axis.clone();
    let mut plot = Plot::new(("chart", id))
        .legend(Legend::default())
        .x_axis_label(x_name.clone())
        .allow_scroll(false)
        .x_axis_formatter(move |m, range| {
            let span = range.end() - range.start();
            match &ax {
                Axis::Numbers => trim_num(m.value),
                Axis::Dates => date_text(m.value, span),
                Axis::Labels(l) => {
                    let i = m.value.round();
                    if (m.value - i).abs() > 1e-6 || i < 0.0 {
                        String::new()
                    } else {
                        l.get(i as usize).cloned().unwrap_or_default()
                    }
                }
            }
        })
        .label_formatter(move |hp| match hp {
            egui_plot::HoverPosition::NearDataPoint { plot_name, position, .. } => Some(format!("{plot_name}\n{x_name}: {}\n{}", x_text(&ax2, position.x, 0.0), trim_num(position.y))),
            _ => None,
        });
    if let Axis::Labels(_) = axis {
        // Labels: one mark per place.
        plot = plot.x_grid_spacer(|g| {
            let (a, b) = g.bounds;
            let n = (b - a).max(1.0);
            let step = (n / 12.0).ceil().max(1.0);
            let mut out = Vec::new();
            let mut x = (a.max(0.0) / step).ceil() * step;
            while x <= b {
                out.push(egui_plot::GridMark { value: x, step_size: step });
                x += step;
            }
            out
        });
    }
    let kind = ch.kind;
    let n = d.series.len().max(1) as f64;
    // The narrowest gap along X: the bars' width.
    let gap = d
        .series
        .iter()
        .flat_map(|(_, _, p)| {
            let mut xs: Vec<f64> = p.iter().map(|p| p[0]).collect();
            xs.sort_by(f64::total_cmp);
            xs.dedup();
            xs.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>()
        })
        .fold(f64::INFINITY, f64::min);
    let gap = if gap.is_finite() && gap > 0.0 { gap } else { 1.0 };
    plot.show(ui, |p| {
        for (k, (c, name, pts)) in d.series.iter().enumerate() {
            let col = tint(*c);
            match kind {
                Kind::Lines => {
                    p.line(Line::new(name.clone(), PlotPoints::new(pts.clone())).color(col).width(2.0));
                    if pts.len() <= 200 {
                        p.points(Points::new(name.clone(), PlotPoints::new(pts.clone())).color(col).radius(2.5));
                    }
                }
                Kind::Points => p.points(Points::new(name.clone(), PlotPoints::new(pts.clone())).color(col).radius(3.0)),
                Kind::Bars => {
                    let w = gap * 0.8 / n;
                    let off = (k as f64 - (n - 1.0) / 2.0) * w;
                    let bars = pts.iter().map(|q| Bar::new(q[0] + off, q[1]).width(w * 0.95)).collect();
                    p.bar_chart(BarChart::new(name.clone(), bars).color(col));
                }
            }
        }
    });
    v.chart = ch;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Col, Table};

    fn view(cols: &[&str], rows: &[&[&str]]) -> ResultView {
        let table = Table { cols: cols.iter().map(|c| Col { name: c.to_string(), ..Default::default() }).collect(), rows: rows.iter().map(|r| r.iter().map(|s| (!s.is_empty()).then(|| s.to_string())).collect()).collect(), more: false };
        ResultView::new("select".into(), table, Default::default(), None)
    }

    #[test]
    fn axes_and_points() {
        assert_eq!(number("1 234,5"), Some(1234.5));
        assert_eq!(number("12%"), Some(12.0));
        assert_eq!(number("abc"), None);
        assert!(date("2026-11-16").is_some() && date("2026-11-16 10:20:30.5+03").is_some() && date("16.11.2026").is_some());
        assert_eq!(date("2026-11-16 00:00:00"), date("2026-11-16"));
        assert_eq!(date("12345"), None);
        // A date X, two numeric Ys; the text column isn't one of them.
        let v = view(&["day", "orders", "total", "who"], &[&["2026-01-03", "5", "10.5", "a"], &["2026-01-01", "3", "7", "b"], &["2026-01-02", "", "8", "c"]]);
        let (x, ys) = guess(&v);
        assert_eq!(x, 0);
        assert_eq!(ys.iter().copied().collect::<Vec<_>>(), [1, 2]);
        let d = data(&v, x, &ys, Kind::Lines);
        assert_eq!(d.axis, Some(Axis::Dates));
        // Sorted along X; an empty value left out.
        let xs: Vec<f64> = d.series[0].2.iter().map(|p| p[0]).collect();
        assert_eq!(xs.len(), 2);
        assert!(xs[0] < xs[1]);
        assert_eq!(d.series[1].2.iter().map(|p| p[1]).collect::<Vec<_>>(), [7.0, 8.0, 10.5]);
        // Text along X: places in the order they come.
        let d = data(&v, 3, &ys, Kind::Bars);
        assert_eq!(d.axis, Some(Axis::Labels(vec!["a".into(), "b".into(), "c".into()])));
        assert_eq!(d.series[1].2, [[0.0, 10.5], [1.0, 7.0], [2.0, 8.0]]);
        // Numbers along X.
        let v = view(&["n", "sq"], &[&["2", "4"], &["1", "1"], &["3", "9"]]);
        let (x, ys) = guess(&v);
        assert_eq!((x, ys.iter().copied().collect::<Vec<_>>()), (0, vec![1]));
        assert_eq!(data(&v, x, &ys, Kind::Lines).series[0].2, [[1.0, 1.0], [2.0, 4.0], [3.0, 9.0]]);
        // Points keep the rows' order.
        assert_eq!(data(&v, x, &ys, Kind::Points).series[0].2[0], [2.0, 4.0]);
    }
}
