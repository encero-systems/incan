//! Native compiler inputs selected for one direct-Rustc execution.
//!
//! Split out of `rustc.rs` rather than annotated in place: that module is over twelve thousand lines, and these
//! types are a self-contained view whose only reader is the gate that has not landed yet. A module-wide allow
//! would be wrong inside `rustc.rs`, where it would also hide dead code in the live compiler paths.
//!
//! The view deliberately separates *what a stored plan says its inputs are* from *materializing them*: a
//! candidate names a registry leaf and its artifact, and only a selected input carries a plan the executor may
//! act on.
#![allow(
    dead_code,
    reason = "Gates 6 and 7 of RFC 119 are the reader; this substrate lands first so their blockers have something to change"
)]

use super::*;

/// Original foundation provenance, preserving distinct store receipts and committed Loaf member identities.
pub(crate) enum OvenNativeInputOrigin<'facts> {
    /// A stored direct plan retains the exact original content, receipt and reusable build-unit identities.
    Store {
        identity: &'facts str,
        receipt_identity: &'facts str,
        build_unit_identity: &'facts str,
    },
    /// A toolchain member has committed generation/member/build/plan coordinates, not a store receipt.
    ToolchainLoaf {
        generation_identity: &'facts str,
        member: &'facts crate::oven::loaf::OvenLoafEnvelopeMember,
    },
}

/// The actual selected owner retained through fact queries and physical attachment.
enum OvenNativeInputOwner<'owner> {
    Store(&'owner OvenStoreExecutionPayload),
    ToolchainLoaf(&'owner crate::oven::loaf::OvenMaterializedLoafCandidate<'owner>),
}

/// Existing stored direct-plan carrier, retaining its original execution owner and one decoded native projection.
///
/// Selection keeps the historical inexpensive materialization path. A batch borrows this carrier only after its
/// existing full owner validation, then enumerates and attaches members without another payload decode or full scan.
pub(crate) struct OvenStoredDirectRustcExecutionPlan {
    owner: OvenStoreExecutionPayload,
    artifacts: OvenRustcArtifactManifest,
    artifact_plan: OvenRustcArtifactPlan,
}

impl OvenStoredDirectRustcExecutionPlan {
    /// Retain an already selected payload and decode its direct plan once, preserving ordinary selection semantics.
    pub(crate) fn from_execution_payload(owner: OvenStoreExecutionPayload) -> Result<Self, OvenRustcError> {
        if owner.manifest.kind != OvenArtifactKind::DirectRustcPlan {
            return Err(OvenRustcError::InvalidInput {
                field: "stored direct plan",
                message: "requires an original selected direct-plan payload".to_string(),
            });
        }
        let artifacts: OvenRustcArtifactManifest =
            serde_json::from_slice(&owner.payload).map_err(|error| OvenRustcError::InvalidInput {
                field: "stored direct plan payload",
                message: error.to_string(),
            })?;
        let artifact_plan = artifacts.materialize_trusted_store(&owner.artifact_root, &owner.manifest.intent)?;
        Ok(Self {
            owner,
            artifacts,
            artifact_plan,
        })
    }

    /// Return the original immutable Store entry identity.
    pub(crate) fn identity(&self) -> &str {
        &self.owner.manifest.identity
    }

    /// Borrow the sole decoded manifest while its original owner remains retained.
    pub(crate) fn artifacts(&self) -> &OvenRustcArtifactManifest {
        &self.artifacts
    }

    /// Borrow the original materialized root; this is not a caller-supplied authority path.
    pub(crate) fn artifact_root(&self) -> &Path {
        &self.owner.artifact_root
    }

    /// Borrow the existing inexpensive materialized plan used by ordinary stored-plan consumers.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.artifact_plan
    }

    /// Validate the original owner once for a batch and borrow its retained decode and native projection.
    pub(crate) fn native_input_view(&self) -> Result<OvenNativeInputView<'_>, OvenRustcError> {
        self.owner.verify_admitted_payload()?;
        Ok(OvenNativeInputView {
            owner: OvenNativeInputOwner::Store(&self.owner),
            artifacts: Cow::Borrowed(&self.artifacts),
            artifact_root: self.owner.artifact_root.clone(),
            plan: Cow::Borrowed(&self.artifact_plan),
        })
    }
}

/// A command-owned physical view of one admitted foundation, borrowing its actual execution owner.
///
/// Store construction validates the original record/files once; Loaf construction borrows an already materialized
/// candidate. Subsequent fact queries do not touch the filesystem or select compatibility. One Incan batch chooses
/// the foundation before any source unit attaches inputs through it. Neither origin substitutes consumer receipts.
pub(crate) struct OvenNativeInputView<'owner> {
    owner: OvenNativeInputOwner<'owner>,
    pub(super) artifacts: Cow<'owner, OvenRustcArtifactManifest>,
    artifact_root: PathBuf,
    pub(super) plan: Cow<'owner, OvenRustcArtifactPlan>,
}

impl<'owner> OvenNativeInputView<'owner> {
    /// Borrow an original store payload after its existing record and physical integrity checks succeed.
    pub(crate) fn from_store_payload(owner: &'owner OvenStoreExecutionPayload) -> Result<Self, OvenRustcError> {
        if owner.manifest.kind != OvenArtifactKind::DirectRustcPlan {
            return Err(OvenRustcError::InvalidInput {
                field: "native input owner",
                message: "requires an original admitted direct-plan payload".to_string(),
            });
        }
        owner.verify_admitted_payload()?;
        let artifacts: OvenRustcArtifactManifest =
            serde_json::from_slice(&owner.payload).map_err(|error| OvenRustcError::InvalidInput {
                field: "native input owner payload",
                message: error.to_string(),
            })?;
        let artifact_root = canonical_directory(&owner.artifact_root, "native input owner root")?;
        let plan = artifacts.materialize_trusted_store(&artifact_root, &owner.manifest.intent)?;
        Ok(Self {
            owner: OvenNativeInputOwner::Store(owner),
            artifacts: Cow::Owned(artifacts),
            artifact_root,
            plan: Cow::Owned(plan),
        })
    }

    /// Borrow an already physically validated committed Loaf without repeating its full artifact walk.
    pub(crate) fn from_materialized_loaf(
        owner: &'owner crate::oven::loaf::OvenMaterializedLoafCandidate<'owner>,
    ) -> Result<Self, OvenRustcError> {
        let candidate = owner.candidate();
        Ok(Self {
            owner: OvenNativeInputOwner::ToolchainLoaf(owner),
            artifacts: Cow::Borrowed(&candidate.metadata().plan),
            artifact_root: canonical_directory(owner.artifact_root(), "native input Loaf root")?,
            plan: Cow::Borrowed(owner.artifact_plan()),
        })
    }

    /// Return the original typed provenance without manufacturing a store receipt for a toolchain member.
    pub(crate) fn origin(&self) -> OvenNativeInputOrigin<'_> {
        match &self.owner {
            OvenNativeInputOwner::Store(owner) => OvenNativeInputOrigin::Store {
                identity: &owner.manifest.identity,
                receipt_identity: &owner.manifest.receipt_identity,
                build_unit_identity: &owner.manifest.build_unit_identity,
            },
            OvenNativeInputOwner::ToolchainLoaf(owner) => {
                let candidate = owner.candidate();
                OvenNativeInputOrigin::ToolchainLoaf {
                    generation_identity: candidate.generation_identity(),
                    member: candidate.member(),
                }
            }
        }
    }

    /// Return the original admitted content identity, not a new consumer receipt identity.
    pub(crate) fn identity(&self) -> &str {
        match self.origin() {
            OvenNativeInputOrigin::Store { identity, .. } => identity,
            OvenNativeInputOrigin::ToolchainLoaf { member, .. } => &member.loaf_identity,
        }
    }

    /// Return the original store receipt when this origin has one; toolchain members retain different provenance.
    pub(crate) fn receipt_identity(&self) -> Option<&str> {
        match self.origin() {
            OvenNativeInputOrigin::Store { receipt_identity, .. } => Some(receipt_identity),
            OvenNativeInputOrigin::ToolchainLoaf { .. } => None,
        }
    }

    /// Require the selected Store's original full publisher recipe; an exact key cannot supply missing SDK recipe
    /// facts.
    pub(crate) fn original_native_receipt(&self) -> Result<&OvenReceipt, OvenRustcError> {
        let receipt = match &self.owner {
            OvenNativeInputOwner::Store(owner) => owner.original_native_receipt(),
            OvenNativeInputOwner::ToolchainLoaf(_) => None,
        };
        receipt.ok_or_else(|| OvenRustcError::InvalidInput {
            field: "native input receipt witness",
            message: "selected native owner has no original full publisher receipt witness".to_string(),
        })
    }

    /// Return the original reusable build-unit identity without recomputing compatibility.
    pub(crate) fn build_unit_identity(&self) -> &str {
        match self.origin() {
            OvenNativeInputOrigin::Store {
                build_unit_identity, ..
            } => build_unit_identity,
            OvenNativeInputOrigin::ToolchainLoaf { member, .. } => &member.build_unit_identity,
        }
    }

    /// Borrow the already admitted original catalog for command-table enumeration without another decode or scan.
    pub(crate) fn artifacts(&self) -> &OvenRustcArtifactManifest {
        &self.artifacts
    }

    /// Borrow the admitted physical root only for the existing direct-rustc output containment boundary.
    pub(crate) fn artifact_root(&self) -> &Path {
        &self.artifact_root
    }

    /// Return the original target, toolchain, profile and features as facts, without filtering candidates.
    pub(crate) fn intent(&self) -> &OvenBuildIntent {
        &self.artifacts.intent
    }

    /// Require an original declared source role; a caller's newly generated source key grants nothing here.
    fn require_original_role(&self, source_role: &str) -> Result<(), OvenRustcError> {
        if !self.artifacts.entrypoint_externs.contains_key(source_role) {
            return Err(OvenRustcError::InvalidInput {
                field: "native input source role",
                message: format!("`{source_role}` is not an original declared role of this foundation"),
            });
        }
        Ok(())
    }

    /// Expose every recorded registry leaf in publisher order; version and feature choice belongs to Incan.
    ///
    /// A handle retains the original source role for later physical membership checks. Enumeration itself neither
    /// discovers files nor promotes an anonymous supporting artifact to a named dependency.
    pub(crate) fn registry_candidates(
        &self,
        source_role: &str,
    ) -> Result<Vec<OvenNativeInputCandidate<'_>>, OvenRustcError> {
        self.require_original_role(source_role)?;
        Ok(self
            .artifacts
            .registry_leaves
            .iter()
            .map(|leaf| OvenNativeInputCandidate {
                view: self,
                artifact: &leaf.artifact,
                registry_leaf: Some(leaf),
                source_role: source_role.to_string(),
            })
            .collect())
    }

    /// Expose only the explicitly named externs of an original source role, in manifest declaration order.
    pub(crate) fn named_candidates(
        &self,
        source_role: &str,
    ) -> Result<Vec<OvenNativeInputCandidate<'_>>, OvenRustcError> {
        self.require_original_role(source_role)?;
        let projected = self.artifacts.for_source_evidence(source_role)?;
        Ok(self
            .artifacts
            .externs
            .iter()
            .filter(|artifact| projected.externs.iter().any(|allowed| allowed == *artifact))
            .map(|artifact| OvenNativeInputCandidate {
                view: self,
                artifact,
                registry_leaf: None,
                source_role: source_role.to_string(),
            })
            .collect())
    }

    /// Attach response-selected members transactionally, preserving this exact foundation's original source role.
    ///
    /// The host's command table must first bind the Incan response to the global batch and each definition. This
    /// method checks only original owner/role membership, identifiers, artifact bytes and physical collisions; it
    /// never interprets dependency requirements. Existing role projection owns search/native paths and environment.
    /// No missing directory is added, no alternative file is sought and no output or child is created.
    pub(crate) fn attach_for_source(
        &self,
        source_role: &str,
        selected: &[OvenSelectedNativeInput<'_>],
    ) -> Result<OvenSelectedNativeSourcePlan<'_>, OvenRustcError> {
        self.require_original_role(source_role)?;
        let mut plan = trusted_artifact_plan_for_source_evidence(&self.plan, &self.artifacts, source_role)?;
        let mut aliases = BTreeSet::new();
        for input in selected {
            let candidate = &input.candidate;
            if !std::ptr::eq(candidate.view, self) || candidate.source_role != source_role {
                return Err(OvenRustcError::InvalidInput {
                    field: "selected native input",
                    message: "belongs to a different foundation view or original source role".to_string(),
                });
            }
            if !aliases.insert(input.alias.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "selected native input",
                    message: format!("duplicates selected alias `{}`", input.alias),
                });
            }
            let artifact = candidate.artifact;
            let output = verified_file(
                &self.artifact_root,
                &artifact.relative_path,
                &artifact.digest,
                "selected native input",
            )?;
            let parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
                field: "selected native input",
                message: "artifact has no dependency directory".to_string(),
            })?;
            if !plan.dependency_search_paths.iter().any(|directory| directory == parent) {
                return Err(OvenRustcError::InvalidInput {
                    field: "selected native input",
                    message: format!(
                        "{} is outside the original source role's dependency directories",
                        output.display()
                    ),
                });
            }
            if let Some((_, existing)) = plan.externs.iter().find(|(name, _)| name == &input.alias) {
                if existing != &output {
                    return Err(OvenRustcError::InvalidInput {
                        field: "selected native input",
                        message: format!("conflicts with existing named extern `{}`", input.alias),
                    });
                }
                continue;
            }
            if plan
                .caller_owned_library_digests
                .insert(input.alias.clone(), artifact.digest.clone())
                .is_some()
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "selected native input",
                    message: format!("conflicts with existing digest evidence for `{}`", input.alias),
                });
            }
            plan.externs.push((input.alias.clone(), output));
        }
        Ok(OvenSelectedNativeSourcePlan { plan, _view: self })
    }
}

/// One original manifest member; only its borrowed foundation view can construct this physical handle.
#[derive(Clone)]
pub(crate) struct OvenNativeInputCandidate<'view> {
    view: &'view OvenNativeInputView<'view>,
    artifact: &'view OvenRustcArtifactExtern,
    registry_leaf: Option<&'view OvenRustcRegistryLeaf>,
    pub(super) source_role: String,
}

impl<'view> OvenNativeInputCandidate<'view> {
    /// Return recorded registry source/package/version/features, if this is a registry candidate.
    pub(crate) fn registry_leaf(&self) -> Option<&OvenRustcRegistryLeaf> {
        self.registry_leaf
    }

    /// Return the original named artifact declaration, without resolving its filesystem location.
    pub(crate) fn artifact(&self) -> &OvenRustcArtifactExtern {
        self.artifact
    }

    /// Carry the already selected alias; matching that alias to an authored slot remains the Incan batch decision.
    pub(crate) fn with_alias(self, alias: String) -> Result<OvenSelectedNativeInput<'view>, OvenRustcError> {
        validate_rust_identifier(&alias)?;
        Ok(OvenSelectedNativeInput { candidate: self, alias })
    }
}

/// A physical member and explicit direct alias selected by the command's bound Incan batch response.
pub(crate) struct OvenSelectedNativeInput<'view> {
    candidate: OvenNativeInputCandidate<'view>,
    alias: String,
}

/// A source-specific plan whose actual selected foundation owner remains borrowed through its complete use.
pub(crate) struct OvenSelectedNativeSourcePlan<'owner> {
    plan: OvenRustcArtifactPlan,
    _view: &'owner OvenNativeInputView<'owner>,
}

impl OvenSelectedNativeSourcePlan<'_> {
    /// Borrow the attached plan while preserving the selected store lease or committed generation lock.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.plan
    }
}
