//! Closed SDK rest arguments keep their checked native carrier and exact call binding.

use super::{IncanType, bir, body_named, build};

/// The unchanged race original packs its checked arms once rather than allocating an untyped refusal temporary.
#[test]
fn sdk_race_packs_closed_arm_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_async_race.incn");
    let module = build(source, &["m", "closed_sdk_race_rest"])?;
    let main = body_named(&module, "main")?;
    let arguments = main
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                args,
                ..
            } if target.name == "race" => Some(args),
            _ => None,
        })
        .ok_or("missing checked race call")?;
    let [bir::ArgumentElement::One(bir::Operand::Place(read))] = arguments.as_slice() else {
        return Err("race requires exactly one owned arm pack".into());
    };
    let local = main
        .locals
        .get(read.place.local_id().ok_or("pack local")?.index())
        .ok_or("pack storage")?;
    assert_eq!(local.ty.to_string(), "List[RaceArm[str]]", "{}", main.render_snapshot());
    assert!(main.locals.iter().all(|local| local.ty != IncanType::Unknown));
    Ok(())
}

/// A source-owned enum with the SDK's spelling does not authorize its native rest ABI.
#[test]
fn source_race_arm_rest_keeps_named_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "enum RaceArm:\n    Own(int)\n\ndef choose(*arms: RaceArm) -> None:\n    pass\n\ndef main() -> None:\n    choose(RaceArm.Own(1))\n",
        &["m", "source_race_rest"],
    )?;
    let main = body_named(&module, "main")?;
    assert!(
        main.render_snapshot()
            .contains("rest call with an unretained concrete pack element"),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}
