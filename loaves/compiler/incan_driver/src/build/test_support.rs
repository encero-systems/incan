//! Fixtures the build tests share: published project outputs, provider authorities, vocab declarations and
//! replacement options.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::backend::selection::{BackendKind, FallbackPolicy, ShadowComparisonState, finalize_receipt, select_backend};
use crate::build::output_paths::packaged_library_metadata_files;
use crate::build::package_loafs::write_packaged_library_loaf_manifest;
use crate::build::publication::project_output_payload_for_bake;
use crate::build::source_authority::{
    baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority,
};
use crate::build::{
    BackendSelectionOptions, BuildCommandOptions, OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
    OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OvenBakeProjectTarget, OvenPackagedLibraryLoafManifest,
    OvenPackagedLibraryLoafProfile, OvenProjectOutputBakeFile, OvenProjectOutputBakeRequest, OvenProjectOutputPayload,
};
use incan_frontend::ast::{Declaration, Span, Spanned};
use incan_frontend::diagnostics;
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest_index::LibraryArtifactMetadata;
use incan_lang::version::INCAN_VERSION;
use oven_model::manifest::LOAF_MANIFEST_FILENAME;
use oven_rustc::rustc::{OvenProjectInspectionAuthorityRef, resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

/// Publish-ready receipt, payload, and files for the fixture project's executable target under one fixed project
/// authority; the shorthand most bake and reuse tests want before they vary anything.
pub fn fixture_project_output_publication(
    project_root: &Path,
    profile: &str,
    label: &str,
) -> Result<
    (
        oven_store::OvenReceipt,
        OvenProjectOutputPayload,
        Vec<OvenProjectOutputBakeFile>,
    ),
    Box<dyn std::error::Error>,
> {
    fixture_project_output_publication_for(
        project_root,
        profile,
        label,
        OvenBakeProjectTarget::Executable,
        OvenProjectInspectionAuthorityRef {
            identity: "sha256:fixture-project-authority".to_string(),
            receipt_identity: "sha256:fixture-authority-receipt".to_string(),
            build_unit_identity: "sha256:fixture-authority-build-unit".to_string(),
        },
    )
}

/// Publish-ready receipt, payload, and files for one bake target of the fixture project.
///
/// The receipt derives from the generated source and the label, so two calls with the same label and target share one
/// receipt lineage; only the payload differs when `inspection_authority` does, which is exactly what a bake that
/// re-seals the project authority for unchanged sources leaves in the store.
pub fn fixture_project_output_publication_for(
    project_root: &Path,
    profile: &str,
    label: &str,
    target: OvenBakeProjectTarget,
    inspection_authority: OvenProjectInspectionAuthorityRef,
) -> Result<
    (
        oven_store::OvenReceipt,
        OvenProjectOutputPayload,
        Vec<OvenProjectOutputBakeFile>,
    ),
    Box<dyn std::error::Error>,
> {
    let entrypoint = project_root.join(target.source_relative_path());
    let generated_source = project_root.join(format!("target/fixture/generated-{label}.rs"));
    let native_output = project_root.join(format!("target/fixture/native-{label}"));
    fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
    fs::write(&generated_source, format!("fn {label}() {{}}\n"))?;
    fs::write(&native_output, format!("native-{label}"))?;
    let rustc = resolve_active_rustc()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project_root,
            "fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            profile,
            Vec::new(),
        )
        .with_generated_source("generated-root", &generated_source)
        .with_build_unit_input("compiler-version", INCAN_VERSION),
    )?;
    let files = vec![OvenProjectOutputBakeFile {
        source_path: native_output,
        caller_relative_path: format!("target/fixture/{profile}-{label}"),
        output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
    }];
    let backend_selection = select_backend(
        BackendKind::Legacy,
        false,
        false,
        format!("sha256:fixture-source-{label}"),
        FallbackPolicy::Refuse,
    );
    let backend_receipt = finalize_receipt(
        &backend_selection,
        BackendKind::Legacy,
        format!("sha256:fixture-output-{label}"),
        ShadowComparisonState::NotRequested,
        diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
    )?;
    let payload = project_output_payload_for_bake(OvenProjectOutputBakeRequest {
        project_root,
        entrypoint: &entrypoint,
        target,
        receipt: &receipt,
        plan_identity: format!("fixture-plan-{label}"),
        profile,
        source_authority_digest: &digest_baked_project_source_authority(project_root)?,
        lock_dependencies_fingerprint: baked_project_lock_dependencies_fingerprint(project_root)?,
        files: files.clone(),
        inspection_authority,
        required_project_loafs: Vec::new(),
        package_loaf_store_relative_path: None,
        backend_receipt,
        build_report: None,
    })?;
    Ok((receipt, payload, files))
}

/// A provider package on disk with a sealed packaged-Loaf profile per requested profile: manifest, authored and
/// generated sources, the `.incnlib` manifest and one receipted output per profile, so authority and plan-selection
/// tests can read a complete provider without baking one.
pub fn packaged_provider_authority_fixture(
    profiles: &[&str],
) -> Result<(tempfile::TempDir, LibraryArtifactMetadata), Box<dyn std::error::Error>> {
    let package = tempfile::tempdir()?;
    let artifact_root = package.path().join("target/lib");
    let authored_source = package.path().join("src/lib.incn");
    let generated_source = artifact_root.join("src/lib.rs");
    fs::create_dir_all(authored_source.parent().ok_or("provider source has no parent")?)?;
    fs::create_dir_all(
        generated_source
            .parent()
            .ok_or("generated provider source has no parent")?,
    )?;
    fs::write(
        package.path().join(LOAF_MANIFEST_FILENAME),
        "[project]\nname = \"provider\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(&authored_source, "pub def provider() -> int:\n    return 1\n")?;
    fs::write(&generated_source, "pub fn provider() {}\n")?;
    let library_manifest = LibraryManifest::new("provider", "0.1.0");
    let library_manifest_path = artifact_root.join("provider.incnlib");
    library_manifest.write_to_path(&library_manifest_path)?;
    let artifact = LibraryArtifactMetadata::from_crate_root("provider", "provider", &artifact_root);
    let mut package_profiles = BTreeMap::new();
    for profile in profiles {
        let output = artifact_root.join(format!("oven/{profile}/libprovider.rlib"));
        fs::create_dir_all(output.parent().ok_or("provider output has no parent")?)?;
        fs::write(&output, format!("sealed {profile} provider output"))?;
        let receipt = oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                &artifact_root,
                "provider",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                *profile,
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_source),
        )?;
        package_profiles.insert(
            (*profile).to_string(),
            OvenPackagedLibraryLoafProfile {
                receipt,
                entries: Vec::new(),
                library_relative_path: format!("oven/{profile}/libprovider.rlib"),
                library_digest: digest_bytes(&fs::read(output)?),
            },
        );
    }
    write_packaged_library_loaf_manifest(
        &artifact_root,
        &OvenPackagedLibraryLoafManifest {
            schema_version: OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
            source_authority_digest: digest_baked_project_source_authority(package.path())?,
            compiler_version: INCAN_VERSION.to_string(),
            metadata_files: packaged_library_metadata_files(&library_manifest_path, &library_manifest, &artifact_root)?,
            profiles: package_profiles,
        },
    )?;
    Ok((package, artifact))
}

/// Build the replacement-backend options a `--backend replacement` build resolves to.
pub fn replacement_build_options() -> BuildCommandOptions {
    BuildCommandOptions {
        backend: BackendSelectionOptions {
            requested: BackendKind::Replacement,
            explicit: true,
            shadow: false,
            fallback_policy: FallbackPolicy::Refuse,
        },
        ..BuildCommandOptions::default()
    }
}

/// A raw vocab declaration whose owning library manifest this compilation never loaded.
///
/// The parser only produces one of these when an import activates a library vocabulary, which the replacement
/// module profile still refuses, so building it by hand is the only way to put the desugar pass in front of a
/// node it must resolve.
pub fn undesugared_vocab_declaration() -> Spanned<Declaration> {
    Spanned::new(
        Declaration::VocabBlock(incan_frontend::ast::VocabBlockStmt {
            keyword: "query".to_string(),
            keyword_binding: incan_frontend::ast::VocabKeywordBinding {
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
        Span::new(0, 1),
    )
}
