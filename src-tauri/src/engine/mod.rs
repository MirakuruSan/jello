use crate::ipc_types::ViewId;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabRuntimeState {
    pub url: String,
    pub title: String,
    pub favicon_url: String,
    pub scroll_y: f64,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

pub trait ContentView {
    fn id(&self) -> ViewId;
    fn navigate(&self, url: &str);
    fn back(&self);
    fn forward(&self);
    fn reload(&self);
    fn stop(&self);
    fn set_bounds(&self, rect: Rect);
    fn set_visible(&self, v: bool);
    fn try_suspend(&self, done: Box<dyn FnOnce(bool) + Send>);
    fn resume(&self);
    fn snapshot(&self) -> TabRuntimeState;
    /// Best-effort async refresh of the internal state cache (title/scroll/nav).
    /// Called on switch-away so a subsequent snapshot() reflects recent scrolling.
    fn refresh_snapshot(&self) {}
    fn mute(&self, m: bool);
    fn find(&self, text: &str, forward: bool);
    fn zoom(&self, factor: f64);
    /// Give this view's webview keyboard focus. After closing a tab the newly
    /// activated webview has no focus, so its in-page accelerators (Alt+←/→,
    /// Ctrl+±, …) don't fire until the user clicks (#3).
    fn focus(&self) {}
    fn close(self: Box<Self>);
    fn is_audio_playing(&self) -> bool { false }
}

pub mod webview2;
pub mod pool;
pub mod fractional_index;
