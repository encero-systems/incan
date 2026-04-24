//! Statement lowering for AST to IR conversion.
//!
//! This module handles lowering of all statement types: let bindings, assignments, control flow (if/while/for), and
//! returns.

use std::collections::HashMap;

use super::super::expr::{IrCallArg, IrExprKind, VarAccess, VarRefKind};
use super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::types::IrType;
use super::super::{IrSpan, Mutability, TypedExpr};
use super::AstLowering;
use super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use incan_semantics_core::SurfaceStmtLoweringAction;

impl AstLowering {
    fn resolve_named_assign_target(&self, name: &str) -> AssignTarget {
        let direct_static = self
            .type_info
            .as_ref()
            .and_then(|info| info.static_binding(name))
            .is_some();

        for (scope_idx, scope) in self.scopes.iter().enumerate().rev() {
            if !scope.contains_key(name) {
                continue;
            }
            if self.is_static_binding(name) {
                return AssignTarget::StaticBinding(name.to_string());
            }
            if scope_idx == 0 && direct_static {
                return AssignTarget::Static(name.to_string());
            }
            return AssignTarget::Var(name.to_string());
        }

        if direct_static {
            AssignTarget::Static(name.to_string())
        } else {
            AssignTarget::Var(name.to_string())
        }
    }

    fn make_static_binding_expr(&self, name: String, ty: IrType) -> TypedExpr {
        TypedExpr::new(IrExprKind::StaticBinding { name }, ty)
    }

    /// Register all loop bindings before lowering the loop body so body reads resolve to local variables.
    fn define_for_pattern_bindings(&mut self, pattern: &ast::Pattern, ty: &IrType) {
        match pattern {
            ast::Pattern::Binding(name) => self.define_local_binding(name.clone(), ty.clone(), false),
            ast::Pattern::Wildcard => {}
            ast::Pattern::Tuple(items) => {
                let element_types = match ty {
                    IrType::Tuple(items) => items.clone(),
                    _ => vec![IrType::Unknown; items.len()],
                };
                for (i, item) in items.iter().enumerate() {
                    let item_ty = element_types.get(i).cloned().unwrap_or(IrType::Unknown);
                    self.define_for_pattern_bindings(&item.node, &item_ty);
                }
            }
            ast::Pattern::Literal(_) | ast::Pattern::Constructor(_, _) => {}
        }
    }

    /// Lower a list of statements to IR.
    ///
    /// # Parameters
    ///
    /// * `stmts` - The AST statements to lower
    ///
    /// # Returns
    ///
    /// A vector of IR statements.
    ///
    /// # Errors
    ///
    /// Returns `LoweringError` if any statement cannot be lowered.
    pub(super) fn lower_statements(&mut self, stmts: &[Spanned<ast::Statement>]) -> Result<Vec<IrStmt>, LoweringError> {
        let mut read_counts = HashMap::new();
        for s in stmts {
            self.count_statement_ident_reads(&s.node, &mut read_counts);
        }
        self.remaining_ident_reads.push(read_counts);

        let lowered = (|| -> Result<Vec<IrStmt>, LoweringError> {
            let mut result = Vec::new();
            for s in stmts {
                let stmt = self.lower_statement(&s.node)?;
                result.push(stmt);
            }
            Ok(result)
        })();

        let _ = self.remaining_ident_reads.pop();
        lowered
    }

    /// Lower a single statement to IR.
    ///
    /// Handles all statement types including:
    /// - Expression statements
    /// - Let bindings (mutable and immutable)
    /// - Assignments (variable, field, index)
    /// - Control flow (if/elif/else, while, for)
    /// - Returns, break, continue, pass
    /// - Compound assignments (+=, -=, etc.)
    /// - Tuple unpacking
    /// - Chained assignments
    ///
    /// # Parameters
    ///
    /// * `stmt` - The AST statement to lower
    ///
    /// # Returns
    ///
    /// The corresponding IR statement.
    ///
    /// # Errors
    ///
    /// Returns `LoweringError` if the statement cannot be lowered.
    pub(super) fn lower_statement(&mut self, stmt: &ast::Statement) -> Result<IrStmt, LoweringError> {
        let kind = match stmt {
            ast::Statement::Expr(e) => IrStmtKind::Expr(self.lower_expr_spanned(e)?),

            ast::Statement::Assignment(a) => {
                let rhs_direct_static = self.is_direct_static_ident(&a.value);
                let lowered_value = self.lower_expr_spanned(&a.value)?;
                let ty =
                    a.ty.as_ref()
                        .map(|t| self.lower_type(&t.node))
                        .unwrap_or_else(|| lowered_value.ty.clone());

                match a.binding {
                    ast::BindingKind::Reassign => {
                        let target = self.resolve_named_assign_target(&a.name);
                        let value = match (&target, rhs_direct_static.clone()) {
                            (AssignTarget::StaticBinding(_), Some(static_name)) => {
                                self.make_static_binding_expr(static_name, ty.clone())
                            }
                            _ => lowered_value.clone(),
                        };
                        return Ok(IrStmt::new(IrStmtKind::Assign { target, value }));
                    }
                    ast::BindingKind::Inferred => {
                        // Check if the variable exists in ANY scope (innermost to outermost).
                        // This allows reassignment of outer scope variables from nested scopes.
                        let var_exists_in_scope = self.scopes.iter().rev().any(|s| s.contains_key(&a.name));

                        if var_exists_in_scope {
                            let target = self.resolve_named_assign_target(&a.name);
                            if matches!(target, AssignTarget::Static(_)) {
                                return Ok(IrStmt::new(IrStmtKind::Assign {
                                    target,
                                    value: lowered_value.clone(),
                                }));
                            }
                            let is_mut = self.mutable_vars.get(&a.name).copied().unwrap_or(false);
                            if is_mut {
                                let value = match (&target, rhs_direct_static.clone()) {
                                    (AssignTarget::StaticBinding(_), Some(static_name)) => {
                                        self.make_static_binding_expr(static_name, ty.clone())
                                    }
                                    _ => lowered_value.clone(),
                                };
                                return Ok(IrStmt::new(IrStmtKind::Assign { target, value }));
                            } else {
                                return Err(LoweringError {
                                    message: format!("Cannot reassign immutable variable '{}'", a.name),
                                    span: IrSpan::default(),
                                });
                            }
                        }
                        if rhs_direct_static.is_some() {
                            self.define_local_binding(a.name.clone(), ty.clone(), true);
                        } else {
                            self.define_local_binding(a.name.clone(), ty.clone(), false);
                        }
                        let value = if let Some(static_name) = rhs_direct_static.clone() {
                            self.make_static_binding_expr(static_name, ty.clone())
                        } else {
                            lowered_value.clone()
                        };
                        // Otherwise, create a new immutable binding in the current scope.
                        IrStmtKind::Let {
                            name: a.name.clone(),
                            ty,
                            mutability: Mutability::Immutable,
                            value,
                        }
                    }
                    ast::BindingKind::Mutable => {
                        // New mutable binding
                        self.mutable_vars.insert(a.name.clone(), true);
                        self.define_local_binding(a.name.clone(), ty.clone(), rhs_direct_static.is_some());
                        let value = if let Some(static_name) = rhs_direct_static.clone() {
                            self.make_static_binding_expr(static_name, ty.clone())
                        } else {
                            lowered_value.clone()
                        };
                        IrStmtKind::Let {
                            name: a.name.clone(),
                            ty,
                            mutability: Mutability::Mutable,
                            value,
                        }
                    }
                    ast::BindingKind::Let => {
                        // New immutable binding
                        self.define_local_binding(a.name.clone(), ty.clone(), rhs_direct_static.is_some());
                        let value = if let Some(static_name) = rhs_direct_static.clone() {
                            self.make_static_binding_expr(static_name, ty.clone())
                        } else {
                            lowered_value
                        };
                        IrStmtKind::Let {
                            name: a.name.clone(),
                            ty,
                            mutability: Mutability::Immutable,
                            value,
                        }
                    }
                }
            }

            ast::Statement::FieldAssignment(fa) => IrStmtKind::Assign {
                target: AssignTarget::Field {
                    object: Box::new(self.lower_expr_spanned(&fa.object)?),
                    field: fa.field.clone(),
                },
                value: self.lower_expr_spanned(&fa.value)?,
            },

            ast::Statement::IndexAssignment(ia) => IrStmtKind::Assign {
                target: AssignTarget::Index {
                    object: Box::new(self.lower_expr_spanned(&ia.object)?),
                    index: Box::new(self.lower_expr_spanned(&ia.index)?),
                },
                value: self.lower_expr_spanned(&ia.value)?,
            },

            ast::Statement::Return(opt) => {
                IrStmtKind::Return(opt.as_ref().map(|e| self.lower_expr_spanned(e)).transpose()?)
            }

            ast::Statement::If(i) => {
                let lowered_if = (|| -> Result<IrStmtKind, LoweringError> {
                    // Lower elif branches as nested if-else in the else branch.
                    // Each branch gets its own scope.
                    let mut else_branch = i
                        .else_body
                        .as_ref()
                        .map(|b| {
                            self.push_scope();
                            let result = self.lower_statements(b);
                            self.pop_scope();
                            result
                        })
                        .transpose()?;

                    // Build elif chain from end to start.
                    for (elif_cond, elif_body) in i.elif_branches.iter().rev() {
                        self.push_scope();
                        let elif_then = self.lower_statements(elif_body)?;
                        self.pop_scope();
                        let elif_stmt = IrStmtKind::If {
                            condition: self.lower_expr_spanned(elif_cond)?,
                            then_branch: elif_then,
                            else_branch,
                        };
                        else_branch = Some(vec![IrStmt::new(elif_stmt)]);
                    }

                    let condition = self.lower_expr_spanned(&i.condition)?;
                    self.push_scope();
                    let then_branch = self.lower_statements(&i.then_body)?;
                    self.pop_scope();

                    Ok(IrStmtKind::If {
                        condition,
                        then_branch,
                        else_branch,
                    })
                })();
                lowered_if?
            }

            ast::Statement::While(w) => {
                // Push a new scope for the while-loop body
                self.push_scope();
                self.non_linear_context_depth += 1;
                let loop_parts = (|| -> Result<(TypedExpr, Vec<IrStmt>), LoweringError> {
                    let condition = self.lower_expr_spanned(&w.condition)?;
                    let body = self.lower_statements(&w.body)?;
                    Ok((condition, body))
                })();
                self.non_linear_context_depth -= 1;
                self.pop_scope();
                let (condition, body) = loop_parts?;
                IrStmtKind::While {
                    label: None,
                    condition,
                    body,
                }
            }

            ast::Statement::Loop(l) => {
                self.push_scope();
                self.non_linear_context_depth += 1;
                let body_result = self.lower_statements(&l.body);
                self.non_linear_context_depth -= 1;
                let body = body_result?;
                self.pop_scope();

                IrStmtKind::Loop { label: None, body }
            }

            ast::Statement::For(f) => {
                // Lower iterable before entering loop scope
                let iterable = self.lower_expr_spanned(&f.iter)?;

                // Push a new scope for the for-loop body
                self.push_scope();

                // Infer loop variable type from iterable and add to scope
                let loop_var_ty = match &iterable.ty {
                    IrType::List(elem) => (**elem).clone(),
                    IrType::Dict(k, _) => (**k).clone(),
                    IrType::String => IrType::String,
                    _ => IrType::Unknown,
                };
                self.define_for_pattern_bindings(&f.pattern.node, &loop_var_ty);

                self.non_linear_context_depth += 1;
                let body_result = self.lower_statements(&f.body);
                self.non_linear_context_depth -= 1;
                let body = body_result?;
                self.pop_scope();

                IrStmtKind::For {
                    label: None,
                    pattern: self.lower_pattern(&f.pattern.node),
                    iterable,
                    body,
                }
            }

            ast::Statement::Surface(surface_stmt) => self.lower_surface_statement(surface_stmt)?,
            ast::Statement::VocabBlock(vocab_block) => {
                return Err(LoweringError {
                    message: format!(
                        "raw vocab block `{}` reached lowering before desugaring",
                        vocab_block.keyword
                    ),
                    span: IrSpan::default(),
                });
            }

            ast::Statement::Pass => IrStmtKind::Expr(TypedExpr::new(IrExprKind::Unit, IrType::Unit)),
            ast::Statement::Break(value) => IrStmtKind::Break {
                label: None,
                value: value.as_ref().map(|value| self.lower_expr_spanned(value)).transpose()?,
            },
            ast::Statement::Continue => IrStmtKind::Continue(None),

            ast::Statement::CompoundAssignment(ca) => {
                // Desugar `x <op>= y` into `x = x <op> y`
                let assign_target = self.resolve_named_assign_target(&ca.name);
                let lhs_ty = self.lookup_var(&ca.name);
                let lhs_expr = match &assign_target {
                    AssignTarget::Static(_) => {
                        TypedExpr::new(IrExprKind::StaticRead { name: ca.name.clone() }, lhs_ty.clone())
                    }
                    AssignTarget::StaticBinding(_) => TypedExpr::new(
                        IrExprKind::Var {
                            name: ca.name.clone(),
                            access: VarAccess::Move,
                            ref_kind: VarRefKind::StaticBinding,
                        },
                        lhs_ty.clone(),
                    ),
                    AssignTarget::Var(_) => TypedExpr::new(
                        IrExprKind::Var {
                            name: ca.name.clone(),
                            access: VarAccess::Move,
                            ref_kind: VarRefKind::Value,
                        },
                        lhs_ty.clone(),
                    ),
                    AssignTarget::Field { .. } | AssignTarget::Index { .. } => unreachable!(),
                };
                let rhs_expr = self.lower_expr_spanned(&ca.value)?;

                // Determine result type using the same policy as binary ops.
                let binop_ast = match ca.op {
                    ast::CompoundOp::Add => ast::BinaryOp::Add,
                    ast::CompoundOp::Sub => ast::BinaryOp::Sub,
                    ast::CompoundOp::Mul => ast::BinaryOp::Mul,
                    ast::CompoundOp::Div => ast::BinaryOp::Div,
                    ast::CompoundOp::FloorDiv => ast::BinaryOp::FloorDiv,
                    ast::CompoundOp::Mod => ast::BinaryOp::Mod,
                };
                let result_ty = self.binary_result_type(&lhs_ty, &rhs_expr.ty, &binop_ast, None);

                let binop_expr = TypedExpr::new(
                    IrExprKind::BinOp {
                        op: self.lower_binop(&binop_ast),
                        left: Box::new(lhs_expr),
                        right: Box::new(rhs_expr),
                    },
                    result_ty,
                );

                IrStmtKind::Assign {
                    target: assign_target,
                    value: binop_expr,
                }
            }

            ast::Statement::TupleUnpack(tu) => {
                let value = self.lower_expr_spanned(&tu.value)?;
                let temp_name = format!("__incan_tuple_unpack_{}", tu.names.join("_"));
                let mutability = match tu.binding {
                    ast::BindingKind::Mutable => Mutability::Mutable,
                    _ => Mutability::Immutable,
                };

                self.define_local_binding(temp_name.clone(), value.ty.clone(), false);

                let mut stmts = vec![IrStmt::new(IrStmtKind::Let {
                    name: temp_name.clone(),
                    ty: value.ty.clone(),
                    mutability: Mutability::Immutable,
                    value,
                })];

                for (idx, name) in tu.names.iter().enumerate() {
                    let field_expr = TypedExpr::new(
                        IrExprKind::Field {
                            object: Box::new(TypedExpr::new(
                                IrExprKind::Var {
                                    name: temp_name.clone(),
                                    access: VarAccess::Copy,
                                    ref_kind: VarRefKind::Value,
                                },
                                self.lookup_var(&temp_name),
                            )),
                            field: idx.to_string(),
                        },
                        IrType::Unknown,
                    );

                    self.define_local_binding(name.clone(), IrType::Unknown, false);
                    if matches!(mutability, Mutability::Mutable) {
                        self.mutable_vars.insert(name.clone(), true);
                    }

                    stmts.push(IrStmt::new(IrStmtKind::Let {
                        name: name.clone(),
                        ty: IrType::Unknown,
                        mutability,
                        value: field_expr,
                    }));
                }

                return Ok(IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                    IrExprKind::Block { stmts, value: None },
                    IrType::Unit,
                ))));
            }

            ast::Statement::TupleAssign(_) => {
                return Err(LoweringError {
                    message: "TupleAssign not yet implemented".to_string(),
                    span: IrSpan::default(),
                });
            }

            ast::Statement::ChainedAssignment(ca) => {
                // Lower chained assignment x = y = z = 5 into:
                // let z = 5; let y = z; let x = y;
                // We return a block expression that does all the assignments
                let value = self.lower_expr_spanned(&ca.value)?;
                let ty = value.ty.clone();

                // Assign to last target first (rightmost)
                let last_target = match ca.targets.last() {
                    Some(t) => t,
                    None => {
                        return Err(LoweringError {
                            message: "empty chained assignment".to_string(),
                            span: IrSpan::default(),
                        });
                    }
                };
                let mutability = match ca.binding {
                    ast::BindingKind::Mutable => Mutability::Mutable,
                    _ => Mutability::Immutable,
                };

                // Record the last target in scope
                self.define_local_binding(last_target.clone(), ty.clone(), false);

                // Create the first assignment statement
                let mut stmts = vec![IrStmt::new(IrStmtKind::Let {
                    name: last_target.clone(),
                    ty: ty.clone(),
                    mutability,
                    value,
                })];

                // Now assign to each previous target from the next one
                for i in (0..ca.targets.len() - 1).rev() {
                    let target = &ca.targets[i];
                    let source = &ca.targets[i + 1];

                    self.define_local_binding(target.clone(), ty.clone(), false);

                    let source_expr = TypedExpr::new(
                        IrExprKind::Var {
                            name: source.clone(),
                            access: if ty.is_copy() { VarAccess::Copy } else { VarAccess::Move },
                            ref_kind: VarRefKind::Value,
                        },
                        ty.clone(),
                    );

                    stmts.push(IrStmt::new(IrStmtKind::Let {
                        name: target.clone(),
                        ty: ty.clone(),
                        mutability,
                        value: source_expr,
                    }));
                }

                // Return a block that does all the assignments and returns unit
                return Ok(IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                    IrExprKind::Block { stmts, value: None },
                    IrType::Unit,
                ))));
            }
        };
        Ok(IrStmt::new(kind))
    }

    /// Lower a surface statement to IR via the semantics registry.
    ///
    /// The registry selects the lowering action; this method executes it.
    fn lower_surface_statement(&mut self, stmt: &ast::SurfaceStmt) -> Result<IrStmtKind, LoweringError> {
        use crate::semantics_registry::semantics_registry;

        let action = semantics_registry()
            .lower_surface_stmt_action(&stmt.key)
            .ok_or_else(|| LoweringError {
                message: format!("no lowering action registered for surface statement {:?}", stmt.key),
                span: IrSpan::default(),
            })?;

        match (action, &stmt.payload) {
            (SurfaceStmtLoweringAction::AssertCall, ast::SurfaceStmtPayload::KeywordArgs(args)) => {
                self.lower_assert_call_surface_stmt(args)
            }
        }
    }

    /// Execute the `AssertCall` lowering action: decompose condition, look up call target, build IR.
    fn lower_assert_call_surface_stmt(&mut self, args: &[Spanned<ast::Expr>]) -> Result<IrStmtKind, LoweringError> {
        let Some(condition_expr) = args.first() else {
            return Err(LoweringError {
                message: "assert surface statement requires a condition".to_string(),
                span: IrSpan::default(),
            });
        };
        let condition = self.lower_expr_spanned(condition_expr)?;
        let message = args.get(1).map(|m| self.lower_expr_spanned(m)).transpose()?;
        let lowered = super::super::surface_semantics::desugar_assert_statement(condition, message);

        let callee = TypedExpr::new(
            IrExprKind::Var {
                name: lowered.local_name.to_string(),
                access: VarAccess::Copy,
                ref_kind: VarRefKind::Value,
            },
            self.lookup_var(lowered.local_name),
        );
        let call_args = lowered
            .args
            .into_iter()
            .map(|expr| IrCallArg { name: None, expr })
            .collect();
        let call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(callee),
                type_args: Vec::new(),
                args: call_args,
                canonical_path: Some(lowered.canonical_path),
            },
            IrType::Unit,
        );
        Ok(IrStmtKind::Expr(call))
    }

    /// Bump the number of ident reads for a given name.
    ///
    /// # Parameters
    ///
    /// * `counts` - The hashmap to count the ident reads
    /// * `name` - The name to bump the ident reads for
    fn bump_ident_read(counts: &mut HashMap<String, usize>, name: &str) {
        let entry = counts.entry(name.to_string()).or_insert(0);
        *entry += 1;
    }

    /// Count the number of ident reads in a list of call arguments.
    ///
    /// # Parameters
    ///
    /// * `args` - The list of call arguments
    /// * `counts` - The hashmap to count the ident reads
    fn count_call_args_ident_reads(&self, args: &[ast::CallArg], counts: &mut HashMap<String, usize>) {
        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) => self.count_expr_ident_reads(&expr.node, counts),
                ast::CallArg::Named(_, expr) => self.count_expr_ident_reads(&expr.node, counts),
            }
        }
    }

    /// Count the number of ident reads in a statement.
    ///
    /// # Parameters
    ///
    /// * `stmt` - The statement to count the ident reads
    /// * `counts` - The hashmap to count the ident reads
    fn count_statement_ident_reads(&self, stmt: &ast::Statement, counts: &mut HashMap<String, usize>) {
        match stmt {
            ast::Statement::Assignment(a) => self.count_expr_ident_reads(&a.value.node, counts),
            ast::Statement::FieldAssignment(fa) => {
                self.count_expr_ident_reads(&fa.object.node, counts);
                self.count_expr_ident_reads(&fa.value.node, counts);
            }
            ast::Statement::IndexAssignment(ia) => {
                self.count_expr_ident_reads(&ia.object.node, counts);
                self.count_expr_ident_reads(&ia.index.node, counts);
                self.count_expr_ident_reads(&ia.value.node, counts);
            }
            ast::Statement::Return(expr) => {
                if let Some(expr) = expr {
                    self.count_expr_ident_reads(&expr.node, counts);
                }
            }
            ast::Statement::If(i) => {
                self.count_expr_ident_reads(&i.condition.node, counts);
                for stmt in &i.then_body {
                    self.count_statement_ident_reads(&stmt.node, counts);
                }
                for (cond, body) in &i.elif_branches {
                    self.count_expr_ident_reads(&cond.node, counts);
                    for stmt in body {
                        self.count_statement_ident_reads(&stmt.node, counts);
                    }
                }
                if let Some(body) = &i.else_body {
                    for stmt in body {
                        self.count_statement_ident_reads(&stmt.node, counts);
                    }
                }
            }
            ast::Statement::While(w) => {
                self.count_expr_ident_reads(&w.condition.node, counts);
                for stmt in &w.body {
                    self.count_statement_ident_reads(&stmt.node, counts);
                }
            }
            ast::Statement::For(f) => {
                self.count_expr_ident_reads(&f.iter.node, counts);
                for stmt in &f.body {
                    self.count_statement_ident_reads(&stmt.node, counts);
                }
            }
            ast::Statement::Loop(l) => {
                for stmt in &l.body {
                    self.count_statement_ident_reads(&stmt.node, counts);
                }
            }
            ast::Statement::Surface(surface_stmt) => match &surface_stmt.payload {
                ast::SurfaceStmtPayload::KeywordArgs(args) => {
                    for arg in args {
                        self.count_expr_ident_reads(&arg.node, counts);
                    }
                }
            },
            ast::Statement::VocabBlock(vocab_block) => {
                for arg in &vocab_block.header_args {
                    self.count_expr_ident_reads(&arg.node, counts);
                }
                for stmt in &vocab_block.body {
                    self.count_statement_ident_reads(&stmt.node, counts);
                }
            }
            ast::Statement::Expr(expr) => self.count_expr_ident_reads(&expr.node, counts),
            ast::Statement::Break(Some(expr)) => self.count_expr_ident_reads(&expr.node, counts),
            ast::Statement::Pass | ast::Statement::Break(None) | ast::Statement::Continue => {}
            ast::Statement::CompoundAssignment(ca) => {
                Self::bump_ident_read(counts, &ca.name);
                self.count_expr_ident_reads(&ca.value.node, counts);
            }
            ast::Statement::TupleUnpack(tu) => self.count_expr_ident_reads(&tu.value.node, counts),
            ast::Statement::TupleAssign(ta) => {
                for target in &ta.targets {
                    self.count_expr_ident_reads(&target.node, counts);
                }
                self.count_expr_ident_reads(&ta.value.node, counts);
            }
            ast::Statement::ChainedAssignment(ca) => self.count_expr_ident_reads(&ca.value.node, counts),
        }
    }

    fn count_expr_ident_reads(&self, expr: &ast::Expr, counts: &mut HashMap<String, usize>) {
        match expr {
            ast::Expr::Ident(name) => Self::bump_ident_read(counts, name),
            ast::Expr::Literal(_) | ast::Expr::SelfExpr => {}
            ast::Expr::Binary(left, _, right) => {
                self.count_expr_ident_reads(&left.node, counts);
                self.count_expr_ident_reads(&right.node, counts);
            }
            ast::Expr::Unary(_, inner) => self.count_expr_ident_reads(&inner.node, counts),
            ast::Expr::Call(func, _type_args, args) => {
                self.count_expr_ident_reads(&func.node, counts);
                self.count_call_args_ident_reads(args, counts);
            }
            ast::Expr::Index(object, index) => {
                self.count_expr_ident_reads(&object.node, counts);
                self.count_expr_ident_reads(&index.node, counts);
            }
            ast::Expr::Slice(target, slice) => {
                self.count_expr_ident_reads(&target.node, counts);
                if let Some(start) = &slice.start {
                    self.count_expr_ident_reads(&start.node, counts);
                }
                if let Some(end) = &slice.end {
                    self.count_expr_ident_reads(&end.node, counts);
                }
                if let Some(step) = &slice.step {
                    self.count_expr_ident_reads(&step.node, counts);
                }
            }
            ast::Expr::Field(object, _) => self.count_expr_ident_reads(&object.node, counts),
            ast::Expr::MethodCall(receiver, _, _type_args, args) => {
                self.count_expr_ident_reads(&receiver.node, counts);
                self.count_call_args_ident_reads(args, counts);
            }
            ast::Expr::Try(inner) | ast::Expr::Paren(inner) => {
                self.count_expr_ident_reads(&inner.node, counts);
            }
            ast::Expr::Surface(surface_expr) => match &surface_expr.payload {
                ast::SurfaceExprPayload::PrefixUnary(inner) => self.count_expr_ident_reads(&inner.node, counts),
            },
            ast::Expr::Match(scrutinee, arms) => {
                self.count_expr_ident_reads(&scrutinee.node, counts);
                for arm in arms {
                    if let Some(guard) = &arm.node.guard {
                        self.count_expr_ident_reads(&guard.node, counts);
                    }
                    match &arm.node.body {
                        ast::MatchBody::Expr(expr) => self.count_expr_ident_reads(&expr.node, counts),
                        ast::MatchBody::Block(stmts) => {
                            for stmt in stmts {
                                self.count_statement_ident_reads(&stmt.node, counts);
                            }
                        }
                    }
                }
            }
            ast::Expr::If(if_expr) => {
                self.count_expr_ident_reads(&if_expr.condition.node, counts);
                for stmt in &if_expr.then_body {
                    self.count_statement_ident_reads(&stmt.node, counts);
                }
                if let Some(else_body) = &if_expr.else_body {
                    for stmt in else_body {
                        self.count_statement_ident_reads(&stmt.node, counts);
                    }
                }
            }
            ast::Expr::Loop(loop_expr) => {
                for stmt in &loop_expr.body {
                    self.count_statement_ident_reads(&stmt.node, counts);
                }
            }
            ast::Expr::ListComp(comp) => {
                self.count_expr_ident_reads(&comp.iter.node, counts);
                self.count_expr_ident_reads(&comp.expr.node, counts);
                if let Some(filter) = &comp.filter {
                    self.count_expr_ident_reads(&filter.node, counts);
                }
            }
            ast::Expr::DictComp(comp) => {
                self.count_expr_ident_reads(&comp.iter.node, counts);
                self.count_expr_ident_reads(&comp.key.node, counts);
                self.count_expr_ident_reads(&comp.value.node, counts);
                if let Some(filter) = &comp.filter {
                    self.count_expr_ident_reads(&filter.node, counts);
                }
            }
            ast::Expr::Closure(_, body) => self.count_expr_ident_reads(&body.node, counts),
            ast::Expr::Tuple(items) | ast::Expr::List(items) | ast::Expr::Set(items) => {
                for item in items {
                    self.count_expr_ident_reads(&item.node, counts);
                }
            }
            ast::Expr::Dict(pairs) => {
                for (key, value) in pairs {
                    self.count_expr_ident_reads(&key.node, counts);
                    self.count_expr_ident_reads(&value.node, counts);
                }
            }
            ast::Expr::Constructor(_, args) => self.count_call_args_ident_reads(args, counts),
            ast::Expr::FString(parts) => {
                for part in parts {
                    if let ast::FStringPart::Expr(expr) = part {
                        self.count_expr_ident_reads(&expr.node, counts);
                    }
                }
            }
            ast::Expr::Yield(expr) => {
                if let Some(expr) = expr {
                    self.count_expr_ident_reads(&expr.node, counts);
                }
            }
            ast::Expr::Range { start, end, .. } => {
                self.count_expr_ident_reads(&start.node, counts);
                self.count_expr_ident_reads(&end.node, counts);
            }
        }
    }
}
