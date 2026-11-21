use super::is_box_carrier_path;
use super::{ancestral_rust_display, ancestral_rust_path};

#[test]
fn box_carrier_paths_are_recognized_through_every_reexport() {
    for spelling in [
        "Box",
        "std::boxed::Box",
        "alloc::boxed::Box",
        "prost::alloc::boxed::Box",
        "carrier::alloc::boxed::Box",
        "::std::boxed::Box",
    ] {
        assert!(is_box_carrier_path(spelling), "{spelling} names the Box carrier");
    }
    for other in ["Rc", "alloc::rc::Rc", "demo::Box", "std::boxed::BoxLike"] {
        assert!(!is_box_carrier_path(other), "{other} is not the Box carrier");
    }
}

/// Every spelling of one standard item must fold onto the crate that defines it.
#[test]
fn std_reexports_fold_onto_their_defining_crate() {
    assert_eq!(ancestral_rust_path("std::boxed::Box"), "alloc::boxed::Box");
    assert_eq!(ancestral_rust_path("rust::std::boxed::Box"), "alloc::boxed::Box");
    assert_eq!(ancestral_rust_path("alloc::boxed::Box"), "alloc::boxed::Box");
    assert_eq!(ancestral_rust_path("std::option::Option"), "core::option::Option");
    assert_eq!(ancestral_rust_path("std::rc::Rc"), "alloc::rc::Rc");
    assert_eq!(ancestral_rust_path("std::sync::Arc"), "alloc::sync::Arc");
    assert_eq!(ancestral_rust_path("std::sync::Mutex"), "std::sync::Mutex");
    assert_eq!(ancestral_rust_path("std::cell::RefCell"), "core::cell::RefCell");
    assert_eq!(ancestral_rust_path("Box"), "alloc::boxed::Box");
    assert_eq!(ancestral_rust_path("rust::Box"), "alloc::boxed::Box");
    assert_eq!(ancestral_rust_path("Boxed"), "Boxed");
    assert_eq!(
        ancestral_rust_path("datafusion::common::Column"),
        "datafusion::common::Column"
    );
}

/// Displays that differ only in re-export spelling compare equal after rewriting.
#[test]
fn displays_unify_across_reexport_spellings() {
    assert_eq!(
        ancestral_rust_display("std::option::Option<std::boxed::Box<substrait::proto::Type>>"),
        ancestral_rust_display("core::option::Option<alloc::boxed::Box<substrait::proto::Type>>"),
    );
    assert_eq!(
        ancestral_rust_display("&mut rust::std::vec::Vec<u8>"),
        "&mut alloc::vec::Vec<u8>"
    );
    assert_eq!(
        ancestral_rust_display("Box<datafusion_expr::expr::Expr>"),
        ancestral_rust_display("std::boxed::Box<datafusion_expr::expr::Expr>"),
    );
    assert_eq!(
        ancestral_rust_display("rust::alloc::rc::Rc<std::cell::RefCell<std::fs::File>,alloc::Global>"),
        ancestral_rust_display("std::rc::Rc<std::cell::RefCell<std::fs::File>>"),
    );
    assert_eq!(ancestral_rust_display("Vec<u8, alloc::Global>"), "alloc::vec::Vec<u8>");
}
