use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{ManagedPaths, NetworkError};

/// Where a validated sing-box executable was discovered.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SingBoxInstallationSource {
    Explicit,
    ManagedCurrent,
    ManagedRoot,
}

/// A validated sing-box executable and its discovery source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SingBoxInstallation {
    executable: PathBuf,
    source: SingBoxInstallationSource,
    #[serde(skip)]
    command_prefix: Vec<OsString>,
}

impl SingBoxInstallation {
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    #[must_use]
    pub const fn source(&self) -> SingBoxInstallationSource {
        self.source
    }

    /// Build a read-only installation for store-level tests that never launches a
    /// sidecar.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self {
            executable: PathBuf::from("sing-box"),
            source: SingBoxInstallationSource::ManagedCurrent,
            command_prefix: Vec::new(),
        }
    }

    pub(crate) fn arguments(&self, command: &str, config: &Path) -> Vec<OsString> {
        let mut arguments = self.command_prefix.clone();
        arguments.extend([
            OsString::from(command),
            OsString::from("-c"),
            config.as_os_str().to_owned(),
        ]);
        arguments
    }
}

/// Resolves an explicit or Guard-managed sing-box runtime without using `PATH`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SingBoxLocator;

impl SingBoxLocator {
    /// Resolve an explicit executable, or the stable managed-runtime entry point.
    ///
    /// # Errors
    ///
    /// Returns a typed configuration error when the selected executable is not
    /// an absolute file or no managed runtime is installed.
    pub fn resolve(
        paths: &ManagedPaths,
        explicit: Option<&Path>,
    ) -> Result<SingBoxInstallation, NetworkError> {
        explicit.map_or_else(
            || Self::discover_managed(paths),
            |path| validate_executable(path.to_path_buf(), SingBoxInstallationSource::Explicit),
        )
    }

    /// Discover the stable managed runtime, preferring the atomic `current` slot.
    ///
    /// # Errors
    ///
    /// Returns a not-found error when neither managed candidate is a file.
    pub fn discover_managed(paths: &ManagedPaths) -> Result<SingBoxInstallation, NetworkError> {
        let root = paths.sing_box_root();
        for (candidate, source) in [
            (
                root.join("current").join(executable_name()),
                SingBoxInstallationSource::ManagedCurrent,
            ),
            (
                root.join(executable_name()),
                SingBoxInstallationSource::ManagedRoot,
            ),
        ] {
            if candidate.is_file() {
                return Ok(SingBoxInstallation {
                    executable: candidate,
                    source,
                    command_prefix: Vec::new(),
                });
            }
        }
        Err(NetworkError::SingBox(format!(
            "sing-box runtime was not found under {}",
            root.display()
        )))
    }
}

fn validate_executable(
    path: PathBuf,
    source: SingBoxInstallationSource,
) -> Result<SingBoxInstallation, NetworkError> {
    if !path.is_absolute() || !path.is_file() {
        return Err(NetworkError::SingBox(format!(
            "sing-box executable is unavailable at {}",
            path.display()
        )));
    }
    Ok(SingBoxInstallation {
        executable: path,
        source,
        command_prefix: Vec::new(),
    })
}

const fn executable_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sing-box.exe"
    } else {
        "sing-box"
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{SingBoxInstallationSource, SingBoxLocator, executable_name};
    use crate::ManagedPaths;

    #[test]
    fn managed_current_slot_has_deterministic_precedence() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        paths.ensure_layout().expect("layout");
        let root = paths.sing_box_root();
        let current = root.join("current").join(executable_name());
        let fallback = root.join(executable_name());
        fs::create_dir_all(current.parent().expect("current parent")).expect("current directory");
        fs::write(&current, b"current").expect("current executable");
        fs::write(&fallback, b"fallback").expect("fallback executable");

        let installation = SingBoxLocator::discover_managed(&paths).expect("managed runtime");

        assert_eq!(installation.executable(), current);
        assert_eq!(
            installation.source(),
            SingBoxInstallationSource::ManagedCurrent
        );
    }

    #[test]
    fn explicit_executable_must_be_absolute_and_present() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        assert!(
            SingBoxLocator::resolve(&paths, Some(std::path::Path::new("sing-box.exe"))).is_err()
        );
    }
}
