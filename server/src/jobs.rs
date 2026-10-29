//! In-memory job store with per-IP limits and a FIFO queue for the workers.

use crate::clock::Clock;
use crate::config::MAX_BYTES;
use crate::model::{ApiError, DroppedElement, ErrorKind, FileKind, JobState};
use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const MAX_JOBS_PER_HOUR: usize = 5;
const MAX_WAITING_UPLOADS: usize = 2;
const MAX_QUEUED: usize = 20;
const DONE_RETENTION_MIN: i64 = 60;
const FAILED_RETENTION_MIN: i64 = 15;
const EXPIRED_RECORD_HOURS: i64 = 24;
/// An unfinished upload is dropped after this long without any accepted data...
const UPLOAD_IDLE_MIN: i64 = 60;
/// ...or this long after the job was created, whichever comes first.
const UPLOAD_MAX_HOURS: i64 = 6;
const HOUR_WINDOW_MIN: i64 = 60;
/// Default for `JobStore::with_disk_budget`: 100 GiB.
const DEFAULT_DISK_BUDGET: u64 = 100 << 30;
const SERVER_FULL_RETRY_S: u64 = 600;

/// The address the per-client limits count by: IPv4 addresses one by one, IPv6 addresses by
/// their /64 network (one household or server usually holds a whole /64).
pub fn rate_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

/// A 128-bit random identifier rendered as 32 lowercase hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId([u8; 16]);

impl JobId {
    pub fn random() -> JobId {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).expect("system random source");
        JobId(b)
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseJobIdError;

impl FromStr for JobId {
    type Err = ParseJobIdError;

    /// Accepts exactly 32 lowercase hex characters.
    fn from_str(s: &str) -> Result<JobId, ParseJobIdError> {
        let bytes = s.as_bytes();
        if bytes.len() != 32 {
            return Err(ParseJobIdError);
        }
        let digit = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let mut out = [0u8; 16];
        for (i, o) in out.iter_mut().enumerate() {
            *o = digit(bytes[2 * i]).ok_or(ParseJobIdError)? << 4
                | digit(bytes[2 * i + 1]).ok_or(ParseJobIdError)?;
        }
        Ok(JobId(out))
    }
}

#[derive(Debug, Clone)]
pub struct JobError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub name: String,
    pub size: u64,
    pub kind: FileKind,
    pub year: u16,
    pub target: u16,
    pub ip: IpAddr,
    pub state: JobState,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error: Option<JobError>,
    pub dropped: Vec<DroppedElement>,
    pub received: u64,
    /// Creation time, then the time of the last accepted upload chunk.
    pub last_activity: DateTime<Utc>,
    /// Size of the converted file once the job is done.
    pub output_size: u64,
    pub cancel: CancellationToken,
    pub report_code: Option<String>,
    /// True while a report for this job is being written.
    pub reporting: bool,
    /// Insertion order, used to break ties in FIFO ordering.
    seq: u64,
}

#[derive(Debug, Clone)]
pub struct NewJob {
    pub name: String,
    pub size: u64,
    pub kind: FileKind,
    pub year: u16,
    pub target: u16,
}

/// What `JobStore::claim_report` found.
#[derive(Debug, Clone)]
pub enum ReportClaim {
    Claimed(Box<Job>),
    NotFound,
    NotFailed,
    Expired,
    AlreadyReported,
}

/// What `JobStore::cancel` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    WasRunning,
    Other,
    NotFound,
}

#[derive(Default)]
struct Inner {
    jobs: HashMap<JobId, Job>,
    /// Creation times per IP, kept apart from the jobs so deleting a job does not reset the limit.
    history: HashMap<IpAddr, VecDeque<DateTime<Utc>>>,
    next_seq: u64,
}

pub struct JobStore {
    data_dir: PathBuf,
    clock: Arc<dyn Clock>,
    inner: Mutex<Inner>,
    notify: Notify,
    disk_budget: u64,
}

/// Disk space a job holds or has reserved: input plus output while it is unfinished, the
/// output once done, the kept input after a failure.
fn disk_use(j: &Job) -> u64 {
    match j.state {
        JobState::WaitingUpload | JobState::Queued | JobState::Running => j.size.saturating_mul(2),
        JobState::Done => j.output_size,
        JobState::Failed if j.report_code.is_none() => j.received,
        _ => 0,
    }
}

fn api(status: StatusCode, code: &'static str, msg: &str) -> ApiError {
    ApiError::new(status, code, msg)
}

impl JobStore {
    pub fn new(data_dir: PathBuf, clock: Arc<dyn Clock>) -> JobStore {
        JobStore {
            data_dir,
            clock,
            inner: Mutex::new(Inner::default()),
            notify: Notify::new(),
            disk_budget: DEFAULT_DISK_BUDGET,
        }
    }

    /// Sets how many bytes all live jobs may use together (see `create`).
    pub fn with_disk_budget(mut self, bytes: u64) -> JobStore {
        self.disk_budget = bytes;
        self
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn job_dir(&self, job: &Job) -> PathBuf {
        self.data_dir.join("jobs").join(job.id.to_string())
    }

    pub fn input_path(&self, job: &Job) -> PathBuf {
        self.job_dir(job).join(format!("in.{}", job.kind.ext()))
    }

    pub fn output_path(&self, job: &Job) -> PathBuf {
        self.job_dir(job).join(format!("out.{}", job.kind.ext()))
    }

    pub fn log_path(&self, job: &Job) -> PathBuf {
        self.job_dir(job).join("log.jsonl")
    }

    pub fn create(&self, req: NewJob, ip: IpAddr, pairs: &[(u16, u16)]) -> Result<Job, ApiError> {
        if req.size > MAX_BYTES {
            return Err(api(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "The file is larger than 1 GiB.",
            ));
        }
        if FileKind::from_name(&req.name) != Some(req.kind) {
            return Err(api(
                StatusCode::UNPROCESSABLE_ENTITY,
                "bad_kind",
                "The file extension does not match the file type.",
            ));
        }
        if !pairs.contains(&(req.year, req.target)) {
            return Err(api(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_pair",
                "This conversion is not supported.",
            ));
        }

        let ip = rate_key(ip);
        let now = self.clock.now();
        let mut g = self.lock();

        // Refused requests leave no history behind, so the map only holds addresses that made jobs.
        let window = Duration::minutes(HOUR_WINDOW_MIN);
        if let Some(hist) = g.history.get_mut(&ip) {
            while hist.front().is_some_and(|t| now - *t >= window) {
                hist.pop_front();
            }
            if hist.len() >= MAX_JOBS_PER_HOUR {
                let oldest = *hist.front().expect("non-empty history");
                let ms = (oldest + window - now).num_milliseconds().max(0) as u64;
                let secs = ms.div_ceil(1000).max(1);
                return Err(api(
                    StatusCode::TOO_MANY_REQUESTS,
                    "hourly_limit",
                    "Hourly limit reached. Try again later.",
                )
                .retry_after(secs));
            }
        }

        let count = |g: &Inner, f: &dyn Fn(&Job) -> bool| {
            g.jobs.values().filter(|j| j.ip == ip && f(j)).count()
        };
        if count(&g, &|j| {
            matches!(j.state, JobState::Queued | JobState::Running)
        }) >= 1
        {
            return Err(api(
                StatusCode::TOO_MANY_REQUESTS,
                "active_job",
                "You already have a conversion in progress.",
            ));
        }
        if count(&g, &|j| j.state == JobState::WaitingUpload) >= MAX_WAITING_UPLOADS {
            return Err(api(
                StatusCode::TOO_MANY_REQUESTS,
                "uploads_in_progress",
                "Too many uploads in progress.",
            ));
        }
        if g.jobs
            .values()
            .filter(|j| j.state == JobState::Queued)
            .count()
            >= MAX_QUEUED
        {
            return Err(api(
                StatusCode::SERVICE_UNAVAILABLE,
                "queue_full",
                "The queue is full. Try again shortly.",
            )
            .retry_after(60));
        }
        let in_use: u64 = g.jobs.values().map(disk_use).sum();
        if in_use.saturating_add(req.size.saturating_mul(2)) > self.disk_budget {
            return Err(api(
                StatusCode::SERVICE_UNAVAILABLE,
                "server_full",
                "The server is busy with other files. Try again later.",
            )
            .retry_after(SERVER_FULL_RETRY_S));
        }

        let id = JobId::random();
        let seq = g.next_seq;
        let job = Job {
            id,
            name: req.name,
            size: req.size,
            kind: req.kind,
            year: req.year,
            target: req.target,
            ip,
            state: JobState::WaitingUpload,
            created_at: now,
            started_at: None,
            finished_at: None,
            expires_at: None,
            error: None,
            dropped: Vec::new(),
            received: 0,
            last_activity: now,
            output_size: 0,
            cancel: CancellationToken::new(),
            report_code: None,
            reporting: false,
            seq,
        };
        if std::fs::create_dir_all(self.job_dir(&job)).is_err() {
            return Err(api(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Could not prepare the job.",
            ));
        }
        g.next_seq += 1;
        g.history.entry(ip).or_default().push_back(now);
        g.jobs.insert(id, job.clone());
        Ok(job)
    }

    pub fn get(&self, id: &JobId) -> Option<Job> {
        self.lock().jobs.get(id).cloned()
    }

    /// For a queued job: queued jobs created before it plus running jobs. Zero for any other state.
    pub fn position(&self, id: &JobId) -> Option<usize> {
        let g = self.lock();
        let job = g.jobs.get(id)?;
        if job.state != JobState::Queued {
            return Some(0);
        }
        let ahead = g
            .jobs
            .values()
            .filter(|j| match j.state {
                JobState::Running => true,
                JobState::Queued => (j.created_at, j.seq) < (job.created_at, job.seq),
                _ => false,
            })
            .count();
        Some(ahead)
    }

    /// Adds to the received byte counter, marks the upload as active and returns the new total.
    pub fn add_received(&self, id: &JobId, n: u64) -> u64 {
        let now = self.clock.now();
        match self.lock().jobs.get_mut(id) {
            Some(j) => {
                j.received += n;
                j.last_activity = now;
                j.received
            }
            None => 0,
        }
    }

    pub fn mark_uploaded(&self, id: &JobId) {
        let queued = {
            let mut g = self.lock();
            match g.jobs.get_mut(id) {
                Some(j) if j.state == JobState::WaitingUpload => {
                    j.state = JobState::Queued;
                    true
                }
                _ => false,
            }
        };
        if queued {
            self.notify.notify_one();
        }
    }

    /// Marks a job whose upload broke as failed; it is kept for 15 minutes.
    pub fn set_failed_upload(&self, id: &JobId, kind: ErrorKind, message: String) {
        let now = self.clock.now();
        if let Some(j) = self.lock().jobs.get_mut(id)
            && j.state == JobState::WaitingUpload
        {
            j.state = JobState::Failed;
            j.error = Some(JobError { kind, message });
            j.finished_at = Some(now);
            j.expires_at = Some(now + Duration::minutes(FAILED_RETENTION_MIN));
        }
    }

    /// Takes the oldest queued job (FIFO) and marks it running.
    pub fn take_next(&self) -> Option<Job> {
        let now = self.clock.now();
        let mut g = self.lock();
        let id = g
            .jobs
            .values()
            .filter(|j| j.state == JobState::Queued)
            .min_by_key(|j| (j.created_at, j.seq))
            .map(|j| j.id)?;
        let j = g.jobs.get_mut(&id).expect("job present");
        j.state = JobState::Running;
        j.started_at = Some(now);
        Some(j.clone())
    }

    /// Waits until a queued job is available and takes it.
    pub async fn next_job(&self) -> Job {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Register interest before checking, so a notification between the check and the wait is not lost.
            notified.as_mut().enable();
            if let Some(j) = self.take_next() {
                return j;
            }
            notified.await;
        }
    }

    /// Moves a running job to done or failed. Returns `true` only when that move happened: a job
    /// that is gone or no longer running (cancelled, say) is left alone and gives `false`.
    pub fn finish(
        &self,
        id: &JobId,
        outcome: Result<Vec<DroppedElement>, (ErrorKind, String)>,
    ) -> bool {
        self.finish_with(id, outcome, 0)
    }

    /// Marks a running job done, recording the size of its converted file. Returns `true` as
    /// `finish` does.
    pub fn finish_done(&self, id: &JobId, dropped: Vec<DroppedElement>, output_size: u64) -> bool {
        self.finish_with(id, Ok(dropped), output_size)
    }

    fn finish_with(
        &self,
        id: &JobId,
        outcome: Result<Vec<DroppedElement>, (ErrorKind, String)>,
        output_size: u64,
    ) -> bool {
        let now = self.clock.now();
        let mut g = self.lock();
        let Some(j) = g.jobs.get_mut(id) else {
            return false;
        };
        if j.state != JobState::Running {
            return false;
        }
        j.finished_at = Some(now);
        match outcome {
            Ok(dropped) => {
                j.state = JobState::Done;
                j.dropped = dropped;
                j.output_size = output_size;
                j.expires_at = Some(now + Duration::minutes(DONE_RETENTION_MIN));
            }
            Err((kind, message)) => {
                j.state = JobState::Failed;
                j.error = Some(JobError { kind, message });
                j.expires_at = Some(now + Duration::minutes(FAILED_RETENTION_MIN));
            }
        }
        true
    }

    /// Cancels a job that has not finished yet and signals its token. The result says, decided
    /// atomically under the lock, whether the job was running at that moment (its worker then
    /// removes it once the engine lets go of the files).
    pub fn cancel(&self, id: &JobId) -> CancelOutcome {
        let now = self.clock.now();
        if let Some(j) = self.lock().jobs.get_mut(id) {
            let outcome = if j.state == JobState::Running {
                CancelOutcome::WasRunning
            } else {
                CancelOutcome::Other
            };
            j.cancel.cancel();
            if matches!(
                j.state,
                JobState::WaitingUpload | JobState::Queued | JobState::Running
            ) {
                j.state = JobState::Cancelled;
                j.finished_at = Some(now);
            }
            outcome
        } else {
            CancelOutcome::NotFound
        }
    }

    /// Directory holding every job's files.
    pub fn jobs_dir(&self) -> PathBuf {
        self.data_dir.join("jobs")
    }

    /// Atomically claims the right to write a report for a failed job. Only one caller at a
    /// time (and only once per job) gets `Claimed`; `release_report` gives the claim back.
    pub fn claim_report(&self, id: &JobId) -> ReportClaim {
        let mut g = self.lock();
        let Some(j) = g.jobs.get_mut(id) else {
            return ReportClaim::NotFound;
        };
        match j.state {
            JobState::Failed => {}
            JobState::Expired => return ReportClaim::Expired,
            _ => return ReportClaim::NotFailed,
        }
        if j.reporting || j.report_code.is_some() {
            return ReportClaim::AlreadyReported;
        }
        j.reporting = true;
        ReportClaim::Claimed(Box::new(j.clone()))
    }

    pub fn release_report(&self, id: &JobId) {
        if let Some(j) = self.lock().jobs.get_mut(id) {
            j.reporting = false;
        }
    }

    /// Removes a job that is still waiting for its upload when no data has arrived for more than
    /// an hour, or when it was created more than 6 hours ago. The state and times are checked
    /// under the lock, so a job that has been queued or has received data in the meantime is
    /// left alone.
    pub fn remove_stale_upload(&self, id: &JobId, now: DateTime<Utc>) -> bool {
        self.remove_if(id, |j| {
            j.state == JobState::WaitingUpload
                && (now - j.last_activity > Duration::minutes(UPLOAD_IDLE_MIN)
                    || now - j.created_at > Duration::hours(UPLOAD_MAX_HOURS))
        })
    }

    /// Drops per-client history older than the hourly window, and clients left with none.
    pub fn prune_history(&self, now: DateTime<Utc>) {
        let window = Duration::minutes(HOUR_WINDOW_MIN);
        self.lock().history.retain(|_, hist| {
            while hist.front().is_some_and(|t| now - *t >= window) {
                hist.pop_front();
            }
            !hist.is_empty()
        });
    }

    /// Number of clients with job history in the last hour.
    pub fn history_len(&self) -> usize {
        self.lock().history.len()
    }

    /// Cancels every job that has not finished, for shutdown. Running jobs are then removed by
    /// their workers once the engine has exited.
    pub fn cancel_all(&self) {
        let ids: Vec<JobId> = self
            .lock()
            .jobs
            .values()
            .filter(|j| {
                matches!(
                    j.state,
                    JobState::WaitingUpload | JobState::Queued | JobState::Running
                )
            })
            .map(|j| j.id)
            .collect();
        for id in ids {
            self.cancel(&id);
        }
    }

    /// True while a cancelled job that was running is still waiting for its worker.
    pub fn has_cancelled_running(&self) -> bool {
        self.lock()
            .jobs
            .values()
            .any(|j| j.state == JobState::Cancelled && j.started_at.is_some())
    }

    /// Removes a cancelled job that never started. A cancelled job that was running belongs to
    /// its worker, which removes it once the engine has exited.
    pub fn remove_unstarted_cancelled(&self, id: &JobId) -> bool {
        self.remove_if(id, |j| {
            j.state == JobState::Cancelled && j.started_at.is_none()
        })
    }

    /// Removes an expired record whose 24 hours are over.
    pub fn remove_expired_before(&self, id: &JobId, now: DateTime<Utc>) -> bool {
        self.remove_if(id, |j| {
            j.state == JobState::Expired && j.expires_at.is_some_and(|t| t < now)
        })
    }

    fn remove_if(&self, id: &JobId, cond: impl FnOnce(&Job) -> bool) -> bool {
        let job = {
            let mut g = self.lock();
            if !g.jobs.get(id).is_some_and(cond) {
                return false;
            }
            g.jobs.remove(id).expect("job present")
        };
        job.cancel.cancel();
        self.delete_dir(&job);
        true
    }

    fn delete_dir(&self, job: &Job) {
        if let Err(e) = std::fs::remove_dir_all(self.job_dir(job))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = ?e.kind(), "could not remove job directory");
        }
    }

    /// True when a directory under `jobs/` belongs to no live job: its name is not a job id,
    /// the store does not know the id, or the job has expired. Decided under the lock, which
    /// `create` also holds while it makes a directory and records the job.
    pub fn is_orphan_dir(&self, name: &str) -> bool {
        let Ok(id) = name.parse::<JobId>() else {
            return true;
        };
        self.lock()
            .jobs
            .get(&id)
            .is_none_or(|j| j.state == JobState::Expired)
    }

    /// A copy of every job, for the retention sweep.
    pub fn snapshot(&self) -> Vec<Job> {
        self.lock().jobs.values().cloned().collect()
    }

    /// Turns a finished (done or failed) job into `Expired`: its files are deleted and the record
    /// stays for 24 hours so that clients get "gone" rather than "not found". Returns false when
    /// the job is missing or in another state.
    pub fn expire(&self, id: &JobId, now: DateTime<Utc>) -> bool {
        let job = {
            let mut g = self.lock();
            match g.jobs.get_mut(id) {
                Some(j) if matches!(j.state, JobState::Done | JobState::Failed) => {
                    j.state = JobState::Expired;
                    j.expires_at = Some(now + Duration::hours(EXPIRED_RECORD_HOURS));
                    j.dropped = Vec::new();
                    j.name = String::new();
                    j.clone()
                }
                _ => return false,
            }
        };
        if let Err(e) = std::fs::remove_dir_all(self.job_dir(&job))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = ?e.kind(), "could not remove job directory");
        }
        true
    }

    /// Records the report code of a job; false when the job is gone or already has one.
    pub fn set_report_code(&self, id: &JobId, code: String) -> bool {
        match self.lock().jobs.get_mut(id) {
            Some(j) if j.report_code.is_none() => {
                j.report_code = Some(code);
                j.reporting = false;
                true
            }
            _ => false,
        }
    }

    /// Forgets the job and deletes its directory.
    pub fn remove(&self, id: &JobId) {
        let job = self.lock().jobs.remove(id);
        if let Some(j) = job {
            j.cancel.cancel();
            if let Err(e) = std::fs::remove_dir_all(self.job_dir(&j))
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(error = ?e.kind(), "could not remove job directory");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use axum::http::StatusCode;
    use std::time::Duration;

    const GIB: u64 = 1 << 30;

    fn pairs() -> Vec<(u16, u16)> {
        vec![(2026, 2025), (2026, 2024), (2025, 2024)]
    }

    fn setup() -> (JobStore, Arc<ManualClock>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
        ));
        let store = JobStore::new(dir.path().to_path_buf(), clock.clone());
        (store, clock, dir)
    }

    fn req() -> NewJob {
        NewJob {
            name: "House.rvt".into(),
            size: 1000,
            kind: FileKind::Rvt,
            year: 2026,
            target: 2024,
        }
    }

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, n])
    }

    fn err_of(r: Result<Job, ApiError>) -> (StatusCode, &'static str, Option<u64>) {
        let e = r.expect_err("expected an error");
        (e.status, e.code, e.retry_after_s)
    }

    #[test]
    fn job_id_is_32_lowercase_hex_and_parses() {
        let a = JobId::random();
        assert_eq!(a.to_string().len(), 32);
        assert_eq!(a.to_string().parse::<JobId>().unwrap(), a);
        assert_ne!(a, JobId::random());
        assert!("ABCDEF0123456789abcdef0123456789".parse::<JobId>().is_err());
        assert!("abc".parse::<JobId>().is_err());
        assert!("../../etc/passwd/../../../xxxxxx".parse::<JobId>().is_err());
        assert!("0123456789abcdef0123456789abcdeg".parse::<JobId>().is_err());
    }

    #[test]
    fn create_makes_dir_and_waits_for_upload() {
        let (s, _c, d) = setup();
        let j = s.create(req(), ip(1), &pairs()).unwrap();
        assert_eq!(j.state, JobState::WaitingUpload);
        assert!(d.path().join("jobs").join(j.id.to_string()).is_dir());
        assert!(s.input_path(&j).ends_with(format!("{}/in.rvt", j.id)));
        assert!(s.output_path(&j).ends_with(format!("{}/out.rvt", j.id)));
        assert!(s.log_path(&j).ends_with(format!("{}/log.jsonl", j.id)));
        s.remove(&j.id);
        assert!(!s.job_dir(&j).exists());
        assert!(s.get(&j.id).is_none());
    }

    #[test]
    fn create_rejects_over_1gib() {
        let (s, _c, _d) = setup();
        let mut r = req();
        r.size = GIB + 1;
        assert_eq!(
            err_of(s.create(r, ip(1), &pairs())),
            (StatusCode::PAYLOAD_TOO_LARGE, "too_large", None)
        );
        let mut r = req();
        r.size = GIB;
        assert!(s.create(r, ip(1), &pairs()).is_ok());
    }

    #[test]
    fn create_rejects_bad_kind() {
        let (s, _c, _d) = setup();
        let mut r = req();
        r.kind = FileKind::Rfa;
        assert_eq!(
            err_of(s.create(r, ip(1), &pairs())),
            (StatusCode::UNPROCESSABLE_ENTITY, "bad_kind", None)
        );
        let mut r = req();
        r.name = "notes.txt".into();
        assert_eq!(err_of(s.create(r, ip(1), &pairs())).1, "bad_kind");
    }

    #[test]
    fn create_rejects_unsupported_pair() {
        let (s, _c, _d) = setup();
        let mut r = req();
        r.target = 2023;
        assert_eq!(
            err_of(s.create(r, ip(1), &pairs())),
            (StatusCode::UNPROCESSABLE_ENTITY, "unsupported_pair", None)
        );
    }

    #[test]
    fn sixth_job_in_hour_is_429_with_retry_after() {
        let (s, c, _d) = setup();
        for _ in 0..5 {
            let j = s.create(req(), ip(1), &pairs()).unwrap();
            s.cancel(&j.id);
            c.advance(Duration::from_secs(60));
        }
        // The oldest entry is 300 s old, so 3300 s remain.
        assert_eq!(
            err_of(s.create(req(), ip(1), &pairs())),
            (StatusCode::TOO_MANY_REQUESTS, "hourly_limit", Some(3300))
        );
    }

    #[test]
    fn cancel_reports_running_atomically() {
        let (s, _c, _d) = setup();
        assert_eq!(s.cancel(&JobId::random()), CancelOutcome::NotFound);
        let waiting = s.create(req(), ip(1), &pairs()).unwrap();
        assert_eq!(s.cancel(&waiting.id), CancelOutcome::Other);
        let queued = s.create(req(), ip(2), &pairs()).unwrap();
        s.mark_uploaded(&queued.id);
        let running = s.create(req(), ip(3), &pairs()).unwrap();
        s.mark_uploaded(&running.id);
        let taken = s.take_next().unwrap();
        assert_eq!(taken.id, queued.id);
        assert_eq!(s.cancel(&queued.id), CancelOutcome::WasRunning);
        assert_eq!(s.get(&queued.id).unwrap().state, JobState::Cancelled);
        // A cancelled queued job is never taken by a worker.
        assert_eq!(s.cancel(&running.id), CancelOutcome::Other);
        assert!(s.take_next().is_none());
        // Cancelling twice reports that it is no longer running.
        assert_eq!(s.cancel(&queued.id), CancelOutcome::Other);
    }

    #[test]
    fn ok_after_cancel_is_ignored_by_finish_so_worker_must_remove() {
        let (s, _c, _d) = setup();
        let j = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&j.id);
        let taken = s.take_next().unwrap();
        assert_eq!(s.cancel(&j.id), CancelOutcome::WasRunning);
        assert!(!s.finish(&j.id, Ok(vec![])));
        let after = s.get(&j.id).unwrap();
        assert_eq!(after.state, JobState::Cancelled);
        assert!(taken.cancel.is_cancelled());
    }

    #[test]
    fn deleting_a_job_does_not_reset_hourly_limit() {
        let (s, _c, _d) = setup();
        for _ in 0..5 {
            let j = s.create(req(), ip(1), &pairs()).unwrap();
            s.remove(&j.id);
        }
        assert_eq!(err_of(s.create(req(), ip(1), &pairs())).1, "hourly_limit");
    }

    #[test]
    fn second_active_job_same_ip_is_429() {
        let (s, _c, _d) = setup();
        let j = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&j.id);
        assert_eq!(
            err_of(s.create(req(), ip(1), &pairs())),
            (StatusCode::TOO_MANY_REQUESTS, "active_job", None)
        );
        let t = s.take_next().unwrap();
        assert_eq!(t.id, j.id);
        assert_eq!(err_of(s.create(req(), ip(1), &pairs())).1, "active_job");
        s.finish(&j.id, Ok(vec![]));
        assert!(s.create(req(), ip(1), &pairs()).is_ok());
    }

    #[test]
    fn third_waiting_upload_is_429() {
        let (s, _c, _d) = setup();
        s.create(req(), ip(1), &pairs()).unwrap();
        s.create(req(), ip(1), &pairs()).unwrap();
        assert_eq!(
            err_of(s.create(req(), ip(1), &pairs())),
            (StatusCode::TOO_MANY_REQUESTS, "uploads_in_progress", None)
        );
    }

    #[test]
    fn queue_full_is_503() {
        let (s, _c, _d) = setup();
        for n in 0..20 {
            let j = s.create(req(), ip(n + 1), &pairs()).unwrap();
            s.mark_uploaded(&j.id);
        }
        assert_eq!(
            err_of(s.create(req(), ip(100), &pairs())),
            (StatusCode::SERVICE_UNAVAILABLE, "queue_full", Some(60))
        );
        s.take_next().unwrap();
        assert!(s.create(req(), ip(100), &pairs()).is_ok());
    }

    #[test]
    fn other_ip_not_limited() {
        let (s, _c, _d) = setup();
        let j = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&j.id);
        assert!(s.create(req(), ip(2), &pairs()).is_ok());
    }

    #[test]
    fn check_order_size_before_kind_before_pair() {
        let (s, _c, _d) = setup();
        let mut r = req();
        r.size = GIB + 1;
        r.kind = FileKind::Rfa;
        r.target = 2023;
        assert_eq!(err_of(s.create(r, ip(1), &pairs())).1, "too_large");
        let mut r = req();
        r.kind = FileKind::Rfa;
        r.target = 2023;
        assert_eq!(err_of(s.create(r, ip(1), &pairs())).1, "bad_kind");
    }

    #[test]
    fn position_counts_queued_before_and_running() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        let b = s.create(req(), ip(2), &pairs()).unwrap();
        let c3 = s.create(req(), ip(3), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        s.mark_uploaded(&b.id);
        s.mark_uploaded(&c3.id);
        assert_eq!(s.position(&a.id), Some(0));
        assert_eq!(s.position(&b.id), Some(1));
        assert_eq!(s.position(&c3.id), Some(2));
        assert_eq!(s.take_next().unwrap().id, a.id);
        assert_eq!(s.position(&b.id), Some(1));
        assert_eq!(s.position(&c3.id), Some(2));
        assert_eq!(s.position(&JobId::random()), None);
    }

    #[test]
    fn limits_reset_after_hour() {
        let (s, c, _d) = setup();
        for _ in 0..5 {
            let j = s.create(req(), ip(1), &pairs()).unwrap();
            s.cancel(&j.id);
        }
        assert_eq!(
            err_of(s.create(req(), ip(1), &pairs())),
            (StatusCode::TOO_MANY_REQUESTS, "hourly_limit", Some(3600))
        );
        c.advance(Duration::from_secs(3600));
        assert!(s.create(req(), ip(1), &pairs()).is_ok());
    }

    #[test]
    fn finish_sets_expiry_and_state() {
        let (s, c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        let t0 = c.now();
        s.take_next().unwrap();
        assert_eq!(s.get(&a.id).unwrap().started_at, Some(t0));
        c.advance(Duration::from_secs(10));
        let d = DroppedElement {
            element_id: 1,
            class: "Wall".into(),
            reason: crate::model::DropReason::NoRule,
            cascade_from: None,
        };
        assert!(s.finish(&a.id, Ok(vec![d.clone()])));
        let j = s.get(&a.id).unwrap();
        assert_eq!(j.state, JobState::Done);
        assert_eq!(j.dropped, vec![d]);
        assert_eq!(j.finished_at, Some(c.now()));
        assert_eq!(j.expires_at, Some(c.now() + chrono::Duration::hours(1)));

        let b = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&b.id);
        s.take_next().unwrap();
        assert!(s.finish(&b.id, Err((ErrorKind::Corrupt, "bad".into()))));
        // A second finish on a job that is no longer running changes nothing.
        assert!(!s.finish(&b.id, Ok(vec![])));
        let j = s.get(&b.id).unwrap();
        assert_eq!(j.state, JobState::Failed);
        assert_eq!(j.error.as_ref().unwrap().kind, ErrorKind::Corrupt);
        assert_eq!(j.expires_at, Some(c.now() + chrono::Duration::minutes(15)));
    }

    #[test]
    fn cancel_cancels_token_and_is_not_overridden_by_finish() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        let running = s.take_next().unwrap();
        s.cancel(&a.id);
        assert!(running.cancel.is_cancelled());
        assert_eq!(s.get(&a.id).unwrap().state, JobState::Cancelled);
        s.finish(&a.id, Ok(vec![]));
        assert_eq!(s.get(&a.id).unwrap().state, JobState::Cancelled);
    }

    #[test]
    fn upload_helpers() {
        let (s, c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        assert_eq!(s.add_received(&a.id, 10), 10);
        assert_eq!(s.add_received(&a.id, 5), 15);
        s.set_failed_upload(&a.id, ErrorKind::Failed, "upload broke".into());
        let j = s.get(&a.id).unwrap();
        assert_eq!(j.state, JobState::Failed);
        assert_eq!(j.expires_at, Some(c.now() + chrono::Duration::minutes(15)));
    }

    #[tokio::test]
    async fn next_job_waits_for_upload() {
        let (s, _c, _d) = setup();
        let s = Arc::new(s);
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        let s2 = s.clone();
        let h = tokio::spawn(async move { s2.next_job().await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!h.is_finished());
        s.mark_uploaded(&a.id);
        let got = tokio::time::timeout(Duration::from_secs(2), h)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.id, a.id);
        assert_eq!(got.state, JobState::Running);
    }

    #[tokio::test]
    async fn next_job_sees_job_queued_before_wait() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        let got = tokio::time::timeout(Duration::from_secs(2), s.next_job())
            .await
            .unwrap();
        assert_eq!(got.id, a.id);
    }

    fn done_job(s: &JobStore) -> Job {
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        std::fs::write(s.input_path(&a), b"x").unwrap();
        s.mark_uploaded(&a.id);
        s.take_next().unwrap();
        s.finish(&a.id, Ok(Vec::new()));
        s.get(&a.id).unwrap()
    }

    #[test]
    fn expire_deletes_files_and_keeps_record_for_a_day() {
        let (s, c, _d) = setup();
        let a = done_job(&s);
        let now = c.now();
        assert!(s.expire(&a.id, now));
        let after = s.get(&a.id).unwrap();
        assert_eq!(after.state, JobState::Expired);
        assert_eq!(after.expires_at, Some(now + chrono::Duration::hours(24)));
        assert!(!s.job_dir(&a).exists());
        assert!(!s.expire(&a.id, now), "only done or failed jobs expire");
    }

    #[test]
    fn expire_ignores_queued_and_unknown_jobs() {
        let (s, c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        assert!(!s.expire(&a.id, c.now()));
        assert_eq!(s.get(&a.id).unwrap().state, JobState::Queued);
        assert!(s.job_dir(&a).exists());
        assert!(!s.expire(&JobId::random(), c.now()));
    }

    #[test]
    fn set_report_code_only_once() {
        let (s, _c, _d) = setup();
        let a = done_job(&s);
        assert!(s.set_report_code(&a.id, "R-AAAAAA".into()));
        assert!(!s.set_report_code(&a.id, "R-BBBBBB".into()));
        assert_eq!(
            s.get(&a.id).unwrap().report_code.as_deref(),
            Some("R-AAAAAA")
        );
        assert!(!s.set_report_code(&JobId::random(), "R-CCCCCC".into()));
    }

    #[test]
    fn snapshot_lists_every_job() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        let b = s.create(req(), ip(2), &pairs()).unwrap();
        let mut ids: Vec<_> = s.snapshot().iter().map(|j| j.id.to_string()).collect();
        ids.sort();
        let mut want = vec![a.id.to_string(), b.id.to_string()];
        want.sort();
        assert_eq!(ids, want);
    }

    #[test]
    fn waiting_removal_rechecks_state_under_the_lock() {
        let (s, c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        let later = c.now() + chrono::Duration::hours(2);
        let stale = s.snapshot();
        assert_eq!(stale[0].state, JobState::WaitingUpload);
        // The upload completes after the snapshot was taken.
        s.mark_uploaded(&a.id);
        assert!(!s.remove_stale_upload(&a.id, later));
        assert_eq!(s.get(&a.id).unwrap().state, JobState::Queued);
        assert!(s.job_dir(&a).exists());
        // A job still waiting and idle long enough goes; one not idle long enough stays.
        let b = s.create(req(), ip(2), &pairs()).unwrap();
        assert!(!s.remove_stale_upload(&b.id, c.now()));
        assert!(s.remove_stale_upload(&b.id, later));
        assert!(s.get(&b.id).is_none());
        assert!(!s.job_dir(&b).exists());
    }

    #[test]
    fn upload_with_recent_data_is_kept_until_the_6_hour_cap() {
        let (s, c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        // A chunk arrives every 50 minutes: never idle for an hour.
        for _ in 0..7 {
            c.advance(Duration::from_secs(50 * 60));
            assert!(!s.remove_stale_upload(&a.id, c.now()), "still active");
            s.add_received(&a.id, 1);
        }
        // 5 h 50 min after creation, 0 min idle: kept. Past 6 h: removed despite recent data.
        assert_eq!(s.get(&a.id).unwrap().last_activity, c.now());
        c.advance(Duration::from_secs(10 * 60));
        assert!(
            !s.remove_stale_upload(&a.id, c.now()),
            "exactly 6 h is not over"
        );
        c.advance(Duration::from_secs(1));
        assert!(s.remove_stale_upload(&a.id, c.now()));
    }

    #[test]
    fn idle_hour_after_last_chunk_removes_upload() {
        let (s, c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        c.advance(Duration::from_secs(1800));
        s.add_received(&a.id, 1);
        c.advance(Duration::from_secs(3600));
        assert!(!s.remove_stale_upload(&a.id, c.now()), "exactly 1 h idle");
        c.advance(Duration::from_secs(1));
        assert!(s.remove_stale_upload(&a.id, c.now()));
    }

    fn sized(size: u64) -> NewJob {
        NewJob { size, ..req() }
    }

    #[test]
    fn disk_budget_counts_reservations_outputs_and_kept_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
        ));
        let s = JobStore::new(dir.path().to_path_buf(), clock).with_disk_budget(1000);
        // Waiting upload: 2 x 200 reserved.
        let a = s.create(sized(200), ip(1), &pairs()).unwrap();
        // Another 2 x 250 reserved: 900 in use.
        let b = s.create(sized(250), ip(2), &pairs()).unwrap();
        let full = s.create(sized(60), ip(3), &pairs()).unwrap_err();
        assert_eq!(
            (full.status, full.code, full.retry_after_s),
            (StatusCode::SERVICE_UNAVAILABLE, "server_full", Some(600))
        );
        assert!(
            s.create(sized(50), ip(3), &pairs()).is_ok(),
            "exactly the budget fits"
        );
        // a finishes with a 30-byte output: 60 + 500 + 100 in use.
        s.mark_uploaded(&a.id);
        s.take_next().unwrap();
        s.finish_done(&a.id, Vec::new(), 30);
        assert!(s.create(sized(170), ip(4), &pairs()).is_ok());
        // b fails after receiving 250 bytes: its input is kept, the output reservation goes.
        s.add_received(&b.id, 250);
        s.set_failed_upload(&b.id, ErrorKind::DiskFull, "x".into());
        // 30 + 250 + 100 + 340 = 720 in use.
        assert_eq!(
            s.create(sized(141), ip(5), &pairs()).unwrap_err().code,
            "server_full"
        );
        assert!(s.create(sized(140), ip(5), &pairs()).is_ok());
    }

    #[test]
    fn ipv6_clients_count_by_slash_64() {
        let (s, _c, _d) = setup();
        let v6 = |last: u16| IpAddr::from([0x2001, 0xdb8, 1, 2, 0, 0, 0, last]);
        s.create(req(), v6(1), &pairs()).unwrap();
        s.create(req(), v6(2), &pairs()).unwrap();
        assert_eq!(
            err_of(s.create(req(), v6(3), &pairs())).1,
            "uploads_in_progress"
        );
        let other_net = IpAddr::from([0x2001, 0xdb8, 1, 3, 0, 0, 0, 1]);
        assert!(s.create(req(), other_net, &pairs()).is_ok());
        assert_eq!(rate_key("::ffff:10.0.0.1".parse().unwrap()), ip(1));
        assert_eq!(rate_key(ip(7)), ip(7));
    }

    #[test]
    fn refused_requests_leave_no_history_and_old_history_is_pruned() {
        let (s, c, _d) = setup();
        let mut bad = req();
        bad.target = 2023;
        assert!(s.create(bad, ip(1), &pairs()).is_err());
        let j = s.create(req(), ip(2), &pairs()).unwrap();
        s.create(req(), ip(2), &pairs()).unwrap();
        // A refused third upload adds nothing to the history either.
        assert!(s.create(req(), ip(2), &pairs()).is_err());
        assert_eq!(s.history_len(), 1);
        s.remove(&j.id);
        c.advance(Duration::from_secs(3599));
        s.prune_history(c.now());
        assert_eq!(s.history_len(), 1);
        c.advance(Duration::from_secs(1));
        s.prune_history(c.now());
        assert_eq!(s.history_len(), 0);
    }

    #[test]
    fn expiry_clears_the_file_name() {
        let (s, c, _d) = setup();
        let a = done_job(&s);
        assert!(s.expire(&a.id, c.now()));
        assert_eq!(s.get(&a.id).unwrap().name, "");
    }

    #[test]
    fn cancel_all_cancels_unfinished_jobs() {
        let (s, _c, _d) = setup();
        let waiting = s.create(req(), ip(1), &pairs()).unwrap();
        let running = s.create(req(), ip(2), &pairs()).unwrap();
        s.mark_uploaded(&running.id);
        let taken = s.take_next().unwrap();
        let done = done_job(&s);
        s.cancel_all();
        assert_eq!(s.get(&waiting.id).unwrap().state, JobState::Cancelled);
        assert!(taken.cancel.is_cancelled());
        assert!(s.has_cancelled_running());
        assert_eq!(s.get(&done.id).unwrap().state, JobState::Done);
        s.remove(&running.id);
        assert!(!s.has_cancelled_running());
    }

    #[test]
    fn only_unstarted_cancelled_jobs_are_removed() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        s.cancel(&a.id);
        assert!(s.remove_unstarted_cancelled(&a.id));
        assert!(s.get(&a.id).is_none());

        let b = s.create(req(), ip(2), &pairs()).unwrap();
        s.mark_uploaded(&b.id);
        s.take_next().unwrap();
        s.cancel(&b.id);
        assert_eq!(s.get(&b.id).unwrap().state, JobState::Cancelled);
        assert!(!s.remove_unstarted_cancelled(&b.id));
        assert!(s.job_dir(&b).exists());
    }

    #[test]
    fn claim_report_is_exclusive() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        assert!(matches!(s.claim_report(&a.id), ReportClaim::NotFailed));
        assert!(matches!(
            s.claim_report(&JobId::random()),
            ReportClaim::NotFound
        ));
        s.mark_uploaded(&a.id);
        s.take_next().unwrap();
        s.finish(&a.id, Err((ErrorKind::Failed, "x".into())));
        assert!(matches!(s.claim_report(&a.id), ReportClaim::Claimed(_)));
        assert!(matches!(
            s.claim_report(&a.id),
            ReportClaim::AlreadyReported
        ));
        s.release_report(&a.id);
        assert!(matches!(s.claim_report(&a.id), ReportClaim::Claimed(_)));
        assert!(s.set_report_code(&a.id, "R-AAAAAA".into()));
        assert!(matches!(
            s.claim_report(&a.id),
            ReportClaim::AlreadyReported
        ));
        s.expire(&a.id, _c.now());
        assert!(matches!(s.claim_report(&a.id), ReportClaim::Expired));
    }

    #[test]
    fn orphan_dir_detection() {
        let (s, _c, _d) = setup();
        let live = s.create(req(), ip(1), &pairs()).unwrap();
        assert!(!s.is_orphan_dir(&live.id.to_string()));
        assert!(s.is_orphan_dir("not-an-id"));
        assert!(s.is_orphan_dir(&JobId::random().to_string()));
        let done = done_job(&s);
        s.expire(&done.id, _c.now());
        assert!(s.is_orphan_dir(&done.id.to_string()));
    }
}
