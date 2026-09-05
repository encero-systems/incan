//! Statement checking: assignments, returns, control flow.

use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved};
use incan_core::lang::errors as runtime_errors;
use incan_core::lang::keywords;
use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::{NumericTy, result_numeric_type};
use incan_semantics_core::SurfaceStmtTypeCheck;
use incan_semantics_core::rust_tuple_arity;

use super::{CAbiSpanLocal, CBindingType, COutputMode, LoopContextKind, TypeChecker};
use crate::frontend::typechecker::helpers::{collection_type_id, ensure_bool_condition, option_ty};

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
/// question inline and fall back to `vec![Unknown; n]` for anything it did not recognise, which sized the element
/// list to the name list and so made the arity guard below it unreachable for a non-tuple (#1125, #1132).
#[derive(Debug)]
pub(in crate::frontend::typechecker) enum TupleShape {
    /// A tuple in either Incan spelling, carrying its element types so arity can be checked against them.
    Tuple(Vec<ResolvedType>),
    /// A Rust-interop tuple whose arity the compiler can actually read, such as the `(String, JsonValue)` a
    /// `rust::HashMap::items()` yields.
    ///
    /// Element types are not modelled, so it carries only the count and binds `Unknown` per name. Arity is still
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
/// A tuple arrives in two spellings: a tuple *literal* infers [`ResolvedType::Tuple`], while a written
/// `tuple[A, B]` annotation resolves through the collection-type registry as a [`ResolvedType::Generic`] named
/// `Tuple`. Both are destructurable and both must be recognised here.
///
/// A bare type variable is deliberately [`TupleShape::NotTuple`]. It is not "not yet known" — it is known to be
/// underdetermined, and `T` can be instantiated as `int`. Incan's bounds are trait-based, so no caller can promise
/// a tuple shape. This does not affect `tuple[K, V]`, whose *elements* are type variables but whose shape is
/// known; that takes the [`TupleShape::Tuple`] arm above.
pub(in crate::frontend::typechecker) fn classify_tuple_shape(ty: &ResolvedType) -> TupleShape {
    match ty {
        ResolvedType::Tuple(types) => TupleShape::Tuple(types.clone()),
        ResolvedType::Generic(name, args)
            if matches!(collection_type_id(name.as_str()), Some(CollectionTypeId::Tuple)) =>
        {
            TupleShape::Tuple(args.clone())
        }
        ResolvedType::Unknown | ResolvedType::Never => TupleShape::Recovery,
        // A Rust type is destructurable only when its shape can actually be read. The parenthesised tuple
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

impl TypeChecker {
    // ========================================================================
    // Statements
    // ========================================================================

    /// Return whether a local annotation names a trait surface that does not yet have a local value representation.
    ///
    /// Callable parameters and returns have dedicated trait-bound lowering paths, but local bindings do not preserve a
    /// hidden concrete adopter yet. Rejecting this shape in the typechecker prevents accepted Incan from reaching Rust
    /// codegen as a bare trait local type.
    fn is_trait_typed_local_annotation(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                self.lookup_semantic_trait_info(name).is_some()
            }
            _ => false,
        }
    }

    /// Validate a statement and its subexpressions.
    ///
    /// Handles assignments (including mutability checks), control flow (`if`, `while`, `for`),
    /// returns, and expression statements. Delegates expression validation to
    /// [`check_expr`](Self::check_expr).
    pub(crate) fn check_statement(&mut self, stmt: &Spanned<Statement>) {
        match &stmt.node {
            Statement::Assignment(assign) => self.check_assignment(assign, stmt.span),
            Statement::FieldAssignment(field_assign) => self.check_field_assignment(field_assign, stmt.span),
            Statement::IndexAssignment(index_assign) => self.check_index_assignment(index_assign, stmt.span),
            Statement::Return(expr) => self.check_return(expr.as_ref(), stmt.span),
            Statement::If(if_stmt) => self.check_if_stmt(if_stmt),
            Statement::Loop(loop_stmt) => self.check_loop_stmt(loop_stmt),
            Statement::While(while_stmt) => self.check_while_stmt(while_stmt),
            Statement::For(for_stmt) => self.check_for_stmt(for_stmt),
            Statement::Unsafe(unsafe_stmt) => self.check_unsafe_stmt(unsafe_stmt),
            Statement::VocabBlock(vocab_block) => {
                self.errors.push(crate::frontend::diagnostics::CompileError::new(
                    format!(
                        "raw vocab block `{}` reached typechecker before desugaring",
                        vocab_block.keyword
                    ),
                    stmt.span,
                ));
            }
            Statement::Assert(assert_stmt) => self.check_assert_stmt(assert_stmt),
            Statement::Surface(surface_stmt) => self.check_surface_stmt(surface_stmt, stmt.span),
            Statement::Expr(expr) => {
                self.check_expr(expr);
            }
            Statement::VocabExpressionItem(_item) => {
                self.errors.push(crate::frontend::diagnostics::CompileError::new(
                    "raw vocab expression-list item reached typechecker before desugaring".to_string(),
                    stmt.span,
                ));
            }
            Statement::Pass => {}
            Statement::Break(value) => self.check_break_stmt(value.as_ref(), stmt.span),
            Statement::Continue => self.check_continue_stmt(stmt.span),
            Statement::CompoundAssignment(compound) => {
                self.record_write_target_identity(compound.name_span, &compound.name);
                // Check that the variable exists and is mutable (search all scopes)
                let var_info_opt = self
                    .symbols
                    .lookup(&compound.name)
                    .and_then(|id| self.symbols.get(id))
                    .and_then(|sym| {
                        if let SymbolKind::Variable(var_info) = &sym.kind {
                            Some((var_info.is_mutable, var_info.ty.clone()))
                        } else {
                            None
                        }
                    });

                if let Some((is_mutable, var_ty)) = var_info_opt {
                    if !is_mutable {
                        self.errors
                            .push(errors::mutation_without_mut(&compound.name, stmt.span));
                    }
                    // Type check the value expression
                    let value_ty = self.check_expr(&compound.value);

                    // Treat `x <op>= y` as `x = x <op> y` using numeric policy.
                    let binop = compound.op.binary_op();

                    let lhs_num = numeric_ty_from_resolved(&var_ty);
                    let rhs_num = numeric_ty_from_resolved(&value_ty);

                    if let (Some(lhs), Some(rhs)) = (lhs_num, rhs_num) {
                        if let Some(num_op) = numeric_op_from_ast(&binop) {
                            let res_num = result_numeric_type(num_op, lhs, rhs, None);
                            let res_ty = match res_num {
                                NumericTy::Int => ResolvedType::Int,
                                NumericTy::Float => ResolvedType::Float,
                            };
                            if !self.types_compatible(&res_ty, &var_ty) {
                                self.errors.push(errors::type_mismatch(
                                    &var_ty.to_string(),
                                    &res_ty.to_string(),
                                    compound.value.span,
                                ));
                            }
                        } else if matches!(
                            binop,
                            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr
                        ) && matches!((lhs, rhs), (NumericTy::Int, NumericTy::Int))
                        {
                            if !self.types_compatible(&ResolvedType::Int, &var_ty) {
                                self.errors.push(errors::type_mismatch(
                                    &var_ty.to_string(),
                                    &ResolvedType::Int.to_string(),
                                    compound.value.span,
                                ));
                            }
                        } else {
                            self.errors.push(errors::type_mismatch(
                                "supported compound operator operands",
                                &format!("{} {} {}", var_ty, binop, value_ty),
                                compound.value.span,
                            ));
                        }
                    } else if self.is_user_operator_receiver(&var_ty) {
                        if let Some(res_ty) = self.resolve_compound_assignment_operator(
                            &var_ty,
                            compound.op,
                            &compound.value,
                            &value_ty,
                            stmt.span,
                        ) {
                            if !self.types_compatible(&res_ty, &var_ty) {
                                self.errors.push(errors::type_mismatch(
                                    &var_ty.to_string(),
                                    &res_ty.to_string(),
                                    compound.value.span,
                                ));
                            }
                        } else {
                            self.errors.push(errors::missing_method(
                                &var_ty.to_string(),
                                compound_assignment_fallback_dunder(compound.op),
                                stmt.span,
                            ));
                        }
                    } else if !self.types_compatible(&value_ty, &var_ty) {
                        // Non-numeric: fall back to simple compatibility check.
                        self.errors.push(errors::type_mismatch(
                            &var_ty.to_string(),
                            &value_ty.to_string(),
                            compound.value.span,
                        ));
                    }
                } else if let Some(static_info) = self.lookup_static_info(&compound.name).cloned() {
                    if static_info.is_imported {
                        self.errors.push(errors::imported_static_reassignment_not_allowed(
                            &compound.name,
                            stmt.span,
                        ));
                        return;
                    }
                    let value_ty = self.check_expr(&compound.value);
                    let var_ty = static_info.ty;

                    let binop = match compound.op {
                        CompoundOp::Add => BinaryOp::Add,
                        CompoundOp::Sub => BinaryOp::Sub,
                        CompoundOp::Mul => BinaryOp::Mul,
                        CompoundOp::Div => BinaryOp::Div,
                        CompoundOp::FloorDiv => BinaryOp::FloorDiv,
                        CompoundOp::Mod => BinaryOp::Mod,
                        CompoundOp::MatMul => BinaryOp::MatMul,
                        CompoundOp::BitAnd => BinaryOp::BitAnd,
                        CompoundOp::BitOr => BinaryOp::BitOr,
                        CompoundOp::BitXor => BinaryOp::BitXor,
                        CompoundOp::Shl => BinaryOp::Shl,
                        CompoundOp::Shr => BinaryOp::Shr,
                    };

                    let lhs_num = numeric_ty_from_resolved(&var_ty);
                    let rhs_num = numeric_ty_from_resolved(&value_ty);

                    if let (Some(lhs), Some(rhs)) = (lhs_num, rhs_num) {
                        if let Some(num_op) = numeric_op_from_ast(&binop) {
                            let res_num = result_numeric_type(num_op, lhs, rhs, None);
                            let res_ty = match res_num {
                                NumericTy::Int => ResolvedType::Int,
                                NumericTy::Float => ResolvedType::Float,
                            };
                            if !self.types_compatible(&res_ty, &var_ty) {
                                self.errors.push(errors::type_mismatch(
                                    &var_ty.to_string(),
                                    &res_ty.to_string(),
                                    compound.value.span,
                                ));
                            }
                        } else if matches!(
                            binop,
                            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr
                        ) && matches!((lhs, rhs), (NumericTy::Int, NumericTy::Int))
                        {
                            if !self.types_compatible(&ResolvedType::Int, &var_ty) {
                                self.errors.push(errors::type_mismatch(
                                    &var_ty.to_string(),
                                    &ResolvedType::Int.to_string(),
                                    compound.value.span,
                                ));
                            }
                        } else {
                            self.errors.push(errors::type_mismatch(
                                "supported compound operator operands",
                                &format!("{} {} {}", var_ty, binop, value_ty),
                                compound.value.span,
                            ));
                        }
                    } else if self.is_user_operator_receiver(&var_ty) {
                        if let Some(res_ty) = self.resolve_compound_assignment_operator(
                            &var_ty,
                            compound.op,
                            &compound.value,
                            &value_ty,
                            stmt.span,
                        ) {
                            if !self.types_compatible(&res_ty, &var_ty) {
                                self.errors.push(errors::type_mismatch(
                                    &var_ty.to_string(),
                                    &res_ty.to_string(),
                                    compound.value.span,
                                ));
                            }
                        } else {
                            self.errors.push(errors::missing_method(
                                &var_ty.to_string(),
                                compound_assignment_fallback_dunder(compound.op),
                                stmt.span,
                            ));
                        }
                    } else if !self.types_compatible(&value_ty, &var_ty) {
                        self.errors.push(errors::type_mismatch(
                            &var_ty.to_string(),
                            &value_ty.to_string(),
                            compound.value.span,
                        ));
                    }
                } else if self.const_decls.contains_key(&compound.name) {
                    self.errors
                        .push(errors::const_reassignment_suggests_static(&compound.name, stmt.span));
                } else {
                    self.errors.push(errors::unknown_symbol(&compound.name, stmt.span));
                }
            }
            Statement::TupleUnpack(unpack) => {
                // Check the value expression and get its type
                let value_ty = self.check_expr(&unpack.value);

                let element_types = self.destructured_element_types(&value_ty, unpack.names.len(), stmt.span);

                // A tuple-unpack source assignment follows the same lexical rule as `x = value`: a plain spelling
                // reassigns the nearest active binding, while `let`/`mut` introduce names in this scope.
                for (i, name) in unpack.names.iter().enumerate() {
                    let ty = element_types.get(i).cloned().unwrap_or(ResolvedType::Unknown);
                    let target_span = unpack.name_spans.get(i).copied().unwrap_or(stmt.span);
                    self.check_unannotated_assignment_target(name, unpack.binding, ty, target_span, unpack.value.span);
                }
            }
            Statement::TupleAssign(assign) => {
                // Check the value expression (should be a tuple)
                let value_ty = self.check_expr(&assign.value);

                let element_types = self.destructured_element_types(&value_ty, assign.targets.len(), stmt.span);

                // Check each target expression - must be a valid lvalue
                for (i, target) in assign.targets.iter().enumerate() {
                    let target_ty = self.check_expr(target);
                    let expected_ty = element_types.get(i).cloned().unwrap_or(ResolvedType::Unknown);

                    // Check that target is a valid lvalue
                    match &target.node {
                        Expr::Ident(name) => {
                            self.record_write_target_identity(target.span, name);
                            // Check that the variable is mutable
                            if let Some(var_info) = self.lookup_local_variable_info(name)
                                && !var_info.is_mutable
                            {
                                self.errors.push(errors::mutation_without_mut(name, target.span));
                            } else if let Some(static_info) = self.lookup_static_info(name) {
                                if static_info.is_imported {
                                    self.errors
                                        .push(errors::imported_static_reassignment_not_allowed(name, target.span));
                                }
                            } else if self.const_decls.contains_key(name) {
                                self.errors
                                    .push(errors::const_reassignment_suggests_static(name, target.span));
                            }
                        }
                        Expr::Index(_, _) | Expr::Field(_, _) => {
                            // Index and field expressions are valid lvalues
                            // Type compatibility is checked below
                        }
                        _ => {
                            self.errors.push(errors::invalid_tuple_assignment_target(target.span));
                        }
                    }

                    // Check type compatibility
                    if !self.types_compatible(&expected_ty, &target_ty) {
                        self.errors.push(errors::type_mismatch(
                            &target_ty.to_string(),
                            &expected_ty.to_string(),
                            target.span,
                        ));
                    }
                }
            }
            Statement::ChainedAssignment(ca) => {
                // Check the value expression
                let value_ty = self.check_expr(&ca.value);

                // Chained source assignment has the same declaration/reassignment distinction as a single target.
                for (index, target) in ca.targets.iter().enumerate() {
                    let target_span = ca.target_spans.get(index).copied().unwrap_or(stmt.span);
                    self.check_unannotated_assignment_target(
                        target,
                        ca.binding,
                        value_ty.clone(),
                        target_span,
                        ca.value.span,
                    );
                }
            }
        }
        self.reject_unbound_c_abi_span_constructors();
    }

    /// Validate assignment to an object field, including generic-owner field substitution.
    fn check_field_assignment(&mut self, field_assign: &FieldAssignmentStmt, span: Span) {
        // Check the object expression
        let obj_ty = self.check_expr(&field_assign.object);
        let field = &field_assign.field;

        // Tuples are immutable - disallow field assignment on tuples
        if matches!(obj_ty, ResolvedType::Tuple(_)) {
            self.errors.push(errors::tuple_field_assignment(span));
            return;
        }

        // Verify field exists on object and value type matches field type
        match &obj_ty {
            ResolvedType::SelfType => {
                if let Some(expected_ty) = self.trait_required_field_type(field, field_assign.target_span) {
                    let value_ty = self.check_expr_with_expected(&field_assign.value, Some(&expected_ty));
                    if !self.types_compatible(&value_ty, &expected_ty) {
                        self.errors.push(errors::field_type_mismatch(
                            field,
                            &expected_ty.to_string(),
                            &value_ty.to_string(),
                            field_assign.value.span,
                        ));
                    }
                }
            }
            ResolvedType::Named(type_name) => {
                match self.resolve_nominal_field_type(type_name, None, field, field_assign.target_span) {
                    Some(expected_ty) => {
                        let value_ty = self.check_expr_with_expected(&field_assign.value, Some(&expected_ty));
                        if !self.types_compatible(&value_ty, &expected_ty) {
                            self.errors.push(errors::field_type_mismatch(
                                field,
                                &expected_ty.to_string(),
                                &value_ty.to_string(),
                                field_assign.value.span,
                            ));
                        }
                    }
                    None => {
                        self.errors.push(errors::missing_field(type_name, field, span));
                    }
                }
            }
            ResolvedType::Generic(type_name, type_args) => {
                match self.resolve_nominal_field_type(
                    type_name,
                    Some(type_args.as_slice()),
                    field,
                    field_assign.target_span,
                ) {
                    Some(expected_ty) => {
                        let value_ty = self.check_expr_with_expected(&field_assign.value, Some(&expected_ty));
                        if !self.types_compatible(&value_ty, &expected_ty) {
                            self.errors.push(errors::field_type_mismatch(
                                field,
                                &expected_ty.to_string(),
                                &value_ty.to_string(),
                                field_assign.value.span,
                            ));
                        }
                    }
                    None => {
                        self.errors.push(errors::missing_field(type_name, field, span));
                    }
                }
            }
            // A value typed by a Rust path (`mut plan = empty_plan()` where the helper returns
            // `rust::substrait::proto::Plan`) assigns a field through the same metadata that answers a read of it,
            // and the expected field type drives the value's boundary coercions exactly as a constructor argument
            // would.
            ResolvedType::RustPath(path) => match self.rust_path_field_type(path, field) {
                Some(expected_ty) => {
                    let value_ty = self.check_expr_with_expected(&field_assign.value, Some(&expected_ty));
                    if !self.types_compatible(&value_ty, &expected_ty) {
                        self.errors.push(errors::field_type_mismatch(
                            field,
                            &expected_ty.to_string(),
                            &value_ty.to_string(),
                            field_assign.value.span,
                        ));
                    }
                }
                None => {
                    self.errors
                        .push(errors::missing_field(&obj_ty.to_string(), field, span));
                }
            },
            ResolvedType::Unknown => {
                // Don't report additional errors on unknown types
            }
            _ => {
                // Cannot assign fields to primitive types
                self.errors
                    .push(errors::missing_field(&obj_ty.to_string(), field, span));
            }
        }
    }

    /// Validate list/dict index assignment or RFC 028 `__setitem__` dispatch for user-defined receivers.
    fn check_index_assignment(&mut self, index_assign: &IndexAssignmentStmt, span: Span) {
        // Check the object expression (should be a collection)
        let obj_ty = self.check_expr(&index_assign.object);
        // Check the index expression
        let index_ty = self.check_expr(&index_assign.index);
        // Check the value expression
        let value_ty = self.check_expr(&index_assign.value);

        // Verify object is indexable and types match
        match &obj_ty {
            ResolvedType::Generic(name, args) => match collection_type_id(name.as_str()) {
                Some(CollectionTypeId::List) => {
                    // List[T] - index must be int, value must be T
                    if !matches!(index_ty, ResolvedType::Int) {
                        self.errors.push(errors::index_type_mismatch(
                            "int",
                            &index_ty.to_string(),
                            index_assign.index.span,
                        ));
                    }
                    if let Some(elem_ty) = args.first()
                        && !self.types_compatible(&value_ty, elem_ty)
                    {
                        self.errors.push(errors::index_value_type_mismatch(
                            &elem_ty.to_string(),
                            &value_ty.to_string(),
                            index_assign.value.span,
                        ));
                    }
                }
                Some(CollectionTypeId::Dict) => {
                    // Dict[K, V] - index must be K, value must be V
                    if let Some(key_ty) = args.first()
                        && !self.types_compatible(&index_ty, key_ty)
                    {
                        self.errors.push(errors::index_type_mismatch(
                            &key_ty.to_string(),
                            &index_ty.to_string(),
                            index_assign.index.span,
                        ));
                    }
                    if let Some(val_ty) = args.get(1)
                        && !self.types_compatible(&value_ty, val_ty)
                    {
                        self.errors.push(errors::index_value_type_mismatch(
                            &val_ty.to_string(),
                            &value_ty.to_string(),
                            index_assign.value.span,
                        ));
                    }
                }
                _ => {
                    if self.is_user_operator_receiver(&obj_ty) {
                        if self
                            .resolve_index_set_dunder(
                                &obj_ty,
                                &index_assign.index,
                                &index_ty,
                                &index_assign.value,
                                &value_ty,
                                span,
                            )
                            .is_none()
                        {
                            self.errors
                                .push(errors::missing_method(&obj_ty.to_string(), "__setitem__", span));
                        }
                    } else {
                        self.errors.push(errors::not_indexable(&obj_ty.to_string(), span));
                    }
                }
            },
            ResolvedType::Tuple(_) => {
                // Tuples are immutable - cannot assign to index
                self.errors.push(errors::tuple_field_assignment(span));
            }
            ResolvedType::Str => {
                // Strings are immutable in Incan
                self.errors.push(errors::string_index_assignment_not_allowed(span));
            }
            ResolvedType::Unknown => {
                // Don't report additional errors on unknown types
            }
            ty if self.is_user_operator_receiver(ty) => {
                if self
                    .resolve_index_set_dunder(ty, &index_assign.index, &index_ty, &index_assign.value, &value_ty, span)
                    .is_none()
                {
                    self.errors
                        .push(errors::missing_method(&ty.to_string(), "__setitem__", span));
                }
            }
            _ => {
                self.errors.push(errors::not_indexable(&obj_ty.to_string(), span));
            }
        }
    }

    /// Validate assignment statements, including declarations, reassignments, and local annotation compatibility.
    ///
    /// This is the frontend boundary for rejecting unsupported local type annotations before lowering. In particular,
    /// trait-typed locals must not proceed to codegen because Rust has no valid bare trait type for `let` annotations.
    fn check_assignment(&mut self, assign: &AssignmentStmt, span: Span) {
        let target_span = assign.name_span;
        let annotated_ty = assign.ty.as_ref().map(|ty_ann| self.resolve_type_checked(ty_ann));
        // `let` and `mut` are declaration forms: they introduce a binding that may deliberately shadow an active
        // outer one, so they never resolve against the scope chain. Only a bare `x = value` asks "does this name
        // already exist somewhere out there", and the two halves have to move together — walking outward without
        // this branch would turn every in-block `let` into a reassignment of the outer binding (#1072).
        let introduces_binding = Self::binding_introduces_name(assign.binding);
        if !introduces_binding {
            self.record_write_target_identity(target_span, &assign.name);
        }
        let reassignment_ty = (!introduces_binding)
            .then(|| {
                self.lookup_variable_info_in_scope_chain(&assign.name)
                    .map(|var_info| var_info.ty.clone())
            })
            .flatten();
        let value_ty = if let Some(var_ty) = reassignment_ty.as_ref() {
            self.check_expr_with_expected(&assign.value, Some(var_ty))
        } else {
            self.check_expr_with_expected(&assign.value, annotated_ty.as_ref())
        };

        // A `const` is registered as a module-scope variable, so the scope-chain walk below finds it. Answer the
        // more specific question first: reassigning a const is not a mutability mistake to be fixed with `mut`, it
        // is a request for a `static`, and the generic "variable is immutable" error would send the reader the
        // wrong way. A `let`/`mut` declaration that shadows a const is a new binding and never reaches here.
        if !introduces_binding && self.active_binding_is_const(&assign.name) {
            self.errors
                .push(errors::const_reassignment_suggests_static(&assign.name, target_span));
            return;
        }

        // Check if it's a re-assignment
        if let Some(var_info) = (!introduces_binding)
            .then(|| self.lookup_variable_info_in_scope_chain(&assign.name))
            .flatten()
        {
            let is_mutable = var_info.is_mutable;
            let var_ty = var_info.ty.clone();

            if !is_mutable {
                self.errors
                    .push(errors::mutation_without_mut(&assign.name, target_span));
            }
            if !self.types_compatible(&value_ty, &var_ty) {
                self.errors.push(errors::assignment_type_mismatch(
                    &assign.name,
                    &var_ty.to_string(),
                    &value_ty.to_string(),
                    assign.value.span,
                ));
            }
            self.consumed_iterator_bindings.remove(&assign.name);
            self.transferred_c_resource_bindings.remove(&assign.name);
            return;
        }

        if let Some(static_info) = self.lookup_static_info(&assign.name) {
            if static_info.is_imported {
                self.errors.push(errors::imported_static_reassignment_not_allowed(
                    &assign.name,
                    target_span,
                ));
                return;
            }
            let static_ty = static_info.ty.clone();
            let value_ty = self.check_expr_with_expected(&assign.value, Some(&static_ty));
            if !self.types_compatible(&value_ty, &static_ty) {
                self.errors.push(errors::type_mismatch(
                    &static_ty.to_string(),
                    &value_ty.to_string(),
                    assign.value.span,
                ));
            }
            return;
        }

        // New binding
        let is_mutable = matches!(assign.binding, BindingKind::Mutable);

        // Tuples are immutable - disallow `mut` on tuple bindings
        if is_mutable && matches!(value_ty, ResolvedType::Tuple(_)) {
            self.errors.push(errors::mutable_tuple(span));
        }

        if is_mutable {
            self.mutable_bindings.insert(assign.name.clone());
        }

        let ty = if let Some(ty_ann) = &assign.ty {
            let ann_ty = annotated_ty.unwrap_or_else(|| self.resolve_type_checked(ty_ann));
            let trait_typed_local = self.is_trait_typed_local_annotation(&ann_ty);
            if trait_typed_local {
                self.errors.push(errors::trait_typed_local_annotation_unsupported(
                    &ann_ty.to_string(),
                    ty_ann.span,
                ));
            }
            // Check value matches annotation
            if !self.types_compatible(&value_ty, &ann_ty)
                && !self.record_validated_newtype_coercion_if_possible(&value_ty, &ann_ty, assign.value.span)
            {
                self.errors.push(errors::assignment_type_mismatch(
                    &assign.name,
                    &ann_ty.to_string(),
                    &value_ty.to_string(),
                    assign.value.span,
                ));
            }
            if trait_typed_local { value_ty } else { ann_ty }
        } else {
            value_ty
        };

        self.record_assignment_binding_type(span, ty.clone());

        self.validate_protected_builtin_binding(&assign.name, span);
        let symbol = Symbol {
            name: assign.name.clone(),
            kind: SymbolKind::Variable(VariableInfo {
                ty,
                is_mutable,
                is_used: false,
            }),
            span: target_span,
            scope: 0,
        };
        if introduces_binding {
            self.symbols.define_explicit_shadow(symbol);
        } else {
            self.symbols.define(symbol);
        }
        self.record_write_target_identity(target_span, &assign.name);
        self.bind_c_abi_output_slot_assignment(&assign.name, assign.value.span);
        self.bind_c_abi_span_assignment(&assign.name, assign.value.span);
        self.bind_c_abi_raw_result_assignment(&assign.name, assign.value.span);
        self.consumed_iterator_bindings.remove(&assign.name);
        self.transferred_c_resource_bindings.remove(&assign.name);
    }

    /// Whether this spelling deliberately declares a new source binding instead of resolving an active one.
    const fn binding_introduces_name(binding: BindingKind) -> bool {
        matches!(binding, BindingKind::Let | BindingKind::Mutable)
    }

    /// Whether the binding currently selected by lexical lookup is the collected module `const`.
    ///
    /// `const_decls` is keyed by spelling for constant evaluation, so consulting it alone mistakes an inner
    /// `mut NAME` for the outer `const NAME`. Pairing it with the active symbol's declaration span preserves normal
    /// lexical shadowing while retaining the dedicated const-reassignment diagnostic for the const itself.
    fn active_binding_is_const(&self, name: &str) -> bool {
        let Some((_, const_span)) = self.const_decls.get(name) else {
            return false;
        };
        self.symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            .is_some_and(|symbol| symbol.span == *const_span)
    }

    /// Check or introduce one target from a tuple-unpack or chained source assignment.
    ///
    /// This is intentionally the unannotated half of [`Self::check_assignment`]: all three spellings share the
    /// binding-form decision, but only a single assignment owns a local type annotation and C-ABI constructor facts.
    fn check_unannotated_assignment_target(
        &mut self,
        name: &str,
        binding: BindingKind,
        value_ty: ResolvedType,
        target_span: Span,
        value_span: Span,
    ) {
        if !Self::binding_introduces_name(binding) {
            self.record_write_target_identity(target_span, name);
            if self.active_binding_is_const(name) {
                self.errors
                    .push(errors::const_reassignment_suggests_static(name, target_span));
                return;
            }
            if let Some(var_info) = self.lookup_variable_info_in_scope_chain(name) {
                let is_mutable = var_info.is_mutable;
                let declared_ty = var_info.ty.clone();
                if !is_mutable {
                    self.errors.push(errors::mutation_without_mut(name, target_span));
                }
                if !self.types_compatible(&value_ty, &declared_ty) {
                    self.errors.push(errors::assignment_type_mismatch(
                        name,
                        &declared_ty.to_string(),
                        &value_ty.to_string(),
                        value_span,
                    ));
                }
                self.consumed_iterator_bindings.remove(name);
                self.transferred_c_resource_bindings.remove(name);
                return;
            }
            if let Some(static_info) = self.lookup_static_info(name) {
                if static_info.is_imported {
                    self.errors
                        .push(errors::imported_static_reassignment_not_allowed(name, target_span));
                    return;
                }
                let static_ty = static_info.ty.clone();
                if !self.types_compatible(&value_ty, &static_ty) {
                    self.errors.push(errors::type_mismatch(
                        &static_ty.to_string(),
                        &value_ty.to_string(),
                        value_span,
                    ));
                }
                return;
            }
        }

        let is_mutable = matches!(binding, BindingKind::Mutable);
        self.validate_protected_builtin_binding(name, target_span);
        let symbol = Symbol {
            name: name.to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: value_ty,
                is_mutable,
                is_used: false,
            }),
            span: target_span,
            scope: 0,
        };
        if Self::binding_introduces_name(binding) {
            self.symbols.define_explicit_shadow(symbol);
        } else {
            self.symbols.define(symbol);
        }
        self.record_write_target_identity(target_span, name);
        if is_mutable {
            self.mutable_bindings.insert(name.to_string());
        }
        self.consumed_iterator_bindings.remove(name);
        self.transferred_c_resource_bindings.remove(name);
    }

    /// Preserve the binding selected for a write at its exact authored target span.
    pub(super) fn record_write_target_identity(&mut self, span: Span, name: &str) {
        let resolved = self.symbols.lookup(name).and_then(|symbol_id| {
            let identity = self.symbols.identity_of(symbol_id)?.clone();
            let ty = match &self.symbols.get(symbol_id)?.kind {
                SymbolKind::Variable(info) => info.ty.clone(),
                SymbolKind::Static(info) => info.ty.clone(),
                _ => return None,
            };
            Some((identity, ty))
        });
        if let Some((identity, ty)) = resolved {
            self.type_info.record_resolved_write_identity(span, name, identity, ty);
        }
    }

    /// Give a compiler-managed C output constructor the ordinary local name supplied by its enclosing assignment.
    fn bind_c_abi_output_slot_assignment(&mut self, name: &str, value_span: Span) {
        let Some(slot) = self
            .unbound_c_abi_output_slot_constructors
            .remove(&(value_span.start, value_span.end))
        else {
            return;
        };
        self.pending_c_abi_output_slots.insert(name.to_string(), slot);
    }

    /// Bind an opaque checked typed span only when its constructor is the exact value of one ordinary local.
    fn bind_c_abi_span_assignment(&mut self, name: &str, value_span: Span) {
        let Some(kind) = self
            .unbound_c_abi_span_constructors
            .remove(&(value_span.start, value_span.end))
        else {
            return;
        };
        let Some(binding_span) = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            .map(|symbol| symbol.span)
        else {
            return;
        };
        self.c_abi_span_bindings
            .insert(name.to_string(), CAbiSpanLocal { kind, binding_span });
    }

    /// Reject constructors that were nested in a return, collection, field, callback, or ordinary call instead of
    /// receiving the compiler-tracked local owner required by the checked bridge.
    fn reject_unbound_c_abi_span_constructors(&mut self) {
        let constructors = std::mem::take(&mut self.unbound_c_abi_span_constructors);
        for ((start, end), _kind) in constructors {
            self.errors.push(crate::frontend::diagnostics::CompileError::type_error(
                "a checked byte carrier must be assigned directly to one local before its closed bridge methods are used"
                    .to_string(),
                Span::new(start, end),
            ));
        }
    }

    /// Give a checked C raw call result the ordinary local name supplied by its enclosing assignment.
    fn bind_c_abi_raw_result_assignment(&mut self, name: &str, value_span: Span) {
        let Some(mut result) = self
            .unbound_c_abi_raw_call_results
            .remove(&(value_span.start, value_span.end))
        else {
            return;
        };
        result.local_symbol_span = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            .map(|symbol| symbol.span);
        result.local_name = Some(name.to_string());
        self.c_abi_raw_call_results.push(result);
    }

    /// Check a return statement against the active function context.
    fn check_return(&mut self, expr: Option<&Spanned<Expr>>, span: Span) {
        if matches!(self.current_yield_context, super::YieldContext::Generator { .. }) {
            if let Some(expr) = expr {
                self.check_expr(expr);
                self.errors.push(errors::generator_return_value_not_supported(span));
            }
            return;
        }

        let return_ty = if let Some(e) = expr {
            let expected_return_ty = self.symbols.current_return_type().cloned();
            self.check_expr_with_expected(e, expected_return_ty.as_ref())
        } else {
            ResolvedType::Unit
        };

        if let Some(expected) = self.symbols.current_return_type()
            && !self.types_compatible(&return_ty, expected)
        {
            self.errors.push(errors::return_type_mismatch(
                &expected.to_string(),
                &return_ty.to_string(),
                span,
            ));
        }
    }

    /// Return the canonical declaration identity for a nominal type name, when resolution proved one.
    fn canonical_isinstance_target_identity(&self, name: &str) -> Option<incan_semantics_core::CanonicalSymbolId> {
        let imported = self.type_info.resolved_import_identity(name).cloned();
        let local = self.lookup_symbol(name).and_then(|symbol| {
            let target = self.source_target_for_symbol(name, &symbol.kind)?;
            let kind = incan_semantics_core::SemanticSourceTargetKind::from_kind_str(&target.kind);
            Some(incan_semantics_core::CanonicalSymbolId::module_declaration(
                target.module_path,
                target.name,
                kind,
                incan_semantics_core::HirSourceSpan::new(symbol.span.start, symbol.span.end),
            ))
        });
        imported.or(local)
    }

    /// Resolve the type argument used by a narrowing expression.
    pub(in crate::frontend::typechecker) fn resolve_isinstance_target(
        &mut self,
        expr: &Spanned<Expr>,
    ) -> Option<super::type_info::IsInstanceTargetInfo> {
        let name = match &expr.node {
            Expr::Ident(name) => name,
            Expr::Paren(inner) => {
                let mut target = self.resolve_isinstance_target(inner)?;
                target.span = expr.span;
                return Some(target);
            }
            _ => return None,
        };
        let source_type = Spanned::new(Type::Simple(name.clone()), expr.span);
        let ty = self.resolve_type_checked(&source_type);
        if ty == ResolvedType::Unknown {
            return None;
        }
        let canonical = self.canonical_isinstance_target_identity(name).or_else(|| match &ty {
            ResolvedType::Named(resolved_name) => self.canonical_isinstance_target_identity(resolved_name),
            _ => None,
        });
        Some(super::type_info::IsInstanceTargetInfo {
            ty,
            canonical,
            span: expr.span,
        })
    }

    /// Return whether two union member candidates are equivalent for narrowing.
    fn union_member_matches(&self, member: &ResolvedType, target: &ResolvedType) -> bool {
        self.types_compatible(member, target) && self.types_compatible(target, member)
    }

    /// Return the type available in the true branch of an `isinstance` check.
    fn narrowed_type_for_isinstance(
        &self,
        current_ty: &ResolvedType,
        target_ty: &ResolvedType,
    ) -> Option<ResolvedType> {
        if let Some(members) = current_ty.union_members() {
            return members
                .iter()
                .find(|member| self.union_member_matches(member, target_ty))
                .cloned();
        }

        if let Some(inner) = current_ty.option_inner_type() {
            if let Some(members) = inner.union_members() {
                return members
                    .iter()
                    .find(|member| self.union_member_matches(member, target_ty))
                    .cloned();
            }
            if self.union_member_matches(inner, target_ty) {
                return Some(inner.clone());
            }
        }

        None
    }

    /// Return the union-minus-target type after a failed `isinstance` check.
    fn union_minus_type(&self, members: &[ResolvedType], target_ty: &ResolvedType) -> Option<ResolvedType> {
        let remaining: Vec<_> = members
            .iter()
            .filter(|member| !self.union_member_matches(member, target_ty))
            .cloned()
            .collect();
        if remaining.len() == members.len() {
            None
        } else {
            Some(union_ty(remaining))
        }
    }

    /// Return the else-branch type for an `isinstance` check.
    fn else_type_for_isinstance(&self, current_ty: &ResolvedType, target_ty: &ResolvedType) -> Option<ResolvedType> {
        if let Some(members) = current_ty.union_members() {
            return self.union_minus_type(members, target_ty);
        }

        if let Some(inner) = current_ty.option_inner_type() {
            if let Some(members) = inner.union_members() {
                return self.union_minus_type(members, target_ty).map(option_ty);
            }
            if self.union_member_matches(inner, target_ty) {
                return Some(ResolvedType::Unit);
            }
        }

        None
    }

    /// Return whether an expression is the source-level `None` value.
    fn is_none_expr(expr: &Spanned<Expr>) -> bool {
        matches!(&expr.node, Expr::Literal(Literal::None))
            || matches!(&expr.node, Expr::Ident(name) if name == constructors::as_str(ConstructorId::None))
    }

    /// Determine branch-local narrowing introduced by a boolean condition.
    fn condition_branch_narrowing(&self, expr: &Spanned<Expr>) -> Option<BranchNarrowing> {
        if let Some(narrowing) = self.isinstance_branch_narrowing(expr) {
            return Some(narrowing);
        }
        self.none_check_branch_narrowing(expr)
    }

    /// Return the value arguments of either source spelling for one checked builtin call.
    fn checked_isinstance_condition_args(expr: &Spanned<Expr>) -> Option<&[CallArg]> {
        match &expr.node {
            Expr::Call(_, _, args) | Expr::MethodCall(_, _, _, args) => Some(args),
            _ => None,
        }
    }

    /// Determine branch-local narrowing introduced by `isinstance`.
    fn isinstance_branch_narrowing(&self, expr: &Spanned<Expr>) -> Option<BranchNarrowing> {
        let args = Self::checked_isinstance_condition_args(expr)?;
        if args.len() != 2 {
            return None;
        }
        let value_expr = match &args[0] {
            CallArg::Positional(expr) => expr,
            _ => return None,
        };
        let Expr::Ident(var_name) = &value_expr.node else {
            return None;
        };

        let target_ty = &self.type_info.isinstance_target(expr.span)?.ty;
        let var_info = self.lookup_variable_info(var_name)?;
        let true_ty = self.narrowed_type_for_isinstance(&var_info.ty, target_ty)?;
        let false_ty = self.else_type_for_isinstance(&var_info.ty, target_ty);

        Some(BranchNarrowing {
            name: var_name.clone(),
            true_ty,
            false_ty,
            is_mutable: var_info.is_mutable,
            span: value_expr.span,
        })
    }

    /// Determine branch-local narrowing introduced by `x is None` or `x is not None`.
    fn none_check_branch_narrowing(&self, expr: &Spanned<Expr>) -> Option<BranchNarrowing> {
        let Expr::Binary(value_expr, op @ (BinaryOp::Is | BinaryOp::IsNot), right_expr) = &expr.node else {
            return None;
        };
        if !Self::is_none_expr(right_expr) {
            return None;
        }
        let Expr::Ident(var_name) = &value_expr.node else {
            return None;
        };
        let var_info = self.lookup_variable_info(var_name)?;
        let inner = var_info.ty.option_inner_type()?.clone();
        let (true_ty, false_ty) = if matches!(op, BinaryOp::IsNot) {
            (inner, ResolvedType::Unit)
        } else {
            (ResolvedType::Unit, inner)
        };

        Some(BranchNarrowing {
            name: var_name.clone(),
            true_ty,
            false_ty: Some(false_ty),
            is_mutable: var_info.is_mutable,
            span: value_expr.span,
        })
    }

    /// Shadow a binding inside a branch with its narrowed type.
    fn define_narrowed_binding(&mut self, name: String, ty: ResolvedType, is_mutable: bool, span: Span) {
        self.symbols.define_refined_binding(Symbol {
            name: name.clone(),
            kind: SymbolKind::Variable(VariableInfo {
                ty,
                is_mutable,
                is_used: false,
            }),
            span,
            scope: 0,
        });
        if is_mutable {
            self.mutable_bindings.insert(name);
        }
    }

    /// Convert a condition narrowing result into the refinement available after the condition is false.
    fn branch_false_refinement(narrowing: BranchNarrowing) -> Option<BranchRefinement> {
        narrowing.false_ty.map(|ty| BranchRefinement {
            name: narrowing.name,
            ty,
            is_mutable: narrowing.is_mutable,
            span: narrowing.span,
        })
    }

    /// Shadow all currently-known branch refinements in the active scope.
    fn apply_branch_refinements(&mut self, refinements: &[BranchRefinement]) {
        for refinement in refinements {
            self.define_narrowed_binding(
                refinement.name.clone(),
                refinement.ty.clone(),
                refinement.is_mutable,
                refinement.span,
            );
        }
    }

    /// Insert or replace the accumulated false-branch refinement for one binding.
    fn upsert_branch_refinement(refinements: &mut Vec<BranchRefinement>, refinement: BranchRefinement) {
        if let Some(existing) = refinements.iter_mut().find(|existing| existing.name == refinement.name) {
            *existing = refinement;
        } else {
            refinements.push(refinement);
        }
    }

    /// Return the slot handles made readable by one explicit raw-result outcome comparison.
    fn c_abi_output_slots_available_for_condition(&self, expr: &Spanned<Expr>) -> HashSet<String> {
        let Expr::Binary(left, BinaryOp::Eq, right) = &expr.node else {
            return HashSet::new();
        };
        let Some((status, outcome)) =
            [(&**left, &**right), (&**right, &**left)]
                .into_iter()
                .find(|(status, outcome)| {
                    let Expr::Ident(result_name) = &status.node else {
                        return false;
                    };
                    self.c_abi_raw_call_result_for_local(result_name).is_some()
                        && Self::c_abi_qualified_expr_name(outcome).is_some()
                })
        else {
            return HashSet::new();
        };
        let (Expr::Ident(result_name), Some(outcome_name)) = (&status.node, Self::c_abi_qualified_expr_name(outcome))
        else {
            return HashSet::new();
        };
        let Some(raw_result) = self.c_abi_raw_call_result_for_local(result_name) else {
            return HashSet::new();
        };
        let Some(binding) = self.type_info.c_abi.bindings.get(&raw_result.binding) else {
            return HashSet::new();
        };
        let Some(symbol) = binding.symbols.iter().find(|symbol| symbol.name == raw_result.symbol) else {
            return HashSet::new();
        };
        // Binding declarations store outcome names relative to their binding (`Status.OK`), while ordinary source
        // compares the emitted class-qualified enum value (`Fixture.Status.OK`). Preserve that declaration-local
        // spelling rather than leaking a second outcome representation into the binding descriptor.
        let binding_prefix = format!("{}.", raw_result.binding);
        let outcome_name = outcome_name.strip_prefix(&binding_prefix).unwrap_or(&outcome_name);
        let Some(declared) = symbol.outcomes.iter().find(|declared| declared.result == outcome_name) else {
            return HashSet::new();
        };

        symbol
            .parameters
            .iter()
            .filter_map(|parameter| {
                let CBindingType::Output { mode, .. } = &parameter.ty else {
                    return None;
                };
                let slot_identity = raw_result.slots_by_parameter.get(&parameter.name)?;
                let readable = match mode {
                    COutputMode::Out => declared.initializes.contains(&parameter.name),
                    COutputMode::InOut => !declared.invalidates.contains(&parameter.name),
                };
                readable.then(|| slot_identity.clone())
            })
            .collect()
    }

    /// Resolve one raw C call outcome relation through the same lexical binding currently selected for a local name.
    fn c_abi_raw_call_result_for_local(&self, name: &str) -> Option<&super::CAbiRawCallResult> {
        let symbol_span = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            .map(|symbol| symbol.span)?;
        self.c_abi_raw_call_results
            .iter()
            .rev()
            .find(|result| result.local_name.as_deref() == Some(name) && result.local_symbol_span == Some(symbol_span))
    }

    /// Render one ordinary source enum/namespace reference into the descriptor outcome spelling.
    fn c_abi_qualified_expr_name(expr: &Spanned<Expr>) -> Option<String> {
        match &expr.node {
            Expr::Ident(name) => Some(name.clone()),
            Expr::Field(base, member) => Some(format!("{}.{}", Self::c_abi_qualified_expr_name(base)?, member)),
            Expr::Paren(inner) => Self::c_abi_qualified_expr_name(inner),
            _ => None,
        }
    }

    /// Check one expression-conditioned branch under incoming false-branch refinements.
    fn check_expr_condition_body(
        &mut self,
        expr: &Spanned<Expr>,
        body: &[Spanned<Statement>],
        incoming_refinements: &[BranchRefinement],
    ) -> Option<BranchRefinement> {
        let available_before = self.available_c_abi_output_slots.clone();
        self.symbols.enter_scope(ScopeKind::Block);
        self.apply_branch_refinements(incoming_refinements);

        let cond_ty = self.check_expr(expr);
        self.validate_truthiness_condition(&cond_ty, expr.span);
        self.available_c_abi_output_slots
            .extend(self.c_abi_output_slots_available_for_condition(expr));
        let true_narrowing = self.condition_branch_narrowing(expr);
        let false_refinement = true_narrowing.as_ref().cloned().and_then(Self::branch_false_refinement);

        if let Some(narrowing) = true_narrowing {
            self.define_narrowed_binding(narrowing.name, narrowing.true_ty, narrowing.is_mutable, narrowing.span);
        }
        self.check_statement_block(body);
        self.symbols.exit_scope();
        self.available_c_abi_output_slots = available_before;

        false_refinement
    }

    /// Check an acknowledgement body in its surrounding ordinary statement scope.
    fn check_unsafe_stmt(&mut self, unsafe_stmt: &UnsafeStmt) {
        self.unsafe_depth += 1;
        self.check_statement_block(&unsafe_stmt.body);
        self.unsafe_depth = self.unsafe_depth.saturating_sub(1);
    }

    /// Validate an `if` statement and apply branch-local narrowing where supported.
    fn check_if_stmt(&mut self, if_stmt: &IfStmt) {
        let mut false_refinements = Vec::new();

        if let Some(refinement) = self.check_condition_body(&if_stmt.condition, &if_stmt.then_body, &false_refinements)
        {
            Self::upsert_branch_refinement(&mut false_refinements, refinement);
        }

        for (elif_cond, elif_body) in &if_stmt.elif_branches {
            if let Some(refinement) = self.check_expr_condition_body(elif_cond, elif_body, &false_refinements) {
                Self::upsert_branch_refinement(&mut false_refinements, refinement);
            }
        }

        if let Some(else_body) = &if_stmt.else_body {
            self.symbols.enter_scope(ScopeKind::Block);
            self.apply_branch_refinements(&false_refinements);
            self.check_statement_block(else_body);
            self.symbols.exit_scope();
        }
    }

    /// Type-check a statement-form `while`, including ordinary truthiness and pattern-driven `while let` conditions.
    fn check_while_stmt(&mut self, while_stmt: &WhileStmt) {
        match &while_stmt.condition {
            // ---- Context: ordinary boolean `while` condition ----
            Condition::Expr(expr) => {
                let cond_ty = self.check_expr(expr);
                self.validate_truthiness_condition(&cond_ty, expr.span);

                self.symbols.enter_scope(ScopeKind::Block);
                self.push_loop_context(LoopContextKind::Statement, None);
                self.check_statement_block(&while_stmt.body);
                let _ = self.pop_loop_context();
                self.symbols.exit_scope();
            }
            // ---- Context: pattern-driven `while let` loop ----
            Condition::Let { pattern, value } => {
                let value_ty = self.check_expr(value);
                self.symbols.enter_scope(ScopeKind::Block);
                self.check_pattern(pattern, &value_ty);
                self.push_loop_context(LoopContextKind::Statement, None);
                self.check_statement_block(&while_stmt.body);
                let _ = self.pop_loop_context();
                self.symbols.exit_scope();
            }
        }
    }

    /// Type-check a statement-form `loop:` body.
    ///
    /// Statement loops share the same loop context stack as `for` / `while`, but they do not accept `break <value>`
    /// because no surrounding expression consumes a result.
    fn check_loop_stmt(&mut self, loop_stmt: &LoopStmt) {
        self.symbols.enter_scope(ScopeKind::Block);
        self.push_loop_context(LoopContextKind::Statement, None);
        self.check_statement_block(&loop_stmt.body);
        let _ = self.pop_loop_context();
        self.symbols.exit_scope();
    }

    /// Type-check a statement-form `for`, binding the loop pattern from builtin collections or RFC 068 iteration hooks.
    fn check_for_stmt(&mut self, for_stmt: &ForStmt) {
        let elem_ty = match &for_stmt.iter.node {
            Expr::Try(inner) => {
                let iter_ty = self.check_expr(inner);
                if let Some(unwrapped_iter_ty) = iter_ty.result_ok_type().cloned() {
                    if self.probe_fallible_iteration_protocol(&unwrapped_iter_ty, for_stmt.iter.span) {
                        self.errors
                            .push(errors::result_wrapped_fallible_iterator_requires_local(
                                for_stmt.iter.span,
                            ));
                        ResolvedType::Unknown
                    } else {
                        // Preserve the established `for item in result_of_iterable?` spelling. Here `?` unwraps the
                        // iterable before ordinary `Option`-based iteration; it is not the fallible-poll protocol.
                        let unwrapped_iter_ty = self.validate_try_result_type(&iter_ty, for_stmt.iter.span);
                        self.record_expr_type(for_stmt.iter.span, unwrapped_iter_ty.clone());
                        self.infer_iterator_element_type_from_expr(&for_stmt.iter, &unwrapped_iter_ty)
                    }
                } else {
                    self.infer_fallible_iterator_element_type_from_expr(inner, &iter_ty, for_stmt.iter.span)
                }
            }
            _ => {
                let iter_ty = self.check_expr(&for_stmt.iter);
                self.infer_iterator_element_type_from_expr(&for_stmt.iter, &iter_ty)
            }
        };

        self.symbols.enter_scope(ScopeKind::Block);
        // Record the resolved element type at the pattern's own span. Body IR's `lower_for` already reads the loop
        // pattern's type back through `TypeCheckInfo::expr_type`, and every binding the pattern introduces -- one
        // for a plain binding, or one per tuple element -- takes its type from it (#1125). Without this the loop
        // bindings would carry `Unknown` even though the element type is fully resolved right here.
        self.record_expr_type(for_stmt.pattern.span, elem_ty.clone());
        self.define_for_pattern_bindings(&for_stmt.pattern, &elem_ty);
        self.push_loop_context(LoopContextKind::Statement, None);

        self.check_statement_block(&for_stmt.body);
        let _ = self.pop_loop_context();
        self.symbols.exit_scope();
    }

    /// Resolve the element types a statement destructure should bind, reporting the right diagnostic when the
    /// value cannot supply them.
    ///
    /// Returns exactly `arity` types so callers can bind positionally without re-checking length. The `Unknown`
    /// padding it returns on the error paths is recovery state for the *rest* of the check, not a claim that the
    /// value had that shape — the diagnostic has already been recorded by then.
    fn destructured_element_types(&mut self, value_ty: &ResolvedType, arity: usize, span: Span) -> Vec<ResolvedType> {
        match classify_tuple_shape(value_ty) {
            TupleShape::Tuple(types) => {
                if types.len() != arity {
                    self.errors
                        .push(errors::tuple_unpack_count_mismatch(arity, types.len(), span));
                    return vec![ResolvedType::Unknown; arity];
                }
                types
            }
            TupleShape::RustTuple(rust_arity) => {
                if rust_arity != arity {
                    self.errors
                        .push(errors::tuple_unpack_count_mismatch(arity, rust_arity, span));
                }
                vec![ResolvedType::Unknown; arity]
            }
            TupleShape::OpaqueRust => {
                self.errors.push(errors::tuple_unpack_rust_shape_unverified(
                    arity,
                    &value_ty.to_string(),
                    span,
                ));
                vec![ResolvedType::Unknown; arity]
            }
            TupleShape::NotTuple => {
                self.errors.push(errors::tuple_unpack_expects_tuple_value(
                    arity,
                    &value_ty.to_string(),
                    span,
                ));
                vec![ResolvedType::Unknown; arity]
            }
            TupleShape::Recovery => vec![ResolvedType::Unknown; arity],
        }
    }

    /// Validate a `break` statement against the innermost active loop context.
    ///
    /// For expression-form `loop:` bodies this records the break value type so the loop result can be resolved after
    /// the body finishes checking. For statement loops it rejects `break <value>` while still type-checking the
    /// provided expression to surface any nested errors.
    fn check_break_stmt(&mut self, value: Option<&Spanned<Expr>>, span: Span) {
        let Some((loop_kind, expected_break_ty)) = self
            .loop_stack
            .last()
            .map(|ctx| (ctx.kind, ctx.expected_break_ty.clone()))
        else {
            if let Some(value) = value {
                self.check_expr(value);
            }
            self.errors.push(errors::break_outside_loop(span));
            return;
        };

        let break_ty = match (loop_kind, value) {
            (LoopContextKind::Statement, Some(value)) => {
                let value_ty = self.check_expr(value);
                self.errors
                    .push(errors::break_value_requires_loop_expression(value.span));
                Some((value_ty, value.span))
            }
            (LoopContextKind::Statement, None) => None,
            (LoopContextKind::Expression, Some(value)) => {
                let value_ty = if let Some(expected) = expected_break_ty.as_ref() {
                    self.check_expr_with_expected(value, Some(expected))
                } else {
                    self.check_expr(value)
                };
                Some((value_ty, value.span))
            }
            (LoopContextKind::Expression, None) => Some((ResolvedType::Unit, span)),
        };

        if let Some(break_ty) = break_ty
            && let Some(loop_ctx) = self.current_loop_context_mut()
        {
            loop_ctx.break_types.push(break_ty);
        }
    }

    /// Validate that `continue` appears inside some active loop context.
    fn check_continue_stmt(&mut self, span: Span) {
        if self.loop_stack.is_empty() {
            self.errors.push(errors::continue_outside_loop(span));
        }
    }

    /// Define loop-scope bindings introduced by a `for` header pattern.
    ///
    /// The parser currently admits only bindings, `_`, and tuple bindings, but the exhaustive match keeps hand-built
    /// ASTs from silently reaching lowering with unsupported pattern forms.
    pub(in crate::frontend::typechecker) fn define_for_pattern_bindings(
        &mut self,
        pattern: &Spanned<Pattern>,
        ty: &ResolvedType,
    ) {
        match &pattern.node {
            Pattern::Binding(name) => {
                self.validate_protected_builtin_binding(name, pattern.span);
                self.symbols.define(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: ty.clone(),
                        is_mutable: false,
                        is_used: false,
                    }),
                    span: pattern.span,
                    scope: 0,
                });
                self.record_write_target_identity(pattern.span, name);
            }
            Pattern::Wildcard => {}
            Pattern::Tuple(items) => {
                // The shape question is answered by `classify_tuple_shape`, shared with the statement-level
                // destructuring arms (#1132). Only the diagnostic differs: a loop names the *iteration item* type,
                // because that is what the reader has to change.
                let element_types = match classify_tuple_shape(ty) {
                    TupleShape::Tuple(types) => {
                        if types.len() != items.len() {
                            self.errors.push(errors::tuple_unpack_count_mismatch(
                                items.len(),
                                types.len(),
                                pattern.span,
                            ));
                            vec![ResolvedType::Unknown; items.len()]
                        } else {
                            types
                        }
                    }
                    TupleShape::RustTuple(rust_arity) => {
                        if rust_arity != items.len() {
                            self.errors.push(errors::tuple_unpack_count_mismatch(
                                items.len(),
                                rust_arity,
                                pattern.span,
                            ));
                        }
                        vec![ResolvedType::Unknown; items.len()]
                    }
                    TupleShape::OpaqueRust => {
                        self.errors.push(errors::for_pattern_rust_shape_unverified(
                            items.len(),
                            &ty.to_string(),
                            pattern.span,
                        ));
                        vec![ResolvedType::Unknown; items.len()]
                    }
                    TupleShape::NotTuple => {
                        // A tuple pattern can only destructure a tuple. Binding `Unknown` per name and staying
                        // silent used to hide `for a, b in items` over a `list[int]` completely, and Body IR would
                        // then project `.0`/`.1` out of an `int` (#1125).
                        self.errors.push(errors::for_pattern_expects_tuple_item(
                            items.len(),
                            &ty.to_string(),
                            pattern.span,
                        ));
                        vec![ResolvedType::Unknown; items.len()]
                    }
                    TupleShape::Recovery => vec![ResolvedType::Unknown; items.len()],
                };

                for (i, item) in items.iter().enumerate() {
                    let item_ty = element_types.get(i).cloned().unwrap_or(ResolvedType::Unknown);
                    self.define_for_pattern_bindings(item, &item_ty);
                }
            }
            Pattern::Constructor(_, _) | Pattern::Literal(_) | Pattern::Group(_) | Pattern::Or(_) => {
                self.errors.push(errors::expected_token_message(
                    "Expected identifier, wildcard, or tuple binding in for-loop pattern",
                    &format!("{:?}", pattern.node),
                    pattern.span,
                ));
            }
        }
    }

    /// Validate a condition and its true branch body.
    fn check_condition_body(
        &mut self,
        condition: &Condition,
        body: &[Spanned<Statement>],
        incoming_refinements: &[BranchRefinement],
    ) -> Option<BranchRefinement> {
        match condition {
            Condition::Expr(expr) => self.check_expr_condition_body(expr, body, incoming_refinements),
            Condition::Let { pattern, value } => {
                let value_ty = self.check_expr(value);
                self.symbols.enter_scope(ScopeKind::Block);
                self.apply_branch_refinements(incoming_refinements);
                self.check_pattern(pattern, &value_ty);
                self.check_statement_block(body);
                self.symbols.exit_scope();
                None
            }
        }
    }

    /// Check an assert statement.
    fn check_assert_stmt(&mut self, assert_stmt: &AssertStmt) {
        match &assert_stmt.kind {
            AssertKind::Condition(condition) => {
                let cond_ty = self.check_expr(condition);
                let is_compatible = self.types_compatible(&cond_ty, &ResolvedType::Bool);
                ensure_bool_condition(&cond_ty, condition.span, is_compatible, &mut self.errors);
            }
            AssertKind::IsPattern { value, pattern } => self.check_assert_is_pattern(value, pattern),
            AssertKind::Raises { call, error_type } => {
                self.check_expr(call);
                if let Type::Simple(name) = &error_type.node
                    && (runtime_errors::from_str(name).is_some() || name == "AssertionError")
                {
                    // Known runtime error vocabulary.
                } else {
                    self.errors
                        .push(errors::unknown_symbol(&error_type.node.to_string(), error_type.span));
                }
            }
        }

        if let Some(message) = &assert_stmt.message {
            let msg_ty = self.check_expr(message);
            if !self.types_compatible(&msg_ty, &ResolvedType::Str) {
                self.errors.push(errors::type_mismatch(
                    &ResolvedType::Str.to_string(),
                    &msg_ty.to_string(),
                    message.span,
                ));
            }
        }
    }

    /// Validate the restricted RFC 018 `assert value is Some/None/Ok/Err` pattern subset.
    fn check_assert_is_pattern(&mut self, scrutinee: &Spanned<Expr>, pattern: &Spanned<Pattern>) {
        let scrutinee_ty = self.check_expr(scrutinee);
        let Some(pattern) = Self::assert_is_pattern_from_pattern(pattern) else {
            self.errors.push(errors::expected_token_message(
                "Expected assert `is` pattern Some(name), Some(_), None, Ok(name), Ok(_), Err(name), or Err(_)",
                &format!("{:?}", pattern.node),
                pattern.span,
            ));
            return;
        };

        let expected = match pattern.kind {
            AssertIsPatternKind::Some | AssertIsPatternKind::None => "Option[_]",
            AssertIsPatternKind::Ok | AssertIsPatternKind::Err => "Result[_, _]",
        };
        let compatible = match pattern.kind {
            AssertIsPatternKind::Some | AssertIsPatternKind::None => scrutinee_ty.is_option(),
            AssertIsPatternKind::Ok | AssertIsPatternKind::Err => scrutinee_ty.is_result(),
        };
        if !compatible && !matches!(scrutinee_ty, ResolvedType::Unknown) {
            self.errors.push(errors::type_mismatch(
                expected,
                &scrutinee_ty.to_string(),
                scrutinee.span,
            ));
            return;
        }

        if compatible {
            let constructor = match pattern.kind {
                AssertIsPatternKind::Some => ConstructorId::Some,
                AssertIsPatternKind::None => ConstructorId::None,
                AssertIsPatternKind::Ok => ConstructorId::Ok,
                AssertIsPatternKind::Err => ConstructorId::Err,
            };
            self.record_pattern_lexical_identity(constructors::as_str(constructor), pattern.constructor_span);
        }

        if let Some((name, span)) = pattern.binding {
            let ty = match pattern.kind {
                AssertIsPatternKind::Some => scrutinee_ty
                    .option_inner_type()
                    .cloned()
                    .unwrap_or(ResolvedType::Unknown),
                AssertIsPatternKind::Ok => scrutinee_ty.result_ok_type().cloned().unwrap_or(ResolvedType::Unknown),
                AssertIsPatternKind::Err => scrutinee_ty.result_err_type().cloned().unwrap_or(ResolvedType::Unknown),
                AssertIsPatternKind::None => ResolvedType::Unit,
            };
            self.validate_protected_builtin_binding(&name, span);
            self.symbols.define(Symbol {
                name: name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: false,
                    is_used: false,
                }),
                span,
                scope: 0,
            });
            self.record_write_target_identity(span, &name);
        }
    }

    /// Build an assertion pattern from a parsed pattern.
    fn assert_is_pattern_from_pattern(pattern: &Spanned<Pattern>) -> Option<AssertIsPattern> {
        match &pattern.node {
            Pattern::Constructor(name, args)
                if name.node == constructors::as_str(ConstructorId::None) && args.is_empty() =>
            {
                Some(AssertIsPattern {
                    kind: AssertIsPatternKind::None,
                    constructor_span: name.span,
                    binding: None,
                })
            }
            Pattern::Constructor(name, args) => {
                let kind = match name.node.as_str() {
                    n if n == constructors::as_str(ConstructorId::Some) => AssertIsPatternKind::Some,
                    n if n == constructors::as_str(ConstructorId::Ok) => AssertIsPatternKind::Ok,
                    n if n == constructors::as_str(ConstructorId::Err) => AssertIsPatternKind::Err,
                    _ => return None,
                };
                let [PatternArg::Positional(arg)] = args.as_slice() else {
                    return None;
                };
                let binding = match &arg.node {
                    Pattern::Wildcard => None,
                    Pattern::Binding(name) => Some((name.clone(), arg.span)),
                    _ => return None,
                };
                Some(AssertIsPattern {
                    kind,
                    constructor_span: name.span,
                    binding,
                })
            }
            _ => None,
        }
    }

    /// Typecheck a surface statement via the semantics registry.
    fn check_surface_stmt(&mut self, stmt: &SurfaceStmt, span: Span) {
        use crate::semantics_registry::semantics_registry;

        let Some(action) = semantics_registry().typecheck_surface_stmt_action(&stmt.key) else {
            // No pack claimed this surface statement — report as unknown.
            let label = match &stmt.key {
                incan_semantics_core::SurfaceFeatureKey::SoftKeyword(id) => keywords::as_str(*id).to_string(),
                incan_semantics_core::SurfaceFeatureKey::Decorator(_) => "decorator-surface-feature".to_string(),
                incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key,
                    descriptor_key,
                } => {
                    format!("{dependency_key}:{descriptor_key}")
                }
            };
            self.errors.push(errors::unknown_symbol(&label, span));
            return;
        };

        match (action, &stmt.payload) {
            (SurfaceStmtTypeCheck::AssertCheck, SurfaceStmtPayload::KeywordArgs(args)) => {
                if let Some(condition) = args.first() {
                    let assert_stmt = AssertStmt {
                        kind: AssertKind::Condition(condition.clone()),
                        message: args.get(1).cloned(),
                    };
                    self.check_assert_stmt(&assert_stmt);
                }
            }
        }
    }

    /// Infer the item type for built-in iterable surfaces.
    ///
    /// This captures language-level iteration semantics, such as strings yielding one-character `str` values and
    /// bytes yielding `int` values, before the backend chooses the Rust iterator adapter that implements them.
    pub(crate) fn infer_iterator_element_type(&self, iter_ty: &ResolvedType) -> ResolvedType {
        match iter_ty {
            ResolvedType::FrozenList(elem) | ResolvedType::FrozenSet(elem) => elem.as_ref().clone(),
            ResolvedType::FrozenDict(key, _) => key.as_ref().clone(),
            ResolvedType::Generic(name, args) => {
                if let Some(elem) = iter_ty.iterator_item_type() {
                    return elem.clone();
                }
                // A range is not a collection, so it has no `collection_type_id`, but iterating one still yields its
                // element type. The inline `for i in 0..10` header never reaches here because `check_for_stmt`
                // resolves that expression directly; only a range *bound to a local* arrives as a `Range[T]` value.
                // Without this arm that binding iterates with an unknown item type, which stays invisible until
                // something downstream needs the type -- `acc + i` refusing to lower, for instance.
                if name == incan_core::lang::surface::types::RANGE_TYPE_NAME && !args.is_empty() {
                    return args[0].clone();
                }
                match collection_type_id(name.as_str()) {
                    Some(CollectionTypeId::List) | Some(CollectionTypeId::Set) if !args.is_empty() => args[0].clone(),
                    Some(CollectionTypeId::Dict) if args.len() >= 2 => {
                        // Iterating dict gives keys
                        args[0].clone()
                    }
                    Some(CollectionTypeId::Tuple) if !args.is_empty() => {
                        // For tuple iteration, return first element type (simplified)
                        args[0].clone()
                    }
                    Some(CollectionTypeId::Generator) if !args.is_empty() => args[0].clone(),
                    _ => ResolvedType::Unknown,
                }
            }
            ResolvedType::Str | ResolvedType::FrozenStr => ResolvedType::Str,
            ResolvedType::Bytes | ResolvedType::FrozenBytes => ResolvedType::Int,
            _ => ResolvedType::Unknown,
        }
    }

    /// Infer a loop item type from an iterable expression, falling back to structural `__iter__` / `__next__` hooks.
    pub(crate) fn infer_iterator_element_type_from_expr(
        &mut self,
        iter_expr: &Spanned<Expr>,
        iter_ty: &ResolvedType,
    ) -> ResolvedType {
        if iter_ty.iterator_item_type().is_some()
            && let Expr::Ident(name) = &iter_expr.node
        {
            self.consumed_iterator_bindings.insert(name.clone(), iter_expr.span);
        }
        let elem_ty = self.infer_iterator_element_type(iter_ty);
        if !matches!(elem_ty, ResolvedType::Unknown) || matches!(iter_ty, ResolvedType::Unknown) {
            return elem_ty;
        }
        if self.is_user_operator_receiver(iter_ty) {
            return self
                .resolve_iteration_protocol(iter_ty, iter_expr.span)
                .unwrap_or(ResolvedType::Unknown);
        }
        elem_ty
    }

    /// Infer a loop item from a `for item in iterable?` header.
    ///
    /// The trailing `?` belongs to the loop protocol rather than to the iterable expression: it asks lowering to
    /// propagate errors returned from each `__next__` poll. Ordinary `expr?` remains the existing Result expression.
    fn infer_fallible_iterator_element_type_from_expr(
        &mut self,
        iter_expr: &Spanned<Expr>,
        iter_ty: &ResolvedType,
        loop_span: Span,
    ) -> ResolvedType {
        let has_source_trait_dispatch = self.type_info.resolved_method_call(iter_expr.span).is_some_and(|call| {
            matches!(
                call.dispatch,
                crate::frontend::typechecker::ResolvedMethodDispatch::Trait { .. }
            )
        });
        if self.is_user_operator_receiver(iter_ty) || has_source_trait_dispatch {
            return self
                .resolve_fallible_iteration_protocol(iter_ty, iter_expr.span, loop_span)
                .unwrap_or(ResolvedType::Unknown);
        }

        self.errors.push(errors::type_mismatch(
            "fallible iterator with __iter__() and __next__() -> Result[Option[_], _]",
            &iter_ty.to_string(),
            iter_expr.span,
        ));
        ResolvedType::Unknown
    }
}
