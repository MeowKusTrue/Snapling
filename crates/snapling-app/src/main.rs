#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    println!("{} {}", snapling_core::APP_NAME, snapling_core::VERSION);
}
