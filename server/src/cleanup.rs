//! Retention: periodic removal of expired jobs and old reports.

use crate::clock::Clock;
use crate::jobs::JobStore;
use crate::model::JobState;
use crate::reports::Reports;
use chrono::{DateTime, Utc};
use std::io;
use std::path::Path;
use std::sync::Arc;
use tokio::task::JoinHandle;

const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

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
    for job in store.snapshot() {
        match job.state {
            JobState::WaitingUpload => {
                store.remove_stale_upload(&job.id, now);
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
    store.prune_history(now);
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
        tracing::warn!(error = ?e.kind(), "could not remove an orphaned entry");
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
                tracing::error!("retention sweep panicked");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::stats::{Outcome, Stats};

    #[test]
    fn startup_and_sweeps_leave_statistics_alone() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let stats = Stats::open(data.join("stats"), now);
        stats.record_outcome(Outcome::Succeeded, "203.0.113.7".parse().unwrap());
        stats.record_contribution();
        let stats_file = data.join("stats").join("stats.json");
        let key_file = data.join("stats").join("key");
        let (before_stats, before_key) = (
            std::fs::read(&stats_file).unwrap(),
            std::fs::read(&key_file).unwrap(),
        );

        // A report over the size cap, so `Reports::enforce` has something to remove.
        let report = data.join("reports").join("R-OLD");
        std::fs::create_dir_all(&report).unwrap();
        std::fs::write(report.join("input.rvt"), vec![0u8; 64]).unwrap();
        let reports = Reports::with_limits(data.join("reports"), 1, chrono::Duration::days(30));

        wipe_jobs_dir(data).unwrap();
        let store = JobStore::new(data.to_path_buf(), Arc::new(ManualClock::new(now)));
        let later = now + chrono::Duration::days(60);
        sweep(&store, &reports, now);
        sweep(&store, &reports, later);
        wipe_jobs_dir(data).unwrap();

        assert!(!report.exists());
        assert_eq!(std::fs::read(&stats_file).unwrap(), before_stats);
        assert_eq!(std::fs::read(&key_file).unwrap(), before_key);
    }
}
