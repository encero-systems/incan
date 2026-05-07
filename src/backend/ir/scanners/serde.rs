//! Serde/JSON feature detection.
//!
//! Activation is primarily import-driven (RFC 022): importing from `std.serde` signals
//! that serde is required.
//!
//! We also check for bare `json_stringify()` calls (legacy builtin that doesn't yet require an import).
//! Once `json_stringify` is behind `from std.serde.json import json_stringify`, the
//! builtin fallback here can be removed entirely.

use crate::frontend::ast::{Expr, Program};
use crate::frontend::ast_walk::any_expr_in_program;
use incan_core::lang::builtins::{self, BuiltinFnId};

use super::decorators::has_stdlib_import;

/// Detect whether serde-backed runtime support is needed for this program.
///
/// This is the broad compatibility detector used by codegen. It returns `true` for both:
/// - import-driven activation (`std.serde.*`)
/// - legacy non-import usage through bare `json_stringify`
pub fn detect_serde_usage(program: &Program) -> bool {
    // Fast path: explicit `import std.serde.json` or `from std.serde import ...`
    if has_stdlib_import(program, "serde") {
        return true;
    }

    detect_serde_non_import_usage(program)
}

/// Detect serde requirements that do *not* come from explicit `std.serde` imports.
///
/// This helper intentionally captures compatibility behavior that cannot yet be represented by import activation alone.
pub fn detect_serde_non_import_usage(program: &Program) -> bool {
    // TODO: Remove this legacy fallback once `json_stringify` requires an explicit import
    // (e.g. `from std.serde.json import json_stringify`). Until then, bare calls activate serde
    // without an import, which breaks the "imported module → activate its features" invariant
    // that the rest of the stdlib follows.
    program_has_json_stringify(program)
}

// ---------------------------------------------------------------------------
// Legacy `json_stringify()` builtin detection
//
// This will be removable once json_stringify requires `from std.serde.json import ...`.
// ---------------------------------------------------------------------------

fn program_has_json_stringify(program: &Program) -> bool {
    any_expr_in_program(program, |expr| {
        let Expr::Call(callee, _type_args, _args) = expr else {
            return false;
        };
        let Expr::Ident(name) = &callee.node else {
            return false;
        };
        builtins::from_str(name.as_str()) == Some(BuiltinFnId::JsonStringify)
    })
}
