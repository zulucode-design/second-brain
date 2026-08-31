use crate::ai_health::AiStatus;
use crate::search::SearchIndex;
use crate::types::AppConfig;
use crate::vault::repair::RepairStatus;
use crate::vault::watcher::VaultWatcher;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex;

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub search_index: Mutex<Option<Arc<SearchIndex>>>,
    pub watcher: Mutex<Option<VaultWatcher>>,
    pub vault_transition: tokio::sync::Mutex<()>,
    pub importing: AtomicBool,
    pub syncing: AtomicBool,
    pub pending_open_file: Mutex<Option<String>>,
    /// Serializes note lifecycle mutations so an older search/index side effect cannot
    /// land after a newer move, delete, restore, or save.
    pub note_mutation: Mutex<()>,
    pub repair_status: Mutex<RepairStatus>,
    pub app_handle: Mutex<Option<tauri::AppHandle>>,
    /// Last known reachability of the AI backend, kept current by a background poller so
    /// features can be shown as unavailable without each one having to find out itself.
    pub ai_status: Mutex<AiStatus>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Mutex::new(config),
            search_index: Mutex::new(None),
            watcher: Mutex::new(None),
            vault_transition: tokio::sync::Mutex::new(()),
            importing: AtomicBool::new(false),
            syncing: AtomicBool::new(false),
            pending_open_file: Mutex::new(None),
            note_mutation: Mutex::new(()),
            repair_status: Mutex::new(RepairStatus::default()),
            app_handle: Mutex::new(None),
            ai_status: Mutex::new(AiStatus::unknown()),
        }
    }
}
