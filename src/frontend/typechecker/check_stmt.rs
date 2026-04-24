//! Statement checking: assignments, returns, control flow.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;
use crate::numeric_adapters::{numeric_op_from_ast, numeric_ty_from_resolved};
use incan_core::lang::keywords;
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::{NumericTy, result_numeric_type};
use incan_semantics_core::SurfaceStmtTypeCheck;

use super::{LoopContextKind, TypeChecker};
use crate::frontend::typechecker::helpers::{collection_type_id, ensure_bool_condition};

impl TypeChecker {
    // ========================================================================
    // Statements
    // ========================================================================

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
            Statement::Surface(surface_stmt) => self.check_surface_stmt(surface_stmt, stmt.span),
            Statement::Expr(expr) => {
                self.check_expr(expr);
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
                    let binop = match compound.op {
                        CompoundOp::Add => BinaryOp::Add,
                        CompoundOp::Sub => BinaryOp::Sub,
                        CompoundOp::Mul => BinaryOp::Mul,
                        CompoundOp::Div => BinaryOp::Div,
                        CompoundOp::FloorDiv => BinaryOp::FloorDiv,
                        CompoundOp::Mod => BinaryOp::Mod,
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
                let Some(type_info) = self.lookup_type_info(type_name) else {
                    // Type not found — already reported elsewhere
                    return;
                };

                let field_type = match type_info {
                    TypeInfo::Model(model) => model.fields.get(field).map(|f| f.ty.clone()),
                    TypeInfo::Class(class) => class.fields.get(field).map(|f| f.ty.clone()),
                    _ => None,
                };

                match field_type {
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
                    self.errors.push(errors::not_indexable(&obj_ty.to_string(), span));
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
            _ => {
                self.errors.push(errors::not_indexable(&obj_ty.to_string(), span));
            }
        }
    }

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
                self.errors.push(errors::type_mismatch(
                    &var_ty.to_string(),
                    &value_ty.to_string(),
                    assign.value.span,
                ));
            }
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
            // Check value matches annotation
            if !self.types_compatible(&value_ty, &ann_ty) {
                self.errors.push(errors::type_mismatch(
                    &ann_ty.to_string(),
                    &value_ty.to_string(),
                    assign.value.span,
                ));
            }
            ann_ty
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
    }

    fn check_return(&mut self, expr: Option<&Spanned<Expr>>, span: Span) {
        let return_ty = if let Some(e) = expr {
            let expected_return_ty = self.symbols.current_return_type().cloned();
            self.check_expr_with_expected(e, expected_return_ty.as_ref())
        } else {
            ResolvedType::Unit
        };

        if let Some(expected) = self.symbols.current_return_type()
            && !self.types_compatible(&return_ty, expected)
        {
            self.errors.push(errors::type_mismatch(
                &expected.to_string(),
                &return_ty.to_string(),
                span,
            ));
        }
    }

    fn check_if_stmt(&mut self, if_stmt: &IfStmt) {
        let cond_ty = self.check_expr(&if_stmt.condition);
        let is_compatible = self.types_compatible(&cond_ty, &ResolvedType::Bool);
        ensure_bool_condition(&cond_ty, if_stmt.condition.span, is_compatible, &mut self.errors);

        self.symbols.enter_scope(ScopeKind::Block);
        for stmt in &if_stmt.then_body {
            self.check_statement(stmt);
        }
        self.symbols.exit_scope();

        for (elif_cond, elif_body) in &if_stmt.elif_branches {
            let elif_cond_ty = self.check_expr(elif_cond);
            let elif_is_compatible = self.types_compatible(&elif_cond_ty, &ResolvedType::Bool);
            ensure_bool_condition(&elif_cond_ty, elif_cond.span, elif_is_compatible, &mut self.errors);

            self.symbols.enter_scope(ScopeKind::Block);
            for stmt in elif_body {
                self.check_statement(stmt);
            }
            self.symbols.exit_scope();
        }

        if let Some(else_body) = &if_stmt.else_body {
            self.symbols.enter_scope(ScopeKind::Block);
            for stmt in else_body {
                self.check_statement(stmt);
            }
            self.symbols.exit_scope();
        }
    }

    fn check_while_stmt(&mut self, while_stmt: &WhileStmt) {
        let cond_ty = self.check_expr(&while_stmt.condition);
        let is_compatible = self.types_compatible(&cond_ty, &ResolvedType::Bool);
        ensure_bool_condition(&cond_ty, while_stmt.condition.span, is_compatible, &mut self.errors);

        self.symbols.enter_scope(ScopeKind::Block);
        self.push_loop_context(LoopContextKind::Statement, None);
        for stmt in &while_stmt.body {
            self.check_statement(stmt);
        }
        let _ = self.pop_loop_context();
        self.symbols.exit_scope();
    }

    fn check_loop_stmt(&mut self, loop_stmt: &LoopStmt) {
        self.symbols.enter_scope(ScopeKind::Block);
        self.push_loop_context(LoopContextKind::Statement, None);
        for stmt in &loop_stmt.body {
            self.check_statement(stmt);
        }
        let _ = self.pop_loop_context();
        self.symbols.exit_scope();
    }

    fn check_for_stmt(&mut self, for_stmt: &ForStmt) {
        let iter_ty = self.check_expr(&for_stmt.iter);

        // Infer element type from iterator
        let elem_ty = self.infer_iterator_element_type(&iter_ty);

        self.symbols.enter_scope(ScopeKind::Block);
        self.define_for_pattern_bindings(&for_stmt.pattern, &elem_ty);
        self.push_loop_context(LoopContextKind::Statement, None);

        for stmt in &for_stmt.body {
            self.check_statement(stmt);
        }
        let _ = self.pop_loop_context();
        self.symbols.exit_scope();
    }

    fn check_break_stmt(&mut self, value: Option<&Spanned<Expr>>, span: Span) {
        let Some((loop_kind, expected_break_ty)) = self
            .loop_stack
            .last()
            .map(|ctx| (ctx.kind, ctx.expected_break_ty.clone()))
        else {
            if let Some(value) = value {
                self.check_expr(value);
            }
            self.errors.push(crate::frontend::diagnostics::CompileError::new(
                "`break` is only valid inside loops".into(),
                span,
            ));
            return;
        };

        let break_ty = match (loop_kind, value) {
            (LoopContextKind::Statement, Some(value)) => {
                let value_ty = self.check_expr(value);
                self.errors.push(crate::frontend::diagnostics::CompileError::new(
                    "`break <value>` is only valid inside `loop:` expressions".into(),
                    value.span,
                ));
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

    fn check_continue_stmt(&mut self, span: Span) {
        if self.loop_stack.is_empty() {
            self.errors.push(crate::frontend::diagnostics::CompileError::new(
                "`continue` is only valid inside loops".into(),
                span,
            ));
        }
    }

    /// Define loop-scope bindings introduced by a `for` header pattern.
    ///
    /// The parser currently admits only bindings, `_`, and tuple bindings, but the exhaustive match keeps hand-built
    /// ASTs from silently reaching lowering with unsupported pattern forms.
    fn define_for_pattern_bindings(&mut self, pattern: &Spanned<Pattern>, ty: &ResolvedType) {
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
            Pattern::Constructor(_, _) | Pattern::Literal(_) => {
                self.errors.push(errors::expected_token_message(
                    "Expected identifier, wildcard, or tuple binding in for-loop pattern",
                    &format!("{:?}", pattern.node),
                    pattern.span,
                ));
            }
        }
    }

    fn check_assert_stmt(&mut self, assert_stmt: &AssertStmt) {
        let cond_ty = self.check_expr(&assert_stmt.condition);
        let is_compatible = self.types_compatible(&cond_ty, &ResolvedType::Bool);
        ensure_bool_condition(&cond_ty, assert_stmt.condition.span, is_compatible, &mut self.errors);

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

    /// Typecheck a surface statement via the semantics registry.
    fn check_surface_stmt(&mut self, stmt: &SurfaceStmt, span: Span) {
        use crate::semantics_registry::semantics_registry;

        let Some(action) = semantics_registry().typecheck_surface_stmt_action(&stmt.key) else {
            // No pack claimed this surface statement — report as unknown.
            let label = match &stmt.key {
                incan_semantics_core::SurfaceFeatureKey::SoftKeyword(id) => keywords::as_str(*id).to_string(),
                incan_semantics_core::SurfaceFeatureKey::Decorator(_) => "decorator-surface-feature".to_string(),
            };
            self.errors.push(errors::unknown_symbol(&label, span));
            return;
        };

        match (action, &stmt.payload) {
            (SurfaceStmtTypeCheck::AssertCheck, SurfaceStmtPayload::KeywordArgs(args)) => {
                if let Some(condition) = args.first() {
                    let assert_stmt = AssertStmt {
                        condition: condition.clone(),
                        message: args.get(1).cloned(),
                    };
                    self.check_assert_stmt(&assert_stmt);
                }
            }
        }
    }

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
                    _ => ResolvedType::Unknown,
                }
            }
            ResolvedType::Str => ResolvedType::Str, // String iteration gives chars/strings
            _ => ResolvedType::Unknown,
        }
    }
}
