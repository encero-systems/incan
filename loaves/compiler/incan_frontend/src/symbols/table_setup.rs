//! Symbol table construction, module and package configuration, and builtin symbols.

use super::*;

impl SymbolTable {
    /// Create a root module scope populated with the language's builtin symbols.
    pub fn new() -> Self {
        let mut table = Self {
            nominal_revision: InventoryRevision::default(),
            nominal_binding_names: HashSet::new(),
            symbols: Vec::new(),
            scopes: vec![Scope::new(None, ScopeKind::Module)],
            current_scope: 0,
            current_scope_binding_transaction: None,
            identities: HashMap::new(),
            module_path: Vec::new(),
            package_identity: None,
            minting_builtins: false,
            dependency_interface_bindings: None,
            dependency_interface_symbol_ids: HashSet::new(),
            import_bindings: HashSet::new(),
            import_binding_paths: HashMap::new(),
            binding_keys: HashMap::new(),
            binding_collisions: Vec::new(),
        };

        // Add builtin types
        table.minting_builtins = true;
        table.add_builtins();
        table.minting_builtins = false;
        table
    }

    /// Record the module path that owns subsequently-defined local declarations.
    ///
    /// Called by the typechecker as soon as it knows its module identity, before user declarations are collected.
    /// Builtins are already defined by then and keep their [`SymbolOrigin::Builtin`] identities.
    pub fn set_module_path(&mut self, module_path: Vec<String>) {
        self.module_path = module_path;
    }

    /// Record that subsequently checked declarations are owned by one compiled package.
    pub fn set_package_identity(&mut self, package_identity: Option<String>) {
        self.package_identity = package_identity;
    }

    /// Return the compiled package that owns the declarations being checked, when the check produces one.
    pub fn package_identity(&self) -> Option<&str> {
        self.package_identity.as_deref()
    }

    /// Return the origin that owns declarations in the currently checked module.
    pub(crate) fn declaration_origin(&self) -> SymbolOrigin {
        self.package_identity
            .as_ref()
            .map(|library| SymbolOrigin::Package {
                library: library.clone(),
                module_path: self.module_path.clone(),
            })
            .unwrap_or_else(|| SymbolOrigin::Module(self.module_path.clone()))
    }

    /// Return the RFC 120 canonical identity minted for a defined symbol, if the definition proved one.
    ///
    /// Absent means the definition was not classifiable at its site (overload set, unclassified placeholder,
    /// module binding) or was an import whose target resolution did not prove an identity. Consumers must treat
    /// absence as "unproven" and fail closed rather than reconstruct an identity from the symbol's spelling.
    pub fn identity_of(&self, id: SymbolId) -> Option<&CanonicalSymbolId> {
        self.identities.get(&id)
    }

    /// Return the canonical root identity for one builtin function registry entry.
    ///
    /// This deliberately bypasses lexical lookup: an ordinary builtin such as `len` may be shadowed locally, while
    /// an explicit `std.builtins.len` resolution still needs the compiler-owned builtin identity. Some builtin
    /// functions are recognized directly by the typechecker and do not need a physical root-scope symbol, but every
    /// closed registry id still has the same canonical registry identity.
    pub fn builtin_function_identity(&self, builtin: BuiltinFnId) -> Option<CanonicalSymbolId> {
        canonical_builtin_function_identity(builtin)
    }

    /// Populate root types, traits, carrier constructors and global functions in their established registry order.
    pub(super) fn add_builtins(&mut self) {
        self.add_builtin_types();
        self.add_builtin_traits();
        self.add_builtin_variants();
        self.add_builtin_functions();
    }

    /// Define canonical builtin type spellings and aliases once per spelling, retaining the shared registry
    /// identity for aliases. Package-scoped surface types still require an explicit import.
    pub(super) fn add_builtin_types(&mut self) {
        // Builtin types (from the canonical `incan_lang::lang::types` registries).
        //
        // We define both canonical spellings and aliases so name lookup stays robust and we avoid
        // drift between the compiler and the language vocabulary registries. Each entry pairs the
        // defined spelling with its canonical registry spelling, so every alias records the one RFC 120
        // registry identity instead of minting a second identity per spelling.
        let mut builtin_types: Vec<(&'static str, &'static str)> = Vec::new();
        for t in numerics::NUMERIC_TYPES {
            builtin_types.push((t.canonical, t.canonical));
            builtin_types.extend(t.aliases.iter().map(|alias| (*alias, t.canonical)));
        }
        for t in stringlike::STRING_LIKE_TYPES {
            builtin_types.push((t.canonical, t.canonical));
            builtin_types.extend(t.aliases.iter().map(|alias| (*alias, t.canonical)));
        }
        for t in collections::COLLECTION_TYPES {
            builtin_types.push((t.canonical, t.canonical));
            builtin_types.extend(t.aliases.iter().map(|alias| (*alias, t.canonical)));
        }
        for mode in incan_lang::lang::types::ownership::OWNERSHIP_MODES {
            builtin_types.push((mode.as_str(), mode.as_str()));
        }
        for t in surface_types::SURFACE_TYPES {
            // RFC 022: stdlib-scoped types must be explicitly imported (e.g. `from std.web import App`).
            // Only truly global surface types (Rust interop helpers) are injected here.
            if surface_types::is_global(t.item.id) {
                builtin_types.push((t.item.canonical, t.item.canonical));
                builtin_types.extend(t.item.aliases.iter().map(|alias| (*alias, t.item.canonical)));
            }
        }
        // Unit-ish types that are not yet modeled in `incan_lang::lang::types`.
        builtin_types.push((conventions::UNIT_TYPE_NAME, conventions::UNIT_TYPE_NAME));
        builtin_types.push((conventions::NONE_TYPE_NAME, conventions::NONE_TYPE_NAME));
        builtin_types.push((UNION_TYPE_NAME, UNION_TYPE_NAME));

        // Deduplicate to avoid defining the same builtin twice.
        let mut seen: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
        for (name, canonical) in builtin_types.into_iter().filter(|(n, _)| seen.insert(*n)) {
            let id = self.define(Symbol {
                name: name.to_string(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: Span::default(),
                scope: 0,
            });
            self.record_builtin_identity(id, canonical);
        }
    }

    /// Define builtin trait placeholders with their canonical identities. Awaitable retains its existing single
    /// output binder; executable trait declarations remain owned by their standard packages.
    pub(super) fn add_builtin_traits(&mut self) {
        for info in traits::TRAITS {
            let type_params = match info.id {
                TraitId::Awaitable => vec!["T".to_string()],
                _ => Vec::new(),
            };
            let id = self.define(Symbol {
                name: info.canonical.to_string(),
                kind: SymbolKind::Trait(TraitInfo {
                    type_params,
                    methods: HashMap::new(),
                    method_aliases: HashMap::new(),
                    properties: HashMap::new(),
                    requires: vec![],
                    supertraits: vec![],
                }),
                span: Span::default(),
                scope: 0,
            });
            self.record_builtin_identity(id, info.canonical);
        }
    }

    /// Define Result and Option constructor symbols before global functions, retaining their canonical constructor
    /// identities and existing payload binders.
    pub(super) fn add_builtin_variants(&mut self) {
        // Ok(T) and Err(E) for Result
        let id = self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::Ok).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                identity: None,
                enum_name: collections::as_str(CollectionTypeId::Result).to_string(),
                fields: vec![ResolvedType::TypeVar("T".to_string())],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, constructors::as_str(constructors::ConstructorId::Ok));
        let id = self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::Err).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                identity: None,
                enum_name: collections::as_str(CollectionTypeId::Result).to_string(),
                fields: vec![ResolvedType::TypeVar("E".to_string())],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, constructors::as_str(constructors::ConstructorId::Err));
        // Some(T) and None for Option
        let id = self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::Some).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                identity: None,
                enum_name: collections::as_str(CollectionTypeId::Option).to_string(),
                fields: vec![ResolvedType::TypeVar("T".to_string())],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, constructors::as_str(constructors::ConstructorId::Some));
        let id = self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::None).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                identity: None,
                enum_name: collections::as_str(CollectionTypeId::Option).to_string(),
                fields: vec![],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, constructors::as_str(constructors::ConstructorId::None));
    }

    /// Define the physically registered global callable signatures and aliases with canonical builtin identities.
    /// Other builtin callables remain selected by the typechecker's existing registry dispatch.
    pub(super) fn add_builtin_functions(&mut self) {
        for name in std::iter::once(builtins::as_str(BuiltinFnId::Print))
            .chain(builtins::aliases(BuiltinFnId::Print).iter().copied())
        {
            let id = self.define(Symbol {
                name: name.to_string(),
                kind: SymbolKind::Function(FunctionInfo {
                    params: vec![CallableParam::named("msg", ResolvedType::Str, ParamKind::Normal)],
                    return_type: ResolvedType::Unit,
                    is_async: false,
                    type_params: vec![],
                    type_param_bounds: HashMap::new(),
                    type_param_bound_details: HashMap::new(),
                    emitted_name: None,
                }),
                span: Span::default(),
                scope: 0,
            });
            self.record_builtin_identity(id, builtins::as_str(BuiltinFnId::Print));
        }
        let id = self.define(Symbol {
            name: builtins::as_str(BuiltinFnId::Len).to_string(),
            kind: SymbolKind::Function(FunctionInfo {
                params: vec![CallableParam::named(
                    "collection",
                    ResolvedType::Unknown,
                    ParamKind::Normal,
                )],
                return_type: ResolvedType::Int,
                is_async: false,
                type_params: vec![],
                type_param_bounds: HashMap::new(),
                type_param_bound_details: HashMap::new(),
                emitted_name: None,
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, builtins::as_str(BuiltinFnId::Len));
        // range() builtin - returns an iterator
        let id = self.define(Symbol {
            name: builtins::as_str(BuiltinFnId::Range).to_string(),
            kind: SymbolKind::Function(FunctionInfo {
                params: vec![CallableParam::named("n", ResolvedType::Int, ParamKind::Normal)],
                return_type: ResolvedType::Named("Range".to_string()), // Iterator-like
                is_async: false,
                type_params: vec![],
                type_param_bounds: HashMap::new(),
                type_param_bound_details: HashMap::new(),
                emitted_name: None,
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, builtins::as_str(BuiltinFnId::Range));
    }
}
