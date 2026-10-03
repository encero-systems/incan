//! `match`, `if let` and `while let` forms matched in place (#1561).
//!
//! When a pattern binds a name from a writable place (a caller-visible `mut` parameter, a `mut` binding, `self` in a
//! `mut self` method, a field or list element of one, or a view into one) and an arm changes a value through that
//! name, the change must reach the place. The checker records such a scrutinee
//! ([`TypeCheckInfo::match_scrutinee_is_changed_in_place`]), and refuses the change when the place is read-only.
//! Lowering then matches it through a mutable reference, so each pattern name binds the part of the scrutinee it names
//! rather than a copy:
//!
//! - a scrutinee that is already a reference (a `mut` parameter, or a name an enclosing in-place arm bound) is matched
//!   as it is; any other scrutinee is borrowed mutably, and the binding at its root is marked as changed, so a `for`
//!   loop that owns it iterates its items in place and a local that holds it is declared mutable;
//! - each name the pattern binds is bound under a compiler name, and the arm binds the source name before its body
//!   runs: to the reference itself for a value that cannot be copied, and to a copy of the value for one that can, such
//!   as an `int` beside a changed list; a guard, which sees pattern bindings only through shared references, binds it
//!   to a shared reborrow. The pattern therefore never binds a source name directly, so no pattern binding is declared
//!   mutable where the reference already carries the change.
//!
//! A scrutinee whose type is a union keeps the ordinary match: its arms bind narrowed union members, not parts of the
//! parameter.
//!
//! [`TypeCheckInfo::match_scrutinee_is_changed_in_place`]:
//!     incan_frontend::typechecker::TypeCheckInfo::match_scrutinee_is_changed_in_place

use super::super::super::TypedExpr;
use super::super::super::expr::{IrExprKind, MatchArm, MatchArmBinding, Pattern, UnaryOp, VarAccess, VarRefKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_frontend::ast::{self, Span, Spanned};

/// The prefix of the compiler name a pattern of an in-place match binds in place of a source name.
const IN_PLACE_BINDING_PREFIX: &str = "__incan_in_place_";

/// The names the pattern of one in-place arm binds, with the type the checker gave each.
pub(in crate::lower) struct InPlaceArmBindings {
    bindings: Vec<(String, IrType)>,
}

impl AstLowering {
    /// Return whether the scrutinee at `span`, lowered as `scrutinee`, is matched in place.
    ///
    /// The checker's fact is keyed by span in the module being checked, so a trait default expanded from another
    /// module does not consult it, and a union-typed scrutinee keeps the ordinary match.
    pub(in crate::lower) fn match_is_in_place(&self, span: Span, scrutinee: &TypedExpr) -> bool {
        if self.active_imported_trait_defaults.last().copied().unwrap_or(false) {
            return false;
        }
        let referent = match &scrutinee.ty {
            IrType::Ref(inner) | IrType::RefMut(inner) => inner.as_ref(),
            other => other,
        };
        !referent.is_union()
            && self
                .type_info
                .as_ref()
                .is_some_and(|info| info.match_scrutinee_is_changed_in_place(span))
    }

    /// Return the scrutinee of an in-place match as a mutable reference to the place it names.
    ///
    /// A scrutinee that is already a reference is returned as it is. Any other one is borrowed mutably, and the
    /// value binding at its root, when that binding is not itself a reference, is marked as borrowed mutably, which is
    /// how every later question about whether a body changes a binding sees the change.
    pub(in crate::lower) fn in_place_scrutinee(mut scrutinee: TypedExpr) -> TypedExpr {
        if matches!(scrutinee.ty, IrType::Ref(_) | IrType::RefMut(_)) {
            return scrutinee;
        }
        mark_place_root_changed(&mut scrutinee);
        let ty = IrType::RefMut(Box::new(scrutinee.ty.clone()));
        let span = scrutinee.span;
        TypedExpr::new(
            IrExprKind::UnaryOp {
                op: UnaryOp::RefMut,
                operand: Box::new(scrutinee),
            },
            ty,
        )
        .with_span(span)
    }

    /// Collect the names an in-place arm's `pattern` binds, with the types the checker recorded for them.
    ///
    /// A name bound in several alternatives of an or-pattern is collected once. A name whose type the checker did not
    /// record keeps an unknown type.
    pub(in crate::lower) fn in_place_arm_bindings(&self, pattern: &Spanned<ast::Pattern>) -> InPlaceArmBindings {
        let mut bindings: Vec<(String, IrType)> = Vec::new();
        let mut pending = vec![pattern];
        while let Some(pattern) = pending.pop() {
            match &pattern.node {
                ast::Pattern::Binding(name) => {
                    if bindings.iter().any(|(bound, _)| bound == name) {
                        continue;
                    }
                    let ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(pattern.span))
                        .map_or(IrType::Unknown, |ty| self.lower_resolved_type(ty));
                    bindings.push((name.clone(), ty));
                }
                ast::Pattern::Tuple(items) | ast::Pattern::Or(items) => pending.extend(items.iter().rev()),
                ast::Pattern::Constructor(_, args) => {
                    pending.extend(args.iter().rev().map(|arg| match arg {
                        ast::PatternArg::Positional(pattern) | ast::PatternArg::Named(_, pattern) => pattern,
                    }));
                }
                ast::Pattern::Group(inner) => pending.push(inner),
                ast::Pattern::Wildcard | ast::Pattern::Literal(_) => {}
            }
        }
        InPlaceArmBindings { bindings }
    }

    /// Define the source names of an in-place arm in the current scope, as the arm's body sees them: a value that
    /// cannot be copied as a mutable reference to the part of the scrutinee it names, and a copyable value as a copy.
    pub(in crate::lower) fn define_in_place_arm_bindings(&mut self, bindings: &InPlaceArmBindings) {
        for (name, ty) in &bindings.bindings {
            self.define_local_binding(name.clone(), source_binding_type(ty), false);
        }
    }
}

impl InPlaceArmBindings {
    /// Rename every source name `pattern` binds to its compiler name, and give each arm built from it the bindings
    /// that bind the source names before the body runs.
    pub(in crate::lower) fn apply(&self, arms: &mut [MatchArm]) {
        for arm in arms {
            rename_bindings(&mut arm.pattern);
            arm.bindings.extend(self.bindings.iter().map(|(name, ty)| {
                let reference_ty = IrType::RefMut(Box::new(ty.clone()));
                let bound = TypedExpr::new(
                    IrExprKind::Var {
                        name: in_place_binding_name(name),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    reference_ty,
                );
                let value = if ty.is_copy() {
                    TypedExpr::new(
                        IrExprKind::UnaryOp {
                            op: UnaryOp::Deref,
                            operand: Box::new(bound),
                        },
                        ty.clone(),
                    )
                } else {
                    bound
                };
                // A guard sees the pattern's bindings only through shared references, so there the source name of a
                // value that cannot be copied is a shared reborrow; its Rust type is left to inference, since the body
                // and the guard bind it at different reference types.
                let (binding_ty, guard_value) = if ty.is_copy() {
                    (ty.clone(), None)
                } else {
                    let shared = TypedExpr::new(
                        IrExprKind::UnaryOp {
                            op: UnaryOp::Ref,
                            operand: Box::new(TypedExpr::new(
                                IrExprKind::UnaryOp {
                                    op: UnaryOp::Deref,
                                    operand: Box::new(value.clone()),
                                },
                                ty.clone(),
                            )),
                        },
                        IrType::Ref(Box::new(ty.clone())),
                    );
                    (IrType::Unknown, Some(shared))
                };
                MatchArmBinding {
                    name: name.clone(),
                    ty: binding_ty,
                    value,
                    guard_value,
                }
            }));
        }
    }
}

/// Return the type the arm's body sees a source name at: the value itself for a copyable one, a mutable reference to
/// it otherwise.
fn source_binding_type(ty: &IrType) -> IrType {
    if ty.is_copy() {
        ty.clone()
    } else {
        IrType::RefMut(Box::new(ty.clone()))
    }
}

/// Return the compiler name a pattern binds in place of the source name `name`.
fn in_place_binding_name(name: &str) -> String {
    format!("{IN_PLACE_BINDING_PREFIX}{name}")
}

/// Rename every name a lowered pattern binds to its compiler name.
fn rename_bindings(pattern: &mut Pattern) {
    match pattern {
        Pattern::Var(name) => *name = in_place_binding_name(name),
        Pattern::Tuple(items) | Pattern::Or(items) | Pattern::Enum { fields: items, .. } => {
            items.iter_mut().for_each(rename_bindings);
        }
        Pattern::Struct { fields, .. } => fields.iter_mut().for_each(|(_, pattern)| rename_bindings(pattern)),
        Pattern::Wildcard | Pattern::Literal(_) => {}
    }
}

/// Mark the value binding at the root of a place (`row`, `holder.inner`, `rows[0]`) as borrowed mutably, unless that
/// binding is itself a reference, which the place is reached through without borrowing the binding.
fn mark_place_root_changed(place: &mut TypedExpr) {
    match &mut place.kind {
        IrExprKind::Var {
            access,
            ref_kind: VarRefKind::Value,
            ..
        } if !matches!(place.ty, IrType::Ref(_) | IrType::RefMut(_)) => *access = VarAccess::BorrowMut,
        IrExprKind::Field { object, .. } | IrExprKind::Index { object, .. } => mark_place_root_changed(object),
        _ => {}
    }
}
