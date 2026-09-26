//! End-to-end proof for the bounded #988 Body-IR replacement executor.
//!
//! One submodule per subject under `replacement_backend_execution_tests/`; `helpers` holds the shared lowering
//! probe. Every submodule starts with `use super::*;`, so the split is a pure relocation.

use incan_test_support as support;

use std::collections::BTreeSet;
use std::fs;

use incan_driver::backend::replacement::{ReplacementExecutionGraph, ReplacementValue, execute_free_function};
use incan_driver::backend::selection::{
    BackendKind, FallbackOutcome, FallbackPolicy, ShadowComparisonState, digest_output, finalize_receipt,
    select_backend,
};
use incan_frontend::body_ir::build_body_ir_module_v0;
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};
use incan_semantics_core::body_ir::{
    BodyIrModule, ConstructorTarget, OwnershipFact, Rvalue, StatementKind, TryErrorRouting,
};
use incan_semantics_core::{CompilerNodeId, IncanType};

#[path = "replacement_backend_execution_tests/helpers.rs"]
mod helpers;

#[path = "replacement_backend_execution_tests/async_tasks_and_race.rs"]
mod async_tasks_and_race;
#[path = "replacement_backend_execution_tests/bodies_bindings_and_scalars.rs"]
mod bodies_bindings_and_scalars;
#[path = "replacement_backend_execution_tests/callables_and_generators.rs"]
mod callables_and_generators;
#[path = "replacement_backend_execution_tests/cli_nominal_and_enum_values.rs"]
mod cli_nominal_and_enum_values;
#[path = "replacement_backend_execution_tests/cli_refusals_shadow_and_boundaries.rs"]
mod cli_refusals_shadow_and_boundaries;
#[path = "replacement_backend_execution_tests/collections_strings_and_print.rs"]
mod collections_strings_and_print;
#[path = "replacement_backend_execution_tests/modules_and_sessions.rs"]
mod modules_and_sessions;
#[path = "replacement_backend_execution_tests/nominal_and_enum_values.rs"]
mod nominal_and_enum_values;

use helpers::lower_typed_body_ir;
