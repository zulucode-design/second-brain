use crate::search::SearchIndex;
use crate::types::AppConfig;
use notify::RecommendedWatcher;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub search_index: Mutex<Option<SearchIndex>>,
    pub watcher: Mutex<Option<RecommendedWatcher>>,
    pub importing: AtomicBool,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Mutex::new(config),
            search_index: Mutex::new(None),
            watcher: Mutex::new(None),
            importing: AtomicBool::new(false),
        }
    }
}
