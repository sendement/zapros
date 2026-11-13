//! Search: Ctrl+F's window (find and replace in the script, the workspace's
//! scripts or all of them; or the database — the code of its views,
//! functions, procedures, packages, triggers, jobs — as cards), and the
//! object tree's search of the database by name.

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use egui::{Color32, RichText};

use crate::app::App;
use crate::db::{Found, ObjKind, Req};
use crate::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scope {
    /// The script on screen (all its cells, in a notebook).
    #[default]
    Tab,
    /// The workspace's scripts.
    Workspace,
    /// Every workspace's.
    All,
    /// The code of the database's objects.
    Database,
}

impl Scope {
    const ALL: [Scope; 4] = [Scope::Tab, Scope::Workspace, Scope::All, Scope::Database];

    fn label(self) -> &'static str {
        match self {
            Scope::Tab => t("В этом скрипте", "In this script"),
            Scope::Workspace => t("Во вкладках пространства", "In the workspace's tabs"),
            Scope::All => t("Во всех вкладках", "In every tab"),
            Scope::Database => t("В коде объектов базы", "In the database's code"),
        }
    }
}

#[derive(Default)]
pub struct Find {
    pub open: bool,
    /// The query field takes the keys next frame.
    pub focus: bool,
    pub query: String,
    pub replace: String,
    pub show_replace: bool,
    pub case: bool,
    pub word: bool,
    pub scope: Scope,
    /// The match it's on (in `hits`' order).
    pub cur: usize,
    /// The database's search: what was asked (connection, query), what
    /// came, and when to ask (typing paused).
    pub db_asked: Option<(String, String)>,
    pub db_hits: Option<Result<Vec<Found>, String>>,
    pub db_due: Option<Instant>,
}

/// The tree's search of the database by name.
#[derive(Default)]
pub struct TreeSearch {
    /// What was asked (typing paused), and when to ask.
    pub q: String,
    pub due: Option<Instant>,
    /// Per connection: None — asked, no answer yet.
    pub hits: HashMap<String, Option<Result<Vec<Found>, String>>>,
}

/// Where `q` is in `text` (bytes); any case unless `case`; whole words
/// only when `word`.
pub fn find_in(text: &str, q: &str, case: bool, word: bool) -> Vec<Range<usize>> {
    if q.is_empty() {
        return Vec::new();
    }
    let (hay, needle) = if case { (text.to_string(), q.to_string()) } else { (text.to_lowercase(), q.to_lowercase()) };
    // (Lower case of some letters is another length: then as written.)
    let (hay, needle) = if hay.len() == text.len() && needle.len() == q.len() { (hay, needle) } else { (text.to_string(), q.to_string()) };
    let is_w = |c: char| c.is_alphanumeric() || c == '_';
    hay.match_indices(&needle)
        .map(|(i, _)| i..i + needle.len())
        .filter(|r| !word || (!text[..r.start].chars().next_back().is_some_and(is_w) && !text[r.end..].chars().next().is_some_and(is_w)))
        .collect()
}

/// A match: in which workspace / tab (/ cell) it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub ws: usize,
    pub tab: usize,
    pub cell: Option<String>,
    pub range: Range<usize>,
}

/// The texts in the scope: (workspace, tab, cell, text).
fn texts(app: &App, scope: Scope) -> Vec<(usize, usize, Option<String>, String)> {
    let mut out = Vec::new();
    let aw = app.session.active;
    for (wi, ws) in app.session.workspaces.iter().enumerate() {
        if scope != Scope::All && wi != aw {
            continue;
        }
        for (ti, tab) in ws.tabs.iter().enumerate() {
            if scope == Scope::Tab && ti != ws.active {
                continue;
            }
            match tab.notebook.as_ref().filter(|n| n.on) {
                Some(nb) => {
                    for c in &nb.cells {
                        out.push((wi, ti, Some(c.id.clone()), c.text.clone()));
                    }
                }
                None => out.push((wi, ti, None, tab.text.clone())),
            }
        }
    }
    out
}

pub fn hits(app: &App) -> Vec<Hit> {
    let f = &app.find;
    texts(app, f.scope).into_iter().flat_map(|(ws, tab, cell, text)| find_in(&text, &f.query, f.case, f.word).into_iter().map(move |range| Hit { ws, tab, cell: cell.clone(), range })).collect()
}

/// The text a hit is in.
fn text_mut<'a>(app: &'a mut App, ws: usize, tab: usize, cell: &Option<String>) -> Option<&'a mut String> {
    let tb = app.session.workspaces.get_mut(ws)?.tabs.get_mut(tab)?;
    match cell {
        Some(c) => tb.notebook.as_mut()?.cells.iter_mut().find(|x| &x.id == c).map(|x| &mut x.text),
        None => Some(&mut tb.text),
    }
}

/// The match shown: its workspace and tab chosen, it picked in its text.
pub fn go(app: &mut App, h: &Hit) {
    app.session.active = h.ws;
    app.session.workspaces[h.ws].active = h.tab;
    let tab_id = app.session.workspaces[h.ws].tabs[h.tab].id.clone();
    let key = match &h.cell {
        Some(c) => {
            if let Some(cell) = app.session.workspaces[h.ws].tabs[h.tab].notebook.as_mut().and_then(|n| n.cells.iter_mut().find(|x| &x.id == c)) {
                cell.collapsed = false;
            }
            App::cell_key(&tab_id, c)
        }
        None => tab_id,
    };
    app.rt.entry(key).or_default().ed.select = Some((h.range.start, h.range.end));
    app.touch();
}

/// The matches lit in the editors on screen (the one it's on, more).
fn mark(app: &mut App, hs: &[Hit], cur: Option<&Hit>) {
    for rt in app.rt.values_mut() {
        rt.ed.marks.clear();
        rt.ed.mark_now = None;
    }
    let aw = app.session.active;
    let at = app.ws().active;
    let tab_id = app.tab().id.clone();
    for h in hs.iter().filter(|h| h.ws == aw && h.tab == at) {
        let key = match &h.cell {
            Some(c) => App::cell_key(&tab_id, c),
            None => tab_id.clone(),
        };
        let rt = app.rt.entry(key).or_default();
        rt.ed.marks.push(h.range.clone());
        if cur == Some(h) {
            rt.ed.mark_now = Some(h.range.clone());
        }
    }
}

/// Clears the lit matches (the bar closed).
pub fn unmark(app: &mut App) {
    for rt in app.rt.values_mut() {
        rt.ed.marks.clear();
        rt.ed.mark_now = None;
    }
}

/// Ctrl+F: the bar opens on the word picked (or as it was).
pub fn open(app: &mut App) {
    let id = app.tab().id.clone();
    let picked = app.rt.get(&id).and_then(|r| crate::editor::selection(&r.ed)).and_then(|r| app.tab().text.get(r).map(str::to_string)).filter(|s| !s.contains('\n') && !s.is_empty());
    if let Some(p) = picked {
        app.find.query = p;
    }
    app.find.open = true;
    app.find.focus = true;
}

/// The search window and what it found.
pub fn bar(app: &mut App, ui: &mut egui::Ui) {
    if !app.find.open {
        return;
    }
    let ctx = ui.ctx().clone();
    let db = app.find.scope == Scope::Database;
    let hs = if db { Vec::new() } else { hits(app) };
    if app.find.cur >= hs.len() {
        app.find.cur = 0;
    }
    let mut goto: Option<usize> = None;
    let mut close = false;
    let mut replace_one = false;
    let mut replace_all = false;
    let before = (app.find.query.clone(), app.find.case, app.find.word, app.find.scope);
    // A window of its own (moved where it's handy), near the editor's top right.
    let mut open = true;
    let at = ui.max_rect().right_top() + egui::vec2(-600.0, 4.0);
    egui::Window::new(t("Поиск", "Find")).id(egui::Id::new("find-window")).open(&mut open).collapsible(false).resizable(true).default_pos(at).default_width(580.0).show(&ctx, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let f = &mut app.find;
            if let Some(img) = crate::icons::image(crate::icons::Ic::Explain, 15.0, ui.visuals().weak_text_color()) {
                ui.add(img);
            }
            let q = ui.add(egui::TextEdit::singleline(&mut f.query).id(egui::Id::new("find-q")).hint_text(t("найти…", "find…")).desired_width(260.0));
            if std::mem::take(&mut f.focus) {
                q.request_focus();
            }
            // Enter: the next one (Shift: the one before); Esc: closed.
            if q.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let back = ui.input(|i| i.modifiers.shift);
                goto = Some(if hs.is_empty() { 0 } else if back { (f.cur + hs.len() - 1) % hs.len() } else { (f.cur + 1) % hs.len() });
                if db {
                    f.db_due = Some(Instant::now());
                }
                q.request_focus();
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) && (q.has_focus() || q.lost_focus()) {
                close = true;
            }
            ui.add(egui::Button::selectable(f.case, "Aa")).on_hover_text(t("С учётом регистра", "Match case")).clicked().then(|| f.case = !f.case);
            ui.add(egui::Button::selectable(f.word, "W")).on_hover_text(t("Слово целиком", "Whole word")).clicked().then(|| f.word = !f.word);
            egui::ComboBox::from_id_salt("find-scope").selected_text(f.scope.label()).width(210.0).show_ui(ui, |ui| {
                for s in Scope::ALL {
                    ui.selectable_value(&mut f.scope, s, s.label());
                }
            });
            if db {
                let n = match &f.db_hits {
                    Some(Ok(v)) => format!("{}: {}", t("Найдено", "Found"), v.len()),
                    Some(Err(_)) => t("ошибка", "failed").into(),
                    None if f.db_asked.is_some() => t("ищу…", "searching…").into(),
                    None => String::new(),
                };
                if f.db_asked.is_some() && f.db_hits.is_none() {
                    ui.add(egui::Spinner::new().size(13.0));
                }
                ui.label(RichText::new(n).weak());
            } else {
                let n = if hs.is_empty() { if f.query.is_empty() { String::new() } else { t("нет совпадений", "no matches").into() } } else { format!("{} {} {}", f.cur + 1, t("из", "of"), hs.len()) };
                ui.label(RichText::new(n).weak());
                if ui.add_enabled(!hs.is_empty(), egui::Button::new("↑")).on_hover_text("Shift+Enter").clicked() {
                    goto = Some((f.cur + hs.len() - 1) % hs.len().max(1));
                }
                if ui.add_enabled(!hs.is_empty(), egui::Button::new("↓")).on_hover_text("Enter").clicked() {
                    goto = Some((f.cur + 1) % hs.len().max(1));
                }
                ui.add(egui::Button::selectable(f.show_replace, "⇄")).on_hover_text(t("Замена", "Replace")).clicked().then(|| f.show_replace = !f.show_replace);
            }
        });
        if app.find.show_replace && !db {
            ui.horizontal(|ui| {
                ui.add_space(21.0);
                ui.add(egui::TextEdit::singleline(&mut app.find.replace).hint_text(t("заменить на…", "replace with…")).desired_width(260.0));
                replace_one = ui.add_enabled(!hs.is_empty(), egui::Button::new(t("Заменить", "Replace"))).clicked();
                replace_all = ui.add_enabled(!hs.is_empty(), egui::Button::new(format!("{} ({})", t("Заменить все", "Replace all"), hs.len()))).clicked();
            });
        }
        // Several scripts: the matches listed (a click: there).
        if !db && app.find.scope != Scope::Tab && !hs.is_empty() {
            egui::ScrollArea::vertical().id_salt("find-list").max_height(150.0).auto_shrink([false, true]).show(ui, |ui| {
                let texts = texts(app, app.find.scope);
                for (k, h) in hs.iter().enumerate().take(500) {
                    let Some((_, _, _, text)) = texts.iter().find(|(w, tb, c, _)| *w == h.ws && *tb == h.tab && *c == h.cell) else { continue };
                    let ws = &app.session.workspaces[h.ws];
                    let line_no = text[..h.range.start].matches('\n').count() + 1;
                    let ls = text[..h.range.start].rfind('\n').map_or(0, |i| i + 1);
                    let le = text[h.range.end..].find('\n').map_or(text.len(), |i| h.range.end + i);
                    let line = text[ls..le].trim();
                    let place = format!("{} · {}{} · {}", ws.name, ws.tabs[h.tab].title, h.cell.as_ref().map_or(String::new(), |_| format!(" · {}", t("блок", "cell"))), line_no);
                    let r = ui.add(egui::Button::selectable(k == app.find.cur, RichText::new(format!("{place}:  {}", line.chars().take(140).collect::<String>())).monospace().small()).frame_when_inactive(k == app.find.cur));
                    if r.clicked() {
                        goto = Some(k);
                    }
                }
            });
        }
        if db {
            cards(app, ui);
        }
    });
    if !open {
        close = true;
    }
    if (app.find.query.clone(), app.find.case, app.find.word, app.find.scope) != before {
        app.find.cur = 0;
        if app.find.scope == Scope::Database {
            app.find.db_due = Some(Instant::now() + Duration::from_millis(450));
        }
    }
    if close {
        app.find.open = false;
        unmark(app);
        return;
    }
    if let Some(k) = goto.filter(|_| !hs.is_empty()) {
        app.find.cur = k;
        go(app, &hs[k]);
    }
    if replace_one {
        if let Some(h) = hs.get(app.find.cur).cloned() {
            let with = app.find.replace.clone();
            if let Some(text) = text_mut(app, h.ws, h.tab, &h.cell) {
                if h.range.end <= text.len() {
                    text.replace_range(h.range.clone(), &with);
                }
            }
            app.session.workspaces[h.ws].tabs[h.tab].modified = app.session.workspaces[h.ws].tabs[h.tab].path.is_some();
            app.touch();
        }
    }
    if replace_all {
        let with = app.find.replace.clone();
        // From the end of each text: the earlier ranges stay right.
        let mut all = hs.clone();
        all.sort_by(|a, b| (a.ws, a.tab, &a.cell).cmp(&(b.ws, b.tab, &b.cell)).then(b.range.start.cmp(&a.range.start)));
        for h in &all {
            if let Some(text) = text_mut(app, h.ws, h.tab, &h.cell) {
                if h.range.end <= text.len() {
                    text.replace_range(h.range.clone(), &with);
                }
            }
            let tb = &mut app.session.workspaces[h.ws].tabs[h.tab];
            tb.modified = tb.path.is_some();
        }
        app.toast(format!("{}: {}", t("Заменено", "Replaced"), all.len()));
        app.touch();
    }
    if db {
        unmark(app);
        ask_db(app, &ctx);
    } else {
        let hs = hits(app);
        let cur = hs.get(app.find.cur).cloned();
        mark(app, &hs, cur.as_ref());
    }
}

/// The database asked once typing has paused.
fn ask_db(app: &mut App, ctx: &egui::Context) {
    let Some(due) = app.find.db_due else { return };
    if Instant::now() < due {
        ctx.request_repaint_after(due - Instant::now());
        return;
    }
    app.find.db_due = None;
    let q = app.find.query.trim().to_string();
    if q.chars().count() < 2 {
        app.find.db_hits = None;
        app.find.db_asked = None;
        return;
    }
    let ws = app.ws().clone();
    let Some(conn) = app.tab_conn(&ws, app.tab()) else {
        app.find.db_hits = Some(Err(t("У вкладки нет подключения", "The tab has no connection").into()));
        return;
    };
    if app.ensure_live(ctx, &conn, None) {
        if let Some(l) = app.live.get(&conn) {
            l.meta.send(Req::Search { q: q.clone(), code: true });
            app.find.db_asked = Some((conn, q));
            app.find.db_hits = None;
        }
    }
}

/// `text` with `q` lit (any case).
fn lit(ui: &egui::Ui, text: &str, q: &str, font: egui::FontId) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let base = egui::TextFormat::simple(font.clone(), ui.visuals().text_color());
    let mut hot = egui::TextFormat::simple(font, ui.visuals().strong_text_color());
    hot.background = Color32::from_rgb(250, 200, 60).gamma_multiply(0.45);
    let mut at = 0;
    for r in find_in(text, q, false, false) {
        job.append(&text[at..r.start], 0.0, base.clone());
        job.append(&text[r.clone()], 0.0, hot.clone());
        at = r.end;
    }
    job.append(&text[at..], 0.0, base);
    job
}

/// What the database search found: a card each (its lines with the
/// match); a double click opens it in a tab beside.
fn cards(app: &mut App, ui: &mut egui::Ui) {
    let Some((conn, q)) = app.find.db_asked.clone() else { return };
    let hits = match &app.find.db_hits {
        Some(Ok(v)) => v.clone(),
        Some(Err(e)) => {
            ui.colored_label(crate::ui::RED, e.as_str());
            return;
        }
        None => return,
    };
    if hits.is_empty() {
        ui.label(RichText::new(t("Ничего не нашлось", "Nothing found")).weak());
        return;
    }
    let mut open: Option<Found> = None;
    let h = (ui.available_height() * 0.5).clamp(140.0, 420.0);
    egui::ScrollArea::vertical().id_salt("find-cards").max_height(h).auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            let w = ((ui.available_width() - 16.0) / 3.0).clamp(260.0, 420.0);
            for f in &hits {
                let fr = egui::Frame::group(ui.style()).corner_radius(8).inner_margin(8).show(ui, |ui| ui.vertical(|ui| {
                    ui.set_width(w);
                    ui.horizontal(|ui| {
                        if let Some(img) = crate::icons::image(crate::icons::Ic::of(f.kind), 15.0, ui.visuals().text_color()) {
                            ui.add(img);
                        }
                        let name = if f.schema.is_empty() { f.name.clone() } else { format!("{}.{}", f.schema, f.name) };
                        ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                    });
                    let mono = egui::FontId::monospace(11.5);
                    for (n, l) in f.lines.iter().take(3) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{n:>4}")).monospace().small().weak());
                            // (A stretch round the match: as much as the card holds.)
                            let fit = ((ui.available_width() - 8.0) / ui.fonts_mut(|f| f.glyph_width(&mono, '0'))).max(12.0) as usize;
                            let l = crate::db::around_n(l.trim(), &q, fit);
                            ui.add(egui::Label::new(lit(ui, &l, &q, mono.clone())).truncate());
                        });
                    }
                }));
                let r = ui.interact(fr.response.rect, ui.id().with(("card", &f.path)), egui::Sense::click()).on_hover_text(t("Двойной щелчок — открыть во вкладке рядом", "Double-click: open in a tab beside"));
                if r.hovered() {
                    ui.painter().rect_stroke(fr.response.rect, 8.0, egui::Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color), egui::StrokeKind::Inside);
                }
                if r.double_clicked() {
                    open = Some(f.clone());
                }
            }
        });
    });
    if let Some(f) = open {
        let title = f.name.split('(').next().unwrap_or(&f.name).to_string();
        match f.kind {
            ObjKind::Table | ObjKind::Job => app.open_object(&conn, f.path.clone(), f.kind, title),
            k => app.open_source_next(&conn, f.path.clone(), k, title),
        }
        // Once its code's in: the first match picked.
        let id = app.tab().id.clone();
        app.rt.entry(id).or_default().find_after_load = Some(q);
    }
}

/// The tree's search: asked of the live connections once typing pauses.
pub fn tree_tick(app: &mut App, ctx: &egui::Context) {
    let q = app.tree_filter.trim().to_string();
    if q.chars().count() < 2 {
        app.tree_search = TreeSearch::default();
        return;
    }
    if q != app.tree_search.q {
        if app.tree_search.due.is_none() {
            app.tree_search.due = Some(Instant::now() + Duration::from_millis(350));
        }
        let due = app.tree_search.due.unwrap();
        if Instant::now() < due {
            ctx.request_repaint_after(due - Instant::now());
            return;
        }
        app.tree_search = TreeSearch { q: q.clone(), due: None, hits: HashMap::new() };
        for (id, l) in &app.live {
            if l.state == crate::app::ConnState::Ready {
                l.meta.send(Req::Search { q: q.clone(), code: false });
                app.tree_search.hits.insert(id.clone(), None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_any_case_and_whole_words() {
        let s = "select Total, subtotal from t where total > 1";
        assert_eq!(find_in(s, "total", false, false).len(), 3);
        assert_eq!(find_in(s, "total", true, false).len(), 2);
        assert_eq!(find_in(s, "total", false, true).len(), 2);
        assert_eq!(find_in("Заказы и заказчики", "заказ", false, false).len(), 2);
        assert!(find_in(s, "", false, false).is_empty());
    }
}
