//! Authority modes, denial reasons, outcomes, grant contexts, provenance and decisions.

use super::*;

/// RFC 104 run mode.
///
/// The mode is part of the decision rather than ambient context: the same request produces a different outcome under
/// `Governed` than under `Permissive`, and a consumer reading a stored decision must be able to tell which rule
/// produced it without re-deriving the run's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityMode {
    /// Operations run normally with authority reporting disabled.
    Permissive,
    /// Operations run normally and receipts are emitted.
    Observe,
    /// Operations require granted capabilities and receipts are emitted.
    Governed,
}

impl Default for AuthorityMode {
    /// Observe authority-bearing operations unless a project-owned policy selects another mode.
    fn default() -> Self {
        Self::Observe
    }
}

impl AuthorityMode {
    /// Return the compact snapshot spelling for this mode.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Permissive => "permissive",
            Self::Observe => "observe",
            Self::Governed => "governed",
        }
    }
}

/// Why a governed authority request was denied.
///
/// This is the machine-usable denial reason RFC 104 requires. A consumer branches on the variant; the prose belongs to
/// the diagnostic that renders it, never to this fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityDenialReason {
    /// The invocation never requested this capability.
    NotGranted,
    /// The invocation requested the capability, but a host ceiling did not permit it.
    OutsideCeiling,
    /// The capability was granted, but not for the scope this operation requested.
    OutOfScope,
    /// A budget for this capability was exhausted before the operation ran.
    BudgetExhausted,
    /// Replay required a recorded fixture that was not available.
    FixtureRequired,
}

impl AuthorityDenialReason {
    /// Return the compact snapshot spelling for this denial reason.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotGranted => "not_granted",
            Self::OutsideCeiling => "outside_ceiling",
            Self::OutOfScope => "out_of_scope",
            Self::BudgetExhausted => "budget_exhausted",
            Self::FixtureRequired => "fixture_required",
        }
    }
}

/// The effect of an authority decision on one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityOutcome {
    /// The operation may perform its authority-bearing behavior.
    Allowed,
    /// The operation must fail before performing its authority-bearing behavior.
    Denied(AuthorityDenialReason),
}

/// The grant context a decision was reached against.
///
/// RFC 104 makes the ceiling a distinct grant source from the per-invocation request, and requires the effective grant
/// to be their **intersection, never their union**: an invocation can only ever receive less than its ceiling allows,
/// regardless of what it asks for. The durable fact retains both the resulting grant set and the exact ceiling, because
/// `Allowed` under a ceiling and `Allowed` with no constraint are different facts about the run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityGrantContext {
    /// Scope dimensions the operation requested, as `(dimension, value)` in the capability's declaration order.
    pub requested_scope: Vec<(String, String)>,
    /// The invocation's effective canonical capability grants after project policy and any host ceiling were
    /// intersected.
    pub effective_grants: Vec<CanonicalSymbolId>,
    /// The host-supplied capability ceiling that constrained this invocation, when one applied.
    pub ceiling: Option<Vec<CanonicalSymbolId>>,
}

/// Enough provenance to raise a source-owned governed denial diagnostic.
///
/// RFC 104 requires a denial to identify the required capability and to be reportable against the source that asked
/// for it. The requesting operation's canonical identity and the use-site span are what make that possible without a
/// consumer re-reading source text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityProvenance {
    /// Canonical identity of the operation that requested the authority.
    pub operation: CanonicalSymbolId,
    /// The use site the request came from, which is where a denial is reported.
    pub request_span: crate::HirSourceSpan,
    /// Grant spelling to suggest in a denial diagnostic, such as `host.http.request`.
    pub suggested_grant: String,
}

/// One RFC 104 authority decision about one operation.
///
/// This is deliberately generic over capability publishers and provider operations: both the capability and the
/// requesting operation are named by [`CanonicalSymbolId`], so the stdlib, a library-defined domain capability, and a
/// provider operation all produce the same fact. A consumer can act on an allowed or denied decision without
/// consulting source text, which is what lets a provider backend avoid inventing its own grant model.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityDecision {
    /// The capability whose authority was requested.
    pub capability: CanonicalSymbolId,
    /// The mode this decision was reached under.
    pub mode: AuthorityMode,
    /// Whether the operation may proceed.
    pub outcome: AuthorityOutcome,
    /// The grant context the decision was reached against.
    pub grant: AuthorityGrantContext,
    /// Where the decision can be reported in source.
    pub provenance: AuthorityProvenance,
}

impl AuthorityDecision {
    /// Build an allowed decision.
    pub fn allowed(
        capability: CanonicalSymbolId,
        mode: AuthorityMode,
        grant: AuthorityGrantContext,
        provenance: AuthorityProvenance,
    ) -> Self {
        Self {
            capability,
            mode,
            outcome: AuthorityOutcome::Allowed,
            grant,
            provenance,
        }
    }

    /// Build a denied decision carrying its machine-usable reason.
    pub fn denied(
        capability: CanonicalSymbolId,
        mode: AuthorityMode,
        reason: AuthorityDenialReason,
        grant: AuthorityGrantContext,
        provenance: AuthorityProvenance,
    ) -> Self {
        Self {
            capability,
            mode,
            outcome: AuthorityOutcome::Denied(reason),
            grant,
            provenance,
        }
    }

    /// Whether the operation may perform its authority-bearing behavior.
    pub const fn is_allowed(&self) -> bool {
        matches!(self.outcome, AuthorityOutcome::Allowed)
    }

    /// The denial reason, when this decision denied the operation.
    pub const fn denial_reason(&self) -> Option<AuthorityDenialReason> {
        match self.outcome {
            AuthorityOutcome::Allowed => None,
            AuthorityOutcome::Denied(reason) => Some(reason),
        }
    }
}

impl std::fmt::Display for AuthorityDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let outcome = match self.outcome {
            AuthorityOutcome::Allowed => "allowed".to_string(),
            AuthorityOutcome::Denied(reason) => format!("denied:{}", reason.as_str()),
        };
        let grants = render_authority_grants(&self.grant.effective_grants);
        let ceiling = self
            .grant
            .ceiling
            .as_ref()
            .map_or_else(|| "none".to_string(), |values| render_authority_grants(values));
        write!(
            f,
            "{} {} {} grants=[{}] ceiling=[{}] <- {}",
            self.capability.declaration_name,
            self.mode.as_str(),
            outcome,
            grants,
            ceiling,
            self.provenance.operation.declaration_name
        )
    }
}

/// Render canonical grant identities for an inspectable maintainer-facing fact snapshot.
pub(super) fn render_authority_grants(grants: &[CanonicalSymbolId]) -> String {
    grants.iter().map(render_authority_grant).collect::<Vec<_>>().join(",")
}

/// Render every identity component that distinguishes one canonical capability grant from another.
pub(super) fn render_authority_grant(grant: &CanonicalSymbolId) -> String {
    let origin = match &grant.origin {
        SymbolOrigin::Module(path) => format!("module:{}", path.join(".")),
        SymbolOrigin::Package { library, module_path } => {
            format!("package:{library}:{}", module_path.join("."))
        }
        SymbolOrigin::RustCrate(path) => format!("rust:{}", path.join(".")),
        SymbolOrigin::Builtin => "builtin".to_string(),
    };
    format!(
        "{origin}:{}:{}@{}..{}{}",
        grant.kind.as_str(),
        grant.declaration_name,
        grant.declaration_span.start,
        grant.declaration_span.end,
        grant
            .declaration_span
            .expansion
            .map(|value| format!("/expansion.{}", hex::encode(value)))
            .unwrap_or_default()
    )
}
