//! Fixtures the frontend's tests share with the crates above it: a seeded rust-inspect workspace and a checked program
//! with a vocabulary declaration injected after typechecking. On under `cfg(test)` and the `test_support` feature,
//! which the root crate's dev-dependency turns on.

use crate::ast;
use crate::body_ir::build_body_ir_module_v0;
use crate::provider::ProviderPlan;
use crate::typechecker::{TypeCheckInfo, TypeChecker};
use crate::{lexer, parser};
use incan_semantics_core::body_ir as bir;
#[cfg(feature = "rust_inspect")]
use std::fs;

/// A temporary Cargo workspace holding one probe package, for tests that drive the inspector against real metadata.
#[cfg(feature = "rust_inspect")]
pub fn seeded_rust_inspect_workspace() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        r#"[package]
name = "ra_seeded_metadata_probe"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    Ok(tmp)
}

/// Lower `source` after adding a raw top-level declaration, **after** typechecking.
///
/// The source parser only builds vocabulary declarations after an imported vocabulary is active, and a healthy
/// compiler desugars them before typechecking. Adding one after checking isolates the lowerer's final safety net:
/// every executable body must refuse the raw declaration rather than silently dropping it during top-level
/// collection.
pub fn build_with_top_level_declaration_injected_after_typecheck(
    source: &str,
    module_path: &[&str],
    injected: ast::Spanned<ast::Declaration>,
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    program.declarations.push(injected);
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// A top-level vocabulary declaration whose source meaning has not been desugared.
pub fn fixture_top_level_vocab_declaration() -> ast::Spanned<ast::Declaration> {
    ast::Spanned::new(
        ast::Declaration::VocabBlock(ast::VocabBlockStmt {
            keyword: "query".to_string(),
            keyword_binding: ast::VocabKeywordBinding {
                is_declaration_owned_clause: false,
                dependency_key: "demo.query".to_string(),
                activation_namespace: "demo".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::FunctionDecl,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
                clause_body_kind: None,
            },
            decorators: Vec::new(),
            signature_head: None,
            header_args: Vec::new(),
            body: Vec::new(),
            body_item_trailing_commas: Vec::new(),
        }),
        ast::Span::new(40, 60),
    )
}

/// Construct the selected provider plan from the checked declaration metadata produced by `source`.
///
/// This intentionally follows the production direction: the declaration-side decorator resolves a capability,
/// publication persists that checked pair into the provider manifest, and consumer lowering projects the selected
/// manifest through its provider plan. Tests must not manually fill `ProviderOperationCatalog`, because that would
/// bypass the compiler-owned producer contract #1213 adds.
pub fn provider_plan_from_checked_source(
    type_info: &TypeCheckInfo,
    state: bir::ProviderActivationState,
) -> Result<ProviderPlan, Box<dyn std::error::Error>> {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use crate::library_manifest::{CompiledProviderMetadata, LibraryManifest, ProviderOperationMetadata};
    use crate::library_manifest_index::LibraryManifestIndex;
    use crate::provider::{NamespaceAuthority, ProviderIdentity, ProviderProvenance, ProviderRecord};

    let operation_descriptors = type_info
        .declarations
        .provider_operations
        .values()
        .map(|operation| ProviderOperationMetadata {
            operation: operation.operation.clone(),
            required_capability: operation.required_capability.clone(),
            runtime_requirements: operation.runtime_requirements.clone(),
        })
        .collect::<Vec<_>>();
    if operation_descriptors.is_empty() {
        return Err("fixture source declared no checked provider operation".into());
    }

    let namespace_claims = operation_descriptors
        .iter()
        .map(|descriptor| {
            descriptor
                .operation
                .module_path()
                .map(ToOwned::to_owned)
                .ok_or("provider operation identity must name a module declaration")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut manifest = LibraryManifest::new("fixture_provider", "0.1.0");
    manifest.contract_metadata.provider = CompiledProviderMetadata {
        operation_descriptors,
        ..CompiledProviderMetadata::default()
    };
    let (enabled, available) = match state {
        bir::ProviderActivationState::Active => (true, true),
        bir::ProviderActivationState::Disabled => (false, true),
        bir::ProviderActivationState::Unavailable => (true, false),
    };
    ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![ProviderRecord {
            identity: ProviderIdentity {
                name: "fixture_provider".to_string(),
                version: "0.1.0".to_string(),
                digest: "fixture:provider-operation".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Compiler,
            authority: NamespaceAuthority::Compiler,
            namespace_claims: namespace_claims.clone(),
            available,
            enabled,
            // The selected metadata remains available for a known-but-unavailable fixture so lowering can issue
            // its explicit source-span refusal instead of treating the operation as an ordinary call.
            manifest: Some(Arc::new(manifest)),
            artifact: None,
            implementation_facets: Vec::new(),
        }],
        namespace_claims,
    )
    .map_err(|error| error.to_string().into())
}
