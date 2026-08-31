#[cfg(target_os = "windows")]
fn link_windows_common_controls() {
    let response_path = std::path::PathBuf::from(
        std::env::var_os("OUT_DIR").expect("Cargo did not provide an OUT_DIR"),
    )
    .join("common-controls-v6.rsp");
    std::fs::write(
        &response_path,
        concat!(
            "/MANIFEST:EMBED\n",
            "/MANIFESTDEPENDENCY:\"type='win32' name='Microsoft.Windows.Common-Controls' ",
            "version='6.0.0.0' processorArchitecture='*' ",
            "publicKeyToken='6595b64144ccf1df' language='*'\"\n",
        ),
    )
    .expect("Could not write the Windows linker response file");
    println!("cargo::rustc-link-arg=@{}", response_path.display());
}

fn main() {
    tauri_build::build();

    #[cfg(target_os = "windows")]
    link_windows_common_controls();
}
