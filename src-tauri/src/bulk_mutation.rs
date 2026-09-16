use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};

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

#[derive(Clone, Debug, Serialize)]
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
        // Refuse a second bulk operation instead of queueing behind the first: a caller that
        // waited would run a full duplicate import or restore once the lease ahead of it drops.
        // Poisoning is reported separately, because "already running" would otherwise be
        // permanent after a panic and no restart would ever clear it.
        let gate = match self.gate.try_lock() {
            Ok(gate) => gate,
            Err(TryLockError::WouldBlock) => {
                return Err("Another bulk vault operation is already running".to_string())
            }
            Err(TryLockError::Poisoned(error)) => return Err(error.to_string()),
        };
        // Blocking here is bounded by a single in-flight note save, so an ordinary save in
        // progress delays the bulk operation rather than failing it.
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

    /// The gate must refuse a concurrent bulk operation outright. Before the gate used
    /// `try_lock` a second caller blocked and then ran a full duplicate operation, and no test
    /// could catch it: written against the old code, this one deadlocks instead of failing.
    #[test]
    fn a_live_lease_refuses_a_second_bulk_mutation_instead_of_queueing_it() {
        let coordinator = BulkMutationCoordinator::new();
        let notes = Mutex::new(());
        let syncing = AtomicBool::new(false);

        let lease = coordinator.acquire(&notes, &syncing).unwrap();
        let refused = coordinator.acquire(&notes, &syncing);
        assert!(refused.is_err(), "a second bulk mutation must be refused");

        // The refusal leaves the live lease intact: still suppressing, still marked active.
        assert!(coordinator.watcher_suppressed());
        assert!(syncing.load(Ordering::SeqCst));

        drop(lease);
        assert!(!coordinator.watcher_suppressed());
        assert!(!syncing.load(Ordering::SeqCst));

        // Once the first operation finishes the next one may proceed.
        assert!(coordinator.acquire(&notes, &syncing).is_ok());
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
