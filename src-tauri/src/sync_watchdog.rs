//! Parent-death cleanup for the bundled Syncthing process.
//!
//! The sidecar lifecycle owns when Syncthing runs. This narrow helper owns only the process
//! boundary needed when the operating system terminates the app without its normal shutdown.

use std::time::Duration;

const WATCHDOG_FLAG: &str = "--helix-sync-watchdog";
const WATCHDOG_API_KEY: &str = "HELIX_SYNC_WATCHDOG_API_KEY";

/// Run the hidden watchdog mode before initializing Tauri.
pub(crate) fn run_if_requested() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(WATCHDOG_FLAG) {
        return false;
    }
    let parsed = args
        .get(2)
        .and_then(|value| value.parse::<u32>().ok())
        .zip(args.get(3).and_then(|value| value.parse::<u32>().ok()))
        .zip(args.get(4).and_then(|value| value.parse::<u16>().ok()));
    let api_key = std::env::var(WATCHDOG_API_KEY).ok();
    let Some(((parent_pid, sidecar_pid), gui_port)) = parsed else {
        return true;
    };
    let Some(api_key) = api_key.filter(|value| !value.is_empty()) else {
        return true;
    };

    while process_is_alive(parent_pid) && process_is_alive(sidecar_pid) {
        std::thread::sleep(Duration::from_millis(100));
    }
    if process_is_alive(parent_pid) {
        return true;
    }

    let endpoint = format!("http://127.0.0.1:{gui_port}/rest/system/shutdown");
    if let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(250))
        .build()
    {
        for _ in 0..40 {
            if client
                .post(&endpoint)
                .header("X-API-Key", &api_key)
                .send()
                .map(|response| response.status().is_success())
                .unwrap_or(false)
            {
                return true;
            }
            if !process_is_alive(sidecar_pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    terminate_process(sidecar_pid);
    true
}

pub(crate) fn spawn(sidecar_pid: u32, gui_port: u16, api_key: &str) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the sidecar watchdog: {error}"))?;
    std::process::Command::new(executable)
        .args([
            WATCHDOG_FLAG,
            &std::process::id().to_string(),
            &sidecar_pid.to_string(),
            &gui_port.to_string(),
        ])
        .env(WATCHDOG_API_KEY, api_key)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not start the sidecar watchdog: {error}"))
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: signal zero performs existence/permission checking and changes no process.
    unsafe { kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
fn terminate_process(pid: u32) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: the PID came directly from the child handle owned by this app instance.
    let _ = unsafe { kill(pid as i32, 15) };
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    type Handle = *mut std::ffi::c_void;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    // SAFETY: the handle is checked for null, used only for a zero-time wait, then closed.
    unsafe {
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        let _ = CloseHandle(handle);
        alive
    }
}

#[cfg(windows)]
fn terminate_process(pid: u32) {
    type Handle = *mut std::ffi::c_void;
    const PROCESS_TERMINATE: u32 = 0x0001;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    // SAFETY: the handle is checked for null, targets the recorded child PID, then is closed.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            let _ = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn process_is_alive(_pid: u32) -> bool {
    false
}

#[cfg(not(any(unix, windows)))]
fn terminate_process(_pid: u32) {}
