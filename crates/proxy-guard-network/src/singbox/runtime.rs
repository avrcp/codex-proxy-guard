use std::{
    process::ExitStatus,
    thread,
    time::{Duration, Instant, SystemTime},
};

use super::{PreparedSingBoxConfig, SingBoxInstallation};
use crate::NetworkError;

use super::child::ManagedChild;

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);
pub const SING_BOX_VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);
pub const SING_BOX_STARTUP_GRACE: Duration = Duration::from_millis(250);

/// Result of bounded network-sidecar shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SingBoxShutdown {
    pub pid: u32,
    pub status: ExitStatus,
    pub forced: bool,
}

/// Validates and launches the pinned sing-box sidecar as an owned process tree.
#[derive(Clone, Debug)]
pub struct SingBoxRuntime {
    installation: SingBoxInstallation,
    validation_timeout: Duration,
    startup_grace: Duration,
}

impl SingBoxRuntime {
    #[must_use]
    pub const fn new(installation: SingBoxInstallation) -> Self {
        Self {
            installation,
            validation_timeout: SING_BOX_VALIDATION_TIMEOUT,
            startup_grace: SING_BOX_STARTUP_GRACE,
        }
    }

    #[must_use]
    pub const fn with_timeouts(
        mut self,
        validation_timeout: Duration,
        startup_grace: Duration,
    ) -> Self {
        self.validation_timeout = validation_timeout;
        self.startup_grace = startup_grace;
        self
    }

    #[must_use]
    pub const fn installation(&self) -> &SingBoxInstallation {
        &self.installation
    }

    /// Execute `sing-box check -c <managed-config>` with a hard timeout.
    ///
    /// # Errors
    ///
    /// Returns a typed spawn, timeout, or rejected-configuration error. The
    /// validation process tree is always reaped before the method returns.
    pub fn validate(&self, config: &PreparedSingBoxConfig) -> Result<(), NetworkError> {
        let arguments = self.installation.arguments("check", config.path());
        let mut child =
            ManagedChild::spawn(self.installation.executable(), &arguments).map_err(|source| {
                runtime_io(
                    "spawn sing-box configuration check",
                    &self.installation,
                    source,
                )
            })?;

        let Some(status) = wait_for_child(&mut child, self.validation_timeout, &self.installation)?
        else {
            terminate_and_reap(&mut child, &self.installation)?;
            return Err(NetworkError::SingBox(format!(
                "configuration check timed out after {} ms",
                duration_millis(self.validation_timeout)
            )));
        };

        if !status.success() {
            return Err(NetworkError::SingBox(format!(
                "sing-box rejected the configuration with {}",
                exit_status_label(status)
            )));
        }
        Ok(())
    }

    /// Validate, launch, and observe the sidecar through its initial startup window.
    ///
    /// Any early process exit fails closed. Endpoint readiness is checked by the
    /// caller once a concrete mixed inbound is known.
    ///
    /// # Errors
    ///
    /// Returns a validation, spawn, status-observation, or early-exit error.
    pub fn launch(&self, config: PreparedSingBoxConfig) -> Result<SingBoxProcess, NetworkError> {
        self.validate(&config)?;
        self.launch_validated(config)
    }

    pub(crate) fn launch_validated(
        &self,
        config: PreparedSingBoxConfig,
    ) -> Result<SingBoxProcess, NetworkError> {
        let arguments = self.installation.arguments("run", config.path());
        let child =
            ManagedChild::spawn(self.installation.executable(), &arguments).map_err(|source| {
                runtime_io(
                    "spawn managed sing-box process tree",
                    &self.installation,
                    source,
                )
            })?;
        let mut process = SingBoxProcess {
            child,
            installation: self.installation.clone(),
            config,
            started_at: SystemTime::now(),
        };

        if let Some(status) = process.wait_for_exit(self.startup_grace)? {
            return Err(NetworkError::SingBox(format!(
                "sing-box exited with {} during startup",
                exit_status_label(status)
            )));
        }
        Ok(process)
    }
}

/// Owns one sing-box process tree for a single managed sidecar.
#[derive(Debug)]
pub struct SingBoxProcess {
    child: ManagedChild,
    installation: SingBoxInstallation,
    config: PreparedSingBoxConfig,
    started_at: SystemTime,
}

impl SingBoxProcess {
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    #[must_use]
    pub const fn started_at(&self) -> SystemTime {
        self.started_at
    }

    #[must_use]
    pub const fn config(&self) -> &PreparedSingBoxConfig {
        &self.config
    }

    /// Observe the root process without releasing its Job ownership.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when process status cannot be collected.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, NetworkError> {
        self.child
            .try_wait()
            .map_err(|source| runtime_io("inspect sing-box", &self.installation, source))
    }

    /// Wait at most `timeout` for the sidecar to exit.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when process status cannot be collected.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Result<Option<ExitStatus>, NetworkError> {
        wait_for_child(&mut self.child, timeout, &self.installation)
    }

    /// Wait until the root sidecar exits and release its process tree.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the wait fails.
    pub fn wait(mut self) -> Result<ExitStatus, NetworkError> {
        self.child
            .wait()
            .map_err(|source| runtime_io("wait for sing-box", &self.installation, source))
    }

    /// Terminate the complete owned sidecar process tree and reap the root.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when termination or status collection fails.
    pub fn terminate(mut self) -> Result<ExitStatus, NetworkError> {
        if self.try_wait()?.is_none() {
            self.child.terminate_tree().map_err(|source| {
                runtime_io(
                    "terminate sing-box process tree",
                    &self.installation,
                    source,
                )
            })?;
        }
        self.child
            .wait()
            .map_err(|source| runtime_io("wait for sing-box", &self.installation, source))
    }

    /// Wait for a prior graceful stop request, then force the complete tree.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when status collection or forced termination fails.
    pub fn finish_shutdown(
        mut self,
        grace_period: Duration,
    ) -> Result<SingBoxShutdown, NetworkError> {
        let pid = self.pid();
        if let Some(status) = self.wait_for_exit(grace_period)? {
            return Ok(SingBoxShutdown {
                pid,
                status,
                forced: false,
            });
        }
        self.child.terminate_tree().map_err(|source| {
            runtime_io(
                "terminate sing-box process tree",
                &self.installation,
                source,
            )
        })?;
        let status = self
            .child
            .wait()
            .map_err(|source| runtime_io("wait for sing-box", &self.installation, source))?;
        Ok(SingBoxShutdown {
            pid,
            status,
            forced: true,
        })
    }
}

impl Drop for SingBoxProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() && self.child.terminate_tree().is_ok() {
            let _ = self.child.wait();
        }
    }
}

fn wait_for_child(
    child: &mut ManagedChild,
    timeout: Duration,
    installation: &SingBoxInstallation,
) -> Result<Option<ExitStatus>, NetworkError> {
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| runtime_io("inspect sing-box", installation, source))?
        {
            return Ok(Some(status));
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Ok(None);
        }
        thread::sleep(PROCESS_POLL_INTERVAL.min(timeout.saturating_sub(elapsed)));
    }
}

fn terminate_and_reap(
    child: &mut ManagedChild,
    installation: &SingBoxInstallation,
) -> Result<(), NetworkError> {
    if child
        .try_wait()
        .map_err(|source| runtime_io("inspect sing-box", installation, source))?
        .is_none()
    {
        child.terminate_tree().map_err(|source| {
            runtime_io("terminate sing-box process tree", installation, source)
        })?;
    }
    child
        .wait()
        .map(|_| ())
        .map_err(|source| runtime_io("wait for sing-box", installation, source))
}

fn runtime_io(
    action: &'static str,
    installation: &SingBoxInstallation,
    source: std::io::Error,
) -> NetworkError {
    NetworkError::SingBox(format!(
        "{action} {}: {source}",
        installation.executable().display()
    ))
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn exit_status_label(status: ExitStatus) -> String {
    status.code().map_or_else(
        || "termination without an exit code".to_owned(),
        |code| format!("exit code {code}"),
    )
}
