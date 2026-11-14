//! The playback switches (gapless / crossfade / normalize / exclusive).
//!
//! A toggle is saved, and the switches the engine acts on are pushed to it as
//! well. Gapless, crossfade and normalization are all live in the engine;
//! exclusive output is a stored preference only.

use crate::effects::Effect;
use crate::persistence::{PlaybackOption, Preferences};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackOptions {
    gapless: bool,
    crossfade: bool,
    normalize: bool,
    exclusive: bool,
}

/// What a fresh database holds, until `restore` reads the saved switches.
impl Default for PlaybackOptions {
    fn default() -> Self {
        Self {
            gapless: true,
            crossfade: true,
            normalize: true,
            exclusive: false,
        }
    }
}

impl PlaybackOptions {
    pub fn enabled(&self, option: PlaybackOption) -> bool {
        match option {
            PlaybackOption::Gapless => self.gapless,
            PlaybackOption::Crossfade => self.crossfade,
            PlaybackOption::Normalize => self.normalize,
            PlaybackOption::Exclusive => self.exclusive,
        }
    }

    /// Boot hydration from the preferences row.
    pub fn restore(&mut self, preferences: &Preferences) -> Vec<Effect> {
        *self = Self {
            gapless: preferences.gapless,
            crossfade: preferences.crossfade,
            normalize: preferences.normalize,
            exclusive: preferences.exclusive,
        };
        [
            PlaybackOption::Gapless,
            PlaybackOption::Crossfade,
            PlaybackOption::Normalize,
        ]
        .into_iter()
        .filter_map(|option| engine_effect(option, self.enabled(option)))
        .collect()
    }

    pub fn toggle(&mut self, option: PlaybackOption) -> Vec<Effect> {
        let enabled = !self.enabled(option);
        match option {
            PlaybackOption::Gapless => self.gapless = enabled,
            PlaybackOption::Crossfade => self.crossfade = enabled,
            PlaybackOption::Normalize => self.normalize = enabled,
            PlaybackOption::Exclusive => self.exclusive = enabled,
        }
        let mut effects = vec![Effect::PersistPlaybackOption { option, enabled }];
        effects.extend(engine_effect(option, enabled));
        effects
    }
}

/// Tells the engine about a switch it acts on.
fn engine_effect(option: PlaybackOption, enabled: bool) -> Option<Effect> {
    match option {
        PlaybackOption::Gapless => Some(Effect::SetGapless(enabled)),
        PlaybackOption::Crossfade => Some(Effect::SetCrossfade(enabled)),
        PlaybackOption::Normalize => Some(Effect::SetNormalize(enabled)),
        PlaybackOption::Exclusive => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preferences() -> Preferences {
        Preferences {
            volume: 0.8,
            eq_gains: vec![0.0; 10],
            eq_preset: "flat".to_owned(),
            eq_linear_phase: false,
            gapless: false,
            crossfade: true,
            normalize: false,
            exclusive: true,
            language: "en".to_owned(),
        }
    }

    #[test]
    fn restoring_pushes_the_three_engine_switches_without_saving_them_back() {
        let mut options = PlaybackOptions::default();

        let effects = options.restore(&preferences());

        assert_eq!(
            effects,
            [
                Effect::SetGapless(false),
                Effect::SetCrossfade(true),
                Effect::SetNormalize(false),
            ]
        );
        assert!(options.enabled(PlaybackOption::Exclusive));
    }

    #[test]
    fn a_toggle_is_saved_and_reaches_the_engine_when_the_engine_acts_on_it() {
        let mut options = PlaybackOptions::default();

        // Crossfade starts on.
        assert_eq!(
            options.toggle(PlaybackOption::Crossfade),
            [
                Effect::PersistPlaybackOption {
                    option: PlaybackOption::Crossfade,
                    enabled: false,
                },
                Effect::SetCrossfade(false),
            ]
        );
        // Exclusive output is remembered, but nothing in the engine reads it.
        assert_eq!(
            options.toggle(PlaybackOption::Exclusive),
            [Effect::PersistPlaybackOption {
                option: PlaybackOption::Exclusive,
                enabled: true,
            }]
        );
        assert!(options.enabled(PlaybackOption::Exclusive));
    }
}
