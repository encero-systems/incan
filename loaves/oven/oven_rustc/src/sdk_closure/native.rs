//! Native-link facts at the explicit adopted-closure publisher boundary.

use std::path::{Path, PathBuf};

use oven_model::manifest::{RustFactArgument, RustFactLink};
use oven_store::process::BoundedProcessLimits;
use oven_store::publisher_owner::publisher_owner_identity;
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

/// Bind a native archive cache key to the fact, admitted source archive, and verified executable owner closure.
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
    let owner = select_owner(link, roots)?;
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
    let selected = context.store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::Engine
            && manifest.domain == domain
            && manifest.receipt_identity == receipt.identity
    })?;
    if let Some(owner) = selected.into_iter().next() {
        let _verified = context.store.select(&owner.manifest.identity)?;
        return Ok(NativeProduct {
            name: link.library.name.clone(),
            owner,
        });
    }
    let staging = tempfile::Builder::new().prefix("native-").tempdir_in(context.output)?;
    let product = bake_publisher_link(&OvenPublisherLinkBakeRequest {
        link,
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
fn validate_source_catalog(link: &RustFactLink) -> Result<(), Error> {
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

/// Select exactly one supplied tool owner whose complete declared closure matches the fact identity.
fn select_owner(link: &RustFactLink, roots: &[PathBuf]) -> Result<PathBuf, Error> {
    let paths: std::collections::BTreeSet<_> = std::iter::once(link.executable.path.as_str())
        .chain(link.objects.iter().flat_map(|object| {
            object.arguments.iter().filter_map(|argument| match argument {
                RustFactArgument::Owner { owner } => Some(owner.as_str()),
                _ => None,
            })
        }))
        .collect();
    let mut selected = Vec::new();
    for root in roots {
        if publisher_owner_identity(root, paths.iter().copied())? == link.executable.owner {
            selected.push(root.clone());
        }
    }
    match selected.as_slice() {
        [root] => Ok(root.clone()),
        [] => Err(format!(
            "executable owner {} absent; supply INCAN_OVEN_LINK_OWNERS",
            link.executable.owner
        )
        .into()),
        _ => Err("ambiguous executable owner roots".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{select_owner, validate_source_catalog};
    use oven_model::manifest::RustFactLink;

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
