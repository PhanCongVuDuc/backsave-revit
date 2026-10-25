mod common;

use backsave_server::engine::{DowngradeRequest, Engine, EngineError};
use backsave_server::model::{ErrorKind, FileKind};
use common::{process_alive, stub_path, wait_for_pid, write_marker};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn engine() -> Engine {
    Engine::new(stub_path())
}

fn request(dir: &std::path::Path, input: std::path::PathBuf, limit: Duration) -> DowngradeRequest {
    DowngradeRequest {
        input,
        output: dir.join("out.rvt"),
        log: dir.join("log.jsonl"),
        target: 2024,
        limit,
    }
}

#[tokio::test]
async fn downgrade_ok_returns_72_dropped_without_internal_fields() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rvt", "ok", Some(2026));
    let req = request(dir.path(), input, Duration::from_secs(60));
    let out = req.output.clone();
    let dropped = engine()
        .downgrade(req, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(dropped.len(), 72);
    assert!(out.exists());
    let json = serde_json::to_string(&dropped).unwrap();
    assert!(!json.contains("\"rule\""));
    assert!(!json.contains("method_doc"));
    assert!(!json.contains("\"step\""));
    assert_eq!(dropped[0].element_id, 1000);
    assert_eq!(dropped[0].class, "CustomElement");
    assert_eq!(dropped[71].cascade_from, Some(1000));
}

#[tokio::test]
async fn refused_carries_last_stderr_line() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rvt", "refuse", Some(2026));
    let err = engine()
        .downgrade(
            request(dir.path(), input, Duration::from_secs(60)),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    match &err {
        EngineError::Refused(m) => assert_eq!(m, "Worksharing central files are not supported yet"),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(err.kind(), Some(ErrorKind::Refused));
}

#[tokio::test]
async fn corrupt_and_fail_map_to_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rvt", "corrupt", Some(2026));
    let err = engine()
        .downgrade(
            request(dir.path(), input, Duration::from_secs(60)),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Corrupt));
    assert_eq!(err.kind(), Some(ErrorKind::Corrupt));

    let input = write_marker(dir.path(), "rvt", "fail", Some(2026));
    let err = engine()
        .downgrade(
            request(dir.path(), input, Duration::from_secs(60)),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Failed));
    assert_eq!(err.kind(), Some(ErrorKind::Failed));
}

#[tokio::test]
async fn hang_is_killed_after_limit() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rvt", "hang", Some(2026));
    let started = Instant::now();
    let req = request(dir.path(), input, Duration::from_secs(1));
    let pid_file = dir.path().join("out.rvt.pid");
    let err = engine()
        .downgrade(req, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::TimedOut), "{err:?}");
    let pid = wait_for_pid(&pid_file).await;
    assert!(
        !process_alive(pid),
        "hung engine process {pid} is still running"
    );
    assert_eq!(err.kind(), Some(ErrorKind::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(5));
    // The adapter is reusable afterwards.
    let ok = write_marker(dir.path(), "rvt", "ok", Some(2026));
    engine()
        .downgrade(
            request(dir.path(), ok, Duration::from_secs(60)),
            CancellationToken::new(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn cancel_kills_running_child() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rvt", "hang", Some(2026));
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    let pid_file = dir.path().join("out.rvt.pid");
    let pf = pid_file.clone();
    tokio::spawn(async move {
        wait_for_pid(&pf).await;
        c2.cancel();
    });
    let started = Instant::now();
    let err = engine()
        .downgrade(request(dir.path(), input, Duration::from_secs(60)), cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "{err:?}");
    let pid = wait_for_pid(&pid_file).await;
    assert!(
        !process_alive(pid),
        "cancelled engine process {pid} is still running"
    );
    assert_eq!(err.kind(), None);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn crash_maps_to_crashed() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rvt", "crash", Some(2026));
    let err = engine()
        .downgrade(
            request(dir.path(), input, Duration::from_secs(60)),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Crashed), "{err:?}");
    assert_eq!(err.kind(), Some(ErrorKind::Crashed));
}

#[tokio::test]
async fn probe_parses_json() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rfa", "ok", Some(2026));
    let p = engine()
        .probe(&input, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(p.kind, FileKind::Rfa);
    assert_eq!(p.year, 2026);
    assert_eq!(p.targets, vec![2025, 2024, 2023]);
}

#[tokio::test]
async fn capabilities_parses_pairs() {
    let pairs = engine().capabilities().await.unwrap();
    assert!(pairs.contains(&(2026, 2024)));
    assert!(pairs.contains(&(2027, 2023)));
    assert!(!pairs.contains(&(2024, 2026)));
}

#[tokio::test]
async fn version_reads_first_line() {
    assert!(engine().version().await.starts_with("engine-stub "));
    assert_eq!(
        Engine::new("/nonexistent/engine").version().await,
        "unknown"
    );
}

#[tokio::test]
async fn probe_hang_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rvt", "probe-hang", Some(2026));
    let e = Engine::with_limits(stub_path(), Duration::from_secs(1), Duration::from_secs(30));
    let started = Instant::now();
    let err = e.probe(&input, CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, EngineError::TimedOut), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn probe_cancel_returns_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rvt", "probe-hang", Some(2026));
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        c2.cancel();
    });
    let started = Instant::now();
    let err = engine().probe(&input, cancel).await.unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn missing_log_after_success_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_marker(dir.path(), "rvt", "no-log", Some(2026));
    let err = engine()
        .downgrade(
            request(dir.path(), input, Duration::from_secs(60)),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Io(_)), "{err:?}");
    assert_eq!(err.kind(), Some(ErrorKind::Failed));
}
