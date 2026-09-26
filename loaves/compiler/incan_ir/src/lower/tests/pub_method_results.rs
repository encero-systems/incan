//! The result type of a method called on a `pub::` dependency's type keeps every union the provider owns (#1797).

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

/// The `querykit` provider: one model whose methods return `int | str` bare, in a list and in an option.
const QUERYKIT: &str = r#"
pub model Box:
    pub value: int

    def answer(self) -> int | str:
        if self.value > 0:
            return self.value
        return "empty"

    def answers(self) -> list[int | str]:
        return [self.answer()]

    def maybe(self) -> Option[int | str]:
        if self.value > 0:
            return Some(self.answer())
        return None
"#;

/// A consumer passing each result to its own `int | str` positions.
const CONSUMER: &str = r#"
from pub::querykit import Box


def show(value: int | str, label: str) -> str:
    match value:
        int(number) => return f"{label} {number}"
        str(text) => return f"{label} {text}"


def show_first(values: list[int | str]) -> str:
    return show(values[0], "first")


def main() -> None:
    println(show(Box(value=3).answer(), "answer"))
    println(show_first(Box(value=5).answers()))
    match Box(value=7).maybe():
        int(number) => println(f"maybe {number}")
        str(text) => println(f"maybe {text}")
        None => println("none")
"#;

/// Check the `querykit` package and index the manifest its checked public exports publish.
fn querykit_index() -> Result<LibraryManifestIndex, String> {
    let program = parse_module(QUERYKIT, "querykit")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["lib".to_string()]));
    checker.set_current_package_identity(Some("querykit".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("querykit should typecheck: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let manifest = LibraryManifest::from_checked_exports("querykit", "0.1.0", &exports);
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        "querykit".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "querykit",
                "querykit",
                std::env::temp_dir().join("incan_issue1797_querykit"),
            ),
        },
    )])))
}

/// Check and lower the consumer against the `querykit` package, as a consumer build does.
fn lowered_consumer() -> Result<IrProgram, String> {
    let index = querykit_index()?;
    let program = parse_module(CONSUMER, "consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(index.clone());
    checker
        .check_program(&program)
        .map_err(|errors| format!("consumer should typecheck: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    lowering.set_provider_plan(Some(Arc::new(ProviderPlan::for_library_index(index))));
    lowering
        .lower_program(&program)
        .map_err(|errors| format!("consumer lowering failed: {errors:?}"))
}

/// Collect every method call on a `querykit` receiver: its result type and the result type of its signature.
#[derive(Default)]
struct DependencyMethodResults(Vec<(IrType, Option<IrType>)>);

impl Visitor for DependencyMethodResults {
    /// Record a method call whose receiver is the dependency's `Box`, then visit its children.
    fn expr(&mut self, expr: &mut TypedExpr) {
        if let IrExprKind::MethodCall {
            receiver,
            callable_signature,
            ..
        } = &expr.kind
            && matches!(&receiver.ty, IrType::Struct(name) if name.ends_with("Box"))
        {
            self.0.push((
                expr.ty.clone(),
                callable_signature
                    .as_ref()
                    .map(|signature| signature.return_type.clone()),
            ));
        }
        walk_expr(expr, self);
    }
}

/// Return whether `ty` is the `querykit` package's own `int | str` carrier.
fn is_querykit_union(ty: &IrType) -> bool {
    matches!(ty, IrType::ExternalUnion { library, union, .. }
    if library == "querykit"
        && union.union_members().is_some_and(|members| {
            members.len() == 2 && members.contains(&IrType::Int) && members.contains(&IrType::String)
        }))
}

/// #1797: `Box(value=3).answer()` lowers with the provider's `int | str` carrier as its result, and so do the union
/// inside `answers()`'s list and `maybe()`'s option. The checker records the call site's parameters only, so the call
/// takes its result from the method's declaration in the provider, which names the owning package at every union
/// position; a union spelled structurally would be re-owned by the consumer while the method returns the provider's.
#[test]
fn pub_dependency_method_results_keep_the_providers_union_issue1797() -> Result<(), String> {
    let mut ir = lowered_consumer()?;
    let main = ir
        .declarations
        .iter_mut()
        .find_map(|decl| match &mut decl.kind {
            IrDeclKind::Function(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .ok_or("the consumer lowers a `main`")?;
    let mut results = DependencyMethodResults::default();
    for stmt in &mut main.body {
        results.stmt(stmt);
    }
    let results = results.0;
    assert_eq!(
        results.len(),
        3,
        "one call each to answer, answers and maybe: {results:?}"
    );

    let (answer, answer_signature) = &results[0];
    assert!(
        is_querykit_union(answer),
        "`answer()` keeps the provider's union, got {answer:?}"
    );
    assert_eq!(
        answer_signature.as_ref(),
        Some(answer),
        "the call's signature carries the same result"
    );

    let (answers, answers_signature) = &results[1];
    assert!(
        matches!(answers, IrType::List(element) if is_querykit_union(element)),
        "the list element of `answers()` keeps the provider's union, got {answers:?}"
    );
    assert_eq!(answers_signature.as_ref(), Some(answers));

    let (maybe, maybe_signature) = &results[2];
    assert!(
        matches!(maybe, IrType::Option(inner) if is_querykit_union(inner)),
        "the payload of `maybe()` keeps the provider's union, got {maybe:?}"
    );
    assert_eq!(maybe_signature.as_ref(), Some(maybe));
    Ok(())
}
