#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Builds `engine-stub` once and returns the path to the binary.
pub fn stub_path() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "-p", "engine-stub"])
            .current_dir(workspace)
            .status()
            .expect("run cargo build");
        assert!(status.success(), "building engine-stub failed");
        let exe = if cfg!(windows) { "engine-stub.exe" } else { "engine-stub" };
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.join("target"));
        let p = target.join("debug").join(exe);
        assert!(p.exists(), "missing {}", p.display());
        p
    })
    .clone()
}

/// Writes `<dir>/in.<ext>` whose bytes select the stub's behaviour: ASCII `stub-mode=<mode>`
/// and, when given, the UTF-16LE text `Format: <year>`.
pub fn write_marker(dir: &Path, ext: &str, mode: &str, year: Option<u32>) -> PathBuf {
    let mut bytes = format!("stub-mode={mode}\n").into_bytes();
    if let Some(y) = year {
        bytes.extend(format!("Format: {y}").encode_utf16().flat_map(|u| u.to_le_bytes()));
    }
    let p = dir.join(format!("in.{ext}"));
    std::fs::write(&p, bytes).unwrap();
    p
}

/// Waits (up to 5 s) for the stub's pid file and returns the pid.
pub async fn wait_for_pid(path: &Path) -> u32 {
    for _ in 0..100 {
        if let Ok(s) = std::fs::read_to_string(path)
            && let Ok(pid) = s.trim().parse()
        {
            return pid;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("pid file {} never appeared", path.display());
}

/// True when a process with this id is still running.
pub fn process_alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .expect("run tasklist");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let want = pid.to_string();
        text.split_whitespace().any(|t| t == want)
    } else {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
