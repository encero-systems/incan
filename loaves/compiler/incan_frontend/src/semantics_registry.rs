//! Shared semantics registry construction.
//!
//! Compiler consumers need a `SurfaceSemanticsRegistry` with the same standard pack set. This module provides their
//! single construction point; source imports and provider catalogs govern which constructs a program may use.

use incan_semantics_core::SurfaceSemanticsRegistry;
use incan_semantics_stdlib::StdlibSemanticsPack;

/// Build the shared registry with the standard surface semantics every compiler consumer supports.
pub fn semantics_registry<'a>() -> SurfaceSemanticsRegistry<'a> {
    static STDLIB_PACK: StdlibSemanticsPack = StdlibSemanticsPack;
    SurfaceSemanticsRegistry::new().with_pack(&STDLIB_PACK)
}
