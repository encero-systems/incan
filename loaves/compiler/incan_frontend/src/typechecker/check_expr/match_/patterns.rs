//! Pattern checking: lexical identities and constructor, nominal, enum, union, builtin, tuple and or patterns.

use super::*;
use crate::typechecker::ResolvedNamespaceMemberKind;

impl TypeChecker {
    /// Record a constructor pattern that resolved through the active lexical binding.
    pub(in crate::typechecker) fn record_pattern_lexical_identity(&mut self, name: &str, span: Span) {
        let identity = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.identity_of(symbol_id))
            .cloned();
        if let Some(identity) = identity {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Record the source enum variant selected by a checked constructor pattern.
    pub(super) fn record_incan_enum_pattern_identity(
        &mut self,
        expected_ty: &ResolvedType,
        qualifier: Option<&str>,
        variant: &str,
        span: Span,
    ) {
        if let Some(qualifier) = qualifier {
            let segments = qualifier.split("::").map(str::to_owned).collect::<Vec<_>>();
            let identity = self
                .resolve_qualified_name(&segments)
                .ok()
                .and_then(|resolved| match resolved.kind {
                    ResolvedNamespaceMemberKind::Symbol(SymbolKind::Type(TypeInfo::Enum(info))) => {
                        info.variant_identities.get(variant).cloned()
                    }
                    _ => None,
                });
            if let Some(identity) = identity {
                self.type_info.record_resolved_identity(span, identity);
            }
            return;
        }
        let enum_name = match expected_ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return,
        };
        let identity = match self.lookup_semantic_type_info(enum_name) {
            Some(TypeInfo::Enum(info)) => info.variant_identities.get(variant).cloned(),
            _ => None,
        };
        if let Some(identity) = identity {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Type-check a pattern against an expected type, defining bindings in the current scope.
    ///
    /// Every pattern node's checked type is recorded at its own span before it is dispatched on, the way a `for`
    /// pattern's item type is (#1125). Lowering has no way to rebuild a payload type on its own -- a user enum's
    /// variant payloads, an imported or Rust-backed enum's, a model field's declared type, or the borrow wrapper
    /// [`borrowed_pattern_payload`] applies -- so this recorded fact is what lets a destructured binding carry its
    /// declared type rather than an unresolved one (#1245). The record is unconditional: a node checked against
    /// [`ResolvedType::Unknown`] records that honestly, and a consumer treats it as "no fact" rather than as a type.
    /// Union constructors also record their selected target at the constructor-name span so Body IR can retain member
    /// selection independently of spelling.
    pub(in crate::typechecker) fn check_pattern(&mut self, pattern: &Spanned<Pattern>, expected_ty: &ResolvedType) {
        self.record_expr_type(pattern.span, expected_ty.clone());
        match &pattern.node {
            Pattern::Wildcard => {}
            Pattern::Binding(name) => {
                self.validate_protected_builtin_binding(name, pattern.span);
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
                self.record_write_target_identity(pattern.span, name);
            }
            Pattern::Group(inner) => {
                self.check_pattern(inner, expected_ty);
            }
            Pattern::Or(alternatives) => {
                self.check_or_pattern(alternatives, expected_ty);
            }
            Pattern::Literal(literal) => self.check_literal_pattern(literal, expected_ty, pattern.span),
            Pattern::Constructor(name, sub_patterns) => {
                self.check_constructor_pattern(name, sub_patterns, expected_ty, pattern.span);
            }
            Pattern::Tuple(sub_patterns) => self.check_tuple_pattern(sub_patterns, expected_ty, pattern.span),
        }
    }

    /// Resolve constructor patterns in union, builtin, nominal and enum order without changing payload borrowing.
    pub(super) fn check_constructor_pattern(
        &mut self,
        name: &Spanned<String>,
        sub_patterns: &[PatternArg],
        expected_ty: &ResolvedType,
        pattern_span: Span,
    ) {
        self.type_info
            .expressions
            .body_ir_nominal_pattern_owners
            .remove(&name.span);
        let (subject_ty, borrow) = borrowed_pattern_subject(expected_ty);
        let (enum_qualifier_opt, ctor_name) = Self::split_pattern_constructor_name(name.node.as_str());
        if enum_qualifier_opt.is_none()
            && self.check_union_constructor_pattern(name, ctor_name, sub_patterns, expected_ty)
        {
            return;
        }

        if self.check_builtin_constructor_pattern(name, ctor_name, sub_patterns, subject_ty, borrow, enum_qualifier_opt)
        {
            return;
        }

        let ctor_name = if name.node.contains("::") {
            name.node.split("::").last().unwrap_or(&name.node)
        } else {
            name.node.as_str()
        };

        if self.check_nominal_constructor_pattern(name, ctor_name, sub_patterns, subject_ty, borrow) {
            return;
        }
        self.check_enum_constructor_pattern(
            name,
            ctor_name,
            enum_qualifier_opt,
            sub_patterns,
            expected_ty,
            pattern_span,
        );
    }

    /// Check a subject-selected model or class record pattern, retaining an owner only after successful checking.
    pub(super) fn check_nominal_constructor_pattern(
        &mut self,
        name: &Spanned<String>,
        ctor_name: &str,
        sub_patterns: &[PatternArg],
        subject_ty: &ResolvedType,
        borrow: Option<PatternBorrow>,
    ) -> bool {
        // A record pattern names the subject's model or class, generic ones included: each field it names
        // is checked against the field's type under the subject's type arguments.
        let model_or_class_fields = match subject_ty {
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _)
                if self.pattern_type_spelling_names_subject(ctor_name, type_name) =>
            {
                let type_args = match subject_ty {
                    ResolvedType::Generic(_, type_args) => type_args.as_slice(),
                    _ => &[],
                };
                // A dependency's type the module reaches only through a signature has no lexical symbol.
                self.lookup_semantic_type_info(type_name)
                    .and_then(|type_info| match type_info {
                        TypeInfo::Model(model_info) => Some((
                            model_info.fields.clone(),
                            type_param_subst_map(&model_info.type_params, type_args),
                        )),
                        TypeInfo::Class(class_info) => Some((
                            class_info.fields.clone(),
                            type_param_subst_map(&class_info.type_params, type_args),
                        )),
                        _ => None,
                    })
                    .map(|(fields, substitutions)| (type_name, fields, substitutions))
            }
            _ => None,
        };

        let Some((type_name, fields, substitutions)) = model_or_class_fields else {
            return false;
        };
        let errors_before_pattern = self.errors.len();
        self.record_pattern_lexical_identity(type_name, name.span);
        self.check_nominal_pattern_fields(type_name, &fields, &substitutions, sub_patterns, borrow);
        if self.errors.len() == errors_before_pattern
            && self.type_info.resolved_identity(name.span).is_none()
            && let Some(identity) = self
                .symbols
                .lookup(ctor_name)
                .and_then(|symbol| self.symbols.identity_of(symbol))
                .cloned()
            && matches!(
                identity.kind,
                incan_semantics_core::SemanticSourceTargetKind::Model
                    | incan_semantics_core::SemanticSourceTargetKind::Class
            )
        {
            self.type_info
                .expressions
                .body_ir_nominal_pattern_owners
                .insert(name.span, identity);
        }
        true
    }

    /// Check named record payloads in source order, recording field identity before visibility and duplicate checks.
    pub(super) fn check_nominal_pattern_fields(
        &mut self,
        type_name: &str,
        fields: &HashMap<String, FieldInfo>,
        substitutions: &HashMap<String, ResolvedType>,
        patterns: &[PatternArg],
        borrow: Option<PatternBorrow>,
    ) {
        let mut provided = HashSet::new();
        for arg in patterns {
            match arg {
                PatternArg::Positional(pattern) => {
                    self.errors
                        .push(errors::positional_pattern_not_supported(type_name, pattern.span));
                }
                PatternArg::Named(field_name, pattern) => {
                    let Some((canonical_name, info)) = self.resolve_field_info(fields, &field_name.node, true, true)
                    else {
                        self.errors
                            .push(errors::missing_field(type_name, &field_name.node, field_name.span));
                        continue;
                    };
                    if let Some(identity) = info.identity.clone() {
                        self.type_info.record_resolved_identity(field_name.span, identity);
                    }
                    if self.private_field_is_inaccessible(type_name, info) {
                        self.errors
                            .push(self.private_field_error(type_name, info, &field_name.node, field_name.span));
                        continue;
                    }
                    if !provided.insert(canonical_name.clone()) {
                        self.errors.push(errors::duplicate_pattern_field(
                            type_name,
                            canonical_name.as_str(),
                            pattern.span,
                        ));
                        continue;
                    }
                    let field_ty = substitute_resolved_type(&info.ty, substitutions);
                    self.check_pattern(pattern, &borrowed_pattern_payload(field_ty, borrow));
                }
            }
        }
    }

    /// Resolve enum payloads and retain the existing arity, qualification and unknown-native-payload diagnostics.
    pub(super) fn check_enum_constructor_pattern(
        &mut self,
        name: &Spanned<String>,
        ctor_name: &str,
        enum_qualifier_opt: Option<&str>,
        sub_patterns: &[PatternArg],
        expected_ty: &ResolvedType,
        pattern_span: Span,
    ) {
        let variant_name = ctor_name;

        let positional_count = sub_patterns
            .iter()
            .filter(|a| matches!(a, PatternArg::Positional(_)))
            .count();

        let incan_resolution = self.incan_enum_constructor_payload_types(expected_ty, variant_name, enum_qualifier_opt);
        let rust_resolution = self.rust_enum_constructor_payload_types(expected_ty, name.node.as_str(), name.span);
        let field_types: Option<Vec<ResolvedType>> =
            incan_resolution.clone().or_else(|| match rust_resolution.as_ref() {
                Some(RustEnumPatternResolution::PayloadTypes { fields, .. }) => Some(fields.clone()),
                Some(RustEnumPatternResolution::QualifierMismatch) | None => None,
            });

        match field_types {
            Some(fields) => {
                if incan_resolution.is_some() {
                    self.record_incan_enum_pattern_identity(expected_ty, enum_qualifier_opt, variant_name, name.span);
                }
                if incan_resolution.is_some() || rust_resolution.is_some() {
                    // One sub-pattern per payload value (#1561); a named sub-pattern is refused on its own.
                    let all_positional = sub_patterns.iter().all(|arg| matches!(arg, PatternArg::Positional(_)));
                    if all_positional && positional_count != fields.len() {
                        self.errors.push(errors::pattern_arity_mismatch(
                            &format!("The pattern '{}'", name.node),
                            "payload value",
                            fields.len(),
                            positional_count,
                            pattern_span,
                        ));
                    }
                }
                self.check_constructor_subpatterns_enum_like(
                    name.node.as_str(),
                    sub_patterns,
                    Some(fields.as_slice()),
                    self.pattern_subject_is_rust_backed(expected_ty),
                    rust_resolution.as_ref(),
                );
            }
            None => {
                let permissive = self.match_subject_allows_unknown_rust_enum_payloads(
                    expected_ty,
                    name.node.as_str(),
                    rust_resolution.as_ref(),
                );
                if !permissive && !matches!(expected_ty, ResolvedType::Unknown) {
                    self.errors.push(errors::unknown_match_constructor_pattern(
                        name.node.as_str(),
                        &expected_ty.to_string(),
                        pattern_span,
                    ));
                }
                self.check_constructor_subpatterns_enum_like(
                    name.node.as_str(),
                    sub_patterns,
                    None,
                    self.pattern_subject_is_rust_backed(expected_ty),
                    rust_resolution.as_ref(),
                );
            }
        }
    }

    /// Narrow a union member through its first positional payload, preserving preceding named-argument diagnostics.
    pub(super) fn check_union_constructor_pattern(
        &mut self,
        name: &Spanned<String>,
        constructor: &str,
        args: &[PatternArg],
        expected: &ResolvedType,
    ) -> bool {
        let Some(member_ty) = self.union_pattern_target_type(expected, constructor) else {
            return false;
        };
        self.record_pattern_lexical_identity(constructor, name.span);
        self.record_expr_type(name.span, member_ty.clone());
        let mut positional = None;
        for arg in args {
            match arg {
                PatternArg::Positional(pattern) => {
                    positional = Some(pattern);
                    break;
                }
                PatternArg::Named(_, pattern) => {
                    self.errors
                        .push(errors::named_pattern_not_supported(&name.node, pattern.span));
                }
            }
        }
        if let Some(pattern) = positional {
            let written = resolve_type(&Type::Simple(constructor.to_string()), &self.symbols);
            self.check_pattern(pattern, &Self::narrowed_union_member_type(&member_ty, &written));
        }
        true
    }

    /// Check Result and Option payloads only when the written qualifier agrees with the checked subject.
    ///
    /// An unqualified None retains the existing builtin treatment. A qualified None over another enum is declined
    /// so that enum's own variant can resolve through the ordinary enum path.
    pub(super) fn check_builtin_constructor_pattern(
        &mut self,
        name: &Spanned<String>,
        constructor: &str,
        patterns: &[PatternArg],
        subject: &ResolvedType,
        borrow: Option<PatternBorrow>,
        qualifier: Option<&str>,
    ) -> bool {
        if !qualifier.is_none_or(|qualifier| Self::pattern_qualifier_matches_expected_type(subject, qualifier)) {
            return false;
        }
        let Some(id) = constructors::from_str(constructor) else {
            return false;
        };
        match id {
            ConstructorId::Ok => {
                if let ResolvedType::Generic(type_name, args) = subject
                    && type_name == collections::as_str(CollectionTypeId::Result)
                    && !args.is_empty()
                {
                    self.record_pattern_lexical_identity(constructor, name.span);
                    let payload = borrowed_pattern_payload(args[0].clone(), borrow);
                    self.check_single_payload_constructor_pattern(name, patterns, &payload);
                    return true;
                }
            }
            ConstructorId::Err => {
                if let ResolvedType::Generic(type_name, args) = subject
                    && type_name == collections::as_str(CollectionTypeId::Result)
                    && args.len() >= 2
                {
                    self.record_pattern_lexical_identity(constructor, name.span);
                    let payload = borrowed_pattern_payload(args[1].clone(), borrow);
                    self.check_single_payload_constructor_pattern(name, patterns, &payload);
                    return true;
                }
            }
            ConstructorId::Some => {
                if let ResolvedType::Generic(type_name, args) = subject
                    && type_name == collections::as_str(CollectionTypeId::Option)
                    && !args.is_empty()
                {
                    self.record_pattern_lexical_identity(constructor, name.span);
                    let payload = borrowed_pattern_payload(args[0].clone(), borrow);
                    self.check_single_payload_constructor_pattern(name, patterns, &payload);
                    return true;
                }
            }
            ConstructorId::None => {
                let option_subject = matches!(
                    subject,
                    ResolvedType::Generic(type_name, _)
                        if type_name == collections::as_str(CollectionTypeId::Option)
                );
                if qualifier.is_none() || option_subject {
                    self.record_pattern_lexical_identity(constructor, name.span);
                    return true;
                }
            }
        }
        false
    }

    /// Destructure either checked tuple spelling and retain the subject's borrow mode for every element.
    pub(super) fn check_tuple_pattern(
        &mut self,
        sub_patterns: &[Spanned<Pattern>],
        expected_ty: &ResolvedType,
        span: Span,
    ) {
        // Tuple literals and `(A, B)` annotations infer ResolvedType::Tuple; `tuple[A, B]` uses the collection
        // registry. Both use the same shape classification as iteration and unpacking, so every sub-pattern is
        // visited (#1714).
        let (subject_ty, borrow) = borrowed_pattern_subject(expected_ty);
        if let TupleShape::Tuple(elem_types) = classify_tuple_shape(subject_ty) {
            // One sub-pattern per element (#1561).
            if sub_patterns.len() != elem_types.len() {
                self.errors.push(errors::pattern_arity_mismatch(
                    "A tuple pattern",
                    "element",
                    elem_types.len(),
                    sub_patterns.len(),
                    span,
                ));
            }
            for (pat, elem_ty) in sub_patterns.iter().zip(elem_types.iter()) {
                self.check_pattern(pat, &borrowed_pattern_payload(elem_ty.clone(), borrow));
            }
        }
    }

    /// Type-check alternatives in isolated scopes, then define only the agreed binding set in the surrounding arm
    /// scope.
    ///
    /// Without the isolation step, `A(x) | B(y)` would accidentally leak both `x` and `y` into the branch body even
    /// though no single successful match can provide both names. RFC 071 requires every alternative to bind the same
    /// names with the same types before any branch-local binding is made visible.
    pub(super) fn check_or_pattern(&mut self, alternatives: &[Spanned<Pattern>], expected_ty: &ResolvedType) {
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
                name: name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty: binding.ty.clone(),
                    is_mutable: false,
                    is_used: false,
                }),
                span: binding.span,
                scope: 0,
            });
            for (_, bindings) in &binding_sets {
                if let Some(alternative_binding) = bindings.get(&name) {
                    // Every syntactic write in a valid OR-pattern introduces the one binding visible to the arm
                    // body. Overwrite the provisional isolated-scope facts with that final binding identity.
                    self.record_write_target_identity(alternative_binding.span, &name);
                }
            }
        }
    }
}
