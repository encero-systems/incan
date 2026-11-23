//! A call whose result is a closed carrier of declared nominals that hold no borrow is a fresh value: it retains no
//! dependency on the call's inputs, so it needs no generic result requirement for lowering to discharge (#2994,
//! #3022). A leaf that may still be a type parameter keeps its requirement.

use super::*;

/// The invocations of the named body, from its proved flow.
fn invocations<'a>(checker: &'a TypeChecker, name: &str) -> Result<Vec<&'a flow::Invocation>, String> {
    let (_, flow) = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == name)
        .ok_or(format!("missing flow for {name}"))?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err(format!("{name} is not proved: {flow:?}"));
    };
    Ok(graph.invocations.values().collect())
}

/// `Result[Slot, str]` from a method with a borrowed input is a fresh value, taken with `?`.
#[test]
fn closed_nominal_carrier_results_need_no_requirement() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Slot:\n    n: int\n\nclass Reader:\n    base: int\n\n    def root_slot(self, shape: Borrowed[Slot]) -> Result[Slot, str]:\n        return Ok(Slot(n=shape.n + self.base))\n\ndef main() -> Result[None, str]:\n    reader = Reader(base=1)\n    shape = Slot(n=2)\n    selected = reader.root_slot(shape)?\n    println(selected.n)\n    return Ok(None)\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let calls = invocations(&checker, "main")?;
    assert!(
        calls.iter().all(|call| call.result_requirement.is_none()),
        "{:?}",
        calls.iter().map(|call| &call.result_requirement).collect::<Vec<_>>()
    );
    Ok(())
}
