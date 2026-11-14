//! Pattern lowering for constructor, Rust enum, union, nominal, fieldless enum, result, option and generic patterns.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
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
    pub(in crate::body_ir) fn lower_match_pattern(
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
                        let storage_ty = super::parameter_types::physical_type_argument(expected_ty.clone());
                        let local =
                            self.declare_new_local_with_reads(name.clone(), storage_ty, arm_scope, span, total_reads);
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
    pub(super) fn temporary_native_payload(&self, place: &bir::Place, ty: &IncanType) -> bool {
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
                        && matches!(
                            local.ty.collection(),
                            Some((CollectionTypeId::Option | CollectionTypeId::Result, _))
                        )
                })
    }

    /// Dispatch a checked constructor through its carrier, nominal and enum identity boundaries. References expose
    /// the retained referent layout for constructor selection; payload binding types and ownership remain checked
    /// independently by `lower_match_pattern`.
    #[allow(clippy::too_many_arguments)] // Carries one pattern and its recursive lowering context.
    pub(super) fn lower_constructor_pattern(
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
    pub(super) fn lower_rust_enum_pattern(
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
    pub(super) fn lower_union_pattern(
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
        if (subject_ty.union_members().is_some() || optional_union)
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
    pub(super) fn lower_nominal_pattern(
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
    pub(super) fn lower_fieldless_enum_pattern(
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
    pub(super) fn lower_intrinsic_result_pattern(
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
    pub(super) fn lower_generic_constructor_pattern(
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

    /// Retain the compiler-owned Option constructors of an already-checked Option carrier when legacy left the pattern
    /// open: `Some` with its payload, and the bare `None` an `assert value is None` pattern spells. A conflicting
    /// checked constructor or a differently shaped carrier never acquires intrinsic meaning by spelling.
    #[allow(clippy::too_many_arguments)] // Shares the recursive pattern context with lower_match_pattern.
    pub(super) fn lower_intrinsic_option_pattern(
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
        let constructor = constructors::from_str(&name.node)
            .filter(|constructor| matches!(constructor, ConstructorId::Some | ConstructorId::None))?;
        let [payload_type] = expected_ty.collection_arguments(CollectionTypeId::Option)? else {
            return None;
        };
        let canonical = crate::symbols::canonical_builtin_constructor_identity(constructor);
        if self
            .type_info
            .resolved_identity(name.span)
            .is_some_and(|checked| checked != &canonical)
        {
            return None;
        }
        let fields = match (constructor, args) {
            (ConstructorId::None, []) => Vec::new(),
            (ConstructorId::Some, [ast::PatternArg::Positional(payload)]) => {
                let mut payload_place = place.clone();
                payload_place
                    .projection
                    .push(bir::PlaceElem::structural_field("0".to_string()));
                vec![self.lower_match_pattern(
                    payload,
                    payload_type,
                    &payload_place,
                    arm_scope,
                    reads,
                    seen,
                    saved_bindings,
                )]
            }
            _ => return None,
        };
        Some(bir::Pattern::Enum {
            canonical: Some(canonical),
            name: "Option".to_string(),
            variant: constructors::as_str(constructor).to_string(),
            fields,
        })
    }
}
