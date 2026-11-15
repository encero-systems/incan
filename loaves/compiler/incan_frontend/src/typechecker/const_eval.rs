//! Const-evaluation / const-validation for RFC 008.
//!
//! This module does not compute runtime values; it validates that an initializer is const-evaluable, determines its
//! type, classifies it (Rust-native vs frozen), and detects const dependency cycles.
//!
//! Numeric semantics follow Python-like rules (via `crate::numeric`):
//! - `/` always yields `Float` (even `int / int`)
//! - `%` supports floats with Python remainder semantics
//! - `**` yields `Int` only for non-negative int literal exponents; otherwise `Float`

use crate::ast::*;
use crate::diagnostics::{CompileError, errors};
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved, pow_exponent_kind_from_ast};
use crate::partial_projection::{PartialPresetRef, merge_named_partial_args};
use crate::symbols::{ResolvedType, SymbolKind, TypeInfo};
use incan_lang::lang::types::numerics::NumericTypeId;
use incan_lang::numeric_values::{IntegerBounds, integer_bounds};
use incan_lang::strings::{self, StringAccessError};
use incan_lang::{NumericTy, result_numeric_type};

use super::{PartialProjectionTargetKind, TypeChecker, numeric_type_id_for_compat, type_info};
use crate::typechecker::helpers::{
    freeze_const_type, frozen_bytes_ty, frozen_str_ty, is_frozen_str, is_intlike_for_index,
    is_mutable_collection_const_annotation, is_str_like,
};

mod const_calls;
mod const_expressions;
mod const_operators;

/// Const category used by RFC 008.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstKind {
    /// Can be emitted as a Rust `const` directly.
    RustNative,
    /// Needs frozen stdlib wrappers (deep immutability / baked static data).
    Frozen,
}

/// The limited values known during const validation; absence leaves evaluation to native compilation.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    /// A known signed integer.
    Int(i64),
    /// A known binary floating-point value.
    Float(f64),
    /// A known Boolean value.
    Bool(bool),
    /// Known immutable UTF-8 text.
    FrozenStr(String),
    /// Known immutable bytes.
    FrozenBytes(Vec<u8>),
}

/// A constant initializer's type, publication category and optional known value.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstEvalResult {
    /// The initializer's resolved type, including frozen collection carriers.
    pub ty: ResolvedType,
    /// Whether native const evaluation suffices or a frozen carrier is required.
    pub kind: ConstKind,
    /// A value retained only when the limited evaluator knows it.
    pub value: Option<ConstValue>,
}

/// Borrow known frozen text without reconstructing or copying the value.
fn const_str(value: &ConstValue) -> Option<&str> {
    match value {
        ConstValue::FrozenStr(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Read a known integer for string indexing or slicing.
fn const_int(value: &ConstValue) -> Option<i64> {
    match value {
        ConstValue::Int(i) => Some(*i),
        _ => None,
    }
}

/// Return the binary floating-point value represented by a const-evaluation result.
fn const_float(value: &ConstValue) -> Option<f64> {
    match value {
        ConstValue::Float(value) => Some(*value),
        _ => None,
    }
}

/// Return whether a non-negative integer literal can initialize a sized numeric type.
fn unsigned_int_literal_fits_numeric_target(lit: &IntLiteral, id: NumericTypeId) -> bool {
    match integer_bounds(id) {
        Some(IntegerBounds::Signed { maximum, .. }) => lit.magnitude <= maximum as u128,
        Some(IntegerBounds::Unsigned { maximum }) => lit.magnitude <= maximum,
        None => false,
    }
}

/// Dependency traversal state used to detect const-initializer cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstEvalState {
    /// The declaration has not entered evaluation.
    NotStarted,
    /// The declaration is on the active dependency stack.
    InProgress,
    /// The declaration's evaluation attempt has finished.
    Done,
}
