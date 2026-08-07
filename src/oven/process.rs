//! Process-tree containment shared by Oven's bounded child-process paths.

use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};

/// Put a child and all normally spawned descendants in an isolated process group.
pub(crate) fn isolate_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
    }
}

/// Terminate and reap an isolated child process group.
///
/// Cargo, Rustdoc, libtest, build scripts, compilers, and linkers inherit the isolated group unless they explicitly
/// create a new session, which none of the supported Oven child paths permit or require. GNU `kill` needs `--`
/// before a negative process-group ID, while the BSD implementation shipped by macOS rejects that separator.
pub(crate) fn terminate_process_group(child: &mut Child) -> io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        let process_group = format!("-{}", child.id());
        let mut command = Command::new("/bin/kill");
        command.arg("-KILL");
        #[cfg(target_os = "linux")]
        command.arg("--");
        let status = command
            .arg(&process_group)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() && child.try_wait()?.is_none() {
            return Err(io::Error::other(format!(
                "failed to terminate process group {process_group}: {status}"
            )));
        }
    }
    #[cfg(not(unix))]
    child.kill()?;

    child.wait()
}

#[cfg(all(test, unix))]
/// Return whether a process still accepts a signal-zero existence probe.
pub(crate) fn process_exists(pid: u32) -> io::Result<bool> {
    Ok(Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success())
}
