//! stdlib async API import signatures (hardcoded fallback).
//!
//! RFC 023: The typechecker first attempts to load signatures from the parsed `.incn` files via
//! [`super::super::stdlib_loader::StdlibAstCache`]. This module is the **fallback** for async
//! submodules whose `.incn` stubs fail to parse (e.g. `stdlib/async/time.incn` contains model
//! declarations with non-trivial method bodies that the current parser cannot handle).
//!
//! Modules that parse successfully (e.g. `channel.incn`) do NOT need entries here — the AST cache
//! handles them. Only modules that fail parsing need fallback entries.
//!
//! Once the parser supports full stdlib compilation, these hardcoded signatures can be removed.
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
        // NOTE: channel, unbounded_channel, oneshot are NOT listed here — stdlib/async/channel.incn
        // parses successfully and signatures are loaded via StdlibAstCache.
        _ => None,
    }
}
