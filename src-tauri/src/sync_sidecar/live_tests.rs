//! Verification against two real Syncthing processes.
//!
//! The unit tests in the parent module prove the batch logic against scripted observations. They
//! cannot prove what the incoming-only backup rests on (#14): that a `sendonly` folder receives
//! the peer's index and reports what it would need while writing nothing, and that switching it to
//! `sendreceive` then applies those changes. This drives `sync_batch`, the function a sync run
//! calls, on two paired sidecars on localhost.
//!
//! Ignored by default, because it needs the bundled Syncthing binary and takes several minutes:
//! each paired batch holds its connection for the peer handoff grace. Prepare the binary with
//! `node scripts/prepare-syncthing.mjs`, or point `SYNCTHING_BIN` at one, then run
//!
//! ```text
//! pnpm test:rust sync_sidecar::live_tests -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::{sync_batch, ControlState};

const FOLDER: &str = "live-test-vault";
const API_KEY: &str = "second-brain-live-test";

struct Node {
    name: &'static str,
    vault: PathBuf,
    control: ControlState,
    listen_port: u16,
    device_id: String,
    child: Child,
}

impl Drop for Node {
    fn drop(&mut self) {
        // `serve` runs Syncthing under a monitor process. Killing the monitor alone leaves the
        // real process running, holding its ports, so it is shut down through its API first.
        let _ = client()
            .post(format!(
                "http://127.0.0.1:{}/rest/system/shutdown",
                self.control.gui_port
            ))
            .header("X-API-Key", API_KEY)
            .send();
        for _ in 0..100 {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn syncthing() -> PathBuf {
    if let Ok(path) = std::env::var("SYNCTHING_BIN") {
        return PathBuf::from(path);
    }
    let triple = match std::env::consts::OS {
        "windows" => format!("{}-pc-windows-msvc.exe", std::env::consts::ARCH),
        _ => format!("{}-unknown-linux-gnu", std::env::consts::ARCH),
    };
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("binaries/syncthing-{triple}"))
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

fn rest(
    node: &Node,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut request = client()
        .request(
            method,
            format!("http://127.0.0.1:{}{path}", node.control.gui_port),
        )
        .header("X-API-Key", API_KEY);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().unwrap().error_for_status().unwrap();
    let text = response.text().unwrap();
    if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap()
    }
}

fn patch(node: &Node, path: &str, body: serde_json::Value) {
    rest(node, reqwest::Method::PATCH, path, Some(body));
}

fn folder(node: &Node) -> serde_json::Value {
    rest(
        node,
        reqwest::Method::GET,
        &format!("/rest/config/folders/{FOLDER}"),
        None,
    )
}

fn start(root: &Path, name: &'static str, gui_port: u16, listen_port: u16) -> Node {
    let home = root.join(name).join("home");
    let vault = root.join(name).join("vault");
    fs::create_dir_all(&vault).unwrap();
    let status = Command::new(syncthing())
        .args(["generate", "--no-port-probing"])
        .arg(format!("--home={}", home.display()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("the Syncthing binary runs; see the module docs");
    assert!(status.success());
    let device_id = String::from_utf8(
        Command::new(syncthing())
            .arg("device-id")
            .arg(format!("--home={}", home.display()))
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let child = Command::new(syncthing())
        .args([
            "serve",
            "--no-browser",
            "--no-restart",
            "--no-upgrade",
            "--paused",
        ])
        .arg(format!("--home={}", home.display()))
        .arg(format!("--gui-address=127.0.0.1:{gui_port}"))
        .arg(format!("--gui-apikey={API_KEY}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let control: ControlState = serde_json::from_value(serde_json::json!({
        "version": 1, "enabled": true, "apiKey": API_KEY, "guiPort": gui_port
    }))
    .unwrap();
    let node = Node {
        name,
        vault,
        control,
        listen_port,
        device_id,
        child,
    };
    for _ in 0..100 {
        let ping = client()
            .get(format!("http://127.0.0.1:{gui_port}/rest/system/ping"))
            .header("X-API-Key", API_KEY)
            .send();
        if ping.is_ok_and(|response| response.status().is_success()) {
            return node;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("{name} did not start");
}

/// The app's pairing shape (`apply_pairing`) on loopback: folder and device paused, folder
/// send-only. Reconnection is fast so batches connect promptly.
fn pair(node: &Node, peer: &Node) {
    let mut config = rest(node, reqwest::Method::GET, "/rest/config", None);
    config["devices"] = serde_json::json!([{
        "deviceID": peer.device_id, "name": peer.name, "paused": true,
        "addresses": [format!("tcp://127.0.0.1:{}", peer.listen_port)],
        "autoAcceptFolders": false
    }]);
    config["folders"] = serde_json::json!([{
        "id": FOLDER, "label": FOLDER, "path": node.vault.to_string_lossy(),
        "type": "sendonly", "paused": true, "fsWatcherEnabled": true,
        "devices": [{"deviceID": peer.device_id}]
    }]);
    let options = &mut config["options"];
    for key in [
        "globalAnnounceEnabled",
        "localAnnounceEnabled",
        "relaysEnabled",
        "natEnabled",
        "crashReportingEnabled",
    ] {
        options[key] = false.into();
    }
    options["urAccepted"] = (-1).into();
    options["reconnectionIntervalS"] = 1.into();
    options["listenAddresses"] =
        serde_json::json!([format!("tcp://127.0.0.1:{}", node.listen_port)]);
    rest(node, reqwest::Method::PUT, "/rest/config", Some(config));
}

/// Every file under the vault except Syncthing's own marker, relative path to bytes.
fn tree(vault: &Path) -> BTreeMap<String, Vec<u8>> {
    walkdir::WalkDir::new(vault)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let relative = entry.path().strip_prefix(vault).unwrap();
            (
                relative.to_string_lossy().replace('\\', "/"),
                fs::read(entry.path()).unwrap(),
            )
        })
        .filter(|(path, _)| !path.starts_with(".st"))
        .collect()
}

/// What one side's batch did: its result, and the vault as its backup saw it on completion, once
/// per backup.
struct Run {
    result: Result<(), String>,
    backups: Vec<BTreeMap<String, Vec<u8>>>,
}

fn batch(node: &Node, peer: &Node, fail_backup: bool) -> Run {
    let client = client();
    let mut backups = Vec::new();
    let result = sync_batch(&client, &node.control, FOLDER, &peer.device_id, || {
        // A real backup takes time. Any transfer that could land before it finishes has time to
        // land here, so the recorded vault proves nothing arrived ahead of the backup.
        std::thread::sleep(Duration::from_secs(3));
        backups.push(tree(&node.vault));
        if fail_backup {
            Err("Sync aborted because its safety backup failed: disk full".to_string())
        } else {
            Ok(())
        }
    });
    Run { result, backups }
}

fn both(a: &Node, b: &Node) -> (Run, Run) {
    std::thread::scope(|scope| {
        let first = scope.spawn(|| batch(a, b, false));
        let second = scope.spawn(|| batch(b, a, false));
        (first.join().unwrap(), second.join().unwrap())
    })
}

fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(path, body)| (path.to_string(), body.as_bytes().to_vec()))
        .collect()
}

#[test]
#[ignore = "needs the bundled Syncthing binary and takes minutes; see the module docs"]
fn only_a_batch_with_incoming_changes_takes_a_backup_and_it_comes_first() {
    let root =
        std::env::temp_dir().join(format!("second-brain-sync-live-{}", uuid::Uuid::new_v4()));
    let a = start(&root, "a", 28511, 22511);
    let b = start(&root, "b", 28512, 22512);
    pair(&a, &b);
    pair(&b, &a);
    fs::write(a.vault.join("note.md"), "from a").unwrap();

    // 1. Only B has something incoming. It backs up the vault as it was, then receives.
    let (run_a, run_b) = both(&a, &b);
    assert_eq!(run_a.result, Ok(()));
    assert_eq!(run_b.result, Ok(()));
    assert!(run_a.backups.is_empty(), "nothing was incoming for A");
    assert_eq!(run_b.backups, vec![BTreeMap::new()]);
    assert_eq!(tree(&b.vault), files(&[("note.md", "from a")]));

    // 2. Nothing incoming on either side: no backup at all.
    let (run_a, run_b) = both(&a, &b);
    assert_eq!((run_a.result, run_b.result), (Ok(()), Ok(())));
    assert!(run_a.backups.is_empty() && run_b.backups.is_empty());

    // 3. A deletes one note and adds another. B's backup still holds the deleted note.
    fs::remove_file(a.vault.join("note.md")).unwrap();
    fs::write(a.vault.join("next.md"), "second").unwrap();
    let (run_a, run_b) = both(&a, &b);
    assert_eq!((run_a.result, run_b.result), (Ok(()), Ok(())));
    assert!(run_a.backups.is_empty());
    assert_eq!(run_b.backups, vec![files(&[("note.md", "from a")])]);
    assert_eq!(tree(&b.vault), files(&[("next.md", "second")]));
    assert_eq!(
        folder(&b)["type"],
        "sendonly",
        "a paused folder is send-only again"
    );

    // 4. B's index arrives late: B's device is reachable but its folder stays paused, so A sees
    // no change at first. When the folder resumes, A still backs up before receiving.
    fs::write(b.vault.join("late.md"), "late").unwrap();
    let before = tree(&a.vault);
    patch(
        &b,
        &format!("/rest/config/devices/{}", a.device_id),
        serde_json::json!({"paused": false}),
    );
    let run_a = std::thread::scope(|scope| {
        let run = scope.spawn(|| batch(&a, &b, false));
        std::thread::sleep(Duration::from_secs(20));
        assert_eq!(
            tree(&a.vault),
            before,
            "nothing arrives while B's index is withheld"
        );
        patch(
            &b,
            &format!("/rest/config/folders/{FOLDER}"),
            serde_json::json!({"paused": false}),
        );
        run.join().unwrap()
    });
    assert_eq!(run_a.result, Ok(()));
    assert_eq!(run_a.backups, vec![before]);
    assert_eq!(
        tree(&a.vault),
        files(&[("late.md", "late"), ("next.md", "second")])
    );

    // 5. A's backup fails: A ends with that error, writes nothing, and pauses the folder. B only
    // serves its index here, because a batch of its own would wait for A to converge.
    fs::write(b.vault.join("from-b.md"), "from b").unwrap();
    let before = tree(&a.vault);
    let run_a = batch(&a, &b, true);
    let error = run_a.result.unwrap_err();
    assert!(error.contains("safety backup failed"), "{error}");
    assert_eq!(run_a.backups.len(), 1);
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(tree(&a.vault), before, "a failed backup lets nothing in");
    assert_eq!(folder(&a)["paused"], true);
    assert_eq!(folder(&a)["type"], "sendonly");

    drop(a);
    drop(b);
    fs::remove_dir_all(root).unwrap();
}
