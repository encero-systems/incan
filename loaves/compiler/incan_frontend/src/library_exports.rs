//! Checked library-export extraction helpers.
//!
//! These types represent the public API surface extracted from typechecked symbols, so later stages (like `.incnlib`
//! manifest writing) can consume a stable, semantic model.

use std::collections::HashMap;

use crate::ast::{
    AliasDecl, ClassDecl, Declaration, DictEntry, EnumDecl, Expr, FunctionDecl, ImportDecl, ImportItem, ImportKind,
    ListEntry, Literal, ModelDecl, NewtypeDecl, PartialDecl, Program, Span, Spanned, TraitBound, TraitDecl,
    TypeAliasDecl, TypeParam, UnaryOp, Visibility,
};
use crate::decorator_resolution;
use crate::diagnostics::{CompileError, errors};
use crate::module::{canonicalize_library_declaration_module_path, canonicalize_source_module_segments};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::{
    BindingRegistration, CallableParam, ClassInfo, FieldInfo, FunctionInfo, ImplementationTypeParamInfo, MethodInfo,
    ModelInfo, NewtypeInfo, PropertyInfo, ResolvedType, SymbolKind, TraitInfo, TypeBoundInfo, TypeInfo,
    ValueEnumBacking, ValueEnumValue, VariableInfo, register_binding, resolve_type,
};
use crate::typechecker::{PartialProjectionTargetKind, TypeChecker};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

pub(crate) mod declaration_exports;
mod members;
mod partials_and_defaults;
mod public_exports;

pub(crate) use declaration_exports::checked_function_export;
use declaration_exports::*;
pub use members::*;
use partials_and_defaults::*;
pub use public_exports::*;

/// Retain feature requirements for public names before projection removes inactive declarations. These records
/// explain an unavailable export; they grant no declaration identity or executable body to an inactive item.
pub fn gated_public_exports(
    program: &Program,
    module_path: &[String],
) -> Vec<crate::library_manifest::ProviderFactRequirement> {
    program
        .declarations
        .iter()
        .filter(|declaration| {
            !declaration.required_features.is_empty() && crate::typechecker::is_public_decl(declaration)
        })
        .filter_map(|declaration| {
            let name = crate::typechecker::declaration_name(declaration)?;
            Some(crate::library_manifest::ProviderFactRequirement {
                kind: crate::library_manifest::ProviderFactKind::Export,
                identity: module_path
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(name))
                    .collect::<Vec<_>>()
                    .join("::"),
                required_features: declaration.required_features.iter().cloned().collect(),
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
struct DefaultPathContext<'a> {
    checker: &'a TypeChecker,
    owner_module_path: Option<&'a [String]>,
    /// Whether a default that constructs one of the package's exported models or classes is carried to consumers.
    ///
    /// A consumer receives a function's or method's parameter default as its own construction of the type. A field
    /// default reaches a consumer only as a value of the constructor it fills, which has no construction form, so a
    /// field default that constructs a type is not carried.
    carries_constructions: bool,
}

impl<'a> DefaultPathContext<'a> {
    /// Build the default-expression path context for the checker currently exporting one source module.
    fn for_checker(checker: &'a TypeChecker) -> Self {
        Self {
            checker,
            owner_module_path: non_root_module_path(checker.current_module_path.as_deref()),
            carries_constructions: false,
        }
    }

    /// Build the path context for the parameter defaults of the functions and methods one source module exports.
    fn for_callable_parameters(checker: &'a TypeChecker) -> Self {
        Self {
            carries_constructions: true,
            ..Self::for_checker(checker)
        }
    }

    /// Return whether a callee expression names a model or class, whose call is a construction.
    fn names_model_or_class(self, callee: &Expr) -> bool {
        let Expr::Ident(name) = callee else {
            return false;
        };
        self.checker
            .lookup_symbol(name)
            .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Type(TypeInfo::Model(_) | TypeInfo::Class(_))))
    }

    /// Return whether the package's consumers can construct the model or class a callee expression names.
    ///
    /// A consumer constructs the type through the package's public path, which exists for a type this module declares
    /// `pub` and for one it imports from another module of the package. A private type, and a type of another
    /// package or of the stdlib, has no such path.
    fn names_exported_type(self, callee: &Expr) -> bool {
        let Expr::Ident(name) = callee else {
            return false;
        };
        match self.checker.import_binding_path(name) {
            Some(path) => !path_is_already_absolute(path),
            None => self.checker.declares_public(name),
        }
    }

    /// Return whether a default's call of `callee` can be carried to the package's consumers.
    ///
    /// A consumer calls a function or constructs a newtype through the package's path to it: one this module declares,
    /// a public symbol alias of one this module declares, or one it imports from another module of the package, named
    /// directly or, for a function, through that module (`helpers.scale`). A builtin such as `abs`, `len` or `Some`, a
    /// callable of another package or of the stdlib, and a partial, whose presets a consumer does not apply, have no
    /// such path. A member of a type, such as a static method or an enum variant with a payload, is left to the
    /// consumer's check, which refuses what it cannot materialize.
    fn call_is_carried(self, callee: &Spanned<Expr>) -> bool {
        match &callee.node {
            Expr::Ident(name) => {
                let identity = self.checker.type_info().resolved_identity(callee.span);
                if self.checker.type_info().partial_projection(name).is_some()
                    || identity.is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Partial)
                {
                    return false;
                }
                let symbol_kind = self.checker.lookup_symbol(name).map(|symbol| &symbol.kind);
                let is_function = matches!(
                    symbol_kind,
                    Some(SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_))
                ) || identity
                    .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Function);
                let is_newtype = matches!(symbol_kind, Some(SymbolKind::Type(TypeInfo::Newtype(_))));
                if !is_function && !is_newtype {
                    return false;
                }
                match self.checker.import_binding_path(name) {
                    Some(path) => !path_is_already_absolute(path),
                    None => {
                        is_newtype
                            || self.checker.local_function_decls.contains_key(name)
                            || (self.checker.declares_public(name)
                                && identity.is_some_and(|identity| self.declares(identity)))
                    }
                }
            }
            Expr::Field(base, _) => {
                let Expr::Ident(base_name) = &base.node else {
                    return false;
                };
                let names_module = self
                    .checker
                    .lookup_symbol(base_name)
                    .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Module(_)));
                if !names_module {
                    return true;
                }
                self.checker
                    .import_binding_path(base_name)
                    .is_some_and(|path| !path_is_already_absolute(path))
                    && self
                        .checker
                        .type_info()
                        .resolved_identity(callee.span)
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Function)
            }
            _ => false,
        }
    }

    /// Return whether `identity` names a declaration of the module being exported.
    fn declares(self, identity: &CanonicalSymbolId) -> bool {
        let module_path = match &identity.origin {
            SymbolOrigin::Module(module_path) | SymbolOrigin::Package { module_path, .. } => module_path,
            SymbolOrigin::RustCrate(_) | SymbolOrigin::Builtin => return false,
        };
        self.checker.current_module_path.as_deref() == Some(module_path.as_slice())
    }

    /// Resolve a default-expression value path to the module that owns it.
    fn canonical_value_path(self, path: Vec<String>) -> Vec<String> {
        let Some(first) = path.first() else {
            return path;
        };
        if let Some(imported_path) = self.checker.import_binding_path(first) {
            let mut canonical = imported_path.to_vec();
            canonical.extend(path.into_iter().skip(1));
            return canonical;
        }
        if path_is_already_absolute(&path) {
            return path;
        }
        let Some(owner_module_path) = self.owner_module_path else {
            return path;
        };
        if path.starts_with(owner_module_path) {
            return path;
        }
        let mut canonical = owner_module_path.to_vec();
        canonical.extend(path);
        canonical
    }
}

/// Return the non-root source module path that should qualify provider-owned default expressions.
fn non_root_module_path(path: Option<&[String]>) -> Option<&[String]> {
    let path = path?;
    if matches!(path, [segment] if segment == "main" || segment == "lib") {
        None
    } else {
        Some(path)
    }
}

/// Return whether a default-expression path is already rooted in a compiler-known namespace.
fn path_is_already_absolute(path: &[String]) -> bool {
    matches!(path.first().map(String::as_str), Some("std" | "rust" | "pub"))
}

/// Checked type parameter and its complete declared and inferred caller obligations.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedTypeParam {
    pub name: String,
    pub bounds: Vec<CheckedTypeBound>,
}

/// Resolved generic obligation, preserving owner provenance and implementation-only requirements for exact replay.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedTypeBound {
    pub name: String,
    pub source_name: Option<String>,
    pub type_args: Vec<ResolvedType>,
    pub module_path: Option<Vec<String>>,
    pub implementation_type_params: Vec<ImplementationTypeParamInfo>,
    /// Whether the checker inferred this `Eq` or `Hash` bound from the callable's body (#1758).
    pub inferred: bool,
}

#[derive(Debug, Clone)]
pub struct CheckedField {
    pub name: String,
    pub canonical: Option<CanonicalSymbolId>,
    /// Compiler-derived stored-record identity from the field's earliest declared name.
    pub record_identity: Vec<u8>,
    /// Whether this field participates in the canonical stored hash.
    pub record_canonical: bool,
    /// Earlier declared names in chronological order, earliest first.
    pub renamed_from: Vec<String>,
    /// Descriptive rename facts retained from the checked declaration.
    pub renamed_from_lifecycle: Option<incan_semantics_core::body_ir::LifecycleMetadata>,
    /// Whether the checked default is a statically empty collection literal.
    pub default_is_empty: bool,
    pub ty: ResolvedType,
    /// Source-level Incan type spelling retained for reflection across compiled-library boundaries.
    pub surface_type_name: Option<String>,
    pub visibility: Visibility,
    pub has_default: bool,
    pub default: Option<CheckedParamDefault>,
    pub alias: Option<String>,
    pub description: Option<String>,
    pub made_private: Option<crate::symbols::MadePrivateInfo>,
}

/// Public computed-property metadata preserved for compiled-library consumers.
#[derive(Debug, Clone)]
pub struct CheckedProperty {
    pub name: String,
    pub canonical: Option<CanonicalSymbolId>,
    pub return_type: ResolvedType,
}

#[derive(Debug, Clone)]
pub struct CheckedMethod {
    /// Checked explicit access and result-source metadata; absence leaves the ordinary inferred contract.
    pub ownership: Option<crate::ownership_metadata::CheckedOwnershipContract>,
    pub name: String,
    pub canonical: Option<CanonicalSymbolId>,
    /// Source-level method visibility (#2570); see `MethodInfo::visibility`.
    pub visibility: crate::ast::Visibility,
    pub alias_of: Option<String>,
    pub type_params: Vec<CheckedTypeParam>,
    pub receiver: Option<crate::ast::Receiver>,
    pub params: Vec<CallableParam>,
    pub param_defaults: Vec<Option<CheckedParamDefault>>,
    pub return_type: ResolvedType,
    pub is_async: bool,
    pub has_body: bool,
    pub made_private: Option<crate::symbols::MadePrivateInfo>,
}

/// Checked callable surface, including keyword binding and materializable defaults. Equality compares all retained
/// facts so a private codec can reject a lossy public-contract projection before publishing a reusable header.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedFunctionExport {
    /// Checked explicit access and result-source metadata; absence leaves the ordinary inferred contract.
    pub ownership: Option<crate::ownership_metadata::CheckedOwnershipContract>,
    pub name: String,
    pub emitted_name: Option<String>,
    pub type_params: Vec<CheckedTypeParam>,
    pub params: Vec<CallableParam>,
    pub param_defaults: Vec<Option<CheckedParamDefault>>,
    pub return_type: ResolvedType,
    pub is_async: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedExportIdentity {
    /// Provider-local declaration path that owns the semantic identity being exported.
    pub source_path: Vec<String>,
    /// Public projection layered on top of `source_path`, such as an alias, reexport, or partial preset.
    pub projection: CheckedExportProjection,
    /// Canonical declaration identity selected by the frontend, absent only when resolution stayed unproven.
    pub canonical: Option<CanonicalSymbolId>,
    /// Checked foreign nominal bindings available while this export was projected; only referenced type leaves
    /// are serialized, so unrelated implementation imports do not become artifact metadata.
    pub type_origins: std::collections::BTreeMap<String, crate::library_manifest::NominalTypeOriginExport>,
}

impl CheckedExportIdentity {
    /// Build identity metadata for a public export that exposes its own source declaration directly.
    pub fn direct(source_path: Vec<String>) -> Self {
        Self {
            source_path,
            projection: CheckedExportProjection::Direct,
            canonical: None,
            type_origins: Default::default(),
        }
    }

    /// Build identity metadata for a public alias that projects another source declaration or overload set.
    pub fn alias(source_path: Vec<String>, target_path: Vec<String>) -> Self {
        Self {
            source_path,
            projection: CheckedExportProjection::Alias { target_path },
            canonical: None,
            type_origins: Default::default(),
        }
    }

    /// Build identity metadata for a public reexport that preserves the target declaration identity through a facade.
    pub fn reexport(source_path: Vec<String>, target_path: Vec<String>) -> Self {
        Self {
            source_path,
            projection: CheckedExportProjection::Reexport { target_path },
            canonical: None,
            type_origins: Default::default(),
        }
    }

    /// Build identity metadata for a public partial preset that projects a callable or constructor target.
    pub fn partial(source_path: Vec<String>, target_path: Vec<String>, target_kind: CheckedPartialTargetKind) -> Self {
        Self {
            source_path,
            projection: CheckedExportProjection::Partial {
                target_path,
                target_kind,
            },
            canonical: None,
            type_origins: Default::default(),
        }
    }

    /// Attach the exact declaration identity proven for this public spelling.
    pub fn with_canonical(mut self, canonical: Option<CanonicalSymbolId>) -> Self {
        self.canonical = canonical;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedExportProjection {
    /// The public export exposes its own declaration directly.
    Direct,
    /// The public export is a source-level alias that preserves the target declaration identity.
    Alias {
        /// Provider-local declaration or overload-set path that the alias projects.
        target_path: Vec<String>,
    },
    /// The public export forwards another declaration through a facade without changing its identity.
    Reexport {
        /// Provider-local declaration path that the reexport projects.
        target_path: Vec<String>,
    },
    /// The public export is a partial preset over a callable or constructor target.
    Partial {
        /// Provider-local declaration or overload-set path that the partial preset projects.
        target_path: Vec<String>,
        /// Kind of target being projected, which lets consumers reconstruct the callable surface.
        target_kind: CheckedPartialTargetKind,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum CheckedParamDefault {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    List(Vec<CheckedParamDefault>),
    Dict(Vec<(CheckedParamDefault, CheckedParamDefault)>),
    ConstRef(Vec<String>),
    Call {
        path: Vec<String>,
        args: Vec<CheckedParamDefaultArg>,
        signature: Option<CheckedParamDefaultCallSignature>,
    },
    /// A static method call whose receiver resolves to an exported type.
    MethodCall {
        receiver_path: Vec<String>,
        method: String,
        args: Vec<CheckedParamDefaultArg>,
    },
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CheckedParamDefaultArg {
    pub name: Option<String>,
    pub value: CheckedParamDefault,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CheckedParamDefaultCallSignature {
    pub params: Vec<CallableParam>,
    pub return_type: ResolvedType,
}

#[derive(Debug, Clone)]
pub struct CheckedPartialExport {
    pub name: String,
    pub target_path: Vec<String>,
    pub target_kind: CheckedPartialTargetKind,
    pub presets: Vec<CheckedPartialPreset>,
    pub type_params: Vec<CheckedTypeParam>,
    pub params: Vec<CallableParam>,
    pub return_type: ResolvedType,
    pub is_async: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedPartialTargetKind {
    Function,
    ModelConstructor,
    ClassConstructor,
    NewtypeConstructor,
    Partial,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct CheckedPartialPreset {
    pub name: String,
    pub ty: ResolvedType,
    pub value: CheckedPresetValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CheckedPresetValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    List(Vec<CheckedPresetValue>),
    Set(Vec<CheckedPresetValue>),
    Tuple(Vec<CheckedPresetValue>),
    Dict(Vec<(CheckedPresetValue, CheckedPresetValue)>),
    ConstRef(Vec<String>),
    ModelLiteral {
        name: String,
        fields: Vec<(String, CheckedPresetValue)>,
    },
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct CheckedTypeAliasExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub target: ResolvedType,
}

#[derive(Debug, Clone)]
pub struct CheckedAliasExport {
    pub name: String,
    pub target_path: Vec<String>,
    pub projected_function: Option<CheckedFunctionExport>,
    /// Checked nominal target retained by a public type re-export.
    pub projected_type: Option<ResolvedType>,
}

#[derive(Debug, Clone)]
pub struct CheckedModelExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<CheckedTypeBound>,
    /// `@derive(...)` names that must remain available to `pub::` consumers.
    pub derives: Vec<String>,
    pub fields: Vec<CheckedField>,
    pub properties: Vec<CheckedProperty>,
    pub methods: Vec<CheckedMethod>,
}

#[derive(Debug, Clone)]
pub struct CheckedClassExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub extends: Option<String>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<CheckedTypeBound>,
    /// `@derive(...)` names that must remain available to `pub::` consumers.
    pub derives: Vec<String>,
    pub fields: Vec<CheckedField>,
    pub properties: Vec<CheckedProperty>,
    pub methods: Vec<CheckedMethod>,
}

#[derive(Debug, Clone)]
pub struct CheckedTraitExport {
    pub name: String,
    pub source_name: String,
    pub type_params: Vec<CheckedTypeParam>,
    /// Direct supertraits after typechecking (RFC 042), including resolved owner module metadata.
    pub supertraits: Vec<CheckedTypeBound>,
    pub requires: Vec<(String, ResolvedType)>,
    pub methods: Vec<CheckedMethod>,
}

#[derive(Debug, Clone)]
pub struct CheckedEnumVariant {
    pub name: String,
    pub canonical: Option<CanonicalSymbolId>,
    pub fields: Vec<ResolvedType>,
    pub value: Option<ValueEnumValue>,
}

#[derive(Debug, Clone)]
pub struct CheckedEnumVariantAlias {
    pub name: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct CheckedEnumExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<CheckedTypeBound>,
    pub value_type: Option<ValueEnumBacking>,
    pub variants: Vec<CheckedEnumVariant>,
    pub variant_aliases: Vec<CheckedEnumVariantAlias>,
    pub methods: Vec<CheckedMethod>,
    pub derives: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CheckedNewtypeExport {
    pub name: String,
    pub type_params: Vec<CheckedTypeParam>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<CheckedTypeBound>,
    /// `@derive(...)` names that must remain available to `pub::` consumers.
    pub derives: Vec<String>,
    pub is_rusttype: bool,
    pub underlying: ResolvedType,
    pub checked_constructor: Option<String>,
    pub constraints: Vec<crate::symbols::NewtypePrimitiveConstraint>,
    pub implicit_coercion_enabled: bool,
    pub methods: Vec<CheckedMethod>,
}

#[derive(Debug, Clone)]
pub struct CheckedConstExport {
    pub name: String,
    pub ty: ResolvedType,
}

#[derive(Debug, Clone)]
pub struct CheckedStaticExport {
    pub name: String,
    pub ty: ResolvedType,
}

#[derive(Debug, Clone)]
pub enum CheckedExportKind {
    Function(CheckedFunctionExport),
    Partial(CheckedPartialExport),
    Alias(CheckedAliasExport),
    TypeAlias(CheckedTypeAliasExport),
    Model(CheckedModelExport),
    Class(CheckedClassExport),
    Trait(CheckedTraitExport),
    Enum(CheckedEnumExport),
    Newtype(CheckedNewtypeExport),
    Const(CheckedConstExport),
    Static(CheckedStaticExport),
}

#[derive(Debug, Clone)]
pub struct CheckedNamedExport {
    pub name: String,
    pub identity: CheckedExportIdentity,
    pub kind: CheckedExportKind,
}

impl CheckedNamedExport {
    /// Rename this export for the public name a re-export binds it to, preserving its semantic export kind.
    ///
    /// Every kind carries its own copy of the name, so the rename is applied kind by kind here, beside the enum, and
    /// a new kind cannot be added without deciding what renaming means for it. The callable an alias projects is
    /// renamed with the alias: the projection describes the binding a consumer resolves under this public name.
    /// Only `emitted_name` stays put, because the declaration behind the rename is unchanged.
    pub fn renamed(&self, exported_name: &str) -> Self {
        let mut renamed = self.clone();
        renamed.name = exported_name.to_string();

        match &mut renamed.kind {
            CheckedExportKind::Function(function_export) => function_export.name = exported_name.to_string(),
            CheckedExportKind::Partial(partial_export) => partial_export.name = exported_name.to_string(),
            CheckedExportKind::Alias(alias_export) => {
                alias_export.name = exported_name.to_string();
                if let Some(projected_function) = alias_export.projected_function.as_mut() {
                    projected_function.name = exported_name.to_string();
                }
            }
            CheckedExportKind::TypeAlias(type_alias_export) => type_alias_export.name = exported_name.to_string(),
            CheckedExportKind::Model(model_export) => model_export.name = exported_name.to_string(),
            CheckedExportKind::Class(class_export) => class_export.name = exported_name.to_string(),
            CheckedExportKind::Trait(trait_export) => trait_export.name = exported_name.to_string(),
            CheckedExportKind::Enum(enum_export) => enum_export.name = exported_name.to_string(),
            CheckedExportKind::Newtype(newtype_export) => newtype_export.name = exported_name.to_string(),
            CheckedExportKind::Const(const_export) => const_export.name = exported_name.to_string(),
            CheckedExportKind::Static(static_export) => static_export.name = exported_name.to_string(),
        }

        renamed
    }

    /// Project this provider export through the entrypoint binding that actually re-exports it.
    ///
    /// The provider export retains the concrete declaration shape (model, trait, function, and so on), while the
    /// entrypoint's checked export owns the re-export path and target identity. Combining those two checked products
    /// avoids relabeling a renamed declaration as a direct export whose public name no longer matches its canonical
    /// declaration. The entrypoint candidate is matched by canonical identity; a lone candidate under the name is
    /// adopted without one, and with no candidate at all the renamed provider identity stands.
    pub fn projected_through_reexport(
        &self,
        exported_name: &str,
        entrypoint_exports: Option<&HashMap<String, Vec<CheckedNamedExport>>>,
    ) -> Self {
        let mut projected = self.renamed(exported_name);
        let Some(candidates) = entrypoint_exports.and_then(|exports| exports.get(exported_name)) else {
            return projected;
        };
        let checked_projection = candidates
            .iter()
            .find(|candidate| candidate.identity.canonical == self.identity.canonical)
            .or_else(|| (candidates.len() == 1).then(|| &candidates[0]));
        if let Some(checked_projection) = checked_projection {
            projected.identity = checked_projection.identity.clone();
        }
        projected
    }
}

/// Group checked exports by public source name while preserving same-name function overload entries.
pub fn checked_exports_by_name(exports: Vec<CheckedNamedExport>) -> HashMap<String, Vec<CheckedNamedExport>> {
    let mut grouped: HashMap<String, Vec<CheckedNamedExport>> = HashMap::new();
    for export in exports {
        grouped.entry(export.name.clone()).or_default().push(export);
    }
    grouped
}

/// Frontend-owned registry for names projected through a library entrypoint.
///
/// Library reexport resolution still needs module-graph facts supplied by the build pipeline, but whether a projected
/// name collides — and which diagnostic that collision produces — is a frontend binding decision. Keeping the state
/// and diagnostic here prevents the CLI from maintaining a second collision rule beside the symbol resolver.
#[derive(Debug, Default)]
pub struct LibraryExportBindingRegistry {
    bindings: HashMap<String, Span>,
}

impl LibraryExportBindingRegistry {
    /// Register one projected public name, preserving the first source site on collision.
    pub fn register(&mut self, name: &str, span: Span) -> Result<(), CompileError> {
        match register_binding(&mut self.bindings, name.to_string(), span) {
            BindingRegistration::Registered => Ok(()),
            BindingRegistration::Collision { existing } => Err(errors::duplicate_library_export(name, existing, span)),
        }
    }
}

#[cfg(test)]
mod tests;
