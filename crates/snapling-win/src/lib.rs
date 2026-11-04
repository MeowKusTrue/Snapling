#![cfg(windows)]

use snapling_core::hotkey::{self, Hotkey};
use snapling_core::i18n::{t, tf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_HOTKEY_ALREADY_REGISTERED, GetLastError,
    GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteKeyValueW,
    RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileOpenDialog,
    IFileSaveDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetSystemMetrics, IDYES, KBDLLHOOKSTRUCT, MB_ICONQUESTION, MB_ICONWARNING,
    MB_YESNO, MSLLHOOKSTRUCT, MessageBoxW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WM_XBUTTONUP,
};
use windows::core::{HSTRING, PCWSTR, w};

pub mod audio;
pub mod countdown;
pub mod dup;
pub mod menu;
pub mod ocr;
pub mod overlay;
pub mod pin;
pub mod rec_ui;
pub mod record;
pub mod secret;
pub mod targets;
pub mod toast;
pub mod uia;

/// Top-down BGRA, alpha forced to 255. `x`/`y`: screen position of the top-left pixel.
pub struct Image {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl Image {
    /// Copy of `r` (image coordinates), clipped to the image.
    pub fn crop(&self, r: snapling_core::selection::Rect) -> Option<Image> {
        crop_bgra(&self.bgra, self.width, self.height, (self.x, self.y), r)
    }
}

pub(crate) fn crop_bgra(
    src: &[u8],
    width: u32,
    height: u32,
    origin: (i32, i32),
    r: snapling_core::selection::Rect,
) -> Option<Image> {
    let l = r.left.clamp(0, width as i32) as usize;
    let t = r.top.clamp(0, height as i32) as usize;
    let rr = r.right().clamp(0, width as i32) as usize;
    let b = r.bottom().clamp(0, height as i32) as usize;
    if rr <= l || b <= t {
        return None;
    }
    let stride = width as usize * 4;
    let mut bgra = Vec::with_capacity((rr - l) * (b - t) * 4);
    for row in src.chunks_exact(stride).skip(t).take(b - t) {
        bgra.extend_from_slice(&row[l * 4..rr * 4]);
    }
    Some(Image {
        x: origin.0 + l as i32,
        y: origin.1 + t as i32,
        width: (rr - l) as u32,
        height: (b - t) as u32,
        bgra,
    })
}

pub fn set_dpi_aware() {
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// False when another Snapling instance already runs in this session.
pub fn single_instance() -> bool {
    let handle = unsafe { CreateMutexW(None, true, w!("Local\\Snapling.SingleInstance")) };
    // Leaked on purpose: the mutex must live as long as the process.
    handle.is_ok() && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
}

/// The window belongs to a `snapling.exe`, not to a folder or an editor named the same.
fn from_snapling(hwnd: HWND) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    use windows::core::PWSTR;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return false;
    };
    let mut path = [0u16; 512];
    let mut len = path.len() as u32;
    let read = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(path.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    read.is_ok()
        && String::from_utf16_lossy(&path[..len as usize])
            .to_lowercase()
            .ends_with("snapling.exe")
}

/// Brings an open Snapling window to the front; `false` when none is on screen.
pub fn focus_app_window() -> bool {
    use windows::Win32::Foundation::{LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, IsIconic, IsWindowVisible, SW_RESTORE, SetForegroundWindow,
        ShowWindow,
    };
    use windows::core::BOOL;
    extern "system" fn check(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut title = [0u16; 16];
        let len = unsafe { GetWindowTextW(hwnd, &mut title) } as usize;
        let ours = String::from_utf16_lossy(&title[..len]) == "Snapling";
        if !ours || !unsafe { IsWindowVisible(hwnd) }.as_bool() || !from_snapling(hwnd) {
            return TRUE;
        }
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
            *(found.0 as *mut bool) = true;
        }
        BOOL(0)
    }
    let mut found = false;
    let _ = unsafe { EnumWindows(Some(check), LPARAM((&raw mut found) as isize)) };
    found
}

pub const WM_SHOW: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 22;
const SHOW_EVENT: windows::core::PCWSTR = w!("Local\\Snapling.Show");

/// Second launch: asks the running instance to open its window. `false` when none listens.
pub fn signal_running() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    let Ok(event) = (unsafe { OpenEventW(EVENT_MODIFY_STATE, false, SHOW_EVENT) }) else {
        return false;
    };
    let ok = unsafe { SetEvent(event) }.is_ok();
    let _ = unsafe { CloseHandle(event) };
    ok
}

/// Posts `WM_SHOW` to the calling thread whenever a second launch signals.
pub fn watch_show() {
    use windows::Win32::System::Threading::{
        CreateEventW, GetCurrentThreadId, INFINITE, WaitForSingleObject,
    };
    use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
    let Ok(event) = (unsafe { CreateEventW(None, false, false, SHOW_EVENT) }) else {
        return;
    };
    let event = event.0 as isize;
    let thread = unsafe { GetCurrentThreadId() };
    std::thread::spawn(move || {
        let event = HANDLE(event as *mut _);
        while unsafe { WaitForSingleObject(event, INFINITE) }.0 == 0 {
            let _ = unsafe { PostThreadMessageW(thread, WM_SHOW, WPARAM(0), LPARAM(0)) };
        }
    });
}

/// Thread that holds the recording hooks, 0 when none.
static RECORD_THREAD: AtomicU32 = AtomicU32::new(0);
static RECORD_MODS: AtomicU8 = AtomicU8::new(0);
static RECORDED: Mutex<Option<String>> = Mutex::new(None);

/// Bit of a modifier key in `RECORD_MODS`.
fn modifier_bit(vk: u32) -> Option<u8> {
    match vk {
        0xA0 | 0xA1 | 0x10 => Some(1),
        0xA2 | 0xA3 | 0x11 => Some(2),
        0xA4 | 0xA5 | 0x12 => Some(4),
        0x5B | 0x5C => Some(8),
        _ => None,
    }
}

/// Any key but a modifier: by name when it has one, else by its code.
fn recorded_hotkey(vk: u32, mods: u8) -> Option<Hotkey> {
    if vk == 0 || vk > 0xFE || modifier_bit(vk).is_some() {
        return None;
    }
    let key = hotkey::key_names()
        .find(|k| vk_code(k) == Some(vk))
        .unwrap_or_else(|| format!("Vk{vk:02X}"));
    Some(Hotkey {
        shift: mods & 1 != 0,
        ctrl: mods & 2 != 0,
        alt: mods & 4 != 0,
        win: mods & 8 != 0,
        key,
    })
}

/// Sees every key before Windows and other programs do, and keeps presses from them: Win+…,
/// PrintScreen and combos other programs own can be recorded without firing. Releases pass.
unsafe extern "system" fn record_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let vk = unsafe { (*(lp.0 as *const KBDLLHOOKSTRUCT)).vkCode };
        let down = wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN;
        match modifier_bit(vk) {
            Some(bit) if down => {
                RECORD_MODS.fetch_or(bit, Ordering::Relaxed);
            }
            Some(bit) => {
                RECORD_MODS.fetch_and(!bit, Ordering::Relaxed);
            }
            None if down => {
                let hk = recorded_hotkey(vk, RECORD_MODS.load(Ordering::Relaxed));
                if let (Some(hk), Ok(mut slot)) = (hk, RECORDED.lock()) {
                    *slot = Some(hk.to_string());
                }
            }
            None => {}
        }
        if down {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// `VK_XBUTTON1`/`VK_XBUTTON2` of a side-button message, `None` for other mouse input.
fn side_button(msg: u32, lp: LPARAM) -> Option<u32> {
    if msg != WM_XBUTTONDOWN && msg != WM_XBUTTONUP {
        return None;
    }
    let data = unsafe { (*(lp.0 as *const MSLLHOOKSTRUCT)).mouseData };
    match data >> 16 {
        1 => Some(0x05),
        2 => Some(0x06),
        _ => None,
    }
}

/// Side mouse buttons while recording; they are kept from other programs like keys.
unsafe extern "system" fn record_mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0
        && let Some(vk) = side_button(wp.0 as u32, lp)
    {
        if wp.0 as u32 == WM_XBUTTONDOWN {
            let hk = recorded_hotkey(vk, RECORD_MODS.load(Ordering::Relaxed));
            if let (Some(hk), Ok(mut slot)) = (hk, RECORDED.lock()) {
                *slot = Some(hk.to_string());
            }
        }
        return LRESULT(1);
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// The recording helper process, while one runs.
static RECORD_CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);

/// Starts or stops catching the keyboard and side mouse buttons for hotkey recording, in a
/// `snapling --record-key` process: in the settings window's own process Windows never called
/// the hooks, while the same hooks in a process of their own see every key, Win and PrtScn too.
pub fn record_keys(on: bool) {
    use std::io::BufRead;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    if let Some(mut child) = RECORD_CHILD.lock().ok().and_then(|mut c| c.take()) {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Ok(mut slot) = RECORDED.lock() {
        *slot = None;
    }
    if !on {
        return;
    }
    const NO_WINDOW: u32 = 0x0800_0000;
    let spawned = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .arg("--record-key")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .creation_flags(NO_WINDOW)
            .spawn()
    });
    let Ok(mut child) = spawned else {
        return;
    };
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut line = String::new();
            if std::io::BufReader::new(out).read_line(&mut line).is_ok()
                && !line.trim().is_empty()
                && let Ok(mut slot) = RECORDED.lock()
            {
                *slot = Some(line.trim().to_string());
            }
        });
    }
    if let Ok(mut c) = RECORD_CHILD.lock() {
        *c = Some(child);
    }
}

/// `snapling --record-key`: hooks the keyboard and side buttons, prints the first combo and
/// exits. Also exits when the parent goes (its end of stdin closes) or after half a minute, so
/// the keyboard is never held for long.
pub fn record_key_process() {
    use std::io::Read;
    std::thread::spawn(|| {
        let _ = std::io::stdin().read_to_end(&mut Vec::new());
        std::process::exit(0);
    });
    hook_here(true);
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(30) {
        if let Some(combo) = recorded_combo() {
            println!("{combo}");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    hook_here(false);
}

/// The recording hooks on a thread of this process with a plain message loop: Windows calls a
/// low-level hook through the installing thread's queue.
fn hook_here(on: bool) {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, PostThreadMessageW, WM_QUIT};
    let old = RECORD_THREAD.swap(0, Ordering::Relaxed);
    if old != 0 {
        let _ = unsafe { PostThreadMessageW(old, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
    RECORD_MODS.store(0, Ordering::Relaxed);
    if let Ok(mut slot) = RECORDED.lock() {
        *slot = None;
    }
    if !on {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        let keys = SetWindowsHookExW(WH_KEYBOARD_LL, Some(record_proc), None, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(record_mouse_proc), None, 0);
        let _ = tx.send(GetCurrentThreadId());
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        for h in [keys, mouse].into_iter().flatten() {
            let _ = UnhookWindowsHookEx(h);
        }
    });
    if let Ok(id) = rx.recv() {
        RECORD_THREAD.store(id, Ordering::Relaxed);
    }
}

pub fn recorded_combo() -> Option<String> {
    RECORDED.lock().ok()?.take()
}

/// Whether Windows would give this combo to us right now. The tray's own combos read as taken,
/// so check for those first.
pub fn hotkey_free(hk: &Hotkey) -> bool {
    const PROBE: i32 = 0x7FFF;
    if is_mouse(hk) {
        return true;
    }
    let free = register_hotkey(PROBE, hk).is_ok();
    if free {
        unregister_hotkey(PROBE);
    }
    free
}

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

const APPROVED_KEY: windows::core::PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");

fn has_run_entry() -> bool {
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            w!("Snapling"),
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        )
    }
    .is_ok()
}

/// Task Manager marks a switched-off entry with an odd first byte.
fn approved_off(value: &[u8]) -> bool {
    value.first().is_some_and(|b| b & 1 == 1)
}

/// Switched off in Task Manager or Windows settings; the Run entry is still there.
pub fn autostart_blocked() -> bool {
    let mut value = [0u8; 12];
    let mut size = value.len() as u32;
    let res = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            APPROVED_KEY,
            w!("Snapling"),
            windows::Win32::System::Registry::RRF_RT_REG_BINARY,
            None,
            Some(value.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    has_run_entry() && res.is_ok() && approved_off(&value[..size as usize])
}

/// Name of the logon task. Explorer holds the Run entries back after a logon, a logon task
/// starts at once; the Run entry stays as the fallback where the task is refused.
const TASK: &str = "Snapling";

fn schtasks(args: &[&std::ffi::OsStr]) -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const NO_WINDOW: u32 = 0x0800_0000;
    Command::new("schtasks")
        .args(args)
        .creation_flags(NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn has_task() -> bool {
    schtasks(&["/Query".as_ref(), "/TN".as_ref(), TASK.as_ref()])
}

/// The task definition: no delay, normal priority, no time limit, on battery too.
fn task_xml(exe: &str, user: &str) -> String {
    let safe = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let (exe, user) = (safe(exe), safe(user));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// Creates the logon task for this exe, or points the existing one at it.
fn write_task() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let var = |name| std::env::var(name).unwrap_or_default();
    let user = format!("{}\\{}", var("USERDOMAIN"), var("USERNAME"));
    let xml = task_xml(&exe.display().to_string(), &user);
    let bytes: Vec<u8> = [0xFEFF]
        .into_iter()
        .chain(xml.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    let file = std::env::temp_dir().join("snapling-autostart.xml");
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let made = schtasks(&[
        "/Create".as_ref(),
        "/TN".as_ref(),
        TASK.as_ref(),
        "/XML".as_ref(),
        file.as_ref(),
        "/F".as_ref(),
    ]);
    let _ = std::fs::remove_file(&file);
    made.then_some(()).ok_or_else(|| "schtasks".to_string())
}

fn delete_run_entry() -> Result<(), String> {
    let e = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Snapling")) };
    if e.is_ok() || e == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!("автозапуск: {e:?}"))
    }
}

pub fn autostart() -> bool {
    has_task() || (has_run_entry() && !autostart_blocked())
}

/// Keeps autostart on this exe after a move or an update, and moves a Run entry to the task.
/// A second start at logon would open a window, so both never stay. On its own thread: the
/// tray does not wait for `schtasks`.
pub fn refresh_autostart() {
    std::thread::spawn(|| {
        let wanted = has_task() || (has_run_entry() && !autostart_blocked());
        if wanted && write_task().is_ok() {
            let _ = delete_run_entry();
        } else if has_run_entry() {
            let _ = write_run_entry();
        }
    });
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    if !on {
        schtasks(&[
            "/Delete".as_ref(),
            "/TN".as_ref(),
            TASK.as_ref(),
            "/F".as_ref(),
        ]);
        return delete_run_entry();
    }
    // Lifts a block from Task Manager: the user has just asked for autostart here.
    let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, APPROVED_KEY, w!("Snapling")) };
    if write_task().is_ok() {
        return delete_run_entry();
    }
    write_run_entry()
}

fn write_run_entry() -> Result<(), String> {
    let fail = |e| format!("автозапуск: {e:?}");
    unsafe {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let value: Vec<u16> = format!("\"{}\"", exe.display())
            .encode_utf16()
            .chain([0])
            .collect();
        let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2);
        let mut key = HKEY::default();
        let e = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key);
        if e.is_err() {
            return Err(fail(e));
        }
        let e = RegSetValueExW(key, w!("Snapling"), None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        if e.is_err() {
            return Err(fail(e));
        }
    }
    Ok(())
}

/// Native folder picker; `None` when cancelled.
pub fn pick_folder() -> Option<String> {
    open_dialog(None)
}

/// Native picker of one WAV file; `None` when cancelled.
pub fn pick_wav() -> Option<String> {
    open_dialog(Some((w!("WAV"), w!("*.wav"))))
}

/// Native picker of a settings file; `None` when cancelled.
pub fn pick_toml() -> Option<String> {
    open_dialog(Some((w!("TOML"), w!("*.toml"))))
}

/// `kind`: name and mask of the file type, `None` picks a folder.
fn open_dialog(kind: Option<(PCWSTR, PCWSTR)>) -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        match kind {
            None => dlg
                .SetOptions(dlg.GetOptions().ok()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
                .ok()?,
            Some((name, spec)) => dlg
                .SetFileTypes(&[COMDLG_FILTERSPEC {
                    pszName: name,
                    pszSpec: spec,
                }])
                .ok()?,
        }
        dlg.Show(None).ok()?;
        let path = dlg
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text
    }
}

/// Language of the Windows interface, as a Snapling language code.
pub fn ui_language() -> &'static str {
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    // Low 10 bits are the primary language; 0x19 is Russian.
    if id & 0x3FF == 0x19 { "ru" } else { "en" }
}

/// Taskbar and tray use the light theme.
pub fn taskbar_is_light() -> bool {
    use windows::Win32::System::Registry::RRF_RT_REG_DWORD;
    let mut value = 0u32;
    let mut size = 4u32;
    let res = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    res.is_ok() && value == 1
}

/// Native «Сохранить как»; the file type comes from the extension the user picks.
pub fn save_file_dialog(name: &str) -> Option<std::path::PathBuf> {
    let types = [
        (w!("PNG"), w!("*.png")),
        (w!("WebP"), w!("*.webp")),
        (w!("JPEG"), w!("*.jpg")),
    ];
    save_dialog(name, &types, w!("png"))
}

/// Where to export the settings; `None` when cancelled.
pub fn save_toml() -> Option<std::path::PathBuf> {
    let types = [(w!("TOML"), w!("*.toml"))];
    save_dialog("snapling-settings.toml", &types, w!("toml"))
}

fn save_dialog(
    name: &str,
    types: &[(PCWSTR, PCWSTR)],
    extension: PCWSTR,
) -> Option<std::path::PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let types: Vec<COMDLG_FILTERSPEC> = types
            .iter()
            .map(|&(name, spec)| COMDLG_FILTERSPEC {
                pszName: name,
                pszSpec: spec,
            })
            .collect();
        dlg.SetFileTypes(&types).ok()?;
        dlg.SetDefaultExtension(extension).ok()?;
        dlg.SetFileName(&HSTRING::from(name)).ok()?;
        dlg.Show(None).ok()?;
        let path = dlg
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text.map(Into::into)
    }
}

/// A borderless window keeps the Windows 11 look: rounded corners and the system shadow.
pub fn style_window(hwnd: isize) {
    let hwnd = HWND(hwnd as _);
    let round = DWMWCP_ROUND;
    let margins = MARGINS {
        cxLeftWidth: 1,
        cxRightWidth: 1,
        cyTopHeight: 1,
        cyBottomHeight: 1,
    };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const round).cast(),
            size_of_val(&round) as u32,
        );
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

pub fn warn(text: &str) {
    unsafe { MessageBoxW(None, &HSTRING::from(text), w!("Snapling"), MB_ICONWARNING) };
}

/// Yes/No question; `true` = Yes.
pub fn ask(text: &str) -> bool {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            w!("Snapling"),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

fn is_mouse(hk: &Hotkey) -> bool {
    hk.key.starts_with("Mouse")
}

/// Modifier bits as `RECORD_MODS` keeps them, read from the keyboard state now.
fn held_mods() -> u8 {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } < 0;
    u8::from(down(0x10))
        | u8::from(down(0x11)) << 1
        | u8::from(down(0x12)) << 2
        | u8::from(down(0x5B) || down(0x5C)) << 3
}

/// Side-button bindings of the tray: id, modifier bits, button.
static MOUSE_BINDS: Mutex<Vec<(i32, u8, u32)>> = Mutex::new(Vec::new());
/// Thread that gets `WM_HOTKEY` for a bound side button: the one that registered it (the tray).
static MOUSE_TARGET: AtomicU32 = AtomicU32::new(0);
static MOUSE_HOOKED: AtomicBool = AtomicBool::new(false);

/// A bound side button posts `WM_HOTKEY` to the tray thread and is kept from other programs;
/// unbound ones pass.
unsafe extern "system" fn bound_mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0
        && let Some(vk) = side_button(wp.0 as u32, lp)
    {
        let mods = held_mods();
        let id = MOUSE_BINDS.lock().ok().and_then(|b| {
            b.iter()
                .find(|(_, m, v)| *m == mods && *v == vk)
                .map(|b| b.0)
        });
        if let Some(id) = id {
            if wp.0 as u32 == WM_XBUTTONDOWN {
                use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_HOTKEY};
                let thread = MOUSE_TARGET.load(Ordering::Relaxed);
                let _ = unsafe {
                    PostThreadMessageW(thread, WM_HOTKEY, WPARAM(id as usize), LPARAM(0))
                };
            }
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

fn register_mouse(id: i32, hk: &Hotkey) -> Result<(), String> {
    let vk =
        vk_code(&hk.key).ok_or_else(|| tf("{hk}: клавиша не поддерживается", &[("hk", &hk)]))?;
    let mods =
        u8::from(hk.shift) | u8::from(hk.ctrl) << 1 | u8::from(hk.alt) << 2 | u8::from(hk.win) << 3;
    if let Ok(mut binds) = MOUSE_BINDS.lock() {
        binds.retain(|b| b.0 != id);
        binds.push((id, mods, vk));
    }
    use windows::Win32::System::Threading::GetCurrentThreadId;
    MOUSE_TARGET.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    // Its own thread and message loop, so a busy tray (a capture, OCR) never delays the hook:
    // Windows removes a low-level hook that answers late. It stays for the life of the process;
    // with no bindings it lets every click through.
    if !MOUSE_HOOKED.swap(true, Ordering::Relaxed) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG};
            let hooked = SetWindowsHookExW(WH_MOUSE_LL, Some(bound_mouse_proc), None, 0);
            let _ = tx.send(hooked.as_ref().map(|_| ()).map_err(ToString::to_string));
            let mut msg = MSG::default();
            while hooked.is_ok() && GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        });
        if let Ok(Err(e)) = rx.recv() {
            MOUSE_HOOKED.store(false, Ordering::Relaxed);
            return Err(tf(
                "{hk} не зарегистрировано: {e}",
                &[("hk", &hk), ("e", &e)],
            ));
        }
    }
    Ok(())
}

pub fn register_hotkey(id: i32, hk: &Hotkey) -> Result<(), String> {
    if is_mouse(hk) {
        return register_mouse(id, hk);
    }
    let vk =
        vk_code(&hk.key).ok_or_else(|| tf("{hk}: клавиша не поддерживается", &[("hk", &hk)]))?;
    let mut mods = MOD_NOREPEAT;
    for (on, m) in [
        (hk.ctrl, MOD_CONTROL),
        (hk.alt, MOD_ALT),
        (hk.shift, MOD_SHIFT),
        (hk.win, MOD_WIN),
    ] {
        if on {
            mods |= m;
        }
    }
    unsafe { RegisterHotKey(None, id, HOT_KEY_MODIFIERS(mods.0), vk) }.map_err(|e| {
        if e.code() == ERROR_HOTKEY_ALREADY_REGISTERED.to_hresult() {
            tf(
                "{hk} занято другой программой или Windows (какой именно — Windows не сообщает)",
                &[("hk", &hk)],
            )
        } else {
            tf("{hk} не зарегистрировано: {e}", &[("hk", &hk), ("e", &e)])
        }
    })
}

pub fn unregister_hotkey(id: i32) {
    if let Ok(mut binds) = MOUSE_BINDS.lock() {
        binds.retain(|b| b.0 != id);
    }
    let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey(None, id) };
}

/// Windows 11 «Use the Print screen key to open screen capture»: on unless explicitly 0.
pub fn snipping_owns_printscreen() -> bool {
    use windows::Win32::System::Registry::RRF_RT_REG_DWORD;
    let mut value = 0u32;
    let mut size = 4u32;
    let res = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\Keyboard"),
            w!("PrintScreenKeyForSnippingEnabled"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    res.is_err() || value != 0
}

/// Moves files to the Recycle Bin, quietly: this runs by itself, not on a click.
pub fn to_recycle_bin(paths: &[String]) -> Result<(), String> {
    use windows::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NO_UI, SHFILEOPSTRUCTW, SHFileOperationW,
    };
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.replace('/', "\\").encode_utf16());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NO_UI).0 as u16,
        ..Default::default()
    };
    match unsafe { SHFileOperationW(&mut op) } {
        0 => Ok(()),
        e => Err(format!("SHFileOperationW: {e:#x}")),
    }
}

/// Gives PrtScn back, on the user's click. The value is per user: no elevation needed.
pub fn free_printscreen() -> Result<(), String> {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Registry::REG_DWORD;
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    let fail = |e| format!("PrtScn: {e:?}");
    unsafe {
        let mut key = HKEY::default();
        let e = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\Keyboard"),
            None,
            KEY_SET_VALUE,
            &mut key,
        );
        if e.is_err() {
            return Err(fail(e));
        }
        let off = 0u32.to_ne_bytes();
        let e = RegSetValueExW(
            key,
            w!("PrintScreenKeyForSnippingEnabled"),
            None,
            REG_DWORD,
            Some(&off),
        );
        let _ = RegCloseKey(key);
        if e.is_err() {
            return Err(fail(e));
        }
        let what = w!("Control Panel\\Keyboard");
        let _ = SendMessageTimeoutW(
            HWND(HWND_BROADCAST.0),
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(what.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            500,
            None,
        );
    }
    Ok(())
}

fn vk_code(key: &str) -> Option<u32> {
    let b = key.as_bytes();
    if b.len() == 1 && b[0].is_ascii_alphanumeric() {
        return Some(b[0].to_ascii_uppercase() as u32);
    }
    if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then_some(0x6F + n);
    }
    Some(match key {
        "PrintScreen" => 0x2C,
        "Space" => 0x20,
        "Tab" => 0x09,
        "Enter" => 0x0D,
        "Escape" => 0x1B,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        "Pause" => 0x13,
        "ScrollLock" => 0x91,
        "Mouse4" => 0x05,
        "Mouse5" => 0x06,
        "Backspace" => 0x08,
        "CapsLock" => 0x14,
        "NumLock" => 0x90,
        "Apps" => 0x5D,
        "NumpadMultiply" => 0x6A,
        "NumpadAdd" => 0x6B,
        "NumpadSubtract" => 0x6D,
        "NumpadDecimal" => 0x6E,
        "NumpadDivide" => 0x6F,
        "Semicolon" => 0xBA,
        "Equal" => 0xBB,
        "Comma" => 0xBC,
        "Minus" => 0xBD,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Backquote" => 0xC0,
        "BracketLeft" => 0xDB,
        "Backslash" => 0xDC,
        "BracketRight" => 0xDD,
        "Quote" => 0xDE,
        "VolumeMute" => 0xAD,
        "VolumeDown" => 0xAE,
        "VolumeUp" => 0xAF,
        "MediaNext" => 0xB0,
        "MediaPrev" => 0xB1,
        "MediaStop" => 0xB2,
        "MediaPlayPause" => 0xB3,
        k => {
            if let Some(n) = k.strip_prefix("Numpad").and_then(|n| n.parse::<u32>().ok()) {
                return (n <= 9).then_some(0x60 + n);
            }
            return k
                .strip_prefix("Vk")
                .and_then(|h| u32::from_str_radix(h, 16).ok());
        }
    })
}

pub(crate) fn dib_header(width: u32, height: i32) -> BITMAPINFOHEADER {
    BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    }
}

/// Client coordinates of a mouse message.
pub(crate) fn lparam_point(
    lp: windows::Win32::Foundation::LPARAM,
) -> snapling_core::selection::Point {
    snapling_core::selection::Point {
        x: (lp.0 & 0xFFFF) as u16 as i16 as i32,
        y: ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
    }
}

/// All monitors as one image (virtual screen), GDI BitBlt. GDI gives black frames for
/// exclusive-fullscreen games: `dup::overlay_foreground_monitor` covers those.
pub fn capture_virtual_screen() -> Result<Image, String> {
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if w <= 0 || h <= 0 {
        return Err(t("пустой виртуальный экран").into());
    }
    let bgra = grab(x, y, w, h, false)?;
    Ok(Image {
        x,
        y,
        width: w as u32,
        height: h as u32,
        bgra,
    })
}

/// Pointer into a memory DC whose origin is the desktop point (`x`, `y`).
unsafe fn draw_cursor(mem: windows::Win32::Graphics::Gdi::HDC, x: i32, y: i32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, ICONINFO,
    };
    let mut ci = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        if GetCursorInfo(&mut ci).is_err() || ci.flags.0 & CURSOR_SHOWING.0 == 0 {
            return;
        }
        let mut info = ICONINFO::default();
        if GetIconInfo(ci.hCursor.into(), &mut info).is_err() {
            return;
        }
        let px = ci.ptScreenPos.x - info.xHotspot as i32 - x;
        let py = ci.ptScreenPos.y - info.yHotspot as i32 - y;
        let _ = DrawIconEx(mem, px, py, ci.hCursor.into(), 0, 0, 0, None, DI_NORMAL);
        let _ = DeleteObject(info.hbmMask.into());
        let _ = DeleteObject(info.hbmColor.into());
    }
}

/// BGRA of a desktop area, opaque.
pub(crate) fn grab(x: i32, y: i32, w: i32, h: i32, cursor: bool) -> Result<Vec<u8>, String> {
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let blt = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
        if cursor {
            draw_cursor(mem, x, y);
        }
        SelectObject(mem, old);

        let mut bgra = vec![0u8; w as usize * h as usize * 4];
        let mut info = BITMAPINFO {
            bmiHeader: dib_header(w as u32, -h),
            ..Default::default()
        };
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);

        blt.map_err(|e| format!("BitBlt: {e}"))?;
        if lines != h {
            return Err(tf(
                "GetDIBits: {lines} из {h} строк",
                &[("lines", &lines), ("h", &h)],
            ));
        }
        for px in bgra.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Ok(bgra)
    }
}

/// CF_DIB, bottom-up rows as most apps expect. A freeform shot also goes as "PNG" with its
/// transparency; apps that read only the DIB get it on white.
pub fn copy_image(img: &Image) -> Result<(), String> {
    let header = dib_header(img.width, img.height as i32);
    let hsize = size_of::<BITMAPINFOHEADER>();
    let row = img.width as usize * 4;
    let cut = snapling_core::storage::has_cutout(&img.bgra);
    let dib = global(hsize + img.bgra.len(), |dst| unsafe {
        std::ptr::copy_nonoverlapping((&raw const header).cast::<u8>(), dst, hsize);
        for (i, src) in img.bgra.chunks_exact(row).rev().enumerate() {
            let out = dst.add(hsize + i * row);
            std::ptr::copy_nonoverlapping(src.as_ptr(), out, row);
            if cut {
                let line = std::slice::from_raw_parts_mut(out, row);
                for px in line.as_chunks_mut::<4>().0 {
                    if px[3] == 0 {
                        *px = [255; 4];
                    }
                }
            }
        }
    })?;
    let mut blocks = vec![(CF_DIB.0 as u32, dib)];
    if cut {
        let png = snapling_core::storage::encode_png(&img.bgra, img.width, img.height)?;
        let block = global(png.len(), |dst| unsafe {
            std::ptr::copy_nonoverlapping(png.as_ptr(), dst, png.len());
        })?;
        blocks.push((unsafe { RegisterClipboardFormatW(w!("PNG")) }, block));
    }
    put_clipboard(&blocks)
}

/// CF_HDROP: a file on the clipboard, pasted as an attachment.
pub fn copy_file(path: &std::path::Path) -> Result<(), String> {
    use windows::Win32::System::Ole::CF_HDROP;
    // DROPFILES: offset of the list, a point, two flags; the last one means wide names.
    let mut head = [0u8; 20];
    head[0] = 20;
    head[16] = 1;
    let name: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain([0, 0])
        .collect();
    let size = head.len() + name.len() * 2;
    let block = global(size, |dst| unsafe {
        std::ptr::copy_nonoverlapping(head.as_ptr(), dst, head.len());
        std::ptr::copy_nonoverlapping(name.as_ptr().cast::<u8>(), dst.add(20), name.len() * 2);
    })?;
    put_clipboard(&[(CF_HDROP.0 as u32, block)])
}

/// CF_UNICODETEXT.
pub fn copy_text(text: &str) -> Result<(), String> {
    let utf16: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let block = global(utf16.len() * 2, |dst| unsafe {
        std::ptr::copy_nonoverlapping(utf16.as_ptr().cast::<u8>(), dst, utf16.len() * 2);
    })?;
    put_clipboard(&[(CF_UNICODETEXT.0 as u32, block)])
}

/// Movable memory of `size` bytes written by `fill`.
fn global(size: usize, fill: impl FnOnce(*mut u8)) -> Result<HGLOBAL, String> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, size).map_err(|e| format!("GlobalAlloc: {e}"))?;
        let dst = GlobalLock(mem) as *mut u8;
        if dst.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err("GlobalLock: null".into());
        }
        fill(dst);
        let _ = GlobalUnlock(mem);
        Ok(mem)
    }
}

/// Replaces the clipboard with these blocks; it owns the ones it accepted.
fn put_clipboard(blocks: &[(u32, HGLOBAL)]) -> Result<(), String> {
    unsafe {
        let free = |from: usize| {
            for (_, mem) in &blocks[from..] {
                let _ = GlobalFree(Some(*mem));
            }
        };
        if let Err(e) = OpenClipboard(None) {
            free(0);
            return Err(format!("OpenClipboard: {e}"));
        }
        let mut res = EmptyClipboard().map_err(|e| e.to_string());
        let mut done = 0;
        for (format, mem) in blocks {
            if res.is_err() {
                break;
            }
            match SetClipboardData(*format, Some(HANDLE(mem.0))) {
                Ok(_) => done += 1,
                Err(e) => res = Err(e.to_string()),
            }
        }
        let _ = CloseClipboard();
        if let Err(e) = res {
            free(done);
            return Err(format!("SetClipboardData: {e}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logon_task_escapes_its_values() {
        let xml = task_xml(r"C:\A&B\snapling.exe", r"PC\me");
        assert!(xml.contains(r"<Command>C:\A&amp;B\snapling.exe</Command>"));
        assert_eq!(xml.matches(r"<UserId>PC\me</UserId>").count(), 2);
    }

    #[test]
    fn vk_codes() {
        assert_eq!(vk_code("A"), Some(0x41));
        assert_eq!(vk_code("7"), Some(0x37));
        assert_eq!(vk_code("F1"), Some(0x70));
        assert_eq!(vk_code("F24"), Some(0x87));
        assert_eq!(vk_code("PrintScreen"), Some(0x2C));
        assert_eq!(vk_code("Nope"), None);
    }

    #[test]
    fn recorded_keys_become_canonical_combos() {
        let bits = |vks: &[u32]| {
            vks.iter()
                .filter_map(|v| modifier_bit(*v))
                .fold(0, |a, b| a | b)
        };
        let hk = recorded_hotkey(0x2C, bits(&[0xA2, 0xA1])).unwrap();
        assert_eq!(hk.to_string(), "Ctrl+Shift+PrintScreen");
        let hk = recorded_hotkey(0x53, bits(&[0x5B])).unwrap();
        assert_eq!(hk.to_string(), "Win+S");
        assert_eq!(modifier_bit(0x41), None);
        assert!(recorded_hotkey(0xFF, 0).is_none());
        assert!(
            recorded_hotkey(0xA2, 0).is_none(),
            "a modifier alone is no key"
        );
        assert_eq!(recorded_hotkey(0xAF, 0).unwrap().to_string(), "VolumeUp");
        assert_eq!(recorded_hotkey(0x63, 0).unwrap().to_string(), "Numpad3");
        // A key without a name keeps its code and registers by it.
        let odd = recorded_hotkey(0xB7, 0).unwrap();
        assert_eq!(odd.to_string(), "VkB7");
        assert_eq!(Hotkey::parse("vkb7").unwrap(), odd);
        assert_eq!(vk_code(&odd.key), Some(0xB7));
    }

    #[test]
    fn task_manager_off_state_is_an_odd_first_byte() {
        assert!(approved_off(&[3, 0, 0, 0]));
        assert!(!approved_off(&[2, 0, 0, 0]));
        assert!(!approved_off(&[]));
    }

    #[test]
    fn taskbar_theme_reads_without_panicking() {
        let _ = taskbar_is_light();
    }

    #[test]
    fn every_parsable_key_has_vk() {
        for k in [
            "PrintScreen",
            "Space",
            "Tab",
            "Enter",
            "Escape",
            "Insert",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Up",
            "Down",
            "Left",
            "Right",
            "Pause",
            "ScrollLock",
            "Mouse4",
            "Mouse5",
        ] {
            let hk = Hotkey::parse(k).unwrap();
            assert!(vk_code(&hk.key).is_some(), "{k}");
        }
    }
}

#[cfg(test)]
mod live {
    #[test]
    #[ignore = "captures the real screen"]
    fn capture_timing() {
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let img = super::capture_virtual_screen().unwrap();
            println!(
                "{}x{} {} ms",
                img.width,
                img.height,
                t.elapsed().as_millis()
            );
            assert_eq!(img.bgra.len(), (img.width * img.height * 4) as usize);
        }
    }
}

/// Exclusive-fullscreen Direct3D app in front (game): the overlay would minimize it.
pub fn fullscreen_game() -> bool {
    use windows::Win32::UI::Shell::{QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN)
}

pub fn local_stamp() -> snapling_core::storage::Stamp {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    snapling_core::storage::Stamp {
        year: t.wYear,
        month: t.wMonth as u8,
        day: t.wDay as u8,
        hour: t.wHour as u8,
        minute: t.wMinute as u8,
        second: t.wSecond as u8,
    }
}

/// The user's Pictures folder (follows OneDrive/relocation), `None` if unavailable.
pub fn pictures_dir() -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Pictures, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.map(std::path::PathBuf::from)
    }
}

/// Executable name of the foreground window's process ("Discord", "firefox"), for `{app}`.
pub fn foreground_app_name() -> String {
    let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    exe_stem(hwnd)
}

/// Display name of the app owning `hwnd` (overlay hover pill): «firefox.exe» → «Firefox».
pub fn app_name_of(hwnd: isize) -> String {
    let stem = exe_stem(windows::Win32::Foundation::HWND(hwnd as *mut _));
    let mut c = stem.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn exe_stem(hwnd: windows::Win32::Foundation::HWND) -> String {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            h,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = windows::Win32::Foundation::CloseHandle(h);
        if ok.is_err() {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&full)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

/// Plays an in-memory WAV without blocking. `'static`: PlaySound keeps reading the buffer
/// after returning, so it must outlive playback.
pub fn play_wav(wav: &'static [u8]) {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    let _ = unsafe {
        PlaySoundW(
            windows::core::PCWSTR(wav.as_ptr().cast()),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        )
    };
}

#[cfg(test)]
mod record_probe {
    /// Recording catches keys while the thread that started it does not pump messages, like a
    /// busy egui frame. Sends real key presses (the hook keeps them from other programs).
    /// `cargo test -p snapling-win record_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn record_probe() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
            SendInput, VIRTUAL_KEY,
        };
        let key = |vk: u16, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    ..Default::default()
                },
            },
        };
        for vk in [0x74u16, 0x21, 0xAF] {
            super::hook_here(true);
            std::thread::spawn(move || unsafe {
                SendInput(&[key(vk, false), key(vk, true)], size_of::<INPUT>() as i32);
            });
            std::thread::sleep(std::time::Duration::from_millis(300));
            println!("{vk:#x} -> {:?}", super::recorded_combo());
            super::hook_here(false);
        }
    }
}
