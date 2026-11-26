//! Rust byte buffers are Incan bytes wherever a signature names them.

use super::*;

/// A `Vec<u8>` result, alone or inside `Result` and `Option`, is Incan `bytes`.
#[test]
fn byte_buffers_are_incan_bytes_inside_results() {
    let checker = TypeChecker::new();
    for (display, expected) in [
        ("alloc::vec::Vec<u8>", ResolvedType::Bytes),
        (
            "core::result::Result<alloc::vec::Vec<u8>, alloc::string::String>",
            ResolvedType::Generic("Result".into(), vec![ResolvedType::Bytes, ResolvedType::Str]),
        ),
        (
            "std::result::Result<std::vec::Vec<u8>, std::string::String>",
            ResolvedType::Generic("Result".into(), vec![ResolvedType::Bytes, ResolvedType::Str]),
        ),
        ("std::vec::Vec<u8>", ResolvedType::Bytes),
        (
            "core::option::Option<alloc::vec::Vec<u8>>",
            ResolvedType::Generic("Option".into(), vec![ResolvedType::Bytes]),
        ),
    ] {
        assert_eq!(checker.resolved_type_from_rust_display(display), expected, "{display}");
    }
}

/// A Rust function returning a byte buffer, alone or inside `Result`, gives Incan `bytes`, as a byte field does.
#[test]
fn byte_buffer_results_of_rust_calls_are_incan_bytes() {
    let span = Span::new(0, 1);
    for (return_type, expected) in [
        ("std::vec::Vec<u8>", ResolvedType::Bytes),
        (
            "std::result::Result<std::vec::Vec<u8>, std::string::String>",
            ResolvedType::Generic("Result".into(), vec![ResolvedType::Bytes, ResolvedType::Str]),
        ),
    ] {
        let mut checker = TypeChecker::new();
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: return_type.to_string(),
            is_async: false,
            is_unsafe: false,
        };
        let result = checker.validate_rust_function_call("rust::demo::encode", &sig, &[], span);
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        assert_eq!(result, expected, "{return_type}");
    }
}
