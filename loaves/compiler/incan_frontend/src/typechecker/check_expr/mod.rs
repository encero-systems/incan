//! Check expressions and resolve their types.
//!
//! This module owns the expression-checking entrypoint (`check_expr`) and delegates to themed submodules for
//! maintainability. Expression checking is error-accumulating: on invalid input it returns [`ResolvedType::Unknown`] so
//! later checks can continue.
//!
//! ## See also
//! - [`super::TypeChecker`]: the main type checker entrypoint.

use crate::ast::*;
use crate::diagnostics::{CompileError, errors};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map_call_site};
use crate::symbols::{
    CallableParam, FieldInfo, FunctionInfo, ResolvedType, SymbolKind, TypeBoundInfo, TypeInfo, VariableInfo,
};
use crate::typechecker::helpers::{decimal_shape, is_frozen_bytes, is_frozen_str};
use incan_lang::lang::keywords;
use incan_lang::lang::types::numerics::{self, NumericFamily, NumericTypeId};
use incan_lang::numeric_values::{IntegerBounds, integer_bounds};
use incan_semantics_core::SurfaceExprTypeCheck;
use std::collections::HashMap;

use super::mut_arguments::MutArgumentCallee;
use super::{IdentKind, TypeChecker, YieldContext, numeric_type_id_for_compat};

mod access;
mod basics;
mod body_ir_closure_parameters;
mod body_ir_empty_lists;
mod body_ir_nominals;
mod body_ir_reexports;
mod body_ir_string_slices;
mod body_ir_values;
mod builtin_method_args;
mod calls;
mod collections;
mod comps;
mod control_flow;
mod error_display;
mod expression_dispatch;
mod field_and_module_members;
mod list_methods;
mod literal_carriers;
mod match_;
mod match_coverage;
mod numeric_literals;
mod ops;
mod partials;
mod printed_form;

pub(in crate::typechecker) use collections::fill_open_result_parts;

/// The generic callable a local partial names as its target, with what instantiating it needs (RFC 084).
///
/// A local partial is a value, and a value is not generic, so the partial instantiates the target: its presets and its
/// written type arguments give each of the target's type parameters its type argument.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct GenericPartialTarget {
    /// The target as the partial spells it, for diagnostics.
    callee: String,
    /// The target's type parameters, in declaration order.
    type_params: Vec<String>,
    /// The target's declared bounds, by type parameter, as a call checks them.
    bounds: HashMap<String, Vec<String>>,
    /// The target's resolved bounds, by type parameter, as a call checks them.
    bound_details: HashMap<String, Vec<TypeBoundInfo>>,
}

impl GenericPartialTarget {
    /// Describe a generic function named `callee` as a partial target.
    pub(in crate::typechecker) fn of_function(callee: &str, info: &FunctionInfo) -> Self {
        Self {
            callee: callee.to_string(),
            type_params: info.type_params.clone(),
            bounds: info.type_param_bounds.clone(),
            bound_details: info.type_param_bound_details.clone(),
        }
    }
}

/// Return whether a resolved type is one of the parameterized decimal families.
fn is_decimal_type(ty: &ResolvedType) -> bool {
    decimal_shape(ty).is_some()
}

/// Extract precision and scale from a checked resolved decimal type.
fn decimal_precision_scale(ty: &ResolvedType) -> Option<(usize, usize)> {
    decimal_shape(ty).map(|shape| (usize::from(shape.precision), usize::from(shape.scale)))
}

/// Count significant integer digits, fractional digits, and their total in a plain decimal literal body.
fn decimal_literal_digit_counts(body: &str) -> Option<(usize, usize, usize)> {
    if body.contains('e') || body.contains('E') {
        return None;
    }
    let (integer, fractional) = body.split_once('.').unwrap_or((body, ""));
    let integer_digits = integer
        .chars()
        .skip_while(|ch| *ch == '0')
        .filter(|ch| ch.is_ascii_digit())
        .count();
    let fractional_digits = fractional.chars().filter(|ch| ch.is_ascii_digit()).count();
    Some((integer_digits, fractional_digits, integer_digits + fractional_digits))
}

/// Return the signed value represented by an integer literal or unary-negative integer literal.
fn signed_int_literal_value(expr: &Spanned<Expr>) -> Option<i128> {
    match &expr.node {
        Expr::Literal(Literal::Int(value)) => value.signed_value(false),
        Expr::Unary(UnaryOp::Neg, inner) => match &inner.node {
            Expr::Literal(Literal::Int(value)) => value.signed_value(true),
            _ => None,
        },
        _ => None,
    }
}

/// Return the unsigned magnitude for a non-negative integer literal.
fn unsigned_int_literal_magnitude(expr: &Spanned<Expr>) -> Option<u128> {
    match &expr.node {
        Expr::Literal(Literal::Int(value)) => Some(value.magnitude),
        _ => None,
    }
}
