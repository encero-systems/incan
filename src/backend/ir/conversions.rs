//! Centralized ownership conversion policy for IR code generation.
//!
//! Incan does not expose Rust's ownership model directly, but generated Rust still needs the right
//! move/borrow/clone/materialization shape at each sink. This module is the low-level policy engine
//! behind duckborrowing: it decides which conversion strategy to apply once the emitter identifies a
//! typed use site.
//!
//! This module provides the **single source of truth** for deciding when code generation should:
//! - materialize owned `String` storage,
//! - borrow or mutably borrow a value for Rust interop,
//! - preserve last-use moves,
//! - clone non-`Copy` values when the source must remain usable, or
//! - keep values as-is.
//!
//! ## Main responsibilities
//!
//! Strings remain the most common ownership mismatch in generated Rust, but this module now also
//! covers:
//! - borrowed method-chain results such as `box.as_ref()`,
//! - field reads that must materialize owned values at storage/return sinks,
//! - backend-inserted clones for owned tuples/collections/assignments,
//! - Rust interop argument shaping, and
//! - numeric coercion planning for binary operators.
//!
//! ## Why strings still matter
//!
//! Strings are still the most common source of borrow/ownership mismatches when compiling to Rust:
//!
//! - **Primitives** (`int`, `float`, `bool`) implement `Copy` in Rust, so they pass by value automatically—no
//!   conversion needed
//! - **Strings** have a fundamental split in Rust: `&str` (borrowed, stack) vs `String` (owned, heap)
//! - Incan's `str` type abstracts this away (like Python), but codegen must handle it
//! - String literals like `"hello"` are `&'static str` in Rust, requiring `.to_string()` for owned contexts
//!
//! ## Architecture
//!
//! The main function is [`determine_conversion`], which takes:
//!
//! - An IR expression (the value being passed/assigned)
//! - An optional target type (what type is expected)
//! - A [`ConversionContext`] (how the value will be used)
//!
//! Based on this context and the types involved, it returns a [`Conversion`] strategy.
//!
//! ## Conversion Rules by Context
//!
//! ### IncanFunctionArg
//!
//! Incan functions expect **owned values**:
//!
//! ```text
//! Incan:  def greet(name: str) -> str: return "Hello, " + name
//! Rust:   fn greet(name: String) -> String { ... }
//!                        ^^^^^^ owned String, not &str
//! ```
//! - String literals → `.to_string()` (e.g., `greet("Alice")` → `greet("Alice".to_string())`)
//! - String variables → `.to_string()` (may be &str at runtime)
//!
//! ### ExternalFunctionArg
//!
//! External Rust functions may expect **borrows**:
//!
//! ```text
//! Incan:  result = rust::json_parse(data)
//! Rust:   let result = json_parse(&data);
//!                                  ^ borrow for external call
//! ```
//! - String literals → `.to_string()` (for enum variants like `Some("x")`)
//! - String variables → `&` (borrow for &str parameters)
//!
//! ### Owned storage sinks
//!
//! Struct fields and collection elements are always **owned**:
//!
//! ```text
//! Incan:  user = User(name="Alice", age=30)
//! Rust:   let user = User { name: "Alice".to_string(), age: 30 };
//!                                  ^^^^^^^^^^^^^^^^^^ owned String field
//! ```
//! - String literals → `.to_string()`
//!
//! ### Assignment
//!
//! Let bindings must match the variable's type:
//!
//! ```text
//! Incan:  name: str = "Alice"
//! Rust:   let name: String = "Alice".to_string();
//!                             ^^^^^^^^^^^^^^^^^^ convert to owned
//! ```
//! - String literals to String variables → `.to_string()`
//!
//! ### ReturnValue
//!
//! Return values must match the function signature:
//!
//! ```text
//! Incan:  def get_name() -> str: return "Alice"
//! Rust:   fn get_name() -> String { return "Alice".to_string(); }
//!                                           ^^^^^^^^^^^^^^^^^^ convert to owned
//! ```
//! - String literals when returning String → `.to_string()`
//!
//! ### MethodArg
//!
//! Method arguments usually don't need conversion (Rust's `Borrow` trait handles it):
//!
//! ```text
//! Incan:  text.contains("hello")
//! Rust:   text.contains("hello")  // &str works directly
//! ```
//!
//! ## Examples
//!
//! ### Example 1: Function Call
//!
//! ```incan
//! def greet(name: str) -> str:
//!     return f"Hello, {name}"
//!
//! result = greet("Alice")
//! ```
//!
//! Generated Rust:
//!
//! ```rust,ignore
//! fn greet(name: String) -> String {
//!     return format!("Hello, {}", name);
//! }
//! let result = greet("Alice".to_string());  // ← conversion applied
//! ```
//!
//! ### Example 2: Struct Construction
//!
//! ```incan
//! model User:
//!     name: str
//!     email: str
//!
//! user = User(name="Alice", email="alice@example.com")
//! ```
//!
//! Generated Rust:
//!
//! ```rust,ignore
//! pub struct User {
//!     pub name: String,
//!     pub email: String,
//! }
//! let user = User {
//!     name: "Alice".to_string(),              // ← conversion applied
//!     email: "alice@example.com".to_string()  // ← conversion applied
//! };
//! ```
//!
//! ### Example 3: External Function with Borrow
//!
//! ```incan
//! import rust::std::fs::read_to_string
//!
//! content: str = "data.txt"
//! data = read_to_string(content)
//! ```
//!
//! Generated Rust:
//!
//! ```rust,ignore
//! let content: String = "data.txt".to_string();
//! let data = std::fs::read_to_string(&content);  // ← borrow applied
//! ```

use super::decl::FunctionParam;
use super::expr::{BinOp, VarAccess};
use super::reference_shape::expr_has_rust_reference_shape;
use super::types::Mutability;
use super::{IrExpr, IrExprKind, IrType, TypedExpr};
use crate::numeric_adapters::{ir_type_to_numeric_ty, numeric_op_from_ir, pow_exponent_kind_from_ir};
use incan_core::lang::types::collections::{self, CollectionTypeId};
use incan_core::lang::types::numerics::{self, NumericFamily};
use incan_core::{NumericOp, NumericTy, needs_float_promotion, result_numeric_type};
use proc_macro2::TokenStream;
use quote::quote;

/// Context in which a value is being used - determines conversion rules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionContext {
    /// Argument to an Incan-defined function (expects owned values)
    IncanFunctionArg,
    /// Argument to an Incan function inside a return statement.
    /// Values can be moved since there's no code after the return.
    IncanFunctionArgInReturn,
    /// Argument to an external Rust function (may expect borrows)
    ExternalFunctionArg,
    /// Field in struct construction (always owned)
    StructField,
    /// Element being stored into an owned collection.
    CollectionElement,
    /// Argument to a method call (context-dependent)
    MethodArg,
    /// Assignment or let binding
    Assignment,
    /// Return value from a function
    ReturnValue,
    /// Value consumed by a generated Rust `match` scrutinee.
    MatchScrutinee,
}

/// Result of conversion analysis
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conversion {
    /// Pass value as-is
    None,
    /// Convert &str to String with .to_string()
    ToString,
    /// Convert via `.into()` — lets the Rust compiler resolve the target type via the `Into` trait.
    /// Used for external Rust crate calls where the target type may be a custom string type (e.g., Polars'
    /// `PlSmallStr`) or other type that implements `From<String>` / `From<&str>`.
    Into,
    /// Borrow with &
    Borrow,
    /// Mutable borrow with &mut
    MutBorrow,
    /// Clone with .clone()
    Clone,
}

impl Conversion {
    /// Apply this conversion to an already-emitted token stream
    pub fn apply(&self, tokens: TokenStream) -> TokenStream {
        match self {
            Conversion::None => tokens,
            Conversion::ToString => quote! { #tokens.to_string() },
            Conversion::Into => quote! { #tokens.into() },
            Conversion::Borrow => quote! { &#tokens },
            Conversion::MutBorrow => quote! { &mut #tokens },
            Conversion::Clone => quote! { #tokens.clone() },
        }
    }
}

/// Numeric coercions for binary operations (int/float promotion).
///
/// Promotes integer operands to `f64` when the paired operand is `f64`,
/// or when the operation requires float (e.g., division).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericConversion {
    None,
    ToFloat,
}

impl NumericConversion {
    /// Apply the numeric coercion to an emitted token stream.
    ///
    /// Uses `as f64` casting to align with the backend's float representation.
    pub fn apply(&self, tokens: TokenStream) -> TokenStream {
        match self {
            NumericConversion::None => tokens,
            // Use `(expr) as f64` to preserve precedence without wrapping the entire cast expression.
            // This avoids Rust's `unused_parens` warnings in call arguments like `f(x, (3 as f64))`.
            NumericConversion::ToFloat => quote! { (#tokens) as f64 },
        }
    }
}

// ---------------------- BinOpPlan (centralized binop emission strategy) ------------------------

/// Emission strategy for a binary op after conversions are applied.
#[derive(Debug, Clone)]
pub enum BinOpEmitKind {
    /// Emit as infix tokens, e.g., `+`, `-`, `*`, `==`
    Infix { token: TokenStream },
    /// Emit a stdlib helper call, e.g., `incan_stdlib::num::py_mod`
    StdlibCall {
        path: TokenStream,
        /// If true, emit as `path(&lhs, &rhs)` to avoid moves and to support &str-based helpers.
        borrow_args: bool,
    },
    /// Emit power; choose powf vs pow based on kind
    Pow { result_is_int: bool },
}

/// Plan for emitting a binary operation.
#[derive(Debug, Clone)]
pub struct BinOpPlan {
    pub lhs_conv: NumericConversion,
    pub rhs_conv: NumericConversion,
    pub result_ty: IrType,
    pub emit: BinOpEmitKind,
}

fn emit_binop_token(op: &BinOp) -> TokenStream {
    match op {
        BinOp::Add => quote! { + },
        BinOp::Sub => quote! { - },
        BinOp::Mul => quote! { * },
        BinOp::Div => quote! { / },
        BinOp::FloorDiv => quote! { / },
        BinOp::Mod => quote! { % },
        BinOp::Pow => quote! { .pow },
        BinOp::Eq => quote! { == },
        BinOp::Ne => quote! { != },
        BinOp::Lt => quote! { < },
        BinOp::Le => quote! { <= },
        BinOp::Gt => quote! { > },
        BinOp::Ge => quote! { >= },
        BinOp::And => quote! { && },
        BinOp::Or => quote! { || },
        BinOp::BitAnd => quote! { & },
        BinOp::BitOr => quote! { | },
        BinOp::BitXor => quote! { ^ },
        BinOp::Shl => quote! { << },
        BinOp::Shr => quote! { >> },
    }
}

/// Return whether an IR type is one of the exact-width unsigned integer types.
fn exact_unsigned_integer_type(ty: &IrType) -> bool {
    matches!(
        ty,
        IrType::Numeric(id) if numerics::info_for(*id).family == NumericFamily::UnsignedInteger
    )
}

/// Return whether an expression is a non-negative integer literal that Rust can infer to an unsigned type.
fn non_negative_integer_literal(expr: &TypedExpr) -> bool {
    match &expr.kind {
        IrExprKind::Int(value) => *value >= 0,
        IrExprKind::IntLiteral(_) => true,
        _ => false,
    }
}

/// Determine a BinOpPlan: conversions + emit strategy in one place.
pub fn determine_binop_plan(op: &BinOp, left: &TypedExpr, right: &TypedExpr) -> BinOpPlan {
    let is_stringish = |ty: &IrType| match ty {
        IrType::String | IrType::StaticStr | IrType::StrRef | IrType::FrozenStr => true,
        IrType::Ref(inner) | IrType::RefMut(inner) => matches!(inner.as_ref(), IrType::String),
        _ => false,
    };
    let is_runtime_list = |ty: &IrType| matches!(ty, IrType::List(_));

    if matches!(op, BinOp::Add) && is_stringish(&left.ty) && is_stringish(&right.ty) {
        return BinOpPlan {
            lhs_conv: NumericConversion::None,
            rhs_conv: NumericConversion::None,
            result_ty: IrType::String,
            emit: BinOpEmitKind::StdlibCall {
                path: quote! { incan_stdlib::strings::str_concat },
                borrow_args: true,
            },
        };
    }

    if matches!(op, BinOp::Add) && is_runtime_list(&left.ty) && is_runtime_list(&right.ty) {
        return BinOpPlan {
            lhs_conv: NumericConversion::None,
            rhs_conv: NumericConversion::None,
            result_ty: left.ty.clone(),
            emit: BinOpEmitKind::StdlibCall {
                path: quote! { incan_stdlib::collections::list_concat },
                borrow_args: true,
            },
        };
    }

    if matches!(
        op,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
    ) && is_stringish(&left.ty)
        && is_stringish(&right.ty)
    {
        let path = match op {
            BinOp::Eq => quote! { incan_stdlib::strings::str_eq },
            BinOp::Ne => quote! { incan_stdlib::strings::str_ne },
            BinOp::Lt => quote! { incan_stdlib::strings::str_lt },
            BinOp::Le => quote! { incan_stdlib::strings::str_le },
            BinOp::Gt => quote! { incan_stdlib::strings::str_gt },
            BinOp::Ge => quote! { incan_stdlib::strings::str_ge },
            _ => unreachable!(),
        };
        return BinOpPlan {
            lhs_conv: NumericConversion::None,
            rhs_conv: NumericConversion::None,
            result_ty: IrType::Bool,
            emit: BinOpEmitKind::StdlibCall {
                path,
                borrow_args: true,
            },
        };
    }

    let num_op = match numeric_op_from_ir(op) {
        Some(op) => op,
        None => {
            return BinOpPlan {
                lhs_conv: NumericConversion::None,
                rhs_conv: NumericConversion::None,
                result_ty: left.ty.clone(),
                emit: BinOpEmitKind::Infix {
                    token: emit_binop_token(op),
                },
            };
        }
    };

    let lhs_num = ir_type_to_numeric_ty(&left.ty);
    let rhs_num = ir_type_to_numeric_ty(&right.ty);

    // Python modulo/floor-division helpers are i64/f64-only. Unsigned exact-width operands can use Rust's native
    // operators because their domain has no negative remainder/flooring case to normalize.
    if matches!(num_op, NumericOp::FloorDiv | NumericOp::Mod)
        && (exact_unsigned_integer_type(&left.ty)
            && (matches!(right.ty, IrType::Int) || exact_unsigned_integer_type(&right.ty))
            || exact_unsigned_integer_type(&right.ty)
                && matches!(left.ty, IrType::Int)
                && non_negative_integer_literal(left))
    {
        return BinOpPlan {
            lhs_conv: NumericConversion::None,
            rhs_conv: NumericConversion::None,
            result_ty: if exact_unsigned_integer_type(&left.ty) {
                left.ty.clone()
            } else {
                right.ty.clone()
            },
            emit: BinOpEmitKind::Infix {
                token: emit_binop_token(op),
            },
        };
    }

    let pow_exp_kind = if matches!(op, BinOp::Pow) {
        Some(pow_exponent_kind_from_ir(right))
    } else {
        None
    };

    let (lhs_conv, rhs_conv, result_ty) = match (lhs_num, rhs_num) {
        (Some(lhs), Some(rhs)) => {
            let (l_promote, r_promote) = needs_float_promotion(num_op, lhs, rhs, pow_exp_kind);
            let res = result_numeric_type(num_op, lhs, rhs, pow_exp_kind);
            let l = if l_promote {
                NumericConversion::ToFloat
            } else {
                NumericConversion::None
            };
            let r = if r_promote {
                NumericConversion::ToFloat
            } else {
                NumericConversion::None
            };
            let ty = match res {
                NumericTy::Int => IrType::Int,
                NumericTy::Float => IrType::Float,
            };
            (l, r, ty)
        }
        _ => (NumericConversion::None, NumericConversion::None, left.ty.clone()),
    };

    let emit = match num_op {
        NumericOp::Pow => {
            let result_is_int = matches!(result_ty, IrType::Int);
            BinOpEmitKind::Pow { result_is_int }
        }
        NumericOp::Mod => {
            let path = match result_ty {
                IrType::Int => quote! { incan_stdlib::num::py_mod_i64 },
                IrType::Float => quote! { incan_stdlib::num::py_mod_f64 },
                _ => quote! { incan_stdlib::num::py_mod },
            };
            BinOpEmitKind::StdlibCall {
                path,
                borrow_args: false,
            }
        }
        NumericOp::FloorDiv => {
            let path = match result_ty {
                IrType::Int => quote! { incan_stdlib::num::py_floor_div_i64 },
                IrType::Float => quote! { incan_stdlib::num::py_floor_div_f64 },
                _ => quote! { incan_stdlib::num::py_floor_div },
            };
            BinOpEmitKind::StdlibCall {
                path,
                borrow_args: false,
            }
        }
        NumericOp::Div => BinOpEmitKind::StdlibCall {
            path: quote! { incan_stdlib::num::py_div },
            borrow_args: false,
        },
        NumericOp::Add
        | NumericOp::Sub
        | NumericOp::Mul
        | NumericOp::Eq
        | NumericOp::NotEq
        | NumericOp::Lt
        | NumericOp::LtEq
        | NumericOp::Gt
        | NumericOp::GtEq => BinOpEmitKind::Infix {
            token: emit_binop_token(op),
        },
    };

    BinOpPlan {
        lhs_conv,
        rhs_conv,
        result_ty,
        emit,
    }
}

/// Determine conversion for destinations that store an owned value.
///
/// Struct fields and collection elements share this policy: literals and static `str` reads must become owned
/// `String`s when the destination type is Incan `str`, while non-Copy field reads and repeated local reads preserve
/// source-level value semantics by cloning.
fn determine_owned_storage_conversion(expr: &IrExpr, target_ty: Option<&IrType>) -> Conversion {
    match (&expr.kind, target_ty) {
        (IrExprKind::String(_), Some(IrType::String)) => Conversion::ToString,
        (IrExprKind::StaticRead { .. }, Some(IrType::String | IrType::Generic(_)))
            if is_borrowed_string_like_type(&expr.ty) =>
        {
            Conversion::ToString
        }
        (IrExprKind::StaticRead { .. }, None) if borrowed_string_like_needs_owned_string(&expr.ty, target_ty) => {
            Conversion::ToString
        }
        (_, Some(IrType::String)) if is_borrowed_string_like_type(&expr.ty) => Conversion::ToString,
        (IrExprKind::String(_), Some(IrType::Generic(_))) => Conversion::ToString,
        (_, Some(IrType::Generic(_))) if is_borrowed_string_like_type(&expr.ty) => Conversion::ToString,
        (IrExprKind::String(_), None) if string_literal_needs_owned_string(&expr.ty, target_ty) => Conversion::ToString,
        (_, None) if borrowed_string_like_needs_owned_string(&expr.ty, target_ty) => Conversion::ToString,
        _ if borrowed_expr_needs_owned_materialization(expr, target_ty) => Conversion::Clone,
        (IrExprKind::Var { access, .. }, Some(IrType::String)) if matches!(expr.ty, IrType::String) => match access {
            VarAccess::Move => Conversion::None,
            _ => Conversion::ToString,
        },
        (IrExprKind::Var { access, .. }, _) if !expr.ty.is_copy() => match access {
            VarAccess::Move => Conversion::None,
            _ => Conversion::Clone,
        },
        (IrExprKind::Field { .. }, _) if matches!(expr.ty, IrType::String) => Conversion::Clone,
        (IrExprKind::Field { .. }, _) if !expr.ty.is_copy() => Conversion::Clone,
        _ => Conversion::None,
    }
}

/// Whether a borrowed expression must be materialized before entering an owned sink.
///
/// This covers true IR borrows (`&T`, `&mut T`) and borrowed method-chain results such as
/// `box.as_ref()`. The helper is intentionally conservative when target metadata is missing:
/// generated Rust should prefer owned Incan semantics over leaking a raw borrow that would force
/// users to add `.clone()` manually.
fn borrowed_expr_needs_owned_materialization(expr: &IrExpr, target_ty: Option<&IrType>) -> bool {
    if let IrExprKind::InteropCoerce { expr, kind, .. } = &expr.kind {
        if matches!(kind, super::expr::IrInteropCoercionKind::RustTypeUnwrap) {
            return borrowed_expr_needs_owned_materialization(expr, target_ty);
        }
        return false;
    }

    let borrowed_inner = match &expr.ty {
        IrType::Ref(inner) | IrType::RefMut(inner) => inner.as_ref(),
        _ => match &expr.kind {
            IrExprKind::MethodCall { receiver, method, .. } if method == "as_ref" => match &receiver.ty {
                IrType::NamedGeneric(_, args) if args.len() == 1 => &args[0],
                _ if !matches!(target_ty, Some(IrType::Ref(_) | IrType::RefMut(_) | IrType::String)) => {
                    return true;
                }
                _ => return false,
            },
            _ => return false,
        },
    };
    if borrowed_inner.is_copy() {
        return false;
    }
    match target_ty {
        Some(IrType::Ref(_) | IrType::RefMut(_)) => false,
        Some(target_ty) => borrowed_inner == target_ty || matches!(target_ty, IrType::Generic(_)),
        // Missing target metadata happens in multi-file/library emission when a local helper's exact signature is not
        // available at the call site. Incan owned sinks still need an owned value; emitting the raw borrow leaks Rust's
        // `&T` into generated code and makes users write `.clone()` manually.
        None => true,
    }
}

/// Return whether an IR type represents Incan's canonical `Result[Ok, Err]` shape.
fn is_result_like_type(ty: &IrType) -> bool {
    match ty {
        IrType::Result(_, _) => true,
        IrType::NamedGeneric(name, args) if args.len() == 2 => {
            collections::from_str(name.rsplit("::").next().unwrap_or(name)) == Some(CollectionTypeId::Result)
        }
        _ => false,
    }
}

/// Return whether a source value has Rust borrowed/static string shape while representing Incan `str`.
fn is_borrowed_string_like_type(ty: &IrType) -> bool {
    matches!(ty, IrType::StaticStr | IrType::StrRef | IrType::FrozenStr)
}

/// Return whether a string literal needs ordinary owned `String` materialization at an Incan boundary.
///
/// Frozen targets are deliberately excluded because the target-aware emitter materializes those literals as
/// `FrozenStr` wrappers instead of converting them through `String`.
fn string_literal_needs_owned_string(source_ty: &IrType, target_ty: Option<&IrType>) -> bool {
    matches!(target_ty, Some(IrType::String | IrType::Generic(_)))
        || (target_ty.is_none() && !matches!(source_ty, IrType::FrozenStr))
}

/// Return whether an owned Incan sink needs borrowed/static string materialization.
fn borrowed_string_like_needs_owned_string(source_ty: &IrType, target_ty: Option<&IrType>) -> bool {
    is_borrowed_string_like_type(source_ty)
        && (matches!(target_ty, Some(IrType::String | IrType::Generic(_)))
            || (target_ty.is_none() && !matches!(source_ty, IrType::FrozenStr)))
}

/// Whether a value type came from Rust interop and can reasonably cross an Incan `str` boundary via `ToString`.
///
/// Lowering maps `ResolvedType::RustPath` to `IrType::Struct(path)`, so the stable signal left in IR is a Rust-style
/// path. Keep this narrower than "any struct" so user-defined Incan structs do not silently stringify at ordinary
/// `str` parameters.
fn is_rust_path_value_type(ty: &IrType) -> bool {
    match ty {
        IrType::Struct(name) | IrType::NamedGeneric(name, _) => name.contains("::"),
        IrType::Ref(inner) | IrType::RefMut(inner) => is_rust_path_value_type(inner),
        _ => false,
    }
}

/// Whether a Rust interop value should be stringified for an Incan `str` target.
fn rust_value_needs_stringification(expr: &IrExpr, target_ty: Option<&IrType>) -> bool {
    matches!(target_ty, Some(IrType::String))
        && (matches!(expr.ty, IrType::Unknown) || is_rust_path_value_type(&expr.ty))
}

/// Return whether a field expression reads through the implicit `&self` receiver.
fn field_access_reads_from_self_receiver(expr: &IrExpr) -> bool {
    let IrExprKind::Field { object, .. } = &expr.kind else {
        return false;
    };
    matches!(
        &object.kind,
        IrExprKind::Var {
            name,
            access: VarAccess::Read | VarAccess::Borrow,
            ..
        } if name == "self"
    )
}

/// Whether a field projection must clone instead of moving directly from its parent object.
///
/// Tuple-unpack temporaries are the notable exemption: lowering marks the temporary tuple binding
/// as `VarAccess::Move`, so moving `tmp.0`, then `tmp.1`, is legitimate and should not introduce
/// a backend clone. Ordinary field reads from borrowed/shared parents still need owned
/// materialization at storage and return sinks.
fn field_read_needs_owned_materialization(expr: &IrExpr) -> bool {
    match &expr.kind {
        IrExprKind::Field { object, .. } => !matches!(
            &object.kind,
            IrExprKind::Var { access, .. }
                if matches!(access, VarAccess::Move) && !matches!(object.ty, IrType::Ref(_) | IrType::RefMut(_))
        ),
        _ => false,
    }
}

/// Determines what conversion (if any) is needed for a value
///
/// ## Type-Specific Behavior
///
/// - **Strings** (`&str` → `String`): Primary focus, requires `.to_string()` in owned contexts
/// - **Primitives** (int, float, bool): Implement `Copy`, no conversion needed
/// - **Collections** (`Vec<&str>` → `Vec<String>`): Element-wise conversion
/// - **Other types**: Currently pass as-is (may be extended in the future)
///
/// ## Parameters
///
/// - `expr`: The IR expression being passed/assigned
/// - `target_ty`: Optional target type (what's expected at the destination)
/// - `context`: How the value will be used (function arg, struct field, return, etc.)
///
/// ## Returns
///
/// A [`Conversion`] strategy indicating what transformation (if any) to apply
pub fn determine_conversion(expr: &IrExpr, target_ty: Option<&IrType>, context: ConversionContext) -> Conversion {
    if let IrExprKind::InteropCoerce { expr, kind, .. } = &expr.kind
        && matches!(kind, super::expr::IrInteropCoercionKind::RustTypeUnwrap)
    {
        return determine_conversion(expr, target_ty, context);
    }
    if matches!(expr.kind, IrExprKind::InteropCoerce { .. }) {
        if borrowed_expr_needs_owned_materialization(expr, target_ty) {
            return Conversion::Clone;
        }
        return Conversion::None;
    }
    match context {
        ConversionContext::IncanFunctionArg => {
            // Incan functions expect owned values
            // Check specific conversions first, then fall back to generic .clone()
            match (&expr.kind, target_ty) {
                // String literal to String param → .to_string()
                (IrExprKind::String(_), Some(IrType::String)) => Conversion::ToString,
                // Static const reads still represent Incan `str` at ordinary call sites.
                (IrExprKind::StaticRead { .. }, Some(IrType::String | IrType::Generic(_)))
                    if is_borrowed_string_like_type(&expr.ty) =>
                {
                    Conversion::ToString
                }
                (IrExprKind::StaticRead { .. }, None)
                    if borrowed_string_like_needs_owned_string(&expr.ty, target_ty) =>
                {
                    Conversion::ToString
                }
                // Const/imported `str` values can lower as borrowed/static Rust string shapes but still follow Incan
                // owned-string semantics at call sites.
                (_, Some(IrType::String)) if is_borrowed_string_like_type(&expr.ty) => Conversion::ToString,
                // String literal to generic type param (e.g. assert_eq[T]) → owned String.
                // Typechecker constrains `T`; this keeps Incan `str` semantics in generic calls.
                (IrExprKind::String(_), Some(IrType::Generic(_))) => Conversion::ToString,
                // Generic `T` instantiated with Incan `str` must still materialize to owned `String`.
                (_, Some(IrType::Generic(_))) if is_borrowed_string_like_type(&expr.ty) => Conversion::ToString,
                // String literal with unknown target (enum variants, etc.) → .to_string()
                (IrExprKind::String(_), None) if string_literal_needs_owned_string(&expr.ty, target_ty) => {
                    Conversion::ToString
                }
                // Const `str` values need the same owned-string materialization when the target is inferred.
                (_, None) if borrowed_string_like_needs_owned_string(&expr.ty, target_ty) => Conversion::ToString,
                // Borrowed method-chain results such as `box.as_ref()` must materialize owned values at Incan call
                // boundaries.
                _ if borrowed_expr_needs_owned_materialization(expr, target_ty) => Conversion::Clone,
                // Rust interop values that cross into an Incan `str` parameter should use Rust's Display/ToString
                // boundary. This keeps callers from spelling `.to_string()` manually when matching on Rust errors.
                _ if rust_value_needs_stringification(expr, target_ty) => Conversion::ToString,

                // String variable to String param:
                // - last-use read can move ownership directly
                // - non-last-use read materializes owned String without consuming source
                (IrExprKind::Var { access, .. }, Some(IrType::String)) if matches!(expr.ty, IrType::String) => {
                    match access {
                        VarAccess::Move => Conversion::None,
                        _ => Conversion::ToString,
                    }
                }
                // Variable with non-Copy type (List, Dict, custom structs):
                // - last-use read (`Move`) can transfer ownership
                // - otherwise clone to preserve source usability
                (IrExprKind::Var { access, .. }, _) if !expr.ty.is_copy() => match access {
                    VarAccess::Move => Conversion::None,
                    _ => Conversion::Clone,
                },
                // Field access with String type → .clone() to avoid moving from struct
                (IrExprKind::Field { .. }, _) if matches!(expr.ty, IrType::String) => Conversion::Clone,
                // Field access with non-Copy type (List, Dict, structs) → .clone()
                (IrExprKind::Field { .. }, _) if !expr.ty.is_copy() => Conversion::Clone,
                // Everything else passes as-is
                _ => Conversion::None,
            }
        }

        ConversionContext::IncanFunctionArgInReturn => {
            // Inside a return statement, variables can be moved since there's no code after.
            // This avoids unnecessary clones like `return merge(left.clone(), right.clone())`
            match (&expr.kind, target_ty) {
                // String literal → .to_string()
                (IrExprKind::String(_), _) if string_literal_needs_owned_string(&expr.ty, target_ty) => {
                    Conversion::ToString
                }
                (IrExprKind::StaticRead { .. }, _) if borrowed_string_like_needs_owned_string(&expr.ty, target_ty) => {
                    Conversion::ToString
                }
                // Const/imported `str` values remain owned `str` at the Incan surface even inside return-context
                // calls.
                (_, _) if borrowed_string_like_needs_owned_string(&expr.ty, target_ty) => Conversion::ToString,
                _ if borrowed_expr_needs_owned_materialization(expr, target_ty) => Conversion::Clone,
                _ if rust_value_needs_stringification(expr, target_ty) => Conversion::ToString,

                // String variable to String param:
                // - last-use read can move ownership directly
                // - repeated reads in the same return expression must not consume early
                (IrExprKind::Var { access, .. }, Some(IrType::String)) if matches!(expr.ty, IrType::String) => {
                    match access {
                        VarAccess::Move => Conversion::None,
                        _ => Conversion::ToString,
                    }
                }

                // Non-Copy vars in return-context calls follow VarAccess:
                // - Move => transfer ownership
                // - Read/Borrow => preserve source via clone/borrow conversion
                (IrExprKind::Var { access, .. }, _) if !expr.ty.is_copy() => match access {
                    VarAccess::Move => Conversion::None,
                    _ => Conversion::Clone,
                },

                // Copy vars can always pass by value.
                (IrExprKind::Var { .. }, _) => Conversion::None,

                // Field access still needs clone (we're borrowing from a struct)
                (IrExprKind::Field { .. }, _) if !expr.ty.is_copy() => Conversion::Clone,

                // Everything else passes as-is
                _ => Conversion::None,
            }
        }

        ConversionContext::ExternalFunctionArg => {
            // External Rust functions/enum variants — use `.into()` for strings so the Rust compiler can resolve the
            // target type via the `Into` trait. This handles crates that use custom string types (e.g., Polars'
            // `PlSmallStr`) implementing `From<String>` / `From<&str>`.
            match (&expr.kind, target_ty) {
                // String literals → .into() (works for String, &str, PlSmallStr, and any From<&str>)
                (IrExprKind::String(_), _) => Conversion::Into,
                // String variables → borrow for external calls (&str param)
                (IrExprKind::StaticRead { .. }, _) if matches!(expr.ty, IrType::StaticStr) => Conversion::Into,
                (IrExprKind::Var { .. }, _) if matches!(expr.ty, IrType::StaticStr) => Conversion::Into,
                (IrExprKind::Var { access, .. }, Some(IrType::String)) if matches!(expr.ty, IrType::String) => {
                    match access {
                        VarAccess::Move => Conversion::None,
                        _ => Conversion::Clone,
                    }
                }
                (IrExprKind::Field { .. }, Some(IrType::String)) if matches!(expr.ty, IrType::String) => {
                    Conversion::Clone
                }
                (_, Some(IrType::StrRef))
                    if matches!(expr.ty, IrType::String) && !expr_has_rust_reference_shape(expr) =>
                {
                    Conversion::Borrow
                }
                (IrExprKind::Var { .. }, _) if matches!(expr.ty, IrType::String) => {
                    if expr_has_rust_reference_shape(expr) {
                        Conversion::None
                    } else {
                        Conversion::Borrow
                    }
                }
                (IrExprKind::Field { .. }, None) if matches!(expr.ty, IrType::String) => {
                    if expr_has_rust_reference_shape(expr) {
                        Conversion::None
                    } else {
                        Conversion::Borrow
                    }
                }
                (_, None) if matches!(expr.ty, IrType::String) && !expr_has_rust_reference_shape(expr) => {
                    Conversion::Borrow
                }
                (_, Some(IrType::Ref(_))) if !expr_has_rust_reference_shape(expr) => Conversion::Borrow,
                (_, Some(IrType::RefMut(_))) if !expr_has_rust_reference_shape(expr) => Conversion::MutBorrow,
                // Rust adapter leaves commonly accept borrowed handles (`&Sender<T>`, `&Mutex<T>`, ...).
                // When metadata is unavailable, do not move non-Copy wrapper fields out of `&self`.
                (IrExprKind::Field { .. }, None)
                    if !expr.ty.is_copy() && field_access_reads_from_self_receiver(expr) =>
                {
                    Conversion::Borrow
                }
                // Everything else as-is (Rust's type system handles it)
                _ => Conversion::None,
            }
        }

        ConversionContext::StructField | ConversionContext::CollectionElement => {
            determine_owned_storage_conversion(expr, target_ty)
        }

        ConversionContext::MethodArg => {
            // Method arguments usually don't need conversion (Rust's Borrow trait)
            Conversion::None
        }

        ConversionContext::Assignment => {
            // Assignments and let bindings need conversion for string literals
            match (&expr.kind, target_ty) {
                // String literal assigned to String variable → .to_string()
                (IrExprKind::String(_), Some(IrType::String)) => Conversion::ToString,
                (IrExprKind::StaticRead { .. }, Some(IrType::String | IrType::Generic(_)))
                    if is_borrowed_string_like_type(&expr.ty) =>
                {
                    Conversion::ToString
                }
                (_, Some(IrType::String)) if is_borrowed_string_like_type(&expr.ty) => Conversion::ToString,
                _ if borrowed_expr_needs_owned_materialization(expr, target_ty) => Conversion::Clone,
                (IrExprKind::Field { .. }, _)
                    if matches!(expr.ty, IrType::String) && field_read_needs_owned_materialization(expr) =>
                {
                    Conversion::Clone
                }
                (IrExprKind::Field { .. }, _) if !expr.ty.is_copy() && field_read_needs_owned_materialization(expr) => {
                    Conversion::Clone
                }
                _ => Conversion::None,
            }
        }

        ConversionContext::ReturnValue => {
            // Return values must match function signature (owned)
            match (&expr.kind, target_ty) {
                // String literal returned when function returns String → .to_string()
                (IrExprKind::String(_), Some(IrType::String)) => Conversion::ToString,
                (IrExprKind::StaticRead { .. }, Some(IrType::String)) if is_borrowed_string_like_type(&expr.ty) => {
                    Conversion::ToString
                }
                (_, Some(IrType::String)) if is_borrowed_string_like_type(&expr.ty) => Conversion::ToString,
                _ if borrowed_expr_needs_owned_materialization(expr, target_ty) => Conversion::Clone,
                // Non-Copy vars can move on last use; otherwise materialize an owned return value.
                (IrExprKind::Var { access, .. }, _) if !expr.ty.is_copy() => match access {
                    VarAccess::Move => Conversion::None,
                    _ => Conversion::Clone,
                },
                // Field access returns borrowed data from the parent object; clone to satisfy owned return semantics.
                (IrExprKind::Field { .. }, _) if !expr.ty.is_copy() => Conversion::Clone,
                // Other cases: as-is
                _ => Conversion::None,
            }
        }

        ConversionContext::MatchScrutinee => {
            // Rust `match` can consume its scrutinee. Preserve existing Incan-owned value semantics by cloning
            // ordinary non-Copy locals when needed, but do not force `.clone()` onto Rust Result values whose Ok/Err
            // payloads may be non-Clone (`std::fs::DirEntry`, `std::io::Error`, ...).
            match (&expr.kind, &expr.ty) {
                (IrExprKind::Var { .. }, IrType::Unknown) => Conversion::None,
                (IrExprKind::Var { .. }, ty) if is_result_like_type(ty) => Conversion::None,
                (IrExprKind::Var { access, .. }, _) if !expr.ty.is_copy() => match access {
                    VarAccess::Move => Conversion::None,
                    _ => Conversion::Clone,
                },
                (IrExprKind::Field { .. }, _) if !expr.ty.is_copy() => Conversion::Clone,
                _ => Conversion::None,
            }
        }
    }
}

/// Returns true when lowering/emission treats this Incan parameter like `name: &mut RustTy` rather than
/// `mut name: RustTy` (small scalars).
fn mut_param_passed_by_rust_mut_ref(ty: &IrType) -> bool {
    !matches!(ty, IrType::Int | IrType::Float | IrType::Bool)
}

/// Returns true when mutable arguments should bypass Incan value materialization at call sites.
///
/// Mutable `str` parameters use `&mut String` in Rust but still need normal Incan argument conversions (e.g.
/// `.to_string()` for literals).
fn mut_param_skips_incan_value_conversions(ty: &IrType) -> bool {
    !matches!(ty, IrType::Int | IrType::Float | IrType::Bool | IrType::String)
}

/// Predicate shared by call emission: mutable non-scalar parameters are reborrowed as `&mut T` at call sites.
pub(crate) fn incan_mutable_param_passed_as_rust_mut_ref(param: &FunctionParam) -> bool {
    param.mutability == Mutability::Mutable && mut_param_passed_by_rust_mut_ref(&param.ty)
}

fn incan_mutable_param_skips_incan_value_conversions(param: &FunctionParam) -> bool {
    param.mutability == Mutability::Mutable && mut_param_skips_incan_value_conversions(&param.ty)
}

/// Like [`determine_conversion`], but uses callee parameter metadata when available.
///
/// For mutable aggregate parameters, codegen emits `&mut` of the binding. The generic Incan rule that clones non-copy
/// locals on non-final reads would produce an owned value and break Rust (`expected &mut Vec`, `found Vec`).
pub(crate) fn determine_conversion_for_incan_call(
    expr: &IrExpr,
    target_ty: Option<&IrType>,
    context: ConversionContext,
    callee_param: Option<&FunctionParam>,
) -> Conversion {
    if matches!(
        context,
        ConversionContext::IncanFunctionArg | ConversionContext::IncanFunctionArgInReturn
    ) {
        match target_ty {
            Some(IrType::Ref(_)) => match &expr.ty {
                _ if expr_has_rust_reference_shape(expr) => return Conversion::None,
                _ => return Conversion::Borrow,
            },
            Some(IrType::RefMut(_)) => match &expr.ty {
                _ if expr_has_rust_reference_shape(expr) => return Conversion::None,
                _ => return Conversion::MutBorrow,
            },
            _ => {}
        }
    }
    if matches!(
        context,
        ConversionContext::IncanFunctionArg | ConversionContext::IncanFunctionArgInReturn
    ) && callee_param.is_some_and(incan_mutable_param_skips_incan_value_conversions)
    {
        return Conversion::None;
    }
    determine_conversion(expr, target_ty, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::decl::FunctionParam;
    use crate::backend::ir::expr::{MethodCallArgPolicy, VarAccess, VarRefKind};
    use crate::backend::ir::types::Mutability;

    // === IncanFunctionArg Tests ===

    #[test]
    fn test_incan_call_skips_clone_for_mutable_list_param_issue244() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "pending".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::List(Box::new(IrType::Int)),
        );
        let param = FunctionParam {
            name: "pending".to_string(),
            ty: IrType::List(Box::new(IrType::Int)),
            mutability: Mutability::Mutable,
            is_self: false,
            kind: crate::frontend::ast::ParamKind::Normal,
            default: None,
        };
        let conv = determine_conversion_for_incan_call(
            &expr,
            Some(&param.ty),
            ConversionContext::IncanFunctionArg,
            Some(&param),
        );
        assert_eq!(
            conv,
            Conversion::None,
            "mutable aggregate args must not clone at call sites"
        );
    }

    #[test]
    fn test_incan_call_keeps_string_conversion_for_mutable_string_param_issue244() {
        let expr = IrExpr::new(IrExprKind::String("x".to_string()), IrType::String);
        let param = FunctionParam {
            name: "s".to_string(),
            ty: IrType::String,
            mutability: Mutability::Mutable,
            is_self: false,
            kind: crate::frontend::ast::ParamKind::Normal,
            default: None,
        };
        let conv = determine_conversion_for_incan_call(
            &expr,
            Some(&param.ty),
            ConversionContext::IncanFunctionArg,
            Some(&param),
        );
        assert_eq!(
            conv,
            Conversion::ToString,
            "mutable string params still require normal Incan string conversion"
        );
    }

    #[test]
    fn test_incan_function_string_literal_to_string() {
        let expr = IrExpr::new(IrExprKind::String("test".to_string()), IrType::String);
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_incan_function_string_var_to_string() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "s".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::String,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_string_var_read_to_string() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "s".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::String,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_incan_function_static_str_var_to_string() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "s".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::StaticBinding,
            },
            IrType::StaticStr,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_incan_function_frozen_str_var_to_string() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "s".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::FrozenStr,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_incan_function_frozen_str_literal_without_target_stays_frozen() {
        let expr = IrExpr::new(IrExprKind::String("policy".to_string()), IrType::FrozenStr);

        let conv = determine_conversion(&expr, None, ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_frozen_str_static_read_without_target_stays_frozen() {
        let expr = IrExpr::new(
            IrExprKind::StaticRead {
                name: "POLICY".to_string(),
            },
            IrType::FrozenStr,
        );

        let conv = determine_conversion(&expr, None, ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_static_str_var_to_generic() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "s".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::StaticBinding,
            },
            IrType::StaticStr,
        );
        let target = IrType::Generic("T".to_string());

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_assignment_frozen_str_var_to_string() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "s".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::FrozenStr,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::Assignment);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_incan_function_rust_path_value_to_string_param() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "err".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("std::io::Error".to_string()),
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(
            conv,
            Conversion::ToString,
            "Rust interop values passed to Incan str parameters should stringify instead of cloning"
        );
    }

    #[test]
    fn test_incan_function_unknown_rust_payload_to_string_param() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "err".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Unknown,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(
            conv,
            Conversion::ToString,
            "Rust-inspected unknown payloads passed to Incan str parameters should stringify instead of cloning"
        );
    }

    #[test]
    fn test_incan_function_local_struct_to_string_param_does_not_stringify_implicitly() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "user".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("User".to_string()),
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(
            conv,
            Conversion::Clone,
            "only Rust-path values should receive implicit ToString conversion for str parameters"
        );
    }

    #[test]
    fn test_incan_function_static_read_to_string() {
        let expr = IrExpr::new(
            IrExprKind::StaticRead {
                name: "PREFIX".to_string(),
            },
            IrType::StaticStr,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_incan_function_static_read_int_to_generic_stays_as_is() {
        let expr = IrExpr::new(
            IrExprKind::StaticRead {
                name: "MARKER".to_string(),
            },
            IrType::Int,
        );
        let target = IrType::Generic("T".to_string());

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_string_list_var_uses_owned_incan_semantics() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "items".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::List(Box::new(IrType::String)),
        );
        let target = IrType::List(Box::new(IrType::String));

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_empty_list_to_string_list_skips_collect_conversion() {
        let expr = IrExpr::new(IrExprKind::List(Vec::new()), IrType::List(Box::new(IrType::String)));
        let target = IrType::List(Box::new(IrType::String));

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_int_no_conversion() {
        // Primitives implement Copy - no conversion needed
        let expr = IrExpr::new(IrExprKind::Int(42), IrType::Int);
        let target = IrType::Int;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_float_no_conversion() {
        // Primitives implement Copy - no conversion needed
        let expr = IrExpr::new(IrExprKind::Float(std::f64::consts::PI), IrType::Float);
        let target = IrType::Float;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_bool_no_conversion() {
        // Primitives implement Copy - no conversion needed
        let expr = IrExpr::new(IrExprKind::Bool(true), IrType::Bool);
        let target = IrType::Bool;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_noncopy_var_last_use_moves() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "user".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("User".to_string()),
        );
        let target = IrType::Struct("User".to_string());

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_function_noncopy_var_non_last_use_clones() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "user".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("User".to_string()),
        );
        let target = IrType::Struct("User".to_string());

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::Clone);
    }

    #[test]
    fn test_assignment_static_str_to_string() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "prefix".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::StaticBinding,
            },
            IrType::StaticStr,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::Assignment);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_assignment_static_read_int_stays_as_is() {
        let expr = IrExpr::new(
            IrExprKind::StaticRead {
                name: "MARKER".to_string(),
            },
            IrType::Int,
        );
        let target = IrType::Int;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::Assignment);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_return_static_str_to_string() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "prefix".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::StaticBinding,
            },
            IrType::StaticStr,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::ReturnValue);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_return_static_read_to_string() {
        let expr = IrExpr::new(
            IrExprKind::StaticRead {
                name: "PREFIX".to_string(),
            },
            IrType::StaticStr,
        );
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::ReturnValue);
        assert_eq!(conv, Conversion::ToString);
    }

    // === ExternalFunctionArg Tests ===

    #[test]
    fn test_external_function_string_literal() {
        // External Rust function args use `.into()` so the Rust compiler can resolve the target type via the `Into`
        // trait — handles crates with custom string types (e.g., Polars' `PlSmallStr`) that implement `From<String>` or
        // `From<&str>`.
        let expr = IrExpr::new(IrExprKind::String("test".to_string()), IrType::String);

        let conv = determine_conversion(&expr, None, ConversionContext::ExternalFunctionArg);
        assert_eq!(conv, Conversion::Into);
    }

    #[test]
    fn test_external_function_string_var_borrow() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "s".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::String,
        );

        let conv = determine_conversion(&expr, None, ConversionContext::ExternalFunctionArg);
        assert_eq!(conv, Conversion::Borrow);
    }

    #[test]
    fn test_external_function_string_expression_to_str_ref_borrows_issue716() {
        let expr = IrExpr::new(IrExprKind::Format { parts: Vec::new() }, IrType::String);

        let conv = determine_conversion(&expr, Some(&IrType::StrRef), ConversionContext::ExternalFunctionArg);
        assert_eq!(conv, Conversion::Borrow);

        let conv = determine_conversion(&expr, None, ConversionContext::ExternalFunctionArg);
        assert_eq!(conv, Conversion::Borrow);
    }

    #[test]
    fn test_external_function_as_slice_arg_does_not_double_borrow() {
        let expr = IrExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "data".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Bytes,
                )),
                method: "as_slice".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Bytes,
        );

        let conv = determine_conversion(&expr, None, ConversionContext::ExternalFunctionArg);
        assert_eq!(
            conv,
            Conversion::None,
            "an explicit as_slice() argument is already a Rust borrow boundary"
        );

        let target = IrType::Ref(Box::new(IrType::Bytes));
        let conv = determine_conversion(&expr, Some(&target), ConversionContext::ExternalFunctionArg);
        assert_eq!(
            conv,
            Conversion::None,
            "an explicit as_slice() argument must not become &&[u8] for ref targets"
        );
    }

    #[test]
    fn test_external_function_string_var_with_by_value_target_does_not_borrow() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "s".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::String,
        );

        let conv = determine_conversion(&expr, Some(&IrType::String), ConversionContext::ExternalFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_external_function_field_with_by_value_target_does_not_borrow() {
        let rust_duration = IrType::Struct("std::time::Duration".to_string());
        let expr = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "other".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("Duration".to_string()),
                )),
                field: "value".to_string(),
            },
            rust_duration.clone(),
        );

        let conv = determine_conversion(&expr, Some(&rust_duration), ConversionContext::ExternalFunctionArg);
        assert_eq!(
            conv,
            Conversion::None,
            "field-backed Rust values passed to by-value Rust params must not be borrowed"
        );
    }

    #[test]
    fn test_external_function_known_field_without_target_does_not_guess_borrow() {
        let expr = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "other".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("Duration".to_string()),
                )),
                field: "value".to_string(),
            },
            IrType::Struct("std::time::Duration".to_string()),
        );

        let conv = determine_conversion(&expr, None, ConversionContext::ExternalFunctionArg);
        assert_eq!(
            conv,
            Conversion::None,
            "known Rust field values must stay by-value when metadata is unavailable"
        );
    }

    #[test]
    fn test_external_function_string_field_without_target_borrows_like_string_variable() {
        let expr = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "path".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("Path".to_string()),
                )),
                field: "0".to_string(),
            },
            IrType::String,
        );

        let conv = determine_conversion(&expr, None, ConversionContext::ExternalFunctionArg);
        assert_eq!(
            conv,
            Conversion::Borrow,
            "metadata-free Rust calls should borrow string-backed field projections"
        );
    }

    #[test]
    fn test_external_function_self_field_without_target_borrows_noncopy_receiver_field() {
        let expr = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "self".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("Sender".to_string()),
                )),
                field: "0".to_string(),
            },
            IrType::Struct("incan_stdlib::async::channel::Sender<T>".to_string()),
        );

        let conv = determine_conversion(&expr, None, ConversionContext::ExternalFunctionArg);
        assert_eq!(
            conv,
            Conversion::Borrow,
            "metadata-free Rust calls must not move non-Copy wrapper fields out of &self"
        );
    }

    #[test]
    fn test_external_function_unknown_field_without_target_keeps_adapter_borrow_fallback() {
        let expr = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "self".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("Wrapper".to_string()),
                )),
                field: "0".to_string(),
            },
            IrType::Unknown,
        );

        let conv = determine_conversion(&expr, None, ConversionContext::ExternalFunctionArg);
        assert_eq!(conv, Conversion::Borrow);
    }

    #[test]
    fn test_external_function_field_with_ref_target_borrows() {
        let rust_duration = IrType::Struct("std::time::Duration".to_string());
        let expr = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "other".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("Duration".to_string()),
                )),
                field: "value".to_string(),
            },
            rust_duration.clone(),
        );
        let target = IrType::Ref(Box::new(rust_duration));

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::ExternalFunctionArg);
        assert_eq!(
            conv,
            Conversion::Borrow,
            "field-backed Rust values still borrow when metadata says the Rust param is by-reference"
        );
    }

    #[test]
    fn test_external_function_int_no_conversion() {
        let expr = IrExpr::new(IrExprKind::Int(42), IrType::Int);

        let conv = determine_conversion(&expr, None, ConversionContext::ExternalFunctionArg);
        assert_eq!(conv, Conversion::None);
    }

    // === StructField Tests ===

    #[test]
    fn test_struct_field_string_literal() {
        let expr = IrExpr::new(IrExprKind::String("Alice".to_string()), IrType::String);

        let conv = determine_conversion(&expr, None, ConversionContext::StructField);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_struct_field_int_no_conversion() {
        let expr = IrExpr::new(IrExprKind::Int(30), IrType::Int);

        let conv = determine_conversion(&expr, None, ConversionContext::StructField);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_struct_field_string_var_no_conversion() {
        // String variables in struct fields are passed as-is (already owned)
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "name".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::String,
        );

        let conv = determine_conversion(&expr, None, ConversionContext::StructField);
        assert_eq!(conv, Conversion::None);
    }

    // === Assignment Tests ===

    #[test]
    fn test_assignment_string_literal_to_string() {
        let expr = IrExpr::new(IrExprKind::String("test".to_string()), IrType::String);
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::Assignment);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_assignment_int_no_conversion() {
        let expr = IrExpr::new(IrExprKind::Int(42), IrType::Int);
        let target = IrType::Int;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::Assignment);
        assert_eq!(conv, Conversion::None);
    }

    // === ReturnValue Tests ===

    #[test]
    fn test_return_string_literal_to_string() {
        let expr = IrExpr::new(IrExprKind::String("result".to_string()), IrType::String);
        let target = IrType::String;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::ReturnValue);
        assert_eq!(conv, Conversion::ToString);
    }

    #[test]
    fn test_return_int_no_conversion() {
        let expr = IrExpr::new(IrExprKind::Int(42), IrType::Int);
        let target = IrType::Int;

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::ReturnValue);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_return_call_context_noncopy_var_read_clones() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "user".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("User".to_string()),
        );
        let target = IrType::Struct("User".to_string());

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArgInReturn);
        assert_eq!(conv, Conversion::Clone);
    }

    #[test]
    fn test_return_call_context_noncopy_var_move_stays_move() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "user".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("User".to_string()),
        );
        let target = IrType::Struct("User".to_string());

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArgInReturn);
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_return_call_context_frozen_str_target_preserves_frozen_literal() {
        let expr = IrExpr::new(IrExprKind::String("policy".to_string()), IrType::FrozenStr);

        let conv = determine_conversion(
            &expr,
            Some(&IrType::FrozenStr),
            ConversionContext::IncanFunctionArgInReturn,
        );
        assert_eq!(conv, Conversion::None);
    }

    #[test]
    fn test_incan_call_borrows_noncopy_var_for_ref_target() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "user".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("User".to_string()),
        );
        let target = IrType::Ref(Box::new(IrType::Struct("User".to_string())));

        let conv = determine_conversion_for_incan_call(&expr, Some(&target), ConversionContext::IncanFunctionArg, None);
        assert_eq!(conv, Conversion::Borrow);
    }

    #[test]
    fn test_return_call_context_borrows_noncopy_var_for_ref_target() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "user".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("User".to_string()),
        );
        let target = IrType::Ref(Box::new(IrType::Struct("User".to_string())));

        let conv = determine_conversion_for_incan_call(
            &expr,
            Some(&target),
            ConversionContext::IncanFunctionArgInReturn,
            None,
        );
        assert_eq!(conv, Conversion::Borrow);
    }

    #[test]
    fn test_incan_call_clones_borrowed_as_ref_result_for_owned_nominal_target() {
        let expr = IrExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "child".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::NamedGeneric("Box".to_string(), vec![IrType::Struct("Node".to_string())]),
                )),
                method: "as_ref".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: crate::backend::ir::expr::MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );
        let target = IrType::Struct("Node".to_string());

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::Clone);
    }

    #[test]
    fn test_incan_call_clones_borrowed_as_ref_result_when_target_is_unknown() {
        let expr = IrExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "child".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::NamedGeneric("Box".to_string(), vec![IrType::Struct("Node".to_string())]),
                )),
                method: "as_ref".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: crate::backend::ir::expr::MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );

        let conv = determine_conversion(&expr, None, ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::Clone);
    }

    #[test]
    fn test_incan_call_clones_interop_unwrapped_as_ref_result_when_target_is_unknown() {
        let inner = IrExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "child".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::NamedGeneric("Box".to_string(), vec![IrType::Struct("Node".to_string())]),
                )),
                method: "as_ref".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: crate::backend::ir::expr::MethodCallArgPolicy::Default,
            },
            IrType::Ref(Box::new(IrType::Struct("Node".to_string()))),
        );
        let expr = IrExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(inner),
                from_ty: IrType::Ref(Box::new(IrType::Struct("Node".to_string()))),
                to_ty: IrType::Ref(Box::new(IrType::Struct("Node".to_string()))),
                kind: crate::backend::ir::expr::IrInteropCoercionKind::RustTypeUnwrap,
            },
            IrType::Ref(Box::new(IrType::Struct("Node".to_string()))),
        );

        let conv = determine_conversion(&expr, None, ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::Clone);
    }

    #[test]
    fn test_incan_call_clones_erased_receiver_as_ref_result_when_target_is_unknown() {
        let expr = IrExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "child".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Unknown,
                )),
                method: "as_ref".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: crate::backend::ir::expr::MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );

        let conv = determine_conversion(&expr, None, ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::Clone);
    }

    #[test]
    fn test_incan_call_clones_erased_receiver_as_ref_result_for_owned_nominal_target() {
        let expr = IrExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "child".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Unknown,
                )),
                method: "as_ref".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: crate::backend::ir::expr::MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );
        let target = IrType::Struct("Node".to_string());

        let conv = determine_conversion(&expr, Some(&target), ConversionContext::IncanFunctionArg);
        assert_eq!(conv, Conversion::Clone);
    }

    #[test]
    fn test_assignment_clones_interop_unwrapped_string_field() {
        let inner = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "assignment".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("ProjectionAssignment".to_string()),
                )),
                field: "output_name".to_string(),
            },
            IrType::String,
        );
        let expr = IrExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(inner),
                from_ty: IrType::String,
                to_ty: IrType::String,
                kind: crate::backend::ir::expr::IrInteropCoercionKind::RustTypeUnwrap,
            },
            IrType::String,
        );

        let conv = determine_conversion(&expr, Some(&IrType::String), ConversionContext::Assignment);
        assert_eq!(conv, Conversion::Clone);
    }

    #[test]
    fn test_assignment_moves_owned_tuple_unpack_field_without_clone() {
        let expr = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "__incan_tuple_unpack_tx_rx".to_string(),
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Tuple(vec![
                        IrType::Struct("Sender".to_string()),
                        IrType::Struct("Receiver".to_string()),
                    ]),
                )),
                field: "1".to_string(),
            },
            IrType::Struct("Receiver".to_string()),
        );

        let conv = determine_conversion(
            &expr,
            Some(&IrType::Struct("Receiver".to_string())),
            ConversionContext::Assignment,
        );
        assert_eq!(conv, Conversion::None);
    }

    // === MethodArg Tests ===

    #[test]
    fn test_method_arg_no_conversion() {
        // Method args rely on Rust's Borrow trait - no conversion needed
        let expr = IrExpr::new(IrExprKind::String("test".to_string()), IrType::String);

        let conv = determine_conversion(&expr, None, ConversionContext::MethodArg);
        assert_eq!(conv, Conversion::None);
    }

    // === Conversion Application Tests ===

    #[test]
    fn test_apply_none() {
        let tokens = quote::quote! { value };
        let result = Conversion::None.apply(tokens.clone());
        assert_eq!(result.to_string(), tokens.to_string());
    }

    #[test]
    fn test_apply_to_string() {
        let tokens = quote::quote! { "test" };
        let result = Conversion::ToString.apply(tokens);
        assert_eq!(result.to_string(), "\"test\" . to_string ()");
    }

    #[test]
    fn test_apply_borrow() {
        let tokens = quote::quote! { value };
        let result = Conversion::Borrow.apply(tokens);
        assert_eq!(result.to_string(), "& value");
    }

    #[test]
    fn test_apply_mut_borrow() {
        let tokens = quote::quote! { value };
        let result = Conversion::MutBorrow.apply(tokens);
        assert_eq!(result.to_string(), "& mut value");
    }

    #[test]
    fn test_apply_clone() {
        let tokens = quote::quote! { value };
        let result = Conversion::Clone.apply(tokens);
        assert_eq!(result.to_string(), "value . clone ()");
    }
}
