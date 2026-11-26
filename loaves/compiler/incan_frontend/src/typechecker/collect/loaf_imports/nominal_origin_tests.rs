use super::collect_nominal_type_origins;
use crate::library_manifest::{
    AliasExport, CanonicalIdentityExport, LibraryManifest, NominalTypeOriginExport, TypeRef,
};
use crate::loaf_index::LoafIdentity;
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

/// Trait projections retain checked owners but do not require nominal layout reconstruction.
#[test]
fn trait_alias_origins_are_not_nominal_layout_requests() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("facade", "0.1.0");
    for (name, kind) in [
        ("Unary", SemanticSourceTargetKind::Trait),
        ("Record", SemanticSourceTargetKind::Model),
    ] {
        let identity = CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Package {
                library: "owner".into(),
                module_path: vec!["contracts".into()],
            },
            declaration_name: name.into(),
            kind,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(1, 2),
        };
        manifest.exports.aliases.push(AliasExport {
            name: name.into(),
            target_path: vec!["owner".into(), name.into()],
            projected_function: None,
            projected_type: Some(TypeRef::Named {
                name: name.into(),
                origin: Some(NominalTypeOriginExport {
                    provider: LoafIdentity {
                        name: "owner".into(),
                        version: "0.1.0".into(),
                        digest: "sha256:test".into(),
                        feature_projection: Default::default(),
                    },
                    canonical: CanonicalIdentityExport::from_canonical("facade", &identity)
                        .ok_or("missing checked identity")?,
                }),
            }),
        });
    }
    let origins = collect_nominal_type_origins(&manifest);
    assert_eq!(origins.len(), 1, "a checked trait is not a nominal layout request");
    assert!(
        origins
            .values()
            .all(|origin| origin.canonical.declaration_name == "Record")
    );
    assert_eq!(manifest.exports.aliases.len(), 2);
    Ok(())
}
