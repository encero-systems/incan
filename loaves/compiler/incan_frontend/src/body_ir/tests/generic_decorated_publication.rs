//! Generic source originals retain invocation-scoped decorator execution without polymorphic module storage.

use std::collections::BTreeSet;

use super::decorated_publication::{checked_module, root};
use incan_semantics_core::executable_representation::{SurfaceReader, build_surface};

/// The complete checked callable type remains open over the source declaration's own parameter.
const SOURCE: &str = "def preserve[F]() -> ((F) -> F):\n    println(\"factory\")\n    return (func) => func\n\n@preserve()\npub def echo[T](value: T) -> T:\n    return value\n";

/// A wrapper retains its original identity, allocates dense frame locals and avoids module-global storage.
#[test]
fn generic_original_retains_its_declaration_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module(SOURCE)?;
    let identity = root(&module, "echo")?;
    let binding = module.decorated_bindings.first().ok_or("generic original missing")?;
    let wrapper = module
        .bodies
        .iter()
        .find(|body| body.canonical.as_ref() == Some(&identity))
        .ok_or("generic wrapper missing")?;
    for (index, local) in wrapper.locals.iter().enumerate() {
        assert_eq!(local.id.index(), index, "generic wrapper local allocation order");
    }
    assert_eq!(binding.original_body.canonical.as_ref(), Some(&identity));
    assert_eq!(binding.original_body.type_parameters, ["T"]);
    assert!(
        !module
            .static_declarations
            .iter()
            .any(|storage| storage.canonical == identity)
    );
    Ok(())
}

/// Publication retains the private factory and generic original without adding either as another public root.
#[test]
fn generic_wrapper_publishes_its_checked_execution_closure() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module(SOURCE)?;
    let identity = root(&module, "echo")?;
    let factory = root(&module, "preserve")?;
    let public = BTreeSet::from([identity.clone()]);
    let bytes = build_surface(&[module], "decorators", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&identity), "{:#?}", reader.index());
    assert_eq!(reader.fragment(&identity)?.identity(), Some(&identity));
    assert!(reader.index().implementation.contains_key(&factory));
    assert!(reader.fragment(&factory).is_err());
    assert_eq!(
        reader.index().declarations.keys().cloned().collect::<BTreeSet<_>>(),
        public
    );
    Ok(())
}

/// A missing original cannot make the undecorated source body satisfy the visible generic callable.
#[test]
fn generic_wrapper_requires_its_original() -> Result<(), Box<dyn std::error::Error>> {
    let mut module = checked_module(SOURCE)?;
    let identity = root(&module, "echo")?;
    module.decorated_bindings.clear();
    let bytes = build_surface(
        &[module],
        "decorators",
        "0.1.0",
        &BTreeSet::from([identity.clone()]),
        &BTreeSet::new(),
    )?;
    assert!(!SurfaceReader::open(&bytes)?.covers(&identity));
    Ok(())
}

/// Closed signatures require legacy's per-instantiation cache and cannot be published as per-call application.
#[test]
fn closed_generic_decorator_signature_keeps_its_cache_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module(
        "def preserve[F]() -> ((F) -> F):\n    println(\"factory\")\n    return (func) => func\n\n@preserve()\npub def cached[T](value: int) -> int:\n    return value\n",
    )?;
    let identity = root(&module, "cached")?;
    let bytes = build_surface(
        &[module],
        "decorators",
        "0.1.0",
        &BTreeSet::from([identity.clone()]),
        &BTreeSet::new(),
    )?;
    assert!(!SurfaceReader::open(&bytes)?.covers(&identity));
    Ok(())
}
