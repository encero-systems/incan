//! `TryFrom[str]` bridge planning for compiler-provided primitive and newtype conversions.

use crate::frontend::ast::{Declaration, ImportKind, Program};
use incan_core::lang::trait_capabilities;

use super::capability_bridge;

/// Return whether a program imports the source-owned `TryFrom` contract that carries string conversion support.
pub(super) fn imports_std_string_try_from_contract(program: &Program) -> bool {
    capability_bridge::imports_contract(program, trait_capabilities::string_try_from())
        || imports_std_environ_typed_accessors(program)
}

/// Return whether any source module in one compilation imports the string conversion contract.
pub(super) fn compilation_imports_std_string_try_from_contract(
    main: &Program,
    deps: &[(&str, &Program, Option<Vec<String>>)],
) -> bool {
    imports_std_string_try_from_contract(main)
        || deps
            .iter()
            .any(|(_, program, _)| imports_std_string_try_from_contract(program))
}

/// Return whether a program imports the stdlib surface that consumes `TryFrom[str]` bounds on caller-defined types.
///
/// `std.environ.get_as()` owns that bound inside the compiled artifact. A caller that imports the module may therefore
/// need a compiler-provided local-newtype implementation even when it does not spell `TryFrom` in its own source.
fn imports_std_environ_typed_accessors(program: &Program) -> bool {
    program.declarations.iter().any(|decl| {
        let Declaration::Import(import) = &decl.node else {
            return false;
        };
        match &import.kind {
            ImportKind::Module(path) => path.segments == ["std", "environ"],
            ImportKind::From { module, items } if module.segments == ["std", "environ"] => {
                items.iter().any(|item| item.name == "get_as")
            }
            _ => false,
        }
    })
}

/// Generated bridge configuration shared by every emitter in one compilation.
#[derive(Debug, Clone)]
pub(super) struct StringTryFromBridgeConfig {
    pub(super) emit_local_newtype_impls: bool,
}

impl StringTryFromBridgeConfig {
    /// Build a bridge configuration for generated internal modules.
    pub(super) fn for_internal_module(uses_contract: bool) -> Self {
        Self {
            emit_local_newtype_impls: uses_contract,
        }
    }

    /// Build the crate-root configuration for local newtype conversion implementations.
    pub(super) fn for_crate_root(uses_contract: bool) -> Self {
        if !uses_contract {
            return Self::for_internal_module(false);
        }
        Self {
            emit_local_newtype_impls: true,
        }
    }
}
