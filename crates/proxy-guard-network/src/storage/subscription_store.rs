use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use proxy_guard_core::{SubscriptionId, SubscriptionNodeBinding, SubscriptionSource};
use serde::{Deserialize, Serialize};

use crate::{ManagedPaths, NetworkError};

const SUBSCRIPTION_FILE: &str = "subscription.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredSubscription {
    pub source: SubscriptionSource,
    pub bindings: Vec<SubscriptionNodeBinding>,
    pub subscription_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct SubscriptionStore {
    root: PathBuf,
    mutation_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SubscriptionDocument {
    source: SubscriptionSource,
    #[serde(default)]
    bindings: Vec<SubscriptionNodeBinding>,
}

impl SubscriptionStore {
    /// Open the managed subscription repository.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the managed layout cannot be created.
    pub fn open(paths: &ManagedPaths) -> Result<Self, NetworkError> {
        paths.ensure_layout()?;
        Ok(Self {
            root: paths.subscriptions.clone(),
            mutation_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Persist new URL-free subscription metadata.
    ///
    /// # Errors
    ///
    /// Returns a validation, conflict, or storage error.
    pub fn create(&self, source: SubscriptionSource) -> Result<StoredSubscription, NetworkError> {
        source.validate().map_err(NetworkError::from)?;
        let _mutation = self.lock();
        if self
            .list()?
            .iter()
            .any(|stored| stored.source.name.to_lowercase() == source.name.to_lowercase())
        {
            return Err(invalid("a subscription with that name already exists"));
        }
        let final_dir = self.root.join(source.id.to_string());
        let staging = self.root.join(format!(".creating-{}", source.id));
        if final_dir.exists() || staging.exists() {
            return Err(invalid("the subscription directory already exists"));
        }
        fs::create_dir(&staging).map_err(|source| {
            io_error("create subscription staging directory", &staging, source)
        })?;
        let document = SubscriptionDocument {
            source,
            bindings: Vec::new(),
        };
        if let Err(error) = write_document(&staging.join(SUBSCRIPTION_FILE), &document) {
            let _ = fs::remove_dir(&staging);
            return Err(error);
        }
        fs::rename(&staging, &final_dir)
            .map_err(|source| io_error("commit subscription directory", &final_dir, source))?;
        self.get(document.source.id)
    }

    /// Load one subscription by its stable identifier.
    ///
    /// # Errors
    ///
    /// Returns a not-found, unsafe-path, schema, or storage error.
    pub fn get(&self, id: SubscriptionId) -> Result<StoredSubscription, NetworkError> {
        let directory = self.root.join(id.to_string());
        if !directory.exists() {
            return Err(invalid("subscription was not found"));
        }
        let directory = self.ensure_managed_directory(id, &directory)?;
        Self::load(id, &directory)
    }

    /// Resolve a subscription by UUID or unique case-insensitive name.
    ///
    /// # Errors
    ///
    /// Returns a not-found or storage error.
    pub fn find(&self, reference: &str) -> Result<StoredSubscription, NetworkError> {
        if let Ok(id) = reference.parse::<SubscriptionId>() {
            return self.get(id);
        }
        let normalized = reference.trim().to_lowercase();
        self.list()?
            .into_iter()
            .find(|stored| stored.source.name.to_lowercase() == normalized)
            .ok_or_else(|| invalid("subscription was not found"))
    }

    /// List all subscriptions in deterministic order.
    ///
    /// # Errors
    ///
    /// Returns an unsafe-path, schema, or storage error.
    pub fn list(&self) -> Result<Vec<StoredSubscription>, NetworkError> {
        let entries = fs::read_dir(&self.root)
            .map_err(|source| io_error("read subscription directory", &self.root, source))?;
        let mut subscriptions = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|source| io_error("read subscription entry", &self.root, source))?;
            let file_type = entry
                .file_type()
                .map_err(|source| io_error("inspect subscription entry", &entry.path(), source))?;
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(id) = name.parse::<SubscriptionId>() else {
                continue;
            };
            let directory = self.ensure_managed_directory(id, &entry.path())?;
            subscriptions.push(Self::load(id, &directory)?);
        }
        subscriptions.sort_by(|left, right| {
            left.source
                .name
                .to_lowercase()
                .cmp(&right.source.name.to_lowercase())
                .then_with(|| left.source.id.cmp(&right.source.id))
        });
        Ok(subscriptions)
    }

    /// Atomically replace metadata and reconciled bindings.
    ///
    /// # Errors
    ///
    /// Returns a validation, conflict, or storage error without replacing the live document.
    pub fn replace(
        &self,
        source: SubscriptionSource,
        bindings: Vec<SubscriptionNodeBinding>,
    ) -> Result<StoredSubscription, NetworkError> {
        validate_document(&source, &bindings)?;
        let _mutation = self.lock();
        let stored = self.get(source.id)?;
        if self.list()?.iter().any(|candidate| {
            candidate.source.id != source.id
                && candidate.source.name.to_lowercase() == source.name.to_lowercase()
        }) {
            return Err(invalid("a subscription with that name already exists"));
        }
        let path = stored.subscription_dir.join(SUBSCRIPTION_FILE);
        let staging = stored.subscription_dir.join("subscription.json.tmp");
        let backup = stored.subscription_dir.join("subscription.json.bak");
        if staging.exists() || backup.exists() {
            return Err(invalid("a previous subscription update requires recovery"));
        }
        write_document(&staging, &SubscriptionDocument { source, bindings })?;
        fs::rename(&path, &backup)
            .map_err(|source| io_error("stage subscription update", &path, source))?;
        if let Err(source) = fs::rename(&staging, &path) {
            let _ = fs::rename(&backup, &path);
            let _ = fs::remove_file(&staging);
            return Err(io_error("commit subscription update", &path, source));
        }
        fs::remove_file(&backup)
            .map_err(|source| io_error("remove subscription update backup", &backup, source))?;
        self.get(stored.source.id)
    }

    /// Delete subscription metadata after its credential has been handled by the service.
    ///
    /// # Errors
    ///
    /// Returns a not-found, unsafe-path, or storage error.
    pub fn delete(&self, id: SubscriptionId) -> Result<(), NetworkError> {
        let _mutation = self.lock();
        let stored = self.get(id)?;
        let deleting = self.root.join(format!(".deleting-{id}"));
        if deleting.exists() {
            return Err(invalid(
                "a previous subscription deletion requires recovery",
            ));
        }
        fs::rename(&stored.subscription_dir, &deleting)
            .map_err(|source| io_error("stage subscription deletion", &deleting, source))?;
        let document = deleting.join(SUBSCRIPTION_FILE);
        fs::remove_file(&document)
            .map_err(|source| io_error("remove subscription metadata", &document, source))?;
        fs::remove_dir(&deleting)
            .map_err(|source| io_error("remove subscription directory", &deleting, source))
    }

    fn load(
        directory_id: SubscriptionId,
        directory: &Path,
    ) -> Result<StoredSubscription, NetworkError> {
        let path = directory.join(SUBSCRIPTION_FILE);
        let file = File::open(&path)
            .map_err(|source| io_error("open subscription metadata", &path, source))?;
        let document: SubscriptionDocument = serde_json::from_reader(BufReader::new(file))
            .map_err(|_| invalid("stored subscription metadata is invalid"))?;
        if document.source.id != directory_id {
            return Err(invalid("subscription directory and document IDs differ"));
        }
        validate_document(&document.source, &document.bindings)?;
        Ok(StoredSubscription {
            source: document.source,
            bindings: document.bindings,
            subscription_dir: directory.to_path_buf(),
        })
    }

    fn ensure_managed_directory(
        &self,
        id: SubscriptionId,
        directory: &Path,
    ) -> Result<PathBuf, NetworkError> {
        let metadata = fs::symlink_metadata(directory)
            .map_err(|source| io_error("inspect subscription directory", directory, source))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(invalid("unsafe subscription directory"));
        }
        let canonical_root = self
            .root
            .canonicalize()
            .map_err(|source| io_error("resolve subscription root", &self.root, source))?;
        let canonical = directory
            .canonicalize()
            .map_err(|source| io_error("resolve subscription directory", directory, source))?;
        if canonical.parent() != Some(canonical_root.as_path())
            || canonical.file_name().and_then(|name| name.to_str()) != Some(id.to_string().as_str())
        {
            return Err(invalid(
                "subscription directory is outside its managed root",
            ));
        }
        Ok(directory.to_path_buf())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn validate_document(
    source: &SubscriptionSource,
    bindings: &[SubscriptionNodeBinding],
) -> Result<(), NetworkError> {
    source.validate().map_err(NetworkError::from)?;
    let mut remote_keys = HashSet::new();
    let mut node_ids = HashSet::new();
    for binding in bindings {
        if binding.subscription_id != source.id
            || binding.remote_key.len() != 64
            || !binding
                .remote_key
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || !remote_keys.insert(binding.remote_key.as_str())
            || !node_ids.insert(binding.node_id)
        {
            return Err(invalid("subscription bindings are invalid"));
        }
    }
    Ok(())
}

fn write_document(path: &Path, document: &SubscriptionDocument) -> Result<(), NetworkError> {
    let file = File::create(path)
        .map_err(|source| io_error("create subscription metadata", path, source))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, document)
        .map_err(|_| invalid("could not serialize subscription metadata"))?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|source| io_error("flush subscription metadata", path, source))
}

fn invalid(reason: impl Into<String>) -> NetworkError {
    NetworkError::Storage(format!("subscription: {}", reason.into()))
}

fn io_error(action: &'static str, path: &Path, source: std::io::Error) -> NetworkError {
    NetworkError::Storage(format!("{action} {}: {source}", path.display()))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::SubscriptionStore;
    use crate::ManagedPaths;
    use proxy_guard_core::SubscriptionSource;

    #[test]
    fn metadata_round_trip_never_serializes_a_url() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let store = SubscriptionStore::open(&paths).expect("store");
        let source = SubscriptionSource::new("Airport").expect("source");

        let stored = store.create(source).expect("create");
        let text = std::fs::read_to_string(stored.subscription_dir.join(super::SUBSCRIPTION_FILE))
            .expect("read");

        assert!(!text.contains("url"));
        assert!(!text.contains("token"));
        assert_eq!(store.list().expect("list"), vec![stored]);
    }
}
