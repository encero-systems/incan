//! Lowering of the two tuple statements: tuple unpacking into names (`a, b = value`) and tuple assignment into places
//! (`grid.width, items[i] = value`).
//!
//! Both read the whole right side into one temporary before writing any target, so a swap such as `a, b = (b, a)` or
//! `items[i], items[j] = (items[j], items[i])` sees the values from before the first write. Each target then receives
//! its element of the temporary:
//!
//! - a name that is already bound and may be reassigned is assigned in place. A fresh `let` would only shadow it until
//!   the end of the enclosing block, so `a, b = (b, a + b)` in a loop would never update the loop's `a` and `b`;
//! - a new name is declared with the statement's binding kind;
//! - a field or an element place is written the way a single `obj.field = value` or `items[i] = value` writes it.
//!
//! The statement lowers to a block in statement position, whose statements the emitter spells inline so the names it
//! declares stay visible to the statements that follow.

use super::super::expr::{IrExprKind, VarAccess, VarRefKind};
use super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::types::IrType;
use super::super::{IrSpan, Mutability, TypedExpr};
use super::AstLowering;
use super::errors::LoweringError;
use incan_frontend::ast::{self, Spanned};

/// Name of the temporary a tuple assignment reads its right side into.
///
/// Consecutive tuple assignments reuse the name; each temporary shadows the previous one, which is no longer read.
const TUPLE_ASSIGN_TEMPORARY: &str = "__incan_tuple_assign";

impl AstLowering {
    /// Lower `a, b = value` (or `let` / `mut a, b = value`).
    ///
    /// A plain spelling reassigns every name that is already bound and mutable (or a module static), exactly as a
    /// single `a = value` does, and declares the others; `let` and `mut` always declare. The checker has already
    /// refused a reassignment of an immutable binding, so a bound name that is not mutable here is a shadowing
    /// declaration.
    pub(super) fn lower_tuple_unpack(&mut self, unpack: &ast::TupleUnpackStmt) -> Result<IrStmt, LoweringError> {
        let value = self.lower_expr_spanned(&unpack.value)?;
        let temporary = format!("__incan_tuple_unpack_{}", unpack.names.join("_"));
        let element_types = Self::tuple_element_types(&value.ty, unpack.names.len());
        let mutability = match unpack.binding {
            ast::BindingKind::Mutable => Mutability::Mutable,
            _ => Mutability::Immutable,
        };

        let mut stmts = vec![self.bind_tuple_temporary(&temporary, value)];
        for (index, (name, element_ty)) in unpack.names.iter().zip(element_types).enumerate() {
            let element = self.tuple_temporary_element(&temporary, index, element_ty.clone());
            if let Some(target) = self.bound_name_assign_target(unpack.binding, name) {
                stmts.push(IrStmt::new(IrStmtKind::Assign { target, value: element }));
                continue;
            }

            self.define_local_binding(name.clone(), element_ty.clone(), false);
            if matches!(mutability, Mutability::Mutable) {
                self.mutable_vars.insert(name.clone(), true);
            }
            stmts.push(IrStmt::new(IrStmtKind::Let {
                name: name.clone(),
                ty: element_ty,
                type_annotation: None,
                mutability,
                value: element,
            }));
        }
        Ok(Self::statement_block(stmts))
    }

    /// Lower `target, target = value` whose targets are places: fields, elements, or names mixed with them.
    ///
    /// Each target is written like the single-target statement of the same shape: a field like `obj.field = value`, an
    /// element like `items[i] = value`, a name like `name = value`. A target's own sub-expressions (the object, the
    /// index) are evaluated when it is written, after the right side, in source order.
    pub(super) fn lower_tuple_assign(&mut self, assign: &ast::TupleAssignStmt) -> Result<IrStmt, LoweringError> {
        let value = self.lower_expr_spanned(&assign.value)?;
        let element_types = Self::tuple_element_types(&value.ty, assign.targets.len());

        let mut stmts = vec![self.bind_tuple_temporary(TUPLE_ASSIGN_TEMPORARY, value)];
        for (index, (target, element_ty)) in assign.targets.iter().zip(element_types).enumerate() {
            let place = self.tuple_assign_place(target)?;
            let element = self.tuple_temporary_element(TUPLE_ASSIGN_TEMPORARY, index, element_ty);
            stmts.push(IrStmt::new(IrStmtKind::Assign {
                target: place,
                value: element,
            }));
        }
        Ok(Self::statement_block(stmts))
    }

    /// Return where a plain tuple-unpacking name is reassigned, or `None` when the statement declares it.
    ///
    /// This is the rule a single `name = value` follows: a name bound in any enclosing scope is reassigned when it is
    /// mutable or resolves to a module static; `let` and `mut` spellings always declare.
    fn bound_name_assign_target(&self, binding: ast::BindingKind, name: &str) -> Option<AssignTarget> {
        match binding {
            ast::BindingKind::Let | ast::BindingKind::Mutable => None,
            ast::BindingKind::Reassign => Some(self.resolve_named_assign_target(name)),
            ast::BindingKind::Inferred => {
                if !self.scopes.iter().rev().any(|scope| scope.contains_key(name)) {
                    return None;
                }
                let target = self.resolve_named_assign_target(name);
                let reassignable = matches!(target, AssignTarget::Static { .. })
                    || self.mutable_vars.get(name).copied().unwrap_or(false);
                reassignable.then_some(target)
            }
        }
    }

    /// Lower one tuple-assignment target to the place a single assignment of the same shape writes.
    ///
    /// The checker accepts only names, fields and elements as targets (parentheses around one are transparent); any
    /// other expression is refused there, so reaching it here is a lowering error rather than a silent skip.
    fn tuple_assign_place(&mut self, target: &Spanned<ast::Expr>) -> Result<AssignTarget, LoweringError> {
        match &target.node {
            ast::Expr::Ident(name) => Ok(self.resolve_named_assign_target(name)),
            ast::Expr::Field(object, field) => self.field_assign_target(object, field, target.span),
            ast::Expr::Index(object, index) => Ok(AssignTarget::Index {
                object: Box::new(self.lower_expr_spanned(object)?),
                index: Box::new(self.lower_expr_spanned(index)?),
            }),
            ast::Expr::Paren(inner) => self.tuple_assign_place(inner),
            _ => Err(LoweringError {
                message: "a tuple assignment target must be a name, a field or an element".to_string(),
                span: IrSpan::default(),
            }),
        }
    }

    /// Declare the temporary that holds a tuple statement's right side and return its `let`.
    fn bind_tuple_temporary(&mut self, temporary: &str, value: TypedExpr) -> IrStmt {
        let ty = value.ty.clone();
        self.define_local_binding(temporary.to_string(), ty.clone(), false);
        IrStmt::new(IrStmtKind::Let {
            name: temporary.to_string(),
            ty,
            type_annotation: None,
            mutability: Mutability::Immutable,
            value,
        })
    }

    /// Read element `index` of a tuple temporary, moving it out: every element is read exactly once.
    fn tuple_temporary_element(&self, temporary: &str, index: usize, ty: IrType) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Field {
                object: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: temporary.to_string(),
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::Value,
                    },
                    self.lookup_var(temporary),
                )),
                field: index.to_string(),
            },
            ty,
        )
    }

    /// Return the element types of a tuple value with `arity` elements, `Unknown` for any the value's type does not
    /// spell (an unresolved type still reads its elements by position).
    fn tuple_element_types(value_ty: &IrType, arity: usize) -> Vec<IrType> {
        let known: &[IrType] = match value_ty {
            IrType::Tuple(items) => items.as_slice(),
            _ => &[],
        };
        (0..arity)
            .map(|index| known.get(index).cloned().unwrap_or(IrType::Unknown))
            .collect()
    }

    /// Wrap the statements of a tuple statement in the statement-position block the emitter spells inline.
    fn statement_block(stmts: Vec<IrStmt>) -> IrStmt {
        IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
            IrExprKind::Block { stmts, value: None },
            IrType::Unit,
        )))
    }
}
