//! Post-parse desugaring pass for imported vocab block DSLs.
//!
//! This module provides:
//! - AST rewriting from raw `Statement::VocabBlock` nodes to ordinary statements
//! - sandboxed WASM desugarer loading/execution for dependency-provided artifacts
//! - deterministic diagnostics for bridge/runtime/deserialization failures
//!
//! Mental model:
//! - the parser records import-activated DSL blocks as raw `Statement::VocabBlock` nodes
//! - this pass bridges those nodes into the public `incan_vocab` AST and serializes a request
//! - a companion-crate desugarer runs inside a Wasmtime guest and returns JSON over linear memory
//! - the result is bridged back into ordinary compiler AST before typechecking/lowering continue
//!
//! We use WASI here for portability, not to grant broad host access. Rust companion crates built for `wasm32-wasip1`
//! expect the standard WASI import surface, so the compiler links that import set to instantiate the module reliably.
//! The runtime still communicates only through explicit request/response buffers in guest memory, and the host does not
//! inherit stdio or open ambient filesystem/network access for desugarers.

mod errors;
mod helper_bindings;
mod rewrite;
mod runtime;

pub use errors::VocabDesugarPassError;
pub use rewrite::desugar_program_vocab_blocks;
pub use runtime::WasmDesugarerRuntime;
