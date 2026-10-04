//! Fixtures that more than one library manifest test module uses: the legacy manifest and the source, member and
//! published identity builders. A fixture only one module needs stays private in that module.

use super::*;

pub(super) fn legacy_manifest_fixture(name: &str, version: &str) -> LibraryManifest {
    let mut manifest = LibraryManifest::new(name, version);
    manifest.contract_metadata.identity_graph = LibraryIdentityGraph {
        schema_version: LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
        exports: Vec::new(),
    };
    manifest
}

pub(super) fn source_identity(
    module_path: &[&str],
    name: &str,
    kind: incan_semantics_core::SemanticSourceTargetKind,
    start: usize,
    end: usize,
) -> incan_semantics_core::CanonicalSymbolId {
    incan_semantics_core::CanonicalSymbolId::module_declaration(
        module_path.iter().map(|part| (*part).to_string()).collect(),
        name,
        kind,
        incan_semantics_core::HirSourceSpan::new(start, end),
    )
}

pub(super) fn source_member_identity(
    module_path: &[&str],
    name: &str,
    kind: incan_semantics_core::SemanticSourceTargetKind,
    start: usize,
    end: usize,
) -> incan_semantics_core::CanonicalSymbolId {
    incan_semantics_core::CanonicalSymbolId {
        namespace: incan_semantics_core::SymbolNamespace::Member,
        origin: incan_semantics_core::SymbolOrigin::Module(
            module_path.iter().map(|part| (*part).to_string()).collect(),
        ),
        declaration_name: name.to_string(),
        kind,
        scope_discriminant: None,
        declaration_span: incan_semantics_core::HirSourceSpan::new(start, end),
    }
}

pub(super) fn published_declaration_identity(
    package: &str,
    module_path: &[&str],
    name: &str,
    kind: incan_semantics_core::SemanticSourceTargetKind,
    start: usize,
    end: usize,
) -> incan_semantics_core::CanonicalSymbolId {
    incan_semantics_core::CanonicalSymbolId {
        namespace: incan_semantics_core::SymbolNamespace::OrdinaryLexical,
        origin: incan_semantics_core::SymbolOrigin::Package {
            library: package.to_string(),
            module_path: module_path.iter().map(|part| (*part).to_string()).collect(),
        },
        declaration_name: name.to_string(),
        kind,
        scope_discriminant: None,
        declaration_span: incan_semantics_core::HirSourceSpan::new(start, end),
    }
}
