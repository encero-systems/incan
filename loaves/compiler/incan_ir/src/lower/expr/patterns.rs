//! Pattern and match-arm lowering.

use super::super::super::TypedExpr;
use super::super::super::expr::{
    BinOp, IrCallArg, IrCallArgKind, IrExprKind, MatchArm, MatchArmBinding, Pattern, VarAccess, VarRefKind,
};
use super::super::super::types::{IrType, union_member_type_matches};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use super::super::types::union_ir_type;
use incan_frontend::ast::{self, Spanned};
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use std::collections::HashMap;

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

    /// Return the union an `Option` scrutinee carries: the union itself, or the stored union a dict lookup whose result
    /// is only read finds in place (`Option[&union]` in Rust).
    fn option_payload_union(ty: &IrType) -> Option<&IrType> {
        let IrType::Option(inner) = ty else {
            return None;
        };
        let payload = match inner.as_ref() {
            IrType::Ref(found) => found.as_ref(),
            payload => payload,
        };
        payload.is_union().then_some(payload)
    }

    /// Resolve how a target type maps onto a union scrutinee.
    fn union_subset_target(&self, expected_ty: &IrType, target_ty: IrType) -> Option<UnionPatternTarget> {
        let union_ty = Self::option_payload_union(expected_ty).unwrap_or(expected_ty);
        let source_members = union_ty.union_members()?;

        if let Some(target_members) = target_ty.union_members() {
            let mut variants = Vec::new();
            for (target_index, target_member) in target_members.iter().enumerate() {
                let source_index = source_members
                    .iter()
                    .position(|member| union_member_type_matches(member, target_member))?;
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
            .position(|member| union_member_type_matches(member, &target_ty))?;
        Some(UnionPatternTarget {
            target_ty: source_members[source_index].clone(),
            variants: vec![UnionPatternVariant {
                source_index,
                target_index: source_index,
                source_ty: source_members[source_index].clone(),
            }],
        })
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
                let field_tys = self.constructor_field_types_for_pattern(&name.node, expected_ty, args.len());
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
            ast::Pattern::Constructor(name, _) if !name.node.contains("::") => {
                if let Some(target) = self.union_pattern_target(subject_ty, &name.node) {
                    remaining.retain(|member| {
                        !target
                            .variants
                            .iter()
                            .any(|variant| union_member_type_matches(member, &variant.source_ty))
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
        let source_union_ty = Self::option_payload_union(scrutinee_ty).unwrap_or(scrutinee_ty);
        let Some(source_union_name) = source_union_ty.union_type_name() else {
            return Ok(Vec::new());
        };

        let mut lowered = Vec::new();
        for variant in target.variants {
            let temp_name = format!("__incan_union_{}_v{}", primary_binding.name, variant.source_index);
            let Some(variant_path) = source_union_ty.union_variant_path(variant.source_index) else {
                continue;
            };
            let union_pattern = Pattern::Enum {
                name: source_union_name.clone(),
                variant: variant_path,
                fields: vec![Pattern::Var(temp_name.clone())],
            };
            let pattern = if Self::option_payload_union(scrutinee_ty).is_some() {
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
                        let guard_uses_binding = guard
                            .as_ref()
                            .is_some_and(|guard| crate::scanners::expr_uses_binding_name(guard, &binding.name));
                        let body_uses_binding = crate::scanners::expr_uses_binding_name(&body, &binding.name);
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
    pub(in crate::lower) fn lower_match_arms(
        &mut self,
        arms: &[Spanned<ast::MatchArm>],
        scrutinee: &TypedExpr,
    ) -> Result<Vec<MatchArm>, LoweringError> {
        let scrutinee_ty = &scrutinee.ty;
        let subject_binding_name = Self::direct_match_subject_binding_name(scrutinee);
        let mut lowered_arms = Vec::new();
        let mut remaining_union_members = scrutinee_ty.union_members().map(|members| members.to_vec());

        // The surrounding statement-block counter contains the sum of every textual arm, but only one unguarded arm
        // executes. Give each arm a counter containing its own reads plus reads after the match. This permits the final
        // read in every mutually exclusive arm to move the same value without inventing a `Clone` requirement. Guarded
        // arms stay on the established conservative counter: a guard can read a value, fail, and allow a later arm to
        // execute. Once all unguarded arms are lowered, restore the counter with the complete syntactic arm total
        // consumed so following source retains the established straight-line last-use behavior.
        let arms_are_mutually_exclusive = arms.iter().all(|arm| arm.node.guard.is_none());
        let arm_read_counts = arms
            .iter()
            .map(|arm| self.count_match_arm_ident_reads(&arm.node))
            .collect::<Vec<_>>();
        let mut all_arm_reads = HashMap::<String, usize>::new();
        for counts in &arm_read_counts {
            for (name, count) in counts {
                *all_arm_reads.entry(name.clone()).or_default() += count;
            }
        }
        let original_remaining_reads = self.remaining_ident_reads.clone();
        let mut remaining_reads_after_match = original_remaining_reads.clone();
        if arms_are_mutually_exclusive {
            for reads in &mut remaining_reads_after_match {
                for (name, count) in &all_arm_reads {
                    if let Some(remaining) = reads.get_mut(name) {
                        *remaining = remaining.saturating_sub(*count);
                    }
                }
            }
        }

        for (a, current_arm_reads) in arms.iter().zip(&arm_read_counts) {
            if arms_are_mutually_exclusive {
                self.remaining_ident_reads = remaining_reads_after_match.clone();
                for reads in &mut self.remaining_ident_reads {
                    for (name, count) in current_arm_reads {
                        if let Some(remaining) = reads.get_mut(name) {
                            *remaining += count;
                        }
                    }
                }
            }
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
                    ast::Pattern::Constructor(name, _) if !name.node.contains("::") => self
                        .union_pattern_target(scrutinee_ty, &name.node)
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
                        ast::Pattern::Constructor(name, _) if !name.node.contains("::") => self
                            .union_pattern_target(scrutinee_ty, &name.node)
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
                    let arms = match self.lower_narrowed_union_capture_arms(a, scrutinee_ty, target, &capture_bindings)
                    {
                        Ok(arms) => arms,
                        Err(error) => {
                            self.remaining_ident_reads = original_remaining_reads;
                            return Err(error);
                        }
                    };
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
            let (pattern, literal_guard) = Self::hoist_nested_string_literal_patterns(pattern);
            self.push_scope();
            self.define_match_pattern_bindings_for_expected_type(&a.node.pattern.node, expected_ty);
            let arm_result = (|| {
                let guard = a.node.guard.as_ref().map(|g| self.lower_expr_spanned(g)).transpose()?;
                let guard = Self::conjoin_match_guards(literal_guard, guard);
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
            match arm_result {
                Ok(arm) => lowered_arms.push(arm),
                Err(error) => {
                    self.remaining_ident_reads = original_remaining_reads;
                    return Err(error);
                }
            }

            if a.node.guard.is_none()
                && let Some(remaining) = remaining_union_members.as_mut()
            {
                self.remove_covered_union_members(remaining, &a.node.pattern.node, scrutinee_ty);
            }
        }

        if arms_are_mutually_exclusive {
            self.remaining_ident_reads = remaining_reads_after_match;
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
            && !name.node.contains("::")
        {
            let target_ty = self.lower_type_pattern_name(&name.node);
            let option_wrapped_union = Self::option_payload_union(expected_ty);
            let union_ty = option_wrapped_union.unwrap_or(expected_ty);
            if let Some(variant_index) = union_ty.union_variant_index_for_member(&target_ty)
                && let Some(union_name) = union_ty.union_type_name()
                && let Some(variant_path) = union_ty.union_variant_path(variant_index)
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
                    variant: variant_path,
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
    pub(in crate::lower) fn lower_pattern(&mut self, p: &ast::Pattern) -> Pattern {
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
                            let canonical = self.resolve_field_alias(&name.node, &field.node);
                            named_fields.push((canonical, self.lower_pattern(&pat.node)));
                        }
                        ast::PatternArg::Positional(pat) => {
                            positional_fields.push(self.lower_pattern(&pat.node));
                        }
                    }
                }

                if has_named {
                    // The pattern names a subset of the fields; the checker recorded the rest (#1708). The struct
                    // pattern shape has no rest marker, so each omitted field is recorded as a wildcard, which is
                    // the shape the backend prints and Rust accepts in place of `..` for accessible fields.
                    if let Some(rest) = self.pattern_rest_fields_for(name.span) {
                        for field in rest {
                            if !named_fields.iter().any(|(named, _)| *named == field) {
                                named_fields.push((field.clone(), Pattern::Wildcard));
                            }
                        }
                    }
                    Pattern::Struct {
                        name: name.node.clone(),
                        fields: named_fields,
                    }
                } else {
                    let mut fields = positional_fields;
                    if has_named {
                        fields.extend(named_fields.into_iter().map(|(_, pat)| pat));
                    }
                    Pattern::Enum {
                        name: String::new(),
                        variant: name.node.clone(),
                        fields,
                    }
                }
            }
            ast::Pattern::Tuple(items) => Pattern::Tuple(items.iter().map(|i| self.lower_pattern(&i.node)).collect()),
            ast::Pattern::Group(pattern) => self.lower_pattern(&pattern.node),
            ast::Pattern::Or(items) => Pattern::Or(items.iter().map(|item| self.lower_pattern(&item.node)).collect()),
        }
    }

    /// Return the fields the checker recorded as unnamed by the constructor pattern whose name sits at `span`.
    ///
    /// Expression facts are keyed by span alone, so while an imported trait default is being expanded into an
    /// adopter the adopter's same-offset facts are not authority for that body (see `lower_expr_spanned`); the rest
    /// record is skipped there like every other span-keyed fact.
    fn pattern_rest_fields_for(&self, span: ast::Span) -> Option<Vec<String>> {
        if self.active_imported_trait_defaults.last().copied().unwrap_or(false) {
            return None;
        }
        self.type_info
            .as_ref()
            .and_then(|info| info.pattern_rest_fields(span))
            .map(<[String]>::to_vec)
    }

    /// The binding name a hoisted nested string literal takes, numbered per arm.
    fn hoisted_string_literal_binding_name(index: usize) -> String {
        format!("__incan_match_str_{index}")
    }

    /// The literal test a hoisted string literal contributes to its arm's guard: `binding == "value"`.
    ///
    /// The comparison is recorded as an ordinary `BinOp::Eq` over two `str` operands, the same fact a source
    /// `word == "answer"` records, so the backend prints its borrowed string comparison for it and every string
    /// carrier the position may hold (owned, borrowed or static) compares through one path.
    fn hoisted_string_literal_test(binding: &str, value: &str) -> TypedExpr {
        let left = TypedExpr::new(
            IrExprKind::Var {
                name: binding.to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::String,
        );
        let right = TypedExpr::new(IrExprKind::String(value.to_string()), IrType::String);
        TypedExpr::new(
            IrExprKind::BinOp {
                op: BinOp::Eq,
                left: Box::new(left),
                right: Box::new(right),
            },
            IrType::Bool,
        )
    }

    /// Join two optional boolean guards with `and`, keeping whichever exists when the other is absent.
    fn conjoin_match_guards(first: Option<TypedExpr>, second: Option<TypedExpr>) -> Option<TypedExpr> {
        Self::join_match_guards(first, second, BinOp::And)
    }

    /// Join two optional boolean guards with `op`, keeping whichever exists when the other is absent.
    fn join_match_guards(first: Option<TypedExpr>, second: Option<TypedExpr>, op: BinOp) -> Option<TypedExpr> {
        match (first, second) {
            (Some(first), Some(second)) => Some(TypedExpr::new(
                IrExprKind::BinOp {
                    op,
                    left: Box::new(first),
                    right: Box::new(second),
                },
                IrType::Bool,
            )),
            (first, None) => first,
            (None, second) => second,
        }
    }

    /// Rewrite the string literals nested inside an arm's pattern into bindings tested by the arm's guard.
    ///
    /// A string literal at the top of a pattern is matched against the scrutinee by the backend's own
    /// `str` handling, and stays as written. A string literal *inside* a tuple, struct or enum pattern has no such
    /// path: printed as a pattern token it is a `&str` that an owned `String` position refuses (#1707). The shape
    /// the backend does consume is a binding plus a guard, so each nested literal becomes a fresh
    /// `__incan_match_str_<n>` binding and its equality test is returned as the guard to conjoin ahead of the
    /// arm's own (the literal decides whether the arm applies; the user's guard runs only once it does). An
    /// alternation whose alternatives are all string literals shares one binding and tests them with `or`, the
    /// nested twin of the backend's top-level alternation handling. Any other alternation is left as written: its
    /// alternatives could only be rewritten as separate arms, and a literal inside one keeps its current shape.
    pub(in crate::lower) fn hoist_nested_string_literal_patterns(pattern: Pattern) -> (Pattern, Option<TypedExpr>) {
        let mut guard = None;
        let mut next_index = 0usize;
        let pattern = match pattern {
            Pattern::Tuple(items) => Pattern::Tuple(
                items
                    .into_iter()
                    .map(|item| Self::hoist_string_literal_subpattern(item, &mut next_index, &mut guard))
                    .collect(),
            ),
            Pattern::Struct { name, fields } => Pattern::Struct {
                name,
                fields: fields
                    .into_iter()
                    .map(|(field, item)| {
                        (
                            field,
                            Self::hoist_string_literal_subpattern(item, &mut next_index, &mut guard),
                        )
                    })
                    .collect(),
            },
            Pattern::Enum { name, variant, fields } => Pattern::Enum {
                name,
                variant,
                fields: fields
                    .into_iter()
                    .map(|item| Self::hoist_string_literal_subpattern(item, &mut next_index, &mut guard))
                    .collect(),
            },
            other => other,
        };
        (pattern, guard)
    }

    /// Rewrite one nested sub-pattern for [`Self::hoist_nested_string_literal_patterns`], accumulating the guard.
    fn hoist_string_literal_subpattern(
        pattern: Pattern,
        next_index: &mut usize,
        guard: &mut Option<TypedExpr>,
    ) -> Pattern {
        match pattern {
            Pattern::Literal(literal) => match &literal.kind {
                IrExprKind::String(value) => {
                    let binding = Self::hoisted_string_literal_binding_name(*next_index);
                    *next_index += 1;
                    let test = Self::hoisted_string_literal_test(&binding, value);
                    *guard = Self::conjoin_match_guards(guard.take(), Some(test));
                    Pattern::Var(binding)
                }
                _ => Pattern::Literal(literal),
            },
            Pattern::Or(items) => {
                let values: Option<Vec<String>> = items
                    .iter()
                    .map(|item| match item {
                        Pattern::Literal(literal) => match &literal.kind {
                            IrExprKind::String(value) => Some(value.clone()),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect();
                match values {
                    Some(values) if !values.is_empty() => {
                        let binding = Self::hoisted_string_literal_binding_name(*next_index);
                        *next_index += 1;
                        let alternatives = values
                            .iter()
                            .map(|value| Some(Self::hoisted_string_literal_test(&binding, value)))
                            .fold(None, |joined, test| Self::join_match_guards(joined, test, BinOp::Or));
                        *guard = Self::conjoin_match_guards(guard.take(), alternatives);
                        Pattern::Var(binding)
                    }
                    _ => Pattern::Or(items),
                }
            }
            Pattern::Tuple(items) => Pattern::Tuple(
                items
                    .into_iter()
                    .map(|item| Self::hoist_string_literal_subpattern(item, &mut *next_index, &mut *guard))
                    .collect(),
            ),
            Pattern::Struct { name, fields } => Pattern::Struct {
                name,
                fields: fields
                    .into_iter()
                    .map(|(field, item)| {
                        (
                            field,
                            Self::hoist_string_literal_subpattern(item, &mut *next_index, &mut *guard),
                        )
                    })
                    .collect(),
            },
            Pattern::Enum { name, variant, fields } => Pattern::Enum {
                name,
                variant,
                fields: fields
                    .into_iter()
                    .map(|item| Self::hoist_string_literal_subpattern(item, &mut *next_index, &mut *guard))
                    .collect(),
            },
            other @ (Pattern::Wildcard | Pattern::Var(_)) => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::decl::IrDeclKind;
    use crate::expr::{BinOp, FormatPart, IrExprKind, MatchArm, Pattern};
    use crate::lower::AstLowering;
    use crate::stmt::IrStmtKind;
    use crate::types::IrType;
    use crate::{IrProgram, TypedExpr};
    use incan_frontend::{lexer, parser, typechecker::TypeChecker};

    type TestResult = Result<(), String>;

    /// Lex, check and lower one source module through the same pipeline the compiler runs.
    fn lower_source(source: &str) -> Result<IrProgram, String> {
        let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| format!("typechecker failed: {errors:?}"))?;
        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        lowering
            .lower_program(&program)
            .map_err(|errors| format!("lowering failed: {errors:?}"))
    }

    /// The arms of the first `match` in the body of the named function, at statement or expression position.
    fn match_arms(program: &IrProgram, function: &str) -> Result<Vec<MatchArm>, String> {
        let body = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(func) if func.name == function => Some(&func.body),
                _ => None,
            })
            .ok_or_else(|| format!("missing function `{function}`"))?;
        body.iter()
            .find_map(|stmt| match &stmt.kind {
                IrStmtKind::Match { arms, .. } => Some(arms.clone()),
                IrStmtKind::Expr(expr) | IrStmtKind::Return(Some(expr)) => match &expr.kind {
                    IrExprKind::Match { arms, .. } => Some(arms.clone()),
                    _ => None,
                },
                _ => None,
            })
            .ok_or_else(|| format!("`{function}` lowers no match"))
    }

    /// Render an arm's guard as `<lhs> == "<rhs>"` when it is one hoisted string-literal test.
    fn string_literal_test(guard: &TypedExpr) -> Option<(String, String)> {
        let IrExprKind::BinOp {
            op: BinOp::Eq,
            left,
            right,
        } = &guard.kind
        else {
            return None;
        };
        let (IrExprKind::Var { name, .. }, IrExprKind::String(value)) = (&left.kind, &right.kind) else {
            return None;
        };
        (left.ty == IrType::String && right.ty == IrType::String && guard.ty == IrType::Bool)
            .then(|| (name.clone(), value.clone()))
    }

    /// A string literal nested in a tuple pattern lowers to a binding tested by the arm's guard (#1707): the
    /// pattern shape the backend prints as a compilable `match` over an owned `String` item. The literal arm at the
    /// top level is left as written, and an arm without a nested literal gets no guard.
    #[test]
    fn nested_string_literal_pattern_lowers_to_a_guarded_binding_issue1707() -> TestResult {
        let source = r#"
def main() -> None:
    pair: tuple[int, str] = (42, "answer")
    match pair:
        (0, _) => println("first is zero")
        (_, "answer") => println("second is answer")
        _ => println("something else")
"#;
        let arms = match_arms(&lower_source(source)?, "main")?;
        let [zero, answer, rest] = arms.as_slice() else {
            return Err(format!("expected three arms, got {}", arms.len()));
        };
        assert!(zero.guard.is_none(), "an arm without a nested literal records no guard");
        assert!(rest.guard.is_none(), "the wildcard arm records no guard");

        let Pattern::Tuple(items) = &answer.pattern else {
            return Err(format!("expected a tuple pattern, got {:?}", answer.pattern));
        };
        let [Pattern::Wildcard, Pattern::Var(binding)] = items.as_slice() else {
            return Err(format!("the nested literal must become a binding, got {items:?}"));
        };
        let guard = answer
            .guard
            .as_ref()
            .ok_or("the literal test must be recorded as the arm's guard")?;
        assert_eq!(
            string_literal_test(guard),
            Some((binding.clone(), "answer".to_string())),
            "the guard compares the hoisted binding with the literal as two `str` operands"
        );
        Ok(())
    }

    /// A hoisted literal test runs ahead of the arm's own guard, and an alternation of string literals in one
    /// nested position shares a single binding tested with `or`.
    #[test]
    fn hoisted_literal_tests_conjoin_with_the_arm_guard_issue1707() -> TestResult {
        let source = r#"
def classify(pair: tuple[int, str]) -> str:
    match pair:
        (n, "yes" | "no") if n > 0 => return "answered"
        _ => return "open"
"#;
        let arms = match_arms(&lower_source(source)?, "classify")?;
        let answered = arms.first().ok_or("expected an answered arm")?;
        let Pattern::Tuple(items) = &answered.pattern else {
            return Err(format!("expected a tuple pattern, got {:?}", answered.pattern));
        };
        let [Pattern::Var(n), Pattern::Var(binding)] = items.as_slice() else {
            return Err(format!("the alternation must become one binding, got {items:?}"));
        };
        assert_eq!(n, "n");
        let guard = answered.guard.as_ref().ok_or("expected a guard")?;
        let IrExprKind::BinOp {
            op: BinOp::And,
            left: literal_tests,
            right: user_guard,
        } = &guard.kind
        else {
            return Err(format!(
                "the literal tests must be conjoined ahead of the user's guard, got {guard:?}"
            ));
        };
        let IrExprKind::BinOp {
            op: BinOp::Or,
            left: yes,
            right: no,
        } = &literal_tests.kind
        else {
            return Err(format!(
                "the alternatives must be tested with `or`, got {literal_tests:?}"
            ));
        };
        assert_eq!(string_literal_test(yes), Some((binding.clone(), "yes".to_string())));
        assert_eq!(string_literal_test(no), Some((binding.clone(), "no".to_string())));
        assert!(
            matches!(&user_guard.kind, IrExprKind::BinOp { op: BinOp::Gt, .. }),
            "the user's guard is kept as the second conjunct, got {user_guard:?}"
        );
        Ok(())
    }

    /// A constructor pattern naming a subset of a model's fields records the omitted fields as wildcards
    /// (#1708), the explicit form of `..` the backend already prints; alias keys resolve to canonical names.
    #[test]
    fn partial_constructor_pattern_records_its_rest_fields_as_wildcards_issue1708() -> TestResult {
        let source = r#"
model Account:
    tier: int
    name: str
    kind_ [alias="kind"]: str

def describe(a: Account) -> str:
    match a:
        Account(kind="premium") => return "Premium"
        _ => return "Other"
"#;
        let arms = match_arms(&lower_source(source)?, "describe")?;
        let premium = arms.first().ok_or("expected a premium arm")?;
        let Pattern::Struct { name, fields } = &premium.pattern else {
            return Err(format!("expected a struct pattern, got {:?}", premium.pattern));
        };
        assert_eq!(name, "Account");
        let shape = fields
            .iter()
            .map(|(field, pattern)| {
                let pattern = match pattern {
                    Pattern::Wildcard => "_".to_string(),
                    Pattern::Var(binding) => binding.clone(),
                    other => format!("{other:?}"),
                };
                format!("{field}: {pattern}")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            shape,
            vec![
                "kind_: __incan_match_str_0".to_string(),
                "tier: _".to_string(),
                "name: _".to_string()
            ],
            "the named field keeps its (hoisted) sub-pattern and every omitted field is a wildcard"
        );
        let guard = premium
            .guard
            .as_ref()
            .ok_or("the literal field test must be recorded as the guard")?;
        assert_eq!(
            string_literal_test(guard),
            Some(("__incan_match_str_0".to_string(), "premium".to_string()))
        );
        Ok(())
    }

    /// A tuple pattern over a `tuple[int, str]`-typed scrutinee binds its names with the element types, and the
    /// arm body reads them as ordinary locals (#1714). The checker defines the bindings; lowering projects the
    /// scrutinee's tuple type onto them.
    #[test]
    fn tuple_pattern_over_a_written_tuple_annotation_binds_its_names_issue1714() -> TestResult {
        let source = r#"
def main() -> None:
    pair: tuple[int, str] = (42, "hello")
    match pair:
        (0, _) => println("zero")
        (number, word) => println(f"{number} {word}")
"#;
        let arms = match_arms(&lower_source(source)?, "main")?;
        let bound = arms.get(1).ok_or("expected a binding arm")?;
        let Pattern::Tuple(items) = &bound.pattern else {
            return Err(format!("expected a tuple pattern, got {:?}", bound.pattern));
        };
        let [Pattern::Var(number), Pattern::Var(word)] = items.as_slice() else {
            return Err(format!("both items must be bindings, got {items:?}"));
        };
        assert_eq!((number.as_str(), word.as_str()), ("number", "word"));

        let mut reads = Vec::new();
        collect_format_reads(&bound.body, &mut reads);
        assert_eq!(
            reads,
            vec![
                ("number".to_string(), IrType::Int),
                ("word".to_string(), IrType::String)
            ],
            "the body reads the bindings with the tuple's element types"
        );
        Ok(())
    }

    /// Collect the `(name, type)` of every variable interpolated by an f-string inside `expr`.
    fn collect_format_reads(expr: &TypedExpr, reads: &mut Vec<(String, IrType)>) {
        match &expr.kind {
            IrExprKind::Format { parts } => {
                for part in parts {
                    if let FormatPart::Expr { expr, .. } = part {
                        if let IrExprKind::Var { name, .. } = &expr.kind {
                            reads.push((name.clone(), expr.ty.clone()));
                        }
                        collect_format_reads(expr, reads);
                    }
                }
            }
            IrExprKind::BuiltinCall { args, .. } => {
                for arg in args {
                    collect_format_reads(arg, reads);
                }
            }
            IrExprKind::Call { args, .. } => {
                for arg in args {
                    collect_format_reads(&arg.expr, reads);
                }
            }
            IrExprKind::Block { stmts, value } => {
                for stmt in stmts {
                    if let IrStmtKind::Expr(expr) = &stmt.kind {
                        collect_format_reads(expr, reads);
                    }
                }
                if let Some(value) = value {
                    collect_format_reads(value, reads);
                }
            }
            _ => {}
        }
    }
}
