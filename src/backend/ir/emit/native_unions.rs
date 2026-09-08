//! Publication of native union representations retained from the final emitted wrapper table.

use std::collections::{BTreeMap, HashMap};

use super::super::decl::{IrDeclKind, IrFunction, VariantFields};
use super::{EmitError, IrEmitter, IrProgram, IrType};
use crate::frontend::api_metadata::{ApiDeclaration, CheckedApiMetadata, SourceAnchor};
use crate::library_manifest::{
    LibraryManifest, NativeUnionExport, NativeUnionOwnerExport, NominalTypeOriginExport, TypeRef, VisitTypeRefs,
};

/// Type projections for one declaration, scoped by its checked source module and declaration anchor.
#[derive(Debug, Clone)]
pub(in crate::backend::ir) struct EmittedDeclarationTypes {
    module_path: Vec<String>,
    anchor: SourceAnchor,
    source_name: String,
    replacements: Vec<(TypeRef, TypeRef)>,
}

impl EmittedDeclarationTypes {
    /// Apply a projection only to the checked declaration and public exports of that exact source declaration.
    pub(in crate::backend::ir) fn apply(&self, manifest: &mut LibraryManifest) {
        if let Some(api) = &mut manifest.contract_metadata.api {
            for module in &mut api.modules {
                if module.module_path != self.module_path {
                    continue;
                }
                for declaration in &mut module.declarations {
                    if declaration_anchor(declaration) == &self.anchor {
                        self.rewrite(declaration);
                    }
                }
            }
        }
        let mut source_path = self.module_path.clone();
        source_path.push(self.source_name.clone());
        let names = manifest
            .contract_metadata
            .identity_graph
            .exports
            .iter()
            .filter(|entry| entry.source_path == source_path)
            .map(|entry| entry.public_name.clone())
            .collect::<Vec<_>>();
        macro_rules! rewrite_exports {
            ($($field:ident),* $(,)?) => { $(
                for export in &mut manifest.exports.$field {
                    if names.contains(&export.name) { self.rewrite(export); }
                }
            )* };
        }
        rewrite_exports!(
            functions,
            models,
            classes,
            traits,
            enums,
            type_aliases,
            newtypes,
            consts,
            statics,
            aliases,
            partials
        );
    }

    /// Replace typed occurrences within a single source declaration without crossing module or artifact boundaries.
    fn rewrite(&self, value: &mut impl VisitTypeRefs) {
        value.visit_type_refs(&mut |ty| {
            if let Some((_, replacement)) = self.replacements.iter().find(|(original, _)| original == ty) {
                *ty = replacement.clone();
            }
        });
    }
}

/// Return the source anchor shared by every checked declaration shape.
fn declaration_anchor(declaration: &ApiDeclaration) -> &SourceAnchor {
    match declaration {
        ApiDeclaration::Function(item) => &item.anchor,
        ApiDeclaration::Model(item) => &item.anchor,
        ApiDeclaration::Class(item) => &item.anchor,
        ApiDeclaration::Trait(item) => &item.anchor,
        ApiDeclaration::Enum(item) => &item.anchor,
        ApiDeclaration::Newtype(item) => &item.anchor,
        ApiDeclaration::TypeAlias(item) => &item.anchor,
        ApiDeclaration::Const(item) => &item.anchor,
        ApiDeclaration::Static(item) => &item.anchor,
        ApiDeclaration::Alias(item) => &item.anchor,
        ApiDeclaration::Partial(item) => &item.anchor,
    }
}

impl IrEmitter<'_> {
    /// Return the actual definitions emitted by this emitter after all alias resolution and filtering.
    pub(in crate::backend::ir) fn emitted_native_union_types(&self) -> HashMap<String, IrType> {
        self.emitted_native_unions.borrow().clone()
    }

    /// Capture public typed slots from the same module's lowered declarations and final emitted definitions.
    ///
    /// Module identity is supplied by the code-generation pass, not inferred from numeric source spans. Symbol aliases
    /// have no local typed definition and therefore never acquire a local wrapper through their target's span.
    pub(in crate::backend::ir) fn capture_native_union_metadata(
        &self,
        module: &CheckedApiMetadata,
        program: &IrProgram,
        definitions: &HashMap<String, IrType>,
        origins: &BTreeMap<String, NominalTypeOriginExport>,
    ) -> Result<(Vec<EmittedDeclarationTypes>, Vec<NativeUnionExport>), EmitError> {
        let mut captured = Vec::new();
        let mut native_definitions = Vec::new();
        for declaration in &module.declarations {
            let anchor = declaration_anchor(declaration);
            let Some(ir) = program.declarations.iter().find(|ir| {
                ir.span.start == anchor.span.start
                    && ir.span.end == anchor.span.end
                    && !matches!(ir.kind, IrDeclKind::SymbolAlias { .. } | IrDeclKind::Import { .. })
            }) else {
                continue;
            };
            let mut pairs = Vec::new();
            match (declaration, &ir.kind) {
                (ApiDeclaration::Function(api), IrDeclKind::Function(function)) => {
                    pair_function(&api.params, &api.return_type, function, &mut pairs)?;
                }
                (ApiDeclaration::TypeAlias(api), IrDeclKind::TypeAlias { ty, .. }) => {
                    pairs.push((&api.type_alias.target, ty))
                }
                (ApiDeclaration::Const(api), IrDeclKind::Const { ty, .. }) => pairs.push((&api.ty, ty)),
                (ApiDeclaration::Static(api), IrDeclKind::Static { ty, .. }) => pairs.push((&api.ty, ty)),
                (ApiDeclaration::Model(api), IrDeclKind::Struct(structure)) => {
                    pair_fields(&api.fields, &structure.fields, &mut pairs);
                }
                (ApiDeclaration::Class(api), IrDeclKind::Struct(structure)) => {
                    pair_fields(&api.fields, &structure.fields, &mut pairs);
                }
                (ApiDeclaration::Newtype(api), IrDeclKind::Struct(structure)) => {
                    if let Some(field) = structure.fields.first() {
                        pairs.push((&api.underlying, &field.ty));
                    }
                }
                (ApiDeclaration::Newtype(api), IrDeclKind::TypeAlias { ty, .. }) => pairs.push((&api.underlying, ty)),
                (ApiDeclaration::Enum(api), IrDeclKind::Enum(enumeration)) => {
                    for variant in &api.variants {
                        let Some(lowered) = enumeration.variants.iter().find(|item| item.name == variant.name) else {
                            continue;
                        };
                        match &lowered.fields {
                            VariantFields::Tuple(types) => {
                                require_arity(variant.fields.len(), types.len())?;
                                pairs.extend(variant.fields.iter().zip(types));
                            }
                            VariantFields::Struct(fields) => {
                                require_arity(variant.fields.len(), fields.len())?;
                                pairs.extend(variant.fields.iter().zip(fields.iter().map(|field| &field.ty)));
                            }
                            VariantFields::Unit => {}
                        }
                    }
                }
                (ApiDeclaration::Trait(api), IrDeclKind::Trait(trait_decl)) => {
                    for method in &api.methods {
                        if let Some(function) = trait_decl.methods.iter().find(|function| function.name == method.name)
                        {
                            pair_function(&method.params, &method.return_type, function, &mut pairs)?;
                        }
                    }
                }
                _ => {}
            }
            let (owner, methods, properties) = match declaration {
                ApiDeclaration::Model(api) => (api.name.as_str(), api.methods.as_slice(), api.properties.as_slice()),
                ApiDeclaration::Class(api) => (api.name.as_str(), api.methods.as_slice(), api.properties.as_slice()),
                ApiDeclaration::Enum(api) => (api.name.as_str(), api.methods.as_slice(), &[][..]),
                ApiDeclaration::Newtype(api) => (api.name.as_str(), api.methods.as_slice(), &[][..]),
                _ => ("", &[][..], &[][..]),
            };
            for method in methods {
                if let Some(identity) = method.canonical.as_ref().and_then(|identity| identity.hydrate()) {
                    if let Some(function) = projected_method(program, owner, &identity) {
                        pair_function(&method.params, &method.return_type, function, &mut pairs)?;
                    }
                }
            }
            for property in properties {
                if let Some(identity) = property.canonical.as_ref().and_then(|identity| identity.hydrate()) {
                    if let Some(function) = projected_method(program, owner, &identity) {
                        pairs.push((&property.return_type, &function.return_type));
                    }
                }
            }
            let mut replacements = Vec::new();
            for (original, lowered) in pairs {
                let projected = self.project_emitted_union_type(original, lowered, definitions, origins)?;
                if projected != *original {
                    projected.clone().visit_type_refs(&mut |ty| {
                        if let TypeRef::NativeUnion(native) = ty {
                            if native.owner == NativeUnionOwnerExport::ContainingArtifact
                                && !native_definitions.contains(native)
                            {
                                native_definitions.push(native.clone());
                            }
                        }
                    });
                    replacements.push((original.clone(), projected));
                }
            }
            if !replacements.is_empty() {
                captured.push(EmittedDeclarationTypes {
                    module_path: module.module_path.clone(),
                    anchor: anchor.clone(),
                    source_name: crate::frontend::api_metadata::api_declaration_public_name(declaration)
                        .ok_or_else(|| {
                            EmitError::InternalInvariant("typed declaration has no source name".to_string())
                        })?
                        .to_string(),
                    replacements,
                });
            }
        }
        Ok((captured, native_definitions))
    }

    /// Describe the exact stored members of a definition that this emitter actually emitted.
    fn native_union_definition(
        &self,
        name: &str,
        ty: &IrType,
        origins: &BTreeMap<String, NominalTypeOriginExport>,
    ) -> Result<NativeUnionExport, EmitError> {
        let members = ty
            .union_members()
            .ok_or_else(|| EmitError::InternalInvariant("emitted union has no members".to_string()))?;
        let members = members
            .iter()
            .map(|member| {
                super::super::codegen::manifest_type_ref_from_ir(member)
                    .map(|ty| crate::library_manifest::with_checked_type_origins(ty, origins))
                    .map_err(EmitError::InternalInvariant)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(NativeUnionExport {
            owner: NativeUnionOwnerExport::ContainingArtifact,
            rust_name: name.to_string(),
            members,
            checked_projection: None,
        })
    }

    /// Preserve a public type's semantic spelling while attaching its actual emitted native union representation.
    fn project_emitted_union_type(
        &self,
        original: &TypeRef,
        lowered: &IrType,
        definitions: &HashMap<String, IrType>,
        origins: &BTreeMap<String, NominalTypeOriginExport>,
    ) -> Result<TypeRef, EmitError> {
        let lowered = self.resolve_type_aliases_for_emit(lowered);
        if let IrType::ExternalUnion {
            native: Some(native), ..
        } = &lowered
        {
            return Ok(TypeRef::NativeUnion(native.for_publication()));
        }
        if lowered.is_union() && !matches!(lowered, IrType::ExternalUnion { .. }) {
            let Some((name, emitted)) = definitions.iter().find(|(_, emitted)| **emitted == lowered) else {
                return Err(EmitError::InternalInvariant(
                    "public union has no exact final emitted definition".to_string(),
                ));
            };
            return Ok(TypeRef::NativeUnion(
                self.native_union_definition(name, emitted, origins)?,
            ));
        }
        if matches!(original, TypeRef::Named { .. })
            && !matches!(
                lowered,
                IrType::Struct(_) | IrType::Enum(_) | IrType::Trait(_) | IrType::Generic(_)
            )
        {
            let expanded =
                super::super::codegen::manifest_type_ref_from_ir(&lowered).map_err(EmitError::InternalInvariant)?;
            let mut has_union = false;
            expanded.clone().visit_type_refs(&mut |ty| {
                has_union |= matches!(ty, TypeRef::NativeUnion(_))
                    || matches!(ty, TypeRef::Applied { name, .. } if name == incan_core::lang::types::UNION_TYPE_NAME);
            });
            if has_union {
                return self.project_emitted_union_type(&expanded, &lowered, definitions, origins);
            }
        }
        let mut projected = original.clone();
        match (&mut projected, &lowered) {
            (TypeRef::Applied { args, .. }, IrType::NamedGeneric(_, members) | IrType::Tuple(members))
            | (TypeRef::Tuple { elements: args }, IrType::Tuple(members)) => {
                require_arity(args.len(), members.len())?;
                for (arg, member) in args.iter_mut().zip(members) {
                    *arg = self.project_emitted_union_type(arg, member, definitions, origins)?;
                }
            }
            (TypeRef::Applied { args, .. }, IrType::List(inner) | IrType::Set(inner) | IrType::Option(inner)) => {
                require_arity(args.len(), 1)?;
                if let Some(arg) = args.first_mut() {
                    *arg = self.project_emitted_union_type(arg, inner, definitions, origins)?;
                }
            }
            (TypeRef::Applied { args, .. }, IrType::Dict(left, right) | IrType::Result(left, right)) => {
                require_arity(args.len(), 2)?;
                for (arg, member) in args.iter_mut().zip([left.as_ref(), right.as_ref()]) {
                    *arg = self.project_emitted_union_type(arg, member, definitions, origins)?;
                }
            }
            (
                TypeRef::Ref { inner } | TypeRef::TypeToken { inner },
                IrType::Ref(ty) | IrType::RefMut(ty) | IrType::TypeToken(ty),
            ) => {
                **inner = self.project_emitted_union_type(inner, ty, definitions, origins)?;
            }
            (
                TypeRef::Function { params, return_type },
                IrType::Function {
                    params: lowered_params,
                    ret,
                },
            ) => {
                require_arity(params.len(), lowered_params.len())?;
                for (param, ty) in params.iter_mut().zip(lowered_params) {
                    *param = self.project_emitted_union_type(param, ty, definitions, origins)?;
                }
                **return_type = self.project_emitted_union_type(return_type, ret, definitions, origins)?;
            }
            _ => {}
        }
        Ok(projected)
    }
}

/// Pair declared callable slots with their native parameter and return types, excluding generated receiver slots.
fn pair_function<'a>(
    params: &'a [crate::library_manifest::ParamExport],
    ret: &'a TypeRef,
    function: &'a IrFunction,
    pairs: &mut Vec<(&'a TypeRef, &'a IrType)>,
) -> Result<(), EmitError> {
    require_arity(
        params.len(),
        function.params.iter().filter(|param| !param.is_self).count(),
    )?;
    for param in params {
        if let Some(lowered) = function
            .params
            .iter()
            .find(|candidate| !candidate.is_self && candidate.name == param.name)
        {
            let ty = if param.has_default {
                match &lowered.ty {
                    IrType::Option(inner) => inner,
                    other => other,
                }
            } else {
                &lowered.ty
            };
            pairs.push((&param.ty, ty));
        }
    }
    pairs.push((ret, &function.return_type));
    Ok(())
}

/// Pair named fields within one already-matched nominal declaration.
fn pair_fields<'a>(
    fields: &'a [crate::library_manifest::FieldExport],
    lowered: &'a [super::super::decl::StructField],
    pairs: &mut Vec<(&'a TypeRef, &'a IrType)>,
) {
    for field in fields {
        if let Some(lowered) = lowered.iter().find(|candidate| candidate.name == field.name) {
            pairs.push((&field.ty, &lowered.ty));
        }
    }
}

/// Reject positional metadata drift before any partial native representation is attached.
fn require_arity(checked: usize, emitted: usize) -> Result<(), EmitError> {
    if checked == emitted {
        Ok(())
    } else {
        Err(EmitError::InternalInvariant(format!(
            "checked/emitted public type arity differs: {checked} versus {emitted}"
        )))
    }
}

/// Find an inherent or trait-ABI implementation by its already-retained canonical member projection.
fn projected_method<'a>(
    program: &'a IrProgram,
    owner: &str,
    identity: &incan_semantics_core::CanonicalSymbolId,
) -> Option<&'a IrFunction> {
    let encoded = incan_semantics_core::encode_incan_symbol_identity(identity);
    program.declarations.iter().find_map(|declaration| {
        let IrDeclKind::Impl(implementation) = &declaration.kind else {
            return None;
        };
        if implementation.target_type != owner {
            return None;
        }
        let projected_name = implementation
            .method_projections
            .iter()
            .find(|projection| &projection.identity == identity)
            .map(|projection| projection.abi_method_name.as_str());
        implementation
            .methods
            .iter()
            .find(|method| method.name == encoded || Some(method.name.as_str()) == projected_name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::AstLowering;
    use crate::frontend::api_metadata::{
        CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
    };
    use crate::frontend::typechecker::TypeChecker;
    use crate::frontend::{lexer, parser};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Build checked source and capture the same real emitter table that production publication consumes.
    fn emitted_module(
        source: &str,
        module: &str,
    ) -> Result<
        (
            CheckedApiMetadata,
            Vec<EmittedDeclarationTypes>,
            Vec<NativeUnionExport>,
            String,
        ),
        Box<dyn std::error::Error>,
    > {
        let ast = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
            .map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.check_program(&ast).map_err(|errors| format!("{errors:?}"))?;
        let api = collect_checked_api_metadata(&ast, &checker, vec![module.to_string()]);
        let program = AstLowering::new_with_type_info(checker.type_info().clone()).lower_program(&ast)?;
        let mut emitter = IrEmitter::new(&program.function_registry);
        let rust = emitter.emit_program(&program)?;
        let (declarations, definitions) = emitter.capture_native_union_metadata(
            &api,
            &program,
            &emitter.emitted_native_union_types(),
            &BTreeMap::new(),
        )?;
        Ok((api, declarations, definitions, rust))
    }

    #[test]
    fn emitted_union_publication_is_scoped_to_module_and_span() -> TestResult {
        let (first, first_captured, first_definitions, first_rust) =
            emitted_module("pub type Answer = int | str\n", "first")?;
        let (second, second_captured, second_definitions, second_rust) =
            emitted_module("pub type Answer = int | i32\n", "second")?;
        assert_eq!(
            declaration_anchor(&first.declarations[0]).span,
            declaration_anchor(&second.declarations[0]).span
        );
        assert_eq!(first_definitions.len(), 1);
        assert_eq!(second_definitions.len(), 1);
        assert_ne!(first_definitions[0].rust_name, second_definitions[0].rust_name);
        assert!(first_rust.contains(&format!("enum {}", first_definitions[0].rust_name)));
        assert!(second_rust.contains(&format!("enum {}", second_definitions[0].rust_name)));
        let mut manifest = LibraryManifest::new("producer", "1.0.0");
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![first, second],
            public_namespaces: Vec::new(),
        });
        for captured in first_captured {
            captured.apply(&mut manifest);
        }
        let modules = &manifest.contract_metadata.api.as_ref().ok_or("API absent")?.modules;
        let ApiDeclaration::TypeAlias(first) = &modules[0].declarations[0] else {
            return Err("first alias absent".into());
        };
        let ApiDeclaration::TypeAlias(second) = &modules[1].declarations[0] else {
            return Err("second alias absent".into());
        };
        assert_eq!(
            first.type_alias.target,
            TypeRef::NativeUnion(first_definitions[0].clone())
        );
        assert!(!matches!(second.type_alias.target, TypeRef::NativeUnion(_)));
        for captured in second_captured {
            captured.apply(&mut manifest);
        }
        let second = &manifest.contract_metadata.api.as_ref().ok_or("API absent")?.modules[1].declarations[0];
        let ApiDeclaration::TypeAlias(second) = second else {
            return Err("second alias absent".into());
        };
        assert_eq!(
            second.type_alias.target,
            TypeRef::NativeUnion(second_definitions[0].clone())
        );
        Ok(())
    }

    #[test]
    fn publication_captures_nominal_method_return_unions() -> TestResult {
        let source =
            "pub model Box:\n    value: int\n\n    def answer(self) -> int | str:\n        return self.value\n";
        let (_, captured, definitions, _) = emitted_module(source, "lib")?;
        assert!(
            !definitions.is_empty(),
            "public method return must retain its emitted wrapper"
        );
        assert!(captured.iter().any(|entry| {
            entry
                .replacements
                .iter()
                .any(|(_, ty)| matches!(ty, TypeRef::NativeUnion(_)))
        }));
        Ok(())
    }

    #[test]
    fn positional_attachment_rejects_incomplete_evidence() {
        assert!(require_arity(2, 1).is_err());
    }
}

/// Retain admitted external alias carriers before the existing checked-API alias materializer follows local facades.
///
/// This pass reads the selected artifact's declared public surface. It does not join dependency source anchors to local
/// IR, infer a wrapper from a semantic union, or rediscover another dependency graph.
pub(in crate::backend::ir) fn preserve_native_aliases(
    manifest: &mut LibraryManifest,
    plan: Option<&crate::provider::ProviderPlan>,
) -> Result<(), String> {
    let Some(api) = &mut manifest.contract_metadata.api else {
        return Ok(());
    };
    for module in &mut api.modules {
        for declaration in &mut module.declarations {
            let ApiDeclaration::Alias(alias) = declaration else {
                continue;
            };
            let [root, library, public_path @ ..] = alias.target_path.as_slice() else {
                continue;
            };
            if root != "pub" {
                continue;
            }
            let Some(plan) = plan else {
                continue;
            };
            let Some(crate::frontend::library_manifest_index::LibraryManifestIndexEntry::Loaded {
                manifest: provider,
                ..
            }) = plan.library_manifest_index().get(library)
            else {
                continue;
            };
            let Some((mut ty, mut function)) = declared_alias_surface(provider, public_path)? else {
                continue;
            };
            let mut failure = None;
            let mut bind = |ty: &mut TypeRef| {
                if let TypeRef::NativeUnion(native) = ty {
                    match plan.public_native_union_projection(library, native) {
                        Ok((bound, _)) => *native = bound.for_publication(),
                        Err(error) => failure = Some(error),
                    }
                }
            };
            ty.visit_type_refs(&mut bind);
            function.visit_type_refs(&mut bind);
            if let Some(error) = failure {
                return Err(error);
            }
            if contains_native_union(&ty) {
                alias.projected_type = ty;
            }
            if contains_native_union(&function) {
                if let Some(function) = function {
                    alias.projected_function = Some(crate::frontend::api_metadata::ApiProjectedFunction {
                        source_path: alias.target_path.clone(),
                        callable: crate::frontend::api_metadata::ApiCallableMetadata {
                            name: alias.name.clone(),
                            anchor: alias.anchor.clone(),
                            type_params: function.type_params,
                            receiver: None,
                            params: function.params,
                            return_type: function.return_type,
                            is_async: function.is_async,
                        },
                        decorators: alias
                            .projected_function
                            .as_ref()
                            .map(|projection| projection.decorators.clone())
                            .unwrap_or_default(),
                    });
                }
            }
        }
    }
    crate::frontend::api_metadata::materialize_api_alias_projections(&mut api.modules);
    // Root aliases retain their own exported name and target provenance, with the typed surface copied from their
    // matching checked declaration rather than decoded through ResolvedType again.
    for export in &mut manifest.exports.aliases {
        for module in &api.modules {
            for declaration in &module.declarations {
                let ApiDeclaration::Alias(alias) = declaration else {
                    continue;
                };
                if alias.name != export.name || alias.target_path != export.target_path {
                    continue;
                }
                if contains_native_union(&alias.projected_type) {
                    export.projected_type = alias.projected_type.clone();
                }
                if contains_native_union(&alias.projected_function) {
                    export.projected_function = alias
                        .projected_function
                        .as_ref()
                        .map(crate::frontend::api_metadata::function_export_from_api_projected);
                }
            }
        }
    }
    Ok(())
}

/// Return whether a declared surface includes an explicit producer-native union carrier.
fn contains_native_union(value: &(impl VisitTypeRefs + Clone)) -> bool {
    let mut found = false;
    value
        .clone()
        .visit_type_refs(&mut |ty| found |= matches!(ty, TypeRef::NativeUnion(_)));
    found
}

/// Read one exact declared public member using the existing materialized namespace projection.
fn declared_alias_surface(
    manifest: &LibraryManifest,
    path: &[String],
) -> Result<Option<(Option<TypeRef>, Option<crate::library_manifest::FunctionExport>)>, String> {
    let Some((name, module_path)) = path.split_last() else {
        return Ok(None);
    };
    if module_path.is_empty() {
        if let Some(alias) = manifest.exports.type_aliases.iter().find(|alias| &alias.name == name) {
            return Ok(Some((Some(alias.target.clone()), None)));
        }
        if let Some(alias) = manifest.exports.aliases.iter().find(|alias| &alias.name == name) {
            return Ok(Some((alias.projected_type.clone(), alias.projected_function.clone())));
        }
        let functions = manifest
            .exports
            .functions
            .iter()
            .filter(|function| &function.name == name)
            .collect::<Vec<_>>();
        if let [function] = functions.as_slice() {
            return Ok(Some((None, Some((*function).clone()))));
        }
        if functions.len() > 1 && functions.iter().any(|function| contains_native_union(*function)) {
            return Err(format!(
                "native union alias `{name}` requires an exact callable overload projection"
            ));
        }
    }
    let Some(api) = &manifest.contract_metadata.api else {
        return Ok(None);
    };
    let Some(namespace) = crate::frontend::api_metadata::checked_api_public_namespace(api, module_path) else {
        return Ok(None);
    };
    let source_paths = namespace
        .members
        .iter()
        .filter(|member| &member.name == name)
        .map(|member| &member.source_path)
        .collect::<Vec<_>>();
    if source_paths.len() > 1 {
        return Err(format!(
            "native alias target `{}` has ambiguous checked namespace membership",
            path.join("::")
        ));
    }
    let Some(source_path) = source_paths.first() else {
        return Ok(None);
    };
    let Some((source_name, source_module)) = source_path.split_last() else {
        return Ok(None);
    };
    let Some(module) = api.modules.iter().find(|module| module.module_path == source_module) else {
        return Ok(None);
    };
    let Some(declaration) = module.declarations.iter().find(|declaration| {
        crate::frontend::api_metadata::api_declaration_public_name(declaration) == Some(source_name.as_str())
    }) else {
        return Ok(None);
    };
    Ok(match declaration {
        ApiDeclaration::TypeAlias(alias) => Some((Some(alias.type_alias.target.clone()), None)),
        ApiDeclaration::Alias(alias) => Some((
            alias.projected_type.clone(),
            alias
                .projected_function
                .as_ref()
                .map(crate::frontend::api_metadata::function_export_from_api_projected),
        )),
        ApiDeclaration::Function(function) => Some((
            None,
            Some(crate::frontend::api_metadata::function_export_from_api(function)),
        )),
        _ => None,
    })
}
