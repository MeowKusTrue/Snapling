#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
use snapling_core::i18n::t;
#[cfg(windows)]
mod app_win;
mod editor;
mod history_ui;
mod icon;
mod settings;
mod translate_ui;
mod ui;

/// `data` next to the exe when `portable.txt` lies there.
fn portable_dir(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = exe.parent()?;
    dir.join("portable.txt").is_file().then(|| dir.join("data"))
}

/// `%APPDATA%\Snapling` or `%LOCALAPPDATA%\Snapling`, or the portable folder.
fn app_dir(env: &str) -> std::path::PathBuf {
    let portable = std::env::current_exe().ok();
    portable
        .as_deref()
        .and_then(portable_dir)
        .unwrap_or_else(|| {
            std::env::var_os(env)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("Snapling")
        })
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let cfg = snapling_core::config::Config::load_or_create(&ui::config_path());
    ui::apply_language(&cfg.map_or("auto".to_string(), |c| c.ui.language));
    if args.len() == 3 && args[1] == "--edit" {
        return editor::run(std::path::Path::new(&args[2]), hooks());
    }
    if args.len() == 3 && args[1] == "--translate" {
        return translate_ui::run(std::path::Path::new(&args[2]), text_hooks(), false);
    }
    if args.len() == 3 && args[1] == "--text" {
        return translate_ui::run(std::path::Path::new(&args[2]), text_hooks(), true);
    }
    if args.len() == 2 && args[1] == "--settings" {
        return settings::run(settings_hooks(), false);
    }
    if args.len() == 2 && args[1] == "--welcome" {
        return settings::run(settings_hooks(), true);
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
        copy: |_, _, _| Err(t("пока только Windows").into()),
        save: |_, _, _, _| Err(t("пока только Windows").into()),
        save_as: |_, _, _, _| Err(t("пока только Windows").into()),
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
        set_autostart: |_| Err(t("пока только Windows").into()),
        pick_folder: || None,
        record_keys: |_| {},
        recorded_combo: || None,
        hotkey_free: |_| true,
        autostart_blocked: || false,
        get_proxy: || None,
        set_proxy: |_| Err(t("пока только Windows").into()),
        reserved: |_| None,
        snipping_owns: || false,
        open: |_| {},
        log: std::path::PathBuf::new(),
        default_dir: || None,
        pick_wav: || None,
        preview: |_, _| Err(t("пока только Windows").into()),
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
        copy_image: |_| Err(t("пока только Windows").into()),
        copy_text: |_| Err(t("пока только Windows").into()),
        edit: |_, _| Err(t("пока только Windows").into()),
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
        copy_text: |_| Err(t("пока только Windows").into()),
        warn: |m| eprintln!("{m}"),
        models: std::path::PathBuf::from("models"),
        online: Default::default(),
        engine: "PaddleOCR",
        get_key: |_| None,
        set_key: |_, _| Err(t("пока только Windows").into()),
        get_proxy: || None,
    }
}

#[cfg(test)]
mod tests {
    use super::portable_dir;

    #[test]
    fn a_marker_next_to_the_exe_makes_it_portable() {
        let dir = std::env::temp_dir().join(format!("snapling-portable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("snapling.exe");
        assert_eq!(portable_dir(&exe), None);
        std::fs::write(dir.join("portable.txt"), "").unwrap();
        assert_eq!(portable_dir(&exe), Some(dir.join("data")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
