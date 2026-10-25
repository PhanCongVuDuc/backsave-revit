//! Retention: periodic removal of expired jobs and old reports.

use crate::clock::Clock;
use crate::jobs::JobStore;
use crate::model::JobState;
use crate::reports::Reports;
use chrono::{DateTime, Duration, Utc};
use std::io;
use std::path::Path;
use std::sync::Arc;
use tokio::task::JoinHandle;

const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
const WAITING_UPLOAD_HOURS: i64 = 1;

/// Empties `<data>/jobs` (creating it when missing). Jobs never survive a restart.
pub fn wipe_jobs_dir(data: &Path) -> io::Result<()> {
    let jobs = data.join("jobs");
    match std::fs::remove_dir_all(&jobs) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(&jobs)
}

/// One retention pass. Queued and running jobs are never touched. Every decision is made by
/// the store under its lock, so a job that changed state since the snapshot is left alone.
pub fn sweep(store: &JobStore, reports: &Reports, now: DateTime<Utc>) {
    let upload_cutoff = now - Duration::hours(WAITING_UPLOAD_HOURS);
    for job in store.snapshot() {
        match job.state {
            JobState::WaitingUpload => {
                store.remove_waiting_upload_older_than(&job.id, upload_cutoff);
            }
            JobState::Done | JobState::Failed if job.expires_at.is_some_and(|t| t < now) => {
                store.expire(&job.id, now);
            }
            JobState::Expired => {
                store.remove_expired_before(&job.id, now);
            }
            JobState::Cancelled => {
                store.remove_unstarted_cancelled(&job.id);
            }
            _ => {}
        }
    }
    remove_orphans(store);
    reports.enforce(now);
}

/// Retries deletions that failed earlier: directories of unknown or expired jobs.
fn remove_orphans(store: &JobStore) {
    let Ok(entries) = std::fs::read_dir(store.jobs_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            remove_path(&entry.path());
            continue;
        };
        if store.is_orphan_dir(&name) {
            remove_path(&entry.path());
        }
    }
}

fn remove_path(path: &Path) {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    if let Err(e) = result
        && e.kind() != io::ErrorKind::NotFound
    {
        eprintln!("could not remove an orphaned entry: {}", e.kind());
    }
}

/// Runs `sweep` every minute on a blocking thread, since it works on the file system.
pub fn spawn_cleanup(
    store: Arc<JobStore>,
    reports: Arc<Reports>,
    clock: Arc<dyn Clock>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let (store, reports, now) = (store.clone(), reports.clone(), clock.now());
            if tokio::task::spawn_blocking(move || sweep(&store, &reports, now))
                .await
                .is_err()
            {
                eprintln!("retention sweep panicked");
            }
        }
    })
}
