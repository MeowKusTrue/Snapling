//! `snapling --doctor`: what the session offers, what is missing and the command that installs it.
//! The same table decides which tools a flag needs before it runs.

use crate::{Desktop, desktop, runtime_dir};
use snapling_core::i18n::tf;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Ok => "OK",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        }
    }
}

pub struct Check {
    pub name: String,
    pub level: Level,
    pub detail: String,
    /// Arch package that fixes it.
    pub package: Option<&'static str>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    /// Every capture.
    Core,
    Record,
    Long,
    Extra,
}

struct Tool {
    /// Any of these is enough.
    names: &'static [&'static str],
    package: &'static str,
    purpose: &'static str,
    need: Need,
}

const fn tool(
    names: &'static [&'static str],
    package: &'static str,
    purpose: &'static str,
    need: Need,
) -> Tool {
    Tool {
        names,
        package,
        purpose,
        need,
    }
}

fn tools(d: Desktop) -> Vec<Tool> {
    let mut list = Vec::new();
    if matches!(d, Desktop::Hyprland | Desktop::Wayland) {
        list.extend([
            tool(&["grim"], "grim", "capture", Need::Core),
            tool(&["wl-copy"], "wl-clipboard", "clipboard", Need::Core),
            tool(&["wf-recorder"], "wf-recorder", "recording", Need::Record),
            tool(&["wlrctl"], "wlrctl", "scrolling screenshot", Need::Long),
            tool(&["ffmpeg"], "ffmpeg", "GIF", Need::Extra),
        ]);
    }
    if d == Desktop::Hyprland {
        list.push(tool(
            &["hyprctl"],
            "hyprland",
            "monitors and windows",
            Need::Core,
        ));
    }
    if d == Desktop::X11 {
        list.extend([
            tool(&["xclip"], "xclip", "clipboard", Need::Core),
            tool(&["ffmpeg"], "ffmpeg", "recording and GIF", Need::Record),
            tool(&["xdotool"], "xdotool", "scrolling screenshot", Need::Long),
        ]);
    }
    list.extend([
        tool(&["notify-send"], "libnotify", "notifications", Need::Extra),
        tool(&["secret-tool"], "libsecret", "API keys", Need::Extra),
        tool(
            &["zenity", "kdialog"],
            "zenity",
            "error dialogs",
            Need::Extra,
        ),
        tool(
            &["pw-play", "paplay", "aplay"],
            "pipewire",
            "sounds",
            Need::Extra,
        ),
    ]);
    list
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Graphical session, or why there is none.
fn session() -> Check {
    let check = |level, detail: String| Check {
        name: "session".into(),
        level,
        detail,
        package: None,
    };
    if let Some(name) = std::env::var_os("WAYLAND_DISPLAY") {
        let socket = runtime_dir().join(&name);
        return if socket.exists() {
            check(Level::Ok, format!("Wayland ({})", name.to_string_lossy()))
        } else {
            check(
                Level::Fail,
                format!("no compositor socket {}", socket.display()),
            )
        };
    }
    match std::env::var("DISPLAY") {
        Ok(d) => check(Level::Ok, format!("X11 ({d})")),
        Err(_) => check(
            Level::Fail,
            "neither WAYLAND_DISPLAY nor DISPLAY is set".into(),
        ),
    }
}

/// Whether a D-Bus name has an owner; `None` when it cannot be asked.
fn bus_name(name: &str) -> Option<bool> {
    let out = Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.NameHasOwner",
            &format!("string:{name}"),
        ])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).contains("boolean true"))
}

/// Writes and removes a probe in the nearest existing folder of `dir`.
fn writable(dir: &Path) -> bool {
    let Some(existing) = dir.ancestors().find(|p| p.is_dir()) else {
        return false;
    };
    let probe = existing.join(format!(".snapling-doctor-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

fn check(name: &str, level: Level, detail: impl Into<String>) -> Check {
    Check {
        name: name.into(),
        level,
        detail: detail.into(),
        package: None,
    }
}

/// Every check; `dirs` are the folders Snapling writes to.
pub fn run(dirs: &[(&str, PathBuf)]) -> Vec<Check> {
    let d = desktop();
    let mut out = vec![session()];
    out.push(match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(p) if Path::new(&p).is_dir() => check("runtime", Level::Ok, p.to_string_lossy()),
        _ => check(
            "runtime",
            Level::Warn,
            "XDG_RUNTIME_DIR missing, temp folder is used",
        ),
    });
    if d == Desktop::Hyprland {
        out.push(match Command::new("hyprctl").arg("version").output() {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                check(
                    "hyprland",
                    Level::Ok,
                    text.lines().next().unwrap_or("").trim(),
                )
            }
            _ => check("hyprland", Level::Fail, "hyprctl does not answer"),
        });
    }
    out.push(if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
        check("dbus", Level::Ok, "session bus")
    } else {
        check(
            "dbus",
            Level::Warn,
            "no session bus: no tray, notifications or keys",
        )
    });
    out.push(match bus_name("org.kde.StatusNotifierWatcher") {
        Some(true) => check("tray", Level::Ok, "StatusNotifierItem host"),
        Some(false) => check(
            "tray",
            Level::Warn,
            "no tray host (Waybar needs the tray module): use key binds and flags",
        ),
        None => check("tray", Level::Warn, "not checked: dbus-send missing"),
    });
    out.push(match bus_name("org.freedesktop.portal.Desktop") {
        Some(true) => check("portal", Level::Ok, "xdg-desktop-portal"),
        _ => check("portal", Level::Warn, "xdg-desktop-portal not running"),
    });
    out.push(if runtime_dir().join("pipewire-0").exists() {
        check("pipewire", Level::Ok, "pipewire-0")
    } else {
        check(
            "pipewire",
            Level::Warn,
            "PipeWire is not running: no sounds",
        )
    });
    for t in tools(d) {
        let found = t.names.iter().find_map(|n| on_path(n));
        out.push(Check {
            name: t.names[0].into(),
            level: match (&found, t.need) {
                (Some(_), _) => Level::Ok,
                (None, Need::Core) => Level::Fail,
                (None, _) => Level::Warn,
            },
            detail: match found {
                Some(p) => format!("{} ({})", t.purpose, p.display()),
                None => format!("{}: not found", t.purpose),
            },
            package: Some(t.package),
        });
    }
    for (name, dir) in dirs {
        out.push(if writable(dir) {
            check(name, Level::Ok, dir.to_string_lossy())
        } else {
            check(
                name,
                Level::Fail,
                format!("{} is not writable", dir.display()),
            )
        });
    }
    out
}

/// `ID` and `ID_LIKE` of `/etc/os-release`.
fn distro() -> String {
    let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    text.lines()
        .filter_map(|l| l.strip_prefix("ID=").or_else(|| l.strip_prefix("ID_LIKE=")))
        .map(|v| v.trim_matches('"'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The install command for the packages of this distribution.
pub fn install_command(packages: &[&str], distro: &str) -> String {
    let has = |id: &str| distro.split_whitespace().any(|w| w == id);
    let rename = |p: &str, debian: bool| -> String {
        match (p, debian) {
            ("libnotify", true) => "libnotify-bin".into(),
            ("libsecret", true) => "libsecret-tools".into(),
            ("pipewire", true) => "pipewire-bin".into(),
            ("pipewire", false) if has("fedora") => "pipewire-utils".into(),
            _ => p.into(),
        }
    };
    let debian = has("debian") || has("ubuntu");
    let list: Vec<String> = packages.iter().map(|p| rename(p, debian)).collect();
    let list = list.join(" ");
    if has("arch") {
        format!("sudo pacman -S --needed {list}")
    } else if debian {
        format!("sudo apt install {list}")
    } else if has("fedora") {
        format!("sudo dnf install {list}")
    } else {
        format!("install: {list}")
    }
}

/// Packages of the missing tools, each once.
fn missing(checks: &[Check]) -> Vec<&'static str> {
    let mut list = Vec::new();
    for c in checks.iter().filter(|c| c.level != Level::Ok) {
        if let Some(p) = c.package
            && !list.contains(&p)
        {
            list.push(p);
        }
    }
    list
}

/// Exit code: 4 without a session, 3 when a needed tool is missing, 1 for another failure.
fn code(checks: &[Check]) -> i32 {
    let failed: Vec<&Check> = checks.iter().filter(|c| c.level == Level::Fail).collect();
    if failed.iter().any(|c| c.name == "session") {
        4
    } else if failed.iter().any(|c| c.package.is_some()) {
        3
    } else if failed.is_empty() {
        0
    } else {
        1
    }
}

/// Prints the report, returns the exit code.
pub fn report(dirs: &[(&str, PathBuf)], json: bool) -> i32 {
    let checks = run(dirs);
    let packages = missing(&checks);
    let install = (!packages.is_empty()).then(|| install_command(&packages, &distro()));
    if json {
        let list: Vec<_> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "level": c.level.label(),
                    "detail": c.detail,
                    "package": c.package,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "checks": list, "install": install })
        );
    } else {
        for c in &checks {
            println!("{:<5} {:<12} {}", c.level.label(), c.name, c.detail);
        }
        if let Some(cmd) = &install {
            println!("\n{cmd}");
        }
    }
    code(&checks)
}

/// Before a flag runs: exit code and message when the session or a tool it needs is missing.
pub fn preflight(flag: &str) -> Result<(), (i32, String)> {
    let s = session();
    if s.level == Level::Fail {
        return Err((4, s.detail));
    }
    let extra = match flag {
        "--record" => Need::Record,
        "--long" => Need::Long,
        "--record-cancel" => return Ok(()),
        _ => Need::Core,
    };
    let lost: Vec<Tool> = tools(desktop())
        .into_iter()
        .filter(|t| t.need == Need::Core || t.need == extra)
        .filter(|t| t.names.iter().all(|n| on_path(n).is_none()))
        .collect();
    if lost.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = lost.iter().map(|t| t.names[0]).collect();
    let packages: Vec<&str> = lost.iter().map(|t| t.package).collect();
    let cmd = install_command(&packages, &distro());
    Err((
        3,
        tf(
            "Не найдена программа {0}. Установите: {1}",
            &[("0", &names.join(", ")), ("1", &cmd)],
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_command_follows_the_distribution() {
        let p = ["grim", "libnotify"];
        assert_eq!(
            install_command(&p, "cachyos arch"),
            "sudo pacman -S --needed grim libnotify"
        );
        assert_eq!(
            install_command(&p, "ubuntu debian"),
            "sudo apt install grim libnotify-bin"
        );
        assert_eq!(install_command(&p, "nixos"), "install: grim libnotify");
    }

    #[test]
    fn no_session_wins_over_a_missing_tool() {
        let c = |name: &str, level, package| Check {
            name: name.into(),
            level,
            detail: String::new(),
            package,
        };
        let tool = c("grim", Level::Fail, Some("grim"));
        assert_eq!(code(&[c("session", Level::Ok, None), tool]), 3);
        let tool = c("grim", Level::Fail, Some("grim"));
        assert_eq!(code(&[c("session", Level::Fail, None), tool]), 4);
        assert_eq!(code(&[c("tray", Level::Warn, None)]), 0);
    }
}
