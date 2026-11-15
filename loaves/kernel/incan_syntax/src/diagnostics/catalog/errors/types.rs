//! Type-system and semantic errors.
//!
//! Diagnostics raised during type checking: mismatches, missing implementations, trait conformance, field/alias
//! validation, mutability, and pattern matching.

use crate::ast::Span;
use incan_lang::lang::builtins::{self, BuiltinFnId};
use incan_lang::lang::conventions;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};

use crate::diagnostics::CompileError;

mod constructors_and_collections;
mod display_and_capabilities;
mod fields_and_aliases;
mod mismatches_and_functions;
mod mutability;
mod symbols_and_decorators;
mod trait_errors;

pub use constructors_and_collections::*;
pub use display_and_capabilities::*;
pub use fields_and_aliases::*;
pub use mismatches_and_functions::*;
pub use mutability::*;
pub use symbols_and_decorators::*;
pub use trait_errors::*;
