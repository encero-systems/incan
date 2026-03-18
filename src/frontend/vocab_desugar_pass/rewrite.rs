use crate::frontend::ast;
use crate::frontend::diagnostics::CompileError;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::vocab_ast_bridge::{internal_vocab_block_to_public, public_statements_to_internal};

use super::helper_bindings::{HelperImportAccumulator, inject_helper_imports, resolve_helper_bindings_in_statements};
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
        match &mut declaration.node {
            ast::Declaration::Function(function) => rewrite_statement_list(
                &mut function.body,
                module_path,
                library_manifest_index,
                &mut runtime,
                &mut helper_imports,
                &mut errors,
            ),
            ast::Declaration::Model(model) => {
                for method in &mut model.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(
                            body,
                            module_path,
                            library_manifest_index,
                            &mut runtime,
                            &mut helper_imports,
                            &mut errors,
                        );
                    }
                }
            }
            ast::Declaration::Class(class) => {
                for method in &mut class.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(
                            body,
                            module_path,
                            library_manifest_index,
                            &mut runtime,
                            &mut helper_imports,
                            &mut errors,
                        );
                    }
                }
            }
            ast::Declaration::Trait(trait_decl) => {
                for method in &mut trait_decl.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(
                            body,
                            module_path,
                            library_manifest_index,
                            &mut runtime,
                            &mut helper_imports,
                            &mut errors,
                        );
                    }
                }
            }
            ast::Declaration::Newtype(newtype_decl) => {
                for method in &mut newtype_decl.methods {
                    if let Some(body) = method.node.body.as_mut() {
                        rewrite_statement_list(
                            body,
                            module_path,
                            library_manifest_index,
                            &mut runtime,
                            &mut helper_imports,
                            &mut errors,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    if errors.is_empty() {
        inject_helper_imports(program, &helper_imports);
        Ok(())
    } else {
        Err(errors)
    }
}

/// Recursively rewrite a statement list so no `Statement::VocabBlock` nodes survive past this pass.
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

                let request_node = incan_vocab::VocabSyntaxNode::Declaration(bridged.clone());
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
                                keyword: bridged.keyword.clone(),
                            },
                            span,
                        ));
                        continue;
                    }
                };
                let mut public_statements = public_statements;
                if let Err(message) = resolve_helper_bindings_in_statements(
                    &mut public_statements,
                    bridged.keyword_metadata.as_ref(),
                    &bridged.keyword,
                    library_manifest_index,
                    helper_imports,
                ) {
                    errors.push(error_from_pass_error(
                        VocabDesugarPassError::HelperBinding {
                            keyword: bridged.keyword.clone(),
                            message,
                        },
                        span,
                    ));
                    continue;
                }

                let mut lowered = match public_statements_to_internal(&public_statements) {
                    Ok(stmts) => stmts,
                    Err(source) => {
                        errors.push(error_from_pass_error(
                            VocabDesugarPassError::Bridge {
                                keyword: bridged.keyword.clone(),
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
            ast::Statement::If(mut if_stmt) => {
                // ---- Context: recurse into ordinary control-flow bodies ----
                rewrite_statement_list(
                    &mut if_stmt.then_body,
                    module_path,
                    library_manifest_index,
                    runtime,
                    helper_imports,
                    errors,
                );
                for (_, elif_body) in &mut if_stmt.elif_branches {
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
            ast::Statement::For(mut for_stmt) => {
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
            other => rewritten.push(ast::Spanned::new(other, span)),
        }
    }

    *statements = rewritten;
}

/// Map a pass/runtime error into a compiler diagnostic.
///
/// These are infrastructure failures (artifact I/O, WASM compilation, checksum mismatches), not semantic type errors,
/// so we use the generic `Error` kind rather than `Type`.
fn error_from_pass_error(error: VocabDesugarPassError, fallback_span: ast::Span) -> CompileError {
    CompileError::new(format!("vocab desugar pass failed: {error}"), fallback_span)
}
