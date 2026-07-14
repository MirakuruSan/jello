// M7.1: app-shell auto-updater plumbing (consent-gated; manual approval).
// NOTE: a real update endpoint + minisign pubkey must be configured in
// tauri.conf.json ("plugins.updater") before this functions against a server.
// Until then check() returns an error which we surface as a toast.
use tauri::{command, AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

#[command]
pub fn updater_enabled() -> bool {
    // A real GitHub Releases endpoint + signing pubkey are configured in
    // tauri.conf.json (plugins.updater), so the feature is live.
    true
}

/// Check for an available update. Returns Some(version) if one is available.
#[command]
pub async fn updater_check(app: AppHandle) -> Result<Option<String>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(update)) => {
            let _ = app.emit("update:available", update.version.clone());
            Ok(Some(update.version))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Download + install the available update, then relaunch (manual approval:
/// only called when the user clicks install).
#[command]
pub async fn updater_apply(app: AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update available".to_string())?;
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}

/// Background 24h check, only if the user consented via the updateCheck setting.
pub fn spawn_periodic_check(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(24 * 3600));
            let consented = crate::capture::screenshot::get_setting(&app, "updateCheck")
                .map(|v| v == "true")
                .unwrap_or(false);
            if consented {
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = updater_check(app2).await;
                });
            }
        }
    });
}
