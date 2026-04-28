//! Stable vocabulary registration contract for Incan library companion crates.
//!
//! Companion crates should expose one canonical Rust entrypoint:
//!
//! ```rust
//! use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, VocabRegistration};
//!
//! pub fn library_vocab() -> VocabRegistration {
//!     VocabRegistration::new().with_surface(
//!         DslSurface::on_import("demo.surface").with_declaration(
//!             DeclarationSurface::named("query")
//!                 .with_clause_body()
//!                 .desugars_to_expression()
//!                 .with_clauses([
//!                     ClauseSurface::expr("FROM").required(),
//!                     ClauseSurface::expr_list("SELECT").required().after("FROM"),
//!                 ]),
//!         ),
//!     )
//! }
//! ```
//!
//! [`VocabRegistration`], [`DslSurface`], [`DeclarationSurface`], [`ClauseSurface`], [`VocabSyntaxNode`], and
//! [`DesugarOutput`] are the canonical author-facing surface. [`VocabMetadata`] and [`KeywordRegistration`] remain
//! available as lower-level transport and escape-hatch types, but they are not the intended starting point for normal
//! companion-crate authoring.

/// Public AST types used by vocab desugarers.
pub mod ast;
/// Desugaring traits and error types for library-provided syntax lowering.
pub mod desugar;
/// Low-level keyword DTOs shared by companion crates and compiler tooling.
pub mod keywords;
/// Stable manifest DTOs carried inside a vocabulary registration.
pub mod manifest;
/// WASM desugarer runtime export helpers for companion crates.
#[cfg(feature = "serde")]
pub mod runtime;
/// Version constants for serialized vocab metadata.
pub mod version;
/// Canonical WASM ABI names shared between desugarers and compiler tooling.
pub mod wasm_abi;

pub use ast::{
    Decorator, DecoratorArg, DecoratorArgValue, IncanBinaryOp, IncanExpr, IncanScopedSurfaceExpr,
    IncanScopedSurfaceOwner, IncanScopedSurfacePayload, IncanStatement, IncanUnaryOp, Span, VocabBodyItem, VocabClause,
    VocabClauseBody, VocabDeclaration, VocabDeclarationHead, VocabFieldSpec, VocabKeywordMetadata, VocabParameter,
    VocabSyntaxNode, VocabTypeExpr,
};
#[cfg(feature = "serde")]
pub use desugar::execute_desugar_request;
pub use desugar::{
    DesugarError, DesugarOutput, DesugarRequest, DesugarResponse, DesugarerArtifactKind, DesugarerMetadata,
    DesugarerRegistration, VocabDesugarer,
};
pub use keywords::{
    ClauseBodyKind, ClauseCardinality, ClausePlacement, ClauseSurface, DeclarationBodyKind, DeclarationHeadKind,
    DeclarationSurface, DesugarTarget, DslSurface, KeywordActivation, KeywordPlacement, KeywordRegistration,
    KeywordSpec, KeywordSurfaceKind, ScopedSurfaceChainMode, ScopedSurfaceDescriptor, ScopedSurfaceDiagnosticKind,
    ScopedSurfaceDiagnosticTemplate, ScopedSurfaceEligibility, ScopedSurfaceFamily, ScopedSurfaceFormatHint,
    ScopedSurfaceMisuseScope, ScopedSurfacePosition, ScopedSurfaceReceiver, ScopedSurfaceSyntax,
};
pub use manifest::{
    CargoDependency, CargoDependencySource, FieldExport, FunctionExport, HelperBinding, LibraryManifest,
    ManifestFormatVersion, ModuleExport, TypeExport, TypeExportKind, TypeRef,
};
pub use version::{VOCAB_METADATA_VERSION, WASM_DESUGAR_ABI_VERSION};
pub use wasm_abi::{
    WASM_DESUGAR_ENTRYPOINT, WASM_DESUGAR_ERROR_LEN_GLOBAL, WASM_DESUGAR_ERROR_PTR_GLOBAL, WASM_DESUGAR_FAILURE_STATUS,
    WASM_DESUGAR_INIT_ENTRYPOINT, WASM_DESUGAR_INPUT_CAPACITY_GLOBAL, WASM_DESUGAR_INPUT_LEN_GLOBAL,
    WASM_DESUGAR_INPUT_PTR_GLOBAL, WASM_DESUGAR_MEMORY_EXPORT, WASM_DESUGAR_OUTPUT_LEN_GLOBAL,
    WASM_DESUGAR_OUTPUT_PTR_GLOBAL, WASM_DESUGAR_REQUIRED_I32_GLOBAL_EXPORTS, WASM_DESUGAR_SUCCESS_STATUS,
};

/// The stable serializable output shape derived from a companion crate registration.
///
/// Tooling may serialize this value into a library artifact, but library authors should generally work with
/// [`VocabRegistration`] rather than constructing this transport DTO directly.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VocabMetadata {
    /// Serialized metadata schema version.
    #[cfg_attr(feature = "serde", serde(default = "default_vocab_metadata_version"))]
    pub metadata_version: u32,
    /// The keyword registrations contributed by the library.
    #[cfg_attr(feature = "serde", serde(default))]
    pub keyword_registrations: Vec<KeywordRegistration>,
    /// Richer activated DSL surfaces contributed by the library.
    #[cfg_attr(feature = "serde", serde(default))]
    pub dsl_surfaces: Vec<DslSurface>,
    /// Additional machine-readable library metadata provided by the companion crate.
    #[cfg_attr(feature = "serde", serde(default))]
    pub library_manifest: LibraryManifest,
    /// Optional desugarer artifact metadata emitted by the companion crate.
    #[cfg_attr(feature = "serde", serde(default))]
    pub desugarer: Option<DesugarerMetadata>,
}

impl Default for VocabMetadata {
    fn default() -> Self {
        Self {
            metadata_version: VOCAB_METADATA_VERSION,
            keyword_registrations: Vec::new(),
            dsl_surfaces: Vec::new(),
            library_manifest: LibraryManifest::default(),
            desugarer: None,
        }
    }
}

#[cfg(feature = "serde")]
fn default_vocab_metadata_version() -> u32 {
    VOCAB_METADATA_VERSION
}

/// High-level Rust entrypoint for one library's vocabulary surface.
///
/// This bundles author-facing DSL surfaces, manifest metadata, and the optional desugarer into one author-facing
/// value. Low-level keyword registrations are still available as an escape hatch, but the intended companion-crate
/// contract is:
///
/// ```rust
/// use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, VocabRegistration};
///
/// pub fn library_vocab() -> VocabRegistration {
///     VocabRegistration::new().with_surface(
///         DslSurface::on_import("demo.surface").with_declaration(
///             DeclarationSurface::named("query")
///                 .with_clause_body()
///                 .desugars_to_expression()
///                 .with_clauses([
///                     ClauseSurface::expr("FROM").required(),
///                     ClauseSurface::expr_list("SELECT").required().after("FROM"),
///                 ]),
///         ),
///     )
/// }
/// ```
#[derive(Default)]
pub struct VocabRegistration {
    keyword_registrations: Vec<KeywordRegistration>,
    dsl_surfaces: Vec<DslSurface>,
    library_manifest: LibraryManifest,
    desugarer: Option<DesugarerRegistration>,
}

impl VocabRegistration {
    /// Create an empty vocabulary registration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one low-level keyword registration group as an escape hatch.
    #[must_use]
    pub fn with_keyword_registration(mut self, registration: KeywordRegistration) -> Self {
        self.keyword_registrations.push(registration);
        self
    }

    /// Add multiple low-level keyword registration groups as an escape hatch.
    #[must_use]
    pub fn with_keyword_registrations<I>(mut self, registrations: I) -> Self
    where
        I: IntoIterator<Item = KeywordRegistration>,
    {
        self.keyword_registrations.extend(registrations);
        self
    }

    /// Add one richer activated DSL surface.
    #[must_use]
    pub fn with_surface(mut self, surface: DslSurface) -> Self {
        self.dsl_surfaces.push(surface);
        self
    }

    /// Add multiple richer activated DSL surfaces.
    #[must_use]
    pub fn with_surfaces<I>(mut self, surfaces: I) -> Self
    where
        I: IntoIterator<Item = DslSurface>,
    {
        self.dsl_surfaces.extend(surfaces);
        self
    }

    /// Replace the library manifest metadata.
    #[must_use]
    pub fn with_library_manifest(mut self, manifest: LibraryManifest) -> Self {
        self.library_manifest = manifest;
        self
    }

    /// Register one Rust desugarer using the default packaging metadata.
    #[must_use]
    pub fn with_desugarer<D>(mut self, desugarer: D) -> Self
    where
        D: VocabDesugarer + 'static,
    {
        self.desugarer = Some(DesugarerRegistration::new(desugarer));
        self
    }

    /// Register one desugarer with explicit packaging metadata overrides.
    #[must_use]
    pub fn with_desugarer_registration(mut self, desugarer: DesugarerRegistration) -> Self {
        self.desugarer = Some(desugarer);
        self
    }

    /// Borrow the parser-facing keyword registrations.
    #[must_use]
    pub fn keyword_registrations(&self) -> &[KeywordRegistration] {
        &self.keyword_registrations
    }

    /// Borrow the manifest metadata.
    #[must_use]
    pub fn library_manifest(&self) -> &LibraryManifest {
        &self.library_manifest
    }

    /// Borrow the richer activated DSL surfaces.
    #[must_use]
    pub fn surfaces(&self) -> &[DslSurface] {
        &self.dsl_surfaces
    }

    /// Borrow the optional desugarer registration.
    #[must_use]
    pub fn desugarer_registration(&self) -> Option<&DesugarerRegistration> {
        self.desugarer.as_ref()
    }

    /// Derive the compiler-facing transport metadata consumed by build tooling.
    #[must_use]
    pub fn metadata(&self) -> VocabMetadata {
        VocabMetadata {
            metadata_version: VOCAB_METADATA_VERSION,
            keyword_registrations: self.derived_keyword_registrations(),
            dsl_surfaces: self.dsl_surfaces.clone(),
            library_manifest: self.library_manifest.clone(),
            desugarer: self.desugarer.as_ref().map(|desugarer| desugarer.metadata().clone()),
        }
    }

    fn derived_keyword_registrations(&self) -> Vec<KeywordRegistration> {
        let mut registrations = self.keyword_registrations.clone();
        registrations.extend(self.dsl_surfaces.iter().map(keyword_registration_from_surface));
        registrations
    }
}

fn keyword_registration_from_surface(surface: &DslSurface) -> KeywordRegistration {
    surface.declarations.iter().fold(
        KeywordRegistration::new(surface.activation.clone()),
        |registration, declaration| {
            let declaration_keyword = KeywordSpec::block(&declaration.keyword)
                .with_compound_tokens(declaration.compound_tokens.clone())
                .with_placement(declaration.placement.clone());

            let clause_keywords = declaration.clauses.iter().map(|clause| {
                let surface_kind = if matches!(clause.body_kind, ClauseBodyKind::NestedItems) {
                    KeywordSurfaceKind::SubBlock
                } else {
                    KeywordSurfaceKind::BlockContextKeyword
                };

                KeywordSpec::new(&clause.keyword, surface_kind)
                    .with_compound_tokens(clause.compound_tokens.clone())
                    .in_block(&declaration.keyword)
            });

            registration
                .with_keyword(declaration_keyword)
                .with_keywords(clause_keywords)
        },
    )
}

/// Serialize one metadata payload as pretty JSON.
#[cfg(feature = "serde")]
pub fn serialize_metadata_json_pretty(metadata: &VocabMetadata) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec_pretty(metadata)
}

/// Serialize one registration's derived metadata as pretty JSON.
#[cfg(feature = "serde")]
pub fn serialize_registration_json_pretty(registration: &VocabRegistration) -> Result<Vec<u8>, serde_json::Error> {
    serialize_metadata_json_pretty(&registration.metadata())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DemoDesugarer;

    impl VocabDesugarer for DemoDesugarer {
        fn desugar(&self, _node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
            Ok(DesugarOutput::Statements(Vec::new()))
        }
    }

    #[test]
    fn vocab_registration_builds_metadata_from_rust_entrypoint() {
        let registration = VocabRegistration::new()
            .with_surface(
                DslSurface::on_import("demo.routes").with_declaration(
                    DeclarationSurface::named("route")
                        .with_header_args()
                        .with_mixed_body()
                        .with_clause(ClauseSurface::nested_items("middleware")),
                ),
            )
            .with_library_manifest(LibraryManifest {
                modules: vec![ModuleExport {
                    path: "demo.routes".to_string(),
                    ..ModuleExport::default()
                }],
                ..LibraryManifest::default()
            })
            .with_desugarer_registration(
                DesugarerRegistration::new(DemoDesugarer)
                    .with_target("wasm32-wasip1")
                    .with_profile("release")
                    .with_file_name("demo_routes.wasm"),
            );

        let metadata = registration.metadata();

        assert_eq!(metadata.keyword_registrations.len(), 1);
        assert_eq!(metadata.keyword_registrations[0].keywords.len(), 2);
        assert_eq!(metadata.dsl_surfaces.len(), 1);
        assert_eq!(metadata.library_manifest.modules[0].path, "demo.routes");
        assert_eq!(
            metadata
                .desugarer
                .as_ref()
                .and_then(|desugarer| desugarer.file_name.as_deref()),
            Some("demo_routes.wasm")
        );
        assert!(registration.desugarer_registration().is_some());
    }

    #[test]
    fn registration_json_helper_serializes_derived_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let registration = VocabRegistration::new()
            .with_surface(DslSurface::on_import("demo.routes").with_declaration(DeclarationSurface::named("route")));

        let encoded = serialize_registration_json_pretty(&registration)?;
        let decoded: VocabMetadata = serde_json::from_slice(&encoded)?;

        assert_eq!(decoded.metadata_version, VOCAB_METADATA_VERSION);
        assert_eq!(decoded.keyword_registrations.len(), 1);
        assert_eq!(decoded.dsl_surfaces.len(), 1);
        assert_eq!(decoded.library_manifest, LibraryManifest::default());
        assert_eq!(decoded.desugarer, None);
        Ok(())
    }

    #[test]
    fn pseudo_query_surface_can_describe_clause_owned_grammar() {
        let registration = VocabRegistration::new().with_surface(
            DslSurface::on_import("demo.analysis").with_declaration(
                DeclarationSurface::named("query")
                    .with_clause_body()
                    .desugars_to_expression()
                    .with_clauses([
                        ClauseSurface::expr("FROM").required(),
                        ClauseSurface::expr("FILTER").optional().after("FROM"),
                        ClauseSurface::expr_list("GROUP BY").optional().after("FILTER"),
                        ClauseSurface::expr_list("SELECT").required().after("GROUP BY"),
                        ClauseSurface::nested_items("WINDOW BY").optional().after("SELECT"),
                    ]),
            ),
        );

        let surface = &registration.surfaces()[0].declarations[0];
        assert_eq!(surface.keyword, "query");
        assert_eq!(surface.body_kind, DeclarationBodyKind::Clauses);
        assert_eq!(surface.desugars_to, DesugarTarget::Expression);
        assert_eq!(surface.clauses.len(), 5);
        assert_eq!(surface.clauses[2].compound_tokens, vec!["BY".to_string()]);
        assert_eq!(surface.clauses[0].cardinality, ClauseCardinality::Required);
    }

    #[test]
    fn pseudo_step_and_workflow_surfaces_can_describe_typed_sections_and_host_bodies() {
        let registration = VocabRegistration::new().with_surface(
            DslSurface::on_import("demo.workflow")
                .with_declaration(
                    DeclarationSurface::named("step")
                        .with_signature_head()
                        .with_mixed_body()
                        .with_clauses([
                            ClauseSurface::fields("config").optional(),
                            ClauseSurface::type_ref("input").required(),
                            ClauseSurface::type_ref("output").required().after("input"),
                        ]),
                )
                .with_declaration(
                    DeclarationSurface::named("workflow")
                        .with_header_args()
                        .with_statement_body(),
                ),
        );

        let step = &registration.surfaces()[0].declarations[0];
        let workflow = &registration.surfaces()[0].declarations[1];
        assert_eq!(step.keyword, "step");
        assert_eq!(step.head_kind, DeclarationHeadKind::Signature);
        assert_eq!(step.body_kind, DeclarationBodyKind::Mixed);
        assert_eq!(workflow.keyword, "workflow");
        assert_eq!(workflow.body_kind, DeclarationBodyKind::Statements);
    }

    #[test]
    fn scoped_surface_descriptors_are_part_of_author_facing_metadata() {
        let registration = VocabRegistration::new().with_surface(
            DslSurface::on_import("demo.analysis")
                .with_declaration(
                    DeclarationSurface::named("query")
                        .with_clause_body()
                        .with_clause(ClauseSurface::expr("SELECT").required()),
                )
                .with_scoped_surfaces([
                    ScopedSurfaceDescriptor::operator("query.pipe", "|>")
                        .in_clause_body("query", "SELECT")
                        .with_misuse_scope(ScopedSurfaceMisuseScope::ActivatingFile)
                        .with_diagnostic(ScopedSurfaceDiagnosticTemplate::new(
                            "query-pipe-outside-scope",
                            ScopedSurfaceDiagnosticKind::OutsideScope,
                            "`|>` is only valid inside query SELECT clauses",
                        ))
                        .pairwise_chain(),
                    ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_clause_body("query", "SELECT")
                        .with_receiver(ScopedSurfaceReceiver::clause("FROM")),
                ]),
        );

        let metadata = registration.metadata();
        let surface = &metadata.dsl_surfaces[0];

        assert_eq!(surface.scoped_surfaces.len(), 2);
        assert_eq!(surface.scoped_surfaces[0].family, ScopedSurfaceFamily::OperatorLike);
        assert_eq!(
            surface.scoped_surfaces[0].format_hint.chain_mode,
            ScopedSurfaceChainMode::Pairwise
        );
        assert_eq!(surface.scoped_surfaces[1].family, ScopedSurfaceFamily::ExpressionForm);
        assert_eq!(
            surface.scoped_surfaces[1].receiver,
            Some(ScopedSurfaceReceiver::clause("FROM"))
        );
    }
}
