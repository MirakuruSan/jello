use std::collections::HashSet;
use std::sync::Mutex;
use crate::ipc_types::Tab;

use std::sync::OnceLock;

static IN_MEMORY_TABS: OnceLock<Mutex<Vec<Tab>>> = OnceLock::new();
static INCOGNITO_WINDOWS: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();
static CLOSED_INCOGNITO: OnceLock<Mutex<Vec<Tab>>> = OnceLock::new();

fn get_closed_incognito() -> &'static Mutex<Vec<Tab>> {
    CLOSED_INCOGNITO.get_or_init(|| Mutex::new(Vec::new()))
}

/// Push a closed incognito tab onto the in-memory undo stack (cap 25 per window).
pub fn push_closed_incognito(tab: Tab) {
    let mut stack = get_closed_incognito().lock().unwrap();
    stack.push(tab.clone());
    let win = tab.window_id;
    let count = stack.iter().filter(|t| t.window_id == win).count();
    if count > 25 {
        if let Some(pos) = stack.iter().position(|t| t.window_id == win) {
            stack.remove(pos);
        }
    }
}

/// Pop the most-recently-closed incognito tab for a window.
pub fn pop_closed_incognito(window_id: i32) -> Option<Tab> {
    let mut stack = get_closed_incognito().lock().unwrap();
    stack
        .iter()
        .rposition(|t| t.window_id == window_id)
        .map(|pos| stack.remove(pos))
}

fn get_in_memory_tabs() -> &'static Mutex<Vec<Tab>> {
    IN_MEMORY_TABS.get_or_init(|| Mutex::new(Vec::new()))
}

fn get_incognito_windows() -> &'static Mutex<HashSet<i32>> {
    INCOGNITO_WINDOWS.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn register_incognito_window(window_id: i32) {
    get_incognito_windows().lock().unwrap().insert(window_id);
}

pub fn unregister_incognito_window(window_id: i32) {
    get_incognito_windows().lock().unwrap().remove(&window_id);
    let mut tabs = get_in_memory_tabs().lock().unwrap();
    tabs.retain(|t| t.window_id != window_id);
}

pub fn is_incognito_window(window_id: i32) -> bool {
    get_incognito_windows().lock().unwrap().contains(&window_id)
}

pub fn list_incognito_tabs(window_id: i32) -> Vec<Tab> {
    let tabs = get_in_memory_tabs().lock().unwrap();
    let mut list: Vec<Tab> = tabs.iter().filter(|t| t.window_id == window_id).cloned().collect();
    list.sort_by(|a, b| a.order_key.cmp(&b.order_key));
    list
}

pub fn get_incognito_tab(id: i32) -> Option<Tab> {
    let tabs = get_in_memory_tabs().lock().unwrap();
    tabs.iter().find(|t| t.id == id).cloned()
}

pub fn add_incognito_tab(window_id: i32, url: String) {
    let mut tabs = get_in_memory_tabs().lock().unwrap();
    let min_id = tabs.iter().map(|t| t.id).min().unwrap_or(0);
    let new_id = if min_id < 0 { min_id - 1 } else { -1 };
    
    register_incognito_window(window_id);
    
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
        
    tabs.push(Tab {
        id: new_id,
        window_id,
        url,
        title: None,
        favicon_id: None,
        pinned: false,
        muted: false,
        order_key: "a".to_string(),
        scroll_y: 0.0,
        last_active: Some(created_at),
        created_at,
    });
}

pub fn create_incognito_tab(window_id: i32, url: String, order_key: String) -> Tab {
    let mut tabs = get_in_memory_tabs().lock().unwrap();
    let min_id = tabs.iter().map(|t| t.id).min().unwrap_or(0);
    let new_id = if min_id < 0 { min_id - 1 } else { -1 };
    
    register_incognito_window(window_id);
    
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
        
    let tab = Tab {
        id: new_id,
        window_id,
        url,
        title: None,
        favicon_id: None,
        pinned: false,
        muted: false,
        order_key,
        scroll_y: 0.0,
        last_active: Some(created_at),
        created_at,
    };
    
    tabs.push(tab.clone());
    tab
}

pub fn update_incognito_tab(id: i32, url: String, title: Option<String>, scroll_y: f64) {
    let mut tabs = get_in_memory_tabs().lock().unwrap();
    if let Some(tab) = tabs.iter_mut().find(|t| t.id == id) {
        tab.url = url;
        if title.is_some() {
            tab.title = title;
        }
        tab.scroll_y = scroll_y;
    }
}

pub fn update_incognito_tab_last_active(id: i32, last_active: i64) {
    let mut tabs = get_in_memory_tabs().lock().unwrap();
    if let Some(tab) = tabs.iter_mut().find(|t| t.id == id) {
        tab.last_active = Some(last_active);
    }
}

pub fn delete_incognito_tab(id: i32) {
    let mut tabs = get_in_memory_tabs().lock().unwrap();
    tabs.retain(|t| t.id != id);
}

pub fn set_incognito_pinned(id: i32, pinned: bool) {
    let mut tabs = get_in_memory_tabs().lock().unwrap();
    if let Some(tab) = tabs.iter_mut().find(|t| t.id == id) {
        tab.pinned = pinned;
    }
}

pub fn set_incognito_muted(id: i32, muted: bool) {
    let mut tabs = get_in_memory_tabs().lock().unwrap();
    if let Some(tab) = tabs.iter_mut().find(|t| t.id == id) {
        tab.muted = muted;
    }
}

pub fn reorder_incognito_tab(id: i32, order_key: String) {
    let mut tabs = get_in_memory_tabs().lock().unwrap();
    if let Some(tab) = tabs.iter_mut().find(|t| t.id == id) {
        tab.order_key = order_key;
    }
}

pub fn clear_all_incognito_tabs() {
    get_in_memory_tabs().lock().unwrap().clear();
    get_incognito_windows().lock().unwrap().clear();
}
