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

    /// Verify the pinned executable is present on disk before any launch.
    ///
    /// Resolution itself stays lazy so read-only node and cache views work
    /// before the runtime is installed; every launch path fails closed here
    /// with an error naming the expected location.
    ///
    /// # Errors
    ///
    /// Returns a typed sing-box error when the executable is not a file.
    pub fn ensure_available(&self) -> Result<(), NetworkError> {
        if self.executable.is_file() {
            return Ok(());
        }
        Err(NetworkError::SingBox(format!(
            "sing-box executable is unavailable at {}; install it at that location or set [managed] sing_box_path",
            self.executable.display()
        )))
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
    /// The resolved installation is only the launch candidate: presence on
    /// disk is validated lazily by [`SingBoxInstallation::ensure_available`],
    /// so read-only node and benchmark-cache views work before the runtime is
    /// installed.
    ///
    /// # Errors
    ///
    /// Returns a typed configuration error when an explicit executable is not
    /// an absolute path.
    pub fn resolve(
        paths: &ManagedPaths,
        explicit: Option<&Path>,
    ) -> Result<SingBoxInstallation, NetworkError> {
        explicit.map_or_else(
            || Ok(Self::discover_managed(paths)),
            |path| validate_explicit(path.to_path_buf()),
        )
    }

    /// Discover the stable managed runtime, preferring the atomic `current`
    /// slot, then the runtime root.
    ///
    /// When neither candidate is installed yet, the canonical `current` slot
    /// is still returned; availability is enforced at launch time.
    #[must_use]
    pub fn discover_managed(paths: &ManagedPaths) -> SingBoxInstallation {
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
                return Self::unvalidated(candidate, source);
            }
        }
        Self::unvalidated(
            root.join("current").join(executable_name()),
            SingBoxInstallationSource::ManagedCurrent,
        )
    }

    fn unvalidated(executable: PathBuf, source: SingBoxInstallationSource) -> SingBoxInstallation {
        SingBoxInstallation {
            executable,
            source,
            command_prefix: Vec::new(),
        }
    }
}

fn validate_explicit(path: PathBuf) -> Result<SingBoxInstallation, NetworkError> {
    if !path.is_absolute() {
        return Err(NetworkError::SingBox(format!(
            "configured sing-box executable must be an absolute path: {}",
            path.display()
        )));
    }
    Ok(SingBoxInstallation {
        executable: path,
        source: SingBoxInstallationSource::Explicit,
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

        let installation = SingBoxLocator::discover_managed(&paths);

        assert_eq!(installation.executable(), current);
        assert_eq!(
            installation.source(),
            SingBoxInstallationSource::ManagedCurrent
        );
        assert!(installation.ensure_available().is_ok());
    }

    #[test]
    fn managed_root_is_used_when_the_current_slot_is_empty() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        paths.ensure_layout().expect("layout");
        let fallback = paths.sing_box_root().join(executable_name());
        fs::create_dir_all(paths.sing_box_root()).expect("sing-box root");
        fs::write(&fallback, b"fallback").expect("fallback executable");

        let installation = SingBoxLocator::discover_managed(&paths);

        assert_eq!(installation.executable(), fallback);
        assert_eq!(
            installation.source(),
            SingBoxInstallationSource::ManagedRoot
        );
    }

    #[test]
    fn missing_managed_runtime_resolves_to_the_canonical_current_slot() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        paths.ensure_layout().expect("layout");

        let installation = SingBoxLocator::resolve(&paths, None).expect("candidate");

        assert_eq!(
            installation.executable(),
            paths
                .sing_box_root()
                .join("current")
                .join(executable_name())
        );
        assert!(installation.ensure_available().is_err());
    }

    #[test]
    fn explicit_executable_must_be_absolute() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        assert!(
            SingBoxLocator::resolve(&paths, Some(std::path::Path::new("sing-box.exe"))).is_err()
        );
    }

    #[test]
    fn explicit_executable_availability_is_checked_lazily() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let absolute = temporary.path().join("sing-box.exe");

        let installation =
            SingBoxLocator::resolve(&paths, Some(&absolute)).expect("unvalidated candidate");

        assert!(installation.ensure_available().is_err());
        fs::write(&absolute, b"runtime").expect("runtime executable");
        assert!(installation.ensure_available().is_ok());
    }
}
