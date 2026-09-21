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

fn git(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn main() {
    let commit = git(&["rev-parse", "HEAD"]);
    println!(
        "cargo::rustc-env=SECOND_BRAIN_BUILD_COMMIT={}",
        commit.as_deref().unwrap_or("unknown")
    );
    // Cargo reruns a build script only for the inputs it declares, so without these an
    // incremental build keeps whichever commit the script last saw (#138). HEAD changes on
    // checkout; its reflog changes on every commit, reset, and checkout made in this checkout or
    // worktree. `--git-path` resolves both for linked worktrees, and before the first commit.
    let mut unresolved = false;
    for git_path in ["HEAD", "logs/HEAD"] {
        match git(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            git_path,
        ]) {
            Some(path) => println!("cargo::rerun-if-changed={path}"),
            None => unresolved = true,
        }
    }
    // Outside a git checkout both lookups fail and there is no commit to go stale.
    if unresolved && commit.is_some() {
        println!(concat!(
            "cargo::warning=could not resolve git HEAD paths; SECOND_BRAIN_BUILD_COMMIT ",
            "may go stale on incremental builds (needs git 2.31+)"
        ));
    }

    tauri_build::build();

    println!("cargo::rerun-if-env-changed=HELIX_WINDOWS_TEST_MANIFEST");

    #[cfg(target_os = "windows")]
    if std::env::var_os("HELIX_WINDOWS_TEST_MANIFEST").as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        link_windows_common_controls();
    }
}
