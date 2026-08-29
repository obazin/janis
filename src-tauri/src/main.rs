// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Entry point + handler registration only. Feature logic lives in the
//! modules: preferences + the SQLite store in `persistence`, the track
//! library (scanner, watched folders, cover art) in `library`, and decoding,
//! DSP, output and transport in `audio`. The backend owns the whole signal
//! path; the webview renders what it is told and sends transport commands.

mod audio;
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
            // The engine thread starts here and outlives every screen — the
            // output device itself is opened lazily on first play, so a
            // machine with no sound card still boots.
            app.manage(audio::init(None));
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
            audio::commands::audio_subscribe,
            audio::commands::audio_devices,
            audio::commands::audio_set_device,
            audio::commands::audio_load_queue,
            audio::commands::audio_play_stream,
            audio::commands::audio_play,
            audio::commands::audio_pause,
            audio::commands::audio_toggle,
            audio::commands::audio_stop,
            audio::commands::audio_next,
            audio::commands::audio_previous,
            audio::commands::audio_jump_to,
            audio::commands::audio_seek,
            audio::commands::audio_set_shuffle,
            audio::commands::audio_set_repeat,
            audio::commands::audio_set_volume,
            audio::commands::audio_set_eq,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        // `build` + `run` rather than `run(context)`, so the engine thread can
        // be stopped and joined while the process is still alive. Dropping a
        // cpal stream during teardown instead risks the OS calling back into
        // freed memory.
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                audio::shutdown(&app.state::<audio::AudioEngine>());
            }
        });
}
