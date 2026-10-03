//! Native public-Incan artifact coverage for `std.async.time.Duration` constructors.

use std::fs;

use incan_test_support as support;

const DURATION_CONTRACT: &str = r#"from std.async.time import Duration


def require_duration(actual: Duration, expected_secs: int, expected_nanos: int) -> None:
    """Assert the canonical seconds and nanoseconds representation."""
    assert actual.secs == expected_secs
    assert actual.nanos == expected_nanos


def main() -> None:
    """Exercise integer constructors through the public standard-library surface."""
    require_duration(Duration.from_millis(0), 0, 0)
    require_duration(Duration.from_millis(1), 0, 1000000)
    require_duration(Duration.from_millis(999), 0, 999000000)
    require_duration(Duration.from_millis(1000), 1, 0)
    require_duration(Duration.from_millis(1001), 1, 1000000)
    require_duration(Duration.from_millis(9007199254740999), 9007199254740, 999000000)
    require_duration(Duration.from_millis(9223372036854775807), 9223372036854775, 807000000)
    require_duration(Duration.from_millis(-1), 0, 0)
    require_duration(Duration.from_millis(-1001), 0, 0)
    require_duration(Duration.from_millis(-9223372036854775807 - 1), 0, 0)

    require_duration(Duration.from_secs(0), 0, 0)
    require_duration(Duration.from_secs(1), 1, 0)
    require_duration(Duration.from_secs(9223372036854775807), 9223372036854775807, 0)
    require_duration(Duration.from_secs(-1), 0, 0)
    require_duration(Duration.from_secs(-9223372036854775807 - 1), 0, 0)
"#;

/// Compile and execute integer duration constructors through the shipped public module.
#[test]
fn public_duration_integer_constructors_are_exact_and_nonnegative() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(temporary.path().join("main.incn"), DURATION_CONTRACT)?;

    let mut command = support::repo_command();
    command
        .current_dir(temporary.path())
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_HOME", temporary.path().join("incan-home"))
        .env("INCAN_NO_BANNER", "1")
        .args(["run", "main.incn", "--offline"]);
    if !support::oven_compiler_suite_is_active() {
        command
            .env(
                "INCAN_GENERATED_CARGO_TARGET_DIR",
                support::generated_cargo_target_dir(),
            )
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", support::sdk_provider_store());
    }

    let output = command.output()?;
    assert!(
        output.status.success(),
        "public Duration artifact failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    Ok(())
}
