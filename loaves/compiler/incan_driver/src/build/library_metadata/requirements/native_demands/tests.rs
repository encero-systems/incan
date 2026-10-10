//! Actual frontend capture and malformed persisted demand controls; no native or macro execution occurs here.

use std::collections::BTreeSet;
use std::fs;

use incan_frontend::ParsedModule;
use incan_frontend::library_exports::collect_checked_public_exports;
use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_frontend::typechecker::TypeChecker;
use incan_provider::ProviderPlan;
use incan_provider::requirements::ProjectRequirements;
use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};

use super::{CheckedNativeDemands, ManifestDemand, capture_checked_native_demands};
use crate::build::library_metadata::requirements::{CheckedLibraryCapture, CheckedLibraryRequirements};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const SCALAR: &str = "pub def scale(mut value: int, factor: int = 3) -> int:\n    \"\"\"Scale the primitive input.\"\"\"\n    let result: int = value * factor\n    return result\n";

/// Parse actual source and retain its real module provenance; negative early gates need not invoke foreign checking.
fn module(root: &std::path::Path, source: &str) -> Result<ParsedModule, Box<dyn std::error::Error>> {
    let tokens = incan_frontend::lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?;
    let ast = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    Ok(ParsedModule {
        name: "lib".to_string(),
        path_segments: vec!["lib".to_string()],
        file_path: root.join("src/lib.incn"),
        source: source.to_string(),
        ast,
    })
}

/// Load a real ordinary manifest and an empty dependency plan without SDK selection or native compilation.
fn project(root: &std::path::Path, extra: &str) -> Result<(ProjectManifest, ProviderPlan), Box<dyn std::error::Error>> {
    fs::write(
        root.join("loaf.toml"),
        format!("[project]\nname='ordinary'\nversion='1.0.0'\n{extra}"),
    )?;
    Ok((
        ProjectManifest::load(&root.join("loaf.toml"))?,
        ProviderPlan::new(LibraryManifestIndex::default(), Vec::new(), std::iter::empty())?,
    ))
}

/// Capture a genuine checked scalar contract using the same frontend exports and demand collector as production.
fn checked(root: &std::path::Path) -> Result<CheckedLibraryRequirements, Box<dyn std::error::Error>> {
    checked_source(root, SCALAR)
}

/// Run real checking and contract capture for an ordinary source case, including non-scalar declarations.
fn checked_source(
    root: &std::path::Path,
    source: &str,
) -> Result<CheckedLibraryRequirements, Box<dyn std::error::Error>> {
    let (project, providers) = project(root, "")?;
    let module = module(root, source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("ordinary".to_string()));
    checker.set_current_module_path(Some(module.path_segments.clone()));
    checker
        .check_program(&module.ast)
        .map_err(|errors| format!("check: {errors:?}"))?;
    let exports = collect_checked_public_exports(&module.ast, &checker);
    let requirements = ProjectRequirements::default();
    let abi = BTreeSet::new();
    let demands =
        capture_checked_native_demands(&project, std::slice::from_ref(&module), &requirements, &providers, &abi)?;
    demands.require_ordinary_source_inspection()?;
    let sources = [("src/lib.incn".to_string(), module.path_segments.clone())].into();
    Ok(CheckedLibraryRequirements::capture(CheckedLibraryCapture {
        project: &project,
        index: &LibraryManifestIndex::default(),
        requirements: &requirements,
        imports: &[],
        exports: &exports,
        version: "1.0.0",
        used_module_paths: BTreeSet::new(),
        source_modules: sources,
        entry_module: module.path_segments,
        rust_abi_queries: abi,
        rust_extern_paths: Vec::new(),
        backend: None,
        native_demands: demands,
    })?)
}

/// Ordinary source shapes reach real checking, retain their checked exports and roundtrip with explicit demand facts.
#[test]
fn ordinary_native_checked_generic_and_model_source_roundtrip() -> TestResult {
    let root = tempfile::tempdir()?;
    let source = "pub model Holder:\n    value: int\n\npub def first_or[T](values: list[T], fallback: T) -> T:\n    if len(values) > 0:\n        return values[0]\n    return fallback\n";
    let contract = checked_source(root.path(), source)?;
    contract.require_ordinary_source_inspection_native()?;
    assert!(contract.require_source_inspection_native().is_err());
    assert!(!contract.caller_exports()?.is_empty());
    let payload = serde_json::to_value(&contract)?;
    let decoded: CheckedLibraryRequirements = serde_json::from_value(payload.clone())?;
    decoded.require_ordinary_source_inspection_native()?;
    for (field, value) in [
        ("native_imports", serde_json::json!(["unbound::item"])),
        ("native_crates", serde_json::json!(["unbound"])),
        ("schema_version", serde_json::json!(999)),
    ] {
        let mut changed = payload.clone();
        changed["native_demands"]["observed"]["ordinary_source"][field] = value;
        let decoded: CheckedLibraryRequirements = serde_json::from_value(changed)?;
        assert!(decoded.require_ordinary_source_inspection_native().is_err(), "{field}");
    }
    let mut missing = payload;
    missing["native_demands"]["observed"]
        .as_object_mut()
        .ok_or("facts missing")?
        .remove("ordinary_source");
    let decoded: CheckedLibraryRequirements = serde_json::from_value(missing)?;
    assert!(decoded.require_ordinary_source_inspection_native().is_err());
    Ok(())
}

/// Broad Incan declaration coverage cannot grant foreign source, provider namespaces or unsupported native execution.
#[test]
fn ordinary_native_loaded_source_refuses_unbound_native_and_provider_demands() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = project(root.path(), "")?;
    for source in [
        "from rust::foreign import item\npub def answer() -> int:\n    return 42\n",
        "import std.prelude\npub def answer() -> int:\n    return 42\n",
        "import pub::other\npub def answer() -> int:\n    return 42\n",
    ] {
        let parsed = module(root.path(), source)?;
        let demands = capture_checked_native_demands(
            &project,
            &[parsed],
            &ProjectRequirements::default(),
            &providers,
            &BTreeSet::new(),
        )?;
        assert!(demands.require_ordinary_source_inspection().is_err(), "{source}");
    }
    let parsed = module(root.path(), SCALAR)?;
    let demands = capture_checked_native_demands(
        &project,
        &[parsed],
        &ProjectRequirements::default(),
        &providers,
        &["unbound::item".to_string()].into(),
    )?;
    assert!(demands.require_ordinary_source_inspection().is_err());
    Ok(())
}

/// Publication collects its explicit mandatory-facet ABI imports independently of consumer prewarm exclusions.
#[cfg(feature = "rust_inspect")]
#[test]
fn ordinary_native_publication_captures_mandatory_facet_imports() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = project(root.path(), "")?;
    let parsed = module(
        root.path(),
        "from rust::incan_std_core::errors import raise_value_error\n",
    )?;
    let queries =
        crate::build::library_exports::collect_library_rust_abi_query_paths(std::slice::from_ref(&parsed), &[])
            .into_iter()
            .collect::<BTreeSet<_>>();
    assert_eq!(
        queries,
        ["incan_std_core::errors::raise_value_error".to_string()].into()
    );
    let requirements = incan_provider::requirements::collect_project_requirements(
        std::slice::from_ref(&parsed),
        &LibraryManifestIndex::default(),
    )?;
    assert_eq!(requirements.stdlib_facets, vec!["incan_std_core"]);
    let demands = capture_checked_native_demands(
        &project,
        std::slice::from_ref(&parsed),
        &requirements,
        &providers,
        &queries,
    )?;
    demands.require_ordinary_source_inspection()?;
    let missing = capture_checked_native_demands(&project, &[parsed], &requirements, &providers, &BTreeSet::new())?;
    assert!(missing.require_ordinary_source_inspection().is_err());
    Ok(())
}

/// Real checked primitive parameters, defaults, bindings and arithmetic preserve explicit coverage on roundtrip.
#[test]
fn ordinary_native_checked_scalar_demands_first_repeat_roundtrip() -> TestResult {
    let root = tempfile::tempdir()?;
    let original = checked(root.path())?;
    original.require_support_only_native()?;
    original.require_source_inspection_native()?;
    assert_eq!(original.schema_version, 2);
    for _ in 0..2 {
        let decoded: CheckedLibraryRequirements = serde_json::from_slice(&serde_json::to_vec(&original)?)?;
        decoded.require_support_only_native()?;
        assert_eq!(decoded.caller_exports()?.len(), 1);
    }
    Ok(())
}

/// Old metadata stays usable by legacy consumers while absent facts never grant ordinary semantic coverage.
#[test]
fn ordinary_native_demands_missing_old_and_current_payloads_are_unknown() -> TestResult {
    let root = tempfile::tempdir()?;
    let original = checked(root.path())?;
    for version in [1, 2] {
        let mut payload = serde_json::to_value(&original)?;
        payload["schema_version"] = serde_json::json!(version);
        payload
            .as_object_mut()
            .ok_or("contract is not an object")?
            .remove("native_demands");
        let decoded: CheckedLibraryRequirements = serde_json::from_value(payload)?;
        decoded.validate()?;
        assert!(decoded.require_support_only_native().is_err());
        assert!(decoded.require_source_inspection_native().is_err());
    }
    assert!(CheckedNativeDemands::default().require_support_only().is_err());
    Ok(())
}

/// Persisted facts cannot be detached from actual checked ABI, module provenance or dependency requirements.
#[test]
fn ordinary_native_demands_refuse_contract_association_substitution() -> TestResult {
    let root = tempfile::tempdir()?;
    let original = checked(root.path())?;
    for (field, replacement) in [
        ("rust_abi_queries", serde_json::json!(["foreign::Type"])),
        ("source_modules", serde_json::json!({"src/other.incn":["other"]})),
        ("dependency_aliases", serde_json::json!(["foreign"])),
        ("stdlib_facets", serde_json::json!(["incan_std_data"])),
        ("schema_version", serde_json::json!(999)),
    ] {
        let mut payload = serde_json::to_value(&original)?;
        payload["native_demands"]["observed"][field] = replacement;
        let decoded: CheckedLibraryRequirements = serde_json::from_value(payload)?;
        assert!(decoded.validate().is_err(), "{field}");
    }
    Ok(())
}

/// Missing manifest, derive, ABI or AST-proof fields inside observed facts cannot deserialize as empty demand.
#[test]
fn ordinary_native_demands_incomplete_observed_payload_refuses() -> TestResult {
    let root = tempfile::tempdir()?;
    let original = checked(root.path())?;
    for field in [
        "vocab_manifest",
        "c_manifest",
        "rust_derive_probe_paths",
        "rust_abi_queries",
        "unsupported_source",
        "provider_contracts",
        "physical_projections",
    ] {
        let mut payload = serde_json::to_value(&original)?;
        payload["native_demands"]["observed"]
            .as_object_mut()
            .ok_or("facts are not an object")?
            .remove(field);
        assert!(
            serde_json::from_value::<CheckedLibraryRequirements>(payload).is_err(),
            "{field}"
        );
    }
    Ok(())
}

/// A supplied checked ABI query is additional demand even when no source Rust import remains in the scalar AST.
#[test]
fn ordinary_native_demands_refuse_recorded_abi_queries() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = project(root.path(), "")?;
    let module = module(root.path(), SCALAR)?;
    let queries = ["foreign::Scalar".to_string()].into();
    let demands = capture_checked_native_demands(
        &project,
        &[module],
        &ProjectRequirements::default(),
        &providers,
        &queries,
    )?;
    assert!(demands.require_support_only().is_err());
    assert_eq!(
        demands.observed.as_ref().ok_or("facts missing")?.rust_abi_queries,
        queries
    );
    Ok(())
}

/// The existing derive collector records canonical imported/explicit paths; an empty ABI set cannot hide a macro.
#[test]
fn ordinary_native_demands_derive_collector_preserves_real_paths() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = project(root.path(), "")?;
    let module = module(
        root.path(),
        "from rust::provider::prelude import Component as ForeignComponent\n@derive(ForeignComponent)\nmodel Velocity:\n    x: int\n",
    )?;
    let demands = capture_checked_native_demands(
        &project,
        &[module],
        &ProjectRequirements::default(),
        &providers,
        &BTreeSet::new(),
    )?;
    assert_eq!(
        demands
            .observed
            .as_ref()
            .ok_or("facts missing")?
            .rust_derive_probe_paths,
        ["provider::prelude::Component".to_string()].into()
    );
    assert!(demands.require_support_only().is_err());
    assert!(demands.require_ordinary_source_inspection().is_err());
    Ok(())
}

/// Actual manifest vocabulary and C sections remain demand even for otherwise supported numeric source.
#[test]
fn ordinary_native_demands_manifest_vocabulary_and_c_are_not_empty() -> TestResult {
    let root = tempfile::tempdir()?;
    for extra in [
        "[vocab]\ncrate='native_vocab'\n",
        "[interop.c]\nschema=1\n[[interop.c.targets]]\ntarget='aarch64-apple-darwin'\n",
    ] {
        let (project, providers) = project(root.path(), extra)?;
        let module = module(root.path(), SCALAR)?;
        let demands = capture_checked_native_demands(
            &project,
            &[module],
            &ProjectRequirements::default(),
            &providers,
            &BTreeSet::new(),
        )?;
        assert!(demands.require_support_only().is_err());
        assert!(demands.require_source_inspection().is_err());
        assert!(demands.require_ordinary_source_inspection().is_err());
        let facts = demands.observed.ok_or("facts missing")?;
        assert!(
            matches!(facts.vocab_manifest, ManifestDemand::Declared(_))
                || matches!(facts.c_manifest, ManifestDemand::Declared(_))
        );
    }
    Ok(())
}

/// Foreign imports, providers, calls, generic declarations and default calls refuse in the parsed early gate.
#[test]
fn ordinary_native_demands_unsupported_parsed_shapes_refuse_before_inspection() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = project(root.path(), "")?;
    for source in [
        "import std.prelude\npub def answer() -> int:\n    return 42\n",
        "import pub::other\npub def answer() -> int:\n    return 42\n",
        "pub def answer(value: int) -> int:\n    return abs(value)\n",
        "pub def answer[T](value: T) -> T:\n    return value\n",
        "pub def answer(value: foreign.Type) -> int:\n    return 42\n",
        "pub def answer(value: List[int]) -> int:\n    return 42\n",
        "pub def answer(value: int = abs(-1)) -> int:\n    return value\n",
        "model Holder:\n    value: int\n",
    ] {
        let module = module(root.path(), source)?;
        let demands = capture_checked_native_demands(
            &project,
            &[module],
            &ProjectRequirements::default(),
            &providers,
            &BTreeSet::new(),
        )?;
        assert!(demands.require_support_only().is_err(), "{source}");
        assert!(demands.require_source_inspection().is_err(), "{source}");
        assert!(
            !demands
                .observed
                .as_ref()
                .ok_or("facts missing")?
                .unsupported_source
                .is_empty()
        );
    }
    Ok(())
}

/// Source requirement aliases and facets cannot be erased because their concrete ABI query set is empty.
#[test]
fn ordinary_native_demands_source_dependencies_and_facets_refuse() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = project(root.path(), "")?;
    let module = module(root.path(), SCALAR)?;
    let mut requirements = ProjectRequirements::default();
    requirements.dependencies.push(DependencySpec {
        crate_name: "foreign".to_string(),
        package: None,
        version: Some("1".to_string()),
        features: Vec::new(),
        default_features: true,
        optional: false,
        source: DependencySource::Registry,
    });
    let demands = capture_checked_native_demands(
        &project,
        std::slice::from_ref(&module),
        &requirements,
        &providers,
        &BTreeSet::new(),
    )?;
    assert!(demands.require_support_only().is_err());
    assert!(demands.require_source_inspection().is_err());
    requirements.dependencies.clear();
    requirements.stdlib_facets.push("incan_std_data".to_string());
    assert!(
        capture_checked_native_demands(&project, &[module], &requirements, &providers, &BTreeSet::new())?
            .require_support_only()
            .is_err()
    );
    Ok(())
}

/// Explicit sysroot aliases and calls carry exact query coverage; their original strict support gate still refuses.
#[test]
fn ordinary_native_sysroot_demands_bind_every_imported_item() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = project(root.path(), "")?;
    let module = module(
        root.path(),
        "from rust::std::thread import panicking as active\npub def answer() -> bool:\n    return not active()\n",
    )?;
    let queries =
        crate::build::library_exports::collect_library_rust_abi_query_paths(std::slice::from_ref(&module), &[])
            .into_iter()
            .collect();
    let demands = capture_checked_native_demands(
        &project,
        std::slice::from_ref(&module),
        &ProjectRequirements::default(),
        &providers,
        &queries,
    )?;
    assert!(demands.require_support_only().is_err());
    demands.require_source_inspection()?;
    for _ in 0..2 {
        let decoded: CheckedNativeDemands = serde_json::from_slice(&serde_json::to_vec(&demands)?)?;
        decoded.require_source_inspection()?;
    }
    for replacement in [
        BTreeSet::new(),
        [
            "std::thread::panicking".to_string(),
            "std::thread::yield_now".to_string(),
        ]
        .into(),
        ["foreign::panicking".to_string()].into(),
    ] {
        let incomplete = capture_checked_native_demands(
            &project,
            std::slice::from_ref(&module),
            &ProjectRequirements::default(),
            &providers,
            &replacement,
        )?;
        assert!(incomplete.require_source_inspection().is_err());
    }
    let mut old = serde_json::to_value(&demands)?;
    let facts = old["observed"].as_object_mut().ok_or("facts missing")?;
    facts.remove("scalar_native_imports");
    facts.remove("declared_native_crates");
    let legacy: CheckedNativeDemands = serde_json::from_value(old.clone())?;
    legacy.require_source_inspection()?;
    old["observed"]
        .as_object_mut()
        .ok_or("facts missing")?
        .remove("scalar_sysroot_imports");
    let unknown: CheckedNativeDemands = serde_json::from_value(old)?;
    assert!(unknown.require_source_inspection().is_err());
    Ok(())
}

/// A real authored Rust facet must be declared before scalar source imports gain inspection coverage.
fn declared_project(root: &std::path::Path) -> Result<(ProjectManifest, ProviderPlan), Box<dyn std::error::Error>> {
    let leaf = root.join("probe_leaf");
    fs::create_dir_all(leaf.join("src"))?;
    fs::write(leaf.join("src/lib.rs"), "pub fn ready() -> bool { false }\n")?;
    fs::write(
        leaf.join("loaf.toml"),
        "[project]\nname='probe_leaf'\nversion='1.0.0'\n[rust]\nname='probe_leaf'\ntype='lib'\nedition='2024'\n",
    )?;
    project(
        root,
        "[dependencies]\nprobe_leaf={loaf='probe_leaf',path='probe_leaf'}\n",
    )
}

/// Actual manifest/AST collection covers mixed sysroot and declared aliases without widening support-only authority.
#[test]
fn ordinary_native_declared_rust_demands_bind_manifest_imports_and_roundtrip() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = declared_project(root.path())?;
    assert!(project.rust_dependencies().contains_key("probe_leaf"));
    let module = module(
        root.path(),
        "from rust::probe_leaf import ready as active\nfrom rust::std::thread import panicking\npub def answer() -> bool:\n    return active() or panicking()\n",
    )?;
    let queries =
        crate::build::library_exports::collect_library_rust_abi_query_paths(std::slice::from_ref(&module), &[])
            .into_iter()
            .collect();
    let demands = capture_checked_native_demands(
        &project,
        &[module],
        &ProjectRequirements::default(),
        &providers,
        &queries,
    )?;
    demands.require_source_inspection()?;
    assert!(demands.require_support_only().is_err());
    let declared = ["probe_leaf".to_string()].into();
    demands.require_declared_crates(&declared)?;
    for _ in 0..2 {
        let decoded: CheckedNativeDemands = serde_json::from_slice(&serde_json::to_vec(&demands)?)?;
        decoded.require_source_inspection()?;
        decoded.require_declared_crates(&declared)?;
    }
    assert!(demands.require_declared_crates(&BTreeSet::new()).is_err());
    assert!(demands.require_declared_crates(&["other".to_string()].into()).is_err());
    for field in [
        "scalar_native_imports",
        "declared_native_crates",
        "scalar_sysroot_imports",
    ] {
        let mut missing = serde_json::to_value(&demands)?;
        missing["observed"]
            .as_object_mut()
            .ok_or("facts missing")?
            .remove(field);
        let decoded: CheckedNativeDemands = serde_json::from_value(missing)?;
        assert!(decoded.require_source_inspection().is_err(), "{field}");
    }
    let mut legacy = serde_json::to_value(&demands)?;
    let facts = legacy["observed"].as_object_mut().ok_or("facts missing")?;
    facts.remove("scalar_native_imports");
    facts.remove("declared_native_crates");
    let decoded: CheckedNativeDemands = serde_json::from_value(legacy)?;
    assert!(decoded.require_source_inspection().is_err());
    assert!(decoded.require_declared_crates(&declared).is_err());
    Ok(())
}

/// Inline source overrides, ambiguous aliases and undeclared roots cannot borrow a manifest dependency grant.
#[test]
fn ordinary_native_declared_rust_demands_refuse_uncovered_imports() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = declared_project(root.path())?;
    for source in [
        "from rust::other import ready\npub def answer() -> bool:\n    return ready()\n",
        "import rust::probe_leaf\npub def answer() -> bool:\n    return False\n",
        "from rust::probe_leaf @ \"1\" import ready\npub def answer() -> bool:\n    return ready()\n",
        "from rust::probe_leaf with [\"feature\"] import ready\npub def answer() -> bool:\n    return ready()\n",
        "from rust::probe_leaf import ready as active\nfrom rust::std::thread import panicking as active\npub def answer() -> bool:\n    return active()\n",
        "from rust::probe_leaf import ready\npub def answer(ready: bool) -> bool:\n    return ready()\n",
    ] {
        let module = module(root.path(), source)?;
        let queries =
            crate::build::library_exports::collect_library_rust_abi_query_paths(std::slice::from_ref(&module), &[])
                .into_iter()
                .collect();
        let demands = capture_checked_native_demands(
            &project,
            &[module],
            &ProjectRequirements::default(),
            &providers,
            &queries,
        )?;
        assert!(demands.require_source_inspection().is_err(), "{source}");
    }
    let module = module(
        root.path(),
        "from rust::probe_leaf import ready\npub def answer() -> bool:\n    return ready()\n",
    )?;
    for queries in [
        BTreeSet::new(),
        ["probe_leaf::missing".to_string()].into(),
        ["probe_leaf::ready".to_string(), "probe_leaf::extra".to_string()].into(),
    ] {
        let demands = capture_checked_native_demands(
            &project,
            std::slice::from_ref(&module),
            &ProjectRequirements::default(),
            &providers,
            &queries,
        )?;
        assert!(demands.require_source_inspection().is_err());
    }
    Ok(())
}

/// Unbound calls, dynamic/generic dispatch and foreign or whole-module imports cannot borrow a sysroot grant.
#[test]
fn ordinary_native_sysroot_demands_refuse_uncovered_calls_and_imports() -> TestResult {
    let root = tempfile::tempdir()?;
    let (project, providers) = project(root.path(), "")?;
    for source in [
        "from rust::std::thread import panicking\npub def answer() -> bool:\n    return unknown()\n",
        "from rust::std::thread import panicking\npub def answer() -> bool:\n    return panicking[bool]()\n",
        "from rust::std::thread import panicking\npub def answer(panicking: bool) -> bool:\n    return panicking()\n",
        "import rust::std::thread\npub def answer() -> bool:\n    return False\n",
        "from rust::foreign import panicking\npub def answer() -> bool:\n    return panicking()\n",
    ] {
        let module = module(root.path(), source)?;
        let queries =
            crate::build::library_exports::collect_library_rust_abi_query_paths(std::slice::from_ref(&module), &[])
                .into_iter()
                .collect();
        let demands = capture_checked_native_demands(
            &project,
            &[module],
            &ProjectRequirements::default(),
            &providers,
            &queries,
        )?;
        assert!(demands.require_source_inspection().is_err(), "{source}");
    }
    Ok(())
}
