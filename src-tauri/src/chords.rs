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
}

static CHORD_MANAGER: Mutex<ChordManager> = Mutex::new(ChordManager {
    hhook: None,
    sequence: Vec::new(),
    app_handle: None,
    timer_cancel_tx: None,
});

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
    let mut manager = CHORD_MANAGER.lock().unwrap();
    if manager.hhook.is_some() {
        return; // Already armed
    }
    
    manager.app_handle = Some(app.clone());
    manager.sequence.clear();
    
    let slots = get_all_quick_launch_slots(&app);
    let _ = app.emit("hotkey:chord-hud", ChordHudPayload {
        keys: String::new(),
        matching_slots: slots,
    });

    // Install low-level keyboard hook
    unsafe {
        use windows::Win32::Foundation::HINSTANCE;
        let hook_res = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(low_level_keyboard_proc),
            Some(HINSTANCE(std::ptr::null_mut())),
            0,
        );
        if let Ok(hook) = hook_res {
            manager.hhook = Some(SendHhook(hook));
            tracing::info!("Low-level keyboard hook installed successfully.");
        } else {
            tracing::error!("Failed to install low-level keyboard hook.");
        }
    }

    // Set 2-second timeout
    let (tx, rx) = std::sync::mpsc::channel();
    manager.timer_cancel_tx = Some(tx);
    
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

pub fn disarm_chords() {
    let mut manager = CHORD_MANAGER.lock().unwrap();
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
            
            if vk == VK_ESCAPE.0 as u32 {
                std::thread::spawn(|| {
                    disarm_chords();
                });
                return LRESULT(1);
            }
            
            if let Some(key_str) = vk_to_string(vk) {
                let run_result = handle_chord_key(key_str);
                if run_result {
                    return LRESULT(1); // Swallow
                }
            } else {
                // Unmapped key, disarm and swallow to avoid passing raw chord keys to parent app
                std::thread::spawn(|| {
                    disarm_chords();
                });
                return LRESULT(1);
            }
        }
    }
    
    CallNextHookEx(None, code, wparam, lparam)
}

fn handle_chord_key(key: String) -> bool {
    let mut manager = CHORD_MANAGER.lock().unwrap();
    manager.sequence.push(key);
    
    let typed_sequence = manager.sequence.join(",");
    let app = match &manager.app_handle {
        Some(a) => a.clone(),
        None => return false,
    };
    
    let slots = get_all_quick_launch_slots(&app);
    let mut matching_slots = Vec::new();
    for slot in slots {
        let normalized_slot = slot.sequence.to_uppercase().replace(" ", "");
        let normalized_typed = typed_sequence.to_uppercase().replace(" ", "");
        if normalized_slot.starts_with(&normalized_typed) {
            matching_slots.push(slot);
        }
    }
    
    if matching_slots.is_empty() {
        let _ = manager.timer_cancel_tx.take();
        let hook = manager.hhook.take();
        if let Some(SendHhook(h)) = hook {
            unsafe { let _ = UnhookWindowsHookEx(h); }
        }
        let _ = app.emit("hotkey:chord-hud", ChordHudPayload {
            keys: String::new(),
            matching_slots: Vec::new(),
        });
        return true;
    }
    
    let normalized_typed = typed_sequence.to_uppercase().replace(" ", "");
    let exact_match = matching_slots.iter().find(|s| {
        s.sequence.to_uppercase().replace(" ", "") == normalized_typed
    });
    
    if let Some(slot) = exact_match {
        let has_prefix_match = matching_slots.iter().any(|s| {
            s.id != slot.id && s.sequence.to_uppercase().replace(" ", "").starts_with(&normalized_typed)
        });
        
        if !has_prefix_match {
            let slot_clone = slot.clone();
            let app_thread = app.clone();
            std::thread::spawn(move || {
                let _ = execute_quick_launch(app_thread, slot_clone);
            });
            
            let _ = manager.timer_cancel_tx.take();
            let hook = manager.hhook.take();
            if let Some(SendHhook(h)) = hook {
                unsafe { let _ = UnhookWindowsHookEx(h); }
            }
            let _ = app.emit("hotkey:chord-hud", ChordHudPayload {
                keys: String::new(),
                matching_slots: Vec::new(),
            });
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
