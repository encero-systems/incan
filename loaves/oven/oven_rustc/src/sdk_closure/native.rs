//! Native-link facts at the explicit adopted-closure publisher boundary.

use std::path::{Path, PathBuf};

use oven_model::manifest::{RustFactArgument, RustFactLink};
use oven_store::process::BoundedProcessLimits;
use oven_store::publisher_owner::VirtualToolchainLoaf;
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStoreExecutionPayload,
};
use oven_store::{OvenReceipt, receipt_with_build_unit_input};

use super::{CompileContext, Error, PreparedUnit};
use crate::rustc::direct_compiler::{OvenPublisherLinkBakeRequest, bake_publisher_link, publisher_archive_format};

/// A store-owned native archive retained while the consuming compiler runs.
pub(super) struct NativeProduct {
    pub(super) name: String,
    pub(super) owner: OvenStoreExecutionPayload,
}

/// Execute only admitted native-link records, refusing names whose source or executable authority is absent.
pub(super) fn prepare(
    unit: &PreparedUnit,
    context: &CompileContext<'_>,
    receipt: &OvenReceipt,
) -> Result<Vec<NativeProduct>, Error> {
    let Some(fact) = &unit.fact else {
        return Ok(Vec::new());
    };
    let roots: Vec<PathBuf> = std::env::var_os("INCAN_OVEN_LINK_OWNERS")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    let mut products = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    let mut libraries = std::collections::BTreeSet::new();
    for link in &fact.link {
        if !names.insert(&link.name) || !libraries.insert(&link.library.name) {
            return Err(format!("link record {}: overlapping producer or library identity", link.name).into());
        }
        products.push(
            prepare_link(unit, context, receipt, link, &roots)
                .map_err(|error| format!("link record {}: {error}", link.name))?,
        );
    }
    Ok(products)
}

/// Reuse an exact source/fact-bound archive before selecting tools; a miss requires a versioned system installation.
fn prepare_link(
    unit: &PreparedUnit,
    context: &CompileContext<'_>,
    receipt: &OvenReceipt,
    link: &RustFactLink,
    roots: &[PathBuf],
) -> Result<NativeProduct, Error> {
    let effective_link = apple_deployment_link(link, context.target);
    let link = &effective_link;
    validate_source_catalog(link)?;
    let receipt = receipt_with_build_unit_input(receipt, "sdk-link-fact", serde_json::to_string(link)?)?;
    let domain = "adopted-native-link";
    oven_store::store_mirror::import_matching_from_mirrors(
        context.store,
        &oven_store::store_mirror::configured_mirrors(|name| std::env::var_os(name)),
        Some(&receipt),
        |manifest| {
            manifest.kind == OvenArtifactKind::Engine
                && manifest.domain == domain
                && manifest.receipt_identity == receipt.identity
        },
    )?;
    let selected = context.store.select_payloads_matching_for_execution(
        |manifest| {
            manifest.kind == OvenArtifactKind::Engine
                && manifest.domain == domain
                && manifest.receipt_identity == receipt.identity
        },
        |_, _| true,
    )?;
    if let Some(owner) = selected.into_iter().next() {
        owner.verify_admitted_payload()?;
        return Ok(NativeProduct {
            name: link.library.name.clone(),
            owner,
        });
    }
    let (owner, descriptor) = select_owner(link, roots)?;
    let mut execution_link = link.clone();
    execution_link.executable.owner = descriptor.identity()?;
    let staging = tempfile::Builder::new().prefix("native-").tempdir_in(context.output)?;
    let product = bake_publisher_link(&OvenPublisherLinkBakeRequest {
        link: &execution_link,
        selected_target: context.target,
        archive_format: publisher_archive_format(context.target),
        toolchain: context.toolchain,
        consuming_unit_identity: &receipt.identity,
        executable_owner_root: &owner,
        source_owner_root: &unit.root,
        output_root: &staging.path().join("product"),
        limits: BoundedProcessLimits {
            stdout_bytes: 1024 * 1024,
            stderr_bytes: 1024 * 1024,
            timeout: Some(std::time::Duration::from_secs(120)),
        },
    })?;
    let entry = context.store.publish(&OvenArtifactPublishRequest {
        receipt,
        domain: domain.to_string(),
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&product.receipt)?,
        materialized_files: vec![OvenArtifactMaterializedFile {
            source_path: product.archive_path,
            relative_path: product.archive_relative_path,
        }],
        materialized_directories: Vec::new(),
    })?;
    let owner = context
        .store
        .select_payloads_for_execution(&[entry.identity])?
        .pop()
        .ok_or("published native archive is unavailable")?;
    Ok(NativeProduct {
        name: link.library.name.clone(),
        owner,
    })
}

/// Bind Apple native objects to the same explicit minimum OS as the pinned Rust linker.
///
/// The effective fact is serialized into the native archive receipt before Store lookup. This policy overrides
/// captured compiler defaults without inheriting the host SDK's minimum, and leaves non-Apple facts unchanged.
fn apple_deployment_link(link: &RustFactLink, target: &str) -> RustFactLink {
    let mut effective = link.clone();
    if target.ends_with("-apple-darwin") {
        for object in &mut effective.objects {
            object.arguments.push(RustFactArgument::Literal {
                literal: "-mmacosx-version-min=11.0".to_string(),
            });
        }
    }
    effective
}

/// Refuse source catalogs requiring inert metadata reads or ambiguous object/library names before execution.
pub(super) fn validate_source_catalog(link: &RustFactLink) -> Result<(), Error> {
    let mut objects = std::collections::BTreeSet::new();
    for object in &link.objects {
        if !objects.insert(&object.name) {
            return Err("duplicate declared object".into());
        }
    }
    if link.library.name.is_empty()
        || !link
            .library
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err("invalid static library name".into());
    }
    for source in &link.sources {
        for path in
            std::iter::once(source.path.as_str()).chain(source.members.iter().map(|member| member.path.as_str()))
        {
            if Path::new(path).components().any(|component| {
                matches!(
                    component.as_os_str().to_str(),
                    Some("Cargo.toml" | "Cargo.toml.orig" | "Cargo.lock")
                )
            }) {
                return Err(format!(
                    "declared source {} requires inert metadata {path}; closure compilation forbids reading it",
                    source.name
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Select one versioned installation with the exact declared compiler bytes, without walking its SDK contents.
fn select_owner(link: &RustFactLink, roots: &[PathBuf]) -> Result<(PathBuf, VirtualToolchainLoaf), Error> {
    let paths = owner_paths(link);
    let mut selected = Vec::new();
    for root in roots {
        let executable = VirtualToolchainLoaf::read(
            root,
            &link.executable.path,
            std::iter::once(link.executable.path.as_str()),
        )?;
        if executable.executable_digest() != link.executable.digest {
            continue;
        }
        oven_store::publisher_owner::publisher_runtime_files(root, &link.executable.runtime_files)?;
        let descriptor = VirtualToolchainLoaf::read(root, &link.executable.path, paths.iter().copied())?;
        if descriptor.executable_digest() == link.executable.digest {
            selected.push((root.clone(), descriptor));
        }
    }
    match selected.len() {
        1 => selected.pop().ok_or_else(|| "selected toolchain disappeared".into()),
        0 => Err(format!(
            "executable owner {} absent; supply INCAN_OVEN_LINK_OWNERS",
            link.executable.owner
        )
        .into()),
        _ => Err("ambiguous executable owner roots".into()),
    }
}

/// Project the exact owner-relative path set shared by native preparation and current-owner revalidation.
fn owner_paths(link: &RustFactLink) -> std::collections::BTreeSet<&str> {
    std::iter::once(link.executable.path.as_str())
        .chain(link.executable.runtime_files.iter().map(|member| member.path.as_str()))
        .chain(link.objects.iter().flat_map(|object| {
            object.arguments.iter().filter_map(|argument| match argument {
                RustFactArgument::Owner { owner } => Some(owner.as_str()),
                _ => None,
            })
        }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{select_owner, validate_source_catalog};
    use oven_model::manifest::RustFactLink;

    /// A pinned cached archive remains usable without its build tools; corrupt output still refuses.
    #[test]
    fn dev7_cached_native_link_needs_no_compiler_installation() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = oven_store::store::OvenStore::new(
            root.path().join("store"),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let source = root.path().join("source.c");
        std::fs::write(&source, b"int shim(void) { return 7; }")?;
        let receipt = oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                root.path(),
                "fixture",
                "1.0.0",
                "fixture-target",
                "fixture-toolchain",
                "debug",
                Vec::new(),
            )
            .with_generated_source("source", &source),
        )?;
        let link: RustFactLink = serde_json::from_value(serde_json::json!({
            "name":"native", "target":"fixture-target",
            "executable":{"name":"cc", "owner":"sha256:absent-installation", "path":"bin/cc", "digest":"sha256:absent-compiler"},
            "library":{"name":"shim", "kind":"static"}
        }))?;
        let archive = root.path().join("libshim.a");
        std::fs::write(&archive, b"immutable native archive")?;
        let bound =
            oven_store::receipt_with_build_unit_input(&receipt, "sdk-link-fact", serde_json::to_string(&link)?)?;
        let entry = store.publish(&oven_store::store::OvenArtifactPublishRequest {
            receipt: bound,
            domain: "adopted-native-link".to_string(),
            kind: oven_store::store::OvenArtifactKind::Engine,
            payload: b"cached publisher receipt".to_vec(),
            materialized_files: vec![oven_store::store::OvenArtifactMaterializedFile {
                source_path: archive,
                relative_path: "libshim.a".to_string(),
            }],
            materialized_directories: Vec::new(),
        })?;
        let unit = super::PreparedUnit {
            binding: serde_json::from_value(
                serde_json::json!({"loaf":"fixture", "version":"1.0.0", "archive_digest":"sha256:source", "domain":"target", "features":[], "target_predicates":[]}),
            )?,
            source_origin: crate::native_loaf::NativeLoafOrigin::Local,
            about: serde_json::Value::Null,
            primary: true,
            _source_lease: None,
            manifest: toml::from_str("")?,
            root: root.path().to_path_buf(),
            build_script: false,
            fact: None,
            fact_out: Vec::new(),
        };
        let context = super::CompileContext {
            rustc: &root.path().join("absent-rustc"),
            target: "fixture-target",
            toolchain: "fixture-toolchain",
            output: root.path(),
            store: &store,
            compiler_digest: "unused".to_string(),
            compiler_executable: "unused".to_string(),
            profile: "debug",
            unit_codegen: &[],
        };
        let product = super::prepare_link(&unit, &context, &receipt, &link, &[])?;
        assert_eq!(product.owner.manifest().identity, entry.identity);
        assert_eq!(product.name, "shim");
        let cached = product.owner.artifact_root().join("libshim.a");
        std::fs::remove_file(&cached)?;
        std::fs::write(&cached, b"corrupt native archive")?;
        assert!(super::prepare_link(&unit, &context, &receipt, &link, &[]).is_err());
        let mut changed = link.clone();
        changed.executable.digest = "sha256:changed-compiler".to_string();
        assert!(super::prepare_link(&unit, &context, &receipt, &changed, &[]).is_err());
        Ok(())
    }

    /// Virtual owner selection must refuse recorded runtime drift instead of adopting the changed bytes.
    #[test]
    fn native_owner_selection_refuses_runtime_drift_before_virtual_admission() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("clang"), b"compiler")?;
        std::fs::write(root.path().join("runtime.so"), b"runtime")?;
        let link: RustFactLink = serde_json::from_value(serde_json::json!({
            "name":"native", "target":"fixture-target",
            "executable":{"name":"cc", "owner":oven_store::digest_bytes(b"recorded owner"), "path":"clang", "digest":oven_store::digest_bytes(b"compiler"),
                "runtime_files":[{"path":"runtime.so", "digest":oven_store::digest_bytes(b"runtime")}]},
            "library":{"name":"shim", "kind":"static"}
        }))?;
        let roots = [root.path().to_path_buf()];
        let (_, original) = select_owner(&link, &roots)?;
        std::fs::write(root.path().join("runtime.so"), b"changed")?;
        let error = select_owner(&link, &roots).err().ok_or("corrupt runtime was adopted")?;
        assert!(error.to_string().contains("runtime file runtime.so digest changed"));
        std::fs::write(root.path().join("runtime.so"), b"runtime")?;
        let (_, restored) = select_owner(&link, &roots)?;
        assert_eq!(original.identity()?, restored.identity()?);
        std::fs::remove_file(root.path().join("runtime.so"))?;
        assert!(select_owner(&link, &roots).is_err());
        Ok(())
    }

    /// Another compiler installation's absent or different runtime does not disqualify the unique matching owner.
    #[test]
    fn native_owner_selection_ignores_nonmatching_runtime_and_refuses_ambiguity()
    -> Result<(), Box<dyn std::error::Error>> {
        let unrelated = tempfile::tempdir()?;
        let matching = tempfile::tempdir()?;
        let duplicate = tempfile::tempdir()?;
        std::fs::write(unrelated.path().join("clang"), b"other compiler")?;
        for root in [matching.path(), duplicate.path()] {
            std::fs::write(root.join("clang"), b"compiler")?;
            std::fs::write(root.join("runtime.so"), b"runtime")?;
        }
        let link: RustFactLink = serde_json::from_value(serde_json::json!({
            "name":"native", "target":"fixture-target",
            "executable":{"name":"cc", "owner":oven_store::digest_bytes(b"recorded owner"), "path":"clang", "digest":oven_store::digest_bytes(b"compiler"),
                "runtime_files":[{"path":"runtime.so", "digest":oven_store::digest_bytes(b"runtime")}]},
            "library":{"name":"shim", "kind":"static"}
        }))?;
        let roots = [unrelated.path().to_path_buf(), matching.path().to_path_buf()];
        assert_eq!(select_owner(&link, &roots)?.0, matching.path());
        std::fs::write(unrelated.path().join("runtime.so"), b"other runtime")?;
        assert_eq!(select_owner(&link, &roots)?.0, matching.path());
        let reversed = [matching.path().to_path_buf(), unrelated.path().to_path_buf()];
        assert_eq!(select_owner(&link, &reversed)?.0, matching.path());
        let ambiguous = [matching.path().to_path_buf(), duplicate.path().to_path_buf()];
        let error = select_owner(&link, &ambiguous)
            .err()
            .ok_or("duplicate matching owners accepted")?;
        assert!(error.to_string().contains("ambiguous executable owner roots"));
        Ok(())
    }

    /// Native fact admission refuses inert manifest reads before trying to resolve or invoke the compiler.
    #[test]
    fn inert_metadata_and_absent_owners_are_named() -> Result<(), Box<dyn std::error::Error>> {
        let mut link: RustFactLink = serde_json::from_str(
            r#"{"name":"shim","target":"aarch64-apple-darwin","executable":{"name":"clang","owner":"sha256:owner","path":"bin/clang","digest":"sha256:compiler"},"objects":[{"name":"shim.o","language":"c","arguments":[{"output":"shim.o"}]}],"sources":[{"name":"source","kind":"file","path":"Cargo.toml","digest":"sha256:source"}],"library":{"name":"shim","kind":"static"}}"#,
        )?;
        let error = validate_source_catalog(&link)
            .err()
            .ok_or("inert metadata was accepted")?;
        assert!(error.to_string().contains("Cargo.toml"));
        link.sources[0].path = "shim.c".to_string();
        validate_source_catalog(&link)?;
        let error = select_owner(&link, &[]).err().ok_or("absent owner was accepted")?;
        assert!(error.to_string().contains("sha256:owner"));
        link.objects.push(link.objects[0].clone());
        assert!(validate_source_catalog(&link).is_err());
        Ok(())
    }

    /// The effective Apple fact names the deployment flag before its cache identity is derived.
    #[test]
    fn apple_native_fact_binds_deployment_policy() -> Result<(), Box<dyn std::error::Error>> {
        let link: oven_model::manifest::RustFactLink = serde_json::from_value(serde_json::json!({
            "name": "native", "target": "aarch64-apple-darwin",
            "executable": {"name": "clang", "owner": "owner", "path": "usr/bin/clang", "digest": "digest"},
            "objects": [{"name": "probe.o", "language": "c", "arguments": [{"output": "probe.o"}]}],
            "library": {"name": "probe", "kind": "static"}
        }))?;
        let apple = super::apple_deployment_link(&link, "aarch64-apple-darwin");
        assert_ne!(serde_json::to_vec(&link)?, serde_json::to_vec(&apple)?);
        assert_eq!(
            apple.objects[0].arguments.last(),
            Some(&super::RustFactArgument::Literal {
                literal: "-mmacosx-version-min=11.0".to_string(),
            })
        );
        assert_eq!(link, super::apple_deployment_link(&link, "x86_64-unknown-linux-gnu"));
        Ok(())
    }
}
