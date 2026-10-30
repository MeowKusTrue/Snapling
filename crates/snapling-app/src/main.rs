#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app_win;
mod editor;
mod history_ui;
mod icon;
mod settings;
mod translate_ui;
mod ui;

/// `%APPDATA%\Snapling` or `%LOCALAPPDATA%\Snapling`.
fn app_dir(env: &str) -> std::path::PathBuf {
    std::env::var_os(env)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Snapling")
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 3 && args[1] == "--edit" {
        return editor::run(std::path::Path::new(&args[2]), hooks());
    }
    if args.len() == 3 && args[1] == "--translate" {
        return translate_ui::run(std::path::Path::new(&args[2]), text_hooks());
    }
    if args.len() == 2 && args[1] == "--settings" {
        return settings::run(settings_hooks());
    }
    if args.len() == 2 && args[1] == "--history" {
        return history_ui::run(history_hooks());
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
        save_as: |_, _, _, _| Err("пока только Windows".into()),
        text: |_, _, _| {},
        warn: |m| eprintln!("{m}"),
    }
}

#[cfg(windows)]
fn settings_hooks() -> settings::Hooks {
    app_win::settings_hooks()
}

#[cfg(not(windows))]
fn settings_hooks() -> settings::Hooks {
    settings::Hooks {
        warn: |m| eprintln!("{m}"),
        autostart: || false,
        set_autostart: |_| Err("пока только Windows".into()),
        pick_folder: || None,
        pressed_combo: || None,
        pause_hotkeys: |_| {},
        reserved: |_| None,
        open: |_| {},
        log: std::path::PathBuf::new(),
        default_dir: || None,
        pick_wav: || None,
        preview: |_, _| Err("пока только Windows".into()),
    }
}

#[cfg(windows)]
fn history_hooks() -> history_ui::Hooks {
    app_win::history_hooks()
}

#[cfg(not(windows))]
fn history_hooks() -> history_ui::Hooks {
    history_ui::Hooks {
        warn: |m| eprintln!("{m}"),
        file: app_dir("LOCALAPPDATA").join("history.jsonl"),
        now: String::new,
        copy_image: |_| Err("пока только Windows".into()),
        copy_text: |_| Err("пока только Windows".into()),
        edit: |_, _| Err("пока только Windows".into()),
        open: |_| {},
        open_folder: || {},
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
