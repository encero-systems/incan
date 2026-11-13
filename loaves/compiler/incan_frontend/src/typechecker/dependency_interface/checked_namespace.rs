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
}

impl CheckedNamespaceInterface {
    const SCHEMA: u32 = 4;

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
        }
        Ok(Some(Self {
            schema: Self::SCHEMA,
            module_name: module_name.to_owned(),
            module_path: module_path.to_vec(),
            api,
            identities,
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
        let converter = TypeChecker::new();
        let mut symbols = HashMap::new();
        let mut callables = HashMap::new();
        let mut symbol_rows = Vec::new();
        for declaration in &self.api.declarations {
            let name = namespace_declaration_name(declaration)
                .ok_or("checked namespace interface contains an unsupported declaration")?;
            let identity = self
                .identities
                .get(name)
                .ok_or_else(|| format!("checked namespace declaration `{name}` has no identity"))?
                .clone();
            let kind = converter
                .symbol_kind_from_api_declaration(None, declaration)
                .ok_or_else(|| format!("checked namespace declaration `{name}` has no semantic symbol"))?;
            if let SymbolKind::Function(function_info) = &kind {
                callables.insert(
                    identity.clone(),
                    DeclarationCallableTypes {
                        params: function_info.params.clone(),
                        return_type: function_info.return_type.clone(),
                    },
                );
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
        let symbols = Arc::new(symbols);
        let identities = Arc::new(
            self.identities
                .iter()
                .map(|(name, identity)| (name.clone(), identity.clone()))
                .collect(),
        );
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
            declaration_nominal_scopes: Arc::default(),
            retained_nominal_carriers: Arc::default(),
            declaration_callables: Arc::new(callables),
            member_reexports: Arc::default(),
            body_ir_pub_reexports: Arc::default(),
            direct_member_partial_projections: Arc::default(),
            derivable_modules: Arc::default(),
            derivable_trait_identities: Arc::default(),
            module_traits: Arc::default(),
            trait_rust_derive_paths: Arc::default(),
            transitive_public_types: Arc::default(),
            transitive_public_traits: Arc::default(),
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
