//! Checked producer facts exercise resolver contracts without pretending to be native archive acceptance.

use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::ExecutableResolutionError;
use crate::backend::replacement::{ReplacementExecutionGraph, execute_free_function};
use crate::frontend::body_ir::build_body_ir_module_v0;
use crate::frontend::library_exports::collect_checked_public_exports;
use crate::frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use crate::frontend::typechecker::TypeChecker;
use crate::frontend::{lexer, parser};
use crate::library_manifest::published_layout::{executable_surface_path, public_executable_identities};
use crate::library_manifest::{ExecutableRepresentationExport, LibraryManifest};
use incan_semantics_core::executable_representation::{EXECUTABLE_REPRESENTATION_VERSION, build_surface};

/// Admit fixtures through the production provider plan before requesting executable fragments.
fn resolve_executable_requirements(
    index: &LibraryManifestIndex,
    required: &BTreeSet<incan_semantics_core::CanonicalSymbolId>,
) -> Result<super::ResolvedExecutableModules, ExecutableResolutionError> {
    let plan =
        crate::provider::ProviderPlan::from_resolved_inputs(index.clone(), None, None, None, []).map_err(|error| {
            ExecutableResolutionError::DependencyArtifact {
                library: "fixture".into(),
                version: "1.2.3".into(),
                reason: error.to_string(),
            }
        })?;
    super::resolve_executable_requirements(&plan, required)
}

/// Produce a manifest and semantic surface from one checked source input, using the real identity exporter.
fn artifact(root: &Path, library: &str, source: &str) -> Result<LibraryManifest, Box<dyn Error>> {
    artifact_with_provider_plan(root, library, source, std::sync::Arc::new(Default::default()))
}

/// Check a producer against an already admitted dependency graph before projecting its public manifest.
fn artifact_with_provider_plan(
    root: &Path,
    library: &str,
    source: &str,
    provider_plan: std::sync::Arc<crate::provider::ProviderPlan>,
) -> Result<LibraryManifest, Box<dyn Error>> {
    let tokens = lexer::lex(source).map_err(|error| format!("{error:?}"))?;
    let program = parser::parse(&tokens).map_err(|error| format!("{error:?}"))?;
    let module_path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_current_package_identity(Some(library.to_string()));
    checker.set_provider_plan(provider_plan);
    checker.check_program(&program).map_err(|error| format!("{error:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut manifest = LibraryManifest::from_checked_exports(library, "1.2.3", &exports);
    let module = build_body_ir_module_v0(&program, &module_path, checker.type_info());
    let unrepresentable = module
        .bodies
        .iter()
        .filter(|body| crate::backend::replacement::validate_direct_body_profile(body).is_err())
        .filter_map(|body| body.canonical.clone())
        .collect();
    let bytes = build_surface(
        &[module],
        library,
        "1.2.3",
        &public_executable_identities(&manifest),
        &unrepresentable,
    )?;
    manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
        representation_version: EXECUTABLE_REPRESENTATION_VERSION,
        content_digest: hex::encode(Sha256::digest(&bytes)),
    });
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("src/lib.rs"),
        "// Artifact contract fixture; not native execution proof.\n",
    )?;
    fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = \"{library}\"\nversion = \"1.2.3\"\n"),
    )?;
    let manifest_path = root.join(format!("{library}.incnlib"));
    let path = executable_surface_path(&manifest_path, &manifest).ok_or("surface path missing")?;
    fs::create_dir_all(path.parent().ok_or("surface directory missing")?)?;
    fs::write(path, bytes)?;
    manifest.write_to_path(&manifest_path)?;
    Ok(manifest)
}

/// Consumer keys deliberately differ from package identities in every resolver fixture.
fn index(root: &Path, alias: &str, manifest: &LibraryManifest) -> LibraryManifestIndex {
    let metadata = LibraryArtifactMetadata::from_manifest_path(
        alias,
        &manifest.name,
        root.join(format!("{}.incnlib", manifest.name)),
        root.to_path_buf(),
    );
    LibraryManifestIndex::from_entries(HashMap::from([(
        alias.to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest.clone()),
            metadata,
        },
    )]))
}

/// The alias edge does not replace the identity minted by the checked producer.
#[test]
fn a_renamed_dependency_executes_its_canonical_public_body() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let manifest = artifact(
        temporary.path(),
        "arithmetic",
        "pub def answer() -> int:\n    return 42\n",
    )?;
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("answer")
        .ok_or("canonical export missing")?;
    let resolved = resolve_executable_requirements(
        &index(temporary.path(), "renamed", &manifest),
        &BTreeSet::from([identity]),
    )?;
    assert_eq!(resolved.decoded_declarations, 1);
    let module = resolved.modules.first().ok_or("execution module missing")?;
    assert_eq!(
        execute_free_function(module, "answer", &[])
            .map_err(|error| format!("{error}; {}", module.render_snapshot()))?
            .value
            .observable_text(),
        "42"
    );
    Ok(())
}

/// A source-written numeric tuple field is covered through checked structural type evidence.
#[test]
fn checked_tuple_field_publication_executes_without_a_nominal_member_identity() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let manifest = artifact(
        temporary.path(),
        "tuples",
        "pub def answer() -> int:\n    pair = (42, 0)\n    return pair.0\n",
    )?;
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("answer")
        .ok_or("tuple export identity missing")?;
    let resolved = resolve_executable_requirements(
        &index(temporary.path(), "renamed", &manifest),
        &BTreeSet::from([identity]),
    )?;
    let module = resolved.modules.first().ok_or("tuple body not published")?;
    assert_eq!(
        execute_free_function(module, "answer", &[])?.value.observable_text(),
        "42"
    );
    Ok(())
}

/// Same module names and spans in different packages cannot collide in a consumer's execution graph.
#[test]
fn two_packages_with_identical_module_paths_keep_distinct_physical_owners() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let alpha = temporary.path().join("alpha");
    let beta = temporary.path().join("beta");
    let first = artifact(
        &alpha,
        "first",
        "pub def answer() -> int:\n    return 41\n\npub model Pair:\n    pub value: int\n",
    )?;
    let second = artifact(
        &beta,
        "second",
        "pub def answer() -> int:\n    return 42\n\npub model Pair:\n    pub value: int\n",
    )?;
    let one = first
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("answer")
        .ok_or("first identity absent")?;
    let two = second
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("answer")
        .ok_or("second identity absent")?;
    let mut entries = HashMap::new();
    for (alias, root, manifest) in [("a", &alpha, first), ("b", &beta, second)] {
        let metadata = LibraryArtifactMetadata::from_manifest_path(
            alias,
            &manifest.name,
            root.join(format!("{}.incnlib", manifest.name)),
            root.to_path_buf(),
        );
        entries.insert(
            alias.into(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata,
            },
        );
    }
    let index = LibraryManifestIndex::from_entries(entries);
    let source = "from pub::a import answer as first, Pair as Left\nfrom pub::b import answer as second, Pair as Right\n\ndef carry(value: Left) -> Result[Left, str]:\n    return Ok(value)\n\ndef main() -> int:\n    left = Left(value=1)\n    right = Right(value=2)\n    carry(left)\n    return first() + second() + left.value + right.value\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["main".into()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_library_manifest_index(index.clone());
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.type_info().semantic_fact_store(&path);
    let main = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "main")
        .ok_or("main identity absent")?
        .clone();
    let required = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores([&facts])
        .reachable_from([main])
        .into_iter()
        .filter(|identity| matches!(identity.origin, incan_semantics_core::SymbolOrigin::Package { .. }))
        .collect();
    let resolved = resolve_executable_requirements(&index, &required)?;
    let primary = resolved.modules.first().ok_or("first module absent")?;
    let graph = ReplacementExecutionGraph::new(primary, resolved.modules.iter().skip(1))?;
    let (first_owner, _) = graph
        .body_for_canonical_target(&one)
        .ok_or("first package target missing")?;
    let (second_owner, _) = graph
        .body_for_canonical_target(&two)
        .ok_or("second package target missing")?;
    assert_ne!(first_owner.module_id, second_owner.module_id);
    assert_eq!(
        execute_free_function(first_owner, "answer", &[])?
            .value
            .observable_text(),
        "41"
    );
    assert_eq!(
        execute_free_function(second_owner, "answer", &[])?
            .value
            .observable_text(),
        "42"
    );
    let consumer = crate::frontend::body_ir::build_body_ir_module_v0_with_executable_context(
        &program,
        &path,
        checker.type_info(),
        &resolved.modules,
    );
    let graph = ReplacementExecutionGraph::new(&consumer, resolved.modules.iter())?;
    let execution = crate::backend::replacement::prepare_free_function_execution_in_graph(graph, "main", &[], None)?;
    assert_eq!(
        crate::backend::replacement::execute_prevalidated_free_function(execution)?
            .value
            .observable_text(),
        "86"
    );
    // Corrupt only the retained expected type identity: both packages still provide a same-named Pair layout.
    // The resulting payload must refuse, even though its diagnostic spelling and field layout still match.
    let wrong_type = resolved
        .modules
        .iter()
        .flat_map(|module| &module.nominal_declarations)
        .find(|declaration| {
            matches!(&declaration.canonical.origin,
            incan_semantics_core::SymbolOrigin::Package { library, .. } if library == "second")
        })
        .ok_or("second Pair context absent")?
        .canonical
        .clone();
    let mut malformed = consumer.clone();
    let carry = malformed
        .bodies
        .iter_mut()
        .find(|body| body.name == "carry")
        .ok_or("carry body absent")?;
    let variant = carry
        .block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            incan_semantics_core::body_ir::StatementKind::Assign {
                rvalue: incan_semantics_core::body_ir::Rvalue::ResultVariant(variant),
                ..
            } => Some(variant),
            _ => None,
        })
        .ok_or("Result construction absent")?;
    variant.canonical_types.insert(vec![0], wrong_type);
    let graph = ReplacementExecutionGraph::new(&malformed, resolved.modules.iter())?;
    let execution = crate::backend::replacement::prepare_free_function_execution_in_graph(graph, "main", &[], None)?;
    let Err(error) = crate::backend::replacement::execute_prevalidated_free_function(execution) else {
        return Err("same-named type from another package satisfied the malformed Result type".into());
    };
    assert!(error.to_string().contains("payload incompatible with retained type"));
    Ok(())
}

/// Public body and default calls load their public closure before a prepared graph executes.
#[test]
fn public_default_calls_and_plain_model_context_execute_from_fragments() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let manifest = artifact(
        temporary.path(),
        "models",
        "pub model Pair:\n    pub left: int\n    pub right: int\n\npub def base() -> int:\n    return 40\n\npub def answer(value: int = base()) -> int:\n    pair = Pair(left=value, right=2)\n    return pair.left + pair.right\n",
    )?;
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("answer")
        .ok_or("canonical answer missing")?;
    let resolved = resolve_executable_requirements(
        &index(temporary.path(), "renamed", &manifest),
        &BTreeSet::from([identity]),
    )?;
    assert_eq!(
        resolved.decoded_declarations, 3,
        "body, default callee, and public model context"
    );
    let module = resolved.modules.first().ok_or("module missing")?;
    assert_eq!(
        execute_free_function(module, "answer", &[])
            .map_err(|error| format!("{error}; {}", module.render_snapshot()))?
            .value
            .observable_text(),
        "42"
    );
    Ok(())
}

/// Child frames share the complete checked graph, including a canonical backedge into its entry module.
#[test]
fn canonical_frames_reenter_the_entry_module_through_a_checked_cycle() -> Result<(), Box<dyn Error>> {
    use crate::cli::commands::common::{
        CompilationSession, collect_modules_detailed_with_session, scoped_compilation_session_analysis_invocations,
    };

    let temporary = tempfile::tempdir()?;
    fs::create_dir(temporary.path().join("src"))?;
    fs::write(
        temporary.path().join("loaf.toml"),
        "[project]\nname = \"frame_cycle\"\n",
    )?;
    let entry_path = temporary.path().join("src/main.incn");
    fs::write(
        &entry_path,
        "from helper import bounce\n\npub def step(value: int) -> int:\n    if value == 0:\n        return 42\n    return bounce(value - 1)\n\ndef main() -> int:\n    return step(4)\n",
    )?;
    fs::write(
        temporary.path().join("src/helper.incn"),
        "from main import step\n\npub def bounce(value: int) -> int:\n    return step(value)\n",
    )?;
    let count = scoped_compilation_session_analysis_invocations();
    let session = CompilationSession::discover_for_collection_with_feature_selection(&entry_path, &Default::default())?;
    let modules = collect_modules_detailed_with_session(entry_path.clone(), &session)
        .map_err(|failure| failure.render_human())?;
    let analysis = session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            None,
        )
        .map_err(|failure| failure.render_human())?;
    let lowered = modules
        .iter()
        .map(|module| {
            let info = analysis
                .type_info_for_path(&module.file_path)
                .ok_or("module analysis absent")?;
            Ok(build_body_ir_module_v0(&module.ast, &module.path_segments, info))
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let entry_index = modules
        .iter()
        .position(|module| module.file_path == entry_path)
        .ok_or("entry absent")?;
    let graph = ReplacementExecutionGraph::new(
        &lowered[entry_index],
        lowered
            .iter()
            .enumerate()
            .filter_map(|(index, module)| (index != entry_index).then_some(module)),
    )?;
    let execution = crate::backend::replacement::prepare_free_function_execution_in_graph(graph, "main", &[], None)?;
    assert_eq!(
        crate::backend::replacement::execute_prevalidated_free_function(execution)?
            .value
            .observable_text(),
        "42"
    );
    assert_eq!(count.invocation_count(), 1);
    Ok(())
}

/// A facade callable accepts the nominal result of the same artifact imported under a direct consumer alias.
#[test]
fn a_facade_signature_retains_the_admitted_catalog_nominal_identity() -> Result<(), Box<dyn Error>> {
    exercise_catalog_signature(false)
}

/// A type facade changes the import route without changing the selected declaration artifact.
#[test]
fn a_type_reexport_diamond_retains_the_admitted_catalog_nominal_identity() -> Result<(), Box<dyn Error>> {
    exercise_catalog_signature(true)
}

/// Check and execute a signature whose nominal reaches the consumer through a different admitted route.
fn exercise_catalog_signature(type_facade: bool) -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let catalog_root = temporary.path().join("catalog");
    let pricing_root = temporary.path().join("pricing");
    let catalog = artifact(
        &catalog_root,
        "catalog",
        "pub model Product:\n    pub value: int\n\npub def first_product() -> Product:\n    return Product(value=42)\n",
    )?;
    let product_identity = catalog
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("Product")
        .ok_or("checked catalog Product identity missing")?;
    let catalog_digest = crate::library_manifest::digest_provider_artifact(&catalog_root)?;
    let facade_root = temporary.path().join("facade");
    let catalog_plan = || {
        crate::provider::ProviderPlan::from_resolved_inputs(
            index(&catalog_root, "catalog", &catalog),
            None,
            None,
            None,
            [],
        )
    };
    let (import_root, import_manifest) = if type_facade {
        let mut facade = artifact_with_provider_plan(
            &facade_root,
            "facade",
            "pub from pub::catalog import Product\n",
            std::sync::Arc::new(catalog_plan()?),
        )?;
        attach_public_dependency(&mut facade, "catalog", &catalog, &catalog_root)?;
        facade.write_to_path(&facade_root.join("facade.incnlib"))?;
        (facade_root, facade)
    } else {
        (catalog_root.clone(), catalog.clone())
    };
    let pricing_plan = crate::provider::ProviderPlan::from_resolved_inputs(
        index(&import_root, "types", &import_manifest),
        None,
        None,
        None,
        [],
    )?;
    let mut pricing = artifact_with_provider_plan(
        &pricing_root,
        "pricing",
        "from pub::types import Product\n\npub def quote(product: Product) -> int:\n    return product.value\n",
        std::sync::Arc::new(pricing_plan),
    )?;
    attach_public_dependency(&mut pricing, "types", &import_manifest, &import_root)?;
    let crate::library_manifest::TypeRef::Named {
        origin: Some(origin), ..
    } = &pricing.exports.functions[0].params[0].ty
    else {
        return Err("pricing lost nominal origin".into());
    };
    assert_eq!(origin.provider.name, "catalog");
    assert_eq!(origin.provider.digest, catalog_digest);
    assert_eq!(origin.canonical.hydrate().as_ref(), Some(&product_identity));
    pricing.write_to_path(&pricing_root.join("pricing.incnlib"))?;
    let mut entries = HashMap::new();
    for (alias, root, manifest) in [
        ("stock", &catalog_root, catalog),
        ("pricing", &pricing_root, pricing.clone()),
    ] {
        let metadata = LibraryArtifactMetadata::from_manifest_path(
            alias,
            &manifest.name,
            root.join(format!("{}.incnlib", manifest.name)),
            root.to_path_buf(),
        );
        entries.insert(
            alias.to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata,
            },
        );
    }
    let plan = crate::provider::ProviderPlan::from_resolved_inputs(
        LibraryManifestIndex::from_entries(entries),
        None,
        None,
        None,
        [],
    )?;
    let selected_catalogs = plan
        .public_artifacts()
        .filter(|artifact| artifact.identity.name == "catalog")
        .collect::<Vec<_>>();
    assert_eq!(
        selected_catalogs.len(),
        1,
        "both routes must reach one admitted catalog artifact"
    );
    assert_eq!(selected_catalogs[0].identity.digest, catalog_digest);
    assert_eq!(
        selected_catalogs[0]
            .manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_name("Product"),
        Some(product_identity)
    );
    let source = "from pub::stock import first_product\nfrom pub::pricing import quote\n\ndef main() -> int:\n    return quote(first_product())\n";
    let program = parser::parse(&lexer::lex(source).map_err(|error| format!("{error:?}"))?)
        .map_err(|error| format!("{error:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.set_provider_plan(std::sync::Arc::new(plan.clone()));
    checker.check_program(&program).map_err(|errors| format!(
        "direct/facade nominal check failed: {errors:?}; published pricing signature: {:?}; retained nominal bindings: {:?}",
        pricing.exports.functions, checker.public_library_type_identities,
    ))?;
    let required = ["quote"]
        .into_iter()
        .filter_map(|name| pricing.contract_metadata.identity_graph.canonical_for_public_name(name))
        .collect();
    let mut required: BTreeSet<_> = required;
    required.extend(
        plan.public_artifacts()
            .filter(|artifact| artifact.identity.name == "catalog")
            .filter_map(|artifact| {
                artifact
                    .manifest
                    .contract_metadata
                    .identity_graph
                    .canonical_for_public_name("first_product")
            }),
    );
    let resolved = super::resolve_executable_requirements(&plan, &required)?;
    let consumer = crate::frontend::body_ir::build_body_ir_module_v0_with_executable_context(
        &program,
        &["main".into()],
        checker.type_info(),
        &resolved.modules,
    );
    let graph = ReplacementExecutionGraph::new(&consumer, resolved.modules.iter())?;
    let execution = crate::backend::replacement::prepare_free_function_execution_in_graph(graph, "main", &[], None)?;
    assert_eq!(
        crate::backend::replacement::execute_prevalidated_free_function(execution)?
            .value
            .observable_text(),
        "42"
    );
    Ok(())
}

/// Record the same exact public artifact edge used by real library publication.
fn attach_public_dependency(
    owner: &mut LibraryManifest,
    key: &str,
    target: &LibraryManifest,
    target_root: &Path,
) -> Result<(), Box<dyn Error>> {
    owner
        .contract_metadata
        .provider
        .provider_dependencies
        .push(crate::library_manifest::ProviderDependencyMetadata {
            kind: crate::library_manifest::ProviderDependencyKind::PublicPackage,
            dependency_key: key.into(),
            provider_name: target.name.clone(),
            provider_version: target.version.clone(),
            artifact_digest: crate::library_manifest::digest_provider_artifact(target_root)?,
            relative_artifact_path: format!("../{}", target.name),
            requested_features: BTreeSet::new(),
            default_features: true,
            optional: false,
        });
    Ok(())
}

/// Package identity and version remain attached to absent and incompatible representation refusals.
#[test]
fn missing_and_future_representations_are_package_refusals() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let mut manifest = artifact(
        temporary.path(),
        "provider",
        "pub def answer() -> int:\n    return 42\n",
    )?;
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("answer")
        .ok_or("answer missing")?;
    let path = executable_surface_path(&temporary.path().join("provider.incnlib"), &manifest).ok_or("path missing")?;
    fs::write(path, [127u8])?;
    let future = resolve_executable_requirements(
        &index(temporary.path(), "renamed", &manifest),
        &BTreeSet::from([identity.clone()]),
    );
    assert!(matches!(
        future,
        Err(ExecutableResolutionError::UnusableRepresentation {
            source:
                incan_semantics_core::executable_representation::ExecutableRepresentationError::UnsupportedVersion {
                    found: 127,
                    ..
                },
            ..
        })
    ));
    manifest.contract_metadata.executable_representation = None;
    let missing = resolve_executable_requirements(
        &index(temporary.path(), "renamed", &manifest),
        &BTreeSet::from([identity]),
    );
    assert!(
        matches!(missing, Err(ExecutableResolutionError::NoPublishedRepresentation { ref library, ref version, .. }) if library == "provider" && version == "1.2.3")
    );
    Ok(())
}

/// A manifest that names a version the surface bytes do not carry is refused on the descriptor alone.
///
/// This is the other half of the version contract, and the surface-bytes test above does not reach it: corrupting
/// the file trips `require_supported_version` on the payload, a different guard. Here the bytes stay valid and
/// only the manifest descriptor disagrees, which is the shape a stale or tampered manifest actually takes.
/// `executable_surface_path` selects the file by digest and ignores the version, so the file is still found and
/// its content still verifies -- the descriptor check is the only thing standing between that manifest and an
/// execution under the wrong representation. Without this test, deleting that check passes the whole suite.
#[test]
fn a_manifest_descriptor_naming_another_version_is_refused() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let mut manifest = artifact(
        temporary.path(),
        "descriptor_version",
        "pub def restated() -> int:\n    return 42\n",
    )?;
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("restated")
        .ok_or("restated missing")?;
    let descriptor = manifest
        .contract_metadata
        .executable_representation
        .as_mut()
        .ok_or("descriptor missing")?;
    let published = descriptor.representation_version;
    descriptor.representation_version = published + 1;

    let refused = resolve_executable_requirements(
        &index(temporary.path(), "renamed", &manifest),
        &BTreeSet::from([identity]),
    );
    assert!(
        matches!(
            refused,
            Err(ExecutableResolutionError::UnusableRepresentation {
                source:
                    incan_semantics_core::executable_representation::ExecutableRepresentationError::UnsupportedVersion {
                        found,
                        ..
                    },
                ..
            }) if found == published + 1
        ),
        "a descriptor naming another version must refuse before execution, got {refused:?}"
    );
    Ok(())
}

/// Content verification streams the file once; only three selected payloads are decoded.
#[test]
fn a_file_reader_loads_three_of_four_hundred_declarations() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let source = (0..400)
        .map(|index| format!("pub def f{index:03}() -> int:\n    return {index}\n\n"))
        .collect::<String>();
    let manifest = artifact(temporary.path(), "many", &source)?;
    let path = executable_surface_path(&temporary.path().join("many.incnlib"), &manifest).ok_or("surface missing")?;
    let bytes = fs::read(&path)?;
    let required = [1, 2, 3]
        .into_iter()
        .map(|index| {
            manifest
                .contract_metadata
                .identity_graph
                .canonical_for_public_name(&format!("f{index:03}"))
                .ok_or("selected identity missing")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let resolved = resolve_executable_requirements(&index(temporary.path(), "renamed", &manifest), &required)?;
    assert_eq!(resolved.decoded_declarations, 3);
    assert_eq!(resolved.content_bytes_verified, u64::try_from(bytes.len())?);
    assert!(resolved.payload_bytes_read < u64::try_from(bytes.len())? / 10);
    Ok(())
}

/// A valid same-length alternative body cannot execute under the descriptor for the previous publication.
#[test]
fn a_valid_same_length_payload_change_is_refused_before_execution() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let original = artifact(
        temporary.path(),
        "arithmetic",
        "pub def answer() -> int:\n    return 41\n",
    )?;
    let old_path = executable_surface_path(&temporary.path().join("arithmetic.incnlib"), &original)
        .ok_or("original surface missing")?;
    let old_length = fs::metadata(&old_path)?.len();
    let changed = artifact(
        temporary.path(),
        "arithmetic",
        "pub def answer() -> int:\n    return 42\n",
    )?;
    let changed_path = executable_surface_path(&temporary.path().join("arithmetic.incnlib"), &changed)
        .ok_or("changed surface missing")?;
    assert_eq!(old_length, fs::metadata(&changed_path)?.len());
    fs::copy(changed_path, old_path)?;
    let required = original
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("answer")
        .ok_or("identity missing")?;
    let Err(error) = resolve_executable_requirements(
        &index(temporary.path(), "renamed", &original),
        &BTreeSet::from([required]),
    ) else {
        return Err("changed bytes satisfied the original publication descriptor".into());
    };
    assert!(error.to_string().contains("package `arithmetic` version 1.2.3"));
    assert!(error.to_string().contains("manifest-selected digest"));
    Ok(())
}

/// Aliased imports acquire exact published type context before the consumer's single Body-IR lowering.
#[test]
fn aliased_public_model_and_enum_execute_without_a_package_function_call() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let manifest = artifact(
        temporary.path(),
        "types",
        "pub model Pair:\n    pub value: int\n\npub enum Mode:\n    Ready\n    Idle\n\npub enum Count(int):\n    Answer = 42\n",
    )?;
    let index = index(temporary.path(), "renamed", &manifest);
    let source = "from pub::renamed import Pair as Item, Mode as State, Count as Tally\n\ndef main() -> int:\n    item = Item(value=0)\n    mode = State.Ready\n    match mode:\n        case State.Ready:\n            match item:\n                case Item(value=number):\n                    return number + Tally.Answer.value()\n        case State.Idle:\n            return 0\n    return 0\n";
    let tokens = lexer::lex(source).map_err(|error| format!("{error:?}"))?;
    let program = parser::parse(&tokens).map_err(|error| format!("{error:?}"))?;
    let module_path = vec!["main".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_library_manifest_index(index.clone());
    checker.check_program(&program).map_err(|error| format!("{error:?}"))?;
    let facts = checker.type_info().semantic_fact_store(&module_path);
    let main = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "main")
        .ok_or("main identity absent")?
        .clone();
    let required = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores([&facts])
        .reachable_from([main])
        .into_iter()
        .filter(|identity| matches!(identity.origin, incan_semantics_core::SymbolOrigin::Package { .. }))
        .collect();
    let resolved = resolve_executable_requirements(&index, &required)?;
    assert_eq!(resolved.decoded_declarations, 3);
    let module = crate::frontend::body_ir::build_body_ir_module_v0_with_executable_context(
        &program,
        &module_path,
        checker.type_info(),
        &resolved.modules,
    );
    let graph = ReplacementExecutionGraph::new(&module, resolved.modules.iter())?;
    let execution = crate::backend::replacement::prepare_free_function_execution_in_graph(graph, "main", &[], None)?;
    let result = crate::backend::replacement::execute_prevalidated_free_function(execution)?;
    assert_eq!(result.value.observable_text(), "42");
    Ok(())
}

/// A facade edge retains the original provider's identity and resolves its artifact through the public dependency
/// graph.
#[test]
fn a_transitive_facade_resolves_the_declaring_artifact() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let origin_root = temporary.path().join("origin");
    let facade_root = temporary.path().join("facade");
    let origin = artifact(&origin_root, "origin", "pub def answer() -> int:\n    return 42\n")?;
    let mut facade = artifact(&facade_root, "facade", "pub def unused() -> int:\n    return 0\n")?;
    let mut export = origin
        .contract_metadata
        .identity_graph
        .exports
        .first()
        .ok_or("origin export absent")?
        .clone();
    export.public_name = "exposed".into();
    export.public_path = vec!["facade".into(), "exposed".into()];
    facade.contract_metadata.identity_graph.exports.push(export);
    facade
        .contract_metadata
        .provider
        .provider_dependencies
        .push(crate::library_manifest::ProviderDependencyMetadata {
            kind: crate::library_manifest::ProviderDependencyKind::PublicPackage,
            dependency_key: "original_alias".into(),
            provider_name: origin.name.clone(),
            provider_version: origin.version.clone(),
            artifact_digest: crate::library_manifest::digest_provider_artifact(&origin_root)?,
            relative_artifact_path: "../origin".into(),
            requested_features: BTreeSet::new(),
            default_features: true,
            optional: false,
        });
    let required = facade
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("exposed")
        .ok_or("facade identity absent")?;
    let resolved = resolve_executable_requirements(
        &index(&facade_root, "renamed_facade", &facade),
        &BTreeSet::from([required.clone()]),
    )?;
    assert_eq!(resolved.decoded_declarations, 1);
    let module = resolved.modules.first().ok_or("provider module absent")?;
    assert_eq!(
        execute_free_function(module, "answer", &[])
            .map_err(|error| format!("{error}; {}", module.render_snapshot()))?
            .value
            .observable_text(),
        "42"
    );
    let edge = facade
        .contract_metadata
        .provider
        .provider_dependencies
        .first_mut()
        .ok_or("facade edge absent")?;
    edge.kind = crate::library_manifest::ProviderDependencyKind::PrivateImplementation;
    let private = resolve_executable_requirements(
        &index(&facade_root, "renamed_facade", &facade),
        &BTreeSet::from([required.clone()]),
    );
    assert!(matches!(private, Err(ExecutableResolutionError::UnknownPackage { .. })));
    let edge = facade
        .contract_metadata
        .provider
        .provider_dependencies
        .first_mut()
        .ok_or("facade edge absent")?;
    edge.kind = crate::library_manifest::ProviderDependencyKind::PublicPackage;
    edge.artifact_digest = format!("sha256:{}", "0".repeat(64));
    let altered = resolve_executable_requirements(
        &index(&facade_root, "renamed_facade", &facade),
        &BTreeSet::from([required]),
    );
    assert!(matches!(
        altered,
        Err(ExecutableResolutionError::DependencyArtifact { .. })
    ));
    Ok(())
}

/// Imported nominal context must remain usable inside the existing structural Result payload profile.
#[test]
fn imported_nominal_result_payload_uses_declaring_type_context() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let manifest = artifact(
        temporary.path(),
        "types",
        "pub model Pair:\n    pub value: int\n\npub enum Mode:\n    Ready\n    Idle\n\npub def make_pair() -> Pair:\n    return Pair(value=42)\n",
    )?;
    let index = index(temporary.path(), "renamed", &manifest);
    let sources = [
        "from pub::renamed import Pair as Item\n\ndef carry(item: Item) -> Result[Item, str]:\n    return Ok(item)\n\ndef main() -> int:\n    carry(Item(value=42))\n    return 42\n",
        "from pub::renamed import Mode as State\n\ndef carry(state: State) -> Result[int, State]:\n    return Err(state)\n\ndef main() -> int:\n    match carry(State.Ready):\n        case Ok(value):\n            return value\n        case Err(_):\n            return 42\n    return 0\n",
        "from pub::renamed import Mode as State\n\ndef carry() -> Result[list[tuple[int, int]], State]:\n    return Ok([(20, 22)])\n\ndef main() -> int:\n    carry()\n    return 42\n",
        "from pub::renamed import Pair as Item\n\ndef carry(item: Item, Item: int = 0) -> Result[Item, str]:\n    return Ok(item)\n\ndef main() -> int:\n    carry(Item(value=42))\n    return 42\n",
        "from pub::renamed import Pair as Item\n\ndef carry(item: Item = Item(value=42)) -> Result[Item, str]:\n    return Ok(item)\n\ndef main() -> int:\n    carry()\n    return 42\n",
        "from pub::renamed import make_pair\n\ndef main() -> int:\n    item = make_pair()\n    return item.value\n",
        "model Pair:\n    value: int\n\ndef carry(item: Pair, Pair: int = 0) -> Result[Pair, str]:\n    return Ok(item)\n\ndef main() -> int:\n    carry(Pair(value=42))\n    return 42\n",
    ];
    for source in sources {
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let path = vec!["main".into()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(path.clone()));
        checker.set_library_manifest_index(index.clone());
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let facts = checker.type_info().semantic_fact_store(&path);
        // Check the inferred provider-only return type without relying on any lexical import of Pair.
        if source.contains("from pub::renamed import make_pair") {
            let start = source.find("make_pair()").ok_or("factory call span absent")?;
            let identities = checker
                .type_info()
                .expressions
                .expression_type_identities
                .get(&(start, start + "make_pair()".len()))
                .ok_or("inferred provider type identity absent")?;
            assert!(identities.values().any(|identity| identity.declaration_name == "Pair"
                && matches!(&identity.origin, incan_semantics_core::SymbolOrigin::Package { library, .. } if library == "types")));
        }
        let main = checker
            .type_info()
            .declarations
            .declaration_identities
            .values()
            .find(|identity| identity.declaration_name == "main")
            .ok_or("main absent")?
            .clone();
        let required = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores([&facts])
            .reachable_from([main])
            .into_iter()
            .filter(|identity| matches!(identity.origin, incan_semantics_core::SymbolOrigin::Package { .. }))
            .collect();
        let resolved = resolve_executable_requirements(&index, &required)?;
        let consumer = crate::frontend::body_ir::build_body_ir_module_v0_with_executable_context(
            &program,
            &path,
            checker.type_info(),
            &resolved.modules,
        );
        let graph = ReplacementExecutionGraph::new(&consumer, resolved.modules.iter())?;
        let execution =
            crate::backend::replacement::prepare_free_function_execution_in_graph(graph, "main", &[], None)?;
        assert_eq!(
            crate::backend::replacement::execute_prevalidated_free_function(execution)?
                .value
                .observable_text(),
            "42"
        );
    }
    Ok(())
}

/// Admit two checked packages whose same-spelled nominal declarations must retain distinct artifact identities.
fn product_pair_provider_plan(root: &Path) -> Result<std::sync::Arc<crate::provider::ProviderPlan>, Box<dyn Error>> {
    let mut entries = HashMap::new();
    for library in ["first", "second"] {
        let root = root.join(library);
        let manifest = artifact(&root, library, "pub model Product:\n    pub value: int\n")?;
        entries.insert(
            library.into(),
            LibraryManifestIndexEntry::Loaded {
                metadata: LibraryArtifactMetadata::from_manifest_path(
                    library,
                    library,
                    root.join(format!("{library}.incnlib")),
                    root,
                ),
                manifest: Box::new(manifest),
            },
        );
    }
    Ok(std::sync::Arc::new(
        crate::provider::ProviderPlan::from_resolved_inputs(
            LibraryManifestIndex::from_entries(entries),
            None,
            None,
            None,
            [],
        )?,
    ))
}

/// Same-spelled nominal declarations retain separate artifacts through aliases, nested leaves and local shadowing.
#[test]
fn two_foreign_products_keep_distinct_signature_origins() -> Result<(), Box<dyn Error>> {
    use crate::library_manifest::{TypeRef, VisitTypeRefs};
    let temporary = tempfile::tempdir()?;
    let plan = product_pair_provider_plan(temporary.path())?;
    let source = "from pub::first import Product as Left\nfrom pub::second import Product as Right\n\npub def combine(left: Left, right: Right, nested: list[Right]) -> int:\n    Left = 1\n    return left.value + right.value + Left\n";
    let mut manifest = artifact_with_provider_plan(&temporary.path().join("pairing"), "pairing", source, plan.clone())?;
    let params = &manifest.exports.functions[0].params;
    let TypeRef::Named {
        origin: Some(first), ..
    } = &params[0].ty
    else {
        return Err("first origin missing".into());
    };
    let TypeRef::Named {
        origin: Some(second), ..
    } = &params[1].ty
    else {
        return Err("second origin missing".into());
    };
    assert_eq!(first.canonical.declaration_name, "Product");
    assert_eq!(second.canonical.declaration_name, "Product");
    assert_eq!(first.provider.name, "first");
    assert_eq!(second.provider.name, "second");
    assert_ne!(first, second);
    let mut origins = Vec::new();
    manifest.visit_type_refs(&mut |leaf| {
        if let TypeRef::Named {
            origin: Some(origin), ..
        } = leaf
        {
            origins.push(origin.provider.name.clone());
        }
    });
    assert_eq!(origins, ["first", "second", "second"]);
    let invalid =
        format!("{source}\ndef main() -> int:\n    wrong = Left(value=1)\n    return combine(wrong, wrong, [])\n");
    let program = parser::parse(&lexer::lex(&invalid).map_err(|error| format!("{error:?}"))?)
        .map_err(|error| format!("{error:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.set_provider_plan(plan);
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("distinct selected nominal types were accepted")?;
    assert!(
        errors.iter().any(|error| error.message.contains("type mismatch")),
        "{errors:?}"
    );
    Ok(())
}

/// A public primitive-union facade retains its admitted provider route without exposing private imports.
#[test]
fn public_type_alias_facades_retain_only_checked_native_bridge_roots() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let manifest = artifact(temporary.path(), "provider", "pub type Answer = int | str\n")?;
    let plan = std::sync::Arc::new(crate::provider::ProviderPlan::from_resolved_inputs(
        index(temporary.path(), "selected", &manifest),
        None,
        None,
        None,
        [],
    )?);
    for (source, public) in [
        ("pub from pub::selected import Answer as Reading\n", true),
        ("from pub::selected import Answer as Reading\n", false),
    ] {
        let program = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
            .map_err(|errors| format!("{errors:?}"))?;
        let module_path = vec!["lib".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker.set_current_package_identity(Some("facade".into()));
        checker.set_provider_plan(plan.clone());
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(
            checker.type_info().declarations.public_type_bridge_roots,
            if public {
                BTreeSet::from(["selected".to_string()])
            } else {
                BTreeSet::new()
            }
        );
        let mut codegen = crate::backend::ir::IrCodegen::new();
        codegen.set_provider_plan(plan.clone());
        codegen.set_preserve_dependency_public_items(true);
        codegen.set_prechecked_type_info(checker.type_info().clone(), HashMap::new());
        let rust = codegen.try_generate(&program)?;
        assert_eq!(rust.contains("pub mod __incan_provider_rust"), public, "{rust}");
        assert_eq!(rust.contains("pub use ::selected;"), public, "{rust}");
    }
    Ok(())
}

/// Union coverage follows admitted nominal identities through aliases, optional subjects, groups and guards.
#[test]
fn package_union_patterns_cover_exact_selected_nominals() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let plan = product_pair_provider_plan(temporary.path())?;
    let imports = "from pub::first import Product as Left\nfrom pub::first import Product as OtherLeft\nfrom pub::second import Product as Right\n\n";
    let cases = [
        (
            "renamed nominal",
            "def read(value: Union[Left, Right, int]) -> int:\n    match value:\n        OtherLeft(item) => return item.value\n        Right(item) => return item.value\n        int(number) => return number\n",
            true,
        ),
        (
            "optional grouped alternation",
            "def read(value: Union[Left, Right, int] | None) -> int:\n    match value:\n        (OtherLeft(_)) | Right(_) => return 1\n        int(number) => return number\n        None => return 0\n",
            true,
        ),
        (
            "union alias subset",
            "type Pair = Union[OtherLeft, Right]\n\ndef read(value: Union[Left, Right, int]) -> int:\n    match value:\n        Pair(_) => return 1\n        int(number) => return number\n",
            true,
        ),
        (
            "other selected artifact uncovered",
            "def read(value: Union[Left, Right, int]) -> int:\n    match value:\n        OtherLeft(_) => return 1\n        Left(_) => return 2\n        int(number) => return number\n",
            false,
        ),
        (
            "guard does not prove coverage",
            "def read(value: Union[Left, Right, int]) -> int:\n    match value:\n        case OtherLeft(item) if item.value > 0:\n            return item.value\n        case Right(item):\n            return item.value\n        case int(number):\n            return number\n",
            false,
        ),
        (
            "optional None uncovered",
            "def read(value: Union[Left, Right, int] | None) -> int:\n    match value:\n        OtherLeft(_) | Right(_) => return 1\n        int(number) => return number\n",
            false,
        ),
    ];
    for (label, body, accepted) in cases {
        let source = format!("{imports}{body}");
        let program = parser::parse(&lexer::lex(&source).map_err(|error| format!("{label}: {error:?}"))?)
            .map_err(|error| format!("{label}: {error:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".into()]));
        checker.set_provider_plan(plan.clone());
        match checker.check_program(&program) {
            Ok(()) if accepted => {
                let origins = &checker.type_info().declarations.named_type_origins;
                let left = origins.get("Left").ok_or("left checked origin missing")?;
                let right = origins.get("Right").ok_or("right checked origin missing")?;
                assert_eq!(left.canonical.declaration_name, right.canonical.declaration_name);
                assert_ne!(left.provider, right.provider);
            }
            Err(errors) if !accepted && errors.iter().any(|error| error.message.contains("Non-exhaustive")) => {}
            result => return Err(format!("{label}: unexpected check result {result:?}").into()),
        }
    }
    Ok(())
}

/// Assignment-compatible numeric representations remain distinct alternatives for exhaustive union matching.
#[test]
fn numeric_union_patterns_preserve_distinct_alternatives() -> Result<(), Box<dyn Error>> {
    for (arms, accepted) in [
        ("        int(_) => return 1\n        i32(_) => return 2\n", true),
        ("        int(_) => return 1\n", false),
    ] {
        let source = format!("def read(value: Union[int, i32]) -> int:\n    match value:\n{arms}");
        let program = parser::parse(&lexer::lex(&source).map_err(|error| format!("{error:?}"))?)
            .map_err(|error| format!("{error:?}"))?;
        let mut checker = TypeChecker::new();
        match checker.check_program(&program) {
            Ok(()) if accepted => {}
            Err(errors) if !accepted && errors.iter().any(|error| error.message.contains("Non-exhaustive")) => {}
            result => return Err(format!("numeric union coverage: unexpected check result {result:?}").into()),
        }
    }
    Ok(())
}
