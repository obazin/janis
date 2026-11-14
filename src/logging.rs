//! Where the application's log lines go: standard error.
//!
//! `JANIS_LOG` sets how much is said (`error`, `warn`, `info`, `debug` or
//! `trace`); warnings and errors are shown when it is unset.

use log::{Level, LevelFilter, Log, Metadata, Record};

/// The crates whose log lines follow `JANIS_LOG`.
const OWN_TARGETS: [&str; 3] = ["Janis", "janis", "audio_stack_rs"];

struct StandardErrorLogger;

static LOGGER: StandardErrorLogger = StandardErrorLogger;

/// Whether a line of `level` from `target` is shown, with Janis's own lines
/// shown up to `own_level`. Libraries are only heard on an error: their
/// warnings are about the files they read, not about Janis.
fn heard(target: &str, level: Level, own_level: LevelFilter) -> bool {
    let own = OWN_TARGETS.iter().any(|own| target.starts_with(own));
    level <= if own { own_level } else { LevelFilter::Error }
}

/// How much Janis says, from `JANIS_LOG`: warnings and errors when it is
/// unset or names no level.
fn level_from(value: Option<&str>) -> LevelFilter {
    value
        .and_then(|value| value.parse::<Level>().ok())
        .map_or(LevelFilter::Warn, |level| level.to_level_filter())
}

impl Log for StandardErrorLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        heard(metadata.target(), metadata.level(), log::max_level())
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
    let level = level_from(std::env::var("JANIS_LOG").ok().as_deref());
    // Fails only when a logger is already installed, which leaves that one in place.
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn janis_log_names_a_level_and_anything_else_means_warnings() {
        assert_eq!(level_from(Some("debug")), LevelFilter::Debug);
        assert_eq!(level_from(Some("TRACE")), LevelFilter::Trace);
        assert_eq!(level_from(Some("loud")), LevelFilter::Warn);
        assert_eq!(level_from(None), LevelFilter::Warn);
    }

    #[test]
    fn janis_lines_follow_janis_log_and_a_librarys_only_its_errors() {
        assert!(heard(
            "Janis::application",
            Level::Debug,
            LevelFilter::Debug
        ));
        assert!(!heard(
            "Janis::application",
            Level::Debug,
            LevelFilter::Warn
        ));
        assert!(heard(
            "audio_stack_rs::engine",
            Level::Info,
            LevelFilter::Info
        ));
        assert!(!heard(
            "symphonia_core::probe",
            Level::Warn,
            LevelFilter::Trace
        ));
        assert!(heard(
            "symphonia_core::probe",
            Level::Error,
            LevelFilter::Warn
        ));
    }
}
