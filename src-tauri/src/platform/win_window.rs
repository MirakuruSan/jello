#[cfg(target_os = "windows")]
pub fn apply_acrylic(window: &tauri::WebviewWindow) {
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW};
    use windows::Win32::Foundation::HWND;

    if let Ok(hwnd) = window.hwnd() {
        let hwnd_raw = HWND(hwnd.0);
        let value = DWMSBT_TRANSIENTWINDOW.0; // Acrylic backdrop type (3)
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd_raw,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &value as *const i32 as *const _,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn apply_acrylic(_window: &tauri::WebviewWindow) {}

// (set_hwnd_click_through removed: WS_EX_TRANSPARENT on the overlay HOST does
// not affect the Chromium child windows inside it — pass-through is done with
// SetWindowRgn in app::apply_overlay_region instead.)

#[cfg(target_os = "windows")]
unsafe extern "system" fn window_subclass_proc(
    hwnd: windows::Win32::Foundation::HWND,
    umsg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _uidsubclass: usize,
    _dwrefdata: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        WM_NCHITTEST, HTLEFT, HTRIGHT, HTTOP, HTBOTTOM, HTTOPLEFT, HTTOPRIGHT, HTBOTTOMLEFT, HTBOTTOMRIGHT,
        GetWindowRect
    };
    use windows::Win32::UI::Shell::DefSubclassProc;
    use windows::Win32::Foundation::LRESULT;

    if umsg == WM_NCHITTEST {
        let x = (lparam.0 & 0xffff) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;

        let mut rect = windows::Win32::Foundation::RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            
            let local_x = x - rect.left;
            let local_y = y - rect.top;

            const BORDER: i32 = 8;
            let on_top = local_y < BORDER;
            let on_bottom = local_y >= height - BORDER;
            let on_left = local_x < BORDER;
            let on_right = local_x >= width - BORDER;

            if on_top && on_left { return LRESULT(HTTOPLEFT as isize); }
            if on_top && on_right { return LRESULT(HTTOPRIGHT as isize); }
            if on_bottom && on_left { return LRESULT(HTBOTTOMLEFT as isize); }
            if on_bottom && on_right { return LRESULT(HTBOTTOMRIGHT as isize); }
            if on_top { return LRESULT(HTTOP as isize); }
            if on_bottom { return LRESULT(HTBOTTOM as isize); }
            if on_left { return LRESULT(HTLEFT as isize); }
            if on_right { return LRESULT(HTRIGHT as isize); }
        }
    }
    DefSubclassProc(hwnd, umsg, wparam, lparam)
}

#[cfg(target_os = "windows")]
pub fn install_resize_subclass(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::Shell::SetWindowSubclass;
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(window_subclass_proc), 101, 0);
    }
}

#[cfg(not(target_os = "windows"))]
pub fn install_resize_subclass(_hwnd: tauri::window::Hwnd) {}
