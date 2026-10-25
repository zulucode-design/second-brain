# deps-zip9 fixture

`old-backup.zip` is a backup written by zip 2.6.1, the version before the move to zip 9 (#237).
`a_backup_written_with_zip_2_restores_in_this_build` in `backup::tests` restores it and compares the
vault to `OLD_BACKUP_VAULT`, the byte strings it was built from.

It was written once, on Linux, by a scratch program outside the repository: `zip = "=2.6.1"` with
`default-features = false, features = ["deflate"]`, `walkdir = "2"`, and Rust 1.90. The program
writes `OLD_BACKUP_VAULT` to a temporary vault, then runs the loop of `create_backup_of_kind` at
`17d98d0`, with two changes: the walk is sorted, so the output is the same on every run, and the
attachment `.helixnotes/attachments/diagram.png` uses `CompressionMethod::Stored` (with no
compression level), so the restore reads both methods. Every other file is Deflated at level 6.
Without the `time` feature every entry is dated 1980-01-01.

```rust
let options = zip::write::SimpleFileOptions::default()
    .compression_method(zip::CompressionMethod::Deflated)
    .compression_level(Some(6));
let walker = walkdir::WalkDir::new(vault)
    .sort_by_file_name()
    .into_iter()
    .filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        entry.depth() != 1 || name == ".helixnotes" || !name.starts_with('.')
    });
for entry in walker {
    let entry = entry.unwrap();
    let path = entry.path();
    let name = path.strip_prefix(vault).unwrap().to_string_lossy().into_owned();
    if path.is_dir() {
        if !name.is_empty() {
            zip.add_directory(format!("{name}/"), options).unwrap();
        }
    } else {
        let options = if name == ".helixnotes/attachments/diagram.png" {
            options.compression_method(zip::CompressionMethod::Stored).compression_level(None)
        } else {
            options
        };
        zip.start_file(&name, options).unwrap();
        zip.write_all(&std::fs::read(path).unwrap()).unwrap();
    }
}
zip.finish().unwrap();
```

Entries, in archive order (directories are Stored, as `add_directory` writes them):

| Entry | Method |
|---|---|
| `.helixnotes/` | directory |
| `.helixnotes/attachments/` | directory |
| `.helixnotes/attachments/diagram.png` | Stored |
| `.helixnotes/history/` | directory |
| `.helixnotes/history/00000000-0000-4000-8000-000000000001/` | directory |
| `.helixnotes/history/00000000-0000-4000-8000-000000000001/2026-10-01T09-30-00.md` | Deflated |
| `.helixnotes/vault_id` | Deflated |
| `Archives/` | directory |
| `Archives/empty.md` | Deflated, empty |
| `Projects/` | directory |
| `Projects/Café ☕/` | directory |
| `Projects/Café ☕/Plan für 2027.md` | Deflated |
| `Resources/` | directory |
| `Resources/日本語/` | directory |
| `Resources/日本語/メモ.md` | Deflated |

SHA-256: `bc91e541d39626f2ae8fe92e79fbd6b1718e3ade74813e09bb702eedf720874c`, 2112 bytes.
