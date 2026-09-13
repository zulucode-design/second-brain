use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

/// Process-wide gate and watcher suppression for operations that replace many vault files.
pub struct BulkMutationCoordinator {
    gate: Mutex<()>,
    watcher_suppressions: AtomicUsize,
}

pub struct BulkMutationLease<'a> {
    coordinator: &'a BulkMutationCoordinator,
    _gate: MutexGuard<'a, ()>,
    _note_mutation: MutexGuard<'a, ()>,
    vault_activity: &'a AtomicBool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BulkMutationOutcome {
    Success,
    ChangedIncomplete,
    Failure,
}

#[derive(Debug, Serialize)]
pub struct BulkMutationTerminal {
    pub success: bool,
    pub outcome: BulkMutationOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl BulkMutationTerminal {
    pub fn success() -> Self {
        Self {
            success: true,
            outcome: BulkMutationOutcome::Success,
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            outcome: BulkMutationOutcome::Failure,
            error: Some(error.into()),
        }
    }

    pub fn changed_incomplete(error: impl Into<String>) -> Self {
        Self {
            success: false,
            outcome: BulkMutationOutcome::ChangedIncomplete,
            error: Some(error.into()),
        }
    }
}

impl BulkMutationCoordinator {
    pub fn new() -> Self {
        Self {
            gate: Mutex::new(()),
            watcher_suppressions: AtomicUsize::new(0),
        }
    }

    pub fn acquire<'a>(
        &'a self,
        note_mutation: &'a Mutex<()>,
        vault_activity: &'a AtomicBool,
    ) -> Result<BulkMutationLease<'a>, String> {
        let gate = self.gate.lock().map_err(|error| error.to_string())?;
        let note_mutation = note_mutation.lock().map_err(|error| error.to_string())?;
        if vault_activity.swap(true, Ordering::SeqCst) {
            return Err("Another sync or bulk vault operation is already running".to_string());
        }
        self.watcher_suppressions.fetch_add(1, Ordering::SeqCst);
        Ok(BulkMutationLease {
            coordinator: self,
            _gate: gate,
            _note_mutation: note_mutation,
            vault_activity,
        })
    }

    pub fn watcher_suppressed(&self) -> bool {
        self.watcher_suppressions.load(Ordering::SeqCst) != 0
    }
}

impl Drop for BulkMutationLease<'_> {
    fn drop(&mut self) {
        self.coordinator
            .watcher_suppressions
            .fetch_sub(1, Ordering::SeqCst);
        self.vault_activity.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::{BulkMutationCoordinator, BulkMutationTerminal};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    #[test]
    fn dropping_a_lease_always_reenables_watcher_delivery() {
        let coordinator = BulkMutationCoordinator::new();
        let notes = Mutex::new(());
        let syncing = AtomicBool::new(false);
        {
            let _lease = coordinator.acquire(&notes, &syncing).unwrap();
            assert!(coordinator.watcher_suppressed());
            assert!(syncing.load(Ordering::SeqCst));
        }
        assert!(!coordinator.watcher_suppressed());
        assert!(!syncing.load(Ordering::SeqCst));
    }

    #[test]
    fn an_active_sync_excludes_a_bulk_mutation() {
        let coordinator = BulkMutationCoordinator::new();
        let notes = Mutex::new(());
        let syncing = AtomicBool::new(true);

        assert!(coordinator.acquire(&notes, &syncing).is_err());
        assert!(!coordinator.watcher_suppressed());
        assert!(syncing.load(Ordering::SeqCst));
    }

    #[test]
    fn terminal_payloads_have_exactly_three_unambiguous_outcomes() {
        assert_eq!(
            serde_json::to_value(BulkMutationTerminal::success()).unwrap(),
            serde_json::json!({
                "success": true, "outcome": "success"
            })
        );
        assert_eq!(
            serde_json::to_value(BulkMutationTerminal::failure("no change")).unwrap(),
            serde_json::json!({
                "success": false, "outcome": "failure", "error": "no change"
            })
        );
        assert_eq!(
            serde_json::to_value(BulkMutationTerminal::changed_incomplete("changed")).unwrap(),
            serde_json::json!({
                "success": false, "outcome": "changed-incomplete", "error": "changed"
            })
        );
    }
}
