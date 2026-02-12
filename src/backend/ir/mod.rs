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
pub mod prelude;

pub mod codegen;
pub mod decl;
pub mod emit;
pub mod emit_service;
pub mod expr;
pub mod facade;
pub mod lower;
pub mod scanners;
pub mod stmt;
pub mod trait_bound_inference;
pub mod types;

pub use codegen::{GenerationError, IrCodegen};
pub use decl::{FunctionParam, IrDecl, IrDeclKind, IrFunction, IrStruct};
pub use emit::{EmitError, IrEmitter};
pub use emit_service::EmitService;
pub use expr::{BuiltinFn, IrExpr, IrExprKind, MethodKind, TypedExpr};
pub use facade::CodegenFacade;
pub use lower::{AstLowering, LoweringError, LoweringErrors};
pub use scanners::{
    check_for_this_import, collect_routes, collect_rust_crates, detect_async_usage, detect_list_helpers_usage,
    detect_serde_usage, detect_web_usage,
};
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

    /// Register a function signature
    pub fn register(&mut self, name: String, params: Vec<FunctionParam>, return_type: IrType) {
        self.signatures.insert(name, FunctionSignature { params, return_type });
    }

    /// Look up a function signature by name
    pub fn get(&self, name: &str) -> Option<&FunctionSignature> {
        self.signatures.get(name)
    }

    /// Merge another registry into this one
    pub fn merge(&mut self, other: &FunctionRegistry) {
        for (name, sig) in &other.signatures {
            self.signatures.insert(name.clone(), sig.clone());
        }
    }
}

/// A complete IR program
#[derive(Debug, Clone)]
pub struct IrProgram {
    /// Top-level declarations
    pub declarations: Vec<IrDecl>,
    /// Entry point function name (usually "main")
    pub entry_point: Option<String>,
    /// Function signature registry for call-site type checking
    pub function_registry: FunctionRegistry,
    /// RFC 023: The `rust.module("path::to::module")` Rust backing path, if declared.
    ///
    /// When present, `@rust.extern` functions in this program emit delegation calls to this Rust module path instead
    /// of compiling their Incan bodies. See RFC 023 for full design.
    pub rust_module_path: Option<String>,
}

impl IrProgram {
    pub fn new() -> Self {
        Self {
            declarations: Vec::new(),
            entry_point: None,
            function_registry: FunctionRegistry::new(),
            rust_module_path: None,
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
