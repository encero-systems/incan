//! Local target decisions use the same source-bound records as ordinary registry loaves.

use super::{LocalFacetSelection, local_dependency_demands};
use crate::native_loaf::NativeLoafPredicate;
use crate::sdk_closure::{SdkClosureReport, SdkCompiledClosure};
use std::collections::BTreeMap;

/// Create an explicit caller-resolved predicate while retaining its original declaration coordinate.
fn predicate(index: usize, condition: &str, matches: bool) -> NativeLoafPredicate {
    NativeLoafPredicate {
        declaration: index,
        target: condition.into(),
        matches,
    }
}

/// Missing, duplicated, stale and undeclared decisions refuse even when the affected dependency is optional.
#[test]
fn local_target_predicates_require_exact_declaration_coverage() -> Result<(), Box<dyn std::error::Error>> {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
a = { loaf='a', version='1' }
z = { loaf='z', version='1', optional=true, target='cfg(windows)' }
"#,
    )?;
    assert!(local_dependency_demands(&manifest, &[], &[]).is_err());
    let inactive = predicate(1, "cfg(windows)", false);
    let demands = local_dependency_demands(&manifest, &[], &[inactive.clone()])?;
    assert_eq!(demands.len(), 1);
    assert_eq!(demands[0].loaf, "a");
    for invalid in [
        vec![inactive.clone(), inactive.clone()],
        vec![predicate(0, "cfg(windows)", false)],
        vec![predicate(1, "cfg(unix)", false)],
        vec![inactive.clone(), predicate(2, "cfg(unix)", true)],
    ] {
        assert!(local_dependency_demands(&manifest, &[], &invalid).is_err());
    }
    assert_eq!(
        local_dependency_demands(&manifest, &["z".into()], &[inactive])?.len(),
        1
    );
    assert_eq!(
        local_dependency_demands(&manifest, &["z".into()], &[predicate(1, "cfg(windows)", true)])?.len(),
        2
    );
    assert!(
        local_dependency_demands(
            &toml::Value::Table(Default::default()),
            &[],
            &[predicate(0, "cfg(unix)", true)]
        )
        .is_err()
    );
    Ok(())
}

/// A local compiler invocation may omit an inactive unavailable dependency, but never a missing decision.
#[test]
fn local_target_predicates_compile_and_reuse_with_original_owners() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let project = fixture.path().join("conditional");
    std::fs::create_dir_all(project.join("src"))?;
    std::fs::write(project.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n")?;
    std::fs::write(
        project.join("loaf.toml"),
        "[project]\nname='conditional'\nversion='1.0.0'\n[rust]\ntype='lib'\nedition='2024'\n[dependencies]\nmissing={loaf='missing',target='cfg(any())'}\n",
    )?;
    let rustc = crate::rustc::resolve_active_rustc()?;
    let target = crate::rustc::rustc_host_target(&rustc)?;
    let mut closure = SdkCompiledClosure {
        report: SdkClosureReport::default(),
        units: Vec::new(),
        auxiliary_targets: BTreeMap::new(),
    };
    let mut selected = LocalFacetSelection {
        project: project.clone(),
        features: Vec::new(),
        domain: "target".into(),
        target: Some(target),
        target_predicates: vec![predicate(0, "cfg(any())", false)],
    };
    let output = fixture.path().join("output");
    super::compile_local_sdk_facets(
        &mut closure,
        std::slice::from_ref(&selected),
        fixture.path(),
        &output,
        &rustc,
    )?;
    assert!(closure.report.failed.is_empty(), "{:?}", closure.report.failed);
    assert_eq!(closure.units.len(), 1);
    let identity = closure.units[0].compiled_identity().to_string();
    assert_eq!(closure.units[0].binding.target_predicates, selected.target_predicates);
    super::compile_local_sdk_facets(
        &mut closure,
        std::slice::from_ref(&selected),
        fixture.path(),
        &output,
        &rustc,
    )?;
    assert_eq!(closure.report.compiled.len(), 1);
    assert_eq!(closure.report.reused.len(), 1);
    assert_eq!(closure.units[0].compiled_identity(), identity);
    selected.target_predicates[0].matches = true;
    super::compile_local_sdk_facets(
        &mut closure,
        std::slice::from_ref(&selected),
        fixture.path(),
        &output,
        &rustc,
    )?;
    assert_eq!(closure.report.failed.len(), 1);
    assert_eq!(closure.report.reused.len(), 1);
    assert_eq!(closure.units[0].compiled_identity(), identity);
    selected.target_predicates[0].matches = false;
    selected.target_predicates.clear();
    assert!(
        super::compile_local_sdk_facets(
            &mut closure,
            std::slice::from_ref(&selected),
            fixture.path(),
            &output,
            &rustc
        )
        .is_err()
    );
    selected.target_predicates.push(predicate(0, "cfg(any())", false));
    selected.target = None;
    assert!(super::compile_local_sdk_facets(&mut closure, &[selected], fixture.path(), &output, &rustc).is_err());
    Ok(())
}
