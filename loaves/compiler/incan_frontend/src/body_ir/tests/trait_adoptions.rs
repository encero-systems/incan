//! Checked trait owners and ordered arguments survive independently of executable method slots.

use super::{bir, build};
use crate::library_manifest_index::LibraryManifestIndex;
use crate::provider::{ProviderPlan, SdkComponentSelection, SdkInventory};
use crate::{body_ir::build_body_ir_module_v0, lexer, parser, typechecker::TypeChecker};
use incan_semantics_core::{CanonicalSymbolId, IncanPrimitiveType, IncanType, SemanticSourceTargetKind, SymbolOrigin};
use std::{path::PathBuf, sync::Arc};

/// A callable alias retains its declaring standard trait and checked argument order for native ABI forwarding.
#[test]
fn callable_adoption_retains_canonical_owner_and_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let module = build_sdk(
        "from std.traits.callable import Callable1 as Transform\n\nmodel Double with Transform[int, bool]:\n    def __call__(self, value: int) -> bool:\n        return value > 0\n",
        &["m", "callable_adoption"],
    )?;
    let (identity, arguments) = adoption(&module, "Double", "Callable1")?;
    assert_eq!(identity.kind, SemanticSourceTargetKind::Trait);
    assert_eq!(
        identity.origin,
        SymbolOrigin::Package {
            library: "incan_stdlib_core".to_owned(),
            module_path: vec!["traits".to_owned(), "callable".to_owned()],
        }
    );
    assert_eq!(
        arguments,
        vec![
            IncanType::Primitive(IncanPrimitiveType::Int),
            IncanType::Primitive(IncanPrimitiveType::Bool)
        ]
    );
    Ok(())
}

/// Actual generic SDK adopters retain declared type variables rather than same-spelled nominal leaves.
#[test]
fn standard_adoption_retains_its_owner_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("../../../../../stdlib/core/src/derives/collection.incn");
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["derives".to_owned(), "collection".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_standard_library_source(true);
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("incan_stdlib_core".to_owned()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let (_, arguments) = adoption(&module, "TakeFallibleIterator", "FallibleIterator")?;
    assert_eq!(
        arguments,
        vec![IncanType::TypeVar("T".to_owned()), IncanType::TypeVar("E".to_owned())]
    );
    Ok(())
}

/// Empty TOML marker protocols remain checked adoptions through both root-module import spellings.
#[test]
fn toml_root_derives_retain_marker_adoptions() -> Result<(), Box<dyn std::error::Error>> {
    let module = build_sdk(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/codegen_modules_and_imports/std_toml_derive_through_root_import.incn"
        ),
        &["m", "toml_adoption"],
    )?;
    for owner in ["Project", "Other"] {
        for protocol in ["TomlSerialize", "TomlDeserialize"] {
            let (identity, arguments) = adoption(&module, owner, protocol)?;
            assert_eq!(identity.kind, SemanticSourceTargetKind::Trait);
            assert_eq!(
                identity.origin,
                SymbolOrigin::Package {
                    library: "incan_stdlib_data".to_owned(),
                    module_path: vec!["toml".to_owned()],
                }
            );
            assert!(arguments.is_empty());
        }
    }
    Ok(())
}

/// A source trait with the same spelling keeps its own declaration rather than acquiring standard authority.
#[test]
fn source_callable_adoption_keeps_its_distinct_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Callable1[A, R]:\n    def __call__(self, value: A) -> R: ...\n\nmodel Double with Callable1[int, int]:\n    def __call__(self, value: int) -> int:\n        return value * 2\n",
        &["m", "source_callable_adoption"],
    )?;
    let (identity, arguments) = adoption(&module, "Double", "Callable1")?;
    assert_eq!(
        identity.origin,
        SymbolOrigin::Module(vec!["m".to_owned(), "source_callable_adoption".to_owned()])
    );
    assert_eq!(
        arguments,
        vec![
            IncanType::Primitive(IncanPrimitiveType::Int),
            IncanType::Primitive(IncanPrimitiveType::Int)
        ]
    );
    Ok(())
}

/// Install the runner's actual selected SDK so provider identity assertions exercise artifact-backed resolution.
fn build_sdk(source: &str, path: &[&str]) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let inventory_path =
        PathBuf::from(std::env::var_os("INCAN_SDK_INVENTORY").ok_or("missing selected SDK inventory")?);
    let inventory = SdkInventory::read_from_path(&inventory_path)?;
    let selected = inventory.resolve(&SdkComponentSelection {
        components: ["stdlib-core", "stdlib-data"].into_iter().map(str::to_owned).collect(),
        ..SdkComponentSelection::default()
    })?;
    let plan = ProviderPlan::from_resolved_inputs(
        LibraryManifestIndex::default(),
        None,
        Some(&inventory),
        Some(&selected),
        [],
    )?;
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = path.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_provider_plan(Arc::new(plan));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// Read the serialized contract so the red tests detect an absent fact without depending on its future Rust type.
fn adoption(
    module: &bir::BodyIrModule,
    owner: &str,
    protocol: &str,
) -> Result<(CanonicalSymbolId, Vec<IncanType>), Box<dyn std::error::Error>> {
    let declaration = module
        .nominal_declarations
        .iter()
        .find(|item| item.name == owner)
        .ok_or("missing nominal")?;
    let value = serde_json::to_value(declaration)?;
    let values = value
        .get("trait_adoptions")
        .and_then(serde_json::Value::as_array)
        .ok_or("missing checked trait adoptions")?;
    for value in values {
        let identity: CanonicalSymbolId = serde_json::from_value(
            value
                .get("trait_identity")
                .ok_or("missing canonical trait owner")?
                .clone(),
        )?;
        if identity.declaration_name == protocol {
            let methods: Option<Vec<CanonicalSymbolId>> =
                serde_json::from_value(value.get("methods").ok_or("missing checked slot coverage")?.clone())?;
            let methods = methods.ok_or("unproven protocol slot coverage")?;
            if protocol == "Callable1" {
                assert_eq!(methods.len(), 1);
                assert_eq!(methods[0].declaration_name, "__call__");
                assert_eq!(methods[0].origin, identity.origin);
            } else if protocol == "FallibleIterator" {
                assert_eq!(methods.len(), 11);
            } else {
                assert!(methods.is_empty(), "a TOML marker declares no method slots");
            }
            let arguments = serde_json::from_value(
                value
                    .get("arguments")
                    .ok_or("missing ordered adoption arguments")?
                    .clone(),
            )?;
            return Ok((identity, arguments));
        }
    }
    Err(format!("missing checked {protocol} adoption for {owner}: {value}").into())
}
