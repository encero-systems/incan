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
    Ok(super::resolve_executable_requirements(&plan, required)?)
}

/// Produce a manifest and semantic surface from one checked source input, using the real identity exporter.
fn artifact(root: &Path, library: &str, source: &str) -> Result<LibraryManifest, Box<dyn Error>> {
    let tokens = lexer::lex(source).map_err(|error| format!("{error:?}"))?;
    let program = parser::parse(&tokens).map_err(|error| format!("{error:?}"))?;
    let module_path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_current_package_identity(Some(library.to_string()));
    checker.check_program(&program).map_err(|error| format!("{error:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut manifest = LibraryManifest::from_checked_exports(library, "1.2.3", &exports);
    let module = build_body_ir_module_v0(&program, &module_path, checker.type_info());
    let unrepresentable = module
        .bodies
        .iter()
        .filter(|body| crate::backend::replacement::validate_published_body_profile(body).is_err())
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
        "pub model Pair:\n    pub value: int\n\npub enum Mode:\n    Ready\n    Idle\n",
    )?;
    let index = index(temporary.path(), "renamed", &manifest);
    let source = "from pub::renamed import Pair as Item, Mode as State\n\ndef main() -> int:\n    item = Item(value=42)\n    mode = State.Ready\n    match mode:\n        case State.Ready:\n            match item:\n                case Item(value=number):\n                    return number\n        case State.Idle:\n            return 0\n    return 0\n";
    let tokens = lexer::lex(source).map_err(|error| format!("{error:?}"))?;
    let program = parser::parse(&tokens).map_err(|error| format!("{error:?}"))?;
    let module_path = vec!["main".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_library_manifest_index(index.clone());
    checker.check_program(&program).map_err(|error| format!("{error:?}"))?;
    let required = checker
        .type_info()
        .references
        .resolved_identities
        .values()
        .filter(|identity| matches!(identity.origin, incan_semantics_core::SymbolOrigin::Package { .. }))
        .cloned()
        .collect();
    let resolved = resolve_executable_requirements(&index, &required)?;
    assert_eq!(resolved.decoded_declarations, 2);
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
        "pub model Pair:\n    pub value: int\n\npub enum Mode:\n    Ready\n    Idle\n",
    )?;
    let index = index(temporary.path(), "renamed", &manifest);
    let sources = [
        "from pub::renamed import Pair as Item\n\ndef carry(item: Item) -> Result[Item, str]:\n    return Ok(item)\n\ndef main() -> int:\n    carry(Item(value=42))\n    return 42\n",
        "from pub::renamed import Mode as State\n\ndef carry(state: State) -> Result[int, State]:\n    return Err(state)\n\ndef main() -> int:\n    match carry(State.Ready):\n        case Ok(value):\n            return value\n        case Err(_):\n            return 42\n    return 0\n",
        "from pub::renamed import Mode as State\n\ndef carry() -> Result[list[tuple[int, int]], State]:\n    return Ok([(20, 22)])\n\ndef main() -> int:\n    carry()\n    return 42\n",
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
