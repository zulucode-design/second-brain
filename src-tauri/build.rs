/// Give the test binaries the Common Controls v6 manifest that the app binary gets from
/// `tauri_build`, so they can load at all.
///
/// `rfd` (via `tauri-plugin-dialog`) imports `TaskDialogIndirect` from `comctl32.dll`. That
/// function exists only in Common Controls **v6**, which is a side-by-side assembly: without
/// a manifest naming it, the loader binds `C:\Windows\System32\comctl32.dll`, which is
/// v5.82 and exports no `TaskDialog*` at all. The import is resolved at load time, so the
/// process dies with `STATUS_ENTRYPOINT_NOT_FOUND` (`0xc0000139`) before `main` runs and
/// before a single test does — with no output naming the symbol, the DLL, or anything else.
///
/// The app binary never had this problem because `tauri_build` embeds its own manifest.
/// Test binaries get no manifest from anyone, which stopped mattering only by luck: until
/// the toolchain on the Windows machine was updated on 2026-09-04, the binaries it produced
/// carried a default manifest that happened to cover this. The same commits that passed 238
/// tests that morning failed to load that afternoon, with no repository change between them.
///
/// Scoped to test targets with `rustc-link-arg-tests`. It must not be `rustc-link-arg`,
/// which would also apply to the app binary and hand the linker a second manifest alongside
/// `tauri_build`'s.
#[cfg(target_os = "windows")]
fn embed_common_controls_manifest_in_tests() {
    let manifest_path = std::path::PathBuf::from(
        std::env::var_os("OUT_DIR").expect("Cargo did not provide an OUT_DIR"),
    )
    .join("common-controls-v6.manifest");
    std::fs::write(
        &manifest_path,
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
            "<assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\">\n",
            "  <dependency>\n",
            "    <dependentAssembly>\n",
            "      <assemblyIdentity type=\"win32\" ",
            "name=\"Microsoft.Windows.Common-Controls\" version=\"6.0.0.0\" ",
            "processorArchitecture=\"*\" publicKeyToken=\"6595b64144ccf1df\" ",
            "language=\"*\" />\n",
            "    </dependentAssembly>\n",
            "  </dependency>\n",
            "</assembly>\n",
        ),
    )
    .expect("Could not write the Windows Common Controls manifest");
    println!("cargo::rustc-link-arg-tests=/MANIFEST:EMBED");
    println!(
        "cargo::rustc-link-arg-tests=/MANIFESTINPUT:{}",
        manifest_path.display()
    );
}

fn main() {
    tauri_build::build();

    #[cfg(target_os = "windows")]
    embed_common_controls_manifest_in_tests();
}
