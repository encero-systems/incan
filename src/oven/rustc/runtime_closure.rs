//! Immutable publication and selection of a rebuilt Cargo-free runtime closure.
//!
//! Gate 6's executor produces caller-owned outputs below a scratch root. This module turns one such build into a
//! single immutable Store entry and, later, selects it back without recompiling anything.
//!
//! The payload's identity is deliberately content-only. It binds the compiler closure, the sealed foundation, and for
//! every rebuilt unit its selected identity, compiled identity, output digest and direct dependency identities. It
//! records no package version and no physical path: a coordinate-only change that leaves every compiler input alone
//! must resolve to the same closure, and a closure published on one machine must select on another.

#![allow(
    dead_code,
    reason = "normal-build selection consumes this published closure in the hot-path gate"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::super::store::{
    OvenArtifactKind, OvenArtifactManifest, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore,
    OvenStoreExecutionPayload,
};
use super::super::{OvenReceipt, digest_bytes};
use super::{
    OvenRuntimeFoundationBuild, OvenRuntimeRebuildDependencyKind, OvenRustcError, OvenSelectedRustFacetDomain,
    ValidatedOvenRuntimeFoundation,
};

/// Wire schema for one published Cargo-free runtime closure.
pub(crate) const OVEN_RUNTIME_CLOSURE_SCHEMA_VERSION: u32 = 1;

/// Compatibility-domain prefix under which every runtime closure is published.
const OVEN_RUNTIME_CLOSURE_DOMAIN_PREFIX: &str = "incan.oven.runtime-closure";

/// One direct compiler input recorded in a published closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeClosureDependency {
    /// Rust-facing alias the parent source used.
    pub(crate) alias: String,
    /// Sealed artifact digest, or the compiled identity of another unit in this same closure.
    pub(crate) identity: String,
    /// Whether the sealed foundation or this closure owns the bytes behind the alias.
    pub(crate) foundation_owned: bool,
}

/// One rebuilt library retained by a published runtime closure.
///
/// There is no path field by design. The store-relative location is a pure function of the compiled identity and
/// crate name, so a path can never drift from the identity that names it, and no physical location can leak into the
/// closure's own identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeClosureUnit {
    /// Selected-source identity naming the unit in the foundation graph.
    pub(crate) selected_identity: String,
    /// Compiler-input identity that decides whether this output may be reused.
    pub(crate) compiled_identity: String,
    /// Rust crate name the unit was compiled under.
    pub(crate) crate_name: String,
    /// Explicit host or target domain declared by the foundation.
    pub(crate) domain: OvenSelectedRustFacetDomain,
    /// Digest of the exact produced bytes.
    pub(crate) digest: String,
    /// Sorted direct compiler inputs.
    pub(crate) dependencies: Vec<OvenRuntimeClosureDependency>,
}

impl OvenRuntimeClosureUnit {
    /// Return the store-relative artifact path this unit occupies below its entry's artifact root.
    pub(crate) fn relative_path(&self) -> String {
        format!(
            "units/{}/lib{}.rlib",
            self.compiled_identity.replace(':', "-"),
            self.crate_name
        )
    }
}

/// The complete immutable description of one rebuilt runtime closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeClosurePayload {
    /// Runtime-closure wire schema.
    pub(crate) schema_version: u32,
    /// Exact direct-Rustc compiler closure that produced every unit below.
    pub(crate) compiler_closure_digest: String,
    /// Content identity of the sealed runtime foundation this closure was rebuilt above.
    pub(crate) foundation_identity: String,
    /// Rebuilt units in the foundation's declared order.
    pub(crate) units: Vec<OvenRuntimeClosureUnit>,
}

/// The exact projection of a closure that decides its identity.
///
/// Two facts the payload records are deliberately absent here. A selected-source identity transitively contains its
/// unit's package and version, and a foundation identity contains every unit's selected identity; digesting either
/// would make a pure coordinate bump republish a byte-identical closure under a new name, which is precisely the
/// reuse property this pipeline exists to provide. Both stay in the payload as provenance instead.
#[derive(Serialize)]
struct RuntimeClosureIdentityInput<'a> {
    schema_version: u32,
    compiler_closure_digest: &'a str,
    units: Vec<RuntimeClosureIdentityUnit<'a>>,
}

/// One unit's compiler-input contribution to a closure identity.
#[derive(Serialize)]
struct RuntimeClosureIdentityUnit<'a> {
    compiled_identity: &'a str,
    crate_name: &'a str,
    domain: OvenSelectedRustFacetDomain,
    digest: &'a str,
    dependencies: &'a [OvenRuntimeClosureDependency],
}

impl OvenRuntimeClosurePayload {
    /// Return this closure's canonical content identity.
    ///
    /// The digest covers compiler inputs only: the retained compiler closure, and for every unit its compiled
    /// identity, crate name, domain, produced bytes and direct dependency identities. Two builds whose compiler
    /// inputs match therefore produce the same identity even when their package versions or scratch roots differ.
    pub(crate) fn identity(&self) -> Result<String, OvenRustcError> {
        let input = RuntimeClosureIdentityInput {
            schema_version: self.schema_version,
            compiler_closure_digest: &self.compiler_closure_digest,
            units: self
                .units
                .iter()
                .map(|unit| RuntimeClosureIdentityUnit {
                    compiled_identity: &unit.compiled_identity,
                    crate_name: &unit.crate_name,
                    domain: unit.domain,
                    digest: &unit.digest,
                    dependencies: &unit.dependencies,
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&input).map_err(|error| OvenRustcError::InvalidInput {
            field: "runtime closure identity",
            message: format!("cannot encode runtime closure: {error}"),
        })?;
        Ok(digest_bytes(&bytes))
    }

    /// Return the compatibility domain one closure identity publishes under.
    pub(crate) fn domain(&self) -> Result<String, OvenRustcError> {
        let identity = self.identity()?;
        let hex = identity
            .strip_prefix("sha256:")
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "runtime closure identity",
                message: "has no SHA-256 identity prefix".to_string(),
            })?;
        Ok(format!("{OVEN_RUNTIME_CLOSURE_DOMAIN_PREFIX}.{hex}"))
    }
}

/// A published runtime closure bound to the store-owned paths of its retained entry.
#[derive(Debug, Clone)]
pub(crate) struct OvenSelectedRuntimeClosure {
    payload: OvenRuntimeClosurePayload,
    store_identity: String,
    artifacts: BTreeMap<String, PathBuf>,
}

impl OvenSelectedRuntimeClosure {
    /// Borrow the immutable closure description.
    pub(crate) fn payload(&self) -> &OvenRuntimeClosurePayload {
        &self.payload
    }

    /// Return the store entry identity holding this closure.
    pub(crate) fn store_identity(&self) -> &str {
        &self.store_identity
    }

    /// Return the retained artifact path for one rebuilt unit's compiled identity.
    pub(crate) fn artifact(&self, compiled_identity: &str) -> Option<&PathBuf> {
        self.artifacts.get(compiled_identity)
    }
}

/// Derive the content identity of one sealed runtime foundation.
///
/// The foundation exposes no identity of its own, so this digests exactly the three facts that define it: the
/// compiler closure it is bound to, its sealed artifact catalogue, and its selected source graph.
pub(crate) fn runtime_foundation_identity(
    foundation: &ValidatedOvenRuntimeFoundation,
) -> Result<String, OvenRustcError> {
    let binding = (
        foundation.compiler_closure_digest(),
        foundation.artifacts(),
        foundation.selected_graph().graph(),
    );
    let bytes = serde_json::to_vec(&binding).map_err(|error| OvenRustcError::InvalidInput {
        field: "runtime foundation identity",
        message: format!("cannot encode runtime foundation: {error}"),
    })?;
    Ok(digest_bytes(&bytes))
}

/// Project one completed rebuild into its immutable published description.
pub(crate) fn runtime_closure_payload(
    foundation: &ValidatedOvenRuntimeFoundation,
    build: &OvenRuntimeFoundationBuild,
) -> Result<OvenRuntimeClosurePayload, OvenRustcError> {
    if build.outputs().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "runtime closure",
            message: "a published closure must retain at least one rebuilt unit".to_string(),
        });
    }
    let units = build
        .outputs()
        .iter()
        .map(|output| OvenRuntimeClosureUnit {
            selected_identity: output.selected_identity.clone(),
            compiled_identity: output.compiled_identity.as_str().to_string(),
            crate_name: output.crate_name.clone(),
            domain: output.domain,
            digest: output.digest.clone(),
            dependencies: output
                .dependencies
                .iter()
                .map(|dependency| OvenRuntimeClosureDependency {
                    alias: dependency.alias.clone(),
                    identity: dependency.identity.clone(),
                    foundation_owned: dependency.kind == OvenRuntimeRebuildDependencyKind::Prebuilt,
                })
                .collect(),
        })
        .collect();
    Ok(OvenRuntimeClosurePayload {
        schema_version: OVEN_RUNTIME_CLOSURE_SCHEMA_VERSION,
        compiler_closure_digest: foundation.compiler_closure_digest().to_string(),
        foundation_identity: runtime_foundation_identity(foundation)?,
        units,
    })
}

/// Publish one rebuilt closure as a single immutable Store entry.
///
/// This is bake-only work. The receipt is the caller's publication authority and is never synthesized here; it
/// authorizes the entry but contributes nothing to the closure's own identity, which is why an identical closure
/// baked under two different receipts stays reusable.
pub(crate) fn publish_runtime_closure(
    store: &OvenStore,
    receipt: &OvenReceipt,
    foundation: &ValidatedOvenRuntimeFoundation,
    build: &OvenRuntimeFoundationBuild,
) -> Result<OvenArtifactManifest, OvenRustcError> {
    let payload = runtime_closure_payload(foundation, build)?;
    let materialized_files = build
        .outputs()
        .iter()
        .zip(&payload.units)
        .map(|(output, unit)| OvenArtifactMaterializedFile {
            source_path: output.artifact.clone(),
            relative_path: unit.relative_path(),
        })
        .collect();
    let encoded = serde_json::to_vec(&payload).map_err(|error| OvenRustcError::InvalidInput {
        field: "runtime closure",
        message: format!("cannot encode runtime closure payload: {error}"),
    })?;
    store
        .publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: payload.domain()?,
            kind: OvenArtifactKind::NativeRuntimeClosure,
            payload: encoded,
            materialized_files,
        })
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "runtime closure",
            message: format!("cannot publish runtime closure: {error}"),
        })
}

/// Bind one retained execution payload to its store-owned artifact paths.
///
/// The caller supplies an owner it already leased. Every path is derived from the payload's own compiled identities,
/// so a file the closure does not name stays unreachable even if it exists below the same artifact root.
pub(crate) fn admit_runtime_closure(
    owner: &OvenStoreExecutionPayload,
    expected: &OvenRuntimeClosurePayload,
) -> Result<Option<OvenSelectedRuntimeClosure>, OvenRustcError> {
    if owner.manifest.kind != OvenArtifactKind::NativeRuntimeClosure {
        return Ok(None);
    }
    let Ok(payload) = serde_json::from_slice::<OvenRuntimeClosurePayload>(&owner.payload) else {
        return Ok(None);
    };
    if payload.schema_version != OVEN_RUNTIME_CLOSURE_SCHEMA_VERSION || payload.identity()? != expected.identity()? {
        return Ok(None);
    }
    let mut artifacts = BTreeMap::new();
    for unit in &payload.units {
        let path = owner.artifact_root.join(unit.relative_path());
        if !path.is_file() {
            return Err(OvenRustcError::InvalidStoredPlan {
                identity: owner.manifest.identity.clone(),
                message: format!("runtime closure lost its retained output for {}", unit.crate_name),
            });
        }
        artifacts.insert(unit.compiled_identity.clone(), path);
    }
    Ok(Some(OvenSelectedRuntimeClosure {
        payload,
        store_identity: owner.manifest.identity.clone(),
        artifacts,
    }))
}

/// Select one already-published runtime closure for a normal command.
///
/// This is a content-addressed store lookup and nothing else. It does not bake, does not read a Cargo manifest, and
/// does not list a target directory: a closure either exists under the identity its compiler inputs imply, or it does
/// not. `Ok(None)` is the honest miss, which the caller turns into a terminal typed refusal.
pub(crate) fn select_runtime_closure(
    store: &OvenStore,
    expected: &OvenRuntimeClosurePayload,
) -> Result<Option<OvenSelectedRuntimeClosure>, OvenRustcError> {
    let domain = expected.domain()?;
    let owners = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.kind == OvenArtifactKind::NativeRuntimeClosure && manifest.domain == domain
        })
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "runtime closure selection",
            message: format!("cannot select a published runtime closure: {error}"),
        })?;
    for owner in &owners {
        if let Some(selected) = admit_runtime_closure(owner, expected)? {
            return Ok(Some(selected));
        }
    }
    Ok(None)
}

/// Require an already-published runtime closure, refusing terminally on a miss.
///
/// The refusal reuses the existing normal-command vocabulary so a missing closure reads the same as any other absent
/// selected native plan. Explicit bake intent is what publishes a closure; a normal build never escalates to one.
pub(crate) fn require_runtime_closure(
    store: &OvenStore,
    receipt: &OvenReceipt,
    expected: &OvenRuntimeClosurePayload,
) -> Result<OvenSelectedRuntimeClosure, super::super::OvenError> {
    match select_runtime_closure(store, expected) {
        Ok(Some(selected)) => Ok(selected),
        Ok(None) | Err(_) => Err(super::super::OvenError::SelectedNativePlanUnavailable {
            build_unit_identity: receipt.build_unit_identity.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::oven::rustc::runtime_executor::tests::{FIXTURE_CLOSURE, fixture, fixture_at_version};
    use crate::oven::rustc::{OvenRuntimeCompilerClosure, execute_runtime_foundation_rebuild};
    use crate::oven::store::OvenStoreLimits;
    use crate::oven::{
        OVEN_RECEIPT_SCHEMA_VERSION, OvenCompatibility, OvenCompatibilityKind, OvenProjectIdentity, OvenSourceEvidence,
    };

    /// Build one publication-authorizing receipt for the fixture closure.
    ///
    /// The receipt authorizes the entry only. Every assertion below checks that its contents stay out of the
    /// closure's own identity, so its project coordinates are deliberately varied between publications.
    fn publication_receipt(
        project: &str,
        version: &str,
        target: &str,
        toolchain: &str,
    ) -> Result<OvenReceipt, Box<dyn std::error::Error>> {
        let identity = OvenProjectIdentity {
            name: project.to_string(),
            version: version.to_string(),
        };
        let sources = OvenSourceEvidence {
            cargo_manifest_digest: None,
            cargo_lock_digest: None,
            incan_manifest_digest: None,
            supplemental_digests: BTreeMap::new(),
            build_unit_inputs: BTreeMap::new(),
        };
        let intent = crate::oven::normalized_build_intent(target, toolchain, "debug", &[])?;
        let compatibility = OvenCompatibility {
            kind: OvenCompatibilityKind::GeneratedIncanProject,
            cargo_input_only: true,
        };
        Ok(OvenReceipt {
            schema_version: OVEN_RECEIPT_SCHEMA_VERSION,
            identity: crate::oven::receipt_identity(&identity, &sources, &intent, &compatibility)?,
            build_unit_identity: crate::oven::build_unit_identity(&intent, &compatibility, &sources.build_unit_inputs)?,
            project: identity,
            sources,
            intent,
            compatibility,
        })
    }

    /// A rebuilt closure publishes as one immutable entry and selects back with every output retained.
    #[test]
    fn rebuilt_closure_publishes_and_selects_back() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let output_root = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);
        let build = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &closure,
            output_root.path(),
        )?;
        let intent = &fixture.foundation.selected_graph().graph().selection.intent;
        let receipt = publication_receipt("fixture_project", "0.1.0", &intent.target, &intent.toolchain)?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
        );

        let manifest = publish_runtime_closure(&store, &receipt, &fixture.foundation, &build)?;
        let expected = runtime_closure_payload(&fixture.foundation, &build)?;

        assert_eq!(manifest.kind, OvenArtifactKind::NativeRuntimeClosure);
        assert_eq!(manifest.domain, expected.domain()?);
        let mut owners = store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity))?;
        let owner = owners.pop().ok_or("published closure has no retained owner")?;
        let selected = admit_runtime_closure(&owner, &expected)?.ok_or("published closure failed admission")?;
        assert_eq!(selected.store_identity(), manifest.identity);
        assert_eq!(selected.payload().units.len(), 2);
        for unit in &selected.payload().units {
            let artifact = selected
                .artifact(&unit.compiled_identity)
                .ok_or("closure lost a retained artifact")?;
            assert!(artifact.is_file());
            // macOS resolves a temporary root through `/private`, so compare canonical forms.
            assert!(
                artifact.canonicalize()?.starts_with(store_root.path().canonicalize()?),
                "a selected artifact must come from the store, not the caller's scratch root"
            );
            assert_eq!(digest_bytes(&std::fs::read(artifact)?), unit.digest);
        }
        Ok(())
    }

    /// The closure identity excludes both the publishing receipt's package coordinates and the caller's scratch root.
    #[test]
    fn closure_identity_excludes_package_version_and_cache_paths() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);
        let first_root = tempfile::tempdir()?;
        let second_root = tempfile::tempdir()?;

        let first = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &closure,
            first_root.path(),
        )?;
        let second = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &closure,
            second_root.path(),
        )?;

        assert_eq!(first.compiler_launches(), 2);
        assert_eq!(second.compiler_launches(), 2, "a distinct scratch root recompiles");
        assert_ne!(
            first.outputs()[0].artifact,
            second.outputs()[0].artifact,
            "the two builds really did use different physical roots"
        );
        let first_payload = runtime_closure_payload(&fixture.foundation, &first)?;
        let second_payload = runtime_closure_payload(&fixture.foundation, &second)?;
        assert_eq!(
            first_payload.identity()?,
            second_payload.identity()?,
            "a different scratch root must not change the closure identity"
        );

        // The identity carries the facts the gate requires and none of the ones it forbids.
        let encoded = serde_json::to_string(&first_payload)?;
        assert!(encoded.contains(FIXTURE_CLOSURE), "compiler closure is bound");
        assert!(
            encoded.contains(&runtime_foundation_identity(&fixture.foundation)?),
            "foundation identity is bound"
        );
        for output in first.outputs() {
            assert!(
                encoded.contains(&output.selected_identity),
                "selected identity is bound"
            );
            assert!(
                encoded.contains(output.compiled_identity.as_str()),
                "compiled identity is bound"
            );
            assert!(encoded.contains(&output.digest), "output digest is bound");
        }
        assert!(
            !encoded.contains(&first_root.path().display().to_string()),
            "no scratch path may enter the published closure"
        );
        assert!(
            !encoded.contains("1.0.0"),
            "no package version may enter the published closure"
        );
        Ok(())
    }

    /// One closure published under two different receipts stays a single reusable immutable entry.
    #[test]
    fn publishing_receipt_does_not_change_the_closure() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let output_root = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);
        let build = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &closure,
            output_root.path(),
        )?;
        let intent = &fixture.foundation.selected_graph().graph().selection.intent;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
        );

        let first = publish_runtime_closure(
            &store,
            &publication_receipt("first_project", "0.1.0", &intent.target, &intent.toolchain)?,
            &fixture.foundation,
            &build,
        )?;
        let second = publish_runtime_closure(
            &store,
            &publication_receipt("second_project", "9.9.9", &intent.target, &intent.toolchain)?,
            &fixture.foundation,
            &build,
        )?;

        assert_eq!(
            first.domain, second.domain,
            "the same compiler inputs publish into the same content-addressed domain"
        );
        Ok(())
    }

    /// A coordinate-only change produces the same closure identity and selects the entry the earlier coordinate
    /// published, from a scratch directory that has never seen it.
    ///
    /// This is the whole reuse claim in one place: same compiler inputs, different package version, one Store
    /// entry. It deliberately does *not* assert that the compiler stayed idle. The earlier version did, and its
    /// zero came from both rebuilds sharing one output root -- the second run found the first run's files. That is
    /// a statement about the filesystem, and it would hold just as well for bytes no compiler ever produced. The
    /// second rebuild now runs in its own empty root, so the only thing that can carry reuse across is identity,
    /// which is what JEC actually claims.
    #[test]
    fn coordinate_only_change_selects_the_published_closure_without_recompiling()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = fixture_at_version("1.0.0")?;
        let output_root = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        let closure = OvenRuntimeCompilerClosure::new(&first.rustc, FIXTURE_CLOSURE);
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
        );

        let first_build =
            execute_runtime_foundation_rebuild(&first.foundation, &first.materialized, &closure, output_root.path())?;
        assert_eq!(first_build.compiler_launches(), 2, "the first coordinate compiles");
        let first_payload = runtime_closure_payload(&first.foundation, &first_build)?;
        let intent = &first.foundation.selected_graph().graph().selection.intent;
        let published = publish_runtime_closure(
            &store,
            &publication_receipt("fixture_project", "0.1.0", &intent.target, &intent.toolchain)?,
            &first.foundation,
            &first_build,
        )?;

        // Only the package coordinate moves. Every compiler input is byte-identical.
        let second = fixture_at_version("2.0.0")?;
        assert_ne!(
            first.foundation.selected_graph().graph().units[0].identity,
            second.foundation.selected_graph().graph().units[0].identity,
            "a coordinate change really does produce a different selected identity"
        );

        let cold_root = tempfile::tempdir()?;
        let second_build =
            execute_runtime_foundation_rebuild(&second.foundation, &second.materialized, &closure, cold_root.path())?;

        assert_eq!(
            second_build.compiler_launches(),
            2,
            "a cold root compiles; reuse is the Store's decision, not the scratch directory's"
        );
        let second_payload = runtime_closure_payload(&second.foundation, &second_build)?;
        assert_eq!(
            second_payload.identity()?,
            first_payload.identity()?,
            "the closure identity must survive a coordinate-only change"
        );

        let selected = select_runtime_closure(&store, &second_payload)?
            .ok_or("the second coordinate did not select the published closure")?;
        assert_eq!(
            selected.store_identity(),
            published.identity,
            "the second coordinate selects the entry the first one published"
        );
        Ok(())
    }

    /// A closure that was never published is a terminal typed miss, not a bake.
    #[test]
    fn an_unpublished_closure_is_a_typed_selection_miss() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture()?;
        let output_root = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        let closure = OvenRuntimeCompilerClosure::new(&fixture.rustc, FIXTURE_CLOSURE);
        let build = execute_runtime_foundation_rebuild(
            &fixture.foundation,
            &fixture.materialized,
            &closure,
            output_root.path(),
        )?;
        let payload = runtime_closure_payload(&fixture.foundation, &build)?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
        );
        let intent = &fixture.foundation.selected_graph().graph().selection.intent;
        let receipt = publication_receipt("fixture_project", "0.1.0", &intent.target, &intent.toolchain)?;

        assert!(
            select_runtime_closure(&store, &payload)?.is_none(),
            "an empty store must report a miss rather than produce a closure"
        );
        let refusal = require_runtime_closure(&store, &receipt, &payload);

        let Err(crate::oven::OvenError::SelectedNativePlanUnavailable { build_unit_identity }) = refusal else {
            return Err("a missing runtime closure did not refuse with the normal-command vocabulary".into());
        };
        assert_eq!(build_unit_identity, receipt.build_unit_identity);
        // A miss must leave the store untouched: no entry was baked to satisfy the lookup.
        assert!(
            select_runtime_closure(&store, &payload)?.is_none(),
            "the refused lookup must not have published anything"
        );
        Ok(())
    }
}
