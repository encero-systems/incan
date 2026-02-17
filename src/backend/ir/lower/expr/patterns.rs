//! Pattern and match-arm lowering.

use super::super::super::TypedExpr;
use super::super::super::expr::{IrExprKind, MatchArm, Pattern};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};

impl AstLowering {
    /// Lower match arms to IR.
    ///
    /// # Parameters
    ///
    /// * `arms` - The AST match arms
    ///
    /// # Returns
    ///
    /// A vector of IR match arms.
    pub(in crate::backend::ir::lower) fn lower_match_arms(
        &mut self,
        arms: &[Spanned<ast::MatchArm>],
    ) -> Result<Vec<MatchArm>, LoweringError> {
        arms.iter()
            .map(|a| {
                let pattern = self.lower_pattern(&a.node.pattern.node);
                let guard = a.node.guard.as_ref().map(|g| self.lower_expr(&g.node)).transpose()?;
                let body = match &a.node.body {
                    ast::MatchBody::Expr(e) => self.lower_expr(&e.node)?,
                    ast::MatchBody::Block(stmts) => {
                        let ir_stmts = self.lower_statements(stmts)?;
                        TypedExpr::new(
                            IrExprKind::Block {
                                stmts: ir_stmts,
                                value: None,
                            },
                            IrType::Unit,
                        )
                    }
                };
                Ok(MatchArm { pattern, guard, body })
            })
            .collect()
    }

    /// Lower a pattern to IR.
    ///
    /// Handles wildcard, binding, literal, constructor, and tuple patterns.
    ///
    /// # Parameters
    ///
    /// * `p` - The AST pattern
    ///
    /// # Returns
    ///
    /// The corresponding IR pattern.
    pub(in crate::backend::ir::lower) fn lower_pattern(&mut self, p: &ast::Pattern) -> Pattern {
        match p {
            ast::Pattern::Wildcard => Pattern::Wildcard,
            ast::Pattern::Binding(name) => Pattern::Var(name.clone()),
            ast::Pattern::Literal(lit) => {
                // Lower the literal to an IR expression
                // If lowering fails (unlikely for literals), fall back to wildcard
                self.lower_expr(&ast::Expr::Literal(lit.clone()))
                    .map(Pattern::Literal)
                    .unwrap_or(Pattern::Wildcard)
            }
            ast::Pattern::Constructor(name, args) => {
                let mut named_fields = Vec::new();
                let mut positional_fields = Vec::new();
                let mut has_named = false;

                for arg in args {
                    match arg {
                        ast::PatternArg::Named(field, pat) => {
                            has_named = true;
                            // RFC 021: resolve field alias to canonical name for struct patterns
                            let canonical = self.resolve_field_alias(name, field);
                            named_fields.push((canonical, self.lower_pattern(&pat.node)));
                        }
                        ast::PatternArg::Positional(pat) => {
                            positional_fields.push(self.lower_pattern(&pat.node));
                        }
                    }
                }

                if has_named {
                    Pattern::Struct {
                        name: name.clone(),
                        fields: named_fields,
                    }
                } else {
                    let mut fields = positional_fields;
                    if has_named {
                        fields.extend(named_fields.into_iter().map(|(_, pat)| pat));
                    }
                    Pattern::Enum {
                        name: String::new(),
                        variant: name.clone(),
                        fields,
                    }
                }
            }
            ast::Pattern::Tuple(items) => Pattern::Tuple(items.iter().map(|i| self.lower_pattern(&i.node)).collect()),
        }
    }
}
