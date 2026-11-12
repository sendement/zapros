//! Colour schemes: the window's colours and the editor's palette together
//! (Windows 11, Dracula, Ubuntu, Nord, Solarized…), and the backdrop's
//! gradient.

use std::sync::Mutex;

use egui::{Color32, Stroke};
use serde::{Deserialize, Serialize};

use crate::config::{Palette, Rgb};

pub struct Scheme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    /// The sheet and panels.
    pub bg: Rgb,
    /// Around the sheet (tabs, status line, side panel).
    pub back: Rgb,
    /// Text fields.
    pub field: Rgb,
    /// Buttons.
    pub surface: Rgb,
    pub border: Rgb,
    pub text: Rgb,
    pub weak: Rgb,
    pub accent: Rgb,
    /// keyword, function, type, string, number, comment, quoted, param,
    /// operator, text.
    pub words: [Rgb; 10],
}

impl Scheme {
    pub fn palette(&self) -> Palette {
        let w = self.words;
        let base = if self.dark { Palette::dark() } else { Palette::light() };
        Palette { keyword: w[0], function: w[1], type_: w[2], string: w[3], number: w[4], comment: w[5], quoted: w[6], param: w[7], operator: w[8], text: w[9], ..base }
    }
}

macro_rules! scheme {
    ($id:literal, $name:literal, $dark:literal, bg $bg:expr, back $back:expr, field $field:expr, surface $surface:expr, border $border:expr, text $text:expr, weak $weak:expr, accent $accent:expr, words $words:expr) => {
        Scheme { id: $id, name: $name, dark: $dark, bg: $bg, back: $back, field: $field, surface: $surface, border: $border, text: $text, weak: $weak, accent: $accent, words: $words }
    };
}

pub const SCHEMES: &[Scheme] = &[
    scheme!("win11-dark", "Windows 11 Dark", true, bg [39, 39, 39], back [32, 32, 32], field [29, 29, 29], surface [55, 55, 55], border [64, 64, 64], text [255, 255, 255], weak [190, 190, 190], accent [96, 205, 255],
        words [[86, 156, 214], [220, 220, 170], [78, 201, 176], [206, 145, 120], [181, 206, 168], [106, 153, 85], [156, 220, 254], [156, 220, 254], [212, 212, 212], [212, 212, 212]]),
    scheme!("win11-light", "Windows 11 Light", false, bg [251, 251, 251], back [238, 240, 244], field [255, 255, 255], surface [253, 253, 253], border [222, 222, 222], text [27, 27, 27], weak [96, 96, 96], accent [0, 95, 184],
        words [[0, 0, 255], [121, 94, 38], [38, 127, 153], [163, 21, 21], [9, 134, 88], [0, 128, 0], [0, 16, 128], [0, 16, 128], [0, 0, 0], [0, 0, 0]]),
    scheme!("dracula", "Dracula", true, bg [40, 42, 54], back [33, 34, 44], field [30, 31, 41], surface [68, 71, 90], border [60, 63, 82], text [248, 248, 242], weak [160, 165, 195], accent [189, 147, 249],
        words [[255, 121, 198], [80, 250, 123], [139, 233, 253], [241, 250, 140], [189, 147, 249], [98, 114, 164], [255, 184, 108], [255, 184, 108], [255, 121, 198], [248, 248, 242]]),
    scheme!("ubuntu", "Ubuntu", true, bg [48, 10, 36], back [38, 6, 28], field [42, 8, 32], surface [84, 32, 68], border [98, 42, 84], text [238, 238, 236], weak [190, 168, 184], accent [233, 84, 32],
        words [[255, 138, 80], [114, 159, 207], [252, 233, 79], [138, 226, 52], [173, 127, 168], [150, 128, 145], [52, 226, 226], [239, 41, 41], [211, 215, 207], [238, 238, 236]]),
    scheme!("ubuntu-light", "Ubuntu Light", false, bg [250, 250, 250], back [236, 233, 235], field [255, 255, 255], surface [242, 242, 242], border [214, 214, 214], text [61, 61, 61], weak [118, 118, 118], accent [233, 84, 32],
        words [[196, 64, 10], [52, 101, 164], [143, 89, 2], [78, 154, 6], [117, 80, 123], [136, 138, 133], [6, 152, 154], [204, 0, 0], [85, 87, 83], [46, 52, 54]]),
    scheme!("one-dark", "One Dark", true, bg [40, 44, 52], back [33, 37, 43], field [35, 39, 46], surface [50, 56, 66], border [62, 68, 81], text [220, 223, 228], weak [127, 132, 142], accent [97, 175, 239],
        words [[198, 120, 221], [97, 175, 239], [229, 192, 123], [152, 195, 121], [209, 154, 102], [110, 118, 129], [86, 182, 194], [224, 108, 117], [171, 178, 191], [220, 223, 228]]),
    scheme!("darcula", "JetBrains Darcula", true, bg [43, 43, 43], back [49, 51, 53], field [40, 40, 40], surface [76, 80, 82], border [85, 85, 85], text [187, 187, 187], weak [128, 128, 128], accent [75, 110, 175],
        words [[204, 120, 50], [255, 198, 109], [152, 118, 170], [106, 135, 89], [104, 151, 187], [128, 128, 128], [152, 118, 170], [187, 181, 41], [169, 183, 198], [169, 183, 198]]),
    scheme!("monokai", "Monokai", true, bg [39, 40, 34], back [30, 31, 26], field [34, 35, 30], surface [62, 61, 50], border [73, 72, 62], text [248, 248, 242], weak [160, 156, 135], accent [166, 226, 46],
        words [[249, 38, 114], [166, 226, 46], [102, 217, 239], [230, 219, 116], [174, 129, 255], [117, 113, 94], [253, 151, 31], [253, 151, 31], [249, 38, 114], [248, 248, 242]]),
    scheme!("nord", "Nord", true, bg [46, 52, 64], back [39, 44, 54], field [41, 46, 57], surface [59, 66, 82], border [76, 86, 106], text [229, 233, 240], weak [140, 150, 170], accent [136, 192, 208],
        words [[129, 161, 193], [136, 192, 208], [143, 188, 187], [163, 190, 140], [180, 142, 173], [97, 110, 136], [235, 203, 139], [208, 135, 112], [129, 161, 193], [216, 222, 233]]),
    scheme!("solarized-dark", "Solarized Dark", true, bg [0, 43, 54], back [0, 34, 43], field [0, 38, 48], surface [7, 54, 66], border [20, 75, 88], text [147, 161, 161], weak [101, 123, 131], accent [38, 139, 210],
        words [[133, 153, 0], [38, 139, 210], [181, 137, 0], [42, 161, 152], [211, 54, 130], [88, 110, 117], [203, 75, 22], [220, 50, 47], [147, 161, 161], [147, 161, 161]]),
    scheme!("solarized-light", "Solarized Light", false, bg [253, 246, 227], back [238, 232, 213], field [255, 251, 238], surface [238, 232, 213], border [210, 203, 180], text [88, 110, 117], weak [147, 161, 161], accent [38, 139, 210],
        words [[133, 153, 0], [38, 139, 210], [181, 137, 0], [42, 161, 152], [211, 54, 130], [147, 161, 161], [203, 75, 22], [220, 50, 47], [101, 123, 131], [88, 110, 117]]),
    scheme!("github-dark", "GitHub Dark", true, bg [13, 17, 23], back [1, 4, 9], field [10, 13, 18], surface [33, 38, 45], border [48, 54, 61], text [230, 237, 243], weak [125, 133, 144], accent [47, 129, 247],
        words [[255, 123, 114], [210, 168, 255], [255, 166, 87], [165, 214, 255], [121, 192, 255], [139, 148, 158], [121, 192, 255], [255, 166, 87], [255, 123, 114], [230, 237, 243]]),
    scheme!("github-light", "GitHub Light", false, bg [255, 255, 255], back [246, 248, 250], field [255, 255, 255], surface [246, 248, 250], border [208, 215, 222], text [31, 35, 40], weak [101, 109, 118], accent [9, 105, 218],
        words [[207, 34, 46], [130, 80, 223], [149, 56, 0], [10, 48, 105], [5, 80, 174], [110, 119, 129], [5, 80, 174], [149, 56, 0], [207, 34, 46], [31, 35, 40]]),
    scheme!("gruvbox", "Gruvbox", true, bg [40, 40, 40], back [29, 32, 33], field [32, 32, 32], surface [60, 56, 54], border [80, 73, 69], text [235, 219, 178], weak [168, 153, 132], accent [250, 189, 47],
        words [[251, 73, 52], [142, 192, 124], [250, 189, 47], [184, 187, 38], [211, 134, 155], [146, 131, 116], [131, 165, 152], [254, 128, 25], [235, 219, 178], [235, 219, 178]]),
    scheme!("catppuccin-mocha", "Catppuccin Mocha", true, bg [30, 30, 46], back [24, 24, 37], field [27, 27, 41], surface [49, 50, 68], border [69, 71, 90], text [205, 214, 244], weak [166, 173, 200], accent [203, 166, 247],
        words [[203, 166, 247], [137, 180, 250], [249, 226, 175], [166, 227, 161], [250, 179, 135], [108, 112, 134], [148, 226, 213], [243, 139, 168], [137, 220, 235], [205, 214, 244]]),
    scheme!("catppuccin-latte", "Catppuccin Latte", false, bg [239, 241, 245], back [230, 233, 239], field [248, 249, 251], surface [220, 224, 232], border [188, 192, 204], text [76, 79, 105], weak [108, 111, 133], accent [136, 57, 239],
        words [[136, 57, 239], [30, 102, 245], [223, 142, 29], [64, 160, 43], [254, 100, 11], [156, 160, 176], [23, 146, 153], [210, 15, 57], [4, 165, 229], [76, 79, 105]]),
    scheme!("tokyo-night", "Tokyo Night", true, bg [26, 27, 38], back [22, 22, 30], field [22, 23, 33], surface [41, 46, 66], border [59, 66, 97], text [192, 202, 245], weak [130, 140, 180], accent [122, 162, 247],
        words [[187, 154, 247], [122, 162, 247], [42, 195, 222], [158, 206, 106], [255, 158, 100], [86, 95, 137], [115, 218, 202], [247, 118, 142], [137, 221, 255], [192, 202, 245]]),
];

pub fn find(id: &str) -> Option<&'static Scheme> {
    SCHEMES.iter().find(|s| s.id == id)
}

fn c(x: Rgb) -> Color32 {
    Color32::from_rgb(x[0], x[1], x[2])
}

pub fn mix(a: Color32, b: Color32, k: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// The window's colours from a scheme.
pub fn visuals(s: &Scheme) -> egui::Visuals {
    let mut v = if s.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let (bg, field, surface, border, text, weak, accent) = (c(s.bg), c(s.field), c(s.surface), c(s.border), c(s.text), c(s.weak), c(s.accent));
    let strong = if s.dark { mix(text, Color32::WHITE, 0.5) } else { mix(text, Color32::BLACK, 0.5) };
    v.panel_fill = bg;
    v.window_fill = mix(bg, surface, 0.2);
    v.extreme_bg_color = field;
    v.text_edit_bg_color = Some(field);
    v.faint_bg_color = mix(bg, text, 0.035);
    v.code_bg_color = surface;
    v.window_stroke = Stroke::new(1.0, border);
    v.hyperlink_color = accent;
    v.weak_text_color = Some(weak);
    v.selection.bg_fill = mix(bg, accent, if s.dark { 0.45 } else { 0.32 });
    v.selection.stroke = Stroke::new(1.0, strong);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = bg;
    w.noninteractive.weak_bg_fill = bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, text);
    w.inactive.bg_fill = surface;
    w.inactive.weak_bg_fill = surface;
    w.inactive.bg_stroke = Stroke::new(1.0, border);
    w.inactive.fg_stroke = Stroke::new(1.0, text);
    for (x, k) in [(&mut w.hovered, 0.10), (&mut w.open, 0.08)] {
        x.bg_fill = mix(surface, text, k);
        x.weak_bg_fill = mix(surface, text, k);
        x.bg_stroke = Stroke::new(1.0, mix(border, accent, 0.5));
        x.fg_stroke = Stroke::new(1.5, strong);
    }
    w.active.bg_fill = mix(surface, accent, 0.35);
    w.active.weak_bg_fill = mix(surface, accent, 0.35);
    w.active.bg_stroke = Stroke::new(1.0, accent);
    w.active.fg_stroke = Stroke::new(2.0, strong);
    v
}

/// Around the sheet, per scheme (dark, light): set when one is applied.
static BACK: Mutex<[Option<Color32>; 2]> = Mutex::new([None, None]);

pub fn set_back(dark: Option<Color32>, light: Option<Color32>) {
    *BACK.lock().unwrap() = [dark, light];
}

static GRADIENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The backdrop is a gradient this frame (the panels leave it bare).
pub fn set_gradient(on: bool) {
    GRADIENT.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn gradient() -> bool {
    GRADIENT.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn back(dark: bool) -> Option<Color32> {
    BACK.lock().unwrap()[if dark { 0 } else { 1 }]
}

/// The backdrop as a gradient of several colours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Backdrop {
    pub on: bool,
    /// From the centre out (else along `angle`).
    pub radial: bool,
    /// Degrees: 0 — left to right, 90 — top to bottom.
    pub angle: f32,
    /// How much of it over the scheme's own backdrop (0…1).
    pub strength: f32,
    pub stops: Vec<Stop>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub at: f32,
    pub color: Rgb,
}

impl Default for Backdrop {
    fn default() -> Self {
        PRESETS[0].1.clone_into_backdrop()
    }
}

/// Ready gradients: (name, stops).
pub struct Preset(pub &'static str, pub PresetStops);
pub struct PresetStops(pub &'static [(f32, Rgb)], pub f32);

impl PresetStops {
    fn clone_into_backdrop(&self) -> Backdrop {
        Backdrop { on: false, radial: false, angle: self.1, strength: 0.7, stops: self.0.iter().map(|&(at, color)| Stop { at, color }).collect() }
    }
}

pub const PRESETS: &[Preset] = &[
    Preset("Аметист · Amethyst", PresetStops(&[(0.0, [76, 40, 130]), (0.5, [40, 60, 140]), (1.0, [22, 110, 120])], 135.0)),
    Preset("Закат · Sunset", PresetStops(&[(0.0, [255, 94, 98]), (0.45, [255, 153, 102]), (1.0, [110, 60, 160])], 120.0)),
    Preset("Северное сияние · Aurora", PresetStops(&[(0.0, [10, 30, 60]), (0.35, [20, 140, 120]), (0.7, [60, 200, 140]), (1.0, [90, 60, 170])], 160.0)),
    Preset("Океан · Ocean", PresetStops(&[(0.0, [0, 60, 120]), (0.6, [0, 130, 170]), (1.0, [120, 220, 220])], 90.0)),
    Preset("Ubuntu", PresetStops(&[(0.0, [44, 0, 30]), (0.6, [119, 33, 111]), (1.0, [233, 84, 32])], 135.0)),
    Preset("Win 11 Bloom", PresetStops(&[(0.0, [20, 40, 110]), (0.5, [60, 110, 230]), (1.0, [150, 190, 255])], 45.0)),
    Preset("Лес · Forest", PresetStops(&[(0.0, [20, 50, 30]), (0.5, [60, 110, 50]), (1.0, [200, 190, 90])], 100.0)),
];

pub fn preset(i: usize) -> Backdrop {
    let mut b = PRESETS[i].1.clone_into_backdrop();
    b.on = true;
    b
}

impl Backdrop {
    /// The gradient's colour at `t` (0…1).
    pub fn at(&self, t: f32) -> Color32 {
        let mut stops: Vec<&Stop> = self.stops.iter().collect();
        stops.sort_by(|a, b| a.at.total_cmp(&b.at));
        match stops.as_slice() {
            [] => Color32::BLACK,
            [one] => c(one.color),
            _ => {
                let t = t.clamp(0.0, 1.0);
                if t <= stops[0].at {
                    return c(stops[0].color);
                }
                for w in stops.windows(2) {
                    if t <= w[1].at {
                        let span = (w[1].at - w[0].at).max(1e-4);
                        return mix(c(w[0].color), c(w[1].color), (t - w[0].at) / span);
                    }
                }
                c(stops.last().unwrap().color)
            }
        }
    }

    /// Painted over `rect`, mixed into `base` by its strength.
    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect, base: Color32) {
        const N: usize = 40;
        let mut mesh = egui::Mesh::default();
        let dir = egui::Vec2::angled(self.angle.to_radians());
        // The rect's extent along the direction.
        let corners = [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()];
        let proj: Vec<f32> = corners.iter().map(|p| p.to_vec2().dot(dir)).collect();
        let (lo, hi) = (proj.iter().cloned().fold(f32::MAX, f32::min), proj.iter().cloned().fold(f32::MIN, f32::max));
        let reach = (rect.size() / 2.0).length().max(1.0);
        for j in 0..=N {
            for i in 0..=N {
                let p = egui::pos2(rect.left() + rect.width() * i as f32 / N as f32, rect.top() + rect.height() * j as f32 / N as f32);
                let t = if self.radial { (p - rect.center()).length() / reach } else { (p.to_vec2().dot(dir) - lo) / (hi - lo).max(1.0) };
                mesh.colored_vertex(p, mix(base, self.at(t), self.strength.clamp(0.0, 1.0)));
            }
        }
        let n = (N + 1) as u32;
        for j in 0..N as u32 {
            for i in 0..N as u32 {
                let a = j * n + i;
                mesh.add_triangle(a, a + 1, a + n);
                mesh.add_triangle(a + 1, a + n + 1, a + n);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// A row of a scheme's colours (for the picker).
pub fn swatch(ui: &mut egui::Ui, s: &Scheme, chosen: bool) -> egui::Response {
    let size = egui::vec2(150.0, 62.0);
    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 8.0, c(s.back));
    let sheet = rect.shrink2(egui::vec2(8.0, 6.0)).translate(egui::vec2(0.0, 8.0));
    let sheet = egui::Rect::from_min_max(sheet.min, egui::pos2(sheet.max.x, rect.bottom() - 6.0));
    p.rect_filled(sheet, 5.0, c(s.bg));
    p.text(rect.left_top() + egui::vec2(8.0, 1.0), egui::Align2::LEFT_TOP, s.name, egui::FontId::proportional(11.0), c(s.text));
    // A few of the editor's colours as "code".
    let mut x = sheet.left() + 6.0;
    for (k, w) in [(0usize, 18.0), (9, 10.0), (1, 22.0), (3, 26.0)] {
        p.rect_filled(egui::Rect::from_min_size(egui::pos2(x, sheet.top() + 7.0), egui::vec2(w, 4.0)), 2.0, c(s.words[k]));
        x += w + 4.0;
    }
    let mut x = sheet.left() + 14.0;
    for (k, w) in [(2usize, 16.0), (4, 12.0), (5, 34.0)] {
        p.rect_filled(egui::Rect::from_min_size(egui::pos2(x, sheet.top() + 16.0), egui::vec2(w, 4.0)), 2.0, c(s.words[k]));
        x += w + 4.0;
    }
    p.rect_filled(egui::Rect::from_min_size(egui::pos2(sheet.right() - 26.0, sheet.bottom() - 12.0), egui::vec2(20.0, 7.0)), 3.0, c(s.accent));
    let edge = if chosen { Stroke::new(2.0, ui.visuals().selection.stroke.color) } else if r.hovered() { Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color) } else { Stroke::new(1.0, c(s.border)) };
    p.rect_stroke(rect, 8.0, edge, egui::StrokeKind::Inside);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_stops() {
        let b = Backdrop { on: true, radial: false, angle: 0.0, strength: 1.0, stops: vec![Stop { at: 1.0, color: [0, 0, 255] }, Stop { at: 0.0, color: [255, 0, 0] }, Stop { at: 0.5, color: [0, 255, 0] }] };
        assert_eq!(b.at(0.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(b.at(0.5), Color32::from_rgb(0, 255, 0));
        assert_eq!(b.at(0.75), Color32::from_rgb(0, 128, 128));
        assert_eq!(b.at(2.0), Color32::from_rgb(0, 0, 255));
        let ids: std::collections::HashSet<_> = SCHEMES.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), SCHEMES.len());
    }
}
