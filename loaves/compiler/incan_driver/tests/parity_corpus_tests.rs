//! Executable backend-cutover parity corpus (#987).
//!
//! Turns the #646 behavior inventory (`workspaces/docs-site/docs/contributing/reference/
//! backend_behavior_inventory.md`) into a runnable corpus with stable case IDs and an explicit disposition per
//! case, per #987's scope. See `loaves/compiler/incan_driver/tests/support/parity_corpus.rs` for the schema;
//! implementation status lives on issue #652, not in permanent contributor docs, since it describes an in-flight
//! migration state rather than a durable 0.6 end-state contract.
//!
//! Run with: `cargo test --test parity_corpus_tests`
//!
//! ## Why these cases
//!
//! The original source-only seed has grown with the RFC 120 cutover matrix. Package/import rows execute the checked
//! graph and both compiler consumers. Materialized-package rows also observe native and non-linking execution of
//! the same source after provider source removal. Their behavior observations remain distinct from the corpus
//! receipt-aware comparison axis. The release-artifact row compiles and inspects its pinned native fixture.
//!
//! Each case's `evaluate` function probes the *current* compiler directly (not a fixture snapshot of past output),
//! so a behavior change shows up as [`parity_corpus::ComparisonOutcome::Mismatch`] the next time this test runs.
//!
//! One submodule per subject under `parity_corpus_tests/`; `helpers` holds the probes, case identifiers and
//! fixture sources the subjects share. Every submodule starts with `use super::*;`, so the split is a pure
//! relocation.

use incan_driver::backend::IrCodegen;
use incan_driver::backend::replacement::provider::{
    PROVIDER_COMPARISON_UNAVAILABLE_REASON, ProviderInputValue, ProviderInvocation, ProviderOperationHost,
    ProviderOperationOutcome, ProviderRuntime,
};
use incan_driver::backend::replacement::{
    ReplacementExecutionError, ReplacementNumericValue, ReplacementValue, execute_free_function_with_providers,
};
use incan_frontend::body_ir::{build_body_ir_module_v0, build_body_ir_module_v0_with_provider_plan};
use incan_frontend::diagnostics::CompileError;
use incan_frontend::library_manifest::{CompiledProviderMetadata, LibraryManifest, ProviderOperationMetadata};
use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_frontend::provider::{
    NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord,
};
use incan_frontend::{lexer, parser, typechecker};
use incan_semantics_core::authority::StaticAuthority;
use incan_semantics_core::receipts::{AttributeSensitivity, ReceiptAttribute, ReceiptStatus, ReplayClassification};
use incan_semantics_core::{
    AuthorityMode, CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use incan_test_support::emitted_symbol_artifact;

#[path = "support/parity_corpus.rs"]
mod parity_corpus;
#[path = "support/shadow_capability.rs"]
mod shadow_capability;

use incan_test_support as support;
use incan_test_support::package_boundary_probe;
use parity_corpus::{
    BehaviorCategory, CheckedIdentityGraph, ComparisonOutcome, Disposition, EvidenceLane, IdentityAssertions,
    IdentityBindingForm, IdentityConformancePlan, IdentityConformanceSubject, IdentityCoverageCell,
    IdentityGraphDeferral, IdentityGraphEntrypoint, IdentityNamespace, IdentityReplacementPlan, IdentityScope,
    IdentitySourceModule, OverallState, ParityCase, ReceiptRef, ReleaseArtifactAssertions,
    SourceIdentityConformancePlan, behavior_observation_identity, exact_rust_identifier,
    identity_conformance_evidence_identity, validate_corpus, validate_identity_coverage,
};

#[path = "parity_corpus_tests/helpers.rs"]
mod helpers;

#[path = "parity_corpus_tests/corpus_proofs.rs"]
mod corpus_proofs;
#[path = "parity_corpus_tests/identity_conformance.rs"]
mod identity_conformance;
#[path = "parity_corpus_tests/language_cases.rs"]
mod language_cases;
#[path = "parity_corpus_tests/paired_row_proofs.rs"]
mod paired_row_proofs;
#[path = "parity_corpus_tests/provider_paths.rs"]
mod provider_paths;
#[path = "parity_corpus_tests/replacement_cases.rs"]
mod replacement_cases;
#[path = "parity_corpus_tests/rfc_120_proofs.rs"]
mod rfc_120_proofs;
#[path = "parity_corpus_tests/seed.rs"]
mod seed;

use helpers::{
    BOOL_TRUTHINESS_SHADOW_CASE_ID, BOOL_TRUTHINESS_SOURCE, COLLECTION_LEN_SHADOW_CASE_ID, COLLECTION_LEN_SOURCE,
    ENUMERATE_ZIP_SHADOW_CASE_ID, HASHED_MEMBERSHIP_SOURCE, HASHED_SHADOW_CASE_ID, ISINSTANCE_TARGETS_SHADOW_CASE_ID,
    ISINSTANCE_TARGETS_SOURCE, JSON_STRINGIFY_SCALARS_EXPECTED, JSON_STRINGIFY_SCALARS_SOURCE,
    JSON_STRINGIFY_SHADOW_CASE_ID, SCALAR_CONVERSIONS_SHADOW_CASE_ID, SHADOW_COMPARED_CASE_ID,
    SHADOW_COMPARED_CASE_IDS, SORTED_INT_LIST_SHADOW_CASE_ID, SORTED_INT_LIST_SOURCE, STRING_HELPER_SHADOW_CASE_ID,
    STRING_HELPER_SOURCE, STRING_LEN_SHADOW_CASE_ID, STRING_LEN_SOURCE, TYPED_NUMERIC_SHADOW_CASE_ID, body_ir_snapshot,
    outcome_from_body_ir, outcome_from_body_ir_refusal, outcome_from_typecheck, typecheck_warnings,
};
use identity_conformance::{
    BUILTIN_REBINDING_MODULES, BUILTIN_REBINDING_SRC, GENERIC_BINDER_MODULES, GENERIC_BINDER_SRC,
    IDENTITY_MATRIX_DEFERRED, IDENTITY_MATRIX_ENTRYPOINTS, IDENTITY_MATRIX_MODULES, IDENTITY_MATRIX_SRC,
    LET_SHADOW_MODULES, LET_SHADOW_SRC, MUT_SHADOW_MODULES, MUT_SHADOW_SRC, expected_eight, expected_eleven,
    expected_forty_two, expected_three, no_replacement_arguments, verify_builtin_rebinding, verify_generic_binder,
    verify_identity_matrix, verify_let_shadow, verify_mut_shadow, verify_release_artifact,
    verify_wrong_owner_member_selection, verify_wrong_path_target_selection,
};
use language_cases::{
    CASE_1_SRC, CASE_2_SRC, CASE_3_SRC, CASE_4_SRC, CASE_5_SRC, CASE_6_SRC, CASE_7_SRC, CASE_8_SRC, CASE_9_SRC,
    CASE_10_SRC, CASE_11_SRC, CASE_12_SRC, CASE_13_SRC, CASE_15_SRC, CASE_16_SRC, CASE_17_SRC, CASE_18_SRC,
    CASE_19_SRC, CASE_20_SRC, CASE_21_SRC, CASE_22_SRC, CASE_23_SRC, PACKAGE_CONSUMER_SRC,
    case_collection_membership_names_its_container, case_diagnostic_chained_comparison_rejected,
    case_diagnostic_statement_tuple_unpack_of_non_tuple, case_diagnostic_unreachable_code_after_return,
    case_generated_artifact_valid_rust_shape, case_inactive_feature_body_never_reaches_body_ir,
    case_list_concatenation_is_not_a_primitive_addition, case_package_consumer_call_executes,
    case_package_representation_refusal_is_packaging_error, case_pattern_assertion_binding_reaches_body_ir,
    case_raises_assertion_reaches_body_ir, case_stdlib_runtime_string_membership, case_supported_await_reaches_body_ir,
    case_supported_builtin_len_shadowing, case_supported_bytes_literal_reaches_body_ir,
    case_supported_call_spreads_reach_body_ir, case_supported_literal_spreads_reach_body_ir,
    case_supported_match_exhaustiveness, case_supported_named_call_arguments_reach_body_ir,
    case_supported_named_construction_reaches_body_ir, case_supported_race_for_reaches_body_ir,
    case_supported_range_value_reaches_body_ir, case_supported_statement_loop_reaches_body_ir,
    case_supported_string_membership_reaches_body_ir, case_unsafe_region_is_a_stated_refusal,
};
use provider_paths::{
    PROVIDER_CASE_SRC, case_provider_allowed_invocation, case_provider_governed_denial,
    case_provider_lifecycle_cleanup, case_provider_operation_failure, case_provider_redaction_classification,
};
use replacement_cases::{
    REPLACEMENT_BODY_V0_001_SRC, REPLACEMENT_BODY_V0_002_SRC, REPLACEMENT_BODY_V0_003_SRC, REPLACEMENT_BODY_V0_004_SRC,
    REPLACEMENT_BODY_V0_005_SRC, REPLACEMENT_BODY_V0_006_SRC, REPLACEMENT_BODY_V0_007_SRC, REPLACEMENT_BODY_V0_008_SRC,
    REPLACEMENT_BODY_V0_009_SRC, REPLACEMENT_BODY_V0_010_SRC, REPLACEMENT_BODY_V0_011_SRC, REPLACEMENT_BODY_V0_012_SRC,
    REPLACEMENT_BODY_V0_013_SRC, REPLACEMENT_BODY_V0_014_SRC, REPLACEMENT_BODY_V0_015_SRC, REPLACEMENT_BODY_V0_016_SRC,
    REPLACEMENT_BODY_V0_017_SRC, REPLACEMENT_BODY_V0_018_SRC, REPLACEMENT_BODY_V0_019_SRC, REPLACEMENT_BODY_V0_022_SRC,
    REPLACEMENT_BODY_V0_023_SRC, REPLACEMENT_BODY_V0_029_SRC, replacement_body_v0_001_arguments,
    replacement_body_v0_001_expected, replacement_body_v0_002_arguments, replacement_body_v0_002_expected,
    replacement_body_v0_003_arguments, replacement_body_v0_003_expected, replacement_body_v0_004_arguments,
    replacement_body_v0_004_expected, replacement_body_v0_005_arguments, replacement_body_v0_005_expected,
    replacement_body_v0_006_arguments, replacement_body_v0_006_expected, replacement_body_v0_007_arguments,
    replacement_body_v0_007_expected, replacement_body_v0_008_arguments, replacement_body_v0_008_expected,
    replacement_body_v0_009_arguments, replacement_body_v0_009_expected, replacement_body_v0_010_arguments,
    replacement_body_v0_010_expected, replacement_body_v0_011_arguments, replacement_body_v0_011_expected,
    replacement_body_v0_012_arguments, replacement_body_v0_012_expected, replacement_body_v0_013_arguments,
    replacement_body_v0_013_expected, replacement_body_v0_014_arguments, replacement_body_v0_014_expected,
    replacement_body_v0_015_arguments, replacement_body_v0_015_expected, replacement_body_v0_016_arguments,
    replacement_body_v0_016_expected, replacement_body_v0_017_arguments, replacement_body_v0_017_expected,
    replacement_body_v0_018_arguments, replacement_body_v0_018_expected, replacement_body_v0_019_arguments,
    replacement_body_v0_019_expected, replacement_body_v0_022_arguments, replacement_body_v0_022_expected,
    replacement_body_v0_023_arguments, replacement_body_v0_023_expected, replacement_body_v0_025_expected,
    replacement_body_v0_029_expected,
};
use seed::seed_corpus;
