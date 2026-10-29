#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app_win;
mod editor;
mod icon;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 3 && args[1] == "--edit" {
        return editor::run(std::path::Path::new(&args[2]), hooks());
    }
    #[cfg(windows)]
    app_win::run();
    #[cfg(not(windows))]
    eprintln!("Snapling: macOS и Linux — в работе, пока поддерживается только Windows.");
}

#[cfg(windows)]
fn hooks() -> editor::Hooks {
    app_win::editor_hooks()
}

#[cfg(not(windows))]
fn hooks() -> editor::Hooks {
    editor::Hooks {
        copy: |_, _, _| Err("пока только Windows".into()),
        save: |_, _, _, _| Err("пока только Windows".into()),
        warn: |m| eprintln!("{m}"),
    }
}
