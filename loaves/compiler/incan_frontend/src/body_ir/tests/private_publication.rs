//! Public roots retain required private execution without widening the finalized import surface.

use std::collections::BTreeSet;

use crate::body_ir::build_body_ir_module_v0;
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::executable_representation::{DeclarationCoverage, SurfaceReader, build_surface};

/// Build an ordinary checked package module without publishing or reconstructing source at consumption.
fn checked_package_module(source: &str, package: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some(package.to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// Select only the exact named canonical declaration retained by the declaring checker.
fn body_identity(module: &BodyIrModule, name: &str) -> Result<CanonicalSymbolId, Box<dyn std::error::Error>> {
    module
        .bodies
        .iter()
        .filter_map(|body| body.canonical.as_ref())
        .find(|identity| identity.declaration_name == name)
        .cloned()
        .ok_or_else(|| format!("missing checked body {name}").into())
}

/// Checked private generic and recursive helpers are reachable only from a public root's retained closure.
#[test]
fn public_root_retains_private_generic_and_recursive_helpers() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def identity[T](value: T) -> T:
    """Keep an internal generic helper out of the import surface."""
    return value


def descend(value: int) -> int:
    """Retain the exact recursive canonical call."""
    if value == 0:
        return 0
    return 1 + descend(value - 1)


def unused() -> int:
    """Stay outside every selected execution closure."""
    return 99


pub def run(value: int) -> int:
    """Execute private implementation through one public entry point."""
    return identity(descend(value))
"#;
    let module = checked_package_module(source, "helpers")?;
    let public_root = body_identity(&module, "run")?;
    let public = BTreeSet::from([public_root.clone()]);
    let bytes = build_surface(&[module], "helpers", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&public_root));
    assert_eq!(
        reader.index().declarations.keys().cloned().collect::<BTreeSet<_>>(),
        public
    );
    let DeclarationCoverage::Covered { requirements, .. } = reader.index().coverage(&public_root)? else {
        return Err("public root lacks its private execution closure".into());
    };
    assert!(
        requirements
            .iter()
            .any(|identity| identity.declaration_name == "identity")
    );
    assert!(
        requirements
            .iter()
            .any(|identity| identity.declaration_name == "descend")
    );
    assert!(
        !requirements
            .iter()
            .any(|identity| identity.declaration_name == "unused")
    );
    Ok(())
}

/// A public layout retains every checked field and its declaration-owned defaults without exporting private members.
#[test]
fn public_layout_retains_required_private_fields() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_package_module(
        "pub model Card:\n    size: int = 4\n    pub label: str = \"card\"\n\npub def show() -> str:\n    return Card().label\n",
        "helpers",
    )?;
    let card = module.nominal_declarations.first().ok_or("missing checked Card")?;
    let root = body_identity(&module, "show")?;
    let mut public = BTreeSet::from([card.canonical.clone(), root.clone()]);
    public.extend(
        card.field_identities
            .iter()
            .zip(&card.field_public)
            .filter(|(_, visible)| **visible)
            .map(|(identity, _)| identity.clone()),
    );
    let bytes = build_surface(&[module], "helpers", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&root));
    assert_eq!(
        reader.index().declarations.keys().cloned().collect::<BTreeSet<_>>(),
        public
    );
    Ok(())
}

/// Missing checked implementation remains uncovered rather than recreating a private callee from its name.
#[test]
fn public_root_requires_the_checked_private_body() -> Result<(), Box<dyn std::error::Error>> {
    let mut module = checked_package_module(
        "def helper() -> int:\n    return 7\n\npub def run() -> int:\n    return helper()\n",
        "helpers",
    )?;
    let root = body_identity(&module, "run")?;
    module.bodies.retain(|body| body.name != "helper");
    let bytes = build_surface(
        &[module],
        "helpers",
        "0.1.0",
        &BTreeSet::from([root.clone()]),
        &BTreeSet::new(),
    )?;
    assert!(!SurfaceReader::open(&bytes)?.covers(&root));
    Ok(())
}

/// A runtime-profile refusal in required private code keeps its public caller uncovered transitively.
#[test]
fn unsupported_private_body_keeps_public_root_uncovered() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_package_module(
        "def helper() -> int:\n    return 7\n\npub def run() -> int:\n    return helper()\n",
        "helpers",
    )?;
    let root = body_identity(&module, "run")?;
    let helper = body_identity(&module, "helper")?;
    let bytes = build_surface(
        &[module],
        "helpers",
        "0.1.0",
        &BTreeSet::from([root.clone()]),
        &BTreeSet::from([helper]),
    )?;
    assert!(!SurfaceReader::open(&bytes)?.covers(&root));
    Ok(())
}

/// A required private layout retains its checked display body, and missing that body refuses the public factory.
#[test]
fn private_layout_requires_its_checked_display_body() -> Result<(), Box<dyn std::error::Error>> {
    let mut module = checked_package_module(
        "model Box:\n    value: int = 3\n    def __str__(self) -> str:\n        return \"box\"\n\npub def make() -> Box:\n    return Box()\n",
        "helpers",
    )?;
    let root = body_identity(&module, "make")?;
    let display = body_identity(&module, "__str__")?;
    let public = BTreeSet::from([root.clone()]);
    let bytes = build_surface(&[module.clone()], "helpers", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&root));
    assert!(reader.index().implementation.contains_key(&display));
    assert!(reader.fragment(&display).is_err());
    module.bodies.retain(|body| body.canonical.as_ref() != Some(&display));
    let missing = build_surface(&[module], "helpers", "0.1.0", &public, &BTreeSet::new())?;
    assert!(!SurfaceReader::open(&missing)?.covers(&root));
    Ok(())
}
