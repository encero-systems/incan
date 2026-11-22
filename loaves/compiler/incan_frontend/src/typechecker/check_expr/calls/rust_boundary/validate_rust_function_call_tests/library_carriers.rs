//! Native signatures that name a compiled Incan library's own carriers keep their Incan types.

use super::*;

/// The standard library's frozen text and bytes carriers named by a Rust path stay native Rust types, as a source
/// that imports them by `rust::` path (the runtime's frozen text observation) relies on: mapping them to Incan frozen
/// values would make `str(value)` lower to the runtime's own formatting call.
#[test]
fn frozen_carriers_named_by_rust_path_stay_native() {
    let checker = TypeChecker::new();
    for display in [
        "incan_std_primitives::frozen::FrozenStr",
        "incan_std_primitives::frozen::FrozenBytes",
    ] {
        let native = ResolvedType::RustPath(display.to_owned());
        assert_eq!(checker.native_rust_signature_type(display, ""), native, "{display}");
        assert_eq!(checker.resolved_type_from_rust_display(display), native, "{display}");
    }
}

/// A compiled library's source-named layout (`<crate>::__incan_layout_<carrier>::<Name>`) is the source declaration
/// its carrier names, alone, borrowed and nested in a collection; an unknown carrier keeps its native path.
#[test]
fn compiled_library_layout_paths_keep_their_source_declaration() {
    let key = "__incan_carrier_model_3_646570_5_56616c7565";
    let mut checker = TypeChecker::new();
    checker.transitive_pub_types.insert(
        key.to_owned(),
        vec![crate::typechecker::TransitiveLayout::ready(TypeInfo::Builtin)],
    );
    let path = format!("dep_lib::__incan_layout_{key}::Value");
    let named = ResolvedType::Named(key.to_owned());
    assert_eq!(checker.resolved_type_from_rust_display(&path), named);
    assert_eq!(checker.native_rust_signature_type(&path, ""), named);
    assert_eq!(
        checker.resolved_type_from_rust_display(&format!("alloc::vec::Vec<{path}>")),
        ResolvedType::Generic("List".to_string(), vec![named])
    );
    let borrowed = ResolvedType::Ref(Box::new(ResolvedType::Named(key.to_owned())));
    assert_eq!(checker.native_rust_signature_type(&format!("&{path}"), ""), borrowed);
    assert_eq!(checker.resolved_type_from_rust_display(&format!("&{path}")), borrowed);
    let unknown = "dep_lib::__incan_layout___incan_carrier_model_3_646570_1_59::Y";
    assert_eq!(
        checker.resolved_type_from_rust_display(unknown),
        ResolvedType::RustPath(unknown.to_owned())
    );
}

/// A compiled library's ordinary layout stays at its crate root under its carrier key (`<crate>::<carrier>`, the
/// standard primitives' `Value` enum): it is the source declaration its carrier names, alone, borrowed and nested in a
/// collection inside a result; an unknown carrier keeps its native path.
#[test]
fn compiled_library_root_carriers_keep_their_source_declaration() {
    let key = "__incan_carrier_enum_3_646570_5_56616c7565";
    let mut checker = TypeChecker::new();
    checker.transitive_pub_types.insert(
        key.to_owned(),
        vec![crate::typechecker::TransitiveLayout::ready(TypeInfo::Builtin)],
    );
    let path = format!("dep_lib::{key}");
    let named = ResolvedType::Named(key.to_owned());
    assert_eq!(checker.resolved_type_from_rust_display(&path), named);
    assert_eq!(checker.native_rust_signature_type(&path, ""), named);
    assert_eq!(
        checker.resolved_type_from_rust_display(&format!(
            "core::result::Result<alloc::vec::Vec<{path}>, alloc::string::String>"
        )),
        ResolvedType::Generic(
            "Result".to_string(),
            vec![
                ResolvedType::Generic("List".to_string(), vec![named]),
                ResolvedType::Str
            ]
        )
    );
    let borrowed = ResolvedType::Ref(Box::new(ResolvedType::Named(key.to_owned())));
    assert_eq!(checker.native_rust_signature_type(&format!("&{path}"), ""), borrowed);
    assert_eq!(checker.resolved_type_from_rust_display(&format!("&{path}")), borrowed);
    let unknown = "dep_lib::__incan_carrier_enum_3_646570_1_59";
    assert_eq!(
        checker.resolved_type_from_rust_display(unknown),
        ResolvedType::RustPath(unknown.to_owned())
    );
}
