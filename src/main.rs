//! zapros: an SQL IDE for PostgreSQL, ClickHouse, Oracle and SQLite.

mod app;
mod config;
mod db;
mod dialogs;
mod editor;
mod export;
mod fonts;
mod i18n;
mod outline_ui;
mod results;
mod secrets;
mod side;
mod sql;
mod ui;
#[cfg(test)]
mod ui_tests;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("zapros").with_app_id("zapros").with_inner_size([1400.0, 900.0]).with_min_inner_size([700.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native("zapros", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
