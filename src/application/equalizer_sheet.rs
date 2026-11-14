//! The controller's side of the equalizer sheet.

use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::{list_place, Application};
use crate::equalizer;
use crate::user_interface;

impl Application {
    /// Wires the `Equalizer` global.
    pub(super) fn connect_equalizer(self: &Rc<Self>) {
        let equalizer = self.window.global::<user_interface::Equalizer>();
        equalizer.set_gains(ModelRc::from(self.equalizer_gains.clone()));
        equalizer.set_presets(ModelRc::from(self.equalizer_presets.clone()));
        equalizer.set_gain_range(equalizer::GAIN_RANGE as f32);
        equalizer.set_frequency_labels(ModelRc::from(Rc::new(VecModel::from(
            equalizer::frequency_labels()
                .map(slint::SharedString::from)
                .to_vec(),
        ))));
        equalizer.on_band_changed({
            let weak = Rc::downgrade(self);
            move |index, gain| {
                if let (Some(application), Some(band)) = (weak.upgrade(), list_place(index)) {
                    let effects = application
                        .equalizer
                        .borrow_mut()
                        .set_band(band, f64::from(gain));
                    application.run(effects);
                    application.refresh_equalizer();
                }
            }
        });
        equalizer.on_preset_selected(self.handler(|application, name: slint::SharedString| {
            let effects = application.equalizer.borrow_mut().set_preset(&name);
            application.run(effects);
            application.refresh_equalizer();
        }));
        equalizer.on_reset(self.action(|application| {
            let effects = application.equalizer.borrow_mut().reset();
            application.run(effects);
            application.refresh_equalizer();
        }));
        equalizer.on_linear_phase_toggled(self.action(|application| {
            let effects = application.equalizer.borrow_mut().toggle_linear_phase();
            application.run(effects);
            application.refresh_equalizer();
        }));
    }

    /// Pushes the equalizer's state to its sheet.
    pub(super) fn refresh_equalizer(&self) {
        let equalizer = self.equalizer.borrow();
        let translations = self.translations.borrow();
        let view = self.window.global::<user_interface::Equalizer>();
        let gains = equalizer.gains().map(|gain| gain as f32);
        if self.equalizer_gains.row_count() == gains.len() {
            // Row by row: replacing the model would rebuild every slider,
            // the one under the pointer included, and end its drag.
            for (index, gain) in gains.into_iter().enumerate() {
                if self.equalizer_gains.row_data(index) != Some(gain) {
                    self.equalizer_gains.set_row_data(index, gain);
                }
            }
        } else {
            self.equalizer_gains.set_vec(gains.to_vec());
        }
        view.set_preset(equalizer.preset().into());
        view.set_linear_phase(equalizer.linear_phase());
        view.set_linear_phase_description(
            match equalizer.latency_milliseconds() {
                Some(milliseconds) => translations.translate(
                    "eq.linearPhase.latency",
                    &[("ms", milliseconds.to_string())],
                ),
                None => translations.text("eq.linearPhase.desc"),
            }
            .into(),
        );
    }

    /// The preset chips, whose labels only change with the language.
    pub(super) fn refresh_equalizer_presets(&self) {
        let translations = self.translations.borrow();
        self.equalizer_presets.set_vec(
            equalizer::PRESETS
                .iter()
                .map(|(name, _)| user_interface::EqualizerPreset {
                    name: (*name).into(),
                    label: translations.text(&equalizer::preset_label_key(name)).into(),
                })
                .collect::<Vec<_>>(),
        );
    }
}
