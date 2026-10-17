//! Checked SDK reader default signatures and authenticated constructor carrier facts.

use super::{body_named, build};

use crate::body_ir::{build_body_ir_module_v0, library_call_delegations};
use crate::typechecker::{TypeCheckInfo, TypeChecker, stdlib_loader::StdlibAstCache};
use crate::{lexer, parser};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, SymbolOrigin};

const READER: &str = r#"from std.io import BinaryReader, BytesIO

def main() -> None:
    reader = BytesIO(b"abcd")
    stream = reader.chunks(2)
    println(stream.chunk_size)
"#;

/// Check genuine source and retain both its method selection and ordinary module facts for collector controls.
fn checked_reader() -> Result<(TypeCheckInfo, CanonicalSymbolId), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(READER).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["reader_default".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let target = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .find_map(|statement| match &statement.kind {
            incan_semantics_core::body_ir::StatementKind::Call {
                callee: incan_semantics_core::body_ir::Callee::Method(target),
                ..
            } => target.canonical.clone(),
            _ => None,
        })
        .ok_or_else(|| format!("checked reader method target absent: {module:?}"))?;
    assert_eq!(target.kind, SemanticSourceTargetKind::Method);
    assert_eq!(target.declaration_name, "chunks");
    Ok((checker.type_info().clone(), target))
}

/// Rebase one authentic std.io source declaration onto the fixed fixture's real SDK provider.
fn published(mut identity: CanonicalSymbolId) -> CanonicalSymbolId {
    identity.origin = SymbolOrigin::Package {
        library: "incan_stdlib_system".to_owned(),
        module_path: vec!["io".to_owned()],
    };
    identity
}

/// The source-origin view retains its checked slot while naming the authenticated SDK trait declaration.
#[test]
fn reader_default_retains_exact_checked_slot_and_self_contract() -> Result<(), Box<dyn std::error::Error>> {
    let (facts, checked) = checked_reader()?;
    assert_selected(&facts, &checked)
}

/// The Package view of the same complete slot retains the same declared SDK Self contract.
#[test]
fn reader_default_accepts_the_exact_published_slot_view() -> Result<(), Box<dyn std::error::Error>> {
    let (mut facts, checked) = checked_reader()?;
    let sdk = published(checked.clone());
    for identity in facts.references.resolved_identities.values_mut() {
        if identity == &checked {
            *identity = sdk.clone();
        }
    }
    assert_selected(&facts, &sdk)
}

/// Require real source declarations and the exact Self binder, shared receiver, and declared result template.
fn assert_selected(facts: &TypeCheckInfo, checked: &CanonicalSymbolId) -> Result<(), Box<dyn std::error::Error>> {
    let records = library_call_delegations(facts);
    let selected = records
        .iter()
        .filter(|record| &record.canonical == checked)
        .collect::<Vec<_>>();
    let [record] = selected.as_slice() else {
        return Err(format!("one exact SDK default record required for {checked:?}; actual {records:?}").into());
    };
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "io".to_owned()];
    let owner = published(
        cache
            .lookup_identity(&path, "BinaryReader")
            .ok_or("source reader trait owner absent")?,
    );
    assert!(cache.callable_source_identity(&owner).is_some());
    let slot = published(checked.clone());
    let source = cache
        .lookup_trait_decl(&path, "BinaryReader")
        .ok_or("source reader trait absent")?;
    assert!(source.methods.iter().any(|method| {
        method.span.start == slot.declaration_span.start && method.span.end == slot.declaration_span.end
    }));
    assert_eq!(owner.kind, SemanticSourceTargetKind::Trait);
    assert_eq!(owner.origin, slot.origin);
    assert!(slot.declaration_span.start >= owner.declaration_span.start);
    assert!(slot.declaration_span.end <= owner.declaration_span.end);
    assert_eq!(
        record.rust_path(),
        Some("incan_stdlib_system::io::BinaryReader::chunks".to_owned())
    );
    assert_eq!(record.canonical, *checked);
    assert_eq!(record.type_parameters, vec!["Self".to_owned()]);
    assert!(record.parameter_defaults.is_empty());
    assert_eq!(
        record.parameters,
        vec![
            incan_semantics_core::IncanType::Ref(Box::new(incan_semantics_core::IncanType::TypeVar("Self".to_owned()))),
            incan_semantics_core::IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int),
        ]
    );
    assert_eq!(
        record.return_type,
        incan_semantics_core::IncanType::Generic {
            base: "ReaderChunks".to_owned(),
            args: vec![incan_semantics_core::IncanType::TypeVar("Self".to_owned())],
        }
    );
    Ok(())
}

/// Removing the checked slot identity cannot be compensated by a retained owner or similarly named member.
#[test]
fn reader_default_requires_checked_slot_identity() -> Result<(), Box<dyn std::error::Error>> {
    let (mut facts, checked) = checked_reader()?;
    assert_selected(&facts, &checked)?;
    facts
        .references
        .resolved_identities
        .retain(|_, identity| identity != &checked);
    assert!(
        library_call_delegations(&facts)
            .iter()
            .all(|record| record.canonical != checked)
    );
    Ok(())
}

/// A changed publication package or declaration span grants no default for a genuine SDK receiver.
#[test]
fn reader_default_requires_complete_published_identity() -> Result<(), Box<dyn std::error::Error>> {
    let (facts, checked) = checked_reader()?;
    assert_selected(&facts, &checked)?;
    let mut wrong_span = published(checked.clone());
    wrong_span.declaration_span.start += 1;
    let mut wrong_package = published(checked.clone());
    wrong_package.origin = SymbolOrigin::Package {
        library: "foreign_system".to_owned(),
        module_path: vec!["io".to_owned()],
    };
    for altered in [wrong_span, wrong_package] {
        let mut damaged = facts.clone();
        for identity in damaged.references.resolved_identities.values_mut() {
            if identity == &checked {
                *identity = altered.clone();
            }
        }
        assert!(
            library_call_delegations(&damaged)
                .iter()
                .all(|record| record.canonical != altered)
        );
    }
    Ok(())
}

/// A selected SDK constructor remains a template over the caller's declared binder, ready for checked specialization.
#[test]
fn sdk_generic_reader_adapter_retains_its_constructor_template() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/std_fallible_adapter_loop.incn"
        ),
        &["m", "generic_reader_adapter"],
    )?;
    let body = body_named(&module, "consume")?;
    assert!(
        body.named_type_identities.contains_key("ReaderChunks"),
        "receiver identities: {:?}",
        body.named_type_identities
    );
    assert!(
        body.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, args } if base == "MapErrFallibleIterator" && args.len() == 5
        )),
        "{}",
        body.render_snapshot()
    );
    assert!(
        !body.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, .. } if base == "FallibleIterator"
        )),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// SDK default adapter calls retain the concrete constructors emitted by legacy, without supplying their bodies.
#[test]
fn sdk_fallible_adapters_retain_concrete_carriers() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/fallible_iterator_take.incn"
        ),
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_derives_collection_adapters.incn"
        ),
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_derives_collection_adapters_with_imported_clone.incn"
        ),
    ] {
        let module = build(source, &["m", "sdk_opaque"])?;
        let body = body_named(&module, "main")?;
        assert!(
            !body
                .locals
                .iter()
                .any(|local| matches!(&local.ty, IncanType::Generic { base, .. } if base == "FallibleIterator")),
            "{}",
            body.render_snapshot()
        );
        assert!(
            body.named_type_identities
                .values()
                .any(|identity| identity.declaration_name.ends_with("FallibleIterator")
                    && identity.kind == SemanticSourceTargetKind::Model),
            "the checked concrete adapter owner must accompany its carrier: {}",
            body.render_snapshot()
        );
    }
    Ok(())
}

/// An SDK trait parameter has no known concrete producer, so its adapter remains opaque.
#[test]
fn sdk_unknown_trait_parameter_remains_opaque() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.derives.collection import FallibleIterator\n\ndef adapt(stream: FallibleIterator[int, str]) -> FallibleIterator[int, str]:\n    return stream.take(1)\n",
        &["m", "sdk_unknown_opaque"],
    )?;
    let body = body_named(&module, "adapt")?;
    assert!(
        body.locals
            .iter()
            .any(|local| matches!(&local.ty, IncanType::Generic { base, .. } if base == "FallibleIterator"))
    );
    assert!(
        !body
            .named_type_identities
            .values()
            .any(|identity| identity.declaration_name == "TakeFallibleIterator")
    );
    Ok(())
}

/// Published views of exact SDK slots preserve the same constructor and Self-returning adapter carriers.
#[test]
fn sdk_reader_published_adapter_slots_preserve_concrete_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/std_fallible_adapter_loop.incn"
    );
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["reader_package_slots".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut facts = checker.type_info().clone();
    for identity in facts.references.resolved_identities.values_mut().chain(
        facts
            .protocols
            .body_ir_iterations
            .values_mut()
            .flat_map(|hooks| [&mut hooks.iter, &mut hooks.next])
            .filter_map(|hook| hook.canonical.as_mut()),
    ) {
        let SymbolOrigin::Module(path) = &identity.origin else {
            continue;
        };
        // These are the actual providers of this fixture's io and collection slots, independent of the collector.
        let library = match path.as_slice() {
            [root, module, ..] if root == "std" && module == "io" => "incan_stdlib_system",
            [root, module, ..] if root == "std" && module == "derives" => "incan_stdlib_core",
            _ => continue,
        };
        identity.origin = SymbolOrigin::Package {
            library: library.to_owned(),
            module_path: path[1..].to_vec(),
        };
    }
    let module = build_body_ir_module_v0(&program, &path, &facts);
    let body = body_named(&module, "consume")?;
    assert!(
        !body.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, .. } if base == "FallibleIterator"
        )),
        "{}",
        body.render_snapshot()
    );
    assert!(body.named_type_identities.contains_key("ReaderChunks"));
    Ok(())
}

/// A checked captured factory retains its complete method frame and authenticated concrete result before native
/// lowering.
#[test]
fn captured_sdk_factory_keeps_method_arguments_and_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.io import BinaryReader, BytesIO

pub def consume[R with BinaryReader](reader: R) -> None:
    prefix = "read failed: "
    mapped = reader.chunks(16).map_err((error) => f"{prefix}{error.detail}")
    println("created")
    println(prefix)

def main() -> None:
    consume(BytesIO(b"0123456789abcdefghij"))
"#;
    let module = build(source, &["m", "captured_sdk_factory"])?;
    let consume = body_named(&module, "consume")?;
    let selected = consume
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            incan_semantics_core::body_ir::StatementKind::Call {
                callee: incan_semantics_core::body_ir::Callee::Method(target),
                ..
            } if target.name == "map_err" => Some(target),
            _ => None,
        })
        .ok_or("checked map_err method absent")?;
    assert_eq!(selected.type_args.len(), 2, "{}", consume.render_snapshot());
    let mapped = consume
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("mapped"))
        .ok_or("mapped local absent")?;
    assert!(
        matches!(&mapped.ty, IncanType::Generic { base, args } if base == "MapErrFallibleIterator" && args.len() == 5),
        "{}",
        consume.render_snapshot()
    );
    Ok(())
}

/// Replacing a trait-facing binding with another adapter must not assign the first producer's fixed physical carrier.
#[test]
fn reassigned_sdk_factory_binding_remains_opaque() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"from std.io import BinaryReader, BytesIO

pub def consume[R with BinaryReader](reader: R) -> None:
    prefix = "read failed: "
    mut mapped = reader.chunks(16).map_err((error) => f"{prefix}{error.detail}")
    mapped = mapped.take(1)
    println("created")

def main() -> None:
    consume(BytesIO(b"0123456789abcdefghij"))
"#,
        &["m", "reassigned_sdk_factory"],
    )?;
    let consume = body_named(&module, "consume")?;
    let mapped = consume
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("mapped"))
        .ok_or("mapped local absent")?;
    assert!(
        matches!(&mapped.ty, IncanType::Generic { base, .. } if base == "FallibleIterator"),
        "{}",
        consume.render_snapshot()
    );
    Ok(())
}
