#![cfg(windows)]

use snapling_core::hotkey::{self, Hotkey};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_HOTKEY_ALREADY_REGISTERED,
    GetLastError, GlobalFree, HANDLE, HGLOBAL, HWND,
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
use windows::Win32::System::Threading::{CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
    RegisterHotKey, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileOpenDialog,
    IFileSaveDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, IDYES, MB_ICONQUESTION, MB_ICONWARNING, MB_YESNO, MessageBoxW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use windows::core::{HSTRING, w};

pub mod ocr;
pub mod overlay;
pub mod secret;
pub mod targets;
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

const HOTKEY_PAUSE: windows::core::PCWSTR = w!("Local\\Snapling.HotkeyRecording");
static PAUSE: AtomicIsize = AtomicIsize::new(0);

/// While the settings window records a combo, the tray must not act on it.
/// A named mutex: it goes away by itself if that window crashes.
pub fn pause_hotkeys(on: bool) {
    let old = PAUSE.swap(0, Ordering::Relaxed);
    if old != 0 {
        let _ = unsafe { CloseHandle(HANDLE(old as _)) };
    }
    if on && let Ok(h) = unsafe { CreateMutexW(None, false, HOTKEY_PAUSE) } {
        PAUSE.store(h.0 as isize, Ordering::Relaxed);
    }
}

pub fn hotkeys_paused() -> bool {
    match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, HOTKEY_PAUSE) } {
        Ok(h) => {
            let _ = unsafe { CloseHandle(h) };
            true
        }
        Err(_) => false,
    }
}

/// The combo held right now, for recording. PrintScreen may report only its "pressed since
/// last call" bit, so the first call clears stale bits and its result is to be ignored.
pub fn pressed_combo() -> Option<String> {
    let state = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) } as u16;
    let down = |vk: u16| state(vk) & 0x8000 != 0;
    let key = hotkey::key_names()
        .filter(|k| vk_code(k).is_some_and(|vk| state(vk as u16) & 0x8001 != 0))
        .last()?;
    let hk = Hotkey {
        ctrl: down(VK_CONTROL.0),
        alt: down(VK_MENU.0),
        shift: down(VK_SHIFT.0),
        win: down(VK_LWIN.0) || down(VK_RWIN.0),
        key,
    };
    Some(hk.to_string())
}

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

pub fn autostart() -> bool {
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

pub fn set_autostart(on: bool) -> Result<(), String> {
    let fail = |e| format!("автозапуск: {e:?}");
    unsafe {
        if !on {
            let e = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Snapling"));
            return if e.is_ok() || e == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(fail(e))
            };
        }
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
    open_dialog(true)
}

/// Native picker of one WAV file; `None` when cancelled.
pub fn pick_wav() -> Option<String> {
    open_dialog(false)
}

fn open_dialog(folder: bool) -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        if folder {
            dlg.SetOptions(dlg.GetOptions().ok()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
                .ok()?;
        } else {
            dlg.SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("WAV"),
                pszSpec: w!("*.wav"),
            }])
            .ok()?;
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

/// Native «Сохранить как»; the file type comes from the extension the user picks.
pub fn save_file_dialog(name: &str) -> Option<std::path::PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let types = [
            (w!("PNG"), w!("*.png")),
            (w!("WebP"), w!("*.webp")),
            (w!("JPEG"), w!("*.jpg")),
        ]
        .map(|(name, spec)| COMDLG_FILTERSPEC {
            pszName: name,
            pszSpec: spec,
        });
        dlg.SetFileTypes(&types).ok()?;
        dlg.SetDefaultExtension(w!("png")).ok()?;
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

pub fn register_hotkey(id: i32, hk: &Hotkey) -> Result<(), String> {
    let vk = vk_code(&hk.key).ok_or_else(|| format!("{hk}: клавиша не поддерживается"))?;
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
            format!(
                "{hk} занято другой программой или Windows (какой именно — Windows не сообщает)"
            )
        } else {
            format!("{hk} не зарегистрировано: {e}")
        }
    })
}

pub fn unregister_hotkey(id: i32) {
    let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey(None, id) };
}

/// Windows 11 «Use the Print screen key to open screen capture»: on unless explicitly 0.
/// Read-only; Snapling never changes this setting.
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
        _ => return None,
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

/// All monitors as one image (virtual screen), GDI BitBlt.
/// ponytail: GDI gives black frames for exclusive-fullscreen games; WGC/DXGI in stage 3.
pub fn capture_virtual_screen() -> Result<Image, String> {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if w <= 0 || h <= 0 {
            return Err("пустой виртуальный экран".into());
        }
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let blt = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
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
            return Err(format!("GetDIBits: {lines} из {h} строк"));
        }
        for px in bgra.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Ok(Image {
            x,
            y,
            width: w as u32,
            height: h as u32,
            bgra,
        })
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
    fn vk_codes() {
        assert_eq!(vk_code("A"), Some(0x41));
        assert_eq!(vk_code("7"), Some(0x37));
        assert_eq!(vk_code("F1"), Some(0x70));
        assert_eq!(vk_code("F24"), Some(0x87));
        assert_eq!(vk_code("PrintScreen"), Some(0x2C));
        assert_eq!(vk_code("Nope"), None);
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
