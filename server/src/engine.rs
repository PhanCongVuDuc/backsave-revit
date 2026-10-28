//! Adapter around the external conversion engine CLI.

use crate::model::{DropReason, DroppedElement, ErrorKind, FileKind};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const MAX_MESSAGE_CHARS: usize = 300;
/// At most this much of each of stdout and stderr is kept; the rest is read and discarded.
const MAX_PIPE_BYTES: usize = 64 * 1024;
/// How long to keep reading the pipes after the engine exited (a helper it left behind may
/// hold them open).
const PIPE_GRACE: Duration = Duration::from_secs(5);
/// At most this much of the engine log is read.
const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum EngineError {
    Refused(String),
    Corrupt,
    Failed,
    /// Exit code, or `None` when a signal ended the process.
    Crashed(Option<i32>),
    TimedOut,
    Cancelled,
    Io(std::io::Error),
}

impl EngineError {
    /// The user-facing error kind; `None` when the job was cancelled by the user.
    pub fn kind(&self) -> Option<ErrorKind> {
        match self {
            EngineError::Refused(_) => Some(ErrorKind::Refused),
            EngineError::Corrupt => Some(ErrorKind::Corrupt),
            EngineError::Failed | EngineError::Io(_) => Some(ErrorKind::Failed),
            EngineError::Crashed(_) => Some(ErrorKind::Crashed),
            EngineError::TimedOut => Some(ErrorKind::TimedOut),
            EngineError::Cancelled => None,
        }
    }

    /// The engine's exit code behind this error, when it exited by itself.
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            EngineError::Refused(_) => Some(3),
            EngineError::Corrupt => Some(4),
            EngineError::Failed => Some(5),
            EngineError::Crashed(code) => *code,
            _ => None,
        }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Refused(m) => write!(f, "refused: {m}"),
            EngineError::Corrupt => f.write_str("file is corrupt or unreadable"),
            EngineError::Failed => f.write_str("conversion failed"),
            EngineError::Crashed(_) => f.write_str("engine crashed"),
            EngineError::TimedOut => f.write_str("time limit exceeded"),
            EngineError::Cancelled => f.write_str("cancelled"),
            EngineError::Io(e) => write!(f, "engine i/o error: {e}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        EngineError::Io(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Probe {
    pub kind: FileKind,
    pub year: u16,
    pub targets: Vec<u16>,
}

#[derive(Debug, Clone)]
pub struct DowngradeRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    pub log: PathBuf,
    pub target: u16,
    pub limit: Duration,
}

#[derive(Debug, Clone)]
pub struct Engine {
    path: PathBuf,
    probe_limit: Duration,
    query_limit: Duration,
}

struct Captured {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Engine {
    /// Fixed limits: 60 s for `probe`, 30 s for `capabilities` and `--version`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_limits(path, Duration::from_secs(60), Duration::from_secs(30))
    }

    pub fn with_limits(
        path: impl Into<PathBuf>,
        probe_limit: Duration,
        query_limit: Duration,
    ) -> Self {
        Self {
            path: path.into(),
            probe_limit,
            query_limit,
        }
    }

    /// First line of `--version`, or `"unknown"` on any failure.
    pub async fn version(&self) -> String {
        let mut cmd = Command::new(&self.path);
        cmd.arg("--version");
        match capture(cmd, Some(self.query_limit), None).await {
            Ok(c) if c.status.success() => String::from_utf8_lossy(&c.stdout)
                .lines()
                .next()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| "unknown".into()),
            _ => "unknown".into(),
        }
    }

    /// Supported `(from, to)` year pairs.
    pub async fn capabilities(&self) -> Result<Vec<(u16, u16)>, EngineError> {
        #[derive(Deserialize)]
        struct Caps {
            pairs: Vec<(u16, u16)>,
        }
        let mut cmd = Command::new(&self.path);
        cmd.args(["capabilities", "--json"]);
        let c = capture(cmd, Some(self.query_limit), None).await?;
        check_status(&c)?;
        Ok(parse_json::<Caps>(&c.stdout)?.pairs)
    }

    pub async fn probe(
        &self,
        file: &Path,
        cancel: CancellationToken,
    ) -> Result<Probe, EngineError> {
        let mut cmd = Command::new(&self.path);
        cmd.arg("probe").arg(file).arg("--json");
        let c = capture(cmd, Some(self.probe_limit), Some(cancel)).await?;
        check_status(&c)?;
        parse_json(&c.stdout)
    }

    pub async fn downgrade(
        &self,
        req: DowngradeRequest,
        cancel: CancellationToken,
    ) -> Result<Vec<DroppedElement>, EngineError> {
        let mut cmd = Command::new(&self.path);
        cmd.arg("downgrade")
            .arg(&req.input)
            .arg("--to")
            .arg(req.target.to_string())
            .arg("--out")
            .arg(&req.output)
            .arg("--log")
            .arg(&req.log);
        let c = capture(cmd, Some(req.limit), Some(cancel)).await?;
        check_status(&c)?;
        let (log, truncated) = read_capped(&req.log, MAX_LOG_BYTES).await?;
        if truncated {
            tracing::warn!(
                cap_mib = MAX_LOG_BYTES >> 20,
                "engine log is over the size cap; the lines past it were ignored"
            );
        }
        Ok(parse_dropped(&String::from_utf8_lossy(&log)))
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, EngineError> {
    serde_json::from_slice(bytes)
        .map_err(|e| EngineError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
}

fn check_status(c: &Captured) -> Result<(), EngineError> {
    match c.status.code() {
        Some(0) => Ok(()),
        Some(3) => Err(EngineError::Refused(last_line(&c.stderr))),
        Some(4) => Err(EngineError::Corrupt),
        Some(5) => Err(EngineError::Failed),
        code => Err(EngineError::Crashed(code)),
    }
}

fn last_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    line.chars().take(MAX_MESSAGE_CHARS).collect()
}

/// Spawns the command with piped output and waits for it. On timeout or cancel the child (on
/// Unix its whole process group) is killed and reaped before returning. Each pipe keeps at
/// most `MAX_PIPE_BYTES`, and after the child exited the pipes are read for at most
/// `PIPE_GRACE`.
async fn capture(
    mut cmd: Command,
    timeout: Option<Duration>,
    cancel: Option<CancellationToken>,
) -> Result<Captured, EngineError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Its own process group, so helpers the engine starts can be killed along with it.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let mut out = tokio::spawn(read_bounded(
        child.stdout.take(),
        out_buf.clone(),
        MAX_PIPE_BYTES,
    ));
    let mut err = tokio::spawn(read_bounded(
        child.stderr.take(),
        err_buf.clone(),
        MAX_PIPE_BYTES,
    ));

    let sleep = async {
        match timeout {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending().await,
        }
    };
    let cancelled = async {
        match &cancel {
            Some(t) => t.cancelled().await,
            None => std::future::pending().await,
        }
    };

    let outcome = tokio::select! {
        r = child.wait() => Ok(r?),
        () = sleep => Err(EngineError::TimedOut),
        () = cancelled => Err(EngineError::Cancelled),
    };
    match outcome {
        Ok(status) => {
            let drained = tokio::time::timeout(PIPE_GRACE, async {
                let _ = (&mut out).await;
                let _ = (&mut err).await;
            })
            .await
            .is_ok();
            if !drained {
                // Something the engine started still holds the pipes: stop it and stop waiting.
                kill_group(pid);
                out.abort();
                err.abort();
            }
            Ok(Captured {
                status,
                stdout: take(&out_buf),
                stderr: take(&err_buf),
            })
        }
        Err(e) => {
            kill_group(pid);
            // `kill` sends the kill signal and waits for the process to be reaped.
            let _ = child.kill().await;
            out.abort();
            err.abort();
            Err(e)
        }
    }
}

/// On Unix, kills every process in the engine's process group. Elsewhere nothing to do: the
/// child itself is killed by the caller.
#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid.and_then(|p| libc::pid_t::try_from(p).ok()) {
        // SAFETY: killpg only sends a signal; the group id is the engine's own pid, which stays
        // reserved while any process of the group is alive.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {}

fn take(buf: &Mutex<Vec<u8>>) -> Vec<u8> {
    std::mem::take(&mut *buf.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Reads `r` to the end, keeping the first `max` bytes in `buf` and discarding the rest, so the
/// writer never blocks on a full pipe.
async fn read_bounded<R: AsyncRead + Unpin>(r: Option<R>, buf: Arc<Mutex<Vec<u8>>>, max: usize) {
    let Some(mut r) = r else { return };
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
                let room = max.saturating_sub(b.len());
                b.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
}

/// Reads at most `cap` bytes of a file, cut back to the last whole line when the file is longer.
/// The flag says whether anything was left out.
async fn read_capped(path: &Path, cap: u64) -> std::io::Result<(Vec<u8>, bool)> {
    let file = tokio::fs::File::open(path).await?;
    let mut buf = Vec::new();
    file.take(cap + 1).read_to_end(&mut buf).await?;
    if buf.len() as u64 <= cap {
        return Ok((buf, false));
    }
    buf.truncate(cap as usize);
    let keep = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    buf.truncate(keep);
    Ok((buf, true))
}

/// Makes an engine message safe to show to users: file system paths (and the data directory)
/// are replaced with `[path]`, and the result is at most 300 characters.
pub fn public_message(raw: &str, data_dir: &Path) -> String {
    let mut text = raw.to_string();
    let dir = data_dir.to_string_lossy();
    if !dir.is_empty() {
        text = text.replace(dir.as_ref(), "[path]");
    }
    let words: Vec<&str> = text
        .split_whitespace()
        .map(|w| if looks_like_path(w) { "[path]" } else { w })
        .collect();
    words.join(" ").chars().take(MAX_MESSAGE_CHARS).collect()
}

fn looks_like_path(word: &str) -> bool {
    let w = word.trim_matches(|c: char| "\"'`()[]{}<>,;:".contains(c));
    let b = w.as_bytes();
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    drive
        || (w.starts_with('/') && w.len() > 1)
        || w.starts_with("~/")
        || w.contains('\\')
        || w.matches('/').count() >= 2
}

#[derive(Deserialize)]
struct LogLine {
    #[serde(rename = "type")]
    ty: String,
    id: Option<u64>,
    class: Option<String>,
    reason: Option<DropReason>,
    cascade_from: Option<u64>,
}

/// Keeps only well-formed `element_dropped` lines and only their four public fields.
fn parse_dropped(log: &str) -> Vec<DroppedElement> {
    log.lines()
        .filter_map(|l| serde_json::from_str::<LogLine>(l).ok())
        .filter(|l| l.ty == "element_dropped")
        .filter_map(|l| {
            Some(DroppedElement {
                element_id: l.id?,
                class: l.class?,
                reason: l.reason?,
                cascade_from: l.cascade_from,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dropped_ignores_other_and_malformed_lines() {
        let log = concat!(
            "{\"type\":\"run\"}\n",
            "not json\n",
            "{\"type\":\"element_dropped\",\"id\":5,\"class\":\"Wall\",\"reason\":\"cascade\",\"cascade_from\":2,\"rule\":\"x\"}\n",
            "{\"type\":\"element_dropped\",\"id\":\"bad\"}\n",
            "{\"type\":\"field_dropped\",\"class\":\"Wall\"}\n",
            "{\"type\":\"element_dropped\",\"id\":6,\"class\":\"Door\",\"reason\":\"no_rule\",\"cascade_from\":null}\n",
        );
        let d = parse_dropped(log);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].cascade_from, Some(2));
        assert_eq!(d[1].reason, DropReason::NoRule);
    }

    #[test]
    fn public_message_strips_paths_and_data_dir() {
        let data = Path::new("/srv/revit-downgrade data");
        assert_eq!(
            public_message(
                "Cannot open /srv/revit-downgrade data/jobs/ab/in.rvt: denied",
                data
            ),
            "Cannot open [path] denied"
        );
        assert_eq!(
            public_message(
                r"Missing C:\Users\me\x.rfa (link) and 'D:/a/b'",
                Path::new("x")
            ),
            "Missing [path] (link) and [path]"
        );
        assert_eq!(
            public_message(
                "See ~/cache and /tmp/x but keep 2026/2024 and a / b",
                Path::new("")
            ),
            "See [path] and [path] but keep 2026/2024 and a / b"
        );
        assert_eq!(
            public_message("Worksharing central files are not supported yet", data),
            "Worksharing central files are not supported yet"
        );
        assert_eq!(public_message(&"y".repeat(400), data).chars().count(), 300);
    }

    #[tokio::test]
    async fn read_bounded_keeps_the_head_and_drains_the_rest() {
        let data = vec![7u8; 200 * 1024];
        let buf = Arc::new(Mutex::new(Vec::new()));
        read_bounded(Some(&data[..]), buf.clone(), MAX_PIPE_BYTES).await;
        assert_eq!(take(&buf).len(), 64 * 1024);
        read_bounded(None::<&[u8]>, buf.clone(), 10).await;
        assert!(take(&buf).is_empty());
    }

    #[tokio::test]
    async fn read_capped_cuts_at_last_whole_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("log.jsonl");
        std::fs::write(&p, b"aaaa\nbbbb\ncccc\n").unwrap();
        assert_eq!(
            read_capped(&p, 100).await.unwrap(),
            (b"aaaa\nbbbb\ncccc\n".to_vec(), false)
        );
        assert_eq!(
            read_capped(&p, 15).await.unwrap(),
            (b"aaaa\nbbbb\ncccc\n".to_vec(), false)
        );
        assert_eq!(
            read_capped(&p, 12).await.unwrap(),
            (b"aaaa\nbbbb\n".to_vec(), true)
        );
        assert!(read_capped(&dir.path().join("none"), 5).await.is_err());
    }

    #[test]
    fn exit_codes_of_errors() {
        assert_eq!(EngineError::Refused("x".into()).exit_code(), Some(3));
        assert_eq!(EngineError::Failed.exit_code(), Some(5));
        assert_eq!(EngineError::Crashed(Some(9)).exit_code(), Some(9));
        assert_eq!(EngineError::TimedOut.exit_code(), None);
    }

    #[test]
    fn last_line_truncates_and_skips_blank() {
        assert_eq!(last_line(b"a\nb\n\n"), "b");
        assert_eq!(last_line("x".repeat(500).as_bytes()).len(), 300);
    }
}
