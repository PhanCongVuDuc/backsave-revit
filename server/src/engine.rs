//! Adapter around the external conversion engine CLI.

use crate::model::{DropReason, DroppedElement, ErrorKind, FileKind};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const MAX_MESSAGE_CHARS: usize = 300;

#[derive(Debug)]
pub enum EngineError {
    Refused(String),
    Corrupt,
    Failed,
    Crashed,
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
            EngineError::Crashed => Some(ErrorKind::Crashed),
            EngineError::TimedOut => Some(ErrorKind::TimedOut),
            EngineError::Cancelled => None,
        }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Refused(m) => write!(f, "refused: {m}"),
            EngineError::Corrupt => f.write_str("file is corrupt or unreadable"),
            EngineError::Failed => f.write_str("conversion failed"),
            EngineError::Crashed => f.write_str("engine crashed"),
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

    pub fn with_limits(path: impl Into<PathBuf>, probe_limit: Duration, query_limit: Duration) -> Self {
        Self { path: path.into(), probe_limit, query_limit }
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

    pub async fn probe(&self, file: &Path, cancel: CancellationToken) -> Result<Probe, EngineError> {
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
        let log = tokio::fs::read(&req.log).await?;
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
        _ => Err(EngineError::Crashed),
    }
}

fn last_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars().take(MAX_MESSAGE_CHARS).collect()
}

/// Spawns the command with piped output and waits for it. On timeout or cancel the child is
/// killed and reaped before returning.
async fn capture(
    mut cmd: Command,
    timeout: Option<Duration>,
    cancel: Option<CancellationToken>,
) -> Result<Captured, EngineError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let out = tokio::spawn(read_all(child.stdout.take()));
    let err = tokio::spawn(read_all(child.stderr.take()));

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
        Ok(status) => Ok(Captured {
            status,
            stdout: out.await.unwrap_or_default(),
            stderr: err.await.unwrap_or_default(),
        }),
        Err(e) => {
            // `kill` sends the kill signal and waits for the process to be reaped.
            let _ = child.kill().await;
            out.abort();
            err.abort();
            Err(e)
        }
    }
}

async fn read_all<R: tokio::io::AsyncRead + Unpin>(r: Option<R>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut r) = r {
        let _ = r.read_to_end(&mut buf).await;
    }
    buf
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
    fn last_line_truncates_and_skips_blank() {
        assert_eq!(last_line(b"a\nb\n\n"), "b");
        assert_eq!(last_line("x".repeat(500).as_bytes()).len(), 300);
    }
}
