use std::path::{Path, PathBuf};

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
