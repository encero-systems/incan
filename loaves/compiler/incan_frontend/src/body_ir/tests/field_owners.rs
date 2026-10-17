//! Original nested-trait field reads keep their declaration-owned storage under project carrier qualification.

use super::{TypeCheckInfo, TypeChecker, ast, bir, body_named, lexer, parser};
use incan_semantics_core::executable_representation::{decode_module, encode_module};

const MAIN: &str = include_str!(
    "../../../../incan_test_support/fixtures/behavior/lowering_declarations/trait_default_nested_module_identity/main.incn"
);
const SHAPES: &str = include_str!(
    "../../../../incan_test_support/fixtures/behavior/lowering_declarations/trait_default_nested_module_identity/geometry/shapes.incn"
);

/// Parse the original source without removing its collision, trait defaults, private constant, or other nominals.
fn parse_original(source: &str) -> Result<ast::Program, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    Ok(parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?)
}

/// Check each original module through the ordinary dependency-AST path, retaining the real nested module segments.
fn original_facts() -> Result<(ast::Program, TypeCheckInfo, ast::Program, TypeCheckInfo), Box<dyn std::error::Error>> {
    let shapes = parse_original(SHAPES)?;
    let main = parse_original(MAIN)?;
    let mut provider = TypeChecker::new();
    provider.set_current_module_path(Some(vec!["geometry".to_owned(), "shapes".to_owned()]));
    provider
        .check_program(&shapes)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut consumer = TypeChecker::new();
    consumer.set_current_module_path(Some(vec!["main".to_owned()]));
    consumer
        .register_dependency_module_path_segments("geometry_shapes", vec!["geometry".to_owned(), "shapes".to_owned()]);
    consumer
        .check_with_imports(&main, &[("geometry_shapes", &shapes)])
        .map_err(|errors| format!("{errors:?}"))?;
    Ok((shapes, provider.type_info().clone(), main, consumer.type_info().clone()))
}

/// Both production profiles receive the same checked provider context; only project carrier qualification differs.
fn original_consumer(
    program: &ast::Program,
    facts: &TypeCheckInfo,
    context: &[bir::BodyIrModule],
    project: bool,
) -> bir::BodyIrModule {
    let path = ["main".to_owned()];
    if project {
        super::build_body_ir_module_v0_with_executable_context(program, &path, facts, context)
    } else {
        super::super::build_body_ir_module_v0_with_provider_operations(
            program,
            &path,
            facts,
            &super::super::ProviderOperationCatalog::new(),
            context,
            false,
            None,
        )
    }
}

/// Collect actual field operands from this fixture's straight-line calls, assignments, and returns.
fn field_reads(body: &bir::Body) -> Vec<&bir::Place> {
    let mut operands = Vec::new();
    for statement in &body.block.stmts {
        match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Use(operand),
                ..
            }
            | bir::StatementKind::Return { value: Some(operand) } => operands.push(operand),
            bir::StatementKind::Call { args, .. } => {
                for argument in args {
                    if let bir::ArgumentElement::One(operand) = argument {
                        operands.push(operand);
                    }
                }
            }
            _ => {}
        }
    }
    operands
        .into_iter()
        .filter_map(|operand| match operand {
            bir::Operand::Place(read) if !read.place.projection.is_empty() => Some(&read.place),
            _ => None,
        })
        .collect()
}

/// Trace the original checked access before fallback and compare its actual projection against its owning layout.
fn check_field(
    module: &bir::BodyIrModule,
    declaration: &bir::NominalDeclaration,
    facts: &TypeCheckInfo,
    source: &str,
    access: &str,
    body_name: &str,
    project: bool,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let body = body_named(module, body_name)?;
    let (base_source, field) = access.rsplit_once('.').ok_or("field access missing")?;
    let start = source.find(access).ok_or("original field access missing")?;
    let span = ast::Span::new(start, start + access.len());
    let base_span = ast::Span::new(start, start + base_source.len());
    let slot = declaration
        .fields
        .iter()
        .position(|name| name == field)
        .ok_or("declared field missing")?;
    let expected = declaration
        .field_identities
        .get(slot)
        .ok_or("declared member identity missing")?;
    let reads = field_reads(body);
    let matching = reads
        .iter()
        .filter(|place| matches!(place.projection.last(), Some(bir::PlaceElem::Field { name, .. }) if name == field))
        .collect::<Vec<_>>();
    let actual = matching.first().ok_or("actual original field operand missing")?;
    assert_eq!(matching.len(), 1, "{access}: {reads:?}");
    let local = actual.local_id().and_then(|id| body.locals.get(id.index()));
    let retained_owner = local.and_then(|local| match &local.ty {
        super::IncanType::Named(owner) => super::super::parameter_types::retained_nominal_identity(owner, facts),
        _ => None,
    });
    let raw_owner = local.and_then(|local| match &local.ty {
        super::IncanType::Named(owner) => facts.declarations.named_type_identities.get(owner),
        _ => None,
    });
    let trace = format!(
        "project={project}, access={access}, body={:?}, checked_field={:?}, checked_base={:?}, checked_call={:?}, body_ir_result={:?}, inferred_result={:?}, self_method_result={:?}, expression_owners={:?}, newtype_field_owner={:?}, newtype_result={:?}, retained_local={local:?}, raw_owner={raw_owner:?}, retained_owner={retained_owner:?}, actual={actual:?}, expected_owner={:?}, expected_field={expected:?}, layout={:?}",
        body.canonical,
        facts.resolved_identity(span),
        facts.expr_type(base_span),
        facts.resolved_identity(base_span),
        facts
            .calls
            .body_ir_call_result_types
            .get(&(base_span.start, base_span.end)),
        facts
            .calls
            .inferred_call_result_types
            .get(&(base_span.start, base_span.end)),
        facts
            .traits
            .self_method_result_types
            .get(&(base_span.start, base_span.end)),
        facts
            .expressions
            .expression_type_identities
            .get(&(base_span.start, base_span.end)),
        facts.body_ir_newtypes.field_owner(span, facts),
        facts.body_ir_newtypes.expression_type(base_span, facts),
        declaration.canonical,
        declaration.fields
    );
    eprintln!("{trace}");
    let correct = matches!(actual.projection.last(), Some(bir::PlaceElem::Field { canonical: Some(identity), structural: false, .. }) if identity == expected);
    Ok((!correct).then_some(trace))
}

/// All four original field reads retain distinct module/owner/member identities in local and project production
/// profiles, and the production executable codec preserves those facts exactly.
#[test]
fn nested_trait_original_fields_keep_declaring_owners() -> Result<(), Box<dyn std::error::Error>> {
    let (shapes, provider_facts, main, consumer_facts) = original_facts()?;
    let shapes_path = vec!["geometry".to_owned(), "shapes".to_owned()];
    let mut failures = Vec::new();
    for project in [false, true] {
        let provider = if project {
            super::build_body_ir_module_v0_with_executable_context(&shapes, &shapes_path, &provider_facts, &[])
        } else {
            super::build_body_ir_module_v0(&shapes, &shapes_path, &provider_facts)
        };
        let consumer = original_consumer(&main, &consumer_facts, std::slice::from_ref(&provider), project);
        let local_extent = consumer
            .nominal_declarations
            .iter()
            .find(|item| item.name == "Extent" && item.fields == ["label"])
            .ok_or("local Extent missing")?;
        let imported_extent = provider
            .nominal_declarations
            .iter()
            .find(|item| item.name == "Extent")
            .ok_or("geometry Extent missing")?;
        let frame = provider
            .nominal_declarations
            .iter()
            .find(|item| item.name == "Frame")
            .ok_or("geometry Frame missing")?;
        let meters = provider
            .nominal_declarations
            .iter()
            .find(|item| item.name == "Meters")
            .ok_or("geometry Meters missing")?;
        assert_eq!(meters.canonical.kind, super::SemanticSourceTargetKind::Newtype);
        assert_eq!(meters.canonical.origin, imported_extent.canonical.origin);
        assert_eq!(meters.fields, ["0"]);
        assert_eq!(meters.field_identities, std::slice::from_ref(&meters.canonical));
        assert_eq!(
            meters.field_types,
            [super::IncanType::Primitive(
                incan_semantics_core::IncanPrimitiveType::Int
            )]
        );
        assert!(provider.is_well_formed_nominal_declaration(meters));
        assert_ne!(local_extent.canonical, imported_extent.canonical);
        assert_ne!(local_extent.canonical.origin, imported_extent.canonical.origin);
        assert_eq!(frame.canonical.origin, imported_extent.canonical.origin);
        assert_eq!(
            body_named(&provider, "extent_size")?
                .canonical
                .as_ref()
                .ok_or("trait method identity missing")?
                .origin,
            imported_extent.canonical.origin
        );
        assert_eq!(
            body_named(&consumer, "main")?
                .canonical
                .as_ref()
                .ok_or("main identity missing")?
                .origin,
            local_extent.canonical.origin
        );
        assert_eq!(imported_extent.fields, ["n"]);
        assert_eq!(frame.fields, ["n"]);
        assert_eq!(
            provider_facts.declarations.named_type_identities.get("Extent"),
            Some(&imported_extent.canonical)
        );
        assert_eq!(
            consumer_facts.declarations.named_type_identities.get("Extent"),
            Some(&local_extent.canonical)
        );
        for (module, declaration, facts, source, access, body) in [
            (
                &consumer,
                local_extent,
                &consumer_facts,
                MAIN,
                "Extent(label=\"local\").label",
                "main",
            ),
            (
                &provider,
                imported_extent,
                &provider_facts,
                SHAPES,
                "self.extent().n",
                "extent_size",
            ),
            (&consumer, frame, &consumer_facts, MAIN, "card.frame().n", "main"),
            (&consumer, meters, &consumer_facts, MAIN, "card.distance().0", "main"),
        ] {
            if let Some(failure) = check_field(module, declaration, facts, source, access, body, project)? {
                failures.push(failure);
            }
        }
        assert_eq!(decode_module(&encode_module(&provider)?)?, provider);
        assert_eq!(decode_module(&encode_module(&consumer)?)?, consumer);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

/// Missing, foreign, stale, or conflicting provider return proofs cannot authorize the original hidden newtype slot.
#[test]
fn nested_trait_newtype_return_rejects_damaged_authority() -> Result<(), Box<dyn std::error::Error>> {
    let (shapes, provider_facts, main, consumer_facts) = original_facts()?;
    let path = vec!["geometry".to_owned(), "shapes".to_owned()];
    for project in [false, true] {
        let provider = if project {
            super::build_body_ir_module_v0_with_executable_context(&shapes, &path, &provider_facts, &[])
        } else {
            super::build_body_ir_module_v0(&shapes, &path, &provider_facts)
        };
        let owner = provider
            .nominal_declarations
            .iter()
            .find(|item| item.name == "Meters")
            .ok_or("Meters missing")?
            .canonical
            .clone();
        for mutation in 0..9 {
            let mut damaged = provider.clone();
            let mut facts = consumer_facts.clone();
            let mut context = Vec::new();
            match mutation {
                0 => damaged.nominal_declarations.retain(|item| item.canonical != owner),
                1 => {
                    let declaration = damaged
                        .nominal_declarations
                        .iter_mut()
                        .find(|item| item.canonical == owner)
                        .ok_or("layout missing")?;
                    declaration.field_identities[0].declaration_span.start += 1;
                }
                2 => {
                    let body = damaged
                        .bodies
                        .iter_mut()
                        .find(|body| body.name == "distance")
                        .ok_or("distance missing")?;
                    body.canonical
                        .as_mut()
                        .ok_or("method identity missing")?
                        .declaration_span
                        .start += 1;
                }
                3 => {
                    let body = damaged
                        .bodies
                        .iter_mut()
                        .find(|body| body.name == "distance")
                        .ok_or("distance missing")?;
                    let super::IncanType::Named(name) = &body.return_type else {
                        return Err("return carrier missing".into());
                    };
                    body.named_type_identities
                        .get_mut(name)
                        .ok_or("return owner missing")?
                        .origin = incan_semantics_core::SymbolOrigin::Module(vec!["foreign".to_owned()]);
                }
                4 => {
                    let mut foreign = owner.clone();
                    foreign.declaration_name = "OtherMeters".to_owned();
                    facts
                        .declarations
                        .named_type_identities
                        .insert(bir::nominal_carrier_key(&owner).ok_or("carrier missing")?, foreign);
                }
                5 => {
                    context.push(provider.clone());
                    let declaration = damaged
                        .nominal_declarations
                        .iter_mut()
                        .find(|item| item.canonical == owner)
                        .ok_or("layout missing")?;
                    declaration.field_types[0] =
                        super::IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Str);
                }
                6 => {
                    let mut foreign = owner.clone();
                    foreign.declaration_name = "OtherMeters".to_owned();
                    facts
                        .declarations
                        .named_type_identities
                        .insert("Meters".to_owned(), foreign);
                }
                7 | 8 => {
                    let declaration = damaged
                        .nominal_declarations
                        .iter_mut()
                        .find(|item| item.canonical == owner)
                        .ok_or("layout missing")?;
                    if mutation == 7 {
                        declaration.public = false;
                    } else {
                        declaration.field_public[0] = false;
                    }
                }
                _ => return Err("unknown mutation".into()),
            }
            context.push(damaged);
            let consumer = original_consumer(&main, &facts, &context, project);
            let body = body_named(&consumer, "main")?;
            let read = field_reads(body)
                .into_iter()
                .find(
                    |place| matches!(place.projection.last(), Some(bir::PlaceElem::Field { name, .. }) if name == "0"),
                )
                .ok_or("payload read missing")?;
            assert!(
                matches!(
                    read.projection.last(),
                    Some(bir::PlaceElem::Field {
                        canonical: None,
                        structural: false,
                        ..
                    })
                ),
                "project={project}, mutation={mutation}, read={read:?}"
            );
        }
    }
    Ok(())
}

/// A duplicate method that loses its return binding or changes its result cannot borrow another copy's authority.
#[test]
fn nested_trait_newtype_return_rejects_conflicting_method_copies() -> Result<(), Box<dyn std::error::Error>> {
    let (shapes, provider_facts, main, consumer_facts) = original_facts()?;
    let path = vec!["geometry".to_owned(), "shapes".to_owned()];
    for project in [false, true] {
        let provider = if project {
            super::build_body_ir_module_v0_with_executable_context(&shapes, &path, &provider_facts, &[])
        } else {
            super::build_body_ir_module_v0(&shapes, &path, &provider_facts)
        };
        for missing_binding in [true, false] {
            let mut damaged = provider.clone();
            let body = damaged
                .bodies
                .iter_mut()
                .find(|body| body.name == "distance")
                .ok_or("distance missing")?;
            let super::IncanType::Named(name) = &body.return_type else {
                return Err("return carrier missing".into());
            };
            if missing_binding {
                body.named_type_identities
                    .remove(name)
                    .ok_or("return binding missing")?;
            } else {
                body.return_type = super::IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int);
            }
            for context in [[provider.clone(), damaged.clone()], [damaged.clone(), provider.clone()]] {
                let consumer = original_consumer(&main, &consumer_facts, &context, project);
                let body = body_named(&consumer, "main")?;
                let read = field_reads(body).into_iter().find(|place| matches!(place.projection.last(), Some(bir::PlaceElem::Field { name, .. }) if name == "0")).ok_or("payload read missing")?;
                assert!(
                    matches!(
                        read.projection.last(),
                        Some(bir::PlaceElem::Field {
                            canonical: None,
                            structural: false,
                            ..
                        })
                    ),
                    "project={project}, missing_binding={missing_binding}, read={read:?}"
                );
            }
        }
    }
    Ok(())
}
