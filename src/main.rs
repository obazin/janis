// Prevents an additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// The state modules are complete ahead of the user interface that drives
// them; this allowance goes away when the interface is wired in.
#![allow(dead_code)]

//! Entry point. Feature logic lives in the modules: preferences and the SQLite
//! store in `persistence`, the track library (scanner, watched folders, cover
//! art) in `library`, the engine bridge in `audio`, and the state the screens
//! render in `player`, `equalizer`, `playback_options`, `library_view`,
//! `radio`, `notifications` and `visualizer`.

mod audio;
mod effects;
mod equalizer;
mod library;
mod library_view;
mod notifications;
mod persistence;
mod platform;
mod playback_options;
mod player;
mod radio;
mod text;
mod translations;
mod visualizer;

use std::process::ExitCode;

fn main() -> ExitCode {
    match report() {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("janis: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Opens the user's library and says what is in it — everything the
/// application can do before it has a window.
fn report() -> Result<String, String> {
    let directory = platform::data_directory()?;
    let database = persistence::init(directory.clone())?;
    let preferences = persistence::get_preferences(&database)?;
    let tracks = library::list_tracks(&database)?;
    let folders = library::list_watched_folders(&database)?;
    let catalog = radio::Catalog::bundled();
    Ok(format!(
        "{}\n{} tracks in {} albums by {} artists, {} watched folders\n{} radio stations\nlanguage {}, volume {:.2}, equalizer preset {}",
        directory.display(),
        tracks.len(),
        library_view::albums(&tracks).len(),
        library_view::artists(&tracks).len(),
        folders.len(),
        catalog.stations().len(),
        translations::Language::from_code(&preferences.language).code(),
        preferences.volume,
        preferences.eq_preset,
    ))
}
