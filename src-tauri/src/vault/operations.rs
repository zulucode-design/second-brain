use crate::types::{NoteContent, NoteEntry, NoteMeta, NotebookEntry, VaultState};
use crate::vault::frontmatter;
use chrono::Utc;
use rayon::prelude::*;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use walkdir::WalkDir;

pub fn helixnotes_dir(vault_path: &str) -> PathBuf {
    Path::new(vault_path).join(".helixnotes")
}

pub fn ensure_vault_structure(vault_path: &str) -> Result<(), String> {
    let hn_dir = helixnotes_dir(vault_path);
    fs::create_dir_all(hn_dir.join("trash")).map_err(|e| e.to_string())?;
    fs::create_dir_all(hn_dir.join("attachments")).map_err(|e| e.to_string())?;

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

pub fn scan_notebooks(vault_path: &str) -> Result<Vec<NotebookEntry>, String> {
    let root = Path::new(vault_path);
    if !root.exists() {
        return Err("Vault path does not exist".to_string());
    }
    Ok(scan_dir_recursive(root, vault_path))
}

fn scan_dir_recursive(dir: &Path, vault_root: &str) -> Vec<NotebookEntry> {
    let mut entries = Vec::new();
    let root = Path::new(vault_root);

    let Ok(read_dir) = fs::read_dir(dir) else {
        return entries;
    };

    let mut dirs: Vec<_> = read_dir
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            path.is_dir() && !is_hidden(&path)
        })
        .collect();

    dirs.sort_by(|a, b| {
        a.file_name()
            .to_string_lossy()
            .to_lowercase()
            .cmp(&b.file_name().to_string_lossy().to_lowercase())
    });

    for dir_entry in dirs {
        let path = dir_entry.path();
        let name = dir_entry.file_name().to_string_lossy().to_string();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        let children = scan_dir_recursive(&path, vault_root);
        let note_count = count_notes_in_dir(&path);

        entries.push(NotebookEntry {
            name,
            path: path.to_string_lossy().to_string(),
            relative_path: relative,
            children,
            note_count,
        });
    }

    entries
}

fn count_notes_in_dir(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let path = e.path();
                    path.is_file() && path.extension().and_then(|x| x.to_str()) == Some("md")
                })
                .count()
        })
        .unwrap_or(0)
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

pub fn scan_notes(vault_path: &str, notebook_path: Option<&str>) -> Result<Vec<NoteEntry>, String> {
    let scan_path = notebook_path.unwrap_or(vault_path);
    let root = Path::new(scan_path);
    let vault_root = Path::new(vault_path);

    if !root.exists() {
        return Err("Path does not exist".to_string());
    }

    let md_files: Vec<PathBuf> = if notebook_path.is_some() {
        // Only direct children for a specific notebook
        fs::read_dir(root)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("md"))
            .collect()
    } else {
        // Recursive for "All Notes"
        WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| !is_hidden(e.path()) && !e.path().starts_with(&helixnotes_dir(vault_path)))
            .map(|e| e.path().to_path_buf())
            .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("md"))
            .collect()
    };

    // Process files in parallel with partial reads
    let mut notes: Vec<NoteEntry> = md_files
        .par_iter()
        .filter_map(|path| read_note_entry_fast(path, vault_root).ok())
        .collect();

    notes.sort_by(|a, b| b.meta.modified.cmp(&a.meta.modified));
    Ok(notes)
}

fn read_note_entry(path: &Path, vault_root: &Path) -> Result<NoteEntry, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    read_note_entry_from_str(&raw, path, vault_root)
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

    // Use filesystem times if frontmatter didn't have them
    if let Ok(fs_meta) = fs::metadata(path) {
        if let Ok(modified) = fs_meta.modified() {
            meta.modified = modified.into();
        }
        if let Ok(created) = fs_meta.created() {
            meta.created = created.into();
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

pub fn read_note(path: &str) -> Result<NoteContent, String> {
    let p = Path::new(path);
    let raw = fs::read_to_string(p).map_err(|e| e.to_string())?;
    let filename = p
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let (meta, content) = frontmatter::parse_note(&raw, &filename);

    Ok(NoteContent {
        path: path.to_string(),
        meta,
        content,
        raw,
    })
}

pub fn save_note(path: &str, meta: &NoteMeta, body: &str) -> Result<(), String> {
    let mut updated_meta = meta.clone();
    updated_meta.modified = Utc::now();

    let raw = frontmatter::update_note_raw(&updated_meta, body);
    fs::write(path, raw).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn create_note(
    vault_path: &str,
    notebook_relative: Option<&str>,
    title: &str,
) -> Result<NoteEntry, String> {
    let dir = match notebook_relative {
        Some(rel) => Path::new(vault_path).join(rel),
        None => PathBuf::from(vault_path),
    };

    if !dir.exists() {
        return Err("Notebook directory does not exist".to_string());
    }

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

pub fn create_notebook(
    vault_path: &str,
    parent_relative: Option<&str>,
    name: &str,
) -> Result<NotebookEntry, String> {
    let parent = match parent_relative {
        Some(rel) => Path::new(vault_path).join(rel),
        None => PathBuf::from(vault_path),
    };

    let dir_path = parent.join(name);
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

pub fn delete_note(vault_path: &str, note_path: &str) -> Result<(), String> {
    let src = Path::new(note_path);
    if !src.exists() {
        return Err("Note does not exist".to_string());
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
    let dest = trash_dir.join(&trash_name);

    fs::rename(src, dest).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn delete_notebook(vault_path: &str, notebook_path: &str) -> Result<(), String> {
    let src = Path::new(notebook_path);
    if !src.exists() {
        return Err("Notebook does not exist".to_string());
    }

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

    fs::rename(src, dest).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn rename_note(path: &str, new_title: &str) -> Result<String, String> {
    let src = Path::new(path);
    if !src.exists() {
        return Err("Note does not exist".to_string());
    }

    // Update frontmatter
    let raw = fs::read_to_string(src).map_err(|e| e.to_string())?;
    let filename = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let (mut meta, content) = frontmatter::parse_note(&raw, &filename);
    meta.title = new_title.to_string();
    meta.modified = Utc::now();
    let updated = frontmatter::update_note_raw(&meta, &content);

    // Rename file
    let new_filename = sanitize_filename(new_title);
    let new_path = src.parent().unwrap().join(format!("{}.md", new_filename));
    fs::write(src, &updated).map_err(|e| e.to_string())?;

    if new_path != src {
        fs::rename(src, &new_path).map_err(|e| e.to_string())?;
    }

    Ok(new_path.to_string_lossy().to_string())
}

pub fn rename_notebook(path: &str, new_name: &str) -> Result<String, String> {
    let src = Path::new(path);
    if !src.exists() {
        return Err("Notebook does not exist".to_string());
    }

    let new_path = src.parent().unwrap().join(new_name);
    if new_path.exists() {
        return Err("A notebook with that name already exists".to_string());
    }

    fs::rename(src, &new_path).map_err(|e| e.to_string())?;
    Ok(new_path.to_string_lossy().to_string())
}

pub fn move_note(note_path: &str, dest_notebook: &str) -> Result<String, String> {
    let src = Path::new(note_path);
    if !src.exists() {
        return Err("Note does not exist".to_string());
    }

    let dest_dir = Path::new(dest_notebook);
    if !dest_dir.is_dir() {
        return Err("Destination notebook does not exist".to_string());
    }

    let filename = src.file_name().unwrap_or_default();
    let dest = dest_dir.join(filename);

    fs::rename(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

pub fn get_trash_notes(vault_path: &str) -> Result<Vec<NoteEntry>, String> {
    let trash_dir = helixnotes_dir(vault_path).join("trash");
    if !trash_dir.exists() {
        return Ok(Vec::new());
    }

    let vault_root = Path::new(vault_path);
    let mut notes = Vec::new();

    for entry in fs::read_dir(&trash_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|x| x.to_str()) == Some("md") {
            if let Ok(note) = read_note_entry(&path, vault_root) {
                notes.push(note);
            }
        }
    }

    notes.sort_by(|a, b| b.meta.modified.cmp(&a.meta.modified));
    Ok(notes)
}

pub fn restore_note(
    vault_path: &str,
    trash_path: &str,
    dest_notebook: Option<&str>,
) -> Result<String, String> {
    let src = Path::new(trash_path);
    if !src.exists() {
        return Err("Trashed note does not exist".to_string());
    }

    let dest_dir = match dest_notebook {
        Some(nb) => PathBuf::from(nb),
        None => PathBuf::from(vault_path),
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

    let dest = dest_dir.join(original_name);
    fs::rename(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

pub fn permanent_delete(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if p.is_dir() {
        fs::remove_dir_all(p).map_err(|e| e.to_string())?;
    } else {
        fs::remove_file(p).map_err(|e| e.to_string())?;
    }
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

pub fn load_notebook_icons(
    vault_path: &str,
) -> Result<std::collections::HashMap<String, String>, String> {
    let icons_path = helixnotes_dir(vault_path).join("notebook_icons.json");
    if icons_path.exists() {
        let data = fs::read_to_string(&icons_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&data).map_err(|e| e.to_string())
    } else {
        Ok(std::collections::HashMap::new())
    }
}

pub fn set_notebook_icon(
    vault_path: &str,
    notebook_relative: &str,
    icon_relative: Option<&str>,
) -> Result<(), String> {
    let mut icons = load_notebook_icons(vault_path)?;
    match icon_relative {
        Some(icon) => {
            icons.insert(notebook_relative.to_string(), icon.to_string());
        }
        None => {
            icons.remove(notebook_relative);
        }
    }
    let icons_path = helixnotes_dir(vault_path).join("notebook_icons.json");
    let data = serde_json::to_string_pretty(&icons).map_err(|e| e.to_string())?;
    fs::write(&icons_path, data).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_quick_access(vault_path: &str) -> Result<Vec<String>, String> {
    let qa_path = helixnotes_dir(vault_path).join("quick_access.json");
    if qa_path.exists() {
        let data = fs::read_to_string(&qa_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&data).map_err(|e| e.to_string())
    } else {
        Ok(Vec::new())
    }
}

pub fn save_quick_access(vault_path: &str, paths: &[String]) -> Result<(), String> {
    let qa_path = helixnotes_dir(vault_path).join("quick_access.json");
    let data = serde_json::to_string_pretty(paths).map_err(|e| e.to_string())?;
    fs::write(&qa_path, data).map_err(|e| e.to_string())?;
    Ok(())
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

pub fn get_quick_access_notes(vault_path: &str) -> Result<Vec<NoteEntry>, String> {
    let list = load_quick_access(vault_path)?;
    let vault_root = Path::new(vault_path);
    let mut notes = Vec::new();

    for relative in &list {
        let full_path = vault_root.join(relative);
        if full_path.exists() {
            if let Ok(entry) = read_note_entry(&full_path, vault_root) {
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
