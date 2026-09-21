use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;
use tauri::{AppHandle, Manager, State};
use zip::write::SimpleFileOptions;

use crate::state::AppState;
use crate::types::AppConfig;

static DEVICE_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Z0-9]{7}(?:-[A-Z0-9]{7}){7}\b").expect("valid device id pattern")
});
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
        .expect("valid UUID pattern")
});
static TAILSCALE_IP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b100\.(?:6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])(?:\.\d{1,3}){2}\b")
        .expect("valid Tailscale address pattern")
});
static WINDOWS_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b[A-Z]:\\[^\r\n\"']+"#).expect("valid Windows path pattern")
});
static NOTE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:Projects|Areas|Resources|Archives|\.helixnotes)[/\\][^\r\n\"']+?\.md"#)
        .expect("valid note path pattern")
});

struct Redactor {
    replacements: HashMap<String, String>,
    next: usize,
}

impl Redactor {
    fn new(config: &AppConfig) -> Self {
        let mut redactor = Self {
            replacements: HashMap::new(),
            next: 1,
        };
        for value in [
            config.ai_api_key.as_deref(),
            config.openai_api_key.as_deref(),
            config.ollama_api_key.as_deref(),
            config.openai_compatible_api_key.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            redactor.add(value);
        }
        for vault in &config.vaults {
            redactor.add(&vault.path);
            // Restore stage, rollback, and journal paths are siblings of the vault (#142).
            // A filesystem root would redact every path separator, so it is skipped.
            if let Some(parent) = std::path::Path::new(&vault.path)
                .parent()
                .filter(|parent| parent.parent().is_some())
                .and_then(|parent| parent.to_str())
            {
                redactor.add(parent);
            }
            redactor.add(&vault.name);
            if let Some(value) = vault.vault_id.as_deref() {
                redactor.add(value);
            }
            if let Some(value) = vault.bookmark_id.as_deref() {
                redactor.add(value);
            }
            if let Some(value) = vault.notion.token.as_deref() {
                redactor.add(value);
            }
        }
        for value in [
            config.active_vault.as_deref(),
            config.active_bookmark_id.as_deref(),
            config.backup_location.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            redactor.add(value);
        }
        redactor
    }

    fn add(&mut self, value: &str) {
        if !value.is_empty() && !self.replacements.contains_key(value) {
            let placeholder = format!("<private-{}>", self.next);
            self.next += 1;
            self.replacements.insert(value.to_string(), placeholder);
        }
    }

    fn token(&mut self, value: &str) -> String {
        if let Some(existing) = self.replacements.get(value) {
            return existing.clone();
        }
        self.add(value);
        self.replacements[value].clone()
    }

    fn replace_pattern(&mut self, text: String, pattern: &Regex) -> String {
        pattern
            .replace_all(&text, |captures: &Captures<'_>| self.token(&captures[0]))
            .into_owned()
    }

    fn redact(&mut self, text: &str) -> String {
        let mut redacted = text.to_string();
        let mut known = self.replacements.clone().into_iter().collect::<Vec<_>>();
        known.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        for (value, placeholder) in known {
            redacted = redacted.replace(&value, &placeholder);
        }
        redacted = self.replace_pattern(redacted, &DEVICE_ID);
        redacted = self.replace_pattern(redacted, &UUID);
        redacted = self.replace_pattern(redacted, &TAILSCALE_IP);
        redacted = self.replace_pattern(redacted, &WINDOWS_PATH);
        self.replace_pattern(redacted, &NOTE_PATH)
    }
}

fn remove_secret_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for key in [
                "ai_api_key",
                "openai_api_key",
                "ollama_api_key",
                "openai_compatible_api_key",
                "api_key",
                "password",
                "token",
            ] {
                object.remove(key);
            }
            for child in object.values_mut() {
                remove_secret_fields(child);
            }
        }
        Value::Array(array) => {
            for child in array {
                remove_secret_fields(child);
            }
        }
        _ => {}
    }
}

fn write_entry(
    archive: &mut zip::ZipWriter<Cursor<Vec<u8>>>,
    name: &str,
    contents: &str,
) -> Result<(), String> {
    archive
        .start_file(
            name,
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(|error| error.to_string())?;
    archive
        .write_all(contents.as_bytes())
        .map_err(|error| error.to_string())
}

fn write_default_export(
    destination: &Path,
    log_dir: &Path,
    config: &AppConfig,
    sync: &Value,
    metadata: &Value,
) -> Result<(), String> {
    let mut redactor = Redactor::new(config);
    let mut config = serde_json::to_value(crate::secret_store::redacted_config(config))
        .map_err(|error| error.to_string())?;
    remove_secret_fields(&mut config);

    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, value) in [
        ("manifest.json", metadata),
        ("config.json", &config),
        ("sync.json", sync),
    ] {
        let contents = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
        write_entry(&mut archive, name, &redactor.redact(&contents))?;
    }

    let mut logs = fs::read_dir(log_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    logs.sort();
    for (index, path) in logs.into_iter().enumerate() {
        let contents = String::from_utf8_lossy(&fs::read(path).map_err(|error| error.to_string())?)
            .into_owned();
        write_entry(
            &mut archive,
            &format!("logs/log-{}.txt", index + 1),
            &redactor.redact(&contents),
        )?;
    }
    let bytes = archive
        .finish()
        .map_err(|error| error.to_string())?
        .into_inner();
    crate::durable::replace(destination, &bytes, crate::durable::Mode::Private)
}

fn os_version() -> String {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|contents| {
                contents.lines().find_map(|line| {
                    line.strip_prefix("PRETTY_NAME=")
                        .map(|value| value.trim_matches('"').to_string())
                })
            })
            .unwrap_or_else(|| "Linux (version unavailable)".to_string())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "ver"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Windows (version unavailable)".to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    std::env::consts::OS.to_string()
}

fn installer_type() -> &'static str {
    #[cfg(target_os = "windows")]
    return "nsis";
    #[cfg(target_os = "linux")]
    return "rpm";
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    return "unsupported";
}

#[tauri::command]
pub async fn export_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let config = state
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    let sync = match crate::sync_sidecar::sync_status(app.clone()).await {
        Ok(status) => serde_json::to_value(status).map_err(|error| error.to_string())?,
        Err(error) => serde_json::json!({ "available": false, "error": error }),
    };
    let metadata = serde_json::json!({
        "product": "Second Brain",
        "version": env!("CARGO_PKG_VERSION"),
        "buildCommit": env!("SECOND_BRAIN_BUILD_COMMIT"),
        "os": os_version(),
        "arch": std::env::consts::ARCH,
        "installerType": installer_type(),
    });
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|error| error.to_string())?;
    write_default_export(Path::new(&path), &log_dir, &config, &sync, &metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn default_export_excludes_vault_content_secrets_and_note_identity() {
        let root = std::env::temp_dir().join(format!("sb-diagnostics-{}", uuid::Uuid::new_v4()));
        let logs = root.join("logs");
        // Restore folders sit next to the vault (#142), so its parent reaches the log too.
        let vault = root.join("Home of Alice").join("Private Vault");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(vault.join("Projects")).unwrap();
        fs::write(
            vault.join("Projects/Acquisition plan.md"),
            "PLANTED NOTE BODY MUST NOT LEAVE",
        )
        .unwrap();

        let secret = "PLANTED-API-SECRET";
        fs::write(
            logs.join("second-brain.log"),
            format!(
                "opened {}/Projects/Acquisition plan.md note 5f934749-17da-4dd3-a219-88f9b9ef2277 with {secret}\n\
                 Cannot remove {}: permission denied",
                vault.display(),
                vault
                    .parent()
                    .unwrap()
                    .join(".second-brain-restore-stage-kept")
                    .display()
            ),
        )
        .unwrap();
        let mut config = AppConfig {
            active_vault: Some(vault.to_string_lossy().into_owned()),
            ai_api_key: Some(secret.to_string()),
            ..Default::default()
        };
        config.vaults.push(crate::types::VaultConfig {
            path: vault.to_string_lossy().into_owned(),
            name: "Private Vault".to_string(),
            vault_id: Some("5f934749-17da-4dd3-a219-88f9b9ef2277".to_string()),
            ..Default::default()
        });
        let destination = root.join("diagnostics.zip");

        write_default_export(
            &destination,
            &logs,
            &config,
            &serde_json::json!({
                "deviceId": "AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH",
                "peerAddress": "100.64.1.2",
                "lastTerminal": { "outcome": "failure", "error": format!("failed at {}/Projects/Acquisition plan.md", vault.display()) }
            }),
            &serde_json::json!({ "version": "0.1.0-alpha.1" }),
        )
        .unwrap();

        let file = fs::File::open(&destination).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut exported = String::new();
        let mut names = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            names.push(entry.name().to_string());
            entry.read_to_string(&mut exported).unwrap();
        }

        assert!(names.iter().any(|name| name == "manifest.json"));
        assert!(names.iter().any(|name| name == "config.json"));
        assert!(names.iter().any(|name| name == "sync.json"));
        assert!(names.iter().any(|name| name.starts_with("logs/")));
        for forbidden in [
            secret,
            "PLANTED NOTE BODY MUST NOT LEAVE",
            "Acquisition plan.md",
            "Private Vault",
            "Home of Alice",
            "5f934749-17da-4dd3-a219-88f9b9ef2277",
            "AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH",
            "100.64.1.2",
            "ai_api_key",
        ] {
            assert!(!exported.contains(forbidden), "export leaked {forbidden}");
        }

        fs::remove_dir_all(root).ok();
    }
}
