//! Rust metadata for checking: the direct inspection root prepared from the command's original ordinary native
//! producer.
//!
//! There is one route. The configured native graph (see [`crate::build::native_loaf_plan::ConfiguredNativeLoafInputs`])
//! selects the declared Rust roots and their exact forward closure; the inspector reads their sources through the
//! compiler-bound Rust toolchain. Rust's own crates (`std`, `core`, `alloc`) need no external root.
//!
//! The whole module rides the `rust_inspect` feature: without the inspector there is nothing here to prepare.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{CliError, CliResult};
use crate::lock::{
    OvenRustInspectSourceAuthorityRequest, PreparedRustInspectTypecheckWorkspace, PreparedRustInspectWorkspace,
    RustInspectTypecheckRequest, RustInspectWorkspaceRequest,
};
use crate::modules::{build_source_map, collect_rust_dependency_uses, format_dependency_error};
use crate::rust_inspect_workspace::{
    collect_rust_inspect_derive_probe_paths, collect_rust_inspect_query_paths, ensure_direct_rust_inspect_workspace,
    mark_oven_direct_rust_inspection, prewarm_rust_inspect_workspace,
};
use incan_provider::dependency_resolver::resolve_reachable_dependencies;
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};

mod ordinary;

/// Prepare a command's inspection root from its configured ordinary native producer.
///
/// The producer owns graph, profile and source admission; its failures propagate. The returned workspace retains the
/// original producer request and compiler-bound toolchain through semantic consumption, including derive-only
/// requests.
#[cfg(feature = "cli")]
pub fn prepare_rust_inspect_workspace_for_command(
    request: RustInspectWorkspaceRequest<'_>,
    configured: &crate::build::native_loaf_plan::ConfiguredNativeLoafInputs,
    rustc: &Path,
) -> CliResult<Option<PreparedRustInspectWorkspace>> {
    if request.rust_inspect_query_paths.is_empty()
        && request.rust_derive_probe_paths.is_empty()
        && !request.prepare_when_empty
    {
        return Ok(None);
    }
    let observation = configured.prepare_inspection(
        &crate::oven_store::open_default_oven_store()?,
        rustc,
        request.authority.target,
        request.authority.profile,
        &request.cache_dir.join("native-inspection"),
    )?;
    prepare_rust_inspect_workspace_with_ordinary_native(request, observation, rustc, None)
}

/// Prepare an inspection root from an already observed original ordinary producer request.
///
/// A metadata publisher supplies its recipe-bound toolchain owner; independent inspection acquires its own.
pub fn prepare_rust_inspect_workspace_with_ordinary_native(
    request: RustInspectWorkspaceRequest<'_>,
    observation: oven_rustc::native_loaf::NativeLoafRequestObservation,
    rustc: &Path,
    toolchain: Option<std::sync::Arc<oven_rustc::rustc::OvenRustInspectionToolchain>>,
) -> CliResult<Option<PreparedRustInspectWorkspace>> {
    let RustInspectWorkspaceRequest {
        project_root,
        project_name,
        rust_edition,
        cache_dir,
        rust_inspect_query_paths,
        rust_derive_probe_paths,
        prepare_when_empty,
        force_direct_prewarm,
        authority,
    } = request;
    if rust_inspect_query_paths.is_empty() && rust_derive_probe_paths.is_empty() && !prepare_when_empty {
        return Ok(None);
    }
    let manifest_dir = ensure_direct_rust_inspect_workspace(
        project_root,
        project_name,
        rust_edition.as_deref(),
        rust_derive_probe_paths,
        &selection_inputs(&authority)?,
    )?;
    let toolchain = ordinary::prepare(
        &manifest_dir,
        cache_dir,
        project_root,
        &observation,
        rustc,
        &authority,
        rust_inspect_query_paths,
        rust_derive_probe_paths,
        toolchain.as_ref(),
    )?;
    mark_oven_direct_rust_inspection(&manifest_dir)?;
    prewarm_rust_inspect_workspace(&manifest_dir, rust_inspect_query_paths, force_direct_prewarm)?;
    Ok(Some(PreparedRustInspectWorkspace {
        manifest_dir,
        _ordinary_native: observation,
        _ordinary_toolchain: toolchain,
    }))
}

/// Name the inspection root by everything that selects its native closure, so a changed selection gets a new root.
fn selection_inputs(authority: &OvenRustInspectSourceAuthorityRequest<'_>) -> CliResult<BTreeMap<String, String>> {
    let dependencies = serde_json::to_vec(authority.registry_dependencies)
        .map_err(|error| CliError::failure(format!("failed to encode Rust inspection dependencies: {error}")))?;
    Ok(BTreeMap::from([
        ("target".to_string(), authority.target.to_string()),
        ("toolchain".to_string(), authority.toolchain.to_string()),
        ("profile".to_string(), authority.profile.to_string()),
        ("features".to_string(), authority.features.join(",")),
        ("dependencies".to_string(), oven_store::digest_bytes(&dependencies)),
    ]))
}

/// Prepare the inspection root needed before metadata-backed typechecking.
///
/// Ordinary native source admission uses the authored Rust dependency surface: the project's declared Rust
/// dependencies plus inline `rust::` imports and declared derive crates. Inspection needs the configured native
/// graph; a command without one is refused.
pub fn prepare_rust_inspect_typecheck_workspace(
    request: RustInspectTypecheckRequest<'_>,
) -> CliResult<Option<PreparedRustInspectTypecheckWorkspace>> {
    let RustInspectTypecheckRequest {
        project_root,
        project_name,
        manifest,
        modules,
        cargo_features,
        rust_edition,
    } = request;
    let query_paths = collect_rust_inspect_query_paths(modules);
    let derive_probe_paths = collect_rust_inspect_derive_probe_paths(modules);
    // A quoted `@rust.derive("crate::path::Macro")` needs a prepared root for its expansion evidence even when the
    // module imports no other `rust::` items, so probe paths keep this preparation alive on their own.
    if query_paths.is_empty() && derive_probe_paths.is_empty() {
        return Ok(None);
    }

    let inline_imports = modules
        .iter()
        .flat_map(|module| {
            collect_rust_dependency_uses(module, false).into_iter().chain(
                crate::modules::collect_declared_rust_derive_dependency_uses(module, manifest, false),
            )
        })
        .collect::<Vec<_>>();
    let resolved = match resolve_reachable_dependencies(manifest, &inline_imports, true, cargo_features) {
        Ok(resolved) => resolved,
        Err(errors) => {
            let mut msg = String::new();
            let sources = build_source_map(modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let cache_dir = oven_model::lock::compiler_lock_state_dir(project_root).join("rust_inspect_cache");
    let request = RustInspectWorkspaceRequest {
        project_root,
        project_name,
        rust_edition,
        cache_dir: &cache_dir,
        rust_inspect_query_paths: &query_paths,
        rust_derive_probe_paths: &derive_probe_paths,
        prepare_when_empty: false,
        force_direct_prewarm: false,
        authority: OvenRustInspectSourceAuthorityRequest {
            target: &target,
            toolchain: &toolchain,
            profile: "debug",
            features: &cargo_features.cargo_features,
            registry_dependencies: &resolved.dependencies,
        },
    };
    #[cfg(feature = "cli")]
    {
        let Some(configured) = crate::build::native_loaf_plan::ConfiguredNativeLoafInputs::discover()? else {
            return Err(CliError::failure(
                "Rust inspection requires the configured native dependency graph (INCAN_SDK_NATIVE_COMPILER_GRAPH, \
                 INCAN_SDK_NATIVE_INDEX and INCAN_SDK_NATIVE_BLOBS)",
            ));
        };
        let workspace = prepare_rust_inspect_workspace_for_command(request, &configured, &rustc)?;
        Ok(workspace.map(|workspace| PreparedRustInspectTypecheckWorkspace { workspace }))
    }
    #[cfg(not(feature = "cli"))]
    {
        let _ = request;
        Err(CliError::failure(
            "Rust inspection requires the driver's command surface and its configured native dependency graph",
        ))
    }
}
