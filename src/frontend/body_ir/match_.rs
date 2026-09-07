//! Lowering for `match` expressions and their patterns.

use super::control_flow::intersect_range_layouts;
use super::primitives::*;
use super::reads::*;
use super::refusals::*;
use super::*;

/// Where the names a pattern binds can still be read once that pattern has matched.
///
/// [`BodyBuilder::lower_match_pattern`] seeds each fresh binding's last-use countdown from this, and its two
/// callers disagree about what "afterwards" means. A `match` arm's bindings are visible only inside that arm, so
/// its guard and body are the whole live range. An `assert value is P` binding instead stays live for the rest of
/// the enclosing block, so its live range is the statement suffix following the assertion. Passing the counting
/// context in rather than hard-coding one keeps a single pattern lowerer serving both.
pub(super) enum PatternReadScope<'ast> {
    /// A `match` arm: its guard plus its body.
    MatchArm(&'ast ast::MatchArm),
    /// The statement suffix following an `assert value is P` in its enclosing block.
    FollowingStatements(&'ast [ast::Spanned<ast::Statement>]),
}

impl PatternReadScope<'_> {
    /// Count reads of `name` reachable from this scope, using the same source-order over-approximation
    /// [`count_reads_in_stmts`] documents.
    fn count_reads(&self, name: &str) -> usize {
        match self {
            Self::MatchArm(arm) => count_reads_in_match_arm(name, arm),
            Self::FollowingStatements(stmts) => count_reads_in_stmts(name, stmts),
        }
    }
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower a `match` expression (`ast::Expr::Match`) into a single [`bir::Rvalue::Match`], mirroring the existing
    /// Rust-emission backend's own `IrExprKind::Match { scrutinee, arms }` node -- see [`bir::Rvalue::Match`]'s docs
    /// for why matching stays one structured node rather than being decomposed into a chain of `If` statements, and
    /// [`bir::Pattern`]'s docs for the closed pattern vocabulary this mirrors and its two deliberate v0 gaps (no
    /// union-type pattern narrowing, no RFC 021 field-alias resolution).
    ///
    /// Bails the whole expression to an explicit unsupported placeholder *before* lowering the scrutinee when any
    /// arm's pattern contains a byte-string literal (a pattern form outside Body IR's current closed matching
    /// vocabulary -- see [`match_pattern_is_supported`]), mirroring [`Self::lower_binary`]'s "check before
    /// partially lowering" precedent so an unrepresentable pattern never produces a partially-lowered
    /// `Rvalue::Match`.
    pub(super) fn lower_match(
        &mut self,
        subject: &ast::Spanned<ast::Expr>,
        arms: &[ast::Spanned<ast::MatchArm>],
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(expr_span);
        if arms
            .iter()
            .any(|arm| !match_pattern_is_supported(&arm.node.pattern.node))
        {
            return self.unsupported_operand(
                "match arm with a byte-string literal pattern".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }

        let scrutinee_ty = self.resolve_ty(subject.span);
        let scrutinee_place = self.lower_expr_to_place(subject, scope, out);
        // Always read as `Borrow` -- see `bir::Rvalue::Match::scrutinee`'s own docs for why the overall scrutinee
        // read must not risk an unconditional move while individual pattern bindings below compute their own,
        // more precise facts against projected places rooted at this same scrutinee.
        let scrutinee_operand = bir::Operand::place(scrutinee_place.clone(), bir::OwnershipFact::Borrow, false);

        let range_layouts_before = self.materialized_range_locals.clone();
        let mut range_layouts_after_arms = Vec::with_capacity(arms.len());
        let mut lowered_arms = Vec::with_capacity(arms.len());
        for arm in arms {
            // At runtime exactly one arm executes. Lower every arm from the predecessor fact set so a range
            // constructed only in an earlier textual arm cannot authorize field projections in a later one.
            self.materialized_range_locals = range_layouts_before.clone();
            let arm_span = hir_span(arm.span);
            let arm_scope = self.new_scope(Some(scope), arm_span);

            // ---- Lower the pattern, declaring one fresh arm-scoped local per distinct bound name ----
            let mut seen: HashMap<String, bir::LocalId> = HashMap::new();
            let mut saved_bindings: Vec<(String, Option<bir::LocalId>)> = Vec::new();
            let pattern = self.lower_match_pattern(
                &arm.node.pattern,
                &scrutinee_ty,
                &scrutinee_place,
                arm_scope,
                &PatternReadScope::MatchArm(&arm.node),
                &mut seen,
                &mut saved_bindings,
            );

            // ---- Guard and body see this arm's own pattern bindings, shadowing any outer binding of the same name
            // ----
            let mut guard_stmts = Vec::new();
            let guard = arm
                .node
                .guard
                .as_ref()
                .map(|g| self.lower_expr_to_operand(g, arm_scope, &mut guard_stmts));

            let (body_stmts, result) = match &arm.node.body {
                ast::MatchBody::Expr(e) => {
                    let mut stmts = Vec::new();
                    let result = self.lower_expr_to_operand(e, arm_scope, &mut stmts);
                    (stmts, result)
                }
                ast::MatchBody::Block(block_stmts) => {
                    let mut stmts = Vec::new();
                    self.lower_block_into(block_stmts, arm_scope, &mut stmts);
                    self.insert_scope_drops(&mut stmts, arm_scope);
                    (stmts, bir::Operand::Constant(bir::Constant::Unit))
                }
            };

            // ---- Restore the enclosing scope's bindings before moving on to the next (mutually exclusive) arm ----
            for (name, previous) in saved_bindings {
                match previous {
                    Some(local) => {
                        self.bindings.insert(name, local);
                    }
                    None => {
                        self.bindings.remove(&name);
                    }
                }
            }
            range_layouts_after_arms.push(self.materialized_range_locals.clone());

            lowered_arms.push(bir::MatchArm {
                pattern,
                guard_stmts,
                guard,
                body_stmts,
                result,
            });
        }
        // Code after a match must be valid for whichever arm matched. Preserve a layout fact only when every
        // arm leaves it true; no-arm matches conservatively retain no constructed-layout fact.
        self.materialized_range_locals = intersect_range_layouts(range_layouts_after_arms);

        let ty = self.resolve_ty(expr_span);
        self.push_assign_temp(
            bir::Rvalue::Match {
                scrutinee: scrutinee_operand,
                arms: lowered_arms,
            },
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Recursively lower one source `ast::Pattern` node into a [`bir::Pattern`], declaring a fresh local in
    /// `arm_scope` the first time a bound name is encountered and reusing it for any later `Or`-alternative
    /// occurrence of the same name (`seen`) -- Incan's typechecker (RFC 071) requires every alternative of an
    /// `A(x) | B(x)` pattern to bind an identical name/type set, so Rust's own single shared binding slot per name
    /// is the correct target shape, not one local per occurrence. `saved_bindings` accumulates
    /// `(name, previous_local)` pairs so [`Self::lower_match`] can restore `self.bindings` to the enclosing scope
    /// once this arm's guard/body have both been lowered, the same save/restore shape [`Self::lower_closure`]
    /// already uses around its own params/captures.
    ///
    /// Both `match` arms and `assert value is P` (RFC 018) lower their patterns here, so "arm" below means
    /// whichever construct owns this pattern. The two differ in binding lifetime, and `arm_scope`/`reads` are the
    /// two knobs that express the difference: an arm binds into its own fresh scope and is read only within that
    /// arm, while an assertion binds into the enclosing block's scope and is read by the statements that follow it
    /// (see [`PatternReadScope`]). An assertion also keeps its bindings afterwards rather than restoring
    /// `saved_bindings`, which is [`Self::lower_assert`]'s decision to make, not this method's.
    ///
    /// `place` is the (possibly already-projected) scrutinee place this pattern node corresponds to; each
    /// recursive call into a `Tuple`/`Struct`/`Enum` sub-pattern extends it with one more
    /// [`bir::PlaceElem::Field`] projection -- named for a struct field, or the zero-based positional index as a
    /// string for a tuple/enum-variant positional field, mirroring [`Self::lower_tuple_unpack`]'s own tuple-element
    /// projection convention (`.0`/`.1` Rust tuple-field-access spelling) rather than inventing a second one.
    ///
    /// `expected_ty` is the best available type for this pattern node: propagated through [`Self::lower_match`]'s
    /// own `Self::resolve_ty` call on the scrutinee for the root pattern, and through
    /// [`tuple_element_types`] for `Tuple` sub-patterns (both already-established sources elsewhere in this file);
    /// a `Struct`/`Enum` constructor pattern's own fields fall back to [`IncanType::Unknown`] per field, since
    /// resolving a model/class/enum-variant's real field types would mean rebuilding the existing Rust-emission
    /// backend's own field-type-projection machinery (`constructor_field_types_for_pattern` in
    /// `src/backend/ir/lower/expr/patterns.rs`), which this bucket deliberately does not mirror -- see
    /// [`bir::Pattern`]'s own docs.
    #[allow(clippy::too_many_arguments)]
    /// Lower one pattern arm whose body is a statement block, restoring the enclosing bindings afterwards.
    ///
    /// Shared by the `if let` and `while let` desugarings (#1161), which both need the same three steps: test a
    /// pattern, run a block with its bindings in scope, then put the enclosing bindings back so they cannot leak
    /// past the construct. A `match` arm deliberately does not use this -- its body may be a single expression and
    /// it may carry a guard, both of which change what the arm produces.
    ///
    /// Restoring is what makes `if let`'s scoping honest: the pattern's names belong to the `then` branch only, and
    /// an `else` branch lowered afterwards must not see them.
    pub(super) fn lower_statement_pattern_arm(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        scrutinee_ty: &IncanType,
        scrutinee_place: &bir::Place,
        scope: bir::ScopeId,
        body: &[ast::Spanned<ast::Statement>],
        span: HirSourceSpan,
    ) -> bir::MatchArm {
        let arm_scope = self.new_scope(Some(scope), span);
        let mut seen: HashMap<String, bir::LocalId> = HashMap::new();
        let mut saved_bindings: Vec<(String, Option<bir::LocalId>)> = Vec::new();
        let lowered_pattern = self.lower_match_pattern(
            pattern,
            scrutinee_ty,
            scrutinee_place,
            arm_scope,
            &PatternReadScope::FollowingStatements(body),
            &mut seen,
            &mut saved_bindings,
        );

        let mut body_stmts = Vec::new();
        self.lower_block_into(body, arm_scope, &mut body_stmts);
        self.insert_scope_drops(&mut body_stmts, arm_scope);

        for (name, previous) in saved_bindings {
            match previous {
                Some(local) => {
                    self.bindings.insert(name, local);
                }
                None => {
                    self.bindings.remove(&name);
                }
            }
        }

        bir::MatchArm {
            pattern: lowered_pattern,
            guard_stmts: Vec::new(),
            guard: None,
            body_stmts,
            // A statement-block arm produces no value, exactly as a `match` arm with a block body does.
            result: bir::Operand::Constant(bir::Constant::Unit),
        }
    }

    #[allow(clippy::too_many_arguments)] // Carries one recursive pattern-lowering context; see the arm helper above.
    /// Lower one checked source pattern while retaining canonical binding and ownership facts.
    pub(super) fn lower_match_pattern(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        expected_ty: &IncanType,
        place: &bir::Place,
        arm_scope: bir::ScopeId,
        reads: &PatternReadScope<'_>,
        seen: &mut HashMap<String, bir::LocalId>,
        saved_bindings: &mut Vec<(String, Option<bir::LocalId>)>,
    ) -> bir::Pattern {
        let span = hir_span(pattern.span);
        match &pattern.node {
            ast::Pattern::Wildcard => bir::Pattern::Wildcard,
            ast::Pattern::Binding(name) => {
                let local = match seen.get(name) {
                    Some(&local) => local,
                    None => {
                        let total_reads = reads.count_reads(name);
                        let previous = self.bindings.get(name).copied();
                        let local = self.declare_new_local_with_reads(
                            name.clone(),
                            expected_ty.clone(),
                            arm_scope,
                            span,
                            total_reads,
                        );
                        seen.insert(name.clone(), local);
                        saved_bindings.push((name.clone(), previous));
                        local
                    }
                };
                let (fact, last_use) = self.ownership_fact_for_place(place, expected_ty);
                bir::Pattern::Var(bir::PatternBinding { local, fact, last_use })
            }
            // `match_pattern_is_supported` has already ruled out byte-string patterns before
            // `Self::lower_match` calls this method. Literal expression lowering is total; this is a separate
            // pattern-vocabulary boundary, retained until Body IR models byte-pattern matching semantics.
            ast::Pattern::Literal(lit) => bir::Pattern::Literal(lower_literal(lit)),
            ast::Pattern::Tuple(items) => {
                let element_types = tuple_element_types(expected_ty, items.len());
                let fields = items
                    .iter()
                    .zip(element_types.iter())
                    .enumerate()
                    .map(|(index, (item, element_ty))| {
                        let mut field_place = place.clone();
                        field_place
                            .projection
                            .push(bir::PlaceElem::synthetic_field(index.to_string()));
                        self.lower_match_pattern(item, element_ty, &field_place, arm_scope, reads, seen, saved_bindings)
                    })
                    .collect();
                bir::Pattern::Tuple(fields)
            }
            ast::Pattern::Constructor(name, args) => {
                // Preserve exact source-local pattern targets instead of asking the executor to recover a
                // declaration from the printed constructor spelling. The direct profile accepts only canonical
                // named fields of a plain model; every other structurally lowered constructor remains the
                // name-only fallback below and is visibly refused by replacement execution.
                if let Some(declaration) = self.local_nominal_declarations.get(&name.node)
                    && matches!(expected_ty, IncanType::Named(type_name) if type_name == &name.node)
                    && args.iter().all(|arg| matches!(arg, ast::PatternArg::Named(_, _)))
                    && self.type_info.resolved_identity(name.span) == Some(&declaration.canonical)
                {
                    let fields = args
                        .iter()
                        .filter_map(|arg| match arg {
                            ast::PatternArg::Named(field, pat) => {
                                let mut field_place = place.clone();
                                field_place.projection.push(bir::PlaceElem::field(
                                    field.node.clone(),
                                    self.type_info.resolved_identity(field.span).cloned(),
                                ));
                                Some((
                                    field.node.clone(),
                                    self.lower_match_pattern(
                                        pat,
                                        &IncanType::Unknown,
                                        &field_place,
                                        arm_scope,
                                        reads,
                                        seen,
                                        saved_bindings,
                                    ),
                                ))
                            }
                            ast::PatternArg::Positional(_) => None,
                        })
                        .collect();
                    return bir::Pattern::Nominal {
                        target: bir::NominalPatternTarget {
                            direct_declaration_id: declaration.direct_declaration_id.clone(),
                            canonical: declaration.canonical.clone(),
                            name: declaration.name.clone(),
                        },
                        fields,
                    };
                }

                if let Some((enum_name, variant_name)) =
                    name.node.rsplit_once("::").or_else(|| name.node.rsplit_once('.'))
                    && args.is_empty()
                    && matches!(expected_ty, IncanType::Named(type_name) if type_name == enum_name)
                    && let Some(declaration) = self.local_fieldless_enum_declarations.get(enum_name)
                    && let Some(variant) = declaration.variants.iter().find(|variant| variant.name == variant_name)
                    && self.type_info.resolved_identity(name.span) == Some(&variant.canonical)
                {
                    return bir::Pattern::FieldlessEnumVariant(bir::FieldlessEnumVariantTarget {
                        enum_declaration_id: declaration.direct_declaration_id.clone(),
                        enum_canonical: declaration.canonical.clone(),
                        variant_declaration_id: variant.direct_declaration_id.clone(),
                        variant_canonical: variant.canonical.clone(),
                        enum_name: declaration.name.clone(),
                        variant_name: variant.name.clone(),
                    });
                }

                if let Some(variant) = result_variant_kind(&name.node)
                    && let Some((ok_type, error_type)) = result_type_parts(expected_ty)
                    && args.len() == 1
                    && let [ast::PatternArg::Positional(payload)] = args.as_slice()
                {
                    let payload_type = match variant {
                        bir::ResultVariantKind::Ok => ok_type,
                        bir::ResultVariantKind::Err => error_type,
                    };
                    let lowered_payload =
                        self.lower_match_pattern(payload, payload_type, place, arm_scope, reads, seen, saved_bindings);
                    return bir::Pattern::Result {
                        variant,
                        fields: vec![lowered_payload],
                    };
                }

                // Mirrors the existing Rust-emission backend's own `lower_pattern` (non-union-aware) mapping
                // exactly: a mix of named and positional arguments (unusual, likely non-representative source)
                // still lowers every sub-pattern's own bindings for side effects, but only the named fields survive
                // into the constructed `Pattern` once `has_named` is known.
                let mut named_fields = Vec::new();
                let mut positional_fields = Vec::new();
                let mut has_named = false;
                let mut positional_index = 0usize;
                for arg in args {
                    match arg {
                        ast::PatternArg::Named(field, pat) => {
                            has_named = true;
                            let mut field_place = place.clone();
                            field_place.projection.push(bir::PlaceElem::field(
                                field.node.clone(),
                                self.type_info.resolved_identity(field.span).cloned(),
                            ));
                            let lowered = self.lower_match_pattern(
                                pat,
                                &IncanType::Unknown,
                                &field_place,
                                arm_scope,
                                reads,
                                seen,
                                saved_bindings,
                            );
                            named_fields.push((field.node.clone(), lowered));
                        }
                        ast::PatternArg::Positional(pat) => {
                            let mut field_place = place.clone();
                            field_place
                                .projection
                                .push(bir::PlaceElem::synthetic_field(positional_index.to_string()));
                            positional_index += 1;
                            let lowered = self.lower_match_pattern(
                                pat,
                                &IncanType::Unknown,
                                &field_place,
                                arm_scope,
                                reads,
                                seen,
                                saved_bindings,
                            );
                            positional_fields.push(lowered);
                        }
                    }
                }
                if has_named {
                    bir::Pattern::Struct {
                        canonical: self.type_info.resolved_identity(name.span).cloned(),
                        name: name.node.clone(),
                        fields: named_fields,
                    }
                } else {
                    bir::Pattern::Enum {
                        canonical: self.type_info.resolved_identity(name.span).cloned(),
                        name: String::new(),
                        variant: name.node.clone(),
                        fields: positional_fields,
                    }
                }
            }
            ast::Pattern::Group(inner) => {
                self.lower_match_pattern(inner, expected_ty, place, arm_scope, reads, seen, saved_bindings)
            }
            ast::Pattern::Or(items) => {
                let alternatives = items
                    .iter()
                    .map(|item| {
                        self.lower_match_pattern(item, expected_ty, place, arm_scope, reads, seen, saved_bindings)
                    })
                    .collect();
                bir::Pattern::Or(alternatives)
            }
        }
    }
}
