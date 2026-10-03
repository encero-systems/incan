use super::incan_command;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_PROJECT_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestProject {
    dir: tempfile::TempDir,
}

impl std::ops::Deref for TestProject {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.dir.path()
    }
}

/// Create a temp directory with a single test file and keep it alive for the test duration.
fn write_test_project(filename: &str, source: &str) -> TestProject {
    let seq = TEST_PROJECT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let prefix = format!("incan_e2e_test_{}_{}_", std::process::id(), seq);
    let Ok(dir) = tempfile::Builder::new().prefix(&prefix).tempdir() else {
        panic!("failed to create temp dir");
    };
    let Ok(()) = std::fs::write(dir.path().join(filename), source) else {
        panic!("failed to write test file");
    };
    TestProject { dir }
}

/// Run `incan test` for the given path argument (file or directory).
fn run_incan_test_path(path: &Path) -> std::process::Output {
    incan_command()
        .args(["test", path.to_string_lossy().as_ref()])
        .output()
        .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
}

/// Run `incan test` on a directory and return the combined output.
fn run_incan_test(dir: &Path) -> std::process::Output {
    run_incan_test_path(dir)
}

/// Run `incan test` with extra flags.
fn run_incan_test_with_args(dir: &Path, extra: &[&str]) -> std::process::Output {
    let mut cmd = incan_command();
    cmd.arg("test");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.arg(dir.to_string_lossy().as_ref());
    cmd.output()
        .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
}

/// Run `incan test` with `cwd` and a relative path argument.
fn run_incan_test_relative(cwd: &Path, relative_path: &str) -> std::process::Output {
    incan_command()
        .arg("test")
        .arg(relative_path)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("failed to run `incan test {relative_path}`: {}", e))
}

/// Run `incan build <entry> <out_dir>` for an inline-test production source.
fn run_incan_build(entry: &Path, out_dir: &Path) -> std::process::Output {
    let output = incan_command()
        .args([
            "build",
            entry.to_string_lossy().as_ref(),
            out_dir.to_string_lossy().as_ref(),
        ])
        .env("CARGO_NET_OFFLINE", "true")
        .output();
    let Ok(output) = output else {
        panic!("failed to run `incan build`");
    };
    output
}
