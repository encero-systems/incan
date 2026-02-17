//! Shared semantics registry construction.
//!
//! Both the frontend (`surface_semantics::SurfaceContext`) and the backend (`ir::surface_semantics`) need a configured
//! `SurfaceSemanticsRegistry` with the same pack set. This module provides a single construction point so the two
//! halves stay in sync automatically.

use incan_semantics_core::SurfaceSemanticsRegistry;
#[cfg(any(feature = "std_testing", feature = "std_async", feature = "std_decorators"))]
use incan_semantics_stdlib::StdlibSemanticsPack;

/// Build the semantics registry populated with all compile-time-enabled packs.
///
/// When no `std_*` features are active the registry is empty and every query returns `None`.
pub(crate) fn semantics_registry<'a>() -> SurfaceSemanticsRegistry<'a> {
    #[cfg(any(feature = "std_testing", feature = "std_async", feature = "std_decorators"))]
    {
        static STDLIB_PACK: StdlibSemanticsPack = StdlibSemanticsPack;
        SurfaceSemanticsRegistry::new().with_pack(&STDLIB_PACK)
    }
    #[cfg(not(any(feature = "std_testing", feature = "std_async", feature = "std_decorators")))]
    {
        SurfaceSemanticsRegistry::new()
    }
}
