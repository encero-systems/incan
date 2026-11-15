//! Type alias registration and expansion, and public-library nominal canonicalization.

use super::*;

impl TypeChecker {
    /// Collect transparent alias names before source-order signature collection begins.
    pub(super) fn collect_source_type_alias_names(declarations: &[Spanned<Declaration>]) -> HashSet<String> {
        let mut names = HashSet::new();
        for declaration in declarations {
            match &declaration.node {
                Declaration::TypeAlias(alias) => {
                    names.insert(alias.name.clone());
                }
                Declaration::TestModule(module) => {
                    names.extend(Self::collect_source_type_alias_names(&module.body));
                }
                _ => {}
            }
        }
        names
    }

    /// Retain the authored annotation only when some leaf or constructor names a transparent type alias.
    ///
    /// Returning no spelling for an alias-free annotation preserves the existing registry-owned diagnostic names.
    /// The normalized [`ResolvedType`] remains the sole semantic type in either case.
    pub(super) fn alias_surface_type_name(&self, ty: &Type) -> Option<String> {
        self.source_type_contains_alias(ty).then(|| ty.to_string())
    }

    /// Return whether an annotation contains a locally visible transparent alias at any nesting depth.
    fn source_type_contains_alias(&self, ty: &Type) -> bool {
        let is_alias = |name: &str| self.type_aliases.contains_key(name) || self.source_type_alias_names.contains(name);
        match ty {
            Type::Simple(name) | Type::ConstrainedPrimitive(name, _) => is_alias(name),
            Type::Generic(name, args) => {
                is_alias(name)
                    || args
                        .iter()
                        .any(|argument| self.source_type_contains_alias(&argument.node))
            }
            Type::Dotted(segments) => is_alias(&segments.join(".")),
            Type::DottedGeneric(segments, args) => {
                is_alias(&segments.join("."))
                    || args
                        .iter()
                        .any(|argument| self.source_type_contains_alias(&argument.node))
            }
            Type::Function(params, result) => {
                params.iter().any(|param| self.source_type_contains_alias(&param.node))
                    || self.source_type_contains_alias(&result.node)
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.source_type_contains_alias(&inner.node)
            }
            Type::Tuple(items) => items.iter().any(|item| self.source_type_contains_alias(&item.node)),
            Type::Qualified(_) | Type::IntLiteral(_) | Type::Unit | Type::SelfType | Type::Infer => false,
        }
    }

    /// Register the resolved target for a source-level type alias.
    pub fn register_type_alias_target(&mut self, alias: &TypeAliasDecl) {
        let type_params = alias
            .type_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>();
        let target = self.resolve_type_checked(&alias.target);
        let mut nominal_identities = std::collections::BTreeMap::new();
        self.retain_alias_nominal_identities(&target, &mut nominal_identities);
        self.type_aliases.insert(
            alias.name.clone(),
            TypeAliasTarget {
                type_params,
                target,
                nominal_identities,
            },
        );
    }

    /// Snapshot alias leaves while their declaration's lexical bindings are active. A consumer must not look up an
    /// expanded alias's unimported member in its own scope or infer its owner from a project-wide spelling search.
    pub(super) fn retain_alias_nominal_identities(
        &self,
        ty: &ResolvedType,
        identities: &mut std::collections::BTreeMap<String, CanonicalSymbolId>,
    ) {
        match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                if let Some(identity) = self.named_type_reference_identity(name) {
                    identities.insert(name.clone(), identity);
                }
                if let ResolvedType::Generic(_, arguments) = ty {
                    for argument in arguments {
                        self.retain_alias_nominal_identities(argument, identities);
                    }
                }
            }
            ResolvedType::Tuple(items) => {
                for item in items {
                    self.retain_alias_nominal_identities(item, identities);
                }
            }
            ResolvedType::Function(parameters, result) => {
                for parameter in parameters {
                    self.retain_alias_nominal_identities(&parameter.ty, identities);
                }
                self.retain_alias_nominal_identities(result, identities);
            }
            ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner) => {
                self.retain_alias_nominal_identities(inner, identities);
            }
            ResolvedType::FrozenDict(key, value) => {
                self.retain_alias_nominal_identities(key, identities);
                self.retain_alias_nominal_identities(value, identities);
            }
            _ => {}
        }
    }

    /// Expand transparent source type aliases inside a resolved type. A nominal Rust alias keeps its public spelling.
    pub(super) fn expand_type_aliases(&self, ty: ResolvedType) -> ResolvedType {
        let mut expanding = HashSet::new();
        self.expand_type_aliases_inner(ty, &mut expanding)
    }

    /// Recursively expand alias leaves within one resolved type while preserving non-aliased structure.
    pub(super) fn expand_type_aliases_inner(&self, ty: ResolvedType, expanding: &mut HashSet<String>) -> ResolvedType {
        match ty {
            ResolvedType::Named(name) => self
                .expand_named_type_alias(name.clone(), Vec::new(), expanding)
                .unwrap_or(ResolvedType::Named(name)),
            ResolvedType::Generic(name, args) => {
                let expanded_args = args
                    .into_iter()
                    .map(|arg| self.expand_type_aliases_inner(arg, expanding))
                    .collect::<Vec<_>>();
                self.expand_named_type_alias(name.clone(), expanded_args.clone(), expanding)
                    .unwrap_or_else(|| {
                        if name == UNION_TYPE_NAME {
                            union_ty(expanded_args)
                        } else {
                            ResolvedType::Generic(name, expanded_args)
                        }
                    })
            }
            ResolvedType::Function(params, ret) => ResolvedType::Function(
                params
                    .into_iter()
                    .map(|param| CallableParam {
                        name: param.name,
                        ty: self.expand_type_aliases_inner(param.ty, expanding),
                        surface_type_name: param.surface_type_name,
                        kind: param.kind,
                        has_default: param.has_default,
                        is_partial_preset: param.is_partial_preset,
                        is_mut: param.is_mut,
                    })
                    .collect(),
                Box::new(self.expand_type_aliases_inner(*ret, expanding)),
            ),
            ResolvedType::Tuple(items) => ResolvedType::Tuple(
                items
                    .into_iter()
                    .map(|item| self.expand_type_aliases_inner(item, expanding))
                    .collect(),
            ),
            ResolvedType::FrozenList(inner) => {
                ResolvedType::FrozenList(Box::new(self.expand_type_aliases_inner(*inner, expanding)))
            }
            ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
                Box::new(self.expand_type_aliases_inner(*key, expanding)),
                Box::new(self.expand_type_aliases_inner(*value, expanding)),
            ),
            ResolvedType::FrozenSet(inner) => {
                ResolvedType::FrozenSet(Box::new(self.expand_type_aliases_inner(*inner, expanding)))
            }
            ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(self.expand_type_aliases_inner(*inner, expanding))),
            ResolvedType::RefMut(inner) => {
                ResolvedType::RefMut(Box::new(self.expand_type_aliases_inner(*inner, expanding)))
            }
            ResolvedType::TypeToken(inner) => {
                ResolvedType::TypeToken(Box::new(self.expand_type_aliases_inner(*inner, expanding)))
            }
            other => other,
        }
    }

    /// Expand one named alias reference, applying generic arguments and stopping on alias cycles.
    pub(super) fn expand_named_type_alias(
        &self,
        name: String,
        args: Vec<ResolvedType>,
        expanding: &mut HashSet<String>,
    ) -> Option<ResolvedType> {
        let alias = self.type_aliases.get(&name)?;
        if alias.type_params.len() != args.len() {
            return None;
        }
        if !expanding.insert(name.clone()) {
            return None;
        }
        let target = if alias.type_params.is_empty() {
            alias.target.clone()
        } else {
            let subst = type_param_subst_map(&alias.type_params, &args);
            substitute_resolved_type(&alias.target, &subst)
        };
        let expanded = self.expand_type_aliases_inner(target, expanding);
        expanding.remove(&name);
        Some(expanded)
    }

    /// Render one Rust generic argument while retaining compiled nominal ownership.
    ///
    /// Public-library type identity is intentionally kept beside `ResolvedType` for ordinary diagnostics and
    /// compatibility. Rust applications are opaque displays, however, so any imported nominal nested inside them must
    /// be encoded before the structured argument is rendered to text.
    pub(super) fn render_provider_aware_rust_arg(&self, ty: &ResolvedType) -> String {
        let mut qualified = ty.clone();
        self.canonicalize_public_library_nominals(&mut qualified);
        render_resolved_type_as_rust_arg(&qualified)
    }

    /// Freeze provider-owned native generic arguments while source annotations keep their local lexical spelling.
    pub(super) fn qualify_native_source_argument(&self, ty: &mut ResolvedType) {
        self.canonicalize_public_library_nominals(ty);
    }

    /// Replace locally aliased public-library nominals with their source-unspellable canonical provider path.
    ///
    /// Native Rust displays and compiled manifests must retain provider identity independently of the aliases that
    /// happen to be in scope for source typechecking.
    pub fn canonicalize_public_library_nominals(&self, ty: &mut ResolvedType) {
        match ty {
            ResolvedType::Trait(_) => {}
            ResolvedType::Named(name) => self.qualify_public_library_nominal_name(name),
            ResolvedType::Generic(name, args) => {
                self.qualify_public_library_nominal_name(name);
                for arg in args {
                    self.canonicalize_public_library_nominals(arg);
                }
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => self.canonicalize_public_library_nominals(inner),
            ResolvedType::FrozenDict(key, value) => {
                self.canonicalize_public_library_nominals(key);
                self.canonicalize_public_library_nominals(value);
            }
            ResolvedType::Function(params, ret) => {
                for param in params {
                    self.canonicalize_public_library_nominals(&mut param.ty);
                }
                self.canonicalize_public_library_nominals(ret);
            }
            ResolvedType::Tuple(items) => {
                for item in items {
                    self.canonicalize_public_library_nominals(item);
                }
            }
            ResolvedType::Never
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::Unknown => {}
        }
    }

    /// Qualify one imported public type name when its local alias still carries provider identity.
    pub(super) fn qualify_public_library_nominal_name(&self, name: &mut String) {
        if split_canonical_public_library_type_name(name).is_some()
            || !self.public_library_type_identities.contains_key(name)
        {
            return;
        }
        let Some([root, dependency_key, public_name]) = self.import_binding_path(name) else {
            return;
        };
        if root == PUBLIC_LIBRARY_NAMESPACE {
            *name = canonical_public_library_type_name(dependency_key, public_name);
        }
    }
}
