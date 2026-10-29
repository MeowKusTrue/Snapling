#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app_win;
mod editor;
mod icon;
mod translate_ui;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 3 && args[1] == "--edit" {
        return editor::run(std::path::Path::new(&args[2]), hooks());
    }
    if args.len() == 3 && args[1] == "--translate" {
        return translate_ui::run(std::path::Path::new(&args[2]), text_hooks());
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

#[cfg(windows)]
fn text_hooks() -> translate_ui::Hooks {
    app_win::text_hooks()
}

#[cfg(not(windows))]
fn text_hooks() -> translate_ui::Hooks {
    translate_ui::Hooks {
        copy_text: |_| Err("пока только Windows".into()),
        warn: |m| eprintln!("{m}"),
        models: std::path::PathBuf::from("models"),
        online: false,
        get_key: || None,
        set_key: |_| Err("пока только Windows".into()),
    }
}
