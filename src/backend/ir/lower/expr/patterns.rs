//! Pattern and match-arm lowering.

use super::super::super::TypedExpr;
use super::super::super::expr::{
    IrCallArg, IrCallArgKind, IrExprKind, MatchArm, MatchArmBinding, Pattern, VarAccess, VarRefKind,
};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use super::super::types::union_ir_type;
use crate::frontend::ast::{self, Spanned};
use incan_core::lang::surface::constructors::{self, ConstructorId};

#[derive(Debug, Clone)]
struct UnionPatternVariant {
    source_index: usize,
    target_index: usize,
    source_ty: IrType,
}

#[derive(Debug, Clone)]
struct UnionPatternTarget {
    target_ty: IrType,
    variants: Vec<UnionPatternVariant>,
}

#[derive(Debug, Clone)]
struct NarrowedUnionCaptureBinding {
    name: String,
}

impl AstLowering {
    /// Lower a type while expanding local transparent aliases used in pattern positions.
    fn lower_type_pattern_name_expanded(&self, name: &str) -> IrType {
        let mut visiting = std::collections::HashSet::new();
        self.lower_pattern_type_with_aliases(&ast::Type::Simple(name.to_string()), &mut visiting)
    }

    /// Lower a pattern type target with local transparent alias expansion and canonical union flattening.
    fn lower_pattern_type_with_aliases(
        &self,
        ty: &ast::Type,
        visiting: &mut std::collections::HashSet<String>,
    ) -> IrType {
        match ty {
            ast::Type::Simple(name) => {
                if let Some(target) = self.source_type_alias_targets.get(name)
                    && visiting.insert(name.clone())
                {
                    let lowered = self.lower_pattern_type_with_aliases(target, visiting);
                    visiting.remove(name);
                    return lowered;
                }
                self.lower_type(ty)
            }
            ast::Type::Generic(base, params) => {
                let lowered_params = params
                    .iter()
                    .map(|param| self.lower_pattern_type_with_aliases(&param.node, visiting))
                    .collect::<Vec<_>>();
                if base == super::super::super::types::IR_UNION_TYPE_NAME {
                    union_ir_type(lowered_params)
                } else {
                    IrType::NamedGeneric(base.clone(), lowered_params)
                }
            }
            ast::Type::Tuple(items) => IrType::Tuple(
                items
                    .iter()
                    .map(|item| self.lower_pattern_type_with_aliases(&item.node, visiting))
                    .collect(),
            ),
            ast::Type::Ref(inner) => IrType::Ref(Box::new(self.lower_pattern_type_with_aliases(&inner.node, visiting))),
            ast::Type::RefMut(inner) => {
                IrType::RefMut(Box::new(self.lower_pattern_type_with_aliases(&inner.node, visiting)))
            }
            _ => self.lower_type(ty),
        }
    }

    /// Resolve how a constructor pattern maps onto a union scrutinee.
    fn union_pattern_target(&self, expected_ty: &IrType, name: &str) -> Option<UnionPatternTarget> {
        let target_ty = self.lower_type_pattern_name_expanded(name);
        self.union_subset_target(expected_ty, target_ty)
    }

    /// Resolve how a target type maps onto a union scrutinee.
    fn union_subset_target(&self, expected_ty: &IrType, target_ty: IrType) -> Option<UnionPatternTarget> {
        let union_ty = match expected_ty {
            IrType::Option(inner) if inner.is_union() => inner.as_ref(),
            _ => expected_ty,
        };
        let source_members = union_ty.union_members()?;

        if let Some(target_members) = target_ty.union_members() {
            let mut variants = Vec::new();
            for (target_index, target_member) in target_members.iter().enumerate() {
                let source_index = source_members
                    .iter()
                    .position(|member| Self::match_union_member_matches(member, target_member))?;
                variants.push(UnionPatternVariant {
                    source_index,
                    target_index,
                    source_ty: source_members[source_index].clone(),
                });
            }
            return Some(UnionPatternTarget { target_ty, variants });
        }

        let source_index = source_members
            .iter()
            .position(|member| Self::match_union_member_matches(member, &target_ty))?;
        Some(UnionPatternTarget {
            target_ty: source_members[source_index].clone(),
            variants: vec![UnionPatternVariant {
                source_index,
                target_index: source_index,
                source_ty: source_members[source_index].clone(),
            }],
        })
    }

    /// Return whether a union member can satisfy a target pattern type.
    fn match_union_member_matches(member: &IrType, target_ty: &IrType) -> bool {
        member == target_ty
            || matches!(
                (member, target_ty),
                (IrType::String, IrType::StaticStr | IrType::StrRef | IrType::FrozenStr)
            )
    }

    /// Return payload types for a constructor pattern when the scrutinee type is known.
    fn constructor_field_types_for_pattern(&self, name: &str, expected_ty: &IrType, field_count: usize) -> Vec<IrType> {
        if let Some(union_target) = self.union_pattern_target(expected_ty, name)
            && field_count == 1
        {
            return vec![union_target.target_ty];
        }
        match (name, expected_ty) {
            (variant, IrType::Option(inner)) if variant == constructors::as_str(ConstructorId::Some) => {
                vec![inner.as_ref().clone()]
            }
            (variant, IrType::Result(ok, _)) if variant == constructors::as_str(ConstructorId::Ok) => {
                vec![ok.as_ref().clone()]
            }
            (variant, IrType::Result(_, err)) if variant == constructors::as_str(ConstructorId::Err) => {
                vec![err.as_ref().clone()]
            }
            _ => vec![IrType::Unknown; field_count],
        }
    }

    /// Define pattern-bound locals with types projected from the expected scrutinee type.
    fn define_match_pattern_bindings_for_expected_type(&mut self, pattern: &ast::Pattern, expected_ty: &IrType) {
        match pattern {
            ast::Pattern::Binding(name) => self.define_local_binding(name.clone(), expected_ty.clone(), false),
            ast::Pattern::Tuple(items) => {
                let item_tys = match expected_ty {
                    IrType::Tuple(items) => items.clone(),
                    _ => vec![IrType::Unknown; items.len()],
                };
                for (idx, item) in items.iter().enumerate() {
                    let item_ty = item_tys.get(idx).cloned().unwrap_or(IrType::Unknown);
                    self.define_match_pattern_bindings_for_expected_type(&item.node, &item_ty);
                }
            }
            ast::Pattern::Constructor(name, args) => {
                let field_tys = self.constructor_field_types_for_pattern(name, expected_ty, args.len());
                for (idx, arg) in args.iter().enumerate() {
                    let field_ty = field_tys.get(idx).cloned().unwrap_or(IrType::Unknown);
                    match arg {
                        ast::PatternArg::Positional(pattern) | ast::PatternArg::Named(_, pattern) => {
                            self.define_match_pattern_bindings_for_expected_type(&pattern.node, &field_ty);
                        }
                    }
                }
            }
            ast::Pattern::Group(inner) => {
                self.define_match_pattern_bindings_for_expected_type(&inner.node, expected_ty);
            }
            ast::Pattern::Or(items) => {
                for item in items {
                    self.define_match_pattern_bindings_for_expected_type(&item.node, expected_ty);
                }
            }
            ast::Pattern::Wildcard | ast::Pattern::Literal(_) => {}
        }
    }

    /// Return the narrowed type represented by remaining union members for wildcard and binding arms.
    fn match_arm_remainder_type(&self, pattern: &ast::Pattern, remaining: &[IrType]) -> Option<IrType> {
        match pattern {
            ast::Pattern::Wildcard | ast::Pattern::Binding(_) if !remaining.is_empty() => {
                Some(union_ir_type(remaining.to_vec()))
            }
            ast::Pattern::Group(inner) => self.match_arm_remainder_type(&inner.node, remaining),
            _ => None,
        }
    }

    /// Remove union members covered by a pattern from the remaining-arm accumulator.
    fn remove_covered_union_members(&self, remaining: &mut Vec<IrType>, pattern: &ast::Pattern, subject_ty: &IrType) {
        match pattern {
            ast::Pattern::Constructor(name, _) if !name.contains("::") => {
                if let Some(target) = self.union_pattern_target(subject_ty, name) {
                    remaining.retain(|member| {
                        !target
                            .variants
                            .iter()
                            .any(|variant| Self::match_union_member_matches(member, &variant.source_ty))
                    });
                }
            }
            ast::Pattern::Or(items) => {
                for item in items {
                    self.remove_covered_union_members(remaining, &item.node, subject_ty);
                }
            }
            ast::Pattern::Group(inner) => self.remove_covered_union_members(remaining, &inner.node, subject_ty),
            ast::Pattern::Wildcard | ast::Pattern::Binding(_) => remaining.clear(),
            _ => {}
        }
    }

    /// Return the direct binding captured by a pattern that should receive a narrowed union wrapper.
    fn narrowed_union_binding_name(pattern: &ast::Pattern) -> Option<&str> {
        match pattern {
            ast::Pattern::Binding(name) => Some(name.as_str()),
            ast::Pattern::Constructor(_, args) => {
                let [ast::PatternArg::Positional(pattern)] = args.as_slice() else {
                    return None;
                };
                match &pattern.node {
                    ast::Pattern::Binding(name) => Some(name.as_str()),
                    _ => None,
                }
            }
            ast::Pattern::Group(inner) => Self::narrowed_union_binding_name(&inner.node),
            _ => None,
        }
    }

    /// Return the direct match subject binding that may need an arm-local narrowed shadow binding.
    fn direct_match_subject_binding_name(scrutinee: &TypedExpr) -> Option<&str> {
        match &scrutinee.kind {
            IrExprKind::Var {
                name,
                ref_kind: VarRefKind::Value,
                ..
            } => Some(name.as_str()),
            _ => None,
        }
    }

    /// Build a value expression that wraps one concrete source union payload into the narrowed target union wrapper.
    fn narrowed_union_binding_value(
        target_ty: &IrType,
        target_index: usize,
        temp_name: String,
        source_ty: IrType,
        payload_access: VarAccess,
    ) -> TypedExpr {
        let union_name = target_ty
            .union_type_name()
            .unwrap_or_else(|| super::super::super::types::IR_UNION_TYPE_NAME.to_string());
        let variant_name = IrType::union_variant_name(target_index);
        let func_ty = IrType::Function {
            params: vec![source_ty.clone()],
            ret: Box::new(target_ty.clone()),
        };
        let func = TypedExpr::new(
            IrExprKind::AssociatedFunction {
                type_name: union_name,
                function_name: variant_name,
            },
            func_ty,
        );
        let payload = TypedExpr::new(
            IrExprKind::Var {
                name: temp_name,
                access: payload_access,
                ref_kind: VarRefKind::Value,
            },
            source_ty.clone(),
        );
        TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(func),
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: payload,
                }],
                callable_signature: None,
                canonical_path: None,
            },
            target_ty.clone(),
        )
    }

    /// Expand a narrowed union capture into one concrete Rust-matchable arm per covered source variant.
    fn lower_narrowed_union_capture_arms(
        &mut self,
        arm: &Spanned<ast::MatchArm>,
        scrutinee_ty: &IrType,
        target: UnionPatternTarget,
        bindings: &[NarrowedUnionCaptureBinding],
    ) -> Result<Vec<MatchArm>, LoweringError> {
        let Some(primary_binding) = bindings.first() else {
            return Ok(Vec::new());
        };
        let source_union_ty = match scrutinee_ty {
            IrType::Option(inner) if inner.is_union() => inner.as_ref(),
            _ => scrutinee_ty,
        };
        let Some(source_union_name) = source_union_ty.union_type_name() else {
            return Ok(Vec::new());
        };

        let mut lowered = Vec::new();
        for variant in target.variants {
            let temp_name = format!("__incan_union_{}_v{}", primary_binding.name, variant.source_index);
            let union_pattern = Pattern::Enum {
                name: source_union_name.clone(),
                variant: format!(
                    "{}::{}",
                    source_union_name,
                    IrType::union_variant_name(variant.source_index)
                ),
                fields: vec![Pattern::Var(temp_name.clone())],
            };
            let pattern = if matches!(scrutinee_ty, IrType::Option(inner) if inner.is_union()) {
                Pattern::Enum {
                    name: "Option".to_string(),
                    variant: constructors::as_str(ConstructorId::Some).to_string(),
                    fields: vec![union_pattern],
                }
            } else {
                union_pattern
            };

            self.push_scope();
            for binding in bindings {
                self.define_local_binding(binding.name.clone(), target.target_ty.clone(), false);
            }
            let arm_result = (|| {
                let guard = arm
                    .node
                    .guard
                    .as_ref()
                    .map(|g| self.lower_expr_spanned(g))
                    .transpose()?;
                let body = match &arm.node.body {
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
                let mut materialized = bindings
                    .iter()
                    .filter_map(|binding| {
                        let guard_uses_binding = guard.as_ref().is_some_and(|guard| {
                            crate::backend::ir::scanners::expr_uses_binding_name(guard, &binding.name)
                        });
                        let body_uses_binding =
                            crate::backend::ir::scanners::expr_uses_binding_name(&body, &binding.name);
                        (guard_uses_binding || body_uses_binding).then_some((
                            binding.name.clone(),
                            guard_uses_binding,
                            body_uses_binding,
                        ))
                    })
                    .collect::<Vec<_>>();
                let move_binding_index = materialized
                    .last()
                    .and_then(|(_, _, body_uses_binding)| body_uses_binding.then_some(materialized.len() - 1));
                let bindings = materialized
                    .drain(..)
                    .enumerate()
                    .map(|(idx, (name, guard_uses_binding, _))| {
                        let value_access = if move_binding_index == Some(idx) {
                            VarAccess::Move
                        } else {
                            VarAccess::Read
                        };
                        let value = Self::narrowed_union_binding_value(
                            &target.target_ty,
                            variant.target_index,
                            temp_name.clone(),
                            variant.source_ty.clone(),
                            value_access,
                        );
                        let guard_value = guard_uses_binding.then(|| {
                            Self::narrowed_union_binding_value(
                                &target.target_ty,
                                variant.target_index,
                                temp_name.clone(),
                                variant.source_ty.clone(),
                                VarAccess::Read,
                            )
                        });
                        MatchArmBinding {
                            name,
                            ty: target.target_ty.clone(),
                            value,
                            guard_value,
                        }
                    })
                    .collect();
                Ok(MatchArm {
                    pattern,
                    bindings,
                    guard,
                    body,
                })
            })();
            self.pop_scope();
            lowered.push(arm_result?);
        }

        Ok(lowered)
    }

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
        scrutinee: &TypedExpr,
    ) -> Result<Vec<MatchArm>, LoweringError> {
        let scrutinee_ty = &scrutinee.ty;
        let subject_binding_name = Self::direct_match_subject_binding_name(scrutinee);
        let mut lowered_arms = Vec::new();
        let mut remaining_union_members = scrutinee_ty.union_members().map(|members| members.to_vec());

        for a in arms {
            let narrowed_subject_ty = remaining_union_members
                .as_ref()
                .and_then(|remaining| self.match_arm_remainder_type(&a.node.pattern.node, remaining));
            let expected_ty = narrowed_subject_ty.as_ref().unwrap_or(scrutinee_ty);

            let mut capture_bindings = Vec::new();
            if let Some(binding_name) = Self::narrowed_union_binding_name(&a.node.pattern.node) {
                capture_bindings.push(NarrowedUnionCaptureBinding {
                    name: binding_name.to_string(),
                });
            }
            if let (Some(binding_name), Some(narrowed_ty)) = (subject_binding_name, narrowed_subject_ty.as_ref())
                && narrowed_ty != scrutinee_ty
                && !capture_bindings.iter().any(|binding| binding.name == binding_name)
            {
                capture_bindings.push(NarrowedUnionCaptureBinding {
                    name: binding_name.to_string(),
                });
            }

            if !capture_bindings.is_empty() {
                let target = match &a.node.pattern.node {
                    ast::Pattern::Constructor(name, _) if !name.contains("::") => self
                        .union_pattern_target(scrutinee_ty, name)
                        .filter(|target| target.target_ty.is_union()),
                    ast::Pattern::Binding(_) => narrowed_subject_ty
                        .clone()
                        .filter(IrType::is_union)
                        .and_then(|target_ty| self.union_subset_target(scrutinee_ty, target_ty)),
                    ast::Pattern::Wildcard => narrowed_subject_ty
                        .clone()
                        .filter(IrType::is_union)
                        .and_then(|target_ty| self.union_subset_target(scrutinee_ty, target_ty)),
                    ast::Pattern::Group(inner) => match &inner.node {
                        ast::Pattern::Constructor(name, _) if !name.contains("::") => self
                            .union_pattern_target(scrutinee_ty, name)
                            .filter(|target| target.target_ty.is_union()),
                        ast::Pattern::Binding(_) => narrowed_subject_ty
                            .clone()
                            .filter(IrType::is_union)
                            .and_then(|target_ty| self.union_subset_target(scrutinee_ty, target_ty)),
                        ast::Pattern::Wildcard => narrowed_subject_ty
                            .clone()
                            .filter(IrType::is_union)
                            .and_then(|target_ty| self.union_subset_target(scrutinee_ty, target_ty)),
                        _ => None,
                    },
                    _ => None,
                };

                if let Some(target) = target {
                    let arms = self.lower_narrowed_union_capture_arms(a, scrutinee_ty, target, &capture_bindings)?;
                    if !arms.is_empty() {
                        lowered_arms.extend(arms);
                        if a.node.guard.is_none()
                            && let Some(remaining) = remaining_union_members.as_mut()
                        {
                            self.remove_covered_union_members(remaining, &a.node.pattern.node, scrutinee_ty);
                        }
                        continue;
                    }
                }
            }

            let pattern = self.lower_pattern_for_expected_type(&a.node.pattern.node, expected_ty);
            self.push_scope();
            self.define_match_pattern_bindings_for_expected_type(&a.node.pattern.node, expected_ty);
            let arm_result = (|| {
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
                Ok(MatchArm {
                    pattern,
                    bindings: Vec::new(),
                    guard,
                    body,
                })
            })();
            self.pop_scope();
            lowered_arms.push(arm_result?);

            if a.node.guard.is_none()
                && let Some(remaining) = remaining_union_members.as_mut()
            {
                self.remove_covered_union_members(remaining, &a.node.pattern.node, scrutinee_ty);
            }
        }

        Ok(lowered_arms)
    }

    /// Lower the type name used by a union type pattern.
    fn lower_type_pattern_name(&self, name: &str) -> IrType {
        self.lower_type_pattern_name_expanded(name)
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

        match p {
            ast::Pattern::Or(items) => Pattern::Or(
                items
                    .iter()
                    .map(|item| self.lower_pattern_for_expected_type(&item.node, expected_ty))
                    .collect(),
            ),
            ast::Pattern::Group(inner) => self.lower_pattern_for_expected_type(&inner.node, expected_ty),
            _ => self.lower_pattern(p),
        }
    }

    /// Lower a pattern to IR.
    ///
    /// Handles wildcard, binding, literal, constructor, tuple, and alternation patterns.
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
            ast::Pattern::Group(pattern) => self.lower_pattern(&pattern.node),
            ast::Pattern::Or(items) => Pattern::Or(items.iter().map(|item| self.lower_pattern(&item.node)).collect()),
        }
    }
}
