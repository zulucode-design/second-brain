use crate::types::{
    NoteContent, NoteEntry, NoteMeta, NoteTitleEntry, NotebookEntry, TrashContents,
    TrashNotebookEntry, VaultState,
};
use crate::vault::frontmatter;
use crate::vault::para::{self, ParaCategory};
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;
use walkdir::WalkDir;

pub fn helixnotes_dir(vault_path: &str) -> PathBuf {
    Path::new(vault_path).join(".helixnotes")
}

fn canonicalize_path(path: &Path, label: &str) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("Invalid {label}: {error}"))
}

fn ensure_vault_content_path(
    vault_path: &str,
    requested_path: &Path,
    allow_root: bool,
) -> Result<PathBuf, String> {
    let vault = canonicalize_path(Path::new(vault_path), "vault path")?;
    let requested = canonicalize_path(requested_path, "vault item path")?;
    let metadata = vault.join(".helixnotes");

    if !requested.starts_with(&vault)
        || requested.starts_with(&metadata)
        || (!allow_root && requested == vault)
    {
        return Err("Path must stay inside the active vault".to_string());
    }

    Ok(requested_path.to_path_buf())
}

fn ensure_vault_content_dir(vault_path: &str, requested_path: &Path) -> Result<PathBuf, String> {
    let requested = ensure_vault_content_path(vault_path, requested_path, true)?;
    if !requested.is_dir() {
        return Err("Vault destination is not a directory".to_string());
    }
    Ok(requested)
}

fn ensure_note_path(vault_path: &str, requested_path: &Path) -> Result<PathBuf, String> {
    let requested = ensure_vault_content_path(vault_path, requested_path, false)?;
    if !requested.is_file()
        || requested
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("md")
    {
        return Err("Note path must point to a Markdown file".to_string());
    }
    Ok(requested)
}

fn ensure_readable_note_path(vault_path: &str, requested_path: &Path) -> Result<PathBuf, String> {
    if let Ok(note) = ensure_note_path(vault_path, requested_path) {
        return Ok(note);
    }

    let trashed_note = ensure_trash_entry(vault_path, requested_path)?;
    if !trashed_note.is_file()
        || trashed_note
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("md")
    {
        return Err("Note path must point to a Markdown file".to_string());
    }
    Ok(trashed_note)
}

fn ensure_notebook_path(vault_path: &str, requested_path: &Path) -> Result<PathBuf, String> {
    let requested = ensure_vault_content_path(vault_path, requested_path, false)?;
    if !requested.is_dir() {
        return Err("Notebook path must point to a directory".to_string());
    }
    Ok(requested)
}

fn ensure_trash_entry(vault_path: &str, requested_path: &Path) -> Result<PathBuf, String> {
    let trash = canonicalize_path(&helixnotes_dir(vault_path).join("trash"), "trash path")?;
    let requested = canonicalize_path(requested_path, "trash item path")?;
    if requested == trash || !requested.starts_with(&trash) {
        return Err("Path must be an item inside the active vault trash".to_string());
    }
    Ok(requested_path.to_path_buf())
}

fn safe_relative_path(path: &str) -> Result<&Path, String> {
    let relative = Path::new(path);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err("Path must be a safe vault-relative path".to_string());
    }
    Ok(relative)
}

fn safe_child_name(name: &str) -> Result<&str, String> {
    let mut components = Path::new(name).components();
    if name.trim().is_empty()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err("Name must not contain path separators".to_string());
    }
    Ok(name)
}

pub fn ensure_vault_structure(vault_path: &str) -> Result<(), String> {
    let hn_dir = helixnotes_dir(vault_path);
    fs::create_dir_all(hn_dir.join("trash")).map_err(|e| e.to_string())?;
    fs::create_dir_all(hn_dir.join("attachments")).map_err(|e| e.to_string())?;

    // Runs on every open, not just on creation, so a vault made before PARA gains the
    // four folders.
    para::ensure_scaffold(vault_path)?;

    let config_path = hn_dir.join("config.json");
    if !config_path.exists() {
        let config = serde_json::json!({
            "version": "0.1.0",
            "created": Utc::now().to_rfc3339()
        });
        fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap())
            .map_err(|e| e.to_string())?;
    }

    let state_path = hn_dir.join("state.json");
    if !state_path.exists() {
        let state = VaultState::default();
        fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap())
            .map_err(|e| e.to_string())?;
    }

    let gitignore_path = hn_dir.join(".gitignore");
    if !gitignore_path.exists() {
        fs::write(&gitignore_path, "trash/\nindex.json\nstate.json\n")
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn digit_run(value: &[u8], start: usize) -> (usize, &[u8]) {
    let mut end = start;
    while end < value.len() && value[end].is_ascii_digit() {
        end += 1;
    }
    let mut significant = start;
    while significant + 1 < end && value[significant] == b'0' {
        significant += 1;
    }
    (end, &value[significant..end])
}

fn compare_natural_names(left: &str, right: &str) -> Ordering {
    let left_lower = left.to_lowercase();
    let right_lower = right.to_lowercase();
    let left = left_lower.as_bytes();
    let right = right_lower.as_bytes();
    let (mut left_index, mut right_index) = (0, 0);
    let mut leading_zero_order = Ordering::Equal;

    while left_index < left.len() && right_index < right.len() {
        if left[left_index].is_ascii_digit() && right[right_index].is_ascii_digit() {
            let (left_end, left_number) = digit_run(left, left_index);
            let (right_end, right_number) = digit_run(right, right_index);
            let number_order = left_number
                .len()
                .cmp(&right_number.len())
                .then_with(|| left_number.cmp(right_number));
            if number_order != Ordering::Equal {
                return number_order;
            }
            if leading_zero_order == Ordering::Equal {
                leading_zero_order = (left_end - left_index).cmp(&(right_end - right_index));
            }
            left_index = left_end;
            right_index = right_end;
        } else {
            let character_order = left[left_index].cmp(&right[right_index]);
            if character_order != Ordering::Equal {
                return character_order;
            }
            left_index += 1;
            right_index += 1;
        }
    }

    match (left_index == left.len(), right_index == right.len()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => leading_zero_order,
    }
}

pub fn scan_notebooks(vault_path: &str) -> Result<Vec<NotebookEntry>, String> {
    let root = Path::new(vault_path);
    if !root.exists() {
        return Err("Vault path does not exist".to_string());
    }
    Ok(scan_dir_recursive(root, vault_path))
}

fn scan_dir_recursive(dir: &Path, vault_root: &str) -> Vec<NotebookEntry> {
    let root = Path::new(vault_root);

    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };

    // Collect subdirs (root level only lists directories, note counts come from scan_dir_with_count)
    let mut dirs: Vec<_> = read_dir
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false) && !is_hidden(&e.path()))
        .collect();

    dirs.sort_by(|a, b| {
        compare_natural_names(
            &a.file_name().to_string_lossy(),
            &b.file_name().to_string_lossy(),
        )
    });

    // Scan sibling notebooks in parallel: each subtree is independent, so
    // overlapping the per-directory read_dir calls hides FUSE latency on Android.
    // par_iter().collect() preserves the (already-sorted) order.
    let paths: Vec<PathBuf> = dirs.iter().map(|e| e.path()).collect();
    paths
        .par_iter()
        .map(|path| {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            let (children, child_note_count) = scan_dir_with_count(path, vault_root);
            NotebookEntry {
                name,
                path: path.to_string_lossy().to_string(),
                relative_path: relative,
                children,
                note_count: child_note_count,
            }
        })
        .collect()
}

/// Combined scan: returns (children, note_count) in a single read_dir pass.
fn scan_dir_with_count(dir: &Path, vault_root: &str) -> (Vec<NotebookEntry>, usize) {
    let root = Path::new(vault_root);

    let Ok(read_dir) = fs::read_dir(dir) else {
        return (Vec::new(), 0);
    };

    let mut subdirs = Vec::new();
    let mut note_count = 0usize;
    for entry in read_dir.filter_map(|e| e.ok()) {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() && !is_hidden(&entry.path()) {
            subdirs.push(entry);
        } else if ft.is_file() && entry.path().extension().and_then(|x| x.to_str()) == Some("md") {
            note_count += 1;
        }
    }

    subdirs.sort_by(|a, b| {
        compare_natural_names(
            &a.file_name().to_string_lossy(),
            &b.file_name().to_string_lossy(),
        )
    });

    let paths: Vec<PathBuf> = subdirs.iter().map(|e| e.path()).collect();
    let entries: Vec<NotebookEntry> = paths
        .par_iter()
        .map(|path| {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            let (children, child_note_count) = scan_dir_with_count(path, vault_root);
            NotebookEntry {
                name,
                path: path.to_string_lossy().to_string(),
                relative_path: relative,
                children,
                note_count: child_note_count,
            }
        })
        .collect();

    (entries, note_count)
}

pub(crate) fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            n.starts_with('.')
                || matches!(
                    n,
                    "_res" | "_resources" | "_attachments" | "_assets" | "assets" | "node_modules"
                )
        })
        .unwrap_or(false)
}

pub fn count_root_notes(vault_path: &str) -> Result<usize, String> {
    let root = Path::new(vault_path);
    if !root.exists() {
        return Err("Vault path does not exist".to_string());
    }
    let count = fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().map(|ft| ft.is_file()).unwrap_or(false)
                && e.path().extension().and_then(|x| x.to_str()) == Some("md")
        })
        .count();
    Ok(count)
}

pub fn scan_notes(vault_path: &str, notebook_path: Option<&str>) -> Result<Vec<NoteEntry>, String> {
    let scan_path = notebook_path.unwrap_or(vault_path);
    let root = Path::new(scan_path);
    ensure_vault_content_dir(vault_path, root)?;
    let vault_root = Path::new(vault_path);

    log::info!(
        "scan_notes: vault={}, scan={}, exists={}",
        vault_path,
        scan_path,
        root.exists()
    );

    if !root.exists() {
        return Err("Path does not exist".to_string());
    }

    // On mobile, use metadata-only scan (no file reads) for fast listing on sandboxed FS
    #[cfg(mobile)]
    {
        // Collect candidate .md paths first, then read their metadata in parallel.
        // Overlapping the per-file reads hides FUSE latency on Android (matches the
        // desktop path below). The previous version also did an extra read_dir purely
        // for debug logging, which doubled the directory I/O on the sandboxed FS.
        let paths: Vec<PathBuf> = if notebook_path.is_some() {
            match fs::read_dir(root) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("md"))
                    .collect(),
                Err(_) => Vec::new(),
            }
        } else {
            let hn_dir = helixnotes_dir(vault_path);
            WalkDir::new(root)
                .into_iter()
                // filter_entry skips descending into hidden dirs (unlike filter)
                .filter_entry(|e| !is_hidden(e.path()) && !e.path().starts_with(&hn_dir))
                .filter_map(|e| e.ok())
                .map(|e| e.path().to_path_buf())
                .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("md"))
                .collect()
        };

        let mut notes: Vec<NoteEntry> = paths
            .par_iter()
            .filter_map(|path| read_note_entry_metadata_only(path, vault_root).ok())
            .collect();

        log::info!("scan_notes: mobile scan found {} notes", notes.len());
        notes.sort_by_key(|note| std::cmp::Reverse(note.meta.modified));
        return Ok(notes);
    }

    #[cfg(desktop)]
    {
        let md_files: Vec<PathBuf> = if notebook_path.is_some() {
            fs::read_dir(root)
                .map_err(|e| e.to_string())?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("md"))
                .collect()
        } else {
            WalkDir::new(root)
                .into_iter()
                .filter_entry(|e| {
                    !is_hidden(e.path()) && !e.path().starts_with(helixnotes_dir(vault_path))
                })
                .filter_map(|e| e.ok())
                .map(|e| e.path().to_path_buf())
                .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("md"))
                .collect()
        };

        let mut notes: Vec<NoteEntry> = md_files
            .par_iter()
            .filter_map(|path| read_note_entry_fast(path, vault_root).ok())
            .collect();

        notes.sort_by_key(|note| std::cmp::Reverse(note.meta.modified));
        Ok(notes)
    }
}

fn read_note_entry(path: &Path, vault_root: &Path) -> Result<NoteEntry, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    read_note_entry_from_str(&raw, path, vault_root)
}

/// Android-only: reads just the frontmatter (first 2KB) for tags/title/pinned,
/// uses filesystem timestamps for dates. No preview text.
#[cfg(mobile)]
fn read_note_entry_metadata_only(path: &Path, vault_root: &Path) -> Result<NoteEntry, String> {
    // Read first 2KB - enough for frontmatter with tags, title, pinned
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 2048];
    let bytes_read = file.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(bytes_read);
    let raw = String::from_utf8_lossy(&buf);

    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let (mut meta, content) = frontmatter::parse_note(&raw, &filename);

    // Always use filesystem timestamps on Android (faster than parsing date strings)
    if let Ok(fs_meta) = fs::metadata(path) {
        if let Ok(m) = fs_meta.modified() {
            meta.modified = m.into();
        }
        if let Ok(c) = fs_meta.created() {
            meta.created = c.into();
        }
    }

    let relative = path
        .strip_prefix(vault_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();

    let preview = frontmatter::extract_preview(&content, 120);

    Ok(NoteEntry {
        path: path.to_string_lossy().to_string(),
        relative_path: relative,
        meta,
        preview,
    })
}

/// Fast version: reads only the first ~2KB of the file (enough for frontmatter + preview).
fn read_note_entry_fast(path: &Path, vault_root: &Path) -> Result<NoteEntry, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 2048];
    let bytes_read = file.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(bytes_read);
    let raw = String::from_utf8_lossy(&buf);
    read_note_entry_from_str(&raw, path, vault_root)
}

fn read_note_entry_from_str(
    raw: &str,
    path: &Path,
    vault_root: &Path,
) -> Result<NoteEntry, String> {
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let (mut meta, content) = frontmatter::parse_note(raw, &filename);

    // Use filesystem times only if frontmatter didn't have created/modified fields
    let has_created = raw.contains("\ncreated:") || raw.starts_with("created:");
    let has_modified = raw.contains("\nmodified:") || raw.starts_with("modified:");
    if let Ok(fs_meta) = fs::metadata(path) {
        if !has_modified {
            if let Ok(modified) = fs_meta.modified() {
                meta.modified = modified.into();
            }
        }
        if !has_created {
            if let Ok(created) = fs_meta.created() {
                meta.created = created.into();
            }
        }
    }

    let relative = path
        .strip_prefix(vault_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();

    let preview = frontmatter::extract_preview(&content, 120);

    Ok(NoteEntry {
        path: path.to_string_lossy().to_string(),
        relative_path: relative,
        meta,
        preview,
    })
}

pub fn read_note(vault_path: &str, path: &str) -> Result<NoteContent, String> {
    let validated = ensure_readable_note_path(vault_path, Path::new(path))?;
    read_note_content(&validated, path)
}

pub fn read_vault_note(vault_path: &str, path: &str) -> Result<NoteContent, String> {
    let validated = ensure_note_path(vault_path, Path::new(path))?;
    read_note_content(&validated, path)
}

fn read_note_content(validated: &Path, reported_path: &str) -> Result<NoteContent, String> {
    let p = validated;
    let raw = fs::read_to_string(p).map_err(|e| e.to_string())?;
    let filename = p
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let (mut meta, content) = frontmatter::parse_note(&raw, &filename);

    // Fall back to filesystem times when the frontmatter has no created/modified, matching the
    // note-list scan (read_note_entry_from_str). Without this, a missing `created:` defaults to
    // Utc::now() here and then gets written to disk on the next save (e.g. a title change),
    // resetting the create date of imported/frontmatter-less notes. (issue #139)
    let has_created = raw.contains("\ncreated:") || raw.starts_with("created:");
    let has_modified = raw.contains("\nmodified:") || raw.starts_with("modified:");
    if let Ok(fs_meta) = fs::metadata(p) {
        if !has_modified {
            if let Ok(modified) = fs_meta.modified() {
                meta.modified = modified.into();
            }
        }
        if !has_created {
            if let Ok(created) = fs_meta.created() {
                meta.created = created.into();
            }
        }
    }

    Ok(NoteContent {
        path: reported_path.to_string(),
        meta,
        content,
        raw,
    })
}

pub fn save_note(vault_path: &str, path: &str, meta: &NoteMeta, body: &str) -> Result<(), String> {
    let path = ensure_note_path(vault_path, Path::new(path))?;
    let mut updated_meta = meta.clone();
    updated_meta.modified = Utc::now();

    // Generate UUID on first save if note didn't have one
    if updated_meta.id.is_empty() {
        updated_meta.id = Uuid::new_v4().to_string();
    }

    // Read existing file to preserve unknown frontmatter fields
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let raw = if existing.is_empty() {
        frontmatter::update_note_raw(&updated_meta, body)
    } else {
        frontmatter::merge_frontmatter(&existing, &updated_meta, body)
    };

    fs::write(path, raw).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn create_note(
    vault_path: &str,
    notebook_relative: Option<&str>,
    title: &str,
) -> Result<NoteEntry, String> {
    // Every note is filed under exactly one PARA category, so the destination has to
    // resolve to one. Refusing here is what keeps an uncategorised note from ever being
    // created in the first place, and it means the vault root is no longer a valid home.
    let uncategorised = || {
        "Notes must be filed under a PARA category: Projects, Areas, Resources, or Archives"
            .to_string()
    };
    let notebook_relative = notebook_relative.ok_or_else(uncategorised)?;
    let category = para::category_for_relative_path(notebook_relative).ok_or_else(uncategorised)?;

    let requested_dir = Path::new(vault_path).join(safe_relative_path(notebook_relative)?);
    let dir = ensure_vault_content_dir(vault_path, &requested_dir)?;

    let filename = sanitize_filename(title);
    let mut file_path = dir.join(format!("{}.md", filename));

    // Deduplicate filename
    let mut counter = 1;
    while file_path.exists() {
        file_path = dir.join(format!("{} {}.md", filename, counter));
        counter += 1;
    }

    let now = Utc::now();
    let meta = NoteMeta {
        id: Uuid::new_v4().to_string(),
        title: title.to_string(),
        tags: Vec::new(),
        pinned: false,
        created: now,
        modified: now,
        category: Some(category),
    };

    let raw = frontmatter::update_note_raw(&meta, "\n");
    fs::write(&file_path, raw).map_err(|e| e.to_string())?;

    let vault_root = Path::new(vault_path);
    let relative = file_path
        .strip_prefix(vault_root)
        .unwrap_or(&file_path)
        .to_string_lossy()
        .to_string();

    Ok(NoteEntry {
        path: file_path.to_string_lossy().to_string(),
        relative_path: relative,
        meta,
        preview: String::new(),
    })
}

pub fn duplicate_note(path: &str, vault_path: &str) -> Result<NoteEntry, String> {
    let validated = ensure_note_path(vault_path, Path::new(path))?;
    let src = validated.as_path();

    let parent = src
        .parent()
        .ok_or_else(|| "Note has no parent directory".to_string())?;
    let raw = fs::read_to_string(src).map_err(|e| e.to_string())?;
    let filename = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let (mut meta, body) = frontmatter::parse_note(&raw, &filename);
    let source_title = meta.title.clone();
    let base_title = format!("{} copy", source_title.trim_end());
    let mut copy_title = base_title.clone();
    let mut copy_number = 2;
    let mut copy_path = parent.join(format!("{}.md", sanitize_filename(&copy_title)));

    while copy_path.exists() {
        copy_title = format!("{} {}", base_title, copy_number);
        copy_number += 1;
        copy_path = parent.join(format!("{}.md", sanitize_filename(&copy_title)));
    }

    let now = Utc::now();
    meta.id = Uuid::new_v4().to_string();
    meta.title = copy_title;
    meta.pinned = false;
    meta.created = now;
    meta.modified = now;

    let body = retitle_leading_heading(&body, &source_title, &meta.title);
    let copy_raw = frontmatter::merge_frontmatter(&raw, &meta, &body);
    fs::write(&copy_path, copy_raw).map_err(|e| e.to_string())?;

    read_note_entry(&copy_path, Path::new(vault_path))
}

fn retitle_leading_heading(body: &str, old_title: &str, new_title: &str) -> String {
    let trimmed = body.trim_start();
    let leading_len = body.len() - trimmed.len();
    let first_line_end = trimmed.find('\n').unwrap_or(trimmed.len());
    let first_line = &trimmed[..first_line_end];
    let heading_len = first_line.chars().take_while(|&c| c == '#').count();

    if !(1..=6).contains(&heading_len) {
        return body.to_string();
    }

    let Some(heading_title) = first_line[heading_len..].strip_prefix(' ') else {
        return body.to_string();
    };
    if !heading_title.trim().eq_ignore_ascii_case(old_title.trim()) {
        return body.to_string();
    }

    let mut retitled = String::with_capacity(body.len() + new_title.len());
    retitled.push_str(&body[..leading_len]);
    retitled.push_str(&"#".repeat(heading_len));
    retitled.push(' ');
    retitled.push_str(new_title);
    retitled.push_str(&trimmed[first_line_end..]);
    retitled
}

pub fn create_notebook(
    vault_path: &str,
    parent_relative: Option<&str>,
    name: &str,
) -> Result<NotebookEntry, String> {
    let requested_parent = match parent_relative {
        Some(rel) => Path::new(vault_path).join(safe_relative_path(rel)?),
        None => PathBuf::from(vault_path),
    };
    let parent = ensure_vault_content_dir(vault_path, &requested_parent)?;
    let dir_path = parent.join(safe_child_name(name)?);
    if dir_path.exists() {
        return Err("Notebook already exists".to_string());
    }

    fs::create_dir_all(&dir_path).map_err(|e| e.to_string())?;

    let vault_root = Path::new(vault_path);
    let relative = dir_path
        .strip_prefix(vault_root)
        .unwrap_or(&dir_path)
        .to_string_lossy()
        .to_string();

    Ok(NotebookEntry {
        name: name.to_string(),
        path: dir_path.to_string_lossy().to_string(),
        relative_path: relative,
        children: Vec::new(),
        note_count: 0,
    })
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TrashedNoteManifest {
    note_id: String,
    original_relative_path: String,
    #[serde(default)]
    quick_access_index: Option<usize>,
}

fn trashed_note_manifest_path(trash_note: &Path) -> PathBuf {
    let filename = trash_note.file_name().unwrap_or_default().to_string_lossy();
    trash_note.with_file_name(format!("{filename}.restore.json"))
}

pub fn delete_note(vault_path: &str, note_path: &str) -> Result<(), String> {
    let validated = ensure_note_path(vault_path, Path::new(note_path))?;
    let src = validated.as_path();

    let raw = fs::read_to_string(src).map_err(|error| error.to_string())?;
    let filename = src.file_name().unwrap_or_default().to_string_lossy();
    let note_id = frontmatter::parse_note(&raw, &filename).0.id;
    let original_relative_path = src
        .strip_prefix(vault_path)
        .map_err(|_| "Note path must stay inside the active vault".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let mut quick_access = load_quick_access_entries(vault_path)?;
    let quick_access_index = quick_access.iter().position(|entry| {
        (!note_id.is_empty() && entry.note_id.as_deref() == Some(note_id.as_str()))
            || entry.relative_path == original_relative_path
    });
    let original_quick_access = quick_access.clone();
    if let Some(index) = quick_access_index {
        quick_access.remove(index);
        save_quick_access_entries(vault_path, &quick_access)?;
    }

    let trash_dir = helixnotes_dir(vault_path).join("trash");
    fs::create_dir_all(&trash_dir).map_err(|e| e.to_string())?;

    let filename = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let timestamp = Utc::now().format("%Y%m%d%H%M%S%3f");
    let trash_name = format!("{}_{}", timestamp, filename);
    let dest = match crate::vault::relocation::relocate_file(
        Path::new(vault_path),
        src,
        &trash_dir,
        std::ffi::OsStr::new(&trash_name),
        |bytes| Ok(bytes.to_vec()),
    ) {
        Ok(path) => path,
        Err(error) => {
            if quick_access_index.is_some() {
                save_quick_access_entries(vault_path, &original_quick_access).map_err(
                    |rollback_error| {
                        format!(
                            "Could not delete note ({error}); Quick Access rollback also failed: {rollback_error}"
                        )
                    },
                )?;
            }
            return Err(error);
        }
    };
    let manifest = TrashedNoteManifest {
        note_id,
        original_relative_path,
        quick_access_index,
    };
    let manifest_data = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    fs::write(trashed_note_manifest_path(&dest), manifest_data).map_err(|error| {
        format!("Note reached Trash, but restore metadata could not be saved: {error}")
    })?;
    Ok(())
}

pub fn delete_notebook(vault_path: &str, notebook_path: &str) -> Result<(), String> {
    let validated = ensure_notebook_path(vault_path, Path::new(notebook_path))?;
    reject_category_root(vault_path, validated.as_path(), "deleted")?;
    let src = validated.as_path();

    let trash_dir = helixnotes_dir(vault_path).join("trash");
    fs::create_dir_all(&trash_dir).map_err(|e| e.to_string())?;

    let dirname = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let timestamp = Utc::now().format("%Y%m%d%H%M%S%3f");
    let trash_name = format!("{}_{}", timestamp, dirname);
    let dest = trash_dir.join(&trash_name);

    // Save original relative path in a sidecar .meta file for restore
    let relative = src
        .strip_prefix(vault_path)
        .unwrap_or(src)
        .to_string_lossy()
        .to_string();
    let meta_path = trash_dir.join(format!("{}.meta", trash_name));
    let _ = fs::write(&meta_path, &relative);

    fs::rename(src, dest).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn rename_note(path: &str, new_title: &str, vault_path: &str) -> Result<String, String> {
    let validated = ensure_note_path(vault_path, Path::new(path))?;
    let src = validated.as_path();

    // Read old title before renaming
    let raw = fs::read_to_string(src).map_err(|e| e.to_string())?;
    let filename = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let (mut meta, content) = frontmatter::parse_note(&raw, &filename);
    let old_title = meta.title.clone();
    let old_path_str = src.to_string_lossy().to_string();

    // Update frontmatter
    meta.title = new_title.to_string();
    meta.modified = Utc::now();
    if meta.id.is_empty() {
        meta.id = Uuid::new_v4().to_string();
    }
    let updated = frontmatter::merge_frontmatter(&raw, &meta, &content);

    // Rename file
    let new_filename = sanitize_filename(new_title);
    let new_path = src.parent().unwrap().join(format!("{}.md", new_filename));
    fs::write(src, &updated).map_err(|e| e.to_string())?;

    if new_path != src {
        fs::rename(src, &new_path).map_err(|e| e.to_string())?;
    }

    let new_path_str = new_path.to_string_lossy().to_string();

    // Update wikilinks in other notes that reference this note
    let _ = update_wikilinks_after_rename(
        vault_path,
        &old_path_str,
        &new_path_str,
        &old_title,
        new_title,
    )?;

    Ok(new_path_str)
}

/// Walk all .md files in the vault and update wikilink references after a note rename.
/// Updates both HTML data-attributes (data-path, data-title) and raw [[old_title]] references.
struct WikilinkUpdate {
    rewritten_paths: Vec<String>,
    before_images: Vec<(PathBuf, String)>,
}

impl WikilinkUpdate {
    fn rollback(&self) -> Result<(), String> {
        let mut failures = Vec::new();
        for (path, content) in self.before_images.iter().rev() {
            if let Err(error) = fs::write(path, content) {
                failures.push(format!("{}: {error}", path.display()));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Could not restore rewritten backlinks: {}",
                failures.join("; ")
            ))
        }
    }
}

fn update_wikilinks_after_rename(
    vault_path: &str,
    old_path: &str,
    new_path: &str,
    old_title: &str,
    new_title: &str,
) -> Result<WikilinkUpdate, String> {
    update_wikilinks_after_rename_with_fault(
        vault_path, old_path, new_path, old_title, new_title, None,
    )
}

fn update_wikilinks_after_rename_with_fault(
    vault_path: &str,
    old_path: &str,
    new_path: &str,
    old_title: &str,
    new_title: &str,
    fail_on_write: Option<usize>,
) -> Result<WikilinkUpdate, String> {
    let vault = Path::new(vault_path);
    let hn_dir = helixnotes_dir(vault_path);

    // Vault-relative path ref (without .md), normalized to forward slashes so it
    // matches [[folder/note]] wikilinks on Windows (OS paths use backslashes there).
    let rel_ref = |p: &str| -> String {
        let rel = Path::new(p)
            .strip_prefix(vault)
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_string())
            .replace('\\', "/");
        rel.strip_suffix(".md").unwrap_or(&rel).to_string()
    };
    let old_rel_ref = rel_ref(old_path);
    let new_rel_ref = rel_ref(new_path);

    let mut note_paths = Vec::new();
    for entry in WalkDir::new(vault)
        .into_iter()
        .filter_entry(|entry| !is_hidden(entry.path()) && !entry.path().starts_with(&hn_dir))
    {
        let entry = entry.map_err(|error| format!("Could not scan note links: {error}"))?;
        let path = entry.path();
        if path.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("md")
            && path.to_string_lossy().as_ref() != new_path
        {
            note_paths.push(path.to_path_buf());
        }
    }

    // Check if another note with the same old_title exists in the vault.
    // If so, title-based replacements (rules 1-3) are ambiguous and must be skipped,
    // because we can't tell which [[Old Title]] ref points to the renamed note
    // vs. the other note with the same title.
    let title_is_unique = !note_paths.iter().any(|path| {
        // Check if this note's filename (without .md) matches the old title
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case(old_title))
            .unwrap_or(false)
    });

    let mut rewritten_paths = Vec::new();
    let mut before_images = Vec::new();
    for path in note_paths {
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read links in {}: {error}", path.display()))?;

        let mut result = content.clone();

        // Notes are saved as markdown with [[ref]] or [[ref|display]] syntax.
        // Update all wikilink forms that reference the renamed note:

        // Title-based rules only apply when the old title is unique in the vault.
        // If another note shares the same title, these would be ambiguous.
        if old_title != new_title && title_is_unique {
            // 1. Short title ref: [[Old Title]] → [[New Title]]
            result = result.replace(&format!("[[{}]]", old_title), &format!("[[{}]]", new_title));

            // 2. Short title with alias: [[Old Title|display]] → [[New Title|display]]
            result = result.replace(&format!("[[{}|", old_title), &format!("[[{}|", new_title));

            // 3. Short title as alias display: [[ref|Old Title]] → [[ref|New Title]]
            result = result.replace(&format!("|{}]]", old_title), &format!("|{}]]", new_title));
        }

        // Path-based rules are always safe (paths are unique).
        if old_rel_ref != new_rel_ref {
            // 4. Path-based ref: [[folder/Old Name]] → [[folder/New Name]]
            result = result.replace(
                &format!("[[{}]]", old_rel_ref),
                &format!("[[{}]]", new_rel_ref),
            );

            // 5. Path-based ref with alias: [[folder/Old Name|display]] → [[folder/New Name|display]]
            //    Also update the alias if it matches the old title.
            //    e.g. [[folder/Old Name|Old Title]] → [[folder/New Name|New Title]]
            if old_title != new_title {
                result = result.replace(
                    &format!("[[{}|{}]]", old_rel_ref, old_title),
                    &format!("[[{}|{}]]", new_rel_ref, new_title),
                );
            }
            // For aliases that DON'T match the old title, just update the ref part
            result = result.replace(
                &format!("[[{}|", old_rel_ref),
                &format!("[[{}|", new_rel_ref),
            );
        }

        if result != content {
            before_images.push((path.clone(), content));
            let write_result = if fail_on_write == Some(rewritten_paths.len()) {
                Err(std::io::Error::other("injected backlink write failure"))
            } else {
                fs::write(&path, &result)
            };
            if let Err(error) = write_result {
                let rollback = WikilinkUpdate {
                    rewritten_paths,
                    before_images,
                }
                .rollback();
                return match rollback {
                    Ok(()) => Err(format!(
                        "Could not update links in {}: {error}",
                        path.display()
                    )),
                    Err(rollback_error) => Err(format!(
                        "Could not update links in {} ({error}); {rollback_error}",
                        path.display()
                    )),
                };
            }
            rewritten_paths.push(path.to_string_lossy().to_string());
        }
    }
    Ok(WikilinkUpdate {
        rewritten_paths,
        before_images,
    })
}

fn preflight_wikilink_reads(vault_path: &str, source_path: &Path) -> Result<(), String> {
    let vault = Path::new(vault_path);
    let helixnotes = helixnotes_dir(vault_path);
    for entry in WalkDir::new(vault)
        .into_iter()
        .filter_entry(|entry| !is_hidden(entry.path()) && !entry.path().starts_with(&helixnotes))
    {
        let entry = entry.map_err(|error| format!("Could not scan note links: {error}"))?;
        let path = entry.path();
        if path == source_path
            || !path.is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("md")
        {
            continue;
        }
        fs::read_to_string(path)
            .map_err(|error| format!("Could not read links in {}: {error}", path.display()))?;
    }
    Ok(())
}

fn rollback_note_relocation(
    vault_path: &str,
    moved_path: &Path,
    original_path: &Path,
    original_bytes: &[u8],
) -> Result<(), String> {
    let original_parent = original_path
        .parent()
        .ok_or_else(|| "Original note has no parent directory".to_string())?;
    let original_name = original_path
        .file_name()
        .ok_or_else(|| "Original note has no filename".to_string())?;
    let restored = crate::vault::relocation::relocate_file(
        Path::new(vault_path),
        moved_path,
        original_parent,
        original_name,
        |_| Ok(original_bytes.to_vec()),
    )?;
    if restored != original_path {
        return Err(format!(
            "The original path was occupied during rollback; the note was recovered at {}",
            restored.display()
        ));
    }
    Ok(())
}

pub fn rename_notebook(vault_path: &str, path: &str, new_name: &str) -> Result<String, String> {
    let validated = ensure_notebook_path(vault_path, Path::new(path))?;
    reject_category_root(vault_path, validated.as_path(), "renamed")?;
    let src = validated.as_path();

    let new_path = src.parent().unwrap().join(safe_child_name(new_name)?);
    if new_path.exists() {
        return Err("A notebook with that name already exists".to_string());
    }

    fs::rename(src, &new_path).map_err(|e| e.to_string())?;
    Ok(new_path.to_string_lossy().to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteMoveOutcome {
    pub path: String,
    pub rewritten_paths: Vec<String>,
}

#[cfg(test)]
pub fn move_note(vault_path: &str, note_path: &str, dest_notebook: &str) -> Result<String, String> {
    move_note_with_outcome(vault_path, note_path, dest_notebook).map(|outcome| outcome.path)
}

pub fn move_note_with_outcome(
    vault_path: &str,
    note_path: &str,
    dest_notebook: &str,
) -> Result<NoteMoveOutcome, String> {
    move_note_with_outcome_inner(vault_path, note_path, dest_notebook, None)
}

fn move_note_with_outcome_inner(
    vault_path: &str,
    note_path: &str,
    dest_notebook: &str,
    fail_link_write: Option<usize>,
) -> Result<NoteMoveOutcome, String> {
    let validated = ensure_note_path(vault_path, Path::new(note_path))?;
    let src = validated.as_path();
    let (dest_dir, category) = ensure_para_destination_dir(vault_path, Path::new(dest_notebook))?;
    if src.parent() == Some(dest_dir.as_path()) {
        return Err("Note is already in that location".to_string());
    }

    // Quick Access is authoritative user state, so malformed data must stop the move
    // before the source is changed. The exact replacement path is filled in after the
    // relocation has chosen its collision-safe filename.
    let mut quick_access = load_quick_access_entries(vault_path)?;
    let original_quick_access = quick_access.clone();
    let old_relative = src
        .strip_prefix(vault_path)
        .map_err(|_| "Note path must stay inside the active vault".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let source_raw = fs::read_to_string(src).map_err(|error| error.to_string())?;
    let source_filename = src.file_name().unwrap_or_default().to_string_lossy();
    let source_id = frontmatter::parse_note(&source_raw, &source_filename).0.id;
    let quick_access_position = quick_access.iter().position(|entry| {
        (!source_id.is_empty() && entry.note_id.as_deref() == Some(source_id.as_str()))
            || entry.relative_path == old_relative
    });
    preflight_wikilink_reads(vault_path, src)?;

    let filename = src.file_name().unwrap_or_default();
    let title = std::cell::RefCell::new(None);
    let original_bytes = std::cell::RefCell::new(None);
    let old_path = src.to_string_lossy().to_string();
    let dest = crate::vault::relocation::relocate_file(
        Path::new(vault_path),
        src,
        &dest_dir,
        filename,
        |raw| {
            original_bytes.replace(Some(raw.to_vec()));
            let raw = std::str::from_utf8(raw)
                .map_err(|error| format!("Note is not valid UTF-8: {error}"))?;
            let filename = filename.to_string_lossy();
            title.replace(Some(
                frontmatter::extract_title(raw)
                    .unwrap_or_else(|| frontmatter::filename_to_title(&filename)),
            ));
            frontmatter::set_category(raw, category).map(String::into_bytes)
        },
    )?;
    let title = title
        .into_inner()
        .ok_or_else(|| "Could not determine moved note title".to_string())?;
    let original_bytes = original_bytes
        .into_inner()
        .ok_or_else(|| "Could not retain the original note for rollback".to_string())?;
    let new_path = dest.to_string_lossy().to_string();
    let link_update = match update_wikilinks_after_rename_with_fault(
        vault_path,
        &old_path,
        &new_path,
        &title,
        &title,
        fail_link_write,
    ) {
        Ok(update) => update,
        Err(error) => {
            return match rollback_note_relocation(
                vault_path,
                &dest,
                Path::new(&old_path),
                &original_bytes,
            ) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "{error}; the note relocation could not be fully rolled back: {rollback_error}"
                )),
            };
        }
    };

    if let Some(position) = quick_access_position {
        quick_access[position].relative_path = dest
            .strip_prefix(vault_path)
            .map_err(|_| "Moved note escaped the active vault".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if let Err(error) = save_quick_access_entries(vault_path, &quick_access) {
            let link_rollback = link_update.rollback();
            let note_rollback =
                rollback_note_relocation(vault_path, &dest, Path::new(&old_path), &original_bytes);
            let quick_access_rollback =
                save_quick_access_entries(vault_path, &original_quick_access);
            return Err(format!(
                "Could not update Quick Access after moving the note: {error}. Backlink rollback: {}. Note rollback: {}. Quick Access rollback: {}",
                rollback_result(&link_rollback),
                rollback_result(&note_rollback),
                rollback_result(&quick_access_rollback),
            ));
        }
    }
    Ok(NoteMoveOutcome {
        path: dest.to_string_lossy().to_string(),
        rewritten_paths: link_update.rewritten_paths,
    })
}

fn rollback_result(result: &Result<(), String>) -> &str {
    match result {
        Ok(()) => "completed",
        Err(error) => error,
    }
}

fn ensure_para_destination_dir(
    vault_path: &str,
    requested_path: &Path,
) -> Result<(PathBuf, ParaCategory), String> {
    let metadata = fs::symlink_metadata(requested_path)
        .map_err(|error| format!("Invalid PARA destination: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("PARA destination must be a real directory".to_string());
    }

    let vault = canonicalize_path(Path::new(vault_path), "vault path")?;
    let destination = canonicalize_path(requested_path, "PARA destination")?;
    let relative = destination
        .strip_prefix(&vault)
        .map_err(|_| "PARA destination must stay inside the active vault".to_string())?;
    let relative = relative.to_string_lossy();
    let category = para::category_for_relative_path(&relative)
        .ok_or_else(|| "Notes must stay inside one of the four PARA categories".to_string())?;

    Ok((destination, category))
}

/// Where notes with no category wait for the user to file them.
pub fn unfiled_dir(vault_path: &str) -> PathBuf {
    helixnotes_dir(vault_path).join(para::UNFILED_DIR)
}

fn ensure_unfiled_directory(vault_path: &str, create: bool) -> Result<PathBuf, String> {
    let vault = canonicalize_path(Path::new(vault_path), "vault path")?;
    let app_data_path = helixnotes_dir(vault_path);
    let holding_path = unfiled_dir(vault_path);

    let app_data_metadata = fs::symlink_metadata(&app_data_path)
        .map_err(|error| format!("Invalid vault app-data directory: {error}"))?;
    if app_data_metadata.file_type().is_symlink() || !app_data_metadata.is_dir() {
        return Err("vault app-data directory must be a real directory".to_string());
    }
    if create {
        match fs::create_dir(&holding_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("Could not create Holding Area: {error}")),
        }
    }
    let holding_metadata = fs::symlink_metadata(&holding_path)
        .map_err(|error| format!("Invalid Holding Area directory: {error}"))?;
    if holding_metadata.file_type().is_symlink() || !holding_metadata.is_dir() {
        return Err("Holding Area directory must be a real directory".to_string());
    }

    let app_data = canonicalize_path(&app_data_path, "vault app-data directory")?;
    let holding = canonicalize_path(&holding_path, "Holding Area directory")?;
    if app_data.parent() != Some(vault.as_path())
        || app_data.file_name().and_then(|name| name.to_str()) != Some(".helixnotes")
        || holding.parent() != Some(app_data.as_path())
    {
        return Err(
            "Holding Area must stay inside the active vault app-data directory".to_string(),
        );
    }

    Ok(holding)
}

fn ensure_unfiled_note_path(vault_path: &str, requested_path: &Path) -> Result<PathBuf, String> {
    let holding = ensure_unfiled_directory(vault_path, false)?;

    let metadata = fs::symlink_metadata(requested_path)
        .map_err(|error| format!("Invalid Holding Area note path: {error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || requested_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("md")
    {
        return Err("Holding Area note must be a regular Markdown file".to_string());
    }

    let requested = canonicalize_path(requested_path, "Holding Area note path")?;
    if requested.parent() != Some(holding.as_path()) {
        return Err("That note is not waiting in the active vault Holding Area".to_string());
    }

    Ok(requested)
}

fn ensure_category_destination(
    vault_path: &str,
    category: ParaCategory,
) -> Result<PathBuf, String> {
    let vault = canonicalize_path(Path::new(vault_path), "vault path")?;
    let destination_path = Path::new(vault_path).join(category.folder_name());
    let metadata = fs::symlink_metadata(&destination_path)
        .map_err(|error| format!("Invalid PARA category destination: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("PARA category destination must be a real directory".to_string());
    }

    let destination = canonicalize_path(&destination_path, "PARA category destination")?;
    if destination.parent() != Some(vault.as_path())
        || destination.file_name().and_then(|name| name.to_str()) != Some(category.folder_name())
    {
        return Err("PARA category destination must stay inside the active vault".to_string());
    }

    Ok(destination)
}

/// Put every note in the folder its own category calls for, and collect the ones that
/// have no category and so cannot be filed.
pub fn reconcile_categories(vault_path: &str) -> Result<para::ReconcileReport, String> {
    para::reconcile_vault(vault_path)
}

/// Notes waiting in the holding area for the user to give them a category.
pub fn list_unfiled_notes(vault_path: &str) -> Result<Vec<NoteEntry>, String> {
    let dir = unfiled_dir(vault_path);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let root = Path::new(vault_path);
    let mut notes: Vec<NoteEntry> = fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("md"))
        .filter_map(|p| read_note_entry(&p, root).ok())
        .collect();

    notes.sort_by(|a, b| compare_natural_names(&a.meta.title, &b.meta.title));
    Ok(notes)
}

/// Give an unfiled note a category and move it into that category's folder.
///
/// Writes the category onto the note first, because the note is the source of truth; the
/// move only puts the file where the note now says it belongs.
pub fn file_unfiled_note(
    vault_path: &str,
    note_path: &str,
    category: &str,
) -> Result<String, String> {
    let category = ParaCategory::from_name(category)
        .ok_or_else(|| format!("Not a PARA category: {}", category))?;

    let src = ensure_unfiled_note_path(vault_path, Path::new(note_path))?;
    let dest_dir = ensure_category_destination(vault_path, category)?;

    let filename = src.file_name().unwrap_or_default();
    crate::vault::relocation::relocate_file(
        Path::new(vault_path),
        &src,
        &dest_dir,
        filename,
        |raw| {
            let raw = std::str::from_utf8(raw)
                .map_err(|error| format!("Note is not valid UTF-8: {error}"))?;
            frontmatter::set_category(raw, category).map(String::into_bytes)
        },
    )
    .map(|p| p.to_string_lossy().to_string())
}

/// Refuse an operation that would rename, move, or delete one of the four category
/// folders. They are fixed by the method, so protecting them here covers every caller
/// rather than relying on the UI to hide the option.
fn reject_category_root(vault_path: &str, target: &Path, verb: &str) -> Result<(), String> {
    let relative = match target.strip_prefix(Path::new(vault_path)) {
        Ok(rel) => rel.to_string_lossy().to_string(),
        Err(_) => return Ok(()),
    };
    if para::is_category_root(&relative) {
        return Err(format!(
            "PARA categories are fixed and cannot be {}: {}",
            verb, relative
        ));
    }
    Ok(())
}

pub fn move_notebook(
    vault_path: &str,
    notebook_path: &str,
    dest_parent: &str,
) -> Result<String, String> {
    let validated = ensure_notebook_path(vault_path, Path::new(notebook_path))?;
    reject_category_root(vault_path, validated.as_path(), "moved")?;
    let src = validated.as_path().to_path_buf();

    let (dest_parent_path, destination_category) =
        ensure_para_destination_dir(vault_path, Path::new(dest_parent))?;

    // No-op if already in that parent
    if src.parent() == Some(dest_parent_path.as_path()) {
        return Err("Notebook is already in that location".to_string());
    }

    let dir_name = src.file_name().unwrap_or_default();
    let dest = dest_parent_path.join(dir_name);

    // Prevent moving into itself or a descendant
    if dest.starts_with(&src) {
        return Err("Cannot move a notebook into itself or its descendants".to_string());
    }

    if dest.exists() {
        return Err("A notebook with that name already exists in the destination".to_string());
    }

    let mut notes = Vec::new();
    for entry in WalkDir::new(&src) {
        let entry = entry.map_err(|error| format!("Could not scan notebook: {error}"))?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let before = fs::read(path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        let raw = std::str::from_utf8(&before)
            .map_err(|error| format!("{} is not valid UTF-8: {error}", path.display()))?;
        let after = frontmatter::set_category(raw, destination_category)?.into_bytes();
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let title = frontmatter::extract_title(raw)
            .unwrap_or_else(|| frontmatter::filename_to_title(&filename));
        let relative = path
            .strip_prefix(&src)
            .map_err(|error| error.to_string())?
            .to_path_buf();
        notes.push((path.to_path_buf(), relative, before, after, title));
    }
    preflight_wikilink_reads(vault_path, &src)?;

    let mut quick_access = load_quick_access_entries(vault_path)?;
    let original_quick_access = quick_access.clone();
    let old_relative = src
        .strip_prefix(vault_path)
        .map_err(|_| "Notebook must stay inside the active vault".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let new_relative = dest
        .strip_prefix(vault_path)
        .map_err(|_| "Notebook destination escaped the active vault".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let old_prefix = format!("{old_relative}/");
    let mut quick_access_changed = false;
    for entry in &mut quick_access {
        if entry.relative_path.starts_with(&old_prefix) {
            entry.relative_path = format!(
                "{new_relative}/{}",
                &entry.relative_path[old_prefix.len()..]
            );
            quick_access_changed = true;
        }
    }

    let original_icons = load_notebook_icons(vault_path)?;
    let updated_icons = remap_notebook_icons(&original_icons, &old_relative, &new_relative);
    let icons_changed = updated_icons != original_icons;

    for (metadata_written, (path, _, _, after, _)) in notes.iter().enumerate() {
        if let Err(error) = fs::write(path, after) {
            for (rollback_path, _, before, _, _) in notes[..metadata_written].iter().rev() {
                let _ = fs::write(rollback_path, before);
            }
            return Err(format!(
                "Could not update category in {}: {error}",
                path.display()
            ));
        }
    }

    if let Err(error) = fs::rename(&src, &dest) {
        for (path, _, before, _, _) in notes.iter().rev() {
            let _ = fs::write(path, before);
        }
        return Err(error.to_string());
    }

    let mut link_updates = Vec::new();
    for (old_path, relative, _, _, title) in &notes {
        let new_path = dest.join(relative);
        match update_wikilinks_after_rename(
            vault_path,
            &old_path.to_string_lossy(),
            &new_path.to_string_lossy(),
            title,
            title,
        ) {
            Ok(update) => link_updates.push(update),
            Err(error) => {
                let rollback = rollback_notebook_move(
                    vault_path,
                    &src,
                    &dest,
                    &notes,
                    &link_updates,
                    &original_quick_access,
                    &original_icons,
                );
                return Err(format!("{error}. Notebook rollback: {rollback}"));
            }
        }
    }

    if quick_access_changed {
        if let Err(error) = save_quick_access_entries(vault_path, &quick_access) {
            let rollback = rollback_notebook_move(
                vault_path,
                &src,
                &dest,
                &notes,
                &link_updates,
                &original_quick_access,
                &original_icons,
            );
            return Err(format!(
                "Could not update Quick Access after moving the notebook: {error}. Notebook rollback: {rollback}"
            ));
        }
    }
    if icons_changed {
        if let Err(error) = save_notebook_icons(vault_path, &updated_icons) {
            let rollback = rollback_notebook_move(
                vault_path,
                &src,
                &dest,
                &notes,
                &link_updates,
                &original_quick_access,
                &original_icons,
            );
            return Err(format!(
                "Could not update notebook icons after the move: {error}. Notebook rollback: {rollback}"
            ));
        }
    }

    Ok(dest.to_string_lossy().to_string())
}

type NotebookMoveNote = (PathBuf, PathBuf, Vec<u8>, Vec<u8>, String);

fn remap_notebook_icons(
    icons: &std::collections::HashMap<String, String>,
    old_relative: &str,
    new_relative: &str,
) -> std::collections::HashMap<String, String> {
    let old_key = normalize_notebook_icon_key(old_relative);
    let new_key = normalize_notebook_icon_key(new_relative);
    let old_prefix = format!("{old_key}/");
    icons
        .iter()
        .map(|(key, value)| {
            let key = if key == &old_key {
                new_key.clone()
            } else if key.starts_with(&old_prefix) {
                format!("{new_key}/{}", &key[old_prefix.len()..])
            } else {
                key.clone()
            };
            (key, value.clone())
        })
        .collect()
}

fn save_notebook_icons(
    vault_path: &str,
    icons: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let data = serde_json::to_string_pretty(icons).map_err(|error| error.to_string())?;
    fs::write(helixnotes_dir(vault_path).join("notebook_icons.json"), data)
        .map_err(|error| error.to_string())
}

fn rollback_notebook_move(
    vault_path: &str,
    source: &Path,
    destination: &Path,
    notes: &[NotebookMoveNote],
    link_updates: &[WikilinkUpdate],
    quick_access: &[QuickAccessEntry],
    icons: &std::collections::HashMap<String, String>,
) -> String {
    let mut failures = Vec::new();
    for update in link_updates.iter().rev() {
        if let Err(error) = update.rollback() {
            failures.push(error);
        }
    }
    if let Err(error) = fs::rename(destination, source) {
        failures.push(format!("directory: {error}"));
    } else {
        for (path, _, before, _, _) in notes.iter().rev() {
            if let Err(error) = fs::write(path, before) {
                failures.push(format!("{}: {error}", path.display()));
            }
        }
    }
    if let Err(error) = save_quick_access_entries(vault_path, quick_access) {
        failures.push(format!("Quick Access: {error}"));
    }
    if let Err(error) = save_notebook_icons(vault_path, icons) {
        failures.push(format!("notebook icons: {error}"));
    }
    if failures.is_empty() {
        "completed".to_string()
    } else {
        failures.join("; ")
    }
}

/// Remove a directory inside trash if it's empty (after restoring/deleting its last note).
fn cleanup_empty_trash_dir(vault_path: &str, dir: Option<&Path>) {
    let trash_dir = helixnotes_dir(vault_path).join("trash");
    if let Some(d) = dir {
        if d != trash_dir && d.starts_with(&trash_dir) {
            if let Ok(mut entries) = fs::read_dir(d) {
                if entries.next().is_none() {
                    let _ = fs::remove_dir(d);
                }
            }
        }
    }
}

pub fn get_trash_contents(vault_path: &str) -> Result<TrashContents, String> {
    let trash_dir = helixnotes_dir(vault_path).join("trash");
    if !trash_dir.exists() {
        return Ok(TrashContents {
            notes: Vec::new(),
            notebooks: Vec::new(),
        });
    }

    let vault_root = Path::new(vault_path);
    let mut notes = Vec::new();
    let mut notebooks = Vec::new();

    for entry in fs::read_dir(&trash_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|x| x.to_str()) == Some("md") {
            if let Ok(note) = read_note_entry(&path, vault_root) {
                notes.push(note);
            }
        } else if path.is_dir() {
            let note_count = WalkDir::new(&path)
                .min_depth(1)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().is_file()
                        && e.path().extension().and_then(|x| x.to_str()) == Some("md")
                })
                .count();
            let dirname = path.file_name().unwrap_or_default().to_string_lossy();
            // Strip timestamp prefix to get original notebook name
            let name = if dirname.len() > 18 && dirname.chars().nth(17) == Some('_') {
                dirname[18..].to_string()
            } else if dirname.len() > 15 && dirname.chars().nth(14) == Some('_') {
                dirname[15..].to_string()
            } else {
                dirname.to_string()
            };
            let modified = fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            notebooks.push(TrashNotebookEntry {
                name,
                path: path.to_string_lossy().to_string(),
                note_count,
                modified,
            });
        }
    }

    notes.sort_by_key(|note| std::cmp::Reverse(note.meta.modified));
    notebooks.sort_by_key(|notebook| std::cmp::Reverse(notebook.modified));
    Ok(TrashContents { notes, notebooks })
}

pub fn restore_note(vault_path: &str, trash_path: &str) -> Result<String, String> {
    let validated = ensure_trash_entry(vault_path, Path::new(trash_path))?;
    let src = validated.as_path();
    if !src.is_file() {
        return Err("Trashed note does not exist".to_string());
    }

    let raw = fs::read_to_string(src).map_err(|error| error.to_string())?;
    let source_filename = src.file_name().unwrap_or_default().to_string_lossy();
    let source_meta = frontmatter::parse_note(&raw, &source_filename).0;
    let manifest_path = trashed_note_manifest_path(src);
    let manifest = if manifest_path.exists() {
        let data = fs::read(&manifest_path).map_err(|error| error.to_string())?;
        let manifest: TrashedNoteManifest =
            serde_json::from_slice(&data).map_err(|error| error.to_string())?;
        if manifest.note_id != source_meta.id {
            return Err("Trash restore metadata does not match the note identity".to_string());
        }
        Some(manifest)
    } else {
        None
    };
    let mut quick_access = if manifest
        .as_ref()
        .and_then(|value| value.quick_access_index)
        .is_some()
    {
        Some(load_quick_access_entries(vault_path)?)
    } else {
        None
    };
    let category = source_meta.category;
    let dest_dir = match category {
        Some(category) => ensure_category_destination(vault_path, category)?,
        None => ensure_unfiled_directory(vault_path, true)?,
    };

    // Strip timestamp prefix from trash filename (17-char with millis or 14-char legacy)
    let filename = src.file_name().unwrap_or_default().to_string_lossy();
    let original_name = if filename.len() > 18 && filename.chars().nth(17) == Some('_') {
        &filename[18..]
    } else if filename.len() > 15 && filename.chars().nth(14) == Some('_') {
        &filename[15..]
    } else {
        &filename
    };

    let parent = src.parent().map(|p| p.to_path_buf());
    let dest = crate::vault::relocation::relocate_file(
        Path::new(vault_path),
        src,
        &dest_dir,
        std::ffi::OsStr::new(original_name),
        |raw| Ok(raw.to_vec()),
    )?;

    if let (Some(manifest), Some(entries)) = (&manifest, quick_access.as_mut()) {
        entries.retain(|entry| entry.note_id.as_deref() != Some(manifest.note_id.as_str()));
        let insert_at = manifest
            .quick_access_index
            .unwrap_or(entries.len())
            .min(entries.len());
        let relative_path = dest
            .strip_prefix(vault_path)
            .map_err(|_| "Restored note escaped the active vault".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        entries.insert(
            insert_at,
            QuickAccessEntry {
                note_id: (!manifest.note_id.is_empty()).then(|| manifest.note_id.clone()),
                relative_path,
            },
        );
        save_quick_access_entries(vault_path, entries)?;
    }
    if manifest.is_some() {
        fs::remove_file(&manifest_path)
            .map_err(|error| format!("Note was restored, but Trash metadata remains: {error}"))?;
    }

    // Clean up empty parent directory (deleted notebook folder) in trash
    cleanup_empty_trash_dir(vault_path, parent.as_deref());

    Ok(dest.to_string_lossy().to_string())
}

pub fn restore_notebook(vault_path: &str, trash_path: &str) -> Result<String, String> {
    let validated = ensure_trash_entry(vault_path, Path::new(trash_path))?;
    let src = validated.as_path();
    if !src.is_dir() {
        return Err("Trashed notebook does not exist".to_string());
    }

    let dirname = src.file_name().unwrap_or_default().to_string_lossy();

    // Try to read original path from sidecar .meta file
    let meta_path = src
        .with_extension("")
        .with_file_name(format!("{}.meta", dirname));
    let relative = if let Ok(original) = fs::read_to_string(&meta_path) {
        original
    } else {
        // Fallback for notebooks trashed before .meta files existed:
        // strip timestamp prefix to get notebook name, restore to root
        let name = if dirname.len() > 18 && dirname.chars().nth(17) == Some('_') {
            &dirname[18..]
        } else if dirname.len() > 15 && dirname.chars().nth(14) == Some('_') {
            &dirname[15..]
        } else {
            &dirname
        };
        name.to_string()
    };

    let dest = Path::new(vault_path).join(safe_relative_path(&relative)?);

    // Recreate parent directories if needed
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    fs::rename(src, &dest).map_err(|e| e.to_string())?;
    // Clean up .meta file
    let _ = fs::remove_file(&meta_path);
    Ok(dest.to_string_lossy().to_string())
}

pub fn permanent_delete(vault_path: &str, path: &str) -> Result<(), String> {
    let validated = ensure_trash_entry(vault_path, Path::new(path))?;
    let p = validated.as_path();
    let parent = p.parent().map(|pp| pp.to_path_buf());
    let sidecar = if p.is_dir() {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        Some(p.with_file_name(format!("{name}.meta")))
    } else if p.extension().and_then(|extension| extension.to_str()) == Some("md") {
        Some(trashed_note_manifest_path(p))
    } else {
        None
    };
    if p.is_dir() {
        fs::remove_dir_all(p).map_err(|e| e.to_string())?;
    } else {
        fs::remove_file(p).map_err(|e| e.to_string())?;
    }
    if let Some(sidecar) = sidecar {
        match fs::remove_file(sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Trash item was deleted, but its metadata remains: {error}"
                ))
            }
        }
    }

    // Clean up empty parent directory (deleted notebook folder) in trash
    cleanup_empty_trash_dir(vault_path, parent.as_deref());

    Ok(())
}

pub fn empty_trash(vault_path: &str) -> Result<(), String> {
    let trash_dir = helixnotes_dir(vault_path).join("trash");
    if trash_dir.exists() {
        fs::remove_dir_all(&trash_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&trash_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn load_vault_state(vault_path: &str) -> Result<VaultState, String> {
    let state_path = helixnotes_dir(vault_path).join("state.json");
    if state_path.exists() {
        let data = fs::read_to_string(&state_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&data).map_err(|e| e.to_string())
    } else {
        Ok(VaultState::default())
    }
}

pub fn save_vault_state(vault_path: &str, state: &VaultState) -> Result<(), String> {
    let state_path = helixnotes_dir(vault_path).join("state.json");
    let data = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::write(&state_path, data).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn get_all_tags(vault_path: &str) -> Result<Vec<(String, usize)>, String> {
    let hn_dir = helixnotes_dir(vault_path);
    let md_files: Vec<PathBuf> = WalkDir::new(vault_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let p = e.path();
            p.is_file()
                && p.extension().and_then(|x| x.to_str()) == Some("md")
                && !is_hidden(p)
                && !p.starts_with(&hn_dir)
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    // Read frontmatter in parallel (only need tags, so partial read is fine)
    let all_tags: Vec<Vec<String>> = md_files
        .par_iter()
        .filter_map(|path| {
            let mut file = fs::File::open(path).ok()?;
            let mut buf = vec![0u8; 1024]; // Tags are in frontmatter, 1KB is plenty
            let n = file.read(&mut buf).ok()?;
            buf.truncate(n);
            let raw = String::from_utf8_lossy(&buf);
            let filename = path.file_name()?.to_string_lossy().to_string();
            let (meta, _) = frontmatter::parse_note(&raw, &filename);
            Some(meta.tags)
        })
        .collect();

    let mut tag_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for tags in all_tags {
        for tag in tags {
            *tag_counts.entry(tag).or_insert(0) += 1;
        }
    }

    let mut tags: Vec<(String, usize)> = tag_counts.into_iter().collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(tags)
}

pub fn save_image(vault_path: &str, name: &str, data: &[u8]) -> Result<String, String> {
    save_attachment(vault_path, name, data)
}

pub fn save_attachment(vault_path: &str, name: &str, data: &[u8]) -> Result<String, String> {
    let attachments_dir = helixnotes_dir(vault_path).join("attachments");
    fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;

    let timestamp = Utc::now().format("%Y%m%d%H%M%S");
    let safe_name = sanitize_filename(name);
    let filename = format!("{}_{}", timestamp, safe_name);
    let dest = attachments_dir.join(&filename);

    fs::write(&dest, data).map_err(|e| e.to_string())?;

    // Return relative path from vault root
    let relative = format!(".helixnotes/attachments/{}", filename);
    Ok(relative)
}

pub(crate) fn normalize_notebook_icon_key(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn load_notebook_icons(
    vault_path: &str,
) -> Result<std::collections::HashMap<String, String>, String> {
    let icons_path = helixnotes_dir(vault_path).join("notebook_icons.json");
    if !icons_path.exists() {
        return Ok(std::collections::HashMap::new());
    }

    let data = fs::read_to_string(&icons_path).map_err(|e| e.to_string())?;
    let icons: std::collections::HashMap<String, String> =
        serde_json::from_str(&data).map_err(|e| e.to_string())?;
    Ok(icons
        .into_iter()
        .map(|(path, icon)| (normalize_notebook_icon_key(&path), icon))
        .collect())
}

pub fn set_notebook_icon(
    vault_path: &str,
    notebook_relative: &str,
    icon_relative: Option<&str>,
) -> Result<(), String> {
    let mut icons = load_notebook_icons(vault_path)?;
    let notebook_key = normalize_notebook_icon_key(notebook_relative);
    match icon_relative {
        Some(icon) => {
            icons.insert(notebook_key, icon.to_string());
        }
        None => {
            icons.remove(&notebook_key);
        }
    }
    let icons_path = helixnotes_dir(vault_path).join("notebook_icons.json");
    let data = serde_json::to_string_pretty(&icons).map_err(|e| e.to_string())?;
    fs::write(&icons_path, data).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct QuickAccessEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note_id: Option<String>,
    relative_path: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct QuickAccessFile {
    version: u8,
    entries: Vec<QuickAccessEntry>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum StoredQuickAccess {
    Current(QuickAccessFile),
    Legacy(Vec<String>),
}

fn quick_access_note_id(vault_path: &str, relative_path: &str) -> Option<String> {
    let path = Path::new(vault_path).join(relative_path);
    let raw = fs::read_to_string(&path).ok()?;
    let filename = path.file_name()?.to_string_lossy();
    let id = frontmatter::parse_note(&raw, &filename).0.id;
    (!id.is_empty()).then_some(id)
}

fn save_quick_access_entries(vault_path: &str, entries: &[QuickAccessEntry]) -> Result<(), String> {
    let qa_path = helixnotes_dir(vault_path).join("quick_access.json");
    let data = serde_json::to_string_pretty(&QuickAccessFile {
        version: 1,
        entries: entries.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    fs::write(&qa_path, data).map_err(|error| error.to_string())
}

fn load_quick_access_entries(vault_path: &str) -> Result<Vec<QuickAccessEntry>, String> {
    let qa_path = helixnotes_dir(vault_path).join("quick_access.json");
    if !qa_path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(&qa_path).map_err(|error| error.to_string())?;
    match serde_json::from_str::<StoredQuickAccess>(&data).map_err(|error| error.to_string())? {
        StoredQuickAccess::Current(file) if file.version == 1 => Ok(file.entries),
        StoredQuickAccess::Current(file) => Err(format!(
            "Unsupported Quick Access version: {}",
            file.version
        )),
        StoredQuickAccess::Legacy(paths) => {
            let entries = paths
                .into_iter()
                .map(|relative_path| QuickAccessEntry {
                    note_id: quick_access_note_id(vault_path, &relative_path),
                    relative_path,
                })
                .collect::<Vec<_>>();
            save_quick_access_entries(vault_path, &entries)?;
            Ok(entries)
        }
    }
}

pub fn load_quick_access(vault_path: &str) -> Result<Vec<String>, String> {
    Ok(load_quick_access_entries(vault_path)?
        .into_iter()
        .map(|entry| entry.relative_path)
        .collect())
}

pub fn save_quick_access(vault_path: &str, paths: &[String]) -> Result<(), String> {
    let entries = paths
        .iter()
        .map(|relative_path| QuickAccessEntry {
            note_id: quick_access_note_id(vault_path, relative_path),
            relative_path: relative_path.clone(),
        })
        .collect::<Vec<_>>();
    save_quick_access_entries(vault_path, &entries)
}

pub fn add_quick_access(vault_path: &str, note_relative: &str) -> Result<(), String> {
    let mut list = load_quick_access(vault_path)?;
    if !list.contains(&note_relative.to_string()) {
        list.push(note_relative.to_string());
        save_quick_access(vault_path, &list)?;
    }
    Ok(())
}

pub fn remove_quick_access(vault_path: &str, note_relative: &str) -> Result<(), String> {
    let mut list = load_quick_access(vault_path)?;
    list.retain(|p| p != note_relative);
    save_quick_access(vault_path, &list)?;
    Ok(())
}

const NOTE_SWITCHER_RECENT_LIMIT: usize = 6;

pub fn get_note_switcher_titles(
    vault_path: &str,
    recent_paths: &[String],
) -> Result<Vec<NoteTitleEntry>, String> {
    let vault_root = Path::new(vault_path);
    if !vault_root.is_dir() {
        return Err("Vault path does not exist".to_string());
    }

    let mut seen = HashSet::new();
    let mut titles = Vec::with_capacity(NOTE_SWITCHER_RECENT_LIMIT);

    for requested_path in recent_paths {
        if titles.len() >= NOTE_SWITCHER_RECENT_LIMIT {
            break;
        }

        let path = Path::new(requested_path);
        let Ok(relative) = path.strip_prefix(vault_root) else {
            continue;
        };
        let safe_relative = !relative.as_os_str().is_empty()
            && relative.components().all(|component| match component {
                Component::Normal(name) => !is_hidden(Path::new(name)),
                Component::CurDir => true,
                _ => false,
            });
        if !safe_relative
            || path.extension().and_then(|extension| extension.to_str()) != Some("md")
            || !path.is_file()
        {
            continue;
        }

        let relative_path = relative.to_path_buf();
        if !seen.insert(relative_path.clone()) {
            continue;
        }
        let Ok(entry) = read_note_entry_fast(path, vault_root) else {
            continue;
        };

        titles.push(NoteTitleEntry {
            title: entry.meta.title,
            path: relative_path.to_string_lossy().replace('\\', "/"),
        });
    }

    Ok(titles)
}

pub fn get_quick_access_notes(vault_path: &str) -> Result<Vec<NoteEntry>, String> {
    let list = load_quick_access(vault_path)?;
    let vault_root = Path::new(vault_path);
    let mut notes = Vec::new();

    for relative in &list {
        let full_path = vault_root.join(relative);
        if full_path.exists() {
            if let Ok(entry) = read_note_entry_fast(&full_path, vault_root) {
                notes.push(entry);
            }
        }
    }

    Ok(notes)
}

pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        compare_natural_names, create_note, create_notebook, duplicate_note,
        ensure_vault_structure, get_note_switcher_titles, helixnotes_dir, load_notebook_icons,
        load_quick_access, move_note, move_note_with_outcome, permanent_delete, read_note,
        restore_notebook, save_quick_access, scan_notebooks, set_notebook_icon, ParaCategory,
    };
    use crate::search::SearchIndex;
    use crate::vault::frontmatter;
    use std::fs;
    use uuid::Uuid;

    /// A vault with the PARA scaffold already in place.
    fn scaffolded_vault(label: &str) -> std::path::PathBuf {
        let vault = std::env::temp_dir().join(format!("para-ops-{}-{}", label, Uuid::new_v4()));
        fs::create_dir_all(&vault).unwrap();
        ensure_vault_structure(&vault.to_string_lossy()).unwrap();
        vault
    }

    fn category_recorded_in(path: &std::path::Path) -> Option<ParaCategory> {
        let raw = fs::read_to_string(path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        frontmatter::parse_note(&raw, &name).0.category
    }

    #[test]
    fn opening_a_vault_creates_the_para_folders() {
        let vault = scaffolded_vault("structure");
        for category in ParaCategory::ALL {
            assert!(vault.join(category.folder_name()).is_dir());
        }
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_new_note_records_the_category_it_was_filed_under() {
        let vault = scaffolded_vault("create");
        let vault_str = vault.to_string_lossy().to_string();

        let entry = create_note(&vault_str, Some("Projects"), "Launch plan").unwrap();

        assert_eq!(entry.meta.category, Some(ParaCategory::Projects));
        assert_eq!(
            category_recorded_in(std::path::Path::new(&entry.path)),
            Some(ParaCategory::Projects),
            "the category must be on disk, not just in the returned value"
        );
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_note_cannot_be_created_outside_a_category() {
        let vault = scaffolded_vault("uncategorised");
        let vault_str = vault.to_string_lossy().to_string();

        // At the vault root, and in a folder that is not a PARA category.
        assert!(create_note(&vault_str, None, "Homeless").is_err());
        create_notebook(&vault_str, None, "Inbox").unwrap();
        assert!(create_note(&vault_str, Some("Inbox"), "Homeless").is_err());

        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_note_can_be_created_in_a_sub_folder_of_a_category() {
        let vault = scaffolded_vault("subfolder");
        let vault_str = vault.to_string_lossy().to_string();
        create_notebook(&vault_str, Some("Areas"), "Health").unwrap();

        let entry = create_note(&vault_str, Some("Areas/Health"), "Running").unwrap();

        assert_eq!(entry.meta.category, Some(ParaCategory::Areas));
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_a_note_between_categories_updates_its_recorded_category() {
        let vault = scaffolded_vault("move");
        let vault_str = vault.to_string_lossy().to_string();
        let entry = create_note(&vault_str, Some("Projects"), "Finished thing").unwrap();
        let original_id = entry.meta.id.clone();

        let archives = vault.join("Archives").to_string_lossy().to_string();
        let moved = move_note(&vault_str, &entry.path, &archives).unwrap();
        let moved_path = std::path::Path::new(&moved);

        assert_eq!(
            category_recorded_in(moved_path),
            Some(ParaCategory::Archives)
        );
        // Identity must survive the move, or version history keyed by id is orphaned.
        let raw = fs::read_to_string(moved_path).unwrap();
        let name = moved_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(frontmatter::parse_note(&raw, &name).0.id, original_id);

        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn every_ordered_category_pair_preserves_note_identity_and_body() {
        let vault = scaffolded_vault("move-category-matrix");
        let vault_str = vault.to_string_lossy().to_string();

        for source_category in ParaCategory::ALL {
            for destination_category in ParaCategory::ALL {
                if source_category == destination_category {
                    continue;
                }
                let title = format!(
                    "{} to {}",
                    source_category.folder_name(),
                    destination_category.folder_name()
                );
                let entry =
                    create_note(&vault_str, Some(source_category.folder_name()), &title).unwrap();
                let raw = fs::read_to_string(&entry.path).unwrap();
                fs::write(&entry.path, format!("{raw}matrix-body-{title}\n")).unwrap();

                let moved = move_note(
                    &vault_str,
                    &entry.path,
                    &vault
                        .join(destination_category.folder_name())
                        .to_string_lossy(),
                )
                .unwrap();
                let moved_raw = fs::read_to_string(&moved).unwrap();
                let moved_meta = frontmatter::parse_note(&moved_raw, &format!("{title}.md")).0;

                assert_eq!(moved_meta.id, entry.meta.id);
                assert_eq!(moved_meta.category, Some(destination_category));
                assert!(moved_raw.contains(&format!("matrix-body-{title}")));
                assert!(!std::path::Path::new(&entry.path).exists());
            }
        }

        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_a_note_preserves_its_body() {
        let vault = scaffolded_vault("move-body");
        let vault_str = vault.to_string_lossy().to_string();
        let entry = create_note(&vault_str, Some("Resources"), "Reference").unwrap();
        let raw = fs::read_to_string(&entry.path).unwrap();
        let body = frontmatter::parse_note(&raw, "Reference.md").1;
        fs::write(
            &entry.path,
            raw.replace(&body, "important content worth keeping\n"),
        )
        .unwrap();

        let archives = vault.join("Archives").to_string_lossy().to_string();
        let moved = move_note(&vault_str, &entry.path, &archives).unwrap();

        let moved_raw = fs::read_to_string(&moved).unwrap();
        assert!(moved_raw.contains("important content worth keeping"));
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_a_note_never_overwrites_a_same_named_destination() {
        let vault = scaffolded_vault("move-collision");
        let vault_str = vault.to_string_lossy().to_string();
        let incoming = create_note(&vault_str, Some("Projects"), "Plan").unwrap();
        let incoming_raw = fs::read_to_string(&incoming.path).unwrap();
        fs::write(&incoming.path, format!("{incoming_raw}incoming body\n")).unwrap();
        let existing = create_note(&vault_str, Some("Resources"), "Plan").unwrap();
        let existing_raw = fs::read_to_string(&existing.path).unwrap();
        fs::write(&existing.path, format!("{existing_raw}existing body\n")).unwrap();

        let resources = vault.join("Resources");
        let moved = move_note(&vault_str, &incoming.path, &resources.to_string_lossy()).unwrap();
        let moved_path = std::path::Path::new(&moved);
        let existing_after = fs::read_to_string(&existing.path).unwrap();
        let moved_after = fs::read_to_string(moved_path).unwrap();

        assert_eq!(moved_path.file_name().unwrap(), "Plan 1.md");
        assert!(existing_after.contains("existing body"));
        assert!(moved_after.contains("incoming body"));
        assert_eq!(
            category_recorded_in(moved_path),
            Some(ParaCategory::Resources)
        );
        assert!(!std::path::Path::new(&incoming.path).exists());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_a_note_rejects_destinations_outside_para_categories() {
        let vault = scaffolded_vault("move-destination-policy");
        let vault_str = vault.to_string_lossy().to_string();
        let root_note = create_note(&vault_str, Some("Projects"), "Root attempt").unwrap();
        let legacy_note = create_note(&vault_str, Some("Projects"), "Legacy attempt").unwrap();
        let legacy_dir = vault.join("Inbox");
        fs::create_dir(&legacy_dir).unwrap();

        let root_result = move_note(&vault_str, &root_note.path, &vault_str);
        let legacy_result = move_note(&vault_str, &legacy_note.path, &legacy_dir.to_string_lossy());

        assert!(root_result.is_err(), "the vault root is not a category");
        assert!(
            legacy_result.is_err(),
            "a legacy top-level folder is not a category"
        );
        assert!(std::path::Path::new(&root_note.path).is_file());
        assert!(std::path::Path::new(&legacy_note.path).is_file());
        assert_eq!(
            category_recorded_in(std::path::Path::new(&root_note.path)),
            Some(ParaCategory::Projects)
        );
        assert_eq!(
            category_recorded_in(std::path::Path::new(&legacy_note.path)),
            Some(ParaCategory::Projects)
        );
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_a_note_updates_path_based_wikilinks() {
        let vault = scaffolded_vault("move-wikilinks");
        let vault_str = vault.to_string_lossy().to_string();
        let target = create_note(&vault_str, Some("Projects"), "Target").unwrap();
        let reference = create_note(&vault_str, Some("Resources"), "Reference").unwrap();
        let reference_raw = fs::read_to_string(&reference.path).unwrap();
        fs::write(
            &reference.path,
            format!("{reference_raw}See [[Projects/Target]] and [[Projects/Target|the target]].\n"),
        )
        .unwrap();

        let archives = vault.join("Archives");
        move_note(&vault_str, &target.path, &archives.to_string_lossy()).unwrap();
        let reference_after = fs::read_to_string(&reference.path).unwrap();

        assert!(reference_after.contains("[[Archives/Target]]"));
        assert!(reference_after.contains("[[Archives/Target|the target]]"));
        assert!(!reference_after.contains("[[Projects/Target"));
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_a_note_updates_quick_access_to_the_collision_safe_destination() {
        let vault = scaffolded_vault("move-quick-access");
        let vault_str = vault.to_string_lossy().to_string();
        let target = create_note(&vault_str, Some("Projects"), "Plan").unwrap();
        let _collision = create_note(&vault_str, Some("Archives"), "Plan").unwrap();
        save_quick_access(&vault_str, &["Projects/Plan.md".to_string()]).unwrap();

        let moved = move_note(
            &vault_str,
            &target.path,
            &vault.join("Archives").to_string_lossy(),
        )
        .unwrap();

        assert_eq!(
            std::path::Path::new(&moved),
            vault.join("Archives/Plan 1.md")
        );
        assert_eq!(
            load_quick_access(&vault_str).unwrap(),
            ["Archives/Plan 1.md"]
        );
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn corrupt_quick_access_blocks_a_move_before_the_source_changes() {
        let vault = scaffolded_vault("move-corrupt-quick-access");
        let vault_str = vault.to_string_lossy().to_string();
        let target = create_note(&vault_str, Some("Projects"), "Plan").unwrap();
        let source_before = fs::read(&target.path).unwrap();
        fs::write(helixnotes_dir(&vault_str).join("quick_access.json"), "{").unwrap();

        let result = move_note(
            &vault_str,
            &target.path,
            &vault.join("Archives").to_string_lossy(),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&target.path).unwrap(), source_before);
        assert!(!vault.join("Archives/Plan.md").exists());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn unreadable_backlink_content_blocks_a_move_before_the_source_changes() {
        let vault = scaffolded_vault("move-unreadable-backlink");
        let vault_str = vault.to_string_lossy().to_string();
        let target = create_note(&vault_str, Some("Projects"), "Plan").unwrap();
        let source_before = fs::read(&target.path).unwrap();
        fs::write(vault.join("Resources/Invalid.md"), [0xff, 0xfe, 0xfd]).unwrap();

        let result = move_note(
            &vault_str,
            &target.path,
            &vault.join("Archives").to_string_lossy(),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&target.path).unwrap(), source_before);
        assert!(!vault.join("Archives/Plan.md").exists());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn backlink_write_failure_rolls_back_the_note_links_and_quick_access() {
        let vault = scaffolded_vault("move-backlink-rollback");
        let vault_str = vault.to_string_lossy().to_string();
        let target = create_note(&vault_str, Some("Projects"), "Target").unwrap();
        let first = create_note(&vault_str, Some("Resources"), "First reference").unwrap();
        let second = create_note(&vault_str, Some("Areas"), "Second reference").unwrap();
        for reference in [&first, &second] {
            let raw = fs::read_to_string(&reference.path).unwrap();
            fs::write(&reference.path, format!("{raw}See [[Projects/Target]].\n")).unwrap();
        }
        super::add_quick_access(&vault_str, "Projects/Target.md").unwrap();
        let source_before = fs::read(&target.path).unwrap();
        let first_before = fs::read(&first.path).unwrap();
        let second_before = fs::read(&second.path).unwrap();
        let quick_access_path = helixnotes_dir(&vault_str).join("quick_access.json");
        let quick_access_before = fs::read(&quick_access_path).unwrap();

        let result = super::move_note_with_outcome_inner(
            &vault_str,
            &target.path,
            &vault.join("Archives").to_string_lossy(),
            Some(1),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&target.path).unwrap(), source_before);
        assert_eq!(fs::read(&first.path).unwrap(), first_before);
        assert_eq!(fs::read(&second.path).unwrap(), second_before);
        assert_eq!(fs::read(quick_access_path).unwrap(), quick_access_before);
        assert!(!vault.join("Archives/Target.md").exists());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_a_note_reindexes_the_target_and_every_rewritten_backlink() {
        let vault = scaffolded_vault("move-search-consumers");
        let vault_str = vault.to_string_lossy().to_string();
        create_notebook(&vault_str, Some("Projects"), "OldUniqueFolder").unwrap();
        create_notebook(&vault_str, Some("Archives"), "NewUniqueFolder").unwrap();
        let target = create_note(&vault_str, Some("Projects/OldUniqueFolder"), "Target").unwrap();
        let reference = create_note(&vault_str, Some("Resources"), "Reference").unwrap();
        let reference_raw = fs::read_to_string(&reference.path).unwrap();
        fs::write(
            &reference.path,
            format!("{reference_raw}See [[Projects/OldUniqueFolder/Target]].\n"),
        )
        .unwrap();
        let search = SearchIndex::new_in_memory().unwrap();
        search.index_note(&target.path).unwrap();
        search.index_note(&reference.path).unwrap();

        let outcome = move_note_with_outcome(
            &vault_str,
            &target.path,
            &vault.join("Archives/NewUniqueFolder").to_string_lossy(),
        )
        .unwrap();
        let mut upserts = vec![outcome.path.clone()];
        upserts.extend(outcome.rewritten_paths.clone());
        search
            .apply_note_changes(std::slice::from_ref(&target.path), &upserts)
            .unwrap();

        assert!(search.search("OldUniqueFolder", 10).unwrap().is_empty());
        let new_results = search.search("NewUniqueFolder", 10).unwrap();
        assert_eq!(new_results.len(), 1);
        assert_eq!(new_results[0].path, reference.path);
        let target_results = search.search("Target", 10).unwrap();
        assert!(target_results
            .iter()
            .any(|result| result.path == outcome.path));
        assert!(target_results
            .iter()
            .all(|result| result.path != target.path));
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_a_note_with_malformed_frontmatter_leaves_the_source_unchanged() {
        let vault = scaffolded_vault("move-malformed-frontmatter");
        let vault_str = vault.to_string_lossy().to_string();
        let source = vault.join("Projects").join("Broken.md");
        let original =
            "---\nid: \"stable-id\"\ntitle: [broken\ncategory: Projects\n---\nvaluable body\n";
        fs::write(&source, original).unwrap();

        let resources = vault.join("Resources");
        let result = move_note(
            &vault_str,
            &source.to_string_lossy(),
            &resources.to_string_lossy(),
        );
        let source_after = fs::read_to_string(&source).ok();
        let destination_exists = resources.join("Broken.md").exists();

        fs::remove_dir_all(vault).unwrap();

        assert!(result.is_err(), "malformed frontmatter must block the move");
        assert_eq!(source_after.as_deref(), Some(original));
        assert!(!destination_exists);
    }

    #[test]
    fn restoring_a_categorized_note_uses_its_category_without_overwriting() {
        let vault = scaffolded_vault("restore-category");
        let vault_str = vault.to_string_lossy().to_string();
        let original = create_note(&vault_str, Some("Projects"), "Plan").unwrap();
        let original_id = original.meta.id.clone();
        super::delete_note(&vault_str, &original.path).unwrap();
        let replacement = create_note(&vault_str, Some("Projects"), "Plan").unwrap();
        let replacement_id = replacement.meta.id.clone();
        let trash = super::get_trash_contents(&vault_str).unwrap();
        let trash_path = trash.notes.first().unwrap().path.clone();

        let restored = super::restore_note(&vault_str, &trash_path).unwrap();
        let restored_path = std::path::Path::new(&restored);

        assert_eq!(restored_path, vault.join("Projects").join("Plan 1.md"));
        assert_eq!(
            category_recorded_in(restored_path),
            Some(ParaCategory::Projects)
        );
        let restored_raw = fs::read_to_string(restored_path).unwrap();
        let restored_meta = frontmatter::parse_note(&restored_raw, "Plan 1.md").0;
        assert_eq!(restored_meta.id, original_id);
        assert_eq!(
            category_recorded_in(std::path::Path::new(&replacement.path)),
            Some(ParaCategory::Projects)
        );
        let replacement_raw = fs::read_to_string(&replacement.path).unwrap();
        assert_eq!(
            frontmatter::parse_note(&replacement_raw, "Plan.md").0.id,
            replacement_id
        );
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn restoring_an_uncategorized_legacy_note_uses_the_holding_area() {
        let vault = scaffolded_vault("restore-uncategorized");
        let vault_str = vault.to_string_lossy().to_string();
        let trash = helixnotes_dir(&vault_str).join("trash");
        let trashed = trash.join("20240101000000000_Legacy.md");
        fs::write(
            &trashed,
            "---\nid: \"legacy-id\"\ntitle: \"Legacy\"\n---\nlegacy body\n",
        )
        .unwrap();

        let restored = super::restore_note(&vault_str, &trashed.to_string_lossy()).unwrap();
        let expected = super::unfiled_dir(&vault_str).join("Legacy.md");

        assert_eq!(std::path::Path::new(&restored), expected);
        assert!(expected.is_file());
        assert!(!vault.join("Legacy.md").exists());
        assert_eq!(category_recorded_in(&expected), None);
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn delete_create_restore_preserves_quick_access_by_note_identity() {
        let vault = scaffolded_vault("restore-quick-access-identity");
        let vault_str = vault.to_string_lossy().to_string();
        let original = create_note(&vault_str, Some("Projects"), "Plan").unwrap();
        super::add_quick_access(&vault_str, "Projects/Plan.md").unwrap();

        super::delete_note(&vault_str, &original.path).unwrap();
        let replacement = create_note(&vault_str, Some("Projects"), "Plan").unwrap();
        let trash_path = super::get_trash_contents(&vault_str).unwrap().notes[0]
            .path
            .clone();
        let restored = super::restore_note(&vault_str, &trash_path).unwrap();

        assert_eq!(
            std::path::Path::new(&restored),
            vault.join("Projects/Plan 1.md")
        );
        assert_eq!(
            super::load_quick_access(&vault_str).unwrap(),
            ["Projects/Plan 1.md"]
        );
        let stored = super::load_quick_access_entries(&vault_str).unwrap();
        assert_eq!(
            stored[0].note_id.as_deref(),
            Some(original.meta.id.as_str())
        );
        assert_ne!(
            stored[0].note_id.as_deref(),
            Some(replacement.meta.id.as_str())
        );
        let restored_raw = fs::read_to_string(restored).unwrap();
        assert_eq!(
            frontmatter::parse_note(&restored_raw, "Plan 1.md").0.id,
            original.meta.id
        );
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn legacy_quick_access_migrates_to_note_ids_without_reordering() {
        let vault = scaffolded_vault("quick-access-migration");
        let vault_str = vault.to_string_lossy().to_string();
        let first = create_note(&vault_str, Some("Projects"), "First").unwrap();
        let second = create_note(&vault_str, Some("Resources"), "Second").unwrap();
        let legacy = vec!["Resources/Second.md", "Projects/First.md"];
        fs::write(
            helixnotes_dir(&vault_str).join("quick_access.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();

        assert_eq!(super::load_quick_access(&vault_str).unwrap(), legacy);
        let stored = super::load_quick_access_entries(&vault_str).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].note_id.as_deref(), Some(second.meta.id.as_str()));
        assert_eq!(stored[1].note_id.as_deref(), Some(first.meta.id.as_str()));
        let disk: serde_json::Value = serde_json::from_slice(
            &fs::read(helixnotes_dir(&vault_str).join("quick_access.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(disk["version"], 1);
        assert!(disk["entries"].is_array());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn filing_an_unfiled_note_gives_it_a_category_and_moves_it() {
        let vault = scaffolded_vault("filing");
        let vault_str = vault.to_string_lossy().to_string();
        let unfiled = super::unfiled_dir(&vault_str);
        fs::create_dir_all(&unfiled).unwrap();
        let waiting = unfiled.join("waiting.md");
        fs::write(&waiting, "---\nid: \"x\"\ntitle: \"Waiting\"\n---\nbody\n").unwrap();

        let filed =
            super::file_unfiled_note(&vault_str, &waiting.to_string_lossy(), "Resources").unwrap();

        let filed_path = std::path::Path::new(&filed);
        assert!(filed_path.starts_with(vault.join("Resources")));
        assert_eq!(
            category_recorded_in(filed_path),
            Some(ParaCategory::Resources),
            "the category must be written onto the note, not just implied by the folder"
        );
        assert!(!waiting.exists());
        assert!(super::list_unfiled_notes(&vault_str).unwrap().is_empty());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn filing_rejects_a_category_that_is_not_one_of_the_four() {
        let vault = scaffolded_vault("filing-bad");
        let vault_str = vault.to_string_lossy().to_string();
        let unfiled = super::unfiled_dir(&vault_str);
        fs::create_dir_all(&unfiled).unwrap();
        let waiting = unfiled.join("waiting.md");
        fs::write(&waiting, "---\nid: \"x\"\ntitle: \"Waiting\"\n---\nbody\n").unwrap();

        assert!(super::file_unfiled_note(&vault_str, &waiting.to_string_lossy(), "Inbox").is_err());
        assert!(
            waiting.exists(),
            "a rejected filing must leave the note alone"
        );
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn filing_only_applies_to_notes_in_the_holding_area() {
        let vault = scaffolded_vault("filing-scope");
        let vault_str = vault.to_string_lossy().to_string();
        let entry = create_note(&vault_str, Some("Projects"), "Already filed").unwrap();

        assert!(super::file_unfiled_note(&vault_str, &entry.path, "Archives").is_err());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn filing_rejects_a_traversal_path_without_touching_the_outside_file() {
        let vault = scaffolded_vault("filing-traversal");
        let vault_str = vault.to_string_lossy().to_string();
        let unfiled = super::unfiled_dir(&vault_str);
        fs::create_dir_all(&unfiled).unwrap();

        let outside = std::env::temp_dir().join(format!("outside-{}.md", Uuid::new_v4()));
        let original = "---\nid: \"outside\"\ntitle: \"Outside\"\n---\nuntouched\n";
        fs::write(&outside, original).unwrap();
        let traversal = unfiled
            .join("..")
            .join("..")
            .join("..")
            .join(outside.file_name().unwrap());

        let result =
            super::file_unfiled_note(&vault_str, &traversal.to_string_lossy(), "Resources");
        let outside_after = fs::read_to_string(&outside).ok();

        fs::remove_dir_all(&vault).unwrap();
        if outside.exists() {
            fs::remove_file(&outside).unwrap();
        }

        assert!(result.is_err(), "path traversal must be rejected");
        assert_eq!(
            outside_after.as_deref(),
            Some(original),
            "a rejected path must not read-modify-write an outside file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn filing_rejects_a_symlinked_note_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let vault = scaffolded_vault("filing-note-symlink");
        let vault_str = vault.to_string_lossy().to_string();
        let unfiled = super::unfiled_dir(&vault_str);
        fs::create_dir_all(&unfiled).unwrap();

        let outside = std::env::temp_dir().join(format!("outside-{}.md", Uuid::new_v4()));
        let original = "---\nid: \"outside\"\ntitle: \"Outside\"\n---\nuntouched\n";
        fs::write(&outside, original).unwrap();
        let linked_note = unfiled.join("linked.md");
        symlink(&outside, &linked_note).unwrap();

        let result =
            super::file_unfiled_note(&vault_str, &linked_note.to_string_lossy(), "Resources");
        let outside_after = fs::read_to_string(&outside).unwrap();

        fs::remove_dir_all(&vault).unwrap();
        fs::remove_file(&outside).unwrap();

        assert!(result.is_err(), "symlinked notes must be rejected");
        assert_eq!(
            outside_after, original,
            "a rejected symlink must not mutate its outside target"
        );
    }

    #[cfg(unix)]
    #[test]
    fn filing_rejects_a_symlinked_holding_directory() {
        use std::os::unix::fs::symlink;

        let vault = scaffolded_vault("filing-dir-symlink");
        let vault_str = vault.to_string_lossy().to_string();
        let unfiled = super::unfiled_dir(&vault_str);
        let outside_dir = std::env::temp_dir().join(format!("outside-holding-{}", Uuid::new_v4()));
        fs::create_dir_all(&outside_dir).unwrap();
        symlink(&outside_dir, &unfiled).unwrap();
        let outside_note = outside_dir.join("waiting.md");
        let original = "---\nid: \"outside\"\ntitle: \"Outside\"\n---\nuntouched\n";
        fs::write(&outside_note, original).unwrap();

        let linked_note = unfiled.join("waiting.md");
        let result =
            super::file_unfiled_note(&vault_str, &linked_note.to_string_lossy(), "Resources");
        let outside_after = fs::read_to_string(&outside_note).ok();

        fs::remove_dir_all(&vault).unwrap();
        fs::remove_dir_all(&outside_dir).unwrap();

        assert!(
            result.is_err(),
            "a symlinked holding directory must be rejected"
        );
        assert_eq!(
            outside_after.as_deref(),
            Some(original),
            "a rejected holding directory must not move or rewrite an outside note"
        );
    }

    #[cfg(unix)]
    #[test]
    fn filing_rejects_a_symlinked_category_destination_before_rewriting_the_note() {
        use std::os::unix::fs::symlink;

        let vault = scaffolded_vault("filing-destination-symlink");
        let vault_str = vault.to_string_lossy().to_string();
        let unfiled = super::unfiled_dir(&vault_str);
        fs::create_dir_all(&unfiled).unwrap();
        let waiting = unfiled.join("waiting.md");
        let original = "---\nid: \"waiting\"\ntitle: \"Waiting\"\n---\nuntouched\n";
        fs::write(&waiting, original).unwrap();

        let resources = vault.join("Resources");
        fs::remove_dir(&resources).unwrap();
        let outside_dir = std::env::temp_dir().join(format!("outside-category-{}", Uuid::new_v4()));
        fs::create_dir_all(&outside_dir).unwrap();
        symlink(&outside_dir, &resources).unwrap();

        let result = super::file_unfiled_note(&vault_str, &waiting.to_string_lossy(), "Resources");
        let source_after = fs::read_to_string(&waiting).ok();
        let outside_entries = fs::read_dir(&outside_dir).unwrap().count();

        fs::remove_dir_all(&vault).unwrap();
        fs::remove_dir_all(&outside_dir).unwrap();

        assert!(result.is_err(), "symlinked category roots must be rejected");
        assert_eq!(
            source_after.as_deref(),
            Some(original),
            "destination validation must happen before the source is rewritten"
        );
        assert_eq!(
            outside_entries, 0,
            "a rejected category destination must not receive a note"
        );
    }

    #[test]
    fn filing_only_accepts_regular_markdown_files() {
        let vault = scaffolded_vault("filing-kind");
        let vault_str = vault.to_string_lossy().to_string();
        let unfiled = super::unfiled_dir(&vault_str);
        fs::create_dir_all(&unfiled).unwrap();
        let text_file = unfiled.join("waiting.txt");
        let original = "not a note";
        fs::write(&text_file, original).unwrap();
        let directory = unfiled.join("directory.md");
        fs::create_dir(&directory).unwrap();

        let text_result =
            super::file_unfiled_note(&vault_str, &text_file.to_string_lossy(), "Resources");
        let directory_result =
            super::file_unfiled_note(&vault_str, &directory.to_string_lossy(), "Resources");
        let text_after = fs::read_to_string(&text_file).ok();

        fs::remove_dir_all(&vault).unwrap();

        assert!(text_result.is_err(), "non-Markdown files must be rejected");
        assert!(directory_result.is_err(), "directories must be rejected");
        assert_eq!(
            text_after.as_deref(),
            Some(original),
            "a rejected non-Markdown file must not be moved or rewritten"
        );
    }

    #[test]
    fn the_category_folders_cannot_be_renamed_deleted_or_moved() {
        let vault = scaffolded_vault("protected");
        let vault_str = vault.to_string_lossy().to_string();
        let projects = vault.join("Projects").to_string_lossy().to_string();

        assert!(super::rename_notebook(&vault_str, &projects, "Stuff").is_err());
        assert!(super::delete_notebook(&vault_str, &projects).is_err());
        let areas = vault.join("Areas").to_string_lossy().to_string();
        assert!(super::move_notebook(&vault_str, &projects, &areas).is_err());

        // Still there, and still a category.
        assert!(vault.join("Projects").is_dir());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn folders_inside_a_category_remain_editable() {
        // Only the four roots are fixed; ordinary organisation within them is not.
        let vault = scaffolded_vault("sub-editable");
        let vault_str = vault.to_string_lossy().to_string();
        create_notebook(&vault_str, Some("Projects"), "Launch").unwrap();
        let sub = vault.join("Projects/Launch").to_string_lossy().to_string();

        let renamed = super::rename_notebook(&vault_str, &sub, "Relaunch").unwrap();

        assert!(std::path::Path::new(&renamed).is_dir());
        assert!(vault.join("Projects/Relaunch").is_dir());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn moving_notebook_between_categories_updates_descendants_and_survives_reconciliation() {
        let vault = scaffolded_vault("move-notebook-category");
        let vault_str = vault.to_string_lossy().to_string();
        create_notebook(&vault_str, Some("Projects"), "Launch").unwrap();
        create_notebook(&vault_str, Some("Projects/Launch"), "Drafts").unwrap();
        let plan = create_note(&vault_str, Some("Projects/Launch"), "Plan").unwrap();
        let draft = create_note(&vault_str, Some("Projects/Launch/Drafts"), "Draft").unwrap();
        let plan_raw = fs::read_to_string(&plan.path).unwrap();
        fs::write(
            &plan.path,
            format!("{plan_raw}unique-folder-move-search-token\n"),
        )
        .unwrap();
        let reference = create_note(&vault_str, Some("Resources"), "Reference").unwrap();
        let reference_raw = fs::read_to_string(&reference.path).unwrap();
        fs::write(
            &reference.path,
            format!("{reference_raw}[[Projects/Launch/Plan]] [[Projects/Launch/Drafts/Draft]]\n"),
        )
        .unwrap();
        super::add_quick_access(&vault_str, "Projects/Launch/Drafts/Draft.md").unwrap();
        let search = SearchIndex::new_in_memory().unwrap();
        search.rebuild(&vault_str).unwrap();

        let moved = super::move_notebook(
            &vault_str,
            &vault.join("Projects/Launch").to_string_lossy(),
            &vault.join("Archives").to_string_lossy(),
        )
        .unwrap();
        let moved_root = std::path::Path::new(&moved);
        let moved_plan = moved_root.join("Plan.md");
        let moved_draft = moved_root.join("Drafts/Draft.md");

        assert_eq!(
            category_recorded_in(&moved_plan),
            Some(ParaCategory::Archives)
        );
        assert_eq!(
            category_recorded_in(&moved_draft),
            Some(ParaCategory::Archives)
        );
        for (path, original) in [(&moved_plan, &plan), (&moved_draft, &draft)] {
            let raw = fs::read_to_string(path).unwrap();
            let filename = path.file_name().unwrap().to_string_lossy();
            assert_eq!(
                frontmatter::parse_note(&raw, &filename).0.id,
                original.meta.id
            );
        }
        let reference_after = fs::read_to_string(&reference.path).unwrap();
        assert!(reference_after.contains("[[Archives/Launch/Plan]]"));
        assert!(reference_after.contains("[[Archives/Launch/Drafts/Draft]]"));
        assert_eq!(
            super::load_quick_access(&vault_str).unwrap(),
            ["Archives/Launch/Drafts/Draft.md"]
        );
        let report = super::reconcile_categories(&vault_str).unwrap();
        search.rebuild(&vault_str).unwrap();
        let search_results = search
            .search("unique-folder-move-search-token", 10)
            .unwrap();
        assert_eq!(report.relocated, 0);
        assert_eq!(search_results.len(), 1);
        assert_eq!(search_results[0].path, moved_plan.to_string_lossy());
        assert!(search_results
            .iter()
            .all(|result| !result.path.contains("Projects/Launch")));
        assert!(moved_plan.is_file());
        assert!(moved_draft.is_file());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn a_pre_para_vault_keeps_its_notes_when_opened() {
        // Opening an existing vault must add structure without moving or losing notes.
        let vault = std::env::temp_dir().join(format!("para-legacy-{}", Uuid::new_v4()));
        fs::create_dir_all(&vault).unwrap();
        fs::write(vault.join("existing.md"), "# Existing\n\nvaluable\n").unwrap();

        ensure_vault_structure(&vault.to_string_lossy()).unwrap();

        assert_eq!(
            fs::read_to_string(vault.join("existing.md")).unwrap(),
            "# Existing\n\nvaluable\n",
            "an existing note must be left exactly as it was"
        );
        assert_eq!(category_recorded_in(&vault.join("existing.md")), None);
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn compares_numeric_segments_anywhere_in_names() {
        let mut names = ["Class 10b", "Class 2b", "Class 10a", "Class 2a"];
        names.sort_by(|left, right| compare_natural_names(left, right));
        assert_eq!(names, ["Class 2a", "Class 2b", "Class 10a", "Class 10b"]);
        assert_eq!(
            compare_natural_names("Class 02", "Class 2b"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn sorts_notebooks_with_numeric_names_naturally() {
        let vault =
            std::env::temp_dir().join(format!("helixnotes-natural-sort-test-{}", Uuid::new_v4()));
        let nested = vault.join("Classes");
        fs::create_dir_all(&nested).unwrap();
        let expected = ["5g", "6g", "7g", "8g", "9g", "10g", "11g", "12g"];
        for name in ["10g", "11g", "12g", "5g", "6g", "7g", "8g", "9g"] {
            fs::create_dir(vault.join(name)).unwrap();
            fs::create_dir(nested.join(name)).unwrap();
        }

        let notebooks = scan_notebooks(&vault.to_string_lossy()).unwrap();
        let root_names: Vec<_> = notebooks
            .iter()
            .filter(|notebook| notebook.name != "Classes")
            .map(|notebook| notebook.name.as_str())
            .collect();
        assert_eq!(root_names, expected);
        let nested_names: Vec<_> = notebooks
            .iter()
            .find(|notebook| notebook.name == "Classes")
            .unwrap()
            .children
            .iter()
            .map(|notebook| notebook.name.as_str())
            .collect();
        assert_eq!(nested_names, expected);

        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn persists_and_removes_builtin_notebook_icons() {
        let vault =
            std::env::temp_dir().join(format!("helixnotes-notebook-icon-test-{}", Uuid::new_v4()));
        let vault_path = vault.to_string_lossy();
        fs::create_dir_all(helixnotes_dir(&vault_path)).unwrap();

        set_notebook_icon(&vault_path, r"Projects\Client", Some("builtin:briefcase")).unwrap();
        let stored: std::collections::HashMap<String, String> = serde_json::from_str(
            &fs::read_to_string(helixnotes_dir(&vault_path).join("notebook_icons.json")).unwrap(),
        )
        .unwrap();
        assert!(stored.contains_key("Projects/Client"));
        assert!(!stored.contains_key(r"Projects\Client"));

        let icons = load_notebook_icons(&vault_path).unwrap();
        assert_eq!(
            icons.get("Projects/Client").map(String::as_str),
            Some("builtin:briefcase")
        );

        set_notebook_icon(&vault_path, "Projects/Client", None).unwrap();
        assert!(load_notebook_icons(&vault_path).unwrap().is_empty());

        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn normalizes_legacy_notebook_icon_keys_when_loading() {
        let vault =
            std::env::temp_dir().join(format!("helixnotes-notebook-icon-test-{}", Uuid::new_v4()));
        let vault_path = vault.to_string_lossy();
        let icons_path = helixnotes_dir(&vault_path).join("notebook_icons.json");
        fs::create_dir_all(helixnotes_dir(&vault_path)).unwrap();
        fs::write(&icons_path, r#"{"Projects\\Client":"builtin:folder"}"#).unwrap();

        let icons = load_notebook_icons(&vault_path).unwrap();
        assert_eq!(
            icons.get("Projects/Client").map(String::as_str),
            Some("builtin:folder")
        );

        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn loads_only_requested_note_switcher_titles() {
        let vault =
            std::env::temp_dir().join(format!("helixnotes-note-switcher-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&vault).unwrap();

        let mut note_paths = Vec::new();
        for index in 0..8 {
            let path = vault.join(format!("Note {index}.md"));
            fs::write(
                &path,
                format!("---\ntitle: Note {index}\n---\n\nNote {index} body.\n"),
            )
            .unwrap();
            note_paths.push(path);
        }
        fs::write(
            vault.join("Unrelated.md"),
            "---\ntitle: Unrelated\n---\n\nMust not be loaded.\n",
        )
        .unwrap();

        let outside =
            std::env::temp_dir().join(format!("helixnotes-outside-note-{}.md", Uuid::new_v4()));
        fs::write(&outside, "---\ntitle: Outside\n---\n").unwrap();

        let requested = vec![
            note_paths[0].to_string_lossy().into_owned(),
            note_paths[0].to_string_lossy().into_owned(),
            vault.join("Missing.md").to_string_lossy().into_owned(),
            outside.to_string_lossy().into_owned(),
            note_paths[1].to_string_lossy().into_owned(),
            note_paths[2].to_string_lossy().into_owned(),
            note_paths[3].to_string_lossy().into_owned(),
            note_paths[4].to_string_lossy().into_owned(),
            note_paths[5].to_string_lossy().into_owned(),
            note_paths[6].to_string_lossy().into_owned(),
            note_paths[7].to_string_lossy().into_owned(),
        ];

        let vault_path = vault.to_string_lossy();
        let titles = get_note_switcher_titles(&vault_path, &requested).unwrap();
        assert_eq!(titles.len(), 6);
        assert_eq!(
            titles
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Note 0.md",
                "Note 1.md",
                "Note 2.md",
                "Note 3.md",
                "Note 4.md",
                "Note 5.md",
            ]
        );
        assert!(titles.iter().all(|entry| entry.title != "Unrelated"));
        assert!(titles.iter().all(|entry| entry.title != "Outside"));

        fs::remove_dir_all(vault).unwrap();
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn reads_markdown_notes_from_trash_without_allowing_external_files() {
        let test_root =
            std::env::temp_dir().join(format!("helixnotes-path-security-test-{}", Uuid::new_v4()));
        let vault = test_root.join("vault");
        let trash = helixnotes_dir(&vault.to_string_lossy()).join("trash");
        let trashed_note = trash.join("20240101000000000_Note.md");
        let outside = test_root.join("outside.md");
        fs::create_dir_all(&trash).unwrap();
        fs::write(&trashed_note, "---\ntitle: Note\n---\n\ntrashed").unwrap();
        fs::write(&outside, "outside").unwrap();

        assert!(read_note(&vault.to_string_lossy(), &trashed_note.to_string_lossy()).is_ok());
        assert!(read_note(&vault.to_string_lossy(), &outside.to_string_lossy()).is_err());

        fs::remove_dir_all(test_root).unwrap();
    }

    #[test]
    fn rejects_permanent_deletion_outside_trash() {
        let test_root =
            std::env::temp_dir().join(format!("helixnotes-path-security-test-{}", Uuid::new_v4()));
        let vault = test_root.join("vault");
        let outside = test_root.join("outside.md");
        fs::create_dir_all(helixnotes_dir(&vault.to_string_lossy()).join("trash")).unwrap();
        fs::write(&outside, "must survive").unwrap();

        let result = permanent_delete(&vault.to_string_lossy(), &outside.to_string_lossy());

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "must survive");
        fs::remove_dir_all(test_root).unwrap();
    }

    #[test]
    fn permanent_deletion_removes_the_note_restore_manifest() {
        let vault = scaffolded_vault("permanent-delete-manifest");
        let vault_str = vault.to_string_lossy().to_string();
        let note = create_note(&vault_str, Some("Projects"), "Disposable").unwrap();
        super::delete_note(&vault_str, &note.path).unwrap();
        let trash_path = super::get_trash_contents(&vault_str).unwrap().notes[0]
            .path
            .clone();
        let manifest = super::trashed_note_manifest_path(std::path::Path::new(&trash_path));
        assert!(manifest.is_file());

        permanent_delete(&vault_str, &trash_path).unwrap();

        assert!(!std::path::Path::new(&trash_path).exists());
        assert!(!manifest.exists());
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn rejects_notebook_creation_outside_vault() {
        let test_root =
            std::env::temp_dir().join(format!("helixnotes-path-security-test-{}", Uuid::new_v4()));
        let vault = test_root.join("vault");
        fs::create_dir_all(&vault).unwrap();

        let result = create_notebook(&vault.to_string_lossy(), Some(".."), "escaped");

        assert!(result.is_err());
        assert!(!test_root.join("escaped").exists());
        fs::remove_dir_all(test_root).unwrap();
    }

    #[test]
    fn rejects_traversal_in_restored_notebook_metadata() {
        let test_root =
            std::env::temp_dir().join(format!("helixnotes-path-security-test-{}", Uuid::new_v4()));
        let vault = test_root.join("vault");
        let trash = helixnotes_dir(&vault.to_string_lossy()).join("trash");
        let trashed_notebook = trash.join("20240101000000000_Notebook");
        fs::create_dir_all(&trashed_notebook).unwrap();
        fs::write(trash.join("20240101000000000_Notebook.meta"), "../escaped").unwrap();

        let result = restore_notebook(
            &vault.to_string_lossy(),
            &trashed_notebook.to_string_lossy(),
        );

        assert!(result.is_err());
        assert!(trashed_notebook.exists());
        assert!(!test_root.join("escaped").exists());
        fs::remove_dir_all(test_root).unwrap();
    }

    #[test]
    fn duplicates_note_content_and_assigns_unique_identity_and_name() {
        let vault =
            std::env::temp_dir().join(format!("helixnotes-duplicate-note-test-{}", Uuid::new_v4()));
        let notebook = vault.join("Projects");
        fs::create_dir_all(&notebook).unwrap();
        let source_path = notebook.join("Project.md");
        let source_raw = "---\nid: source-id\ntitle: Project\ntags:\n  - work\npinned: true\ncreated: 2020-01-01T00:00:00Z\nmodified: 2020-01-02T00:00:00Z\naliases:\n  - Plan\n---\n# Project\n\nOriginal body.\n";
        fs::write(&source_path, source_raw).unwrap();

        let vault_path = vault.to_string_lossy();
        let first = duplicate_note(&source_path.to_string_lossy(), &vault_path).unwrap();
        let second = duplicate_note(&source_path.to_string_lossy(), &vault_path).unwrap();

        assert_eq!(first.meta.title, "Project copy");
        assert_eq!(second.meta.title, "Project copy 2");
        assert_eq!(first.meta.tags, vec!["work"]);
        assert!(!first.meta.pinned);
        assert_ne!(first.meta.id, "source-id");
        assert_ne!(first.meta.id, second.meta.id);
        assert_eq!(first.relative_path, "Projects/Project copy.md");
        assert_eq!(second.relative_path, "Projects/Project copy 2.md");

        let first_raw = fs::read_to_string(&first.path).unwrap();
        assert!(first_raw.contains("aliases:\n- Plan"));
        assert!(first_raw.contains("# Project copy\n\nOriginal body."));
        assert!(!first_raw.contains("\n# Project\n"));
        assert_eq!(fs::read_to_string(&source_path).unwrap(), source_raw);

        fs::remove_dir_all(vault).unwrap();
    }
}
