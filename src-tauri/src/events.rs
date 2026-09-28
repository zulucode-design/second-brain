//! Tauri wire names. Keep in sync with src/lib/events.ts; tests check parity.
pub const AI_STREAM: &str = "ai-stream";
pub const AI_STATUS_CHANGED: &str = "ai-status-changed";
pub const AI_TEST_RESULT: &str = "ai-test-result";
pub const BACKUP_DONE: &str = "backup-done";
pub const EDITOR_FONT_SIZE_CHANGED: &str = "editor-font-size-changed";
pub const FILE_CHANGED: &str = "file-changed";
/// The hotkey's registration state changed, so settings never shows a stale one.
pub const HOTKEY_STATUS_CHANGED: &str = "hotkey-status-changed";
pub const IMPORT_DONE: &str = "import-done";
pub const NOTION_PUBLISH_FAILED: &str = "notion-publish-failed";
pub const NOTION_PUBLISH_FINISHED: &str = "notion-publish-finished";
pub const NOTION_PUBLISH_PROGRESS: &str = "notion-publish-progress";
pub const OPEN_FILE: &str = "open-file";
/// The capture window was shown, so its field can take the caret. The window cannot infer
/// this: it is shown and hidden repeatedly without ever being reloaded.
pub const QUICK_CAPTURE_SHOWN: &str = "quick-capture-shown";
pub const REPAIR_STATUS_CHANGED: &str = "repair-status-changed";
pub const RESTORE_DONE: &str = "restore-done";
pub const SAVE_BEFORE_CLOSE: &str = "save-before-close";
pub const SAVE_CLOSE_RELEASED: &str = "save-close-released";
pub const SYNC_DONE: &str = "sync-done";
pub const UI_SCALE_CHANGED: &str = "ui-scale-changed";
