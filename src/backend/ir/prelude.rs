//! Internal prelude for backend modules
//!
//! This module re-exports IR types for internal use within the backend.
//! External users should interact via the public API (`IrCodegen`, `ProjectGenerator`)
//! rather than manipulating IR types directly.

// IR types
pub use super::decl::{FunctionParam, IrDecl, IrDeclKind, IrFunction, IrStruct};
pub use super::expr::{IrExpr, IrExprKind, TypedExpr};
pub use super::stmt::{IrStmt, IrStmtKind};
pub use super::types::{IrType, Mutability, Ownership};

// Lowering and emission
pub use super::emit::{EmitError, IrEmitter};
pub use super::lower::{AstLowering, LoweringError};

// Program representation (defined in mod.rs)
pub use super::{FunctionRegistry, FunctionSignature, IrProgram};

// Scanners
pub use super::scanners::{check_for_this_import, collect_rust_crates, detect_serde_usage};

// Services
pub use super::emit_service::EmitService;
pub use super::facade::CodegenFacade;
