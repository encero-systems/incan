/// Build a capability identity and a requesting-operation identity for authority-decision tests.
fn authority_fixture() -> (super::CanonicalSymbolId, super::AuthorityProvenance) {
    use super::{AuthorityProvenance, CanonicalSymbolId, SemanticSourceTargetKind};

    let capability = CanonicalSymbolId::module_declaration(
        vec!["host".to_string(), "http".to_string()],
        "request",
        SemanticSourceTargetKind::Capability,
        crate::HirSourceSpan::new(10, 20),
    );
    let operation = CanonicalSymbolId::module_declaration(
        vec!["app".to_string(), "billing".to_string()],
        "charge",
        SemanticSourceTargetKind::Function,
        crate::HirSourceSpan::new(80, 96),
    );
    let provenance = AuthorityProvenance {
        operation,
        request_span: crate::HirSourceSpan::new(120, 140),
        suggested_grant: "host.http.request".to_string(),
    };
    (capability, provenance)
}

/// Build a distinct canonical capability for ceiling and identity-separation tests.
fn fs_read_capability() -> super::CanonicalSymbolId {
    use super::{CanonicalSymbolId, SemanticSourceTargetKind};

    CanonicalSymbolId::module_declaration(
        vec!["host".to_string(), "fs".to_string()],
        "read",
        SemanticSourceTargetKind::Capability,
        crate::HirSourceSpan::new(30, 40),
    )
}

/// An allowed decision must be actionable without a consumer re-reading source or emitted Rust.
#[test]
fn an_allowed_authority_decision_carries_its_mode_and_grant_context() {
    use super::{AuthorityDecision, AuthorityGrantContext, AuthorityMode};

    let (capability, provenance) = authority_fixture();
    let decision = AuthorityDecision::allowed(
        capability.clone(),
        AuthorityMode::Governed,
        AuthorityGrantContext {
            requested_scope: vec![("host".to_string(), "api.example.com".to_string())],
            effective_grants: vec![capability.clone()],
            ceiling: Some(vec![capability.clone()]),
        },
        provenance,
    );

    assert!(decision.is_allowed());
    assert_eq!(decision.denial_reason(), None);
    assert_eq!(decision.mode, AuthorityMode::Governed);
    assert_eq!(decision.grant.effective_grants, vec![capability.clone()]);
    assert_eq!(decision.grant.ceiling, Some(vec![capability]));
    assert_eq!(decision.provenance.suggested_grant, "host.http.request");
}

/// A denied decision must carry a machine-usable reason and enough provenance to raise a source-owned diagnostic.
#[test]
fn a_denied_authority_decision_carries_a_machine_usable_reason_and_its_use_site() {
    use super::{AuthorityDecision, AuthorityDenialReason, AuthorityGrantContext, AuthorityMode};

    let (capability, provenance) = authority_fixture();
    let ceiling = fs_read_capability();
    let decision = AuthorityDecision::denied(
        capability,
        AuthorityMode::Governed,
        AuthorityDenialReason::OutsideCeiling,
        AuthorityGrantContext {
            requested_scope: Vec::new(),
            effective_grants: Vec::new(),
            ceiling: Some(vec![ceiling]),
        },
        provenance,
    );

    assert!(!decision.is_allowed());
    assert_eq!(decision.denial_reason(), Some(AuthorityDenialReason::OutsideCeiling));
    assert_eq!(
        decision.provenance.request_span,
        crate::HirSourceSpan::new(120, 140),
        "a denial is reported at the use site, not at the capability declaration",
    );
    assert_eq!(
        decision.provenance.operation.declaration_name, "charge",
        "the requesting operation stays identified so the diagnostic can name it",
    );
}

/// The fact must be generic over capability publishers rather than assuming a stdlib host capability.
#[test]
fn authority_decisions_work_for_a_library_defined_capability() {
    use super::{
        AuthorityDecision, AuthorityDenialReason, AuthorityGrantContext, AuthorityMode, CanonicalSymbolId,
        SemanticSourceTargetKind,
    };

    let (_, provenance) = authority_fixture();
    let package_capability = CanonicalSymbolId::module_declaration(
        vec!["acme".to_string(), "ledger".to_string()],
        "post_entry",
        SemanticSourceTargetKind::Capability,
        crate::HirSourceSpan::new(4, 14),
    );
    let decision = AuthorityDecision::denied(
        package_capability,
        AuthorityMode::Governed,
        AuthorityDenialReason::NotGranted,
        AuthorityGrantContext {
            requested_scope: Vec::new(),
            effective_grants: Vec::new(),
            ceiling: None,
        },
        provenance,
    );

    assert_eq!(decision.capability.kind, SemanticSourceTargetKind::Capability);
    assert_eq!(decision.capability.declaration_name, "post_entry");
    assert_eq!(decision.denial_reason(), Some(AuthorityDenialReason::NotGranted));
}

/// Every mode and denial reason needs a distinct snapshot spelling.
#[test]
fn authority_modes_and_denial_reasons_have_distinct_spellings() {
    use super::{AuthorityDenialReason as R, AuthorityMode as M};

    let modes = [M::Permissive, M::Observe, M::Governed];
    let mode_spellings: std::collections::HashSet<&str> = modes.iter().map(|m| m.as_str()).collect();
    assert_eq!(mode_spellings.len(), modes.len(), "two modes share one spelling");

    let reasons = [
        R::NotGranted,
        R::OutsideCeiling,
        R::OutOfScope,
        R::BudgetExhausted,
        R::FixtureRequired,
    ];
    let reason_spellings: std::collections::HashSet<&str> = reasons.iter().map(|r| r.as_str()).collect();
    assert_eq!(
        reason_spellings.len(),
        reasons.len(),
        "two denial reasons share one spelling",
    );
}

/// Every declaration category round-trips through its own spelling.
///
/// The two arms are hand-written and 22 variants long; a typo in either would silently reclassify a declaration
/// as `Other`, which compares unequal to the variant it came from and would split one declaration's identity.
#[test]
fn every_source_target_kind_round_trips_through_its_spelling() {
    use super::SemanticSourceTargetKind as K;
    let all = [
        K::Function,
        K::Model,
        K::Class,
        K::Newtype,
        K::Rusttype,
        K::Enum,
        K::TypeAlias,
        K::Partial,
        K::Variant,
        K::Trait,
        K::Capability,
        K::Field,
        K::Method,
        K::Property,
        K::Const,
        K::Static,
        K::Local,
        K::Parameter,
        K::Receiver,
        K::GenericBinder,
        K::Module,
        K::RustItem,
        K::Builtin,
    ];
    for kind in &all {
        assert_eq!(K::from_kind_str(kind.as_str()), *kind, "round trip failed for {kind}");
    }
    let spellings: std::collections::HashSet<&str> = all.iter().map(|kind| kind.as_str()).collect();
    assert_eq!(spellings.len(), all.len(), "two categories share one spelling");
}

use super::*;

#[test]
fn compiler_node_ids_render_kind_prefixed_identity() {
    let id = CompilerNodeId::new(CompilerNodeKind::Declaration, "pkg::module::build");

    assert_eq!(id.kind(), CompilerNodeKind::Declaration);
    assert_eq!(id.path(), "pkg::module::build");
    assert_eq!(id.to_string(), "decl:pkg::module::build");
    assert_eq!(CompilerNodeId::module("pkg::module").to_string(), "module:pkg::module");
    assert_eq!(
        CompilerNodeId::declaration("pkg::module", "build").to_string(),
        "decl:pkg::module::build"
    );
    assert_eq!(
        CompilerNodeId::expression_span("pkg::module", 7, 11).to_string(),
        "expr:pkg::module#7..11"
    );
    assert_eq!(
        CompilerNodeId::declaration_binding_span("pkg::module", 3, 17, 1).to_string(),
        "decl:pkg::module#decl.3..17.binding.1"
    );
    assert_eq!(
        CompilerNodeId::statement_span("pkg::module", 11, 19).to_string(),
        "stmt:pkg::module#stmt.11..19"
    );
    assert_eq!(
        CompilerNodeId::local("pkg::module", "value").to_string(),
        "local:pkg::module::value"
    );
    assert_eq!(
        CompilerNodeId::type_identity("pkg::module", "User").to_string(),
        "type:pkg::module::User"
    );
}

#[test]
fn semantic_fact_store_iterates_subjects_deterministically() {
    let expr = CompilerNodeId::new(CompilerNodeKind::Expression, "pkg::main#expr.2");
    let decl = CompilerNodeId::new(CompilerNodeKind::Declaration, "pkg::main");
    let mut store = SemanticFactStore::new();

    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::Type,
        SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
    ));
    store.insert(SemanticFact::new(
        decl.clone(),
        SemanticFactKind::RuntimeRequirement,
        SemanticFactValue::text("hosted_std"),
    ));

    let subjects = store.subjects().map(ToString::to_string).collect::<Vec<_>>();
    assert_eq!(subjects, vec!["decl:pkg::main", "expr:pkg::main#expr.2"]);
    assert_eq!(store.facts_for(&decl).len(), 1);
    assert!(
        store
            .facts_for(&CompilerNodeId::new(CompilerNodeKind::Type, "missing"))
            .is_empty()
    );
}

#[test]
fn semantic_fact_store_sorts_facts_for_the_same_subject() {
    let expr = CompilerNodeId::new(CompilerNodeKind::Expression, "pkg::main#expr.2");
    let mut store = SemanticFactStore::new();

    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::SymbolTarget,
        SemanticFactValue::source_target(SemanticSourceTarget::from_kind_str(
            vec!["pkg".to_string()],
            "main",
            "function",
        )),
    ));
    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::Type,
        SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
    ));

    let kinds = store.facts_for(&expr).iter().map(|fact| fact.kind).collect::<Vec<_>>();
    assert_eq!(kinds, vec![SemanticFactKind::Type, SemanticFactKind::SymbolTarget]);
}

#[test]
fn semantic_fact_store_queries_facts_by_kind_deterministically() {
    let decl = CompilerNodeId::declaration("pkg::main", "build");
    let expr = CompilerNodeId::expression_span("pkg::main", 3, 8);
    let mut store = SemanticFactStore::new();

    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::SymbolTarget,
        SemanticFactValue::source_target(SemanticSourceTarget::from_kind_str(
            vec!["pkg".to_string()],
            "build",
            "function",
        )),
    ));
    store.insert(SemanticFact::new(
        decl.clone(),
        SemanticFactKind::Type,
        SemanticFactValue::semantic_type(IncanType::Named("Builder".to_string())),
    ));
    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::Type,
        SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
    ));

    let type_subjects = store
        .facts_by_kind(SemanticFactKind::Type)
        .map(|fact| fact.subject.to_string())
        .collect::<Vec<_>>();
    assert_eq!(type_subjects, vec!["decl:pkg::main::build", "expr:pkg::main#3..8"]);

    let expr_kinds = store
        .facts_for_kind(&expr, SemanticFactKind::SymbolTarget)
        .map(|fact| fact.kind)
        .collect::<Vec<_>>();
    assert_eq!(expr_kinds, vec![SemanticFactKind::SymbolTarget]);

    assert_eq!(
        store
            .facts_for_kind(&CompilerNodeId::module("missing"), SemanticFactKind::Type)
            .count(),
        0
    );
}

#[test]
fn semantic_fact_store_extracts_typed_payloads() {
    let expr = CompilerNodeId::expression_span("pkg::main", 3, 8);
    let target = SemanticSourceTarget::from_kind_str(vec!["pkg".to_string()], "build", "function");
    let identity = CanonicalSymbolId::module_declaration(
        vec!["pkg".to_string()],
        "build",
        SemanticSourceTargetKind::Function,
        crate::HirSourceSpan::new(10, 25),
    );
    let mut store = SemanticFactStore::new();

    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::Type,
        SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
    ));
    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::Type,
        SemanticFactValue::text("legacy diagnostic payload"),
    ));
    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::SymbolTarget,
        SemanticFactValue::source_target(target.clone()),
    ));
    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::SymbolIdentity,
        SemanticFactValue::canonical_identity(identity.clone()),
    ));

    let type_facts = store.type_facts_for(&expr).cloned().collect::<Vec<_>>();
    assert_eq!(type_facts, vec![IncanType::Primitive(crate::IncanPrimitiveType::Int)]);

    let source_targets = store.source_targets_for(&expr).cloned().collect::<Vec<_>>();
    assert_eq!(source_targets, vec![target]);

    let identities = store.symbol_identities_for(&expr).cloned().collect::<Vec<_>>();
    assert_eq!(identities, vec![identity]);
}

#[test]
fn semantic_fact_store_renders_deterministic_snapshot() {
    let expr = CompilerNodeId::expression_span("pkg::main", 3, 8);
    let mut store = SemanticFactStore::new();

    store.insert(SemanticFact::new(
        expr.clone(),
        SemanticFactKind::SymbolTarget,
        SemanticFactValue::source_target(SemanticSourceTarget::from_kind_str(
            vec!["pkg".to_string()],
            "build",
            "function",
        )),
    ));
    store.insert(SemanticFact::new(
        expr,
        SemanticFactKind::Type,
        SemanticFactValue::semantic_type(IncanType::Primitive(crate::IncanPrimitiveType::Int)),
    ));
    store.insert(SemanticFact::new(
        CompilerNodeId::module("pkg::main"),
        SemanticFactKind::Diagnostic,
        SemanticFactValue::text("line one\nline two"),
    ));

    assert_eq!(
        store.render_snapshot(),
        "module:pkg::main diagnostic=\"line one\\nline two\"\n\
             expr:pkg::main#3..8 type=int\n\
             expr:pkg::main#3..8 symbol_target=function:pkg::build\n"
    );
}

#[test]
fn semantic_source_target_kind_preserves_known_and_unknown_kinds() {
    assert_eq!(
        SemanticSourceTargetKind::from_kind_str("function"),
        SemanticSourceTargetKind::Function
    );
    assert_eq!(SemanticSourceTargetKind::from_kind_str("macro").as_str(), "macro");
}
