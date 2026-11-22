//! Durable checked namespace projection for editable source dependencies.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::loaf_index::LoafIdentity;

mod wire;
pub use wire::{NamespaceCell, NamespaceCrumb};
pub(crate) use wire::{Project as CheckedCellProjection, callable_fields as callable_projection};

/// Durable checked surface of one editable source namespace.
///
/// Frozen Loaves carry this same [`crate::api_metadata::CheckedApiMetadata`] projection inside their manifest. The
/// editable-namespace record adds the source module key, canonical declaration identities and final published nominal
/// scope needed to install that projection through [`ModuleDependencyInterface`], without serializing transient
/// [`TypeCheckInfo`].
#[derive(Clone, Debug)]
pub struct CheckedNamespaceInterface {
    schema: u32,
    module_name: String,
    module_path: Vec<String>,
    api: crate::api_metadata::CheckedApiMetadata,
    identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
    /// Member identities by declaration name. API metadata carries a member's identity only for a package origin, so
    /// an editable module's variants, fields, properties and methods keep theirs here (#3154).
    members: std::collections::BTreeMap<String, NamespaceMemberIdentities>,
    /// Source-owned default proofs in canonical declaration order, including private nested targets.
    constructor_defaults: Vec<(
        CanonicalSymbolId,
        Vec<Option<ownership_contracts::defaults::DefaultValue>>,
    )>,
    nominal_scope: Arc<std::collections::BTreeMap<String, CanonicalSymbolId>>,
    /// Keys in the lexical scope the completed provider actually admitted for consumer Body IR.
    provider_carriers: Option<Vec<String>>,
    compiled_libraries: BTreeSet<String>,
    compiled_providers: Vec<LoafIdentity>,
    compiled_types: BTreeMap<String, NamespaceCompiledType>,
}

/// One checked binding into the compiled Loaf environment already admitted by the unit.
/// Most canonical declarations are already in the namespace's nominal scope. The differing or legacy entries
/// retain their original evidence instead of treating a spelling as a declaration.
#[derive(Clone, Debug)]
struct NamespaceCompiledType {
    dependency_key: String,
    source_path: Vec<String>,
    scope_binding: bool,
    canonical: Option<CanonicalSymbolId>,
    selected_provider: Option<u32>,
}

/// The canonical identities of one declaration's members, by member name, as the cold checker recorded them.
///
/// A consumer checked against the installed namespace records the same member identities a cold check of both
/// modules records, so its Body IR selects an imported enum variant, field or method by identity in a warm build too.
#[derive(Clone, Debug, Default)]
struct NamespaceMemberIdentities {
    variants: std::collections::BTreeMap<String, CanonicalSymbolId>,
    fields: std::collections::BTreeMap<String, CanonicalSymbolId>,
    properties: std::collections::BTreeMap<String, CanonicalSymbolId>,
    methods: std::collections::BTreeMap<String, CanonicalSymbolId>,
}

impl CheckedNamespaceInterface {
    /// Schema of the compiler-owned checked declaration root and its identity projection.
    pub const SCHEMA: u32 = 9;

    /// Capture one complete installable public namespace from an accepted editable module.
    ///
    /// Declaration forms outside checked API metadata remain on ordinary checking. A miss is explicit and never
    /// publishes a partial interface.
    pub fn from_checked(
        module_name: &str,
        module_path: &[String],
        program: &Program,
        checker: &TypeChecker,
    ) -> Result<Option<Self>, String> {
        if program.declarations.iter().any(|declaration| {
            !matches!(
                declaration.node,
                Declaration::Function(_)
                    | Declaration::Model(_)
                    | Declaration::Class(_)
                    | Declaration::Trait(_)
                    | Declaration::Enum(_)
                    | Declaration::Newtype(_)
                    | Declaration::TypeAlias(_)
                    | Declaration::Const(_)
                    | Declaration::Static(_)
                    | Declaration::Alias(_)
                    | Declaration::Partial(_)
                    | Declaration::Import(_)
                    | Declaration::Docstring(_)
            )
        }) {
            return Ok(None);
        }
        let mut api = crate::api_metadata::collect_checked_api_metadata(program, checker, module_path.to_vec());
        api.declarations.retain(
            |declaration| !matches!(declaration, crate::api_metadata::ApiDeclaration::Alias(alias) if !alias.is_public),
        );
        let imports_project_namespace = program
            .declarations
            .iter()
            .any(|declaration| matches!(declaration.node, Declaration::Import(_) | Declaration::Alias(_)));
        if api.declarations.is_empty() && !imports_project_namespace {
            return Ok(None);
        }
        let mut identities = std::collections::BTreeMap::new();
        let mut members = std::collections::BTreeMap::new();
        for declaration in &api.declarations {
            let Some(name) = namespace_declaration_name(declaration) else {
                return Ok(None);
            };
            let Some(symbol_id) = checker.symbols.lookup(name) else {
                return Err(format!("checked namespace declaration `{name}` has no symbol"));
            };
            let Some(identity) = checker.symbols.identity_of(symbol_id) else {
                return Err(format!(
                    "checked namespace declaration `{name}` has no canonical identity"
                ));
            };
            if identities.insert(name.to_owned(), identity.clone()).is_some() {
                return Ok(None);
            }
            if let Some(symbol) = checker.symbols.get(symbol_id) {
                let declared = NamespaceMemberIdentities::of(&symbol.kind);
                if !declared.is_empty() {
                    members.insert(name.to_owned(), declared);
                }
            }
        }
        let mut nominal_scope = checker.type_info().declarations.named_type_identities.clone();
        for (name, identity) in &checker.type_info().declarations.body_ir_native_type_identities {
            nominal_scope.entry(name.clone()).or_insert_with(|| identity.clone());
            if let Some(key) = incan_semantics_core::body_ir::nominal_carrier_key(identity) {
                nominal_scope.entry(key).or_insert_with(|| identity.clone());
            }
        }
        let mut constructor_defaults: Vec<_> = checker
            .constructor_defaults
            .0
            .iter()
            .filter(|(owner, _)| identity_owned_by(owner, module_path))
            .map(|(owner, fields)| (owner.clone(), fields.clone()))
            .collect();
        constructor_defaults.sort_by(|(left, _), (right, _)| left.cmp(right));
        let compiled_libraries = checker.cached_pub_libraries.iter().cloned().collect();
        let (compiled_providers, compiled_types) = Self::capture_compiled_types(checker, &nominal_scope)?;
        let retained = Self {
            schema: Self::SCHEMA,
            constructor_defaults,
            module_name: module_name.to_owned(),
            module_path: module_path.to_vec(),
            api,
            identities,
            members,
            nominal_scope: Arc::new(nominal_scope),
            provider_carriers: None,
            compiled_libraries,
            compiled_providers,
            compiled_types,
        };
        // A partial ownership projection cannot replace the ordinary provider. Compare the projected contracts
        // with the completed source check before this namespace is made available for future builds.
        let Ok(mut projected) = retained.projected_input_contracts() else {
            return Ok(None);
        };
        for (identity, contract) in &checker.type_info().declarations.ownership_contracts {
            if provider_input_contract(identity, module_path) && projected.remove(identity).as_ref() != Some(contract) {
                return Ok(None);
            }
        }
        Ok(projected.is_empty().then_some(retained))
    }

    /// Use the declaration interface chosen by ordinary dependency checking, including an import component's
    /// public declaration frontier. The lexical scope of the completed body check may contain private types.
    pub fn bind_provider_carriers(&mut self, provider: &ModuleDependencyInterface) -> Result<(), String> {
        if provider.module_path != self.module_path {
            return Err("checked namespace carrier provider disagrees with its module".into());
        }
        let mut keys = Vec::with_capacity(provider.retained_nominal_carriers.len());
        for (identity, key) in provider.retained_nominal_carriers.iter() {
            if self.nominal_scope.get(key) != Some(identity) {
                return Err(format!(
                    "checked namespace carrier {key} is missing from its declaration scope"
                ));
            }
            keys.push(key.clone());
        }
        keys.sort_unstable();
        self.provider_carriers = Some(keys);
        Ok(())
    }

    /// Keep exact provider identities once per namespace and refer to the existing canonical scope where possible.
    fn capture_compiled_types(
        checker: &TypeChecker,
        scope: &BTreeMap<String, CanonicalSymbolId>,
    ) -> Result<(Vec<LoafIdentity>, BTreeMap<String, NamespaceCompiledType>), String> {
        let providers: BTreeSet<_> = checker
            .public_library_type_identities
            .values()
            .filter_map(|identity| identity.selected_provider.as_ref())
            .collect();
        let positions = providers
            .iter()
            .enumerate()
            .map(|(ordinal, &provider)| {
                Ok((
                    provider,
                    u32::try_from(ordinal).map_err(|_| "too many compiled namespace providers")?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let bindings = checker
            .public_library_type_identities
            .iter()
            .map(|(name, identity)| {
                let scope_binding = scope
                    .get(name)
                    .is_some_and(|owner| identity.canonical.as_ref() == Some(owner));
                (
                    name.clone(),
                    NamespaceCompiledType {
                        dependency_key: identity.dependency_key.clone(),
                        source_path: identity.source_path.clone(),
                        scope_binding,
                        canonical: if scope_binding {
                            None
                        } else {
                            identity.canonical.clone()
                        },
                        selected_provider: identity.selected_provider.as_ref().map(|provider| positions[provider]),
                    },
                )
            })
            .collect();
        Ok((providers.into_iter().cloned().collect(), bindings))
    }

    /// Open the same compiled type aliases and declaring instances as the fresh source dependency.
    fn restore_compiled_types(&self) -> Result<HashMap<String, PublicLibraryTypeIdentity>, String> {
        self.compiled_types
            .iter()
            .map(|(name, binding)| {
                let canonical = if binding.scope_binding {
                    Some(
                        self.nominal_scope
                            .get(name)
                            .ok_or("checked namespace has no compiled type scope entry")?
                            .clone(),
                    )
                } else {
                    binding.canonical.clone()
                };
                let selected_provider = binding
                    .selected_provider
                    .map(|ordinal| {
                        self.compiled_providers
                            .get(ordinal as usize)
                            .cloned()
                            .ok_or("checked namespace has no selected compiled provider")
                    })
                    .transpose()?;
                Ok((
                    name.clone(),
                    PublicLibraryTypeIdentity {
                        dependency_key: binding.dependency_key.clone(),
                        source_path: binding.source_path.clone(),
                        canonical,
                        selected_provider,
                    },
                ))
            })
            .collect()
    }

    /// Project the checked declarations into their compiler-owned Crumb schemas.
    pub fn to_bytes(&self, codec: &impl NamespaceCrumb) -> Result<Vec<u8>, String> {
        codec.write(self.wire_value()?)
    }

    /// The complete checked value, before selecting its stored or semantic projection.
    pub fn wire_value(&self) -> Result<NamespaceCell, String> {
        self.validate()?;
        Ok(wire::project(self))
    }

    /// Admit the bounded typed Crumb before installing any identities or checked tables in a consumer.
    pub fn from_bytes(bytes: &[u8], codec: &impl NamespaceCrumb) -> Result<Self, String> {
        Self::from_wire_value(codec.read(bytes)?)
    }

    /// Validate and hydrate an already decoded compiler-owned Crumb projection.
    pub fn from_wire_value(root: NamespaceCell) -> Result<Self, String> {
        let value = wire::restore(root)?;
        value.validate()?;
        Ok(value)
    }

    /// Join an exact emitted method to its position-independent declaration under the checked enclosing nominal.
    /// Same-name overloads require an independently proved binding ordinal and stay on full namespace admission.
    pub fn lowering_member_owners(&self) -> std::collections::BTreeMap<CanonicalSymbolId, String> {
        use crate::api_metadata::ApiDeclaration;
        use incan_semantics_core::stable_identity::{StableDeclarationContext, StableDeclarationId};
        let mut owners = std::collections::BTreeMap::new();
        for declaration in &self.api.declarations {
            let methods = match declaration {
                ApiDeclaration::Model(value) => &value.methods,
                ApiDeclaration::Class(value) => &value.methods,
                ApiDeclaration::Enum(value) => &value.methods,
                ApiDeclaration::Trait(value) => &value.methods,
                ApiDeclaration::Newtype(value) => &value.methods,
                _ => continue,
            };
            let name = declaration.name();
            let Some(outer) = self
                .identities
                .get(name)
                .and_then(|id| StableDeclarationId::from_canonical(id, None, None))
            else {
                continue;
            };
            let Some(declared) = self.members.get(name) else {
                continue;
            };
            let mut names = std::collections::HashMap::new();
            for method in methods {
                *names.entry(&method.name).or_insert(0) += 1;
            }
            for method in methods {
                let Some(id) = declared.methods.get(&method.name) else {
                    continue;
                };
                if names.get(&method.name) != Some(&1)
                    || id.declaration_span != method.anchor.span
                    || id.kind != SemanticSourceTargetKind::Method
                {
                    continue;
                }
                let context = StableDeclarationContext {
                    owner: outer.clone(),
                    binding_ordinal: 0,
                };
                if let Some(stable) = StableDeclarationId::from_canonical(id, None, Some(context)) {
                    owners.insert(id.clone(), stable.render_compact());
                }
            }
        }
        owners
    }

    /// Return the source module key this interface installs.
    pub fn module_name(&self) -> &str {
        &self.module_name
    }

    /// Return the canonical logical path this interface owns.
    pub fn module_path(&self) -> &[String] {
        &self.module_path
    }

    /// Rebuild the exact immutable dependency tables consumed by downstream editable modules.
    pub fn installable(&self) -> Result<Arc<ModuleDependencyInterface>, String> {
        self.validate()?;
        let mut converter = TypeChecker::new();
        let foreign_aliases = checked_namespace_foreign_nominal_remapping(&self.api);
        let mut symbols = HashMap::new();
        let mut reexports = HashMap::new();
        let mut symbol_rows = Vec::new();
        for declaration in &self.api.declarations {
            let name = namespace_declaration_name(declaration)
                .ok_or("checked namespace interface contains an unsupported declaration")?;
            let identity = self
                .identities
                .get(name)
                .ok_or_else(|| format!("checked namespace declaration `{name}` has no identity"))?
                .clone();
            let Some(mut kind) = converter.symbol_kind_from_api_declaration(None, declaration) else {
                let crate::api_metadata::ApiDeclaration::Alias(alias) = declaration else {
                    return Err(format!("checked namespace declaration `{name}` has no semantic symbol"));
                };
                let Some((target_name, target_module)) = alias.target_path.split_last() else {
                    return Err(format!("checked namespace alias `{name}` has no target"));
                };
                reexports.insert(
                    name.to_owned(),
                    DependencyMemberReexport {
                        target_module: ImportPath::simple(target_module.to_vec()),
                        target_name: target_name.clone(),
                        declaration_origin: SymbolOrigin::Module(self.module_path.clone()),
                        declaration_span: namespace_declaration_span(declaration),
                    },
                );
                continue;
            };
            converter.remap_checked_namespace_aliases(&mut kind, &self.nominal_scope, &foreign_aliases);
            if let Some(members) = self.members.get(name) {
                members.restore(&mut kind);
            }
            symbols.insert(name.to_owned(), kind.clone());
            symbol_rows.push((
                Symbol {
                    name: name.to_owned(),
                    kind,
                    span: namespace_declaration_span(declaration),
                    scope: 0,
                },
                Some(identity),
            ));
        }
        let exports = self
            .api
            .declarations
            .iter()
            .flat_map(namespace_exports)
            .collect::<Vec<_>>();
        let identities = self
            .identities
            .iter()
            .map(|(name, identity)| (name.clone(), identity.clone()))
            .collect::<HashMap<_, _>>();
        let nominal_scope = &self.nominal_scope;
        let mut retained_scope = false;
        for (name, kind) in &symbols {
            if let Some(identity) = identities.get(name) {
                retained_scope |= converter.retain_declaration_nominal_scope(identity, kind, nominal_scope);
            }
        }
        if let Some(keys) = &self.provider_carriers {
            converter
                .retained_nominal_carriers
                .extend(keys.iter().map(|key| (nominal_scope[key].clone(), key.clone())));
        } else if retained_scope {
            // Direct in-memory projections use the provider's own admitted declarations.
            converter.retain_nominal_scope_carriers(
                nominal_scope
                    .values()
                    .filter(|identity| identity_owned_by(identity, &self.module_path)),
            );
        }
        let (transitive_public_types, transitive_public_traits) = provider_transitive_semantics(&symbols, &identities);
        let symbols = Arc::new(symbols);
        let identities = Arc::new(identities);
        let interface = ModuleDependencyInterface {
            module_name: self.module_name.clone(),
            module_path: self.module_path.clone(),
            public_only: true,
            exports: Arc::from(exports),
            member_symbols: Arc::clone(&symbols),
            member_type_aliases: Arc::default(),
            registry_definitions: None,
            registry_consts: Arc::default(),
            member_partial_projections: Arc::default(),
            direct_member_symbols: symbols,
            direct_member_type_aliases: Arc::default(),
            direct_member_identities: identities,
            declaration_nominal_scopes: Arc::new(std::mem::take(&mut converter.declaration_nominal_scopes)),
            retained_nominal_carriers: Arc::new(std::mem::take(&mut converter.retained_nominal_carriers)),
            declaration_callables: Arc::new(std::mem::take(&mut converter.declaration_callables)),
            input_contracts: Arc::new(self.projected_input_contracts()?),
            constructor_defaults: Arc::new(self.constructor_defaults.iter().cloned().collect()),
            member_reexports: Arc::new(reexports),
            body_ir_pub_reexports: Arc::default(),
            direct_member_partial_projections: Arc::default(),
            derivable_modules: Arc::default(),
            derivable_trait_identities: Arc::default(),
            module_traits: Arc::default(),
            trait_rust_derive_paths: Arc::default(),
            transitive_public_types: Arc::new(transitive_public_types),
            transitive_public_traits: Arc::new(transitive_public_traits),
            compiled_libraries: self.compiled_libraries.iter().cloned().collect(),
            public_library_type_identities: Arc::new(self.restore_compiled_types()?),
            symbol_rows: Arc::from(symbol_rows),
            snapshot: std::sync::OnceLock::new(),
        };
        Ok(Arc::new(interface))
    }

    /// Key the exported ownership facts by the same checked declaration selected at the editable call site.
    fn projected_input_contracts(
        &self,
    ) -> Result<
        std::collections::BTreeMap<CanonicalSymbolId, crate::ownership_metadata::CheckedOwnershipContract>,
        String,
    > {
        use crate::api_metadata::ApiDeclaration;
        use crate::library_manifest::ownership::{OwnershipExport, hydrate};
        let mut contracts = std::collections::BTreeMap::new();
        let mut add = |name: &str,
                       id: Option<&CanonicalSymbolId>,
                       kind: SemanticSourceTargetKind,
                       contract: &OwnershipExport| {
            let identity = id
                .filter(|id| id.declaration_name == name && id.kind == kind && identity_owned_by(id, &self.module_path))
                .ok_or_else(|| format!("checked ownership contract has no matching declaration for {name}"))?;
            let value = hydrate(contract).map_err(|error| error.to_string())?;
            if contracts.insert(identity.clone(), value).is_some() {
                return Err(format!("ambiguous editable ownership contract for {name}"));
            }
            Ok(())
        };
        for declaration in &self.api.declarations {
            let methods = match declaration {
                ApiDeclaration::Function(function) => {
                    if let Some(contract) = &function.ownership {
                        add(
                            &function.name,
                            self.identities.get(&function.name),
                            SemanticSourceTargetKind::Function,
                            contract,
                        )?;
                    }
                    continue;
                }
                ApiDeclaration::Model(value) => &value.methods,
                ApiDeclaration::Class(value) => &value.methods,
                ApiDeclaration::Enum(value) => &value.methods,
                ApiDeclaration::Trait(value) => &value.methods,
                ApiDeclaration::Newtype(value) => &value.methods,
                _ => continue,
            };
            for method in methods {
                if let Some(contract) = &method.ownership {
                    let id = self
                        .members
                        .get(declaration.name())
                        .and_then(|group| group.methods.get(&method.name))
                        .filter(|id| id.declaration_span == method.anchor.span);
                    add(&method.name, id, SemanticSourceTargetKind::Method, contract)?;
                }
            }
        }
        Ok(contracts)
    }

    /// Refuse crossed module identities, duplicate names and schema drift before installation.
    fn validate(&self) -> Result<(), String> {
        if self.schema != Self::SCHEMA
            || self.api.module_path != self.module_path
            || self.api.schema_version != crate::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION
            || self.api.declarations.len() != self.identities.len()
        {
            return Err("checked namespace interface has mismatched authority".into());
        }
        if self.constructor_defaults.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || self.constructor_defaults.iter().any(|(owner, _)| {
                !matches!(
                    owner.kind,
                    SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
                ) || !identity_owned_by(owner, &self.module_path)
            })
        {
            return Err("checked namespace constructor defaults disagree with their declaration owner".into());
        }
        if self.compiled_providers.windows(2).any(|pair| pair[0] >= pair[1])
            || self.compiled_types.iter().any(|(name, binding)| {
                binding.scope_binding && (binding.canonical.is_some() || !self.nominal_scope.contains_key(name))
                    || binding
                        .selected_provider
                        .is_some_and(|ordinal| ordinal as usize >= self.compiled_providers.len())
            })
        {
            return Err("checked namespace has invalid compiled type bindings".into());
        }
        if self.provider_carriers.as_ref().is_some_and(|keys| {
            keys.windows(2).any(|pair| pair[0] >= pair[1])
                || keys.iter().any(|key| {
                    !self.nominal_scope.get(key).is_some_and(|identity| {
                        identity_owned_by(identity, &self.module_path)
                            && incan_semantics_core::body_ir::nominal_carrier_key(identity).as_ref() == Some(key)
                    })
                })
        }) {
            return Err("checked namespace has invalid provider carrier keys".into());
        }
        if self.members.keys().any(|name| !self.identities.contains_key(name)) {
            return Err("checked namespace interface has members of an undeclared name".into());
        }
        for declaration in &self.api.declarations {
            let Some(name) = namespace_declaration_name(declaration) else {
                return Err("checked namespace interface contains an unsupported declaration".into());
            };
            let Some(identity) = self.identities.get(name) else {
                return Err("checked namespace interface is missing a declaration identity".into());
            };
            if let crate::api_metadata::ApiDeclaration::Alias(alias) = declaration
                && alias.target_path.is_empty()
                && TypeChecker::new()
                    .symbol_kind_from_api_declaration(None, declaration)
                    .is_none()
            {
                return Err(format!("checked namespace alias `{name}` has no target"));
            }
            if !matches!(declaration, crate::api_metadata::ApiDeclaration::Alias(_))
                && (identity.declaration_name != name
                    || !matches!(&identity.origin, SymbolOrigin::Module(path) if path == &self.module_path))
            {
                return Err("checked namespace interface declaration identity disagrees with its module".into());
            }
        }
        Ok(())
    }
}

impl NamespaceMemberIdentities {
    /// Read the member identities a checked declaration's symbol carries.
    fn of(kind: &SymbolKind) -> Self {
        let mut members = Self::default();
        match kind {
            SymbolKind::Type(TypeInfo::Enum(info)) => {
                members.variants = info
                    .variant_identities
                    .iter()
                    .map(|(name, identity)| (name.clone(), identity.clone()))
                    .collect();
                members.methods = method_identities(&info.methods);
            }
            SymbolKind::Type(TypeInfo::Model(info)) => {
                members.fields = field_identities(&info.fields);
                members.properties = property_identities(&info.properties);
                members.methods = method_identities(&info.methods);
            }
            SymbolKind::Type(TypeInfo::Class(info)) => {
                members.fields = field_identities(&info.fields);
                members.properties = property_identities(&info.properties);
                members.methods = method_identities(&info.methods);
            }
            SymbolKind::Type(TypeInfo::Newtype(info)) => members.methods = method_identities(&info.methods),
            SymbolKind::Trait(info) => {
                members.properties = property_identities(&info.properties);
                members.methods = method_identities(&info.methods);
            }
            _ => {}
        }
        members
    }

    /// Whether the declaration has no member identity to keep.
    fn is_empty(&self) -> bool {
        self.variants.is_empty() && self.fields.is_empty() && self.properties.is_empty() && self.methods.is_empty()
    }

    /// Give an installed declaration's members the identities their API rows could not carry. A member the
    /// installed symbol does not have, or one that already has an identity, is left as it is.
    fn restore(&self, kind: &mut SymbolKind) {
        match kind {
            SymbolKind::Type(TypeInfo::Enum(info)) => {
                for (name, identity) in &self.variants {
                    if info.variants.contains(name) || info.variant_aliases.contains_key(name) {
                        info.variant_identities
                            .entry(name.clone())
                            .or_insert_with(|| identity.clone());
                    }
                }
                restore_methods(&self.methods, &mut info.methods, &mut info.method_overloads);
            }
            SymbolKind::Type(TypeInfo::Model(info)) => {
                restore_fields(&self.fields, &mut info.fields);
                restore_properties(&self.properties, &mut info.properties);
                restore_methods(&self.methods, &mut info.methods, &mut info.method_overloads);
            }
            SymbolKind::Type(TypeInfo::Class(info)) => {
                restore_fields(&self.fields, &mut info.fields);
                restore_properties(&self.properties, &mut info.properties);
                restore_methods(&self.methods, &mut info.methods, &mut info.method_overloads);
            }
            SymbolKind::Type(TypeInfo::Newtype(info)) => {
                restore_methods(&self.methods, &mut info.methods, &mut info.method_overloads);
            }
            SymbolKind::Trait(info) => {
                restore_properties(&self.properties, &mut info.properties);
                let mut overloads = HashMap::new();
                restore_methods(&self.methods, &mut info.methods, &mut overloads);
            }
            _ => {}
        }
    }
}

/// Each field's identity, by field name.
fn field_identities(fields: &HashMap<String, FieldInfo>) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
    fields
        .iter()
        .filter_map(|(name, field)| Some((name.clone(), field.identity.clone()?)))
        .collect()
}

/// Each property's identity, by property name.
fn property_identities(
    properties: &HashMap<String, PropertyInfo>,
) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
    properties
        .iter()
        .filter_map(|(name, property)| Some((name.clone(), property.identity.clone()?)))
        .collect()
}

/// Each method's identity, by method name.
fn method_identities(methods: &HashMap<String, MethodInfo>) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
    methods
        .iter()
        .filter_map(|(name, method)| Some((name.clone(), method.identity.clone()?)))
        .collect()
}

/// Give each installed field without an identity its kept one.
fn restore_fields(
    identities: &std::collections::BTreeMap<String, CanonicalSymbolId>,
    fields: &mut HashMap<String, FieldInfo>,
) {
    for (name, identity) in identities {
        if let Some(field) = fields.get_mut(name)
            && field.identity.is_none()
        {
            field.identity = Some(identity.clone());
        }
    }
}

/// Give each installed property without an identity its kept one.
fn restore_properties(
    identities: &std::collections::BTreeMap<String, CanonicalSymbolId>,
    properties: &mut HashMap<String, PropertyInfo>,
) {
    for (name, identity) in identities {
        if let Some(property) = properties.get_mut(name)
            && property.identity.is_none()
        {
            property.identity = Some(identity.clone());
        }
    }
}

/// Give each installed method without an identity its kept one, and a single overload of that name the same.
fn restore_methods(
    identities: &std::collections::BTreeMap<String, CanonicalSymbolId>,
    methods: &mut HashMap<String, MethodInfo>,
    overloads: &mut HashMap<String, Vec<MethodInfo>>,
) {
    for (name, identity) in identities {
        if let Some(method) = methods.get_mut(name)
            && method.identity.is_none()
        {
            method.identity = Some(identity.clone());
        }
        if let Some(group) = overloads.get_mut(name)
            && let [method] = group.as_mut_slice()
            && method.identity.is_none()
        {
            method.identity = Some(identity.clone());
        }
    }
}

/// Convert artifact-qualified API leaves to the canonical carrier spellings used by editable dependency interfaces.
fn checked_namespace_foreign_nominal_remapping(
    api: &crate::api_metadata::CheckedApiMetadata,
) -> HashMap<String, String> {
    use crate::library_manifest::{TypeRef, VisitTypeRefs};

    let mut projection = api.clone();
    let mut remapping = HashMap::new();
    projection.visit_type_refs(&mut |ty| {
        let origin = match ty {
            TypeRef::Named { origin, .. } | TypeRef::Applied { origin, .. } => origin.as_ref(),
            _ => None,
        };
        let Some(origin) = origin else { return };
        let Some(identity) = origin.canonical.hydrate() else {
            return;
        };
        let Some(carrier) = incan_semantics_core::body_ir::nominal_carrier_key(&identity) else {
            return;
        };
        remapping.insert(origin.binding_key(), carrier);
    });
    remapping
}

/// Return the declaration spelling shared by the frozen API row, symbol table and stable identity map.
fn namespace_declaration_name(declaration: &crate::api_metadata::ApiDeclaration) -> Option<&str> {
    match declaration {
        crate::api_metadata::ApiDeclaration::Function(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::Model(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::Class(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::Trait(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::Enum(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::Newtype(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::TypeAlias(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::Const(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::Static(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::Alias(item) => Some(&item.name),
        crate::api_metadata::ApiDeclaration::Partial(item) => Some(&item.name),
    }
}

/// Return the checked source anchor shared by every currently supported namespace declaration.
fn namespace_declaration_span(declaration: &crate::api_metadata::ApiDeclaration) -> crate::ast::Span {
    match declaration {
        crate::api_metadata::ApiDeclaration::Function(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::Model(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::Class(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::Trait(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::Enum(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::Newtype(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::TypeAlias(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::Const(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::Static(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::Alias(item) => item.anchor.span,
        crate::api_metadata::ApiDeclaration::Partial(item) => item.anchor.span,
    }
}

/// Project one supported frozen API row into the source module's public export table.
fn namespace_exports(declaration: &crate::api_metadata::ApiDeclaration) -> Vec<ExportedSymbol> {
    match declaration {
        crate::api_metadata::ApiDeclaration::Function(item) => vec![ExportedSymbol::Function(item.name.clone())],
        crate::api_metadata::ApiDeclaration::Model(item) => vec![ExportedSymbol::Type(item.name.clone())],
        crate::api_metadata::ApiDeclaration::Class(item) => vec![ExportedSymbol::Type(item.name.clone())],
        crate::api_metadata::ApiDeclaration::Trait(item) => vec![ExportedSymbol::Trait(item.name.clone())],
        crate::api_metadata::ApiDeclaration::Enum(item) => std::iter::once(ExportedSymbol::Type(item.name.clone()))
            .chain(item.variants.iter().map(|variant| ExportedSymbol::Variant {
                enum_name: item.name.clone(),
                variant_name: variant.name.clone(),
            }))
            .collect(),
        crate::api_metadata::ApiDeclaration::Newtype(item) => vec![ExportedSymbol::Type(item.name.clone())],
        crate::api_metadata::ApiDeclaration::TypeAlias(item) => vec![ExportedSymbol::Type(item.name.clone())],
        crate::api_metadata::ApiDeclaration::Const(item) => vec![ExportedSymbol::Const(item.name.clone())],
        crate::api_metadata::ApiDeclaration::Static(item) => vec![ExportedSymbol::Static(item.name.clone())],
        crate::api_metadata::ApiDeclaration::Alias(item) => vec![ExportedSymbol::Reexported(item.name.clone())],
        crate::api_metadata::ApiDeclaration::Partial(item) => vec![ExportedSymbol::Function(item.name.clone())],
    }
}

#[cfg(test)]
mod import_refusals;

#[cfg(test)]
mod tests {
    use super::*;

    /// A retained editable interface must install the same nominal ownership tables used by ordinary dependencies.
    #[test]
    fn installable_retains_nominal_scopes_carriers_and_layouts() -> Result<(), String> {
        let source = "pub model Order:\n    pub quantity: int\n    pub unit_price: float\n\npub def total(order: Order) -> float:\n    return float(order.quantity) * order.unit_price\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let module_path = vec!["domain".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let mut retained = CheckedNamespaceInterface::from_checked("domain", &module_path, &program, &checker)?
            .ok_or("model namespace was not retained")?;
        let external = CanonicalSymbolId::module_declaration(
            vec!["dependency".into()],
            "PrivateState",
            SemanticSourceTargetKind::Model,
            Default::default(),
        );
        Arc::make_mut(&mut retained.nominal_scope).insert("PrivateState".into(), external.clone());
        let installed = retained.installable()?;
        let order = retained.identities.get("Order").ok_or("missing Order identity")?;
        let scope = installed
            .declaration_nominal_scopes
            .get(order)
            .ok_or("missing Order declaration scope")?;
        assert_eq!(scope.get("Order"), Some(order));
        assert_eq!(scope.get("PrivateState"), Some(&external));
        assert!(!installed.retained_nominal_carriers.contains_key(&external));
        let carrier =
            incan_semantics_core::body_ir::nominal_carrier_key(order).ok_or("missing Order nominal carrier")?;
        assert_eq!(installed.retained_nominal_carriers.get(order), Some(&carrier));
        assert!(installed.transitive_public_types.contains_key("Order"));
        assert!(installed.transitive_public_types.contains_key(&carrier));
        let SymbolKind::Type(TypeInfo::Model(model)) = installed
            .direct_member_symbols
            .get("Order")
            .ok_or("missing Order symbol")?
        else {
            return Err("Order is not a retained model".into());
        };
        assert!(model.fields.contains_key("quantity"));
        assert!(model.fields.contains_key("unit_price"));
        Ok(())
    }

    /// A cyclic provider's checked body can see private names outside the interface sealed for its consumers.
    #[test]
    fn cyclic_provider_publishes_only_its_selected_nominal_carriers() -> Result<(), String> {
        fn parse(source: &str) -> Result<Program, String> {
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
        }
        let domain = parse(
            "from peer import bump\n\nmodel PrivateState:\n    pub value: int\n\npub model Handle:\n    pub value: int\n\npub def count() -> int:\n    return bump(2)\n",
        )?;
        let peer = parse("from domain import Handle\n\npub def bump(value: int) -> int:\n    return value + 1\n")?;
        let domain_path = vec!["domain".to_owned()];
        let peer_path = vec!["peer".to_owned()];
        let configured = || {
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(domain_path.clone()));
            checker.register_dependency_module_path_segments("domain", domain_path.clone());
            checker.register_dependency_module_path_segments("peer", peer_path.clone());
            checker
        };
        let component = [
            ("domain", &domain, Arc::from(crate::module::exported_symbols(&domain))),
            ("peer", &peer, Arc::from(crate::module::exported_symbols(&peer))),
        ];
        let imports = [("peer", &peer)];
        let providers = configured().build_dependency_interface_component(&component, &imports, true);
        let provider = providers
            .first()
            .ok_or("the component did not seal its domain interface")?;
        let mut checker = configured();
        let peer_provider = providers
            .get(1)
            .ok_or("the component did not seal its peer interface")?;
        checker.select_dependency_interfaces(vec![Arc::clone(peer_provider)]);
        checker
            .check_with_imports(&domain, &imports)
            .map_err(|errors| format!("{errors:?}"))?;
        let mut retained = CheckedNamespaceInterface::from_checked("domain", &domain_path, &domain, &checker)?
            .ok_or("the checked domain could not publish its namespace")?;
        let private = retained
            .nominal_scope
            .get("PrivateState")
            .ok_or("missing private lexical type")?;
        assert!(!provider.retained_nominal_carriers.contains_key(private));
        assert_ne!(
            retained.installable()?.retained_nominal_carriers,
            provider.retained_nominal_carriers
        );

        retained.bind_provider_carriers(provider)?;
        let reopened = CheckedNamespaceInterface::from_wire_value(retained.wire_value()?)?;
        assert_eq!(
            reopened.installable()?.retained_nominal_carriers,
            provider.retained_nominal_carriers
        );
        assert!(reopened.nominal_scope.contains_key("PrivateState"));
        Ok(())
    }

    /// Restore exact compiled aliases, including bindings with no canonical proof in older compiled Loaves.
    #[test]
    fn reopened_namespace_retains_compiled_type_bindings() -> Result<(), String> {
        let source = "pub def value() -> int:\n    return 7\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let path = vec!["facade".to_owned()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let selected = LoafIdentity {
            name: "compiled".into(),
            version: "1.0.0".into(),
            digest: "f".repeat(64),
            feature_projection: BTreeSet::from(["model".into()]),
        };
        let mut owner = CanonicalSymbolId::module_declaration(
            vec!["values".into()],
            "Packet",
            SemanticSourceTargetKind::Model,
            Default::default(),
        );
        owner.origin = SymbolOrigin::Package {
            library: "compiled".into(),
            module_path: vec!["values".into()],
        };
        let canonical = PublicLibraryTypeIdentity {
            dependency_key: "compiled".into(),
            source_path: vec!["values".into(), "Packet".into()],
            canonical: Some(owner.clone()),
            selected_provider: Some(selected.clone()),
        };
        checker
            .public_library_type_identities
            .insert("Packet".into(), canonical.clone());
        checker
            .public_library_type_identities
            .insert("FilteredAlias".into(), canonical);
        checker.public_library_type_identities.insert(
            "Legacy".into(),
            PublicLibraryTypeIdentity {
                dependency_key: "compiled".into(),
                source_path: vec!["values".into(), "Legacy".into()],
                canonical: None,
                selected_provider: Some(selected.clone()),
            },
        );
        checker
            .type_info
            .declarations
            .named_type_identities
            .insert("Packet".into(), owner);
        checker.cached_pub_libraries.insert("compiled".into());
        let proof = CheckedNamespaceInterface::from_checked("facade", &path, &program, &checker)?
            .ok_or("facade namespace was not captured")?;
        assert!(proof.compiled_types["Packet"].scope_binding);
        assert!(proof.compiled_types["Packet"].canonical.is_none());
        assert!(proof.compiled_types["FilteredAlias"].canonical.is_some());
        let restored = CheckedNamespaceInterface::from_wire_value(proof.wire_value()?)?;
        assert_eq!(restored.compiled_providers, [selected]);
        let installed = restored.installable()?;
        let sealed = checker.seal_current_module_dependency_interface(
            "facade",
            &program,
            Arc::from(crate::module::exported_symbols(&program)),
            true,
        );
        assert_eq!(installed.compiled_libraries, sealed.compiled_libraries);
        assert_eq!(
            installed.public_library_type_identities,
            sealed.public_library_type_identities
        );
        Ok(())
    }

    /// Reopening a namespace must preserve each explicit argument contract under the identity the caller resolves.
    #[test]
    fn reopened_namespace_retains_explicit_function_and_method_contracts() -> Result<(), String> {
        let source = "pub model Parcel:\n    pub value: int\n    pub def inspect(self, other: Borrowed[Parcel], proof: Owned[int] = 0) -> int:\n        return other.value\n\npub def inspect(other: Borrowed[Parcel], proof: Owned[int] = 0) -> int:\n    return other.value\n";
        let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|e| format!("{e:?}"))?)
            .map_err(|e| format!("{e:?}"))?;
        let path = vec!["parcels".to_owned()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(path.clone()));
        checker.check_program(&program).map_err(|e| format!("{e:?}"))?;
        let expected = checker
            .type_info()
            .declarations
            .ownership_contracts
            .iter()
            .filter(|(identity, _)| provider_input_contract(identity, &path))
            .map(|(id, contract)| (id.clone(), contract.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(expected.len(), 2);
        let selected = CheckedNamespaceInterface::from_checked("parcels", &path, &program, &checker)?
            .ok_or("provider was not captured")?;
        let reopened = wire::restore(selected.wire_value()?)?;
        reopened.validate()?;
        let installed = reopened.installable()?;
        assert_eq!(*installed.input_contracts, expected);
        Ok(())
    }

    /// A recovered facade's public signature keeps the provider it checked against, even if another facade imports
    /// a different model under the same name. A client need not import either model to pass the returned value.
    #[test]
    fn installed_facades_retain_distinct_parameter_and_result_providers() -> Result<(), String> {
        fn program(source: &str) -> Result<Program, String> {
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
        }

        let record = program("pub model Fact:\n    pub value: int\n")?;
        let mut carriers = Vec::new();
        for provider in ["left_records", "right_records"] {
            let facade = program(&format!(
                "from {provider} import Fact\n\npub def find() -> Option[Fact]:\n    return None\n\npub def read(fact: Fact) -> int:\n    return fact.value\n"
            ))?;
            let name = format!("{provider}_facade");
            let path = vec![name.clone()];
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(path.clone()));
            checker.import_module(&record, provider);
            checker.check_program(&facade).map_err(|errors| format!("{errors:?}"))?;
            let retained = CheckedNamespaceInterface::from_checked(&name, &path, &facade, &checker)?
                .ok_or("missing checked facade")?;
            let identity = retained
                .nominal_scope
                .get("Fact")
                .ok_or("missing checked Fact provider")?;
            let carrier = incan_semantics_core::body_ir::nominal_carrier_key(identity).ok_or("missing Fact carrier")?;
            let restored = retained.installable()?;
            let Some(SymbolKind::Function(read)) = restored.direct_member_symbols.get("read") else {
                return Err("missing read signature".into());
            };
            let Some(SymbolKind::Function(find)) = restored.direct_member_symbols.get("find") else {
                return Err("missing find signature".into());
            };
            let expected = ResolvedType::Named(carrier.clone());
            assert_eq!(read.params[0].ty, expected);
            assert!(
                matches!(&find.return_type, ResolvedType::Generic(_, args) if args == std::slice::from_ref(&expected))
            );
            carriers.push(carrier);
        }
        assert_ne!(carriers[0], carriers[1]);
        Ok(())
    }
}
