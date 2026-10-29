#![cfg(windows)]

use snapling_core::hotkey::Hotkey;
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_HOTKEY_ALREADY_REGISTERED, GetLastError, GlobalFree, HANDLE,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIB;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, MB_ICONWARNING, MessageBoxW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use windows::core::{HSTRING, w};

pub mod overlay;
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

pub fn warn(text: &str) {
    unsafe { MessageBoxW(None, &HSTRING::from(text), w!("Snapling"), MB_ICONWARNING) };
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
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
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
        for px in bgra.chunks_exact_mut(4) {
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

/// CF_DIB, bottom-up rows as most apps expect.
pub fn copy_image(img: &Image) -> Result<(), String> {
    let header = dib_header(img.width, img.height as i32);
    let hsize = size_of::<BITMAPINFOHEADER>();
    let row = img.width as usize * 4;
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, hsize + img.bgra.len())
            .map_err(|e| format!("GlobalAlloc: {e}"))?;
        let dst = GlobalLock(mem) as *mut u8;
        if dst.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err("GlobalLock: null".into());
        }
        std::ptr::copy_nonoverlapping((&raw const header).cast::<u8>(), dst, hsize);
        for (i, src) in img.bgra.chunks_exact(row).rev().enumerate() {
            std::ptr::copy_nonoverlapping(src.as_ptr(), dst.add(hsize + i * row), row);
        }
        let _ = GlobalUnlock(mem);

        if let Err(e) = OpenClipboard(None) {
            let _ = GlobalFree(Some(mem));
            return Err(format!("OpenClipboard: {e}"));
        }
        let res =
            EmptyClipboard().and_then(|_| SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(mem.0))));
        let _ = CloseClipboard();
        if let Err(e) = res {
            let _ = GlobalFree(Some(mem));
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
