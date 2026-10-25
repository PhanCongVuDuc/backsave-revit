//! Failure reports: inputs that users agreed to share, kept for 30 days within a size cap.
//!
//! A report directory holds `input.<ext>`, `log.jsonl` (when present) and `meta.json`. The
//! metadata never contains the original file name or the uploader's address. Nothing in the
//! HTTP API reads reports back.

use crate::jobs::Job;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const GIB: u64 = 1024 * 1024 * 1024;
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 6;

#[derive(Debug, Clone)]
pub struct Reports {
    dir: PathBuf,
    max_bytes: u64,
    ttl: Duration,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    engine_version: String,
    error_kind: String,
    failed_at: String,
    kind: String,
    size: u64,
}

impl Reports {
    /// Reports under `dir`: 20 GiB in total, 30 days each.
    pub fn new(dir: PathBuf) -> Reports {
        Reports::with_limits(dir, 20 * GIB, Duration::days(30))
    }

    pub fn with_limits(dir: PathBuf, max_bytes: u64, ttl: Duration) -> Reports {
        Reports {
            dir,
            max_bytes,
            ttl,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Moves the failed job's input (and log, if any) into a new report and returns its code.
    pub fn create(
        &self,
        job: &Job,
        input_path: &Path,
        log_path: &Path,
        engine_version: &str,
        now: DateTime<Utc>,
    ) -> io::Result<String> {
        fs::create_dir_all(&self.dir)?;
        let (code, dir) = self.new_dir()?;
        let result = self.fill(&dir, job, input_path, log_path, engine_version, now);
        if result.is_err() {
            let _ = fs::remove_dir_all(&dir);
        }
        result.map(|()| code)
    }

    /// Picks an unused code and creates its directory.
    fn new_dir(&self) -> io::Result<(String, PathBuf)> {
        for _ in 0..100 {
            let mut raw = [0u8; CODE_LEN];
            getrandom::fill(&mut raw).map_err(|e| io::Error::other(e.to_string()))?;
            let tail: String = raw
                .iter()
                .map(|b| CODE_ALPHABET[usize::from(b & 31)] as char)
                .collect();
            let code = format!("R-{tail}");
            let dir = self.dir.join(&code);
            match fs::create_dir(&dir) {
                Ok(()) => return Ok((code, dir)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("no free report code"))
    }

    fn fill(
        &self,
        dir: &Path,
        job: &Job,
        input_path: &Path,
        log_path: &Path,
        engine_version: &str,
        now: DateTime<Utc>,
    ) -> io::Result<()> {
        let ext = job.kind.ext();
        let size = fs::metadata(input_path)?.len();
        move_file(input_path, &dir.join(format!("input.{ext}")))?;
        match move_file(log_path, &dir.join("log.jsonl")) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let error_kind = job
            .error
            .as_ref()
            .and_then(|e| serde_json::to_value(e.kind).ok())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".into());
        let meta = Meta {
            engine_version: engine_version.to_string(),
            error_kind,
            failed_at: job
                .finished_at
                .unwrap_or(now)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            kind: ext.to_string(),
            size,
        };
        let text = serde_json::to_vec_pretty(&meta).map_err(io::Error::other)?;
        fs::write(dir.join("meta.json"), text)
    }

    /// Deletes reports older than the time limit, then the oldest ones until the total fits.
    pub fn enforce(&self, now: DateTime<Utc>) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut all: Vec<(DateTime<Utc>, u64, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let at = failed_at(&path).unwrap_or(now);
            if now - at > self.ttl {
                remove_dir(&path);
                continue;
            }
            all.push((at, dir_size(&path), path));
        }
        all.sort_by_key(|(at, _, _)| *at);
        let mut total: u64 = all.iter().map(|(_, size, _)| size).sum();
        for (_, size, path) in all {
            if total <= self.max_bytes {
                break;
            }
            remove_dir(&path);
            total -= size;
        }
    }
}

/// When the report was made: `meta.json`, else the directory's modification time.
fn failed_at(dir: &Path) -> Option<DateTime<Utc>> {
    let from_meta = fs::read(dir.join("meta.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Meta>(&b).ok())
        .and_then(|m| DateTime::parse_from_rfc3339(&m.failed_at).ok())
        .map(|t| t.with_timezone(&Utc));
    from_meta.or_else(|| {
        fs::metadata(dir)
            .and_then(|m| m.modified())
            .ok()
            .map(DateTime::<Utc>::from)
    })
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn remove_dir(path: &Path) {
    if let Err(e) = fs::remove_dir_all(path)
        && e.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(error = ?e.kind(), "could not remove a report");
    }
}

/// Renames, or copies and deletes when the two paths are on different devices.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if !from.exists() {
        return Err(io::ErrorKind::NotFound.into());
    }
    fs::copy(from, to)?;
    fs::remove_file(from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Clock, ManualClock};
    use crate::jobs::{JobStore, NewJob};
    use crate::model::{ErrorKind, FileKind};
    use std::sync::Arc;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    /// Writes a report directory by hand with the given age and payload size.
    fn fake_report(reports: &Reports, code: &str, age: Duration, bytes: usize) {
        let dir = reports.dir().join(code);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("input.rvt"), vec![0u8; bytes]).unwrap();
        let meta = Meta {
            engine_version: "e".into(),
            error_kind: "failed".into(),
            failed_at: (now() - age).to_rfc3339_opts(SecondsFormat::Secs, true),
            kind: "rvt".into(),
            size: bytes as u64,
        };
        fs::write(dir.join("meta.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
    }

    fn exists(reports: &Reports, code: &str) -> bool {
        reports.dir().join(code).is_dir()
    }

    #[test]
    fn reports_over_cap_drop_oldest() {
        let d = tempfile::tempdir().unwrap();
        let r = Reports::with_limits(d.path().to_path_buf(), 2500, Duration::days(30));
        fake_report(&r, "R-OLDEST", Duration::days(3), 1000);
        fake_report(&r, "R-MIDDLE", Duration::days(2), 1000);
        fake_report(&r, "R-NEWEST", Duration::days(1), 1000);
        r.enforce(now());
        assert!(!exists(&r, "R-OLDEST"));
        assert!(exists(&r, "R-MIDDLE"));
        assert!(exists(&r, "R-NEWEST"));
    }

    #[test]
    fn reports_older_than_30_days_removed() {
        let d = tempfile::tempdir().unwrap();
        let r = Reports::new(d.path().to_path_buf());
        fake_report(
            &r,
            "R-STALE0",
            Duration::days(30) + Duration::seconds(1),
            10,
        );
        fake_report(&r, "R-EDGE00", Duration::days(30), 10);
        fake_report(&r, "R-FRESH0", Duration::days(1), 10);
        r.enforce(now());
        assert!(!exists(&r, "R-STALE0"));
        assert!(exists(&r, "R-EDGE00"));
        assert!(exists(&r, "R-FRESH0"));
    }

    #[test]
    fn report_without_meta_uses_directory_age() {
        let d = tempfile::tempdir().unwrap();
        let r = Reports::new(d.path().to_path_buf());
        fs::create_dir_all(d.path().join("R-NOMETA")).unwrap();
        // The directory was just made, so a far-future "now" makes it old.
        r.enforce(Utc::now() + Duration::days(31));
        assert!(!exists(&r, "R-NOMETA"));
    }

    fn failed_job(dir: &Path) -> (JobStore, Job) {
        let clock = Arc::new(ManualClock::new(now()));
        let store = JobStore::new(dir.to_path_buf(), clock);
        let req = NewJob {
            name: "Secret Name.rvt".into(),
            size: 5,
            kind: FileKind::Rvt,
            year: 2026,
            target: 2025,
        };
        let ip = "203.0.113.77".parse().unwrap();
        let job = store.create(req, ip, &[(2026, 2025)]).unwrap();
        fs::write(store.input_path(&job), b"12345").unwrap();
        fs::write(store.log_path(&job), b"{}\n").unwrap();
        store.mark_uploaded(&job.id);
        store.take_next().unwrap();
        store.finish(&job.id, Err((ErrorKind::Corrupt, "bad".into())));
        let job = store.get(&job.id).unwrap();
        (store, job)
    }

    #[test]
    fn meta_json_has_no_name_or_ip() {
        let d = tempfile::tempdir().unwrap();
        let (store, job) = failed_job(d.path());
        let r = Reports::new(d.path().join("reports"));
        let code = r
            .create(
                &job,
                &store.input_path(&job),
                &store.log_path(&job),
                "engine 1.2.3",
                store_now(),
            )
            .unwrap();
        let text = fs::read_to_string(r.dir().join(&code).join("meta.json")).unwrap();
        assert!(!text.contains("Secret"));
        assert!(!text.contains("203.0.113.77"));
        assert!(!text.contains(&job.id.to_string()));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["engine_version", "error_kind", "failed_at", "kind", "size"]
        );
        assert_eq!(v["engine_version"], "engine 1.2.3");
        assert_eq!(v["error_kind"], "corrupt");
        assert_eq!(v["size"], 5);
        assert_eq!(
            fs::read(r.dir().join(&code).join("input.rvt")).unwrap(),
            b"12345"
        );
        assert!(!store.input_path(&job).exists());
        assert!(!store.log_path(&job).exists());
    }

    fn store_now() -> DateTime<Utc> {
        ManualClock::new(now()).now()
    }

    #[test]
    fn code_uses_crockford_alphabet() {
        let d = tempfile::tempdir().unwrap();
        let r = Reports::new(d.path().to_path_buf());
        for _ in 0..50 {
            let (code, dir) = r.new_dir().unwrap();
            assert_eq!(code.len(), 8);
            let tail = code.strip_prefix("R-").unwrap();
            assert!(tail.bytes().all(|b| CODE_ALPHABET.contains(&b)), "{code}");
            assert!(dir.is_dir());
        }
    }

    #[test]
    fn create_without_log_still_works() {
        let d = tempfile::tempdir().unwrap();
        let (store, job) = failed_job(d.path());
        fs::remove_file(store.log_path(&job)).unwrap();
        let r = Reports::new(d.path().join("reports"));
        let code = r
            .create(
                &job,
                &store.input_path(&job),
                &store.log_path(&job),
                "e",
                now(),
            )
            .unwrap();
        assert!(!r.dir().join(&code).join("log.jsonl").exists());
        assert!(r.dir().join(&code).join("input.rvt").is_file());
    }

    #[test]
    fn create_missing_input_leaves_nothing_behind() {
        let d = tempfile::tempdir().unwrap();
        let (store, job) = failed_job(d.path());
        fs::remove_file(store.input_path(&job)).unwrap();
        let r = Reports::new(d.path().join("reports"));
        let err = r
            .create(
                &job,
                &store.input_path(&job),
                &store.log_path(&job),
                "e",
                now(),
            )
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(fs::read_dir(r.dir()).unwrap().count(), 0);
    }
}
