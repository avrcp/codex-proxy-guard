use std::{fs, path::PathBuf};

use proxy_guard_core::GuardConfig;

use crate::NetworkError;

/// Persistent and transient filesystem locations owned by Guard Managed Mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedPaths {
    pub root: PathBuf,
    pub subscriptions: PathBuf,
    pub nodes: PathBuf,
    pub benchmarks: PathBuf,
    pub runtime: PathBuf,
}

impl ManagedPaths {
    /// Resolve the managed layout under the user configuration directory.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the configuration directory cannot be resolved.
    pub fn discover() -> Result<Self, NetworkError> {
        GuardConfig::data_dir()
            .map(Self::from_root)
            .map_err(|error| NetworkError::Storage(error.to_string()))
    }

    #[must_use]
    pub fn from_root(root: PathBuf) -> Self {
        let managed = root.join("managed");
        Self {
            subscriptions: managed.join("subscriptions"),
            nodes: managed.join("nodes"),
            benchmarks: managed.join("benchmarks"),
            runtime: root.join("runtime"),
            root,
        }
    }

    /// Create the stable directory layout.
    ///
    /// # Errors
    ///
    /// Returns a storage error when a required directory cannot be created.
    pub fn ensure_layout(&self) -> Result<(), NetworkError> {
        for path in [
            &self.root,
            &self.subscriptions,
            &self.nodes,
            &self.benchmarks,
            &self.runtime,
        ] {
            fs::create_dir_all(path).map_err(|source| {
                NetworkError::Storage(format!("create {}: {source}", path.display()))
            })?;
        }
        Ok(())
    }

    #[must_use]
    pub fn sing_box_root(&self) -> PathBuf {
        self.runtime.join("sing-box")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn layout_uses_the_managed_and_runtime_roots() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));

        paths.ensure_layout().expect("layout");

        assert_eq!(
            paths.subscriptions,
            temporary.path().join("data/managed/subscriptions")
        );
        assert_eq!(paths.nodes, temporary.path().join("data/managed/nodes"));
        assert_eq!(
            paths.benchmarks,
            temporary.path().join("data/managed/benchmarks")
        );
        assert_eq!(paths.runtime, temporary.path().join("data/runtime"));
        assert!(paths.subscriptions.is_dir());
        assert!(paths.benchmarks.is_dir());
    }
}
