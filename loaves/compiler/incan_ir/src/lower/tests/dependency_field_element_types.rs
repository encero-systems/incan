//! A consumer builds a `pub::` dependency's model with empty collections and `None` for fields whose element types it
//! never imports, and each such type is spelled from the dependency crate (#1561).

use super::import_paths::parse_module;
use super::*;
use crate::visit::{Visitor, walk_expr};
use incan_frontend::library_exports::collect_checked_public_exports;
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::provider::ProviderPlan;
use std::sync::Arc;

/// The `records` provider: a card whose fields hold the provider's own identifiers in a list, a dict and an option.
const RECORDS: &str = r#"
pub newtype EvidenceId = str


pub newtype MemoryId = str


pub model Card:
    pub evidence_ids: list[EvidenceId]
    pub memory_ids: dict[str, MemoryId]
    pub first: Option[EvidenceId]
"#;

/// A consumer importing only `Card`, building it with an empty list, an empty dict and `None`.
const CONSUMER: &str = r#"
from pub::records import Card


def main() -> None:
    card = Card(evidence_ids=[], memory_ids={}, first=None)
    println(len(card.evidence_ids))
"#;

/// Check the `records` package and index the manifest its checked public exports publish.
fn records_index() -> Result<LibraryManifestIndex, String> {
    let program = parse_module(RECORDS, "records")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["lib".to_string()]));
    checker.set_current_package_identity(Some("records".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("records should typecheck: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let manifest = LibraryManifest::from_checked_exports("records", "0.1.0", &exports);
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        "records".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "records",
                "records",
                std::env::temp_dir().join("incan_issue1561_records"),
            ),
        },
    )])))
}

/// Collect the type of every empty list, empty dict and `None` in the lowered program.
#[derive(Default)]
struct EmptyValueTypes(Vec<IrType>);

impl Visitor for EmptyValueTypes {
    /// Record an empty collection literal or a `None`, then visit the children.
    fn expr(&mut self, expr: &mut TypedExpr) {
        let empty = match &expr.kind {
            IrExprKind::List(entries) => entries.is_empty(),
            IrExprKind::Dict(entries) => entries.is_empty(),
            IrExprKind::None => true,
            _ => false,
        };
        if empty {
            self.0.push(expr.ty.clone());
        }
        walk_expr(expr, self);
    }
}

/// Return whether `ty` names a nominal type by its bare name, which the consumer's module never binds.
fn names_a_bare_dependency_type(ty: &IrType) -> bool {
    match ty {
        IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
            (name == "EvidenceId" || name == "MemoryId")
                || matches!(ty, IrType::NamedGeneric(_, args) if args.iter().any(names_a_bare_dependency_type))
        }
        IrType::List(element) | IrType::Option(element) => names_a_bare_dependency_type(element),
        IrType::Dict(key, value) => names_a_bare_dependency_type(key) || names_a_bare_dependency_type(value),
        _ => false,
    }
}

/// #1561: `Card(evidence_ids=[], memory_ids={}, first=None)` in a consumer that imports only `Card` gives the empty
/// list, the empty dict and `None` the field types with `EvidenceId` and `MemoryId` spelled from the `records` crate;
/// the bare names resolve to nothing in the consumer's crate (rustc E0425).
#[test]
fn dependency_field_element_types_the_consumer_never_imports_are_spelled_from_the_dependency_issue1561()
-> Result<(), String> {
    let index = records_index()?;
    let program = parse_module(CONSUMER, "consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(index.clone());
    checker
        .check_program(&program)
        .map_err(|errors| format!("consumer should typecheck: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    lowering.set_provider_plan(Some(Arc::new(ProviderPlan::for_library_index(index))));
    let mut ir = lowering
        .lower_program(&program)
        .map_err(|errors| format!("consumer lowering failed: {errors:?}"))?;
    let main = ir
        .declarations
        .iter_mut()
        .find_map(|decl| match &mut decl.kind {
            IrDeclKind::Function(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .ok_or("the consumer lowers a `main`")?;
    let mut types = EmptyValueTypes::default();
    for stmt in &mut main.body {
        types.stmt(stmt);
    }
    let types = types.0;
    assert_eq!(types.len(), 3, "an empty list, an empty dict and a `None`: {types:?}");
    assert!(
        !types.iter().any(names_a_bare_dependency_type),
        "each dependency type is spelled from its crate: {types:?}"
    );
    Ok(())
}
