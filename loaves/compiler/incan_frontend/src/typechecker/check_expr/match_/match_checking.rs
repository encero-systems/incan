//! Match expression checking, union arm coverage, and match exhaustiveness.

use super::*;

impl TypeChecker {
    /// Split a constructor pattern name into its optional enum qualifier and variant segment.
    ///
    /// The parser normalizes qualified surface patterns like `Color.Red` to `Color::Red`, while bare constructors
    /// such as `Some` and `Ok` keep the unqualified spelling. Match checking needs both pieces separately so
    /// qualifier validation and variant symbol lookup stay consistent.
    pub(in crate::typechecker::check_expr) fn split_pattern_constructor_name(name: &str) -> (Option<&str>, &str) {
        match name.rsplit_once("::") {
            Some((qualifier, variant)) => (Some(qualifier), variant),
            None => (None, name),
        }
    }

    /// Whether a type spelled `written` in a pattern names the subject's type `subject_name`.
    ///
    /// The spellings agree, or they are two spellings of one compiled-library declaration: a consumer's import of the
    /// type and the provider-qualified key a dependency signature carries when the signature was imported before the
    /// type, or two public paths of the type. Both carry the declaration's identity, so the pattern names the subject's
    /// type.
    /// A retained storage key is compared through that same canonical declaration identity, never by parsing its
    /// spelling.
    pub(super) fn pattern_type_spelling_names_subject(&self, written: &str, subject_name: &str) -> bool {
        self.source_type_names_same_declaration(written, subject_name)
    }

    /// Whether an explicit pattern qualifier names the same enum-like scrutinee type being matched.
    pub(super) fn pattern_qualifier_matches_expected_type(expected_ty: &ResolvedType, qualifier: &str) -> bool {
        match expected_ty {
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => qualifier == type_name,
            _ => false,
        }
    }

    /// Resolve the semantic type captured by a union type pattern.
    ///
    /// A constructor pattern over a union may name either one concrete member (`A(value)`) or a transparent alias whose
    /// expanded union members are a subset of the scrutinee union (`Base(value)` where `Input = Union[Base, int]`).
    pub(super) fn union_pattern_target_type(&self, expected_ty: &ResolvedType, name: &str) -> Option<ResolvedType> {
        let target_ty = self.union_member_target_spelling(
            self.expand_type_aliases(resolve_type(&Type::Simple(name.to_string()), &self.symbols)),
        );
        let members = Self::expected_union_members(expected_ty)?;

        if let Some(target_members) = target_ty.union_members()
            && target_members.iter().all(|target| {
                members
                    .iter()
                    .any(|member| self.match_union_member_matches(member, target))
            })
        {
            return Some(union_ty(target_members.to_vec()));
        }

        members
            .iter()
            .find(|member| self.match_union_member_matches(member, &target_ty))
            .cloned()
    }

    /// Return the active union member set for a match subject, including `Option[Union[...]]`.
    pub(super) fn expected_union_members(expected_ty: &ResolvedType) -> Option<&[ResolvedType]> {
        if let Some(members) = expected_ty.union_members() {
            Some(members)
        } else if let Some(inner) = expected_ty.option_inner_type() {
            inner.union_members()
        } else {
            None
        }
    }

    /// Type-check a `match` expression and return its resolved type: the one type its arms unify to.
    ///
    /// Every arm produces the match's value, in statement position too, since the generated match needs one type
    /// across its arms. An expression arm is checked against `expected`, the type of the place the match is written to
    /// when there is one, so a literal arm takes that type as it does at the place itself. A block arm produces no
    /// value (`None`): it takes part only when it can complete, and a block that ends in `return`, `break` or
    /// `continue` cannot. The arm types unify as [`Self::unify_branch_value_types`] states: a narrower numeric arm is
    /// widened and a payload arm is wrapped in `Some` in the arm itself, and arms that share no type are refused with
    /// a type mismatch. Each arm is written to the expected type directly when that type is numeric or an `Option`
    /// that holds no union, the adaptations lowering makes in an arm; otherwise the arms unify among themselves and
    /// the whole match is written to the place, since an arm is not made a union member or any other type in place.
    /// Pattern checking retains native layouts before coverage reads them, so exhaustiveness never depends on an
    /// earlier match having selected the same native enum.
    pub(in crate::typechecker::check_expr) fn check_match(
        &mut self,
        subject: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
        _span: Span,
        expected: Option<&ResolvedType>,
    ) -> ResolvedType {
        let subject_ty = self.check_expr(subject);
        let mut subject_ty = self.settle_open_constructor_sides_in_place(subject, subject_ty);
        if matches!(subject_ty, ResolvedType::TypeToken(_)) {
            self.errors.push(errors::type_token_operation_not_supported(
                "pattern matching",
                subject.span,
            ));
            subject_ty = ResolvedType::Unknown;
        }
        self.note_loan_free_scrutinee(&subject_ty, subject.span);
        let subject_binding = if let Expr::Ident(name) = &subject.node {
            self.lookup_variable_info(name)
                .cloned()
                .map(|info| (name.clone(), info, subject.span))
        } else {
            None
        };
        let mut remaining_union_members = subject_ty.union_members().map(|members| members.to_vec());
        let view_param = self.pattern_view_param(subject);

        let mut arm_values = Vec::new();

        for arm in arms {
            let narrowed_subject_ty = remaining_union_members
                .as_ref()
                .and_then(|remaining| self.match_arm_remainder_type(&arm.node.pattern, remaining));
            let expected_ty = narrowed_subject_ty.as_ref().unwrap_or(&subject_ty);

            self.symbols.enter_scope(ScopeKind::Block);
            self.check_pattern(&arm.node.pattern, expected_ty);
            let views = self.enter_pattern_views(&arm.node.pattern.node, view_param.clone());
            if let (Some((name, info, span)), Some(ty)) = (&subject_binding, narrowed_subject_ty.clone()) {
                self.symbols.define_refined_binding(Symbol {
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

            if let Some(guard) = &arm.node.guard {
                let guard_ty = self.check_expr(guard);
                self.validate_truthiness_condition(&guard_ty, guard.span);
            }

            match &arm.node.body {
                MatchBody::Expr(e) => {
                    let arm_ty = match expected {
                        Some(expected) => self.check_expr_with_expected(e, Some(expected)),
                        None => self.check_expr(e),
                    };
                    arm_values.push((arm_ty, e.span));
                }
                MatchBody::Block(stmts) => {
                    self.check_statement_block(stmts);
                    if !block_cannot_complete(stmts) {
                        arm_values.push((ResolvedType::Unit, arm.span));
                    }
                }
            }

            self.exit_pattern_views(views);
            self.symbols.exit_scope();

            if arm.node.guard.is_none()
                && let Some(remaining) = remaining_union_members.as_mut()
            {
                self.remove_covered_union_members(remaining, &arm.node.pattern, &subject_ty);
            }
        }
        self.check_match_exhaustiveness(&subject_ty, arms, _span);
        self.note_dict_lookup_match(subject, arms);

        let arm_destination = expected
            .map(|expected| self.expand_type_aliases(expected.clone()))
            .filter(|expected| {
                (expected.is_option() || crate::typechecker::numeric_type_id_for_compat(expected).is_some())
                    && !Self::type_holds_union(expected)
            });
        self.unify_branch_value_types(&arm_values, arm_destination.as_ref())
            .or_else(|| arm_values.first().map(|(ty, _)| ty.clone()))
            .unwrap_or(ResolvedType::Unit)
    }

    /// Return the type represented by the as-yet-uncovered union members for wildcard and binding arms.
    pub(super) fn match_arm_remainder_type(
        &self,
        pattern: &Spanned<Pattern>,
        remaining: &[ResolvedType],
    ) -> Option<ResolvedType> {
        match &pattern.node {
            Pattern::Wildcard | Pattern::Binding(_) if !remaining.is_empty() => {
                Some(Self::localize_union_member(union_ty(remaining.to_vec())))
            }
            Pattern::Group(inner) => self.match_arm_remainder_type(inner, remaining),
            _ => None,
        }
    }

    /// Compare exact checked union alternatives for both narrowing and exhaustiveness.
    ///
    /// Public nominal aliases share their selected artifact and canonical declaration. Assignment conversions do
    /// not prove coverage: even mutually compatible numeric types remain separate union alternatives.
    pub(super) fn match_union_member_matches(&self, member: &ResolvedType, target: &ResolvedType) -> bool {
        match (
            self.public_library_type_identity_for_type(member),
            self.public_library_type_identity_for_type(target),
        ) {
            (Some((left, left_args)), Some((right, right_args))) => {
                return left == right
                    && left_args.len() == right_args.len()
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.match_union_member_matches(left, right));
            }
            (Some(_), None) | (None, Some(_)) => return false,
            (None, None) => {}
        }
        match (member, target) {
            (ResolvedType::Generic(left, left_args), ResolvedType::Generic(right, right_args)) => {
                left == right
                    && left_args.len() == right_args.len()
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.match_union_member_matches(left, right))
            }
            (ResolvedType::Tuple(left), ResolvedType::Tuple(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| self.match_union_member_matches(left, right))
            }
            (ResolvedType::FrozenList(left), ResolvedType::FrozenList(right))
            | (ResolvedType::FrozenSet(left), ResolvedType::FrozenSet(right))
            | (ResolvedType::TypeToken(left), ResolvedType::TypeToken(right))
            | (ResolvedType::Ref(left), ResolvedType::Ref(right))
            | (ResolvedType::RefMut(left), ResolvedType::RefMut(right)) => self.match_union_member_matches(left, right),
            (ResolvedType::FrozenDict(left_key, left_value), ResolvedType::FrozenDict(right_key, right_value)) => {
                self.match_union_member_matches(left_key, right_key)
                    && self.match_union_member_matches(left_value, right_value)
            }
            _ => member == target,
        }
    }

    /// Remove the union members covered by a pattern from the remaining-arm accumulator.
    pub(super) fn remove_covered_union_members(
        &self,
        remaining: &mut Vec<ResolvedType>,
        pattern: &Spanned<Pattern>,
        subject_ty: &ResolvedType,
    ) {
        match &pattern.node {
            Pattern::Constructor(name, sub_patterns) => {
                let (enum_qualifier_opt, ctor_name) = Self::split_pattern_constructor_name(name.node.as_str());
                if enum_qualifier_opt.is_none()
                    && let Some(member_ty) = self.union_pattern_target_type(subject_ty, ctor_name)
                {
                    if let Some(target_members) = member_ty.union_members() {
                        remaining.retain(|member| {
                            !target_members.iter().any(|target| {
                                self.match_union_member_matches(member, target)
                                    && self.union_type_pattern_payload_is_exhaustive(sub_patterns, target)
                            })
                        });
                    } else {
                        remaining.retain(|member| {
                            !self.match_union_member_matches(member, &member_ty)
                                || !self.union_type_pattern_payload_is_exhaustive(sub_patterns, &member_ty)
                        });
                    }
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

    /// Return whether a union type pattern covers every value of the member it names.
    ///
    /// `int(n)` covers the `int` member, while `int(0)` leaves every other integer uncovered. The nested pattern is
    /// the payload stored by the generated union wrapper, so it is judged by the same pattern-matrix walk used for an
    /// enum variant payload rather than treating the outer type name as proof of complete coverage (#1876).
    pub(super) fn union_type_pattern_payload_is_exhaustive(
        &self,
        sub_patterns: &[PatternArg],
        member_ty: &ResolvedType,
    ) -> bool {
        let rows = sub_patterns
            .iter()
            .find_map(|arg| match arg {
                PatternArg::Positional(pattern) => Some(vec![vec![Some(&pattern.node)]]),
                PatternArg::Named(_, _) => None,
            })
            .unwrap_or_default();
        self.coverage_rows_exhaustive(rows, std::slice::from_ref(member_ty), 0)
    }

    /// Check the one positional payload accepted by `Some`, `Ok`, and `Err` patterns.
    pub(super) fn check_single_payload_constructor_pattern(
        &mut self,
        constructor: &Spanned<String>,
        sub_patterns: &[PatternArg],
        payload_ty: &ResolvedType,
    ) {
        if sub_patterns.len() != 1 {
            self.errors.push(errors::builtin_arity(
                &constructor.node,
                1,
                sub_patterns.len(),
                constructor.span,
            ));
        }
        let mut checked_positional = false;
        for arg in sub_patterns {
            match arg {
                PatternArg::Positional(pattern) if !checked_positional => {
                    self.check_pattern(pattern, payload_ty);
                    checked_positional = true;
                }
                PatternArg::Positional(_) => {}
                PatternArg::Named(_, pattern) => self
                    .errors
                    .push(errors::named_pattern_not_supported(&constructor.node, pattern.span)),
            }
        }
    }

    /// Check that a match expression covers all possible cases.
    ///
    /// For enums, `Result`, and `Option`, verifies every variant is handled. Wildcards (`_`) satisfy all remaining
    /// cases. A variant counts as handled only when the unguarded arms that name it also cover its payload, which the
    /// pattern-matrix walk in `match_coverage` decides (#1741). Emits a
    /// [`non_exhaustive_match`](errors::non_exhaustive_match) error naming each missing variant, spelled with wildcard
    /// payloads (`Some(_)`) when arms name it but leave part of its payload uncovered. Every other subject is held to
    /// the same walk and reported as missing `_`: literal arms over an `int` or a `str` never cover it on their own.
    pub(super) fn check_match_exhaustiveness(
        &mut self,
        subject_ty: &ResolvedType,
        arms: &[Spanned<MatchArm>],
        span: Span,
    ) {
        if let Some(members) = Self::expected_union_members(subject_ty) {
            let mut remaining = members.to_vec();
            let mut option_variants = HashSet::new();
            let mut has_wildcard = false;
            for arm in arms.iter().filter(|arm| arm.node.guard.is_none()) {
                self.remove_covered_union_members(&mut remaining, &arm.node.pattern, subject_ty);
                if subject_ty.is_option() {
                    self.collect_pattern_coverage(
                        &arm.node.pattern.node,
                        subject_ty,
                        &mut option_variants,
                        &mut has_wildcard,
                    );
                }
            }
            let mut missing = remaining.iter().map(ToString::to_string).collect::<Vec<_>>();
            let none = constructors::as_str(ConstructorId::None);
            if subject_ty.is_option() && !has_wildcard && !option_variants.contains(none) {
                missing.push(none.to_string());
            }
            if !missing.is_empty() {
                self.errors.push(errors::non_exhaustive_match(&missing, span));
            }
            return;
        }
        let rows = expand_coverage_heads(
            arms.iter()
                .filter(|arm| arm.node.guard.is_none())
                .map(|arm| vec![Some(&arm.node.pattern.node)])
                .collect(),
        );
        if rows.iter().any(coverage_row_head_is_wild) {
            return;
        }
        let Some(variant_constructors) = self.match_subject_variant_constructors(subject_ty) else {
            // Any other subject (a scalar, a tuple, a model): the arms must cover every value, so arms that are only
            // literals over an open type need a wildcard (#1741).
            if !self.coverage_rows_exhaustive(rows, std::slice::from_ref(subject_ty), 0) {
                self.errors.push(errors::non_exhaustive_match(&["_".to_string()], span));
            }
            return;
        };
        // A variant is covered when the arms that name it cover its payload too: `Some(0)` alone leaves `Some(_)`
        // open, while `Ok(Some(x))` beside `Ok(None)` covers `Ok` (#1741).
        let missing: Vec<String> = variant_constructors
            .iter()
            .filter_map(|constructor| {
                let payload_rows = self.specialize_coverage_rows(&rows, constructor);
                let named = !payload_rows.is_empty();
                (!self.coverage_rows_exhaustive(payload_rows, constructor.payload_types(), 0))
                    .then(|| constructor.missing_label(named))
            })
            .collect();
        if !missing.is_empty() {
            self.errors.push(errors::non_exhaustive_match(&missing, span));
        }
    }

    /// Add the variants covered by a pattern to the match-exhaustiveness accumulator.
    pub(super) fn collect_pattern_coverage(
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
                let variant_name = if name.node.contains("::") {
                    name.node.split("::").last().unwrap_or(&name.node).to_string()
                } else {
                    name.node.clone()
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
