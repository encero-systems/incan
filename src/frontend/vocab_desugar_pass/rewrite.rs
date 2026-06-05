use crate::frontend::ast;
use crate::frontend::diagnostics::CompileError;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::vocab_ast_bridge::{
    internal_vocab_block_to_public, public_expr_to_internal_with_anchor, public_statements_to_internal_with_anchor,
};

use super::helper_bindings::{
    HelperImportAccumulator, inject_helper_imports, resolve_helper_bindings_in_expr,
    resolve_helper_bindings_in_statements,
};
use super::{VocabDesugarPassError, WasmDesugarerRuntime};

/// Rewrite all raw vocab blocks in a parsed program before typechecking/lowering.
///
/// This pass is the hard boundary ensuring downstream phases operate only on ordinary compiler statements, never
/// `Statement::VocabBlock`.
///
/// # Errors
///
/// Returns compiler diagnostics when:
/// - AST bridge mapping fails,
/// - desugarer artifact resolution/loading fails,
/// - WASM runtime execution fails, or
/// - desugarer output cannot be decoded/mapped back into internal AST.
pub fn desugar_program_vocab_blocks(
    program: &mut ast::Program,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
) -> Result<(), Vec<CompileError>> {
    let mut runtime = match WasmDesugarerRuntime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            return Err(vec![CompileError::new(
                format!("failed to initialize vocab wasm runtime: {err}"),
                ast::Span::default(),
            )]);
        }
    };
    let mut errors = Vec::new();
    let mut helper_imports = HelperImportAccumulator::default();

    for declaration in &mut program.declarations {
        rewrite_declaration(
            &mut declaration.node,
            module_path,
            library_manifest_index,
            &mut runtime,
            &mut helper_imports,
            &mut errors,
        );
    }

    if errors.is_empty() {
        inject_helper_imports(program, &helper_imports);
        Ok(())
    } else {
        Err(errors)
    }
}

/// Rewrite vocab blocks inside every expression-bearing surface of one declaration.
fn rewrite_declaration(
    declaration: &mut ast::Declaration,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    match declaration {
        ast::Declaration::Const(konst) => {
            rewrite_spanned_expr(
                &mut konst.value,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Declaration::Static(static_decl) => rewrite_spanned_expr(
            &mut static_decl.value,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
            errors,
        ),
        ast::Declaration::Partial(partial) => {
            rewrite_partial_args(
                &mut partial.args,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Declaration::Function(function) => {
            rewrite_statement_list(
                &mut function.body,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Declaration::Model(model) => {
            rewrite_field_defaults(
                &mut model.fields,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_method_partial_args(
                &mut model.method_partials,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            for property in &mut model.properties {
                if let Some(body) = property.node.body.as_mut() {
                    rewrite_statement_list(
                        body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
            }
            for method in &mut model.methods {
                if let Some(body) = method.node.body.as_mut() {
                    rewrite_statement_list(
                        body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
            }
        }
        ast::Declaration::Class(class) => {
            rewrite_field_defaults(
                &mut class.fields,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_method_partial_args(
                &mut class.method_partials,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            for property in &mut class.properties {
                if let Some(body) = property.node.body.as_mut() {
                    rewrite_statement_list(
                        body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
            }
            for method in &mut class.methods {
                if let Some(body) = method.node.body.as_mut() {
                    rewrite_statement_list(
                        body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
            }
        }
        ast::Declaration::Trait(trait_decl) => {
            rewrite_method_partial_args(
                &mut trait_decl.method_partials,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            for property in &mut trait_decl.properties {
                if let Some(body) = property.node.body.as_mut() {
                    rewrite_statement_list(
                        body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
            }
            for method in &mut trait_decl.methods {
                if let Some(body) = method.node.body.as_mut() {
                    rewrite_statement_list(
                        body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
            }
        }
        ast::Declaration::Newtype(newtype_decl) => {
            rewrite_method_partial_args(
                &mut newtype_decl.method_partials,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            for method in &mut newtype_decl.methods {
                if let Some(body) = method.node.body.as_mut() {
                    rewrite_statement_list(
                        body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
            }
        }
        ast::Declaration::TestModule(test_module) => {
            for nested in &mut test_module.body {
                rewrite_declaration(
                    &mut nested.node,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
        }
        _ => {}
    }
}

/// Rewrite vocab expressions in model or class field default values.
fn rewrite_field_defaults(
    fields: &mut [ast::Spanned<ast::FieldDecl>],
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    for field in fields {
        if let Some(default) = field.node.default.as_mut() {
            rewrite_spanned_expr(
                default,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
    }
}

/// Rewrite vocab expressions inside method-level partial preset arguments.
fn rewrite_method_partial_args(
    partials: &mut [ast::Spanned<ast::MethodPartialDecl>],
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    for partial in partials {
        rewrite_partial_args(
            &mut partial.node.args,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
            errors,
        );
    }
}

/// Rewrite vocab expressions inside one partial preset argument list.
fn rewrite_partial_args(
    args: &mut [ast::PartialArg],
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    for arg in args {
        rewrite_spanned_expr(
            &mut arg.value,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
            errors,
        );
    }
}

/// Recursively rewrite a statement list so no raw vocab block statement or expression nodes survive past this pass.
///
/// The recursion matters because desugarers may emit statements that themselves still contain nested control-flow
/// bodies, and those bodies may contain additional vocab blocks introduced earlier by parsing.
fn rewrite_statement_list(
    statements: &mut Vec<ast::Spanned<ast::Statement>>,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    let mut rewritten = Vec::new();

    for statement in statements.drain(..) {
        let span = statement.span;
        match statement.node {
            ast::Statement::VocabBlock(block) => {
                // ---- Context: bridge one raw DSL block into the public vocab AST ----
                let bridged = internal_vocab_block_to_public(&block, span);
                let bridged = match bridged {
                    Ok(value) => value,
                    Err(source) => {
                        errors.push(error_from_pass_error(
                            VocabDesugarPassError::Bridge {
                                keyword: block.keyword.clone(),
                                source,
                            },
                            span,
                        ));
                        continue;
                    }
                };

                let bridged_keyword = bridged.keyword.clone();
                let bridged_keyword_metadata = bridged.keyword_metadata.clone();
                let request_node = incan_vocab::VocabSyntaxNode::Declaration(bridged);
                let desugared = runtime.desugar_node(library_manifest_index, &request_node, module_path);
                let desugared = match desugared {
                    Ok(value) => value,
                    Err(err) => {
                        errors.push(error_from_pass_error(err, span));
                        continue;
                    }
                };

                let public_statements = match desugared.output {
                    incan_vocab::DesugarOutput::Statements(statements) => statements,
                    incan_vocab::DesugarOutput::Expression(expression) => {
                        vec![incan_vocab::IncanStatement::Expr(expression)]
                    }
                    _ => {
                        errors.push(error_from_pass_error(
                            VocabDesugarPassError::UnsupportedOutput {
                                keyword: bridged_keyword.clone(),
                            },
                            span,
                        ));
                        continue;
                    }
                };
                let mut public_statements = public_statements;
                if let Err(message) = resolve_helper_bindings_in_statements(
                    &mut public_statements,
                    bridged_keyword_metadata.as_ref(),
                    &bridged_keyword,
                    library_manifest_index,
                    helper_imports,
                ) {
                    errors.push(error_from_pass_error(
                        VocabDesugarPassError::HelperBinding {
                            keyword: bridged_keyword.clone(),
                            message,
                        },
                        span,
                    ));
                    continue;
                }

                let mut lowered = match public_statements_to_internal_with_anchor(&public_statements, span) {
                    Ok(stmts) => stmts,
                    Err(source) => {
                        errors.push(error_from_pass_error(
                            VocabDesugarPassError::Bridge {
                                keyword: bridged_keyword.clone(),
                                source,
                            },
                            span,
                        ));
                        continue;
                    }
                };
                for lowered_statement in &mut lowered {
                    lowered_statement.span = span;
                }
                rewrite_statement_list(
                    &mut lowered,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.extend(lowered);
            }
            ast::Statement::Assignment(mut assignment) => {
                rewrite_spanned_expr(
                    &mut assignment.value,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::Assignment(assignment), span));
            }
            ast::Statement::FieldAssignment(mut assignment) => {
                rewrite_spanned_expr(
                    &mut assignment.object,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewrite_spanned_expr(
                    &mut assignment.value,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::FieldAssignment(assignment), span));
            }
            ast::Statement::IndexAssignment(mut assignment) => {
                rewrite_spanned_expr(
                    &mut assignment.object,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewrite_spanned_expr(
                    &mut assignment.index,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewrite_spanned_expr(
                    &mut assignment.value,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::IndexAssignment(assignment), span));
            }
            ast::Statement::Return(mut expr) => {
                if let Some(expr) = expr.as_mut() {
                    rewrite_spanned_expr(
                        expr,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
                rewritten.push(ast::Spanned::new(ast::Statement::Return(expr), span));
            }
            ast::Statement::If(mut if_stmt) => {
                // ---- Context: recurse into ordinary control-flow bodies ----
                rewrite_condition_exprs(
                    &mut if_stmt.condition,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewrite_statement_list(
                    &mut if_stmt.then_body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                for (elif_condition, elif_body) in &mut if_stmt.elif_branches {
                    rewrite_spanned_expr(
                        elif_condition,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                    rewrite_statement_list(
                        elif_body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
                if let Some(else_body) = if_stmt.else_body.as_mut() {
                    rewrite_statement_list(
                        else_body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
                rewritten.push(ast::Spanned::new(ast::Statement::If(if_stmt), span));
            }
            ast::Statement::While(mut while_stmt) => {
                rewrite_condition_exprs(
                    &mut while_stmt.condition,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewrite_statement_list(
                    &mut while_stmt.body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::While(while_stmt), span));
            }
            ast::Statement::Loop(mut loop_stmt) => {
                rewrite_statement_list(
                    &mut loop_stmt.body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::Loop(loop_stmt), span));
            }
            ast::Statement::For(mut for_stmt) => {
                rewrite_spanned_expr(
                    &mut for_stmt.iter,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewrite_statement_list(
                    &mut for_stmt.body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::For(for_stmt), span));
            }
            ast::Statement::Expr(mut expr) => {
                rewrite_spanned_expr(
                    &mut expr,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::Expr(expr), span));
            }
            ast::Statement::VocabExpressionItem(mut item) => {
                rewrite_spanned_expr(
                    &mut item.expr,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                for modifier in &mut item.modifiers {
                    rewrite_spanned_expr(
                        &mut modifier.value,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
                rewritten.push(ast::Spanned::new(ast::Statement::VocabExpressionItem(item), span));
            }
            ast::Statement::Assert(mut assert_stmt) => {
                rewrite_assert_exprs(
                    &mut assert_stmt,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::Assert(assert_stmt), span));
            }
            ast::Statement::Break(mut expr) => {
                if let Some(expr) = expr.as_mut() {
                    rewrite_spanned_expr(
                        expr,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
                rewritten.push(ast::Spanned::new(ast::Statement::Break(expr), span));
            }
            ast::Statement::CompoundAssignment(mut assignment) => {
                rewrite_spanned_expr(
                    &mut assignment.value,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::CompoundAssignment(assignment), span));
            }
            ast::Statement::TupleUnpack(mut assignment) => {
                rewrite_spanned_expr(
                    &mut assignment.value,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::TupleUnpack(assignment), span));
            }
            ast::Statement::TupleAssign(mut assignment) => {
                for target in &mut assignment.targets {
                    rewrite_spanned_expr(
                        target,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
                rewrite_spanned_expr(
                    &mut assignment.value,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::TupleAssign(assignment), span));
            }
            ast::Statement::ChainedAssignment(mut assignment) => {
                rewrite_spanned_expr(
                    &mut assignment.value,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                rewritten.push(ast::Spanned::new(ast::Statement::ChainedAssignment(assignment), span));
            }
            other => rewritten.push(ast::Spanned::new(other, span)),
        }
    }

    *statements = rewritten;
}

/// Rewrite vocab expressions inside conditional expression wrappers.
fn rewrite_condition_exprs(
    condition: &mut ast::Condition,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    match condition {
        ast::Condition::Expr(expr) | ast::Condition::Let { value: expr, .. } => rewrite_spanned_expr(
            expr,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
            errors,
        ),
    }
}

/// Rewrite vocab expressions inside all assert statement payloads.
fn rewrite_assert_exprs(
    assert_stmt: &mut ast::AssertStmt,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    match &mut assert_stmt.kind {
        ast::AssertKind::Condition(expr) => rewrite_spanned_expr(
            expr,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
            errors,
        ),
        ast::AssertKind::IsPattern { value, .. } => rewrite_spanned_expr(
            value,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
            errors,
        ),
        ast::AssertKind::Raises { call, .. } => rewrite_spanned_expr(
            call,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
            errors,
        ),
    }
    if let Some(message) = assert_stmt.message.as_mut() {
        rewrite_spanned_expr(
            message,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
            errors,
        );
    }
}

/// Recursively rewrite raw vocab expression declarations nested inside ordinary expressions.
fn rewrite_spanned_expr(
    expr: &mut ast::Spanned<ast::Expr>,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    if matches!(expr.node, ast::Expr::VocabBlock(_)) {
        let placeholder = ast::Expr::Literal(ast::Literal::None);
        let ast::Expr::VocabBlock(block) = std::mem::replace(&mut expr.node, placeholder) else {
            unreachable!("raw vocab expression was checked immediately before replacement");
        };
        match desugar_vocab_block_to_expression(
            &block,
            expr.span,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
        ) {
            Ok(node) => expr.node = node,
            Err(err) => {
                errors.push(err);
                return;
            }
        }
    }

    match &mut expr.node {
        ast::Expr::Binary(left, _, right) => {
            rewrite_spanned_expr(
                left,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_spanned_expr(
                right,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::Unary(_, inner)
        | ast::Expr::Try(inner)
        | ast::Expr::Paren(inner)
        | ast::Expr::Yield(Some(inner)) => {
            rewrite_spanned_expr(
                inner,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::Call(callee, _, args) => {
            rewrite_spanned_expr(
                callee,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_call_args(
                args,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::Index(base, index) => {
            rewrite_spanned_expr(
                base,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_spanned_expr(
                index,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::Slice(base, slice) => {
            rewrite_spanned_expr(
                base,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            if let Some(start) = slice.start.as_mut() {
                rewrite_spanned_expr(
                    start,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
            if let Some(end) = slice.end.as_mut() {
                rewrite_spanned_expr(
                    end,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
            if let Some(step) = slice.step.as_mut() {
                rewrite_spanned_expr(
                    step,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
        }
        ast::Expr::Field(base, _) => {
            rewrite_spanned_expr(
                base,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::MethodCall(receiver, _, _, args) => {
            rewrite_spanned_expr(
                receiver,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_call_args(
                args,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::Partial(partial) => {
            rewrite_spanned_expr(
                &mut partial.target,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            for arg in &mut partial.args {
                rewrite_spanned_expr(
                    &mut arg.value,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
        }
        ast::Expr::Match(scrutinee, arms) => {
            rewrite_spanned_expr(
                scrutinee,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            for arm in arms {
                if let Some(guard) = arm.node.guard.as_mut() {
                    rewrite_spanned_expr(
                        guard,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
                match &mut arm.node.body {
                    ast::MatchBody::Expr(body) => rewrite_spanned_expr(
                        body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    ),
                    ast::MatchBody::Block(body) => rewrite_statement_list(
                        body,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    ),
                }
            }
        }
        ast::Expr::If(if_expr) => {
            rewrite_spanned_expr(
                &mut if_expr.condition,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_statement_list(
                &mut if_expr.then_body,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            if let Some(else_body) = if_expr.else_body.as_mut() {
                rewrite_statement_list(
                    else_body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
        }
        ast::Expr::Loop(loop_expr) => {
            rewrite_statement_list(
                &mut loop_expr.body,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::ListComp(comp) => {
            rewrite_spanned_expr(
                &mut comp.expr,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_spanned_expr(
                &mut comp.iter,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            if let Some(filter) = comp.filter.as_mut() {
                rewrite_spanned_expr(
                    filter,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
            rewrite_comprehension_clauses(
                &mut comp.clauses,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::DictComp(comp) => {
            rewrite_spanned_expr(
                &mut comp.key,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_spanned_expr(
                &mut comp.value,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_spanned_expr(
                &mut comp.iter,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            if let Some(filter) = comp.filter.as_mut() {
                rewrite_spanned_expr(
                    filter,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
            rewrite_comprehension_clauses(
                &mut comp.clauses,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::Generator(generator) => {
            rewrite_spanned_expr(
                &mut generator.expr,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_comprehension_clauses(
                &mut generator.clauses,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::Closure(_, body) => {
            rewrite_spanned_expr(
                body,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::Tuple(items) | ast::Expr::Set(items) => {
            for item in items {
                rewrite_spanned_expr(
                    item,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
        }
        ast::Expr::List(items) => {
            for item in items {
                match item {
                    ast::ListEntry::Element(expr) | ast::ListEntry::Spread(expr) => {
                        rewrite_spanned_expr(
                            expr,
                            module_path,
                            library_manifest_index,
                            runtime,
                            helper_imports,
                            errors,
                        );
                    }
                }
            }
        }
        ast::Expr::Dict(entries) => {
            for entry in entries {
                match entry {
                    ast::DictEntry::Pair(key, value) => {
                        rewrite_spanned_expr(
                            key,
                            module_path,
                            library_manifest_index,
                            runtime,
                            helper_imports,
                            errors,
                        );
                        rewrite_spanned_expr(
                            value,
                            module_path,
                            library_manifest_index,
                            runtime,
                            helper_imports,
                            errors,
                        );
                    }
                    ast::DictEntry::Spread(value) => {
                        rewrite_spanned_expr(
                            value,
                            module_path,
                            library_manifest_index,
                            runtime,
                            helper_imports,
                            errors,
                        );
                    }
                }
            }
        }
        ast::Expr::Constructor(_, args) => {
            rewrite_call_args(
                args,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::FString(parts) => {
            for part in parts {
                if let ast::FStringPart::Expr { expr, .. } = part {
                    rewrite_spanned_expr(
                        expr,
                        module_path,
                        library_manifest_index,
                        runtime,
                        helper_imports,
                        errors,
                    );
                }
            }
        }
        ast::Expr::Range { start, end, .. } => {
            rewrite_spanned_expr(
                start,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_spanned_expr(
                end,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::Expr::Surface(surface) => rewrite_surface_expr(
            surface,
            module_path,
            library_manifest_index,
            runtime,
            helper_imports,
            errors,
        ),
        ast::Expr::Ident(_)
        | ast::Expr::Literal(_)
        | ast::Expr::SelfExpr
        | ast::Expr::Yield(None)
        | ast::Expr::VocabBlock(_) => {}
    }
}

/// Rewrite vocab expressions inside positional, named, and unpacked call arguments.
fn rewrite_call_args(
    args: &mut [ast::CallArg],
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    for arg in args {
        match arg {
            ast::CallArg::Positional(expr)
            | ast::CallArg::Named(_, expr)
            | ast::CallArg::PositionalUnpack(expr)
            | ast::CallArg::KeywordUnpack(expr) => {
                rewrite_spanned_expr(
                    expr,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
        }
    }
}

/// Rewrite vocab expressions inside comprehension iterator and filter clauses.
fn rewrite_comprehension_clauses(
    clauses: &mut [ast::ComprehensionClause],
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    for clause in clauses {
        match clause {
            ast::ComprehensionClause::For { iter, .. } | ast::ComprehensionClause::If(iter) => {
                rewrite_spanned_expr(
                    iter,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
            }
        }
    }
}

/// Rewrite nested expressions held by non-core surface-expression payloads.
fn rewrite_surface_expr(
    surface: &mut ast::SurfaceExpr,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
    errors: &mut Vec<CompileError>,
) {
    match &mut surface.payload {
        ast::SurfaceExprPayload::PrefixUnary(inner) => {
            rewrite_spanned_expr(
                inner,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::SurfaceExprPayload::RaceFor(race) => {
            for arm in &mut race.arms {
                rewrite_spanned_expr(
                    &mut arm.awaitable,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                match &mut arm.body {
                    ast::RaceForBody::Expr(expr) => {
                        rewrite_spanned_expr(
                            expr,
                            module_path,
                            library_manifest_index,
                            runtime,
                            helper_imports,
                            errors,
                        );
                    }
                    ast::RaceForBody::Block(body) => {
                        rewrite_statement_list(
                            body,
                            module_path,
                            library_manifest_index,
                            runtime,
                            helper_imports,
                            errors,
                        );
                    }
                }
            }
        }
        ast::SurfaceExprPayload::LeadingDotPath { .. } => {}
        ast::SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
            rewrite_spanned_expr(
                left,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
            rewrite_spanned_expr(
                right,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
        ast::SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
            rewrite_call_args(
                args,
                module_path,
                library_manifest_index,
                runtime,
                helper_imports,
                errors,
            );
        }
    }
}

/// Desugar one expression-position vocab block and convert the expression result back to compiler AST.
fn desugar_vocab_block_to_expression(
    block: &ast::VocabBlockStmt,
    span: ast::Span,
    module_path: Option<&str>,
    library_manifest_index: &LibraryManifestIndex,
    runtime: &mut WasmDesugarerRuntime,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<ast::Expr, CompileError> {
    let bridged = internal_vocab_block_to_public(block, span).map_err(|source| {
        error_from_pass_error(
            VocabDesugarPassError::Bridge {
                keyword: block.keyword.clone(),
                source,
            },
            span,
        )
    })?;
    let bridged_keyword = bridged.keyword.clone();
    let bridged_keyword_metadata = bridged.keyword_metadata.clone();
    let request_node = incan_vocab::VocabSyntaxNode::Declaration(bridged);
    let mut desugared = runtime
        .desugar_node(library_manifest_index, &request_node, module_path)
        .map_err(|err| error_from_pass_error(err, span))?;

    let incan_vocab::DesugarOutput::Expression(expression) = &mut desugared.output else {
        return Err(error_from_pass_error(
            VocabDesugarPassError::UnsupportedOutput {
                keyword: bridged_keyword,
            },
            span,
        ));
    };

    resolve_helper_bindings_in_expr(
        expression,
        bridged_keyword_metadata.as_ref(),
        &bridged_keyword,
        library_manifest_index,
        helper_imports,
    )
    .map_err(|message| {
        error_from_pass_error(
            VocabDesugarPassError::HelperBinding {
                keyword: bridged_keyword.clone(),
                message,
            },
            span,
        )
    })?;

    public_expr_to_internal_with_anchor(expression, span).map_err(|source| {
        error_from_pass_error(
            VocabDesugarPassError::Bridge {
                keyword: bridged_keyword,
                source,
            },
            span,
        )
    })
}

/// Map a pass/runtime error into a compiler diagnostic.
///
/// These are infrastructure failures (artifact I/O, WASM compilation, checksum mismatches), not semantic type errors,
/// so we use the generic `Error` kind rather than `Type`.
fn error_from_pass_error(error: VocabDesugarPassError, fallback_span: ast::Span) -> CompileError {
    CompileError::new(format!("vocab desugar pass failed: {error}"), fallback_span)
}
