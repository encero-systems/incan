//! Selecting the receipt-bound plan a command executes against, and the test-dependency envelope a test run adds to it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::backend::ProjectGenerator;
use crate::build::oven_project::{
    bake_generated_project_compatibility_plan, project_extension_base_loaf, remove_completed_generated_cargo_lock,
    select_oven_direct_rustc_plan, select_oven_direct_rustc_plan_with_materialization,
};
use crate::build::package_loafs::import_checked_packaged_library_loaf;
use crate::build::plan_authority::CallerOwnedProviderRegistryClosure;
use crate::build::provider_compilation::checked_test_dependency_package_profiles;
use crate::build::{
    OvenDirectRustcPlanPreparation, OvenProjectBakeAuthorityContext, OvenProjectDependencySurface, OvenProjectPlanMode,
    OvenToolchainMaterialization, PreparedOvenTestDependencyEnvelope,
};
use crate::build_unit::promoted_oven_test_dependencies;
use crate::error::{CliError, CliResult, oven_plan_error, oven_rustc_error};
use incan_lang::version::INCAN_VERSION;
use incan_provider::dependency_resolver::ResolvedDependencies;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::interop::OVEN_INTEROP_EXECUTION_RECEIPT_INPUT;
use oven_rustc::legacy_cargo::cargo_process::resolved_cargo_executable;
use oven_rustc::legacy_cargo::{
    OvenLegacyCargoBaseLoaf, OvenLegacyCargoDirectDependencyClosure, OvenLegacyCargoPrepareRequest,
    OvenLegacyCargoPublicationKind, direct_rustc_reusable_project_plan_environment, prepare_direct_rustc_plan,
    stage_locked_loaf_fixture,
};
use oven_rustc::loaf::{
    OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT, OvenToolchainLoaf,
    resolve_compiler_owned_loaf_for_registry_dependencies,
};
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_rustc::plan::selection::{
    select_receipt_direct_rustc_execution_plan, select_receipt_project_extension_execution_plan,
};
use oven_rustc::rustc::{OvenRegistryLeafAuthority, OvenRustcArtifactPlan, resolve_active_rustc};
use oven_store::store::OvenStore;
use oven_store::{OvenGeneratedProjectRequest, digest_dependency_specs, receipt_generated_project};

/// Remove public package-provider roots from the Cargo-published test delta without narrowing project authority.
///
/// The caller retains `dependencies` unchanged for the singular inspection authority. A validated package Loaf owns
/// its public direct-Rustc library and complete Rust closure, so publishing the same path root in the synthetic test
/// constituent would attach two independently materialized externs with one Rust-facing alias.
fn test_dependency_publisher_dependencies(
    dependencies: &[DependencySpec],
    packaged_provider_aliases: &BTreeSet<String>,
) -> Vec<DependencySpec> {
    dependencies
        .iter()
        .filter(|dependency| {
            let alias = dependency.crate_name.replace('-', "_");
            !packaged_provider_aliases.contains(&alias)
        })
        .cloned()
        .collect()
}

/// Preserve normal/dev ownership labels while applying Cargo's canonical feature union to duplicate root aliases.
pub fn canonical_project_inspection_dependencies(
    resolved: &ResolvedDependencies,
) -> CliResult<(Vec<DependencySpec>, Vec<DependencySpec>)> {
    let promoted = promoted_oven_test_dependencies(resolved)?;
    let select = |dependencies: &[DependencySpec]| {
        dependencies
            .iter()
            .map(|dependency| {
                promoted
                    .iter()
                    .find(|candidate| candidate.crate_name == dependency.crate_name)
                    .cloned()
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "canonical project inspection dependency `{}` disappeared during promotion",
                            dependency.crate_name
                        ))
                    })
            })
            .collect::<CliResult<Vec<_>>>()
    };
    Ok((select(&resolved.dependencies)?, select(&resolved.dev_dependencies)?))
}

/// Digest each promoted dependency independently so generated test batches can prove an exact subset later.
fn oven_test_dependency_root_digests(dependencies: &[DependencySpec]) -> CliResult<BTreeMap<String, String>> {
    let mut roots = BTreeMap::new();
    for dependency in dependencies {
        let alias = dependency.crate_name.replace('-', "_");
        let digest = digest_dependency_specs(
            std::slice::from_ref(dependency),
            incan_oven_facet::provider_hooks().as_ref(),
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
        if roots.insert(alias.clone(), digest).is_some() {
            return Err(CliError::failure(format!(
                "test dependency surface contains duplicate Rust-facing alias `{alias}`"
            )));
        }
    }
    Ok(roots)
}

/// Return whether a prepared debug target already binds the exact non-package dependency delta for tests.
fn debug_target_receipt_covers_test_publisher_dependencies(
    receipt: &oven_store::OvenReceipt,
    dependency_surface_digest: &str,
) -> bool {
    receipt.intent.profile == "debug"
        && receipt.compatibility.kind == oven_store::OvenCompatibilityKind::GeneratedIncanProject
        && receipt
            .sources
            .build_unit_inputs
            .get("rust-dependencies")
            .is_some_and(|digest| digest == dependency_surface_digest)
}

/// Publish the one project-owned test dependency delta through Cargo's explicit compatibility boundary.
fn bake_generated_project_test_dependency_plan(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    generated_project: &Path,
    generated_root: &Path,
    rustc: &Path,
    base_loaf: Option<&OvenToolchainLoaf>,
) -> CliResult<OvenToolchainMaterialization> {
    let compile_environment = direct_rustc_reusable_project_plan_environment(generated_project, generated_root)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let publication = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
        compiler: incan_oven_facet::compiler_identity(),
        provider_hooks: incan_oven_facet::provider_hooks(),
        store,
        receipt: receipt.clone(),
        generated_project: generated_project.to_path_buf(),
        cargo: resolved_cargo_executable()
            .map_err(|error| CliError::failure(format!("cannot resolve Cargo for explicit Oven bake: {error}")))?,
        rustc: rustc.to_path_buf(),
        sdk_inventory: None,
        compiler_loaf_root: None,
        domain: format!("incan-release-{INCAN_VERSION}"),
        publication_kind: OvenLegacyCargoPublicationKind::Executable,
        source_evidence_key: "generated-root".to_string(),
        compile_environment,
        inspection_packages: None,
        direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::CheckedDeclared,
        provider_compilations: &[],
        compact_debug_info: true,
        source_compiler_vocab_support: false,
        base_loaf: base_loaf.map(|base| OvenLegacyCargoBaseLoaf {
            loaf_identity: base.loaf_identity.clone(),
            build_unit_identity: base.loaf_build_unit_identity.clone(),
            artifacts: &base.artifacts,
            artifact_root: &base.artifact_root,
        }),
    })
    .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(if publication.cargo_version == "not-run-existing-plan" {
        OvenToolchainMaterialization::Reused
    } else {
        OvenToolchainMaterialization::CompatibilityBaked
    })
}

/// Prepare one debug-only dependency envelope from the same whole-project graph used by `incan lock`.
///
/// The generated root is intentionally stable and contains no authored test code. Package-provider roots remain in
/// the singular authority but are supplied by their separately validated Loafs. A compiler-shipped release Loaf is
/// returned directly when it covers the remaining selected surface; only a genuine third-party/path delta crosses
/// the explicit Cargo baker, and it does so with `build --locked --offline` through the executable publisher.
pub fn prepare_oven_test_dependency_envelope(
    store: &OvenStore,
    project_root: &Path,
    resolved: &ResolvedDependencies,
    debug_target_receipts: &[oven_store::OvenReceipt],
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<PreparedOvenTestDependencyEnvelope> {
    let dependencies = promoted_oven_test_dependencies(resolved)?;
    let dependency_surface_digest = digest_dependency_specs(&dependencies, incan_oven_facet::provider_hooks().as_ref())
        .map_err(|error| CliError::failure(error.to_string()))?;
    let dependency_root_digests = oven_test_dependency_root_digests(&dependencies)?;
    let base_receipt = debug_target_receipts.first().ok_or_else(|| {
        CliError::failure("explicit Oven project bake prepared no debug target receipt for its test dependency surface")
    })?;
    let checked_package_profiles =
        checked_test_dependency_package_profiles(&dependencies, base_receipt, authority_context)?;
    for checked in &checked_package_profiles {
        import_checked_packaged_library_loaf(store, checked)?;
    }
    let packaged_provider_aliases = checked_package_profiles
        .iter()
        .map(|checked| checked.dependency_key.replace('-', "_"))
        .collect::<BTreeSet<_>>();
    let publisher_dependencies = test_dependency_publisher_dependencies(&dependencies, &packaged_provider_aliases);
    let publisher_dependency_surface_digest =
        digest_dependency_specs(&publisher_dependencies, incan_oven_facet::provider_hooks().as_ref())
            .map_err(|error| CliError::failure(error.to_string()))?;
    for receipt in debug_target_receipts {
        let covers =
            debug_target_receipt_covers_test_publisher_dependencies(receipt, &publisher_dependency_surface_digest);
        tracing::debug!(
            "test dependency envelope: debug target receipt {} kind={:?} rust-dependencies={:?} publisher-surface={} covers={covers}",
            receipt.identity,
            receipt.compatibility.kind,
            receipt.sources.build_unit_inputs.get("rust-dependencies"),
            publisher_dependency_surface_digest
        );
        if !covers {
            continue;
        }
        let Some(plan_selection) = select_oven_direct_rustc_plan(store, receipt, &publisher_dependencies)? else {
            tracing::debug!(
                "test dependency envelope: receipt {} covers the surface but selects no stored plan",
                receipt.identity
            );
            continue;
        };
        if matches!(
            &plan_selection,
            OvenDirectRustcPlanSelection::Stored(_)
                | OvenDirectRustcPlanSelection::ToolchainLoaf(_)
                | OvenDirectRustcPlanSelection::ProjectExtension(_)
        ) {
            return Ok(PreparedOvenTestDependencyEnvelope {
                receipt: receipt.clone(),
                dependency_surface_digest,
                dependencies,
                dependency_root_digests,
                plan_selection,
            });
        }
    }
    let generated_project = project_root
        .join("target")
        .join("incan")
        .join("oven")
        .join("test-dependency-envelope");
    let mut generator = ProjectGenerator::new(&generated_project, "incan_test_dependency_envelope", true);
    generator.set_package_metadata(Some(INCAN_VERSION.to_string()), None);
    generator.set_dependencies(publisher_dependencies.clone());
    generator.set_dev_dependencies(Vec::new());
    generator
        .generate("fn main() {}\n")
        .map_err(|error| CliError::failure(format!("failed to generate Oven test dependency envelope: {error}")))?;
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let mut receipt_request = OvenGeneratedProjectRequest::new(
        project_root,
        "incan-test-dependency-envelope",
        INCAN_VERSION,
        base_receipt.intent.target.clone(),
        base_receipt.intent.toolchain.clone(),
        "debug",
        base_receipt.intent.features.clone(),
    )
    .with_generated_source("generated-root", generator.crate_root_path())
    .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
    for (name, value) in &base_receipt.sources.build_unit_inputs {
        receipt_request = receipt_request.with_build_unit_input(name.clone(), value.clone());
    }
    receipt_request = receipt_request.with_build_unit_input("rust-dependencies", publisher_dependency_surface_digest);
    let receipt = receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))?;
    let plan_selection = if publisher_dependencies
        .iter()
        .all(|dependency| matches!(dependency.source, DependencySource::Registry))
        && let Some(loaf) = resolve_compiler_owned_loaf_for_registry_dependencies(&receipt, &publisher_dependencies)
            .map_err(|error| CliError::failure(error.to_string()))?
    {
        OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(loaf))
    } else {
        let base_loaf = project_extension_base_loaf(&receipt)?;
        let materialization = bake_generated_project_test_dependency_plan(
            store,
            &receipt,
            generator.output_dir(),
            &generator.crate_root_path(),
            &rustc,
            base_loaf.as_ref(),
        )?;
        select_published_project_plan(store, &receipt, materialization)?
            .ok_or_else(|| {
                CliError::failure(
                    "the explicit Oven project bake completed without its checked test dependency envelope",
                )
            })?
            .plan_selection
    };
    remove_completed_generated_cargo_lock(generator.output_dir())?;
    Ok(PreparedOvenTestDependencyEnvelope {
        receipt,
        dependency_surface_digest,
        dependencies,
        dependency_root_digests,
        plan_selection,
    })
}

/// Select a plan for an explicit project bake, reusing only an exact project Loaf before publishing once.
pub fn select_or_bake_generated_project_plan(
    mode: OvenProjectPlanMode,
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    dependency_surface: OvenProjectDependencySurface<'_>,
    generated_project: &Path,
    generated_root: &Path,
    rustc: &Path,
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    if receipt_requires_final_interop_plan(receipt) {
        return select_published_project_plan(store, receipt, OvenToolchainMaterialization::Reused)?.map_or_else(
            || Err(interop_final_plan_required_error()),
            |selection| Ok(Some(selection)),
        );
    }
    let source_compiler_vocab_support = receipt
        .sources
        .build_unit_inputs
        .get(OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT)
        .is_some_and(|value| value == "v1");
    if mode.is_explicit_publisher() {
        // The completed project-output Loaf owns generated project sources and the final native result. Do not bake
        // an empty project extension when the installed release Loaf already supplies the complete native dependency
        // closure: that would duplicate release-owned bytes without adding project authority.
        if let Some(selected) =
            select_published_project_extension_plan(store, receipt, OvenToolchainMaterialization::Reused)?
        {
            return Ok(Some(selected));
        }
        if mode == OvenProjectPlanMode::ExplicitBake
            && dependency_surface
                .selection
                .iter()
                .all(|dependency| matches!(dependency.source, DependencySource::Registry))
            && let Some(loaf) =
                resolve_compiler_owned_loaf_for_registry_dependencies(receipt, dependency_surface.selection)
                    .map_err(|error| CliError::failure(error.to_string()))?
        {
            return Ok(Some(OvenDirectRustcPlanPreparation {
                plan_selection: OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(loaf)),
                materialization: OvenToolchainMaterialization::ToolchainLoaf,
                cargo_process_started: false,
            }));
        }
        let bootstrap_lock_seeded = if mode == OvenProjectPlanMode::InteropBootstrap {
            // The bootstrap has no caller-owned Rust registry inputs. Seed its generated manifest from the checked
            // compiler lock, normalize the local path records offline, and make the later compatibility build
            // unconditionally locked. That closes the first-plan loop without turning native interop into ambient
            // Cargo or network discovery. The lock is resolved through the toolchain layout: an installed release
            // carries it below `crates/Cargo.lock`, and only a development checkout keeps it at the workspace root.
            let compiler_lock = oven_model::toolchain_layout::resolve_toolchain_runtime_lockfile();
            let cargo = resolved_cargo_executable()
                .map_err(|error| CliError::failure(format!("cannot resolve Cargo for interop bootstrap: {error}")))?;
            stage_locked_loaf_fixture(&cargo, generated_project, &compiler_lock).map_err(|error| {
                CliError::failure(format!("could not seed the interop bootstrap Cargo.lock: {error}"))
            })?;
            true
        } else {
            false
        };
        // A direct-C bootstrap must publish a project-owned base plan even when a compiler Loaf could otherwise
        // satisfy the Rust closure. `oven interop bake` extends that exact stored plan with the locked native
        // search paths and runtime bundles; a Loaf selected outside this store would leave no base artifact to
        // extend, and would reintroduce the circular "link before sealed" failure.
        let base_loaf = project_extension_base_loaf(receipt)?;
        let materialization = bake_generated_project_compatibility_plan(
            store,
            receipt,
            generated_project,
            generated_root,
            rustc,
            base_loaf.as_ref(),
            source_compiler_vocab_support,
            if mode == OvenProjectPlanMode::InteropBootstrap {
                OvenLegacyCargoPublicationKind::InteropBootstrap
            } else {
                OvenLegacyCargoPublicationKind::Executable
            },
            dependency_surface.provider_compilations,
        )?;
        let mut prepared = select_published_project_plan(store, receipt, materialization)?.ok_or_else(|| {
            CliError::failure("the explicit Oven project bake completed without a receipt-compatible direct-rustc plan")
        })?;
        prepared.cargo_process_started =
            bootstrap_lock_seeded || materialization == OvenToolchainMaterialization::CompatibilityBaked;
        return Ok(Some(prepared));
    }
    select_oven_direct_rustc_plan_with_materialization(store, receipt, dependency_surface.selection)
}

/// Return whether this receipt requires an exact final native interop plan rather than a base Loaf.
pub fn receipt_requires_final_interop_plan(receipt: &oven_store::OvenReceipt) -> bool {
    receipt
        .sources
        .build_unit_inputs
        .contains_key(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT)
}

/// Return the actionable fail-closed error for a selected interop receipt without its final native plan.
pub fn interop_final_plan_required_error() -> CliError {
    CliError::failure(
        "Oven interop has a selected execution receipt but no matching final native direct-Rustc plan. Run `incan oven interop bake` for this locked target; normal build and run will not materialize a generic Loaf, discover native tools, or invoke Cargo.",
    )
}

/// Select one receipt-bound project-extension publication from the bounded store.
///
/// An extension is the unambiguous representation of a caller project: it records both its complete Cargo-published
/// closure and the exact installed standard-library base it deliberately partitions from.  A plain direct-Rustc
/// plan can instead be a broad compiler Loaf with coincidentally matching build-unit inputs, so an explicit project
/// bake must never treat one as evidence that its caller closure has already been published.
fn select_published_project_extension_plan(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    materialization: OvenToolchainMaterialization,
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    Ok(select_receipt_project_extension_execution_plan(store, receipt, None)
        .map_err(oven_plan_error)?
        .map(|selected| OvenDirectRustcPlanPreparation {
            plan_selection: OvenDirectRustcPlanSelection::ProjectExtension(Box::new(selected)),
            materialization,
            cargo_process_started: false,
        }))
}

/// Select either receipt-bound project publication form from the bounded store.
///
/// Current project Loafs are extensions and win over a plain direct plan. Older self-contained project Loafs can
/// still run during the migration, but only after the unambiguous extension form was absent. This preserves valid
/// older local work while ensuring an installed compiler Loaf can never shadow a newly baked caller closure.
pub fn select_published_project_plan(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    materialization: OvenToolchainMaterialization,
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    if let Some(selected) = select_published_project_extension_plan(store, receipt, materialization)? {
        return Ok(Some(selected));
    }
    Ok(select_receipt_direct_rustc_execution_plan(store, receipt)
        .map_err(oven_plan_error)?
        .map(|selected| OvenDirectRustcPlanPreparation {
            plan_selection: OvenDirectRustcPlanSelection::Stored(Box::new(selected)),
            materialization,
            cargo_process_started: false,
        }))
}

/// Render the registry requirements that made sealed Loaf selection impossible.
///
/// Oven does not invoke Cargo to diagnose an unavailable registry version, so this preserves the manifest-level
/// dependency identity that a user must correct instead of returning an opaque Loaf-selection failure.
pub fn format_oven_registry_dependency_requirements(dependencies: &[DependencySpec]) -> String {
    let mut requirements = dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .map(|dependency| {
            let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
            let version = dependency.version.as_deref().unwrap_or("<missing version>");
            format!("`{package}` `{version}`")
        })
        .collect::<Vec<_>>();
    requirements.sort();
    requirements.dedup();
    if requirements.is_empty() {
        "none".to_string()
    } else {
        requirements.join(", ")
    }
}

/// Return the registry catalog copied with the active, receipt-selected direct-rustc plan.
///
/// A registry artifact's metadata is valid only with the feature-unified compatibility domain that published it.
/// Normal selection therefore chooses a Loaf that covers every caller-visible registry root, then resolves only the
/// catalog sealed into that one leased plan—never an aggregate Cargo cache or a second Loaf's dependency directory.
pub fn registry_leaf_authority_for_plan_selection(
    selection: &OvenDirectRustcPlanSelection,
) -> CliResult<Option<OvenRegistryLeafAuthority>> {
    Ok(selection.registry_leaf_authority())
}

/// Detect whether a caller-owned provider's own registry closure would silently link a second, incompatible
/// compiled instance of a package `plan` already links explicitly, returning the first such package.
///
/// Linking a provider's own registry-resolved package (for example an async runtime a query-engine provider pulls
/// in through its own dependency graph) alongside the SDK/consumer's own separately compiled copy of that same
/// package is a real, reproduced defect, not a theoretical one: it produced a runtime panic ("no reactor running")
/// from two distinct compiled `tokio` instances silently linked into one binary, discovered only by inspecting the
/// linked executable's own symbol table after the build otherwise succeeded. Properly unifying a provider's
/// independently Cargo-resolved registry closure with the consumer's own is out of scope for Oven Alpha's
/// direct-rustc execution (#1241). A bake that hits this shape refuses ([`oven_native_closure_refusal`]); there is
/// no Cargo fallback.
pub fn caller_owned_provider_registry_conflict(
    consumer_authority: Option<&OvenRegistryLeafAuthority>,
    closure: &CallerOwnedProviderRegistryClosure,
    plan: &OvenRustcArtifactPlan,
) -> CliResult<Option<(String, Option<PathBuf>)>> {
    for provider_authority in &closure.provider_authorities {
        // A shared package can enter both closures transitively without ever being a named extern of either
        // compile (the reproduced `tokio` duplication was exactly this shape), so the catalogs themselves are
        // compared first; the extern comparison then covers packages the selected plan links directly.
        if let Some(consumer_authority) = consumer_authority
            && let Some((package, pinned_by)) =
                consumer_authority.first_diverging_shared_package_pin(provider_authority)
        {
            return Ok(Some((package, Some(pinned_by))));
        }
        if let Some(package) = provider_authority
            .first_conflicting_package_with(plan)
            .map_err(oven_rustc_error)?
        {
            return Ok(Some((package, None)));
        }
    }
    Ok(None)
}

/// Describe one provider registry conflict for a refusal, naming the contributor that pins the package.
///
/// "Two copies of `itoa` exist" leaves a reader with nowhere to go; "this prebuilt provider was compiled against
/// that copy" says what would have to change. The distinction is also the boundary of the unimplemented capability:
/// a leaf can be reconciled wherever every dependent linking it is recompiled against the choice, and a provider
/// consumed from the store as an already-compiled artifact is exactly the case that cannot be (#1241).
pub fn provider_registry_conflict_reason(package: &str, pinned_by: Option<&Path>) -> String {
    match pinned_by {
        Some(root) => format!(
            "a caller-owned provider's own registry closure resolves `{package}` to a different compiled artifact \
             than this project's own closure already links, and `{package}` is pinned by an already-compiled provider \
             artifact at `{}`, which would have to be rebuilt against the reconciled closure to agree",
            root.display()
        ),
        None => format!(
            "a caller-owned provider's own registry closure resolves `{package}` to a different compiled artifact \
             than this project's own closure already links, and `{package}` is already linked by this project's own \
             selected plan"
        ),
    }
}

/// Refuse the one build shape direct-rustc composition cannot finish yet, naming it exactly.
///
/// Oven never launches Cargo during a normal command, and there is no fallback to declare: a project that hits this
/// shape waits for the Oven-native reconciliation (#1241, one compiled instance of every shared registry package,
/// every dependent relinked against it) or restructures so the shape does not arise.
pub fn oven_native_closure_refusal(crate_name: &str, reason: &str) -> CliError {
    CliError::failure(format!(
        "Oven refuses to build `{crate_name}`: {reason}. Linking both would silently admit two incompatible compiled \
         instances of the same crate into one binary -- for a crate that carries process-wide runtime state (most \
         dangerously an async runtime), this can produce a runtime panic instead of a build failure. Oven does not \
         reconcile this shape through direct rustc yet (#1241) and never falls back to Cargo; prepare an explicit \
         Oven-native closure that reconciles the shared package to one compiled artifact, or consume the provider \
         from source rather than as a sealed packaged closure."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::{env, fs};

    use crate::backend::ProjectGenerator;
    use crate::build::output_paths::project_inspection_test_dependency_roots;
    use crate::build::{OvenProjectDependencySurface, OvenProjectPlanMode, OvenToolchainMaterialization};
    use crate::build_unit::promoted_oven_test_dependencies;
    use incan_provider::dependency_resolver::ResolvedDependencies;
    use oven_model::manifest::{DependencySource, DependencySpec};
    use oven_rustc::interop::OVEN_INTEROP_EXECUTION_RECEIPT_INPUT;
    use oven_rustc::loaf::{OVEN_NESTED_DEPENDENCY_MISS_SUMMARY, OVEN_NO_IMPLICIT_DEPENDENCY_BUILD};
    use oven_rustc::plan::OvenDirectRustcPlanSelection;
    use oven_rustc::rustc::{
        OvenProjectInspectionRootDependency, OvenProjectInspectionTestDependencyRoot, OvenRustcArtifactManifest,
        resolve_active_rustc, rustc_host_target, rustc_identity,
    };
    use oven_store::store::{OvenArtifactKind, OvenArtifactPublishRequest, OvenStore};
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

    /// The registry-closure refusal names the package and the artifact that pinned it, and its wording never
    /// points the reader at Cargo or a compatibility mode.
    #[test]
    fn a_closure_refusal_names_the_package_the_pinning_artifact_and_never_offers_cargo() {
        let pinned = provider_registry_conflict_reason("tokio", Some(Path::new("/store/entries/x/artifacts")));
        assert!(pinned.contains("`tokio`"));
        assert!(pinned.contains("/store/entries/x/artifacts"));
        let linked = provider_registry_conflict_reason("tokio", None);
        assert!(linked.contains("already linked by this project's own selected plan"));
        let refusal = oven_native_closure_refusal("app", &pinned).to_string();
        assert!(refusal.contains("Oven refuses to build `app`"));
        assert!(refusal.contains("never falls back to Cargo"));
        assert!(!refusal.to_lowercase().contains("cargo-compatibility"));
    }

    #[test]
    fn test_dependency_envelope_promotes_aliases_features_and_paths_into_dependencies()
    -> Result<(), Box<dyn std::error::Error>> {
        let path_package = tempfile::tempdir()?;
        fs::write(
            path_package.path().join("Cargo.toml"),
            "[package]\nname = \"fixture_path\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::create_dir(path_package.path().join("src"))?;
        fs::write(path_package.path().join("src/lib.rs"), "pub fn value() {}\n")?;
        let normal = DependencySpec {
            crate_name: "json_api".to_string(),
            version: Some("1".to_string()),
            features: vec!["preserve_order".to_string()],
            default_features: false,
            source: DependencySource::Registry,
            optional: true,
            package: Some("serde_json".to_string()),
        };
        let mut dev = normal.clone();
        dev.features = vec!["raw_value".to_string()];
        dev.default_features = true;
        let path = DependencySpec {
            crate_name: "fixture_alias".to_string(),
            version: Some("0.1.0".to_string()),
            features: vec!["testing".to_string()],
            default_features: false,
            source: DependencySource::Path {
                path: path_package.path().to_path_buf(),
            },
            optional: true,
            package: Some("fixture_path".to_string()),
        };

        let promoted = promoted_oven_test_dependencies(&ResolvedDependencies {
            dependencies: vec![normal],
            dev_dependencies: vec![dev, path],
        })?;
        assert_eq!(promoted.len(), 2);
        let json = promoted
            .iter()
            .find(|dependency| dependency.crate_name == "json_api")
            .ok_or("renamed registry dependency disappeared")?;
        assert_eq!(json.package.as_deref(), Some("serde_json"));
        assert_eq!(json.features, ["preserve_order", "raw_value"]);
        assert!(json.default_features);
        assert!(!json.optional);
        let path = promoted
            .iter()
            .find(|dependency| dependency.crate_name == "fixture_alias")
            .ok_or("path dependency disappeared")?;
        assert!(matches!(path.source, DependencySource::Path { .. }));
        assert!(!path.optional);
        let root_digests = oven_test_dependency_root_digests(&promoted)?;
        let locked_json = OvenProjectInspectionRootDependency {
            alias: "json_api".to_string(),
            package: "serde_json".to_string(),
            version: "1.0.140".to_string(),
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "serde-json-checksum".to_string(),
            requested_features: vec!["preserve_order".to_string(), "raw_value".to_string()],
            default_features: true,
        };
        let exact_roots = project_inspection_test_dependency_roots(
            &promoted,
            &root_digests,
            std::slice::from_ref(&locked_json),
            &[],
        )?;
        assert!(matches!(
            exact_roots.get("json_api"),
            Some(OvenProjectInspectionTestDependencyRoot::Registry { locked, .. }) if locked == &locked_json
        ));
        assert!(matches!(
            exact_roots.get("fixture_alias"),
            Some(OvenProjectInspectionTestDependencyRoot::Path { .. })
        ));

        let generated = tempfile::tempdir()?;
        let mut generator = ProjectGenerator::new(generated.path(), "dependency_envelope", true);
        generator.set_dependencies(promoted.clone());
        generator.set_dev_dependencies(Vec::new());
        generator.generate("fn main() {}\n")?;
        let manifest = fs::read_to_string(generated.path().join("Cargo.toml"))?;
        assert!(manifest.contains("[dependencies.json_api]"));
        assert!(manifest.contains("package = \"serde_json\""));
        assert!(manifest.contains("features = [\"preserve_order\", \"raw_value\"]"));
        assert!(manifest.contains("[dependencies.fixture_alias]"));
        assert!(manifest.contains("package = \"fixture_path\""));
        assert!(!manifest.contains("[dev-dependencies"));

        fs::write(path_package.path().join("src/lib.rs"), "pub fn changed() {}\n")?;
        let changed_root_digests = oven_test_dependency_root_digests(&promoted)?;
        assert_ne!(
            root_digests.get("fixture_alias"),
            changed_root_digests.get("fixture_alias")
        );
        Ok(())
    }

    #[test]
    fn packaged_provider_root_stays_authoritative_without_entering_test_dependency_publisher_issue951()
    -> Result<(), Box<dyn std::error::Error>> {
        let provider = tempfile::tempdir()?;
        fs::create_dir(provider.path().join("src"))?;
        fs::write(
            provider.path().join("Cargo.toml"),
            "[package]\nname = \"set_library\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(provider.path().join("src/lib.rs"), "pub fn unique() {}\n")?;
        let provider_dependency = DependencySpec {
            crate_name: "set_library".to_string(),
            version: Some("0.1.0".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: provider.path().to_path_buf(),
            },
            optional: false,
            package: None,
        };
        let serde = DependencySpec {
            crate_name: "serde".to_string(),
            version: Some("1".to_string()),
            features: vec!["derive".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let promoted = promoted_oven_test_dependencies(&ResolvedDependencies {
            dependencies: vec![provider_dependency],
            dev_dependencies: vec![serde],
        })?;
        let packaged_provider_aliases = BTreeSet::from(["set_library".to_string()]);
        let publisher_dependencies = test_dependency_publisher_dependencies(&promoted, &packaged_provider_aliases);

        assert!(promoted.iter().any(|dependency| dependency.crate_name == "set_library"));
        assert!(
            oven_test_dependency_root_digests(&promoted)?.contains_key("set_library"),
            "the singular project authority must retain the provider root"
        );
        assert!(
            publisher_dependencies
                .iter()
                .all(|dependency| dependency.crate_name != "set_library")
        );
        assert!(
            publisher_dependencies
                .iter()
                .any(|dependency| dependency.crate_name == "serde")
        );
        assert!(
            publisher_dependencies
                .iter()
                .map(|dependency| dependency.crate_name.replace('-', "_"))
                .all(|alias| !packaged_provider_aliases.contains(&alias)),
            "the Cargo-published test delta and sealed package-provider externs must be disjoint"
        );
        Ok(())
    }

    #[test]
    fn prepared_debug_target_reuse_requires_exact_generated_dependency_surface()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated/src/main.rs");
        fs::create_dir_all(generated.parent().ok_or("generated source has no parent")?)?;
        fs::write(&generated, "fn main() {}\n")?;
        let exact_digest = digest_bytes(b"canonical normal+dev dependency surface");
        let debug_request = OvenGeneratedProjectRequest::new(
            project.path(),
            "fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc 1.96.0",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &generated)
        .with_build_unit_input("rust-dependencies", exact_digest.clone());
        let debug = receipt_generated_project(&debug_request)?;
        assert!(debug_target_receipt_covers_test_publisher_dependencies(
            &debug,
            &exact_digest
        ));
        assert!(!debug_target_receipt_covers_test_publisher_dependencies(
            &debug,
            &digest_bytes(b"stale surface")
        ));

        let missing = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        assert!(!debug_target_receipt_covers_test_publisher_dependencies(
            &missing,
            &exact_digest
        ));

        let mut wrong_kind = debug.clone();
        wrong_kind.compatibility.kind = oven_store::OvenCompatibilityKind::FrozenCargoPackage;
        assert!(!debug_target_receipt_covers_test_publisher_dependencies(
            &wrong_kind,
            &exact_digest
        ));

        let release = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated)
            .with_build_unit_input("rust-dependencies", exact_digest.clone()),
        )?;
        assert!(!debug_target_receipt_covers_test_publisher_dependencies(
            &release,
            &exact_digest
        ));
        Ok(())
    }

    #[test]
    fn normal_oven_consumer_selects_a_sealed_final_interop_plan() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated/src/main.rs");
        fs::create_dir_all(generated.parent().ok_or("generated source parent missing")?)?;
        fs::write(&generated, "fn main() {}\n")?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-final-consumer",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated)
            .with_build_unit_input(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, "sha256:selected-native-execution")
            .with_build_unit_input(oven_rustc::interop::OVEN_INTEROP_PLAN_SCHEMA_INPUT, "5"),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "interop-final-consumer".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&OvenRustcArtifactManifest {
                schema_version: oven_rustc::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: receipt.intent.clone(),
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_dependency_search_paths: Default::default(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            })?,
            materialized_files: Vec::new(),
        })?;

        let selected = select_or_bake_generated_project_plan(
            OvenProjectPlanMode::ConsumeOnly,
            &store,
            &receipt,
            OvenProjectDependencySurface {
                selection: &[],
                provider_compilations: &[],
            },
            project.path(),
            &generated,
            &PathBuf::from("/usr/bin/rustc"),
        )?
        .ok_or("normal Oven consumer did not select its exact final interop plan")?;
        assert!(matches!(
            selected.plan_selection,
            OvenDirectRustcPlanSelection::Stored(_)
        ));
        assert!(!selected.cargo_process_started);
        Ok(())
    }

    #[test]
    fn explicit_project_bake_publishes_a_generated_receipt_closure() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated_root = project.path().join("src/main.rs");
        let dependency_root = project.path().join("dependency");
        fs::create_dir_all(generated_root.parent().ok_or("generated root has no parent")?)?;
        fs::create_dir_all(dependency_root.join("src"))?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"oven_explicit_bake_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\noven_bake_dependency = { path = \"dependency\" }\n",
        )?;
        fs::write(
            dependency_root.join("Cargo.toml"),
            "[package]\nname = \"oven_bake_dependency\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(dependency_root.join("src/lib.rs"), "pub fn value() -> u8 { 7 }\n")?;
        fs::write(
            &generated_root,
            "fn main() { let _ = oven_bake_dependency::value(); }\n",
        )?;

        let rustc = resolve_active_rustc()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "oven_explicit_bake_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_root)
            .with_build_unit_input("provider-plan", digest_bytes(b"")),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            oven_store::store::OvenStoreLimits::new(1024 * 1024 * 1024, 1024 * 1024 * 1024, 1024 * 1024 * 1024),
        );

        let consume_only = select_or_bake_generated_project_plan(
            OvenProjectPlanMode::ConsumeOnly,
            &store,
            &receipt,
            OvenProjectDependencySurface {
                selection: &[],
                provider_compilations: &[],
            },
            project.path(),
            &generated_root,
            &rustc,
        );
        let compiler_suite_native = env::var_os("INCAN_INTERNAL_OVEN_LOAF_EXECUTION").is_some_and(|value| value == "1");
        if compiler_suite_native {
            let Err(error) = consume_only else {
                return Err("a compiler-suite normal consumer must reject a caller-owned Loaf miss".into());
            };
            let message = error.to_string();
            assert!(
                message.contains(OVEN_NESTED_DEPENDENCY_MISS_SUMMARY)
                    && message.contains(OVEN_NO_IMPLICIT_DEPENDENCY_BUILD),
                "compiler-suite normal consumers must remain Cargo-free even when the explicit baker is tested, got: {message}"
            );
        } else {
            let consume_only = consume_only?;
            assert!(
                consume_only.is_none(),
                "a normal consumer must not invoke the compatibility baker on a miss"
            );
        }
        assert!(
            !project.path().join("Cargo.lock").exists(),
            "a consume-only normal command must not create Cargo publisher state"
        );
        fs::write(
            project.path().join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"oven_bake_dependency\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"oven_explicit_bake_fixture\"\nversion = \"0.1.0\"\ndependencies = [\"oven_bake_dependency\"]\n",
        )?;

        let first = select_or_bake_generated_project_plan(
            OvenProjectPlanMode::ExplicitBake,
            &store,
            &receipt,
            OvenProjectDependencySurface {
                selection: &[],
                provider_compilations: &[],
            },
            project.path(),
            &generated_root,
            &rustc,
        )?
        .ok_or("explicit Oven bake did not select its published plan")?;
        assert_eq!(first.materialization, OvenToolchainMaterialization::CompatibilityBaked);
        assert!(project.path().join("Cargo.lock").is_file());
        let OvenDirectRustcPlanSelection::Stored(first_stored) = &first.plan_selection else {
            return Err("an explicit project bake must select its project Loaf".into());
        };
        let loaf_root = first_stored
            .artifact_root
            .parent()
            .ok_or("a project Loaf artifact root must have its owning entry directory")?;
        assert_eq!(
            loaf_root.extension().and_then(|extension| extension.to_str()),
            Some("loaf")
        );
        assert!(loaf_root.join("loaf.json").is_file());

        let second = select_or_bake_generated_project_plan(
            OvenProjectPlanMode::ExplicitBake,
            &store,
            &receipt,
            OvenProjectDependencySurface {
                selection: &[],
                provider_compilations: &[],
            },
            project.path(),
            &generated_root,
            &rustc,
        )?
        .ok_or("repeated explicit Oven bake lost its selected plan")?;
        assert_eq!(second.materialization, OvenToolchainMaterialization::Reused);
        assert_eq!(
            first.plan_selection.report_identity(),
            second.plan_selection.report_identity(),
            "an unchanged project receipt must reuse the already published direct-rustc plan"
        );
        Ok(())
    }
}
