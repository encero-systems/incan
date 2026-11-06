//! Generic trait inputs preserve checked adoption arguments and the concrete hidden input carrier.

use super::{IncanType, build, named_targets};
use incan_semantics_core::IncanPrimitiveType;

/// The original's DataSet[int] adoption closes T before the hidden DataFrame[int] argument is appended.
#[test]
fn generic_trait_input_retains_complete_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/lowering_expressions/trait_typed_receivers.incn"
        ),
        &["m", "trait_input_arguments"],
    )?;
    let targets = named_targets(&module, "main");
    let target = targets
        .iter()
        .find(|target| target.name == "display")
        .ok_or("missing display")?;
    assert_eq!(target.type_args.len(), 2, "{target:?}");
    assert_eq!(target.type_args[0], IncanType::Primitive(IncanPrimitiveType::Int));
    assert_eq!(
        target.type_args[1],
        IncanType::Generic {
            base: "DataFrame".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Int)]
        }
    );
    let module = build(
        "trait DataSet[T]:\n    def plan(self) -> int: ...\n\nmodel FloatFrame with DataSet[float]:\n    rows: int\n    def plan(self) -> int:\n        return self.rows\n\ndef display[T](data: DataSet[T]) -> int:\n    return data.plan()\n\ndef main() -> None:\n    println(display(FloatFrame(rows=7)))\n",
        &["m", "trait_input_adoption"],
    )?;
    let targets = named_targets(&module, "main");
    let target = targets
        .iter()
        .find(|target| target.name == "display")
        .ok_or("missing adoption control")?;
    assert_eq!(
        target.type_args,
        vec![
            IncanType::Primitive(IncanPrimitiveType::Float),
            IncanType::Named("FloatFrame".into())
        ]
    );
    Ok(())
}

/// Type-level generated metadata already carries its exact owner and generated member span, not an imported body.
#[test]
fn static_metadata_retains_generated_owner_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "pub model Report:\n    pub label: str\n\ndef main() -> None:\n    for field in Report.__fields__():\n        println(field.type_name)\n",
        &["m", "static_metadata"],
    )?;
    let owner = module
        .nominal_declarations
        .iter()
        .find(|item| item.name == "Report")
        .ok_or("missing Report")?;
    let targets = named_targets(&module, "main");
    let target = targets
        .iter()
        .find(|target| {
            target
                .canonical
                .as_ref()
                .is_some_and(|identity| identity.declaration_name == "__fields__")
        })
        .ok_or("missing static metadata")?;
    let identity = target.canonical.as_ref().ok_or("missing generated identity")?;
    assert_eq!(identity.origin, owner.canonical.origin);
    assert_eq!(identity.declaration_span, owner.canonical.declaration_span);
    assert_eq!(target.receiver_type, Some(IncanType::Named("Report".into())));
    assert!(target.type_args.is_empty());
    Ok(())
}
