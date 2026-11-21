//! The public record surface: every reference model's fields by name and type, and its defaults.

use crate::lowering_events::{
    AdmittedDefinition, AdmittedFunction, AdmittedModule, AdmittedPlacement, EventPosition, EventReference,
    LoweringEvent, LoweringRefusal, NativeInterface,
};
use crate::overload_members::{MethodLookup, MethodReads};
use crate::plan_module_records::{PlanModuleFunction, PlanModuleValue};
use crate::{
    CheckedBodyCounts, CheckerRecordCounts, CompiledBodyDependencyRead, CompiledBodyFallback, CompiledBodyOutput,
    CompiledBodyOutputs, CompiledBodyValue, CompiledNamespaceValue, CompilerContributors, CompilerExportMember,
    GraphBodyFields, GraphBodyValue, GraphNamespaceValue, ManifestCacheValue, ManifestInventory, NamespaceAtom,
    NamespaceEntry, NamespaceField, NamespaceIdentity, NamespaceOrder, NamespacePublication, NamespaceReference,
    PayloadAdmission, RecordKey, SettledCallContract, SettledCallRead,
};

/// A complete key whose context keeps its reference default.
fn key() -> RecordKey {
    RecordKey {
        family: "declaration".into(),
        identity: "i".into(),
        identity_profile: "p".into(),
        origin: "o".into(),
        facet: "f".into(),
        context: String::new(),
    }
}

/// Models whose every field has a reference default publish those defaults through `Default`.
#[test]
fn fully_defaulted_models_keep_reference_defaults() {
    assert_eq!(
        CheckedBodyCounts::default(),
        CheckedBodyCounts {
            checked: 0,
            reused: 0,
            decoded: 0
        }
    );
    let counts = CheckerRecordCounts::default();
    assert_eq!(counts.summary, "0/0 reusable declarations");
    assert_eq!((counts.reusable_declarations, counts.all_declarations), (0, 0));
    assert!(counts.artifact_refusals.is_empty() && counts.restored.is_empty() && counts.restored_body_ir.is_empty());
    let order = NamespaceOrder::default();
    assert_eq!((order.rank, order.evidence.as_str()), (0, "compiled-check"));
    assert!(order.members.is_empty() && order.certificate.is_empty());
    let reads = MethodReads::default();
    assert!(reads.groups.is_empty() && reads.retained.is_empty());
}

/// Compiled outputs carry every reference field, including the namespace contract and publication fields.
#[test]
fn compiled_outputs_carry_every_reference_field() {
    let read = SettledCallRead {
        reader: "r".into(),
        target: "t".into(),
        digest: "d".into(),
    };
    let namespace = CompiledNamespaceValue {
        module: "m".into(),
        interface: vec![1],
        body_ir: vec![2],
        dependencies: vec![],
        lowering_inputs: vec![],
        package_roots: vec![],
        call_reads: vec![read.clone()],
        semantic_interface: vec![],
        member_witnesses: vec![],
        call_contracts: Some(vec![SettledCallContract {
            target: "t".into(),
            digest: "d".into(),
            reader: "r".into(),
            owner: "o".into(),
        }]),
        identity_proof: vec![],
        published_record: vec![],
    };
    let value = CompiledBodyValue {
        module: "m".into(),
        name: "n".into(),
        identity: "i".into(),
        declaration: vec![],
        local: vec![],
        r#final: vec![],
        body_ir: vec![],
        dependency_reads: vec![CompiledBodyDependencyRead {
            package: None,
            module: "m".into(),
            name: "n".into(),
            identity: "i".into(),
            reference_subject: "s".into(),
        }],
        scope_names: vec![],
    };
    let fallback = CompiledBodyFallback {
        module: "m".into(),
        name: "n".into(),
        identity: None,
        reason: "r".into(),
    };
    let outputs = CompiledBodyOutputs {
        namespaces: vec![namespace.clone()],
        values: vec![value],
        fallbacks: vec![],
    };
    assert_eq!(outputs.namespaces.len(), 1);
    assert_ne!(
        CompiledBodyOutput::Namespace(namespace),
        CompiledBodyOutput::Fallback(fallback)
    );
    assert_eq!(
        CompiledBodyOutput::LegacyNamespace("m".into()),
        CompiledBodyOutput::LegacyNamespace("m".into())
    );
    let graph = GraphNamespaceValue {
        module: "m".into(),
        interface: vec![],
        body_ir: vec![],
        lowering_inputs: vec![],
        package_roots: vec![],
        call_reads: vec![read],
        call_reads_known: true,
        identity_proof: vec![],
    };
    assert!(graph.call_reads_known);
    let body = GraphBodyValue {
        identity: "i".into(),
        local: vec![],
        r#final: vec![],
        body_ir: vec![],
    };
    let fields = GraphBodyFields {
        identity: body.identity,
        local: vec![],
        final_fields: vec![],
        body_ir: vec![],
    };
    assert!(fields.local.is_empty());
}

/// Lowering, plan and namespace records carry every reference field.
#[test]
fn lowering_plan_and_namespace_records_carry_every_reference_field() {
    let reference = EventReference {
        kind: "model".into(),
        index: 0,
        name: "M".into(),
    };
    let position = EventPosition { line: 1, column: 2 };
    let event = LoweringEvent {
        identity: "i".into(),
        key: "k".into(),
        function: "f".into(),
        product: vec![],
        reused: false,
        admitted: false,
        module: String::new(),
        role: String::new(),
        demand: String::new(),
        runtime_requirements: 0,
        references: vec![reference.clone()],
        positions: vec![position],
    };
    let refusal = LoweringRefusal {
        identity: "i".into(),
        function: "f".into(),
        refusal: "r".into(),
    };
    let interface = NativeInterface {
        entry: "e".into(),
        plan: vec![],
    };
    let definition = AdmittedDefinition {
        function: "f".into(),
        identity: "i".into(),
        key: "k".into(),
        digest: "d".into(),
    };
    let placement = AdmittedPlacement {
        key: "k".into(),
        positions: vec![position],
    };
    let function = AdmittedFunction {
        function: "f".into(),
        identity: "i".into(),
        references: vec![reference],
        placement: Some(placement),
    };
    let module = AdmittedModule {
        module: "m".into(),
        complete: true,
        functions: vec![function],
        inspected: false,
    };
    let plan = PlanModuleValue {
        module: "m".into(),
        functions: vec![PlanModuleFunction {
            identity: "i".into(),
            symbol: "s".into(),
            function: "f".into(),
            key: "k".into(),
            event: vec![],
        }],
    };
    let lookup = MethodLookup {
        reader: "r".into(),
        module: "m".into(),
        receiver: "x".into(),
        method: "m".into(),
        candidates: vec![],
    };
    let atom = NamespaceAtom::Fields(vec![NamespaceField {
        identity: 1,
        value: NamespaceAtom::Mapping(vec![NamespaceEntry {
            key: NamespaceAtom::Missing,
            value: NamespaceAtom::Real(1.5),
        }]),
    }]);
    let identity = NamespaceIdentity {
        name: "n".into(),
        path: vec![],
    };
    let publication = NamespacePublication {
        interface: vec![],
        semantic_interface: vec![],
    };
    let reference = NamespaceReference {
        key: key(),
        segment: "s".into(),
        offset: 0,
        length: 0,
        frame_digest: vec![],
        value_digest: "v".into(),
    };
    assert!(event.references.len() == 1 && module.functions.len() == 1 && plan.functions.len() == 1);
    assert!(refusal.refusal == "r" && interface.entry == "e" && definition.digest == "d");
    assert!(lookup.candidates.is_empty() && identity.path.is_empty() && publication.interface.is_empty());
    assert!(matches!(atom, NamespaceAtom::Fields(_)) && reference.offset == 0);
}

/// Unit, manifest and contributor records carry every reference field.
#[test]
fn unit_manifest_and_contributor_records_carry_every_reference_field() {
    let admission = PayloadAdmission {
        identity: "i".into(),
        payload: "p".into(),
        observation: "o".into(),
        provenance: vec![],
        facts: String::new(),
        refusal: String::new(),
    };
    let cache = ManifestCacheValue {
        digest: vec![],
        payload: vec![],
    };
    let inventory = ManifestInventory {
        name: "n".into(),
        version: "v".into(),
        features: vec![],
        vocabulary: false,
        operations: false,
        native_unions: false,
        rust_paths: vec![],
        declares_abi_cache: false,
        has_abi_cache: false,
        abi_digest: None,
    };
    let contributors = CompilerContributors {
        profile: "release".into(),
        scope: "s".into(),
        vocabulary: vec![],
        operations: vec![],
    };
    let member = CompilerExportMember {
        key: "k".into(),
        revision: "r".into(),
    };
    assert!(admission.facts.is_empty() && cache.digest.is_empty() && inventory.abi_digest.is_none());
    assert!(contributors.vocabulary.is_empty() && member.key == "k");
}

/// Every reference field default is published to Incan callers under the name the frontend reads, with the
/// reference value.
#[test]
fn every_reference_field_default_is_published() {
    use crate::body_ir_declarations::CompilerBodyIrModule;
    use crate::compiler_records::HirSourceSpan;
    use crate::crumb_rows::ModelField;
    use crate::record_scope::{DEFAULT_READ_BYTES, RecordRequest};

    assert!(CompilerBodyIrModule::__incan_default_runtime_binding_scopes().is_empty());
    let counts = [
        CheckedBodyCounts::__incan_default_checked(),
        CheckedBodyCounts::__incan_default_reused(),
        CheckedBodyCounts::__incan_default_decoded(),
        CheckerRecordCounts::__incan_default_reusable_declarations(),
        CheckerRecordCounts::__incan_default_all_declarations(),
        LoweringEvent::__incan_default_runtime_requirements(),
        NamespaceOrder::__incan_default_rank(),
    ];
    assert_eq!(counts, [0; 7]);
    assert_eq!(
        CheckerRecordCounts::__incan_default_summary(),
        "0/0 reusable declarations"
    );
    assert!(CheckerRecordCounts::__incan_default_artifact_refusals().is_empty());
    assert!(CheckerRecordCounts::__incan_default_restored().is_empty());
    assert!(CheckerRecordCounts::__incan_default_restored_body_ir().is_empty());
    assert!(GraphNamespaceValue::__incan_default_lowering_inputs().is_empty());
    assert!(GraphNamespaceValue::__incan_default_package_roots().is_empty());
    assert!(GraphNamespaceValue::__incan_default_call_reads().is_empty());
    assert!(GraphNamespaceValue::__incan_default_call_reads_known());
    assert!(GraphNamespaceValue::__incan_default_identity_proof().is_empty());
    assert!(CompiledBodyValue::__incan_default_dependency_reads().is_empty());
    assert!(CompiledBodyValue::__incan_default_scope_names().is_empty());
    assert!(CompiledNamespaceValue::__incan_default_lowering_inputs().is_empty());
    assert!(CompiledNamespaceValue::__incan_default_package_roots().is_empty());
    assert!(CompiledNamespaceValue::__incan_default_call_reads().is_empty());
    assert!(CompiledNamespaceValue::__incan_default_semantic_interface().is_empty());
    assert!(CompiledNamespaceValue::__incan_default_member_witnesses().is_empty());
    assert_eq!(CompiledNamespaceValue::__incan_default_call_contracts(), None);
    assert!(CompiledNamespaceValue::__incan_default_identity_proof().is_empty());
    assert!(CompiledNamespaceValue::__incan_default_published_record().is_empty());
    assert!(CompiledBodyOutputs::__incan_default_namespaces().is_empty());
    assert!(CompilerContributors::__incan_default_vocabulary().is_empty());
    assert!(CompilerContributors::__incan_default_operations().is_empty());
    assert_eq!(HirSourceSpan::__incan_default_expansion(), None);
    assert!(ModelField::__incan_default_canonical());
    assert!(!LoweringEvent::__incan_default_admitted());
    let texts = [
        LoweringEvent::__incan_default_module(),
        LoweringEvent::__incan_default_role(),
        LoweringEvent::__incan_default_demand(),
        RecordKey::__incan_default_context(),
        PayloadAdmission::__incan_default_facts(),
        PayloadAdmission::__incan_default_refusal(),
    ];
    assert!(texts.iter().all(String::is_empty));
    assert!(LoweringEvent::__incan_default_references().is_empty());
    assert!(LoweringEvent::__incan_default_positions().is_empty());
    assert_eq!(AdmittedFunction::__incan_default_placement(), None);
    assert!(!AdmittedModule::__incan_default_inspected());
    assert_eq!(NamespaceOrder::__incan_default_evidence(), "compiled-check");
    assert!(NamespaceOrder::__incan_default_members().is_empty());
    assert!(NamespaceOrder::__incan_default_certificate().is_empty());
    assert!(MethodReads::__incan_default_groups().is_empty());
    assert!(MethodReads::__incan_default_retained().is_empty());
    assert_eq!(RecordRequest::__incan_default_byte_limit(), DEFAULT_READ_BYTES);
}
