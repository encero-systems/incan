//! Check `match` expressions, patterns, and exhaustiveness.
//!
//! This module validates `match` expressions by type-checking each arm, binding pattern variables,
//! and ensuring exhaustiveness for enums, `Result`, and `Option`.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;
use incan_core::interop::RustItemKind;
use incan_core::lang::surface::constructors;
use incan_core::lang::surface::constructors::ConstructorId;
use incan_core::lang::types::collections::{self, CollectionTypeId};

use super::TypeChecker;

#[derive(Clone)]
struct PatternBinding {
    ty: ResolvedType,
    span: Span,
}

/// Return a stable name list for binding-set comparison and diagnostics.
fn sorted_binding_names(bindings: &HashMap<String, PatternBinding>) -> Vec<String> {
    let mut names: Vec<_> = bindings.keys().cloned().collect();
    names.sort();
    names
}

impl TypeChecker {
    /// Split a constructor pattern name into its optional enum qualifier and variant segment.
    ///
    /// The parser normalizes qualified surface patterns like `Color.Red` to `Color::Red`, while bare constructors
    /// such as `Some` and `Ok` keep the unqualified spelling. Match checking needs both pieces separately so
    /// qualifier validation and variant symbol lookup stay consistent.
    fn split_pattern_constructor_name(name: &str) -> (Option<&str>, &str) {
        match name.rsplit_once("::") {
            Some((qualifier, variant)) => (Some(qualifier), variant),
            None => (None, name),
        }
    }

    /// Whether an explicit pattern qualifier names the same enum-like scrutinee type being matched.
    fn pattern_qualifier_matches_expected_type(expected_ty: &ResolvedType, qualifier: &str) -> bool {
        match expected_ty {
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => qualifier == type_name,
            _ => false,
        }
    }

    /// Resolve the member type targeted by a union type pattern.
    fn union_pattern_member_type(&self, expected_ty: &ResolvedType, name: &str) -> Option<ResolvedType> {
        let target_ty = self.expand_type_aliases(resolve_type(&Type::Simple(name.to_string()), &self.symbols));
        let members = if let Some(members) = expected_ty.union_members() {
            members
        } else if let Some(inner) = expected_ty.option_inner_type() {
            inner.union_members()?
        } else {
            return None;
        };

        members
            .iter()
            .find(|member| self.types_compatible(member, &target_ty) && self.types_compatible(&target_ty, member))
            .cloned()
    }

    /// Type-check a `match` expression and return its resolved type.
    pub(in crate::frontend::typechecker::check_expr) fn check_match(
        &mut self,
        subject: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
        _span: Span,
    ) -> ResolvedType {
        let subject_ty = self.check_expr(subject);
        let subject_binding = if let Expr::Ident(name) = &subject.node {
            self.lookup_variable_info(name)
                .cloned()
                .map(|info| (name.clone(), info, subject.span))
        } else {
            None
        };
        let mut remaining_union_members = subject_ty.union_members().map(|members| members.to_vec());

        self.check_match_exhaustiveness(&subject_ty, arms, _span);

        let mut arm_types = Vec::new();

        for arm in arms {
            let narrowed_subject_ty = remaining_union_members
                .as_ref()
                .and_then(|remaining| self.match_arm_remainder_type(&arm.node.pattern, remaining));
            let expected_ty = narrowed_subject_ty.as_ref().unwrap_or(&subject_ty);

            self.symbols.enter_scope(ScopeKind::Block);
            self.check_pattern(&arm.node.pattern, expected_ty);
            if let (Some((name, info, span)), Some(ty)) = (&subject_binding, narrowed_subject_ty.clone()) {
                self.symbols.define(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty,
                        is_mutable: info.is_mutable,
                        is_used: false,
                    }),
                    span: *span,
                    scope: 0,
                });
                if info.is_mutable {
                    self.mutable_bindings.insert(name.clone());
                }
            }

            let arm_ty = match &arm.node.body {
                MatchBody::Expr(e) => self.check_expr(e),
                MatchBody::Block(stmts) => {
                    for stmt in stmts {
                        self.check_statement(stmt);
                    }
                    ResolvedType::Unit
                }
            };
            arm_types.push(arm_ty);

            self.symbols.exit_scope();

            if let Some(remaining) = remaining_union_members.as_mut() {
                self.remove_covered_union_members(remaining, &arm.node.pattern, &subject_ty);
            }
        }

        arm_types.first().cloned().unwrap_or(ResolvedType::Unit)
    }

    /// Return the type represented by the as-yet-uncovered union members for wildcard and binding arms.
    fn match_arm_remainder_type(&self, pattern: &Spanned<Pattern>, remaining: &[ResolvedType]) -> Option<ResolvedType> {
        match &pattern.node {
            Pattern::Wildcard | Pattern::Binding(_) if !remaining.is_empty() => Some(union_ty(remaining.to_vec())),
            Pattern::Group(inner) => self.match_arm_remainder_type(inner, remaining),
            _ => None,
        }
    }

    /// Whether two union member candidates are equivalent for match-arm narrowing.
    fn match_union_member_matches(&self, member: &ResolvedType, target: &ResolvedType) -> bool {
        self.types_compatible(member, target) && self.types_compatible(target, member)
    }

    /// Remove the union members covered by a pattern from the remaining-arm accumulator.
    fn remove_covered_union_members(
        &self,
        remaining: &mut Vec<ResolvedType>,
        pattern: &Spanned<Pattern>,
        subject_ty: &ResolvedType,
    ) {
        match &pattern.node {
            Pattern::Constructor(name, _) => {
                let (enum_qualifier_opt, ctor_name) = Self::split_pattern_constructor_name(name.as_str());
                if enum_qualifier_opt.is_none()
                    && let Some(member_ty) = self.union_pattern_member_type(subject_ty, ctor_name)
                {
                    remaining.retain(|member| !self.match_union_member_matches(member, &member_ty));
                }
            }
            Pattern::Or(alternatives) => {
                for alternative in alternatives {
                    self.remove_covered_union_members(remaining, alternative, subject_ty);
                }
            }
            Pattern::Group(inner) => self.remove_covered_union_members(remaining, inner, subject_ty),
            Pattern::Wildcard | Pattern::Binding(_) => remaining.clear(),
            _ => {}
        }
    }

    /// Type-check a pattern against an expected type, defining bindings in the current scope.
    pub(in crate::frontend::typechecker) fn check_pattern(
        &mut self,
        pattern: &Spanned<Pattern>,
        expected_ty: &ResolvedType,
    ) {
        match &pattern.node {
            Pattern::Wildcard => {}
            Pattern::Binding(name) => {
                self.symbols.define(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: expected_ty.clone(),
                        is_mutable: false,
                        is_used: false,
                    }),
                    span: pattern.span,
                    scope: 0,
                });
            }
            Pattern::Group(inner) => {
                self.check_pattern(inner, expected_ty);
            }
            Pattern::Or(alternatives) => {
                self.check_or_pattern(alternatives, expected_ty);
            }
            Pattern::Literal(_) => {}
            Pattern::Constructor(name, sub_patterns) => {
                let (enum_qualifier_opt, ctor_name) = Self::split_pattern_constructor_name(name.as_str());
                if enum_qualifier_opt.is_none()
                    && let Some(member_ty) = self.union_pattern_member_type(expected_ty, ctor_name)
                {
                    let mut positional = None;
                    for arg in sub_patterns {
                        match arg {
                            PatternArg::Positional(pat) => {
                                positional = Some(pat);
                                break;
                            }
                            PatternArg::Named(_, pat) => {
                                self.errors.push(errors::named_pattern_not_supported(name, pat.span));
                            }
                        }
                    }
                    if let Some(pat) = positional {
                        self.check_pattern(pat, &member_ty);
                    }
                    return;
                }

                let qualifier_matches_expected = enum_qualifier_opt
                    .is_none_or(|qualifier| Self::pattern_qualifier_matches_expected_type(expected_ty, qualifier));

                if qualifier_matches_expected && let Some(cid) = constructors::from_str(ctor_name) {
                    match cid {
                        ConstructorId::Ok => {
                            if let ResolvedType::Generic(type_name, args) = expected_ty
                                && type_name == collections::as_str(CollectionTypeId::Result)
                                && !args.is_empty()
                            {
                                let mut positional = None;
                                for arg in sub_patterns {
                                    match arg {
                                        PatternArg::Positional(pat) => {
                                            positional = Some(pat);
                                            break;
                                        }
                                        PatternArg::Named(_, pat) => {
                                            self.errors.push(errors::named_pattern_not_supported(name, pat.span));
                                        }
                                    }
                                }
                                if let Some(pat) = positional {
                                    self.check_pattern(pat, &args[0]);
                                }
                                return;
                            }
                        }
                        ConstructorId::Err => {
                            if let ResolvedType::Generic(type_name, args) = expected_ty
                                && type_name == collections::as_str(CollectionTypeId::Result)
                                && args.len() >= 2
                            {
                                let mut positional = None;
                                for arg in sub_patterns {
                                    match arg {
                                        PatternArg::Positional(pat) => {
                                            positional = Some(pat);
                                            break;
                                        }
                                        PatternArg::Named(_, pat) => {
                                            self.errors.push(errors::named_pattern_not_supported(name, pat.span));
                                        }
                                    }
                                }
                                if let Some(pat) = positional {
                                    self.check_pattern(pat, &args[1]);
                                }
                                return;
                            }
                        }
                        ConstructorId::Some => {
                            if let ResolvedType::Generic(type_name, args) = expected_ty
                                && type_name == collections::as_str(CollectionTypeId::Option)
                                && !args.is_empty()
                            {
                                let mut positional = None;
                                for arg in sub_patterns {
                                    match arg {
                                        PatternArg::Positional(pat) => {
                                            positional = Some(pat);
                                            break;
                                        }
                                        PatternArg::Named(_, pat) => {
                                            self.errors.push(errors::named_pattern_not_supported(name, pat.span));
                                        }
                                    }
                                }
                                if let Some(pat) = positional {
                                    self.check_pattern(pat, &args[0]);
                                }
                                return;
                            }
                        }
                        ConstructorId::None => {
                            return;
                        }
                    }
                }

                let ctor_name = if name.contains("::") {
                    name.split("::").last().unwrap_or(name)
                } else {
                    name.as_str()
                };

                let model_or_class_fields = match expected_ty {
                    ResolvedType::Named(type_name) if ctor_name == type_name => self
                        .lookup_type_info(type_name)
                        .and_then(|type_info| match type_info {
                            TypeInfo::Model(model_info) => Some(model_info.fields.clone()),
                            TypeInfo::Class(class_info) => Some(class_info.fields.clone()),
                            _ => None,
                        })
                        .map(|fields| (type_name, fields)),
                    _ => None,
                };

                if let Some((type_name, fields)) = model_or_class_fields {
                    let mut provided = HashSet::new();
                    for arg in sub_patterns {
                        match arg {
                            PatternArg::Positional(pat) => {
                                self.errors
                                    .push(errors::positional_pattern_not_supported(type_name, pat.span));
                            }
                            PatternArg::Named(field_name, pat) => {
                                let Some((canonical_name, info)) =
                                    self.resolve_field_info(&fields, field_name, true, true)
                                else {
                                    self.errors.push(errors::missing_field(type_name, field_name, pat.span));
                                    continue;
                                };

                                if !provided.insert(canonical_name.clone()) {
                                    self.errors.push(errors::duplicate_pattern_field(
                                        type_name,
                                        canonical_name.as_str(),
                                        pat.span,
                                    ));
                                    continue;
                                }
                                self.check_pattern(pat, &info.ty);
                            }
                        }
                    }
                    return;
                }

                let variant_name = ctor_name;

                let positional_count = sub_patterns
                    .iter()
                    .filter(|a| matches!(a, PatternArg::Positional(_)))
                    .count();

                let incan_resolution = self.incan_enum_constructor_payload_types(
                    expected_ty,
                    variant_name,
                    positional_count,
                    enum_qualifier_opt,
                );
                let rust_resolution =
                    self.rust_enum_constructor_payload_types(expected_ty, name.as_str(), positional_count);
                let field_types: Option<Vec<ResolvedType>> =
                    incan_resolution.clone().or_else(|| match rust_resolution.as_ref() {
                        Some(RustEnumPatternResolution::PayloadTypes(fields)) => Some(fields.clone()),
                        Some(RustEnumPatternResolution::QualifierMismatch) | None => None,
                    });

                match field_types {
                    Some(fields) => {
                        self.check_constructor_subpatterns_enum_like(
                            name.as_str(),
                            sub_patterns,
                            Some(fields.as_slice()),
                        );
                    }
                    None => {
                        let permissive = self.match_subject_allows_unknown_rust_enum_payloads(
                            expected_ty,
                            name.as_str(),
                            rust_resolution.as_ref(),
                        );
                        if !permissive && !matches!(expected_ty, ResolvedType::Unknown) {
                            self.errors.push(errors::unknown_match_constructor_pattern(
                                name.as_str(),
                                &expected_ty.to_string(),
                                pattern.span,
                            ));
                        }
                        self.check_constructor_subpatterns_enum_like(name.as_str(), sub_patterns, None);
                    }
                }
            }
            Pattern::Tuple(sub_patterns) => {
                if let ResolvedType::Tuple(elem_types) = expected_ty {
                    for (pat, elem_ty) in sub_patterns.iter().zip(elem_types.iter()) {
                        self.check_pattern(pat, elem_ty);
                    }
                }
            }
        }
    }

    /// Type-check alternatives in isolated scopes, then define only the agreed binding set in the surrounding arm
    /// scope.
    ///
    /// Without the isolation step, `A(x) | B(y)` would accidentally leak both `x` and `y` into the branch body even
    /// though no single successful match can provide both names. RFC 071 requires every alternative to bind the same
    /// names with the same types before any branch-local binding is made visible.
    fn check_or_pattern(&mut self, alternatives: &[Spanned<Pattern>], expected_ty: &ResolvedType) {
        let mut binding_sets = Vec::new();

        for alternative in alternatives {
            let before = self.symbols.all_symbols().len();
            self.symbols.enter_scope(ScopeKind::Block);
            self.check_pattern(alternative, expected_ty);
            let bindings = self.collect_pattern_bindings_since(before);
            self.symbols.exit_scope();
            binding_sets.push((alternative.span, bindings));
        }

        let Some((_, first_bindings)) = binding_sets.first() else {
            return;
        };

        let expected_names = sorted_binding_names(first_bindings);
        let mut agreement_ok = true;

        for (span, bindings) in binding_sets.iter().skip(1) {
            let found_names = sorted_binding_names(bindings);
            if found_names != expected_names {
                self.errors.push(errors::pattern_alternation_binding_mismatch(
                    &expected_names,
                    &found_names,
                    *span,
                ));
                agreement_ok = false;
                continue;
            }

            for name in &expected_names {
                let Some(expected) = first_bindings.get(name) else {
                    continue;
                };
                let Some(found) = bindings.get(name) else {
                    continue;
                };
                if expected.ty != found.ty {
                    self.errors.push(errors::pattern_alternation_binding_type_mismatch(
                        name,
                        &expected.ty.to_string(),
                        &found.ty.to_string(),
                        found.span,
                    ));
                    agreement_ok = false;
                }
            }
        }

        if !agreement_ok {
            return;
        }

        for name in expected_names {
            let Some(binding) = first_bindings.get(&name) else {
                continue;
            };
            self.symbols.define(Symbol {
                name,
                kind: SymbolKind::Variable(VariableInfo {
                    ty: binding.ty.clone(),
                    is_mutable: false,
                    is_used: false,
                }),
                span: binding.span,
                scope: 0,
            });
        }
    }

    /// Collect variable bindings defined while checking one isolated alternation alternative.
    fn collect_pattern_bindings_since(&self, start: usize) -> HashMap<String, PatternBinding> {
        self.symbols
            .all_symbols()
            .iter()
            .skip(start)
            .filter_map(|symbol| match &symbol.kind {
                SymbolKind::Variable(info) => Some((
                    symbol.name.clone(),
                    PatternBinding {
                        ty: info.ty.clone(),
                        span: symbol.span,
                    },
                )),
                _ => None,
            })
            .collect()
    }

    /// Positional sub-patterns for enum-like constructor patterns: known payload types per index, or all
    /// [`ResolvedType::Unknown`] when `known_fields` is `None` (Rust interop best-effort).
    fn check_constructor_subpatterns_enum_like(
        &mut self,
        ctor_label: &str,
        sub_patterns: &[PatternArg],
        known_fields: Option<&[ResolvedType]>,
    ) {
        let mut idx = 0usize;
        for arg in sub_patterns {
            match arg {
                PatternArg::Positional(pat) => {
                    if let Some(fields) = known_fields {
                        if let Some(field_ty) = fields.get(idx) {
                            self.check_pattern(pat, field_ty);
                        }
                    } else {
                        self.check_pattern(pat, &ResolvedType::Unknown);
                    }
                    idx += 1;
                }
                PatternArg::Named(_, pat) => {
                    self.errors
                        .push(errors::named_pattern_not_supported(ctor_label, pat.span));
                }
            }
        }
    }

    /// When constructor payload types are missing, only Rust-backed match subjects use permissive
    /// [`ResolvedType::Unknown`] payload checking, and only when the written constructor does not already prove the
    /// pattern is invalid (for example an explicit mismatched rusttype qualifier).
    fn match_subject_allows_unknown_rust_enum_payloads(
        &self,
        expected_ty: &ResolvedType,
        pattern_full_name: &str,
        rust_resolution: Option<&RustEnumPatternResolution>,
    ) -> bool {
        let is_rust_backed = match expected_ty {
            ResolvedType::RustPath(_) => true,
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => {
                self.lookup_type_info(type_name).is_some_and(|info| {
                    matches!(
                        info,
                        TypeInfo::Newtype(nt) if nt.is_rusttype && matches!(&nt.underlying, ResolvedType::RustPath(_))
                    )
                })
            }
            _ => false,
        };
        if !is_rust_backed {
            return false;
        }

        match rust_resolution {
            Some(RustEnumPatternResolution::PayloadTypes(_)) => true,
            Some(RustEnumPatternResolution::QualifierMismatch) => false,
            None => !pattern_full_name.contains("::"),
        }
    }

    /// Payload types for a source-defined enum variant, using the enum type's own metadata.
    ///
    /// Qualified patterns such as `Color.Red` should not depend on a module-level `Red` symbol being importable or
    /// winning same-scope shadowing. The scrutinee already tells us which enum is being matched, so resolve the
    /// variant from that enum's table.
    fn incan_enum_constructor_payload_types(
        &self,
        expected_ty: &ResolvedType,
        variant_name: &str,
        positional_count: usize,
        enum_qualifier_opt: Option<&str>,
    ) -> Option<Vec<ResolvedType>> {
        let enum_name = match expected_ty {
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => type_name,
            _ => return None,
        };
        if enum_qualifier_opt.is_some_and(|qualifier| qualifier != enum_name) {
            return None;
        }
        let Some(TypeInfo::Enum(enum_info)) = self.lookup_semantic_type_info(enum_name) else {
            return None;
        };
        let canonical_variant = enum_info
            .variant_aliases
            .get(variant_name)
            .map(String::as_str)
            .unwrap_or(variant_name);
        if !enum_info.variants.iter().any(|variant| variant == canonical_variant) {
            return None;
        }
        let fields = enum_info
            .variant_fields
            .get(canonical_variant)
            .cloned()
            .unwrap_or_default();
        if positional_count > fields.len() {
            return None;
        }
        Some(fields)
    }

    /// Tuple-variant payload types for `match` patterns on Rust-backed enum surfaces.
    ///
    /// Incan registers [`SymbolKind::Variant`] for source/manifest enums; imported Rust enums and prost-style oneofs
    /// are usually spelled as a `rusttype` / [`TypeInfo::Newtype`] wrapper over [`ResolvedType::RustPath`]. For a
    /// single positional sub-pattern, the payload type is `RustPath("{backing}::{variant}")`, consistent with Rust
    /// field/member path composition in `check_expr::access`. Multiple positional patterns without precise metadata use
    /// [`ResolvedType::Unknown`] per slot.
    ///
    /// When the scrutinee is already [`ResolvedType::RustPath`], any `Type::Variant` prefix in the pattern is not
    /// validated against that path (unlike [`ResolvedType::Named`] rusttypes, where the prefix must match the Incan
    /// type name). Payload typing still uses `{scrutinee_rust_path}::{variant}`.
    fn rust_enum_constructor_payload_types(
        &self,
        expected_ty: &ResolvedType,
        pattern_full_name: &str,
        positional_count: usize,
    ) -> Option<RustEnumPatternResolution> {
        let (enum_qualifier_opt, variant_segment) = match pattern_full_name.rsplit_once("::") {
            Some((e, v)) => (Some(e), v),
            None => (None, pattern_full_name),
        };

        let base_rust_path: String = match expected_ty {
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => {
                if let Some(q) = enum_qualifier_opt
                    && q != type_name.as_str()
                {
                    return Some(RustEnumPatternResolution::QualifierMismatch);
                }
                let info = self.lookup_type_info(type_name)?;
                match info {
                    TypeInfo::Newtype(nt) if nt.is_rusttype => match &nt.underlying {
                        ResolvedType::RustPath(p) => p.clone(),
                        _ => return None,
                    },
                    _ => return None,
                }
            }
            ResolvedType::RustPath(p) => p.clone(),
            _ => return None,
        };

        let (metadata_rust_path, _) = self.rust_path_base_and_args(base_rust_path.as_str());
        if let Some(meta) = self.rust_item_metadata_for_path(metadata_rust_path.as_str())
            && let RustItemKind::Type(info) = meta.kind
            && let Some(variant) = info.variants.iter().find(|variant| variant.name == variant_segment)
        {
            let fields: Vec<ResolvedType> = variant
                .fields
                .iter()
                .map(|field| self.resolved_type_from_rust_shape(field))
                .collect();
            return Some(RustEnumPatternResolution::payloads(fields));
        }

        Some(RustEnumPatternResolution::payloads(match positional_count {
            0 => vec![],
            1 => vec![ResolvedType::RustPath(format!("{base_rust_path}::{variant_segment}"))],
            n => (0..n).map(|_| ResolvedType::Unknown).collect(),
        }))
    }

    /// Check that a match expression covers all possible cases.
    ///
    /// For enums, `Result`, and `Option`, verifies every variant is handled. Wildcards
    /// (`_`) satisfy all remaining cases. Emits a [`non_exhaustive_match`](errors::non_exhaustive_match)
    /// error if patterns are missing.
    fn check_match_exhaustiveness(&mut self, subject_ty: &ResolvedType, arms: &[Spanned<MatchArm>], span: Span) {
        let variants = if let Some(members) = subject_ty.union_members() {
            Some(members.iter().map(ToString::to_string).collect())
        } else if let Some(inner) = subject_ty.option_inner_type()
            && let Some(members) = inner.union_members()
        {
            let mut variants: Vec<String> = members.iter().map(ToString::to_string).collect();
            variants.push(constructors::as_str(ConstructorId::None).to_string());
            Some(variants)
        } else if let ResolvedType::Named(name) = subject_ty {
            match self.lookup_type_info(name) {
                Some(TypeInfo::Enum(enum_info)) => Some(enum_info.variants.clone()),
                _ => None,
            }
        } else if subject_ty.is_result() || subject_ty.is_option() {
            if subject_ty.is_result() {
                Some(vec![
                    constructors::as_str(ConstructorId::Ok).to_string(),
                    constructors::as_str(ConstructorId::Err).to_string(),
                ])
            } else {
                Some(vec![
                    constructors::as_str(ConstructorId::Some).to_string(),
                    constructors::as_str(ConstructorId::None).to_string(),
                ])
            }
        } else {
            None
        };

        if let Some(all_variants) = variants {
            let mut covered: HashSet<String> = HashSet::new();
            let mut has_wildcard = false;

            for arm in arms {
                self.collect_pattern_coverage(&arm.node.pattern.node, subject_ty, &mut covered, &mut has_wildcard);
            }

            if !has_wildcard {
                let missing: Vec<String> = all_variants.iter().filter(|v| !covered.contains(*v)).cloned().collect();

                if !missing.is_empty() {
                    self.errors.push(errors::non_exhaustive_match(&missing, span));
                }
            }
        }
    }

    /// Add the variants covered by a pattern to the match-exhaustiveness accumulator.
    fn collect_pattern_coverage(
        &self,
        pattern: &Pattern,
        subject_ty: &ResolvedType,
        covered: &mut HashSet<String>,
        has_wildcard: &mut bool,
    ) {
        match pattern {
            Pattern::Wildcard | Pattern::Binding(_) => {
                *has_wildcard = true;
            }
            Pattern::Literal(Literal::None) if subject_ty.is_option() => {
                covered.insert(constructors::as_str(ConstructorId::None).to_string());
            }
            Pattern::Constructor(name, _) => {
                let variant_name = if subject_ty.union_members().is_some()
                    || subject_ty
                        .option_inner_type()
                        .is_some_and(|inner| inner.union_members().is_some())
                {
                    self.expand_type_aliases(resolve_type(&Type::Simple(name.clone()), &self.symbols))
                        .to_string()
                } else if name.contains("::") {
                    name.split("::").last().unwrap_or(name).to_string()
                } else {
                    name.clone()
                };
                covered.insert(variant_name);
            }
            Pattern::Or(alternatives) => {
                for alternative in alternatives {
                    self.collect_pattern_coverage(&alternative.node, subject_ty, covered, has_wildcard);
                }
            }
            Pattern::Group(inner) => {
                self.collect_pattern_coverage(&inner.node, subject_ty, covered, has_wildcard);
            }
            _ => {}
        }
    }
}

enum RustEnumPatternResolution {
    PayloadTypes(Vec<ResolvedType>),
    QualifierMismatch,
}

impl RustEnumPatternResolution {
    fn payloads(fields: Vec<ResolvedType>) -> Self {
        Self::PayloadTypes(fields)
    }
}
