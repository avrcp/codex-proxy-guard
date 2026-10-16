//! Configuration storage and the consented Codex Home `.env` block — the
//! port of the C++ `core.cpp`. Every mutation holds an exclusive file
//! transaction; `.env` edits run inside TxF with expected-bytes re-read;
//! consent state changes preserve the documented ordering.

use cpg_domain::config::{CONFIG_LIMIT, Config, may_contain_consent};
use cpg_domain::dotenv::{
    ENV_LIMIT, EnvFile, current_values, parse_env, prepare_updated_bytes, revoke_updated_bytes,
};
use cpg_domain::{Error, Result};
use cpg_windows::fsio::{create_file, read_raw, write_raw};
use cpg_windows::handles::OwnedHandle;
use cpg_windows::{paths, txf};
use std::path::{Path, PathBuf};
use windows::Win32::Storage::FileSystem::{
    CREATE_NEW, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, OPEN_EXISTING,
};

pub fn default_config_path() -> Result<PathBuf> {
    paths::default_config_path()
}

pub fn default_home() -> Result<PathBuf> {
    paths::default_home()
}

const GENERIC_READ_WRITE: u32 = FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0;

fn ensure_parent(path: &Path, code: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| Error::new(code, "Cannot create the destination directory."))?;
    }
    Ok(())
}

pub fn load_config(path: &Path) -> Result<Config> {
    let handle = create_file(path, FILE_GENERIC_READ.0, FILE_SHARE_READ, OPEN_EXISTING)
        .map_err(|_| Error::new("CONFIG_READ_FAILED", "Cannot read the Guard configuration."))?;
    let bytes = read_raw(handle.raw(), CONFIG_LIMIT, "CONFIG_READ_FAILED")?;
    Config::parse(&bytes)
}

pub fn load_or_create_config(path: &Path) -> Result<Config> {
    if path.exists() {
        return load_config(path);
    }
    match initialize_config(path, &Config::default(), false) {
        Ok(()) => load_config(path),
        Err(error) if path.exists() => load_config(path).map_err(|_| error),
        Err(error) => Err(error),
    }
}

fn try_config(bytes: &[u8]) -> Option<Config> {
    Config::parse(bytes).ok()
}

/// The held write transaction over `config.toml`: exclusive open (share
/// read), expected-content comparison, whole rewrite with rollback.
struct Transaction {
    handle: OwnedHandle,
    original: Vec<u8>,
}

impl Transaction {
    fn open(
        path: &Path,
        expected: Option<&Config>,
        invalid: bool,
        create: bool,
    ) -> Result<Transaction> {
        if create || invalid {
            ensure_parent(path, "CONFIG_LOCK_FAILED")?;
        }
        let mut created = create;
        let handle = if create {
            create_file(path, GENERIC_READ_WRITE, FILE_SHARE_READ, CREATE_NEW)
        } else {
            create_file(path, GENERIC_READ_WRITE, FILE_SHARE_READ, OPEN_EXISTING)
        };
        let handle = match handle {
            Ok(handle) => Some(handle),
            Err(error) if !create && invalid && error.code == "CONFIG_LOCK_FAILED" => {
                // Missing path during invalid-config recovery: recreate it.
                match create_file(path, GENERIC_READ_WRITE, FILE_SHARE_READ, CREATE_NEW) {
                    Ok(handle) => {
                        created = true;
                        Some(handle)
                    }
                    Err(_) => None,
                }
            }
            Err(_) => None,
        };
        let Some(handle) = handle else {
            return Err(Error::new(
                "CONFIG_LOCK_FAILED",
                "Configuration is busy, missing, or not writable.",
            ));
        };
        let original = read_raw(handle.raw(), CONFIG_LIMIT, "CONFIG_READ_FAILED")?;
        let current = try_config(&original);
        if !created
            && ((invalid && current.is_some())
                || expected.is_some_and(|expected| current.as_ref() != Some(expected)))
        {
            return Err(Error::new(
                "CONFIG_CHANGED",
                "Refresh and confirm the configuration again.",
            ));
        }
        if !created && current.is_none() && may_contain_consent(&original) {
            return Err(Error::new(
                "CONFIG_CONSENT_REPAIR_REQUIRED",
                "The invalid configuration contains proxy authorization fields; repair it manually while preserving the bound Home before resetting or saving.",
            ));
        }
        Ok(Transaction { handle, original })
    }

    fn commit(&self, config: &Config) -> Result<()> {
        let bytes = config.to_toml()?;
        if bytes.len() > CONFIG_LIMIT {
            return Err(Error::new(
                "CONFIG_TOO_LARGE",
                "Configuration exceeds 64 KiB.",
            ));
        }
        if !write_raw(self.handle.raw(), bytes.as_bytes()) {
            if !write_raw(self.handle.raw(), &self.original) {
                return Err(Error::new(
                    "CONFIG_ROLLBACK_FAILED",
                    "The device rejected both the write and restoration; inspect the Guard configuration.",
                ));
            }
            return Err(Error::new(
                "CONFIG_WRITE_FAILED",
                "The original configuration was restored.",
            ));
        }
        Ok(())
    }
}

/// Opaque held read lease over the configuration for the duration of a
/// launch; a mismatch aborts before any side effect.
pub struct ConfigLease {
    _handle: OwnedHandle,
}

impl ConfigLease {
    pub fn new(path: &Path, expected: &Config) -> Result<ConfigLease> {
        let handle = create_file(path, FILE_GENERIC_READ.0, FILE_SHARE_READ, OPEN_EXISTING)
            .map_err(|_| {
                Error::new(
                    "CONFIG_LOCK_FAILED",
                    "Configuration is changing or unavailable.",
                )
            })?;
        let bytes = read_raw(handle.raw(), CONFIG_LIMIT, "CONFIG_READ_FAILED")?;
        let current = try_config(&bytes);
        if current.as_ref() != Some(expected) {
            return Err(Error::new(
                "CONFIG_CHANGED",
                "Refresh before launching with the new configuration.",
            ));
        }
        Ok(ConfigLease { _handle: handle })
    }
}

pub fn initialize_config(path: &Path, config: &Config, force: bool) -> Result<()> {
    config.validate()?;
    if !force || !path.exists() {
        return Transaction::open(path, None, false, true)?.commit(config);
    }
    // Force initialization still holds the same write/delete exclusion and
    // must never silently discard an existing authorization binding.
    let transaction = Transaction::open(path, None, false, false)?;
    if let Some(current) = try_config(&transaction.original)
        && current.manage_backend
        && (current.home != config.home || !config.manage_backend)
    {
        return Err(Error::new(
            "BACKEND_PROXY_REVOKE_REQUIRED",
            "Revoke the existing proxy authorization before resetting this configuration.",
        ));
    }
    transaction.commit(config)
}

pub fn update_proxy(
    path: &Path,
    expected: &Config,
    invalid: bool,
    host: &str,
    port: i64,
) -> Result<Config> {
    let transaction = Transaction::open(
        path,
        if invalid { None } else { Some(expected) },
        invalid,
        false,
    )?;
    let mut updated = expected.clone();
    updated.host = host.to_owned();
    updated.port = port;
    transaction.commit(&updated)?;
    Ok(updated)
}

/// `true` for a drive-rooted or UNC absolute Windows path; extended-length
/// prefixes from persisted canonicalization are accepted, device namespaces
/// refused. Port of `absoluteHome`.
pub fn absolute_home(home: &Path) -> bool {
    let text = home.to_string_lossy();
    let wide: &str = &text;
    if wide.is_empty() || wide.len() >= 4096 {
        return false;
    }
    if wide.contains(['\0', '\r', '\n']) {
        return false;
    }
    let mut normalized = wide.replace('\\', "/");
    let lower = |text: &str| text.to_lowercase();
    if normalized.len() > 8 && lower(&normalized[..8]) == "//?/unc/" {
        normalized = format!("//{}", &normalized[8..]);
    } else if normalized.len() > 4 && lower(&normalized[..4]) == "//?/" {
        normalized = normalized[4..].to_owned();
    }
    if normalized.len() > 4 && lower(&normalized[..4]) == "//./" {
        return false;
    }
    let bytes = normalized.as_bytes();
    let drive = normalized.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'/';
    if drive {
        return true;
    }
    if !normalized.starts_with("//") {
        return false;
    }
    // //server/share...: two non-empty segments before any further content.
    let rest = &normalized[2..];
    let mut parts = rest.split('/');
    let server = parts.next().unwrap_or_default();
    let share = parts.next().unwrap_or_default();
    !server.is_empty() && !share.is_empty()
}

/// The Home bound by an active authorization.
pub fn bound_home(config: &Config) -> Result<PathBuf> {
    if !config.manage_backend || !absolute_home(&config.home) {
        return Err(Error::new(
            "BACKEND_PROXY_SCOPE_UNCONFIRMED",
            "An absolute Codex Home must be explicitly authorized first.",
        ));
    }
    Ok(config.home.clone())
}

/// The candidate Home shown in consent proposals (default when unset).
pub fn consent_home(config: &Config) -> Result<PathBuf> {
    let home = if config.home.as_os_str().is_empty() {
        default_home()?
    } else {
        config.home.clone()
    };
    if !absolute_home(&home) {
        return Err(Error::new(
            "BACKEND_PROXY_SCOPE_UNCONFIRMED",
            "The Codex Home must be an absolute path.",
        ));
    }
    Ok(home)
}

fn read_env_file(path: &Path) -> Result<EnvFile> {
    const NOT_FOUND: u32 = 2; // ERROR_FILE_NOT_FOUND
    const PATH_NOT_FOUND: u32 = 3; // ERROR_PATH_NOT_FOUND
    let handle = match cpg_windows::fsio::try_open_read_shared(path) {
        Ok(handle) => Some(handle),
        Err(NOT_FOUND | PATH_NOT_FOUND) => None,
        Err(_) => {
            return Err(Error::new(
                "BACKEND_PROXY_ENV_IO",
                "Cannot read the authorized Home proxy file.",
            ));
        }
    };
    let Some(handle) = handle else {
        return Ok(EnvFile::default());
    };
    let bytes = read_raw(handle.raw(), ENV_LIMIT, "BACKEND_PROXY_ENV_IO")?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| {
            Error::new(
                "BACKEND_PROXY_ENV_ENCODING",
                "The proxy file is not valid UTF-8.",
            )
        })?
        .to_owned();
    let mut env = parse_env(&text)?;
    env.exists = true;
    Ok(env)
}

/// TxF-guarded atomic `.env` edit. Port of `atomicEnv`.
fn atomic_env(path: &Path, expected: &EnvFile, updated: &str, remove_if_empty: bool) -> Result<()> {
    if updated.len() > ENV_LIMIT {
        return Err(Error::new(
            "BACKEND_PROXY_ENV_IO",
            "The updated proxy file exceeds 1 MiB.",
        ));
    }
    ensure_parent(path, "BACKEND_PROXY_ENV_IO")?;
    let transaction = txf::Transaction::new()?;
    let file = match txf::create_file_transacted(path, expected.exists, &transaction)? {
        txf::TransactedOpen::File(file) => file,
        txf::TransactedOpen::Conflict => {
            return Err(Error::new(
                "BACKEND_PROXY_CONFIG_CONFLICT",
                "The proxy file changed; no changes were applied.",
            ));
        }
        txf::TransactedOpen::Unavailable => {
            return Err(Error::new(
                "BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE",
                "This location does not support the required Windows file transactions; the proxy file was not changed.",
            ));
        }
    };
    // A transacted writer locks both data and namespace identity — including
    // against non-transacted rename-and-save editors — for the lifetime of
    // the transaction. Re-read inside the same transaction.
    let observed = read_raw(file.raw(), ENV_LIMIT, "BACKEND_PROXY_ENV_IO")?;
    if observed != expected.bytes.as_bytes() {
        return Err(Error::new(
            "BACKEND_PROXY_CONFIG_CONFLICT",
            "The proxy file changed; no changes were applied.",
        ));
    }
    if remove_if_empty && updated.is_empty() {
        txf::delete_file_transacted(path, &transaction)?;
    } else if !write_raw(file.raw(), updated.as_bytes()) {
        return Err(Error::new(
            "BACKEND_PROXY_ENV_IO",
            "Cannot stage the proxy file transaction.",
        ));
    }
    drop(file);
    transaction.commit()
}

pub fn update_consent(
    path: &Path,
    expected: &Config,
    enable: bool,
    confirmed_home: &Path,
) -> Result<Config> {
    let transaction = Transaction::open(path, Some(expected), false, false)?;
    if !absolute_home(confirmed_home) || consent_home(expected)? != confirmed_home {
        return Err(Error::new(
            "BACKEND_PROXY_SCOPE_UNCONFIRMED",
            "The confirmed Home no longer matches the configuration.",
        ));
    }
    let mut updated = expected.clone();
    if enable {
        if expected.manage_backend {
            return Err(Error::new(
                "CONFIG_CHANGED",
                "Proxy management is already authorized; refresh before confirming.",
            ));
        }
        let canonical = paths::canonical_path_of(confirmed_home, true);
        updated.home = canonical.unwrap_or_else(|| confirmed_home.to_path_buf());
        updated.manage_backend = true;
    } else {
        if !expected.manage_backend || expected.home != confirmed_home {
            return Err(Error::new(
                "CONFIG_CHANGED",
                "Proxy management is no longer authorized for the confirmed Home.",
            ));
        }
        if let Err(error) = revoke_proxy_env(confirmed_home) {
            const RETRYABLE: &[&str] = &[
                "BACKEND_PROXY_ENV_IO",
                "BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE",
                "BACKEND_PROXY_CONFIG_CONFLICT",
                "BACKEND_PROXY_BLOCK_INVALID",
                "BACKEND_PROXY_ENV_ENCODING",
                "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED",
                "BACKEND_PROXY_SCOPE_UNCONFIRMED",
            ];
            let reason = if RETRYABLE.contains(&error.code.as_str()) {
                error.code
            } else {
                "BACKEND_PROXY_ENV_FAILURE".to_owned()
            };
            return Err(Error::new(
                "BACKEND_PROXY_REVOKE_FAILED",
                format!(
                    "The managed block was not safely removed; consent and its bound Home remain unchanged. Cause: {reason}."
                ),
            ));
        }
        updated.manage_backend = false;
        updated.home = PathBuf::new();
    }
    if let Err(error) = transaction.commit(&updated) {
        if !enable && error.code != "CONFIG_ROLLBACK_FAILED" {
            return Err(Error::new(
                "BACKEND_PROXY_CONSENT_SAVE_FAILED",
                "The block was removed but consent could not be saved; refresh and retry.",
            ));
        }
        return Err(error);
    }
    Ok(updated)
}

pub fn inspect_proxy_env(config: &Config) -> &'static str {
    if !config.manage_backend {
        return "not_authorized";
    }
    let attempt = || -> Result<&'static str> {
        config.validate()?;
        let env = read_env_file(&bound_home(config)?.join(".env"))?;
        if env.conflict {
            return Ok("conflict");
        }
        if !env.has_block() {
            return Ok("pending");
        }
        Ok(if current_values(&env, config) {
            "current"
        } else {
            "stale"
        })
    };
    match attempt() {
        Ok(state) => state,
        Err(error) if error.code == "BACKEND_PROXY_ENV_IO" => "unavailable",
        Err(error) if error.code == "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED" => "unsupported",
        Err(_) => "invalid",
    }
}

pub fn prepare_proxy_env(config: &Config) -> Result<()> {
    config.validate()?;
    let path = bound_home(config)?.join(".env");
    let env = read_env_file(&path)?;
    if env.conflict {
        return Err(Error::new(
            "BACKEND_PROXY_CONFIG_CONFLICT",
            "Proxy keys already exist outside Guard's block; resolve them manually.",
        ));
    }
    let Some(updated) = prepare_updated_bytes(&env, config) else {
        return Ok(());
    };
    atomic_env(&path, &env, &updated, false)
}

pub fn revoke_proxy_env(home: &Path) -> Result<()> {
    if !absolute_home(home) {
        return Err(Error::new(
            "BACKEND_PROXY_SCOPE_UNCONFIRMED",
            "The Codex Home must be an absolute path.",
        ));
    }
    let path = home.join(".env");
    let env = read_env_file(&path)?;
    if !env.has_block() {
        return Ok(());
    }
    let updated = revoke_updated_bytes(&env).unwrap_or_default();
    atomic_env(&path, &env, &updated, updated.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpg_domain::dotenv;
    use cpg_domain::dotenv::managed_block;
    use std::fs;
    use std::io::Write as _;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cpg-engine-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn absolute_home_forms() {
        assert!(absolute_home(Path::new(r"C:\Users\x\.codex")));
        assert!(absolute_home(Path::new(r"\\server\share\codex")));
        assert!(absolute_home(Path::new(r"\\?\C:\a\b")));
        assert!(absolute_home(Path::new(r"\\?\UNC\s\share\b")));
        assert!(!absolute_home(Path::new(r"C:relative\path")));
        assert!(!absolute_home(Path::new(r"\\.\PhysicalDrive0")));
        assert!(!absolute_home(Path::new("relative")));
        assert!(!absolute_home(Path::new("")));
        assert!(!absolute_home(Path::new(r"\\server")));
        // Deeper UNC content under the server\share root is still absolute.
        assert!(absolute_home(Path::new(r"\\server\share2\extra")));
        assert!(absolute_home(Path::new(r"\\server\share")));
    }

    #[test]
    fn init_roundtrip_and_update_semantics() {
        let dir = temp_dir("config");
        let path = dir.join("config.toml");
        initialize_config(&path, &Config::default(), false).expect("init");
        let config = load_config(&path).expect("load");
        assert_eq!(config, Config::default());

        // Concurrent expectation mismatch.
        let stale = config.clone();
        let updated = update_proxy(&path, &config, false, "localhost", 8080).expect("update");
        assert_eq!(updated.host, "localhost");
        let error = update_proxy(&path, &stale, false, "127.0.0.1", 1).unwrap_err();
        assert_eq!(error.code, "CONFIG_CHANGED");

        // A broken file without consent evidence is recovered by the edit;
        // with consent evidence it must be refused whole-file.
        fs::write(&path, "broken toml [\n").expect("break");
        update_proxy(&path, &Config::default(), true, "127.0.0.1", 1).expect("invalid recovery");
        fs::write(&path, "manage_codex_proxy_env = true\ngarbage\n").expect("break with consent");
        let error = update_proxy(&path, &Config::default(), true, "127.0.0.1", 1).unwrap_err();
        assert_eq!(error.code, "CONFIG_CONSENT_REPAIR_REQUIRED");
    }

    #[test]
    fn consent_guards_refuse_before_any_write() {
        let dir = temp_dir("consent");
        let path = dir.join("config.toml");
        initialize_config(&path, &Config::default(), false).expect("init");
        let config = load_config(&path).expect("load");
        // Enable without a matching proposal Home (absolute mismatch path is
        // caught before any file write).
        let error =
            update_consent(&path, &config, true, Path::new(r"C:\Elsewhere\.codex")).unwrap_err();
        assert_eq!(error.code, "BACKEND_PROXY_SCOPE_UNCONFIRMED");
    }

    #[test]
    fn env_block_prepare_revoke_and_conflicts() {
        let dir = temp_dir("env");
        let home = dir.join("home");
        fs::create_dir_all(home.join(".codex")).expect("home");
        let codex_home = home.join(".codex");
        let env_path = codex_home.join(".env");

        let config = Config {
            manage_backend: true,
            home: codex_home.clone(),
            ..Config::default()
        };

        // Fresh prepare writes the block and keeps outside bytes verbatim.
        fs::write(&env_path, "EXISTING=1\r\n").expect("seed");
        prepare_proxy_env(&config).expect("prepare");
        let text = fs::read_to_string(&env_path).unwrap();
        assert!(text.starts_with("EXISTING=1\r\n"));
        assert!(text.contains(dotenv::BLOCK_BEGIN));
        assert_eq!(inspect_proxy_env(&config), "current");

        // Stale after changing the proxy.
        config.port = 8080;
        assert_eq!(inspect_proxy_env(&config), "stale");
        prepare_proxy_env(&config).expect("reprepare");
        assert_eq!(inspect_proxy_env(&config), "current");

        // Conflict keys outside the block.
        fs::write(
            &env_path,
            format!("all_proxy=x\n{}", managed_block(&config)),
        )
        .expect("conflict");
        assert_eq!(inspect_proxy_env(&config), "conflict");
        let error = prepare_proxy_env(&config).unwrap_err();
        assert_eq!(error.code, "BACKEND_PROXY_CONFIG_CONFLICT");

        // Revoke removes exactly the block.
        fs::write(&env_path, format!("A=1\n{}\nB=2\n", managed_block(&config))).expect("layout");
        revoke_proxy_env(&codex_home).expect("revoke");
        assert_eq!(fs::read_to_string(&env_path).unwrap(), "A=1\n\nB=2\n");
        assert_eq!(inspect_proxy_env(&config), "pending");

        // Deleting an otherwise-empty file is transactional and final.
        fs::write(&env_path, managed_block(&config)).expect("only-block");
        revoke_proxy_env(&codex_home).expect("revoke-empty");
        assert!(!env_path.exists(), "empty file removed transactionally");
    }

    #[test]
    fn concurrent_env_writer_is_refused_not_overwritten() {
        let dir = temp_dir("race");
        let codex_home = dir.join("home");
        fs::create_dir_all(&codex_home).expect("home");
        let env_path = codex_home.join(".env");
        let config = Config {
            manage_backend: true,
            home: codex_home.clone(),
            ..Config::default()
        };
        fs::write(&env_path, "X=1\n").expect("seed");
        // Simulate an external edit between the read and the transaction by
        // racing a writer thread against prepare with a stale expectation.
        let mut guard = config.clone();
        guard.port = 8080;
        let path_clone = env_path.clone();
        let racer = std::thread::spawn(move || {
            for _ in 0..200 {
                if let Ok(mut file) = fs::OpenOptions::new().write(true).open(&path_clone) {
                    let _ = file.write_all(b"X=2\n");
                }
            }
        });
        let outcome = prepare_proxy_env(&guard);
        racer.join().expect("racer");
        // Either the transaction observed consistent bytes and committed, or
        // it refused with CONFLICT; never a torn merge.
        match outcome {
            Ok(()) => {}
            Err(error) => assert!(
                error.code == "BACKEND_PROXY_CONFIG_CONFLICT"
                    || error.code == "BACKEND_PROXY_ENV_IO",
                "unexpected refusal: {}",
                error.code
            ),
        }
        let text = fs::read_to_string(&env_path).unwrap();
        assert!(
            text == "X=2\n" || text.starts_with("X=2\n") || text.contains(dotenv::BLOCK_BEGIN),
            "no torn merge: {text:?}"
        );
    }
}
