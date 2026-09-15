//! Internal prelude for backend modules
//!
//! This module re-exports IR types for internal use within the backend. External users should interact via the public
//! API (`IrCodegen`, `ProjectGenerator`) rather than manipulating IR types directly.

// IR types
pub use incan_ir::decl::{FunctionParam, IrDecl, IrDeclKind, IrFunction, IrStruct};
pub use incan_ir::expr::{IrExpr, IrExprKind, TypedExpr};
pub use incan_ir::stmt::{IrStmt, IrStmtKind};
pub use incan_ir::types::{IrType, Mutability, Ownership};

// Lowering and emission
pub use crate::emit::{EmitError, IrEmitter};
pub use incan_ir::lower::{AstLowering, LoweringError};

// Program representation (defined in mod.rs)
pub use incan_ir::{FunctionRegistry, FunctionSignature, IrProgram};

// Scanners
pub use incan_ir::scanners::{check_for_this_import, collect_rust_crates, detect_serde_usage};

// Services
pub use crate::emit_service::EmitService;
pub use crate::facade::CodegenFacade;
