//! Concrete producers of local trait-returning functions retain the opaque carrier legacy emits.

use super::{IncanPrimitiveType, IncanType, body_named, build};

/// An SDK default on a qualified source receiver retains its checked concrete adapter rather than an opaque trait.
#[test]
fn sdk_linked_iterator_adopter_retains_checked_factory_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/iterator_adopters_spelled_through_their_module.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = super::TypeChecker::new();
    let path = vec!["main".to_owned()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let (module, _, _) =
        super::super::build_body_ir_module_v0_with_sdk_linkage(&program, &path, checker.type_info(), &[]);
    let main = body_named(&module, "main")?;
    assert!(
        !main.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, .. } if base == "Iterator"
        )),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// The SDK-linked collector qualifies source carriers before refining local opaque adapter returns.
#[test]
fn sdk_linked_fallible_chain_retains_checked_adapter_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/fallible_iterator_adapter_chain.incn"
    );
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = super::TypeChecker::new();
    let path = vec!["main".to_owned()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let (module, _, _) =
        super::super::build_body_ir_module_v0_with_sdk_linkage(&program, &path, checker.type_info(), &[]);
    let main = body_named(&module, "main")?;
    assert!(
        !main.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, .. } if base == "FallibleStream"
        )),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// The original local generic adapter chain retains concrete checked carriers before native lowering sees its locals.
#[test]
fn original_fallible_chain_retains_checked_adapter_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/fallible_iterator_adapter_chain.incn"
        ),
        &["m", "original_fallible_chain"],
    )?;
    let main = body_named(&module, "main")?;
    let opaque = main
        .locals
        .iter()
        .any(|local| matches!(&local.ty, IncanType::Generic { base, .. } if base == "FallibleStream"));
    if opaque {
        eprintln!("trait records: {:?}", module.trait_implementations);
        eprintln!(
            "retained owners: {:?}",
            module
                .nominal_declarations
                .iter()
                .map(|owner| (&owner.name, &owner.type_parameters))
                .collect::<Vec<_>>()
        );
        for body in &module.bodies {
            eprintln!(
                "return witness {} parameters={:?} return={:?} identities={:?}",
                body.name, body.type_parameters, body.return_type, body.named_type_identities
            );
            eprintln!("{}", body.render_snapshot());
        }
    }
    assert!(!opaque, "{}", main.render_snapshot());
    Ok(())
}

/// A checked generic method's constructor return grants its own concrete witness without relying on a Self-return slot.
#[test]
fn generic_method_return_retains_constructor_witness() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"trait Read:
    """Declare the interface of the returned value."""
    pass

model Box[T with Clone] with Read:
    """Store the checked generic argument."""
    value: T

model Factory:
    """Return a newly constructed value through its declared interface."""
    def make[T with Clone](self, value: T) -> Read:
        """Retain the concrete Box witness independently of the receiver."""
        return Box[T](value=value)

def main() -> None:
    """Keep the checked generic method result in a binding."""
    result = Factory().make(7)
"#,
        &["m", "generic_method_witness"],
    )?;
    let main = body_named(&module, "main")?;
    assert!(
        main.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, args } if base == "Box" && args == &[IncanType::Primitive(IncanPrimitiveType::Int)]
        )),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// Trait and nominal binders with swapped names retain their distinct checked scopes when a method returns a carrier.
#[test]
fn method_return_witness_keeps_swapped_adoption_binders() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"trait Read:
    """Declare the return interface."""
    pass

model Box[T with Clone] with Read:
    """Keep the represented payload."""
    value: T

trait Source[T with Clone, E with Clone]:
    """Use separate owner binders for the selected result."""
    def second(self) -> E: ...
    def get[U with Clone](self, witness: U) -> Read:
        """Return the trait's second argument through a concrete constructor."""
        return Box[E](value=self.second())

model Pair[T with Clone, E with Clone] with Source[E, T]:
    """Swap the adoption's arguments without capturing either declaration's binder."""
    value: T
    marker: E
    def second(self) -> T:
        """Read the payload belonging to the nominal's first argument."""
        return self.value

def main() -> None:
    """The trait's E is the nominal's T, which is int here."""
    result = Pair[int, str](value=7, marker="marker").get(true)
"#,
        &["m", "swapped_method_witness"],
    )?;
    let main = body_named(&module, "main")?;
    assert!(
        main.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, args } if base == "Box" && args == &[IncanType::Primitive(IncanPrimitiveType::Int)]
        )),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// A default template may bind its retained nominal's parameters; foreign names and unresolved types grant no witness.
#[test]
fn default_template_arguments_require_the_exact_owner_scope() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"trait Read[T]:
    """Keep the default's owner parameter scope."""
    def identity(self) -> Self:
        """Return the same concrete adopter."""
        return self

model Value[T with Clone] with Read[T]:
    """Retain the parameter the checked adoption binds."""
    value: T
"#,
        &["m", "default_template_scope"],
    )?;
    let record = module
        .trait_implementations
        .iter()
        .find(|record| record.method.declaration_name == "identity")
        .ok_or_else(|| {
            format!(
                "missing checked default implementation; owners={:?}; traits={:?}; records={:?}",
                module.nominal_declarations, module.trait_declarations, module.trait_implementations
            )
        })?;
    assert!(module.is_well_formed_trait_implementation(record));
    for argument in [
        IncanType::TypeVar("Foreign".into()),
        IncanType::SelfType,
        IncanType::Unknown,
    ] {
        let mut forged = record.clone();
        forged
            .type_arguments
            .first_mut()
            .ok_or("missing checked owner argument")?
            .argument = argument;
        assert!(!module.is_well_formed_trait_implementation(&forged));
    }
    let mut forged = record.clone();
    forged.owner.declaration_name = "MissingOwner".into();
    assert!(!module.is_well_formed_trait_implementation(&forged));
    Ok(())
}

/// An ordinary loop borrows the exact SDK adapter its source factory retained instead of an erased Iterator.
#[test]
fn sdk_adapter_iteration_keeps_checked_borrow_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let original = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/loops_over_calls_returning_iterator_adopters/main.incn"
    );
    let source = original
        .split("\n    clock = clock_at")
        .next()
        .ok_or("missing original local-adopter section")?
        .replace("from clocks import clock_at\n", "");
    let tokens = super::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = super::TypeChecker::new();
    let path = vec!["main".to_owned()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let (module, _, _) =
        super::super::build_body_ir_module_v0_with_sdk_linkage(&program, &path, checker.type_info(), &[]);
    let main = body_named(&module, "main")?;
    assert!(
        !main.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, .. } if base == "Iterator"
        )),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}
