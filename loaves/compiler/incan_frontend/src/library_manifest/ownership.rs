//! Lossless ownership metadata and the refusal boundary for contracts this compiler cannot execute.

use super::{CanonicalIdentityExport, LibraryManifestError, TypeRef};
use crate::ownership_metadata::{CheckedOwnershipContract, OwnershipContract};
use incan_lang::lang::types::ownership::OwnershipMode;
use serde::{Deserialize, Serialize};

/// Explicit provider contracts are versioned independently from the container manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "contract", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnershipExport {
    /// A checked source contract projected through canonical package identities.
    Checked(OwnershipContract<TypeRef, CanonicalIdentityExport>),
    /// The projection could not represent a required identity; publication must refuse this sentinel.
    Unsupported,
}

/// Project source contracts without dropping a field identity the artifact cannot carry.
pub(crate) fn project(package: &str, contract: &CheckedOwnershipContract) -> OwnershipExport {
    match contract.try_map(
        |ty| {
            if representable_type(ty) {
                Ok(super::type_ref_from_resolved(ty))
            } else {
                Err(())
            }
        },
        |identity| CanonicalIdentityExport::from_canonical(package, identity).ok_or(()),
    ) {
        Ok(contract) => OwnershipExport::Checked(contract),
        Err(()) => OwnershipExport::Unsupported,
    }
}

/// Legacy manifests erase internal mutable references and unresolved types; an explicit contract must refuse them.
fn representable_type(ty: &crate::symbols::ResolvedType) -> bool {
    use crate::symbols::ResolvedType;
    match ty {
        ResolvedType::RefMut(_) | ResolvedType::Never | ResolvedType::CallSiteInfer | ResolvedType::Unknown => false,
        ResolvedType::Generic(_, children) | ResolvedType::Tuple(children) => children.iter().all(representable_type),
        ResolvedType::Function(parameters, result) => {
            parameters.iter().all(|parameter| representable_type(&parameter.ty)) && representable_type(result)
        }
        ResolvedType::FrozenDict(key, value) => representable_type(key) && representable_type(value),
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner)
        | ResolvedType::Ref(inner) => representable_type(inner),
        _ => true,
    }
}

/// Rehydrate only explicit checked facts; this does not authorize invoking the imported callable.
pub(super) fn hydrate(contract: &OwnershipExport) -> Result<CheckedOwnershipContract, LibraryManifestError> {
    let OwnershipExport::Checked(contract) = contract else {
        return Err(invalid("unrepresentable ownership projection"));
    };
    contract.try_map(
        |ty| Ok(super::resolved_type_from_manifest_type_ref(ty)),
        |identity| {
            identity
                .hydrate()
                .ok_or_else(|| invalid("ownership source field has no canonical declaration"))
        },
    )
}

/// Validate the contract and its explicit type markers against one exact signature.
pub(super) fn validate(
    contract: Option<&OwnershipExport>,
    params: &[super::ParamExport],
    result: &TypeRef,
    receiver: bool,
) -> Result<(), LibraryManifestError> {
    let mut marked = Vec::new();
    for (slot, parameter) in params.iter().enumerate() {
        markers(&parameter.ty, Some(slot), &mut Vec::new(), &mut marked);
    }
    markers(result, None, &mut Vec::new(), &mut marked);
    let Some(contract) = contract else {
        return if marked.is_empty() {
            Ok(())
        } else {
            Err(invalid("explicit ownership type is missing its contract metadata"))
        };
    };
    let OwnershipExport::Checked(contract) = contract else {
        return Err(invalid("unrepresentable ownership projection"));
    };
    contract.validate(params.len(), receiver).map_err(invalid)?;
    let expected: Vec<_> = contract
        .modes
        .iter()
        .map(|access| {
            (
                access.component.parameter,
                access.component.children.clone(),
                access.mode,
            )
        })
        .collect();
    if marked != expected {
        return Err(invalid("ownership type markers differ from checked access metadata"));
    }
    for access in &contract.modes {
        let mut root = match access.component.parameter {
            Some(slot) => params
                .get(slot)
                .ok_or_else(|| invalid("ownership parameter slot is absent"))?
                .ty
                .clone(),
            None => result.clone(),
        };
        let Some(TypeRef::Ownership { inner, .. }) = component(&mut root, &access.component.children) else {
            return Err(invalid("ownership component is not an explicit mode"));
        };
        if super::resolved_type_from_manifest_type_ref(inner)
            != super::resolved_type_from_manifest_type_ref(&access.underlying)
        {
            return Err(invalid("ownership underlying type differs from its checked signature"));
        }
    }
    if let Some(source) = &contract.result_source {
        for field in &source.fields {
            if field.kind != "field" || field.hydrate().is_none() {
                return Err(invalid("ownership source projection is not a canonical stored field"));
            }
        }
    }
    Ok(())
}

/// Add canonical markers only at components the checker explicitly identified; user lookalikes remain ordinary types.
pub(crate) fn mark_types(contract: Option<&OwnershipExport>, params: &mut [super::ParamExport], result: &mut TypeRef) {
    let Some(OwnershipExport::Checked(contract)) = contract else {
        return;
    };
    // Inner paths are rewritten first so outer replacement keeps every nested marker.
    for access in contract.modes.iter().rev() {
        let root = match access.component.parameter {
            Some(slot) => params.get_mut(slot).map(|parameter| &mut parameter.ty),
            None => Some(&mut *result),
        };
        let Some(target) = root.and_then(|root| component(root, &access.component.children)) else {
            continue;
        };
        if let TypeRef::Applied { args, .. } = target
            && args.len() == 1
        {
            let inner = args.remove(0);
            *target = TypeRef::Ownership {
                mode: access.mode,
                inner: Box::new(inner),
            };
        }
    }
}

/// Select a structural checked type component; no names are resolved at the artifact boundary.
fn component<'a>(ty: &'a mut TypeRef, path: &[usize]) -> Option<&'a mut TypeRef> {
    // Callable mutation is parameter metadata, not an extra semantic type child.
    let ty = match ty {
        TypeRef::MutParam { inner } => inner.as_mut(),
        other => other,
    };
    let Some((&index, rest)) = path.split_first() else {
        return Some(ty);
    };
    let child = match ty {
        TypeRef::Applied { args, .. } => args.get_mut(index),
        TypeRef::Tuple { elements } => elements.get_mut(index),
        TypeRef::Function { params, return_type } => {
            if index == params.len() {
                Some(return_type.as_mut())
            } else {
                params.get_mut(index)
            }
        }
        TypeRef::Ownership { inner, .. }
        | TypeRef::Ref { inner }
        | TypeRef::TypeToken { inner }
        | TypeRef::MutParam { inner }
            if index == 0 =>
        {
            Some(inner.as_mut())
        }
        _ => None,
    }?;
    component(child, rest)
}

/// Inventory explicit markers in exactly the checker's traversal order.
fn markers(
    ty: &TypeRef,
    parameter: Option<usize>,
    path: &mut Vec<usize>,
    output: &mut Vec<(Option<usize>, Vec<usize>, OwnershipMode)>,
) {
    if let TypeRef::MutParam { inner } = ty {
        markers(inner, parameter, path, output);
        return;
    }
    if let TypeRef::Ownership { mode, .. } = ty {
        output.push((parameter, path.clone(), *mode));
    }
    let children: Vec<&TypeRef> = match ty {
        TypeRef::Applied { args, .. } => args.iter().collect(),
        TypeRef::Tuple { elements } => elements.iter().collect(),
        TypeRef::Function { params, return_type } => {
            params.iter().chain(std::iter::once(return_type.as_ref())).collect()
        }
        TypeRef::Ownership { inner, .. }
        | TypeRef::Ref { inner }
        | TypeRef::TypeToken { inner }
        | TypeRef::MutParam { inner } => vec![inner],
        _ => Vec::new(),
    };
    for (index, child) in children.into_iter().enumerate() {
        path.push(index);
        markers(child, parameter, path, output);
        path.pop();
    }
}

/// Return a manifest-boundary refusal instead of silently defaulting an incomplete contract.
fn invalid(message: &str) -> LibraryManifestError {
    LibraryManifestError::Invalid(message.to_owned())
}

/// Every public namespace uses the same ownership gate, including checked API declarations absent from root exports.
pub(super) fn has_contracts(
    exports: &super::LibraryExports,
    api: Option<&crate::api_metadata::CheckedApiMetadataPackage>,
) -> bool {
    let mut functions = exports.functions.iter().chain(
        exports
            .aliases
            .iter()
            .filter_map(|alias| alias.projected_function.as_ref()),
    );
    if functions.any(|function| function.ownership.is_some()) {
        return true;
    }
    if exports
        .models
        .iter()
        .flat_map(|owner| &owner.methods)
        .chain(exports.classes.iter().flat_map(|owner| &owner.methods))
        .chain(exports.traits.iter().flat_map(|owner| &owner.methods))
        .chain(exports.enums.iter().flat_map(|owner| &owner.methods))
        .chain(exports.newtypes.iter().flat_map(|owner| &owner.methods))
        .any(|method| method.ownership.is_some())
    {
        return true;
    }
    api.into_iter()
        .flat_map(|api| &api.modules)
        .flat_map(|module| &module.declarations)
        .any(|declaration| {
            api_signatures(declaration)
                .iter()
                .any(|signature| signature.0.is_some())
        })
}

/// One API callable's contract, source parameters, result and receiver presence.
type ApiSignature<'a> = (Option<&'a OwnershipExport>, &'a [super::ParamExport], &'a TypeRef, bool);

/// Shared callable inventory for API validation and executable admission; aliases preserve their checked projection.
fn api_signatures(declaration: &crate::api_metadata::ApiDeclaration) -> Vec<ApiSignature<'_>> {
    use crate::api_metadata::ApiDeclaration;
    let methods = match declaration {
        ApiDeclaration::Function(function) => {
            return vec![(
                function.ownership.as_ref(),
                &function.params,
                &function.return_type,
                false,
            )];
        }
        ApiDeclaration::Alias(alias) => {
            return alias
                .projected_function
                .iter()
                .map(|function| {
                    let callable = &function.callable;
                    (
                        callable.ownership.as_ref(),
                        callable.params.as_slice(),
                        &callable.return_type,
                        callable.receiver.is_some(),
                    )
                })
                .collect();
        }
        ApiDeclaration::Partial(partial) => return vec![(None, &partial.params, &partial.return_type, false)],
        ApiDeclaration::Model(owner) => &owner.methods,
        ApiDeclaration::Class(owner) => &owner.methods,
        ApiDeclaration::Trait(owner) => &owner.methods,
        ApiDeclaration::Enum(owner) => &owner.methods,
        ApiDeclaration::Newtype(owner) => &owner.methods,
        _ => return Vec::new(),
    };
    methods
        .iter()
        .map(|method| {
            (
                method.ownership.as_ref(),
                method.params.as_slice(),
                &method.return_type,
                method.receiver.is_some(),
            )
        })
        .collect()
}

/// API modules are not exempt from contract completeness just because their declaration is not a root export.
pub(super) fn validate_api(
    api: Option<&crate::api_metadata::CheckedApiMetadataPackage>,
    format: u32,
) -> Result<(), LibraryManifestError> {
    for declaration in api
        .into_iter()
        .flat_map(|api| &api.modules)
        .flat_map(|module| &module.declarations)
    {
        for (contract, params, result, receiver) in api_signatures(declaration) {
            validate_format(format, contract)?;
            validate(contract, params, result, receiver)?;
        }
    }
    Ok(())
}

/// Old readers ignore unknown JSON properties, so source-only dependencies need an explicit container capability.
pub(super) fn validate_format(format: u32, contract: Option<&OwnershipExport>) -> Result<(), LibraryManifestError> {
    if contract.is_some() && format != super::OWNERSHIP_MANIFEST_FORMAT {
        return Err(invalid("explicit ownership requires manifest format 7"));
    }
    Ok(())
}

impl super::LibraryManifest {
    /// A parsed provider may retain explicit contract metadata for tooling while executable admission stays refused.
    pub fn has_explicit_ownership_contracts(&self) -> bool {
        has_contracts(&self.exports, self.contract_metadata.api.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_exports::{CheckedExportKind, collect_checked_public_exports};
    use crate::library_manifest::{CheckedExportReplay, LibraryManifest};
    use crate::{lexer, parser, typechecker::TypeChecker};

    /// Real checked source feeds provider metadata; the provider's returned view executes in its own package.
    fn source_export() -> Result<crate::library_exports::CheckedNamedExport, Box<dyn std::error::Error>> {
        let source = "pub def view(value: Borrowed[str]) -> Borrowed[str] from value:\n    return value\n";
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_package_identity(Some("provider".to_owned()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        collect_checked_public_exports(&program, &checker)
            .into_iter()
            .next()
            .ok_or_else(|| "missing checked export".into())
    }

    /// Publication, replay and facade renaming retain source slots rather than reconstructing them from parameter
    /// names.
    #[test]
    fn provider_replay_preserves_ownership_and_renamed_parameters() -> Result<(), Box<dyn std::error::Error>> {
        let export = source_export()?;
        let replay = CheckedExportReplay::from_checked("provider", "1.0.0", &export)?;
        let serialized = serde_json::to_string(&replay)?;
        let replay: CheckedExportReplay = serde_json::from_str(&serialized)?;
        let mut recovered = replay.to_checked()?.renamed("forwarded_view");
        let target = vec!["provider".to_owned(), "view".to_owned()];
        recovered.identity = crate::library_exports::CheckedExportIdentity::reexport(target.clone(), target)
            .with_canonical(recovered.identity.canonical.clone());
        let CheckedExportKind::Function(function) = &mut recovered.kind else {
            return Err("wrong export kind".into());
        };
        function.params[0].name = Some("renamed_input".to_owned());
        let contract = function.ownership.as_ref().ok_or("lost contract")?;
        assert_eq!(
            contract.result_source.as_ref().ok_or("lost source")?.slot,
            crate::ownership_metadata::SourceSlot::Parameter(0)
        );
        let manifest = LibraryManifest::from_checked_exports("facade", "1.0.0", &[recovered]);
        let restored = LibraryManifest::from_json_str(&manifest.to_json_string()?)?;
        assert!(restored.has_explicit_ownership_contracts());
        assert_eq!(restored.exports.functions[0].params[0].name, "renamed_input");
        Ok(())
    }

    /// Same-name source overloads retain their own parameter and result-source contracts in API metadata.
    #[test]
    fn source_overloads_keep_their_canonical_ownership_contracts() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub def view(value: Borrowed[str]) -> Borrowed[str] from value:\n    return value\npub def view(prefix: str, value: Borrowed[str]) -> Borrowed[str] from value:\n    return value\n";
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_package_identity(Some("provider".into()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let api = crate::api_metadata::collect_checked_api_metadata(&program, &checker, Vec::new());
        assert_eq!(api.declarations.len(), 2);
        for (index, declaration) in api.declarations.iter().enumerate() {
            let crate::api_metadata::ApiDeclaration::Function(function) = declaration else {
                return Err("expected source function metadata".into());
            };
            let Some(OwnershipExport::Checked(contract)) = &function.ownership else {
                return Err("missing checked ownership".into());
            };
            assert_eq!(function.params.len(), index + 1);
            assert_eq!(
                contract.result_source.as_ref().ok_or("missing source")?.slot,
                crate::ownership_metadata::SourceSlot::Parameter(index)
            );
            assert_eq!(contract.modes[0].component.parameter, Some(index));
        }
        let exports = collect_checked_public_exports(&program, &checker);
        let mut manifest = LibraryManifest::from_checked_exports("provider", "1.0.0", &exports);
        manifest.contract_metadata.api = Some(crate::api_metadata::CheckedApiMetadataPackage {
            schema_version: crate::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![api],
            public_namespaces: Vec::new(),
        });
        LibraryManifest::from_json_str(&manifest.to_json_string()?)?;
        Ok(())
    }

    /// An older reader must refuse even a source-only contract with no explicit wrapper in its result type.
    #[test]
    fn source_only_contract_requires_new_provider_format() -> Result<(), Box<dyn std::error::Error>> {
        let mut export = source_export()?;
        let CheckedExportKind::Function(function) = &mut export.kind else {
            return Err("wrong export kind".into());
        };
        function.params[0].ty = crate::symbols::ResolvedType::Str;
        function.return_type = crate::symbols::ResolvedType::Str;
        function.ownership.as_mut().ok_or("missing contract")?.modes.clear();
        let manifest = LibraryManifest::from_checked_exports("provider", "1.0.0", &[export]);
        let text = manifest.to_json_string()?;
        assert!(text.contains("\"manifest_format\": 7"));
        assert!(
            LibraryManifest::from_json_str(&text.replace("\"manifest_format\": 7", "\"manifest_format\": 6")).is_err()
        );
        Ok(())
    }

    /// A checked submodule cannot bypass provider execution refusal by omitting its declaration from root exports.
    #[test]
    fn api_submodule_contracts_refuse_consumer_execution() -> Result<(), Box<dyn std::error::Error>> {
        use crate::api_metadata::{
            CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
            materialize_checked_api_public_namespaces,
        };
        let source = "pub def view(value: Borrowed[str]) -> Borrowed[str] from value:\n    return value\n";
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_package_identity(Some("provider".into()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let mut api = CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![collect_checked_api_metadata(&program, &checker, vec!["views".into()])],
            public_namespaces: Vec::new(),
        };
        materialize_checked_api_public_namespaces(&mut api)?;
        let mut manifest = LibraryManifest::new("provider", "1.0.0");
        manifest.contract_metadata.api = Some(api);
        assert!(manifest.has_explicit_ownership_contracts());
        let directory = tempfile::tempdir()?;
        std::fs::write(directory.path().join("provider.incnlib"), manifest.to_json_string()?)?;
        let entry = crate::loaf_index::LoafIndex::dependency("provider", "provider", directory.path())?;
        let mut consumer = TypeChecker::new();
        consumer.set_loaves(std::sync::Arc::new(crate::loaf_index::LoafIndex::new([entry])?));
        let tokens = lexer::lex("from pub::provider.views import view\n").map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let errors = consumer
            .check_program(&program)
            .err()
            .ok_or("ownership provider admitted")?;
        assert!(
            errors.iter().any(|error| error.stable_code() == Some("INCAN-T0123")),
            "{errors:?}"
        );
        Ok(())
    }

    /// A callable's mut marker does not shift its explicit ownership component path.
    #[test]
    fn nested_mutable_callable_modes_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub def accept(action: (mut Borrowed[str]) -> str) -> None:\n    pass\n";
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_package_identity(Some("provider".into()));
        assert!(checker.check_program(&program).is_err());
        let exports = collect_checked_public_exports(&program, &checker);
        let manifest = LibraryManifest::from_checked_exports("provider", "1.0.0", &exports);
        let restored = LibraryManifest::from_json_str(&manifest.to_json_string()?)?;
        assert!(restored.has_explicit_ownership_contracts());
        Ok(())
    }

    /// Explicit contract publication must not inherit legacy shared-reference or unknown-type erasure.
    #[test]
    fn unrepresentable_inner_contract_is_refused() {
        use crate::ownership_metadata::{AccessMode, TypeComponent};
        use crate::symbols::ResolvedType;
        for underlying in [
            ResolvedType::RefMut(Box::new(ResolvedType::Str)),
            ResolvedType::Generic("list".into(), vec![ResolvedType::Unknown]),
        ] {
            let contract = CheckedOwnershipContract {
                schema_version: 1,
                modes: vec![AccessMode {
                    mode: OwnershipMode::Owned,
                    component: TypeComponent {
                        parameter: Some(0),
                        children: Vec::new(),
                    },
                    underlying,
                }],
                result_source: None,
            };
            assert!(matches!(project("provider", &contract), OwnershipExport::Unsupported));
        }
    }

    /// Dedicated frozen-container and type-token nodes retain modes just as generic and callable nodes do.
    #[test]
    fn compound_ownership_modes_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub def inspect(a: FrozenList[Borrowed[str]], b: FrozenDict[str, Owned[int]], c: FrozenSet[BorrowedMut[int]], d: Type[Owned[int]]) -> None:\n    pass\n";
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_package_identity(Some("provider".into()));
        assert!(checker.check_program(&program).is_err());
        let exports = collect_checked_public_exports(&program, &checker);
        let manifest = LibraryManifest::from_checked_exports("provider", "1.0.0", &exports);
        let restored = LibraryManifest::from_json_str(&manifest.to_json_string()?)?;
        let contract = restored
            .exports
            .functions
            .first()
            .and_then(|function| function.ownership.as_ref())
            .ok_or("missing contract")?;
        let OwnershipExport::Checked(contract) = contract else {
            return Err("unsupported compound contract".into());
        };
        assert_eq!(contract.modes.len(), 4);
        Ok(())
    }

    /// Missing and unsupported contract schemas cannot turn marked types into ordinary generic parameters.
    #[test]
    fn incomplete_ownership_metadata_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let export = source_export()?;
        let mut manifest = LibraryManifest::from_checked_exports("provider", "1.0.0", &[export]);
        let contract = manifest.exports.functions[0]
            .ownership
            .take()
            .ok_or("missing published contract")?;
        assert!(manifest.to_json_string().is_err());
        manifest.exports.functions[0].ownership = Some(contract);
        let Some(OwnershipExport::Checked(contract)) = &mut manifest.exports.functions[0].ownership else {
            return Err("unsupported projection".into());
        };
        contract.modes[0].underlying = TypeRef::Named {
            name: "int".into(),
            origin: None,
        };
        assert!(manifest.to_json_string().is_err());
        let Some(OwnershipExport::Checked(contract)) = &mut manifest.exports.functions[0].ownership else {
            return Err("unsupported projection".into());
        };
        contract.schema_version = 99;
        assert!(manifest.to_json_string().is_err());
        Ok(())
    }
}
