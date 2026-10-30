//! Whether anyone can see Jello's main window. WebView2 keeps drawing a page
//! at the display's refresh rate when its window is completely covered by
//! other windows, on another virtual desktop, hidden to the tray or behind
//! the lock screen. Jello tracks those cases and hides its pages then, as it
//! does while minimized: they stop painting, a video stops decoding frames
//! nobody sees, and audio keeps playing.
//!
//! A thread of its own listens for window changes anywhere on the desktop
//! and checks the window shortly after they settle. Being seen again takes
//! effect at once; being covered only after a second, so dragging a window
//! across Jello does not flicker its pages.

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::OnceLock;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::Graphics::Gdi::{
        CombineRgn, CreateRectRgn, CreateRectRgnIndirect, DeleteObject, EnumDisplayMonitors, GetMonitorInfoW, GetWindowRgnBox, HDC,
        HMONITOR, HRGN, MONITORINFO, NULLREGION, RGN_AND, RGN_DIFF, RGN_ERROR, RGN_OR,
    };
    use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS};
    use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetAncestor, GetDesktopWindow, GetLayeredWindowAttributes, GetMessageW, GetWindow,
        GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, KillTimer, SetTimer,
        TranslateMessage, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_REORDER, EVENT_OBJECT_SHOW,
        EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_DESKTOPSWITCH, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
        EVENT_SYSTEM_MINIMIZESTART, GA_ROOT, GWL_EXSTYLE, GW_HWNDPREV, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA,
        LWA_COLORKEY, MSG, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WM_TIMER, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };

    /// Delay after the last window change before checking.
    const SETTLE_MS: u32 = 60;
    /// How long the window must stay covered before its pages are hidden.
    const CONFIRM_MS: u32 = 1000;

    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    /// Timer ids of this thread's pending check and covered confirmation.
    static SETTLE_TIMER: AtomicUsize = AtomicUsize::new(0);
    static CONFIRM_TIMER: AtomicUsize = AtomicUsize::new(0);
    static OCCLUDED: AtomicBool = AtomicBool::new(false);

    pub fn start(app: &tauri::AppHandle) {
        if APP.set(app.clone()).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new().name("occlusion".into()).spawn(|| unsafe {
            for (min, max) in [
                (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
                (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
                (EVENT_SYSTEM_DESKTOPSWITCH, EVENT_SYSTEM_DESKTOPSWITCH),
                (EVENT_OBJECT_SHOW, EVENT_OBJECT_REORDER),
                (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
                (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
            ] {
                let hook = SetWinEventHook(min, max, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT);
                if hook.is_invalid() {
                    tracing::warn!("occlusion: could not watch window events {:#x}..{:#x}", min, max);
                }
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_TIMER {
                    on_timer(msg.wParam.0);
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("occlusion: could not start: {}", e);
        }
    }

    unsafe extern "system" fn on_event(
        _hook: HWINEVENTHOOK,
        event: u32,
        hwnd: HWND,
        id_object: i32,
        id_child: i32,
        _thread: u32,
        _time: u32,
    ) {
        // Only whole top-level windows, and the stacking order of the
        // desktop's windows; the cursor and controls inside windows report
        // location and order changes too.
        let top_level = |hwnd: HWND| unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd;
        let window_event = event == EVENT_SYSTEM_DESKTOPSWITCH
            || (event == EVENT_OBJECT_REORDER && (hwnd.is_invalid() || hwnd == unsafe { GetDesktopWindow() }))
            || (!hwnd.is_invalid() && id_object == OBJID_WINDOW.0 && id_child == 0 && top_level(hwnd));
        if window_event && SETTLE_TIMER.load(Ordering::Relaxed) == 0 {
            let id = unsafe { SetTimer(None, 0, SETTLE_MS, None) };
            SETTLE_TIMER.store(id, Ordering::Relaxed);
        }
    }

    unsafe fn on_timer(id: usize) {
        let confirming = id == CONFIRM_TIMER.load(Ordering::Relaxed);
        unsafe {
            let _ = KillTimer(None, id);
        }
        if confirming {
            CONFIRM_TIMER.store(0, Ordering::Relaxed);
        } else if id == SETTLE_TIMER.load(Ordering::Relaxed) {
            SETTLE_TIMER.store(0, Ordering::Relaxed);
        } else {
            return;
        }
        let main = HWND(crate::windows::MAIN_HWND.load(Ordering::SeqCst) as *mut _);
        if main.is_invalid() {
            return;
        }
        let occluded = unsafe { is_occluded(main) };
        if !occluded {
            let pending = CONFIRM_TIMER.swap(0, Ordering::Relaxed);
            if pending != 0 {
                unsafe {
                    let _ = KillTimer(None, pending);
                }
            }
            report(false);
        } else if confirming {
            report(true);
        } else if !OCCLUDED.load(Ordering::Relaxed) && CONFIRM_TIMER.load(Ordering::Relaxed) == 0 {
            let id = unsafe { SetTimer(None, 0, CONFIRM_MS, None) };
            CONFIRM_TIMER.store(id, Ordering::Relaxed);
        }
    }

    fn report(occluded: bool) {
        if OCCLUDED.swap(occluded, Ordering::Relaxed) != occluded {
            if let Some(app) = APP.get() {
                crate::windows::on_main_occluded(app, occluded);
            }
        }
    }

    /// Nothing of `hwnd` can be seen: it is hidden, minimized, on another
    /// virtual desktop, behind the lock screen, off every screen, or every
    /// visible part of it is under opaque windows.
    pub unsafe fn is_occluded(hwnd: HWND) -> bool {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() || cloaked(hwnd) || !input_desktop() {
                return true;
            }
            let Some(frame) = bounds(hwnd) else { return false };
            let visible = on_screen(&frame);
            let own_process = std::process::id();
            let mut above = GetWindow(hwnd, GW_HWNDPREV).unwrap_or_default();
            let mut empty = region_empty(visible);
            while !empty && !above.is_invalid() {
                if covers(above, own_process) {
                    if let Some(r) = bounds(above) {
                        let cover = CreateRectRgnIndirect(&r);
                        CombineRgn(Some(visible), Some(visible), Some(cover), RGN_DIFF);
                        let _ = DeleteObject(cover.into());
                        empty = region_empty(visible);
                    }
                }
                above = GetWindow(above, GW_HWNDPREV).unwrap_or_default();
            }
            let _ = DeleteObject(visible.into());
            empty
        }
    }

    /// The part of `frame` on any screen, as a new region.
    unsafe fn on_screen(frame: &RECT) -> HRGN {
        unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            let screens = unsafe { &*(data.0 as *const HRGN) };
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
                unsafe {
                    let r = CreateRectRgnIndirect(&info.rcMonitor);
                    CombineRgn(Some(*screens), Some(*screens), Some(r), RGN_OR);
                    let _ = DeleteObject(r.into());
                }
            }
            true.into()
        }
        unsafe {
            let screens = CreateRectRgn(0, 0, 0, 0);
            let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&screens as *const HRGN as isize));
            let window = CreateRectRgnIndirect(frame);
            CombineRgn(Some(window), Some(window), Some(screens), RGN_AND);
            let _ = DeleteObject(screens.into());
            window
        }
    }

    unsafe fn region_empty(region: HRGN) -> bool {
        let probe = unsafe { CreateRectRgn(0, 0, 0, 0) };
        let kind = unsafe { CombineRgn(Some(probe), Some(region), None, windows::Win32::Graphics::Gdi::RGN_COPY) };
        unsafe {
            let _ = DeleteObject(probe.into());
        }
        kind == NULLREGION || kind == RGN_ERROR
    }

    /// A window that hides whatever is under it: shown, on this desktop, not
    /// see-through, rectangular, and not one of Jello's own.
    unsafe fn covers(hwnd: HWND, own_process: u32) -> bool {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() || cloaked(hwnd) {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == own_process {
                return false;
            }
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TRANSPARENT.0 != 0 {
                return false;
            }
            if ex & WS_EX_LAYERED.0 != 0 {
                // Opaque only with a constant full alpha; per-pixel alpha
                // (no attributes) and color keys can show what is under it.
                let mut alpha = 0u8;
                let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS::default();
                if GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags)).is_err()
                    || flags.0 & LWA_COLORKEY.0 != 0
                    || flags.0 & LWA_ALPHA.0 == 0
                    || alpha != 255
                {
                    return false;
                }
            }
            let mut shape = RECT::default();
            GetWindowRgnBox(hwnd, &mut shape) == RGN_ERROR
        }
    }

    unsafe fn cloaked(hwnd: HWND) -> bool {
        let mut value = 0u32;
        unsafe {
            DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut value as *mut u32 as *mut _, 4).is_ok() && value != 0
        }
    }

    /// The window's visible frame, without the invisible resize borders.
    unsafe fn bounds(hwnd: HWND) -> Option<RECT> {
        let mut r = RECT::default();
        let ok = unsafe {
            DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32)
                .is_ok()
                || GetWindowRect(hwnd, &mut r).is_ok()
        };
        (ok && r.right > r.left && r.bottom > r.top).then_some(r)
    }

    /// The desktop windows are on takes input: false behind the lock screen
    /// or a UAC prompt.
    unsafe fn input_desktop() -> bool {
        match unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) } {
            Ok(desktop) => {
                unsafe {
                    let _ = CloseDesktop(desktop);
                }
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(target_os = "windows")]
pub use imp::start;

#[cfg(not(target_os = "windows"))]
pub fn start(_app: &tauri::AppHandle) {}
