
//! The compiler driver: the compilation session, module collection, typecheck orchestration, checking, Loaf
//! publication and the inspection analyses that the Oven (DD-0008), the pinned driver and the language server call.
//!
//! Nothing in this crate parses `clap` arguments or plans a build; callers render what the driver returns.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
pub mod backend;
pub mod check;
pub mod diagnostics;
pub mod error;
pub mod features;
pub mod inspect;
pub mod invocation_trace;
pub mod loaf_publication;
pub mod metadata_packages;
pub mod modules;
pub mod project;
pub mod session;
pub mod testing;
pub mod typecheck;
