//! Typed Intermediate Representation (IR)
//!
//! This module defines a typed IR that sits between the Incan AST and Rust code
//! generation. The IR is:
//!
//! - **Typed**: Every expression carries its resolved type
//! - **Ownership-aware**: Tracks borrow, move, and copy semantics
//! - **Rust-oriented**: Closer to Rust's semantics than the Incan AST
//!
//! ## Pipeline
//!
//! ```text
//! Incan source → AST → Typechecker → IR → Rust Code
//! ```
//!
//! ## Benefits
//!
//! 1. Type information is available during codegen without re-analysis
//! 2. Ownership decisions are made once during lowering
//! 3. The IR can be validated independently
//! 4. Potential future backends (LLVM, WASM, etc.) can target IR instead of AST

pub mod conversions;
pub mod ownership;
pub mod prelude;
pub(crate) mod reference_shape;

pub mod codegen;
pub mod decl;
pub mod emit;
pub mod emit_service;
pub mod expr;
pub mod facade;
pub mod lower;
pub mod scanners;
pub mod stmt;
pub mod surface_semantics;
pub mod trait_bound_inference;
pub mod types;

pub use codegen::{GenerationError, IrCodegen};
pub use decl::{FunctionParam, IrDecl, IrDeclKind, IrFunction, IrStruct};
pub use emit::{EmitError, IrEmitter};
pub use emit_service::EmitService;
pub use expr::{BuiltinFn, IrExpr, IrExprKind, MethodKind, TypedExpr};
pub use facade::CodegenFacade;
pub use lower::{AstLowering, LoweringError, LoweringErrors};
pub use scanners::{check_for_this_import, collect_rust_crates, detect_serde_non_import_usage, detect_serde_usage};
pub use stmt::{IrStmt, IrStmtKind};
pub use types::{IrType, Mutability, Ownership};

use crate::frontend::ast::Span;
use incan_core::lang::c_abi::{LinkCapabilityId, ScalarTypeId};
use incan_semantics_core::{CanonicalSymbolId, encode_incan_symbol_identity};
use std::collections::HashMap;

/// Function signature for call-site type checking
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub params: Vec<FunctionParam>,
    pub return_type: IrType,
}

impl FunctionSignature {
    /// Build a positional callable signature from a lowered function type.
    pub fn from_function_type(params: &[IrType], ret: &IrType) -> Self {
        Self {
            params: params
                .iter()
                .enumerate()
                .map(|(idx, ty)| FunctionParam {
                    name: format!("__incan_arg_{idx}"),
                    ty: ty.clone(),
                    mutability: Mutability::Immutable,
                    is_self: false,
                    kind: crate::frontend::ast::ParamKind::Normal,
                    default: None,
                })
                .collect(),
            return_type: ret.clone(),
        }
    }

    /// Return the effective call signature when one source carries precise callable type metadata and another carries
    /// source defaults for the same callable surface.
    pub fn merge_default_source(
        primary: Option<&FunctionSignature>,
        default_source: Option<&FunctionSignature>,
    ) -> Option<Self> {
        Self::merge_default_source_by(primary, default_source, |left, right| left == right)
    }

    /// Return the effective call signature using a caller-supplied type equivalence rule for default inheritance.
    pub fn merge_default_source_by(
        primary: Option<&FunctionSignature>,
        default_source: Option<&FunctionSignature>,
        types_match: impl Fn(&IrType, &IrType) -> bool,
    ) -> Option<Self> {
        let Some(primary) = primary else {
            return default_source.cloned();
        };
        let Some(default_source) = default_source else {
            return Some(primary.clone());
        };
        let mut merged = primary.clone();
        if Self::params_match_for_default_inheritance(primary, default_source, &types_match) {
            for (param, default_param) in merged.params.iter_mut().zip(&default_source.params) {
                if param.default.is_none() {
                    param.default = default_param.default.clone();
                }
                // Typechecker callable metadata captures Rust callable shape, but source declarations own the
                // mutable-parameter ABI selected by the emitter. Retain that source fact so a borrowed Rust
                // callback value is forwarded instead of cloned at a source-owned call boundary.
                param.mutability = default_param.mutability;
            }
        }
        Some(merged)
    }

    /// Return whether parameter lists are compatible for default inheritance.
    fn params_match_for_default_inheritance(
        left: &FunctionSignature,
        right: &FunctionSignature,
        types_match: &impl Fn(&IrType, &IrType) -> bool,
    ) -> bool {
        left.params.len() == right.params.len()
            && left
                .params
                .iter()
                .zip(&right.params)
                .all(|(left, right)| Self::param_matches_for_default_inheritance(left, right, types_match))
    }

    /// Return whether one parameter is compatible for default inheritance.
    fn param_matches_for_default_inheritance(
        left: &FunctionParam,
        right: &FunctionParam,
        types_match: &impl Fn(&IrType, &IrType) -> bool,
    ) -> bool {
        left.kind == right.kind
            && types_match(&left.ty, &right.ty)
            && (left.name == right.name
                || left.name.starts_with("__incan_arg_")
                || right.name.starts_with("__incan_arg_"))
    }
}

/// Registry of all function signatures in the program
#[derive(Debug, Clone, Default)]
pub struct FunctionRegistry {
    /// Map from function name to its signature
    signatures: HashMap<String, FunctionSignature>,
    /// Source declaration spelling for each emitted/local registry key.
    source_names: HashMap<String, String>,
    /// Compiler-owned identity for each backend registry key that projects an Incan declaration.
    canonical_identities: HashMap<String, CanonicalSymbolId>,
    /// Exact compiler-created projection-to-registry-key relationship.
    ///
    /// This is intentionally not populated by decoding generated names. Semantic consumers can follow this map only
    /// because lowering inserted both sides from the same canonical identity.
    projection_targets: HashMap<String, String>,
    /// Physical Rust identifiers for compiler-generated functions whose registry keys are deliberately not source
    /// identifiers.
    ///
    /// Generated declarations use collision-proof internal keys so a valid Incan declaration can never overwrite
    /// their registry metadata merely by matching a hidden helper spelling. Emission follows this compiler-created
    /// map; semantic stages never recover meaning from the physical name.
    generated_physical_names: HashMap<String, String>,
}

impl FunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the registry key used for a canonical module path such as `helpers.normalize`.
    pub fn canonical_key(path: &[String]) -> Option<String> {
        if path.len() < 2 {
            return None;
        }
        Some(path.join("::"))
    }

    /// Register a function signature
    pub fn register(&mut self, name: String, params: Vec<FunctionParam>, return_type: IrType) {
        self.source_names.entry(name.clone()).or_insert_with(|| name.clone());
        self.signatures.insert(name, FunctionSignature { params, return_type });
    }

    /// Register an emitted projection while retaining its compiler-known source declaration spelling.
    pub fn register_projection(
        &mut self,
        emitted_name: String,
        source_name: String,
        params: Vec<FunctionParam>,
        return_type: IrType,
    ) {
        self.source_names.insert(emitted_name.clone(), source_name);
        self.signatures
            .insert(emitted_name, FunctionSignature { params, return_type });
    }

    /// Register a compiler-generated function under a collision-proof internal key and retain its physical Rust name.
    pub fn register_generated(
        &mut self,
        registry_name: String,
        physical_name: String,
        source_name: String,
        params: Vec<FunctionParam>,
        return_type: IrType,
    ) {
        self.source_names.insert(registry_name.clone(), source_name);
        self.generated_physical_names
            .insert(registry_name.clone(), physical_name);
        self.signatures
            .insert(registry_name, FunctionSignature { params, return_type });
    }

    /// Register an Incan-origin function projection from its compiler-owned canonical identity.
    pub fn register_canonical_projection(
        &mut self,
        registry_name: String,
        source_name: String,
        identity: CanonicalSymbolId,
        params: Vec<FunctionParam>,
        return_type: IrType,
    ) {
        let projection = encode_incan_symbol_identity(&identity);
        self.source_names.insert(registry_name.clone(), source_name);
        self.canonical_identities.insert(registry_name.clone(), identity);
        self.projection_targets.insert(projection, registry_name.clone());
        self.signatures
            .insert(registry_name, FunctionSignature { params, return_type });
    }

    /// Register a function signature under its canonical module path.
    pub fn register_canonical_path(&mut self, path: &[String], params: Vec<FunctionParam>, return_type: IrType) {
        if let Some(key) = Self::canonical_key(path) {
            self.register(key, params, return_type);
        }
    }

    /// Register a canonical module path while preserving the source declaration identity that path projects.
    pub fn register_canonical_path_projection(
        &mut self,
        path: &[String],
        source_name: String,
        identity: CanonicalSymbolId,
        params: Vec<FunctionParam>,
        return_type: IrType,
    ) {
        if let Some(key) = Self::canonical_key(path) {
            self.register_canonical_projection(key, source_name, identity, params, return_type);
        }
    }

    /// Look up a function signature by name
    pub fn get(&self, name: &str) -> Option<&FunctionSignature> {
        self.signatures.get(name).or_else(|| {
            self.projection_targets
                .get(name)
                .and_then(|target| self.signatures.get(target))
        })
    }

    /// Return the source declaration spelling retained for one registry key.
    pub fn source_name(&self, name: &str) -> Option<&str> {
        self.source_names
            .get(name)
            .or_else(|| {
                self.projection_targets
                    .get(name)
                    .and_then(|target| self.source_names.get(target))
            })
            .map(String::as_str)
    }

    /// Return the exact registry key paired with an emitted projection created by lowering.
    pub fn registry_key<'a>(&'a self, name: &'a str) -> &'a str {
        self.projection_targets.get(name).map(String::as_str).unwrap_or(name)
    }

    /// Return the compiler-owned identity retained for one registry key or its exact emitted projection.
    pub fn canonical_identity(&self, name: &str) -> Option<&CanonicalSymbolId> {
        self.canonical_identities.get(name).or_else(|| {
            self.projection_targets
                .get(name)
                .and_then(|target| self.canonical_identities.get(target))
        })
    }

    /// Return the emitted Rust projection selected for a registered Incan declaration.
    pub fn emitted_projection(&self, name: &str) -> Option<String> {
        self.canonical_identity(name).map(encode_incan_symbol_identity)
    }

    /// Return the exact compiler-selected physical Rust name for a generated function.
    pub fn generated_physical_name(&self, name: &str) -> Option<&str> {
        self.generated_physical_names.get(name).map(String::as_str)
    }

    /// Look up a function signature by canonical module path.
    pub fn get_canonical_path(&self, path: &[String]) -> Option<&FunctionSignature> {
        let key = Self::canonical_key(path)?;
        self.get(&key)
    }

    /// Return the canonical identity registered for a canonical module path.
    pub fn canonical_identity_for_path(&self, path: &[String]) -> Option<&CanonicalSymbolId> {
        let key = Self::canonical_key(path)?;
        self.canonical_identity(&key)
    }

    /// Iterate over registered function signatures.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &FunctionSignature)> {
        self.signatures.iter()
    }

    /// Merge another registry into this one
    pub fn merge(&mut self, other: &FunctionRegistry) {
        for (name, sig) in &other.signatures {
            self.signatures.insert(name.clone(), sig.clone());
        }
        for (name, source_name) in &other.source_names {
            self.source_names.insert(name.clone(), source_name.clone());
        }
        for (name, identity) in &other.canonical_identities {
            self.canonical_identities.insert(name.clone(), identity.clone());
        }
        for (projection, target) in &other.projection_targets {
            self.projection_targets.insert(projection.clone(), target.clone());
        }
        for (name, physical_name) in &other.generated_physical_names {
            self.generated_physical_names
                .insert(name.clone(), physical_name.clone());
        }
    }

    /// Resolve the effective function-call signature for one IR call site.
    ///
    /// This is the single merge point for callable metadata during emission. Typechecker/lowering metadata can carry a
    /// precise callable surface, while the source registry can carry default expressions. Canonical paths resolve
    /// through the cross-module registry, local names resolve through the module registry, and lowered function types
    /// are only a final fallback.
    pub fn effective_call_signature(
        local_registry: &FunctionRegistry,
        canonical_registry: &FunctionRegistry,
        local_name: Option<&str>,
        canonical_path: Option<&[String]>,
        callable_signature: Option<&FunctionSignature>,
        callee_ty: Option<&IrType>,
    ) -> Option<FunctionSignature> {
        Self::effective_call_signature_by(
            local_registry,
            canonical_registry,
            local_name,
            canonical_path,
            callable_signature,
            callee_ty,
            |left, right| left == right,
        )
    }

    /// Resolve the effective function-call signature using a caller-supplied type equivalence rule.
    pub fn effective_call_signature_by(
        local_registry: &FunctionRegistry,
        canonical_registry: &FunctionRegistry,
        local_name: Option<&str>,
        canonical_path: Option<&[String]>,
        callable_signature: Option<&FunctionSignature>,
        callee_ty: Option<&IrType>,
        types_match: impl Fn(&IrType, &IrType) -> bool,
    ) -> Option<FunctionSignature> {
        let registry_signature = if let Some(path) = canonical_path {
            canonical_registry.get_canonical_path(path)
        } else {
            local_name.and_then(|name| local_registry.get(name))
        };
        FunctionSignature::merge_default_source_by(callable_signature, registry_signature, types_match).or_else(|| {
            match callee_ty {
                Some(IrType::Function { params, ret }) => Some(FunctionSignature::from_function_type(params, ret)),
                _ => None,
            }
        })
    }
}

/// Public source import re-export that should behave like the imported callable for metadata lookups.
#[derive(Debug, Clone)]
pub struct FunctionReexport {
    pub name: String,
    pub target_path: Vec<String>,
}

/// Construction semantics for one local newtype.
///
/// Production codegen carries this from checked frontend metadata. Direct AST-lowering tests may construct a
/// conservative fallback plan when no `TypeCheckInfo` is available.
#[derive(Debug, Clone)]
pub struct IrNewtypeConstructionPlan {
    /// Declared generic parameters, including source bounds.
    pub type_params: Vec<decl::IrTypeParam>,
    /// Wrapped runtime value type after frontend resolution.
    pub underlying: IrType,
    /// Canonical validation hook selected by the checked frontend or conservative fallback.
    pub checked_constructor: Option<String>,
    /// Generated primitive predicates used when no explicit validation hook exists.
    pub constraints: Vec<crate::frontend::symbols::NewtypePrimitiveConstraint>,
    /// Whether ordinary implicit underlying-to-newtype coercion is enabled.
    pub implicit_coercion_enabled: bool,
    /// Whether the available construction plan supports `TryFrom[str]` composition for this newtype.
    pub supports_string_conversion: bool,
}

/// One contained checked-C carrier used by an emitted raw binding wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrCheckedCType {
    /// One exact C scalar category represented by the ordinary Incan integer carrier.
    Scalar(ScalarTypeId),
    /// A checked raw pointer whose pointee contract remains distinct from an integer address.
    Pointer {
        /// Whether native code may mutate the pointed-to value.
        mutable: bool,
        /// Exact checked pointee contract.
        pointee: Box<IrCheckedCType>,
    },
    /// An opaque resource passed by value, shared borrow, or exclusive borrow.
    Resource {
        /// Call-site ownership relationship declared by the binding.
        access: crate::frontend::typechecker::CResourceAccess,
        /// Binding-local resource name.
        resource: String,
    },
    /// Compiler-managed output storage passed as one mutable ABI position.
    Output {
        /// Initialization contract for the storage.
        mode: crate::frontend::typechecker::COutputMode,
        /// Exact C value carried by the storage.
        value: Box<IrCheckedCType>,
    },
    /// A nullable return value.
    Nullable(Box<IrCheckedCType>),
    /// A C `void` return.
    Void,
}

/// One binding-local opaque resource required by the emitted raw-call wrappers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrCheckedCResource {
    /// Binding class that scopes the resource name.
    pub binding: String,
    /// Binding-local resource name.
    pub resource: String,
    /// Native release symbol selected by the checked descriptor.
    pub release_native_symbol: String,
    /// Exact result carrier of the declared release symbol.
    pub release_return_type: IrCheckedCType,
    /// Logical system-library capability selected by the checked binding.
    pub system_library: String,
    /// Exact native linker form selected by the checked binding declaration.
    pub link_capability: LinkCapabilityId,
}

/// One source-checked C function that lowering has authorized for direct emission.
///
/// The typechecker owns the full binding descriptor. This IR record contains the bounded scalar, resource, and output
/// subset selected at a checked call site, so the Rust emitter never rediscovers a C signature from source syntax or
/// a header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrCheckedCFunction {
    /// Source binding class that owns this function.
    pub binding: String,
    /// Source member name within the binding.
    pub symbol: String,
    /// Exact C linker symbol verified from the binding descriptor.
    pub native_symbol: String,
    /// Logical system library selected by the checked binding.
    pub system_library: String,
    /// Exact native linker form selected by the checked binding declaration.
    pub link_capability: LinkCapabilityId,
    /// Exact checked parameters in declaration order.
    pub parameters: Vec<IrCheckedCType>,
    /// Source parameter names paired positionally with `parameters`.
    pub parameter_names: Vec<String>,
    /// Exact checked return contract.
    pub return_type: IrCheckedCType,
    /// Opaque resource release facts scoped by this binding.
    pub resources: Vec<IrCheckedCResource>,
}

impl IrCheckedCFunction {
    /// Encode one source identifier component without allowing two spellings to collide.
    fn component(value: &str) -> String {
        value.bytes().fold(String::new(), |mut encoded, byte| {
            if byte.is_ascii_alphanumeric() {
                encoded.push(byte.into());
            } else {
                encoded.push('_');
                encoded.push_str(&format!("{byte:02x}"));
            }
            encoded
        })
    }

    /// Deterministic compiler-private Rust wrapper name.
    pub fn rust_name(&self) -> String {
        format!(
            "__incan_c_{}__{}",
            Self::component(&self.binding),
            Self::component(&self.symbol)
        )
    }

    /// Deterministic compiler-private FFI declaration name.
    pub fn ffi_rust_name(&self) -> String {
        format!("{}__ffi", self.rust_name())
    }

    /// Deterministic private nominal wrapper for one owned opaque resource.
    pub fn resource_rust_type_name(binding: &str, resource: &str) -> String {
        format!(
            "__incan_c_resource_{}__{}",
            Self::component(binding),
            Self::component(resource)
        )
    }

    /// Deterministic private storage carrier for one checked C output parameter.
    pub fn output_slot_rust_type_name(binding: &str, symbol: &str, parameter: &str) -> String {
        format!(
            "__incan_c_output_{}__{}__{}",
            Self::component(binding),
            Self::component(symbol),
            Self::component(parameter)
        )
    }
}

/// A complete IR program
#[derive(Debug, Clone)]
pub struct IrProgram {
    /// Top-level declarations
    pub declarations: Vec<IrDecl>,
    /// Compiler-owned initialisation statements that run after this module's statics have been constructed.
    ///
    /// This is intentionally distinct from declaration initialisers: a statement here may mutate an already-created
    /// static through the same storage semantics that source method calls use.
    pub module_init: Vec<IrStmt>,
    /// Source module path for this program when known.
    pub source_module_name: Option<String>,
    /// Entry point function name (usually "main")
    pub entry_point: Option<String>,
    /// Function signature registry for call-site type checking
    pub function_registry: FunctionRegistry,
    /// Public source-function re-exports keyed by local exported name and canonical target path.
    pub function_reexports: Vec<FunctionReexport>,
    /// RFC 023: The `rust.module("path::to::module")` Rust backing path, if declared.
    ///
    /// When present, `@rust.extern` functions in this program emit delegation calls to this Rust module path instead
    /// of compiling their Incan bodies. See RFC 023 for full design.
    pub rust_module_path: Option<String>,
    /// Construction plans keyed by local newtype name; production entries come from checked frontend metadata.
    pub newtype_construction: std::collections::HashMap<String, IrNewtypeConstructionPlan>,
    /// Checked C functions selected by source calls in this module.
    pub checked_c_functions: Vec<IrCheckedCFunction>,
    /// Whether this module uses compiler-private checked C string temporaries.
    pub uses_checked_c_strings: bool,
    /// Whether this module copies returned scoped C string views through the bounded compiler-private helper.
    pub uses_scoped_c_string_views: bool,
    /// Whether this module finishes caller-owned checked byte buffers through the bounded compiler-private helper.
    pub uses_checked_c_span_buffers: bool,
}

impl IrProgram {
    /// Create an empty IR program with no declarations and default metadata.
    pub fn new() -> Self {
        Self {
            declarations: Vec::new(),
            module_init: Vec::new(),
            source_module_name: None,
            entry_point: None,
            function_registry: FunctionRegistry::new(),
            function_reexports: Vec::new(),
            rust_module_path: None,
            newtype_construction: std::collections::HashMap::new(),
            checked_c_functions: Vec::new(),
            uses_checked_c_strings: false,
            uses_scoped_c_string_views: false,
            uses_checked_c_span_buffers: false,
        }
    }
}

impl Default for IrProgram {
    fn default() -> Self {
        Self::new()
    }
}

/// Span information preserved from AST
#[derive(Debug, Clone, Copy, Default)]
pub struct IrSpan {
    pub start: usize,
    pub end: usize,
}

impl From<Span> for IrSpan {
    fn from(span: Span) -> Self {
        Self {
            start: span.start,
            end: span.end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FunctionParam, FunctionRegistry, FunctionSignature, IrCheckedCFunction, IrCheckedCType, IrType, Mutability,
        ScalarTypeId,
    };
    use incan_core::lang::c_abi::LinkCapabilityId;

    fn checked_c_function(binding: &str, symbol: &str) -> IrCheckedCFunction {
        IrCheckedCFunction {
            binding: binding.to_string(),
            symbol: symbol.to_string(),
            native_symbol: "fixture".to_string(),
            system_library: "fixture".to_string(),
            link_capability: LinkCapabilityId::SystemLibrary,
            parameters: vec![IrCheckedCType::Scalar(ScalarTypeId::I32)],
            parameter_names: vec!["value".to_string()],
            return_type: IrCheckedCType::Scalar(ScalarTypeId::I32),
            resources: Vec::new(),
        }
    }

    #[test]
    fn checked_c_wrapper_names_do_not_merge_component_boundaries() {
        let left = checked_c_function("Fixture_", "symbol");
        let right = checked_c_function("Fixture", "_symbol");

        assert_ne!(left.rust_name(), right.rust_name());
        assert_ne!(left.ffi_rust_name(), right.ffi_rust_name());
    }

    #[test]
    fn function_registry_retains_source_identity_without_parsing_the_emitted_name() {
        let mut registry = FunctionRegistry::new();
        registry.register_projection(
            "opaque_backend_projection".to_string(),
            "calculate".to_string(),
            Vec::new(),
            IrType::Int,
        );
        registry.register("opaque_backend_projection".to_string(), Vec::new(), IrType::Int);

        assert_eq!(registry.source_name("opaque_backend_projection"), Some("calculate"));
    }

    #[test]
    fn merged_source_signature_keeps_mutable_parameter_abi() -> Result<(), Box<dyn std::error::Error>> {
        let checked = FunctionSignature {
            params: vec![FunctionParam {
                name: "ui".to_string(),
                ty: IrType::Struct("Ui".to_string()),
                mutability: Mutability::Immutable,
                is_self: false,
                kind: crate::frontend::ast::ParamKind::Normal,
                default: None,
            }],
            return_type: IrType::Unknown,
        };
        let source = FunctionSignature {
            params: vec![FunctionParam {
                name: "ui".to_string(),
                ty: IrType::Struct("Ui".to_string()),
                mutability: Mutability::Mutable,
                is_self: false,
                kind: crate::frontend::ast::ParamKind::Normal,
                default: None,
            }],
            return_type: IrType::Unit,
        };

        let merged = FunctionSignature::merge_default_source(Some(&checked), Some(&source))
            .ok_or("both signatures should merge")?;
        assert_eq!(merged.params[0].mutability, Mutability::Mutable);
        Ok(())
    }
}
