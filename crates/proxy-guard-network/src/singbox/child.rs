use std::{ffi::OsString, io, path::Path, process::ExitStatus};

#[cfg(not(windows))]
use std::process::{Child, Command, Stdio};

#[cfg(windows)]
use windows_spawn::{Child, Command, Job, SpawnOptions, Stdio};

/// Platform child ownership used by the sing-box runtime.
///
/// On Windows the process is attached to a dedicated Job Object during
/// `CreateProcessW` so every descendant is reaped when the sidecar is dropped,
/// cancelled, or terminated.
#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct ManagedChild {
    child: Child,
    job: Job,
}

#[cfg(not(windows))]
#[derive(Debug)]
pub(crate) struct ManagedChild {
    child: Child,
}

impl ManagedChild {
    pub(crate) fn spawn(executable: &Path, arguments: &[OsString]) -> io::Result<Self> {
        Self::spawn_command(executable, arguments)
    }

    #[cfg(windows)]
    fn spawn_command(executable: &Path, arguments: &[OsString]) -> io::Result<Self> {
        let job = Job::create()?;
        job.set_kill_on_close(true)?;

        let mut command = Command::new(executable);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command.spawn_with(SpawnOptions::new().job(&job))?;

        Ok(Self { child, job })
    }

    #[cfg(not(windows))]
    fn spawn_command(executable: &Path, arguments: &[OsString]) -> io::Result<Self> {
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().map(|child| Self { child })
    }

    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    #[cfg(windows)]
    pub(crate) fn terminate_tree(&mut self) -> io::Result<()> {
        self.job.terminate(1)
    }

    #[cfg(not(windows))]
    pub(crate) fn terminate_tree(&mut self) -> io::Result<()> {
        self.child.kill()
    }
}
