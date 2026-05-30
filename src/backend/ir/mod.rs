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
        self.signatures.insert(name, FunctionSignature { params, return_type });
    }

    /// Register a function signature under its canonical module path.
    pub fn register_canonical_path(&mut self, path: &[String], params: Vec<FunctionParam>, return_type: IrType) {
        if let Some(key) = Self::canonical_key(path) {
            self.register(key, params, return_type);
        }
    }

    /// Look up a function signature by name
    pub fn get(&self, name: &str) -> Option<&FunctionSignature> {
        self.signatures.get(name)
    }

    /// Look up a function signature by canonical module path.
    pub fn get_canonical_path(&self, path: &[String]) -> Option<&FunctionSignature> {
        let key = Self::canonical_key(path)?;
        self.signatures.get(&key)
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

/// A complete IR program
#[derive(Debug, Clone)]
pub struct IrProgram {
    /// Top-level declarations
    pub declarations: Vec<IrDecl>,
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
    /// Newtype -> selected checked constructor method.
    ///
    /// Backend-generated code uses this when it must construct a newtype while preserving normal
    /// `from_underlying`/`from_*` validation semantics.
    pub newtype_checked_ctor: std::collections::HashMap<String, String>,
}

impl IrProgram {
    /// Create an empty IR program with no declarations and default metadata.
    pub fn new() -> Self {
        Self {
            declarations: Vec::new(),
            source_module_name: None,
            entry_point: None,
            function_registry: FunctionRegistry::new(),
            function_reexports: Vec::new(),
            rust_module_path: None,
            newtype_checked_ctor: std::collections::HashMap::new(),
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
