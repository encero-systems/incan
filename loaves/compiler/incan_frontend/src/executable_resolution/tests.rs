//! Executable public closures keep the selected SDK artifact boundary and its admission checks.

use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::executable_representation::{EXECUTABLE_REPRESENTATION_VERSION, build_surface};
use sha2::{Digest, Sha256};

use super::{ExecutableResolutionError, resolve_executable_requirements};
use crate::body_ir::build_body_ir_module_v0;
use crate::library_exports::collect_checked_public_exports;
use crate::library_manifest::published_layout::{executable_surface_path, public_executable_identities};
use crate::library_manifest::{ExecutableRepresentationExport, LibraryManifest, digest_provider_artifact};
use crate::library_manifest_index::{LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::provider::{NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord};
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};

type TestResult = Result<(), Box<dyn Error>>;

/// One public root calls an internal body that is not importable through the finalized manifest.
const PRIVATE_SOURCE: &str = "def helper(value: int) -> int:\n    return value + 1\n\ndef unused() -> int:\n    return 99\n\npub def run(value: int) -> int:\n    return helper(value)\n";

/// Select a real checked private identity and publish only the declaring compilation's public manifest.
fn private_package(root: &Path) -> Result<(ProviderPlan, CanonicalSymbolId, CanonicalSymbolId), Box<dyn Error>> {
    let manifest = artifact(root, "helpers", PRIVATE_SOURCE, ProviderPlan::default())?;
    let public = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("run")
        .ok_or("public root missing")?;
    let tokens = lexer::lex(PRIVATE_SOURCE).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["lib".to_string()];
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("helpers".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let private = module
        .bodies
        .iter()
        .find(|body| body.name == "helper")
        .and_then(|body| body.canonical.clone())
        .ok_or("checked private identity missing")?;
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "helpers".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "helpers",
                "helpers",
                root.join("helpers.incnlib"),
                root.to_path_buf(),
            ),
            manifest: Box::new(manifest),
        },
    )]));
    let plan = ProviderPlan::from_resolved_inputs(index, None, None, None, [])?;
    Ok((plan, public, private))
}

/// Required private implementation is loaded from selected artifact bytes after the provider's source disappears.
#[test]
fn public_root_loads_private_implementation_without_source() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("package");
    let (plan, public, private) = private_package(&root)?;
    fs::remove_dir_all(root.join("src"))?;
    let resolved = resolve_executable_requirements(&plan, &BTreeSet::from([public]))?;
    assert!(resolved.modules.iter().any(|module| {
        module
            .bodies
            .iter()
            .any(|body| body.canonical.as_ref() == Some(&private))
    }));
    assert!(
        !resolved
            .modules
            .iter()
            .any(|module| module.bodies.iter().any(|body| body.name == "unused"))
    );
    assert_eq!(resolved.decoded_declarations, 2);
    Ok(())
}

/// Explicit private roots never gain authority from a public root in the same request or traversal order.
#[test]
fn explicit_private_roots_refuse_with_or_without_public_roots() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, public, private) = private_package(temporary.path())?;
    for roots in [BTreeSet::from([private.clone()]), BTreeSet::from([public, private])] {
        let result = resolve_executable_requirements(&plan, &roots);
        assert!(matches!(result, Err(ExecutableResolutionError::UnusableRepresentation {
            source: incan_semantics_core::executable_representation::ExecutableRepresentationError::DeclarationNotCovered { declaration }, ..
        }) if declaration == "helper"));
    }
    Ok(())
}

/// A checked private constructor field requires its public layout, without becoming an executable import root.
#[test]
fn private_constructor_fields_resolve_through_the_checked_public_layout() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("sealed");
    let manifest = artifact(
        &root,
        "sealed",
        "pub class Vault:\n    secret: int\n    pub label: str = \"sealed\"\n\n    def reveal(self) -> str:\n        return f\"{self.label}:{self.secret}\"\n",
        ProviderPlan::default(),
    )?;
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "sealed".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "sealed",
                "sealed",
                root.join("sealed.incnlib"),
                root.clone(),
            ),
            manifest: Box::new(manifest),
        },
    )]));
    let plan = ProviderPlan::from_resolved_inputs(index, None, None, None, [])?;
    let source = "from pub::sealed import Vault as PublicVault\n\ndef main() -> None:\n    println(PublicVault(secret=42).reveal())\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["main".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_provider_plan(Arc::new(plan.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let info = checker.type_info();
    let facts = info.semantic_fact_store(&module_path);
    let (field_span, field_reference) = facts
        .checked_reference_sites()
        .find(|(_, reference)| {
            reference.target.kind == incan_semantics_core::SemanticSourceTargetKind::Field
                && reference.target.declaration_name == "secret"
        })
        .ok_or("checked constructor field reference missing")?;
    assert_eq!(field_reference.requirement.declaration_name, "Vault");
    let field_identity = field_reference.target.clone();
    let mut replaced = info.clone();
    let other = info
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "main")
        .ok_or("checked main identity missing")?;
    replaced
        .references
        .resolved_identities
        .insert((field_span.start, field_span.end), other.clone());
    let replaced_facts = replaced.semantic_fact_store(&module_path);
    let replaced_reference = replaced_facts
        .checked_reference(&incan_semantics_core::CompilerNodeId::expression_span(
            "main",
            field_span.start,
            field_span.end,
        ))
        .ok_or("replaced checked reference missing")?;
    assert_eq!(replaced_reference.requirement, other);
    let required = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores([&facts])
        .reachable_from(info.declarations.declaration_identities.values().cloned())
        .into_iter()
        .filter(|identity| matches!(identity.origin, incan_semantics_core::SymbolOrigin::Package { .. }))
        .collect();
    fs::remove_dir_all(root.join("src"))?;
    let resolved = resolve_executable_requirements(&plan, &required)?;
    let vault = resolved
        .modules
        .iter()
        .flat_map(|module| &module.nominal_declarations)
        .find(|declaration| declaration.name == "Vault")
        .ok_or("checked public Vault layout missing")?;
    assert_eq!(vault.fields, ["secret", "label"]);
    assert_eq!(vault.field_public, [false, true]);
    assert!(matches!(
        resolve_executable_requirements(&plan, &BTreeSet::from([field_identity])),
        Err(ExecutableResolutionError::UnusableRepresentation { .. })
    ));
    Ok(())
}

/// Publish checked source through the real executable codec without claiming a native library build.
fn artifact(root: &Path, name: &str, source: &str, plan: ProviderPlan) -> Result<LibraryManifest, Box<dyn Error>> {
    artifact_with_uncovered(root, name, source, plan, &[])
}

/// Publish a real checked surface with selected source declarations explicitly excluded from executable coverage.
fn artifact_with_uncovered(
    root: &Path,
    name: &str,
    source: &str,
    plan: ProviderPlan,
    excluded: &[&str],
) -> Result<LibraryManifest, Box<dyn Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_current_package_identity(Some(name.to_string()));
    checker.set_provider_plan(Arc::new(plan));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut manifest = LibraryManifest::from_checked_exports(name, "1.2.3", &exports);
    let first = crate::body_ir::build_body_ir_module_v0_with_checked_sources(
        &program,
        &module_path,
        checker.type_info(),
        &[],
        &checker.stdlib_cache,
    )?;
    let module = crate::body_ir::build_body_ir_module_v0_with_checked_sources(
        &program,
        &module_path,
        checker.type_info(),
        std::slice::from_ref(&first),
        &checker.stdlib_cache,
    )?;
    let unrepresentable = module
        .bodies
        .iter()
        .filter(|body| body.first_representation_gap().is_some() || excluded.contains(&body.name.as_str()))
        .filter_map(|body| body.canonical.clone())
        .collect();
    let bytes = build_surface(
        &[module],
        name,
        "1.2.3",
        &public_executable_identities(&manifest),
        &unrepresentable,
    )?;
    manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
        representation_version: EXECUTABLE_REPRESENTATION_VERSION,
        content_digest: hex::encode(Sha256::digest(&bytes)),
    });
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("src/lib.rs"), "// Semantic artifact fixture only.\n")?;
    fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = '{name}'\nversion = '1.2.3'\n"),
    )?;
    let manifest_path = root.join(format!("{name}.incnlib"));
    let surface = executable_surface_path(&manifest_path, &manifest).ok_or("surface path missing")?;
    fs::create_dir_all(surface.parent().ok_or("surface directory missing")?)?;
    fs::write(surface, bytes)?;
    manifest.write_to_path(&manifest_path)?;
    Ok(manifest)
}

/// Retain an independently selected SDK artifact, with reserved authority and a physical manifest.
fn sdk_record(root: &Path, manifest: &LibraryManifest) -> Result<ProviderRecord, Box<dyn Error>> {
    Ok(ProviderRecord {
        identity: ProviderIdentity {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            digest: digest_provider_artifact(root)?,
            feature_projection: BTreeSet::new(),
        },
        provenance: ProviderProvenance::Sdk {
            sdk_identity: "test-sdk".into(),
            component_id: "fixture".into(),
            inventory_path: None,
        },
        authority: NamespaceAuthority::SdkReserved,
        namespace_claims: BTreeSet::from([vec!["std".into(), "fixture".into()]]),
        available: true,
        enabled: true,
        manifest: Some(Arc::new(manifest.clone())),
        artifact: Some(LibraryArtifactMetadata::from_manifest_path(
            &manifest.name,
            &manifest.name,
            root.join(format!("{}.incnlib", manifest.name)),
            root.to_path_buf(),
        )),
        implementation_facets: Vec::new(),
    })
}

/// Resolve an ordinary declaration whose only transitive type context belongs to the selected SDK.
fn package_with_sdk(root: &Path) -> Result<(ProviderPlan, CanonicalSymbolId, ProviderRecord), Box<dyn Error>> {
    let sdk_root = root.join("sdk");
    let sdk = artifact(
        &sdk_root,
        "sdk_fixture",
        "pub model Payload:\n    pub value: int\n",
        ProviderPlan::default(),
    )?;
    let sdk_record = sdk_record(&sdk_root, &sdk)?;
    let producer_index = LibraryManifestIndex::from_entries(HashMap::from([(
        "fixture".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "fixture",
                &sdk.name,
                sdk_root.join("sdk_fixture.incnlib"),
                sdk_root,
            ),
            manifest: Box::new(sdk),
        },
    )]));
    let producer_plan = ProviderPlan::from_resolved_inputs(producer_index, None, None, None, [])?;
    let package_root = root.join("package");
    let package = artifact(
        &package_root,
        "reader",
        "from pub::fixture import Payload\n\npub def read(value: Payload) -> int:\n    return value.value\n",
        producer_plan,
    )?;
    let identity = package
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("read")
        .ok_or("read identity missing")?;
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "reader".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "reader",
                "reader",
                package_root.join("reader.incnlib"),
                package_root,
            ),
            manifest: Box::new(package),
        },
    )]));
    let ordinary = ProviderPlan::from_resolved_inputs(index.clone(), None, None, None, [])?;
    let mut records = ordinary.records().cloned().collect::<Vec<_>>();
    records.push(sdk_record.clone());
    Ok((ProviderPlan::new(index, records, [])?, identity, sdk_record))
}

/// An ordinary public body loads its SDK type context selectively from the active artifact.
#[test]
fn package_closure_reaches_the_selected_sdk_artifact() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, identity, _) = package_with_sdk(temporary.path())?;
    let resolved = resolve_executable_requirements(&plan, &BTreeSet::from([identity]))?;
    assert!(
        resolved
            .modules
            .iter()
            .any(|module| module.bodies.iter().any(|body| body.name == "read"))
    );
    assert!(resolved.modules.iter().any(|module| {
        module
            .nominal_declarations
            .iter()
            .any(|declaration| declaration.name == "Payload")
    }));
    assert_eq!(
        resolved.package_versions.get("sdk_fixture").map(String::as_str),
        Some("1.2.3")
    );
    Ok(())
}

/// A disabled provider cannot satisfy a public closure just because its bytes are installed.
#[test]
fn package_closure_rejects_a_disabled_sdk_artifact() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, identity, _) = package_with_sdk(temporary.path())?;
    let records = plan
        .records()
        .cloned()
        .map(|mut record| {
            if matches!(record.authority, NamespaceAuthority::SdkReserved) {
                record.enabled = false;
            }
            record
        })
        .collect::<Vec<_>>();
    let disabled = ProviderPlan::new(plan.library_manifest_index().clone(), records, [])?;
    let result = resolve_executable_requirements(&disabled, &BTreeSet::from([identity]));
    assert!(matches!(result, Err(ExecutableResolutionError::UnknownPackage { library }) if library == "sdk_fixture"));
    Ok(())
}

/// SDK context retains the executable descriptor's digest check when reached through an ordinary package.
#[test]
fn package_closure_rejects_altered_sdk_surface_bytes() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, identity, sdk) = package_with_sdk(temporary.path())?;
    let manifest = sdk.manifest.as_ref().ok_or("SDK manifest missing")?;
    let metadata = sdk.artifact.as_ref().ok_or("SDK artifact missing")?;
    let surface = executable_surface_path(&metadata.manifest_path, manifest).ok_or("SDK surface missing")?;
    let mut bytes = fs::read(&surface)?;
    let last = bytes.last_mut().ok_or("SDK surface empty")?;
    *last ^= 1;
    fs::write(surface, bytes)?;
    let result = resolve_executable_requirements(&plan, &BTreeSet::from([identity]));
    assert!(
        matches!(result, Err(ExecutableResolutionError::UnusableRepresentation { library, .. }) if library == "sdk_fixture")
    );
    Ok(())
}

/// Canonical package names cannot select between different active SDK projections.
#[test]
fn package_closure_rejects_conflicting_sdk_instances() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, identity, mut sdk) = package_with_sdk(temporary.path())?;
    sdk.namespace_claims.clear();
    sdk.identity.feature_projection.insert("different".into());
    let mut records = plan.records().cloned().collect::<Vec<_>>();
    records.push(sdk);
    let conflicting = ProviderPlan::new(plan.library_manifest_index().clone(), records, [])?;
    let result = resolve_executable_requirements(&conflicting, &BTreeSet::from([identity]));
    assert!(
        matches!(result, Err(ExecutableResolutionError::AmbiguousPackage { library, .. }) if library == "sdk_fixture")
    );
    Ok(())
}

/// An ordinary package with the SDK's name cannot replace its selected artifact by map insertion.
#[test]
fn package_closure_rejects_conflicting_package_and_sdk_instances() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (_, _, sdk) = package_with_sdk(temporary.path())?;
    let root = temporary.path().join("conflicting-package");
    let manifest = artifact(
        &root,
        "sdk_fixture",
        "pub model Payload:\n    pub value: int\n    pub other: int\n",
        ProviderPlan::default(),
    )?;
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("Payload")
        .ok_or("Payload identity missing")?;
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "different".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "different",
                &manifest.name,
                root.join("sdk_fixture.incnlib"),
                root,
            ),
            manifest: Box::new(manifest),
        },
    )]));
    let ordinary = ProviderPlan::from_resolved_inputs(index.clone(), None, None, None, [])?;
    let mut records = ordinary.records().cloned().collect::<Vec<_>>();
    records.push(sdk);
    let plan = ProviderPlan::new(index, records, [])?;
    let result = resolve_executable_requirements(&plan, &BTreeSet::from([identity]));
    assert!(
        matches!(result, Err(ExecutableResolutionError::AmbiguousPackage { library, .. }) if library == "sdk_fixture")
    );
    Ok(())
}
/// A selected artifact retains private replacement initialization after its source disappears, in source order.
#[test]
fn public_entry_loads_decorated_initialization_without_source() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("package");
    let manifest = artifact(
        &root,
        "decorators",
        "def keep(func: () -> int) -> () -> int:\n    println(func())\n    return func\n\n@keep\ndef zebra() -> int:\n    return 7\n\n@keep\ndef alpha() -> int:\n    return 3\n\npub def plain() -> int:\n    return 5\n",
        ProviderPlan::default(),
    )?;
    let public = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("plain")
        .ok_or("public root missing")?;
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "decorators".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "decorators",
                "decorators",
                root.join("decorators.incnlib"),
                root.clone(),
            ),
            manifest: Box::new(manifest),
        },
    )]));
    let plan = ProviderPlan::from_resolved_inputs(index, None, None, None, [])?;
    fs::remove_dir_all(root.join("src"))?;
    let resolved = resolve_executable_requirements(&plan, &BTreeSet::from([public]))?;
    let module = resolved.modules.first().ok_or("declaring module missing")?;
    assert_eq!(
        module
            .static_declarations
            .iter()
            .map(|value| value.canonical.declaration_name.as_str())
            .collect::<Vec<_>>(),
        ["zebra", "alpha"]
    );
    assert_eq!(module.decorated_bindings.len(), 2);
    assert!(
        module
            .decorated_bindings
            .iter()
            .all(|binding| module.is_well_formed_decorated_binding(binding))
    );
    Ok(())
}

/// Linked numeric constants load their authenticated value and layout without requiring unused method bodies.
#[test]
fn linked_numeric_constant_loads_only_value_and_layout() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let source = "pub type Wide = newtype u128:\n    def bits(self) -> u128:\n        return self.0\n\npub const MAXIMUM: Wide = Wide(340282366920938463463374607431768211455)\n";
    let manifest = artifact(temporary.path(), "numeric", source, ProviderPlan::default())?;
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("MAXIMUM")
        .ok_or("constant identity missing")?;
    let sdk = sdk_record(temporary.path(), &manifest)?;
    let plan = ProviderPlan::new(LibraryManifestIndex::from_entries(HashMap::new()), vec![sdk], [])?;
    let roots = BTreeSet::from([identity.clone()]);
    let resolved = super::resolve_sdk_requirements(&plan, &roots)?;
    assert_eq!(resolved.decoded_declarations, 2);
    assert_eq!(
        resolved.modules.iter().map(|module| module.bodies.len()).sum::<usize>(),
        0
    );
    let constant = resolved
        .modules
        .iter()
        .flat_map(|module| &module.constant_declarations)
        .next()
        .ok_or("constant value missing")?;
    let incan_semantics_core::body_ir::Constant::NumericNewtype(value) = &constant.value else {
        return Err("numeric newtype value missing".into());
    };
    let layout = resolved
        .modules
        .iter()
        .flat_map(|module| &module.nominal_declarations)
        .next()
        .ok_or("constant layout missing")?;
    assert!(value.matches_layout(layout));
    let mut wrong_identity = identity;
    wrong_identity.declaration_span.end += 1;
    assert!(super::resolve_sdk_requirements(&plan, &BTreeSet::from([wrong_identity])).is_err());
    let surface =
        executable_surface_path(&temporary.path().join("numeric.incnlib"), &manifest).ok_or("surface path missing")?;
    let mut bytes = fs::read(&surface)?;
    let last = bytes.last_mut().ok_or("surface empty")?;
    *last ^= 1;
    fs::write(surface, bytes)?;
    assert!(super::resolve_sdk_requirements(&plan, &roots).is_err());
    Ok(())
}

/// Ordinary admitted consumers retain dependency values and defaults from decoded publication after producer source
/// disappears. No ambient standard-library cache may substitute a value at the lowering handoff.
#[test]
fn checked_lowering_retains_selected_constant_and_default_records_without_source() -> TestResult {
    use incan_semantics_core::body_ir::{CallableParamDefault, Constant, Operand, StatementKind};
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("numbers");
    let manifest = artifact(
        &root,
        "numbers",
        "pub const VALUE: float = 3.25\npub def adjusted(offset: float = VALUE) -> float:\n    return offset\n",
        ProviderPlan::default(),
    )?;
    let roots = ["VALUE", "adjusted"]
        .into_iter()
        .map(|name| {
            manifest
                .contract_metadata
                .identity_graph
                .canonical_for_public_name(name)
                .ok_or_else(|| format!("missing published {name}"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "numbers".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "numbers",
                "numbers",
                root.join("numbers.incnlib"),
                root.clone(),
            ),
            manifest: Box::new(manifest),
        },
    )]));
    let plan = Arc::new(ProviderPlan::from_admitted_libraries(index, &[], [])?);
    let tokens = lexer::lex(
        "from pub::numbers import VALUE, adjusted\npub def value() -> float:\n    return VALUE\npub def defaulted() -> float:\n    return adjusted()\n",
    ).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["consumer".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_provider_plan(Arc::clone(&plan));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    fs::remove_dir_all(root.join("src"))?;
    let context = resolve_executable_requirements(&plan, &roots)?;
    let module = crate::body_ir::build_body_ir_module_v0_with_checked_sources(
        &program,
        &module_path,
        checker.type_info(),
        &context.modules,
        &checker.stdlib_cache,
    )?;
    assert!(
        module
            .bodies
            .iter()
            .all(|body| body.first_representation_gap().is_none())
    );
    let value = module
        .bodies
        .iter()
        .find(|body| body.name == "value")
        .ok_or("missing consumer")?;
    assert!(value.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        StatementKind::Return { value: Some(Operand::Constant(Constant::Float(value))) } if value == "3.25"
    )));
    let adjusted = context
        .modules
        .iter()
        .flat_map(|module| &module.bodies)
        .find(|body| body.name == "adjusted")
        .ok_or("missing selected callable")?;
    assert!(matches!(
        adjusted.params.first().map(|parameter| &parameter.default),
        Some(CallableParamDefault::Source(computation))
            if matches!(&computation.result, Operand::Constant(Constant::Float(value)) if value == "3.25")
    ));
    Ok(())
}

/// Admit a generated test artifact through the same selected package index as ordinary publication.
fn context_test_plan(root: &Path, manifest: &LibraryManifest) -> Result<ProviderPlan, Box<dyn Error>> {
    let name = &manifest.name;
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        name.clone(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                name,
                name,
                root.join(format!("{name}.incnlib")),
                root.to_path_buf(),
            ),
            manifest: Box::new(manifest.clone()),
        },
    )]));
    Ok(ProviderPlan::from_resolved_inputs(index, None, None, None, [])?)
}

/// Keep deliberately uncovered public roots visible in authenticated artifacts without inventing executable facts.
fn uncovered_context_fixture(
    root: &Path,
) -> Result<(ProviderPlan, LibraryManifest, CanonicalSymbolId), Box<dyn Error>> {
    let manifest = artifact_with_uncovered(
        root,
        "context",
        "pub def missing() -> int:\n    return 1\n",
        ProviderPlan::default(),
        &["missing"],
    )?;
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("missing")
        .ok_or("missing fixture identity")?;
    let plan = context_test_plan(root, &manifest)?;
    Ok((plan, manifest, identity))
}

/// An authenticated, explicitly uncovered context root contributes no decoded facts or executable permission.
#[test]
fn optional_context_omits_authenticated_uncovered_root() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, _, identity) = uncovered_context_fixture(temporary.path())?;
    let resolved = super::resolve_available_executable_context(&plan, &BTreeSet::from([identity.clone()]))?;
    assert!(resolved.modules.is_empty());
    assert_eq!(resolved.decoded_declarations, 0);
    assert!(resolved.content_bytes_verified > 0);
    let mut forged = identity;
    forged.declaration_span.end += 1;
    assert!(super::resolve_available_executable_context(&plan, &BTreeSet::from([forged])).is_err());
    Ok(())
}

/// The execution entrypoint continues to demand coverage for the same root omitted from optional context.
#[test]
fn strict_execution_still_refuses_uncovered_context_root() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, _, identity) = uncovered_context_fixture(temporary.path())?;
    assert!(
        matches!(resolve_executable_requirements(&plan, &BTreeSet::from([identity])), Err(ExecutableResolutionError::UnusableRepresentation {
        source: incan_semantics_core::executable_representation::ExecutableRepresentationError::DeclarationNotCovered { declaration }, ..
    }) if declaration == "missing")
    );
    Ok(())
}

/// Optional context never suppresses malformed content or its selected manifest digest check.
#[test]
fn optional_context_refuses_changed_artifact_bytes() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, manifest, identity) = uncovered_context_fixture(temporary.path())?;
    let surface =
        executable_surface_path(&temporary.path().join("context.incnlib"), &manifest).ok_or("missing surface path")?;
    let mut bytes = fs::read(&surface)?;
    let final_byte = bytes.last_mut().ok_or("empty executable surface")?;
    *final_byte ^= 1;
    fs::write(surface, bytes)?;
    assert!(super::resolve_available_executable_context(&plan, &BTreeSet::from([identity])).is_err());
    Ok(())
}

/// An uncovered requested root cannot be treated as visited when a covered public root requires it later.
#[test]
fn optional_context_keeps_covered_dependency_coverage_mandatory() -> TestResult {
    use incan_semantics_core::executable_representation::{DeclarationCoverage, SurfaceReader};
    let temporary = tempfile::tempdir()?;
    let dependency_root = temporary.path().join("dependency");
    let (dependency_plan, dependency, child) = uncovered_context_fixture(&dependency_root)?;
    let parent_root = temporary.path().join("parent");
    let parent = artifact(
        &parent_root,
        "parent",
        "from pub::context import missing\n\npub def run() -> int:\n    return missing()\n",
        dependency_plan,
    )?;
    let selected = parent
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("run")
        .ok_or("parent root missing")?;
    let surface =
        executable_surface_path(&parent_root.join("parent.incnlib"), &parent).ok_or("parent surface missing")?;
    let bytes = fs::read(surface)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        matches!(reader.index().coverage(&selected)?, DeclarationCoverage::Covered { requirements, .. } if requirements.contains(&child))
    );
    let mut entries = HashMap::new();
    for (root, manifest) in [(&dependency_root, &dependency), (&parent_root, &parent)] {
        let name = &manifest.name;
        entries.insert(
            name.clone(),
            LibraryManifestIndexEntry::Loaded {
                metadata: LibraryArtifactMetadata::from_manifest_path(
                    name,
                    name,
                    root.join(format!("{name}.incnlib")),
                    root.clone(),
                ),
                manifest: Box::new(manifest.clone()),
            },
        );
    }
    let plan = ProviderPlan::from_resolved_inputs(LibraryManifestIndex::from_entries(entries), None, None, None, [])?;
    // `context` precedes `parent`, so the optional child is encountered before the mandatory dependency edge.
    for roots in [BTreeSet::from([selected.clone()]), BTreeSet::from([child, selected])] {
        assert!(
            matches!(super::resolve_available_executable_context(&plan, &roots), Err(ExecutableResolutionError::UnusableRepresentation {
            source: incan_semantics_core::executable_representation::ExecutableRepresentationError::DeclarationNotCovered { declaration }, ..
        }) if declaration == "missing")
        );
    }
    Ok(())
}
