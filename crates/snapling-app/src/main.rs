#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app_win;
#[cfg(windows)]
mod icon;

fn main() {
    #[cfg(windows)]
    app_win::run();
    #[cfg(not(windows))]
    eprintln!("Snapling: macOS и Linux — в работе, пока поддерживается только Windows.");
}
