//! Canonical qualified-name resolution across module, type, and value namespaces.

use super::*;

/// The declaration selected from a namespace before the requesting position applies its operation.
#[derive(Debug, Clone)]
pub(crate) enum ResolvedNamespaceMemberKind {
    /// An ordinary checked declaration from the symbol table or dependency registry.
    Symbol(SymbolKind),
    /// A method selected from a type or value namespace.
    Method(MethodInfo),
    /// A compiler-owned surface type whose stdlib namespace publishes it without a source declaration.
    SurfaceType(SurfaceTypeId),
}

/// One qualified declaration together with the canonical and source ownership facts later stages consume.
#[derive(Debug, Clone)]
pub(super) struct ResolvedNamespaceMember {
    /// Selected declaration kind.
    pub kind: ResolvedNamespaceMemberKind,
    /// Canonical declaration identity, when the namespace authority proves one.
    pub canonical: Option<CanonicalSymbolId>,
    /// Namespace path that answered the lookup.
    pub namespace_path: Vec<String>,
    /// Authored module that declares the selected member, when it is module-owned.
    pub source_module_path: Option<Vec<String>>,
    /// Name bound by the declaring namespace.
    pub source_name: String,
    /// Public Loaf that owns the namespace, when applicable.
    pub public_library: Option<String>,
    /// Checked Rust export metadata retained when a compiler surface normalizes a Rust-backed manifest member.
    pub surface_rust_item: Option<RustItemInfo>,
}

/// A failed namespace walk retained until the requesting position emits its one diagnostic.
#[derive(Debug, Clone)]
pub(super) enum NamespaceResolutionFailure {
    /// A public package exposed more than one declaration under the requested member name.
    AmbiguousPublicMember {
        library: String,
        module_path: Vec<String>,
        member: String,
        source_modules: Vec<String>,
    },
    /// The head resolved, but its namespace exposes no member under the requested name.
    MissingMember { namespace: String, member: String },
    /// The head is not a namespace that the remaining path can traverse.
    NotNamespace { head: String },
    /// Re-export links return to a name already visited on this walk.
    LinkLoop { links: Vec<DependencyMemberLinkLocation> },
}

impl TypeChecker {
    /// Return the Incan module namespace bound by `name`, excluding Rust and unsupported Python imports.
    ///
    /// This is the only typechecker decision that inspects [`SymbolKind::Module`] for name resolution. Every qualified
    /// source position asks this boundary instead of reinterpreting a module binding as a value with a nominal type.
    pub(super) fn module_namespace_path(&self, name: &str) -> Option<Vec<String>> {
        let symbol = self.lookup_symbol(name)?;
        let SymbolKind::Module(info) = &symbol.kind else {
            return None;
        };
        if info.is_python || info.path.first().is_some_and(|segment| segment == "rust") {
            return None;
        }
        Some(info.path.clone())
    }

    /// Classify a bare module binding without inventing an ordinary value type for it.
    pub(super) fn module_identifier_facts(&self, name: &str) -> Option<(IdentKind, ResolvedType)> {
        let symbol = self.lookup_symbol(name)?;
        let SymbolKind::Module(info) = &symbol.kind else {
            return None;
        };
        if info.path.first().is_some_and(|segment| segment == "rust") {
            Some((IdentKind::RustImport, ResolvedType::Unknown))
        } else {
            Some((IdentKind::Module, ResolvedType::Unknown))
        }
    }

    /// Retain the canonical module path selected for a complete qualified receiver expression.
    pub(super) fn record_module_namespace_reference(&mut self, span: Span, module_path: &[String]) {
        if let Some(identity) = SymbolTable::module_path_identity(module_path) {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Return whether a lexical binding is the root of a Rust namespace.
    pub(super) fn is_rust_namespace_binding(&self, name: &str) -> bool {
        self.lookup_symbol(name).is_some_and(|symbol| {
            matches!(&symbol.kind, SymbolKind::RustItem(_))
                || matches!(&symbol.kind, SymbolKind::Module(info)
                    if info.path.first().is_some_and(|segment| segment == "rust"))
        })
    }

    /// Resolve a path through the namespace of its head and return the selected declaration once.
    ///
    /// Module heads use the same checked dependency or Loaf registry as an import by name. Type and value heads use
    /// their collected member tables. The caller then decides whether the result may be called, used as a type, read
    /// as a value, or applied as a decorator; this resolver never performs that operation itself.
    pub(super) fn resolve_qualified_name(
        &mut self,
        segments: &[String],
    ) -> Result<ResolvedNamespaceMember, NamespaceResolutionFailure> {
        let Some((head, tail)) = segments.split_first() else {
            return Err(NamespaceResolutionFailure::NotNamespace { head: String::new() });
        };
        let mut resolved = self.resolve_namespace_head(head)?;
        for member in tail {
            resolved = self.resolve_namespace_member(resolved, member)?;
        }
        Ok(resolved)
    }

    /// Resolve the lexical head and follow an unresolved import placeholder through the dependency link graph.
    fn resolve_namespace_head(&mut self, head: &str) -> Result<ResolvedNamespaceMember, NamespaceResolutionFailure> {
        let symbol_id = self
            .symbols
            .lookup(head)
            .ok_or_else(|| NamespaceResolutionFailure::NotNamespace { head: head.to_string() })?;
        let symbol = self
            .symbols
            .get(symbol_id)
            .ok_or_else(|| NamespaceResolutionFailure::NotNamespace { head: head.to_string() })?;
        let existing = ResolvedNamespaceMember {
            kind: ResolvedNamespaceMemberKind::Symbol(symbol.kind.clone()),
            canonical: self.symbols.identity_of(symbol_id).cloned(),
            namespace_path: Vec::new(),
            source_module_path: None,
            source_name: head.to_string(),
            public_library: None,
            surface_rust_item: None,
        };
        if !matches!(symbol.kind, SymbolKind::Module(_)) || existing.canonical.is_some() {
            return Ok(existing);
        }
        let Some(binding_path) = self.import_binding_path(head).map(<[String]>::to_vec) else {
            return Ok(existing);
        };
        let Some((item_name, module_path)) = binding_path.split_last() else {
            return Ok(existing);
        };
        if module_path.is_empty()
            || self
                .resolved_source_module_path(&ImportPath::simple(binding_path.clone()))
                .is_some()
        {
            return Ok(existing);
        }
        let module = ImportPath::simple(module_path.to_vec());
        let resolved = self
            .resolve_dependency_member_links(&module, item_name)
            .map_err(|failure| match failure {
                DependencyMemberResolutionFailure::LinkLoop { links } => NamespaceResolutionFailure::LinkLoop { links },
            })?;
        let Some(resolved) = resolved else {
            return Ok(existing);
        };
        let Some(kind) = self.dependency_member_symbol_for_path(&module, item_name) else {
            return Ok(existing);
        };
        let source_name = resolved.declared_name;
        let public_library = match &resolved.identity.origin {
            SymbolOrigin::Package { library, .. } => Some(library.clone()),
            _ => None,
        };
        Ok(ResolvedNamespaceMember {
            kind: ResolvedNamespaceMemberKind::Symbol(kind),
            canonical: Some(resolved.identity),
            namespace_path: module_path.to_vec(),
            source_module_path: None,
            source_name,
            public_library,
            surface_rust_item: None,
        })
    }

    /// Continue a qualified walk through the namespace exposed by one already resolved declaration.
    fn resolve_namespace_member(
        &mut self,
        owner: ResolvedNamespaceMember,
        member: &str,
    ) -> Result<ResolvedNamespaceMember, NamespaceResolutionFailure> {
        let namespace = owner
            .namespace_path
            .iter()
            .chain(std::iter::once(&owner.source_name))
            .cloned()
            .collect::<Vec<_>>();
        match &owner.kind {
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Module(info)) => {
                self.resolve_module_namespace_member(&info.path, member)
            }
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Type(info)) => {
                let carrier = owner
                    .canonical
                    .as_ref()
                    .and_then(incan_semantics_core::body_ir::nominal_carrier_key);
                let exact = owner
                    .public_library
                    .as_deref()
                    .and_then(|library| {
                        self.public_nominal_namespace_info(library, &owner.source_name, owner.canonical.as_ref())
                    })
                    .or_else(|| {
                        carrier
                            .as_deref()
                            .and_then(|name| self.lookup_semantic_type_info(name))
                            .cloned()
                    })
                    .unwrap_or_else(|| info.clone());
                Self::resolved_nominal_member(
                    carrier.as_deref().unwrap_or(&owner.source_name),
                    &exact,
                    member,
                    namespace,
                )
            }
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Variable(info)) => {
                let (type_name, type_info) = self.type_namespace_for_value(&info.ty).ok_or_else(|| {
                    NamespaceResolutionFailure::NotNamespace {
                        head: namespace.join("."),
                    }
                })?;
                Self::resolved_nominal_member(type_name, type_info, member, namespace)
            }
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Static(info)) => {
                let (type_name, type_info) = self.type_namespace_for_value(&info.ty).ok_or_else(|| {
                    NamespaceResolutionFailure::NotNamespace {
                        head: namespace.join("."),
                    }
                })?;
                Self::resolved_nominal_member(type_name, type_info, member, namespace)
            }
            ResolvedNamespaceMemberKind::Symbol(_)
            | ResolvedNamespaceMemberKind::Method(_)
            | ResolvedNamespaceMemberKind::SurfaceType(_) => Err(NamespaceResolutionFailure::NotNamespace {
                head: namespace.join("."),
            }),
        }
    }

    /// Rebuild one public nominal namespace from its checked manifest rather than a projected type placeholder.
    fn public_nominal_namespace_info(
        &mut self,
        library: &str,
        source_name: &str,
        canonical: Option<&CanonicalSymbolId>,
    ) -> Option<TypeInfo> {
        let manifest = std::sync::Arc::clone(&self.loaves.entry(library)?.artifact.manifest);
        let mut kind = SymbolKind::Type(self.manifest_nominal_type_info(&manifest, source_name)?);
        let remapping = self.public_library_canonical_type_remapping(library, &manifest);
        self.remap_symbol_kind_through_declaring_module(library, &manifest, canonical, &mut kind);
        self.remap_symbol_kind_with_import_aliases(&mut kind, &remapping);
        match kind {
            SymbolKind::Type(info) => Some(info),
            _ => None,
        }
    }

    /// Package one member selected from a nominal namespace with its retained identity and traversal path.
    fn resolved_nominal_member(
        type_name: &str,
        info: &TypeInfo,
        member: &str,
        namespace_path: Vec<String>,
    ) -> Result<ResolvedNamespaceMember, NamespaceResolutionFailure> {
        let namespace = namespace_path.join(".");
        let resolved = Self::resolve_nominal_namespace_member(type_name, info, member).ok_or_else(|| {
            NamespaceResolutionFailure::MissingMember {
                namespace,
                member: member.to_string(),
            }
        })?;
        Ok(ResolvedNamespaceMember {
            canonical: Self::namespace_member_identity(&resolved),
            kind: resolved,
            namespace_path,
            source_module_path: None,
            source_name: member.to_string(),
            public_library: None,
            surface_rust_item: None,
        })
    }

    /// Resolve one member of an already checked module namespace.
    pub(super) fn resolve_module_namespace_member(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Result<ResolvedNamespaceMember, NamespaceResolutionFailure> {
        if let Some((library, rest)) = self.loaf_module(module_path) {
            return self.resolve_public_module_namespace_member(module_path, member, library, rest);
        }
        if let Some(surface) = surface_types::from_str(member)
            && surface_types::is_exported_from_module(surface, &module_path.join("."))
        {
            return Ok(ResolvedNamespaceMember {
                kind: ResolvedNamespaceMemberKind::SurfaceType(surface),
                canonical: Some(Self::surface_namespace_identity(surface)),
                namespace_path: module_path.to_vec(),
                source_module_path: Some(module_path.to_vec()),
                source_name: member.to_string(),
                public_library: None,
                surface_rust_item: None,
            });
        }
        let mut child_path = module_path.to_vec();
        child_path.push(member.to_string());
        if let Some(child_path) = self.resolved_source_module_path(&ImportPath::simple(child_path)) {
            return Ok(ResolvedNamespaceMember {
                kind: ResolvedNamespaceMemberKind::Symbol(SymbolKind::Module(ModuleInfo {
                    path: child_path.clone(),
                    is_python: false,
                })),
                canonical: SymbolTable::module_path_identity(&child_path),
                namespace_path: module_path.to_vec(),
                source_module_path: Some(child_path),
                source_name: member.to_string(),
                public_library: None,
                surface_rust_item: None,
            });
        }
        let written_import = ImportPath::simple(module_path.to_vec());
        let source_module_path = self
            .resolved_source_module_path(&written_import)
            .unwrap_or_else(|| module_path.to_vec());
        let import = ImportPath::simple(source_module_path.clone());
        if let Some(exports) = self.dependency_module_entry_for_path(&import, &self.dependency_exports)
            && !exports.iter().any(|export| Self::export_names_member(export, member))
        {
            return Err(NamespaceResolutionFailure::MissingMember {
                namespace: Self::module_import_spelling(module_path),
                member: member.to_string(),
            });
        }
        let kind = self.dependency_member_symbol_for_path(&import, member).ok_or_else(|| {
            NamespaceResolutionFailure::MissingMember {
                namespace: Self::module_import_spelling(module_path),
                member: member.to_string(),
            }
        })?;
        let canonical = self.dependency_member_identity(&import, member);
        let source_module_path = canonical
            .as_ref()
            .and_then(|identity| match &identity.origin {
                SymbolOrigin::Module(path) => Some(path.clone()),
                SymbolOrigin::Package { library, module_path }
                    if self.symbols.package_identity() == Some(library.as_str()) =>
                {
                    Some(module_path.clone())
                }
                _ => None,
            })
            .or(Some(source_module_path));
        Ok(ResolvedNamespaceMember {
            kind: ResolvedNamespaceMemberKind::Symbol(kind),
            canonical,
            namespace_path: module_path.to_vec(),
            source_module_path,
            source_name: member.to_string(),
            public_library: None,
            surface_rust_item: None,
        })
    }

    /// Resolve one declaration, compiler surface, or child namespace published by a checked Loaf module.
    fn resolve_public_module_namespace_member(
        &mut self,
        module_path: &[String],
        member: &str,
        library: String,
        public_path: Vec<String>,
    ) -> Result<ResolvedNamespaceMember, NamespaceResolutionFailure> {
        let resolved = self
            .resolve_pub_library_module_symbol_member(&library, &public_path, member)
            .map_err(|source_modules| NamespaceResolutionFailure::AmbiguousPublicMember {
                library: library.clone(),
                module_path: public_path.clone(),
                member: member.to_string(),
                source_modules,
            })?;
        if let Some(resolved) = resolved {
            let surface = surface_types::from_str(member)
                .filter(|surface| surface_types::is_exported_from_module(*surface, &module_path.join(".")));
            let surface_rust_item = surface.and_then(|_| match &resolved.kind {
                SymbolKind::RustItem(info) => Some(info.clone()),
                _ => None,
            });
            return Ok(ResolvedNamespaceMember {
                kind: surface.map_or_else(
                    || ResolvedNamespaceMemberKind::Symbol(resolved.kind),
                    ResolvedNamespaceMemberKind::SurfaceType,
                ),
                canonical: resolved.canonical,
                namespace_path: module_path.to_vec(),
                source_module_path: Some(resolved.source_module_path),
                source_name: resolved.source_name,
                public_library: Some(library),
                surface_rust_item,
            });
        }
        if let Some(surface) = surface_types::from_str(member)
            && surface_types::is_exported_from_module(surface, &module_path.join("."))
        {
            return Ok(ResolvedNamespaceMember {
                kind: ResolvedNamespaceMemberKind::SurfaceType(surface),
                canonical: Some(Self::surface_namespace_identity(surface)),
                namespace_path: module_path.to_vec(),
                source_module_path: Some(module_path.to_vec()),
                source_name: member.to_string(),
                public_library: None,
                surface_rust_item: None,
            });
        }
        let mut child_public_path = public_path;
        child_public_path.push(member.to_string());
        if self
            .loaves
            .manifest(&library)
            .is_some_and(|manifest| Self::manifest_public_module_exists(manifest, &child_public_path))
        {
            let mut child_path = self.loaf_binding_root(&library);
            child_path.extend(child_public_path);
            return Ok(ResolvedNamespaceMember {
                kind: ResolvedNamespaceMemberKind::Symbol(SymbolKind::Module(ModuleInfo {
                    path: child_path.clone(),
                    is_python: false,
                })),
                canonical: SymbolTable::module_path_identity(&child_path),
                namespace_path: module_path.to_vec(),
                source_module_path: Some(child_path),
                source_name: member.to_string(),
                public_library: Some(library),
                surface_rust_item: None,
            });
        }
        Err(NamespaceResolutionFailure::MissingMember {
            namespace: Self::module_import_spelling(module_path),
            member: member.to_string(),
        })
    }

    /// Return whether one source export exposes `member` in its module namespace.
    fn export_names_member(export: &ExportedSymbol, member: &str) -> bool {
        match export {
            ExportedSymbol::Const(name)
            | ExportedSymbol::Static(name)
            | ExportedSymbol::Type(name)
            | ExportedSymbol::Trait(name)
            | ExportedSymbol::Function(name)
            | ExportedSymbol::Capability(name)
            | ExportedSymbol::Reexported(name) => name == member,
            ExportedSymbol::Variant { variant_name, .. } => variant_name == member,
        }
    }

    /// Resolve one associated or instance member from a lexical type or value namespace.
    fn resolve_lexical_namespace_member(
        &self,
        head: &str,
        member: &str,
    ) -> Result<ResolvedNamespaceMember, NamespaceResolutionFailure> {
        let symbol_id = self
            .symbols
            .lookup(head)
            .ok_or_else(|| NamespaceResolutionFailure::NotNamespace { head: head.to_string() })?;
        let symbol = self
            .symbols
            .get(symbol_id)
            .ok_or_else(|| NamespaceResolutionFailure::NotNamespace { head: head.to_string() })?;
        let (type_name, type_info) = match &symbol.kind {
            SymbolKind::Type(info) => (head, info),
            SymbolKind::Variable(info) => self
                .type_namespace_for_value(&info.ty)
                .ok_or_else(|| NamespaceResolutionFailure::NotNamespace { head: head.to_string() })?,
            SymbolKind::Static(info) => self
                .type_namespace_for_value(&info.ty)
                .ok_or_else(|| NamespaceResolutionFailure::NotNamespace { head: head.to_string() })?,
            _ => return Err(NamespaceResolutionFailure::NotNamespace { head: head.to_string() }),
        };
        Self::resolved_nominal_member(type_name, type_info, member, vec![head.to_string()])
    }

    /// Select the nominal declaration that owns a value namespace.
    fn type_namespace_for_value<'a>(&'a self, ty: &'a ResolvedType) -> Option<(&'a str, &'a TypeInfo)> {
        let mut ty = ty;
        while let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = ty {
            ty = inner;
        }
        let name = match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name.as_str(),
            _ => return None,
        };
        Some((name, self.lookup_semantic_type_info(name)?))
    }

    /// Select one member from a collected nominal namespace without applying call or value rules.
    fn resolve_nominal_namespace_member(
        type_name: &str,
        info: &TypeInfo,
        member: &str,
    ) -> Option<ResolvedNamespaceMemberKind> {
        match info {
            TypeInfo::Class(info) => {
                Self::resolve_stored_nominal_member(&info.fields, &info.properties, &info.methods, member)
            }
            TypeInfo::Model(info) => {
                Self::resolve_stored_nominal_member(&info.fields, &info.properties, &info.methods, member)
            }
            TypeInfo::Newtype(info) => info
                .methods
                .get(member)
                .cloned()
                .map(ResolvedNamespaceMemberKind::Method),
            TypeInfo::Enum(info) => {
                if let Some(canonical) = info
                    .variant_aliases
                    .get(member)
                    .map(String::as_str)
                    .or_else(|| info.variants.iter().any(|name| name == member).then_some(member))
                {
                    return Some(ResolvedNamespaceMemberKind::Symbol(SymbolKind::Variant(VariantInfo {
                        identity: info.variant_identities.get(member).cloned(),
                        enum_name: type_name.to_string(),
                        fields: info.variant_fields.get(canonical).cloned().unwrap_or_default(),
                    })));
                }
                info.methods
                    .get(member)
                    .cloned()
                    .map(ResolvedNamespaceMemberKind::Method)
            }
            TypeInfo::Builtin | TypeInfo::TypeAlias => None,
        }
    }

    /// Select a field, property, or method from a class-like nominal member table.
    fn resolve_stored_nominal_member(
        fields: &HashMap<String, FieldInfo>,
        properties: &HashMap<String, PropertyInfo>,
        methods: &HashMap<String, MethodInfo>,
        member: &str,
    ) -> Option<ResolvedNamespaceMemberKind> {
        if let Some(field) = fields.get(member) {
            return Some(ResolvedNamespaceMemberKind::Symbol(SymbolKind::Field(field.clone())));
        }
        if let Some(property) = properties.get(member) {
            return Some(ResolvedNamespaceMemberKind::Symbol(SymbolKind::Property(
                property.clone(),
            )));
        }
        methods.get(member).cloned().map(ResolvedNamespaceMemberKind::Method)
    }

    /// Return the canonical identity carried by a nominal member result.
    fn namespace_member_identity(member: &ResolvedNamespaceMemberKind) -> Option<CanonicalSymbolId> {
        match member {
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Field(info)) => info.identity.clone(),
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Property(info)) => info.identity.clone(),
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Variant(info)) => info.identity.clone(),
            ResolvedNamespaceMemberKind::Method(info) => info.identity.clone(),
            ResolvedNamespaceMemberKind::Symbol(_) | ResolvedNamespaceMemberKind::SurfaceType(_) => None,
        }
    }

    /// Build the stable compiler identity of a surface type reached through its publishing module.
    fn surface_namespace_identity(surface: SurfaceTypeId) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Builtin,
            declaration_name: surface_types::as_str(surface).to_string(),
            kind: SemanticSourceTargetKind::Builtin,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(0, 0),
        }
    }

    /// Emit the one namespace diagnostic owned by a failed qualified lookup.
    pub(super) fn report_namespace_resolution_failure(&mut self, failure: NamespaceResolutionFailure, span: Span) {
        let error = match failure {
            NamespaceResolutionFailure::AmbiguousPublicMember {
                library,
                module_path,
                member,
                source_modules,
            } => errors::pub_library_module_member_ambiguous(&library, &module_path, &member, &source_modules, span),
            NamespaceResolutionFailure::MissingMember { namespace, member } => {
                errors::namespace_member_not_found(&namespace, &member, span)
            }
            NamespaceResolutionFailure::NotNamespace { head } => errors::qualified_name_head_not_namespace(&head, span),
            NamespaceResolutionFailure::LinkLoop { links } => {
                let locations = links
                    .into_iter()
                    .map(|link| (link.spelling, link.declaration_origin, link.declaration_span))
                    .collect::<Vec<_>>();
                errors::import_link_loop(&locations, span)
            }
        };
        self.errors.push(error);
    }
}
