//! Statement checking: assignments, returns, control flow.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved};
use incan_core::lang::builtins::{self as core_builtins, BuiltinFnId};
use incan_core::lang::errors as runtime_errors;
use incan_core::lang::keywords;
use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::{NumericTy, result_numeric_type};
use incan_semantics_core::SurfaceStmtTypeCheck;

use super::{LoopContextKind, TypeChecker};
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

                // Extract element types if it's a tuple
                let element_types: Vec<ResolvedType> = match &value_ty {
                    ResolvedType::Tuple(types) => types.clone(),
                    _ => {
                        // Not a tuple, create Unknown types for each name
                        vec![ResolvedType::Unknown; unpack.names.len()]
                    }
                };

                // Check that tuple has enough elements
                if element_types.len() < unpack.names.len() {
                    self.errors.push(errors::tuple_unpack_count_mismatch(
                        unpack.names.len(),
                        element_types.len(),
                        stmt.span,
                    ));
                }

                // Define each variable with its corresponding type
                let is_mutable = matches!(unpack.binding, BindingKind::Mutable);
                for (i, name) in unpack.names.iter().enumerate() {
                    let ty = element_types.get(i).cloned().unwrap_or(ResolvedType::Unknown);
                    self.symbols.define(Symbol {
                        name: name.clone(),
                        kind: SymbolKind::Variable(VariableInfo {
                            ty,
                            is_mutable,
                            is_used: false,
                        }),
                        span: stmt.span,
                        scope: 0,
                    });
                    if is_mutable {
                        self.mutable_bindings.insert(name.clone());
                    }
                }
            }
            Statement::TupleAssign(assign) => {
                // Check the value expression (should be a tuple)
                let value_ty = self.check_expr(&assign.value);

                // Extract element types if it's a tuple
                let element_types: Vec<ResolvedType> = match &value_ty {
                    ResolvedType::Tuple(types) => types.clone(),
                    _ => {
                        // Not a tuple, create Unknown types for each target
                        vec![ResolvedType::Unknown; assign.targets.len()]
                    }
                };

                // Check that tuple has enough elements
                if element_types.len() < assign.targets.len() {
                    self.errors.push(errors::tuple_unpack_count_mismatch(
                        assign.targets.len(),
                        element_types.len(),
                        stmt.span,
                    ));
                }

                // Check each target expression - must be a valid lvalue
                for (i, target) in assign.targets.iter().enumerate() {
                    let target_ty = self.check_expr(target);
                    let expected_ty = element_types.get(i).cloned().unwrap_or(ResolvedType::Unknown);

                    // Check that target is a valid lvalue
                    match &target.node {
                        Expr::Ident(name) => {
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

                // Define all target variables with the same type
                let is_mutable = matches!(ca.binding, BindingKind::Mutable);
                for target in &ca.targets {
                    self.symbols.define(Symbol {
                        name: target.clone(),
                        kind: SymbolKind::Variable(VariableInfo {
                            ty: value_ty.clone(),
                            is_mutable,
                            is_used: false,
                        }),
                        span: stmt.span,
                        scope: 0,
                    });
                    if is_mutable {
                        self.mutable_bindings.insert(target.clone());
                    }
                }
            }
        }
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
        let annotated_ty = assign.ty.as_ref().map(|ty_ann| self.resolve_type_checked(ty_ann));
        let reassignment_ty = self
            .lookup_local_variable_info(&assign.name)
            .map(|var_info| var_info.ty.clone());
        let value_ty = if let Some(var_ty) = reassignment_ty.as_ref() {
            self.check_expr_with_expected(&assign.value, Some(var_ty))
        } else {
            self.check_expr_with_expected(&assign.value, annotated_ty.as_ref())
        };

        // Check if it's a re-assignment
        if let Some(var_info) = self.lookup_local_variable_info(&assign.name) {
            let is_mutable = var_info.is_mutable;
            let var_ty = var_info.ty.clone();

            if !is_mutable {
                self.errors.push(errors::mutation_without_mut(&assign.name, span));
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
            return;
        }

        if let Some(static_info) = self.lookup_static_info(&assign.name) {
            if static_info.is_imported {
                self.errors
                    .push(errors::imported_static_reassignment_not_allowed(&assign.name, span));
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

        if self.const_decls.contains_key(&assign.name) {
            self.errors
                .push(errors::const_reassignment_suggests_static(&assign.name, span));
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

        self.symbols.define(Symbol {
            name: assign.name.clone(),
            kind: SymbolKind::Variable(VariableInfo {
                ty,
                is_mutable,
                is_used: false,
            }),
            span,
            scope: 0,
        });
        self.consumed_iterator_bindings.remove(&assign.name);
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

    /// Resolve the type argument used by a narrowing expression.
    fn resolve_narrowing_type_expr(&self, expr: &Spanned<Expr>) -> Option<ResolvedType> {
        match &expr.node {
            Expr::Ident(name) => {
                Some(self.expand_type_aliases(resolve_type(&Type::Simple(name.clone()), &self.symbols)))
            }
            Expr::Paren(inner) => self.resolve_narrowing_type_expr(inner),
            _ => None,
        }
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

    /// Determine branch-local narrowing introduced by `isinstance`.
    fn isinstance_branch_narrowing(&self, expr: &Spanned<Expr>) -> Option<BranchNarrowing> {
        let Expr::Call(callee, _, args) = &expr.node else {
            return None;
        };
        let Expr::Ident(call_name) = &callee.node else {
            return None;
        };
        if core_builtins::from_str(call_name) != Some(BuiltinFnId::IsInstance) || args.len() != 2 {
            return None;
        }
        let value_expr = match &args[0] {
            CallArg::Positional(expr) => expr,
            _ => return None,
        };
        let Expr::Ident(var_name) = &value_expr.node else {
            return None;
        };

        let target_expr = match &args[1] {
            CallArg::Positional(expr) => expr,
            _ => return None,
        };
        let target_ty = self.resolve_narrowing_type_expr(target_expr)?;
        let var_info = self.lookup_variable_info(var_name)?;
        let true_ty = self.narrowed_type_for_isinstance(&var_info.ty, &target_ty)?;
        let false_ty = self.else_type_for_isinstance(&var_info.ty, &target_ty);

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
        self.symbols.define(Symbol {
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

    /// Check one expression-conditioned branch under incoming false-branch refinements.
    fn check_expr_condition_body(
        &mut self,
        expr: &Spanned<Expr>,
        body: &[Spanned<Statement>],
        incoming_refinements: &[BranchRefinement],
    ) -> Option<BranchRefinement> {
        self.symbols.enter_scope(ScopeKind::Block);
        self.apply_branch_refinements(incoming_refinements);

        let cond_ty = self.check_expr(expr);
        self.validate_truthiness_condition(&cond_ty, expr.span);
        let true_narrowing = self.condition_branch_narrowing(expr);
        let false_refinement = true_narrowing.as_ref().cloned().and_then(Self::branch_false_refinement);

        if let Some(narrowing) = true_narrowing {
            self.define_narrowed_binding(narrowing.name, narrowing.true_ty, narrowing.is_mutable, narrowing.span);
        }
        for stmt in body {
            self.check_statement(stmt);
        }
        self.symbols.exit_scope();

        false_refinement
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
            for stmt in else_body {
                self.check_statement(stmt);
            }
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
                for stmt in &while_stmt.body {
                    self.check_statement(stmt);
                }
                let _ = self.pop_loop_context();
                self.symbols.exit_scope();
            }
            // ---- Context: pattern-driven `while let` loop ----
            Condition::Let { pattern, value } => {
                let value_ty = self.check_expr(value);
                self.symbols.enter_scope(ScopeKind::Block);
                self.check_pattern(pattern, &value_ty);
                self.push_loop_context(LoopContextKind::Statement, None);
                for stmt in &while_stmt.body {
                    self.check_statement(stmt);
                }
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
        for stmt in &loop_stmt.body {
            self.check_statement(stmt);
        }
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
        self.define_for_pattern_bindings(&for_stmt.pattern, &elem_ty);
        self.push_loop_context(LoopContextKind::Statement, None);

        for stmt in &for_stmt.body {
            self.check_statement(stmt);
        }
        let _ = self.pop_loop_context();
        self.symbols.exit_scope();
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
            }
            Pattern::Wildcard => {}
            Pattern::Tuple(items) => {
                let element_types = match ty {
                    ResolvedType::Tuple(types) => {
                        if types.len() != items.len() {
                            self.errors.push(errors::tuple_unpack_count_mismatch(
                                items.len(),
                                types.len(),
                                pattern.span,
                            ));
                        }
                        types.clone()
                    }
                    _ => vec![ResolvedType::Unknown; items.len()],
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
                for stmt in body {
                    self.check_statement(stmt);
                }
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

        if let Some((name, span)) = pattern.binding {
            if self.symbols.lookup_local(&name).is_some() {
                self.errors.push(errors::duplicate_definition(&name, span));
                return;
            }
            let ty = match pattern.kind {
                AssertIsPatternKind::Some => scrutinee_ty
                    .option_inner_type()
                    .cloned()
                    .unwrap_or(ResolvedType::Unknown),
                AssertIsPatternKind::Ok => scrutinee_ty.result_ok_type().cloned().unwrap_or(ResolvedType::Unknown),
                AssertIsPatternKind::Err => scrutinee_ty.result_err_type().cloned().unwrap_or(ResolvedType::Unknown),
                AssertIsPatternKind::None => ResolvedType::Unit,
            };
            self.symbols.define(Symbol {
                name,
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: false,
                    is_used: false,
                }),
                span,
                scope: 0,
            });
        }
    }

    /// Build an assertion pattern from a parsed pattern.
    fn assert_is_pattern_from_pattern(pattern: &Spanned<Pattern>) -> Option<AssertIsPattern> {
        match &pattern.node {
            Pattern::Constructor(name, args)
                if name == constructors::as_str(ConstructorId::None) && args.is_empty() =>
            {
                Some(AssertIsPattern {
                    kind: AssertIsPatternKind::None,
                    binding: None,
                })
            }
            Pattern::Constructor(name, args) => {
                let kind = match name.as_str() {
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
                Some(AssertIsPattern { kind, binding })
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
