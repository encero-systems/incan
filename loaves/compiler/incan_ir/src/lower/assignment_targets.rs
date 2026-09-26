//! Lowering of the assignments with several targets: tuple unpacking into names (`a, b = value`), tuple assignment into
//! places (`grid.width, items[i] = value`) and chained assignment (`x = y = value`).
//!
//! The tuple statements read their whole right side into one temporary before writing any target, so a swap such as
//! `a, b = (b, a)` or `items[i], items[j] = (items[j], items[i])` sees the values from before the first write. Each
//! target then receives its element of the temporary. A chained assignment also reads its value into one temporary and
//! gives it to each target from left to right, Python's order. The temporary has the type the bound targets agree on,
//! or the value's own type, and each target converts it to its own type (`maybe = 5` wraps an `Option[int]` target), so
//! no target is written from another target.
//!
//! Every target is written the way a single assignment of its shape writes it:
//!
//! - a name that is already bound and may be reassigned is assigned in place. A fresh `let` would only shadow it until
//!   the end of the enclosing block, so `a, b = (b, a + b)` or `x = y = x + 1` in a loop would never update the loop's
//!   names;
//! - a new name is declared with the statement's binding kind;
//! - a field or an element place is written the way a single `obj.field = value` or `items[i] = value` writes it.
//!
//! The statement lowers to a block in statement position, whose statements the emitter spells inline so the names it
//! declares stay visible to the statements that follow.

use super::super::expr::{IrDictEntry, IrExprKind, IrListEntry, MethodCallArgPolicy, VarAccess, VarRefKind};
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

/// Name of the temporary a chained assignment reads its value into, reused the same way.
const CHAIN_VALUE_TEMPORARY: &str = "__incan_chain_value";

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

        let mut stmts = vec![self.bind_temporary(&temporary, value)];
        for (index, (name, element_ty)) in unpack.names.iter().zip(element_types).enumerate() {
            let element = self.tuple_temporary_element(&temporary, index, element_ty.clone());
            stmts.push(self.assign_or_declare_name(unpack.binding, name, element_ty, mutability, element));
        }
        Ok(Self::statement_block(stmts))
    }

    /// Lower `x = y = value` (or `let` / `mut x = y = value`).
    ///
    /// The value is read into one temporary, then each target takes it from left to right: a copy of it for every
    /// target but the last, the temporary itself for the last. The temporary has the type the already-bound targets
    /// agree on, as the checker decided it (so `a = b = None` over two `Option[int]` targets is an `Option[int]`, and
    /// `a = b = empty()` over a `list[int]` and a `list[i64]` is a `list[int]`), or the value's own type when they
    /// differ or none is bound, and each target converts it to its own type. A value the checker recorded as
    /// written per target is evaluated once per target instead; see [`Self::lower_chained_literal`]. A plain spelling
    /// reassigns every target that is already bound and mutable (or a module static) and declares the others, the
    /// rule a single `x = value` follows; `let` and `mut` spellings declare every target.
    pub(super) fn lower_chained_assignment(
        &mut self,
        chain: &ast::ChainedAssignmentStmt,
    ) -> Result<IrStmt, LoweringError> {
        let mutability = match chain.binding {
            ast::BindingKind::Mutable => Mutability::Mutable,
            _ => Mutability::Immutable,
        };
        let Some(last) = chain.targets.len().checked_sub(1) else {
            return Err(LoweringError {
                message: "empty chained assignment".to_string(),
                span: IrSpan::default(),
            });
        };

        let (written_per_target, targets_agree) = self.type_info.as_ref().map_or((false, false), |info| {
            (
                info.chained_value_is_written_per_target(chain.value.span),
                info.chained_targets_agree(chain.value.span),
            )
        });
        if written_per_target {
            return self.lower_chained_literal(chain, mutability);
        }
        let agreed_ty = if targets_agree {
            self.chain_first_bound_type(chain)
        } else {
            None
        };
        let value = self.lower_expr_spanned(&chain.value)?;
        let ty = agreed_ty.clone().unwrap_or_else(|| value.ty.clone());
        self.define_local_binding(CHAIN_VALUE_TEMPORARY.to_string(), ty.clone(), false);
        let mut stmts = vec![IrStmt::new(IrStmtKind::Let {
            name: CHAIN_VALUE_TEMPORARY.to_string(),
            ty: ty.clone(),
            type_annotation: agreed_ty,
            mutability: Mutability::Immutable,
            value,
        })];
        for (index, name) in chain.targets.iter().enumerate() {
            let place = self.bound_name_assign_target(chain.binding, name);
            let read = Self::chain_value_read(&ty, index == last, place.as_ref());
            stmts.push(match place {
                Some(target) => IrStmt::new(IrStmtKind::Assign { target, value: read }),
                None => self.declare_name(name, ty.clone(), mutability, read),
            });
        }
        Ok(Self::statement_block(stmts))
    }

    /// Lower a chain the checker recorded as written per target: its bound targets have incompatible types and its
    /// value is built only from literals and builtin empty constructors (`None`, `[]`, `{}`, `(None)`, `[None]`,
    /// `list()`, a number).
    ///
    /// There is no one type to read the value in, so each target gets its own evaluation of it, left to right, written
    /// as a single `target = value` writes it: `a = b = None` over an `Option[int]` and an `Option[str]` gives each its
    /// own `None`. Every evaluation of such a value gives an equal value and does nothing else; the checker records the
    /// fact only for such a value, so a call to a user function spelled `list` or `set` never comes here and is
    /// evaluated once. The checker checked the value once per target, and records one type per source span, so each
    /// evaluation takes its own target's type here.
    fn lower_chained_literal(
        &mut self,
        chain: &ast::ChainedAssignmentStmt,
        mutability: Mutability,
    ) -> Result<IrStmt, LoweringError> {
        let mut stmts = Vec::with_capacity(chain.targets.len());
        for name in &chain.targets {
            let mut value = self.lower_expr_spanned(&chain.value)?;
            let place = self.bound_name_assign_target(chain.binding, name);
            if place.is_some() {
                let target_ty = self.lookup_var(name);
                if target_ty != IrType::Unknown {
                    Self::retype_literal(&mut value, &target_ty);
                }
            }
            stmts.push(match place {
                Some(target) => IrStmt::new(IrStmtKind::Assign { target, value }),
                None => {
                    let ty = value.ty.clone();
                    self.declare_name(name, ty, mutability, value)
                }
            });
        }
        Ok(Self::statement_block(stmts))
    }

    /// Give one copy of a literal-built value its target's type, down through the elements of its literal containers.
    ///
    /// Only a node of the target's own container kind (a `None` for an `Option`, a list for a `list`, a dict, a set, a
    /// tuple of the same length) or of unknown type takes the target's type; a scalar keeps its own, so the conversion
    /// a single assignment applies (`5` into an `Option[int]` or an `int | str`) still applies.
    fn retype_literal(value: &mut TypedExpr, target: &IrType) {
        if value.ty != IrType::Unknown && !Self::same_container_kind(&value.ty, target) {
            return;
        }
        value.ty = target.clone();
        match (&mut value.kind, target) {
            (IrExprKind::List(entries), IrType::List(inner)) => {
                for entry in entries.iter_mut() {
                    if let IrListEntry::Element(item) = entry {
                        Self::retype_literal(item, inner);
                    }
                }
            }
            (IrExprKind::Set(items), IrType::Set(inner)) => {
                for item in items.iter_mut() {
                    Self::retype_literal(item, inner);
                }
            }
            (IrExprKind::Tuple(items), IrType::Tuple(types)) => {
                for (item, ty) in items.iter_mut().zip(types) {
                    Self::retype_literal(item, ty);
                }
            }
            (IrExprKind::Dict(entries), IrType::Dict(key_ty, value_ty)) => {
                for entry in entries.iter_mut() {
                    if let IrDictEntry::Pair(key, item) = entry {
                        Self::retype_literal(key, key_ty);
                        Self::retype_literal(item, value_ty);
                    }
                }
            }
            _ => {}
        }
    }

    /// Return whether two types are the same kind of container: both `Option`, `list`, `set`, `dict`, or tuples of one
    /// length.
    fn same_container_kind(left: &IrType, right: &IrType) -> bool {
        match (left, right) {
            (IrType::Option(_), IrType::Option(_))
            | (IrType::List(_), IrType::List(_))
            | (IrType::Set(_), IrType::Set(_))
            | (IrType::Dict(_, _), IrType::Dict(_, _)) => true,
            (IrType::Tuple(left_items), IrType::Tuple(right_items)) => left_items.len() == right_items.len(),
            _ => false,
        }
    }

    /// Return the type of a chain's first already-bound target whose type is known, the type the checker checked the
    /// value against when the bound targets agree. New names take whatever the value's type is, so they are skipped.
    fn chain_first_bound_type(&self, chain: &ast::ChainedAssignmentStmt) -> Option<IrType> {
        chain
            .targets
            .iter()
            .filter(|name| self.bound_name_assign_target(chain.binding, name).is_some())
            .map(|name| self.lookup_var(name))
            .find(|ty| *ty != IrType::Unknown)
    }

    /// Read the chain's value temporary for one target.
    ///
    /// A `Copy` value is copied to every target. Otherwise every target but the last reads the temporary without taking
    /// it, and the last takes it. A module static before the last gets an explicit `.clone()`: a static's assignment
    /// takes the value it is given whatever the read says, and the temporary is still needed by the targets after it.
    fn chain_value_read(ty: &IrType, is_last: bool, place: Option<&AssignTarget>) -> TypedExpr {
        let access = if ty.is_copy() {
            VarAccess::Copy
        } else if is_last {
            VarAccess::Move
        } else {
            VarAccess::Read
        };
        let read = TypedExpr::new(
            IrExprKind::Var {
                name: CHAIN_VALUE_TEMPORARY.to_string(),
                access,
                ref_kind: VarRefKind::Value,
            },
            ty.clone(),
        );
        let static_place = matches!(
            place,
            Some(AssignTarget::Static { .. } | AssignTarget::StaticBinding(_))
        );
        if access != VarAccess::Read || !static_place {
            return read;
        }
        TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(read),
                method: "clone".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            ty.clone(),
        )
    }

    /// Give `value` to one named target: assign it when the statement reassigns the name, otherwise declare the name.
    fn assign_or_declare_name(
        &mut self,
        binding: ast::BindingKind,
        name: &str,
        ty: IrType,
        mutability: Mutability,
        value: TypedExpr,
    ) -> IrStmt {
        if let Some(target) = self.bound_name_assign_target(binding, name) {
            return IrStmt::new(IrStmtKind::Assign { target, value });
        }
        self.declare_name(name, ty, mutability, value)
    }

    /// Declare `name` with `value` in the current scope, recording a `mut` declaration as mutable.
    fn declare_name(&mut self, name: &str, ty: IrType, mutability: Mutability, value: TypedExpr) -> IrStmt {
        self.define_local_binding(name.to_string(), ty.clone(), false);
        if matches!(mutability, Mutability::Mutable) {
            self.mutable_vars.insert(name.to_string(), true);
        }
        IrStmt::new(IrStmtKind::Let {
            name: name.to_string(),
            ty,
            type_annotation: None,
            mutability,
            value,
        })
    }

    /// Lower `target, target = value` whose targets are places: fields, elements, or names mixed with them.
    ///
    /// Each target is written like the single-target statement of the same shape: a field like `obj.field = value`, an
    /// element like `items[i] = value`, a name like `name = value`. A target's own sub-expressions (the object, the
    /// index) are evaluated when it is written, after the right side, in source order.
    pub(super) fn lower_tuple_assign(&mut self, assign: &ast::TupleAssignStmt) -> Result<IrStmt, LoweringError> {
        let value = self.lower_expr_spanned(&assign.value)?;
        let element_types = Self::tuple_element_types(&value.ty, assign.targets.len());

        let mut stmts = vec![self.bind_temporary(TUPLE_ASSIGN_TEMPORARY, value)];
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

    /// Return where a plain named target is reassigned, or `None` when the statement declares it.
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
    /// The checker accepts only names, fields and elements as targets and refuses any other expression, a parenthesized
    /// target included, so reaching one here is a lowering error rather than a silent skip.
    fn tuple_assign_place(&mut self, target: &Spanned<ast::Expr>) -> Result<AssignTarget, LoweringError> {
        match &target.node {
            ast::Expr::Ident(name) => Ok(self.resolve_named_assign_target(name)),
            ast::Expr::Field(object, field) => self.field_assign_target(object, field, target.span),
            ast::Expr::Index(object, index) => Ok(AssignTarget::Index {
                object: Box::new(self.lower_expr_spanned(object)?),
                index: Box::new(self.lower_expr_spanned(index)?),
            }),
            _ => Err(LoweringError {
                message: "a tuple assignment target must be a name, a field or an element".to_string(),
                span: IrSpan::default(),
            }),
        }
    }

    /// Declare the temporary that holds a statement's right side and return its `let`.
    fn bind_temporary(&mut self, temporary: &str, value: TypedExpr) -> IrStmt {
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
