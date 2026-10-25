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

/// One retention pass. Queued and running jobs are never touched.
pub fn sweep(store: &JobStore, reports: &Reports, now: DateTime<Utc>) {
    for job in store.snapshot() {
        match job.state {
            JobState::WaitingUpload
                if job.created_at + Duration::hours(WAITING_UPLOAD_HOURS) < now =>
            {
                // Cancel first so an upload still in flight stops writing.
                store.cancel(&job.id);
                store.remove(&job.id);
            }
            JobState::Done | JobState::Failed if job.expires_at.is_some_and(|t| t < now) => {
                store.expire(&job.id, now);
            }
            JobState::Expired if job.expires_at.is_some_and(|t| t < now) => store.remove(&job.id),
            JobState::Cancelled => store.remove(&job.id),
            _ => {}
        }
    }
    reports.enforce(now);
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
