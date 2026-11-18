//! Shared compiler declarations and canonical checker record contracts.

pub mod activation_envelopes;
pub mod body_ir_declarations;
pub mod body_ir_values;
pub mod call_artifact_codecs;
pub mod checked_types;
pub mod checker_field_grammar;
pub mod checker_records;
mod codec_fault;
pub mod compiled_body_frames;
pub mod compiler_records;
pub mod crumb_rows;
mod frame_prefix;
mod general_rows;
mod graph_records;
mod input_fields;
pub mod read_receipts;
pub mod record_scope;
pub mod record_service;
mod row_shapes;
mod semantic_digest;
pub mod store_record_source;

pub use activation_envelopes::{
    DEMAND_FAMILY, DEMAND_SCHEMA, PREPARED_FAMILY, PREPARED_SCHEMA, WORK_FAMILY, WORK_SCHEMA, decode_demand_page,
    decode_prepared_inputs, decode_ready_work, encode_demand_page, encode_prepared_inputs, encode_ready_work,
    envelope_key, stage_name,
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
pub use checked_types::{
    CheckedCallableParam, CheckedCallableParamKind, CheckedInputAccess, CheckedNumericType, CheckedPrimitiveType,
    CheckedRuntimeBuiltin, CheckedType, valid_checked_type,
};
pub use checker_records::{
    GraphBodyFields, GraphBodyValue, GraphNamespaceValue, content_digest, decode_graph_body_page,
    decode_graph_body_values, decode_graph_namespace_value, encode_graph_body_values, encode_graph_namespace_value,
    encode_record, from_native_fields, from_native_final_fields, from_native_local_fields, record_root,
    semantic_digest,
};
pub use codec_fault::CodecFault;
pub use compiled_body_frames::{
    CompiledBodyDependencyRead, CompiledBodyFallback, CompiledBodyOutput, CompiledBodyOutputs, CompiledBodyValue,
    CompiledNamespaceValue, decode_compiled_body_frame, decode_compiled_body_payload, encode_compiled_body_outputs,
};
pub use compiler_records::{
    CanonicalSymbolId, DeclarationKind, DeclarationSignature, HirSourceSpan, IDENTITY_ADAPTER_GENERATION,
    IDENTITY_ADAPTER_PROFILE, IDENTITY_ADAPTER_REMOVAL_GATE, IDENTITY_ADAPTER_SOURCE, PackageIdentity,
    PlanSourceSpanRecord, StableDeclarationComponent, StableDeclarationId, StableDeclarationLocation, SymbolNamespace,
    SymbolOrigin, valid_canonical_symbol_id, valid_hir_source_span, valid_stable_declaration_id,
};
pub use frame_prefix::compiled_body_frame_size;
pub use read_receipts::{RECEIPT_SCHEMA, decode_read_receipt, encode_read_receipt};
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

pub mod records {
    pub use crate::record_scope::{
        AbsentFact, DeletedFact, MemberFact, MemberSelection, PayloadSelection, PreparationEvidence, READ_SCHEMA,
        ReadCounters, ReadFault, ReadReceipt, ReadScope, ReadWitness, RecordKey, RecordRead, RecordReads,
        RecordRequest, RecordSelection, RecordSource, RevisionEvidence, SelectedFact,
    };
    pub use crate::store_record_source::{StoreRecordSource, encode_record_key, selected_record_bytes};
}

#[cfg(test)]
mod tests {
    use super::{CodecFault, compiled_body_frame_size};

    /// Callers importing the concrete store source can read through it with no trait import.
    #[test]
    fn concrete_store_source_exposes_read() {
        use super::{ReadScope, RecordRead, RecordRequest, StoreRecordSource};

        let _: fn(&StoreRecordSource, &ReadScope, &RecordRequest) -> RecordRead = StoreRecordSource::read;
    }

    /// Oven reads exactly eight little-endian bytes before allocating a compiled body frame.
    #[test]
    fn compiled_body_frame_size_keeps_wire_limits() {
        assert_eq!(compiled_body_frame_size(&[1, 0, 0, 0, 0, 0, 0, 0]), Ok(1));
        assert_eq!(compiled_body_frame_size(&[0, 0, 0, 4, 0, 0, 0, 0]), Ok(67_108_864));
        for prefix in [&[][..], &[1; 7][..], &[1; 9][..]] {
            assert_eq!(compiled_body_frame_size(prefix), Err(CodecFault::Malformed));
        }
        for prefix in [[0; 8], [1, 0, 0, 4, 0, 0, 0, 0], [0xff; 8]] {
            assert_eq!(compiled_body_frame_size(&prefix), Err(CodecFault::Limit));
        }
    }
}
