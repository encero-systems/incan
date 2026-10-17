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
mod callable_names;
mod calls_and_arguments;
mod canonical_builtin_methods;
mod channel_errors;
mod checked_json;
mod class_inheritance;
mod closed_callback_types;
mod closed_partial_instances;
mod closed_sdk_rest;
mod closures_comprehensions_and_generators;
mod collect_publication;
mod constructor_owners;
mod control_flow_and_loops;
mod decorated_bindings;
mod decorated_publication;
mod derived_display;
mod empty_list_elements;
mod enum_payload_publication;
mod executable_roundtrips;
mod field_metadata;
mod field_owners;
mod for_patterns;
mod frozen_collections;
mod generic_decorated_publication;
mod generic_default_publication;
mod hashed_models;
mod identities_and_imports;
mod implicit_closure_parameters;
mod imported_integer_constants;
mod inferred_clone;
mod input_contract_and_refusals;
mod iteration_protocol;
mod library_types;
mod method_partials;
mod methods_and_defaults;
mod model_constants;
mod module_carriers;
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
mod private_publication;
mod properties;
mod protocol_hooks;
mod provider_plans;
mod qualified_async_calls;
mod raises_assertions;
mod reexported_call_types;
mod registry_descriptions;
mod result_callbacks;
mod return_pass_order;
mod returned_callables;
mod sdk_factory_defaults;
mod sdk_generic_newtypes;
mod sdk_iterator_dispatch;
mod sdk_optional_defaults;
mod sdk_ordinal_dispatch;
mod set_constructors;
mod slice_hooks;
mod source_comparison;
mod static_publication;
mod static_storage;
mod string_slice_calls;
mod task_handles;
mod text_codecs;
mod trait_adoptions;
mod trait_input_arguments;
mod trait_partial_presets;
mod trait_placeholders;
mod trait_templates;
mod tuple_transfers;
mod value_enum_methods;
mod web_path;

mod executable_wire;
mod newtype_string_conversion;
mod published_generics;
mod reader_callable_ownership;
mod sdk_reader_defaults;
mod trait_return_witnesses;

use helpers::{
    body_named, build, build_after_expected_typecheck_errors, build_with_stand_in_refusal, local_for_binding,
    local_types_at, named_targets, rendered_f, stand_in_refusal_stmt,
};

mod constructed_constants;
mod project_nominal_carriers;
mod sdk_selected_trait_calls;
