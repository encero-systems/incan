//! Statement lowering for AST to IR conversion.
//!
//! This module handles lowering of all statement types: let bindings, assignments, control flow (if/while/for), and
//! returns.

use std::collections::HashMap;

use super::super::expr::{
    IrCallArg, IrCallArgKind, IrExprKind, Literal as IrLiteral, MatchArm, MethodCallArgPolicy, Pattern as IrPattern,
    VarAccess, VarRefKind,
};
use super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::types::IrType;
use super::super::{IrSpan, Mutability, TypedExpr};
use super::AstLowering;
use super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::typechecker::ResolvedOperatorKind;
use incan_core::lang::builtins::{self as core_builtins, BuiltinFnId};
use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_semantics_core::SurfaceStmtLoweringAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssertIsPatternKind {
    Some,
    None,
    Ok,
    Err,
}

struct AssertIsPattern<'a> {
    kind: AssertIsPatternKind,
    scrutinee: &'a Spanned<ast::Expr>,
    binding: Option<String>,
}

struct UnionIsInstanceTest<'a> {
    value: &'a Spanned<ast::Expr>,
    binding: String,
    union_ty: IrType,
    member_ty: IrType,
    variant_index: usize,
    false_remainder: Option<UnionRemainder>,
}

struct UnionRemainder {
    ty: IrType,
    variants: Vec<(usize, IrType)>,
}

struct OptionNoneTest<'a> {
    value: &'a Spanned<ast::Expr>,
    binding: String,
    inner_ty: IrType,
    is_not_none: bool,
}

struct OptionIsInstanceTest<'a> {
    value: &'a Spanned<ast::Expr>,
    binding: String,
    member_ty: IrType,
    true_pattern: IrPattern,
    false_remainder: Option<OptionRemainder>,
}

struct OptionRemainder {
    ty: IrType,
    some_variants: Vec<(IrPattern, IrType)>,
}

/// Remaining `elif`/`else` branches that must run after a narrowing false arm.
struct BranchTail<'a> {
    elif_branches: &'a [(Spanned<ast::Expr>, Vec<Spanned<ast::Statement>>)],
    else_body: Option<&'a [Spanned<ast::Statement>]>,
}

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
    pub(in crate::backend::ir::lower) fn define_for_pattern_bindings(&mut self, pattern: &ast::Pattern, ty: &IrType) {
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

    /// Lower a statement slice into a unit-valued block expression.
    ///
    /// RFC 049 lowering uses this to reuse statement-body lowering inside match
    /// arms while preserving the branch-local scope rules of `if let` and
    /// `while let`.
    fn lower_block_expr(
        &mut self,
        stmts: &[Spanned<ast::Statement>],
        scoped: bool,
    ) -> Result<TypedExpr, LoweringError> {
        if scoped {
            self.push_scope();
        }
        let lowered = self.lower_statements(stmts);
        if scoped {
            self.pop_scope();
        }
        Ok(TypedExpr::new(
            IrExprKind::Block {
                stmts: lowered?,
                value: None,
            },
            IrType::Unit,
        ))
    }

    /// Lower `elif` / `else` branches into nested IR `if` statements.
    ///
    /// The returned statement list becomes the else-branch payload for the
    /// preceding branch, which lets `if let` reuse the same fallback lowering as
    /// ordinary `if` chains.
    fn lower_if_else_chain(
        &mut self,
        elif_branches: &[(Spanned<ast::Expr>, Vec<Spanned<ast::Statement>>)],
        else_body: Option<&[Spanned<ast::Statement>]>,
    ) -> Result<Option<Vec<IrStmt>>, LoweringError> {
        if let Some((elif_cond, elif_body)) = elif_branches.first() {
            let elif_stmt = self.lower_if_expr_stmt_kind(elif_cond, elif_body, &elif_branches[1..], else_body)?;
            return Ok(Some(vec![IrStmt::new(elif_stmt)]));
        }

        else_body
            .map(|body| {
                self.push_scope();
                let result = self.lower_statements(body);
                self.pop_scope();
                result
            })
            .transpose()
    }

    /// Resolve the type argument from an `isinstance(value, Type)` condition.
    fn resolve_isinstance_target_type(&self, expr: &Spanned<ast::Expr>) -> Option<IrType> {
        match &expr.node {
            ast::Expr::Ident(name) => Some(self.lower_type(&ast::Type::Simple(name.clone()))),
            ast::Expr::Paren(inner) => self.resolve_isinstance_target_type(inner),
            _ => None,
        }
    }

    /// Build the canonical IR type for a narrowed set of union members.
    fn union_type_from_members(members: Vec<IrType>) -> IrType {
        match members.as_slice() {
            [] => IrType::Unit,
            [single] => single.clone(),
            _ => IrType::NamedGeneric(super::super::types::IR_UNION_TYPE_NAME.to_string(), members),
        }
    }

    /// Return whether a known concrete value can satisfy an `isinstance(..., T)` target.
    fn isinstance_member_matches(member: &IrType, target_ty: &IrType) -> bool {
        member == target_ty
            || matches!(
                (member, target_ty),
                (IrType::String, IrType::StaticStr | IrType::StrRef | IrType::FrozenStr)
            )
    }

    /// Extract a union-aware `isinstance` test from an `if` condition.
    fn union_isinstance_test<'a>(&self, condition: &'a Spanned<ast::Expr>) -> Option<UnionIsInstanceTest<'a>> {
        let ast::Expr::Call(callee, _, args) = &condition.node else {
            return None;
        };
        let ast::Expr::Ident(call_name) = &callee.node else {
            return None;
        };
        if core_builtins::from_str(call_name) != Some(BuiltinFnId::IsInstance) || args.len() != 2 {
            return None;
        }

        let value = match &args[0] {
            ast::CallArg::Positional(value) => value,
            _ => return None,
        };
        let ast::Expr::Ident(binding) = &value.node else {
            return None;
        };
        let target = match &args[1] {
            ast::CallArg::Positional(target) => target,
            _ => return None,
        };
        let target_ty = self.resolve_isinstance_target_type(target)?;
        let union_ty = self.lookup_var(binding);
        let variant_index = union_ty.union_variant_index_for_member(&target_ty)?;
        let members = union_ty.union_members()?;
        let member_ty = members.get(variant_index).cloned().unwrap_or(target_ty);
        let remaining: Vec<_> = members
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != variant_index)
            .map(|(index, member_ty)| (index, member_ty.clone()))
            .collect();
        let false_remainder = if remaining.is_empty() {
            None
        } else {
            let members = remaining.iter().map(|(_, member_ty)| member_ty.clone()).collect();
            Some(UnionRemainder {
                ty: Self::union_type_from_members(members),
                variants: remaining,
            })
        };

        Some(UnionIsInstanceTest {
            value,
            binding: binding.clone(),
            union_ty,
            member_ty,
            variant_index,
            false_remainder,
        })
    }

    /// Return whether an expression is the source-level `None` value.
    fn is_none_expr(expr: &Spanned<ast::Expr>) -> bool {
        matches!(&expr.node, ast::Expr::Literal(ast::Literal::None))
            || matches!(&expr.node, ast::Expr::Ident(name) if name == constructors::as_str(ConstructorId::None))
    }

    /// Extract an `x is None` or `x is not None` test for an option-typed local.
    fn option_none_test<'a>(&self, condition: &'a Spanned<ast::Expr>) -> Option<OptionNoneTest<'a>> {
        let ast::Expr::Binary(value, op @ (ast::BinaryOp::Is | ast::BinaryOp::IsNot), right) = &condition.node else {
            return None;
        };
        if !Self::is_none_expr(right) {
            return None;
        }
        let ast::Expr::Ident(binding) = &value.node else {
            return None;
        };
        let IrType::Option(inner_ty) = self.lookup_var(binding) else {
            return None;
        };

        Some(OptionNoneTest {
            value,
            binding: binding.clone(),
            inner_ty: *inner_ty,
            is_not_none: matches!(op, ast::BinaryOp::IsNot),
        })
    }

    /// Extract `isinstance(value, T)` for an option-typed local whose payload is a union or direct member.
    fn option_isinstance_test<'a>(&self, condition: &'a Spanned<ast::Expr>) -> Option<OptionIsInstanceTest<'a>> {
        let ast::Expr::Call(callee, _, args) = &condition.node else {
            return None;
        };
        let ast::Expr::Ident(call_name) = &callee.node else {
            return None;
        };
        if core_builtins::from_str(call_name) != Some(BuiltinFnId::IsInstance) || args.len() != 2 {
            return None;
        }

        let value = match &args[0] {
            ast::CallArg::Positional(value) => value,
            _ => return None,
        };
        let ast::Expr::Ident(binding) = &value.node else {
            return None;
        };
        let target = match &args[1] {
            ast::CallArg::Positional(target) => target,
            _ => return None,
        };
        let target_ty = self.resolve_isinstance_target_type(target)?;
        let IrType::Option(inner_ty) = self.lookup_var(binding) else {
            return None;
        };

        if let Some(variant_index) = inner_ty.union_variant_index_for_member(&target_ty) {
            let members = inner_ty.union_members()?;
            let union_name = inner_ty.union_type_name()?;
            let member_ty = members.get(variant_index).cloned().unwrap_or(target_ty);
            let remaining: Vec<_> = members
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != variant_index)
                .map(|(index, member_ty)| {
                    (
                        IrPattern::Enum {
                            name: "Option".to_string(),
                            variant: constructors::as_str(ConstructorId::Some).to_string(),
                            fields: vec![IrPattern::Enum {
                                name: union_name.clone(),
                                variant: format!("{}::{}", union_name, IrType::union_variant_name(index)),
                                fields: vec![IrPattern::Var(format!("__incan_option_union_{}_{}", binding, index))],
                            }],
                        },
                        member_ty.clone(),
                    )
                })
                .collect();
            let false_remainder = if remaining.is_empty() {
                Some(OptionRemainder {
                    ty: IrType::Unit,
                    some_variants: Vec::new(),
                })
            } else {
                let remainder_members = remaining.iter().map(|(_, member_ty)| member_ty.clone()).collect();
                Some(OptionRemainder {
                    ty: IrType::Option(Box::new(Self::union_type_from_members(remainder_members))),
                    some_variants: remaining,
                })
            };

            return Some(OptionIsInstanceTest {
                value,
                binding: binding.clone(),
                member_ty,
                true_pattern: IrPattern::Enum {
                    name: "Option".to_string(),
                    variant: constructors::as_str(ConstructorId::Some).to_string(),
                    fields: vec![IrPattern::Enum {
                        name: union_name.clone(),
                        variant: format!("{}::{}", union_name, IrType::union_variant_name(variant_index)),
                        fields: vec![IrPattern::Var(binding.clone())],
                    }],
                },
                false_remainder,
            });
        }

        if Self::isinstance_member_matches(inner_ty.as_ref(), &target_ty) {
            return Some(OptionIsInstanceTest {
                value,
                binding: binding.clone(),
                member_ty: inner_ty.as_ref().clone(),
                true_pattern: Self::some_pattern(binding.clone()),
                false_remainder: Some(OptionRemainder {
                    ty: IrType::Unit,
                    some_variants: Vec::new(),
                }),
            });
        }

        None
    }

    /// Build the IR pattern for an option `None` arm.
    fn none_pattern() -> IrPattern {
        IrPattern::Literal(TypedExpr::new(
            IrExprKind::None,
            IrType::Option(Box::new(IrType::Unknown)),
        ))
    }

    /// Build the IR pattern for an option `Some(binding)` arm.
    fn some_pattern(binding: String) -> IrPattern {
        IrPattern::Enum {
            name: "Option".to_string(),
            variant: constructors::as_str(ConstructorId::Some).to_string(),
            fields: vec![IrPattern::Var(binding)],
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
                let stmt = self.lower_statement(&s.node, s.span)?;
                result.push(stmt);
            }
            Ok(result)
        })();

        let _ = self.remaining_ident_reads.pop();
        lowered
    }

    /// Build a move read from a local binding introduced by a generated match pattern.
    fn local_value_expr(name: String, ty: IrType) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Var {
                name,
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            ty,
        )
    }

    /// Resolve `isinstance(binding, T)` when the current match arm already proves `binding` has one concrete type.
    fn known_member_isinstance_matches(
        &self,
        condition: &Spanned<ast::Expr>,
        binding: &str,
        member_ty: &IrType,
    ) -> Option<bool> {
        let ast::Expr::Call(callee, _, args) = &condition.node else {
            return None;
        };
        let ast::Expr::Ident(call_name) = &callee.node else {
            return None;
        };
        if core_builtins::from_str(call_name) != Some(BuiltinFnId::IsInstance) || args.len() != 2 {
            return None;
        }
        let ast::CallArg::Positional(value) = &args[0] else {
            return None;
        };
        let ast::Expr::Ident(value_name) = &value.node else {
            return None;
        };
        if value_name != binding {
            return None;
        }
        let ast::CallArg::Positional(target) = &args[1] else {
            return None;
        };
        let target_ty = self.resolve_isinstance_target_type(target)?;
        Some(Self::isinstance_member_matches(member_ty, &target_ty))
    }

    /// Lower an `elif`/`else` tail after a union match arm has already narrowed to one concrete member.
    fn lower_known_member_tail(
        &mut self,
        binding: &str,
        member_ty: &IrType,
        branch_tail: &BranchTail<'_>,
    ) -> Result<Option<Vec<IrStmt>>, LoweringError> {
        for (index, (condition, body)) in branch_tail.elif_branches.iter().enumerate() {
            let Some(matches) = self.known_member_isinstance_matches(condition, binding, member_ty) else {
                return self.lower_if_else_chain(&branch_tail.elif_branches[index..], branch_tail.else_body);
            };
            if matches {
                self.push_scope();
                self.define_local_binding(binding.to_string(), member_ty.clone(), false);
                let lowered = self.lower_statements(body);
                self.pop_scope();
                return lowered.map(Some);
            }
        }

        self.lower_if_else_chain(&[], branch_tail.else_body)
    }

    /// Lower one false-branch arm for `isinstance` over an ordinary union.
    fn lower_union_false_arm(
        &mut self,
        binding: &str,
        union_name: &str,
        variant_index: usize,
        member_ty: &IrType,
        false_ty: &IrType,
        branch_tail: &BranchTail<'_>,
    ) -> Result<MatchArm, LoweringError> {
        let variant = format!("{}::{}", union_name, IrType::union_variant_name(variant_index));
        let direct_member_binding = false_ty == member_ty;
        let pattern_binding = if direct_member_binding {
            binding.to_string()
        } else {
            format!("__incan_union_{}_{}", binding, variant_index)
        };

        self.push_scope();
        let mut stmts = Vec::new();
        if direct_member_binding {
            self.define_local_binding(binding.to_string(), member_ty.clone(), false);
        } else {
            self.define_local_binding(binding.to_string(), false_ty.clone(), false);
            stmts.push(IrStmt::new(IrStmtKind::Let {
                name: binding.to_string(),
                ty: false_ty.clone(),
                type_annotation: None,
                mutability: Mutability::Immutable,
                value: Self::local_value_expr(pattern_binding.clone(), member_ty.clone()),
            }));
        }
        let lowered_tail = if direct_member_binding {
            self.lower_known_member_tail(binding, member_ty, branch_tail)?
        } else {
            self.lower_if_else_chain(branch_tail.elif_branches, branch_tail.else_body)?
        };
        if let Some(mut lowered_tail) = lowered_tail {
            stmts.append(&mut lowered_tail);
        }
        self.pop_scope();

        Ok(MatchArm {
            pattern: IrPattern::Enum {
                name: union_name.to_string(),
                variant,
                fields: vec![IrPattern::Var(pattern_binding)],
            },
            guard: None,
            body: TypedExpr::new(IrExprKind::Block { stmts, value: None }, IrType::Unit),
        })
    }

    /// Lower one false-branch arm for `isinstance` over an `Option[...]` payload.
    fn lower_option_false_arm(
        &mut self,
        binding: &str,
        pattern: IrPattern,
        pattern_binding: Option<String>,
        member_ty: Option<IrType>,
        false_ty: &IrType,
        branch_tail: &BranchTail<'_>,
    ) -> Result<MatchArm, LoweringError> {
        self.push_scope();
        let mut stmts = Vec::new();
        self.define_local_binding(binding.to_string(), false_ty.clone(), false);
        if let (Some(pattern_binding), Some(member_ty)) = (pattern_binding, member_ty) {
            stmts.push(IrStmt::new(IrStmtKind::Let {
                name: binding.to_string(),
                ty: false_ty.clone(),
                type_annotation: None,
                mutability: Mutability::Immutable,
                value: Self::local_value_expr(pattern_binding, member_ty),
            }));
        } else {
            let value = if matches!(false_ty, IrType::Unit) {
                TypedExpr::new(IrExprKind::Unit, IrType::Unit)
            } else {
                TypedExpr::new(IrExprKind::None, IrType::Option(Box::new(IrType::Unknown)))
            };
            stmts.push(IrStmt::new(IrStmtKind::Let {
                name: binding.to_string(),
                ty: false_ty.clone(),
                type_annotation: None,
                mutability: Mutability::Immutable,
                value,
            }));
        }
        if let Some(mut lowered_tail) = self.lower_if_else_chain(branch_tail.elif_branches, branch_tail.else_body)? {
            stmts.append(&mut lowered_tail);
        }
        self.pop_scope();

        Ok(MatchArm {
            pattern,
            guard: None,
            body: TypedExpr::new(IrExprKind::Block { stmts, value: None }, IrType::Unit),
        })
    }

    /// Lower an expression-conditioned `if` while preserving branch-local narrowing.
    fn lower_if_expr_stmt_kind(
        &mut self,
        condition: &Spanned<ast::Expr>,
        then_body: &[Spanned<ast::Statement>],
        elif_branches: &[(Spanned<ast::Expr>, Vec<Spanned<ast::Statement>>)],
        else_body: Option<&[Spanned<ast::Statement>]>,
    ) -> Result<IrStmtKind, LoweringError> {
        let branch_tail = BranchTail {
            elif_branches,
            else_body,
        };

        if let Some(test) = self.union_isinstance_test(condition) {
            let scrutinee = self.lower_expr_spanned(test.value)?;
            let then_body = {
                self.push_scope();
                self.define_local_binding(test.binding.clone(), test.member_ty.clone(), false);
                let lowered = self.lower_statements(then_body)?;
                self.pop_scope();
                lowered
            };
            let union_name = test
                .union_ty
                .union_type_name()
                .unwrap_or_else(|| super::super::types::IR_UNION_TYPE_NAME.to_string());
            let variant = format!("{}::{}", union_name, IrType::union_variant_name(test.variant_index));
            let mut arms = vec![MatchArm {
                pattern: IrPattern::Enum {
                    name: union_name.clone(),
                    variant,
                    fields: vec![IrPattern::Var(test.binding.clone())],
                },
                guard: None,
                body: TypedExpr::new(
                    IrExprKind::Block {
                        stmts: then_body,
                        value: None,
                    },
                    IrType::Unit,
                ),
            }];

            if let Some(false_remainder) = &test.false_remainder {
                for (variant_index, member_ty) in &false_remainder.variants {
                    arms.push(self.lower_union_false_arm(
                        &test.binding,
                        &union_name,
                        *variant_index,
                        member_ty,
                        &false_remainder.ty,
                        &branch_tail,
                    )?);
                }
            }

            return Ok(IrStmtKind::Match { scrutinee, arms });
        }

        if let Some(test) = self.option_isinstance_test(condition) {
            let scrutinee = self.lower_expr_spanned(test.value)?;
            let then_body = {
                self.push_scope();
                self.define_local_binding(test.binding.clone(), test.member_ty.clone(), false);
                let lowered = self.lower_statements(then_body)?;
                self.pop_scope();
                lowered
            };
            let mut arms = vec![MatchArm {
                pattern: test.true_pattern,
                guard: None,
                body: TypedExpr::new(
                    IrExprKind::Block {
                        stmts: then_body,
                        value: None,
                    },
                    IrType::Unit,
                ),
            }];

            if let Some(false_remainder) = &test.false_remainder {
                for (pattern, member_ty) in &false_remainder.some_variants {
                    let pattern_binding = match pattern {
                        IrPattern::Enum { fields, .. } => match fields.as_slice() {
                            [IrPattern::Enum { fields, .. }] => match fields.as_slice() {
                                [IrPattern::Var(name)] => Some(name.clone()),
                                _ => None,
                            },
                            _ => None,
                        },
                        _ => None,
                    };
                    arms.push(self.lower_option_false_arm(
                        &test.binding,
                        pattern.clone(),
                        pattern_binding,
                        Some(member_ty.clone()),
                        &false_remainder.ty,
                        &branch_tail,
                    )?);
                }

                arms.push(self.lower_option_false_arm(
                    &test.binding,
                    Self::none_pattern(),
                    None,
                    None,
                    &false_remainder.ty,
                    &branch_tail,
                )?);
            }

            return Ok(IrStmtKind::Match { scrutinee, arms });
        }

        if let Some(test) = self.option_none_test(condition) {
            let scrutinee = self.lower_expr_spanned(test.value)?;
            let then_body = {
                self.push_scope();
                if test.is_not_none {
                    self.define_local_binding(test.binding.clone(), test.inner_ty.clone(), false);
                }
                let lowered = self.lower_statements(then_body)?;
                self.pop_scope();
                lowered
            };
            let else_body = {
                self.push_scope();
                if !test.is_not_none {
                    self.define_local_binding(test.binding.clone(), test.inner_ty.clone(), false);
                }
                let lowered = self.lower_if_else_chain(elif_branches, else_body)?.unwrap_or_default();
                self.pop_scope();
                lowered
            };
            let then_body = TypedExpr::new(
                IrExprKind::Block {
                    stmts: then_body,
                    value: None,
                },
                IrType::Unit,
            );
            let else_body = TypedExpr::new(
                IrExprKind::Block {
                    stmts: else_body,
                    value: None,
                },
                IrType::Unit,
            );
            let some = Self::some_pattern(test.binding);
            let none = Self::none_pattern();
            let (then_pattern, else_pattern) = if test.is_not_none { (some, none) } else { (none, some) };

            return Ok(IrStmtKind::Match {
                scrutinee,
                arms: vec![
                    MatchArm {
                        pattern: then_pattern,
                        guard: None,
                        body: then_body,
                    },
                    MatchArm {
                        pattern: else_pattern,
                        guard: None,
                        body: else_body,
                    },
                ],
            });
        }

        let else_branch = self.lower_if_else_chain(elif_branches, else_body)?;
        let condition = self.lower_condition_expr(condition)?;
        self.push_scope();
        let then_branch = self.lower_statements(then_body)?;
        self.pop_scope();

        Ok(IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        })
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
    pub(super) fn lower_statement(
        &mut self,
        stmt: &ast::Statement,
        stmt_span: ast::Span,
    ) -> Result<IrStmt, LoweringError> {
        let kind = match stmt {
            ast::Statement::Expr(e) => {
                if let ast::Expr::Yield(Some(value)) = &e.node {
                    IrStmtKind::Yield(self.lower_expr_spanned(value)?)
                } else {
                    IrStmtKind::Expr(self.lower_expr_spanned(e)?)
                }
            }
            ast::Statement::Assert(assert_stmt) => return Ok(IrStmt::new(self.lower_assert_stmt(assert_stmt)?)),

            ast::Statement::Assignment(a) => {
                let rhs_direct_static = self.is_direct_static_ident(&a.value);
                let lowered_value = self.lower_expr_spanned(&a.value)?;
                let type_annotation = a.ty.as_ref().map(|t| self.lower_type(&t.node));
                let ty = type_annotation.clone().unwrap_or_else(|| lowered_value.ty.clone());

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
                            type_annotation,
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
                            type_annotation,
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
                            type_annotation,
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

            ast::Statement::IndexAssignment(ia) => {
                let object = self.lower_expr_spanned(&ia.object)?;
                let index = self.lower_expr_spanned(&ia.index)?;
                let value = self.lower_expr_spanned(&ia.value)?;

                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(stmt_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::IndexAssign
                {
                    IrStmtKind::Expr(TypedExpr::new(
                        IrExprKind::MethodCall {
                            receiver: Box::new(object),
                            method: resolved_operator.method,
                            type_args: Vec::new(),
                            args: vec![
                                IrCallArg {
                                    name: None,
                                    kind: IrCallArgKind::Positional,
                                    expr: index,
                                },
                                IrCallArg {
                                    name: None,
                                    kind: IrCallArgKind::Positional,
                                    expr: value,
                                },
                            ],
                            callable_signature: self.callable_signature_for_call_span(stmt_span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        IrType::Unit,
                    ))
                } else {
                    IrStmtKind::Assign {
                        target: AssignTarget::Index {
                            object: Box::new(object),
                            index: Box::new(index),
                        },
                        value,
                    }
                }
            }

            ast::Statement::Return(opt) => {
                IrStmtKind::Return(opt.as_ref().map(|e| self.lower_expr_spanned(e)).transpose()?)
            }

            ast::Statement::If(i) => {
                let lowered_if = (|| -> Result<IrStmtKind, LoweringError> {
                    match &i.condition {
                        ast::Condition::Expr(condition) => self.lower_if_expr_stmt_kind(
                            condition,
                            &i.then_body,
                            &i.elif_branches,
                            i.else_body.as_deref(),
                        ),
                        ast::Condition::Let { pattern, value } => {
                            let else_branch = self.lower_if_else_chain(&i.elif_branches, i.else_body.as_deref())?;
                            let scrutinee = self.lower_expr_spanned(value)?;
                            let then_body = self.lower_block_expr(&i.then_body, true)?;
                            let fallback_body = TypedExpr::new(
                                IrExprKind::Block {
                                    stmts: else_branch.unwrap_or_default(),
                                    value: None,
                                },
                                IrType::Unit,
                            );

                            Ok(IrStmtKind::Match {
                                scrutinee,
                                arms: vec![
                                    MatchArm {
                                        pattern: self.lower_pattern(&pattern.node),
                                        guard: None,
                                        body: then_body,
                                    },
                                    MatchArm {
                                        pattern: IrPattern::Wildcard,
                                        guard: None,
                                        body: fallback_body,
                                    },
                                ],
                            })
                        }
                    }
                })();
                lowered_if?
            }

            ast::Statement::While(w) => {
                self.non_linear_context_depth += 1;
                let loop_stmt = (|| -> Result<IrStmtKind, LoweringError> {
                    match &w.condition {
                        ast::Condition::Expr(condition) => {
                            self.push_scope();
                            let loop_parts = (|| -> Result<(TypedExpr, Vec<IrStmt>), LoweringError> {
                                let condition = self.lower_condition_expr(condition)?;
                                let body = self.lower_statements(&w.body)?;
                                Ok((condition, body))
                            })();
                            self.pop_scope();
                            let (condition, body) = loop_parts?;
                            Ok(IrStmtKind::While {
                                label: None,
                                condition,
                                body,
                            })
                        }
                        ast::Condition::Let { pattern, value } => {
                            let scrutinee = self.lower_expr_spanned(value)?;
                            let body_expr = self.lower_block_expr(&w.body, true)?;
                            let break_expr = TypedExpr::new(
                                IrExprKind::Block {
                                    stmts: vec![IrStmt::new(IrStmtKind::Break {
                                        label: None,
                                        value: None,
                                    })],
                                    value: None,
                                },
                                IrType::Unit,
                            );

                            Ok(IrStmtKind::Loop {
                                label: None,
                                body: vec![IrStmt::new(IrStmtKind::Match {
                                    scrutinee,
                                    arms: vec![
                                        MatchArm {
                                            pattern: self.lower_pattern(&pattern.node),
                                            guard: None,
                                            body: body_expr,
                                        },
                                        MatchArm {
                                            pattern: IrPattern::Wildcard,
                                            guard: None,
                                            body: break_expr,
                                        },
                                    ],
                                })],
                            })
                        }
                    }
                })();
                self.non_linear_context_depth -= 1;
                loop_stmt?
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
                let protocol_iteration = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.protocol_iteration(f.iter.span).cloned());
                let loop_var_ty = if let Some(protocol) = &protocol_iteration {
                    self.lower_resolved_type(&protocol.item_type)
                } else {
                    match &iterable.ty {
                        IrType::List(elem) => (**elem).clone(),
                        IrType::Dict(k, _) => (**k).clone(),
                        IrType::String => IrType::String,
                        _ => IrType::Unknown,
                    }
                };
                self.define_for_pattern_bindings(&f.pattern.node, &loop_var_ty);

                self.non_linear_context_depth += 1;
                let body_result = self.lower_statements(&f.body);
                self.non_linear_context_depth -= 1;
                let body = body_result?;
                self.pop_scope();

                if let Some(protocol) = protocol_iteration {
                    let iterator_ty = self.lower_resolved_type(&protocol.iterator_type);
                    let option_item_ty = IrType::Option(Box::new(loop_var_ty));
                    let iter_name = format!("__incan_iter_{}_{}", stmt_span.start, stmt_span.end);
                    let iter_value = TypedExpr::new(
                        IrExprKind::MethodCall {
                            receiver: Box::new(iterable),
                            method: protocol.iter_method,
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: self.callable_signature_for_call_span(f.iter.span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        iterator_ty.clone(),
                    );
                    let iter_var = TypedExpr::new(
                        IrExprKind::Var {
                            name: iter_name.clone(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        iterator_ty,
                    );
                    let next_value = TypedExpr::new(
                        IrExprKind::MethodCall {
                            receiver: Box::new(iter_var),
                            method: protocol.next_method,
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: None,
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        option_item_ty.clone(),
                    );
                    let some_pattern = IrPattern::Enum {
                        name: "Option".to_string(),
                        variant: constructors::as_str(ConstructorId::Some).to_string(),
                        fields: vec![self.lower_pattern(&f.pattern.node)],
                    };
                    let none_pattern = IrPattern::Literal(TypedExpr::new(IrExprKind::None, option_item_ty));
                    IrStmtKind::Block(vec![
                        IrStmt::new(IrStmtKind::Let {
                            name: iter_name,
                            ty: iter_value.ty.clone(),
                            type_annotation: None,
                            mutability: Mutability::Mutable,
                            value: iter_value,
                        }),
                        IrStmt::new(IrStmtKind::Loop {
                            label: None,
                            body: vec![IrStmt::new(IrStmtKind::Match {
                                scrutinee: next_value,
                                arms: vec![
                                    MatchArm {
                                        pattern: some_pattern,
                                        guard: None,
                                        body: TypedExpr::new(
                                            IrExprKind::Block {
                                                stmts: body,
                                                value: None,
                                            },
                                            IrType::Unit,
                                        ),
                                    },
                                    MatchArm {
                                        pattern: none_pattern,
                                        guard: None,
                                        body: TypedExpr::new(
                                            IrExprKind::Block {
                                                stmts: vec![IrStmt::new(IrStmtKind::Break {
                                                    label: None,
                                                    value: None,
                                                })],
                                                value: None,
                                            },
                                            IrType::Unit,
                                        ),
                                    },
                                ],
                            })],
                        }),
                    ])
                } else {
                    IrStmtKind::For {
                        label: None,
                        pattern: self.lower_pattern(&f.pattern.node),
                        iterable,
                        body,
                    }
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

                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(stmt_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Binary
                {
                    let method_call = TypedExpr::new(
                        IrExprKind::MethodCall {
                            receiver: Box::new(lhs_expr),
                            method: resolved_operator.method,
                            type_args: Vec::new(),
                            args: vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: rhs_expr,
                            }],
                            callable_signature: self.callable_signature_for_call_span(stmt_span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        lhs_ty.clone(),
                    );

                    return Ok(IrStmt::new(IrStmtKind::Assign {
                        target: assign_target,
                        value: method_call,
                    }));
                }

                // Determine result type using the same policy as binary ops.
                let binop_ast = match ca.op {
                    ast::CompoundOp::Add => ast::BinaryOp::Add,
                    ast::CompoundOp::Sub => ast::BinaryOp::Sub,
                    ast::CompoundOp::Mul => ast::BinaryOp::Mul,
                    ast::CompoundOp::Div => ast::BinaryOp::Div,
                    ast::CompoundOp::FloorDiv => ast::BinaryOp::FloorDiv,
                    ast::CompoundOp::Mod => ast::BinaryOp::Mod,
                    ast::CompoundOp::MatMul => ast::BinaryOp::MatMul,
                    ast::CompoundOp::BitAnd => ast::BinaryOp::BitAnd,
                    ast::CompoundOp::BitOr => ast::BinaryOp::BitOr,
                    ast::CompoundOp::BitXor => ast::BinaryOp::BitXor,
                    ast::CompoundOp::Shl => ast::BinaryOp::Shl,
                    ast::CompoundOp::Shr => ast::BinaryOp::Shr,
                };
                let result_ty = self.binary_result_type(&lhs_ty, &rhs_expr.ty, &binop_ast, None);

                let binop_expr = TypedExpr::new(
                    IrExprKind::BinOp {
                        op: self.lower_binop(&binop_ast, stmt_span)?,
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
                let value_ty = value.ty.clone();
                let temp_name = format!("__incan_tuple_unpack_{}", tu.names.join("_"));
                let mutability = match tu.binding {
                    ast::BindingKind::Mutable => Mutability::Mutable,
                    _ => Mutability::Immutable,
                };

                self.define_local_binding(temp_name.clone(), value_ty.clone(), false);

                let mut stmts = vec![IrStmt::new(IrStmtKind::Let {
                    name: temp_name.clone(),
                    ty: value_ty.clone(),
                    type_annotation: None,
                    mutability: Mutability::Immutable,
                    value,
                })];
                let element_types = match &value_ty {
                    IrType::Tuple(items) => items.clone(),
                    _ => vec![IrType::Unknown; tu.names.len()],
                };

                for (idx, name) in tu.names.iter().enumerate() {
                    let field_ty = element_types.get(idx).cloned().unwrap_or(IrType::Unknown);
                    let field_expr = TypedExpr::new(
                        IrExprKind::Field {
                            object: Box::new(TypedExpr::new(
                                IrExprKind::Var {
                                    name: temp_name.clone(),
                                    access: VarAccess::Move,
                                    ref_kind: VarRefKind::Value,
                                },
                                self.lookup_var(&temp_name),
                            )),
                            field: idx.to_string(),
                        },
                        field_ty.clone(),
                    );

                    self.define_local_binding(name.clone(), field_ty.clone(), false);
                    if matches!(mutability, Mutability::Mutable) {
                        self.mutable_vars.insert(name.clone(), true);
                    }

                    stmts.push(IrStmt::new(IrStmtKind::Let {
                        name: name.clone(),
                        ty: field_ty,
                        type_annotation: None,
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
                    type_annotation: None,
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
                        type_annotation: None,
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
        if let Some(pattern) = Self::assert_is_pattern_from_expr(condition_expr) {
            return self.lower_assert_is_pattern_stmt(pattern, args.get(1));
        }
        let message = args.get(1).map(|m| self.lower_expr_spanned(m)).transpose()?;
        let condition = self.lower_expr_spanned(condition_expr)?;
        self.lower_assert_condition_expr(condition, message)
    }

    fn lower_assert_stmt(&mut self, assert_stmt: &ast::AssertStmt) -> Result<IrStmtKind, LoweringError> {
        match &assert_stmt.kind {
            ast::AssertKind::Condition(condition) => {
                let message = assert_stmt
                    .message
                    .as_ref()
                    .map(|m| self.lower_expr_spanned(m))
                    .transpose()?;
                let condition = self.lower_expr_spanned(condition)?;
                self.lower_assert_condition_expr(condition, message)
            }
            ast::AssertKind::IsPattern { value, pattern } => {
                let Some(pattern) = Self::assert_is_pattern_from_pattern(value, pattern) else {
                    return Err(LoweringError {
                        message: "unsupported assert `is` pattern reached lowering".to_string(),
                        span: IrSpan::default(),
                    });
                };
                self.lower_assert_is_pattern_stmt(pattern, assert_stmt.message.as_ref())
            }
            ast::AssertKind::Raises { call, error_type } => {
                self.lower_assert_raises_stmt(call, error_type, assert_stmt.message.as_ref())
            }
        }
    }

    fn lower_assert_condition_expr(
        &mut self,
        condition: TypedExpr,
        message: Option<TypedExpr>,
    ) -> Result<IrStmtKind, LoweringError> {
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
            .map(|expr| IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr,
            })
            .collect();
        let call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(callee),
                type_args: Vec::new(),
                args: call_args,
                callable_signature: None,
                canonical_path: Some(lowered.canonical_path),
            },
            IrType::Unit,
        );
        Ok(IrStmtKind::Expr(call))
    }

    fn lower_assert_raises_stmt(
        &mut self,
        call: &Spanned<ast::Expr>,
        error_type: &Spanned<ast::Type>,
        message: Option<&Spanned<ast::Expr>>,
    ) -> Result<IrStmtKind, LoweringError> {
        let mut call_args = vec![
            IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: self.lower_expr_spanned(call)?,
            },
            IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: TypedExpr::new(
                    IrExprKind::Literal(IrLiteral::StaticStr(error_type.node.to_string())),
                    IrType::StaticStr,
                ),
            },
        ];
        if let Some(message) = message {
            call_args.push(IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: self.lower_expr_spanned(message)?,
            });
        }

        let helper_name = "assert_raises";
        let callee = TypedExpr::new(
            IrExprKind::Var {
                name: helper_name.to_string(),
                access: VarAccess::Copy,
                ref_kind: VarRefKind::Value,
            },
            self.lookup_var(helper_name),
        );
        Ok(IrStmtKind::Expr(TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(callee),
                type_args: Vec::new(),
                args: call_args,
                callable_signature: None,
                canonical_path: Some(vec!["std".to_string(), "testing".to_string(), helper_name.to_string()]),
            },
            IrType::Unit,
        )))
    }

    /// Lower RFC 018 `assert value is Some/None/Ok/Err` forms to typed assertion helper calls.
    fn lower_assert_is_pattern_stmt(
        &mut self,
        pattern: AssertIsPattern<'_>,
        message: Option<&Spanned<ast::Expr>>,
    ) -> Result<IrStmtKind, LoweringError> {
        let scrutinee = self.lower_expr_spanned(pattern.scrutinee)?;
        let return_ty = match (&pattern.kind, &scrutinee.ty) {
            (AssertIsPatternKind::Some, IrType::Option(inner)) => inner.as_ref().clone(),
            (AssertIsPatternKind::Ok, IrType::Result(ok, _)) => ok.as_ref().clone(),
            (AssertIsPatternKind::Err, IrType::Result(_, err)) => err.as_ref().clone(),
            (AssertIsPatternKind::None, _) => IrType::Unit,
            _ => IrType::Unknown,
        };
        let helper_name = match pattern.kind {
            AssertIsPatternKind::Some => "assert_is_some",
            AssertIsPatternKind::None => "assert_is_none",
            AssertIsPatternKind::Ok => "assert_is_ok",
            AssertIsPatternKind::Err => "assert_is_err",
        };
        let mut call_args = vec![IrCallArg {
            name: None,
            kind: IrCallArgKind::Positional,
            expr: scrutinee,
        }];
        if let Some(message) = message {
            call_args.push(IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: self.lower_expr_spanned(message)?,
            });
        }

        let callee = TypedExpr::new(
            IrExprKind::Var {
                name: helper_name.to_string(),
                access: VarAccess::Copy,
                ref_kind: VarRefKind::Value,
            },
            self.lookup_var(helper_name),
        );
        let call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(callee),
                type_args: Vec::new(),
                args: call_args,
                callable_signature: None,
                canonical_path: Some(vec!["std".to_string(), "testing".to_string(), helper_name.to_string()]),
            },
            return_ty.clone(),
        );

        if let Some(binding) = pattern.binding {
            self.define_local_binding(binding.clone(), return_ty.clone(), false);
            return Ok(IrStmtKind::Let {
                name: binding,
                ty: return_ty,
                type_annotation: None,
                mutability: Mutability::Immutable,
                value: call,
            });
        }

        Ok(IrStmtKind::Expr(call))
    }

    fn assert_is_pattern_from_expr(expr: &Spanned<ast::Expr>) -> Option<AssertIsPattern<'_>> {
        let ast::Expr::Binary(scrutinee, ast::BinaryOp::Is, pattern_expr) = &expr.node else {
            return None;
        };
        match &pattern_expr.node {
            ast::Expr::Literal(ast::Literal::None) => Some(AssertIsPattern {
                kind: AssertIsPatternKind::None,
                scrutinee,
                binding: None,
            }),
            ast::Expr::Ident(name) if name == constructors::as_str(ConstructorId::None) => Some(AssertIsPattern {
                kind: AssertIsPatternKind::None,
                scrutinee,
                binding: None,
            }),
            ast::Expr::Call(callee, type_args, args) if type_args.is_empty() => {
                let ast::Expr::Ident(name) = &callee.node else {
                    return None;
                };
                let kind = match name.as_str() {
                    n if n == constructors::as_str(ConstructorId::Some) => AssertIsPatternKind::Some,
                    n if n == constructors::as_str(ConstructorId::Ok) => AssertIsPatternKind::Ok,
                    n if n == constructors::as_str(ConstructorId::Err) => AssertIsPatternKind::Err,
                    _ => return None,
                };
                let [ast::CallArg::Positional(arg)] = args.as_slice() else {
                    return None;
                };
                let binding = match &arg.node {
                    ast::Expr::Ident(name) if name == "_" => None,
                    ast::Expr::Ident(name) => Some(name.clone()),
                    _ => return None,
                };
                Some(AssertIsPattern {
                    kind,
                    scrutinee,
                    binding,
                })
            }
            _ => None,
        }
    }

    fn assert_is_pattern_from_pattern<'a>(
        scrutinee: &'a Spanned<ast::Expr>,
        pattern: &Spanned<ast::Pattern>,
    ) -> Option<AssertIsPattern<'a>> {
        match &pattern.node {
            ast::Pattern::Constructor(name, args)
                if name == constructors::as_str(ConstructorId::None) && args.is_empty() =>
            {
                Some(AssertIsPattern {
                    kind: AssertIsPatternKind::None,
                    scrutinee,
                    binding: None,
                })
            }
            ast::Pattern::Constructor(name, args) => {
                let kind = match name.as_str() {
                    n if n == constructors::as_str(ConstructorId::Some) => AssertIsPatternKind::Some,
                    n if n == constructors::as_str(ConstructorId::Ok) => AssertIsPatternKind::Ok,
                    n if n == constructors::as_str(ConstructorId::Err) => AssertIsPatternKind::Err,
                    _ => return None,
                };
                let [ast::PatternArg::Positional(arg)] = args.as_slice() else {
                    return None;
                };
                let binding = match &arg.node {
                    ast::Pattern::Wildcard => None,
                    ast::Pattern::Binding(name) => Some(name.clone()),
                    _ => return None,
                };
                Some(AssertIsPattern {
                    kind,
                    scrutinee,
                    binding,
                })
            }
            _ => None,
        }
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
                ast::CallArg::Positional(expr)
                | ast::CallArg::Named(_, expr)
                | ast::CallArg::PositionalUnpack(expr)
                | ast::CallArg::KeywordUnpack(expr) => self.count_expr_ident_reads(&expr.node, counts),
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
                self.count_condition_ident_reads(&i.condition, counts);
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
                self.count_condition_ident_reads(&w.condition, counts);
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
            ast::Statement::Assert(assert_stmt) => {
                match &assert_stmt.kind {
                    ast::AssertKind::Condition(condition) => self.count_expr_ident_reads(&condition.node, counts),
                    ast::AssertKind::IsPattern { value, .. } => self.count_expr_ident_reads(&value.node, counts),
                    ast::AssertKind::Raises { call, .. } => self.count_expr_ident_reads(&call.node, counts),
                }
                if let Some(message) = &assert_stmt.message {
                    self.count_expr_ident_reads(&message.node, counts);
                }
            }
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

    fn count_condition_ident_reads(&self, condition: &ast::Condition, counts: &mut HashMap<String, usize>) {
        match condition {
            ast::Condition::Expr(expr) => self.count_expr_ident_reads(&expr.node, counts),
            ast::Condition::Let { value, .. } => self.count_expr_ident_reads(&value.node, counts),
        }
    }

    /// Count identifier reads inside an expression so lowering can plan moves, borrows, and clones.
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
                ast::SurfaceExprPayload::LeadingDotPath { .. } => {}
                ast::SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                    self.count_expr_ident_reads(&left.node, counts);
                    self.count_expr_ident_reads(&right.node, counts);
                }
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
            ast::Expr::Generator(generator) => {
                self.count_expr_ident_reads(&generator.expr.node, counts);
                for clause in &generator.clauses {
                    match clause {
                        ast::ComprehensionClause::For { iter, .. } => {
                            self.count_expr_ident_reads(&iter.node, counts);
                        }
                        ast::ComprehensionClause::If(condition) => {
                            self.count_expr_ident_reads(&condition.node, counts);
                        }
                    }
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
            ast::Expr::Tuple(items) | ast::Expr::Set(items) => {
                for item in items {
                    self.count_expr_ident_reads(&item.node, counts);
                }
            }
            ast::Expr::List(items) => {
                for item in items {
                    match item {
                        ast::ListEntry::Element(value) | ast::ListEntry::Spread(value) => {
                            self.count_expr_ident_reads(&value.node, counts);
                        }
                    }
                }
            }
            ast::Expr::Dict(pairs) => {
                for entry in pairs {
                    match entry {
                        ast::DictEntry::Pair(key, value) => {
                            self.count_expr_ident_reads(&key.node, counts);
                            self.count_expr_ident_reads(&value.node, counts);
                        }
                        ast::DictEntry::Spread(value) => self.count_expr_ident_reads(&value.node, counts),
                    }
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
