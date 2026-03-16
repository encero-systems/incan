//! Rich surrogate companion crate used to design the high-level `incan_vocab` surface.
//!
//! This example is intentionally outward-safe. It pressure-tests the public authoring API with
//! query-like and workflow-like declarations inspired by private requirements, without exposing the
//! private DSLs themselves.
//!
//! The intended consumer-facing feel is roughly:
//!
//! ```text
//! revenue_story = query {
//!     FROM orders
//!     FILTER .status == "paid"
//!     GROUP BY .region
//!     SELECT region, total(.amount) as revenue
//!     WINDOW BY:
//!         rolling(7 days)
//! }
//!
//! step normalize_orders(data: Records[RawOrder]) -> Records[Order]:
//!     config:
//!         currency: str = "EUR"
//!     input: Records[RawOrder]
//!     output: Records[Order]
//!     return query {
//!         FROM data
//!         SELECT normalize(.amount, currency=currency) as amount
//!     }
//!
//! workflow daily_revenue:
//!     orders = load_orders()
//!     clean = normalize_orders(orders)
//!     revenue = query {
//!         FROM clean
//!         SELECT .region, total(.amount) as revenue
//!     }
//! ```

mod desugar;

use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, LibraryManifest, VocabRegistration};

pub use desugar::StudioKitDesugarer;

/// Import namespace that activates the surrogate DSL surface.
pub const NAMESPACE: &str = "studiokit";

/// Query-like declaration that desugars to an expression value.
pub const QUERY_KW: &str = "query";

/// Workflow node declaration with typed/config-like sections and host code.
pub const STEP_KW: &str = "step";

/// Top-level orchestration declaration that references steps and host bindings.
pub const WORKFLOW_KW: &str = "workflow";

/// Return the complete vocabulary registration for the surrogate companion crate.
#[must_use]
pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import(NAMESPACE)
                .with_declaration(
                    DeclarationSurface::named(QUERY_KW)
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            ClauseSurface::expr("FROM").required(),
                            ClauseSurface::expr("RELATE").repeating(),
                            ClauseSurface::expr("FILTER").optional().after("FROM"),
                            ClauseSurface::expr_list("GROUP BY").optional().after("FILTER"),
                            ClauseSurface::expr_list("SELECT").required().after("GROUP BY"),
                            ClauseSurface::nested_items("WINDOW BY").optional().after("SELECT"),
                        ]),
                )
                .with_declaration(
                    DeclarationSurface::named(STEP_KW)
                        .with_signature_head()
                        .with_mixed_body()
                        .with_clauses([
                            ClauseSurface::fields("config").optional(),
                            ClauseSurface::type_ref("input").required(),
                            ClauseSurface::type_ref("output").required().after("input"),
                        ]),
                )
                .with_declaration(
                    DeclarationSurface::named(WORKFLOW_KW)
                        .with_header_args()
                        .with_statement_body(),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
        .with_desugarer(StudioKitDesugarer)
}
