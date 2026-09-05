//! RFC 104 operation receipts.
//!
//! An operation receipt is the durable, machine-readable record of what one capability-aware operation actually did.
//! It is the counterpart to [`crate::facts::AuthorityDecision`]: a decision records whether a *requesting* source
//! operation may proceed; this receipt records the distinct, canonical provider operation that actually ran (or would
//! have run), together with the decision's caller/use-site provenance.
//!
//! This module owns one validated, versioned shape for every publisher. A stdlib host boundary, a package-defined
//! domain operation, and a provider operation all use it. Run-report correlation and any backend-execution linkage
//! belong to their owning report/producer contracts: a sequence number alone is intentionally not exported as a
//! standalone cross-run reference.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::facts::{AuthorityDecision, AuthorityMode, CanonicalSymbolId};

/// Current wire format for [`OperationReceipt`].
pub const OPERATION_RECEIPT_SCHEMA_VERSION: u32 = 1;

/// What happened to one capability-aware operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReceiptStatus {
    /// The operation ran and was recorded without governed authority being enforced.
    Observed,
    /// Governed authority was granted and the operation ran to completion.
    Allowed,
    /// Governed authority was refused, so the operation never performed its behavior.
    Denied,
    /// Authority permitted the operation, but the operation itself failed.
    Failed,
    /// The operation ran, but its recorded attributes were redacted before reaching a sink.
    Redacted,
    /// Authority permitted the operation, but it was not attempted for another reason.
    Skipped,
    /// The operation performed part of its work before stopping.
    Partial,
}

impl ReceiptStatus {
    /// Return the stable wire spelling for this status.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Allowed => "allowed",
            Self::Denied => "denied",
            Self::Failed => "failed",
            Self::Redacted => "redacted",
            Self::Skipped => "skipped",
            Self::Partial => "partial",
        }
    }
}

/// How replayable an operation is.
///
/// RFC 104 does not require the runtime to implement replay. It requires the runtime not to make dishonest replay
/// claims, which is why this is recorded per receipt rather than inferred later from the operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReplayClassification {
    /// Replayable from recorded local inputs, such as a filesystem write determined by its recorded arguments.
    Deterministic,
    /// Replay depends on an external system and cannot be exact without a recording.
    External,
    /// Replay needs a recorded fixture or test double.
    FixtureRequired,
    /// Replay data existed but was intentionally not persisted.
    Redacted,
    /// Replay is not supported for this operation.
    Unavailable,
}

impl ReplayClassification {
    /// Return the stable wire spelling for this classification.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::External => "external",
            Self::FixtureRequired => "fixture-required",
            Self::Redacted => "redacted",
            Self::Unavailable => "unavailable",
        }
    }
}

/// How sensitive one recorded attribute's value is.
///
/// This travels with the attribute rather than being decided at the sink, so a receipt that crosses a boundary keeps
/// the provenance a redaction policy needs. Only [`Self::Public`] attributes may retain a cleartext value in this
/// contract; `Internal` and `Secret` require the separate, explicit reveal policy RFC 104 reserves for a later slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttributeSensitivity {
    /// Safe to record as written.
    Public,
    /// Must be redacted until an explicit reveal policy permits otherwise.
    Internal,
    /// Never recorded in the clear.
    Secret,
}

impl AttributeSensitivity {
    /// Return the stable wire spelling for this sensitivity level.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Secret => "secret",
        }
    }

    /// Whether a value at this sensitivity may be persisted by this baseline contract.
    const fn permits_cleartext(self) -> bool {
        matches!(self, Self::Public)
    }
}

/// One attribute recorded on a receipt.
///
/// A redacted attribute keeps its key and sensitivity and drops only the value. That lets a reader distinguish a
/// deliberately withheld HTTP URL from an operation that never recorded one, while a downstream policy still knows
/// why the value is absent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ReceiptAttribute {
    key: String,
    value: Option<String>,
    sensitivity: AttributeSensitivity,
}

#[derive(Deserialize)]
struct ReceiptAttributeWire {
    key: String,
    value: Option<String>,
    sensitivity: AttributeSensitivity,
}

impl<'de> Deserialize<'de> for ReceiptAttribute {
    /// Decode only an attribute that satisfies the redaction invariant.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ReceiptAttributeWire::deserialize(deserializer)?;
        let attribute = Self {
            key: wire.key,
            value: wire.value,
            sensitivity: wire.sensitivity,
        };
        attribute
            .validate()
            .map_err(|violation| serde::de::Error::custom(violation.to_string()))?;
        Ok(attribute)
    }
}

impl ReceiptAttribute {
    /// Record an attribute whose value is safe to persist as written.
    pub fn public(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: Some(value.into()),
            sensitivity: AttributeSensitivity::Public,
        }
    }

    /// Record an attribute whose value was withheld.
    pub fn redacted(key: impl Into<String>, sensitivity: AttributeSensitivity) -> Self {
        Self {
            key: key.into(),
            value: None,
            sensitivity,
        }
    }

    /// The attribute's stable name.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The persisted public value, when one exists.
    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    /// The sensitivity retained whether or not the value was persisted.
    pub const fn sensitivity(&self) -> AttributeSensitivity {
        self.sensitivity
    }

    /// Whether this attribute's value was withheld.
    pub const fn is_redacted(&self) -> bool {
        self.value.is_none()
    }

    /// Reject cleartext whose sensitivity requires a future explicit reveal policy.
    fn validate(&self) -> Result<(), ReceiptContractViolation> {
        if self.value.is_some() && !self.sensitivity.permits_cleartext() {
            return Err(ReceiptContractViolation::CleartextSensitiveAttribute {
                key: self.key.clone(),
                sensitivity: self.sensitivity,
            });
        }
        Ok(())
    }
}

/// Ways a receipt can contradict its own durable contract.
///
/// These are contract violations rather than runtime errors: each one means the receipt claims something its own
/// authority, identity, sensitivity, or persistence fields deny.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptContractViolation {
    /// A receipt used a schema version this compiler does not understand.
    UnsupportedSchemaVersion {
        /// Version recorded by the receipt.
        found: u32,
        /// Version this compiler supports.
        expected: u32,
    },
    /// A status is incompatible with the linked authority decision's mode or outcome.
    StatusContradictsAuthority {
        /// The status the receipt claims.
        status: ReceiptStatus,
        /// The mode under which the authority decision was made.
        authority_mode: AuthorityMode,
        /// Whether the linked decision allowed the operation.
        authority_allowed: bool,
    },
    /// The receipt's capability differs from the capability the authority decision evaluated.
    CapabilityContradictsAuthority,
    /// The receipt's source span differs from the authority decision's requesting use site.
    SourceSpanContradictsAuthority,
    /// A non-public attribute retained a cleartext value without an explicit reveal policy.
    CleartextSensitiveAttribute {
        /// The offending attribute key.
        key: String,
        /// The sensitivity that required redaction.
        sensitivity: AttributeSensitivity,
    },
    /// The receipt withheld attribute values yet claims replay from recorded local inputs.
    DeterministicReplayOverRedactedAttributes {
        /// The keys whose values were withheld.
        redacted_keys: Vec<String>,
    },
}

impl std::fmt::Display for ReceiptContractViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { found, expected } => write!(
                f,
                "unsupported operation-receipt schema version {found}; expected {expected}"
            ),
            Self::StatusContradictsAuthority {
                status,
                authority_mode,
                authority_allowed,
            } => write!(
                f,
                "receipt status `{}` contradicts a {} authority decision that {}",
                status.as_str(),
                authority_mode.as_str(),
                if *authority_allowed { "allowed" } else { "denied" },
            ),
            Self::CapabilityContradictsAuthority => {
                f.write_str("receipt capability contradicts its authority decision")
            }
            Self::SourceSpanContradictsAuthority => {
                f.write_str("receipt source span contradicts its authority decision")
            }
            Self::CleartextSensitiveAttribute { key, sensitivity } => write!(
                f,
                "receipt attribute `{key}` retains cleartext despite {} sensitivity",
                sensitivity.as_str(),
            ),
            Self::DeterministicReplayOverRedactedAttributes { redacted_keys } => write!(
                f,
                "receipt claims deterministic replay but withheld {}",
                redacted_keys.join(", "),
            ),
        }
    }
}

/// The durable record of one capability-aware operation.
///
/// Construction derives the capability and source use site from the linked [`AuthorityDecision`], while the distinct
/// `operation` identity names the provider/callable operation that actually ran. This keeps authority requester
/// provenance from being mistaken for the invoked provider operation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationReceipt {
    schema_version: u32,
    sequence_id: u64,
    capability: CanonicalSymbolId,
    operation: CanonicalSymbolId,
    operation_kind: String,
    status: ReceiptStatus,
    authority: AuthorityDecision,
    source_span: crate::HirSourceSpan,
    parent_context: Option<u64>,
    attributes: Vec<ReceiptAttribute>,
    replay: ReplayClassification,
}

#[derive(Serialize)]
struct OperationReceiptWireRef<'a> {
    schema_version: u32,
    sequence_id: u64,
    capability: &'a CanonicalSymbolId,
    operation: &'a CanonicalSymbolId,
    operation_kind: &'a str,
    status: ReceiptStatus,
    authority: &'a AuthorityDecision,
    source_span: crate::HirSourceSpan,
    parent_context: Option<u64>,
    attributes: &'a [ReceiptAttribute],
    replay: ReplayClassification,
}

#[derive(Deserialize)]
struct OperationReceiptWire {
    schema_version: u32,
    sequence_id: u64,
    capability: CanonicalSymbolId,
    operation: CanonicalSymbolId,
    operation_kind: String,
    status: ReceiptStatus,
    authority: AuthorityDecision,
    source_span: crate::HirSourceSpan,
    parent_context: Option<u64>,
    attributes: Vec<ReceiptAttribute>,
    replay: ReplayClassification,
}

impl Serialize for OperationReceipt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate()
            .map_err(|violation| serde::ser::Error::custom(violation.to_string()))?;
        OperationReceiptWireRef {
            schema_version: self.schema_version,
            sequence_id: self.sequence_id,
            capability: &self.capability,
            operation: &self.operation,
            operation_kind: &self.operation_kind,
            status: self.status,
            authority: &self.authority,
            source_span: self.source_span,
            parent_context: self.parent_context,
            attributes: &self.attributes,
            replay: self.replay,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OperationReceipt {
    /// Decode and validate the complete versioned receipt before exposing it to a consumer.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = OperationReceiptWire::deserialize(deserializer)?;
        let receipt = Self {
            schema_version: wire.schema_version,
            sequence_id: wire.sequence_id,
            capability: wire.capability,
            operation: wire.operation,
            operation_kind: wire.operation_kind,
            status: wire.status,
            authority: wire.authority,
            source_span: wire.source_span,
            parent_context: wire.parent_context,
            attributes: wire.attributes,
            replay: wire.replay,
        };
        receipt
            .validate()
            .map_err(|violation| serde::de::Error::custom(violation.to_string()))?;
        Ok(receipt)
    }
}

impl OperationReceipt {
    /// Build a validated receipt for an operation outcome.
    ///
    /// `operation` is the canonical identity of the provider/callable operation that ran. The linked decision owns
    /// the requester and the source use-site, so this constructor derives the capability and span rather than asking
    /// a publisher to repeat facts that could drift.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sequence_id: u64,
        operation: CanonicalSymbolId,
        operation_kind: impl Into<String>,
        status: ReceiptStatus,
        authority: AuthorityDecision,
        parent_context: Option<u64>,
        attributes: Vec<ReceiptAttribute>,
        replay: ReplayClassification,
    ) -> Result<Self, ReceiptContractViolation> {
        let receipt = Self {
            schema_version: OPERATION_RECEIPT_SCHEMA_VERSION,
            sequence_id,
            capability: authority.capability.clone(),
            operation,
            operation_kind: operation_kind.into(),
            status,
            source_span: authority.provenance.request_span,
            authority,
            parent_context,
            attributes,
            replay,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    /// Build the receipt for a governed denial.
    ///
    /// A denial produces a receipt without the provider ever being invoked. The method is fallible because it refuses
    /// to turn an allowed or non-governed authority decision into a durable governed-denial claim.
    pub fn denied(
        sequence_id: u64,
        operation: CanonicalSymbolId,
        authority: AuthorityDecision,
        operation_kind: impl Into<String>,
    ) -> Result<Self, ReceiptContractViolation> {
        Self::new(
            sequence_id,
            operation,
            operation_kind,
            ReceiptStatus::Denied,
            authority,
            None,
            Vec::new(),
            ReplayClassification::Unavailable,
        )
    }

    /// The receipt wire-schema version.
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Position within the containing run report.
    pub const fn sequence_id(&self) -> u64 {
        self.sequence_id
    }

    /// The capability whose authority the operation required.
    pub fn capability(&self) -> &CanonicalSymbolId {
        &self.capability
    }

    /// The canonical provider/callable operation that ran, or would have run.
    pub fn operation(&self) -> &CanonicalSymbolId {
        &self.operation
    }

    /// The publisher's stable kind label, such as `http.request`.
    pub fn operation_kind(&self) -> &str {
        &self.operation_kind
    }

    /// What happened.
    pub const fn status(&self) -> ReceiptStatus {
        self.status
    }

    /// The authority decision this receipt records the outcome of.
    pub fn authority(&self) -> &AuthorityDecision {
        &self.authority
    }

    /// The source use-site that requested the operation's authority.
    pub const fn source_span(&self) -> crate::HirSourceSpan {
        self.source_span
    }

    /// The enclosing context sequence id, when the operation ran inside one.
    pub const fn parent_context(&self) -> Option<u64> {
        self.parent_context
    }

    /// Operation-specific attributes, redacted or public.
    pub fn attributes(&self) -> &[ReceiptAttribute] {
        &self.attributes
    }

    /// How replayable this operation is.
    pub const fn replay(&self) -> ReplayClassification {
        self.replay
    }

    /// The keys whose values were withheld.
    pub fn redacted_keys(&self) -> Vec<String> {
        self.attributes
            .iter()
            .filter(|attribute| attribute.is_redacted())
            .map(|attribute| attribute.key.clone())
            .collect()
    }

    /// Check that this receipt does not contradict its own durable contract.
    pub fn validate(&self) -> Result<(), ReceiptContractViolation> {
        if self.schema_version != OPERATION_RECEIPT_SCHEMA_VERSION {
            return Err(ReceiptContractViolation::UnsupportedSchemaVersion {
                found: self.schema_version,
                expected: OPERATION_RECEIPT_SCHEMA_VERSION,
            });
        }

        if self.capability != self.authority.capability {
            return Err(ReceiptContractViolation::CapabilityContradictsAuthority);
        }

        if self.source_span != self.authority.provenance.request_span {
            return Err(ReceiptContractViolation::SourceSpanContradictsAuthority);
        }

        if !status_matches_authority(self.status, &self.authority) {
            return Err(ReceiptContractViolation::StatusContradictsAuthority {
                status: self.status,
                authority_mode: self.authority.mode,
                authority_allowed: self.authority.is_allowed(),
            });
        }

        for attribute in &self.attributes {
            attribute.validate()?;
        }

        if self.replay == ReplayClassification::Deterministic {
            let redacted_keys = self.redacted_keys();
            if !redacted_keys.is_empty() {
                return Err(ReceiptContractViolation::DeterministicReplayOverRedactedAttributes { redacted_keys });
            }
        }

        Ok(())
    }
}

/// Whether this status is truthful for the decision's authority mode and outcome.
fn status_matches_authority(status: ReceiptStatus, authority: &AuthorityDecision) -> bool {
    matches!(
        (authority.mode, authority.is_allowed(), status),
        (AuthorityMode::Governed, false, ReceiptStatus::Denied)
            | (
                AuthorityMode::Governed,
                true,
                ReceiptStatus::Allowed
                    | ReceiptStatus::Failed
                    | ReceiptStatus::Redacted
                    | ReceiptStatus::Skipped
                    | ReceiptStatus::Partial,
            )
            | (
                AuthorityMode::Observe,
                true,
                ReceiptStatus::Observed
                    | ReceiptStatus::Failed
                    | ReceiptStatus::Redacted
                    | ReceiptStatus::Skipped
                    | ReceiptStatus::Partial,
            )
    )
}

impl std::fmt::Display for OperationReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "#{} {} {} {} replay={}",
            self.sequence_id,
            self.operation_kind,
            self.operation.declaration_name,
            self.status.as_str(),
            self.replay.as_str(),
        )?;
        let redacted = self.redacted_keys();
        if !redacted.is_empty() {
            write!(f, " redacted=[{}]", redacted.join(","))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{AuthorityDenialReason, AuthorityGrantContext, AuthorityProvenance, SemanticSourceTargetKind};

    /// Build an authority decision for `host.http.request` requested by `app.billing.charge`.
    fn authority(allowed: bool, mode: AuthorityMode) -> AuthorityDecision {
        let capability = CanonicalSymbolId::module_declaration(
            vec!["host".to_string(), "http".to_string()],
            "request",
            SemanticSourceTargetKind::Capability,
            crate::HirSourceSpan::new(10, 20),
        );
        let provenance = AuthorityProvenance {
            operation: CanonicalSymbolId::module_declaration(
                vec!["app".to_string(), "billing".to_string()],
                "charge",
                SemanticSourceTargetKind::Function,
                crate::HirSourceSpan::new(80, 96),
            ),
            request_span: crate::HirSourceSpan::new(120, 140),
            suggested_grant: "host.http.request".to_string(),
        };
        let grant = AuthorityGrantContext {
            requested_scope: Vec::new(),
            effective_grants: Vec::new(),
            ceiling: None,
        };
        if allowed {
            AuthorityDecision::allowed(capability, mode, grant, provenance)
        } else {
            AuthorityDecision::denied(capability, mode, AuthorityDenialReason::NotGranted, grant, provenance)
        }
    }

    /// The provider operation is distinct from the caller that requested its capability.
    fn provider_operation() -> CanonicalSymbolId {
        CanonicalSymbolId::module_declaration(
            vec!["std".to_string(), "http".to_string()],
            "request",
            SemanticSourceTargetKind::Function,
            crate::HirSourceSpan::new(200, 214),
        )
    }

    /// Build a governed allowed receipt with the given attributes and replay classification.
    fn allowed_receipt(
        attributes: Vec<ReceiptAttribute>,
        replay: ReplayClassification,
    ) -> Result<OperationReceipt, ReceiptContractViolation> {
        OperationReceipt::new(
            7,
            provider_operation(),
            "http.request",
            ReceiptStatus::Allowed,
            authority(true, AuthorityMode::Governed),
            None,
            attributes,
            replay,
        )
    }

    /// A governed denial produces a receipt without the provider ever being invoked.
    #[test]
    fn a_governed_denial_preserves_the_provider_target_and_request_provenance() -> Result<(), String> {
        let decision = authority(false, AuthorityMode::Governed);
        let receipt = OperationReceipt::denied(1, provider_operation(), decision.clone(), "http.request")
            .map_err(|violation| violation.to_string())?;

        assert_eq!(receipt.status(), ReceiptStatus::Denied);
        assert!(receipt.attributes().is_empty(), "nothing ran, so nothing was recorded");
        assert_eq!(receipt.replay(), ReplayClassification::Unavailable);
        assert_eq!(receipt.capability(), &decision.capability);
        assert_eq!(receipt.operation(), &provider_operation());
        assert_ne!(receipt.operation(), &decision.provenance.operation);
        assert_eq!(receipt.source_span(), decision.provenance.request_span);
        receipt.validate().map_err(|violation| violation.to_string())
    }

    /// An allowed receipt validates and carries its recorded public attributes.
    #[test]
    fn an_allowed_receipt_records_its_attributes() -> Result<(), String> {
        let receipt = allowed_receipt(
            vec![ReceiptAttribute::public("http.method", "GET")],
            ReplayClassification::External,
        )
        .map_err(|violation| violation.to_string())?;

        assert_eq!(receipt.status(), ReceiptStatus::Allowed);
        assert!(receipt.redacted_keys().is_empty());
        assert_eq!(receipt.attributes()[0].value(), Some("GET"));
        receipt.validate().map_err(|violation| violation.to_string())
    }

    /// A failed operation is still authority-permitted: the operation itself did not succeed.
    #[test]
    fn a_failed_operation_keeps_its_allowed_authority() -> Result<(), String> {
        let receipt = OperationReceipt::new(
            7,
            provider_operation(),
            "http.request",
            ReceiptStatus::Failed,
            authority(true, AuthorityMode::Governed),
            None,
            Vec::new(),
            ReplayClassification::External,
        )
        .map_err(|violation| violation.to_string())?;

        receipt.validate().map_err(|violation| violation.to_string())
    }

    /// A redacted attribute keeps its key and sensitivity, and drops only the value.
    #[test]
    fn a_redacted_attribute_keeps_its_key_and_sensitivity() -> Result<(), String> {
        let receipt = OperationReceipt::new(
            7,
            provider_operation(),
            "http.request",
            ReceiptStatus::Redacted,
            authority(true, AuthorityMode::Governed),
            None,
            vec![
                ReceiptAttribute::public("http.method", "GET"),
                ReceiptAttribute::redacted("http.url", AttributeSensitivity::Secret),
            ],
            ReplayClassification::Redacted,
        )
        .map_err(|violation| violation.to_string())?;

        assert_eq!(receipt.redacted_keys(), vec!["http.url".to_string()]);
        let withheld = receipt
            .attributes()
            .iter()
            .find(|attribute| attribute.key() == "http.url")
            .ok_or("the redacted attribute is missing")?;
        assert_eq!(withheld.value(), None);
        assert_eq!(withheld.sensitivity(), AttributeSensitivity::Secret);
        receipt.validate().map_err(|violation| violation.to_string())
    }

    /// A denial constructor refuses a decision that did not make a governed denial.
    #[test]
    fn a_denial_constructor_refuses_an_allowing_or_non_governed_decision() {
        let allowed = OperationReceipt::denied(
            1,
            provider_operation(),
            authority(true, AuthorityMode::Governed),
            "http.request",
        );
        assert_eq!(
            allowed,
            Err(ReceiptContractViolation::StatusContradictsAuthority {
                status: ReceiptStatus::Denied,
                authority_mode: AuthorityMode::Governed,
                authority_allowed: true,
            }),
        );

        let observed = OperationReceipt::denied(
            1,
            provider_operation(),
            authority(false, AuthorityMode::Observe),
            "http.request",
        );
        assert_eq!(
            observed,
            Err(ReceiptContractViolation::StatusContradictsAuthority {
                status: ReceiptStatus::Denied,
                authority_mode: AuthorityMode::Observe,
                authority_allowed: false,
            }),
        );
    }

    /// The durable contract rejects cleartext sensitive attributes, including corrupted in-memory construction.
    #[test]
    fn cleartext_sensitive_attributes_are_refused_before_persistence() -> Result<(), String> {
        let mut receipt =
            allowed_receipt(Vec::new(), ReplayClassification::External).map_err(|violation| violation.to_string())?;
        receipt.attributes = vec![ReceiptAttribute {
            key: "http.authorization".to_string(),
            value: Some("Bearer secret".to_string()),
            sensitivity: AttributeSensitivity::Secret,
        }];

        assert_eq!(
            receipt.validate(),
            Err(ReceiptContractViolation::CleartextSensitiveAttribute {
                key: "http.authorization".to_string(),
                sensitivity: AttributeSensitivity::Secret,
            }),
        );
        assert!(serde_json::to_string(&receipt).is_err());
        Ok(())
    }

    /// Linked authority facts cannot drift in a receipt assembled from untrusted persisted data.
    #[test]
    fn a_corrupted_authority_link_is_rejected() -> Result<(), String> {
        let mut receipt =
            allowed_receipt(Vec::new(), ReplayClassification::External).map_err(|violation| violation.to_string())?;
        receipt.capability = provider_operation();
        assert_eq!(
            receipt.validate(),
            Err(ReceiptContractViolation::CapabilityContradictsAuthority)
        );

        let mut receipt =
            allowed_receipt(Vec::new(), ReplayClassification::External).map_err(|violation| violation.to_string())?;
        receipt.source_span = crate::HirSourceSpan::new(1, 2);
        assert_eq!(
            receipt.validate(),
            Err(ReceiptContractViolation::SourceSpanContradictsAuthority)
        );
        Ok(())
    }

    /// Claiming deterministic replay over withheld inputs is the dishonest claim RFC 104 forbids.
    #[test]
    fn deterministic_replay_over_redacted_attributes_is_rejected() {
        let receipt = OperationReceipt::new(
            7,
            provider_operation(),
            "http.request",
            ReceiptStatus::Redacted,
            authority(true, AuthorityMode::Governed),
            None,
            vec![ReceiptAttribute::redacted("http.url", AttributeSensitivity::Secret)],
            ReplayClassification::Deterministic,
        );

        assert_eq!(
            receipt,
            Err(ReceiptContractViolation::DeterministicReplayOverRedactedAttributes {
                redacted_keys: vec!["http.url".to_string()],
            }),
        );
    }

    /// Only the authority-mode/status combinations RFC 104 defines can construct a receipt.
    #[test]
    fn authority_mode_and_status_form_a_checked_truth_table() {
        let valid = [
            (AuthorityMode::Governed, false, ReceiptStatus::Denied),
            (AuthorityMode::Governed, true, ReceiptStatus::Allowed),
            (AuthorityMode::Governed, true, ReceiptStatus::Failed),
            (AuthorityMode::Governed, true, ReceiptStatus::Redacted),
            (AuthorityMode::Governed, true, ReceiptStatus::Skipped),
            (AuthorityMode::Governed, true, ReceiptStatus::Partial),
            (AuthorityMode::Observe, true, ReceiptStatus::Observed),
            (AuthorityMode::Observe, true, ReceiptStatus::Failed),
            (AuthorityMode::Observe, true, ReceiptStatus::Redacted),
            (AuthorityMode::Observe, true, ReceiptStatus::Skipped),
            (AuthorityMode::Observe, true, ReceiptStatus::Partial),
        ];
        for (mode, allowed, status) in valid {
            assert!(
                OperationReceipt::new(
                    1,
                    provider_operation(),
                    "http.request",
                    status,
                    authority(allowed, mode),
                    None,
                    Vec::new(),
                    ReplayClassification::External,
                )
                .is_ok(),
                "{mode:?} {allowed} {status:?} should be accepted",
            );
        }

        let invalid = [
            (AuthorityMode::Governed, true, ReceiptStatus::Observed),
            (AuthorityMode::Governed, false, ReceiptStatus::Failed),
            (AuthorityMode::Permissive, true, ReceiptStatus::Allowed),
            (AuthorityMode::Permissive, true, ReceiptStatus::Observed),
            (AuthorityMode::Permissive, true, ReceiptStatus::Failed),
            (AuthorityMode::Observe, true, ReceiptStatus::Denied),
        ];
        for (mode, allowed, status) in invalid {
            assert!(
                OperationReceipt::new(
                    1,
                    provider_operation(),
                    "http.request",
                    status,
                    authority(allowed, mode),
                    None,
                    Vec::new(),
                    ReplayClassification::External,
                )
                .is_err(),
                "{mode:?} {allowed} {status:?} should be rejected",
            );
        }
    }

    /// The wire contract is versioned and round-trips every first-version receipt outcome.
    #[test]
    fn versioned_json_round_trips_allowed_denied_failed_and_redacted_receipts() -> Result<(), String> {
        let allowed = allowed_receipt(
            vec![ReceiptAttribute::public("http.method", "GET")],
            ReplayClassification::External,
        )
        .map_err(|violation| violation.to_string())?;
        let denied = OperationReceipt::denied(
            8,
            provider_operation(),
            authority(false, AuthorityMode::Governed),
            "http.request",
        )
        .map_err(|violation| violation.to_string())?;
        let failed = OperationReceipt::new(
            9,
            provider_operation(),
            "http.request",
            ReceiptStatus::Failed,
            authority(true, AuthorityMode::Governed),
            None,
            Vec::new(),
            ReplayClassification::External,
        )
        .map_err(|violation| violation.to_string())?;
        let redacted = OperationReceipt::new(
            10,
            provider_operation(),
            "http.request",
            ReceiptStatus::Redacted,
            authority(true, AuthorityMode::Governed),
            None,
            vec![ReceiptAttribute::redacted("http.url", AttributeSensitivity::Secret)],
            ReplayClassification::Redacted,
        )
        .map_err(|violation| violation.to_string())?;

        for (receipt, expected_status, expected_replay) in [
            (allowed, "allowed", "external"),
            (denied, "denied", "unavailable"),
            (failed, "failed", "external"),
            (redacted, "redacted", "redacted"),
        ] {
            let value = serde_json::to_value(&receipt).map_err(|error| error.to_string())?;
            assert_eq!(value["schema_version"], OPERATION_RECEIPT_SCHEMA_VERSION);
            assert_eq!(value["status"], expected_status);
            assert_eq!(value["replay"], expected_replay);
            let decoded: OperationReceipt = serde_json::from_value(value).map_err(|error| error.to_string())?;
            assert_eq!(decoded, receipt);
        }
        Ok(())
    }

    /// Unknown receipt versions and cleartext sensitive JSON are rejected at deserialization.
    #[test]
    fn persisted_receipts_refuse_unknown_schemas_and_cleartext_secrets() -> Result<(), String> {
        let receipt =
            allowed_receipt(Vec::new(), ReplayClassification::External).map_err(|violation| violation.to_string())?;
        let mut unknown_version = serde_json::to_value(&receipt).map_err(|error| error.to_string())?;
        unknown_version["schema_version"] = serde_json::json!(OPERATION_RECEIPT_SCHEMA_VERSION + 1);
        let error = serde_json::from_value::<OperationReceipt>(unknown_version)
            .err()
            .ok_or("unknown schema unexpectedly deserialized")?;
        assert!(
            error
                .to_string()
                .contains("unsupported operation-receipt schema version")
        );

        let mut cleartext_secret = serde_json::to_value(&receipt).map_err(|error| error.to_string())?;
        cleartext_secret["attributes"] = serde_json::json!([{
            "key": "http.authorization",
            "value": "Bearer secret",
            "sensitivity": "secret"
        }]);
        let error = serde_json::from_value::<OperationReceipt>(cleartext_secret)
            .err()
            .ok_or("cleartext secret unexpectedly deserialized")?;
        assert!(
            error
                .to_string()
                .contains("retains cleartext despite secret sensitivity")
        );
        Ok(())
    }

    /// Every status and replay classification has an exact, unique stable wire spelling.
    #[test]
    fn statuses_and_replay_classifications_have_exact_wire_spellings() -> Result<(), String> {
        let statuses = [
            (ReceiptStatus::Observed, "observed"),
            (ReceiptStatus::Allowed, "allowed"),
            (ReceiptStatus::Denied, "denied"),
            (ReceiptStatus::Failed, "failed"),
            (ReceiptStatus::Redacted, "redacted"),
            (ReceiptStatus::Skipped, "skipped"),
            (ReceiptStatus::Partial, "partial"),
        ];
        let status_spellings: std::collections::HashSet<&str> =
            statuses.iter().map(|(status, _)| status.as_str()).collect();
        assert_eq!(
            status_spellings.len(),
            statuses.len(),
            "two statuses share one spelling"
        );
        for (status, wire) in statuses {
            assert_eq!(status.as_str(), wire);
            assert_eq!(serde_json::to_value(status).map_err(|error| error.to_string())?, wire);
        }

        let replays = [
            (ReplayClassification::Deterministic, "deterministic"),
            (ReplayClassification::External, "external"),
            (ReplayClassification::FixtureRequired, "fixture-required"),
            (ReplayClassification::Redacted, "redacted"),
            (ReplayClassification::Unavailable, "unavailable"),
        ];
        let replay_spellings: std::collections::HashSet<&str> =
            replays.iter().map(|(replay, _)| replay.as_str()).collect();
        assert_eq!(
            replay_spellings.len(),
            replays.len(),
            "two replay classifications share one spelling"
        );
        for (replay, wire) in replays {
            assert_eq!(replay.as_str(), wire);
            assert_eq!(serde_json::to_value(replay).map_err(|error| error.to_string())?, wire);
        }
        Ok(())
    }
}
