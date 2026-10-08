//! Exhaustion at the native integer boundary on both range loop advancement edges.

use super::{body_named, build};

/// Inclusive source ranges retain exhaustion before advancement, including an explicit `continue` edge.
#[test]
fn range_maximum_exhaustion_precedes_both_advancement_edges() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def run(start: int, end: int, skip: bool) -> None:\n    values = start..=end\n    for item in values:\n        if skip:\n            continue\n        println(item)\n",
        &["m", "range_bounds"],
    )?;
    let snapshot = body_named(&module, "run")?.render_snapshot();
    assert_eq!(
        snapshot.matches("9223372036854775807").count(),
        2,
        "both advancement edges must retain maximum exhaustion: {snapshot}"
    );
    assert_eq!(
        snapshot.matches("continue").count(),
        1,
        "the fixture must exercise the continue edge: {snapshot}"
    );
    Ok(())
}
