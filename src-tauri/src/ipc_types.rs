// Shared IPC types (Rust ⇄ TS)
// Keep in sync with src/types.ts using camelCase

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ViewId(pub u32);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: i32,
    pub window_id: i32,
    pub url: String,
    pub title: Option<String>,
    pub favicon_id: Option<i32>,
    pub pinned: bool,
    pub muted: bool,
    pub order_key: String,
    pub scroll_y: f64,
    pub last_active: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    pub id: String,
    pub version: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteItem {
    pub id: String,
    pub item_type: String, // "tab" | "history" | "bookmark" | "search"
    pub title: String,
    pub url: String,
    pub matched_ranges: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteResults {
    pub open_tabs: Vec<PaletteItem>,
    pub history: Vec<PaletteItem>,
    pub bookmarks: Vec<PaletteItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickLaunchItem {
    pub id: i32,
    pub target_url: String,
    pub title: Option<String>,
    pub sequence: String,
    pub disposition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i32,
    pub url: String,
    pub title: String,
    pub visit_count: i32,
    pub last_visit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub id: i32,
    pub url: String,
    pub title: String,
    pub position: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorCaptureInfo {
    pub index: usize,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale_factor: f64,
    pub image_path: String,
}
