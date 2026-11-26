//! Lazy-reader conflicts and last-use acceptance across source control flow.

use super::*;

/// Check one matrix cell and require either no ownership error or one precise lazy-reader diagnostic.
fn check_cell(label: &str, source: &str, conflict: bool) -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(source)?;
    let ownership: Vec<_> = checker
        .errors
        .iter()
        .filter(|error| matches!(error.stable_code(), Some("INCAN-T0001" | "INCAN-T0123")))
        .collect();
    if conflict {
        assert_eq!(ownership.len(), 1, "{label}: {ownership:?}\n{source}");
        assert_eq!(ownership[0].stable_code(), Some("INCAN-T0001"), "{label}");
        assert_eq!(ownership[0].related_spans().len(), 3, "{label}: {:?}", ownership[0]);
    } else {
        assert!(checker.errors.is_empty(), "{label}: {:?}\n{source}", checker.errors);
    }
    Ok(())
}

/// The matrix crosses each control-flow location and reader spelling with conflicts before the last use and legal
/// source actions after it. Escape is covered by the dedicated exact diagnostic test because it has no later use.
#[test]
fn lazy_reader_matrix_tracks_last_use_across_control_flow() -> Result<(), Box<dyn std::error::Error>> {
    let cells = [
        (
            "mutation / straight / before / named",
            "def main() -> None:\n    mut xs = [1, 2, 3]\n    it = xs.iter()\n    xs.append(4)\n    println(it.collect())\n",
            true,
        ),
        (
            "replacement / if / before / generator",
            "def main(flag: bool) -> None:\n    mut xs = [1, 2, 3]\n    generated = (value for value in xs)\n    if flag:\n        xs = [4]\n    println(generated.sum())\n",
            true,
        ),
        (
            "replacement / range / before / adapter",
            "def double(value: int) -> int:\n    return value * 2\ndef main() -> None:\n    mut xs = [1, 2, 3]\n    doubled = xs.iter().map(double)\n    for round in range(2):\n        xs = [round]\n        println(doubled.collect())\n",
            true,
        ),
        (
            "mutation / other list / before / named",
            "def main() -> None:\n    mut xs = [1, 2, 3]\n    it = xs.iter()\n    for value in [4, 5]:\n        xs.append(value)\n    println(it.collect())\n",
            true,
        ),
        (
            "replacement / while / before / generator",
            "def main() -> None:\n    mut xs = [1, 2, 3]\n    generated = (value for value in xs)\n    while len(xs) < 4:\n        xs = [4]\n    println(generated.sum())\n",
            true,
        ),
        (
            "mutation / nested loops / before / adapter",
            "def main() -> None:\n    mut xs = [1, 2, 3]\n    selected = xs.iter().take(2)\n    for outer in range(2):\n        for inner in [outer]:\n            xs.append(inner)\n    println(selected.sum())\n",
            true,
        ),
        (
            "move / straight / after / named",
            "def consume(values: Owned[list[int]]) -> None:\n    println(values)\ndef main() -> None:\n    xs = [1, 2, 3]\n    it = xs.iter()\n    println(it.collect())\n    consume(xs)\n",
            false,
        ),
        (
            "mutation / if else / after / generator",
            "def main(flag: bool) -> None:\n    mut xs = [1, 2, 3]\n    generated = (value for value in xs)\n    println(generated.sum())\n    if flag:\n        xs.append(4)\n    else:\n        xs.append(5)\n",
            false,
        ),
        (
            "replacement / range / after / adapter",
            "def main() -> None:\n    mut xs = [1, 2, 3]\n    selected = xs.iter().take(2)\n    println(selected.sum())\n    for value in range(2):\n        xs = [value]\n",
            false,
        ),
        (
            "mutation / other list / after / named",
            "def main() -> None:\n    mut xs = [1, 2, 3]\n    it = xs.iter()\n    println(it.collect())\n    for value in [4, 5]:\n        xs.append(value)\n",
            false,
        ),
        (
            "replacement / while / after / generator",
            "def main() -> None:\n    mut xs = [1, 2, 3]\n    generated = (value for value in xs)\n    println(generated.sum())\n    while len(xs) < 4:\n        xs = [4]\n",
            false,
        ),
        (
            "replacement / nested loops / after / adapter",
            "def main() -> None:\n    mut xs = [1, 2, 3]\n    selected = xs.iter().take(2)\n    println(selected.sum())\n    for outer in range(2):\n        for inner in [outer]:\n            xs = [inner]\n",
            false,
        ),
    ];
    for (label, source, conflict) in cells {
        check_cell(label, source, conflict)?;
    }
    Ok(())
}

/// Ordinary source constructs after the last reader use remain provable, including eager comprehension loops and
/// control-flow exits. None may turn a resolved lazy-reader lifetime into the compiler-facing T0123 refusal.
#[test]
fn constructs_after_last_reader_use_remain_accepted() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def show(value: int) -> None:\n    println(value)\ndef decide(flag: bool) -> list[int]:\n    mut xs = [1, 2, 3]\n    it = xs.iter()\n    println(it.collect())\n    println(abs(-2))\n    for value in range(2):\n        show(value)\n    for value in [4, 5]:\n        show(value)\n    mut count = 0\n    while count < 2:\n        count += 1\n    doubled = [value * 2 for value in [4, 5]]\n    match count:\n        2 => show(count)\n        _ => show(0)\n    for value in doubled:\n        if value == 8:\n            continue\n        if value == 10:\n            break\n        show(value)\n    if flag:\n        return [4]\n    xs = doubled\n    return xs\ndef main() -> None:\n    println(decide(false))\n";
    check_cell("ordinary constructs after last use", source, false)
}

/// A loop-carried adapter conflict reports its origin, replacement and next iteration use exactly once.
#[test]
fn loop_carried_reader_reports_exact_source_places() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def double(value: int) -> int:\n    return value * 2\ndef main() -> None:\n    mut xs = [1, 2, 3]\n    doubled = xs.iter().map(double)\n    for round in range(2):\n        xs = [round]\n        println(doubled.collect())\n";
    let checker = checked(source)?;
    assert_eq!(checker.errors.len(), 1, "{:?}", checker.errors);
    let conflict = checker.errors.first().ok_or("missing loop-carried iterator conflict")?;
    assert_eq!(conflict.stable_code(), Some("INCAN-T0001"));
    assert_eq!(
        conflict.message,
        "cannot replace `xs` while `doubled` is still reading it"
    );
    assert_eq!(source.get(conflict.span.start..conflict.span.end), Some("xs"));
    let related = conflict.related_spans();
    assert_eq!(related.len(), 3, "{conflict:?}");
    assert_eq!(
        source.get(related[0].span.start..related[0].span.end),
        Some("xs.iter().map(double)")
    );
    assert_eq!(related[0].label, "`doubled` reads `xs` from here");
    assert_eq!(source.get(related[1].span.start..related[1].span.end), Some("xs"));
    assert_eq!(related[1].label, "`xs` is replaced here");
    assert_eq!(source.get(related[2].span.start..related[2].span.end), Some("doubled"));
    assert_eq!(related[2].label, "`doubled` is used again here");
    assert_eq!(
        conflict.hints(),
        ["finish using `doubled` before replacing `xs`, or write `xs.clone().iter()` to iterate over a copy"]
    );
    Ok(())
}
