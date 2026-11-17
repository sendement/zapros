//! "Table from a file…": a file picked (or dropped on the window), its
//! columns with guessed types to change, the table's name, its key and
//! indexes; then made and filled, its progress shown.

use std::path::{Path, PathBuf};

use crossbeam_channel::Receiver;
use egui::RichText;

use crate::app::App;
use crate::db::{Kind, Req};
use crate::design::IndexDef;
use crate::i18n::t;
use crate::import::{self, Col, Data, ReadOpts};
use crate::sql::complete::quote;

/// The key its worker events come under.
pub const KEY: &str = "import";

pub enum Stage {
    Shape,
    Running { done: usize, total: usize },
    Done(Result<usize, String>),
}

/// An index being set up: its name, its columns (by their names, in the
/// order picked), unique.
#[derive(Clone, Debug, Default)]
pub struct Ix {
    pub name: String,
    pub cols: Vec<String>,
    pub unique: bool,
}

pub struct ImportDlg {
    pub conn: String,
    pub kind: Kind,
    pub schema: String,
    /// The tree's folder of tables (refreshed after).
    pub folder: Vec<String>,
    /// The folder shown to pick from; the path as typed.
    pub dir: PathBuf,
    pub path_text: String,
    pub file: Option<PathBuf>,
    pub opts: ReadOpts,
    pub data: Option<Result<Data, String>>,
    loading: Option<Receiver<Result<Data, String>>>,
    pub cols: Vec<Col>,
    pub table: String,
    pub indexes: Vec<Ix>,
    pub stage: Stage,
    /// The picker's open (no file yet, or asked).
    pub browsing: bool,
}

impl ImportDlg {
    pub fn new(conn: &str, kind: Kind, schema: &str, folder: Vec<String>, start: PathBuf) -> Self {
        ImportDlg {
            conn: conn.into(),
            kind,
            schema: schema.into(),
            folder,
            path_text: String::new(),
            dir: start,
            file: None,
            opts: ReadOpts::default(),
            data: None,
            loading: None,
            cols: Vec::new(),
            table: String::new(),
            indexes: Vec::new(),
            stage: Stage::Shape,
            browsing: true,
        }
    }

    /// Reads the file (on a thread: a big one takes a while).
    pub fn load(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.file.as_ref() != Some(&path) {
            self.opts = ReadOpts::default();
            self.table = import::table_name(self.kind, &path);
            self.indexes.clear();
        }
        self.path_text = path.display().to_string();
        if let Some(p) = path.parent() {
            self.dir = p.to_path_buf();
        }
        self.file = Some(path.clone());
        self.browsing = false;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let opts = self.opts.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            tx.send(import::read(&path, &opts)).ok();
            ctx.request_repaint();
        });
        self.loading = Some(rx);
    }

    /// Waits for what's being read (tests).
    #[cfg(test)]
    pub fn wait(&mut self) {
        if let Some(rx) = self.loading.take() {
            let r = rx.recv().unwrap_or_else(|_| Err("gone".into()));
            self.loaded(r);
        }
    }

    fn poll(&mut self) {
        if let Some(r) = self.loading.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.loading = None;
            self.loaded(r);
        }
    }

    fn loaded(&mut self, r: Result<Data, String>) {
        if let Ok(d) = &r {
            // (Read again with other options: the choices made stay where
            // the columns are the same.)
            let fresh = import::columns(self.kind, d);
            let same = fresh.len() == self.cols.len();
            if !same {
                self.cols = fresh;
            } else {
                for (c, f) in self.cols.iter_mut().zip(fresh) {
                    if c.guess != f.guess {
                        c.ty = f.ty;
                        c.guess = f.guess;
                    }
                    c.nullable = f.nullable;
                }
            }
        }
        self.data = Some(r);
    }

    pub fn index_defs(&self) -> Vec<IndexDef> {
        let d = self.kind.dialect();
        self.indexes
            .iter()
            .filter(|x| !x.cols.is_empty())
            .map(|x| IndexDef { name: x.name.clone(), cols: x.cols.iter().map(|c| quote(c, d)).collect::<Vec<_>>().join(", "), unique: x.unique && self.kind != Kind::ClickHouse, ..Default::default() })
            .collect()
    }

    pub fn ddl(&self) -> Vec<String> {
        import::ddl(self.kind, &self.schema, &self.table, &self.cols, &self.index_defs())
    }

    fn rows(&self) -> &[Vec<Option<String>>] {
        match &self.data {
            Some(Ok(d)) => &d.rows,
            _ => &[],
        }
    }

    /// What stops it being made (None: ready).
    pub fn problem(&self) -> Option<String> {
        if self.table.trim().is_empty() {
            return Some(t("Нужно имя таблицы", "The table needs a name").into());
        }
        let on: Vec<&Col> = self.cols.iter().filter(|c| c.on).collect();
        if on.is_empty() {
            return Some(t("Ни одной колонки не выбрано", "No column picked").into());
        }
        if let Some(c) = on.iter().find(|c| c.name.trim().is_empty() || c.ty.trim().is_empty()) {
            return Some(format!("{} {}: {}", t("Колонка", "Column"), c.from + 1, t("нужны имя и тип", "needs a name and a type")));
        }
        for (k, c) in on.iter().enumerate() {
            if on[..k].iter().any(|o| o.name.eq_ignore_ascii_case(&c.name)) {
                return Some(format!("{}: {}", t("Имя дважды", "A name twice"), c.name));
            }
        }
        // A key over empty values won't go in.
        if let Some(c) = on.iter().find(|c| c.pk && self.rows().iter().any(|r| r.get(c.from).is_none_or(Option::is_none))) {
            return Some(format!("{} «{}» {}", t("В ключевой колонке", "The key column"), c.name, t("есть пустые значения", "has empty values")));
        }
        if let Some(x) = self.indexes.iter().find(|x| !x.cols.is_empty() && x.name.trim().is_empty()) {
            return Some(format!("{} ({})", t("Индексу нужно имя", "An index needs a name"), x.cols.join(", ")));
        }
        None
    }

    /// Off it goes to the connection's worker.
    pub fn start(&mut self, app: &App) {
        let Some(l) = app.live.get(&self.conn) else { return };
        let make = self.ddl();
        let rows = import::inserts(self.kind, &self.schema, &self.table, &self.cols, self.rows(), import::batch(self.kind));
        let t = crate::design::TableDef { schema: self.schema.clone(), name: self.table.clone(), ..Default::default() };
        let undo = format!("DROP TABLE {}", crate::design::table_name(self.kind, &t));
        self.stage = Stage::Running { done: 0, total: rows.len() };
        l.meta.send(Req::Import { key: KEY.into(), make, rows, undo });
    }
}

fn picker(ui: &mut egui::Ui, d: &mut ImportDlg, ctx: &egui::Context) {
    // Up, and where it is.
    ui.horizontal(|ui| {
        if ui.add_enabled(d.dir.parent().is_some(), egui::Button::new("⬆")).on_hover_text(t("Папка выше", "Up a folder")).clicked() {
            if let Some(p) = d.dir.parent() {
                d.dir = p.to_path_buf();
            }
        }
        for (label, p) in [(t("Домой", "Home"), dirs::home_dir()), (t("Загрузки", "Downloads"), dirs::download_dir()), (t("Документы", "Documents"), dirs::document_dir())] {
            if let Some(p) = p {
                if ui.small_button(label).clicked() {
                    d.dir = p;
                }
            }
        }
        ui.label(RichText::new(d.dir.display().to_string()).small().weak());
    });
    let mut entries: Vec<(bool, String, PathBuf)> = std::fs::read_dir(&d.dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        return None;
                    }
                    let p = e.path();
                    let dir = p.is_dir();
                    (dir || import::Format::of(&p).is_some()).then_some((dir, name, p))
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    egui::Frame::NONE.stroke(ui.visuals().widgets.noninteractive.bg_stroke).corner_radius(4).inner_margin(4).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("import-files").max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
            if entries.is_empty() {
                ui.label(RichText::new(t("Здесь нет подходящих файлов (CSV, TSV, XLSX, XLS, ODS, JSON)", "No files it reads here (CSV, TSV, XLSX, XLS, ODS, JSON)")).weak());
            }
            for (dir, name, p) in entries {
                let icon = if dir { "📁" } else { "📄" };
                let r = ui.selectable_label(d.file.as_ref() == Some(&p), format!("{icon} {name}"));
                if r.clicked() || r.double_clicked() {
                    if dir {
                        d.dir = p;
                    } else {
                        d.load(p, ctx);
                    }
                }
            }
        });
    });
}

fn columns_ui(ui: &mut egui::Ui, d: &mut ImportDlg) {
    let kind = d.kind;
    let samples: Vec<String> = d
        .cols
        .iter()
        .map(|c| {
            let vals: Vec<&str> = d.rows().iter().filter_map(|r| r.get(c.from).and_then(|v| v.as_deref())).take(3).collect();
            vals.join(" · ")
        })
        .collect();
    let names: Vec<String> = match &d.data {
        Some(Ok(x)) => x.names.clone(),
        _ => Vec::new(),
    };
    egui::ScrollArea::vertical().id_salt("import-cols").max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("import-cols-grid").striped(true).num_columns(7).spacing([10.0, 4.0]).show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new(t("В файле", "In the file")).strong());
            ui.label(RichText::new(t("Колонка", "Column")).strong());
            ui.label(RichText::new(t("Тип", "Type")).strong());
            ui.label(RichText::new("NULL").strong()).on_hover_text(t("Могут быть пустые значения", "Empty values allowed"));
            ui.label(RichText::new(if kind == Kind::ClickHouse { t("Сортировка", "Order by") } else { t("Ключ", "Key") }).strong())
                .on_hover_text(if kind == Kind::ClickHouse { t("ORDER BY таблицы (MergeTree)", "The table's ORDER BY (MergeTree)") } else { t("Первичный ключ", "Primary key") });
            ui.label(RichText::new(t("Значения", "Values")).strong());
            ui.end_row();
            for (k, c) in d.cols.iter_mut().enumerate() {
                ui.checkbox(&mut c.on, "").on_hover_text(t("Брать эту колонку", "Take this column"));
                ui.label(RichText::new(names.get(c.from).map(String::as_str).unwrap_or("")).weak());
                ui.add_enabled_ui(c.on, |ui| ui.add_sized([170.0, 20.0], egui::TextEdit::singleline(&mut c.name)));
                ui.add_enabled_ui(c.on, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.add(egui::TextEdit::singleline(&mut c.ty).desired_width(150.0).font(egui::TextStyle::Monospace));
                        egui::containers::menu::MenuButton::new("▾").ui(ui, |ui| {
                            let guessed = import::sql_type(kind, c.guess);
                            for ty in std::iter::once(guessed.as_str()).chain(import::type_choices(kind).iter().copied().filter(|x| *x != guessed)) {
                                if ui.selectable_label(c.ty == ty, ty).clicked() {
                                    c.ty = ty.to_string();
                                    ui.close();
                                }
                            }
                        });
                    });
                });
                ui.add_enabled(c.on && !c.pk, egui::Checkbox::without_text(&mut c.nullable));
                ui.add_enabled(c.on, egui::Checkbox::without_text(&mut c.pk));
                ui.add(egui::Label::new(RichText::new(samples.get(k).map(String::as_str).unwrap_or("")).small().weak()).truncate());
                ui.end_row();
            }
        });
    });
}

fn indexes_ui(ui: &mut egui::Ui, d: &mut ImportDlg) {
    let on: Vec<String> = d.cols.iter().filter(|c| c.on).map(|c| c.name.clone()).collect();
    let ch = d.kind == Kind::ClickHouse;
    let mut drop = None;
    for (k, x) in d.indexes.iter_mut().enumerate() {
        x.cols.retain(|c| on.contains(c));
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut x.name).desired_width(160.0).hint_text(t("имя индекса", "index name")));
            if !ch {
                ui.checkbox(&mut x.unique, t("уникальный", "unique"));
            }
            ui.label(RichText::new(t("по полям:", "on:")).weak());
            for c in &on {
                let at = x.cols.iter().position(|y| y == c);
                let label = match at {
                    Some(i) => format!("{} {c}", i + 1),
                    None => c.clone(),
                };
                if ui.selectable_label(at.is_some(), label).on_hover_text(t("Щелчок — добавить или убрать; порядок — как выбраны", "Click: add or take away; in the order picked")).clicked() {
                    match at {
                        Some(i) => {
                            x.cols.remove(i);
                        }
                        None => x.cols.push(c.clone()),
                    }
                }
            }
            if ui.small_button("✖").on_hover_text(t("Убрать индекс", "Remove the index")).clicked() {
                drop = Some(k);
            }
        });
    }
    if let Some(k) = drop {
        d.indexes.remove(k);
    }
    if ui.button(format!("+ {}", t("Индекс", "Index"))).on_hover_text(if ch { t("В ClickHouse — индекс пропуска данных (minmax)", "In ClickHouse: a data-skipping index (minmax)") } else { t("Индекс по выбранным полям", "An index on the fields picked") }).clicked() {
        let n = d.indexes.len() + 1;
        let base = d.table.trim().trim_matches('"');
        let name = format!("{base}_ix{n}");
        d.indexes.push(Ix { name: if d.kind == Kind::Oracle { name.to_uppercase() } else { name }, ..Default::default() });
    }
}

fn preview(ui: &mut egui::Ui, d: &ImportDlg) {
    let on: Vec<&Col> = d.cols.iter().filter(|c| c.on).collect();
    egui::ScrollArea::both().id_salt("import-preview").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("import-preview-grid").striped(true).spacing([14.0, 2.0]).show(ui, |ui| {
            for c in &on {
                ui.label(RichText::new(&c.name).strong().small());
            }
            ui.end_row();
            for r in d.rows().iter().take(30) {
                for c in &on {
                    match r.get(c.from).and_then(|v| v.as_deref()) {
                        Some(v) => ui.label(RichText::new(crate::app::head(v, 40)).small().monospace()),
                        None => ui.label(RichText::new("NULL").small().italics().weak()),
                    };
                }
                ui.end_row();
            }
        });
    });
}

/// The window, while there's one.
pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(mut d) = app.import.take() else { return };
    d.poll();
    // A file dropped on the window.
    if let Some(p) = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty())) {
        if matches!(d.stage, Stage::Shape) {
            d.load(p, ctx);
        }
    }
    let mut open = true;
    let mut close = false;
    let mut start = false;
    let mut open_table = false;
    let title = format!("{} — {}", t("Таблица из файла", "Table from a file"), if d.schema.is_empty() { app.spec(&d.conn).map(|s| s.title()).unwrap_or_default() } else { d.schema.clone() });
    egui::Window::new(title)
        .id(egui::Id::new("import-window"))
        .open(&mut open)
        .resizable(true)
        .collapsible(false)
        .default_size([920.0, 640.0])
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let running = matches!(d.stage, Stage::Running { .. });
            ui.add_enabled_ui(!running, |ui| {
                // The file.
                ui.horizontal(|ui| {
                    ui.label(t("Файл", "File"));
                    let r = ui.add(egui::TextEdit::singleline(&mut d.path_text).desired_width((ui.available_width() - 110.0).max(200.0)).hint_text(t("путь к CSV, XLSX, ODS, JSON… или перетащите файл сюда", "a CSV, XLSX, ODS, JSON… path, or drop a file here")));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let p = crate::config::expand(d.path_text.trim());
                        if p.is_dir() {
                            d.dir = p;
                            d.browsing = true;
                        } else {
                            d.load(p, ctx);
                        }
                    }
                    if ui.selectable_label(d.browsing, format!("📂 {}", t("Обзор", "Browse"))).clicked() {
                        d.browsing = !d.browsing;
                    }
                });
                if d.browsing {
                    picker(ui, &mut d, ctx);
                }
                if d.loading.is_some() {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label(t("Читается…", "Reading…"));
                    });
                    return;
                }
                let err = match &d.data {
                    None => return,
                    Some(Err(e)) => Some(e.clone()),
                    Some(Ok(_)) => None,
                };
                if let Some(e) = err {
                    ui.colored_label(crate::ui::RED, e);
                    return;
                }
                let (sheets, sep, enc, n) = match &d.data {
                    Some(Ok(x)) => (x.sheets.clone(), x.sep, x.encoding, x.rows.len()),
                    _ => return,
                };
                // How it's read.
                let mut reread = false;
                ui.horizontal_wrapped(|ui| {
                    let csv = d.file.as_deref().and_then(import::Format::of) == Some(import::Format::Csv);
                    if !sheets.is_empty() {
                        ui.label(t("Лист", "Sheet"));
                        let cur = d.opts.sheet.clone().unwrap_or_else(|| sheets[0].clone());
                        egui::ComboBox::from_id_salt("import-sheet").selected_text(&cur).show_ui(ui, |ui| {
                            for s in &sheets {
                                if ui.selectable_label(*s == cur, s).clicked() {
                                    d.opts.sheet = Some(s.clone());
                                    reread = true;
                                }
                            }
                        });
                    }
                    if csv {
                        ui.label(t("Разделитель", "Separator"));
                        let name = |c: char| match c {
                            '\t' => "Tab".to_string(),
                            c => format!("«{c}»"),
                        };
                        let shown = match d.opts.sep {
                            None => format!("{} ({})", t("сам", "auto"), name(sep)),
                            Some(c) => name(c),
                        };
                        egui::ComboBox::from_id_salt("import-sep").selected_text(shown).show_ui(ui, |ui| {
                            if ui.selectable_label(d.opts.sep.is_none(), t("Определить", "Detect")).clicked() {
                                d.opts.sep = None;
                                reread = true;
                            }
                            for c in [',', ';', '\t', '|'] {
                                if ui.selectable_label(d.opts.sep == Some(c), name(c)).clicked() {
                                    d.opts.sep = Some(c);
                                    reread = true;
                                }
                            }
                        });
                    }
                    if d.file.as_deref().and_then(import::Format::of) != Some(import::Format::Json) && ui.checkbox(&mut d.opts.header, t("Первая строка — названия", "First row: names")).changed() {
                        reread = true;
                    }
                    let mut facts = format!("{n} {}", t("строк", "rows"));
                    if !enc.is_empty() {
                        facts += &format!(" · {enc}");
                    }
                    ui.label(RichText::new(facts).weak());
                });
                if reread {
                    if let Some(p) = d.file.clone() {
                        d.load(p, ctx);
                    }
                    return;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(t("Таблица", "Table"));
                    if !d.schema.is_empty() {
                        ui.label(RichText::new(format!("{}.", d.schema)).weak());
                    }
                    ui.add(egui::TextEdit::singleline(&mut d.table).desired_width(260.0));
                });
                columns_ui(ui, &mut d);
                ui.separator();
                ui.label(RichText::new(t("Индексы", "Indexes")).strong());
                indexes_ui(ui, &mut d);
                ui.separator();
                egui::CollapsingHeader::new(t("Первые строки", "First rows")).id_salt("import-rows").default_open(true).show(ui, |ui| preview(ui, &d));
                egui::CollapsingHeader::new("SQL").id_salt("import-sql").show(ui, |ui| {
                    let mut s = crate::design::script(d.kind, &d.ddl());
                    if let Some(first) = import::inserts(d.kind, &d.schema, &d.table, &d.cols, &d.rows()[..d.rows().len().min(3)], 3).first() {
                        s += &format!("{first};\n-- …");
                    }
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut s.as_str()).code_editor().desired_width(f32::INFINITY));
                    });
                });
            });
            ui.separator();
            // How it goes, and the buttons.
            ui.horizontal(|ui| match &d.stage {
                Stage::Shape => {
                    let problem = if d.data.as_ref().is_some_and(|x| x.is_ok()) { d.problem() } else { Some(String::new()) };
                    let n = d.rows().len();
                    let r = ui.add_enabled(problem.is_none() && app.live.contains_key(&d.conn), crate::ui::tinted(format!("✔ {} ({n} {})", t("Создать и загрузить", "Make and fill"), t("строк", "rows")), crate::ui::GREEN));
                    if r.clicked() {
                        start = true;
                    }
                    if ui.button(t("Отмена", "Cancel")).clicked() {
                        close = true;
                    }
                    if let Some(p) = problem.filter(|p| !p.is_empty()) {
                        ui.colored_label(crate::notebook_ui::YELLOW, p);
                    }
                }
                Stage::Running { done, total } => {
                    let f = if *total == 0 { 0.0 } else { *done as f32 / *total as f32 };
                    ui.add(egui::ProgressBar::new(f).desired_width(320.0).show_percentage());
                    if ui.add(crate::ui::tinted(format!("■ {}", t("Стоп", "Stop")), crate::ui::RED)).on_hover_text(t("Остановить: таблица будет удалена", "Stop: the table is dropped")).clicked() {
                        if let Some(l) = app.live.get(&d.conn) {
                            l.meta.cancel();
                        }
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                Stage::Done(Ok(n)) => {
                    ui.colored_label(crate::ui::GREEN, format!("{}: {n}", t("Готово — таблица создана, строк", "Done: the table is made, rows")));
                    if ui.button(t("Открыть таблицу", "Open the table")).clicked() {
                        open_table = true;
                    }
                    if ui.button(t("Закрыть", "Close")).clicked() {
                        close = true;
                    }
                }
                Stage::Done(Err(e)) => {
                    ui.colored_label(crate::ui::RED, if e.is_empty() { t("Остановлено, таблица удалена", "Stopped; the table is dropped").to_string() } else { format!("{}: {e}", t("Не вышло, таблица удалена", "Failed; the table is dropped")) });
                    if ui.button(t("Ещё раз", "Again")).clicked() {
                        d.stage = Stage::Shape;
                    }
                }
            });
        });
    if start {
        d.start(app);
    }
    if open_table {
        let tbl = crate::design::TableDef { schema: d.schema.clone(), name: d.table.clone(), ..Default::default() };
        let q = crate::design::table_name(d.kind, &tbl);
        let sql = match d.kind {
            Kind::Oracle => format!("SELECT *\n  FROM {q}\n FETCH FIRST 200 ROWS ONLY"),
            _ => format!("SELECT *\n  FROM {q}\n LIMIT 200"),
        };
        app.new_tab(Some(d.table.clone()), sql, Some(d.conn.clone()));
        app.run(ctx, true, false);
        close = true;
    }
    if open && !close {
        app.import = Some(d);
    }
}

/// The worker's word on it.
pub fn step(app: &mut App, done: usize, total: usize) {
    if let Some(d) = app.import.as_mut() {
        d.stage = Stage::Running { done, total };
    }
}

pub fn finished(app: &mut App, result: Result<usize, String>) {
    let Some(d) = app.import.as_mut() else { return };
    let ok = result.is_ok();
    let rows = d.rows().len();
    d.stage = Stage::Done(result.map(|_| rows));
    if ok {
        let (conn, folder) = (d.conn.clone(), d.folder.clone());
        app.mark_stale(&conn, &folder);
        app.request_children(&conn, folder);
        app.refresh_relations(&conn);
    }
}

/// Where the picker starts: the project, Downloads, home.
pub fn start_dir(app: &App) -> PathBuf {
    app.session.project.clone().or_else(dirs::download_dir).or_else(dirs::home_dir).unwrap_or_else(|| Path::new("/").to_path_buf())
}
