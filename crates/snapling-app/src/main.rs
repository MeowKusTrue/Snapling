#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
mod app_unix;
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

/// Settings (`APPDATA`) and data (`LOCALAPPDATA`) folders, or the portable folder.
fn app_dir(env: &str) -> std::path::PathBuf {
    let portable = std::env::current_exe().ok();
    portable
        .as_deref()
        .and_then(portable_dir)
        .unwrap_or_else(|| system_dir(env))
}

#[cfg(windows)]
fn system_dir(env: &str) -> std::path::PathBuf {
    std::env::var_os(env)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Snapling")
}

#[cfg(not(windows))]
fn system_dir(env: &str) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support/Snapling");
    }
    let (var, fallback) = match env {
        "APPDATA" => ("XDG_CONFIG_HOME", ".config"),
        _ => ("XDG_DATA_HOME", ".local/share"),
    };
    std::env::var_os(var)
        .map_or_else(|| home.join(fallback), std::path::PathBuf::from)
        .join("snapling")
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
    #[cfg(windows)]
    if args.len() == 3 && args[1] == "--pin" {
        return app_win::run_pin(std::path::Path::new(&args[2]));
    }
    #[cfg(not(windows))]
    if args.len() == 3 && args[1] == "--pin" {
        return app_unix::run_pin(std::path::Path::new(&args[2]));
    }
    #[cfg(not(windows))]
    if args.len() == 2 && args[1].to_str().is_some_and(app_unix::run_flag) {
        return;
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
    app_unix::run_tray();
}

#[cfg(windows)]
fn hooks() -> editor::Hooks {
    app_win::editor_hooks()
}

#[cfg(not(windows))]
fn hooks() -> editor::Hooks {
    app_unix::editor_hooks()
}

#[cfg(windows)]
fn settings_hooks() -> settings::Hooks {
    app_win::settings_hooks()
}

#[cfg(not(windows))]
fn settings_hooks() -> settings::Hooks {
    app_unix::settings_hooks()
}

#[cfg(windows)]
fn history_hooks() -> history_ui::Hooks {
    app_win::history_hooks()
}

#[cfg(not(windows))]
fn history_hooks() -> history_ui::Hooks {
    app_unix::history_hooks()
}

#[cfg(windows)]
fn text_hooks() -> translate_ui::Hooks {
    app_win::text_hooks()
}

#[cfg(not(windows))]
fn text_hooks() -> translate_ui::Hooks {
    app_unix::text_hooks()
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
