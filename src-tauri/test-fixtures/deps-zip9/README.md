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

## old-backslash-names.zip

A backup of a Fedora vault whose file and folder names hold backslashes, as the code
v0.1.0-alpha.1 shipped writes it (#237, attempt 2).
`backslash_names_in_a_zip_2_backup_restore_at_their_native_paths` in `backup::tests` restores it
and compares the vault to `BACKSLASH_VAULT`, the byte strings it was built from.

It was written once, on Linux, by a scratch crate outside the repository. That crate compiles
`src-tauri/src/backup.rs` at `17d98d0`, unchanged, against `zip = "=2.4.2"`, the version
v0.1.0-alpha.1 locks, with `default-features = false, features = ["deflate"]` and Rust 1.90.
Stand-ins replace the crate modules the file uses (`types`, `app_dirs`, `durable`, and
`vault::path` copied as it is). It writes `BACKSLASH_VAULT` to a temporary vault and calls
`create_backup(vault, backups, true)`. The entry-writing code of `create_backup` is the same at
`ca8062a`, the commit that set the version to 0.1.0-alpha.1: names from `to_portable_string`,
`add_directory` and `start_file`, Deflated at level 6. Built against `zip = "=2.6.1"`, the same
crate wrote a byte-identical file, and a second zip 2.4.2 run reproduced it. Entries follow the
walk's directory listing order.

| Entry | Method |
|---|---|
| `Resources/` | directory |
| `Resources/a\b.md` | Deflated |
| `Areas/` | directory |
| `Areas/x\y/` | directory |
| `Areas/x\y/note.md` | Deflated |
| `.helixnotes/` | directory |
| `.helixnotes/attachments/` | directory |
| `.helixnotes/attachments/scan\1.png` | Deflated |
| `.helixnotes/vault_id` | Deflated |
| `Projects/` | directory |
| `Projects/plain.md` | Deflated |

SHA-256: `8efe3e65ca63f1f57d5c313af4184be5551fcdeb1557c1b9f2d502ad77786629`, 1341 bytes.

## old-fedora-literal-names.zip

A backup of a Fedora vault holding two names that are ordinary files only on Fedora, as the code
v0.1.0-alpha.1 shipped writes it (#237, attempt 3): `Resources/a\..\b.md`, one file whose name
holds a backslash, two dots and a backslash, and `Projects/ends in\`, a regular file whose name
ends in a backslash. `fedora_literal_names_in_a_zip_2_backup_follow_each_os` in `backup::tests`
restores it on Fedora and compares the vault to `FEDORA_LITERAL_VAULT`, the byte strings it was
built from. On Windows, where the backslashes separate components, the test requires the restore
to refuse the archive whole and leave the live vault unchanged.

It was written once, on Linux, the same way as `old-backslash-names.zip`: a scratch crate outside
the repository compiles `src-tauri/src/backup.rs` at `17d98d0`, unchanged, against
`zip = "=2.4.2"` (the version v0.1.0-alpha.1 locks), with `default-features = false,
features = ["deflate"]` and Rust 1.90, writes `FEDORA_LITERAL_VAULT` to a temporary vault and
calls `create_backup(vault, backups, true)`. Built against `zip = "=2.6.1"`, the same crate wrote
a byte-identical file, and a second zip 2.4.2 run reproduced it. `Projects/ends in\` is written by
`start_file`, as a regular file entry; the zip 2.4.2 reader of v0.1.0-alpha.1 nevertheless takes
any entry whose name ends in a backslash for a directory, so that release restores this file as an
empty folder. This build restores it as the file it is.

| Entry | Method |
|---|---|
| `Resources/` | directory |
| `Resources/a\..\b.md` | Deflated |
| `Projects/` | directory |
| `Projects/ends in\` | Deflated, regular file |
| `Projects/plain.md` | Deflated |
| `.helixnotes/` | directory |
| `.helixnotes/vault_id` | Deflated |

SHA-256: `a228735752547fcd049d527a63f931fb74614c7d8dd6393bb480ddd4260d3f3c`, 923 bytes.
