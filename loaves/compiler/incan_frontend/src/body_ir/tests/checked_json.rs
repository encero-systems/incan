//! Checked serde reconstruction retains one source-selected hook rather than bypassing validation.

use super::{
    BodyBuilder, BodyIrLoweringFacts, FunctionDefaultSources, LocalFieldlessEnumDeclarations,
    LocalFunctionDeclarations, LocalNominalDeclarations, LocalValueEnumDeclarations, ProviderOperationCatalog, build,
    build_body_ir_module_v0, build_body_ir_module_v0_with_executable_context,
};
use crate::{
    ast, lexer, parser,
    typechecker::{TypeCheckInfo, TypeChecker},
};
use incan_semantics_core::{SemanticSourceTargetKind, body_ir as bir};
use std::collections::{BTreeSet, HashMap, HashSet};

const ORIGINAL: &str = include_str!(
    "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_json_validation.incn"
);

/// A source mutable input cannot acquire an owned-string reconstruction policy from normalized binding facts.
#[test]
fn mutable_deserialize_hook_is_not_admitted() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.serde.json import Deserialize\n\n@derive(Clone, Deserialize)\ntype CheckedText = newtype str:\n    def from_underlying(mut value: str) -> Result[Self, ValidationError]:\n        return Ok(CheckedText(\"fixed\"))\n\ndef main() -> None:\n    pass\n";
    let (program, facts) = checked(source)?;
    let declaration = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            ast::Declaration::Newtype(value) => Some(value),
            _ => None,
        })
        .ok_or("source newtype absent")?;
    assert!(declaration.methods[0].node.params[0].node.is_mut);
    let module = build_body_ir_module_v0(&program, &["checked_json".to_owned()], &facts);
    assert!(
        !super::is_direct_replacement_checked_newtype(declaration, &facts),
        "normalized facts: {:?}; nominal records: {}",
        facts.declarations.method_bindings_by_span,
        serde_json::to_string(&module.nominal_declarations)?,
    );
    assert!(
        !module
            .nominal_declarations
            .iter()
            .any(|value| value.name == "CheckedText")
    );
    Ok(())
}

/// Both original owners retain their exact checked hooks; generic reconstruction keeps declaration-order binders.
#[test]
fn checked_deserialize_retains_original_selected_hooks() -> Result<(), Box<dyn std::error::Error>> {
    let (program, facts) = checked(ORIGINAL)?;
    let module = build_body_ir_module_v0(&program, &["checked_json".to_owned()], &facts);
    let project = build_body_ir_module_v0_with_executable_context(&program, &["checked_json".to_owned()], &facts, &[]);
    for module in [module, project] {
        for (name, binders) in [("ShortId", Vec::<String>::new()), ("CheckedBox", vec!["D".to_owned()])] {
            let declaration = module
                .nominal_declarations
                .iter()
                .find(|value| value.name == name)
                .ok_or_else(|| format!("missing checked Deserialize owner {name}"))?;
            let hook = facts
                .declarations
                .newtype_construction
                .get(name)
                .and_then(|value| value.checked_constructor_identity.as_ref())
                .ok_or("missing checked hook")?;
            assert_eq!(hook.kind, SemanticSourceTargetKind::Method);
            assert_eq!(declaration.type_parameters, binders);
            assert!(declaration.derives.iter().any(|value| value == "serde::Deserialize"));
            assert_eq!(
                serde_json::to_value(declaration)?["newtype_deserialize"],
                serde_json::json!({"Checked": serde_json::to_value(hook)?})
            );
            let matching: Vec<_> = module
                .bodies
                .iter()
                .filter(|body| body.canonical.as_ref() == Some(hook))
                .collect();
            let [body] = matching.as_slice() else {
                return Err("hook Body is not unique".into());
            };
            assert!(
                module.is_well_formed_nominal_declaration(declaration),
                "{name}: carrier={}, hook result={:?}",
                declaration.carrier_name(),
                body.return_type,
            );
            assert_eq!(body.type_parameters, binders);
            assert_eq!(body.params.len(), 1);
            assert!(!body.params[0].mutable);
            assert!(
                !body.render_snapshot().contains("unsupported("),
                "{}",
                body.render_snapshot()
            );
        }
    }
    Ok(())
}

/// Ordinary raw Deserialize layouts carry no checked hook marker and retain their existing derive contract.
#[test]
fn raw_deserialize_has_no_checked_hook_marker() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.serde.json import Deserialize\n\n@derive(Clone, Deserialize)\ntype Raw = newtype str\n\ndef main() -> None:\n    pass\n",
        &["raw_json"],
    )?;
    let [declaration] = module.nominal_declarations.as_slice() else {
        return Err("raw layout absent".into());
    };
    assert_eq!(serde_json::to_value(declaration)?["newtype_deserialize"], "Ordinary");
    Ok(())
}

/// Corrupted selected bindings cannot authorize a serde hook by name or by one convenient matching candidate.
#[test]
fn checked_deserialize_rejects_damaged_hook_bindings() -> Result<(), Box<dyn std::error::Error>> {
    let (program, original) = checked(ORIGINAL)?;
    let hook = original
        .declarations
        .newtype_construction
        .get("ShortId")
        .and_then(|value| value.checked_constructor_identity.as_ref())
        .ok_or("missing selected hook")?
        .clone();
    let coordinates = (hook.declaration_span.start, hook.declaration_span.end);
    for damage in ["mutable", "default", "async", "duplicate", "missing", "renamed"] {
        let mut facts = original.clone();
        if damage == "missing" {
            facts.declarations.method_bindings_by_span.remove(&coordinates);
        } else if damage == "duplicate" {
            let binding = facts
                .declarations
                .method_bindings_by_span
                .get(&coordinates)
                .ok_or("hook binding absent")?
                .clone();
            facts.declarations.method_bindings_by_span.insert((0, 1), binding);
        } else {
            let binding = facts
                .declarations
                .method_bindings_by_span
                .get_mut(&coordinates)
                .ok_or("hook binding absent")?;
            match damage {
                "mutable" => binding.params[0].is_mut = true,
                "default" => binding.params[0].has_default = true,
                "async" => binding.is_async = true,
                "renamed" => {
                    binding
                        .identity
                        .as_mut()
                        .ok_or("hook identity absent")?
                        .declaration_name = "different".to_owned()
                }
                _ => return Err("unknown corruption control".into()),
            }
        }
        let module = build_body_ir_module_v0(&program, &["checked_json".to_owned()], &facts);
        assert!(
            !module.nominal_declarations.iter().any(|value| value.name == "ShortId"),
            "admitted {damage} hook"
        );
    }
    Ok(())
}

/// Retained checked markers remain tied to one exact Body and never authorize corrupt frame or result facts.
#[test]
fn checked_deserialize_rejects_corrupt_retained_hook_frames() -> Result<(), Box<dyn std::error::Error>> {
    let (program, facts) = checked(ORIGINAL)?;
    let original = build_body_ir_module_v0(&program, &["checked_json".to_owned()], &facts);
    let owner = original
        .nominal_declarations
        .iter()
        .position(|value| value.name == "ShortId")
        .ok_or("missing checked owner")?;
    let hook = facts
        .declarations
        .newtype_construction
        .get("ShortId")
        .and_then(|value| value.checked_constructor_identity.as_ref())
        .ok_or("missing checked hook")?;
    let body_index = original
        .bodies
        .iter()
        .position(|body| body.canonical.as_ref() == Some(hook))
        .ok_or("missing checked Body")?;
    for damage in [
        "missing",
        "duplicate",
        "name",
        "mutable",
        "parameter",
        "result",
        "binders",
        "call-id",
    ] {
        let mut module = original.clone();
        if damage == "missing" {
            module.bodies.remove(body_index);
        } else if damage == "duplicate" {
            module.bodies.push(module.bodies[body_index].clone());
        } else {
            let body = &mut module.bodies[body_index];
            match damage {
                "name" => body.name = "different".to_owned(),
                "mutable" => body.params[0].mutable = true,
                "parameter" => body.params[0].ty = incan_semantics_core::IncanType::Unknown,
                "result" => body.return_type = incan_semantics_core::IncanType::Unknown,
                "binders" => body.type_parameters.push("unselected".to_owned()),
                "call-id" => body.direct_call_id = module.nominal_declarations[owner].direct_declaration_id.clone(),
                _ => return Err("unknown retained-frame corruption".into()),
            }
        }
        assert!(
            !module.is_well_formed_nominal_declaration(&module.nominal_declarations[owner]),
            "admitted {damage} frame"
        );
    }
    Ok(())
}

/// Missing serialized reconstruction metadata cannot authorize an older raw-looking Deserialize layout.
#[test]
fn deserialize_without_reconstruction_selection_is_not_well_formed() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.serde.json import Deserialize\n\n@derive(Clone, Deserialize)\ntype Raw = newtype str\n\ndef main() -> None:\n    pass\n",
        &["raw_json"],
    )?;
    let mut encoded = serde_json::to_value(&module)?;
    encoded["nominal_declarations"][0]
        .as_object_mut()
        .ok_or("nominal record absent")?
        .remove("newtype_deserialize");
    let decoded: bir::BodyIrModule = serde_json::from_value(encoded)?;
    let [declaration] = decoded.nominal_declarations.as_slice() else {
        return Err("raw layout absent".into());
    };
    assert!(!decoded.is_well_formed_nominal_declaration(declaration));
    Ok(())
}

/// Parse and check once before controlled damage; source errors are not substitutes for malformed-fact coverage.
fn checked(source: &str) -> Result<(ast::Program, TypeCheckInfo), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["checked_json".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok((program, checker.type_info().clone()))
}
/// An already checked string owner retains its canonical payload slot without reopening raw generic inference.
#[test]
fn checked_string_payload_retains_owner_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = checked_payload_source();
    let (program, facts) = checked(source)?;
    let module = build_body_ir_module_v0(&program, &["checked_json".to_owned()], &facts);
    let owner = facts
        .declarations
        .named_type_identities
        .get("CheckedBox")
        .ok_or("owner absent")?;
    assert_payload_identity(&module, "read_string", Some(owner))?;
    assert_payload_identity(&module, "read_integer", None)?;
    assert_payload_identity(&module, "read_open", None)?;
    Ok(())
}

/// A damaged selection cannot be repaired using the shared field-reference table.
#[test]
fn checked_string_payload_requires_selected_hook() -> Result<(), Box<dyn std::error::Error>> {
    let source = checked_payload_source();
    let (program, original) = checked(source)?;
    let owner = original
        .declarations
        .named_type_identities
        .get("CheckedBox")
        .ok_or("owner absent")?;
    let start = source.find("value.0").ok_or("projection absent")?;
    for damage in ["missing", "renamed", "binder"] {
        let mut facts = original.clone();
        facts
            .references
            .resolved_identities
            .insert((start, start + "value.0".len()), owner.clone());
        let construction = facts
            .declarations
            .newtype_construction
            .get_mut("CheckedBox")
            .ok_or("construction absent")?;
        match damage {
            "missing" => construction.checked_constructor_identity = None,
            "renamed" => {
                construction
                    .checked_constructor_identity
                    .as_mut()
                    .ok_or("hook absent")?
                    .declaration_name = "other".to_owned()
            }
            "binder" => construction.type_params = vec!["Other".to_owned()],
            _ => return Err("unknown damage".into()),
        }
        let module = build_body_ir_module_v0(&program, &["checked_json".to_owned()], &facts);
        assert_payload_identity(&module, "read_string", None)?;
    }
    Ok(())
}

/// The positive closed owner and excluded open/non-string instances share the same selected source declaration.
fn checked_payload_source() -> &'static str {
    r#"from std.serde.json import Deserialize

@derive(Clone, Deserialize)
type CheckedBox[D] = newtype D:
    def from_underlying(value: D) -> Result[Self, ValidationError]:
        return Ok(CheckedBox[D](value))

def read_string(value: CheckedBox[str]) -> None:
    payload = value.0
    println(payload)

def read_integer(value: CheckedBox[int]) -> None:
    payload = value.0

def read_open[D](value: CheckedBox[D]) -> None:
    payload = value.0

def main() -> None:
    pass
"#
}

/// Inspect the direct payload assignment rather than relying on a physical carrier name in a rendered snapshot.
fn assert_payload_identity(
    module: &bir::BodyIrModule,
    name: &str,
    expected: Option<&incan_semantics_core::CanonicalSymbolId>,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = super::body_named(module, name)?;
    let mut count = 0;
    for statement in &body.block.stmts {
        if let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            ..
        } = &statement.kind
        {
            for projection in &read.place.projection {
                if let bir::PlaceElem::Field {
                    name,
                    canonical,
                    structural,
                } = projection
                    && name == "0"
                {
                    assert_eq!(canonical.as_ref(), expected, "{}", body.render_snapshot());
                    assert!(!structural);
                    count += 1;
                }
            }
        }
    }
    assert_eq!(count, 1, "{}", body.render_snapshot());
    Ok(())
}
/// Damaged retained authority cannot authorize a checked payload, while its unchanged string instance can.
#[test]
fn checked_string_payload_requires_retained_layout_authority() -> Result<(), Box<dyn std::error::Error>> {
    let source = checked_payload_source();
    let (program, facts) = checked(source)?;
    let module = build_body_ir_module_v0(&program, &["checked_json".to_owned()], &facts);
    let declaration = module
        .nominal_declarations
        .iter()
        .find(|declaration| declaration.name == "CheckedBox")
        .ok_or("checked owner absent")?;
    assert!(module.is_well_formed_nominal_declaration(declaration));
    let start = source.find("value.0").ok_or("checked payload span absent")?;
    let base = ast::Spanned::new(ast::Expr::Ident("value".to_owned()), ast::Span::new(start, start + 5));
    let projection_span = ast::Span::new(start, start + "value.0".len());
    let module_identity = module.module_id.path();
    assert!(facts.body_ir_newtypes.field_owner(projection_span, &facts).is_none());

    for damage in [
        "unselected",
        "ordinary",
        "foreign-hook",
        "field-owner",
        "field-template",
        "binder-count",
        "binder-name",
        "source-id",
        "foreign-module",
    ] {
        let mut encoded = serde_json::to_value(declaration)?;
        let mut builder_module = module_identity;
        match damage {
            "unselected" => encoded["newtype_deserialize"] = serde_json::json!("Unselected"),
            "ordinary" => encoded["newtype_deserialize"] = serde_json::json!("Ordinary"),
            "foreign-hook" => {
                encoded["newtype_deserialize"]["Checked"]["declaration_name"] = serde_json::json!("other_hook");
            }
            "field-owner" => {
                let mut other = declaration.canonical.clone();
                other.declaration_name = "DifferentOwner".to_owned();
                encoded["field_identities"] = serde_json::to_value(vec![other])?;
            }
            "field-template" => {
                encoded["field_types"] = serde_json::to_value(vec![incan_semantics_core::IncanType::Unknown])?;
            }
            "binder-count" => encoded["type_parameter_count"] = serde_json::json!(2),
            "binder-name" => encoded["type_parameters"] = serde_json::json!(["Other"]),
            "source-id" => encoded["direct_declaration_id"] = serde_json::to_value(&module.module_id)?,
            "foreign-module" => builder_module = "different_source_module",
            _ => return Err("unknown retained-layout mutation".into()),
        }
        let retained: bir::NominalDeclaration = serde_json::from_value(encoded)?;
        let projection = projection_with_retained_layout(&facts, retained, &base, projection_span, builder_module);
        assert_eq!(projection, bir::PlaceElem::field("0", None), "admitted {damage}");
    }

    // Green restores only the retained declaration; final red's earlier admission failure does not reach this point.
    let projection =
        projection_with_retained_layout(&facts, declaration.clone(), &base, projection_span, module_identity);
    assert_eq!(
        projection,
        bir::PlaceElem::field("0", Some(declaration.canonical.clone()))
    );
    Ok(())
}

/// Invoke the existing field producer with one retained layout, avoiding recollection or predicate reimplementation.
fn projection_with_retained_layout(
    facts: &TypeCheckInfo,
    declaration: bir::NominalDeclaration,
    base: &ast::Spanned<ast::Expr>,
    span: ast::Span,
    module_identity: &str,
) -> bir::PlaceElem {
    let function_default_sources = FunctionDefaultSources::new();
    let local_function_declarations = LocalFunctionDeclarations::new();
    let local_nongeneric_functions = HashSet::new();
    let mut local_nominal_declarations = LocalNominalDeclarations::new();
    local_nominal_declarations.insert(declaration.name.clone(), declaration);
    let local_fieldless_enum_declarations = LocalFieldlessEnumDeclarations::new();
    let local_value_enum_declarations = LocalValueEnumDeclarations::new();
    let provider_operations = ProviderOperationCatalog::new();
    let published_constants = HashMap::new();
    let published_callable_returns = HashMap::new();
    let published_newtype_returns = HashMap::new();
    let published_storage_types = HashMap::new();
    let local_enum_declarations = HashMap::new();
    let runtime_type_conflicts = BTreeSet::new();
    let lowering_facts = BodyIrLoweringFacts {
        model_constants: &HashMap::new(),
        local_enum_declarations: &local_enum_declarations,
        published_methods: &[],
        published_callable_returns: &published_callable_returns,
        published_newtype_returns: &published_newtype_returns,
        published_storage_types: &published_storage_types,
        published_constants: &published_constants,
        type_info: facts,
        runtime_type_conflicts: &runtime_type_conflicts,
        retain_nominal_carriers: false,
        function_default_sources: &function_default_sources,
        local_function_declarations: &local_function_declarations,
        local_nongeneric_functions: &local_nongeneric_functions,
        local_receiver_methods: &[],
        local_trait_partials: &[],
        local_nominal_declarations: &local_nominal_declarations,
        local_fieldless_enum_declarations: &local_fieldless_enum_declarations,
        local_value_enum_declarations: &local_value_enum_declarations,
        module_identity,
        registry_package: None,
        provider_operations: &provider_operations,
        rust_module: None,
    };
    let builder = BodyBuilder::new(&lowering_facts, incan_semantics_core::IncanType::Unknown);
    builder.lower_checked_field_projection(base, "0", span)
}
