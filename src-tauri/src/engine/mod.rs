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

/// What a picture-in-picture request should do in the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipRequest {
    /// User toggle: leave PiP if a video is in it, else float the playing (or
    /// largest) video.
    Toggle,
    /// Tab left / window hidden: float a video only if one is playing.
    Auto,
    /// Back on the tab: leave PiP, but only if Jello put it there.
    ExitAuto,
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
    /// Ctrl+±, …) don't fire until the user clicks.
    fn focus(&self) {}
    /// Hint Chromium to trim memory (true = LOW target, false = NORMAL). Unlike
    /// TrySuspend this does NOT freeze JS/media — safe while hidden.
    fn set_memory_target(&self, _low: bool) {}
    fn close(self: Box<Self>);
    fn is_audio_playing(&self) -> bool { false }
    /// A download this view started is still running (or paused/resumable).
    /// WebView2 cancels such a download when the view closes, so the pool parks
    /// the view instead of closing it.
    fn has_active_downloads(&self) -> bool { false }
    /// May heal_blank_tab re-navigate this view? False while a navigation is
    /// still in flight or after the view's navigation became a download — a
    /// blank document is expected then, and re-navigating restarts the load or
    /// starts a duplicate download.
    fn blank_heal_allowed(&self) -> bool { true }
    fn picture_in_picture(&self, _req: PipRequest) {}
    /// Jello last put a video from this view into picture-in-picture.
    fn in_picture_in_picture(&self) -> bool { false }
}

pub mod webview2;
pub mod pool;
pub mod fractional_index;
