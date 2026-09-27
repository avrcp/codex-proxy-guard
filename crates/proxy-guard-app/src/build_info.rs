//! Build provenance reported by `--build-info` and the TUI help page.

use serde::Serialize;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("CPG_EMBEDDED_COMMIT");

/// Whether the embedded provenance marks the source tree as dirty. A build
/// that could not prove cleanliness reports itself as dirty.
pub fn dirty() -> bool {
    env!("CPG_EMBEDDED_DIRTY") == "true"
}

#[derive(Debug, Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    pub commit: &'static str,
    pub dirty: bool,
    /// Actual running executable path. This is a local diagnostic output only;
    /// machine-readable launch receipts never embed user directory paths.
    pub executable: String,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: VERSION,
        commit: COMMIT,
        dirty: dirty(),
        executable: std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|error| format!("<unresolved: {error}>")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_is_always_populated() {
        let info = build_info();
        assert!(!info.version.is_empty());
        assert!(
            !info.commit.is_empty(),
            "the build script must always embed a commit (or 'unknown')"
        );
    }
}
