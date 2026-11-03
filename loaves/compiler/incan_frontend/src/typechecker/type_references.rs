//! Standard-library type usage, type reference identities, and qualified type annotations.

use super::*;

impl TypeChecker {
    pub(super) fn validate_stdlib_type_usage(&mut self, ty: &Spanned<Type>) {
        self.validate_stdlib_type_usage_inner(&ty.node, ty.span);
    }

    /// Validate stdlib-owned type names recursively inside a type annotation.
    pub(super) fn validate_stdlib_type_usage_inner(&mut self, ty: &Type, span: Span) {
        match ty {
            Type::Simple(name) => self.validate_stdlib_type_name(name, span),
            Type::Qualified(segments) => {
                if let Some(first) = segments.first() {
                    self.validate_stdlib_type_name(first, span);
                }
            }
            Type::Dotted(_) => {}
            Type::Generic(name, args) => {
                self.validate_stdlib_type_name(name, span);
                for arg in args {
                    self.validate_stdlib_type_usage_inner(&arg.node, arg.span);
                }
            }
            Type::DottedGeneric(_, args) => {
                for arg in args {
                    self.validate_stdlib_type_usage_inner(&arg.node, arg.span);
                }
            }
            Type::ConstrainedPrimitive(name, _) => self.validate_stdlib_type_name(name, span),
            Type::Function(params, ret) => {
                for param in params {
                    self.validate_stdlib_type_usage_inner(&param.node, param.span);
                }
                self.validate_stdlib_type_usage_inner(&ret.node, ret.span);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.validate_stdlib_type_usage_inner(&inner.node, inner.span);
            }
            Type::Tuple(elems) => {
                for elem in elems {
                    self.validate_stdlib_type_usage_inner(&elem.node, elem.span);
                }
            }
            Type::IntLiteral(_) | Type::Unit | Type::SelfType | Type::Infer => {}
        }
    }

    /// Report a stdlib surface type name (`Response`, `FieldInfo`, ...) that no binding in scope provides.
    ///
    /// A name the program declares at module scope is its own type and never the stdlib's, whether or not the
    /// declaration has been collected yet when this annotation is resolved: a model's own method signatures are
    /// collected before the model itself (#1795).
    pub(super) fn validate_stdlib_type_name(&mut self, name: &str, span: Span) {
        let Some(id) = surface_types::from_str(name) else {
            return;
        };
        if surface_types::stdlib_module_path(id).is_some()
            && !self.module_declared_type_names.contains(name)
            && self.symbols.lookup(name).is_none()
        {
            self.errors.push(errors::unknown_symbol(name, span));
        }
    }

    /// Collect the type names one program declares at module scope: models, classes, enums, newtypes, traits and
    /// type aliases.
    pub(super) fn collect_module_declared_type_names(program: &Program) -> HashSet<String> {
        program
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Model(model) => Some(model.name.clone()),
                Declaration::Class(class) => Some(class.name.clone()),
                Declaration::Enum(enum_decl) => Some(enum_decl.name.clone()),
                Declaration::Newtype(newtype) => Some(newtype.name.clone()),
                Declaration::Trait(trait_decl) => Some(trait_decl.name.clone()),
                Declaration::TypeAlias(alias) => Some(alias.name.clone()),
                _ => None,
            })
            .collect()
    }

    /// Record every compiler-proven declaration identity referenced by one source type annotation.
    ///
    /// Generic constructors and their arguments are independent reference sites, so the walk retains each AST
    /// node's own span. Qualified paths resolve through the checked import metadata rather than being reconstructed
    /// from their written spelling. `Self` records the enclosing concrete nominal declaration in methods and the
    /// enclosing trait declaration in an abstract trait signature.
    pub(super) fn record_type_reference_identities(&mut self, ty: &Spanned<Type>) {
        match &ty.node {
            Type::Simple(name) | Type::ConstrainedPrimitive(name, _) => {
                self.record_named_type_reference_identity(name, ty.span);
            }
            Type::Generic(name, args) => {
                self.record_named_type_reference_identity(name, ty.span);
                for arg in args {
                    self.record_type_reference_identities(arg);
                }
            }
            Type::DottedGeneric(segments, args) => {
                self.record_dotted_type_reference_identity(segments, ty.span);
                for arg in args {
                    self.record_type_reference_identities(arg);
                }
            }
            Type::Function(params, ret) => {
                for param in params {
                    self.record_type_reference_identities(param);
                }
                self.record_type_reference_identities(ret);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.record_type_reference_identities(inner)
            }
            Type::Tuple(items) => {
                for item in items {
                    self.record_type_reference_identities(item);
                }
            }
            Type::SelfType => {
                if let Some(owner) = self
                    .current_method_owner
                    .clone()
                    .or_else(|| self.current_trait_name.clone())
                {
                    self.record_named_type_reference_identity(&owner, ty.span);
                }
            }
            Type::Qualified(segments) => self.record_qualified_rust_type_reference_identity(segments, ty.span),
            Type::Dotted(segments) => self.record_dotted_type_reference_identity(segments, ty.span),
            Type::IntLiteral(_) | Type::Unit | Type::Infer => {}
        }
    }

    /// Record one Rust item reached through an already-resolved `rust::` module binding.
    ///
    /// The import collector owns the root's canonical crate path. Appending the syntactically checked suffix yields
    /// the same target identity as a direct `from rust::... import Item` binding; no generated Rust spelling or local
    /// alias is consulted.
    pub(super) fn record_qualified_rust_type_reference_identity(&mut self, segments: &[String], span: Span) {
        let Some((root, suffix)) = segments.split_first() else {
            return;
        };
        let Some(info) = self.lookup_symbol(root).and_then(|symbol| match &symbol.kind {
            SymbolKind::RustItem(info) if info.binding != RustImportBindingKind::FromImport => Some(info),
            _ => None,
        }) else {
            return;
        };
        let mut canonical_path = info.path.split("::").map(str::to_string).collect::<Vec<_>>();
        canonical_path.extend(suffix.iter().cloned());
        let Some(declaration_name) = canonical_path.last().cloned() else {
            return;
        };
        self.type_info.record_resolved_identity(
            span,
            CanonicalSymbolId {
                namespace: SymbolNamespace::OrdinaryLexical,
                origin: SymbolOrigin::RustCrate(canonical_path),
                declaration_name,
                kind: SemanticSourceTargetKind::RustItem,
                scope_discriminant: None,
                declaration_span: HirSourceSpan::new(0, 0),
            },
        );
    }

    /// Record one type reached through an already-resolved Incan module binding.
    ///
    /// The spelling was resolved by [`Self::resolve_qualified_type_annotations`] before this walk; this only attaches
    /// that proven identity to the reference site's own span.
    pub(super) fn record_dotted_type_reference_identity(&mut self, segments: &[String], span: Span) {
        let spelling = segments.join(".");
        if let Some(identity) = self
            .type_info
            .qualified_type_reference(&spelling)
            .map(|reference| reference.identity.clone())
        {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Resolve every module-qualified spelling (`mod.Type`, `mod.Box[T]`) inside one annotation before the
    /// annotation itself is resolved.
    ///
    /// The shared type resolver only sees the symbol table, while a qualified spelling resolves through the module
    /// member registry, the stdlib cache, or a public library's checked manifest. So the checker proves each spelling
    /// here first and records the result keyed by the spelling; the resolver then reads that fact, and so does
    /// lowering when it places the nominal type. Nothing is reported here: the validation walk owns the diagnostic
    /// for a spelling that did not resolve, so it fires once in the checking pass like the other annotation
    /// diagnostics, while the facts are needed during declaration collection as well.
    pub(super) fn resolve_qualified_type_annotations(&mut self, ty: &Spanned<Type>) {
        match &ty.node {
            Type::Dotted(segments) => self.resolve_qualified_type_annotation(segments),
            Type::DottedGeneric(segments, args) => {
                self.resolve_qualified_type_annotation(segments);
                for arg in args {
                    self.resolve_qualified_type_annotations(arg);
                }
            }
            Type::Generic(_, args) | Type::Tuple(args) => {
                for arg in args {
                    self.resolve_qualified_type_annotations(arg);
                }
            }
            Type::Function(params, ret) => {
                for param in params {
                    self.resolve_qualified_type_annotations(param);
                }
                self.resolve_qualified_type_annotations(ret);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.resolve_qualified_type_annotations(inner)
            }
            Type::Simple(_)
            | Type::Qualified(_)
            | Type::ConstrainedPrimitive(..)
            | Type::IntLiteral(_)
            | Type::Unit
            | Type::SelfType
            | Type::Infer => {}
        }
    }

    /// Resolve one module-qualified type spelling and retain the proof under that spelling.
    ///
    /// A spelling is a module-scope fact -- its root is a module binding and its tail names a member of that module --
    /// so every occurrence in one module resolves the same way. It is still resolved afresh each time rather than
    /// memoized: dependency interfaces are collected through this same path before the consumer's facts are reset,
    /// and two dependencies can spell `errors.T` against different `errors` bindings, so a remembered answer could
    /// outlive the binding it was proven against. A failed resolution removes any earlier entry for the same reason.
    pub(super) fn resolve_qualified_type_annotation(&mut self, segments: &[String]) {
        let spelling = segments.join(".");
        match self.qualified_type_declaration(segments) {
            Some(reference) => {
                self.type_info
                    .declarations
                    .qualified_type_references
                    .insert(spelling, reference);
            }
            None => {
                self.type_info.declarations.qualified_type_references.remove(&spelling);
            }
        }
    }

    /// Resolve the declaration a module-qualified type spelling names, if the checker can prove one.
    ///
    /// The root must be an Incan module binding; the remaining segments walk nested modules and end at a member that
    /// declares a type or trait. A Loaf module answers through its checked manifest and a source module through the
    /// module member registry, each the way the matching direct import would. The resolved type is the one that direct
    /// import would have bound -- the declaration's own name, the target of a non-generic source type alias, or the
    /// Loaf-qualified spelling -- so values checked against either spelling of the same declaration are compatible.
    /// `None` means no type was proven; it never means "assume the written path".
    pub(super) fn qualified_type_declaration(&mut self, segments: &[String]) -> Option<QualifiedTypeReferenceInfo> {
        let (root, remainder) = segments.split_first()?;
        let (name, nested_module) = remainder.split_last()?;
        let mut module_path = self.lookup_symbol(root).and_then(|symbol| match &symbol.kind {
            SymbolKind::Module(info) if !info.is_python => Some(info.path.clone()),
            _ => None,
        })?;
        module_path.extend(nested_module.iter().cloned());

        // A Rust item a Loaf module re-exports is that Rust item, spelled qualified or not.
        if let Some((library, rest)) = self.loaf_module(&module_path)
            && let Ok(Some(member)) = self.resolve_pub_library_module_symbol_member(&library, &rest, name)
            && let SymbolKind::RustItem(info) = &member.kind
            && let Some(identity) = member.canonical.clone()
        {
            return Some(QualifiedTypeReferenceInfo {
                identity,
                module_path,
                resolved: ResolvedType::RustPath(info.path.clone()),
            });
        }

        // Compiler-known surface types have no source declaration for the stdlib cache to return. Their registry
        // ownership is nevertheless the same proof a direct `from std.web import Html` uses, so retain that stable
        // builtin identity and canonical resolved spelling for a qualified annotation as well (#1824).
        if let Some(surface_type) = surface_types::from_str(name)
            && surface_types::stdlib_module_path(surface_type).is_some_and(|owner| owner == module_path.join("."))
        {
            let canonical = surface_types::as_str(surface_type).to_string();
            return Some(QualifiedTypeReferenceInfo {
                identity: CanonicalSymbolId {
                    namespace: SymbolNamespace::OrdinaryLexical,
                    origin: SymbolOrigin::Builtin,
                    declaration_name: canonical.clone(),
                    kind: SemanticSourceTargetKind::Builtin,
                    scope_discriminant: None,
                    declaration_span: HirSourceSpan::new(0, 0),
                },
                module_path,
                resolved: ResolvedType::Named(canonical),
            });
        }

        // ---- Loaf module: the checked manifest owns both the identity and the qualified type name ----
        if let Some((library, rest)) = self.loaf_module(&module_path) {
            let resolved = self
                .resolve_pub_library_module_symbol_member(&library, &rest, name)
                .ok()
                .flatten()?;
            if !matches!(resolved.kind, SymbolKind::Type(_) | SymbolKind::Trait(_)) {
                return None;
            }
            let identity = resolved.canonical?;
            let root_len = self.loaf_binding_root(&library).len();
            let mut source_type_path = resolved
                .source_module_path
                .iter()
                .skip(root_len)
                .cloned()
                .collect::<Vec<_>>();
            source_type_path.push(resolved.source_name);
            let canonical_name = canonical_public_library_type_name(&library, &source_type_path.join("::"));
            return Some(QualifiedTypeReferenceInfo {
                identity,
                module_path,
                resolved: ResolvedType::Named(canonical_name),
            });
        }

        // ---- Source module: the member registry proves the kind and the identity ----
        let import = ImportPath::simple(module_path.clone());
        let member_kind = self.dependency_member_symbol_for_path(&import, name);
        let type_like = member_kind
            .as_ref()
            .is_some_and(|kind| matches!(kind, SymbolKind::Type(_) | SymbolKind::Trait(_)));
        if !type_like {
            return None;
        }
        let identity = self.dependency_member_identity(&import, name)?;
        // A direct import of a type alias registers its target and lets alias expansion replace the name; the
        // qualified spelling registers nothing local, so the target is the resolved type here. A generic alias
        // needs its arguments substituted, which only the local alias registry does, so it keeps the nominal name.
        let resolved = match member_kind {
            Some(SymbolKind::Type(TypeInfo::TypeAlias)) => self
                .dependency_member_type_alias_for_path(&import, name)
                .filter(|alias| alias.type_params.is_empty())
                .map_or_else(|| ResolvedType::Named(name.clone()), |alias| alias.target),
            _ => ResolvedType::Named(name.clone()),
        };
        Some(QualifiedTypeReferenceInfo {
            identity,
            module_path,
            resolved,
        })
    }

    /// Record one type-like lexical binding without guessing an identity from its source spelling.
    pub(super) fn record_named_type_reference_identity(&mut self, name: &str, span: Span) {
        if let Some(identity) = self.named_type_reference_identity(name) {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Return the identity already assigned to a checked type binding; lexical names select bindings only here.
    pub(super) fn named_type_reference_identity(&self, name: &str) -> Option<CanonicalSymbolId> {
        self.symbols.lookup(name).and_then(|symbol_id| {
            let symbol = self.symbols.get(symbol_id)?;
            let type_like = matches!(symbol.kind, SymbolKind::Type(_) | SymbolKind::Trait(_))
                || matches!(
                    &symbol.kind,
                    SymbolKind::RustItem(info) if info.binding != RustImportBindingKind::CrateRoot
                );
            if !type_like {
                return None;
            }
            self.symbols.identity_of(symbol_id).cloned()
        })
    }
}
