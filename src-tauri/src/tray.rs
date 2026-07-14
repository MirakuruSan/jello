// M6.7: system tray icon. Left-click opens the palette; menu offers common actions.
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

pub fn setup_tray(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let summon = MenuItem::with_id(app, "summon", "Show Jello", true, None::<&str>)?;
    let palette = MenuItem::with_id(app, "palette", "Quick palette", true, None::<&str>)?;
    let new_tab = MenuItem::with_id(app, "new_tab", "New Tab", true, None::<&str>)?;
    let screenshot = MenuItem::with_id(app, "screenshot", "Screenshot", true, None::<&str>)?;
    let ocr = MenuItem::with_id(app, "ocr", "Extract Text (OCR)", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&summon, &palette, &new_tab, &screenshot, &ocr, &quit])?;

    TrayIconBuilder::with_id("jello-tray")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Jello")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "summon" => {
                // Window show/create MUST run on the main thread; the tray
                // callback fires on a different thread, so marshal it (doing it
                // here directly left the window hidden — see threading rules).
                let app_h = app.clone();
                let _ = app.run_on_main_thread(move || {
                    crate::windows::ensure_main_window(&app_h);
                });
            }
            "palette" => {
                let app_h = app.clone();
                std::thread::spawn(move || crate::palette::show_palette(&app_h, "search", ""));
            }
            "new_tab" => {
                // Window creation off the main thread (see tabs.rs deadlock note).
                let app_h = app.clone();
                std::thread::spawn(move || crate::palette::show_palette(&app_h, "newtab", ""));
            }
            "screenshot" => {
                let app_h = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::capture::capture_trigger(app_h, "screenshot".to_string()).await;
                });
            }
            "ocr" => {
                let app_h = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::capture::capture_trigger(app_h, "ocr".to_string()).await;
                });
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click brings the browser window back (was: opened the palette).
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                // Marshal to the main thread (tray events fire off-thread).
                let app_h = tray.app_handle().clone();
                let _ = app_h.clone().run_on_main_thread(move || {
                    crate::windows::ensure_main_window(&app_h);
                });
            }
        })
        .build(app)?;

    Ok(())
}
