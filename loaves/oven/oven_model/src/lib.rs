//! What Oven knows about a project without knowing Incan: its manifest, its workspace, its lifecycle policy, the
//! layout of an installed toolchain, and its interop declarations.
//!
//! Nothing here names a compiler crate. The `oven.lock` model is here too; the compiler fills its semantic state
//! from its provider plan.

pub mod compiler_identity;
pub mod compiler_suite_env;
pub mod digest;
pub mod lock;
pub mod manifest;
pub mod oven_interop;
pub mod project_lifecycle;
pub mod toolchain_layout;
pub mod workspace;
