//! zapros: an SQL IDE for PostgreSQL, ClickHouse, Oracle and SQLite.

mod app;
mod block;
mod config;
mod db;
mod design;
mod design_ui;
mod dialogs;
mod drivers;
mod editor;
mod export;
mod fonts;
mod i18n;
mod md;
mod icons;
mod icons_data;
mod notebook_ui;
mod outline_ui;
mod results;
mod sched;
mod search;
mod themes;
mod titlebar;
mod secrets;
mod side;
mod sql;
mod ui;
#[cfg(test)]
mod ui_tests;

fn main() -> eframe::Result {
    // The desktop's title bar, or the window's own (a setting).
    let settings: config::Settings = config::load(&config::config_dir().join("settings.json"));
    // Oracle's client from the folder chosen in the settings (it's loaded
    // once, on the first connection).
    drivers::prepare(&settings.oracle_client);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("zapros").with_app_id(std::env::var("FLATPAK_ID").unwrap_or_else(|_| "zapros".into())).with_inner_size([1400.0, 900.0]).with_min_inner_size([700.0, 400.0]).with_decorations(settings.system_titlebar),
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
