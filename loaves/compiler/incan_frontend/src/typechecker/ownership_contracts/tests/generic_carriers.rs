//! A value that retains a dependency keeps it through every use: borrowing a dependent local for a call reads its
//! dependency, and a generic carrier instantiated with a view type holds that view through its type argument, so a
//! method whose result holds a borrow only through the carrier's type argument retains the dependencies its receiver
//! carries (#2972, RFC 164: dependencies propagate through returned structures and calls).

use super::*;

/// The carrier declarations from #2972: `Envelope.select` returns a `Selected` view of its plan, and
/// `Selected.walker` returns a `Walker` holding the same plan through the type argument.
const CARRIERS: &str = "trait Plan:\n    def width(self) -> int:\n        pass\n\nclass StoredPlan with Plan:\n    pub value: int\n\n    def width(self) -> int:\n        return self.value\n\nclass Walker[P with Plan]:\n    pub plan: P\n    total: int = 0\n\n    pub def write(mut self, value: int) -> None:\n        self.total += value + self.plan.width()\n\n    pub def finish(self) -> int:\n        return self.total\n\nmodel Selected[P with Plan]:\n    plan: P\n\n    pub def walker(self) -> Walker[P]:\n        return Walker(plan=self.plan)\n\n    pub def joined(self, other: Selected[P]) -> Walker[P]:\n        return Walker(plan=other.plan)\n\nclass Envelope:\n    pub plan: StoredPlan\n    pub total: int = 0\n\n    pub def select(self) -> Selected[Borrowed[StoredPlan]] from self.plan:\n        return Selected[Borrowed[StoredPlan]](plan=self.plan)\n\n    pub def append(mut self, value: int) -> None:\n        self.total += value\n\n";

/// `main`'s body after the declarations, checked with its outcome per body.
fn main_body(body: &str) -> Result<TypeChecker, Box<dyn std::error::Error>> {
    checked(&format!(
        "{CARRIERS}def main() -> None:\n    mut envelope = Envelope(plan=StoredPlan(value=2), total=0)\n{body}"
    ))
}

/// Borrowing the selected view as a receiver after the envelope changes is refused: the borrow reads the view's
/// dependency on the plan. Borrowing it before the change is sound.
#[test]
fn dependent_local_borrowed_after_its_source_changes_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let body = "    selected = envelope.select()\n    envelope.append(1)\n    mut walker = selected.walker()\n    walker.write(3)\n";
    let checker = main_body(body)?;
    assert!(
        !checker.errors.is_empty(),
        "admitted a borrow after the source changed:\n{body}\n{:?}",
        flow_summary(&checker)
    );
    let checker =
        main_body("    selected = envelope.select()\n    mut walker = selected.walker()\n    envelope.append(1)\n")?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

/// The walker carries the selected plan: it is consumed before the envelope changes in the sound body, and a use
/// after the change is refused.
#[test]
fn generic_carrier_results_retain_carried_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let checker = main_body(
        "    selected = envelope.select()\n    mut walker = selected.walker()\n    walker.write(3)\n    completed = walker.finish()\n    envelope.append(completed)\n    assert envelope.total == 5\n",
    )?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert!(
        flow_summary(&checker)
            .iter()
            .any(|(name, outcome)| name == "main" && outcome == "checked []"),
        "{:?}",
        flow_summary(&checker)
    );
    let body = "    selected = envelope.select()\n    mut walker = selected.walker()\n    walker.write(3)\n    envelope.append(1)\n    walker.write(4)\n";
    let checker = main_body(body)?;
    assert!(
        !checker.errors.is_empty(),
        "admitted a carried use after the source changed:\n{body}\n{:?}",
        flow_summary(&checker)
    );
    Ok(())
}

/// A carried result keeps its dependencies only in a local binding, and only from one carrying input: a carried
/// walker used as a temporary, and a walker built from two selected views, are refused.
#[test]
fn carried_results_need_a_local_and_one_source() -> Result<(), Box<dyn std::error::Error>> {
    for (body, reason) in [
        (
            "    selected = envelope.select()\n    assert selected.walker().finish() == 0\n",
            "a carried result needs a local binding",
        ),
        (
            "    first = envelope.select()\n    second = envelope.select()\n    walker = first.joined(second)\n    assert walker.finish() == 0\n",
            "a carried result depends on more than one input",
        ),
    ] {
        let checker = main_body(body)?;
        assert!(
            flow_summary(&checker)
                .iter()
                .any(|(name, outcome)| name == "main" && outcome.contains(reason)),
            "{body}\n{:?}",
            flow_summary(&checker)
        );
    }
    Ok(())
}
