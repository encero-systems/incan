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
    /// [`bir::Pattern`]'s docs for the closed pattern vocabulary, retained union member targets, and the remaining
    /// RFC 021 field-alias refinement gap.
    ///
    /// Bails the whole expression to an explicit unsupported placeholder *before* lowering the scrutinee when any
    /// arm's pattern contains a byte-string literal (a pattern form outside Body IR's current closed matching
    /// vocabulary -- see [`match_pattern_is_supported`]), mirroring [`Self::lower_binary`]'s "check before
    /// partially lowering" precedent so an unrepresentable pattern never produces a partially-lowered
    /// `Rvalue::Match`. Source reads in an arm preserve enclosing owners across alternative paths; arm-local
    /// payload transfers remain governed by their checked pattern facts.
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

        let scrutinee_place = self.lower_expr_to_place(subject, scope, out);
        let scrutinee_ty = if scrutinee_place.projection.is_empty() {
            scrutinee_place
                .local_id()
                .and_then(|local| self.locals.get(local.index()))
                .map_or_else(|| self.resolve_ty(subject.span), |local| local.ty.clone())
        } else {
            self.resolve_ty(subject.span)
        };
        let in_place = self.matches_in_place(subject.span, &scrutinee_ty);
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
            let enclosing_in_place = std::mem::replace(&mut self.in_place_pattern, in_place);
            let mut pattern = self.lower_match_pattern(
                &arm.node.pattern,
                &scrutinee_ty,
                &scrutinee_place,
                arm_scope,
                &PatternReadScope::MatchArm(&arm.node),
                &mut seen,
                &mut saved_bindings,
            );
            self.in_place_pattern = enclosing_in_place;
            if arm.node.guard.is_none() {
                self.retain_owned_carrier_payload_move(&scrutinee_place, &mut pattern);
            }

            // ---- Guard and body see this arm's own pattern bindings, shadowing any outer binding of the same name
            // ----
            let mut guard_stmts = Vec::new();
            self.conditional_scopes.push(arm_scope);
            let guard = arm
                .node
                .guard
                .as_ref()
                .map(|g| self.lower_expr_to_operand(g, arm_scope, &mut guard_stmts));

            let (body_stmts, result) = match &arm.node.body {
                ast::MatchBody::Expr(e) => {
                    let mut stmts = Vec::new();
                    let result = self.lower_boundary_value(e, None, arm_scope, &mut stmts);
                    (stmts, result)
                }
                ast::MatchBody::Block(block_stmts) => {
                    let mut stmts = Vec::new();
                    self.lower_block_into(block_stmts, arm_scope, &mut stmts);
                    self.insert_scope_drops(&mut stmts, arm_scope);
                    (stmts, bir::Operand::Constant(bir::Constant::Unit))
                }
            };
            self.conditional_scopes.pop();

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

    /// Transfer a single owned native payload from an unguarded match on a fresh enum temporary or the last
    /// read of an owned native iterator item, local binding or owned parameter. No later read can reuse that carrier,
    /// and a successful arm has no other payload to preserve. Borrowed parameters, loop-reused carriers, nested
    /// patterns and guards retain their facts. A final owned local read authorizes transfer independently of whether
    /// its payload's declaration is available in this consumer. Checked non-Clone tuple newtypes retain the same
    /// transfer boundary as their native payload; reference payloads retain their existing facts.
    fn retain_owned_carrier_payload_move(&mut self, source: &bir::Place, pattern: &mut bir::Pattern) {
        if !source.projection.is_empty() {
            return;
        }
        let Some(id) = source.local_id() else {
            return;
        };
        let Some(local) = self.locals.get(id.index()) else {
            return;
        };
        if matches!(local.ty, IncanType::Ref(_) | IncanType::RefMut(_)) {
            return;
        }
        let owned_last_read = (self.native_enum_payload_bindings.contains(&id)
            || matches!(
                local.origin,
                bir::LocalOrigin::Parameter | bir::LocalOrigin::UserBinding
            ))
            && !self.is_borrowed_parameter(id)
            && !self.loop_reuses_local(id)
            && !self.pending_argument_reads.contains(&id)
            && self
                .remaining_reads
                .get(&id)
                .is_some_and(|remaining| remaining.remaining == 1);
        if local.origin != bir::LocalOrigin::Temporary && !owned_last_read {
            return;
        }
        let fields = match pattern {
            bir::Pattern::Result { fields, .. } | bir::Pattern::Enum { fields, .. } => fields,
            _ => return,
        };
        let [bir::Pattern::Var(binding)] = fields.as_mut_slice() else {
            return;
        };
        let Some(payload) = self.locals.get(binding.local.index()) else {
            return;
        };
        let owned_local = local.origin == bir::LocalOrigin::UserBinding
            && owned_last_read
            && !matches!(payload.ty, IncanType::Ref(_) | IncanType::RefMut(_));
        if (owned_local || self.payload_needs_owned_transfer(&payload.ty))
            && matches!(
                binding.fact,
                bir::OwnershipFact::Borrow | bir::OwnershipFact::Clone | bir::OwnershipFact::Move
            )
        {
            binding.fact = bir::OwnershipFact::Move;
            self.native_enum_payload_bindings.insert(binding.local);
        }
    }

    /// Identify native owners and checked non-Clone tuple newtypes through owned carrier arguments.
    /// References never grant an owned transfer, and source declarations are selected by retained canonical identity.
    fn payload_needs_owned_transfer(&self, ty: &IncanType) -> bool {
        let name = match ty {
            IncanType::Generic { base, args } => {
                if args.iter().any(|argument| self.payload_needs_owned_transfer(argument)) {
                    return true;
                }
                base
            }
            IncanType::Tuple(fields) => {
                return fields.iter().any(|field| self.payload_needs_owned_transfer(field));
            }
            IncanType::Named(name) => name,
            _ => return false,
        };
        if bir::rust_item_identity(name).is_some() {
            return true;
        }
        parameter_types::retained_nominal_identity(name, self.type_info).is_some_and(|identity| {
            self.local_nominal_declarations
                .values()
                .find(|declaration| &declaration.canonical == identity)
                .is_some_and(|declaration| {
                    declaration.fields == ["0"] && !declaration.derives.iter().any(|derive| derive == "Clone")
                })
        })
    }

    /// Return whether the `match`, `if let` or `while let` over the scrutinee at `span` matches it in place.
    ///
    /// The checker records a scrutinee when an arm changes a part of a writable place through a name the pattern
    /// binds (#1561); legacy then matches through `&mut` so the change lands in the place. A union-typed scrutinee
    /// keeps the ordinary match, because its arms bind narrowed members rather than parts of the place.
    pub(super) fn matches_in_place(&self, span: ast::Span, scrutinee_ty: &IncanType) -> bool {
        !matches!(scrutinee_ty, IncanType::Generic { base, .. } if base == "Union")
            && self.type_info.match_scrutinee_is_changed_in_place(span)
    }

    /// Lower one pattern arm whose body is a statement block, restoring the enclosing bindings afterwards.
    ///
    /// Shared by the `if let` and `while let` desugarings (#1161), which both need the same three steps: test a
    /// pattern, run a block with its bindings in scope, then put the enclosing bindings back so they cannot leak
    /// past the construct. A `match` arm deliberately does not use this -- its body may be a single expression and
    /// it may carry a guard, both of which change what the arm produces.
    ///
    /// Restoring is what makes `if let`'s scoping honest: the pattern's names belong to the `then` branch only, and
    /// an `else` branch lowered afterwards must not see them. `in_place` is [`Self::matches_in_place`] for the
    /// construct's scrutinee; it applies to the pattern only, never to patterns nested in the body.
    /// Unguarded statement arms use the same checked native payload transfer as expression matches; a fresh owned
    /// Option/Result temporary must not require cloning its payload merely because the source spells `if let`.
    /// Source reads in the conditional body retain enclosing owners for the other path and its scope-exit drops.
    #[allow(clippy::too_many_arguments)] // The scrutinee's type, place and in-place fact travel together into one arm.
    pub(super) fn lower_statement_pattern_arm(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        scrutinee_ty: &IncanType,
        scrutinee_place: &bir::Place,
        in_place: bool,
        scope: bir::ScopeId,
        body: &[ast::Spanned<ast::Statement>],
        span: HirSourceSpan,
    ) -> bir::MatchArm {
        let arm_scope = self.new_scope(Some(scope), span);
        let mut seen: HashMap<String, bir::LocalId> = HashMap::new();
        let mut saved_bindings: Vec<(String, Option<bir::LocalId>)> = Vec::new();
        let enclosing_in_place = std::mem::replace(&mut self.in_place_pattern, in_place);
        let mut lowered_pattern = self.lower_match_pattern(
            pattern,
            scrutinee_ty,
            scrutinee_place,
            arm_scope,
            &PatternReadScope::FollowingStatements(body),
            &mut seen,
            &mut saved_bindings,
        );
        self.in_place_pattern = enclosing_in_place;
        self.retain_owned_carrier_payload_move(scrutinee_place, &mut lowered_pattern);

        let mut body_stmts = Vec::new();
        self.conditional_scopes.push(arm_scope);
        self.lower_block_into(body, arm_scope, &mut body_stmts);
        self.conditional_scopes.pop();
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

    /// Recursively lower one source `ast::Pattern` node into a [`bir::Pattern`], declaring a fresh local in
    /// `arm_scope` the first time a bound name is encountered and reusing it for any later `Or`-alternative occurrence
    /// of the same name (`seen`) -- Incan's typechecker (RFC 071) requires every alternative of an `A(x) | B(x)`
    /// pattern to bind an identical name/type set, so Rust's own single shared binding slot per name is the correct
    /// target shape, not one local per occurrence. `saved_bindings` accumulates `(name, previous_local)` pairs so
    /// [`Self::lower_match`] can restore `self.bindings` to the enclosing scope once this arm's guard/body have both
    /// been lowered, the same save/restore shape [`Self::lower_closure`] already uses around its own params/captures.
    ///
    /// `match` arms, `if let`/`while let` (through [`Self::lower_statement_pattern_arm`]) and `assert value is P`
    /// (RFC 018) all lower their patterns here, so "arm" below means whichever construct owns this pattern. They
    /// differ in binding lifetime, and `arm_scope`/`reads` are the two knobs that express the difference: an arm
    /// binds into its own fresh scope and is read only within that arm, while an assertion binds into the enclosing
    /// block's scope and is read by the statements that follow it (see [`PatternReadScope`]). An assertion also
    /// keeps its bindings afterwards rather than restoring `saved_bindings`, which is [`Self::lower_assert`]'s
    /// decision to make, not this method's.
    ///
    /// `place` is the (possibly already-projected) scrutinee place this pattern node corresponds to; each
    /// recursive call into a `Tuple`/`Struct`/`Enum` sub-pattern extends it with one more
    /// [`bir::PlaceElem::Field`] projection -- named for a struct field, or the zero-based positional index as a
    /// string for a tuple/enum-variant positional field, mirroring [`Self::lower_tuple_unpack`]'s own tuple-element
    /// projection convention (`.0`/`.1` Rust tuple-field-access spelling) rather than inventing a second one.
    ///
    /// The type a node is lowered against comes from two sources, and the checker's wins. `TypeChecker::check_pattern`
    /// records the checked type of every pattern node at that node's span (#1245), so a constructor payload -- an
    /// `Option`'s element, a user or imported enum variant's declared payload, a model field, or the borrow-wrapped
    /// form of any of those -- is read back here rather than re-derived; this is the same fact `for` patterns
    /// carry (#1125), and it is what a binding's [`bir::OwnershipFact`] follows from. `expected_ty` is the threaded
    /// fallback for a node the checker recorded no type for: [`Self::lower_match`]'s scrutinee type for the root,
    /// [`tuple_element_types`] for `Tuple` elements, [`result_type_parts`] for an intrinsic `Result` payload, and the
    /// retained intrinsic `Option` carrier for `Some`'s single payload.
    /// Caller-open parts of a checked binding type take the retained scrutinee's matching payload type; fixed node
    /// types and declaration-owned parameters remain authoritative. A generic constructor's sub-patterns thread
    /// [`IncanType::Unknown`], because lowering has no payload resolution of its own and must not grow one that
    /// could disagree with the checker's.
    #[allow(clippy::too_many_arguments)] // Carries one recursive pattern-lowering context; see the rustdoc above.
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
        let literal_storage_type = expected_ty;

        // ---- Retain the checked node type, closing only caller-open parts from the retained scrutinee ----
        let checked_ty = self.resolve_ty(pattern.span);
        let settled_ty = self.fill_open_type_parts(&checked_ty, expected_ty);
        let expected_ty = if matches!(checked_ty, IncanType::Unknown) {
            expected_ty
        } else {
            &settled_ty
        };

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
                let (mut fact, mut last_use) = self.ownership_fact_for_place(place, expected_ty);
                if fact == bir::OwnershipFact::Borrow && self.temporary_native_payload(place, expected_ty) {
                    fact = bir::OwnershipFact::Move;
                    last_use = true;
                    self.native_enum_payload_bindings.insert(local);
                }
                // An arm changes this part of the scrutinee through the name, so the name borrows the part itself.
                if self.in_place_pattern
                    && !place.projection.is_empty()
                    && !expected_ty.abi_v0_facts().ownership.is_trivially_copy()
                {
                    fact = bir::OwnershipFact::MutBorrow;
                }
                bir::Pattern::Var(bir::PatternBinding { local, fact, last_use })
            }
            // `match_pattern_is_supported` has already ruled out byte-string patterns before
            // `Self::lower_match` calls this method. Literal expression lowering is total; this is a separate
            // pattern-vocabulary boundary, retained until Body IR models byte-pattern matching semantics.
            ast::Pattern::Literal(lit) => bir::Pattern::Literal(lower_checked_literal(lit, literal_storage_type)),
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
                            .push(bir::PlaceElem::structural_field(index.to_string()));
                        self.lower_match_pattern(item, element_ty, &field_place, arm_scope, reads, seen, saved_bindings)
                    })
                    .collect();
                bir::Pattern::Tuple(fields)
            }
            ast::Pattern::Constructor(name, args) => self.lower_constructor_pattern(
                pattern,
                name,
                args,
                expected_ty,
                place,
                arm_scope,
                reads,
                seen,
                saved_bindings,
            ),
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

    /// A temporary Option/Result owns its native payload and has no source binding that can observe a later read.
    /// Preserve that move as a Body IR fact rather than asking native lowering to invent Clone for foreign resources.
    fn temporary_native_payload(&self, place: &bir::Place, ty: &IncanType) -> bool {
        let native = match ty {
            IncanType::Named(name) | IncanType::Generic { base: name, .. } => bir::rust_item_identity(name).is_some(),
            _ => false,
        };
        native
            && place.projection.len() == 1
            && place
                .local_id()
                .and_then(|id| self.locals.get(id.index()))
                .is_some_and(|local| {
                    local.origin == bir::LocalOrigin::Temporary
                        && matches!(&local.ty, IncanType::Generic { base, .. } if base == "Option" || base == "Result")
                })
    }

    /// Dispatch a checked constructor through its carrier, nominal and enum identity boundaries. References expose
    /// the retained referent layout for constructor selection; payload binding types and ownership remain checked
    /// independently by `lower_match_pattern`.
    #[allow(clippy::too_many_arguments)] // Carries one pattern and its recursive lowering context.
    fn lower_constructor_pattern(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        name: &ast::Spanned<ast::Ident>,
        args: &[ast::PatternArg],
        expected_ty: &IncanType,
        place: &bir::Place,
        arm_scope: bir::ScopeId,
        reads: &PatternReadScope<'_>,
        seen: &mut HashMap<String, bir::LocalId>,
        saved_bindings: &mut Vec<(String, Option<bir::LocalId>)>,
    ) -> bir::Pattern {
        let mut expected_ty = expected_ty;
        while let IncanType::Ref(inner) | IncanType::RefMut(inner) = expected_ty {
            expected_ty = inner;
        }
        if let Some(value) = self.lower_rust_enum_pattern(name, args, place, arm_scope, reads, seen, saved_bindings) {
            return value;
        }
        if let Some(value) =
            self.lower_union_pattern(pattern, name, args, place, arm_scope, reads, seen, saved_bindings)
        {
            return value;
        }
        if let Some(value) = self.lower_nominal_pattern(name, args, place, arm_scope, reads, seen, saved_bindings) {
            return value;
        }
        if let Some(value) = self.lower_fieldless_enum_pattern(name, args) {
            return value;
        }
        if let Some(value) =
            self.lower_intrinsic_result_pattern(name, args, expected_ty, place, arm_scope, reads, seen, saved_bindings)
        {
            return value;
        }
        if let Some(value) =
            self.lower_intrinsic_option_pattern(name, args, expected_ty, place, arm_scope, reads, seen, saved_bindings)
        {
            return value;
        }
        self.lower_generic_constructor_pattern(name, args, expected_ty, place, arm_scope, reads, seen, saved_bindings)
    }

    /// Order native payload patterns by the checker's selected metadata fields, filling unnamed storage with wildcards.
    /// A native variant identity authorizes no source-spelling lookup or guessed positional field.
    #[allow(clippy::too_many_arguments)]
    fn lower_rust_enum_pattern(
        &mut self,
        name: &ast::Spanned<ast::Ident>,
        args: &[ast::PatternArg],
        place: &bir::Place,
        arm_scope: bir::ScopeId,
        reads: &PatternReadScope<'_>,
        seen: &mut HashMap<String, bir::LocalId>,
        saved_bindings: &mut Vec<(String, Option<bir::LocalId>)>,
    ) -> Option<bir::Pattern> {
        let identity = self.type_info.resolved_identity(name.span)?.clone();
        let incan_semantics_core::SymbolOrigin::RustCrate(path) = &identity.origin else {
            return None;
        };
        let layout = self.type_info.rust.checked_enums.get(&path.join("::"))?;
        let (_, types, names) = layout
            .variants
            .iter()
            .find(|(variant, _, _)| *variant == identity.declaration_name)?;
        let mut fields = vec![bir::Pattern::Wildcard; types.len()];
        for (position, arg) in args.iter().enumerate() {
            let (slot, pattern) = match arg {
                ast::PatternArg::Positional(pattern) => (position, pattern),
                ast::PatternArg::Named(field, pattern) => (names.iter().position(|name| *name == field.node)?, pattern),
            };
            let mut field_place = place.clone();
            field_place
                .projection
                .push(bir::PlaceElem::structural_field(slot.to_string()));
            // The checker retained each payload sub-pattern's instantiated payload type at its span; a literal's
            // storage takes that type, not a guess.
            let payload_ty = self.resolve_ty(pattern.span);
            let lowered = self.lower_match_pattern(
                pattern,
                &payload_ty,
                &field_place,
                arm_scope,
                reads,
                seen,
                saved_bindings,
            );
            *fields.get_mut(slot)? = lowered;
        }
        Some(bir::Pattern::Enum {
            canonical: Some(identity.clone()),
            name: String::new(),
            variant: identity.declaration_name,
            fields,
        })
    }

    /// Retain the checked union member and optional carrier before projecting payload bindings.
    #[allow(clippy::too_many_arguments)] // Carries the recursive pattern context without reconstructing bindings.
    fn lower_union_pattern(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        name: &ast::Spanned<ast::Ident>,
        args: &[ast::PatternArg],
        place: &bir::Place,
        arm_scope: bir::ScopeId,
        reads: &PatternReadScope<'_>,
        seen: &mut HashMap<String, bir::LocalId>,
        saved_bindings: &mut Vec<(String, Option<bir::LocalId>)>,
    ) -> Option<bir::Pattern> {
        // Pattern checking records the selected union member at the constructor span. Keep that fact separate
        // from generic enum spelling.
        let mut target_ty = self.resolve_ty(name.span);
        if self.retain_nominal_carriers {
            super::nominal_carriers::retain_checked_site_type(&mut target_ty, name.span, self.type_info);
        }
        let subject_ty = self.resolve_ty(pattern.span);
        // A member pattern over an optional union (`int | str | None`) selects `Some` first, then the member
        // of its union payload.
        let optional_union = is_optional_union(&subject_ty);
        if (matches!(&subject_ty, IncanType::Generic { base, .. } if base == "Union") || optional_union)
            && !matches!(target_ty, IncanType::Unknown)
            && args.iter().all(|arg| matches!(arg, ast::PatternArg::Positional(_)))
        {
            let mut union_place = place.clone();
            if optional_union {
                union_place
                    .projection
                    .push(bir::PlaceElem::structural_field("0".to_string()));
            }
            let mut fields = Vec::new();
            for arg in args {
                if let ast::PatternArg::Positional(payload) = arg {
                    let mut payload_place = union_place.clone();
                    payload_place
                        .projection
                        .push(bir::PlaceElem::UnionMember { ty: target_ty.clone() });
                    fields.push(self.lower_match_pattern(
                        payload,
                        &target_ty,
                        &payload_place,
                        arm_scope,
                        reads,
                        seen,
                        saved_bindings,
                    ));
                }
            }
            let member = bir::Pattern::UnionMember { ty: target_ty, fields };
            if optional_union {
                return Some(bir::Pattern::Enum {
                    canonical: Some(crate::symbols::canonical_builtin_constructor_identity(
                        constructors::ConstructorId::Some,
                    )),
                    name: String::new(),
                    variant: constructors::as_str(constructors::ConstructorId::Some).to_string(),
                    fields: vec![member],
                });
            }
            return Some(member);
        }
        None
    }

    /// Retain canonical named fields of a checked nominal; aliases never establish layout equality.
    #[allow(clippy::too_many_arguments)] // Carries the recursive pattern context without reconstructing bindings.
    fn lower_nominal_pattern(
        &mut self,
        name: &ast::Spanned<ast::Ident>,
        args: &[ast::PatternArg],
        place: &bir::Place,
        arm_scope: bir::ScopeId,
        reads: &PatternReadScope<'_>,
        seen: &mut HashMap<String, bir::LocalId>,
        saved_bindings: &mut Vec<(String, Option<bir::LocalId>)>,
    ) -> Option<bir::Pattern> {
        // Retain the checked pattern target from local or imported declaration context. Source aliases
        // never participate in declaration or layout equality. The direct profile accepts only canonical
        // named fields of a plain model; every other structurally lowered constructor remains the
        // name-only fallback below and is visibly refused by replacement execution.
        let selected = self.type_info.resolved_identity(name.span).or_else(|| {
            self.type_info
                .expressions
                .body_ir_nominal_pattern_owners
                .get(&name.span)
        });
        if let Some(declaration) = self
            .local_nominal_declarations
            .values()
            .find(|declaration| selected == Some(&declaration.canonical))
            && args.iter().all(|arg| matches!(arg, ast::PatternArg::Named(_, _)))
            && selected == Some(&declaration.canonical)
        {
            let fields = args
                .iter()
                .filter_map(|arg| match arg {
                    ast::PatternArg::Named(field, pat) => {
                        let identity = self.type_info.resolved_identity(field.span).cloned();
                        let name = canonical_pattern_field(declaration, identity.as_ref(), &field.node);
                        let mut field_place = place.clone();
                        field_place
                            .projection
                            .push(bir::PlaceElem::field(name.clone(), identity));
                        Some((
                            name,
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
            return Some(bir::Pattern::Nominal {
                target: bir::NominalPatternTarget {
                    direct_declaration_id: declaration.direct_declaration_id.clone(),
                    canonical: declaration.canonical.clone(),
                    name: declaration.name.clone(),
                },
                fields,
            });
        }
        None
    }

    /// Select a retained unit variant by its checked canonical declaration identity.
    #[allow(clippy::too_many_arguments)] // Carries the recursive pattern context without reconstructing bindings.
    fn lower_fieldless_enum_pattern(
        &self,
        name: &ast::Spanned<ast::Ident>,
        args: &[ast::PatternArg],
    ) -> Option<bir::Pattern> {
        if args.is_empty()
            && let Some(declaration) = self.local_fieldless_enum_declarations.values().find(|declaration| {
                declaration
                    .variants
                    .iter()
                    .any(|variant| self.type_info.resolved_identity(name.span) == Some(&variant.canonical))
            })
            && let Some(variant) = declaration
                .variants
                .iter()
                .find(|variant| self.type_info.resolved_identity(name.span) == Some(&variant.canonical))
        {
            return Some(bir::Pattern::FieldlessEnumVariant(bir::FieldlessEnumVariantTarget {
                enum_declaration_id: declaration.direct_declaration_id.clone(),
                enum_canonical: declaration.canonical.clone(),
                variant_declaration_id: variant.direct_declaration_id.clone(),
                variant_canonical: variant.canonical.clone(),
                enum_name: declaration.name.clone(),
                variant_name: variant.name.clone(),
            }));
        }
        None
    }

    /// Thread exact checked Result payload types through nested patterns without changing legacy inference.
    #[allow(clippy::too_many_arguments)] // Carries the recursive pattern context without reconstructing bindings.
    fn lower_intrinsic_result_pattern(
        &mut self,
        name: &ast::Spanned<ast::Ident>,
        args: &[ast::PatternArg],
        expected_ty: &IncanType,
        place: &bir::Place,
        arm_scope: bir::ScopeId,
        reads: &PatternReadScope<'_>,
        seen: &mut HashMap<String, bir::LocalId>,
        saved_bindings: &mut Vec<(String, Option<bir::LocalId>)>,
    ) -> Option<bir::Pattern> {
        if let Some(variant) = result_variant_kind(&name.node)
            && let Some((ok_type, error_type)) = result_type_parts(expected_ty)
            && args.len() == 1
            && let [ast::PatternArg::Positional(payload)] = args
        {
            let payload_type = match variant {
                bir::ResultVariantKind::Ok => ok_type,
                bir::ResultVariantKind::Err => error_type,
            };
            // The payload is read from inside the scrutinee, as for every other positional constructor, so its
            // binding's fact follows from that projected place rather than from the whole scrutinee: an
            // untracked temporary scrutinee (a call result) would otherwise yield `Unknown`.
            let mut payload_place = place.clone();
            payload_place
                .projection
                .push(bir::PlaceElem::structural_field("0".to_string()));
            let lowered_payload = self.lower_match_pattern(
                payload,
                payload_type,
                &payload_place,
                arm_scope,
                reads,
                seen,
                saved_bindings,
            );
            return Some(bir::Pattern::Result {
                variant,
                fields: vec![lowered_payload],
            });
        }
        None
    }

    /// Preserve checked enum payload facts and leave unresolved layouts visibly unproven.
    #[allow(clippy::too_many_arguments)] // Carries the recursive pattern context without reconstructing bindings.
    fn lower_generic_constructor_pattern(
        &mut self,
        name: &ast::Spanned<ast::Ident>,
        args: &[ast::PatternArg],
        expected_ty: &IncanType,
        place: &bir::Place,
        arm_scope: bir::ScopeId,
        reads: &PatternReadScope<'_>,
        seen: &mut HashMap<String, bir::LocalId>,
        saved_bindings: &mut Vec<(String, Option<bir::LocalId>)>,
    ) -> bir::Pattern {
        // Mirrors the existing Rust-emission backend's own `lower_pattern` (non-union-aware) mapping
        // exactly: a mix of named and positional arguments (unusual, likely non-representative source)
        // still lowers every sub-pattern's own bindings for side effects, but only the named fields survive
        // into the constructed `Pattern` once `has_named` is known.
        let mut named_fields = Vec::new();
        let mut positional_fields = Vec::new();
        let mut has_named = false;
        let mut positional_index = 0usize;
        let variant_identity = self
            .type_info
            .resolved_identity(name.span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Variant)
            .cloned();
        let payload_types = variant_identity
            .as_ref()
            .and_then(|identity| self.enum_payload_types(identity, expected_ty));
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
                        .push(bir::PlaceElem::structural_field(positional_index.to_string()));
                    if let Some(ty) = payload_types.as_ref().and_then(|fields| fields.get(positional_index)) {
                        self.settled_types.insert(pat.span, ty.clone());
                    }
                    positional_index += 1;
                    let mut lowered = self.lower_match_pattern(
                        pat,
                        &IncanType::Unknown,
                        &field_place,
                        arm_scope,
                        reads,
                        seen,
                        saved_bindings,
                    );
                    if variant_identity.as_ref().is_some_and(|identity| {
                        matches!(&identity.origin,
                        incan_semantics_core::SymbolOrigin::Package { library, module_path }
                        if library == "incan_std_async" && module_path == &["time"])
                    }) && let bir::Pattern::Var(binding) = &mut lowered
                    {
                        self.native_enum_payload_bindings.insert(binding.local);
                        if binding.fact == bir::OwnershipFact::Borrow {
                            binding.fact = bir::OwnershipFact::Move;
                        }
                    }
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
                canonical: variant_identity.clone(),
                name: String::new(),
                variant: variant_identity
                    .as_ref()
                    .filter(|identity| identity.kind == SemanticSourceTargetKind::Variant)
                    .map_or_else(|| name.node.clone(), |identity| identity.declaration_name.clone()),
                fields: positional_fields,
            }
        }
    }

    /// Retain the compiler-owned Some payload from an already-checked Option carrier when legacy left the pattern open.
    /// A conflicting checked constructor or a differently shaped carrier never acquires intrinsic meaning by spelling.
    #[allow(clippy::too_many_arguments)] // Shares the recursive pattern context with lower_match_pattern.
    fn lower_intrinsic_option_pattern(
        &mut self,
        name: &ast::Spanned<ast::Ident>,
        args: &[ast::PatternArg],
        expected_ty: &IncanType,
        place: &bir::Place,
        arm_scope: bir::ScopeId,
        reads: &PatternReadScope<'_>,
        seen: &mut HashMap<String, bir::LocalId>,
        saved_bindings: &mut Vec<(String, Option<bir::LocalId>)>,
    ) -> Option<bir::Pattern> {
        if constructors::from_str(&name.node) != Some(ConstructorId::Some) {
            return None;
        }
        let [payload_type] = collection_member_types(expected_ty, CollectionTypeId::Option)? else {
            return None;
        };
        let [ast::PatternArg::Positional(payload)] = args else {
            return None;
        };
        let canonical = crate::symbols::canonical_builtin_constructor_identity(ConstructorId::Some);
        if self
            .type_info
            .resolved_identity(name.span)
            .is_some_and(|checked| checked != &canonical)
        {
            return None;
        }
        let mut payload_place = place.clone();
        payload_place
            .projection
            .push(bir::PlaceElem::structural_field("0".to_string()));
        let field = self.lower_match_pattern(
            payload,
            payload_type,
            &payload_place,
            arm_scope,
            reads,
            seen,
            saved_bindings,
        );
        Some(bir::Pattern::Enum {
            canonical: Some(canonical),
            name: "Option".to_string(),
            variant: constructors::as_str(ConstructorId::Some).to_string(),
            fields: vec![field],
        })
    }
}

/// Name a model pattern's field by its declared canonical name rather than the spelling the pattern wrote.
///
/// An RFC 021 alias (`Plan(type="premium")` for a field declared `type_`) resolves through the checker to the declared
/// field's identity, so that identity selects the canonical name a consumer finds in the declaration's layout. A field
/// whose identity the checker did not record keeps its written spelling, which a consumer then refuses as unproven.
fn canonical_pattern_field(
    declaration: &bir::NominalDeclaration,
    identity: Option<&CanonicalSymbolId>,
    written: &str,
) -> String {
    identity
        .and_then(|identity| {
            declaration
                .field_identities
                .iter()
                .position(|candidate| candidate == identity)
        })
        .and_then(|slot| declaration.fields.get(slot))
        .map_or_else(|| written.to_string(), Clone::clone)
}
