//! Project carrier facts retain each source nominal's checked module identity before native layout assembly.

use super::identities_and_imports::build_with_published_imports;
use super::*;
use crate::library_exports::collect_checked_public_exports;
use crate::library_manifest::LibraryManifest;
use crate::library_manifest_index::{LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry};
use incan_semantics_core::SymbolOrigin;
use incan_semantics_core::executable_representation::{ExecutableDeclaration, SurfaceReader, build_surface};
use std::collections::BTreeSet;

/// A dependency payload pattern uses the consumer's checked import alias, even when its signature names the owner.
#[test]
fn nested_dependency_pattern_retains_checked_import_alias() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_public_consumer(
        &[(
            "records",
            "pub model Nomination:\n    pub rank: int\n\npub enum Outcome:\n    Found(Nomination)\n    Missing\n\npub def best() -> Outcome:\n    return Outcome.Found(Nomination(rank=3))\n",
        )],
        "from pub::records import Outcome, best\nfrom pub::records import Nomination as RootNomination\n\ndef main() -> None:\n    match best():\n        case Outcome.Found(RootNomination(rank=rank)):\n            println(rank)\n        case Outcome.Missing:\n            pass\n",
    )?;
    let main = body_named(&module, "main")?;
    let pattern = main
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => arms.first().map(|arm| &arm.pattern),
            _ => None,
        })
        .ok_or("missing enum match")?;
    let bir::Pattern::Enum { fields, .. } = pattern else {
        return Err(format!("missing selected enum pattern: {pattern:?}").into());
    };
    let Some(bir::Pattern::Nominal { target, .. }) = fields.first() else {
        return Err(format!("missing canonical dependency payload pattern: {fields:?}").into());
    };
    assert_eq!(target.canonical.declaration_name, "Nomination");
    assert!(matches!(&target.canonical.origin, SymbolOrigin::Package { library, .. } if library == "records"));
    Ok(())
}

/// Retain producer-owned constructor field types even when the consumer imports only the constructor and factory.
fn constructor_field_consumer(shadow: bool) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let ids = "@derive(Eq, Hash)\npub type EvidenceId = newtype str\n";
    let records = "from ids import EvidenceId\n\npub model Decision:\n    pub items: list[EvidenceId]\n    pub first: Option[EvidenceId]\n    pub pairs: list[tuple[EvidenceId, int]]\n\npub def make_id(text: str) -> EvidenceId:\n    return EvidenceId(text)\n";
    let owner = build_with_published_imports(ids, &["ids"], &[], &[])?;
    let provider = build_with_published_imports(
        records,
        &["records"],
        &[("ids", &["ids"], ids)],
        std::slice::from_ref(&owner),
    )?;
    let local = if shadow {
        "@derive(Eq)\ntype EvidenceId = newtype int\n\n"
    } else {
        ""
    };
    let own = if shadow { "    own = EvidenceId(7)\n" } else { "" };
    let source = format!(
        "from records import Decision, make_id\n\n{local}def main() -> None:\n{own}    value = Decision(items=[], first=Some(make_id(\"e\")), pairs=[])\n    println(value.items == [])\n"
    );
    build_with_published_imports(
        &source,
        &["consumer"],
        &[("ids", &["ids"], ids), ("records", &["records"], records)],
        &[owner, provider],
    )
}

/// Empty collections and an enclosing Option retain the exact foreign newtype selected by their constructor field.
#[test]
fn constructor_arguments_keep_unimported_nominal_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let module = constructor_field_consumer(false)?;
    require_foreign_constructor_arguments(&module)?;
    require_foreign_comparison_arguments(&module)?;
    Ok(())
}

/// A consumer's same-named declaration stays independent from the foreign constructor's checked field slots.
#[test]
fn constructor_field_carriers_preserve_same_named_local_owner() -> Result<(), Box<dyn std::error::Error>> {
    let module = constructor_field_consumer(true)?;
    require_foreign_constructor_arguments(&module)?;
    require_foreign_comparison_arguments(&module)?;
    let body = body_named(&module, "main")?;
    let own = body.locals.iter().find(|local| local.name.as_deref() == Some("own"));
    let own = own.ok_or("local newtype value missing")?;
    let IncanType::Named(key) = &own.ty else {
        return Err("local newtype carrier missing".into());
    };
    let identity = body
        .named_type_identities
        .get(key)
        .ok_or("local owner identity missing")?;
    assert_eq!(
        identity.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["consumer".to_owned()])
    );
    Ok(())
}

/// Check each complete bound argument against the canonical provider field's retained type, including nested leaves.
fn require_foreign_constructor_arguments(module: &bir::BodyIrModule) -> Result<(), Box<dyn std::error::Error>> {
    let body = body_named(module, "main")?;
    for statement in &body.block.stmts {
        let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), arguments),
            ..
        } = &statement.kind
        else {
            continue;
        };
        if target.name != "Decision" {
            continue;
        }
        assert_eq!(arguments.len(), 3);
        for argument in arguments {
            let Some(bir::Operand::Place(read)) = argument.as_one() else {
                return Err("expected a retained field argument temporary".into());
            };
            let local = read.place.local_id().ok_or("field argument has no local")?;
            let ty = &body.locals[local.index()].ty;
            let mut leaves = Vec::new();
            super::super::named_leaf_paths(ty, &mut Vec::new(), &mut leaves);
            assert!(!leaves.is_empty(), "field argument lost its nominal leaf: {ty:?}");
            for (_, leaf) in leaves {
                let super::super::RetainedTypeLeaf::Named(key) = leaf else {
                    continue;
                };
                assert_ne!(key, "EvidenceId", "field argument lost its owner: {ty:?}");
                let identity = body
                    .named_type_identities
                    .get(&key)
                    .ok_or("field owner identity missing")?;
                assert_eq!(
                    identity.origin,
                    incan_semantics_core::SymbolOrigin::Module(vec!["ids".to_owned()])
                );
            }
        }
        return Ok(());
    }
    Err("selected Decision construction missing".into())
}

/// A checked field supplies the owner of an empty comparison operand without importing that owner into the caller.
fn require_foreign_comparison_arguments(module: &bir::BodyIrModule) -> Result<(), Box<dyn std::error::Error>> {
    let body = body_named(module, "main")?;
    for statement in &body.block.stmts {
        let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::BinaryOp(bir::BinOp::Eq, _, bir::Operand::Place(read)),
            ..
        } = &statement.kind
        else {
            continue;
        };
        let local = read.place.local_id().ok_or("comparison operand has no local")?;
        let IncanType::Generic { base, args } = &body.locals[local.index()].ty else {
            return Err("comparison list storage missing".into());
        };
        assert_eq!(base, "List");
        let [IncanType::Named(key)] = args.as_slice() else {
            return Err("comparison nominal element missing".into());
        };
        assert_ne!(key, "EvidenceId", "comparison lost its field owner");
        let identity = body
            .named_type_identities
            .get(key)
            .ok_or("comparison owner identity missing")?;
        assert_eq!(
            identity.origin,
            incan_semantics_core::SymbolOrigin::Module(vec!["ids".to_owned()])
        );
        return Ok(());
    }
    Err("checked field comparison missing".into())
}

/// Check the original two providers through their actual public manifests, then retain the consumer's execution
/// context.
fn checked_widget_consumer(source: &str) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    checked_public_consumer(
        &[
            (
                "widgets",
                include_str!(
                    "../../../../incan_test_support/fixtures/behavior/lowering_dependencies/provider_nominal_types_and_unions_issue892/deps/widgets/src/lib.incn"
                ),
            ),
            (
                "other",
                include_str!(
                    "../../../../incan_test_support/fixtures/behavior/lowering_dependencies/provider_nominal_types_and_unions_issue892/deps/other/src/lib.incn"
                ),
            ),
        ],
        source,
    )
}

/// Check unchanged provider declarations and import their actual public manifests before building consumer context.
fn checked_public_consumer(
    providers: &[(&str, &str)],
    source: &str,
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let mut entries = std::collections::HashMap::new();
    let mut context = Vec::new();
    for &(name, source) in providers {
        let program =
            parser::parse(&lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?)
                .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(Vec::new()));
        checker.set_current_package_identity(Some(name.to_owned()));
        checker
            .check_program(&program)
            .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        let manifest =
            LibraryManifest::from_checked_exports(name, "0.1.0", &collect_checked_public_exports(&program, &checker));
        entries.insert(
            name.to_owned(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    name,
                    name,
                    std::path::PathBuf::from("tests/widget-artifact"),
                ),
            },
        );
        context.push(build_body_ir_module_v0_with_executable_context(
            &program,
            &[],
            checker.type_info(),
            &[],
        ));
    }
    let program = parser::parse(&lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".to_owned()]));
    checker.set_library_manifest_index(LibraryManifestIndex::from_entries(entries));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0_with_executable_context(
        &program,
        &["main".to_owned()],
        checker.type_info(),
        &context,
    ))
}

/// A consumer without a trait import retains the actual SDK map carrier after its dependency's checked return.
#[test]
fn package_return_factory_without_trait_import_retains_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_public_consumer(
        &[(
            "fallible_streams",
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/cli_dependencies/fallible_iterator_chain_without_trait_import/deps/fallible_streams/src/lib.incn"
            ),
        )],
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/cli_dependencies/fallible_iterator_chain_without_trait_import/src/main.incn"
        ),
    )?;
    assert_package_map_carriers(&module, 1)
}

/// Both unchanged success/error chains retain their concrete SDK result after a public package factory call.
#[test]
fn package_return_factory_with_trait_import_retains_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_public_consumer(
        &[(
            "fallible_streams",
            include_str!(
                "../../../../incan_test_support/fixtures/behavior/lowering_dependencies/fallible_iterator_through_public_library/deps/fallible_streams/src/lib.incn"
            ),
        )],
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/lowering_dependencies/fallible_iterator_through_public_library/src/main.incn"
        ),
    )?;
    assert_package_map_carriers(&module, 2)
}

/// Every original map call keeps its exact SDK constructor and its checked provider-owned Self carrier.
fn assert_package_map_carriers(module: &bir::BodyIrModule, expected: usize) -> Result<(), Box<dyn std::error::Error>> {
    let body = body_named(module, "main")?;
    let mut count = 0;
    for statement in &body.block.stmts {
        let bir::StatementKind::Call {
            callee: bir::Callee::Method(target),
            destination: Some(destination),
            ..
        } = &statement.kind
        else {
            continue;
        };
        if target.name != "map" {
            continue;
        }
        count += 1;
        let local = destination.local_id().ok_or("map needs a whole destination")?;
        let IncanType::Generic { base, args } = &body.locals[local.index()].ty else {
            return Err(format!("map result lost its checked carrier: {}", body.render_snapshot()).into());
        };
        assert_eq!(base, "MapFallibleIterator", "{}", body.render_snapshot());
        assert!(
            args.iter().any(|argument| match argument {
                IncanType::Named(name) => body.named_type_identities.get(name).is_some_and(|identity| matches!(
                    &identity.origin,
                    SymbolOrigin::Package { library, .. } if library == "fallible_streams"
                )),
                _ => false,
            }),
            "map lost its exact provider-owned Self: {args:?}"
        );
    }
    assert_eq!(count, expected, "every unchanged original chain must be checked");
    Ok(())
}

/// A union pattern selecting an aliased same-named provider member keeps that member, not the other import's carrier.
#[test]
fn qualified_provider_union_patterns_keep_selected_member() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_widget_consumer(include_str!(
        "../../../../incan_test_support/fixtures/behavior/lowering_dependencies/provider_nominal_types_and_unions_issue892/src/main.incn"
    ))?;
    let body = body_named(&module, "pick")?;
    let pattern = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => arms.first().map(|arm| &arm.pattern),
            _ => None,
        })
        .ok_or("pick member pattern missing")?;
    let bir::Pattern::UnionMember {
        ty: IncanType::Named(key),
        fields,
    } = pattern
    else {
        return Err(format!("expected a nominal member pattern: {pattern:?}").into());
    };
    let Some(bir::Pattern::Var(binding)) = fields.first() else {
        return Err("selected union payload binding missing".into());
    };
    let payload = &body.locals[binding.local.index()];
    let IncanType::Named(payload_key) = &payload.ty else {
        return Err("selected union payload carrier missing".into());
    };
    let payload_identity = body
        .named_type_identities
        .get(payload_key)
        .ok_or("payload identity missing")?;
    assert!(
        matches!(&payload_identity.origin, incan_semantics_core::SymbolOrigin::Package { library, .. } if library == "other"),
        "checked payload: {payload_identity:?}"
    );
    let selected = body
        .named_type_identities
        .get(key)
        .ok_or("pattern carrier identity missing")?;
    assert!(
        matches!(&selected.origin, incan_semantics_core::SymbolOrigin::Package { library, .. } if library == "other"),
        "{selected:?}"
    );
    let IncanType::Generic { args, .. } = &body.params[0].ty else {
        return Err("choice union missing".into());
    };
    assert!(
        args.contains(&IncanType::Named(key.clone())),
        "pattern is outside its checked union: {key}; {args:?}"
    );
    Ok(())
}

/// Discarding the matched payload must not discard the selected provider's nominal identity.
#[test]
fn qualified_provider_union_wildcard_keeps_selected_member() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("../../../../incan_test_support/fixtures/behavior/lowering_dependencies/provider_nominal_types_and_unions_issue892/src/main.incn")
        .replace("OtherWidget(widget)", "OtherWidget(_)")
        .replace("f\"other widget {widget.value}\"", "\"other widget\"");
    let module = checked_widget_consumer(&source)?;
    let body = body_named(&module, "pick")?;
    let key = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => match arms.first().map(|arm| &arm.pattern) {
                Some(bir::Pattern::UnionMember {
                    ty: IncanType::Named(key),
                    ..
                }) => Some(key),
                _ => None,
            },
            _ => None,
        })
        .ok_or("wildcard union member missing")?;
    let selected = body
        .named_type_identities
        .get(key)
        .ok_or("wildcard member identity missing")?;
    assert!(
        matches!(&selected.origin, incan_semantics_core::SymbolOrigin::Package { library, .. } if library == "other"),
        "{selected:?}"
    );
    Ok(())
}

/// Build a direct project's module from ordinary checked facts, independently of another same-named owner.
fn project_module(path: &str) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    checked_module(path, None)
}

/// A published compilation supplies its package identity before the checker creates any nominal carrier facts.
fn checked_module(path: &str, package: Option<&str>) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let source = "pub model Size:\n    pub n: int\n\npub def make(size: Size = Size(n=1)) -> Size:\n    return size\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec![path.to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(package.map(str::to_owned));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0_with_executable_context(
        &program,
        &path,
        checker.type_info(),
        &[],
    ))
}

/// Publication preserves a nominal's own carrier key even when all its stored fields are primitives.
#[test]
fn published_nominal_keeps_its_qualified_carrier_key() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module("a", Some("carrier_provider"))?;
    let nominal = &module.nominal_declarations[0];
    let public = std::iter::once(nominal.canonical.clone())
        .chain(nominal.field_identities.iter().cloned())
        .collect::<BTreeSet<_>>();
    let bytes = build_surface(
        std::slice::from_ref(&module),
        "carrier_provider",
        "0.1.0",
        &public,
        &BTreeSet::new(),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    let ExecutableDeclaration::Nominal(projected) = reader.fragment(&nominal.canonical)? else {
        return Err("expected the published nominal fragment".into());
    };
    assert_ne!(nominal.carrier_name(), nominal.name);
    assert_eq!(
        projected.carrier_name(),
        nominal.carrier_name(),
        "publication dropped the owning carrier identity"
    );
    Ok(())
}

/// Two checked modules retain distinct use-site carriers even when their source declarations have equal names.
#[test]
fn same_named_project_nominals_keep_qualified_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let first = project_module("a")?;
    let second = project_module("b")?;
    let left = body_named(&first, "make")?;
    let right = body_named(&second, "make")?;
    assert_ne!(
        left.return_type, right.return_type,
        "module identity was erased at the return boundary"
    );
    assert_eq!(left.return_type, left.params[0].ty);
    assert_eq!(right.return_type, right.params[0].ty);
    for (module, body) in [(&first, left), (&second, right)] {
        let IncanType::Named(key) = &body.return_type else {
            return Err("expected a nominal carrier".into());
        };
        assert_eq!(
            body.named_type_identities.get(key),
            Some(&module.nominal_declarations[0].canonical)
        );
        assert!(module.is_well_formed_nominal_declaration(&module.nominal_declarations[0]));
    }
    Ok(())
}

/// Deferred defaults keep the declaring module's carrier rather than acquiring the eventual caller's same name.
#[test]
fn project_nominal_defaults_keep_qualified_local_types() -> Result<(), Box<dyn std::error::Error>> {
    for path in ["a", "b"] {
        let module = project_module(path)?;
        let body = body_named(&module, "make")?;
        let bir::CallableParamDefault::Source(default) = &body.params[0].default else {
            return Err("missing declaration-owned default".into());
        };
        let IncanType::Named(key) = &body.return_type else {
            return Err("expected nominal result".into());
        };
        assert_ne!(key, "Size", "a default needs its owner's qualified carrier");
        let bir::Operand::Place(result) = &default.result else {
            return Err("expected constructed default".into());
        };
        let local = result.place.local_id().ok_or("default has no carrier local")?;
        assert_eq!(body.locals[local.index()].ty, body.return_type);
    }
    Ok(())
}

/// A fully defaulted class constructed through a public module keeps its checked owner and omitted field binding.
#[test]
fn public_module_defaulted_class_retains_constructor_binding() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_public_consumer(
        &[("builders", "pub class IndexBuilder:\n    pub size: int = 3\n")],
        "import pub::builders as namespace\n\ndef main() -> None:\n    builder = namespace.IndexBuilder()\n    println(builder.size)\n",
    )?;
    let main = body_named(&module, "main")?;
    assert!(
        main.locals.iter().all(|local| local.ty != IncanType::Unknown),
        "{}",
        main.render_snapshot()
    );
    let constructor = main
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("missing canonical defaulted class constructor")?;
    assert!(
        matches!(&constructor.binding, bir::ArgumentBinding::Resolved { arguments, defaulted_slots } if arguments.is_empty() && defaulted_slots == &[0])
    );
    assert_eq!(
        constructor.canonical.as_ref().map(|identity| &identity.kind),
        Some(&SemanticSourceTargetKind::Class)
    );
    let owner = constructor.canonical.clone().ok_or("missing constructor owner")?;
    assert!(matches!(&owner.origin, SymbolOrigin::Package { library, .. } if library == "builders"));
    let span = crate::ast::Span::new(1, 2);
    let mut facts = TypeCheckInfo::default();
    facts.record_resolved_identity(span, owner.clone());
    let program = parser::parse(
        &lexer::lex("class Local:\n    size: int = 3\n\ndef main() -> None:\n    value = Local()\n")
            .map_err(|errors| format!("{errors:?}"))?,
    )
    .map_err(|errors| format!("{errors:?}"))?;
    let mut local_checker = TypeChecker::new();
    local_checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let binding = local_checker
        .type_info()
        .calls
        .constructor_field_bindings
        .values()
        .next()
        .ok_or("missing local checked default binding")?
        .clone();
    facts
        .calls
        .body_ir_module_constructor_bindings
        .insert((span.start, span.end), (owner, binding));
    assert!(
        facts.constructor_field_binding(span).is_none(),
        "legacy constructor facts changed"
    );
    assert!(facts.body_ir_constructor_field_binding(span).is_some());
    facts
        .calls
        .body_ir_module_constructor_bindings
        .get_mut(&(span.start, span.end))
        .ok_or("missing private binding")?
        .0
        .declaration_name
        .push_str("_foreign");
    assert!(
        facts.body_ir_constructor_field_binding(span).is_none(),
        "foreign owner granted a default binding"
    );
    Ok(())
}

/// An unsupplied required field remains a source error through the public module constructor path.
#[test]
fn public_module_required_class_field_stays_required() {
    assert!(
        checked_public_consumer(
            &[("builders", "pub class IndexBuilder:\n    pub size: int\n")],
            "import pub::builders as namespace\n\ndef main() -> None:\n    builder = namespace.IndexBuilder()\n",
        )
        .is_err()
    );
}
