//! Body IR lowering tests, one module per subject; `helpers` holds the shared builders and readers.
//!
//! `use super::*` keeps every item these tests reach in the parent module, so the split is a pure relocation with no
//! visibility change.

use super::defaults::*;
use super::*;
use crate::test_support::provider_plan_from_checked_source;
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_lang::lang::surface::constructors;

mod helpers;

mod async_and_race;
mod calls_and_arguments;
mod channel_errors;
mod class_inheritance;
mod closed_callback_types;
mod closed_sdk_rest;
mod closures_comprehensions_and_generators;
mod control_flow_and_loops;
mod decorated_bindings;
mod derived_display;
mod for_patterns;
mod frozen_collections;
mod identities_and_imports;
mod inferred_clone;
mod input_contract_and_refusals;
mod iteration_protocol;
mod library_types;
mod method_partials;
mod methods_and_defaults;
mod native_carriers;
mod newtype_constraints;
mod newtype_fields;
mod newtype_hooks;
mod newtype_json;
mod newtype_sum;
mod newtypes;
mod nominal_factories;
mod nominal_parameters;
mod opaque_returns;
mod open_nominal_types;
mod operators_literals_and_assignment;
mod patterns_and_assertions;
mod properties;
mod protocol_hooks;
mod provider_plans;
mod raises_assertions;
mod result_callbacks;
mod return_pass_order;
mod returned_callables;
mod sdk_factory_defaults;
mod sdk_iterator_dispatch;
mod static_storage;
mod task_handles;
mod text_codecs;
mod trait_placeholders;
mod value_enum_methods;
mod web_path;

use helpers::{
    body_named, build, build_after_expected_typecheck_errors, build_with_stand_in_refusal, local_for_binding,
    local_types_at, named_targets, rendered_f, stand_in_refusal_stmt,
};
