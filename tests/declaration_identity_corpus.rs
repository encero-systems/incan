//! Corpus proof that a declaration identity is unique once it stops carrying position.
//!
//! The unit tests beside `StableDeclarationId` prove the rules in isolation. This proves the property that actually
//! matters and cannot be reasoned about: that across a real standard library, no two distinct declarations claim one
//! identity. Collision classes are found by measuring a corpus, not by predicting them — the overload case this
//! guards was discovered exactly that way, and it was the only one in 2,078 declarations.

use incan::frontend::body_ir::{apply_body_ir_input_contract, build_body_ir_module_v0};
use incan::frontend::hir::build_hir_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::body_ir::Body;
use incan_semantics_core::stable_identity::{DeclarationSignature, StableDeclarationId};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Collect every Incan source under a root, skipping build output.
fn collect_sources(root: &Path, out: &mut Vec<PathBuf>) -> TestResult {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if path.file_name().map(|name| name == "target").unwrap_or(false) {
                continue;
            }
            collect_sources(&path, out)?;
        } else if path.extension().map(|extension| extension == "incn").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

/// Derive the signature discriminant for a declaration from its lowered body.
///
/// A declaration with no body cannot be a callable and therefore cannot be overloaded, so `None` is correct there
/// rather than merely convenient.
fn signature_of(body: Option<&Body>) -> Option<DeclarationSignature> {
    body.map(|body| {
        DeclarationSignature::from_callable_types(body.params.iter().map(|param| &param.ty), &body.return_type)
    })
}

/// Mint every stable identity one module contributes, paired with the declaration name that claimed it.
///
/// Declaration and body are joined on the full span-carrying `CanonicalSymbolId`, which is exactly what that type is
/// valid for: unique within one compilation. The span is used for the join and excluded from the identity.
fn identities_in_module(path: &Path, source: &str) -> Result<Vec<(StableDeclarationId, String)>, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    let program = apply_body_ir_input_contract(program, path).map_err(|errors| format!("contract: {errors:?}"))?;
    let module_path = vec![path.file_stem().unwrap_or_default().to_string_lossy().to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("typecheck: {errors:?}"))?;

    let hir = build_hir_v0(&program, &module_path, checker.type_info());
    let body_ir = build_body_ir_module_v0(&program, &module_path, checker.type_info());

    let mut body_by_identity: BTreeMap<CanonicalSymbolId, &Body> = BTreeMap::new();
    for body in &body_ir.bodies {
        if let Some(canonical) = body.canonical.as_ref() {
            body_by_identity.insert(canonical.clone(), body);
        }
    }

    let mut identities = Vec::new();
    for declaration in &hir.declarations {
        let Some(canonical) = declaration.canonical.as_ref() else {
            continue;
        };
        // Only declarations this module *declares*. An import, alias, or re-export carries the declaring module's
        // identity on purpose (RFC 120: "an import, an alias, and a re-export of one declaration all carry this
        // same value"), so counting those as claimants would measure how often a declaration is referenced, not
        // whether two declarations collide.
        let declared_here = matches!(&canonical.origin, SymbolOrigin::Module(path) if path == &module_path);
        if !declared_here {
            continue;
        }
        let signature = signature_of(body_by_identity.get(canonical).copied());
        identities.push((
            StableDeclarationId::from_canonical(canonical, signature),
            declaration.name.clone().unwrap_or_else(|| "<anonymous>".to_string()),
        ));
    }
    Ok(identities)
}

#[test]
fn stable_declaration_identity_is_unique_across_the_standard_library() -> TestResult {
    let stdlib_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib");
    let mut sources = Vec::new();
    collect_sources(&stdlib_root, &mut sources)?;
    sources.sort();

    let outcome = incan::compiler_stack::run_on_compiler_stack(move || {
        let mut claims: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut modules_checked = 0usize;
        let mut modules_skipped = 0usize;
        for path in &sources {
            let Ok(source) = fs::read_to_string(path) else {
                modules_skipped += 1;
                continue;
            };
            // A module that does not compile standalone contributes no identities. That is a limitation of
            // per-file corpus evaluation, not a defect under test, so it is counted and skipped rather than failing.
            let Ok(identities) = identities_in_module(path, &source) else {
                modules_skipped += 1;
                continue;
            };
            modules_checked += 1;
            let module = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            for (identity, name) in identities {
                claims
                    .entry(identity.render_compact())
                    .or_default()
                    .push(format!("{module}::{name}"));
            }
        }
        Ok::<_, String>((claims, modules_checked, modules_skipped))
    })
    .map_err(|error| Box::<dyn std::error::Error>::from(error))?;

    let (claims, modules_checked, modules_skipped) = outcome;
    let total: usize = claims.values().map(Vec::len).sum();
    let collisions: Vec<_> = claims.iter().filter(|(_, claimants)| claimants.len() > 1).collect();

    println!("CORPUS modules_checked={modules_checked} modules_skipped={modules_skipped} declarations={total}");
    println!("CORPUS distinct_identities={} colliding={}", claims.len(), collisions.len());

    if !collisions.is_empty() {
        let detail = collisions
            .iter()
            .map(|(identity, claimants)| format!("  {identity}\n    claimed by {claimants:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "{} identities are claimed by more than one declaration:\n{detail}",
            collisions.len()
        )
        .into());
    }

    // A corpus that checked almost nothing would pass vacuously.
    if modules_checked < 90 {
        return Err(format!("corpus too small to be evidence: only {modules_checked} modules checked").into());
    }
    Ok(())
}

/// Visibility must reach HIR, because it is what roots RFC 106's external closure.
///
/// Without it a consumer cannot tell which declarations a dependent can observe, so it cannot distinguish an
/// internal-only change from one that propagates — the distinction RFC 124 relies on to avoid rebaking dependents.
#[test]
fn hir_records_declaration_visibility() -> TestResult {
    use incan_semantics_core::DeclarationVisibility;

    let source = r#"
pub def exported(value: int) -> int:
    return value


def internal(value: int) -> int:
    return value
"#;
    let source = source.to_string();
    let declarations = incan::compiler_stack::run_on_compiler_stack(move || {
        let tokens = lexer::lex(&source).map_err(|errors| format!("lex: {errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
        let program = apply_body_ir_input_contract(program, Path::new("visibility.incn"))
            .map_err(|errors| format!("contract: {errors:?}"))?;
        let module_path = vec!["visibility".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("typecheck: {errors:?}"))?;
        let hir = build_hir_v0(&program, &module_path, checker.type_info());
        Ok::<_, String>(
            hir.declarations
                .iter()
                .filter_map(|declaration| {
                    declaration
                        .name
                        .clone()
                        .map(|name| (name, declaration.visibility))
                })
                .collect::<Vec<_>>(),
        )
    })
    .map_err(|error| Box::<dyn std::error::Error>::from(error))?;

    let exported = declarations.iter().find(|(name, _)| name == "exported");
    let internal = declarations.iter().find(|(name, _)| name == "internal");
    assert_eq!(
        exported.map(|(_, visibility)| *visibility),
        Some(DeclarationVisibility::Public),
        "in {declarations:?}"
    );
    assert_eq!(
        internal.map(|(_, visibility)| *visibility),
        Some(DeclarationVisibility::Private),
        "in {declarations:?}"
    );
    Ok(())
}
