// Declaration parsing methods.
//
// This chunk is responsible for parsing top-level declarations such as imports,
// type/enum/newtype declarations, models/classes/traits, and functions/methods.
//
// Notes:
// - Most entrypoints in this file return `Spanned<T>` to preserve source locations.
// - Error recovery is handled by `Parser::synchronize()` (in `helpers.rs`).
// - The implementation is split into focused include files to avoid a god-module.

include!("entrypoints.rs");
include!("decorators.rs");
include!("imports.rs");
include!("models.rs");
include!("types.rs");
include!("adt.rs");
include!("functions.rs");
