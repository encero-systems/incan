//! Direct-source regressions for exact numeric defaults and guarded static results (#1337).

use super::{check_native_case, dev7_carrier_admission, driver_command, driver_fixture, success};

#[path = "numeric_boundaries_sources.rs"]
mod sources;

/// Every exact carrier defaults to zero, recursively, while explicit sized field defaults retain their values.
#[test]
fn exact_numeric_defaults_preserve_carriers_and_declared_values() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case(
        "exact-numeric-defaults",
        sources::DEFAULTS,
        b"0\n0\n0\n0\n0\n0\n0\n0\n0\n0\n0\n0.0\n0.0\n0\n0.0\n7\n-1.0\n",
    )
}

/// Sized numeric values returned under a computed static's guard remain ordinary owned scalar results.
#[test]
fn static_methods_return_exact_numeric_values() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case("static-numeric-results", sources::STATIC_RESULTS, b"1.5\n-7\n255\n")
}

/// Actual checked plans retain typed zeros and admit owned numeric layouts without permitting borrowed escapes.
#[test]
fn exact_numeric_plans_preserve_static_guard_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let (_caller, binary) = dev7_carrier_admission::baked_carrier_caller(fixture, "exact-numeric-plan-boundaries")?;
    let output = driver_command(&binary).arg("numeric-boundaries").output()?;
    success(&output, "exact numeric plan boundary controls");
    assert_eq!(
        output.stdout,
        b"exact numeric boundaries: typed defaults, 15 scalar returns, 2 nested owned returns, 4 escape refusals passed\n"
    );
    Ok(())
}
