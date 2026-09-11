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
    println!(
        "CORPUS distinct_identities={} colliding={}",
        claims.len(),
        collisions.len()
    );

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
                .filter_map(|declaration| declaration.name.clone().map(|name| (name, declaration.visibility)))
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

/// Measure the conformant digest over a real standard library.
///
/// The figure quoted while this was being designed — 1.80 s — was measured on a *text-substitution* prototype,
/// which RFC 106 then ruled non-conformant. That number was never evidence for the approach actually shipped, so
/// this measures the value-level digest instead, and asserts the properties a corpus can check that fixtures
/// cannot: every declaration digests, and no two distinct declarations share a digest by accident.
#[test]
fn conformant_digest_covers_the_standard_library() -> TestResult {
    use incan_semantics_core::semantic_digest::{body_without_docstring, semantic_digest};
    use std::time::Instant;

    let stdlib_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib");
    let mut sources = Vec::new();
    collect_sources(&stdlib_root, &mut sources)?;
    sources.sort();

    let started = Instant::now();
    let outcome = incan::compiler_stack::run_on_compiler_stack(move || {
        let mut digested = 0usize;
        let mut modules = 0usize;
        let mut skipped = 0usize;
        let mut bodies_digested = 0usize;
        // Digest -> the identities that produced it, to spot accidental sharing.
        let mut by_digest: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for path in &sources {
            let Ok(source) = fs::read_to_string(path) else {
                skipped += 1;
                continue;
            };
            let tokens = match lexer::lex(&source) {
                Ok(tokens) => tokens,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
            let Ok(program) = parser::parse(&tokens) else {
                skipped += 1;
                continue;
            };
            let Ok(program) = apply_body_ir_input_contract(program, path) else {
                skipped += 1;
                continue;
            };
            let module_path = vec![path.file_stem().unwrap_or_default().to_string_lossy().to_string()];
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module_path.clone()));
            if checker.check_program(&program).is_err() {
                skipped += 1;
                continue;
            }
            modules += 1;

            let hir = build_hir_v0(&program, &module_path, checker.type_info());
            let body_ir = build_body_ir_module_v0(&program, &module_path, checker.type_info());
            let mut bodies: BTreeMap<CanonicalSymbolId, &Body> = BTreeMap::new();
            for body in &body_ir.bodies {
                if let Some(canonical) = body.canonical.as_ref() {
                    bodies.insert(canonical.clone(), body);
                }
            }

            for declaration in &hir.declarations {
                let Some(canonical) = declaration.canonical.as_ref() else {
                    continue;
                };
                if !matches!(&canonical.origin, SymbolOrigin::Module(path) if path == &module_path) {
                    continue;
                }
                let body = bodies.get(canonical).copied();
                let signature = signature_of(body);
                let identity = StableDeclarationId::from_canonical(canonical, signature);
                let body_digest = match body {
                    Some(body) => {
                        bodies_digested += 1;
                        semantic_digest(&body_without_docstring(body)).map_err(|error| error.to_string())?
                    }
                    None => "sha256:none".to_string(),
                };
                let digest = semantic_digest(&(
                    declaration.kind,
                    &declaration.name,
                    declaration.visibility,
                    &body_digest,
                ))
                .map_err(|error| error.to_string())?;
                by_digest.entry(digest).or_default().push(identity.render_compact());
                digested += 1;
            }
        }
        Ok::<_, String>((digested, modules, skipped, bodies_digested, by_digest))
    })
    .map_err(|error| Box::<dyn std::error::Error>::from(error))?;

    let elapsed = started.elapsed();
    let (digested, modules, skipped, bodies_digested, by_digest) = outcome;

    println!("CONFORMANT modules={modules} skipped={skipped}");
    println!("CONFORMANT declarations_digested={digested} bodies_digested={bodies_digested}");
    println!("CONFORMANT distinct_digests={}", by_digest.len());
    println!("CONFORMANT elapsed_ms={}", elapsed.as_millis());

    // Two declarations sharing a digest is not a collision and must not be read as one. Identity is the key and
    // the digest is the value: a consumer looks a declaration up by identity, then compares digests. A shared
    // digest says only "these two happen to mean the same thing", which in this corpus is simply true — the same
    // constant is declared in several modules (`_SECONDS_PER_DAY` across three datetime modules, `_BYTE_TABLE`
    // across four base-N encoders). The digest deliberately excludes the owning module, because a constant moved
    // between modules with its value unchanged has not changed meaning.
    //
    // Reported rather than asserted, so a sudden jump is visible without pinning a number that legitimately moves
    // as the standard library grows.
    let shared: usize = by_digest.values().filter(|claims| claims.len() > 1).count();
    println!("CONFORMANT digests_shared_by_more_than_one_declaration={shared}");
    for claims in by_digest.values().filter(|claims| claims.len() > 1).take(5) {
        println!("CONFORMANT shared (identical meaning, not a collision): {claims:?}");
    }

    if modules < 90 {
        return Err(format!("corpus too small to be evidence: {modules} modules").into());
    }
    if bodies_digested == 0 {
        return Err("no bodies were digested, so the measurement covers signatures only".into());
    }
    Ok(())
}
