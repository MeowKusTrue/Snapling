//! Linux and macOS through the tools those systems ship: `grim`, `hyprctl`, `wl-copy`, `xclip`,
//! `pw-play`, `secret-tool`, `notify-send`, or `screencapture`, `pbcopy`, `osascript`, `afplay`,
//! `security`. A missing tool becomes a message that names it.

use snapling_core::i18n::{t, tf};
use snapling_core::selection::Rect;
use snapling_overlay::Shot;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(target_os = "linux")]
pub mod tray;
#[cfg(target_os = "linux")]
mod x11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Hyprland,
    /// Sway and other compositors `grim` works on.
    Wayland,
    X11,
    Mac,
}

pub fn desktop() -> Desktop {
    if cfg!(target_os = "macos") {
        Desktop::Mac
    } else if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        Desktop::Hyprland
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Desktop::Wayland
    } else {
        Desktop::X11
    }
}

/// Stdout of a tool. The message says what to install when the tool is not there.
fn output(tool: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new(tool)
        .args(args)
        .stderr(Stdio::piped())
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(o.stdout),
        Ok(o) => Err(format!(
            "{tool}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(_) => Err(tf("Не найдена программа {0}", &[("0", &tool)])),
    }
}

fn feed(tool: &str, args: &[&str], data: &[u8]) -> Result<(), String> {
    let child = Command::new(tool)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = child.map_err(|_| tf("Не найдена программа {0}", &[("0", &tool)]))?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(data).map_err(|e| format!("{tool}: {e}"))?;
    }
    let status = child.wait().map_err(|e| format!("{tool}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{tool}: {status}"))
    }
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from)
}

/// Binary PPM (`P6`, 8 bits) as BGRA.
pub fn ppm_to_bgra(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut at = 0;
    let mut field = || {
        loop {
            while data.get(at)?.is_ascii_whitespace() {
                at += 1;
            }
            if data[at] != b'#' {
                break;
            }
            at += data[at..].iter().position(|b| *b == b'\n')?;
        }
        let start = at;
        while !data.get(at)?.is_ascii_whitespace() {
            at += 1;
        }
        Some(std::str::from_utf8(&data[start..at]).ok()?.to_string())
    };
    if field()? != "P6" {
        return None;
    }
    let (w, h): (u32, u32) = (field()?.parse().ok()?, field()?.parse().ok()?);
    if field()? != "255" {
        return None;
    }
    let body = data.get(at + 1..at + 1 + (w as usize * h as usize * 3))?;
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for px in body.as_chunks::<3>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    Some((out, w, h))
}

/// One Hyprland monitor; `x`, `y` are in layout units, the size in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub scale: f32,
    pub workspace: i64,
    pub focused: bool,
}

pub fn parse_monitors(json: &str) -> Vec<Monitor> {
    let Ok(serde_json::Value::Array(all)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    all.iter()
        .filter_map(|m| {
            Some(Monitor {
                id: m["id"].as_i64()?,
                name: m["name"].as_str()?.to_string(),
                x: m["x"].as_i64()? as i32,
                y: m["y"].as_i64()? as i32,
                scale: m["scale"].as_f64().unwrap_or(1.0) as f32,
                workspace: m["activeWorkspace"]["id"].as_i64().unwrap_or(0),
                focused: m["focused"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// Windows on the monitor's workspace, topmost first, in the shot's pixels.
pub fn parse_clients(json: &str, mon: &Monitor) -> Vec<(Rect, String)> {
    let Ok(serde_json::Value::Array(all)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    let mut found: Vec<(i64, Rect, String)> = all
        .iter()
        .filter(|c| c["mapped"].as_bool() == Some(true) && c["hidden"].as_bool() != Some(true))
        .filter(|c| c["workspace"]["id"].as_i64() == Some(mon.workspace))
        .filter_map(|c| {
            let num = |k: &str, i: usize| c[k][i].as_f64().map(|v| v as f32);
            let local = |v: f32, origin: i32| ((v - origin as f32) * mon.scale).round() as i32;
            Some((
                c["focusHistoryID"].as_i64().unwrap_or(i64::MAX),
                Rect {
                    left: local(num("at", 0)?, mon.x),
                    top: local(num("at", 1)?, mon.y),
                    width: (num("size", 0)? * mon.scale).round() as i32,
                    height: (num("size", 1)? * mon.scale).round() as i32,
                },
                c["class"].as_str().unwrap_or_default().to_string(),
            ))
        })
        .collect();
    found.sort_by_key(|f| f.0);
    found.into_iter().map(|f| (f.1, f.2)).collect()
}

/// A shot of the current monitor with what is needed to reach it again.
pub struct Capture {
    pub shot: Shot,
    pub targets: Vec<(Rect, String)>,
    /// Layout position and scale of the monitor, for tools that take layout units.
    pub origin: (i32, i32),
    pub scale: f32,
}

fn hypr_monitor() -> Result<Monitor, String> {
    let json = output("hyprctl", &["monitors", "-j"])?;
    let all = parse_monitors(&String::from_utf8_lossy(&json));
    let first = all.first().cloned();
    all.into_iter()
        .find(|m| m.focused)
        .or(first)
        .ok_or_else(|| t("Монитор не найден").to_string())
}

fn shot_of(bgra: Vec<u8>, width: u32, height: u32) -> Shot {
    Shot {
        x: 0,
        y: 0,
        width,
        height,
        bgra,
    }
}

pub fn capture() -> Result<Capture, String> {
    let no_image = || t("Снимок не прочитан").to_string();
    match desktop() {
        Desktop::Hyprland => {
            let mon = hypr_monitor()?;
            let ppm = output("grim", &["-o", &mon.name, "-t", "ppm", "-"])?;
            let (bgra, w, h) = ppm_to_bgra(&ppm).ok_or_else(no_image)?;
            let clients = output("hyprctl", &["clients", "-j"])?;
            let targets = parse_clients(&String::from_utf8_lossy(&clients), &mon);
            Ok(Capture {
                shot: shot_of(bgra, w, h),
                targets,
                origin: (mon.x, mon.y),
                scale: mon.scale,
            })
        }
        Desktop::Wayland => {
            let ppm = output("grim", &["-t", "ppm", "-"])?;
            let (bgra, w, h) = ppm_to_bgra(&ppm).ok_or_else(no_image)?;
            Ok(Capture {
                shot: shot_of(bgra, w, h),
                targets: Vec::new(),
                origin: (0, 0),
                scale: 1.0,
            })
        }
        Desktop::X11 => x11_capture(),
        Desktop::Mac => {
            let file = runtime_dir().join("snapling-shot.png");
            let text = file.to_string_lossy().to_string();
            output("screencapture", &["-x", "-t", "png", &text])?;
            let (bgra, w, h) = snapling_core::storage::load_bgra(&file)?;
            let _ = std::fs::remove_file(&file);
            Ok(Capture {
                shot: shot_of(bgra, w, h),
                targets: Vec::new(),
                origin: (0, 0),
                scale: 1.0,
            })
        }
    }
}

#[cfg(target_os = "linux")]
fn x11_capture() -> Result<Capture, String> {
    let (bgra, w, h) = x11::root_image()?;
    Ok(Capture {
        shot: shot_of(bgra, w, h),
        targets: Vec::new(),
        origin: (0, 0),
        scale: 1.0,
    })
}

#[cfg(not(target_os = "linux"))]
fn x11_capture() -> Result<Capture, String> {
    Err(t("Захват здесь не поддерживается").into())
}

/// `x,y wxh` of an area of the shot in layout units, as `grim` and `wf-recorder` take it.
pub fn geometry(area: Rect, origin: (i32, i32), scale: f32) -> String {
    let unit = |v: i32| (v as f32 / scale).round() as i32;
    format!(
        "{},{} {}x{}",
        origin.0 + unit(area.left),
        origin.1 + unit(area.top),
        unit(area.width),
        unit(area.height)
    )
}

/// An area of the screen again, for a long shot: BGRA of the same area every time.
pub fn capture_area(area: Rect, origin: (i32, i32), scale: f32) -> Result<Vec<u8>, String> {
    let geo = geometry(area, origin, scale);
    let ppm = output("grim", &["-g", &geo, "-t", "ppm", "-"])?;
    let (bgra, ..) = ppm_to_bgra(&ppm).ok_or(t("Снимок не прочитан"))?;
    Ok(bgra)
}

/// Puts the pointer over `area` and turns the wheel down. Needs `wlrctl` or `xdotool`.
pub fn scroll_down(area: Rect, origin: (i32, i32), scale: f32) -> Result<(), String> {
    let center = |pos: i32, side: i32, o: i32| o + ((pos + side / 2) as f32 / scale) as i32;
    let (x, y) = (
        center(area.left, area.width, origin.0),
        center(area.top, area.height, origin.1),
    );
    match desktop() {
        Desktop::Hyprland => {
            output(
                "hyprctl",
                &["dispatch", "movecursor", &x.to_string(), &y.to_string()],
            )?;
            output("wlrctl", &["pointer", "scroll", "0", "45"]).map(|_| ())
        }
        Desktop::Wayland => output("wlrctl", &["pointer", "scroll", "0", "45"]).map(|_| ()),
        Desktop::X11 => {
            output("xdotool", &["mousemove", &x.to_string(), &y.to_string()])?;
            output("xdotool", &["click", "--repeat", "3", "5"]).map(|_| ())
        }
        Desktop::Mac => Err(t("Длинный скриншот здесь не поддерживается").into()),
    }
}

pub fn copy_png(png: &[u8]) -> Result<(), String> {
    match desktop() {
        Desktop::Hyprland | Desktop::Wayland => feed("wl-copy", &["--type", "image/png"], png),
        Desktop::X11 => feed(
            "xclip",
            &["-selection", "clipboard", "-t", "image/png", "-i"],
            png,
        ),
        Desktop::Mac => {
            let file = runtime_dir().join("snapling-clip.png");
            std::fs::write(&file, png).map_err(|e| e.to_string())?;
            let script = format!(
                "set the clipboard to (read (POSIX file \"{}\") as «class PNGf»)",
                file.display()
            );
            output("osascript", &["-e", &script]).map(|_| ())
        }
    }
}

pub fn copy_text(text: &str) -> Result<(), String> {
    let data = text.as_bytes();
    match desktop() {
        Desktop::Hyprland | Desktop::Wayland => feed("wl-copy", &[], data),
        Desktop::X11 => feed("xclip", &["-selection", "clipboard", "-i"], data),
        Desktop::Mac => feed("pbcopy", &[], data),
    }
}

/// `text/uri-list` of a file, so chats take it as an attachment.
pub fn copy_file(path: &Path) -> Result<(), String> {
    let uri = format!("file://{}\n", path.display());
    match desktop() {
        Desktop::Hyprland | Desktop::Wayland => {
            feed("wl-copy", &["--type", "text/uri-list"], uri.as_bytes())
        }
        Desktop::X11 => feed(
            "xclip",
            &["-selection", "clipboard", "-t", "text/uri-list", "-i"],
            uri.as_bytes(),
        ),
        Desktop::Mac => copy_text(&path.to_string_lossy()),
    }
}

/// Plays a WAV and returns at once: the sound may outlive this process.
pub fn play(wav: &[u8]) -> Result<(), String> {
    let file = runtime_dir().join(format!("snapling-{}.wav", std::process::id()));
    std::fs::write(&file, wav).map_err(|e| e.to_string())?;
    let tools: &[&str] = if cfg!(target_os = "macos") {
        &["afplay"]
    } else {
        &["pw-play", "paplay", "aplay"]
    };
    let started = tools.iter().any(|tool| {
        Command::new(tool)
            .arg(&file)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    });
    if started {
        Ok(())
    } else {
        Err(tf("Не найдена программа {0}", &[("0", &tools[0])]))
    }
}

pub fn notify(title: &str, body: &str) {
    let _ = if cfg!(target_os = "macos") {
        let script = format!("display notification {:?} with title {:?}", body, title);
        Command::new("osascript").args(["-e", &script]).spawn()
    } else {
        Command::new("notify-send")
            .args(["-a", "Snapling", title, body])
            .spawn()
    };
}

/// Warns in a window when a dialog tool is there, in the notification otherwise.
pub fn warn(text: &str) {
    let shown = if cfg!(target_os = "macos") {
        let script = format!("display alert \"Snapling\" message {text:?}");
        output("osascript", &["-e", &script]).is_ok()
    } else {
        output("zenity", &["--error", "--title=Snapling", "--text", text]).is_ok()
            || output("kdialog", &["--error", text]).is_ok()
    };
    if !shown {
        notify("Snapling", text);
    }
}

pub fn ask(text: &str) -> bool {
    if cfg!(target_os = "macos") {
        let script =
            format!("display dialog {text:?} buttons {{\"Cancel\", \"OK\"}} default button \"OK\"");
        return output("osascript", &["-e", &script]).is_ok();
    }
    output(
        "zenity",
        &["--question", "--title=Snapling", "--text", text],
    )
    .is_ok()
        || output("kdialog", &["--yesno", text]).is_ok()
}

pub fn open(path: &Path) {
    let tool = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = Command::new(tool).arg(path).spawn();
}

/// Shows a file in its folder.
pub fn reveal(path: &Path) {
    if cfg!(target_os = "macos") {
        let _ = Command::new("open").arg("-R").arg(path).spawn();
    } else if let Some(dir) = path.parent() {
        open(dir);
    }
}

fn picked(out: Result<Vec<u8>, String>) -> Option<String> {
    let text = String::from_utf8_lossy(&out.ok()?).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn choose(mac: &str, zenity: &[&str], kdialog: &[&str]) -> Option<String> {
    if cfg!(target_os = "macos") {
        return picked(output("osascript", &["-e", mac]));
    }
    picked(output("zenity", zenity)).or_else(|| picked(output("kdialog", kdialog)))
}

pub fn pick_folder() -> Option<String> {
    choose(
        "POSIX path of (choose folder)",
        &["--file-selection", "--directory"],
        &["--getexistingdirectory"],
    )
}

pub fn pick_file(extension: &str) -> Option<String> {
    let filter = format!("--file-filter=*.{extension}");
    let mask = format!("*.{extension}");
    let mac = format!("POSIX path of (choose file of type {{\"{extension}\"}})");
    choose(
        &mac,
        &["--file-selection", &filter],
        &["--getopenfilename", ".", &mask],
    )
}

pub fn save_file(name: &str) -> Option<PathBuf> {
    let flag = format!("--filename={name}");
    let mac = format!("POSIX path of (choose file name default name \"{name}\")");
    choose(
        &mac,
        &["--file-selection", "--save", "--confirm-overwrite", &flag],
        &["--getsavefilename", name],
    )
    .map(PathBuf::from)
}

/// A secret by name, from the system keyring.
pub fn secret_get(name: &str) -> Option<String> {
    let out = if cfg!(target_os = "macos") {
        output(
            "security",
            &["find-generic-password", "-s", "snapling", "-a", name, "-w"],
        )
    } else {
        output("secret-tool", &["lookup", "app", "snapling", "name", name])
    };
    picked(out)
}

pub fn secret_set(name: &str, value: &str) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        // ponytail: the value shows in `ps` for a moment; `security` reads no stdin here.
        return output(
            "security",
            &[
                "add-generic-password",
                "-U",
                "-s",
                "snapling",
                "-a",
                name,
                "-w",
                value,
            ],
        )
        .map(|_| ());
    }
    if value.is_empty() {
        return output("secret-tool", &["clear", "app", "snapling", "name", name]).map(|_| ());
    }
    let label = format!("--label=Snapling {name}");
    feed(
        "secret-tool",
        &["store", &label, "app", "snapling", "name", name],
        value.as_bytes(),
    )
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from)
}

fn autostart_file() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/LaunchAgents/app.snapling.plist")
    } else {
        home().join(".config/autostart/snapling.desktop")
    }
}

pub fn autostart() -> bool {
    autostart_file().is_file()
}

/// Text of the autostart entry for `exe`.
pub fn autostart_text(exe: &Path) -> String {
    if cfg!(target_os = "macos") {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>app.snapling</string>\n\
             <key>ProgramArguments</key><array><string>{}</string></array>\n\
             <key>RunAtLoad</key><true/>\n\
             </dict></plist>\n",
            exe.display()
        )
    } else {
        format!(
            "[Desktop Entry]\nType=Application\nName=Snapling\nExec={}\nX-GNOME-Autostart-enabled=true\n",
            exe.display()
        )
    }
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let file = autostart_file();
    if !on {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, autostart_text(&exe)).map_err(|e| e.to_string())
}

/// Language of the session, as a Snapling language code.
pub fn ui_language() -> &'static str {
    let lang = std::env::var("LANG").unwrap_or_default();
    if lang.starts_with("ru") { "ru" } else { "en" }
}

pub fn pictures_dir() -> Option<PathBuf> {
    Some(home().join("Pictures"))
}

fn record_state() -> PathBuf {
    runtime_dir().join("snapling-record")
}

/// A running recording: pid, seconds at start and the file it writes.
#[derive(Debug, PartialEq, Eq)]
pub struct Running {
    pub pid: u32,
    pub path: PathBuf,
    pub gif: Option<PathBuf>,
}

fn state_text(r: &Running) -> String {
    let gif = r
        .gif
        .as_ref()
        .map_or(String::new(), |g| g.display().to_string());
    format!("{}\n{}\n{gif}\n", r.pid, r.path.display())
}

fn parse_state(text: &str) -> Option<Running> {
    let mut lines = text.lines();
    let pid = lines.next()?.parse().ok()?;
    let path = PathBuf::from(lines.next()?);
    let gif = lines.next().filter(|l| !l.is_empty()).map(PathBuf::from);
    Some(Running { pid, path, gif })
}

pub fn recording() -> Option<Running> {
    parse_state(&std::fs::read_to_string(record_state()).ok()?)
}

/// Starts the recorder for an area of the shot. `gif`: where the final GIF goes after stop.
pub fn start_recording(
    area: Rect,
    origin: (i32, i32),
    scale: f32,
    fps: u32,
    path: &Path,
    gif: Option<PathBuf>,
) -> Result<(), String> {
    let (tool, args): (&str, Vec<String>) = match desktop() {
        Desktop::Hyprland | Desktop::Wayland => (
            "wf-recorder",
            vec![
                "-g".into(),
                geometry(area, origin, scale),
                "-r".into(),
                fps.to_string(),
                "-f".into(),
                path.display().to_string(),
                "-y".into(),
            ],
        ),
        Desktop::X11 => (
            "ffmpeg",
            vec![
                "-y".into(),
                "-f".into(),
                "x11grab".into(),
                "-framerate".into(),
                fps.to_string(),
                "-video_size".into(),
                format!("{}x{}", area.width & !1, area.height & !1),
                "-i".into(),
                format!(
                    "{}+{},{}",
                    std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into()),
                    area.left,
                    area.top
                ),
                path.display().to_string(),
            ],
        ),
        Desktop::Mac => return Err(t("Запись здесь не поддерживается").into()),
    };
    let child = Command::new(tool)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| tf("Не найдена программа {0}", &[("0", &tool)]))?;
    let state = Running {
        pid: child.id(),
        path: path.to_path_buf(),
        gif,
    };
    std::fs::write(record_state(), state_text(&state)).map_err(|e| e.to_string())
}

/// Stops the running recorder, makes the GIF if one was asked for, returns the final file.
pub fn stop_recording(run: Running) -> Result<PathBuf, String> {
    let _ = std::fs::remove_file(record_state());
    output("kill", &["-INT", &run.pid.to_string()])?;
    let proc = PathBuf::from(format!("/proc/{}", run.pid));
    for _ in 0..50 {
        if !proc.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let Some(gif) = run.gif else {
        return Ok(run.path);
    };
    let from = run.path.to_string_lossy().to_string();
    let to = gif.to_string_lossy().to_string();
    let filter = "scale='min(960,iw)':-2,split[a][b];[a]palettegen[p];[b][p]paletteuse";
    output("ffmpeg", &["-y", "-i", &from, "-vf", filter, &to])?;
    let _ = std::fs::remove_file(&run.path);
    Ok(gif)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppm_reads_with_comments_and_keeps_colors() {
        let mut data = b"P6\n# by grim\n2 1\n255\n".to_vec();
        data.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let (px, w, h) = ppm_to_bgra(&data).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(px, [3, 2, 1, 255, 6, 5, 4, 255]);
        assert!(ppm_to_bgra(b"P6\n2 1\n255\n\x01").is_none(), "short data");
        assert!(ppm_to_bgra(b"P5\n1 1\n255\n\x01").is_none());
    }

    const MONITORS: &str = r#"[
        {"id":0,"name":"DP-1","x":0,"y":0,"scale":1.0,"focused":false,"activeWorkspace":{"id":1}},
        {"id":1,"name":"HDMI-A-1","x":2560,"y":100,"scale":1.5,"focused":true,"activeWorkspace":{"id":4}}
    ]"#;

    #[test]
    fn monitors_and_their_windows_map_to_shot_pixels() {
        let all = parse_monitors(MONITORS);
        assert_eq!(all.len(), 2);
        let mon = &all[1];
        assert_eq!(
            (mon.name.as_str(), mon.focused, mon.workspace),
            ("HDMI-A-1", true, 4)
        );
        let clients = r#"[
            {"mapped":true,"hidden":false,"at":[2600,120],"size":[800,600],
             "workspace":{"id":4},"class":"firefox","focusHistoryID":2},
            {"mapped":true,"hidden":false,"at":[2700,200],"size":[100,50],
             "workspace":{"id":4},"class":"kitty","focusHistoryID":0},
            {"mapped":true,"hidden":false,"at":[0,0],"size":[100,100],
             "workspace":{"id":1},"class":"other","focusHistoryID":1},
            {"mapped":false,"hidden":false,"at":[2600,120],"size":[9,9],
             "workspace":{"id":4},"class":"gone","focusHistoryID":3}
        ]"#;
        let got = parse_clients(clients, mon);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].1, "kitty", "the focused window is first");
        let r = got[1].0;
        assert_eq!((r.left, r.top, r.width, r.height), (60, 30, 1200, 900));
    }

    #[test]
    fn geometry_is_in_layout_units() {
        let area = Rect {
            left: 150,
            top: 300,
            width: 600,
            height: 450,
        };
        assert_eq!(geometry(area, (2560, 100), 1.5), "2660,300 400x300");
    }

    #[test]
    fn broken_tool_output_gives_nothing() {
        assert!(parse_monitors("not json").is_empty());
        assert!(parse_clients("{}", &parse_monitors(MONITORS)[0]).is_empty());
    }

    #[test]
    fn recording_state_survives_the_file() {
        let run = Running {
            pid: 42,
            path: "/tmp/a.mp4".into(),
            gif: Some("/tmp/a.gif".into()),
        };
        assert_eq!(parse_state(&state_text(&run)), Some(run));
        let plain = Running {
            pid: 7,
            path: "/tmp/b.mp4".into(),
            gif: None,
        };
        assert_eq!(parse_state(&state_text(&plain)), Some(plain));
    }

    #[test]
    fn an_autostart_entry_names_the_exe() {
        let text = autostart_text(Path::new("/usr/bin/snapling"));
        assert!(text.contains("/usr/bin/snapling"));
    }
}
