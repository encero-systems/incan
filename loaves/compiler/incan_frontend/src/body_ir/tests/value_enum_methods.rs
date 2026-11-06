//! Checked value-enum layouts remain independent of their ordinary and static method bodies.

use super::{bir, build};
use crate::body_ir::build_body_ir_module_v0;
use crate::library_exports::collect_checked_public_exports;
use crate::library_manifest::LibraryManifest;
use crate::library_manifest::published_layout::public_executable_identities;
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_semantics_core::executable_representation::{ExecutableDeclaration, SurfaceReader, build_surface};

const CODEC: &str = "pub enum Codec(str):\n    \"\"\"Keep checked codec values beside a static selector.\"\"\"\n    Gzip = \"gzip\"\n    Zlib = \"zlib\"\n\n    @staticmethod\n    def select() -> Codec:\n        \"\"\"Return one checked member through the enum's static body.\"\"\"\n        return Codec.Zlib\n";

/// A static method does not erase the independently checked raw values or their canonical unit layout.
#[test]
fn static_value_enum_retains_checked_values_and_layout() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(CODEC, &["m", "codec"])?;
    let value = module.value_enum_declarations.first().ok_or("missing value enum")?;
    let layout = module.enum_declarations.first().ok_or("missing enum layout")?;
    assert_eq!(value.canonical, layout.canonical);
    assert_eq!(value.variants.len(), 2);
    assert_eq!(value.variants[0].raw_value, bir::Constant::Str("gzip".into()));
    assert_eq!(value.variants[1].raw_value, bir::Constant::Str("zlib".into()));
    for (value, layout) in value.variants.iter().zip(&layout.variants) {
        assert_eq!(value.canonical, layout.canonical);
        assert_eq!(value.name, layout.name);
        assert!(layout.fields.is_empty());
    }
    assert!(module.bodies.iter().any(|body| body.name == "select"));
    Ok(())
}

/// Public publication uses the checked enum and exact public members, rather than recovering a missing layout.
#[test]
fn public_value_enum_with_static_method_publishes_checked_context() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(CODEC).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("codec_fixture".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let manifest = LibraryManifest::from_checked_exports("codec_fixture", "1.2.3", &exports);
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("Codec")
        .ok_or("missing public Codec identity")?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let bytes = build_surface(
        &[module],
        "codec_fixture",
        "1.2.3",
        &public_executable_identities(&manifest),
        &Default::default(),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        reader.covers(&identity),
        "checked public enum has no executable coverage"
    );
    let ExecutableDeclaration::ValueEnumWithLayout(value, layout) = reader.fragment(&identity)? else {
        return Err("public enum lost its raw-value/layout pair".into());
    };
    assert_eq!(value.canonical, identity);
    assert_eq!(value.canonical, layout.canonical);
    assert_eq!(value.variants[1].raw_value, bir::Constant::Str("zlib".into()));
    Ok(())
}

/// Plain value enums keep their exact layout and literals when there are no method bodies to retain.
#[test]
fn plain_value_enum_preserves_checked_registry() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "enum Status(int):\n    \"\"\"Keep the existing scalar registry.\"\"\"\n    Ready = 7\n    Waiting = 11\n",
        &["m", "plain_status"],
    )?;
    let value = module
        .value_enum_declarations
        .first()
        .ok_or("missing plain value enum")?;
    assert_eq!(value.variants[0].raw_value, bir::Constant::Int(7));
    assert_eq!(value.variants[1].raw_value, bir::Constant::Int(11));
    assert_eq!(module.enum_declarations[0].canonical, value.canonical);
    Ok(())
}
