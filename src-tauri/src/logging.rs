//! Field-diagnostics logging (P0.4).
//!
//! The app previously had NO tracing subscriber, so every `tracing::error!` /
//! `tracing::info!` in the codebase went to the void — silent field bugs. This
//! installs a subscriber that writes to `%APPDATA%\Jello\logs\jello.log` so that
//! hotkey-registration failures, DB errors, window recreation, etc. become
//! diagnosable after the fact.
//!
//! Returns the `WorkerGuard` for the non-blocking writer. The caller MUST keep
//! it alive for the lifetime of the process (drop it and buffered lines are
//! lost) — `run()` holds it in a local that outlives `app.run()`.

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

pub fn init() -> Option<WorkerGuard> {
    let app_data = std::env::var("APPDATA").ok()?;
    let log_dir = std::path::Path::new(&app_data).join("Jello").join("logs");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("jello: could not create log dir {log_dir:?}: {e}");
        return None;
    }

    // If the current log has grown past ~5 MB, truncate it (single-file policy —
    // keep diagnostics lightweight, never let the log grow unbounded in the
    // field).
    let log_path = log_dir.join("jello.log");
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 5 * 1024 * 1024 {
            let _ = std::fs::write(&log_path, b"");
        }
    }

    let file_appender = tracing_appender::rolling::never(&log_dir, "jello.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // INFO default; RUST_LOG can raise/lower it in the field.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .finish();

    if tracing::subscriber::set_global_default(subscriber).is_err() {
        eprintln!("jello: tracing subscriber already set");
        return None;
    }

    tracing::info!("jello logging initialized at {:?}", log_path);
    Some(guard)
}
