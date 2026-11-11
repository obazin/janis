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

mod application;
mod audio;
mod background;
mod covers;
mod effects;
mod equalizer;
mod halo;
mod library;
mod library_view;
mod logging;
mod notifications;
mod persistence;
mod platform;
mod playback_options;
mod player;
mod radio;
mod text;
mod translations;
mod visualizer;

/// The compiled user interface, and the function that fills its `Strings`
/// global from a translation lookup.
mod user_interface {
    slint::include_modules!();
    include!(concat!(env!("OUT_DIR"), "/strings.rs"));
}

use std::process::ExitCode;

use slint::ComponentHandle;

use application::Application;
use user_interface::AppWindow;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("janis: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    logging::init();
    platform::select_backend()?;
    let window = AppWindow::new().map_err(|error| format!("create the window: {error}"))?;
    let application = Application::start(window.clone_strong())?;

    #[cfg(debug_assertions)]
    let _snapshot_timer = development::snapshot_when_asked(&window);
    #[cfg(debug_assertions)]
    development::play_when_asked(&application);
    #[cfg(debug_assertions)]
    development::open_what_was_asked(&window);

    let outcome = window
        .run()
        .map_err(|error| format!("run the event loop: {error}"));
    application.shut_down();
    outcome
}

/// Development-only helpers, absent from release builds.
#[cfg(debug_assertions)]
mod development {
    use std::time::Duration;

    use slint::{ComponentHandle, Timer, TimerMode};

    use crate::application::Application;
    use crate::user_interface::AppWindow;

    /// With `JANIS_AUTOPLAY=<search>` set, starts playing the tracks the
    /// library search finds for it at launch — silently — so playback can be
    /// exercised without a person clicking or listening. An empty value plays
    /// from the top of the library.
    pub fn play_when_asked(application: &Application) {
        if let Ok(search) = std::env::var("JANIS_AUTOPLAY") {
            application.silence();
            let found = crate::library_view::search_library(&application.tracks(), &search);
            application.play_queue(found.into_iter().take(5).collect(), 0);
        }
    }

    /// `JANIS_SCREEN=<identifier>` opens on that screen, `JANIS_EQUALIZER`
    /// opens the equalizer sheet, `JANIS_LIBRARY` a Library tab and
    /// `JANIS_SEARCH` fills the search field, so each can be looked at in a
    /// snapshot.
    pub fn open_what_was_asked(window: &AppWindow) {
        let shell = window.global::<crate::user_interface::Shell>();
        if let Ok(screen) = std::env::var("JANIS_SCREEN") {
            shell.set_screen(screen.into());
        }
        if std::env::var_os("JANIS_EQUALIZER").is_some() {
            shell.set_equalizer_open(true);
        }
        // `JANIS_LIBRARY=<tab>[:<card>]` opens a Library tab and, in a grid,
        // the card at that place.
        if let Ok(request) = std::env::var("JANIS_LIBRARY") {
            let library = window.global::<crate::user_interface::Library>();
            let (tab, card) = request.split_once(':').unwrap_or((&request, ""));
            library.invoke_tab_selected(tab.into());
            if let Ok(card) = card.parse() {
                match tab {
                    "artists" => library.invoke_artist_clicked(card),
                    _ => library.invoke_album_clicked(card),
                }
            }
        }
        // `JANIS_STATION=<place>` starts the station at that place in the
        // Radio grid; pair it with `JANIS_AUTOPLAY` to keep it silent.
        if let Some(station) = std::env::var("JANIS_STATION")
            .ok()
            .and_then(|place| place.parse().ok())
        {
            window
                .global::<crate::user_interface::Radio>()
                .invoke_station_clicked(station);
        }
        if let Ok(search) = std::env::var("JANIS_SEARCH") {
            shell.set_search(search.as_str().into());
            shell.invoke_search_edited(search.into());
        }
    }

    /// With `JANIS_SNAPSHOT=<file.png>` set, saves what the window shows after
    /// `JANIS_SNAPSHOT_SECONDS` (2 by default) and quits — how a change to the
    /// interface is looked at without a person at the screen.
    pub fn snapshot_when_asked(window: &AppWindow) -> Option<Timer> {
        let path = std::env::var("JANIS_SNAPSHOT").ok()?;
        let seconds = std::env::var("JANIS_SNAPSHOT_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(2);
        let weak = window.as_weak();
        let timer = Timer::default();
        timer.start(
            TimerMode::SingleShot,
            Duration::from_secs(seconds),
            move || {
                if let Some(window) = weak.upgrade() {
                    match window.window().take_snapshot() {
                        Ok(buffer) => {
                            let saved = image::save_buffer(
                                &path,
                                buffer.as_bytes(),
                                buffer.width(),
                                buffer.height(),
                                image::ColorType::Rgba8,
                            );
                            if let Err(error) = saved {
                                eprintln!("janis: save snapshot {path}: {error}");
                            }
                        }
                        Err(error) => eprintln!("janis: take snapshot: {error}"),
                    }
                }
                let _ = slint::quit_event_loop();
            },
        );
        Some(timer)
    }
}
