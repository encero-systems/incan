use std::collections::BTreeSet;
use std::fs;

use incan_frontend::executable_resolution::resolve_loaf_declarations;
use incan_frontend::library_manifest::{
    CanonicalIdentityExport, ExecutableRepresentationExport, ExportIdentity, ExportIdentityKind,
    ExportIdentityProjection, LibraryManifest, StaticExport, TypeRef,
};
use incan_semantics_core::body_ir::{BodyIrModule, Constant, StaticDeclaration, StaticInitializer};
use incan_semantics_core::executable_representation::{prepare_surface, representation_version};
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind};
use sha2::{Digest, Sha256};

use super::CompilationSession;
use crate::unit::{UnitDescription, UnitNativeCrate};

/// Only source interfaces can supply initializer bodies. The selected native Loaf must own the cell.
#[test]
fn selected_native_loaf_requires_producer_owned_static_storage() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let dir = root.path();
    let archive = dir.join("libproducer.rlib");
    fs::write(&archive, b"selected archive path")?;
    let identity = CanonicalSymbolId::module_declaration(
        vec!["lib".into()],
        "count",
        SemanticSourceTargetKind::Static,
        HirSourceSpan::new(4, 16),
    );
    let exported = CanonicalIdentityExport::from_canonical("producer", &identity).ok_or("missing exported identity")?;
    let identity = exported.hydrate().ok_or("missing package identity")?;
    let mut manifest = LibraryManifest::new("producer", "0.1.0");
    manifest.exports.statics.push(StaticExport {
        name: "count".into(),
        ty: TypeRef::Named {
            name: "int".into(),
            origin: None,
        },
    });
    manifest.contract_metadata.identity_graph.exports.push(ExportIdentity {
        public_name: "count".into(),
        public_path: vec!["producer".into(), "count".into()],
        source_path: vec!["lib".into(), "count".into()],
        kind: ExportIdentityKind::Static,
        projection: ExportIdentityProjection::Direct,
        canonical: Some(exported),
    });
    let owner = incan_semantics_core::canonical_module_identity(&identity).ok_or("missing static owner")?;
    let mut module = BodyIrModule::declarations_only(owner);
    module.static_declarations.push(StaticDeclaration {
        canonical: identity.clone(),
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        initial: StaticInitializer::Literal(Constant::Int(1)),
    });
    let public = BTreeSet::from([identity]);
    let unit = serde_json::json!({
        "schema": "incan.unit/1", "source": dir.join("main.incn"), "project_root": dir, "source_root": dir,
        "package": "consumer", "version": "0.1.0", "profile": "debug", "declared_features": [],
        "active_features": [], "loaves": [{"key": "alias", "name": "producer", "dir": dir, "standard": false}],
        "rust_crates": [], "contract_models": [], "c_headers": [], "externs": [], "search": [], "rust_abi": [],
    });
    let source_consumer = UnitDescription::parse(unit.to_string().as_bytes(), &dir.join("main.unit"))?;
    let mut native_consumer = source_consumer.clone();
    native_consumer.externs.push(UnitNativeCrate {
        name: "producer".into(),
        path: archive,
    });
    let resolve = |unit| {
        let session = CompilationSession::new(unit)?;
        Ok::<_, Box<dyn std::error::Error>>(resolve_loaf_declarations(session.loaves(), &public, &BTreeSet::new())?)
    };

    for native_storage in [true, false] {
        let surface = prepare_surface(
            std::slice::from_ref(&module),
            "producer",
            "0.1.0",
            &public,
            &BTreeSet::new(),
            native_storage,
            None,
        )?;
        manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
            representation_version: representation_version(&surface.bytes)?,
            content_digest: hex::encode(Sha256::digest(&surface.bytes)),
        });
        fs::write(dir.join("producer.incnbody"), &surface.bytes)?;
        fs::write(dir.join("producer.incnlib"), manifest.to_json_string()?)?;
        if native_storage {
            assert!(matches!(
                resolve(&native_consumer)?.modules[0].static_declarations[0].initial,
                StaticInitializer::Linked(_)
            ));
        } else {
            assert_eq!(
                resolve(&source_consumer)?.modules[0].static_declarations,
                module.static_declarations
            );
            let Err(error) = resolve(&native_consumer) else {
                return Err("selected native Loaf accepted a consumer-owned copy of its static".into());
            };
            assert!(
                error
                    .to_string()
                    .contains("static `count` has no producer-owned storage binding"),
                "{error}"
            );
        }
    }
    Ok(())
}
