//! stdlib async API import signatures.
//!
//! The typechecker needs function signatures for `from std.async.* import ...` so calls can be
//! type-checked without parsing the stdlib stubs.
//!
//! FIXME: this is a temporary bridge until RFC 023 wires a compilable stdlib into the compiler pipeline.
//!        Until then, keep these signatures aligned with `stdlib/async/*.incn`.

use crate::frontend::symbols::{FunctionInfo, ResolvedType};

/// Function signatures for `from std.async.* import ...`.
///
/// Returns the signature and the expected std.async submodule name (`time`, `task`, `channel`, `select`).
pub(super) fn async_import_function_info(name: &str) -> Option<(FunctionInfo, &'static str)> {
    match name {
        "sleep" => Some((
            FunctionInfo {
                params: vec![("seconds".to_string(), ResolvedType::Float)],
                return_type: ResolvedType::Unit,
                is_async: true,
                type_params: vec![],
            },
            "time",
        )),
        "sleep_ms" => Some((
            FunctionInfo {
                params: vec![("millis".to_string(), ResolvedType::Int)],
                return_type: ResolvedType::Unit,
                is_async: true,
                type_params: vec![],
            },
            "time",
        )),
        "timeout" => Some((
            FunctionInfo {
                params: vec![
                    ("seconds".to_string(), ResolvedType::Float),
                    ("task".to_string(), ResolvedType::Unknown),
                ],
                return_type: ResolvedType::Unknown,
                is_async: true,
                type_params: vec![],
            },
            "time",
        )),
        "timeout_ms" => Some((
            FunctionInfo {
                params: vec![
                    ("millis".to_string(), ResolvedType::Int),
                    ("task".to_string(), ResolvedType::Unknown),
                ],
                return_type: ResolvedType::Unknown,
                is_async: true,
                type_params: vec![],
            },
            "time",
        )),
        "select_timeout" => Some((
            FunctionInfo {
                params: vec![
                    ("seconds".to_string(), ResolvedType::Float),
                    ("task".to_string(), ResolvedType::Unknown),
                ],
                return_type: ResolvedType::Unknown,
                is_async: true,
                type_params: vec![],
            },
            "select",
        )),
        "spawn" => Some((
            FunctionInfo {
                params: vec![("task".to_string(), ResolvedType::Unknown)],
                return_type: ResolvedType::Unknown,
                is_async: true,
                type_params: vec![],
            },
            "task",
        )),
        "spawn_blocking" => Some((
            FunctionInfo {
                params: vec![("task".to_string(), ResolvedType::Unknown)],
                return_type: ResolvedType::Unknown,
                is_async: true,
                type_params: vec![],
            },
            "task",
        )),
        "yield_now" => Some((
            FunctionInfo {
                params: vec![],
                return_type: ResolvedType::Unit,
                is_async: true,
                type_params: vec![],
            },
            "task",
        )),
        "channel" => Some((
            FunctionInfo {
                params: vec![("buffer".to_string(), ResolvedType::Int)],
                return_type: ResolvedType::Unknown,
                is_async: false,
                type_params: vec![],
            },
            "channel",
        )),
        "unbounded_channel" => Some((
            FunctionInfo {
                params: vec![],
                return_type: ResolvedType::Unknown,
                is_async: false,
                type_params: vec![],
            },
            "channel",
        )),
        "oneshot" => Some((
            FunctionInfo {
                params: vec![],
                return_type: ResolvedType::Unknown,
                is_async: false,
                type_params: vec![],
            },
            "channel",
        )),
        _ => None,
    }
}
