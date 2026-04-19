//! Centralized type conversion and borrow checking for code generation
//!
//! Incan doesn't have manual borrowing like Rust, but when generating Rust code, we need to ensure that
//! values are correctly converted between owned and borrowed forms as needed.
//!
//! This module provides a **single source of truth** for determining when and how to convert values between different
//! ownership/borrowing states during code generation.
//!
//! ## Why Focus on Strings?
//!
//! This module **primarily handles string conversions** because strings are the main source of borrow/ownership
//! mismatches when compiling to Rust:
//!
//! - **Primitives** (`int`, `float`, `bool`) implement `Copy` in Rust, so they pass by value automatically—no
//!   conversion needed
//! - **Strings** have a fundamental split in Rust: `&str` (borrowed, stack) vs `String` (owned, heap)
//! - Incan's `str` type abstracts this away (like Python), but codegen must handle it
//! - String literals like `"hello"` are `&'static str` in Rust, requiring `.to_string()` for owned contexts
//!
//! **Other types currently supported:**
//! - `Vec<&str>` → `Vec<String>` conversion for collections
//! - `List[T] + List[T]` concatenation via `incan_stdlib::collections::list_concat`
//!
//! **Future extensions may include:**
//! - Collections with non-Copy elements (may need `.clone()`)
//! - Custom types passed to external functions (may need `&` or `&mut`)
//! - `Option<T>` / `Result<T, E>` with mismatched inner types
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
//! - Vec<&str> → `.into_iter().map(|s| s.to_string()).collect()`
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
//! ### StructField
//!
//! Struct fields are always **owned**:
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
use super::types::Mutability;
use super::{IrExpr, IrExprKind, IrType, TypedExpr};
use crate::numeric_adapters::{ir_type_to_numeric_ty, numeric_op_from_ir, pow_exponent_kind_from_ir};
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
    /// Argument to a method call (context-dependent)
    MethodArg,
    /// Assignment or let binding
    Assignment,
    /// Return value from a function
    ReturnValue,
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
    /// Convert `Vec<&str>` to `Vec<String>`.
    VecStringConversion,
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
            Conversion::VecStringConversion => {
                quote! { #tokens.into_iter().map(|s| s.to_string()).collect() }
            }
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
    if matches!(expr.kind, IrExprKind::InteropCoerce { .. }) {
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
                    if matches!(expr.ty, IrType::StaticStr) =>
                {
                    Conversion::ToString
                }
                (IrExprKind::StaticRead { .. }, None) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                // Const `str` values lower as `&'static str` but still follow Incan owned-string semantics at call
                // sites.
                (_, Some(IrType::String)) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                // String literal to generic type param (e.g. assert_eq[T]) → owned String.
                // Typechecker constrains `T`; this keeps Incan `str` semantics in generic calls.
                (IrExprKind::String(_), Some(IrType::Generic(_))) => Conversion::ToString,
                // Generic `T` instantiated with Incan `str` must still materialize to owned `String`.
                (_, Some(IrType::Generic(_))) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                // String literal with unknown target (enum variants, etc.) → .to_string()
                (IrExprKind::String(_), None) => Conversion::ToString,
                // Const `str` values need the same owned-string materialization when the target is inferred.
                (_, None) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,

                // Vec<&str> to Vec<String> - check before generic variable handling
                (_, Some(IrType::List(elem))) if matches!(elem.as_ref(), IrType::String) => {
                    if matches!(expr.ty, IrType::List(_)) {
                        match &expr.kind {
                            IrExprKind::List(items) if items.is_empty() => Conversion::None,
                            _ => Conversion::VecStringConversion,
                        }
                    } else {
                        Conversion::None
                    }
                }

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
                (IrExprKind::String(_), _) => Conversion::ToString,
                (IrExprKind::StaticRead { .. }, _) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                // Const `str` values remain owned `str` at the Incan surface even inside return-context calls.
                (_, _) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,

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
            match &expr.kind {
                // String literals → .into() (works for String, &str, PlSmallStr, and any From<&str>)
                IrExprKind::String(_) => Conversion::Into, // String variables → borrow for external calls (&str param)
                IrExprKind::StaticRead { .. } if matches!(expr.ty, IrType::StaticStr) => Conversion::Into,
                IrExprKind::Var { .. } if matches!(expr.ty, IrType::StaticStr) => Conversion::Into,
                IrExprKind::Var { .. } if matches!(expr.ty, IrType::String) => Conversion::Borrow,
                // Rust adapter leaves commonly accept borrowed handles (`&Sender<T>`, `&Mutex<T>`, ...).
                // When the frontend cannot surface an explicit `&T` parameter type for inline Rust imports,
                // preserve handle ownership by borrowing field-based wrapper access like `self.0`.
                IrExprKind::Field { .. } if matches!(expr.ty, IrType::Unknown) || !expr.ty.is_copy() => {
                    Conversion::Borrow
                }
                // Everything else as-is (Rust's type system handles it)
                _ => Conversion::None,
            }
        }

        ConversionContext::StructField => {
            // Struct fields are owned sinks, so field reads and non-final local reads need the same materialization
            // rules as ordinary Incan-owned call arguments.
            match (&expr.kind, target_ty) {
                (IrExprKind::String(_), Some(IrType::String)) => Conversion::ToString,
                (IrExprKind::StaticRead { .. }, Some(IrType::String | IrType::Generic(_)))
                    if matches!(expr.ty, IrType::StaticStr) =>
                {
                    Conversion::ToString
                }
                (IrExprKind::StaticRead { .. }, None) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                (_, Some(IrType::String)) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                (IrExprKind::String(_), Some(IrType::Generic(_))) => Conversion::ToString,
                (_, Some(IrType::Generic(_))) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                (IrExprKind::String(_), None) => Conversion::ToString,
                (_, None) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                (_, Some(IrType::List(elem))) if matches!(elem.as_ref(), IrType::String) => {
                    if matches!(expr.ty, IrType::List(_)) {
                        match &expr.kind {
                            IrExprKind::List(items) if items.is_empty() => Conversion::None,
                            _ => Conversion::VecStringConversion,
                        }
                    } else {
                        Conversion::None
                    }
                }
                (IrExprKind::Var { access, .. }, Some(IrType::String)) if matches!(expr.ty, IrType::String) => {
                    match access {
                        VarAccess::Move => Conversion::None,
                        _ => Conversion::ToString,
                    }
                }
                (IrExprKind::Var { access, .. }, _) if !expr.ty.is_copy() => match access {
                    VarAccess::Move => Conversion::None,
                    _ => Conversion::Clone,
                },
                (IrExprKind::Field { .. }, _) if matches!(expr.ty, IrType::String) => Conversion::Clone,
                (IrExprKind::Field { .. }, _) if !expr.ty.is_copy() => Conversion::Clone,
                _ => Conversion::None,
            }
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
                    if matches!(expr.ty, IrType::StaticStr) =>
                {
                    Conversion::ToString
                }
                (_, Some(IrType::String)) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                _ => Conversion::None,
            }
        }

        ConversionContext::ReturnValue => {
            // Return values must match function signature (owned)
            match (&expr.kind, target_ty) {
                // String literal returned when function returns String → .to_string()
                (IrExprKind::String(_), Some(IrType::String)) => Conversion::ToString,
                (IrExprKind::StaticRead { .. }, Some(IrType::String)) if matches!(expr.ty, IrType::StaticStr) => {
                    Conversion::ToString
                }
                (_, Some(IrType::String)) if matches!(expr.ty, IrType::StaticStr) => Conversion::ToString,
                // Other cases: as-is
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
    use crate::backend::ir::expr::{VarAccess, VarRefKind};
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
    fn test_incan_function_vec_string_conversion() {
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
        assert_eq!(conv, Conversion::VecStringConversion);
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

    #[test]
    fn test_apply_vec_string_conversion() {
        let tokens = quote::quote! { items };
        let result = Conversion::VecStringConversion.apply(tokens);
        assert_eq!(
            result.to_string(),
            "items . into_iter () . map (| s | s . to_string ()) . collect ()"
        );
    }
}
