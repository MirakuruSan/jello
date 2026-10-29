//! Taskbar identity for Jello and the windows WebView2 opens on its behalf.
//!
//! The picture-in-picture window belongs to the WebView2 browser process
//! (msedgewebview2.exe), so Windows groups it under that executable and shows
//! the WebView2 icon. Jello gives its own process the AppUserModelID its Start
//! menu shortcut carries, and stamps the same ID and icon onto every top-level
//! window the browser process shows, so they join Jello's taskbar group.

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY, WPARAM};
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
    use windows::Win32::System::Com::CoTaskMemAlloc;
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetWindowLongW, PostMessageW, SendMessageW, EVENT_OBJECT_SHOW, GA_ROOT, GWL_STYLE, ICON_BIG,
        ICON_SMALL, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_GETICON, WM_SETICON, WS_CHILD,
    };

    /// Must match the AppUserModelID on the installer's Start menu shortcut and
    /// the registered app (file_assoc.rs).
    const APP_ID: &str = "com.jello.app";

    /// System.AppUserModel.ID
    const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid: 5,
    };

    static HOOKED_PID: AtomicU32 = AtomicU32::new(0);
    static HOOK: AtomicIsize = AtomicIsize::new(0);

    pub fn set_process_app_id() {
        unsafe {
            if let Err(e) = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) {
                tracing::warn!("could not set the process AppUserModelID: {}", e);
            }
        }
    }

    /// Watch the WebView2 browser process for windows it shows. Idempotent per
    /// process; a restarted browser process replaces the old hook. Main thread
    /// only: the hook is delivered through this thread's message loop.
    pub fn watch_browser_process(pid: u32) {
        if pid == 0 || HOOKED_PID.swap(pid, Ordering::SeqCst) == pid {
            return;
        }
        unsafe {
            let old = HOOK.swap(0, Ordering::SeqCst);
            if old != 0 {
                let _ = UnhookWinEvent(HWINEVENTHOOK(old as *mut _));
            }
            let hook = SetWinEventHook(
                EVENT_OBJECT_SHOW,
                EVENT_OBJECT_SHOW,
                None,
                Some(on_window_shown),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            if hook.is_invalid() {
                tracing::warn!("could not watch WebView2 browser process {} for new windows", pid);
                HOOKED_PID.store(0, Ordering::SeqCst);
            } else {
                HOOK.store(hook.0 as isize, Ordering::SeqCst);
            }
        }
    }

    unsafe extern "system" fn on_window_shown(
        _hook: HWINEVENTHOOK,
        _event: u32,
        hwnd: HWND,
        id_object: i32,
        id_child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if hwnd.is_invalid() || id_object != OBJID_WINDOW.0 || id_child != 0 {
            return;
        }
        unsafe {
            if GetAncestor(hwnd, GA_ROOT) != hwnd || (GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CHILD.0) != 0 {
                return;
            }
            adopt_window(hwnd);
        }
    }

    /// Give a WebView2-owned top-level window Jello's taskbar group and icon.
    unsafe fn adopt_window(hwnd: HWND) {
        unsafe {
            match SHGetPropertyStoreForWindow::<IPropertyStore>(hwnd) {
                Ok(store) => {
                    let mut value = lpwstr_propvariant(APP_ID);
                    if store.SetValue(&PKEY_APP_USER_MODEL_ID, &value).is_ok() {
                        let _ = store.Commit();
                    }
                    // The store copies the value; release our string.
                    let _ = PropVariantClear(&mut value);
                }
                Err(e) => tracing::debug!("no property store for WebView2 window: {}", e),
            }

            let main = HWND(crate::windows::MAIN_HWND.load(Ordering::SeqCst) as *mut _);
            if main.is_invalid() {
                return;
            }
            for kind in [ICON_BIG, ICON_SMALL] {
                let icon = SendMessageW(main, WM_GETICON, Some(WPARAM(kind as usize)), Some(LPARAM(0)));
                if icon.0 != 0 {
                    // Posted, not sent: the window lives in another process.
                    let _ = PostMessageW(Some(hwnd), WM_SETICON, WPARAM(kind as usize), LPARAM(icon.0));
                }
            }
        }
    }

    /// A VT_LPWSTR PROPVARIANT owning a CoTaskMem copy of `s`.
    fn lpwstr_propvariant(s: &str) -> PROPVARIANT {
        let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
        let mut value = PROPVARIANT::default();
        unsafe {
            let buf = CoTaskMemAlloc(wide.len() * 2) as *mut u16;
            if buf.is_null() {
                return value;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), buf, wide.len());
            let inner = &mut *value.Anonymous.Anonymous;
            inner.vt = VT_LPWSTR;
            inner.Anonymous.pwszVal = PWSTR(buf);
        }
        value
    }
}

#[cfg(target_os = "windows")]
pub use imp::{set_process_app_id, watch_browser_process};

#[cfg(not(target_os = "windows"))]
pub fn set_process_app_id() {}

#[cfg(not(target_os = "windows"))]
pub fn watch_browser_process(_pid: u32) {}
