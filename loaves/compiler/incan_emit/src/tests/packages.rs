//! Packages published the way a library build publishes them, for tests that need a consumer of a `pub::` dependency.

use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use crate::IrCodegen;
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
};
use incan_frontend::ast::Program;
use incan_frontend::library_exports::collect_checked_public_exports;
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::provider::{
    NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord,
};
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};

/// Result of a test that reports failures as errors.
pub(super) type TestResult = Result<(), Box<dyn Error>>;

/// Parse one module, keeping lexer and parser failures as test errors.
pub(super) fn parse(source: &str) -> Result<Program, Box<dyn Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    Ok(parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?)
}

/// Check one package's `lib` module against its dependencies, publish its manifest the way a library build does, and
/// return the manifest with the package's generated Rust.
pub(super) fn publish_package(
    name: &str,
    source: &str,
    dependencies: &[&LibraryManifest],
) -> Result<(LibraryManifest, String), Box<dyn Error>> {
    let plan = Arc::new(provider_plan_of(dependencies)?);
    let program = parse(source)?;
    let module_path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some(name.to_string()));
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_provider_plan(plan.clone());
    checker
        .check_program(&program)
        .map_err(|errors| format!("{name} should typecheck: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut manifest = LibraryManifest::from_checked_exports(name, "0.1.0", &exports);
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&program, &checker, module_path.clone())],
        public_namespaces: Vec::new(),
    });
    let mut codegen = IrCodegen::new();
    codegen.set_provider_plan(plan);
    codegen.set_prechecked_type_info(checker.type_info().clone(), HashMap::new());
    codegen.set_publication_api(manifest.contract_metadata.api.clone());
    codegen.set_publication_identities(manifest.name.clone(), manifest.contract_metadata.identity_graph.clone());
    let (code, metadata) = codegen.try_generate_with_metadata(&program, &module_path)?;
    metadata.apply_to_library_manifest(&mut manifest)?;
    Ok((manifest, code))
}

/// Admit `manifests` as a consumer's resolved dependencies, each under its package name.
pub(super) fn provider_plan_of(manifests: &[&LibraryManifest]) -> Result<ProviderPlan, Box<dyn Error>> {
    let mut entries = HashMap::new();
    let mut records = Vec::new();
    for manifest in manifests {
        let artifact = LibraryArtifactMetadata::from_crate_root(
            &manifest.name,
            &manifest.name,
            std::env::temp_dir().join(format!("incan_test_package_{}", manifest.name)),
        );
        entries.insert(
            manifest.name.clone(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new((*manifest).clone()),
                metadata: artifact.clone(),
            },
        );
        records.push(ProviderRecord {
            identity: ProviderIdentity {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                digest: format!(
                    "{:0>64}",
                    manifest
                        .name
                        .bytes()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                ),
                feature_projection: Default::default(),
            },
            provenance: ProviderProvenance::ProjectDependency {
                dependency_key: manifest.name.clone(),
                manifest_path: artifact.manifest_path.clone(),
            },
            authority: NamespaceAuthority::ProjectDependency {
                dependency_key: manifest.name.clone(),
            },
            namespace_claims: Default::default(),
            available: true,
            enabled: true,
            manifest: Some(Arc::new((*manifest).clone())),
            artifact: Some(artifact),
            implementation_facets: Vec::new(),
        });
    }
    Ok(ProviderPlan::new(
        LibraryManifestIndex::from_entries(entries),
        records,
        [],
    )?)
}

/// Return the projection of the first function a generated module declares.
pub(super) fn declared_function_projection(code: &str) -> Result<String, Box<dyn Error>> {
    Ok(code
        .lines()
        .find_map(|line| {
            line.trim_start()
                .strip_prefix("pub fn __incan_v1_")
                .and_then(|tail| tail.split(['(', '<']).next())
                .map(|payload| format!("__incan_v1_{payload}"))
        })
        .ok_or_else(|| format!("no projected function declared:\n{code}"))?)
}
