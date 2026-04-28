//! Pattern and match-arm lowering.

use super::super::super::TypedExpr;
use super::super::super::expr::{IrExprKind, MatchArm, Pattern};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use incan_core::lang::surface::constructors::{self, ConstructorId};

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
        scrutinee_ty: &IrType,
    ) -> Result<Vec<MatchArm>, LoweringError> {
        arms.iter()
            .map(|a| {
                let pattern = self.lower_pattern_for_expected_type(&a.node.pattern.node, scrutinee_ty);
                let guard = a.node.guard.as_ref().map(|g| self.lower_expr_spanned(g)).transpose()?;
                let body = match &a.node.body {
                    ast::MatchBody::Expr(e) => self.lower_expr_spanned(e)?,
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

    /// Lower the type name used by a union type pattern.
    fn lower_type_pattern_name(&self, name: &str) -> IrType {
        self.lower_type(&ast::Type::Simple(name.to_string()))
    }

    /// Lower a pattern with enough scrutinee type context to rewrite union type patterns.
    fn lower_pattern_for_expected_type(&mut self, p: &ast::Pattern, expected_ty: &IrType) -> Pattern {
        if let ast::Pattern::Constructor(name, args) = p
            && !name.contains("::")
        {
            let target_ty = self.lower_type_pattern_name(name);
            let option_wrapped_union = match expected_ty {
                IrType::Option(inner) if inner.is_union() => Some(inner.as_ref()),
                _ => None,
            };
            let union_ty = option_wrapped_union.unwrap_or(expected_ty);
            if let Some(variant_index) = union_ty.union_variant_index_for_member(&target_ty)
                && let Some(union_name) = union_ty.union_type_name()
            {
                let member_ty = expected_ty
                    .union_members()
                    .or_else(|| option_wrapped_union.and_then(IrType::union_members))
                    .and_then(|members| members.get(variant_index))
                    .cloned()
                    .unwrap_or(target_ty);
                let fields = args
                    .iter()
                    .filter_map(|arg| match arg {
                        ast::PatternArg::Positional(pat) => {
                            Some(self.lower_pattern_for_expected_type(&pat.node, &member_ty))
                        }
                        ast::PatternArg::Named(_, _) => None,
                    })
                    .collect();
                let union_pattern = Pattern::Enum {
                    name: union_name.clone(),
                    variant: format!("{}::{}", union_name, IrType::union_variant_name(variant_index)),
                    fields,
                };
                if option_wrapped_union.is_some() {
                    return Pattern::Enum {
                        name: "Option".to_string(),
                        variant: constructors::as_str(ConstructorId::Some).to_string(),
                        fields: vec![union_pattern],
                    };
                }
                return union_pattern;
            }
        }

        self.lower_pattern(p)
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
                self.lower_expr(&ast::Expr::Literal(lit.clone()), ast::Span::default())
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
