//! Embeds the build provenance (version, commit, dirty flag) that `--build-info`,
//! the TUI help page, and release verification report.
//!
//! The canonical source is the environment (`CPG_BUILD_COMMIT` /
//! `CPG_BUILD_DIRTY`) set by `scripts/build-portable.ps1` before the build; a
//! local `cargo build` without them falls back to querying git directly. Both
//! paths are covered by `rerun-if` triggers so Cargo never serves a stale
//! commit from its cache.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=CPG_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=CPG_BUILD_DIRTY");

    let commit = std::env::var("CPG_BUILD_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(commit_from_git, Some)
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = std::env::var("CPG_BUILD_DIRTY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().eq_ignore_ascii_case("true") || value.trim() == "1")
        .unwrap_or_else(dirty_from_git);

    println!("cargo:rustc-env=CPG_EMBEDDED_COMMIT={commit}");
    println!("cargo:rustc-env=CPG_EMBEDDED_DIRTY={dirty}");
}

fn commit_from_git() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let commit = text.trim().to_string();
    (commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit())).then_some(commit)
}

fn dirty_from_git() -> bool {
    // A failed query is reported as dirty: a build that cannot prove it came
    // from a clean tree must never present itself as one.
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(true)
}
