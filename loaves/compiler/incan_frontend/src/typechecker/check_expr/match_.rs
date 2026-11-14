//! Check `match` expressions, patterns, and exhaustiveness.
//!
//! This module validates `match` expressions by type-checking each arm, binding pattern variables, and ensuring
//! exhaustiveness for enums, `Result`, and `Option`.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostics::errors;
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::*;
use incan_lang::interop::RustItemKind;
use incan_lang::lang::surface::constructors;
use incan_lang::lang::surface::constructors::ConstructorId;
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_lang::lang::types::numerics;

use super::TypeChecker;
use super::match_coverage::{coverage_row_head_is_wild, expand_coverage_heads};

mod enum_payloads;
mod match_checking;
mod patterns;
use crate::typechecker::check_stmt::{TupleShape, classify_tuple_shape};

#[derive(Clone)]
struct PatternBinding {
    ty: ResolvedType,
    span: Span,
}

/// Return a stable name list for binding-set comparison and diagnostics.
fn sorted_binding_names(bindings: &HashMap<String, PatternBinding>) -> Vec<String> {
    let mut names: Vec<_> = bindings.keys().cloned().collect();
    names.sort();
    names
}

/// Default payload binding mode after matching through explicit Rust references.
#[derive(Clone, Copy)]
pub(super) enum PatternBorrow {
    Shared,
    Mutable,
}

/// Peel reference layers for constructor lookup while preserving Rust match ergonomics for payload bindings. A shared
/// reference fixes shared binding mode even when another reference layer is mutable.
pub(super) fn borrowed_pattern_subject(mut subject: &ResolvedType) -> (&ResolvedType, Option<PatternBorrow>) {
    let mut borrow = None;
    loop {
        match subject {
            ResolvedType::Ref(inner) => {
                borrow = Some(PatternBorrow::Shared);
                subject = inner;
            }
            ResolvedType::RefMut(inner) => {
                if borrow.is_none() {
                    borrow = Some(PatternBorrow::Mutable);
                }
                subject = inner;
            }
            _ => return (subject, borrow),
        }
    }
}

/// The family of a match position's type that a literal pattern is compared against (#1741).
///
/// A literal spells one of a few scalar types, so a position of one of these families can say for certain whether a
/// literal can ever match there. Following the literal rules of the numeric reference, an integer literal is an `int`
/// and matches only an integer position, whose width then bounds its value; a float literal matches only a float
/// position; `None` matches only an `Option` position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiteralPatternFamily {
    Integer,
    Float,
    Str,
    Bool,
    Option,
}

impl LiteralPatternFamily {
    /// Return the family of a position type, or `None` when the type is of no literal family.
    fn of_position(ty: &ResolvedType) -> Option<Self> {
        match ty {
            ResolvedType::Int => Some(Self::Integer),
            ResolvedType::Float => Some(Self::Float),
            ResolvedType::Numeric(id) if numerics::is_integer(*id) => Some(Self::Integer),
            ResolvedType::Numeric(id) if numerics::is_binary_float(*id) => Some(Self::Float),
            ResolvedType::Numeric(_) | ResolvedType::Bool => Some(Self::Bool),
            ResolvedType::Str | ResolvedType::FrozenStr => Some(Self::Str),
            _ if ty.is_option() => Some(Self::Option),
            _ => None,
        }
    }

    /// Whether a position type of no literal family is known well enough to say that no literal matches a value of it:
    /// `bytes`, a tuple, a collection, a model, class, enum or newtype, a union, a decimal and a type parameter are. An
    /// unresolved type, a Rust type the checker does not see into and `Self` are not, and are left to the checks that
    /// own them.
    fn position_is_known(ty: &ResolvedType) -> bool {
        !matches!(
            ty,
            ResolvedType::Unknown
                | ResolvedType::CallSiteInfer
                | ResolvedType::Never
                | ResolvedType::RustPath(_)
                | ResolvedType::SelfType
                | ResolvedType::Ref(_)
                | ResolvedType::RefMut(_)
        )
    }

    /// Whether a literal of this spelling can match a value of this family.
    fn admits(self, literal: &Literal) -> bool {
        match literal {
            Literal::Int(_) => self == Self::Integer,
            Literal::Float(_) => self == Self::Float,
            Literal::String(_) => self == Self::Str,
            Literal::Bool(_) => self == Self::Bool,
            Literal::None => self == Self::Option,
            Literal::Decimal(_) | Literal::Bytes(_) => false,
        }
    }
}

/// Apply the inherited match binding mode to a payload before checking its nested pattern.
fn borrowed_pattern_payload(field: ResolvedType, borrow: Option<PatternBorrow>) -> ResolvedType {
    match borrow {
        Some(PatternBorrow::Shared) => ResolvedType::Ref(Box::new(field)),
        Some(PatternBorrow::Mutable) => ResolvedType::RefMut(Box::new(field)),
        None => field,
    }
}

impl TypeChecker {
    /// Refuse a literal pattern whose value can never match the position it is in (#1741).
    ///
    /// A decimal or bytes literal has no pattern form at all. Any other literal is compared with the position's type
    /// (the scrutinee, a tuple element, a variant payload or a field) after peeling the borrow wrappers match
    /// ergonomics add: a literal matches only a position of its own family (`LiteralPatternFamily`), so one in a known
    /// position of no family (a type parameter, a union, a nominal, a collection, a tuple) is refused too, and only an
    /// unresolved or Rust-only position type is left to the checks that own it. A numeric literal that has the
    /// position's family is then held to the position's width and range by the same rules a value literal of that type
    /// follows; a suffixed one is held to its suffix's range, and its suffix must name the position's exact type.
    fn check_literal_pattern(&mut self, literal: &Literal, expected_ty: &ResolvedType, span: Span) {
        let unmatchable = match literal {
            Literal::Decimal(_) => Some("decimal"),
            Literal::Bytes(_) => Some("bytes"),
            _ => None,
        };
        if let Some(kind) = unmatchable {
            self.errors.push(errors::pattern_literal_not_matchable(kind, span));
            return;
        }
        let (position_ty, _) = borrowed_pattern_subject(expected_ty);
        let family = LiteralPatternFamily::of_position(position_ty);
        if family.is_none() && !LiteralPatternFamily::position_is_known(position_ty) {
            return;
        }
        if !family.is_some_and(|family| family.admits(literal)) {
            let found = match literal {
                Literal::None => constructors::as_str(ConstructorId::None).to_string(),
                _ => self.check_literal(literal, span).to_string(),
            };
            self.errors.push(errors::pattern_literal_type_mismatch(
                &position_ty.to_string(),
                &found,
                span,
            ));
            return;
        }
        let position_ty = position_ty.clone();
        if matches!(literal, Literal::Int(value) if value.suffix.is_some())
            || matches!(literal, Literal::Float(value) if value.suffix.is_some())
        {
            // A suffix names the literal's type: its value is held to that type's range, and a pattern literal is
            // compared without conversion, so the position must be that exact type.
            let errors_before = self.errors.len();
            let literal_ty = self.check_literal(literal, span);
            if self.errors.len() == errors_before
                && super::super::numeric_type_id_for_compat(&literal_ty)
                    != super::super::numeric_type_id_for_compat(&position_ty)
            {
                self.errors.push(errors::pattern_literal_type_mismatch(
                    &position_ty.to_string(),
                    &literal_ty.to_string(),
                    span,
                ));
            }
            return;
        }
        let literal_expr = Spanned::new(Expr::Literal(literal.clone()), span);
        match literal {
            Literal::Int(_) => {
                self.check_int_literal_with_expected(&literal_expr, &position_ty);
            }
            Literal::Float(_) => {
                self.check_float_literal_with_expected(&literal_expr, &position_ty);
            }
            _ => {}
        }
    }
}

enum RustEnumPatternResolution {
    PayloadTypes {
        fields: Vec<ResolvedType>,
        names: Vec<String>,
    },
    QualifierMismatch,
}

impl RustEnumPatternResolution {
    /// Retain positional payload types without granting any named field identity.
    fn payloads(fields: Vec<ResolvedType>) -> Self {
        Self::PayloadTypes {
            fields,
            names: Vec::new(),
        }
    }
}

/// Whether a statement block cannot complete, because its last statement leaves it: a `return`, `break` or `continue`.
///
/// A `match` arm with such a body produces no value of its own, so it does not take part in unifying the arm types.
fn block_cannot_complete(stmts: &[Spanned<Statement>]) -> bool {
    stmts.last().is_some_and(|stmt| {
        matches!(
            stmt.node,
            Statement::Return(_) | Statement::Break(_) | Statement::Continue
        )
    })
}

impl TypeChecker {
    /// Whether a type is or holds an anonymous union anywhere inside it, as an `Option` payload, a collection element
    /// or a tuple item.
    fn type_holds_union(ty: &ResolvedType) -> bool {
        match ty {
            _ if ty.is_union() => true,
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => args.iter().any(Self::type_holds_union),
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => Self::type_holds_union(inner),
            ResolvedType::FrozenDict(key, value) => Self::type_holds_union(key) || Self::type_holds_union(value),
            _ => false,
        }
    }
}
