//! Expand imported vocabulary through admitted, bounded native workers before ordinary typechecking.
//! Worker requests and replies use the published `std.vocab` syntax schema, never compiler-private AST serialization.

mod checked_c;
mod errors;
mod fragments;
mod helper_bindings;
mod rewrite;
mod runtime;
mod surface;

pub use errors::VocabDesugarPassError;
pub use rewrite::{desugar_program_vocab_blocks, desugar_program_vocab_blocks_with_runtime};
pub use runtime::NativeDesugarerRuntime;
