//! Checked decorator execution closures retain originals, replacements and module initialization across packages.

use std::collections::BTreeSet;

use crate::body_ir::build_body_ir_module_v0;
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::executable_representation::{SurfaceReader, build_surface};

/// Checked source retains a private decorator, an original body, a visible wrapper and its computed replacement.
const SOURCE: &str = r#"def keep(func: (int) -> int) -> (int) -> int:
    println("keep")
    println(func(3))
    return func

@keep
@keep
pub def double(value: int) -> int:
    return value * 2
"#;

/// Use the ordinary declaring checker, with no source reconstruction at the package-consumer boundary.
pub(super) fn checked_module(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("decorators".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// Select the exact visible declaration retained by the declaring compilation.
pub(super) fn root(module: &BodyIrModule, name: &str) -> Result<CanonicalSymbolId, Box<dyn std::error::Error>> {
    module
        .bodies
        .iter()
        .filter_map(|body| body.canonical.as_ref())
        .find(|identity| identity.declaration_name == name)
        .cloned()
        .ok_or_else(|| format!("missing checked {name}").into())
}

/// A public decorated function keeps its checked original, replacement computation and private factory closure.
#[test]
fn public_decorated_function_retains_its_checked_execution() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module(SOURCE)?;
    assert_eq!(module.decorated_bindings.len(), 1);
    let public_root = root(&module, "double")?;
    let private = root(&module, "keep")?;
    let public = BTreeSet::from([public_root.clone()]);
    let bytes = build_surface(&[module], "decorators", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&public_root), "{:#?}", reader.index());
    assert_eq!(reader.fragment(&public_root)?.identity(), Some(&public_root));
    assert_eq!(
        reader.index().declarations.keys().cloned().collect::<BTreeSet<_>>(),
        public
    );
    assert!(reader.index().implementation.contains_key(&private));
    assert!(reader.fragment(&private).is_err());
    Ok(())
}

/// A generic private factory's closed application does not erase the visible decorated declaration's semantics.
#[test]
fn public_decorated_function_keeps_its_generic_factory() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module(
        "def preserve[Tag, F](tag: Tag) -> ((F) -> F):\n    println(\"factory\")\n    return (func) => func\n\n@preserve(7)\npub def increment(value: int) -> int:\n    return value + 1\n",
    )?;
    let public_root = root(&module, "increment")?;
    let bytes = build_surface(
        &[module],
        "decorators",
        "0.1.0",
        &BTreeSet::from([public_root.clone()]),
        &BTreeSet::new(),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&public_root), "{:#?}", reader.index());
    Ok(())
}

/// Missing roles or a substituted physical address refuse rather than publishing the source body as undecorated.
#[test]
fn public_decorated_function_requires_original_and_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let original = checked_module(SOURCE)?;
    let public_root = root(&original, "double")?;
    let public = BTreeSet::from([public_root.clone()]);
    let mut no_original = original.clone();
    no_original.decorated_bindings.clear();
    let mut wrong_source_address = original.clone();
    let other_address = original
        .bodies
        .iter()
        .find(|body| body.name == "keep")
        .ok_or("private decorator frame missing")?
        .direct_call_id
        .clone();
    wrong_source_address
        .decorated_bindings
        .first_mut()
        .ok_or("checked original missing")?
        .original_body
        .direct_call_id = other_address;
    let mut no_wrapper = original.clone();
    no_wrapper
        .bodies
        .retain(|body| body.canonical.as_ref() != Some(&public_root));
    let mut no_replacement = original;
    no_replacement.static_declarations.clear();
    for module in [no_original, no_wrapper, no_replacement, wrong_source_address] {
        let bytes = build_surface(&[module], "decorators", "0.1.0", &public, &BTreeSet::new())?;
        assert!(!SurfaceReader::open(&bytes)?.covers(&public_root));
    }
    Ok(())
}

/// An ordinary public entry retains the module's private decorated initialization without widening public lookup.
#[test]
fn public_entry_retains_private_decorated_initializers() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module(
        "def keep(func: () -> int) -> () -> int:\n    println(func())\n    return func\n\n@keep\ndef zebra() -> int:\n    return 7\n\n@keep\ndef alpha() -> int:\n    return 3\n\npub def plain() -> int:\n    return 5\n\ndef unused() -> int:\n    return 9\n",
    )?;
    let plain = root(&module, "plain")?;
    let zebra = root(&module, "zebra")?;
    let alpha = root(&module, "alpha")?;
    let unused = root(&module, "unused")?;
    let public = BTreeSet::from([plain.clone()]);
    let bytes = build_surface(&[module], "decorators", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&plain), "{:#?}", reader.index());
    assert!(reader.index().implementation.contains_key(&zebra));
    assert!(reader.index().implementation.contains_key(&alpha));
    assert!(!reader.index().implementation.contains_key(&unused));
    assert!(reader.fragment(&zebra).is_err());
    assert!(reader.fragment(&alpha).is_err());
    assert_eq!(
        reader.index().declarations.keys().cloned().collect::<BTreeSet<_>>(),
        public
    );
    Ok(())
}

/// A literal storage read does not enter its module's functions or require unrelated computed initialization.
#[test]
fn public_literal_storage_does_not_enter_decorated_initialization() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module(
        "def keep(func: () -> int) -> () -> int:\n    println(func())\n    return func\n\n@keep\ndef zebra() -> int:\n    return 7\n\npub static count: int = 10\n",
    )?;
    let storage = module
        .static_declarations
        .iter()
        .find(|storage| storage.canonical.declaration_name == "count")
        .ok_or("public storage missing")?
        .canonical
        .clone();
    let private = root(&module, "zebra")?;
    let bytes = build_surface(
        &[module],
        "decorators",
        "0.1.0",
        &BTreeSet::from([storage.clone()]),
        &BTreeSet::new(),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&storage));
    assert!(!reader.index().implementation.contains_key(&private));
    assert!(reader.fragment(&private).is_err());
    Ok(())
}
