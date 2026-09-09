#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use incan::oven::native_test::{OvenNativeTestBatchRequest, run_native_test_batch_all_for_request};

#[test]
fn native_timeout_terminates_descendants_that_retain_output_pipes() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let executable = fixture.path().join("stalled-native-test");
    let descendant_pid = fixture.path().join("native-descendant-pid");
    write_executable(
        &executable,
        &format!(
            "#!/bin/sh\nfor argument in \"$@\"; do\n  if [ \"$argument\" = \"--list\" ]; then\n    exit 0\n  fi\ndone\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"{}\"\nwait\n",
            descendant_pid.display(),
        ),
    )?;

    let started = Instant::now();
    let report = run_native_test_batch_all_for_request(&OvenNativeTestBatchRequest {
        executable: &executable,
        environment: &BTreeMap::new(),
        working_directory: Some(fixture.path()),
        timeout: Some(Duration::from_millis(100)),
        test_threads: None,
        root_label: None,
        progress: None,
    })?;

    assert!(report.timed_out, "stalled native test unexpectedly completed");
    assert!(!report.success, "timed-out native test was reported as successful");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a descendant retaining output pipes held the timeout supervisor open"
    );
    assert!(descendant_pid.is_file(), "fake native-test descendant was not started");
    assert_process_stopped(read_pid(&descendant_pid)?)
}

fn write_executable(path: &Path, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(path, contents)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn read_pid(path: &Path) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(fs::read_to_string(path)?.trim().parse()?)
}

fn assert_process_stopped(pid: u32) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..100 {
        if !process_is_running(pid)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(format!("supervised descendant process {pid} remained alive").into())
}

/// Return whether a descendant can still execute or retain the supervisor's resources.
///
/// Linux keeps a killed child visible to `kill -0` until init reaps it. That zombie cannot retain a pipe or consume
/// capacity, so it is evidence of successful process-tree termination rather than a live escaped descendant.
fn process_is_running(pid: u32) -> Result<bool, Box<dyn std::error::Error>> {
    let status = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Ok(false);
    }
    #[cfg(target_os = "linux")]
    if fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .is_some_and(|(_, after)| after.trim_start().starts_with('Z'))
    }) {
        return Ok(false);
    }
    Ok(true)
}
