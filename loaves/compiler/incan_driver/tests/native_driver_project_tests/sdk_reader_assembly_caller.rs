//! Compare the real session's retained reader dispatch facts with its authenticated selected SDK closure.

#[path = "frontend.rs"]
mod frontend;

use incan_driver::session::CompilationSession;
use incan_frontend::executable_resolution::resolve_sdk_requirements;
use incan_frontend::provider::ProviderPlan;
use incan_semantics_core::body_ir::{Body, BodyIrModule, TraitImplementation};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, SymbolOrigin};
use std::collections::BTreeSet;
use std::path::Path;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Inspect genuine selected declarations before asking whether the actual assembler kept the same relation and body.
fn main() -> TestResult<()> {
    let source = std::env::args().nth(1).ok_or("reader source path absent")?;
    let source = Path::new(&source);
    let (assembled, sources, files, _) = frontend::collect_for_test(source)?;
    let owner = assembled
        .iter()
        .flat_map(|module| &module.nominal_declarations)
        .find(|owner| {
            owner.canonical.declaration_name == "ReaderChunks"
                && package_module(&owner.canonical, "incan_stdlib_system", &["io"])
        })
        .ok_or("checked reader owner absent")?;
    assert_eq!(owner.type_parameters, ["R"]);
    let session = CompilationSession::discover_with_feature_selection(source, &Default::default())?;
    let published = resolve_sdk_requirements(&session.provider_plan, &BTreeSet::from([owner.canonical.clone()]))?;
    let relation = reader_relation(&published.modules, &owner.canonical)?;
    let body = concrete_body(&published.modules, &relation.implementation)?;
    check_owner_frame(body)?;
    check_identity_refusals(&session.provider_plan, relation)?;
    eprintln!(
        "authenticated reader closure: owner={:?}; slot={:?}; implementation={:?}; generic receiver and three identity refusals passed",
        relation.owner, relation.method, relation.implementation
    );

    // The expected relation is obtained from the SDK resolver, never manufactured from source spelling.
    let retained = assembled
        .iter()
        .flat_map(|module| &module.trait_implementations)
        .filter(|candidate| *candidate == relation)
        .count();
    let bodies = assembled
        .iter()
        .flat_map(|module| &module.bodies)
        .filter(|candidate| candidate.canonical.as_ref() == Some(&relation.implementation))
        .count();
    assert_eq!(
        (retained, bodies),
        (1, 1),
        "selected reader protocol relation/body lost during actual session assembly"
    );
    for (index, module) in assembled.iter().enumerate() {
        if module
            .bodies
            .iter()
            .any(|candidate| candidate.canonical.as_ref() == Some(&relation.implementation))
        {
            assert!(
                sources[index].is_empty(),
                "published SDK body acquired entry-source provenance"
            );
            assert_eq!(files[index], module.module_id.path());
        }
    }
    println!("reader protocol assembly: exact relation, body, owner frame and provenance retained");
    Ok(())
}

/// Identify the concrete override of the checked fallible iterator slot for this exact published nominal owner.
fn reader_relation<'a>(modules: &'a [BodyIrModule], owner: &CanonicalSymbolId) -> TestResult<&'a TraitImplementation> {
    let mut records = modules
        .iter()
        .flat_map(|module| &module.trait_implementations)
        .filter(|record| {
            record.owner == *owner
                && record.method.declaration_name == "__next__"
                && package_module(&record.method, "incan_stdlib_core", &["derives", "collection"])
        });
    let record = records
        .next()
        .ok_or("authenticated reader implementation relation absent")?;
    assert!(records.next().is_none(), "ambiguous checked reader slot");
    assert_ne!(record.method, record.implementation);
    assert_eq!(record.implementation.kind, SemanticSourceTargetKind::Method);
    assert!(package_module(&record.implementation, "incan_stdlib_system", &["io"]));
    assert!(
        record.type_arguments.is_empty(),
        "own method acquired trait-default type arguments"
    );
    Ok(record)
}

/// The verified concrete body must retain its owner's independent binder rather than a bare nominal or method binder.
fn check_owner_frame(body: &Body) -> TestResult<()> {
    assert!(body.type_parameters.is_empty());
    let receiver = body.param_locals.first().ok_or("reader method receiver absent")?;
    let local = body
        .locals
        .get(usize::try_from(receiver.0)?)
        .ok_or("reader receiver local absent")?;
    assert_eq!(
        local.ty,
        IncanType::Generic {
            base: "ReaderChunks".into(),
            args: vec![IncanType::TypeVar("R".into())]
        }
    );
    Ok(())
}

/// Read a concrete method by the full implementation identity the artifact supplies, including its declaration span.
fn concrete_body<'a>(modules: &'a [BodyIrModule], identity: &CanonicalSymbolId) -> TestResult<&'a Body> {
    let mut bodies = modules
        .iter()
        .flat_map(|module| &module.bodies)
        .filter(|body| body.canonical.as_ref() == Some(identity));
    let body = bodies.next().ok_or("authenticated reader implementation body absent")?;
    assert!(
        bodies.next().is_none(),
        "duplicate authenticated reader implementation body"
    );
    Ok(body)
}

/// A same-spelled owner, slot or implementation in the wrong checked source location cannot select a fragment.
fn check_identity_refusals(plan: &ProviderPlan, record: &TraitImplementation) -> TestResult<()> {
    for identity in [&record.owner, &record.method, &record.implementation] {
        let mut wrong = identity.clone();
        wrong.declaration_span.start = wrong.declaration_span.start.saturating_add(1);
        assert!(
            resolve_sdk_requirements(plan, &BTreeSet::from([wrong])).is_err(),
            "wrong checked source span selected SDK executable context"
        );
    }
    Ok(())
}

/// Use package/module spelling only to locate a candidate; all assembly comparisons use the full checked identity.
fn package_module(identity: &CanonicalSymbolId, expected_library: &str, expected_path: &[&str]) -> bool {
    matches!(&identity.origin, SymbolOrigin::Package { library, module_path }
        if library == expected_library && module_path.iter().map(String::as_str).eq(expected_path.iter().copied()))
}
