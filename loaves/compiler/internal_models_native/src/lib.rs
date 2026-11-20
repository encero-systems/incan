//! The compiler's shared record contracts, one module per contract; none names Oven, a worker or a native compiler
//! object.
//!
//! `records` reads keyed compiler facts through one revision-bound scope, `record_service` is the producer interface
//! and `checker_records` the canonical checker record codec. Each module keeps the name and public items of the
//! reference Incan module of the same name; the root re-exports the reference facade.

pub mod activation_envelopes;
pub mod body_ir_declarations;
pub mod body_ir_values;
pub mod call_artifact_codecs;
pub mod callable_schema;
pub mod checked_body_counts;
pub mod checked_types;
pub mod checker_field_grammar;
pub mod checker_publication;
pub mod checker_record_models;
pub mod checker_records;
pub mod compiled_body_rows;
pub mod compiler_contributors;
pub mod compiler_export;
pub mod compiler_records;
pub mod crumb_rows;
pub mod graph_crumb_fields;
mod input_fields;
pub mod lowering_events;
pub mod manifest_cache;
pub mod namespace_order;
pub mod namespace_references;
pub mod namespace_rows;
pub mod namespace_schema;
pub mod overload_members;
pub mod plan_function_records;
pub mod plan_module_records;
pub mod plan_wire_rows;
pub mod read_receipts;
pub mod record_scope;
pub mod record_service;
pub mod source_value_records;
pub mod store_record_source;
pub mod unit_admission;

/// The public read boundary over `record_scope`.
pub mod records {
    pub use crate::record_scope::{
        AbsentFact, DeletedFact, MemberFact, MemberSelection, PayloadSelection, PreparationEvidence, READ_SCHEMA,
        ReadCounters, ReadFault, ReadReceipt, ReadScope, ReadWitness, RecordKey, RecordRead, RecordReads,
        RecordRequest, RecordSelection, RecordSource, RevisionEvidence, SelectedFact,
    };
    pub use crate::store_record_source::{StoreRecordSource, encode_record_key, selected_record_bytes};
}

pub use activation_envelopes::{
    DEMAND_FAMILY, DEMAND_SCHEMA, PREPARED_FAMILY, PREPARED_SCHEMA, WORK_FAMILY, WORK_SCHEMA,
};
pub use body_ir_declarations::{
    BODY_IR_ADAPTER_GENERATION, BODY_IR_ADAPTER_PROFILE, BODY_IR_ADAPTER_REMOVAL_GATE, BODY_IR_ADAPTER_SOURCE,
    CompilerBody, CompilerBodyCoverage, CompilerBodyIrModule, CompilerDeclarationSet, CompilerLoafCallable,
    CompilerNominalDeclaration, CompilerTraitAdoption, NamedTypeIdentity, TraitDefaultInstantiation, TraitFieldWitness,
    TraitTypeArgument, covered_body_count, invalidate_owned_callable_fields, valid_compiler_body,
    valid_invocation_view, valid_loaf_callable, valid_nominal_declaration, valid_optional_body,
    valid_published_expanded_body,
};
pub use body_ir_values::{CompilerBlock, CompilerCallee, CompilerRvalue, CompilerStatement, CompilerStatementKind};
pub use checked_body_counts::CheckedBodyCounts;
pub use checked_types::{
    CheckedCallableParam, CheckedCallableParamKind, CheckedInputAccess, CheckedNumericType, CheckedPrimitiveType,
    CheckedRuntimeBuiltin, CheckedType, valid_checked_type,
};
pub use checker_publication::CheckerRecordCounts;
pub use checker_record_models::{
    CheckedFamily, CodecFault, CompiledBodyDependencyRead, CompiledBodyFallback, CompiledBodyOutputs,
    CompiledBodyValue, CompiledNamespaceValue, GraphBodyFields, GraphBodyValue, GraphNamespaceValue,
    GraphNamespaceView, SettledCallContract, SettledCallRead,
};
pub use compiled_body_rows::CompiledBodyOutput;
pub use compiler_contributors::CompilerContributors;
pub use compiler_export::CompilerExportMember;
pub use compiler_records::{
    CanonicalSymbolId, DeclarationKind, DeclarationSignature, HirSourceSpan, IDENTITY_ADAPTER_GENERATION,
    IDENTITY_ADAPTER_PROFILE, IDENTITY_ADAPTER_REMOVAL_GATE, IDENTITY_ADAPTER_SOURCE, PackageIdentity,
    PlanSourceSpanRecord, StableDeclarationComponent, StableDeclarationId, StableDeclarationLocation, SymbolNamespace,
    SymbolOrigin, valid_canonical_symbol_id, valid_hir_source_span, valid_stable_declaration_id,
};
pub use internal_models_bindings::MappedView;
pub use lowering_events::{ADMITTED_DEFINITIONS, INTERFACE_REPORT};
pub use manifest_cache::{ManifestCacheValue, ManifestInventory};
pub use namespace_order::NamespaceOrder;
pub use namespace_references::NamespaceReference;
pub use namespace_rows::{
    NamespaceAtom, NamespaceEntry, NamespaceField, NamespaceIdentity, NamespacePublication, PreparedNamespaceReader,
};
pub use plan_function_records::PlanFunctionRecord;
pub use read_receipts::RECEIPT_SCHEMA;
pub use record_service::{
    CompilerFault, CompilerFaultKind, CompilerRecordService, DemandPage, Evaluation, InputSnapshot, NeedPrerequisite,
    Preparation, PreparedInputs, Produced, ProducerCounters, ProducerProfile, ProducerRank, ProducerStage,
    RECORD_SERVICE_SCHEMA, RankCertificate, ReadyWork, RecordRoot, SemanticRefusal, ValueReference, prepared_reader,
};
pub use records::{
    AbsentFact, DeletedFact, MemberFact, MemberSelection, PayloadSelection, PreparationEvidence, READ_SCHEMA,
    ReadCounters, ReadFault, ReadReceipt, ReadScope, ReadWitness, RecordKey, RecordRead, RecordReads, RecordRequest,
    RecordSelection, RecordSource, RevisionEvidence, SelectedFact, StoreRecordSource, encode_record_key,
    selected_record_bytes,
};
pub use source_value_records::SOURCE_PROFILE;
pub use unit_admission::PayloadAdmission;

#[cfg(test)]
mod surface_tests;

#[cfg(test)]
mod tests {
    /// Callers importing the concrete store source can read through it with no trait import.
    #[test]
    fn concrete_store_source_exposes_read() {
        use super::{ReadScope, RecordRead, RecordRequest, StoreRecordSource};

        let _: fn(&StoreRecordSource, &ReadScope, &RecordRequest) -> RecordRead = StoreRecordSource::read;
    }
}
