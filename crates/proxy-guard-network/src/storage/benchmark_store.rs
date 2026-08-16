use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use proxy_guard_core::{BenchmarkReport, CodexRegion, NodeId};

use crate::{ManagedPaths, NetworkError};

/// Filesystem-backed cache of fingerprint-bound benchmark reports.
#[derive(Clone, Debug)]
pub struct BenchmarkStore {
    root: PathBuf,
    ttl: Duration,
    mutation_lock: Arc<Mutex<()>>,
}

impl BenchmarkStore {
    /// Open the benchmark cache with a fixed freshness window.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the layout cannot be created.
    pub fn open(paths: &ManagedPaths, ttl: Duration) -> Result<Self, NetworkError> {
        paths.ensure_layout()?;
        Ok(Self {
            root: paths.benchmarks.clone(),
            ttl,
            mutation_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Persist one validated benchmark report keyed by node ID.
    ///
    /// # Errors
    ///
    /// Returns a validation or storage error.
    pub fn save(&self, report: &BenchmarkReport) -> Result<(), NetworkError> {
        report.validate().map_err(NetworkError::from)?;
        let _mutation = self.lock();
        let path = self.path_for(report.node_id);
        write_report(report, &path)
    }

    /// Load one cached report, if present.
    ///
    /// # Errors
    ///
    /// Returns a storage error only when the cache cannot be read or cleaned up.
    pub fn load(&self, node_id: NodeId) -> Result<Option<BenchmarkReport>, NetworkError> {
        let _mutation = self.lock();
        let path = self.path_for(node_id);
        load_report(&path)
    }

    /// Load a report only when it is fresh and matches the node's current fingerprint and region.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the cache cannot be read or cleaned up.
    pub fn get_fresh(
        &self,
        node_id: NodeId,
        fingerprint: &str,
        expected_region: CodexRegion,
        now: DateTime<Utc>,
    ) -> Result<Option<BenchmarkReport>, NetworkError> {
        let Some(report) = self.load(node_id)? else {
            return Ok(None);
        };
        if report.node_fingerprint != fingerprint {
            return Ok(None);
        }
        if report.expected_region != expected_region {
            return Ok(None);
        }
        let age = now
            .signed_duration_since(report.measured_at)
            .to_std()
            .unwrap_or_default();
        if age > self.ttl {
            return Ok(None);
        }
        Ok(Some(report))
    }

    /// Remove one cached report.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the file cannot be removed.
    pub fn delete(&self, node_id: NodeId) -> Result<(), NetworkError> {
        let _mutation = self.lock();
        let path = self.path_for(node_id);
        if path.is_file() {
            fs::remove_file(&path).map_err(|source| {
                NetworkError::Storage(format!("remove {}: {source}", path.display()))
            })?;
        }
        Ok(())
    }

    /// List all cached reports, ignoring unrelated files.
    ///
    /// # Errors
    ///
    /// Returns a storage error on unreadable cache data.
    pub fn list(&self) -> Result<Vec<BenchmarkReport>, NetworkError> {
        let _mutation = self.lock();
        let entries = fs::read_dir(&self.root).map_err(|source| {
            NetworkError::Storage(format!("read {}: {source}", self.root.display()))
        })?;
        let mut reports = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| {
                NetworkError::Storage(format!("read {}: {source}", self.root.display()))
            })?;
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if let Some(report) = load_report(&path)? {
                reports.push(report);
            }
        }
        reports.sort_by_key(|report| report.node_id);
        Ok(reports)
    }

    fn path_for(&self, node_id: NodeId) -> PathBuf {
        self.root.join(format!("{node_id}.json"))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn write_report(report: &BenchmarkReport, path: &Path) -> Result<(), NetworkError> {
    let staging = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        write_staged_report(report, &staging)?;
        replace_report(&staging, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

fn write_staged_report(report: &BenchmarkReport, path: &Path) -> Result<(), NetworkError> {
    let file = File::create(path)
        .map_err(|source| NetworkError::Storage(format!("create {}: {source}", path.display())))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, report)
        .map_err(|source| NetworkError::Storage(format!("serialize {}", source)))?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|source| NetworkError::Storage(format!("flush {}: {source}", path.display())))?;
    let file = writer.into_inner().map_err(|source| {
        NetworkError::Storage(format!("close {}: {}", path.display(), source.error()))
    })?;
    file.sync_all()
        .map_err(|source| NetworkError::Storage(format!("sync {}: {source}", path.display())))
}

fn replace_report(staging: &Path, path: &Path) -> Result<(), NetworkError> {
    let backup = path.with_extension("json.bak");
    recover_interrupted_replace(path, &backup)?;
    if !path.exists() {
        return fs::rename(staging, path).map_err(|source| {
            NetworkError::Storage(format!("commit {}: {source}", path.display()))
        });
    }
    fs::rename(path, &backup).map_err(|source| {
        NetworkError::Storage(format!("stage existing {}: {source}", path.display()))
    })?;
    if let Err(source) = fs::rename(staging, path) {
        let _ = fs::rename(&backup, path);
        return Err(NetworkError::Storage(format!(
            "commit {}: {source}",
            path.display()
        )));
    }
    fs::remove_file(&backup).map_err(|source| {
        NetworkError::Storage(format!("remove backup {}: {source}", backup.display()))
    })
}

fn recover_interrupted_replace(path: &Path, backup: &Path) -> Result<(), NetworkError> {
    if !backup.exists() {
        return Ok(());
    }
    if path.exists() {
        return fs::remove_file(backup).map_err(|source| {
            NetworkError::Storage(format!("remove backup {}: {source}", backup.display()))
        });
    }
    fs::rename(backup, path)
        .map_err(|source| NetworkError::Storage(format!("recover {}: {source}", path.display())))
}

fn load_report(path: &Path) -> Result<Option<BenchmarkReport>, NetworkError> {
    match read_report(path) {
        Ok(report) => Ok(Some(report)),
        Err(ReadReportError::NotFound) => Ok(None),
        Err(ReadReportError::Corrupt) => {
            fs::remove_file(path).map_err(|source| {
                NetworkError::Storage(format!("remove corrupt {}: {source}", path.display()))
            })?;
            Ok(None)
        }
        Err(ReadReportError::Io(source)) => Err(NetworkError::Storage(format!(
            "read {}: {source}",
            path.display()
        ))),
    }
}

enum ReadReportError {
    NotFound,
    Corrupt,
    Io(std::io::Error),
}

fn read_report(path: &Path) -> Result<BenchmarkReport, ReadReportError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(ReadReportError::NotFound);
        }
        Err(source) => return Err(ReadReportError::Io(source)),
    };
    let mut bytes = Vec::new();
    BufReader::new(file)
        .read_to_end(&mut bytes)
        .map_err(ReadReportError::Io)?;
    let report: BenchmarkReport =
        serde_json::from_slice(&bytes).map_err(|_| ReadReportError::Corrupt)?;
    report.validate().map_err(|_| ReadReportError::Corrupt)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::{fs, net::Ipv4Addr};

    use proxy_guard_core::{
        BENCHMARK_SCHEMA_VERSION, BenchmarkRejection, BenchmarkVerdict, CodexRegion,
    };
    use tempfile::tempdir;

    use super::BenchmarkStore;
    use crate::ManagedPaths;

    fn report(
        node_id: proxy_guard_core::NodeId,
        fingerprint: &str,
        minutes_ago: i64,
    ) -> proxy_guard_core::BenchmarkReport {
        proxy_guard_core::BenchmarkReport {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            node_id,
            node_fingerprint: fingerprint.to_owned(),
            expected_region: CodexRegion::JP,
            verified_region: CodexRegion::JP,
            first_exit_ip: Ipv4Addr::new(8, 8, 8, 8).into(),
            second_exit_ip: Ipv4Addr::new(8, 8, 8, 8).into(),
            exit_ip_stable: true,
            attempts: 5,
            successes: 5,
            median_header_ms: 84,
            p95_header_ms: 121,
            jitter_ms: 10,
            score: 93,
            verdict: BenchmarkVerdict::Healthy,
            measured_at: chrono::Utc::now() - chrono::Duration::minutes(minutes_ago),
        }
    }

    #[test]
    fn fresh_fingerprint_match_is_usable() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let store =
            BenchmarkStore::open(&paths, std::time::Duration::from_secs(6 * 3600)).expect("store");
        let node_id = proxy_guard_core::NodeId::new();
        let fingerprint = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        store.save(&report(node_id, fingerprint, 0)).expect("save");

        let fresh = store
            .get_fresh(node_id, fingerprint, CodexRegion::JP, chrono::Utc::now())
            .expect("fresh");
        assert!(fresh.is_some());
    }

    #[test]
    fn ttl_and_fingerprint_mismatch_invalidate() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let store =
            BenchmarkStore::open(&paths, std::time::Duration::from_secs(3600)).expect("store");
        let node_id = proxy_guard_core::NodeId::new();
        let fingerprint = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        store
            .save(&report(node_id, fingerprint, 24 * 60))
            .expect("save stale");

        assert!(
            store
                .get_fresh(node_id, fingerprint, CodexRegion::JP, chrono::Utc::now())
                .expect("stale")
                .is_none()
        );
        assert!(
            store
                .get_fresh(
                    node_id,
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    CodexRegion::JP,
                    chrono::Utc::now(),
                )
                .expect("mismatch")
                .is_none()
        );
    }

    #[test]
    fn rejected_verdict_round_trips() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let store =
            BenchmarkStore::open(&paths, std::time::Duration::from_secs(3600)).expect("store");
        let node_id = proxy_guard_core::NodeId::new();
        let mut report = report(
            node_id,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            0,
        );
        report.verdict = BenchmarkVerdict::Rejected {
            reason: BenchmarkRejection::CountryMismatch,
        };
        store.save(&report).expect("save");
        let loaded = store.load(node_id).expect("load").expect("present");
        assert_eq!(loaded.verdict, report.verdict);
    }

    #[test]
    fn region_mismatch_invalidates_an_otherwise_fresh_report() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let store =
            BenchmarkStore::open(&paths, std::time::Duration::from_secs(3600)).expect("store");
        let node_id = proxy_guard_core::NodeId::new();
        let fingerprint = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        store.save(&report(node_id, fingerprint, 0)).expect("save");

        assert!(
            store
                .get_fresh(node_id, fingerprint, CodexRegion::SG, chrono::Utc::now())
                .expect("region mismatch")
                .is_none()
        );
        assert!(
            store
                .get_fresh(node_id, fingerprint, CodexRegion::JP, chrono::Utc::now())
                .expect("region match")
                .is_some()
        );
    }

    #[test]
    fn corrupt_cache_is_removed_and_treated_as_a_miss() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let store =
            BenchmarkStore::open(&paths, std::time::Duration::from_secs(3600)).expect("store");
        let node_id = proxy_guard_core::NodeId::new();
        let fingerprint = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let path = store.path_for(node_id);
        fs::write(&path, b"{not valid json").expect("write corrupt cache");

        assert!(
            store
                .get_fresh(node_id, fingerprint, CodexRegion::JP, chrono::Utc::now())
                .expect("cache miss")
                .is_none()
        );
        assert!(!path.exists());
    }

    #[test]
    fn invalid_cache_report_is_removed_and_treated_as_a_miss() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let store =
            BenchmarkStore::open(&paths, std::time::Duration::from_secs(3600)).expect("store");
        let node_id = proxy_guard_core::NodeId::new();
        let fingerprint = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let mut invalid = report(node_id, fingerprint, 0);
        invalid.schema_version = 0;
        let path = store.path_for(node_id);
        fs::write(
            &path,
            serde_json::to_vec(&invalid).expect("serialize invalid report"),
        )
        .expect("write invalid cache");

        assert!(
            store
                .get_fresh(node_id, fingerprint, CodexRegion::JP, chrono::Utc::now())
                .expect("cache miss")
                .is_none()
        );
        assert!(!path.exists());
    }

    #[test]
    fn save_replaces_reports_without_leaving_staging_files() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let store =
            BenchmarkStore::open(&paths, std::time::Duration::from_secs(3600)).expect("store");
        let node_id = proxy_guard_core::NodeId::new();
        let fingerprint = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        store
            .save(&report(node_id, fingerprint, 0))
            .expect("first save");
        let mut replacement = report(node_id, fingerprint, 0);
        replacement.score = 99;
        store.save(&replacement).expect("replacement save");

        assert_eq!(
            store.load(node_id).expect("load").expect("report").score,
            99
        );
        let names = fs::read_dir(&paths.benchmarks)
            .expect("cache directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        assert!(names.iter().all(|name| {
            let name = name.to_string_lossy();
            !name.ends_with(".tmp") && !name.ends_with(".bak")
        }));
    }
}
