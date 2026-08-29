// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Entry point + handler registration only. Feature logic lives in the
//! modules: preferences + the SQLite store in `persistence`, the track
//! library (scanner, watched folders, cover art) in `library`. Audio
//! decoding and the EQ/analyser graph live in the webview (Web Audio API) —
//! the backend owns metadata, persistence and filesystem scope, never PCM.

mod library;
mod persistence;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        // FIRST plugin, deliberately: a second Janis launch is handed to the
        // running instance (which focuses its window) before any state or
        // setup work happens, so two processes can never race the SQLite
        // write lock on janis.db.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let db = persistence::init(app_data_dir).map_err(std::io::Error::other)?;
            app.manage(db);
            // The asset-protocol scope is in-memory; the library is not.
            // Re-grant every watched folder + ad-hoc file so playback of an
            // already-imported track works on a fresh launch.
            library::restore_asset_scope(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            persistence::get_preferences,
            persistence::set_volume,
            persistence::set_eq,
            persistence::set_playback_option,
            persistence::set_language,
            library::list_tracks,
            library::list_watched_folders,
            library::add_watched_folder,
            library::remove_watched_folder,
            library::import_files,
            library::rescan_library,
            library::get_track_cover,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
