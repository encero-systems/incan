//! stdlib testing API import signatures.
//!
//! The typechecker needs function signatures for `from std.testing import ...` so calls can be
//! type-checked without depending on the stdlib stubs at compile time.
//!
//! FIXME: This is intended to be replaced by RFC 023 (“compilable stdlib and rust module binding”).

use crate::frontend::symbols::{FunctionInfo, ResolvedType};

/// Function signatures for `from std.testing import ...`.
pub(super) fn testing_import_function_info(name: &str) -> Option<FunctionInfo> {
    match name {
        "assert" | "assert_true" | "assert_false" => Some(FunctionInfo {
            params: vec![("condition".to_string(), ResolvedType::Bool)],
            return_type: ResolvedType::Unit,
            is_async: false,
            type_params: vec![],
        }),
        "assert_eq" | "assert_ne" => Some(FunctionInfo {
            params: vec![
                ("left".to_string(), ResolvedType::TypeVar("T".to_string())),
                ("right".to_string(), ResolvedType::TypeVar("T".to_string())),
            ],
            return_type: ResolvedType::Unit,
            is_async: false,
            type_params: vec!["T".to_string()],
        }),
        "fail" => Some(FunctionInfo {
            params: vec![("msg".to_string(), ResolvedType::Str)],
            return_type: ResolvedType::Unit,
            is_async: false,
            type_params: vec![],
        }),
        _ => None,
    }
}
