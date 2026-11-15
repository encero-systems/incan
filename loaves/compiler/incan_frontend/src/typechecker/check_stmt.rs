//! Statement checking: assignments, returns, control flow.

use std::collections::HashSet;

use crate::ast::*;
use crate::diagnostics::errors::{self, SelfMutation};
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved};
use crate::symbols::*;
use incan_lang::NumericTy;
use incan_lang::lang::derives::DeriveId;
use incan_lang::lang::errors as runtime_errors;
use incan_lang::lang::keywords;
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::traits::{self as builtin_traits, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::SurfaceStmtTypeCheck;
use incan_semantics_core::rust_tuple_arity;

use super::derive_requirements::DeriveSupport;
use super::{
    CAbiRawCallResult, CAbiSpanLocal, CBindingType, COutputMode, LoopContextKind, OpenRustGenericBinding,
    OpenRustGenericCall, ResolvedMethodDispatch, TypeChecker, YieldContext, ownership_contracts, type_info,
};

mod assignments;
mod chained;
mod control_statements;
mod narrowing;
mod native_bindings;
mod place_writes;
mod statements;
use crate::typechecker::helpers::{collection_type_id, ensure_bool_condition, option_ty};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssertIsPatternKind {
    Some,
    None,
    Ok,
    Err,
}

struct AssertIsPattern {
    kind: AssertIsPatternKind,
    constructor_span: Span,
    binding: Option<(String, Span)>,
}

#[derive(Clone)]
struct BranchNarrowing {
    name: String,
    true_ty: ResolvedType,
    false_ty: Option<ResolvedType>,
    is_mutable: bool,
    span: Span,
}

#[derive(Clone)]
struct BranchRefinement {
    name: String,
    ty: ResolvedType,
    is_mutable: bool,
    span: Span,
}

/// What a value's type says about whether it can be destructured into a fixed number of names.
///
/// Reading this once, in one place, is what keeps the three destructuring sites honest. Each used to answer the
/// question inline and fall back to `vec![Unknown; n]` for anything it did not recognize, which sized the element
/// list to the name list and so made the arity guard below it unreachable for a non-tuple (#1125, #1132).
#[derive(Debug)]
pub(in crate::typechecker) enum TupleShape {
    /// A tuple in either Incan spelling, carrying its element types so arity can be checked against them.
    Tuple(Vec<ResolvedType>),
    /// A Rust-interop tuple whose arity the compiler can actually read, such as the `(String, JsonValue)` a
    /// `rust::HashMap::items()` yields.
    ///
    /// Element types are not modeled, so it carries only the count and binds `Unknown` per name. Arity is still
    /// checked, so interop is not simply exempted from the guard.
    RustTuple(usize),
    /// A Rust-interop value whose shape the compiler cannot establish.
    ///
    /// Deliberately *not* [`TupleShape::Recovery`]. "The frontend has no structural model of this type" is not the
    /// same claim as "checking already failed", and treating it as recovery would let a genuine non-tuple Rust
    /// value reach a generated field projection — the exact leakage class #1132 exists to close, arriving through
    /// interop instead of through `int`. This is the same rule applied to a bare type variable: not proven
    /// tuple-shaped must refuse.
    OpaqueRust,
    /// `Unknown` or `Never`: bind `Unknown` per name and stay silent.
    ///
    /// `Unknown` is recovery-only — checking already failed upstream, so a second diagnostic here is noise on top
    /// of the real one. `Never` is the bottom type, and [`TypeChecker::types_compatible`] already answers
    /// `(Never, _) => true` for every type including a tuple; accepting it is that established policy rather than
    /// a carve-out, and the code is unreachable either way.
    Recovery,
    /// Anything else, including a bare type variable, which must be reported.
    NotTuple,
}

/// Classify a value type for destructuring.
///
/// A tuple arrives in two spellings: a tuple *literal* infers [`ResolvedType::Tuple`], while a written `tuple[A, B]`
/// annotation resolves through the collection-type registry as a [`ResolvedType::Generic`] named `Tuple`. Both are
/// destructurable and both must be recognized here.
///
/// A bare type variable is deliberately [`TupleShape::NotTuple`]. It is not "not yet known" — it is known to be
/// underdetermined, and `T` can be instantiated as `int`. Incan's bounds are trait-based, so no caller can promise
/// a tuple shape. This does not affect `tuple[K, V]`, whose *elements* are type variables but whose shape is
/// known; that takes the [`TupleShape::Tuple`] arm above.
/// A shared tuple reference binds a shared reference to each component, preserving the native iterator's borrow.
/// A mutable tuple reference binds mutable component references under that same iterator borrow.
pub(in crate::typechecker) fn classify_tuple_shape(ty: &ResolvedType) -> TupleShape {
    match ty {
        ResolvedType::Tuple(types) => TupleShape::Tuple(types.clone()),
        ResolvedType::Ref(inner) => match classify_tuple_shape(inner) {
            TupleShape::Tuple(types) => {
                TupleShape::Tuple(types.into_iter().map(|ty| ResolvedType::Ref(Box::new(ty))).collect())
            }
            _ => TupleShape::NotTuple,
        },
        ResolvedType::RefMut(inner) => match classify_tuple_shape(inner) {
            TupleShape::Tuple(types) => {
                TupleShape::Tuple(types.into_iter().map(|ty| ResolvedType::RefMut(Box::new(ty))).collect())
            }
            _ => TupleShape::NotTuple,
        },
        ResolvedType::Generic(name, args)
            if matches!(collection_type_id(name.as_str()), Some(CollectionTypeId::Tuple)) =>
        {
            TupleShape::Tuple(args.clone())
        }
        ResolvedType::Unknown | ResolvedType::Never => TupleShape::Recovery,
        // A Rust type is destructurable only when its shape can actually be read. The parenthesized tuple
        // spelling gives a reliable arity — `std.json`'s `key, value = item` over a `rust::HashMap` item is
        // exactly that shape — and everything else is refused rather than assumed.
        ResolvedType::RustPath(path) => match rust_tuple_arity(path) {
            Some(arity) => TupleShape::RustTuple(arity),
            None => TupleShape::OpaqueRust,
        },
        _ => TupleShape::NotTuple,
    }
}

/// Return the fallback binary dunder used when compound assignment cannot resolve an explicit in-place hook.
fn compound_assignment_fallback_dunder(op: CompoundOp) -> &'static str {
    match op {
        CompoundOp::Add => "__add__",
        CompoundOp::Sub => "__sub__",
        CompoundOp::Mul => "__mul__",
        CompoundOp::Div => "__div__",
        CompoundOp::FloorDiv => "__floordiv__",
        CompoundOp::Mod => "__mod__",
        CompoundOp::MatMul => "__matmul__",
        CompoundOp::BitAnd => "__and__",
        CompoundOp::BitOr => "__or__",
        CompoundOp::BitXor => "__xor__",
        CompoundOp::Shl => "__lshift__",
        CompoundOp::Shr => "__rshift__",
    }
}
