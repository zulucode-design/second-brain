//! Opt-in performance probe for the external-alpha budgets (issue #88).
//!
//! Setting `SECOND_BRAIN_PERF_LOG` to a file path makes the app append one JSON object per
//! measurement to that file. Unset, every call is a cheap no-op and nothing is written.
//! Samples carry timings only: never note content, titles, paths, or queries.

use std::io::Write;
use std::path::PathBuf;

pub const LOG_ENV: &str = "SECOND_BRAIN_PERF_LOG";
/// When also set to `1`, the app exits through its normal save-aware path after it reports
/// startup readiness, so a harness can repeat cold starts without killing the process.
pub const EXIT_AFTER_STARTUP_ENV: &str = "SECOND_BRAIN_PERF_EXIT_AFTER_STARTUP";

pub fn log_path() -> Option<PathBuf> {
    std::env::var_os(LOG_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn exit_after_startup() -> bool {
    std::env::var(EXIT_AFTER_STARTUP_ENV).as_deref() == Ok("1")
}

/// Appends `sample` with a wall-clock `epochMs`, so harness launch times and in-app marks share
/// one clock. Failures are logged, not returned: a probe must never break the app it measures.
pub fn record(mut sample: serde_json::Value) {
    let Some(path) = log_path() else { return };
    append(&path, &mut sample);
}

fn append(path: &std::path::Path, sample: &mut serde_json::Value) {
    if let Some(object) = sample.as_object_mut() {
        object.insert(
            "epochMs".into(),
            chrono::Utc::now().timestamp_millis().into(),
        );
    }
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{sample}"));
    if let Err(error) = written {
        log::warn!("Could not write a performance sample: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_append_as_json_lines_with_a_wall_clock() {
        let path = std::env::temp_dir().join(format!("perf-probe-{}.jsonl", uuid::Uuid::new_v4()));
        append(&path, &mut serde_json::json!({ "kind": "a", "ms": 1.5 }));
        append(&path, &mut serde_json::json!({ "kind": "b" }));
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["kind"], "a");
        assert_eq!(lines[1]["kind"], "b");
        assert!(lines[0]["epochMs"].as_i64().unwrap() > 1_700_000_000_000);
    }
}
