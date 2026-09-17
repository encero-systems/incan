//! What Oven knows about a project without knowing Incan: its manifest, its workspace, its lifecycle policy, the
//! layout of an installed toolchain, and its interop declarations.
//!
//! Nothing here depends on a compiler crate. The toolchain layout still spells the compiler's crate names, an
//! Incan fact this ring carries until RFC 118 makes it a provider-supplied one. The `oven.lock` model is here too;
//! the compiler fills its semantic state from its provider plan.

pub mod compiler_identity;
pub mod compiler_suite_env;
pub mod digest;
pub mod lock;
pub mod manifest;
pub mod oven_interop;
pub mod project_lifecycle;
pub mod toolchain_layout;
pub mod workspace;
