use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::NetworkError;

pub const SING_BOX_CONFIG_FILE_NAME: &str = "sing-box.json";
const STAGING_CONFIG_FILE_NAME: &str = ".sing-box.json.tmp";

/// A sing-box configuration committed inside one managed sidecar directory.
#[derive(Debug)]
pub struct PreparedSingBoxConfig {
    dir: PathBuf,
    path: PathBuf,
}

impl PreparedSingBoxConfig {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PreparedSingBoxConfig {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.dir.join(STAGING_CONFIG_FILE_NAME));
        let _ = fs::remove_dir(&self.dir);
    }
}

/// Persists a generated document at the only configuration path accepted by the runtime.
#[derive(Clone, Copy, Debug, Default)]
pub struct SingBoxConfigFile;

impl SingBoxConfigFile {
    /// Atomically create `sing-box.json` inside an existing sidecar directory.
    ///
    /// # Errors
    ///
    /// Returns a typed configuration/storage error if the value is not an object,
    /// the directory is missing, or the atomic commit fails.
    pub fn write(dir: &Path, document: &Value) -> Result<PreparedSingBoxConfig, NetworkError> {
        let config_path = dir.join(SING_BOX_CONFIG_FILE_NAME);
        if !document.is_object() {
            return Err(NetworkError::SingBox(
                "the root JSON value must be an object".into(),
            ));
        }
        let mut encoded = serde_json::to_vec_pretty(document)
            .map_err(|source| NetworkError::SingBox(format!("encode JSON: {source}")))?;
        encoded.push(b'\n');
        if !dir.is_dir() {
            return Err(NetworkError::SingBox(
                "the managed sidecar directory does not exist".into(),
            ));
        }
        if config_path.exists() {
            return Err(NetworkError::SingBox(
                "the sidecar already has a sing-box configuration".into(),
            ));
        }
        let result = commit_config(dir, &config_path, &encoded);
        if result.is_err() {
            let _ = fs::remove_file(dir.join(STAGING_CONFIG_FILE_NAME));
        }
        result.map(|()| PreparedSingBoxConfig {
            dir: dir.to_path_buf(),
            path: config_path,
        })
    }
}

fn commit_config(dir: &Path, config_path: &Path, encoded: &[u8]) -> Result<(), NetworkError> {
    let staging_path = dir.join(STAGING_CONFIG_FILE_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging_path)
        .map_err(|source| NetworkError::Storage(format!("create staging config: {source}")))?;
    file.write_all(encoded)
        .and_then(|()| file.sync_all())
        .map_err(|source| NetworkError::Storage(format!("write staging config: {source}")))?;
    drop(file);
    fs::rename(&staging_path, config_path)
        .map_err(|source| NetworkError::Storage(format!("commit config: {source}")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::{SING_BOX_CONFIG_FILE_NAME, SingBoxConfigFile};

    #[test]
    fn writes_one_pretty_config_inside_the_sidecar_directory() {
        let temporary = tempdir().expect("temporary directory");
        let dir = temporary.path().join("sidecar");
        std::fs::create_dir(&dir).expect("sidecar dir");

        let prepared = SingBoxConfigFile::write(&dir, &json!({"log": {"disabled": true}}))
            .expect("prepare config");

        assert_eq!(
            prepared.path().file_name().and_then(|name| name.to_str()),
            Some(SING_BOX_CONFIG_FILE_NAME)
        );
        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(prepared.path()).expect("read")).expect("json");
        assert_eq!(persisted, json!({"log": {"disabled": true}}));
        drop(prepared);
        assert!(!dir.is_dir());
    }

    #[test]
    fn rejects_non_object_and_duplicate_write() {
        let temporary = tempdir().expect("temporary directory");
        let dir = temporary.path().join("sidecar");
        std::fs::create_dir(&dir).expect("sidecar dir");
        assert!(SingBoxConfigFile::write(&dir, &json!([])).is_err());

        SingBoxConfigFile::write(&dir, &json!({"first": true})).expect("first");
        assert!(SingBoxConfigFile::write(&dir, &json!({"first": false})).is_err());
    }
}
