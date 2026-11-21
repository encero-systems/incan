//! One explicit adapter from native compiler carriers to the compiler-owned identity and checked-type records.
//!
//! The records a plan names (a carrier's semantic key, a trait identity) are the plan's mirror of the record models.
//! Their validators live once, in the record models; a record is checked there through its identical serde form until
//! the plan types re-export the record models directly.

use std::collections::BTreeMap;

use incan_mir_plan_native::record_types::{
    CanonicalSymbolId, CheckedCallableParam, CheckedCallableParamKind, CheckedInputAccess, CheckedNumericType,
    CheckedPrimitiveType, CheckedRuntimeBuiltin, CheckedType, DeclarationKind, HirSourceSpan, PackageIdentity,
    SymbolNamespace, SymbolOrigin,
};
use incan_semantics_core::{
    CanonicalSymbolId as NativeCanonicalSymbolId, HirSourceSpan as NativeHirSourceSpan, IncanCallableParam,
    IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType, IncanType, NumericTypeId, RuntimeBuiltinType,
    ScopeDiscriminant, SemanticSourceTargetKind, SymbolNamespace as NativeSymbolNamespace,
    SymbolOrigin as NativeSymbolOrigin, canonical_module_identity,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Read a plan record as the record model with the same serde form.
fn as_record_model<From: Serialize, To: DeserializeOwned>(value: &From) -> Option<To> {
    postcard::to_allocvec(value)
        .ok()
        .and_then(|bytes| postcard::from_bytes(&bytes).ok())
}

fn valid_checked_type(ty: &CheckedType) -> bool {
    as_record_model::<_, internal_models_native::checked_types::CheckedType>(ty)
        .is_some_and(|record| internal_models_native::checked_types::valid_checked_type(&record))
}

fn valid_canonical_symbol_id(identity: &CanonicalSymbolId) -> bool {
    as_record_model::<_, internal_models_native::compiler_records::CanonicalSymbolId>(identity)
        .is_some_and(|record| internal_models_native::compiler_records::valid_canonical_symbol_id(&record))
}

fn valid_hir_source_span(span: &HirSourceSpan) -> bool {
    as_record_model::<_, internal_models_native::compiler_records::HirSourceSpan>(span)
        .is_some_and(|record| internal_models_native::compiler_records::valid_hir_source_span(&record))
}

/// Copy every native canonical field into the record and attach boundary-owned source facts.
pub fn canonical_symbol_id(
    identity: &NativeCanonicalSymbolId,
    source_file: &str,
    package_version: &str,
) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace: symbol_namespace(identity.namespace),
        origin: symbol_origin(&identity.origin, package_version),
        declaration_name: identity.declaration_name.clone(),
        kind: declaration_kind(&identity.kind),
        scope_discriminant: identity.scope_discriminant.map(|scope| scope.0 as i64),
        declaration_span: hir_source_span(&identity.declaration_span, source_file),
    }
}

/// Attach an owned source filename and copy the exact byte range and optional 32-byte expansion identity.
pub fn hir_source_span(span: &NativeHirSourceSpan, source_file: &str) -> HirSourceSpan {
    HirSourceSpan {
        file: source_file.to_owned(),
        start: span.start as i64,
        end: span.end as i64,
        expansion: span
            .expansion
            .map(|values| values.iter().map(|value| i64::from(*value)).collect()),
    }
}

/// Restore one validated record identity.
pub fn native_canonical_symbol_id(identity: &CanonicalSymbolId) -> Result<NativeCanonicalSymbolId, String> {
    if !valid_canonical_symbol_id(identity) {
        return Err("invalid authored canonical symbol identity".into());
    }
    Ok(NativeCanonicalSymbolId {
        namespace: native_symbol_namespace(&identity.namespace),
        origin: native_symbol_origin(&identity.origin),
        declaration_name: identity.declaration_name.clone(),
        kind: native_declaration_kind(&identity.kind),
        scope_discriminant: identity
            .scope_discriminant
            .map(|value| ScopeDiscriminant(value as usize)),
        declaration_span: native_hir_source_span(&identity.declaration_span)?,
    })
}

/// Restore a validated record range and its optional 32-byte expansion exactly.
pub fn native_hir_source_span(span: &HirSourceSpan) -> Result<NativeHirSourceSpan, String> {
    if !valid_hir_source_span(span) {
        return Err("invalid authored source span".into());
    }
    let start = usize::try_from(span.start).map_err(|_| "authored source span exceeds the native index range")?;
    let end = usize::try_from(span.end).map_err(|_| "authored source span exceeds the native index range")?;
    let native = NativeHirSourceSpan::new(start, end);
    match &span.expansion {
        Some(expansion) => {
            let bytes: [u8; 32] = std::array::from_fn(|index| expansion[index] as u8);
            Ok(native.with_expansion(Some(bytes)))
        }
        None => Ok(native),
    }
}

/// Copy every semantic type alternative recursively without using diagnostic display text as identity.
pub fn checked_type(ty: &IncanType, source_file: &str, package_version: &str) -> CheckedType {
    let close = |inner: &IncanType| checked_type(inner, source_file, package_version);
    match ty {
        IncanType::Never => CheckedType::Never,
        IncanType::Primitive(primitive) => CheckedType::Primitive(checked_primitive_type(primitive)),
        IncanType::Named(name) => CheckedType::Named(name.clone()),
        IncanType::Generic { base, args } => CheckedType::Generic(base.clone(), args.iter().map(close).collect()),
        IncanType::Decimal { precision, scale } => CheckedType::Decimal(*precision, *scale),
        IncanType::Function { params, return_type } => CheckedType::Function(
            checked_parameters(params, source_file, package_version),
            vec![close(return_type)],
        ),
        IncanType::TypeToken(represented) => CheckedType::TypeToken(vec![close(represented)]),
        IncanType::Tuple(elements) => CheckedType::Tuple(elements.iter().map(close).collect()),
        IncanType::TypeVar(name) => CheckedType::TypeVar(name.clone()),
        IncanType::SelfType => CheckedType::SelfType,
        IncanType::Ref(referent) => CheckedType::Ref(vec![close(referent)]),
        IncanType::RefMut(referent) => CheckedType::RefMut(vec![close(referent)]),
        IncanType::RustInteropPath(path) => CheckedType::RustInteropPath(path.clone()),
        IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError) => {
            CheckedType::NativeBuiltin(CheckedRuntimeBuiltin::ValidationError)
        }
        IncanType::Unsupported(reason) => CheckedType::Unsupported(reason.clone()),
        IncanType::Infer => CheckedType::Infer,
        IncanType::Unknown => CheckedType::Unknown,
        IncanType::NativeArray { element, length } => CheckedType::NativeArray(vec![close(element)], *length),
        IncanType::NativeTraitObject { path, send, sync } => CheckedType::NativeTraitObject(path.clone(), *send, *sync),
        IncanType::NativePointer { mutable, pointee } => CheckedType::NativePointer(*mutable, vec![close(pointee)]),
        IncanType::NativeCChar => CheckedType::NativeCChar,
        IncanType::Trait(identity) => CheckedType::Trait(canonical_symbol_id(identity, source_file, package_version)),
    }
}

/// Restore every validated record checked-type alternative without parsing diagnostic display text.
pub fn native_checked_type(ty: &CheckedType) -> Result<IncanType, String> {
    if !valid_checked_type(ty) {
        return Err("invalid authored checked type".into());
    }
    native_checked_type_unchecked(ty)
}

fn native_checked_type_unchecked(ty: &CheckedType) -> Result<IncanType, String> {
    let first = |types: &[CheckedType]| -> Result<IncanType, String> {
        native_checked_type_unchecked(types.first().ok_or("invalid authored checked type")?)
    };
    Ok(match ty {
        CheckedType::Never => IncanType::Never,
        CheckedType::Primitive(primitive) => IncanType::Primitive(native_primitive_type(primitive)),
        CheckedType::Named(name) => IncanType::Named(name.clone()),
        CheckedType::Generic(base, arguments) => IncanType::Generic {
            base: base.clone(),
            args: arguments
                .iter()
                .map(native_checked_type_unchecked)
                .collect::<Result<_, _>>()?,
        },
        CheckedType::Decimal(precision, scale) => IncanType::Decimal {
            precision: *precision,
            scale: *scale,
        },
        CheckedType::Function(parameters, result) => IncanType::Function {
            params: native_checked_parameters(parameters)?,
            return_type: Box::new(first(result)?),
        },
        CheckedType::TypeToken(represented) => IncanType::TypeToken(Box::new(first(represented)?)),
        CheckedType::Tuple(elements) => IncanType::Tuple(
            elements
                .iter()
                .map(native_checked_type_unchecked)
                .collect::<Result<_, _>>()?,
        ),
        CheckedType::TypeVar(name) => IncanType::TypeVar(name.clone()),
        CheckedType::SelfType => IncanType::SelfType,
        CheckedType::Ref(referent) => IncanType::Ref(Box::new(first(referent)?)),
        CheckedType::RefMut(referent) => IncanType::RefMut(Box::new(first(referent)?)),
        CheckedType::RustInteropPath(path) => IncanType::RustInteropPath(path.clone()),
        CheckedType::NativeBuiltin(CheckedRuntimeBuiltin::ValidationError) => {
            IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError)
        }
        CheckedType::Unsupported(reason) => IncanType::Unsupported(reason.clone()),
        CheckedType::Infer => IncanType::Infer,
        CheckedType::Unknown => IncanType::Unknown,
        CheckedType::NativeArray(element, length) => IncanType::NativeArray {
            element: Box::new(first(element)?),
            length: *length,
        },
        CheckedType::NativeTraitObject(path, send, sync) => IncanType::NativeTraitObject {
            path: path.clone(),
            send: *send,
            sync: *sync,
        },
        CheckedType::NativePointer(mutable, pointee) => IncanType::NativePointer {
            mutable: *mutable,
            pointee: Box::new(first(pointee)?),
        },
        CheckedType::NativeCChar => IncanType::NativeCChar,
        CheckedType::Trait(identity) => IncanType::Trait(native_canonical_symbol_id(identity)?),
    })
}

/// Attach an identity's owning module file and authenticated selected package version to one plan fact.
pub fn authored_plan_symbol_id(
    identity: &NativeCanonicalSymbolId,
    fallback_source_file: &str,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
) -> Result<CanonicalSymbolId, String> {
    let source_file = selected_source_file(identity, fallback_source_file, source_files)?;
    let package_version = selected_package_version(identity, package_versions)?;
    let authored = canonical_symbol_id(identity, &source_file, &package_version);
    if !valid_canonical_symbol_id(&authored) {
        return Err("Plan identity is missing complete source provenance".into());
    }
    Ok(authored)
}

/// Project a plan-owned checked fact with each nested trait's authenticated module file and package version.
pub fn authored_plan_checked_type(
    ty: &IncanType,
    fallback_source_file: &str,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
) -> Result<CheckedType, String> {
    let authored = complete_checked_type_provenance(
        checked_type(ty, fallback_source_file, ""),
        fallback_source_file,
        source_files,
        package_versions,
    )?;
    if !valid_checked_type(&authored) {
        return Err("Plan checked type is missing selected identity provenance".into());
    }
    Ok(authored)
}

fn selected_source_file(
    identity: &NativeCanonicalSymbolId,
    fallback_source_file: &str,
    source_files: &BTreeMap<String, String>,
) -> Result<String, String> {
    match canonical_module_identity(identity) {
        Some(owner) => match source_files.get(&owner) {
            Some(source_file) if !source_file.is_empty() => Ok(source_file.clone()),
            _ => Err(format!("Plan identity has no source file for module {owner}")),
        },
        None => {
            if fallback_source_file.is_empty() {
                return Err("Plan identity has no native or builtin source file".into());
            }
            Ok(fallback_source_file.to_owned())
        }
    }
}

fn selected_package_version(
    identity: &NativeCanonicalSymbolId,
    package_versions: &BTreeMap<String, String>,
) -> Result<String, String> {
    match &identity.origin {
        NativeSymbolOrigin::Package { library, .. } => match package_versions.get(library) {
            Some(version) if !version.is_empty() => Ok(version.clone()),
            _ => Err(format!("Plan identity has no selected package version for {library}")),
        },
        _ => Ok(String::new()),
    }
}

fn complete_checked_type_provenance(
    ty: CheckedType,
    fallback_source_file: &str,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
) -> Result<CheckedType, String> {
    let complete = |inner: CheckedType| {
        complete_checked_type_provenance(inner, fallback_source_file, source_files, package_versions)
    };
    let first = |types: Vec<CheckedType>| -> Result<Vec<CheckedType>, String> {
        let head = types.into_iter().next().ok_or("invalid authored checked type")?;
        Ok(vec![complete(head)?])
    };
    Ok(match ty {
        CheckedType::Generic(base, arguments) => {
            CheckedType::Generic(base, arguments.into_iter().map(complete).collect::<Result<_, _>>()?)
        }
        CheckedType::Function(parameters, result) => {
            let mut completed = Vec::with_capacity(parameters.len());
            for parameter in parameters {
                completed.push(CheckedCallableParam {
                    ty: complete(parameter.ty.clone())?,
                    ..parameter
                });
            }
            CheckedType::Function(completed, first(result)?)
        }
        CheckedType::TypeToken(represented) => CheckedType::TypeToken(first(represented)?),
        CheckedType::Tuple(elements) => {
            CheckedType::Tuple(elements.into_iter().map(complete).collect::<Result<_, _>>()?)
        }
        CheckedType::Ref(referent) => CheckedType::Ref(first(referent)?),
        CheckedType::RefMut(referent) => CheckedType::RefMut(first(referent)?),
        CheckedType::NativeArray(element, length) => CheckedType::NativeArray(first(element)?, length),
        CheckedType::NativePointer(mutable, pointee) => CheckedType::NativePointer(mutable, first(pointee)?),
        CheckedType::Trait(identity) => CheckedType::Trait(complete_authored_provenance(
            identity,
            fallback_source_file,
            source_files,
            package_versions,
        )?),
        other => other,
    })
}

/// Complete one nested record identity without requiring its intentionally incomplete boundary form to validate.
fn complete_authored_provenance(
    identity: CanonicalSymbolId,
    fallback_source_file: &str,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
) -> Result<CanonicalSymbolId, String> {
    let source_file = authored_source_file(&identity.origin, fallback_source_file, source_files)?;
    let origin = match identity.origin {
        SymbolOrigin::Package(package, module_path) => match package_versions.get(&package.name) {
            Some(version) => {
                if version.is_empty() {
                    return Err(format!(
                        "Plan identity has an empty selected package version for {}",
                        package.name
                    ));
                }
                SymbolOrigin::Package(
                    PackageIdentity {
                        name: package.name,
                        version: version.clone(),
                    },
                    module_path,
                )
            }
            None => {
                return Err(format!(
                    "Plan identity has no selected package version for {}",
                    package.name
                ));
            }
        },
        other => other,
    };
    let span = identity.declaration_span;
    Ok(CanonicalSymbolId {
        namespace: identity.namespace,
        origin,
        declaration_name: identity.declaration_name,
        kind: identity.kind,
        scope_discriminant: identity.scope_discriminant,
        declaration_span: HirSourceSpan {
            file: source_file,
            start: span.start,
            end: span.end,
            expansion: span.expansion,
        },
    })
}

fn authored_source_file(
    origin: &SymbolOrigin,
    fallback_source_file: &str,
    source_files: &BTreeMap<String, String>,
) -> Result<String, String> {
    let module = match origin {
        SymbolOrigin::Module(path) => Some(source_module_identity(path)),
        SymbolOrigin::Package(package, path) => {
            Some(format!("pub::{}::{}", package.name, source_module_identity(path)))
        }
        _ => None,
    };
    match module {
        Some(owner) => match source_files.get(&owner) {
            Some(source_file) if !source_file.is_empty() => Ok(source_file.clone()),
            _ => Err(format!("Plan identity has no source file for module {owner}")),
        },
        None => {
            if fallback_source_file.is_empty() {
                return Err("Plan identity has no native or builtin source file".into());
            }
            Ok(fallback_source_file.to_owned())
        }
    }
}

/// Render a record module path with the canonical Body IR empty-module spelling.
fn source_module_identity(path: &[String]) -> String {
    if path.is_empty() {
        return "<module>".to_owned();
    }
    path.join("::")
}

/// Compare a plan-owned record fact with a native checked type through the explicit reverse adapter.
pub fn checked_type_matches_native(authored: &CheckedType, native: &IncanType) -> Result<bool, String> {
    Ok(native_checked_type(authored)? == *native)
}

/// Compare an optional plan semantic key with one native carrier without display reconstruction.
pub fn semantic_key_matches_native(authored: &Option<CheckedType>, native: &IncanType) -> Result<bool, String> {
    match authored {
        Some(retained) => checked_type_matches_native(retained, native),
        None => Ok(false),
    }
}

fn symbol_namespace(namespace: NativeSymbolNamespace) -> SymbolNamespace {
    match namespace {
        NativeSymbolNamespace::OrdinaryLexical => SymbolNamespace::OrdinaryLexical,
        NativeSymbolNamespace::Member => SymbolNamespace::Member,
        NativeSymbolNamespace::ModulePath => SymbolNamespace::ModulePath,
    }
}

fn symbol_origin(origin: &NativeSymbolOrigin, package_version: &str) -> SymbolOrigin {
    match origin {
        NativeSymbolOrigin::Module(path) => SymbolOrigin::Module(path.clone()),
        NativeSymbolOrigin::Package { library, module_path } => SymbolOrigin::Package(
            PackageIdentity {
                name: library.clone(),
                version: package_version.to_owned(),
            },
            module_path.clone(),
        ),
        NativeSymbolOrigin::RustCrate(path) => SymbolOrigin::RustCrate(path.clone()),
        NativeSymbolOrigin::Builtin => SymbolOrigin::Builtin,
    }
}

fn declaration_kind(kind: &SemanticSourceTargetKind) -> DeclarationKind {
    use SemanticSourceTargetKind as Native;
    match kind {
        Native::Function => DeclarationKind::Function,
        Native::Model => DeclarationKind::Model,
        Native::Class => DeclarationKind::Class,
        Native::Newtype => DeclarationKind::Newtype,
        Native::Rusttype => DeclarationKind::Rusttype,
        Native::Enum => DeclarationKind::Enum,
        Native::TypeAlias => DeclarationKind::TypeAlias,
        Native::Partial => DeclarationKind::Partial,
        Native::Variant => DeclarationKind::Variant,
        Native::Trait => DeclarationKind::Trait,
        Native::Capability => DeclarationKind::Capability,
        Native::Field => DeclarationKind::Field,
        Native::Method => DeclarationKind::Method,
        Native::Property => DeclarationKind::Property,
        Native::Const => DeclarationKind::Const,
        Native::Static => DeclarationKind::Static,
        Native::Local => DeclarationKind::Local,
        Native::Parameter => DeclarationKind::Parameter,
        Native::Receiver => DeclarationKind::Receiver,
        Native::GenericBinder => DeclarationKind::GenericBinder,
        Native::Module => DeclarationKind::Module,
        Native::RustItem => DeclarationKind::RustItem,
        Native::Builtin => DeclarationKind::Builtin,
        Native::Other(spelling) => DeclarationKind::Other(spelling.clone()),
    }
}

fn checked_parameters(
    parameters: &[IncanCallableParam],
    source_file: &str,
    package_version: &str,
) -> Vec<CheckedCallableParam> {
    parameters
        .iter()
        .map(|parameter| CheckedCallableParam {
            name: parameter.name.clone(),
            ty: checked_type(&parameter.ty, source_file, package_version),
            kind: match parameter.kind {
                IncanCallableParamKind::Normal => CheckedCallableParamKind::Normal,
                IncanCallableParamKind::RestPositional => CheckedCallableParamKind::RestPositional,
                IncanCallableParamKind::RestKeyword => CheckedCallableParamKind::RestKeyword,
            },
            has_default: parameter.has_default,
            is_partial_preset: parameter.is_partial_preset,
            is_mut: parameter.is_mut,
            native_owned: parameter.native_owned,
            input_access: match parameter.input_access {
                IncanInputAccess::Inferred => CheckedInputAccess::Inferred,
                IncanInputAccess::Shared => CheckedInputAccess::Shared,
                IncanInputAccess::Exclusive => CheckedInputAccess::Exclusive,
                IncanInputAccess::Owned => CheckedInputAccess::Owned,
                IncanInputAccess::InferredShared => CheckedInputAccess::InferredShared,
            },
        })
        .collect()
}

fn checked_primitive_type(primitive: &IncanPrimitiveType) -> CheckedPrimitiveType {
    match primitive {
        IncanPrimitiveType::Int => CheckedPrimitiveType::Int,
        IncanPrimitiveType::Float => CheckedPrimitiveType::Float,
        IncanPrimitiveType::Numeric(numeric) => CheckedPrimitiveType::Numeric(checked_numeric_type(*numeric)),
        IncanPrimitiveType::Bool => CheckedPrimitiveType::Bool,
        IncanPrimitiveType::Str => CheckedPrimitiveType::Str,
        IncanPrimitiveType::Bytes => CheckedPrimitiveType::Bytes,
        IncanPrimitiveType::FrozenStr => CheckedPrimitiveType::FrozenStr,
        IncanPrimitiveType::FrozenBytes => CheckedPrimitiveType::FrozenBytes,
        IncanPrimitiveType::Unit => CheckedPrimitiveType::Unit,
    }
}

fn checked_numeric_type(numeric: NumericTypeId) -> CheckedNumericType {
    match numeric {
        NumericTypeId::I8 => CheckedNumericType::I8,
        NumericTypeId::I16 => CheckedNumericType::I16,
        NumericTypeId::I32 => CheckedNumericType::I32,
        NumericTypeId::I64 => CheckedNumericType::I64,
        NumericTypeId::I128 => CheckedNumericType::I128,
        NumericTypeId::U8 => CheckedNumericType::U8,
        NumericTypeId::U16 => CheckedNumericType::U16,
        NumericTypeId::U32 => CheckedNumericType::U32,
        NumericTypeId::U64 => CheckedNumericType::U64,
        NumericTypeId::U128 => CheckedNumericType::U128,
        NumericTypeId::F32 => CheckedNumericType::F32,
        NumericTypeId::F64 => CheckedNumericType::F64,
        NumericTypeId::ISize => CheckedNumericType::ISize,
        NumericTypeId::USize => CheckedNumericType::USize,
        NumericTypeId::Bool => CheckedNumericType::Bool,
    }
}

fn native_symbol_namespace(namespace: &SymbolNamespace) -> NativeSymbolNamespace {
    match namespace {
        SymbolNamespace::OrdinaryLexical => NativeSymbolNamespace::OrdinaryLexical,
        SymbolNamespace::Member => NativeSymbolNamespace::Member,
        SymbolNamespace::ModulePath => NativeSymbolNamespace::ModulePath,
    }
}

fn native_symbol_origin(origin: &SymbolOrigin) -> NativeSymbolOrigin {
    match origin {
        SymbolOrigin::Module(path) => NativeSymbolOrigin::Module(path.clone()),
        SymbolOrigin::Package(package, path) => NativeSymbolOrigin::Package {
            library: package.name.clone(),
            module_path: path.clone(),
        },
        SymbolOrigin::RustCrate(path) => NativeSymbolOrigin::RustCrate(path.clone()),
        SymbolOrigin::Builtin => NativeSymbolOrigin::Builtin,
    }
}

fn native_declaration_kind(kind: &DeclarationKind) -> SemanticSourceTargetKind {
    use SemanticSourceTargetKind as Native;
    match kind {
        DeclarationKind::Function => Native::Function,
        DeclarationKind::Model => Native::Model,
        DeclarationKind::Class => Native::Class,
        DeclarationKind::Newtype => Native::Newtype,
        DeclarationKind::Rusttype => Native::Rusttype,
        DeclarationKind::Enum => Native::Enum,
        DeclarationKind::TypeAlias => Native::TypeAlias,
        DeclarationKind::Partial => Native::Partial,
        DeclarationKind::Variant => Native::Variant,
        DeclarationKind::Trait => Native::Trait,
        DeclarationKind::Capability => Native::Capability,
        DeclarationKind::Field => Native::Field,
        DeclarationKind::Method => Native::Method,
        DeclarationKind::Property => Native::Property,
        DeclarationKind::Const => Native::Const,
        DeclarationKind::Static => Native::Static,
        DeclarationKind::Local => Native::Local,
        DeclarationKind::Parameter => Native::Parameter,
        DeclarationKind::Receiver => Native::Receiver,
        DeclarationKind::GenericBinder => Native::GenericBinder,
        DeclarationKind::Module => Native::Module,
        DeclarationKind::RustItem => Native::RustItem,
        DeclarationKind::Builtin => Native::Builtin,
        DeclarationKind::Other(spelling) => Native::Other(spelling.clone()),
    }
}

fn native_checked_parameters(parameters: &[CheckedCallableParam]) -> Result<Vec<IncanCallableParam>, String> {
    parameters
        .iter()
        .map(|parameter| {
            Ok(IncanCallableParam {
                name: parameter.name.clone(),
                ty: native_checked_type_unchecked(&parameter.ty)?,
                kind: match parameter.kind {
                    CheckedCallableParamKind::Normal => IncanCallableParamKind::Normal,
                    CheckedCallableParamKind::RestPositional => IncanCallableParamKind::RestPositional,
                    CheckedCallableParamKind::RestKeyword => IncanCallableParamKind::RestKeyword,
                },
                has_default: parameter.has_default,
                is_partial_preset: parameter.is_partial_preset,
                is_mut: parameter.is_mut,
                native_owned: parameter.native_owned,
                input_access: match parameter.input_access {
                    CheckedInputAccess::Inferred => IncanInputAccess::Inferred,
                    CheckedInputAccess::Shared => IncanInputAccess::Shared,
                    CheckedInputAccess::Exclusive => IncanInputAccess::Exclusive,
                    CheckedInputAccess::Owned => IncanInputAccess::Owned,
                    CheckedInputAccess::InferredShared => IncanInputAccess::InferredShared,
                },
            })
        })
        .collect()
}

fn native_primitive_type(primitive: &CheckedPrimitiveType) -> IncanPrimitiveType {
    match primitive {
        CheckedPrimitiveType::Int => IncanPrimitiveType::Int,
        CheckedPrimitiveType::Float => IncanPrimitiveType::Float,
        CheckedPrimitiveType::Numeric(numeric) => IncanPrimitiveType::Numeric(native_numeric_type(numeric)),
        CheckedPrimitiveType::Bool => IncanPrimitiveType::Bool,
        CheckedPrimitiveType::Str => IncanPrimitiveType::Str,
        CheckedPrimitiveType::Bytes => IncanPrimitiveType::Bytes,
        CheckedPrimitiveType::FrozenStr => IncanPrimitiveType::FrozenStr,
        CheckedPrimitiveType::FrozenBytes => IncanPrimitiveType::FrozenBytes,
        CheckedPrimitiveType::Unit => IncanPrimitiveType::Unit,
    }
}

fn native_numeric_type(numeric: &CheckedNumericType) -> NumericTypeId {
    match numeric {
        CheckedNumericType::I8 => NumericTypeId::I8,
        CheckedNumericType::I16 => NumericTypeId::I16,
        CheckedNumericType::I32 => NumericTypeId::I32,
        CheckedNumericType::I64 => NumericTypeId::I64,
        CheckedNumericType::I128 => NumericTypeId::I128,
        CheckedNumericType::U8 => NumericTypeId::U8,
        CheckedNumericType::U16 => NumericTypeId::U16,
        CheckedNumericType::U32 => NumericTypeId::U32,
        CheckedNumericType::U64 => NumericTypeId::U64,
        CheckedNumericType::U128 => NumericTypeId::U128,
        CheckedNumericType::F32 => NumericTypeId::F32,
        CheckedNumericType::F64 => NumericTypeId::F64,
        CheckedNumericType::ISize => NumericTypeId::ISize,
        CheckedNumericType::USize => NumericTypeId::USize,
        CheckedNumericType::Bool => NumericTypeId::Bool,
    }
}
