//! Canonical storage identity for same-spelled declarations imported from distinct modules.

use super::identities_and_imports::build_with_published_imports;
use super::{body_named, build};

/// Imported alias projection requires a complete exact context and never selects between competing checked owners.
#[test]
fn imported_alias_leaf_projection_preserves_owner_authority() -> Result<(), Box<dyn std::error::Error>> {
    use super::identities_and_imports::build_with_published_imports_and_facts;
    let first_source = "pub model Item:\n    pub value: int\n\npub type Choice = Item | str\n";
    let second_source = "pub model Item:\n    pub value: bool\n\npub type Choice = Item | str\n";
    let imports = [
        ("first", &["first"][..], first_source),
        ("second", &["second"][..], second_source),
    ];
    let first = build_with_published_imports(first_source, &["first"], &[], &[])?;
    let second = build_with_published_imports(second_source, &["second"], &[], &[])?;
    let owner = first
        .nominal_declarations
        .first()
        .ok_or("missing checked first owner")?
        .canonical
        .clone();
    let (_, facts) = build_with_published_imports_and_facts(
        "from first import Choice\n\ndef hold(value: Choice) -> None:\n    pass\n",
        &["consumer"],
        &imports[..1],
        &[],
    )?;
    assert!(!facts.declarations.named_type_identities.contains_key("Item"));
    let key = super::bir::nominal_carrier_key(&owner).ok_or("missing checked Item carrier")?;
    let mut accepted = facts.clone();
    super::super::nominal_carriers::project_imported_alias_identities(&mut accepted, std::slice::from_ref(&first));
    assert_eq!(
        accepted.declarations.named_type_identities.get(&key),
        Some(&owner),
        "imports={:?}; aliases={:?}",
        facts.declarations.resolved_import_identities,
        first.type_alias_declarations
    );
    let mut repeated = facts.clone();
    super::super::nominal_carriers::project_imported_alias_identities(&mut repeated, &[first.clone(), first.clone()]);
    assert_eq!(repeated.declarations.named_type_identities.get(&key), Some(&owner));
    for mutation in 0..4 {
        let mut context = vec![first.clone()];
        match mutation {
            0 => context.clear(),
            1 => context[0].type_alias_declarations[0].named_type_identities.clear(),
            2 => {
                let foreign = second
                    .nominal_declarations
                    .first()
                    .ok_or("missing foreign owner")?
                    .canonical
                    .clone();
                context[0].type_alias_declarations[0]
                    .named_type_identities
                    .insert(key.clone(), foreign);
            }
            _ => {
                let mut conflicting = context[0].clone();
                conflicting.type_alias_declarations[0].ty = super::IncanType::Primitive(super::IncanPrimitiveType::Int);
                context.push(conflicting);
            }
        }
        let mut refused = facts.clone();
        super::super::nominal_carriers::project_imported_alias_identities(&mut refused, &context);
        assert_eq!(
            refused.declarations.named_type_identities, facts.declarations.named_type_identities,
            "mutation={mutation}"
        );
    }
    let other_source = "pub model Other:\n    pub flag: bool\n";
    let shadow_imports = [
        ("first", &["first"][..], first_source),
        ("other", &["other"][..], other_source),
    ];
    let (_, mut shadowed) = build_with_published_imports_and_facts(
        "from first import Choice\nfrom other import Other as Item\n\ndef hold(value: Choice) -> None:\n    pass\n",
        &["shadowed"],
        &shadow_imports,
        &[],
    )?;
    let original = shadowed.declarations.named_type_identities.clone();
    assert_eq!(
        original
            .get("Item")
            .ok_or("missing checked shadowing import")?
            .declaration_name,
        "Other"
    );
    super::super::nominal_carriers::project_imported_alias_identities(&mut shadowed, std::slice::from_ref(&first));
    assert_eq!(shadowed.declarations.named_type_identities, original);
    let (_, mut competing) = build_with_published_imports_and_facts(
        "from first import Choice as FirstChoice\nfrom second import Choice as SecondChoice\n\ndef hold(one: FirstChoice, two: SecondChoice) -> None:\n    pass\n",
        &["competing"],
        &imports,
        &[],
    )?;
    let original = competing.declarations.named_type_identities.clone();
    super::super::nominal_carriers::project_imported_alias_identities(&mut competing, &[first, second]);
    assert_eq!(competing.declarations.named_type_identities, original);
    Ok(())
}

/// Build a checked original with the same nominal-retaining entrypoint used by source project compilation.
fn project(source: &str, leaf: &str) -> Result<super::bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = super::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["m".to_owned(), leaf.to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(super::build_body_ir_module_v0_with_executable_context(
        &program,
        &path,
        checker.type_info(),
        &[],
    ))
}

/// Nested ingress and payload reads keep concrete locals throughout the unchanged original program.
#[test]
fn original_nested_newtype_locals_keep_checked_types() -> Result<(), Box<dyn std::error::Error>> {
    let module = project(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_implicit_coercion.incn"
        ),
        "nested_ingress",
    )?;
    for body in &module.bodies {
        assert!(
            body.locals.iter().all(|local| local.ty != super::IncanType::Unknown),
            "{}",
            body.render_snapshot()
        );
    }
    Ok(())
}

/// A checked local opaque producer carries its concrete receiver through the fallible trait's Self-returning slot.
#[test]
fn original_fallible_trait_frame_keeps_concrete_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/fallible_iteration_protocol.incn"
    );
    for module in [
        build(source, &["m", "fallible_carrier"])?,
        project(source, "fallible_carrier")?,
    ] {
        let body = body_named(&module, "drain_trait")?;
        assert!(
            !body
                .locals
                .iter()
                .any(|local| matches!(&local.ty, super::IncanType::Generic { base, .. } if base == "FallibleStream")),
            "{}",
            body.render_snapshot()
        );
    }
    Ok(())
}

/// The original generic method consumer retains closed checked locals before native instance planning.
#[test]
fn original_generic_method_use_sites_keep_closed_locals() -> Result<(), Box<dyn std::error::Error>> {
    let module = project(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/method_type_param_clone_bound.incn"
        ),
        "method_generic_carriers",
    )?;
    let main = body_named(&module, "main")?;
    assert!(
        !serde_json::to_string(&main.locals)?.contains("TypeVar"),
        "{}",
        main.render_snapshot()
    );
    for name in ["pick", "pick_plain", "decode"] {
        assert_eq!(body_named(&module, name)?.type_parameters, ["T"], "{name}");
    }
    Ok(())
}

/// A partial constructor reexport retains its exact provider owner even when the consumer imports no type name.
#[test]
fn original_partial_reexport_keeps_result_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let provider_source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/codegen_modules_and_imports/reexported_partial_and_static/provider.incn"
    );
    let facade_source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/codegen_modules_and_imports/reexported_partial_and_static/facade.incn"
    );
    let consumer_source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/codegen_modules_and_imports/reexported_partial_and_static/main.incn"
    );
    let imports = [
        ("provider", &["provider"][..], provider_source),
        ("facade", &["facade"][..], facade_source),
    ];
    let provider = build_with_published_imports(provider_source, &["provider"], &imports, &[])?;
    let owner = provider
        .nominal_declarations
        .first()
        .ok_or("missing original Spec owner")?
        .canonical
        .clone();
    let facade = build_with_published_imports(facade_source, &["facade"], &imports, std::slice::from_ref(&provider))?;
    let module = build_with_published_imports(consumer_source, &["main"], &imports, &[provider, facade])?;
    let main = body_named(&module, "main")?;
    let local = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("spec"))
        .ok_or("missing original spec binding")?;
    let super::IncanType::Named(key) = &local.ty else {
        return Err("original spec has no nominal carrier".into());
    };
    assert_eq!(
        main.named_type_identities.get(key),
        Some(&owner),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// Original facade forwarders preserve the canonical return owner across their source import aliases.
#[test]
fn original_facade_forwarders_keep_return_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let imports = [
        (
            "projection_builders",
            &["projection_builders"][..],
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/facade_reexport_alias_target/projection_builders.incn"
            ),
        ),
        (
            "aggregate_builders",
            &["aggregate_builders"][..],
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/facade_reexport_alias_target/aggregate_builders.incn"
            ),
        ),
        (
            "functions_references_col",
            &["functions", "references", "col"][..],
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/facade_reexport_alias_target/functions/references/col.incn"
            ),
        ),
        (
            "functions_aggregates_count",
            &["functions", "aggregates", "count"][..],
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/facade_reexport_alias_target/functions/aggregates/count.incn"
            ),
        ),
        (
            "functions",
            &["functions"][..],
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/facade_reexport_alias_target/functions.incn"
            ),
        ),
        (
            "main",
            &["main"][..],
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/facade_reexport_alias_target/main.incn"
            ),
        ),
    ];
    let mut context = Vec::new();
    for (name, path, source) in &imports {
        let module = build_with_published_imports(source, path, &imports, &context)?;
        for body in &module.bodies {
            if let super::IncanType::Named(key) = &body.return_type {
                assert!(
                    body.named_type_identities.contains_key(key),
                    "{name}: {}\n{:?}",
                    body.render_snapshot(),
                    body.named_type_identities
                );
            }
        }
        context.push(module);
    }
    Ok(())
}

/// The original package-owned union retains every named Result leaf when literals settle at its call boundary.
#[test]
fn original_dependency_result_union_keeps_payload_identities() -> Result<(), Box<dyn std::error::Error>> {
    use crate::library_exports::collect_checked_public_exports;
    use crate::library_manifest::LibraryManifest;
    use crate::library_manifest_index::{LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry};
    let provider_source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_dependencies/some_member_into_dependency_option_union/deps/unionlib/src/lib.incn"
    );
    let consumer_source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_dependencies/some_member_into_dependency_option_union/src/main.incn"
    );
    let tokens = super::lexer::lex(provider_source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = super::TypeChecker::new();
    checker.set_current_package_identity(Some("unionlib".into()));
    checker.set_current_module_path(Some(Vec::new()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let manifest =
        LibraryManifest::from_checked_exports("unionlib", "0.1.0", &collect_checked_public_exports(&program, &checker));
    let provider = super::build_body_ir_module_v0_with_executable_context(&program, &[], checker.type_info(), &[]);
    let index = LibraryManifestIndex::from_entries(
        [(
            "unionlib".into(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    "unionlib",
                    "unionlib",
                    std::path::PathBuf::from("tests/original-unionlib/target/lib"),
                ),
            },
        )]
        .into(),
    );
    let tokens = super::lexer::lex(consumer_source).map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["main".into()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_library_manifest_index(index);
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module =
        super::build_body_ir_module_v0_with_executable_context(&program, &path, checker.type_info(), &[provider]);
    let main = body_named(&module, "main")?;
    let mut constructions = 0;
    for statement in &main.block.stmts {
        if let super::bir::StatementKind::Assign {
            rvalue: super::bir::Rvalue::ResultVariant(value),
            ..
        } = &statement.kind
        {
            constructions += 1;
            assert!(
                value.has_complete_canonical_types(),
                "{value:?}\n{}",
                main.render_snapshot()
            );
        }
    }
    assert_eq!(constructions, 2);
    Ok(())
}

/// Same-spelled owners reached through distinct checked imports must not collapse to one local storage type.
#[test]
fn same_named_imported_constructor_results_keep_distinct_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let first_source = "pub model Product:\n    pub value: int\n";
    let second_source = "pub model Product:\n    pub value: str\n";
    let first = build(first_source, &["first"])?;
    let second = build(second_source, &["second"])?;
    let module = build_with_published_imports(
        "from first import Product as FirstProduct\nfrom second import Product as SecondProduct\n\ndef main() -> None:\n    first = FirstProduct(value=1)\n    second = SecondProduct(value=\"two\")\n    println(first.value)\n    println(second.value)\n",
        &["consumer"],
        &[
            ("first", &["first"], first_source),
            ("second", &["second"], second_source),
        ],
        &[first, second],
    )?;
    let main = body_named(&module, "main")?;
    let first = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("first"))
        .ok_or("missing first binding")?;
    let second = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("second"))
        .ok_or("missing second binding")?;
    assert_ne!(
        first.ty,
        second.ty,
        "{}\n{:?}",
        main.render_snapshot(),
        main.named_type_identities
    );
    Ok(())
}
