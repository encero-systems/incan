//! Library manifest (`.incnlib`) semantic model and stable IO boundary.
//!
//! The semantic model in this file is intentionally transport-agnostic. JSON is the current on-disk encoding, but
//! callers interact with typed read/write APIs only.

use std::fs;
use std::path::{Path, PathBuf};

use incan_core::lang::conventions;
use incan_core::lang::types::collections::{self, CollectionTypeId};
use incan_core::lang::types::numerics::{self, NumericTypeId};
use incan_core::lang::types::stringlike::{self, StringLikeId};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::frontend::library_exports::{
    CheckedClassExport, CheckedConstExport, CheckedEnumExport, CheckedExportKind, CheckedFunctionExport,
    CheckedModelExport, CheckedNamedExport, CheckedNewtypeExport, CheckedTraitExport, CheckedTypeAliasExport,
    CheckedTypeBound, CheckedTypeParam,
};
use crate::frontend::symbols::ResolvedType;

pub const LIBRARY_MANIFEST_FORMAT: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum LibraryManifestError {
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("failed to write {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
    #[error("failed to parse library manifest: {0}")]
    Parse(String),
    #[error("failed to serialize library manifest: {0}")]
    Serialize(String),
    #[error("invalid library manifest: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryManifest {
    pub name: String,
    pub version: String,
    pub incan_version: String,
    pub manifest_format: u32,
    pub exports: LibraryExports,
    pub soft_keywords: SoftKeywordExports,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LibraryExports {
    pub models: Vec<ModelExport>,
    pub classes: Vec<ClassExport>,
    pub functions: Vec<FunctionExport>,
    pub traits: Vec<TraitExport>,
    pub enums: Vec<EnumExport>,
    pub type_aliases: Vec<TypeAliasExport>,
    pub newtypes: Vec<NewtypeExport>,
    pub consts: Vec<ConstExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SoftKeywordExports {
    pub activations: Vec<SoftKeywordActivation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftKeywordActivation {
    pub namespace: String,
    pub keyword: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeParamExport {
    pub name: String,
    pub bounds: Vec<TypeBoundExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeBoundExport {
    pub name: String,
    pub type_args: Vec<TypeRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeRef {
    Named {
        name: String,
    },
    Applied {
        name: String,
        args: Vec<TypeRef>,
    },
    Function {
        params: Vec<TypeRef>,
        return_type: Box<TypeRef>,
    },
    Tuple {
        elements: Vec<TypeRef>,
    },
    TypeParam {
        name: String,
    },
    SelfType,
    Ref {
        inner: Box<TypeRef>,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldExport {
    pub name: String,
    pub ty: TypeRef,
    pub has_default: bool,
    pub alias: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiverExport {
    Immutable,
    Mutable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodExport {
    pub name: String,
    pub receiver: Option<ReceiverExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    pub is_async: bool,
    pub has_body: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamExport {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    pub params: Vec<ParamExport>,
    pub return_type: TypeRef,
    pub is_async: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeAliasExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    pub target: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    pub traits: Vec<String>,
    pub fields: Vec<FieldExport>,
    pub methods: Vec<MethodExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    pub extends: Option<String>,
    pub traits: Vec<String>,
    pub fields: Vec<FieldExport>,
    pub methods: Vec<MethodExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    pub requires: Vec<FieldRequirementExport>,
    pub methods: Vec<MethodExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldRequirementExport {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    pub variants: Vec<EnumVariantExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumVariantExport {
    pub name: String,
    pub fields: Vec<TypeRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewtypeExport {
    pub name: String,
    pub type_params: Vec<TypeParamExport>,
    pub underlying: TypeRef,
    pub methods: Vec<MethodExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstExport {
    pub name: String,
    pub ty: TypeRef,
}

impl LibraryManifest {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            incan_version: crate::version::INCAN_VERSION.to_string(),
            manifest_format: LIBRARY_MANIFEST_FORMAT,
            exports: LibraryExports::default(),
            soft_keywords: SoftKeywordExports::default(),
        }
    }

    pub fn from_checked_exports(
        name: impl Into<String>,
        version: impl Into<String>,
        checked_exports: &[CheckedNamedExport],
    ) -> Self {
        let mut manifest = Self::new(name, version);
        manifest.exports = LibraryExports::from_checked_exports(checked_exports);
        manifest
    }

    pub fn write_to_path(&self, path: &Path) -> Result<(), LibraryManifestError> {
        let raw = RawLibraryManifest::from_semantic(self);
        let content =
            serde_json::to_string_pretty(&raw).map_err(|err| LibraryManifestError::Serialize(err.to_string()))?;
        fs::write(path, format!("{content}\n")).map_err(|source| LibraryManifestError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    pub fn read_from_path(path: &Path) -> Result<Self, LibraryManifestError> {
        let content = fs::read_to_string(path).map_err(|source| LibraryManifestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json_str(&content)
    }

    pub fn from_json_str(content: &str) -> Result<Self, LibraryManifestError> {
        let raw: RawLibraryManifest =
            serde_json::from_str(content).map_err(|err| LibraryManifestError::Parse(err.to_string()))?;
        raw.validate()?;
        raw.into_semantic()
    }
}

impl LibraryExports {
    fn from_checked_exports(exports: &[CheckedNamedExport]) -> Self {
        let mut model = Self::default();

        for export in exports {
            match &export.kind {
                CheckedExportKind::Function(function_export) => {
                    model.functions.push(function_export_from_checked(function_export));
                }
                CheckedExportKind::TypeAlias(type_alias_export) => {
                    model
                        .type_aliases
                        .push(type_alias_export_from_checked(type_alias_export));
                }
                CheckedExportKind::Model(model_export) => {
                    model.models.push(model_export_from_checked(model_export));
                }
                CheckedExportKind::Class(class_export) => {
                    model.classes.push(class_export_from_checked(class_export));
                }
                CheckedExportKind::Trait(trait_export) => {
                    model.traits.push(trait_export_from_checked(trait_export));
                }
                CheckedExportKind::Enum(enum_export) => {
                    model.enums.push(enum_export_from_checked(enum_export));
                }
                CheckedExportKind::Newtype(newtype_export) => {
                    model.newtypes.push(newtype_export_from_checked(newtype_export));
                }
                CheckedExportKind::Const(const_export) => {
                    model.consts.push(const_export_from_checked(const_export));
                }
            }
        }

        model.sort_deterministically();
        model
    }

    fn sort_deterministically(&mut self) {
        self.models.sort_by(|left, right| left.name.cmp(&right.name));
        self.classes.sort_by(|left, right| left.name.cmp(&right.name));
        self.functions.sort_by(|left, right| left.name.cmp(&right.name));
        self.traits.sort_by(|left, right| left.name.cmp(&right.name));
        self.enums.sort_by(|left, right| left.name.cmp(&right.name));
        self.type_aliases.sort_by(|left, right| left.name.cmp(&right.name));
        self.newtypes.sort_by(|left, right| left.name.cmp(&right.name));
        self.consts.sort_by(|left, right| left.name.cmp(&right.name));
    }
}

fn type_ref_from_resolved(ty: &ResolvedType) -> TypeRef {
    match ty {
        ResolvedType::Int => named_type_ref(numerics::as_str(NumericTypeId::Int)),
        ResolvedType::Float => named_type_ref(numerics::as_str(NumericTypeId::Float)),
        ResolvedType::Bool => named_type_ref(numerics::as_str(NumericTypeId::Bool)),
        ResolvedType::Str => named_type_ref(stringlike::as_str(StringLikeId::Str)),
        ResolvedType::Bytes => named_type_ref(stringlike::as_str(StringLikeId::Bytes)),
        // Keep existing surface spellings used by ResolvedType display for frozen string-like types.
        ResolvedType::FrozenStr => named_type_ref(ResolvedType::FrozenStr.to_string()),
        ResolvedType::FrozenBytes => named_type_ref(ResolvedType::FrozenBytes.to_string()),
        ResolvedType::FrozenList(inner) => TypeRef::Applied {
            name: collections::as_str(CollectionTypeId::FrozenList).to_string(),
            args: vec![type_ref_from_resolved(inner)],
        },
        ResolvedType::FrozenDict(key, value) => TypeRef::Applied {
            name: collections::as_str(CollectionTypeId::FrozenDict).to_string(),
            args: vec![type_ref_from_resolved(key), type_ref_from_resolved(value)],
        },
        ResolvedType::FrozenSet(inner) => TypeRef::Applied {
            name: collections::as_str(CollectionTypeId::FrozenSet).to_string(),
            args: vec![type_ref_from_resolved(inner)],
        },
        ResolvedType::Unit => named_type_ref(conventions::UNIT_TYPE_NAME),
        ResolvedType::Named(name) => named_type_ref(name.clone()),
        ResolvedType::Generic(name, args) => TypeRef::Applied {
            name: name.clone(),
            args: args.iter().map(type_ref_from_resolved).collect(),
        },
        ResolvedType::Function(params, return_type) => TypeRef::Function {
            params: params.iter().map(type_ref_from_resolved).collect(),
            return_type: Box::new(type_ref_from_resolved(return_type)),
        },
        ResolvedType::Tuple(elements) => TypeRef::Tuple {
            elements: elements.iter().map(type_ref_from_resolved).collect(),
        },
        ResolvedType::TypeVar(name) => TypeRef::TypeParam { name: name.clone() },
        ResolvedType::SelfType => TypeRef::SelfType,
        ResolvedType::Ref(inner) => TypeRef::Ref {
            inner: Box::new(type_ref_from_resolved(inner)),
        },
        ResolvedType::Unknown => TypeRef::Unknown,
    }
}

fn named_type_ref(name: impl Into<String>) -> TypeRef {
    TypeRef::Named { name: name.into() }
}

fn type_param_from_checked(type_param: &CheckedTypeParam) -> TypeParamExport {
    TypeParamExport {
        name: type_param.name.clone(),
        bounds: type_param.bounds.iter().map(type_bound_from_checked).collect(),
    }
}

fn type_bound_from_checked(bound: &CheckedTypeBound) -> TypeBoundExport {
    TypeBoundExport {
        name: bound.name.clone(),
        type_args: bound.type_args.iter().map(type_ref_from_resolved).collect(),
    }
}

fn params_from_checked(params: &[(String, ResolvedType)]) -> Vec<ParamExport> {
    params
        .iter()
        .map(|(name, ty)| ParamExport {
            name: name.clone(),
            ty: type_ref_from_resolved(ty),
        })
        .collect()
}

fn receiver_from_checked(receiver: Option<crate::frontend::ast::Receiver>) -> Option<ReceiverExport> {
    receiver.map(|value| match value {
        crate::frontend::ast::Receiver::Immutable => ReceiverExport::Immutable,
        crate::frontend::ast::Receiver::Mutable => ReceiverExport::Mutable,
    })
}

fn method_from_checked(method: &crate::frontend::library_exports::CheckedMethod) -> MethodExport {
    MethodExport {
        name: method.name.clone(),
        receiver: receiver_from_checked(method.receiver),
        params: params_from_checked(&method.params),
        return_type: type_ref_from_resolved(&method.return_type),
        is_async: method.is_async,
        has_body: method.has_body,
    }
}

fn field_from_checked(field: &crate::frontend::library_exports::CheckedField) -> FieldExport {
    FieldExport {
        name: field.name.clone(),
        ty: type_ref_from_resolved(&field.ty),
        has_default: field.has_default,
        alias: field.alias.clone(),
        description: field.description.clone(),
    }
}

fn function_export_from_checked(export: &CheckedFunctionExport) -> FunctionExport {
    FunctionExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        params: params_from_checked(&export.params),
        return_type: type_ref_from_resolved(&export.return_type),
        is_async: export.is_async,
    }
}

fn type_alias_export_from_checked(export: &CheckedTypeAliasExport) -> TypeAliasExport {
    TypeAliasExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        target: type_ref_from_resolved(&export.target),
    }
}

fn model_export_from_checked(export: &CheckedModelExport) -> ModelExport {
    ModelExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        traits: export.traits.clone(),
        fields: export.fields.iter().map(field_from_checked).collect(),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

fn class_export_from_checked(export: &CheckedClassExport) -> ClassExport {
    ClassExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        extends: export.extends.clone(),
        traits: export.traits.clone(),
        fields: export.fields.iter().map(field_from_checked).collect(),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

fn trait_export_from_checked(export: &CheckedTraitExport) -> TraitExport {
    TraitExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        requires: export
            .requires
            .iter()
            .map(|(name, ty)| FieldRequirementExport {
                name: name.clone(),
                ty: type_ref_from_resolved(ty),
            })
            .collect(),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

fn enum_export_from_checked(export: &CheckedEnumExport) -> EnumExport {
    EnumExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        variants: export
            .variants
            .iter()
            .map(|variant| EnumVariantExport {
                name: variant.name.clone(),
                fields: variant.fields.iter().map(type_ref_from_resolved).collect(),
            })
            .collect(),
    }
}

fn newtype_export_from_checked(export: &CheckedNewtypeExport) -> NewtypeExport {
    NewtypeExport {
        name: export.name.clone(),
        type_params: export.type_params.iter().map(type_param_from_checked).collect(),
        underlying: type_ref_from_resolved(&export.underlying),
        methods: export.methods.iter().map(method_from_checked).collect(),
    }
}

fn const_export_from_checked(export: &CheckedConstExport) -> ConstExport {
    ConstExport {
        name: export.name.clone(),
        ty: type_ref_from_resolved(&export.ty),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RawLibraryManifest {
    name: String,
    version: String,
    incan_version: String,
    manifest_format: u32,
    exports: RawLibraryExports,
    soft_keywords: RawSoftKeywordExports,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct RawLibraryExports {
    #[serde(default)]
    models: Vec<ModelExport>,
    #[serde(default)]
    classes: Vec<ClassExport>,
    #[serde(default)]
    functions: Vec<FunctionExport>,
    #[serde(default)]
    traits: Vec<TraitExport>,
    #[serde(default)]
    enums: Vec<EnumExport>,
    #[serde(default)]
    type_aliases: Vec<TypeAliasExport>,
    #[serde(default)]
    newtypes: Vec<NewtypeExport>,
    #[serde(default)]
    consts: Vec<ConstExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct RawSoftKeywordExports {
    #[serde(default)]
    activations: Vec<SoftKeywordActivation>,
}

impl RawLibraryManifest {
    fn from_semantic(semantic: &LibraryManifest) -> Self {
        Self {
            name: semantic.name.clone(),
            version: semantic.version.clone(),
            incan_version: semantic.incan_version.clone(),
            manifest_format: semantic.manifest_format,
            exports: RawLibraryExports {
                models: semantic.exports.models.clone(),
                classes: semantic.exports.classes.clone(),
                functions: semantic.exports.functions.clone(),
                traits: semantic.exports.traits.clone(),
                enums: semantic.exports.enums.clone(),
                type_aliases: semantic.exports.type_aliases.clone(),
                newtypes: semantic.exports.newtypes.clone(),
                consts: semantic.exports.consts.clone(),
            },
            soft_keywords: RawSoftKeywordExports {
                activations: semantic.soft_keywords.activations.clone(),
            },
        }
    }

    fn into_semantic(self) -> Result<LibraryManifest, LibraryManifestError> {
        Ok(LibraryManifest {
            name: self.name,
            version: self.version,
            incan_version: self.incan_version,
            manifest_format: self.manifest_format,
            exports: LibraryExports {
                models: self.exports.models,
                classes: self.exports.classes,
                functions: self.exports.functions,
                traits: self.exports.traits,
                enums: self.exports.enums,
                type_aliases: self.exports.type_aliases,
                newtypes: self.exports.newtypes,
                consts: self.exports.consts,
            },
            soft_keywords: SoftKeywordExports {
                activations: self.soft_keywords.activations,
            },
        })
    }

    fn validate(&self) -> Result<(), LibraryManifestError> {
        if self.manifest_format != LIBRARY_MANIFEST_FORMAT {
            return Err(LibraryManifestError::Invalid(format!(
                "unsupported manifest_format {} (expected {})",
                self.manifest_format, LIBRARY_MANIFEST_FORMAT
            )));
        }

        let manifest_version = Version::parse(&self.incan_version).map_err(|err| {
            LibraryManifestError::Invalid(format!("invalid `incan_version` value `{}`: {err}", self.incan_version))
        })?;
        let compiler_version = Version::parse(crate::version::INCAN_VERSION).map_err(|err| {
            LibraryManifestError::Invalid(format!(
                "invalid compiler version `{}`: {err}",
                crate::version::INCAN_VERSION
            ))
        })?;

        if manifest_version > compiler_version {
            return Err(LibraryManifestError::Invalid(format!(
                "manifest requires Incan {} but compiler is {}",
                manifest_version, compiler_version
            )));
        }

        for activation in &self.soft_keywords.activations {
            if activation.keyword.trim().is_empty() {
                return Err(LibraryManifestError::Invalid("soft keyword activation keyword cannot be empty".to_string()));
            }
            if activation.namespace.trim().is_empty() {
                return Err(LibraryManifestError::Invalid("soft keyword activation namespace cannot be empty".to_string()));
            }
            if let Some(id) = incan_core::lang::keywords::from_str(&activation.keyword) {
                if !incan_core::lang::keywords::is_soft(id) {
                    return Err(LibraryManifestError::Invalid(format!("keyword `{}` is not a soft keyword", activation.keyword)));
                }
            } else {
                return Err(LibraryManifestError::Invalid(format!("unknown soft keyword `{}`", activation.keyword)));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_io_round_trip_preserves_recursive_types_and_bounds() -> Result<(), Box<dyn std::error::Error>> {
        let mut manifest = LibraryManifest::new("mylib", "0.1.0");
        manifest.exports.functions.push(FunctionExport {
            name: "map_result".to_string(),
            type_params: vec![TypeParamExport {
                name: "T".to_string(),
                bounds: vec![TypeBoundExport {
                    name: "Clone".to_string(),
                    type_args: Vec::new(),
                }],
            }],
            params: vec![ParamExport {
                name: "value".to_string(),
                ty: TypeRef::Applied {
                    name: "Result".to_string(),
                    args: vec![
                        TypeRef::Applied {
                            name: "Option".to_string(),
                            args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                        },
                        TypeRef::Named {
                            name: "str".to_string(),
                        },
                    ],
                },
            }],
            return_type: TypeRef::Function {
                params: vec![TypeRef::Tuple {
                    elements: vec![
                        TypeRef::TypeParam { name: "T".to_string() },
                        TypeRef::Named {
                            name: "int".to_string(),
                        },
                    ],
                }],
                return_type: Box::new(TypeRef::Named {
                    name: "bool".to_string(),
                }),
            },
            is_async: false,
        });

        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("mylib.incnlib");
        manifest.write_to_path(&path)?;
        let loaded = LibraryManifest::read_from_path(&path)?;

        assert_eq!(loaded, manifest);
        Ok(())
    }

    #[test]
    fn manifest_reader_rejects_unknown_manifest_format() -> Result<(), Box<dyn std::error::Error>> {
        let content = r#"{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.1.0",
  "manifest_format": 999,
  "exports": {},
  "soft_keywords": {}
}"#;

        let err = LibraryManifest::from_json_str(content);
        assert!(err.is_err(), "expected invalid manifest_format to fail");
        Ok(())
    }

    #[test]
    fn manifest_reader_rejects_newer_required_compiler_version() -> Result<(), Box<dyn std::error::Error>> {
        let content = r#"{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "999.0.0",
  "manifest_format": 1,
  "exports": {},
  "soft_keywords": {}
}"#;

        let err = LibraryManifest::from_json_str(content);
        assert!(err.is_err(), "expected newer compiler requirement to fail");
        Ok(())
    }

    #[test]
    fn manifest_reader_rejects_invalid_soft_keyword() {
        let content = format!(
            r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.1.0",
  "manifest_format": {},
  "exports": {{}},
  "soft_keywords": {{
    "activations": [
      {{ "namespace": "mylib.dsl", "keyword": "not_a_real_keyword" }}
    ]
  }}
}}"#,
            LIBRARY_MANIFEST_FORMAT
        );
        let err = LibraryManifest::from_json_str(&content);
        assert!(matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("unknown soft keyword `not_a_real_keyword`")));
    }

    #[test]
    fn manifest_reader_rejects_hard_keyword_in_soft_keyword_activations() {
        let content = format!(
            r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.1.0",
  "manifest_format": {},
  "exports": {{}},
  "soft_keywords": {{
    "activations": [
      {{ "namespace": "mylib.dsl", "keyword": "def" }}
    ]
  }}
}}"#,
            LIBRARY_MANIFEST_FORMAT
        );
        let err = LibraryManifest::from_json_str(&content);
        assert!(matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("keyword `def` is not a soft keyword")));
    }
}
