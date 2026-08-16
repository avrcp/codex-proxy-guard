use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use proxy_guard_core::{ManagedNode, NodeId};

use crate::{ManagedPaths, NetworkError};

const NODE_FILE: &str = "node.json";

/// A validated managed node and the filesystem location derived from its ID.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredNode {
    pub node: ManagedNode,
    pub node_dir: PathBuf,
}

/// Filesystem-backed repository for imported JP/SG/US node documents.
#[derive(Clone, Debug)]
pub struct NodeStore {
    root: PathBuf,
    mutation_lock: Arc<Mutex<()>>,
}

impl NodeStore {
    /// Open the node repository and create its parent layout when needed.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the application layout cannot be created.
    pub fn open(paths: &ManagedPaths) -> Result<Self, NetworkError> {
        paths.ensure_layout()?;
        Ok(Self {
            root: paths.nodes.clone(),
            mutation_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Validate and atomically import one node document.
    ///
    /// # Errors
    ///
    /// Returns a validation, duplicate-name, or storage error.
    pub fn create(&self, node: ManagedNode) -> Result<StoredNode, NetworkError> {
        node.validate().map_err(NetworkError::from)?;
        let _mutation = self.lock();
        if self
            .list()?
            .iter()
            .any(|stored| stored.node.name.to_lowercase() == node.name.to_lowercase())
        {
            return Err(NetworkError::Node(format!(
                "a node named {:?} already exists",
                node.name
            )));
        }

        let final_directory = self.root.join(node.id.to_string());
        if final_directory.exists() {
            return Err(NetworkError::Node("node directory already exists".into()));
        }
        let staging_directory = self.root.join(format!(".creating-{}", node.id));
        fs::create_dir(&staging_directory).map_err(|source| {
            io_error("create node staging directory", &staging_directory, source)
        })?;

        let result = self.commit_node(&node, &staging_directory, &final_directory);
        if result.is_err() {
            let _ = fs::remove_file(staging_directory.join(NODE_FILE));
            let _ = fs::remove_dir(&staging_directory);
        }
        result
    }

    /// Atomically replace one validated node while preserving its stable ID.
    ///
    /// # Errors
    ///
    /// Returns a validation, conflict, unsafe-path, or storage error.
    pub fn replace(&self, node: ManagedNode) -> Result<StoredNode, NetworkError> {
        node.validate().map_err(NetworkError::from)?;
        let _mutation = self.lock();
        let current = self.get(node.id)?;
        if self.list()?.iter().any(|stored| {
            stored.node.id != node.id && stored.node.name.to_lowercase() == node.name.to_lowercase()
        }) {
            return Err(NetworkError::Node(format!(
                "a node named {:?} already exists",
                node.name
            )));
        }

        let staging = self.root.join(format!(".replacing-{}", node.id));
        let backup = self.root.join(format!(".replaced-{}", node.id));
        if staging.exists() || backup.exists() {
            return Err(NetworkError::Node(
                "a previous node replacement requires recovery".into(),
            ));
        }
        fs::create_dir(&staging)
            .map_err(|source| io_error("create node replacement directory", &staging, source))?;
        if let Err(error) = write_node_document(&node, &staging.join(NODE_FILE)) {
            let _ = fs::remove_dir(&staging);
            return Err(error);
        }
        fs::rename(&current.node_dir, &backup)
            .map_err(|source| io_error("stage existing node replacement", &backup, source))?;
        if let Err(source) = fs::rename(&staging, &current.node_dir) {
            let _ = fs::rename(&backup, &current.node_dir);
            let _ = fs::remove_file(staging.join(NODE_FILE));
            let _ = fs::remove_dir(&staging);
            return Err(io_error(
                "commit node replacement",
                &current.node_dir,
                source,
            ));
        }
        fs::remove_file(backup.join(NODE_FILE))
            .map_err(|source| io_error("remove replaced node metadata", &backup, source))?;
        fs::remove_dir(&backup)
            .map_err(|source| io_error("remove replaced node directory", &backup, source))?;
        self.get(node.id)
    }

    /// Read and validate a node by ID.
    ///
    /// # Errors
    ///
    /// Returns a typed missing, unsafe-path, malformed-document, or storage error.
    pub fn get(&self, id: NodeId) -> Result<StoredNode, NetworkError> {
        let directory = self.root.join(id.to_string());
        if !directory.exists() {
            return Err(NetworkError::Node(format!("node {id} was not found")));
        }
        let directory = self.ensure_managed_node_directory(id, &directory)?;
        Self::load_node(id, &directory)
    }

    /// Resolve a node by UUID or unique case-insensitive display name.
    ///
    /// # Errors
    ///
    /// Returns a not-found error when no node matches.
    pub fn find(&self, reference: &str) -> Result<StoredNode, NetworkError> {
        if let Ok(id) = reference.parse::<NodeId>() {
            return self.get(id);
        }
        let normalized = reference.trim().to_lowercase();
        self.list()?
            .into_iter()
            .find(|stored| stored.node.name.to_lowercase() == normalized)
            .ok_or_else(|| NetworkError::Node(format!("node {reference} was not found")))
    }

    /// List managed nodes in deterministic name/ID order.
    ///
    /// # Errors
    ///
    /// Returns an error when the root cannot be read or a UUID directory is invalid.
    pub fn list(&self) -> Result<Vec<StoredNode>, NetworkError> {
        let entries = fs::read_dir(&self.root)
            .map_err(|source| io_error("read node directory", &self.root, source))?;
        let mut nodes = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| io_error("read node entry", &self.root, source))?;
            let file_type = entry
                .file_type()
                .map_err(|source| io_error("inspect node entry", &entry.path(), source))?;
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(id) = name.parse::<NodeId>() else {
                continue;
            };
            let directory = self.ensure_managed_node_directory(id, &entry.path())?;
            nodes.push(Self::load_node(id, &directory)?);
        }
        nodes.sort_by(|left, right| {
            left.node
                .name
                .to_lowercase()
                .cmp(&right.node.name.to_lowercase())
                .then_with(|| left.node.id.cmp(&right.node.id))
        });
        Ok(nodes)
    }

    /// Permanently delete one managed node document.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the node is absent, unsafe, or cannot be removed.
    pub fn delete(&self, id: NodeId) -> Result<(), NetworkError> {
        let _mutation = self.lock();
        let stored = self.get(id)?;
        let deletion_directory = self.root.join(format!(".deleting-{id}"));
        if deletion_directory.exists() {
            return Err(NetworkError::Node(
                "a previous node deletion requires recovery".into(),
            ));
        }
        fs::rename(&stored.node_dir, &deletion_directory).map_err(|source| {
            io_error(
                "move node into deletion directory",
                &stored.node_dir,
                source,
            )
        })?;
        let document = deletion_directory.join(NODE_FILE);
        fs::remove_file(&document)
            .map_err(|source| io_error("remove node metadata", &document, source))?;
        fs::remove_dir(&deletion_directory)
            .map_err(|source| io_error("remove node directory", &deletion_directory, source))
    }

    fn commit_node(
        &self,
        node: &ManagedNode,
        staging_directory: &Path,
        final_directory: &Path,
    ) -> Result<StoredNode, NetworkError> {
        write_node_document(node, &staging_directory.join(NODE_FILE))?;
        fs::rename(staging_directory, final_directory)
            .map_err(|source| io_error("commit node directory", final_directory, source))?;
        self.get(node.id)
    }

    fn load_node(directory_id: NodeId, directory: &Path) -> Result<StoredNode, NetworkError> {
        let node_path = directory.join(NODE_FILE);
        let file = File::open(&node_path)
            .map_err(|source| io_error("open node metadata", &node_path, source))?;
        let node: ManagedNode = serde_json::from_reader(BufReader::new(file))
            .map_err(|source| invalid_node(&node_path, source.to_string()))?;
        if node.id != directory_id {
            return Err(NetworkError::Node(format!(
                "node directory {directory_id} and document {} differ",
                node.id
            )));
        }
        node.validate()
            .map_err(|source| invalid_node(&node_path, source.to_string()))?;
        Ok(StoredNode {
            node,
            node_dir: directory.to_path_buf(),
        })
    }

    fn ensure_managed_node_directory(
        &self,
        id: NodeId,
        directory: &Path,
    ) -> Result<PathBuf, NetworkError> {
        let metadata = fs::symlink_metadata(directory)
            .map_err(|source| io_error("inspect node directory", directory, source))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(NetworkError::Node("unsafe node directory".into()));
        }
        let canonical_root = self
            .root
            .canonicalize()
            .map_err(|source| io_error("resolve node root", &self.root, source))?;
        let canonical_directory = directory
            .canonicalize()
            .map_err(|source| io_error("resolve node directory", directory, source))?;
        let expected_name = id.to_string();
        if canonical_directory.parent() != Some(canonical_root.as_path())
            || canonical_directory
                .file_name()
                .and_then(|name| name.to_str())
                != Some(expected_name.as_str())
        {
            return Err(NetworkError::Node(
                "node directory is outside its managed root".into(),
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

fn write_node_document(node: &ManagedNode, path: &Path) -> Result<(), NetworkError> {
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| io_error("create node metadata", path, source))?;
    let mut writer = BufWriter::new(&file);
    serde_json::to_writer_pretty(&mut writer, node)
        .map_err(|source| invalid_node(path, source.to_string()))?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|source| io_error("write node metadata", path, source))?;
    drop(writer);
    file.sync_all()
        .map_err(|source| io_error("sync node metadata", path, source))
}

fn invalid_node(path: &Path, reason: impl Into<String>) -> NetworkError {
    NetworkError::Node(format!("{}: {}", path.display(), reason.into()))
}

fn io_error(action: &'static str, path: &Path, source: std::io::Error) -> NetworkError {
    NetworkError::Storage(format!("{action} {}: {source}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use proxy_guard_core::{CodexRegion, ManagedNode, NodeId, SingBoxOutbound, SubscriptionId};
    use serde_json::json;
    use tempfile::tempdir;

    use super::{NODE_FILE, NodeStore};
    use crate::ManagedPaths;

    fn store() -> (tempfile::TempDir, NodeStore) {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let store = NodeStore::open(&paths).expect("open node store");
        (temporary, store)
    }

    fn node(name: &str) -> ManagedNode {
        ManagedNode::new(
            name,
            SubscriptionId::new(),
            CodexRegion::JP,
            SingBoxOutbound::new(json!({
                "type": "socks",
                "server": "proxy.example",
                "server_port": 1080
            }))
            .expect("outbound"),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("node")
    }

    #[test]
    fn imports_lists_finds_and_deletes_a_node() {
        let (_temporary, store) = store();
        let created = store.create(node("JP Tokyo")).expect("create node");
        assert!(created.node_dir.join(NODE_FILE).is_file());
        assert_eq!(store.get(created.node.id).expect("get node"), created);
        assert_eq!(store.find("jp tokyo").expect("find by name"), created);
        assert_eq!(store.list().expect("list nodes"), vec![created.clone()]);

        store.delete(created.node.id).expect("delete node");
        assert!(!created.node_dir.exists());
    }

    #[test]
    fn rejects_duplicate_names_and_mismatched_ids() {
        let (_temporary, store) = store();
        let created = store.create(node("JP Tokyo")).expect("create node");
        assert!(store.create(node("jp tokyo")).is_err());

        let path = created.node_dir.join(NODE_FILE);
        let text = fs::read_to_string(&path).expect("read node");
        fs::write(
            &path,
            text.replace(&created.node.id.to_string(), &NodeId::new().to_string()),
        )
        .expect("corrupt node");
        assert!(store.get(created.node.id).is_err());
    }
}
