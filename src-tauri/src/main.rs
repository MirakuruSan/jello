// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod app;
pub mod logging;
pub mod db;
pub mod engine;
pub mod error;
pub mod ipc_types;
pub mod tabs;
pub mod extensions;
pub mod search;
pub mod platform;
pub mod windows;
pub mod palette;
pub mod chords;
pub mod incognito;
pub mod capture;
pub mod data_cmds;
pub mod sessions;
pub mod tray;
pub mod updater;
pub mod deeplink;

fn main() {
    app::run();
}
