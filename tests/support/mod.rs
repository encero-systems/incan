use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Instant;

/// Start one opt-in nested Incan-command measurement for an integration-test diagnostic run.
///
/// Normal test execution neither reads a clock nor emits timing output. The caller supplies a stable command label,
/// while the current libtest thread identifies the regression that started it.
#[allow(dead_code)]
pub(crate) fn command_timing_started() -> Option<Instant> {
    std::env::var_os("INCAN_TEST_COMMAND_TIMINGS")
        .filter(|value| !value.is_empty())
        .map(|_| Instant::now())
}

/// Emit one machine-searchable nested command duration for an explicit diagnostic run.
#[allow(dead_code)]
pub(crate) fn report_command_timing(label: &str, started: Option<Instant>) {
    let Some(started) = started else {
        return;
    };
    let thread = std::thread::current();
    let test_name = thread.name().map_or("unnamed", |name| name);
    eprintln!(
        "incan-test-command-timing {}",
        serde_json::json!({
            "test_name": test_name,
            "command": label,
            "elapsed_ms": started.elapsed().as_millis(),
        })
    );
}

/// Emit the phase breakdown from an opt-in JSON build report already produced by a nested command.
///
/// A malformed or non-build response is ignored because this is only diagnostic evidence. The command's own status
/// and its regression assertions remain authoritative.
#[allow(dead_code)]
pub(crate) fn report_build_phase_timing(label: &str, output: &Output) {
    if std::env::var_os("INCAN_TEST_COMMAND_TIMINGS").is_none() {
        return;
    }
    let Ok(report) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return;
    };
    let Some(timings) = report.get("timings_ms").and_then(serde_json::Value::as_object) else {
        return;
    };
    let phase_timings_ms = timings
        .iter()
        .filter_map(|(phase, elapsed_ms)| elapsed_ms.as_u64().map(|elapsed_ms| (phase, elapsed_ms)))
        .collect::<std::collections::BTreeMap<_, _>>();
    if phase_timings_ms.is_empty() {
        return;
    }
    let thread = std::thread::current();
    let test_name = thread.name().map_or("unnamed", |name| name);
    eprintln!(
        "incan-test-build-phase-timing {}",
        serde_json::json!({
            "test_name": test_name,
            "command": label,
            "phase_timings_ms": phase_timings_ms,
        })
    );
}

/// Return the Incan CLI built alongside the current integration-test executable.
///
/// Nextest rewrites `CARGO_BIN_EXE_incan` when a portable archive is extracted on another runner. Read it at runtime
/// instead of embedding the archive producer's absolute `target/debug/incan` path in the test executable.
#[allow(dead_code)]
pub(crate) fn incan_binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_incan")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/incan"))
}

/// Return whether this test binary is executing with the scheduler-owned Oven Loaf closure.
///
/// A stored compiler-suite child carries a direct-Rustc executable as well as the Loaf capability. Portable
/// nextest acceptance archives receive the latter directly from their immutable provider artifact. Both forms have
/// already received a sealed SDK inventory and must not replace it with the legacy generated-Cargo provider store.
#[allow(dead_code)]
pub(crate) fn oven_compiler_suite_is_active() -> bool {
    std::env::var_os("INCAN_OVEN_COMPILER_SUITE_RUSTC").is_some_and(|value| !value.is_empty())
        || (std::env::var_os("INCAN_INTERNAL_OVEN_LOAF_EXECUTION").is_some_and(|value| value == "1")
            && std::env::var_os("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT").is_some_and(|value| !value.is_empty()))
}

/// Return the generated Cargo target selected by the outer test harness.
///
/// `make` and CI preheat one task-local target before starting nextest. Subprocess helpers must preserve that
/// selection instead of silently redirecting nested Cargo back into the repository's default `target/` tree.
#[allow(dead_code)]
pub(crate) fn generated_cargo_target_dir() -> PathBuf {
    selected_harness_path(
        "INCAN_GENERATED_CARGO_TARGET_DIR",
        "target/incan_generated_shared_target",
    )
}

/// Preserve a caller-selected generated Cargo target while retaining a test-local fallback.
///
/// Cold-provider acceptance tests need independent provider stores, not three duplicate compilations of the same
/// Cargo dependency graph. CI can select one job-local target for those tests; standalone runs remain isolated.
#[allow(dead_code)]
pub(crate) fn generated_cargo_target_dir_or(fallback: &Path) -> PathBuf {
    let selected = std::env::var_os("INCAN_GENERATED_CARGO_TARGET_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.to_path_buf());
    anchor_harness_path(selected)
}

/// Return the compiled SDK provider store selected by the outer test harness.
#[allow(dead_code)]
pub(crate) fn sdk_provider_store() -> PathBuf {
    selected_harness_path(
        "INCAN_INTERNAL_SDK_PROVIDER_STORE",
        "target/incan_test_sdk_provider_store",
    )
}

/// Preserve a cold-acceptance provider store selected explicitly by the outer test harness.
///
/// Ordinary test runs retain their isolated fallback. The dedicated CI lane can opt into one empty store for
/// compatible cold consumers without allowing an already-warmed general provider store to weaken the proof.
#[allow(dead_code)]
pub(crate) fn cold_sdk_provider_store_or(fallback: &Path) -> PathBuf {
    let selected = std::env::var_os("INCAN_TEST_COLD_PROVIDER_STORE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.to_path_buf());
    anchor_harness_path(selected)
}

/// Anchor relative outer-harness paths before nested commands switch to a fixture working directory.
fn selected_harness_path(variable: &str, fallback: &str) -> PathBuf {
    let selected = std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(fallback));
    anchor_harness_path(selected)
}

fn anchor_harness_path(selected: PathBuf) -> PathBuf {
    if selected.is_absolute() {
        selected
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
            .join(selected)
    }
}
