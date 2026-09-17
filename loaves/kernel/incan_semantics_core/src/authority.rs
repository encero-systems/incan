//! The compiler-to-runtime seam for RFC 104 authority decisions.
//!
//! The `facts` module defines what an authority decision *is*. This module defines how a consumer *obtains* one, and it
//! exists so that no consumer has to define grant semantics of its own. A provider-operation plan, a runtime
//! entrypoint, and a test harness all ask the same question through [`AuthorityDecisionSource`] and all receive the
//! same [`AuthorityDecision`].
//!
//! The seam is deliberately narrow. A consumer supplies an [`AuthorityRequest`] — which capability, which operation
//! asked, where it asked, and with what scope — and receives a decision. It never inspects a grant set, never
//! intersects a ceiling, and never decides what a mode means; those belong to whoever implements this trait.

use std::collections::BTreeSet;

use crate::facts::{
    AuthorityDecision, AuthorityDenialReason, AuthorityGrantContext, AuthorityMode, AuthorityProvenance,
    CanonicalSymbolId,
};

/// One operation's request for a capability's authority.
///
/// The requesting operation is identified canonically rather than by name so a decision stays reportable without a
/// consumer re-reading source, and so the same request shape works for a stdlib call, a package-defined domain
/// operation, and a provider operation alike.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuthorityRequest {
    /// The capability whose authority is being requested.
    pub capability: CanonicalSymbolId,
    /// The operation making the request.
    pub operation: CanonicalSymbolId,
    /// The use site, which is where a denial is reported.
    pub request_span: crate::HirSourceSpan,
    /// Scope dimensions the operation is requesting, as `(dimension, value)`.
    pub requested_scope: Vec<(String, String)>,
    /// The grant spelling to suggest if this request is denied, such as `host.http.request`.
    pub suggested_grant: String,
}

impl AuthorityRequest {
    /// Build the provenance a decision about this request carries.
    fn provenance(&self) -> AuthorityProvenance {
        AuthorityProvenance {
            operation: self.operation.clone(),
            request_span: self.request_span,
            suggested_grant: self.suggested_grant.clone(),
        }
    }

    /// Build the grant context a decision about this request carries.
    fn grant_context(
        &self,
        effective_grants: Vec<CanonicalSymbolId>,
        ceiling: Option<Vec<CanonicalSymbolId>>,
    ) -> AuthorityGrantContext {
        AuthorityGrantContext {
            requested_scope: self.requested_scope.clone(),
            effective_grants,
            ceiling,
        }
    }
}

/// Whatever decides RFC 104 authority for a run.
///
/// This is the one seam between the compiler's canonical facts and a runtime's policy. Implementors own mode
/// semantics, grant resolution, and ceiling intersection; consumers own none of it. Keeping the trait object-safe is
/// deliberate: a consumer holds `&dyn AuthorityDecisionSource` so a governed host, a permissive local run, and a test
/// double are interchangeable without the consumer knowing which it has.
pub trait AuthorityDecisionSource {
    /// Decide whether one operation may exercise one capability's authority.
    fn decide(&self, request: &AuthorityRequest) -> AuthorityDecision;
}

/// An authority source backed by a fixed grant set and an optional host ceiling.
///
/// This is the reference implementation of RFC 104's resolution rules, and the one a test or a simple local run uses.
/// It is not a policy engine: it holds no budgets and no scope matchers, so it denies for the two reasons a grant set
/// alone can establish.
#[derive(Debug, Clone)]
pub struct StaticAuthority {
    mode: AuthorityMode,
    granted: BTreeSet<CanonicalSymbolId>,
    ceiling: Option<BTreeSet<CanonicalSymbolId>>,
}

impl StaticAuthority {
    /// Build an authority source for `mode` granting exactly these canonical capability identities, with no host
    /// ceiling.
    ///
    /// A grant is the compiler-resolved capability identity, never its rendered diagnostic spelling. That keeps a
    /// request for one package or module capability from acquiring authority through another capability that happens
    /// to render to the same text.
    pub fn new(mode: AuthorityMode, granted: impl IntoIterator<Item = CanonicalSymbolId>) -> Self {
        Self {
            mode,
            granted: granted.into_iter().collect(),
            ceiling: None,
        }
    }

    /// Bound this source by a host-supplied ceiling.
    ///
    /// RFC 104 makes the ceiling a grant source distinct from the invocation's own request, combined by
    /// **intersection and never union**: an invocation can only ever receive less than its ceiling allows, however
    /// much it asks for. A grant outside the ceiling is therefore denied even though it was requested.
    pub fn with_ceiling(mut self, ceiling: impl IntoIterator<Item = CanonicalSymbolId>) -> Self {
        self.ceiling = Some(ceiling.into_iter().collect());
        self
    }

    /// The effective project grant set bounded by the host ceiling, when one is supplied.
    pub fn effective_grants(&self) -> BTreeSet<CanonicalSymbolId> {
        match &self.ceiling {
            Some(ceiling) => self.granted.intersection(ceiling).cloned().collect(),
            None => self.granted.clone(),
        }
    }
}

impl Default for StaticAuthority {
    /// Build the default observe-mode authority source with no explicit grants or ceiling.
    fn default() -> Self {
        Self::new(AuthorityMode::default(), Vec::new())
    }
}

impl AuthorityDecisionSource for StaticAuthority {
    fn decide(&self, request: &AuthorityRequest) -> AuthorityDecision {
        let effective_grants = self.effective_grants();
        let grant = request.grant_context(
            effective_grants
                .iter()
                .filter(|grant| *grant == &request.capability)
                .cloned()
                .collect(),
            self.ceiling.as_ref().map(|values| values.iter().cloned().collect()),
        );
        let provenance = request.provenance();

        // Permissive and observe runs never deny; the difference between them is whether receipts are emitted, which
        // is a reporting concern rather than an authority one.
        if !matches!(self.mode, AuthorityMode::Governed) {
            return AuthorityDecision::allowed(request.capability.clone(), self.mode, grant, provenance);
        }

        let outside_ceiling = self
            .ceiling
            .as_ref()
            .is_some_and(|ceiling| !ceiling.contains(&request.capability));

        // Report the ceiling first: an invocation that asked for authority its host never permitted is a different
        // failure from one that simply never asked, and only the former tells the caller their request was capped.
        let reason = if outside_ceiling {
            Some(AuthorityDenialReason::OutsideCeiling)
        } else if !self.granted.contains(&request.capability) {
            Some(AuthorityDenialReason::NotGranted)
        } else {
            None
        };

        match reason {
            Some(reason) => AuthorityDecision::denied(request.capability.clone(), self.mode, reason, grant, provenance),
            None => AuthorityDecision::allowed(request.capability.clone(), self.mode, grant, provenance),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::SemanticSourceTargetKind;

    /// Build a request for `host.http.request` made by `app.billing.charge`.
    fn request() -> AuthorityRequest {
        AuthorityRequest {
            capability: CanonicalSymbolId::module_declaration(
                vec!["host".to_string(), "http".to_string()],
                "request",
                SemanticSourceTargetKind::Capability,
                crate::HirSourceSpan::new(10, 20),
            ),
            operation: CanonicalSymbolId::module_declaration(
                vec!["app".to_string(), "billing".to_string()],
                "charge",
                SemanticSourceTargetKind::Function,
                crate::HirSourceSpan::new(80, 96),
            ),
            request_span: crate::HirSourceSpan::new(120, 140),
            requested_scope: vec![("host".to_string(), "api.example.com".to_string())],
            suggested_grant: "host.http.request".to_string(),
        }
    }

    /// A permissive run never denies, so a consumer needs no grant set to run locally.
    #[test]
    fn a_permissive_run_allows_an_ungranted_capability() {
        let authority = StaticAuthority::new(AuthorityMode::Permissive, Vec::new());

        let decision = authority.decide(&request());

        assert!(decision.is_allowed());
        assert_eq!(decision.mode, AuthorityMode::Permissive);
    }

    /// Ordinary development observes authority use unless an invoking project selects another mode.
    #[test]
    fn the_default_authority_source_observes_an_ungranted_capability() {
        let authority = StaticAuthority::default();

        let decision = authority.decide(&request());

        assert!(decision.is_allowed());
        assert_eq!(decision.mode, AuthorityMode::Observe);
        assert!(decision.grant.effective_grants.is_empty());
        assert_eq!(decision.grant.ceiling, None);
    }

    /// A governed run denies what was never granted, and says so in a way a consumer can branch on.
    #[test]
    fn a_governed_run_denies_a_capability_that_was_never_granted() {
        let authority = StaticAuthority::new(AuthorityMode::Governed, Vec::new());

        let decision = authority.decide(&request());

        assert!(!decision.is_allowed());
        assert_eq!(decision.denial_reason(), Some(AuthorityDenialReason::NotGranted));
        assert_eq!(decision.provenance.suggested_grant, "host.http.request");
    }

    /// A governed run allows what was granted, and preserves the requested scope on the decision.
    #[test]
    fn a_governed_run_allows_a_granted_capability_and_keeps_its_scope() {
        let request = request();
        let authority = StaticAuthority::new(AuthorityMode::Governed, [request.capability.clone()]);

        let decision = authority.decide(&request);

        assert!(decision.is_allowed());
        assert_eq!(
            decision.grant.requested_scope,
            vec![("host".to_string(), "api.example.com".to_string())],
        );
    }

    /// A ceiling bounds the grant by intersection: requesting more than the ceiling allows yields less, never more.
    #[test]
    fn a_ceiling_denies_a_grant_the_invocation_requested_but_the_host_did_not_permit() {
        let request = request();
        let ceiling = CanonicalSymbolId::module_declaration(
            vec!["host".to_string(), "fs".to_string()],
            "read",
            SemanticSourceTargetKind::Capability,
            crate::HirSourceSpan::new(30, 40),
        );
        let authority =
            StaticAuthority::new(AuthorityMode::Governed, [request.capability.clone()]).with_ceiling([ceiling.clone()]);

        let decision = authority.decide(&request);

        assert!(
            !decision.is_allowed(),
            "an invocation cannot widen its own authority past its ceiling",
        );
        assert_eq!(decision.denial_reason(), Some(AuthorityDenialReason::OutsideCeiling));
        assert_eq!(decision.grant.ceiling, Some(vec![ceiling]));
        assert!(
            authority.effective_grants().is_empty(),
            "the effective grant is the intersection of ceiling and request, not their union",
        );
    }

    /// A grant present in both the request and the ceiling survives the intersection.
    #[test]
    fn a_ceiling_keeps_a_grant_that_both_sides_permit() {
        let request = request();
        let other_capability = CanonicalSymbolId::module_declaration(
            vec!["host".to_string(), "fs".to_string()],
            "read",
            SemanticSourceTargetKind::Capability,
            crate::HirSourceSpan::new(30, 40),
        );
        let authority = StaticAuthority::new(AuthorityMode::Governed, [request.capability.clone(), other_capability])
            .with_ceiling([request.capability.clone()]);

        let decision = authority.decide(&request);

        assert!(decision.is_allowed());
        assert_eq!(
            authority.effective_grants(),
            [request.capability.clone()].into_iter().collect(),
            "only the capability both sides permit survives",
        );
    }

    /// A durable decision retains the actual grant intersection and the ceiling that bounded it.
    #[test]
    fn a_ceiling_decision_carries_the_effective_grant_and_applicable_constraint() {
        let request = request();
        let other_capability = CanonicalSymbolId::module_declaration(
            vec!["host".to_string(), "fs".to_string()],
            "read",
            SemanticSourceTargetKind::Capability,
            crate::HirSourceSpan::new(30, 40),
        );
        let authority = StaticAuthority::new(AuthorityMode::Governed, [request.capability.clone(), other_capability])
            .with_ceiling([request.capability.clone()]);

        let decision = authority.decide(&request);

        assert!(decision.is_allowed());
        assert_eq!(decision.grant.effective_grants, vec![request.capability.clone()]);
        assert_eq!(decision.grant.ceiling, Some(vec![request.capability]));
    }

    /// A diagnostic suggestion is never an authority key: only the request capability identity can be granted.
    #[test]
    fn a_governed_run_rejects_a_capability_that_only_borrows_an_allowed_diagnostic_spelling() {
        let mut mismatched = request();
        let granted = mismatched.capability.clone();
        mismatched.capability = CanonicalSymbolId::module_declaration(
            vec!["host".to_string(), "fs".to_string()],
            "read",
            SemanticSourceTargetKind::Capability,
            crate::HirSourceSpan::new(30, 40),
        );
        let authority = StaticAuthority::new(AuthorityMode::Governed, [granted]);

        let decision = authority.decide(&mismatched);

        assert!(!decision.is_allowed());
        assert_eq!(decision.denial_reason(), Some(AuthorityDenialReason::NotGranted));
        assert_eq!(decision.provenance.suggested_grant, "host.http.request");
    }

    /// The seam must be usable through a trait object so a host, a local run, and a test double are interchangeable.
    #[test]
    fn the_seam_is_object_safe() {
        let request = request();
        let authority = StaticAuthority::new(AuthorityMode::Governed, [request.capability.clone()]);
        let source: &dyn AuthorityDecisionSource = &authority;

        assert!(source.decide(&request).is_allowed());
    }
}
