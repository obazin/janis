//! The controller's side of the Settings screen: the playback switches and
//! the interface language.

use std::rc::Rc;

use slint::ComponentHandle;

use super::Application;
use crate::persistence::{self, PlaybackOption};
use crate::translations::Language;
use crate::user_interface;

impl Application {
    pub(super) fn connect_settings(self: &Rc<Self>) {
        let view = self.window.global::<user_interface::Settings>();
        view.on_option_toggled(self.handler(|application, option: slint::SharedString| {
            let option = match option.as_str() {
                "gapless" => PlaybackOption::Gapless,
                "crossfade" => PlaybackOption::Crossfade,
                "normalize" => PlaybackOption::Normalize,
                "exclusive" => PlaybackOption::Exclusive,
                _ => return,
            };
            let effects = application.playback_options.borrow_mut().toggle(option);
            application.run(effects);
            application.refresh_settings();
        }));
        view.on_language_selected(self.handler(|application, code: slint::SharedString| {
            application.set_language(Language::from_code(&code));
        }));
    }

    /// Switches the interface language: every label, and every text the
    /// controller formatted, follows at once.
    fn set_language(&self, language: Language) {
        self.translations.borrow_mut().set_current(language);
        self.saved(
            "language",
            persistence::set_language(&self.database, language.code()),
        );
        self.apply_strings();
        self.refresh_everything();
    }

    pub(super) fn refresh_settings(&self) {
        let options = self.playback_options.borrow();
        let view = self.window.global::<user_interface::Settings>();
        view.set_gapless(options.enabled(PlaybackOption::Gapless));
        view.set_crossfade(options.enabled(PlaybackOption::Crossfade));
        view.set_normalize(options.enabled(PlaybackOption::Normalize));
        view.set_exclusive(options.enabled(PlaybackOption::Exclusive));
        view.set_language(self.translations.borrow().current().code().into());
    }
}
