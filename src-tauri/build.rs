#[cfg(target_os = "windows")]
fn link_windows_common_controls() {
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
    println!("cargo::rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo::rustc-link-arg=/MANIFESTINPUT:{}",
        manifest_path.display()
    );
}

fn main() {
    tauri_build::build();

    println!("cargo::rerun-if-env-changed=HELIX_WINDOWS_TEST_MANIFEST");

    #[cfg(target_os = "windows")]
    if std::env::var_os("HELIX_WINDOWS_TEST_MANIFEST").as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        link_windows_common_controls();
    }
}
