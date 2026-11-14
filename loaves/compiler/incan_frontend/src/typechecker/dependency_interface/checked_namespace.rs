//! Durable checked namespace projection for editable source dependencies.

use super::*;

/// Durable checked surface of one editable source namespace.
///
/// Frozen Loaves carry this same [`crate::api_metadata::CheckedApiMetadata`] projection inside their manifest. The
/// editable-namespace record adds only the source module key and canonical declaration identities needed to install
/// that projection through [`ModuleDependencyInterface`], without serializing transient [`TypeCheckInfo`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckedNamespaceInterface {
    schema: u32,
    module_name: String,
    module_path: Vec<String>,
    api: crate::api_metadata::CheckedApiMetadata,
    identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
    /// Member identities by declaration name. API metadata carries a member's identity only for a package origin, so
    /// an editable module's variants, fields, properties and methods keep theirs here (#3154).
    members: std::collections::BTreeMap<String, NamespaceMemberIdentities>,
}

/// The canonical identities of one declaration's members, by member name, as the cold checker recorded them.
///
/// A consumer checked against the installed namespace records the same member identities a cold check of both
/// modules records, so its Body IR selects an imported enum variant, field or method by identity in a warm build too.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NamespaceMemberIdentities {
    variants: std::collections::BTreeMap<String, CanonicalSymbolId>,
    fields: std::collections::BTreeMap<String, CanonicalSymbolId>,
    properties: std::collections::BTreeMap<String, CanonicalSymbolId>,
    methods: std::collections::BTreeMap<String, CanonicalSymbolId>,
}

impl CheckedNamespaceInterface {
    const SCHEMA: u32 = 5;

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
        Ok(Some(Self {
            schema: Self::SCHEMA,
            module_name: module_name.to_owned(),
            module_path: module_path.to_vec(),
            api,
            identities,
            members,
        }))
    }

    /// Encode one checked namespace using the manifest's existing JSON-ready semantic model.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| format!("cannot encode checked namespace interface: {error}"))
    }

    /// Decode and validate one complete checked namespace before it reaches checker installation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let interface: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("cannot decode checked namespace interface: {error}"))?;
        interface.validate()?;
        Ok(interface)
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
        let remapping = checked_namespace_foreign_nominal_remapping(&self.api);
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
            converter.remap_symbol_kind_with_import_aliases(&mut kind, &remapping);
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
        let nominal_scope = Arc::new(
            identities
                .iter()
                .filter(|(_, identity)| {
                    matches!(
                        identity.kind,
                        SemanticSourceTargetKind::Model
                            | SemanticSourceTargetKind::Class
                            | SemanticSourceTargetKind::Enum
                            | SemanticSourceTargetKind::Newtype
                    )
                })
                .map(|(name, identity)| (name.clone(), identity.clone()))
                .collect(),
        );
        for (name, kind) in &symbols {
            if let Some(identity) = identities.get(name) {
                converter.retain_declaration_nominal_scope(identity, kind, &nominal_scope);
            }
        }
        converter.retain_nominal_scope_carriers(&nominal_scope);
        let (transitive_public_types, transitive_public_traits) = provider_transitive_semantics(&symbols, &identities);
        let symbols = Arc::new(symbols);
        let identities = Arc::new(identities);
        let mut interface = ModuleDependencyInterface {
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
            declaration_nominal_scopes: Arc::new(converter.declaration_nominal_scopes.clone()),
            retained_nominal_carriers: Arc::new(converter.retained_nominal_carriers.clone()),
            declaration_callables: Arc::new(converter.declaration_callables.clone()),
            input_contracts: Arc::default(),
            member_reexports: Arc::new(reexports),
            body_ir_pub_reexports: Arc::default(),
            direct_member_partial_projections: Arc::default(),
            derivable_modules: Arc::default(),
            derivable_trait_identities: Arc::default(),
            module_traits: Arc::default(),
            trait_rust_derive_paths: Arc::default(),
            transitive_public_types: Arc::new(transitive_public_types),
            transitive_public_traits: Arc::new(transitive_public_traits),
            compiled_libraries: Arc::from([]),
            public_library_type_identities: Arc::default(),
            symbol_rows: Arc::from(symbol_rows),
            snapshot: ModuleDependencyInterfaceSnapshot {
                version: MODULE_DEPENDENCY_INTERFACE_SNAPSHOT_VERSION,
                tables: Vec::new(),
            },
        };
        let mut installed = TypeChecker::new();
        installed.install_dependency_interfaces_directly(&[Arc::new(interface.clone())]);
        interface.snapshot = installed.dependency_interface_snapshot_for(
            &interface.module_name,
            Some(interface.symbol_rows.as_ref()),
            interface.public_library_type_identities.as_ref(),
        );
        Ok(Arc::new(interface))
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
            if !matches!(declaration, crate::api_metadata::ApiDeclaration::Alias(_))
                && (identity.declaration_name != name
                    || identity.origin != SymbolOrigin::Module(self.module_path.clone()))
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
        let retained = CheckedNamespaceInterface::from_checked("domain", &module_path, &program, &checker)?
            .ok_or("model namespace was not retained")?;
        let installed = retained.installable()?;
        let order = retained.identities.get("Order").ok_or("missing Order identity")?;
        let scope = installed
            .declaration_nominal_scopes
            .get(order)
            .ok_or("missing Order declaration scope")?;
        assert_eq!(scope.get("Order"), Some(order));
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
}
