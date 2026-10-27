//! Window material: the Windows system backdrop drawn behind Jello's
//! transparent window. Acrylic blurs whatever is behind the window, live; Mica
//! tints with the desktop wallpaper and costs less; Solid turns the backdrop
//! off. The window theme is set to match Jello's so the backdrop tint follows
//! light/dark mode.

use tauri::window::{Effect, EffectsBuilder};
use tauri::{AppHandle, Manager, Theme, Window};

/// `material`: "acrylic" | "mica" | "solid".
pub fn apply(window: &Window, material: &str, dark: bool) {
    let _ = window.set_theme(Some(if dark { Theme::Dark } else { Theme::Light }));
    round_corners(window);
    let effect = match material {
        "solid" => None,
        "mica" if dark => Some(Effect::MicaDark),
        "mica" => Some(Effect::MicaLight),
        _ => Some(Effect::Acrylic),
    };
    let result = match effect {
        Some(e) => window.set_effects(EffectsBuilder::new().effect(e).build()),
        None => window.set_effects(None),
    };
    if let Err(e) = result {
        tracing::warn!("window material '{}' not applied: {}", material, e);
    }
}

/// Ask DWM for Windows 11's rounded window corners, so the backdrop's outline
/// matches the CSS radius of frameless windows like the palette.
fn round_corners(window: &Window) {
    #[cfg(target_os = "windows")]
    if let Ok(hwnd) = window.hwnd() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        };
        let pref = DWMWCP_ROUND;
        unsafe {
            let _ = DwmSetWindowAttribute(
                HWND(hwnd.0),
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const _,
                std::mem::size_of_val(&pref) as u32,
            );
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

/// The saved material and whether the saved theme is dark (the default).
fn saved(app: &AppHandle) -> (String, bool) {
    let get = |key: &str| crate::capture::screenshot::get_setting(app, key);
    let material = get("windowMaterial").unwrap_or_else(|| "acrylic".to_string());
    let dark = get("theme").map_or(true, |t| t != "light");
    (material, dark)
}

/// Apply the saved material + theme to one window.
pub fn apply_saved_to(app: &AppHandle, window: &Window) {
    let (material, dark) = saved(app);
    apply(window, &material, dark);
}

/// Apply the saved material + theme to the main window and the palette.
pub fn apply_saved(app: &AppHandle) {
    let (material, dark) = saved(app);
    for label in ["main", "palette"] {
        if let Some(window) = app.get_window(label) {
            apply(&window, &material, dark);
        }
    }
}

/// Re-apply after Settings changes the material or theme.
#[tauri::command]
pub async fn appearance_apply(app: AppHandle) -> Result<(), String> {
    let app2 = app.clone();
    app.run_on_main_thread(move || apply_saved(&app2))
        .map_err(|e| e.to_string())
}
