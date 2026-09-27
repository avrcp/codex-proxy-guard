//! Managed proxy block inside an authorized Codex Home `.env`.
//!
//! Every entry point here is strictly consent-gated by the caller: without
//! the user's explicit, home-bound confirmation nothing is created, read,
//! modified, or deleted. The module edits exactly one BEGIN/END-marked block
//! and never becomes a general dotenv editor:
//!
//! * every other byte of the file is preserved verbatim — the file is never
//!   re-serialized as a whole, and its full contents are never echoed;
//! * conflicting proxy keys outside the block (including `ALL_PROXY` and
//!   case variants) are reported by key name and never overridden;
//! * duplicate, truncated, or unknown-version Guard blocks refuse edits;
//! * writes go through a same-directory temporary file plus an atomic
//!   replace, with the original content re-verified immediately before;
//! * revocation removes only Guard's own unmodified block, and deletes the
//!   file only when the file consists of nothing but that block.
//!
//! A successfully prepared block is a file fact
//! (`backend_proxy_config_prepared`), never a network verification.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// Marker lines of the managed block. The embedded version makes unknown
/// future blocks refuse automatic edits instead of being misparsed.
pub const BLOCK_BEGIN: &str = "# BEGIN CODEX PROXY GUARD: proxy-v1";
pub const BLOCK_END: &str = "# END CODEX PROXY GUARD: proxy-v1";
/// Any line starting with this prefix is Guard-managed territory; a marker
/// that is not exactly the current version is an unknown block.
const GUARD_MARKER_PREFIX: &str = "# BEGIN CODEX PROXY GUARD:";
/// Keys Guard refuses to shadow when they appear outside its block.
const CONFLICTING_KEYS: [&str; 4] = ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "ALL_PROXY"];
/// Refuse to reason about implausibly large dotenv files.
const MAX_ENV_FILE_BYTES: usize = 1024 * 1024;

/// The three values Guard manages inside its block. No authentication
/// material, no model or account configuration — proxies only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyEnvValues {
    pub http_proxy: String,
    pub https_proxy: String,
    pub no_proxy: String,
}

impl ProxyEnvValues {
    pub fn from_config(config: &proxy_guard_core::GuardConfig) -> Self {
        let url = config.proxy_url();
        Self {
            http_proxy: url.clone(),
            https_proxy: url,
            no_proxy: config.no_proxy_value(),
        }
    }
}

/// What `inspect` found in (or at) the target `.env`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvFileInspection {
    pub exists: bool,
    /// One complete, current-version managed block was found.
    pub managed_block_present: bool,
    /// The block's current values, when a well-formed block exists.
    pub managed_values: Option<ProxyEnvValues>,
    /// Conflicting proxy keys outside the block (key names only).
    pub conflicting_keys: Vec<String>,
    /// The file is exactly Guard's block (nothing else of substance).
    pub file_is_only_managed_block: bool,
}

/// Result of a successful mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareOutcome {
    /// The block was written (created or updated).
    Written,
    /// The exact block was already present; the file was not touched.
    AlreadyCurrent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevokeOutcome {
    /// Guard's unmodified block was removed; other content was preserved.
    Removed,
    /// The exact block was already absent; the file was not touched.
    AlreadyAbsent,
}

struct ManagedBlock {
    values: ProxyEnvValues,
}

enum ParsedEnvFile {
    Absent,
    Present {
        /// Byte-preserving segments split on `\n`; each segment keeps any
        /// trailing `\r` so user CRLF endings survive every reconstruction.
        parts: Vec<String>,
        block: Option<ManagedBlock>,
    },
}

pub fn env_path(home: &Path) -> PathBuf {
    home.join(".env")
}

/// Read-only inspection used by status displays. It never writes, and an
/// unreadable or undecodable file is an error rather than a guess.
pub fn inspect(path: &Path) -> Result<EnvFileInspection, String> {
    let parsed = read_and_parse(path)?;
    Ok(match parsed {
        ParsedEnvFile::Absent => EnvFileInspection {
            exists: false,
            ..EnvFileInspection::default()
        },
        ParsedEnvFile::Present { parts, block } => build_inspection(&parts, block.as_ref()),
    })
}

/// Writes the managed block for an authorized launch. Must only be called
/// after the user's explicit, home-bound consent.
pub fn prepare(path: &Path, values: &ProxyEnvValues) -> Result<PrepareOutcome, String> {
    validate_values(values)?;
    let parsed = read_and_parse(path)?;
    let ParsedEnvFile::Present { parts, block } = parsed else {
        // No file yet: create it containing only the managed block.
        let rendered = render_block(values);
        write_atomically(path, "", &rendered)?;
        return Ok(PrepareOutcome::Written);
    };
    let inspection = build_inspection(&parts, block.as_ref());
    refuse_conflicts(&inspection)?;
    if let Some(block) = &block
        && &block.values == values
    {
        return Ok(PrepareOutcome::AlreadyCurrent);
    }
    let original = join_parts(&parts);
    let updated = replace_or_append_block(&parts, values);
    write_atomically(path, &original, &updated)?;
    Ok(PrepareOutcome::Written)
}

/// Removes Guard's own managed block. A block whose contents no longer match
/// the documented shape (externally edited) is left untouched and reported.
pub fn revoke(path: &Path) -> Result<RevokeOutcome, String> {
    let parsed = read_and_parse(path)?;
    let ParsedEnvFile::Present { parts, block } = parsed else {
        return Ok(RevokeOutcome::AlreadyAbsent);
    };
    let Some(block) = block else {
        return Ok(RevokeOutcome::AlreadyAbsent);
    };
    let inspection = build_inspection(&parts, Some(&block));
    let original = join_parts(&parts);
    let stripped = strip_block(&parts);
    if inspection.file_is_only_managed_block {
        // The file holds nothing but Guard's block; removing the block would
        // leave an empty shell, so remove the file itself.
        fs::remove_file(path).map_err(|error| {
            format!(
                "BACKEND_PROXY_ENV_IO: cannot remove {}: {error}",
                display(path)
            )
        })?;
        return Ok(RevokeOutcome::Removed);
    }
    write_atomically(path, &original, &stripped)?;
    Ok(RevokeOutcome::Removed)
}

fn validate_values(values: &ProxyEnvValues) -> Result<(), String> {
    for (name, value) in [
        ("HTTP_PROXY", &values.http_proxy),
        ("HTTPS_PROXY", &values.https_proxy),
        ("NO_PROXY", &values.no_proxy),
    ] {
        if value.is_empty() || value.len() > 4096 || value.contains(['\r', '\n', '\0']) {
            return Err(format!(
                "BACKEND_PROXY_ENV_INVALID: {name} value is empty, oversized, or contains \
                 control characters"
            ));
        }
    }
    if values.http_proxy != values.https_proxy {
        return Err(
            "BACKEND_PROXY_ENV_INVALID: HTTP_PROXY and HTTPS_PROXY must be the same endpoint"
                .into(),
        );
    }
    Ok(())
}

/// The visible text of one split segment: identical to the raw segment except
/// that a single CRLF terminator's `\r` is dropped for comparisons only.
fn line_text(part: &str) -> &str {
    part.strip_suffix('\r').unwrap_or(part)
}

fn split_parts(content: &str) -> Vec<String> {
    content.split('\n').map(str::to_string).collect()
}

fn join_parts(parts: &[String]) -> String {
    parts.join("\n")
}

fn read_and_parse(path: &Path) -> Result<ParsedEnvFile, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ParsedEnvFile::Absent);
        }
        Err(error) => {
            return Err(format!(
                "BACKEND_PROXY_ENV_IO: cannot read {}: {error}",
                display(path)
            ));
        }
    };
    if bytes.len() > MAX_ENV_FILE_BYTES {
        return Err(format!(
            "BACKEND_PROXY_ENV_IO: {} is larger than 1 MiB; refusing automatic edits",
            display(path)
        ));
    }
    let content = String::from_utf8(bytes).map_err(|_| {
        format!(
            "BACKEND_PROXY_ENV_ENCODING: {} is not valid UTF-8; refusing automatic edits",
            display(path)
        )
    })?;
    let parts = split_parts(&content);
    let block = parse_block(&parts)?;
    Ok(ParsedEnvFile::Present { parts, block })
}

/// Finds and validates the managed block. Structural problems — duplicate
/// begin markers, a begin without end, an end before begin, or a Guard marker
/// with an unknown version — refuse every edit.
fn parse_block(parts: &[String]) -> Result<Option<ManagedBlock>, String> {
    let mut begin_index = None;
    let mut end_index = None;
    for (index, part) in parts.iter().enumerate() {
        let line = line_text(part);
        if line == BLOCK_BEGIN {
            if begin_index.is_some() {
                return Err("BACKEND_PROXY_BLOCK_INVALID: multiple managed blocks found".into());
            }
            begin_index = Some(index);
        } else if line == BLOCK_END {
            if end_index.is_some() {
                return Err(
                    "BACKEND_PROXY_BLOCK_INVALID: multiple managed block ends found".into(),
                );
            }
            end_index = Some(index);
        } else if line.starts_with(GUARD_MARKER_PREFIX) {
            return Err(format!(
                "BACKEND_PROXY_BLOCK_INVALID: unknown Guard block version: {line}"
            ));
        }
    }
    match (begin_index, end_index) {
        (None, None) => Ok(None),
        (Some(begin), Some(end)) if end > begin => {
            let body: Vec<&str> = parts[begin + 1..end]
                .iter()
                .map(|part| line_text(part))
                .collect();
            let values = parse_block_body(&body)?;
            Ok(Some(ManagedBlock { values }))
        }
        (Some(_), None) | (None, Some(_)) => {
            Err("BACKEND_PROXY_BLOCK_INVALID: the managed block is truncated".into())
        }
        (Some(_), Some(_)) => Err("BACKEND_PROXY_BLOCK_INVALID: block end precedes begin".into()),
    }
}

/// The block body must consist of exactly the three managed keys, each once,
/// in any order, with plain `KEY=value` lines. Anything else means someone
/// edited Guard's block and it is no longer Guard's to touch blindly.
fn parse_block_body(body: &[&str]) -> Result<ProxyEnvValues, String> {
    let mut http_proxy = None;
    let mut https_proxy = None;
    let mut no_proxy = None;
    for line in body {
        if line.trim().is_empty() {
            return Err("BACKEND_PROXY_BLOCK_INVALID: blank lines inside the managed block".into());
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(
                "BACKEND_PROXY_BLOCK_INVALID: unrecognized line inside the managed block".into(),
            );
        };
        match key {
            "HTTP_PROXY" if http_proxy.is_none() => http_proxy = Some(value.to_string()),
            "HTTPS_PROXY" if https_proxy.is_none() => https_proxy = Some(value.to_string()),
            "NO_PROXY" if no_proxy.is_none() => no_proxy = Some(value.to_string()),
            other => {
                return Err(format!(
                    "BACKEND_PROXY_BLOCK_INVALID: unexpected or duplicate key {other:?} inside \
                     the managed block"
                ));
            }
        }
    }
    match (http_proxy, https_proxy, no_proxy) {
        (Some(http_proxy), Some(https_proxy), Some(no_proxy)) => Ok(ProxyEnvValues {
            http_proxy,
            https_proxy,
            no_proxy,
        }),
        _ => Err(
            "BACKEND_PROXY_BLOCK_INVALID: the managed block is missing one of HTTP_PROXY, \
             HTTPS_PROXY, NO_PROXY"
                .into(),
        ),
    }
}

fn build_inspection(parts: &[String], block: Option<&ManagedBlock>) -> EnvFileInspection {
    let mut conflicting_keys: Vec<String> = Vec::new();
    let mut inside = false;
    for part in parts {
        let line = line_text(part);
        if line == BLOCK_BEGIN {
            inside = true;
            continue;
        }
        if line == BLOCK_END {
            inside = false;
            continue;
        }
        if inside {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim();
            if CONFLICTING_KEYS
                .iter()
                .any(|managed| key.eq_ignore_ascii_case(managed))
                && !conflicting_keys
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(key))
            {
                // Report the key as written; never the value.
                conflicting_keys.push(key.to_string());
            }
        }
    }
    let stripped = strip_block(parts);
    let file_is_only_managed_block =
        block.is_some() && stripped.split('\n').all(|line| line.trim().is_empty());
    EnvFileInspection {
        exists: true,
        managed_block_present: block.is_some(),
        managed_values: block.map(|block| block.values.clone()),
        conflicting_keys,
        file_is_only_managed_block,
    }
}

fn refuse_conflicts(inspection: &EnvFileInspection) -> Result<(), String> {
    if inspection.conflicting_keys.is_empty() {
        return Ok(());
    }
    Err(format!(
        "BACKEND_PROXY_CONFIG_CONFLICT: proxy keys {} are already set outside Guard's block; \
         resolve them manually (Guard never overrides existing entries)",
        inspection.conflicting_keys.join(", ")
    ))
}

fn render_block(values: &ProxyEnvValues) -> String {
    format!(
        "{BLOCK_BEGIN}\nHTTP_PROXY={}\nHTTPS_PROXY={}\nNO_PROXY={}\n{BLOCK_END}\n",
        values.http_proxy, values.https_proxy, values.no_proxy
    )
}

fn block_parts(values: &ProxyEnvValues) -> Vec<String> {
    split_parts(&render_block(values))
}

/// Replaces the existing block in place, or appends one after the existing
/// content. Segments outside the block are reused verbatim, including any
/// CRLF endings and the presence or absence of a trailing newline.
fn replace_or_append_block(parts: &[String], values: &ProxyEnvValues) -> String {
    let block = block_parts(values);
    let begin = parts.iter().position(|part| line_text(part) == BLOCK_BEGIN);
    if let Some(begin) = begin {
        let end = parts
            .iter()
            .position(|part| line_text(part) == BLOCK_END)
            .unwrap_or(begin);
        let mut updated: Vec<String> = Vec::with_capacity(parts.len());
        updated.extend_from_slice(&parts[..begin]);
        // Guard's own block is rewritten in canonical form when it is
        // structurally valid (a malformed one never reaches this function).
        updated.extend(block.iter().cloned());
        updated.extend_from_slice(&parts[end + 1..]);
        return join_parts(&updated);
    }
    let mut updated: Vec<String> = parts.to_vec();
    match updated.last() {
        // No trailing newline yet: add one separator line before the block.
        Some(last) if !last.is_empty() => updated.push(String::new()),
        // Content already ends with a newline: the block continues directly
        // after it instead of introducing a blank line.
        Some(_) => {
            updated.pop();
        }
        None => updated.push(String::new()),
    }
    updated.extend(block.iter().cloned());
    join_parts(&updated)
}

fn strip_block(parts: &[String]) -> String {
    let mut inside = false;
    let mut kept: Vec<&str> = Vec::new();
    for part in parts {
        let line = line_text(part);
        if line == BLOCK_BEGIN {
            inside = true;
            continue;
        }
        if line == BLOCK_END {
            inside = false;
            continue;
        }
        if inside {
            continue;
        }
        kept.push(part);
    }
    kept.join("\n")
}

/// Atomic replace through a same-directory temporary file. The original
/// content is re-read immediately before the rename and must be unchanged,
/// otherwise the edit is abandoned: another writer won the race.
fn write_atomically(path: &Path, expected: &str, updated: &str) -> Result<(), String> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory).map_err(|error| {
        format!(
            "BACKEND_PROXY_ENV_IO: cannot create {}: {error}",
            display(directory)
        )
    })?;
    let temp = directory.join(format!(
        ".cpg-proxy-env-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    // The exclusive create refuses to clobber anything another process (or a
    // previous crashed run) left behind at this exact unique name.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            format!("BACKEND_PROXY_ENV_IO: cannot create the temporary file: {error}")
        })?;
    if let Err(error) = file
        .write_all(updated.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "BACKEND_PROXY_ENV_IO: cannot write the block: {error}"
        ));
    }
    drop(file);
    // Re-verify the original has not changed since it was read.
    let current = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "BACKEND_PROXY_ENV_IO: cannot re-verify {}: {error}",
                display(path)
            ));
        }
    };
    if current != expected.as_bytes() {
        let _ = fs::remove_file(&temp);
        return Err(
            "BACKEND_PROXY_CONFIG_CONFLICT: the .env file changed while Guard was preparing \
             the block; no changes were applied"
                .into(),
        );
    }
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(format!(
                "BACKEND_PROXY_ENV_IO: cannot replace {}: {error}",
                display(path)
            ))
        }
    }
}

fn display(path: &Path) -> String {
    proxy_guard_core::display_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cpg-proxy-env-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn values(port: u16) -> ProxyEnvValues {
        ProxyEnvValues {
            http_proxy: format!("http://127.0.0.1:{port}"),
            https_proxy: format!("http://127.0.0.1:{port}"),
            no_proxy: "localhost,127.0.0.1,::1".into(),
        }
    }

    fn read(path: &Path) -> String {
        String::from_utf8(fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn prepare_creates_the_block_in_a_new_file() {
        let dir = temp_dir();
        let path = dir.join(".env");
        assert_eq!(prepare(&path, &values(10808)), Ok(PrepareOutcome::Written));
        assert_eq!(
            read(&path),
            "# BEGIN CODEX PROXY GUARD: proxy-v1\n\
             HTTP_PROXY=http://127.0.0.1:10808\n\
             HTTPS_PROXY=http://127.0.0.1:10808\n\
             NO_PROXY=localhost,127.0.0.1,::1\n\
             # END CODEX PROXY GUARD: proxy-v1\n"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepare_is_idempotent_and_updates_only_guard_values() {
        let dir = temp_dir();
        let path = dir.join(".env");
        fs::write(&path, "OPENAI_API_BASE=https://example.test\n# a comment\n").unwrap();
        assert_eq!(prepare(&path, &values(10808)), Ok(PrepareOutcome::Written));
        let once = read(&path);
        assert!(once.starts_with("OPENAI_API_BASE=https://example.test\n# a comment\n"));
        assert!(once.contains("HTTP_PROXY=http://127.0.0.1:10808\n"));
        // Re-preparing with the same values must not even touch the file.
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            prepare(&path, &values(10808)),
            Ok(PrepareOutcome::AlreadyCurrent)
        );
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            before,
            "an already-current block must not rewrite the file"
        );
        // Changing the port updates only the block.
        assert_eq!(prepare(&path, &values(7890)), Ok(PrepareOutcome::Written));
        let updated = read(&path);
        assert!(updated.contains("HTTP_PROXY=http://127.0.0.1:7890\n"));
        assert!(!updated.contains("10808"));
        assert!(updated.contains("OPENAI_API_BASE=https://example.test\n"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn other_bytes_are_preserved_verbatim() {
        let dir = temp_dir();
        let path = dir.join(".env");
        let original = "A=1\r\n\r\nB two words =x\n\nexport C=3\r\n";
        fs::write(&path, original).unwrap();
        prepare(&path, &values(10808)).unwrap();
        let content = read(&path);
        let stripped = strip_block(&split_parts(&content));
        assert!(
            stripped.starts_with("A=1\r\n"),
            "user CRLF endings survive byte-for-byte: {stripped:?}"
        );
        assert!(
            stripped.contains("export C=3\r\n"),
            "trailing CRLF content is preserved: {stripped:?}"
        );
        // Revoking restores the original bytes exactly.
        revoke(&path).unwrap();
        assert_eq!(read(&path), original);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn conflicting_keys_outside_the_block_refuse_edits_by_name_only() {
        let dir = temp_dir();
        let path = dir.join(".env");
        let original = "http_proxy=socks5://127.0.0.1:1080\nALL_PROXY=socks5://x\n";
        fs::write(&path, original).unwrap();
        let error = prepare(&path, &values(10808)).unwrap_err();
        assert!(
            error.starts_with("BACKEND_PROXY_CONFIG_CONFLICT:"),
            "{error}"
        );
        assert!(error.contains("http_proxy") && error.contains("ALL_PROXY"));
        assert!(
            !error.contains("socks5"),
            "conflicting values must not be echoed: {error}"
        );
        assert_eq!(read(&path), original);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn duplicate_or_truncated_or_unknown_blocks_refuse_edits() {
        let dir = temp_dir();
        let block = render_block(&values(10808));
        let path = dir.join("dup.env");
        fs::write(&path, format!("{block}{block}")).unwrap();
        assert!(
            prepare(&path, &values(10808))
                .unwrap_err()
                .starts_with("BACKEND_PROXY_BLOCK_INVALID:")
        );
        let path = dir.join("truncated.env");
        fs::write(&path, format!("{BLOCK_BEGIN}\nHTTP_PROXY=x\n")).unwrap();
        assert!(
            prepare(&path, &values(10808))
                .unwrap_err()
                .contains("truncated")
        );
        let path = dir.join("future.env");
        fs::write(
            &path,
            "# BEGIN CODEX PROXY GUARD: proxy-v2\nX=1\n# END CODEX PROXY GUARD: proxy-v2\n",
        )
        .unwrap();
        assert!(
            prepare(&path, &values(10808))
                .unwrap_err()
                .contains("unknown Guard block version")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn externally_edited_block_shapes_refuse_automatic_edits() {
        let dir = temp_dir();
        let path = dir.join(".env");
        let original =
            format!("{BLOCK_BEGIN}\nHTTP_PROXY=http://127.0.0.1:1\nEXTRA=2\n{BLOCK_END}\n");
        fs::write(&path, &original).unwrap();
        assert!(
            prepare(&path, &values(10808))
                .unwrap_err()
                .contains("unexpected or duplicate key")
        );
        assert!(
            revoke(&path)
                .unwrap_err()
                .contains("unexpected or duplicate key"),
            "revocation must not tear out a block someone else edited"
        );
        assert_eq!(read(&path), original);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_utf8_and_oversized_files_refuse_edits() {
        let dir = temp_dir();
        let path = dir.join("binary.env");
        fs::write(&path, [0xFF_u8, 0xFE, 0x00, 0x01]).unwrap();
        assert!(
            prepare(&path, &values(10808))
                .unwrap_err()
                .starts_with("BACKEND_PROXY_ENV_ENCODING:")
        );
        let path = dir.join("huge.env");
        fs::write(&path, vec![b'A'; MAX_ENV_FILE_BYTES + 1]).unwrap();
        assert!(
            prepare(&path, &values(10808))
                .unwrap_err()
                .contains("1 MiB")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn revoke_removes_only_guard_block_and_deletes_guard_created_files() {
        let dir = temp_dir();
        // File Guard created itself: revoke deletes the whole file.
        let path = dir.join("guard-created.env");
        prepare(&path, &values(10808)).unwrap();
        assert_eq!(revoke(&path), Ok(RevokeOutcome::Removed));
        assert!(!path.exists(), "a Guard-created file must not linger");

        // File with other content: only the block disappears.
        let path = dir.join("shared.env");
        fs::write(&path, "KEEP=1\n").unwrap();
        prepare(&path, &values(10808)).unwrap();
        assert_eq!(revoke(&path), Ok(RevokeOutcome::Removed));
        assert_eq!(read(&path), "KEEP=1\n");

        // No block at all: nothing happens.
        let path = dir.join("plain.env");
        fs::write(&path, "KEEP=1\n").unwrap();
        assert_eq!(revoke(&path), Ok(RevokeOutcome::AlreadyAbsent));
        assert_eq!(read(&path), "KEEP=1\n");

        // Missing file: absent.
        assert_eq!(
            revoke(&dir.join("missing.env")),
            Ok(RevokeOutcome::AlreadyAbsent)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn inspect_reports_state_without_touching_the_file() {
        let dir = temp_dir();
        let path = dir.join(".env");
        let missing = inspect(&path).unwrap();
        assert!(!missing.exists && !missing.managed_block_present);
        fs::write(&path, "HTTPS_PROXY=http://elsewhere:1\n").unwrap();
        let conflict = inspect(&path).unwrap();
        assert_eq!(conflict.conflicting_keys, ["HTTPS_PROXY"]);
        assert!(conflict.managed_values.is_none());
        assert!(
            prepare(&path, &values(10808))
                .unwrap_err()
                .starts_with("BACKEND_PROXY_CONFIG_CONFLICT:")
        );
        fs::write(&path, render_block(&values(10808))).unwrap();
        let present = inspect(&path).unwrap();
        assert!(present.managed_block_present);
        assert_eq!(present.managed_values, Some(values(10808)));
        assert!(present.file_is_only_managed_block);
        assert!(present.conflicting_keys.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn values_must_be_a_matching_loopback_pair() {
        let mut bad = values(10808);
        bad.https_proxy = "http://127.0.0.1:9999".into();
        assert!(
            prepare(Path::new("unused.env"), &bad)
                .unwrap_err()
                .contains("same endpoint")
        );
        let mut empty = values(10808);
        empty.no_proxy = String::new();
        assert!(
            prepare(Path::new("unused.env"), &empty)
                .unwrap_err()
                .starts_with("BACKEND_PROXY_ENV_INVALID:")
        );
    }
}
