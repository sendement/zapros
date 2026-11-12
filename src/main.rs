//! zapros: an SQL IDE for PostgreSQL, ClickHouse, Oracle and SQLite.

mod app;
mod config;
mod db;
mod dialogs;
mod editor;
mod export;
mod fonts;
mod i18n;
mod notebook_ui;
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
        // No waiting for the display in eglSwapBuffers: on Wayland a window
        // the compositor doesn't show (another workspace or screen, behind a
        // full-screen one) gets no frame callbacks, so the wait never ended
        // and the window stopped answering ("not responding"). Frames are
        // paced in App::ui instead. (As in Polka.)
        glow_options: eframe::egui_glow::GlowConfiguration { vsync: false, ..Default::default() },
        ..Default::default()
    };
    eframe::run_native("zapros", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
