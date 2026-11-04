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

/// Settings (`APPDATA`) and data (`LOCALAPPDATA`) folders, or the portable folder. Linux and
/// macOS also take `STATE` (history) and `CACHE` (temporary files).
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
        let lib = if env == "CACHE" {
            "Library/Caches"
        } else {
            "Library/Application Support"
        };
        return home.join(lib).join("Snapling");
    }
    let (var, fallback) = match env {
        "APPDATA" => ("XDG_CONFIG_HOME", ".config"),
        "STATE" => ("XDG_STATE_HOME", ".local/state"),
        "CACHE" => ("XDG_CACHE_HOME", ".cache"),
        _ => ("XDG_DATA_HOME", ".local/share"),
    };
    std::env::var_os(var)
        .map_or_else(|| home.join(fallback), std::path::PathBuf::from)
        .join("snapling")
}

/// Folders and files Snapling uses, for `--print-paths` and `--doctor`.
fn paths() -> Vec<(&'static str, std::path::PathBuf)> {
    let data = app_dir("LOCALAPPDATA");
    let mut list = vec![
        ("config", ui::config_path()),
        ("data", data.clone()),
        ("models", data.join("models")),
    ];
    #[cfg(windows)]
    list.push(("log", data.join("snapling.log")));
    #[cfg(not(windows))]
    {
        list.push(("state", app_dir("STATE")));
        list.push(("cache", app_dir("CACHE")));
        list.push(("runtime", snapling_unix::runtime_dir()));
        if let Some(p) = snapling_unix::pictures_dir() {
            list.push(("pictures", p.join("Snapling")));
        }
    }
    list
}

const HELP: &str = r#"Snapling — screenshots, annotations, OCR and translation

Usage: snapling [OPTION] [FILE]

  -h, --help             Show this help and exit
  -V, --version          Show version, commit and target
      --region           Select a screen region (Linux/macOS)
      --screen           Capture the screen (Linux/macOS)
      --ocr              Recognize text in a region (Linux/macOS)
      --ocr-translate    Recognize and translate a region (Linux/macOS)
      --record           Start or stop recording (Linux/macOS)
      --record-cancel    Cancel recording and delete its file (Linux/macOS)
      --long             Take a scrolling screenshot (Linux/macOS)
      --delay            Select after the configured delay (Linux/macOS)
      --settings         Open settings
      --gallery          Open the gallery
      --welcome          Open the welcome screen
      --edit FILE        Edit an image
      --text FILE        Open recognized text
      --translate FILE   Open the translation window
      --pin FILE         Pin an image
      --print-paths      Show config, data and other folders
      --doctor [--json]  Check the session and tools, print install commands (Linux)

Exit codes: 0 success, 1 error, 2 invalid arguments, 3 missing tool,
4 no graphical session, 5 cancelled."#;

fn validate_args(args: &[std::ffi::OsString]) -> Result<(), String> {
    let Some(flag) = args.get(1) else {
        return Ok(());
    };
    let Some(flag) = flag.to_str() else {
        return Err("argument is not valid UTF-8".into());
    };
    let file_arg = matches!(flag, "--edit" | "--translate" | "--pin" | "--text");
    let window = matches!(flag, "--settings" | "--welcome" | "--gallery");
    let window = window || flag == "--print-paths";
    #[cfg(not(windows))]
    let action = app_unix::ACTIONS.contains(&flag);
    #[cfg(windows)]
    let action = false;
    let doctor = cfg!(target_os = "linux")
        && flag == "--doctor"
        && (args.len() == 2 || (args.len() == 3 && args[2] == "--json"));
    let valid = doctor || (file_arg && args.len() == 3) || ((window || action) && args.len() == 2);
    if valid {
        Ok(())
    } else {
        Err(format!("unknown option or wrong arguments: {flag}"))
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 2 && (args[1] == "--help" || args[1] == "-h") {
        println!("{HELP}");
        return;
    }
    if args.len() == 2 && (args[1] == "--version" || args[1] == "-V") {
        println!(
            "snapling {} ({}, {})",
            env!("CARGO_PKG_VERSION"),
            env!("SNAPLING_COMMIT"),
            env!("SNAPLING_TARGET")
        );
        return;
    }
    if let Err(e) = validate_args(&args) {
        eprintln!("snapling: {e}\nTry 'snapling --help' for more information.");
        std::process::exit(2);
    }
    if args.len() == 2 && args[1] == "--print-paths" {
        for (name, path) in paths() {
            println!("{name:<9} {}", path.display());
        }
        return;
    }
    #[cfg(target_os = "linux")]
    if args.get(1).is_some_and(|a| a == "--doctor") {
        let dirs: Vec<_> = paths()
            .into_iter()
            .map(|(n, p)| match p.parent() {
                Some(dir) if n == "config" => (n, dir.to_path_buf()),
                _ => (n, p),
            })
            .collect();
        std::process::exit(snapling_unix::doctor::report(&dirs, args.len() == 3));
    }
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
    if args.len() == 2
        && let Some(code) = args[1].to_str().and_then(app_unix::run_flag)
    {
        std::process::exit(code);
    }
    if args.len() == 3 && args[1] == "--text" {
        return translate_ui::run(std::path::Path::new(&args[2]), text_hooks(), true);
    }
    if args.len() == 2 && args[1] == "--settings" {
        return settings::run(settings_hooks(), settings::Page::Settings);
    }
    if args.len() == 2 && args[1] == "--welcome" {
        return settings::run(settings_hooks(), settings::Page::Welcome);
    }
    if args.len() == 2 && args[1] == "--gallery" {
        return settings::run(settings_hooks(), settings::Page::Gallery);
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
fn text_hooks() -> translate_ui::Hooks {
    app_win::text_hooks()
}

#[cfg(not(windows))]
fn text_hooks() -> translate_ui::Hooks {
    app_unix::text_hooks()
}

#[cfg(test)]
mod tests {
    use super::{portable_dir, validate_args};

    fn args(parts: &[&str]) -> Vec<std::ffi::OsString> {
        parts.iter().map(Into::into).collect()
    }

    #[test]
    fn cli_rejects_unknown_flags_and_missing_file_arguments() {
        assert!(validate_args(&args(&["snapling"])).is_ok());
        assert!(validate_args(&args(&["snapling", "--edit", "shot.raw"])).is_ok());
        assert!(validate_args(&args(&["snapling", "--settings"])).is_ok());
        assert!(validate_args(&args(&["snapling", "--hlep"])).is_err());
        assert!(validate_args(&args(&["snapling", "--edit"])).is_err());
        assert!(validate_args(&args(&["snapling", "--settings", "extra"])).is_err());
        assert!(validate_args(&args(&["snapling", "--help", "extra"])).is_err());
        assert!(validate_args(&args(&["snapling", "--print-paths"])).is_ok());
        assert!(validate_args(&args(&["snapling", "--doctor", "--yaml"])).is_err());
    }

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
