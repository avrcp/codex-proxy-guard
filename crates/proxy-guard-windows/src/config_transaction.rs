//! A held Windows file handle serializes Guard configuration mutations with
//! both other Guard frontends and ordinary external editors. The handle is
//! kept across a consent revoke, so an authorized Home cannot change between
//! removing its block and persisting the revoked consent.

use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::Path,
};
#[cfg(windows)]
use std::{fs::OpenOptions, io::Read};

use proxy_guard_core::GuardConfig;

const MAX_CONFIG_BYTES: usize = 64 * 1024;

pub enum ConfigExpectation<'a> {
    Current(&'a GuardConfig),
    /// Editor recovery is allowed only while the locked file is still invalid.
    /// It must never overwrite a valid configuration another frontend saved.
    Invalid,
}

pub struct GuardConfigTransaction {
    file: File,
    original: Vec<u8>,
}

/// A launch keeps its authorized configuration stable until its final
/// outcome. This is read-only, but excludes writers/deletion so a concurrent
/// revoke cannot clear the binding while a launch prepares its Home block.
pub struct GuardConfigLease {
    _file: File,
}

impl GuardConfigLease {
    pub fn acquire(path: &Path, expected: &GuardConfig) -> Result<Self, String> {
        #[cfg(not(windows))]
        {
            let _ = (path, expected);
            Err("CONFIG_LOCK_UNSUPPORTED: configuration leases require Windows".into())
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
            let mut file = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(path)
                .map_err(|_| "CONFIG_LOCK_FAILED: configuration is changing or unavailable")?;
            let bytes = bounded_read(&mut file)?;
            if !parse(&bytes)
                .as_ref()
                .is_some_and(|current| current == expected)
            {
                return Err(
                    "CONFIG_CHANGED: refresh before launching with the new configuration".into(),
                );
            }
            Ok(Self { _file: file })
        }
    }
}

impl GuardConfigTransaction {
    pub fn begin(path: &Path, expectation: ConfigExpectation<'_>) -> Result<Self, String> {
        // Windows is the supported product platform. Without its share-mode
        // exclusion we cannot promise the consent transaction invariant.
        #[cfg(not(windows))]
        {
            let _ = (path, expectation);
            Err("CONFIG_LOCK_UNSUPPORTED: configuration transactions require Windows".into())
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

            let mut options = OpenOptions::new();
            options.read(true).write(true).share_mode(FILE_SHARE_READ);
            let mut file = match options.open(path) {
                Ok(file) => file,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && matches!(expectation, ConfigExpectation::Invalid) =>
                {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(
                            |_| "CONFIG_LOCK_FAILED: configuration directory is not writable",
                        )?;
                    }
                    // Never truncate a file that appeared between attempts.
                    options.create_new(true).open(path).map_err(
                        |_| "CONFIG_LOCK_FAILED: configuration changed or is not writable",
                    )?
                }
                Err(_) => {
                    return Err(
                        "CONFIG_LOCK_FAILED: configuration is busy, missing, or not writable"
                            .into(),
                    );
                }
            };
            let original = bounded_read(&mut file)?;
            let current = parse(&original);
            let matches = match expectation {
                ConfigExpectation::Current(expected) => {
                    current.as_ref().is_some_and(|config| config == expected)
                }
                ConfigExpectation::Invalid => current.is_none(),
            };
            if !matches {
                return Err("CONFIG_CHANGED: refresh and confirm the configuration again".into());
            }
            Ok(Self { file, original })
        }
    }

    /// Write through the same handle that excludes other writers/deletion.
    /// Keep original bytes for best-effort rollback if the device rejects a
    /// write/flush. No temporary unlocked config pathname is introduced.
    pub fn commit(mut self, updated: &GuardConfig) -> Result<(), String> {
        updated
            .validate()
            .map_err(|_| "CONFIG_INVALID: updated configuration is invalid")?;
        let bytes = toml::to_string_pretty(updated)
            .map_err(|_| "CONFIG_INVALID: cannot serialize configuration")?
            .into_bytes();
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err("CONFIG_TOO_LARGE: configuration exceeds 64 KiB".into());
        }
        if replace(&mut self.file, &bytes).is_err() {
            if replace(&mut self.file, &self.original).is_err() {
                return Err("CONFIG_ROLLBACK_FAILED: the device rejected both the write and restoration; inspect the Guard configuration before continuing".into());
            }
            return Err("CONFIG_WRITE_FAILED: the original configuration was restored".into());
        }
        Ok(())
    }
}

#[cfg(windows)]
fn bounded_read(file: &mut File) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "CONFIG_READ_FAILED: cannot read the locked configuration")?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("CONFIG_TOO_LARGE: configuration exceeds 64 KiB".into());
    }
    Ok(bytes)
}

#[cfg(windows)]
fn parse(bytes: &[u8]) -> Option<GuardConfig> {
    let config: GuardConfig = toml::from_str(std::str::from_utf8(bytes).ok()?).ok()?;
    config.validate().ok()?;
    Some(config)
}

fn replace(file: &mut File, bytes: &[u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    file.write_all(bytes)?;
    file.set_len(bytes.len() as u64)?;
    file.sync_all()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "cpg-config-transaction-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn held_handle_excludes_guard_save_external_write_and_delete() {
        let path = temp_path();
        let expected = GuardConfig::default();
        expected.save(&path).unwrap();
        let transaction =
            GuardConfigTransaction::begin(&path, ConfigExpectation::Current(&expected)).unwrap();
        let mut next = expected.clone();
        next.proxy.port = 7890;
        assert!(next.save(&path).is_err());
        assert!(std::fs::write(&path, "version=1").is_err());
        assert!(std::fs::remove_file(&path).is_err());
        assert!(
            GuardConfigLease::acquire(&path, &expected).is_err(),
            "launch cannot overlap consent mutation"
        );
        assert_eq!(
            GuardConfig::load(&path).unwrap(),
            expected,
            "readers remain allowed"
        );
        transaction.commit(&next).unwrap();
        assert_eq!(GuardConfig::load(&path).unwrap(), next);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn launch_lease_is_read_only_and_excludes_revoke_until_dropped() {
        let path = temp_path();
        let expected = GuardConfig::default();
        expected.save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let lease = GuardConfigLease::acquire(&path, &expected).unwrap();
        assert!(
            GuardConfigTransaction::begin(&path, ConfigExpectation::Current(&expected)).is_err()
        );
        assert!(expected.save(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        assert!(
            GuardConfigLease::acquire(&path, &expected).is_ok(),
            "launch leases may share read access"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(lease);
        assert!(
            GuardConfigTransaction::begin(&path, ConfigExpectation::Current(&expected)).is_ok()
        );
        let mut stale = expected.clone();
        stale.proxy.port = 7890;
        assert!(GuardConfigLease::acquire(&path, &stale).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stale_expected_and_repaired_invalid_state_cannot_overwrite_valid_config() {
        let path = temp_path();
        let stale = GuardConfig::default();
        let mut current = stale.clone();
        current.proxy.port = 7890;
        current.save(&path).unwrap();
        assert!(GuardConfigTransaction::begin(&path, ConfigExpectation::Current(&stale)).is_err());
        assert!(GuardConfigTransaction::begin(&path, ConfigExpectation::Invalid).is_err());
        assert_eq!(GuardConfig::load(&path).unwrap(), current);
        std::fs::write(&path, "version=1\n").unwrap();
        GuardConfigTransaction::begin(&path, ConfigExpectation::Invalid)
            .unwrap()
            .commit(&stale)
            .unwrap();
        assert_eq!(GuardConfig::load(&path).unwrap(), stale);
        std::fs::remove_file(path).unwrap();
    }
}
