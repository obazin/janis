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

/// The compiled user interface, and the function that fills its `Strings`
/// global from a translation lookup.
mod user_interface {
    slint::include_modules!();
    include!(concat!(env!("OUT_DIR"), "/strings.rs"));
}

use std::process::ExitCode;

use slint::ComponentHandle;

use translations::{Language, Translations};
use user_interface::{AppWindow, Strings};

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
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .select()
        .map_err(|error| format!("select the windowing backend: {error}"))?;

    let directory = platform::data_directory()?;
    let database = persistence::init(directory)?;
    let preferences = persistence::get_preferences(&database)?;
    let translations = Translations::new(Language::from_code(&preferences.language));

    let window = AppWindow::new().map_err(|error| format!("create the window: {error}"))?;
    user_interface::apply_strings(&window.global::<Strings>(), &|key| translations.text(key));

    #[cfg(debug_assertions)]
    let _snapshot_timer = development::snapshot_when_asked(&window);

    window
        .run()
        .map_err(|error| format!("run the event loop: {error}"))
}

/// Development-only helpers, absent from release builds.
#[cfg(debug_assertions)]
mod development {
    use std::time::Duration;

    use slint::{ComponentHandle, Timer, TimerMode};

    use crate::user_interface::AppWindow;

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
