use std::{path::PathBuf, sync::Arc, time::Duration};

use proxy_guard_core::{
    AppEffect, AppState, DesktopAppInfo, DesktopProcessState, GuardConfig, LaunchOptions,
    LaunchReceipt, TaskResult,
};
use proxy_guard_windows::{
    desktop_process_state, discover_desktop_app, inspect_backend_proxy_state, launch_codex,
};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

/// Bounded wait for the single foreground task to finish its cleanup when
/// Guard shuts down.
// The daemon helper can require five seconds to reap after cancellation.
const SHUTDOWN_JOIN_BUDGET: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub struct EffectDispatcher {
    tx: mpsc::Sender<TaskResult>,
    cancellation: CancellationToken,
    cached_app: Arc<Mutex<Option<DesktopAppInfo>>>,
    foreground_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    foreground_cancellation: Arc<std::sync::Mutex<Option<CancellationToken>>>,
}

impl EffectDispatcher {
    pub fn new(tx: mpsc::Sender<TaskResult>, cancellation: CancellationToken) -> Self {
        Self {
            tx,
            cancellation,
            cached_app: Arc::new(Mutex::new(None)),
            foreground_task: Arc::new(Mutex::new(None)),
            foreground_cancellation: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn dispatch(&self, effect: AppEffect, state: &AppState) {
        if matches!(effect, AppEffect::Shutdown) {
            self.cancellation.cancel();
            return;
        }
        let tx = self.tx.clone();
        let cancellation = self.cancellation.child_token();
        if let Ok(mut slot) = self.foreground_cancellation.lock() {
            *slot = Some(cancellation.clone());
        }
        let config = state.config.clone();
        let repair_required =
            state.config_readiness == proxy_guard_core::ConfigReadiness::RepairRequired;
        let config_path = state.config_path.clone();
        let cached_app = Arc::clone(&self.cached_app);
        let handle = tokio::spawn(async move {
            let result = match effect {
                AppEffect::RefreshLocalState => {
                    let desktop_app = discover_desktop_app(&config, None, &cancellation).await;
                    let process = desktop_app
                        .as_ref()
                        .map_or(DesktopProcessState::Unknown, |info| {
                            desktop_process_state(info)
                        });
                    // The real disk state of the authorized home block rides
                    // along with every refresh; without a discovered target
                    // the state stays unknown. Inspection never reads the
                    // Codex Home when consent is off.
                    let backend_proxy = desktop_app.as_ref().map_or(
                        proxy_guard_core::BackendProxyRuntimeState::Unknown,
                        |info| inspect_backend_proxy_state(info, &config),
                    );
                    if let Ok(info) = &desktop_app {
                        *cached_app.lock().await = Some(info.clone());
                    }
                    TaskResult::LocalStateRefreshed {
                        desktop_app,
                        process,
                        backend_proxy,
                    }
                }
                AppEffect::LaunchDesktop(options) => {
                    let result =
                        launch_pipeline(&config, &config_path, options, &cancellation).await;
                    if let Ok((info, _)) = &result {
                        *cached_app.lock().await = Some(info.clone());
                    }
                    TaskResult::LaunchCompleted(result)
                }
                AppEffect::SaveConfig(updated) => {
                    let result = tokio::task::spawn_blocking(move || {
                        let expected = if repair_required {
                            proxy_guard_windows::ConfigExpectation::Invalid
                        } else {
                            proxy_guard_windows::ConfigExpectation::Current(&config)
                        };
                        proxy_guard_windows::GuardConfigTransaction::begin(&config_path, expected)?
                            .commit(&updated)
                            .map(|()| updated)
                    })
                    .await
                    .map_err(|error| format!("configuration save task failed: {error}"))
                    .and_then(|result| result.map_err(|error| error.to_string()));
                    TaskResult::ConfigSaved(result)
                }
                AppEffect::UpdateBackendProxyConsent { enable, home } => {
                    let result =
                        update_backend_proxy_consent(&config, &config_path, enable, home).await;
                    TaskResult::BackendProxyConsentUpdated(result)
                }
                AppEffect::Shutdown => return,
            };
            let _ = tx.send(result).await;
        });
        // Only one foreground operation exists by design; if a stale handle is
        // somehow still present, stop it before tracking the new task.
        if let Ok(mut slot) = self.foreground_task.try_lock()
            && let Some(previous) = slot.replace(handle)
        {
            previous.abort();
        }
    }

    /// Waits (bounded) for the in-flight foreground task to finish its
    /// cleanup. Call after cancelling; never terminates Desktop or any other
    /// external process.
    pub async fn shutdown(&self) {
        let task = self.foreground_task.lock().await.take();
        if let Some(task) = task {
            let _ = timeout(SHUTDOWN_JOIN_BUDGET, task).await;
        }
    }

    /// Cancel only the current operation; subsequent launches get a fresh
    /// child token. Never terminates Desktop or the shared daemon.
    pub fn cancel_foreground(&self) {
        if let Ok(slot) = self.foreground_cancellation.lock()
            && let Some(token) = slot.as_ref()
        {
            token.cancel();
        }
    }

    /// The bridge checks this before checking its result channel. A
    /// finished task without a queued result indicates a panic/early exit,
    /// which must close the operation instead of leaving the GUI busy.
    pub async fn foreground_finished(&self) -> bool {
        self.foreground_task
            .lock()
            .await
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
    }
}

/// Applies one explicit consent decision.
///
/// Enabling records the bound home only — the `.env` block itself is prepared
/// by the next authorized launch, never at consent time. Disabling removes
/// Guard's own managed block first and persists the restricted configuration
/// only after the block is safely gone: while the revoke fails, the consent,
/// the bound home, and the file all stay untouched and the user can simply
/// retry, so a block can never be orphaned in a home Guard no longer records.
/// If the block is gone but the configuration save fails, the still-enabled
/// authorization honestly reports the missing block as pending on the next
/// refresh.
async fn update_backend_proxy_consent(
    config: &GuardConfig,
    config_path: &std::path::Path,
    enable: bool,
    home: PathBuf,
) -> Result<GuardConfig, String> {
    let config = config.clone();
    let config_path = config_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        // Acquire write/delete exclusion and compare the actual file before
        // any Home mutation. Keep this exact handle until consent is saved.
        let transaction = proxy_guard_windows::GuardConfigTransaction::begin(
            &config_path,
            proxy_guard_windows::ConfigExpectation::Current(&config),
        )?;
        if enable {
        let mut updated = config.clone();
        updated.codex.manage_codex_proxy_env = true;
        updated.codex.proxy_env_home = canonical_home(&home)?;
        transaction
            .commit(&updated)
            .map(|()| updated)
    } else {
        // The saved configuration is the only durable record of which Home
        // the block belongs to, so the block must be gone before that record
        // is dropped — the reverse order could strand an orphan block no
        // later Guard run could even locate.
        proxy_guard_windows::revoke(&proxy_guard_windows::env_path(&home))
        .map_err(|error| {
            format!(
                "BACKEND_PROXY_REVOKE_FAILED: {error}; the managed block was not safely \
                 removed, so the consent remains enabled and the bound Home is unchanged"
            )
        })?;
        // The block is gone (or never existed); revoking an absent block
        // succeeds, so retrying after a failed save below is always safe.
        let mut updated = config.clone();
        updated.codex.manage_codex_proxy_env = false;
        updated.codex.proxy_env_home = PathBuf::new();
        transaction
            .commit(&updated)
            .map(|()| updated)
            .map_err(|error| {
                format!(
                    "BACKEND_PROXY_CONSENT_SAVE_FAILED: Guard removed its managed .env \
                     block but could not persist the revoked consent: {error}; the \
                     configuration still authorizes management — retry after fixing the \
                     configuration write problem"
                )
            })
        }
    }).await.map_err(|_| "BACKEND_PROXY_REVOKE_TASK_FAILED: the configuration task did not return; refresh before retrying".to_string())?
}

/// Canonicalizes the confirmed home when it exists and keeps the absolute path
/// as written otherwise (the directory may be created by the first prepare).
fn canonical_home(home: &std::path::Path) -> Result<PathBuf, String> {
    if let Ok(canonical) = std::fs::canonicalize(home) {
        return Ok(canonical);
    }
    if home.is_absolute() && home.as_os_str().len() < 4096 {
        Ok(home.to_path_buf())
    } else {
        Err(
            "BACKEND_PROXY_SCOPE_UNCONFIRMED: the authorized Codex Home is not a usable \
             absolute path"
                .into(),
        )
    }
}

pub async fn launch_pipeline(
    config: &GuardConfig,
    config_path: &std::path::Path,
    options: LaunchOptions,
    cancellation: &CancellationToken,
) -> Result<(DesktopAppInfo, LaunchReceipt), String> {
    config.validate().map_err(|error| error.to_string())?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    let expected = config.clone();
    let lease_path = config_path.to_path_buf();
    let _configuration_lease = tokio::task::spawn_blocking(move || {
        proxy_guard_windows::GuardConfigLease::acquire(&lease_path, &expected)
    })
    .await
    .map_err(|_| "CONFIG_LOCK_FAILED: configuration lease task did not return".to_string())??;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    // Launch always re-discovers the Desktop instead of trusting the cached
    // entry the TUI displays; an app update may have replaced the entry point.
    let info = discover_desktop_app(config, None, cancellation).await?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    let receipt = launch_codex(&info, config, options, cancellation).await?;
    Ok((info, receipt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_home_prefers_the_real_path_but_accepts_a_future_directory() {
        let existing = std::env::temp_dir();
        let canonical = canonical_home(&existing).unwrap();
        assert!(canonical.is_absolute());
        let future = existing.join("cpg-never-created-home");
        let kept = canonical_home(&future).unwrap();
        assert_eq!(kept, future);
        assert!(canonical_home(std::path::Path::new("relative")).is_err());
    }

    /// A Guard block someone edited (here: an unknown extra key) — `revoke`
    /// refuses to tear it out, which is exactly the failure under test.
    const CORRUPTED_BLOCK: &str = "# BEGIN CODEX PROXY GUARD: proxy-v1\n\
         HTTP_PROXY=http://127.0.0.1:10808\n\
         EXTRA=1\n\
         # END CODEX PROXY GUARD: proxy-v1\n";

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cpg-consent-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn authorized_config(home: &std::path::Path) -> GuardConfig {
        let mut config = GuardConfig::default();
        config.codex.manage_codex_proxy_env = true;
        config.codex.proxy_env_home = home.to_path_buf();
        config
    }

    #[tokio::test]
    async fn failed_revoke_keeps_the_config_and_the_env_untouched() {
        let dir = temp_root("revoke-fail");
        let config_path = dir.join("config.toml");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = proxy_guard_windows::env_path(&home);
        std::fs::write(&env, CORRUPTED_BLOCK).unwrap();
        let config = authorized_config(&home);
        config.save(&config_path).unwrap();
        let saved_config = std::fs::read_to_string(&config_path).unwrap();

        let error = update_backend_proxy_consent(&config, &config_path, false, home.clone())
            .await
            .unwrap_err();

        assert!(error.starts_with("BACKEND_PROXY_REVOKE_FAILED:"), "{error}");
        assert!(error.contains("consent remains enabled"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            saved_config,
            "a failed revoke must not touch config.toml"
        );
        let reloaded = GuardConfig::load(&config_path).unwrap();
        assert!(reloaded.codex.manage_codex_proxy_env);
        assert_eq!(reloaded.codex.proxy_env_home, home);
        assert_eq!(
            std::fs::read(&env).unwrap(),
            CORRUPTED_BLOCK.as_bytes(),
            "a failed revoke must not touch the .env file"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn successful_revoke_removes_the_block_before_disabling_consent() {
        let dir = temp_root("revoke-ok");
        let config_path = dir.join("config.toml");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = proxy_guard_windows::env_path(&home);
        std::fs::write(&env, "KEEP=1\n").unwrap();
        let config = authorized_config(&home);
        config.save(&config_path).unwrap();
        proxy_guard_windows::prepare(
            &env,
            &proxy_guard_windows::ProxyEnvValues::from_config(&config),
        )
        .unwrap();

        let updated = update_backend_proxy_consent(&config, &config_path, false, home)
            .await
            .unwrap();

        assert!(!updated.codex.manage_codex_proxy_env);
        assert!(updated.codex.proxy_env_home.as_os_str().is_empty());
        let on_disk = GuardConfig::load(&config_path).unwrap();
        assert!(!on_disk.codex.manage_codex_proxy_env);
        assert!(on_disk.codex.proxy_env_home.as_os_str().is_empty());
        assert_eq!(
            std::fs::read_to_string(&env).unwrap(),
            "KEEP=1\n",
            "only Guard's block disappears; user content survives"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn revoke_without_a_block_still_disables_the_consent() {
        let dir = temp_root("revoke-absent");
        let config_path = dir.join("config.toml");
        // A home whose .env exists but holds no Guard block…
        let with_file = dir.join("home-with-file");
        std::fs::create_dir_all(&with_file).unwrap();
        std::fs::write(with_file.join(".env"), "KEEP=1\n").unwrap();
        authorized_config(&with_file).save(&config_path).unwrap();
        let updated = update_backend_proxy_consent(
            &authorized_config(&with_file),
            &config_path,
            false,
            with_file.clone(),
        )
        .await
        .unwrap();
        assert!(!updated.codex.manage_codex_proxy_env);
        assert_eq!(
            std::fs::read_to_string(with_file.join(".env")).unwrap(),
            "KEEP=1\n"
        );

        // …and a home whose .env does not exist at all.
        let missing = dir.join("home-missing-env");
        std::fs::create_dir_all(&missing).unwrap();
        authorized_config(&missing).save(&config_path).unwrap();
        let updated = update_backend_proxy_consent(
            &authorized_config(&missing),
            &config_path,
            false,
            missing,
        )
        .await
        .unwrap();
        assert!(!updated.codex.manage_codex_proxy_env);
        assert!(updated.codex.proxy_env_home.as_os_str().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn unsaveable_config_is_rejected_before_removing_the_block() {
        let dir = temp_root("save-fail");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = proxy_guard_windows::env_path(&home);
        let config = authorized_config(&home);
        proxy_guard_windows::prepare(
            &env,
            &proxy_guard_windows::ProxyEnvValues::from_config(&config),
        )
        .unwrap();
        let original_env = std::fs::read(&env).unwrap();
        // A regular file where the config directory should be prevents the
        // transaction from acquiring its configuration handle. Revoke must
        // not begin until that handle is acquired and the config is matched.
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();

        let error = update_backend_proxy_consent(
            &config,
            &blocker.join("config.toml"),
            false,
            home.clone(),
        )
        .await
        .unwrap_err();

        assert!(error.starts_with("CONFIG_LOCK_FAILED:"), "{error}");
        assert_eq!(
            std::fs::read(&env).unwrap(),
            original_env,
            "failed lock must leave the authorized block untouched"
        );
        // The authorized config is still intact on the in-memory side, and a
        // retry against a writable path now succeeds end to end.
        let retry_path = dir.join("real-config.toml");
        config.save(&retry_path).unwrap();
        let updated = update_backend_proxy_consent(&config, &retry_path, false, home)
            .await
            .unwrap();
        assert!(!updated.codex.manage_codex_proxy_env);
        let on_disk = GuardConfig::load(&retry_path).unwrap();
        assert!(!on_disk.codex.manage_codex_proxy_env);
        assert!(!env.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn stale_consent_cannot_revoke_a_home_or_overwrite_a_new_binding() {
        let dir = temp_root("stale-consent");
        let config_path = dir.join("config.toml");
        let home_a = dir.join("home-a");
        let home_b = dir.join("home-b");
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let config_a = authorized_config(&home_a);
        let config_b = authorized_config(&home_b);
        let env_a = proxy_guard_windows::env_path(&home_a);
        let env_b = proxy_guard_windows::env_path(&home_b);
        for (env, config) in [(&env_a, &config_a), (&env_b, &config_b)] {
            proxy_guard_windows::prepare(
                env,
                &proxy_guard_windows::ProxyEnvValues::from_config(config),
            )
            .unwrap();
        }
        config_b.save(&config_path).unwrap();
        let before_a = std::fs::read(&env_a).unwrap();
        let before_b = std::fs::read(&env_b).unwrap();
        let error = update_backend_proxy_consent(&config_a, &config_path, false, home_a)
            .await
            .unwrap_err();
        assert!(error.starts_with("CONFIG_CHANGED:"), "{error}");
        assert_eq!(std::fs::read(&env_a).unwrap(), before_a);
        assert_eq!(std::fs::read(&env_b).unwrap(), before_b);
        assert_eq!(GuardConfig::load(&config_path).unwrap(), config_b);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
