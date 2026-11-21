//! The field defaults the record models publish to Incan callers.
//!
//! An Incan caller may leave a field out of a Rust record's construction when the record publishes that field's
//! default as the associated function `__incan_default_<field>`: public, taking no arguments, returning the field's
//! type. The frontend evaluates it after the written arguments, as Rust evaluates a struct update's base after the
//! written fields. A record that publishes any default states exactly which fields a caller may leave out; every other
//! field stays required. These are the reference models' defaults, field for field.

use std::collections::{HashMap, HashSet};

use crate::body_ir_declarations::CompilerBodyIrModule;
use crate::checked_body_counts::CheckedBodyCounts;
use crate::checker_publication::CheckerRecordCounts;
use crate::checker_record_models::{
    CompiledBodyDependencyRead, CompiledBodyOutputs, CompiledBodyValue, CompiledNamespaceValue, GraphNamespaceValue,
    SettledCallContract, SettledCallRead,
};
use crate::compiler_contributors::CompilerContributors;
use crate::compiler_records::HirSourceSpan;
use crate::crumb_rows::ModelField;
use crate::lowering_events::{
    AdmittedFunction, AdmittedModule, AdmittedPlacement, EventPosition, EventReference, LoweringEvent,
};
use crate::namespace_order::NamespaceOrder;
use crate::overload_members::{MethodLookup, MethodReads};
use crate::record_scope::{DEFAULT_READ_BYTES, RecordKey, RecordRequest};
use crate::unit_admission::PayloadAdmission;

/// Publish each `function -> Type = value` as the associated function an Incan construction calls for the field it
/// leaves out. Each function is named `__incan_default_` followed by its field's name.
macro_rules! published_defaults {
    ($owner:ty { $($function:ident -> $field:ty = $value:expr),+ $(,)? }) => {
        impl $owner {
            $(
                #[doc = concat!("Published Incan field default: `", stringify!($value), "`.")]
                pub fn $function() -> $field {
                    $value
                }
            )+
        }
    };
}

published_defaults!(CompilerBodyIrModule {
    __incan_default_runtime_binding_scopes -> Vec<(String, Vec<String>)> = Vec::new(),
});

published_defaults!(CheckedBodyCounts {
    __incan_default_checked -> i64 = 0,
    __incan_default_reused -> i64 = 0,
    __incan_default_decoded -> i64 = 0,
});

published_defaults!(CheckerRecordCounts {
    __incan_default_reusable_declarations -> i64 = 0,
    __incan_default_all_declarations -> i64 = 0,
    __incan_default_summary -> String = "0/0 reusable declarations".to_owned(),
    __incan_default_artifact_refusals -> HashMap<String, i64> = HashMap::new(),
    __incan_default_restored -> Vec<String> = Vec::new(),
    __incan_default_restored_body_ir -> Vec<String> = Vec::new(),
});

published_defaults!(GraphNamespaceValue {
    __incan_default_lowering_inputs -> Vec<Vec<String>> = Vec::new(),
    __incan_default_package_roots -> Vec<String> = Vec::new(),
    __incan_default_call_reads -> Vec<SettledCallRead> = Vec::new(),
    __incan_default_call_reads_known -> bool = true,
    __incan_default_identity_proof -> Vec<u8> = Vec::new(),
});

published_defaults!(CompiledBodyValue {
    __incan_default_dependency_reads -> Vec<CompiledBodyDependencyRead> = Vec::new(),
    __incan_default_scope_names -> Vec<String> = Vec::new(),
});

published_defaults!(CompiledNamespaceValue {
    __incan_default_lowering_inputs -> Vec<Vec<String>> = Vec::new(),
    __incan_default_package_roots -> Vec<String> = Vec::new(),
    __incan_default_call_reads -> Vec<SettledCallRead> = Vec::new(),
    __incan_default_semantic_interface -> Vec<u8> = Vec::new(),
    __incan_default_member_witnesses -> Vec<Vec<String>> = Vec::new(),
    __incan_default_call_contracts -> Option<Vec<SettledCallContract>> = None,
    __incan_default_identity_proof -> Vec<u8> = Vec::new(),
    __incan_default_published_record -> Vec<u8> = Vec::new(),
});

published_defaults!(CompiledBodyOutputs {
    __incan_default_namespaces -> Vec<CompiledNamespaceValue> = Vec::new(),
});

published_defaults!(CompilerContributors {
    __incan_default_vocabulary -> Vec<String> = Vec::new(),
    __incan_default_operations -> Vec<String> = Vec::new(),
});

published_defaults!(HirSourceSpan {
    __incan_default_expansion -> Option<Vec<i64>> = None,
});

published_defaults!(ModelField {
    __incan_default_canonical -> bool = true,
});

published_defaults!(LoweringEvent {
    __incan_default_admitted -> bool = false,
    __incan_default_module -> String = String::new(),
    __incan_default_role -> String = String::new(),
    __incan_default_demand -> String = String::new(),
    __incan_default_runtime_requirements -> i64 = 0,
    __incan_default_references -> Vec<EventReference> = Vec::new(),
    __incan_default_positions -> Vec<EventPosition> = Vec::new(),
});

published_defaults!(AdmittedFunction {
    __incan_default_placement -> Option<AdmittedPlacement> = None,
});

published_defaults!(AdmittedModule {
    __incan_default_inspected -> bool = false,
});

published_defaults!(NamespaceOrder {
    __incan_default_rank -> i64 = 0,
    __incan_default_evidence -> String = "compiled-check".to_owned(),
    __incan_default_members -> Vec<String> = Vec::new(),
    __incan_default_certificate -> Vec<u8> = Vec::new(),
});

published_defaults!(MethodReads {
    __incan_default_groups -> HashMap<String, Vec<MethodLookup>> = HashMap::new(),
    __incan_default_retained -> HashSet<String> = HashSet::new(),
});

published_defaults!(RecordKey {
    __incan_default_context -> String = String::new(),
});

published_defaults!(RecordRequest {
    __incan_default_byte_limit -> i64 = DEFAULT_READ_BYTES,
});

published_defaults!(PayloadAdmission {
    __incan_default_facts -> String = String::new(),
    __incan_default_refusal -> String = String::new(),
});
