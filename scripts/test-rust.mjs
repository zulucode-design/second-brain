import { spawnSync } from "node:child_process";

const windows = process.platform === "win32";
const args = ["test", "--manifest-path", "src-tauri/Cargo.toml"];
const env = { ...process.env };

if (windows) {
  // Tauri embeds this manifest in application binaries, but Cargo's generated
  // library test executable needs it supplied separately on Windows.
  args.push("--lib");
  env.HELIX_WINDOWS_TEST_MANIFEST = "1";
}

const result = spawnSync(windows ? "cargo.exe" : "cargo", args, {
  cwd: process.cwd(),
  env,
  stdio: "inherit",
});

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

if (result.signal) {
  console.error(`cargo test terminated by ${result.signal}`);
  process.exit(1);
}

process.exit(result.status ?? 1);
