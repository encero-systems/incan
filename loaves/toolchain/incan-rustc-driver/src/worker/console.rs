//! Request-scoped Unix console descriptors. The serial worker restores its original descriptors before replying,
//! including after a caught panic, so diagnostics cannot become a later request's output.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Saved original descriptors, restored before the request's completion frame is sent.
pub(super) struct Capture {
    stdout: OwnedFd,
    stderr: OwnedFd,
    cwd: PathBuf,
}

impl Capture {
    /// Open private append-only sinks and redirect only this serialized request's process console.
    pub(super) fn open(stdout: &Path, stderr: &Path, cwd: &Path) -> io::Result<Self> {
        let out = OpenOptions::new().create(true).append(true).mode(0o600).open(stdout)?;
        let err = OpenOptions::new().create(true).append(true).mode(0o600).open(stderr)?;
        io::stdout().flush()?;
        io::stderr().flush()?;
        let saved = Self {
            stdout: duplicate(libc::STDOUT_FILENO)?,
            stderr: duplicate(libc::STDERR_FILENO)?,
            cwd: std::env::current_dir()?,
        };
        std::env::set_current_dir(cwd)?;
        replace(out.as_raw_fd(), libc::STDOUT_FILENO)?;
        replace(err.as_raw_fd(), libc::STDERR_FILENO)?;
        Ok(saved)
    }
}

impl Drop for Capture {
    /// Flush request-local buffers and restore both originals even when compilation unwinds.
    fn drop(&mut self) {
        let _ = io::stdout().flush();
        let _ = io::stderr().flush();
        let _ = replace(self.stdout.as_raw_fd(), libc::STDOUT_FILENO);
        let _ = replace(self.stderr.as_raw_fd(), libc::STDERR_FILENO);
        let _ = std::env::set_current_dir(&self.cwd);
    }
}

/// Own a duplicate without taking ownership of the process's original standard descriptor.
fn duplicate(descriptor: i32) -> io::Result<OwnedFd> {
    // SAFETY: dup borrows the live standard descriptor and returns a new owned descriptor on success.
    let copied = unsafe { libc::dup(descriptor) };
    if copied < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful dup returned this unique descriptor, which OwnedFd closes exactly once.
    Ok(unsafe { OwnedFd::from_raw_fd(copied) })
}

/// Atomically replace a standard descriptor while leaving the supplied source handle owned by its caller.
fn replace(source: i32, destination: i32) -> io::Result<()> {
    // SAFETY: both integers name open descriptors owned by this request or the process's standard streams.
    if unsafe { libc::dup2(source, destination) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
