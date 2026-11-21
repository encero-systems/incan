//! Native signatures that name a compiled Incan library's own carriers keep their Incan types.

use super::*;

/// The standard library's frozen text and bytes carriers are Incan's frozen text and bytes, as a compiled Incan
/// `str` or `bytes` constant is.
#[test]
fn frozen_text_and_bytes_carriers_are_incan_frozen_values() {
    let checker = TypeChecker::new();
    for (display, expected) in [
        ("incan_std_primitives::frozen::FrozenStr", ResolvedType::FrozenStr),
        ("incan_std_primitives::frozen::FrozenBytes", ResolvedType::FrozenBytes),
    ] {
        assert_eq!(checker.native_rust_signature_type(display, ""), expected, "{display}");
        assert_eq!(checker.resolved_type_from_rust_display(display), expected, "{display}");
    }
}

/// A compiled library's source-named layout (`<crate>::__incan_layout_<carrier>::<Name>`) is the source declaration
/// its carrier names, alone and nested in a collection; an unknown carrier keeps its native path.
#[test]
fn compiled_library_layout_paths_keep_their_source_declaration() {
    let key = "__incan_carrier_model_3_646570_5_56616c7565";
    let mut checker = TypeChecker::new();
    checker
        .transitive_pub_types
        .insert(key.to_owned(), vec![TypeInfo::Builtin]);
    let path = format!("dep_lib::__incan_layout_{key}::Value");
    let named = ResolvedType::Named(key.to_owned());
    assert_eq!(checker.resolved_type_from_rust_display(&path), named);
    assert_eq!(checker.native_rust_signature_type(&path, ""), named);
    assert_eq!(
        checker.resolved_type_from_rust_display(&format!("alloc::vec::Vec<{path}>")),
        ResolvedType::Generic("List".to_string(), vec![named])
    );
    let unknown = "dep_lib::__incan_layout___incan_carrier_model_3_646570_1_59::Y";
    assert_eq!(
        checker.resolved_type_from_rust_display(unknown),
        ResolvedType::RustPath(unknown.to_owned())
    );
}
