//! RFC 120 member-name collisions and source-ordered member bindings.

use super::*;

impl TypeChecker {
    /// Register every authored member spelling through one source-ordered first-wins collision mechanism.
    ///
    /// Model/class fields and properties occupy the instance surface. Receiverless methods occupy the type surface,
    /// so their spelling may match an instance member without becoming ambiguous at either use site. Method aliases
    /// and partials inherit the surface of the method they project.
    pub(super) fn validate_member_name_collisions(
        &mut self,
        owner: &str,
        fields: &[Spanned<FieldDecl>],
        aliases: &[Spanned<MethodAliasDecl>],
        partials: &[Spanned<MethodPartialDecl>],
        properties: &[Spanned<PropertyDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let mut method_surfaces: HashMap<String, HashSet<MemberBindingSurface>> = HashMap::new();
        for method in methods {
            method_surfaces
                .entry(method.node.name.clone())
                .or_default()
                .insert(MemberBindingSurface::for_method(&method.node));
        }
        let mut alias_surfaces: HashMap<String, HashSet<MemberBindingSurface>> = HashMap::new();
        for alias in aliases {
            if let Some(surfaces) = method_surfaces.get(&alias.node.target) {
                alias_surfaces.insert(alias.node.name.clone(), surfaces.clone());
            }
        }

        let mut declarations =
            Vec::with_capacity(fields.len() + aliases.len() + partials.len() + properties.len() + methods.len());
        declarations.extend(fields.iter().map(|field| {
            (
                field.node.name.clone(),
                MemberBindingKind::Field,
                MemberBindingSurface::Instance,
                field.span,
            )
        }));
        declarations.extend(methods.iter().map(|method| {
            (
                method.node.name.clone(),
                MemberBindingKind::Method,
                MemberBindingSurface::for_method(&method.node),
                method.span,
            )
        }));
        declarations.extend(properties.iter().map(|property| {
            (
                property.node.name.clone(),
                MemberBindingKind::Property,
                MemberBindingSurface::Instance,
                property.span,
            )
        }));
        for alias in aliases {
            let surfaces = alias_surfaces
                .get(&alias.node.name)
                .cloned()
                .unwrap_or_else(|| HashSet::from([MemberBindingSurface::Instance]));
            declarations.extend(surfaces.into_iter().map(|surface| {
                (
                    alias.node.name.clone(),
                    MemberBindingKind::MethodAlias,
                    surface,
                    alias.span,
                )
            }));
        }
        for partial in partials {
            let surfaces = method_surfaces
                .get(&partial.node.target)
                .or_else(|| alias_surfaces.get(&partial.node.target))
                .cloned()
                .unwrap_or_else(|| HashSet::from([MemberBindingSurface::Instance]));
            declarations.extend(surfaces.into_iter().map(|surface| {
                (
                    partial.node.name.clone(),
                    MemberBindingKind::MethodPartial,
                    surface,
                    partial.span,
                )
            }));
        }
        self.register_source_ordered_member_bindings(owner, declarations);
    }

    /// Register enum variants, their aliases, and enum methods through the shared member-namespace mechanism.
    pub(super) fn validate_enum_member_name_collisions(
        &mut self,
        owner: &str,
        variants: &[Spanned<VariantDecl>],
        aliases: &[Spanned<EnumVariantAliasDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let mut declarations = Vec::with_capacity(variants.len() + aliases.len() + methods.len());
        declarations.extend(variants.iter().map(|variant| {
            (
                variant.node.name.clone(),
                MemberBindingKind::Variant,
                MemberBindingSurface::Type,
                variant.span,
            )
        }));
        declarations.extend(aliases.iter().map(|alias| {
            (
                alias.node.name.clone(),
                MemberBindingKind::VariantAlias,
                MemberBindingSurface::Type,
                alias.span,
            )
        }));
        declarations.extend(methods.iter().map(|method| {
            (
                method.node.name.clone(),
                MemberBindingKind::Method,
                MemberBindingSurface::for_method(&method.node),
                method.span,
            )
        }));
        self.register_source_ordered_member_bindings(owner, declarations);
    }

    /// Apply the one source-ordered, first-wins collision decision shared by every member-bearing declaration.
    pub(super) fn register_source_ordered_member_bindings(
        &mut self,
        owner: &str,
        mut declarations: Vec<(String, MemberBindingKind, MemberBindingSurface, Span)>,
    ) {
        declarations.sort_by_key(|(_, _, _, span)| *span);

        let mut seen = HashMap::new();
        for (name, kind, surface, span) in declarations {
            match register_binding(&mut seen, (name.clone(), surface), (kind, span)) {
                BindingRegistration::Registered => {
                    self.record_member_declaration_identity(&name, kind, span);
                }
                BindingRegistration::Collision {
                    existing: (MemberBindingKind::Method, _),
                } if kind == MemberBindingKind::Method => {
                    // Same-name methods form one overload binding.
                    self.record_member_declaration_identity(&name, kind, span);
                }
                BindingRegistration::Collision {
                    existing: (previous_kind, _),
                } if matches!(
                    (previous_kind, kind),
                    (MemberBindingKind::Field, MemberBindingKind::Method)
                        | (MemberBindingKind::Method, MemberBindingKind::Field)
                ) =>
                {
                    // A stored selection and method invocation are distinct source forms: `value.name` versus
                    // `value.name()`. Both declarations therefore remain active even when their spellings match. If
                    // the field appears second, retain it as the registry sentinel so a later duplicate field or
                    // field-like declaration still collides with storage rather than being mistaken for another
                    // callable coexistence case.
                    self.record_member_declaration_identity(&name, kind, span);
                    if kind == MemberBindingKind::Field {
                        seen.insert((name, surface), (kind, span));
                    }
                }
                BindingRegistration::Collision {
                    existing: (previous_kind, previous_span),
                } => {
                    self.rejected_member_bindings.insert(span);
                    self.record_member_name_collision(owner, &name, previous_kind, previous_span, kind, span);
                }
            }
        }
    }

    /// Return whether one member declaration survived the shared first-wins registry.
    pub(crate) fn member_binding_is_active(&self, span: Span) -> bool {
        !self.rejected_member_bindings.contains(&span)
    }

    /// Export one accepted source-owned member declaration from the shared member registry.
    pub(super) fn record_member_declaration_identity(&mut self, name: &str, kind: MemberBindingKind, span: Span) {
        let Some(kind) = kind.declaration_kind() else {
            // Aliases and partials introduce bindings to an existing method declaration; they do not mint one.
            return;
        };
        let identity = self.symbols.member_declaration_identity(name, kind, span);
        self.type_info
            .declarations
            .member_declaration_identities
            .insert(span, identity);
    }

    /// Render one collision already decided by the shared member registration.
    pub(super) fn record_member_name_collision(
        &mut self,
        owner: &str,
        name: &str,
        previous_kind: MemberBindingKind,
        previous_span: Span,
        kind: MemberBindingKind,
        span: Span,
    ) {
        if matches!(
            previous_kind,
            MemberBindingKind::Variant | MemberBindingKind::VariantAlias
        ) && matches!(kind, MemberBindingKind::Variant | MemberBindingKind::VariantAlias)
        {
            self.errors.push(errors::duplicate_binding(name, previous_span, span));
            return;
        }

        let message = match kind {
            MemberBindingKind::MethodAlias => {
                format!("Duplicate method alias '{}' on '{}'", name, owner)
            }
            MemberBindingKind::MethodPartial => {
                format!("Duplicate method partial '{}.{}'", owner, name)
            }
            _ => format!(
                "Duplicate member '{}.{}' declared as both {} and {}",
                owner,
                name,
                previous_kind.label(),
                kind.label()
            ),
        };
        self.errors
            .push(CompileError::type_error(message, span).with_note(format!(
                "First declaration span: {}..{}",
                previous_span.start, previous_span.end
            )));
    }
}
