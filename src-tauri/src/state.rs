use crate::search::SearchIndex;
use crate::types::AppConfig;
use notify::RecommendedWatcher;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::sync::Arc;

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub search_index: Mutex<Option<Arc<SearchIndex>>>,
    pub watcher: Mutex<Option<RecommendedWatcher>>,
    pub importing: AtomicBool,
    pub pending_open_file: Mutex<Option<String>>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Mutex::new(config),
            search_index: Mutex::new(None),
            watcher: Mutex::new(None),
            importing: AtomicBool::new(false),
            pending_open_file: Mutex::new(None),
        }
    }
}
