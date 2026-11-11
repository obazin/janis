//! Telling the listener that something failed: the toasts.

use slint::Timer;

use super::{with_application, Application};
use crate::notifications;
use crate::user_interface::Toast;

impl Application {
    pub(super) fn notify_error(
        &self,
        key: &'static str,
        parameters: Vec<(&'static str, String)>,
        dedupe_key: Option<&'static str>,
    ) {
        let raised = self
            .notifications
            .borrow_mut()
            .error(key, parameters, dedupe_key);
        let Some(id) = raised else {
            return;
        };
        self.refresh_toasts();
        Timer::single_shot(notifications::DEFAULT_TIMEOUT, move || {
            with_application(|application| application.dismiss_notification(id));
        });
    }

    pub(super) fn dismiss_notification(&self, id: u64) {
        self.notifications.borrow_mut().dismiss(id);
        self.refresh_toasts();
    }

    pub(super) fn refresh_toasts(&self) {
        let translations = self.translations.borrow();
        let toasts: Vec<Toast> = self
            .notifications
            .borrow()
            .items()
            .iter()
            .map(|notification| Toast {
                id: i32::try_from(notification.id).unwrap_or(i32::MAX),
                message: translations
                    .translate(notification.message_key, &notification.parameters)
                    .into(),
            })
            .collect();
        self.toasts.set_vec(toasts);
    }

    /// Tells the user when a choice could not be saved, instead of letting
    /// the next launch silently forget it.
    pub(super) fn saved(&self, what: &'static str, outcome: Result<(), String>) {
        if let Err(error) = outcome {
            log::error!("save {what}: {error}");
            self.notify_error("error.preferences", Vec::new(), Some(what));
        }
    }
}
