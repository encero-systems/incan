//! The emitter declares the standard library line it generates code for; the facet it links in tests carries the
//! line it actually is. The two are kept equal by `scripts/check_ring_versions.py` at the manifest level; this root
//! keeps the compiled constants honest, so a bump that reaches one crate and not the other fails here before it
//! reaches a generated project's version check.

use incan_emit::GENERATED_FOR_STDLIB_VERSION;
use incan_std_core::version::INCAN_STDLIB_VERSION;

#[test]
fn the_emitter_declares_the_stdlib_line_the_core_facet_is() {
    assert_eq!(
        GENERATED_FOR_STDLIB_VERSION, INCAN_STDLIB_VERSION,
        "incan_emit::GENERATED_FOR_STDLIB_VERSION must equal the incan_std_core crate version"
    );
}

#[test]
fn the_declared_line_is_compatible_with_itself() {
    assert!(incan_std_core::version::stdlib_versions_compatible(
        GENERATED_FOR_STDLIB_VERSION.as_bytes(),
        INCAN_STDLIB_VERSION.as_bytes()
    ));
}
