use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, Emitter};
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowsHookExW, UnhookWindowsHookEx, CallNextHookEx,
    WH_KEYBOARD_LL, KBDLLHOOKSTRUCT, HHOOK
};
use windows::Win32::Foundation::{LRESULT, WPARAM, LPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use crate::db::DbState;
use crate::ipc_types::{QuickLaunchItem, Tab};
use crate::engine::pool::TabPool;

// Thread-safe HHOOK wrapper since raw pointers aren't Send/Sync
struct SendHhook(HHOOK);
unsafe impl Send for SendHhook {}
unsafe impl Sync for SendHhook {}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChordHudPayload {
    pub keys: String,
    pub matching_slots: Vec<QuickLaunchItem>,
}

pub struct ChordManager {
    hhook: Option<SendHhook>,
    sequence: Vec<String>,
    app_handle: Option<AppHandle>,
    timer_cancel_tx: Option<std::sync::mpsc::Sender<()>>,
    /// Quick-launch slots cached ONCE at arm time (P0.3.1). The low-level
    /// keyboard hook must never touch the DB — a slow round trip inside
    /// `low_level_keyboard_proc` stalls every keystroke system-wide and gets the
    /// hook killed by Windows.
    slots: Vec<QuickLaunchItem>,
    /// Synchronous armed flag. The hook itself is installed asynchronously on the
    /// main thread, so `hhook` alone can't guard against a double-arm race.
    armed: bool,
}

static CHORD_MANAGER: Mutex<ChordManager> = Mutex::new(ChordManager {
    hhook: None,
    sequence: Vec::new(),
    app_handle: None,
    timer_cancel_tx: None,
    slots: Vec::new(),
    armed: false,
});

/// VKs for modifier keys — while armed, these are passed through untouched and
/// do NOT disarm (the user may still be releasing the Ctrl of the Ctrl+Space
/// leader, and modifiers are never part of a chord sequence).
fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 // Shift / Ctrl / Alt (generic)
        | 0xA0 | 0xA1      // L/R Shift
        | 0xA2 | 0xA3      // L/R Ctrl
        | 0xA4 | 0xA5      // L/R Alt
        | 0x5B | 0x5C      // L/R Win
        | 0x14 | 0x90 | 0x91 // Caps/Num/Scroll lock
    )
}

fn vk_to_string(vk: u32) -> Option<String> {
    match vk {
        0x1B => Some("Esc".to_string()),
        0x30..=0x39 => Some(((vk - 0x30) as u8 + b'0') as char).map(|c| c.to_string()),
        0x60..=0x69 => Some(((vk - 0x60) as u8 + b'0') as char).map(|c| c.to_string()),
        0x41..=0x5A => Some(((vk - 0x41) as u8 + b'A') as char).map(|c| c.to_string()),
        0x20 => Some("Space".to_string()),
        _ => None,
    }
}

pub fn arm_chords(app: AppHandle) {
    // Fetch slots + set up state synchronously (this runs off the hook, on the
    // global-shortcut poller thread — DB access is acceptable HERE, never inside
    // the hook). Guard against a double-arm race with the `armed` flag since the
    // hook is installed asynchronously on the main thread below.
    let rx = {
        let mut manager = CHORD_MANAGER.lock().unwrap();
        if manager.armed {
            return; // Already armed
        }
        manager.armed = true;
        manager.app_handle = Some(app.clone());
        manager.sequence.clear();

        let slots = get_all_quick_launch_slots(&app);
        manager.slots = slots.clone();
        let _ = app.emit("hotkey:chord-hud", ChordHudPayload {
            keys: String::new(),
            matching_slots: slots,
        });

        // 2-second timeout.
        let (tx, rx) = std::sync::mpsc::channel();
        manager.timer_cancel_tx = Some(tx);
        rx
    };

    // Install the low-level keyboard hook on the MAIN thread. A WH_KEYBOARD_LL
    // hook only fires reliably when installed from a thread that pumps messages;
    // arm_chords is called from the poller thread (no message loop), so
    // marshaling to the main thread is required (P0.3.2). The hook callback then
    // also runs on the main thread.
    let _ = app.run_on_main_thread(move || unsafe {
        use windows::Win32::Foundation::HINSTANCE;
        let hook_res = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(low_level_keyboard_proc),
            Some(HINSTANCE(std::ptr::null_mut())),
            0,
        );
        match hook_res {
            Ok(hook) => {
                CHORD_MANAGER.lock().unwrap().hhook = Some(SendHhook(hook));
                tracing::info!("Low-level keyboard hook installed successfully.");
            }
            Err(_) => {
                tracing::error!("Failed to install low-level keyboard hook.");
                // Roll back the armed flag so a later leader press can retry.
                let mut m = CHORD_MANAGER.lock().unwrap();
                m.armed = false;
                m.timer_cancel_tx = None;
            }
        }
    });

    std::thread::spawn(move || {
        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                tracing::info!("Chord sequence timed out.");
                disarm_chords();
            }
            _ => {
                // Cancelled
            }
        }
    });
}

/// Disarm with the manager lock already held (callable from the hook callback,
/// which holds the lock and cannot re-lock the non-reentrant Mutex).
fn disarm_locked(manager: &mut ChordManager) {
    manager.armed = false;
    manager.sequence.clear();
    manager.slots.clear();
    if let Some(tx) = manager.timer_cancel_tx.take() {
        let _ = tx.send(());
    }
    if let Some(SendHhook(hook)) = manager.hhook.take() {
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
            tracing::info!("Low-level keyboard hook uninstalled successfully.");
        }
    }
    if let Some(app) = manager.app_handle.take() {
        let _ = app.emit("hotkey:chord-hud", ChordHudPayload {
            keys: String::new(),
            matching_slots: Vec::new(),
        });
    }
}

pub fn disarm_chords() {
    let mut manager = CHORD_MANAGER.lock().unwrap();
    disarm_locked(&mut manager);
}

unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{HC_ACTION, WM_KEYDOWN, WM_SYSKEYDOWN};
    
    if code as u32 == HC_ACTION {
        let event_type = wparam.0 as u32;
        if event_type == WM_KEYDOWN || event_type == WM_SYSKEYDOWN {
            let hook_struct = *(lparam.0 as *const KBDLLHOOKSTRUCT);
            let vk = hook_struct.vkCode;

            // Esc cancels the chord — swallow it so it doesn't also reach the app.
            if vk == VK_ESCAPE.0 as u32 {
                disarm_chords();
                return LRESULT(1);
            }

            // Modifiers pass straight through and never disarm (the user may still
            // be releasing the Ctrl of the leader; modifiers are never chord keys).
            if is_modifier_vk(vk) {
                return CallNextHookEx(None, code, wparam, lparam);
            }

            if let Some(key_str) = vk_to_string(vk) {
                // Swallow ONLY if this key extends a valid chord prefix; otherwise
                // handle_chord_key disarms and returns false → we pass it through
                // so we never eat unrelated keystrokes system-wide (P0.3.3).
                if handle_chord_key(key_str) {
                    return LRESULT(1);
                }
            } else {
                // Unmapped, non-modifier key: not part of any chord. Disarm and
                // pass it through rather than eating it.
                disarm_chords();
            }
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}

/// Returns true if the key should be SWALLOWED (it extends a valid chord prefix
/// or triggered a launch); false if it should PASS THROUGH (it matched nothing —
/// the chord is abandoned and the manager is disarmed). Reads slots from the
/// arm-time cache: NO DB access here — this runs inside the keyboard hook.
fn handle_chord_key(key: String) -> bool {
    let mut manager = CHORD_MANAGER.lock().unwrap();
    let app = match &manager.app_handle {
        Some(a) => a.clone(),
        None => return false,
    };
    manager.sequence.push(key);

    let typed_sequence = manager.sequence.join(",");
    let normalized_typed = typed_sequence.to_uppercase().replace(' ', "");

    let matching_slots: Vec<QuickLaunchItem> = manager
        .slots
        .iter()
        .filter(|slot| {
            slot.sequence
                .to_uppercase()
                .replace(' ', "")
                .starts_with(&normalized_typed)
        })
        .cloned()
        .collect();

    if matching_slots.is_empty() {
        // Nothing matches this prefix — abandon the chord and let the key through.
        disarm_locked(&mut manager);
        return false;
    }

    let exact_match = matching_slots
        .iter()
        .find(|s| s.sequence.to_uppercase().replace(' ', "") == normalized_typed);

    if let Some(slot) = exact_match {
        let has_prefix_match = matching_slots.iter().any(|s| {
            s.id != slot.id
                && s.sequence.to_uppercase().replace(' ', "").starts_with(&normalized_typed)
        });

        if !has_prefix_match {
            let slot_clone = slot.clone();
            let app_thread = app.clone();
            std::thread::spawn(move || {
                let _ = execute_quick_launch(app_thread, slot_clone);
            });
            disarm_locked(&mut manager);
            return true;
        }
    }

    let display_keys = manager.sequence.join(" ➔ ");
    let _ = app.emit("hotkey:chord-hud", ChordHudPayload {
        keys: display_keys,
        matching_slots,
    });

    true
}

fn get_all_quick_launch_slots(app: &AppHandle) -> Vec<QuickLaunchItem> {
    let db = app.state::<DbState>();
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = crate::db::quick_launch::list_quick_launch(conn);
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Ok(Vec::new())).unwrap_or(Vec::new())
}

fn get_focused_window_id(app: &AppHandle) -> i32 {
    for win in app.webview_windows().values() {
        if win.is_focused().unwrap_or(false) {
            let label = win.label();
            if label == "main" {
                return 1;
            }
            if let Some(pos) = label.rfind('_') {
                if let Ok(id) = label[pos + 1..].parse::<i32>() {
                    return id;
                }
            }
        }
    }
    1
}

/// Public entry point for dispatching a quick-launch slot (used by plain global
/// combos registered in app.rs, not just leader chords).
pub fn execute_quick_launch_public(app: AppHandle, item: QuickLaunchItem) -> Result<(), String> {
    execute_quick_launch(app, item)
}

fn execute_quick_launch(app: AppHandle, item: QuickLaunchItem) -> Result<(), String> {
    let pool = app.state::<Arc<Mutex<TabPool>>>();
    let db = app.state::<DbState>();
    
    match item.disposition.as_str() {
        "focus_or_open" => {
            let (tx, rx) = std::sync::mpsc::channel();
            let db_clone = db.clone();
            let url_clone = item.target_url.clone();
            db_clone.execute(move |conn| {
                let res = crate::db::tabs_repo::list_all_tabs(conn);
                let _ = tx.send(res);
            });
            let all_tabs = rx.recv().unwrap_or(Ok(Vec::new())).map_err(|e| e.to_string())?;
            
            let found_tab = all_tabs.into_iter().find(|t| t.url == url_clone);
            if let Some(tab) = found_tab {
                crate::tabs::tabs_activate_impl(tab.id, &db, &pool, &app)?;
                
                let win_label = if tab.window_id == 1 {
                    "main".to_string()
                } else {
                    let main_label = format!("main_{}", tab.window_id);
                    if app.get_window(&main_label).is_some() {
                        main_label
                    } else {
                        format!("incognito_{}", tab.window_id)
                    }
                };
                if let Some(win) = app.get_window(&win_label) {
                    let _ = win.set_focus();
                }
            } else {
                let active_win_id = get_focused_window_id(&app);
                let _ = crate::tabs::tabs_create_impl(
                    Some(item.target_url),
                    Some(false),
                    Some(active_win_id),
                    &db,
                    &pool,
                    &app,
                )?;
            }
        }
        "new_tab" => {
            let active_win_id = get_focused_window_id(&app);
            let _ = crate::tabs::tabs_create_impl(
                Some(item.target_url),
                Some(false),
                Some(active_win_id),
                &db,
                &pool,
                &app,
            )?;
        }
        "new_window" => {
            let win_id = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() & 0x7FFFFFFF) as i32;
            let label = format!("main_{}", win_id);
            
            let handle = app.clone();
            let target_url = item.target_url.clone();
            
            let created_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
            let dummy_tab = Tab {
                id: 0,
                window_id: win_id,
                url: target_url,
                title: None,
                favicon_id: None,
                pinned: false,
                muted: false,
                order_key: "a".to_string(),
                scroll_y: 0.0,
                last_active: Some(created_at),
                created_at,
            };
            
            let (tx, rx) = std::sync::mpsc::channel();
            let db_clone = db.clone();
            db_clone.execute(move |conn| {
                let res = crate::db::tabs_repo::insert_tab(conn, &dummy_tab);
                let _ = tx.send(res);
            });
            let _ = rx.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows)).map_err(|e| e.to_string())?;
            
            let _ = tauri::WebviewWindowBuilder::new(
                &handle,
                &label,
                tauri::WebviewUrl::App("index.html".into()),
            )
            .inner_size(800.0, 600.0)
            .title("Jello")
            .decorations(false)
            .transparent(true)
            .build()
            .map_err(|e| e.to_string())?;

            if let Some(win) = handle.get_window(&label) {
                crate::app::attach_window_plumbing(&handle, win);
            }
        }
        "new_incognito" => {
            let win_id = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() & 0x7FFFFFFF) as i32;
            let label = format!("incognito_{}", win_id);
            
            let handle = app.clone();
            let target_url = item.target_url.clone();
            
            crate::incognito::add_incognito_tab(win_id, target_url);
            
            let _ = tauri::WebviewWindowBuilder::new(
                &handle,
                &label,
                tauri::WebviewUrl::App("index.html?incognito=true".into()),
            )
            .inner_size(800.0, 600.0)
            .title("Jello (Incognito)")
            .decorations(false)
            .transparent(true)
            .incognito(true)
            .build()
            .map_err(|e| e.to_string())?;

            if let Some(win) = handle.get_window(&label) {
                crate::app::attach_window_plumbing(&handle, win);
            }
        }
        _ => {}
    }
    
    Ok(())
}
