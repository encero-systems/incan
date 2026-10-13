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
mod class_inheritance;
mod closures_comprehensions_and_generators;
mod control_flow_and_loops;
mod decorated_bindings;
mod derived_display;
mod for_patterns;
mod frozen_collections;
mod identities_and_imports;
mod input_contract_and_refusals;
mod iteration_protocol;
mod library_types;
mod method_partials;
mod methods_and_defaults;
mod native_carriers;
mod newtype_constraints;
mod newtype_hooks;
mod newtype_json;
mod newtypes;
mod nominal_parameters;
mod opaque_returns;
mod operators_literals_and_assignment;
mod patterns_and_assertions;
mod properties;
mod provider_plans;
mod raises_assertions;
mod static_storage;

use helpers::{
    body_named, build, build_after_expected_typecheck_errors, local_for_binding, local_types_at, named_targets,
    rendered_f, stand_in_refusal_stmt,
};
