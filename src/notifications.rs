//! Application-wide, short-lived notifications — the one place a failure
//! becomes visible to the user instead of a silent log line.
//!
//! Messages are kept as a translation key and its parameters, not as resolved
//! text, so a notification already on screen follows a language change.

use std::collections::HashMap;
use std::time::Duration;

/// How long a notification stays before dismissing itself.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(7000);

#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub id: u64,
    pub message_key: &'static str,
    pub parameters: Vec<(&'static str, String)>,
}

#[derive(Default)]
pub struct Notifications {
    items: Vec<Notification>,
    last_id: u64,
    /// Dedupe key → the live notification it raised, so a storm collapses
    /// into one.
    keyed: HashMap<&'static str, u64>,
}

impl Notifications {
    pub fn items(&self) -> &[Notification] {
        &self.items
    }

    /// Raises an error notification and returns its id, for the caller to
    /// dismiss after [`DEFAULT_TIMEOUT`].
    ///
    /// `dedupe_key` collapses a burst into a single message — a disconnected
    /// network share failing hundreds of covers shows one notification, not
    /// hundreds. `None` is returned when the key is already on screen.
    pub fn error(
        &mut self,
        message_key: &'static str,
        parameters: Vec<(&'static str, String)>,
        dedupe_key: Option<&'static str>,
    ) -> Option<u64> {
        if dedupe_key.is_some_and(|key| self.keyed.contains_key(key)) {
            return None;
        }
        self.last_id += 1;
        let id = self.last_id;
        self.items.push(Notification {
            id,
            message_key,
            parameters,
        });
        if let Some(key) = dedupe_key {
            self.keyed.insert(key, id);
        }
        Some(id)
    }

    /// Removes a notification — by click or by timeout — and frees its dedupe
    /// key, so the same failure can be reported again later.
    pub fn dismiss(&mut self, id: u64) {
        self.items.retain(|notification| notification.id != id);
        self.keyed.retain(|_, keyed_id| *keyed_id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_with_the_same_key_shows_one_notification_until_it_is_dismissed() {
        let mut notifications = Notifications::default();

        let first = notifications.error("error.cover", Vec::new(), Some("cover-unavailable"));
        let repeat = notifications.error("error.cover", Vec::new(), Some("cover-unavailable"));

        assert!(first.is_some());
        assert_eq!(repeat, None);
        assert_eq!(notifications.items().len(), 1);

        notifications.dismiss(first.expect("the first one was raised"));
        assert!(notifications.items().is_empty());
        assert!(notifications
            .error("error.cover", Vec::new(), Some("cover-unavailable"))
            .is_some());
    }

    #[test]
    fn notifications_without_a_key_stack_and_dismiss_independently() {
        let mut notifications = Notifications::default();
        let parameters = |message: &str| vec![("message", message.to_owned())];

        let first = notifications.error("error.playback", parameters("a"), None);
        let second = notifications.error("error.playback", parameters("b"), None);

        assert_ne!(first, second);
        notifications.dismiss(first.expect("raised"));
        assert_eq!(notifications.items().len(), 1);
        assert_eq!(notifications.items()[0].parameters, parameters("b"));
    }
}
