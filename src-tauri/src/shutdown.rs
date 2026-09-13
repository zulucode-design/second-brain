use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShutdownIntent {
    HideMain,
    CloseWindow(String),
    ExitApp,
}

#[derive(Clone, Debug)]
struct PendingShutdown {
    request_id: String,
    intent: ShutdownIntent,
    waiting: HashSet<String>,
    timeout_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BeginOutcome {
    Started {
        request_id: String,
        notify: HashSet<String>,
        timeout_generation: u64,
    },
    Upgraded {
        request_id: String,
        notify: HashSet<String>,
        timeout_generation: u64,
    },
    Joined {
        request_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcknowledgeOutcome {
    Ignored,
    Pending,
    Cancelled,
    Complete(ShutdownIntent),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    InvalidCaller,
    InvalidReservation,
}

#[derive(Default)]
pub struct ShutdownState {
    pending: Option<PendingShutdown>,
    registered_participants: HashSet<String>,
    reservations: HashMap<String, String>,
    authorized_closes: HashSet<String>,
    authorized_exit: bool,
    vault_switch_pending: bool,
}

impl ShutdownState {
    pub fn reserve_note_window(&mut self, label: String, token: String) -> Result<(), String> {
        if self.vault_switch_pending
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.intent == ShutdownIntent::ExitApp)
        {
            return Err("Cannot open a note window during exit or vault switching".into());
        }
        if !label.starts_with("note-") || self.reservations.contains_key(&label) {
            return Err("Invalid or duplicate note-window reservation".into());
        }
        self.reservations.insert(label, token);
        Ok(())
    }

    pub fn cancel_reservation(&mut self, label: &str, token: &str) -> AcknowledgeOutcome {
        if self.reservations.get(label).map(String::as_str) != Some(token) {
            return AcknowledgeOutcome::Ignored;
        }
        self.reservations.remove(label);
        self.remove_participant(label)
    }

    pub fn register_participant(
        &mut self,
        caller_label: &str,
        token: Option<&str>,
    ) -> Result<Option<SaveBeforeCloseRequest>, RegisterError> {
        if caller_label == "main" {
            if token.is_some() {
                return Err(RegisterError::InvalidCaller);
            }
        } else if caller_label.starts_with("note-") {
            if self.vault_switch_pending {
                return Err(RegisterError::InvalidReservation);
            }
            if self.reservations.get(caller_label).map(String::as_str) != token {
                return Err(RegisterError::InvalidReservation);
            }
            self.reservations.remove(caller_label);
        } else {
            return Err(RegisterError::InvalidCaller);
        }
        self.registered_participants
            .insert(caller_label.to_string());
        if let Some(pending) = self.pending.as_mut() {
            if pending.intent == ShutdownIntent::ExitApp {
                pending.waiting.insert(caller_label.to_string());
                return Ok(Some(SaveBeforeCloseRequest {
                    request_id: pending.request_id.clone(),
                }));
            }
        }
        Ok(None)
    }

    pub fn unregister_participant(&mut self, label: &str) -> AcknowledgeOutcome {
        self.registered_participants.remove(label);
        self.reservations.remove(label);
        self.remove_participant(label)
    }

    fn remove_participant(&mut self, label: &str) -> AcknowledgeOutcome {
        let Some(pending) = self.pending.as_mut() else {
            return AcknowledgeOutcome::Ignored;
        };
        pending.waiting.remove(label);
        if !pending.waiting.is_empty() {
            return AcknowledgeOutcome::Pending;
        }
        let intent = pending.intent.clone();
        self.pending = None;
        AcknowledgeOutcome::Complete(intent)
    }

    pub fn is_registered(&self, label: &str) -> bool {
        self.registered_participants.contains(label)
    }
    pub fn registered(&self) -> HashSet<String> {
        self.registered_participants.clone()
    }
    pub fn all_participants(&self) -> HashSet<String> {
        self.registered_participants
            .iter()
            .chain(self.reservations.keys())
            .cloned()
            .collect()
    }

    pub fn begin_vault_switch(&mut self) -> Result<(), String> {
        if self.vault_switch_pending || self.pending.is_some() || !self.reservations.is_empty() {
            return Err(
                "A window lifecycle transition or note-window reservation is already pending"
                    .into(),
            );
        }
        self.vault_switch_pending = true;
        Ok(())
    }
    pub fn end_vault_switch(&mut self) {
        self.vault_switch_pending = false;
    }

    pub fn begin(
        &mut self,
        request_id: String,
        intent: ShutdownIntent,
        participants: HashSet<String>,
    ) -> BeginOutcome {
        if let Some(pending) = self.pending.as_mut() {
            if intent == ShutdownIntent::ExitApp && pending.intent != ShutdownIntent::ExitApp {
                pending.intent = ShutdownIntent::ExitApp;
                pending.timeout_generation += 1;
                let notify: HashSet<_> =
                    participants.difference(&pending.waiting).cloned().collect();
                pending.waiting.extend(participants);
                return BeginOutcome::Upgraded {
                    request_id: pending.request_id.clone(),
                    notify,
                    timeout_generation: pending.timeout_generation,
                };
            }
            return BeginOutcome::Joined {
                request_id: pending.request_id.clone(),
            };
        }
        self.pending = Some(PendingShutdown {
            request_id: request_id.clone(),
            intent,
            waiting: participants.clone(),
            timeout_generation: 0,
        });
        BeginOutcome::Started {
            request_id,
            notify: participants,
            timeout_generation: 0,
        }
    }

    pub fn acknowledge(
        &mut self,
        request_id: &str,
        window_label: &str,
        saved: bool,
    ) -> AcknowledgeOutcome {
        let Some(pending) = self.pending.as_mut() else {
            return AcknowledgeOutcome::Ignored;
        };
        if pending.request_id != request_id || !pending.waiting.contains(window_label) {
            return AcknowledgeOutcome::Ignored;
        }
        if !saved {
            self.pending = None;
            return AcknowledgeOutcome::Cancelled;
        }
        pending.waiting.remove(window_label);
        if !pending.waiting.is_empty() {
            return AcknowledgeOutcome::Pending;
        }
        let intent = pending.intent.clone();
        self.pending = None;
        AcknowledgeOutcome::Complete(intent)
    }

    pub fn cancel_if_generation(&mut self, request_id: &str, timeout_generation: u64) -> bool {
        if self.pending.as_ref().is_some_and(|pending| {
            pending.request_id == request_id && pending.timeout_generation == timeout_generation
        }) {
            self.pending = None;
            return true;
        }
        false
    }

    pub fn cancel(&mut self, request_id: &str) -> bool {
        if self
            .pending
            .as_ref()
            .map(|pending| pending.request_id.as_str())
            != Some(request_id)
        {
            return false;
        }
        self.pending = None;
        true
    }
    pub fn authorize_close(&mut self, label: String) {
        self.authorized_closes.insert(label);
    }
    pub fn consume_authorized_close(&mut self, label: &str) -> bool {
        self.authorized_closes.remove(label)
    }
    pub fn authorize_exit(&mut self) {
        self.authorized_exit = true;
    }
    pub fn consume_authorized_exit(&mut self) -> bool {
        std::mem::take(&mut self.authorized_exit)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveBeforeCloseRequest {
    pub request_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowReservation {
    pub label: String,
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn participants(labels: &[&str]) -> HashSet<String> {
        labels.iter().map(|label| (*label).to_string()).collect()
    }

    #[test]
    fn quit_supersedes_hide_without_changing_request_identity() {
        let mut state = ShutdownState::default();
        let first = state.begin(
            "first".into(),
            ShutdownIntent::HideMain,
            participants(&["main"]),
        );
        assert!(matches!(first, BeginOutcome::Started { .. }));
        let upgrade = state.begin(
            "second".into(),
            ShutdownIntent::ExitApp,
            participants(&["main", "note-1"]),
        );
        assert!(
            matches!(upgrade, BeginOutcome::Upgraded { request_id, .. } if request_id == "first")
        );
        assert_eq!(
            state.acknowledge("first", "main", true),
            AcknowledgeOutcome::Pending
        );
        assert_eq!(
            state.acknowledge("first", "note-1", true),
            AcknowledgeOutcome::Complete(ShutdownIntent::ExitApp)
        );
    }

    #[test]
    fn quit_supersedes_note_close_and_failure_cancels_the_exit() {
        let mut state = ShutdownState::default();
        state.begin(
            "close".into(),
            ShutdownIntent::CloseWindow("note-1".into()),
            participants(&["note-1"]),
        );
        state.begin(
            "quit".into(),
            ShutdownIntent::ExitApp,
            participants(&["main", "note-1"]),
        );
        assert_eq!(
            state.acknowledge("close", "main", false),
            AcknowledgeOutcome::Cancelled
        );
        assert_eq!(
            state.acknowledge("close", "note-1", true),
            AcknowledgeOutcome::Ignored
        );
    }

    #[test]
    fn reservation_is_atomic_with_exit_and_late_registration_joins() {
        let mut state = ShutdownState::default();
        state
            .reserve_note_window("note-1".into(), "secret".into())
            .unwrap();
        let all = state.all_participants();
        state.begin("exit".into(), ShutdownIntent::ExitApp, all);
        assert!(state
            .reserve_note_window("note-2".into(), "other".into())
            .is_err());
        let request = state
            .register_participant("note-1", Some("secret"))
            .unwrap()
            .unwrap();
        assert_eq!(request.request_id, "exit");
        assert_eq!(
            state.acknowledge("exit", "note-1", true),
            AcknowledgeOutcome::Complete(ShutdownIntent::ExitApp)
        );
    }

    #[test]
    fn registration_authenticates_label_and_token_and_destroy_cleans_up() {
        let mut state = ShutdownState::default();
        assert_eq!(
            state.register_participant("settings", None),
            Err(RegisterError::InvalidCaller)
        );
        state
            .reserve_note_window("note-1".into(), "right".into())
            .unwrap();
        assert_eq!(
            state.register_participant("note-1", Some("wrong")),
            Err(RegisterError::InvalidReservation)
        );
        state.unregister_participant("note-1");
        assert!(state.all_participants().is_empty());
    }

    #[test]
    fn stale_timeout_cannot_cancel_a_new_request() {
        let mut state = ShutdownState::default();
        state.begin(
            "first".into(),
            ShutdownIntent::HideMain,
            participants(&["main"]),
        );
        assert!(state.cancel("first"));
        state.begin(
            "second".into(),
            ShutdownIntent::ExitApp,
            participants(&["main"]),
        );
        assert!(!state.cancel("first"));
        assert_eq!(
            state.acknowledge("second", "main", true),
            AcknowledgeOutcome::Complete(ShutdownIntent::ExitApp)
        );
    }
}

#[cfg(test)]
mod lifecycle_race_tests {
    use super::*;
    use std::collections::HashSet;

    fn participants(labels: &[&str]) -> HashSet<String> {
        labels.iter().map(|label| (*label).to_string()).collect()
    }

    #[test]
    fn vault_switch_rejects_preexisting_reservations() {
        let mut state = ShutdownState::default();
        state
            .reserve_note_window("note-1".into(), "token".into())
            .unwrap();
        assert!(state.begin_vault_switch().is_err());
    }

    #[test]
    fn cancelling_last_reserved_exit_participant_completes_exit() {
        let mut state = ShutdownState::default();
        state
            .reserve_note_window("note-1".into(), "token".into())
            .unwrap();
        state.begin(
            "exit".into(),
            ShutdownIntent::ExitApp,
            participants(&["main", "note-1"]),
        );
        assert_eq!(
            state.acknowledge("exit", "main", true),
            AcknowledgeOutcome::Pending
        );
        assert_eq!(
            state.cancel_reservation("note-1", "token"),
            AcknowledgeOutcome::Complete(ShutdownIntent::ExitApp)
        );
    }

    #[test]
    fn upgraded_exit_invalidates_the_old_timeout_generation() {
        let mut state = ShutdownState::default();
        let first = state.begin(
            "request".into(),
            ShutdownIntent::HideMain,
            participants(&["main"]),
        );
        let old_generation = match first {
            BeginOutcome::Started {
                timeout_generation, ..
            } => timeout_generation,
            _ => unreachable!(),
        };
        let upgraded = state.begin(
            "ignored".into(),
            ShutdownIntent::ExitApp,
            participants(&["main", "note-1"]),
        );
        let new_generation = match upgraded {
            BeginOutcome::Upgraded {
                timeout_generation, ..
            } => timeout_generation,
            _ => unreachable!(),
        };
        assert_ne!(old_generation, new_generation);
        assert!(!state.cancel_if_generation("request", old_generation));
        assert!(state.cancel_if_generation("request", new_generation));
    }
}
