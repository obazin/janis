//! Where the application's log lines go: standard error.
//!
//! `JANIS_LOG` sets how much is said (`error`, `warn`, `info`, `debug` or
//! `trace`); warnings and errors are shown when it is unset.

use log::{Level, LevelFilter, Log, Metadata, Record};

/// The crates whose log lines follow `JANIS_LOG`.
const OWN_TARGETS: [&str; 3] = ["Janis", "janis", "audio_stack_rs"];

struct StandardErrorLogger;

static LOGGER: StandardErrorLogger = StandardErrorLogger;

impl Log for StandardErrorLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Libraries are only heard when something is wrong: their warnings
        // are about the files they read, not about Janis.
        let own = OWN_TARGETS
            .iter()
            .any(|target| metadata.target().starts_with(target));
        metadata.level()
            <= if own {
                log::max_level()
            } else {
                LevelFilter::Error
            }
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "{:5} {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

/// Installs the logger. Called once, first thing.
pub fn init() {
    let level = std::env::var("JANIS_LOG")
        .ok()
        .and_then(|value| value.parse::<Level>().ok())
        .map_or(LevelFilter::Warn, |level| level.to_level_filter());
    // Fails only when a logger is already installed, which leaves that one in place.
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}
