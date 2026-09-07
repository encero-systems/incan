//! Which source modules the direct replacement profile will execute, and why it refuses the rest.
//!
//! This is the replacement backend's admission contract at the *source* level. Its Body-IR counterpart lives in
//! [`validate_call_profile`](super::validate_call_profile), which decides the same question one stage later, about
//! constructs the profile has already agreed to lower. Keeping both in this module tree means the answer to "can
//! this backend execute this?" has one owner rather than being reassembled by a caller from separate predicates.
//!
//! Every refusal carries the span it was measured at, and the caller pairs that span with the file the module came
//! from. A span means nothing beside the wrong file, which is why the profile reports per module rather than
//! per program.

use incan_semantics_core::HirSourceSpan;

use crate::frontend::ast::{Declaration, ImportKind, Visibility};
use crate::frontend::body_ir::{
    is_direct_replacement_fieldless_enum, is_direct_replacement_plain_model, is_direct_replacement_value_enum,
};

use super::ReplacementExecutionError;

/// Whether a module reached by the analysis is held to the source profile at all.
///
/// The one analysis also pulls in the standard-library modules the graph reached, which arrive under the generated
/// `__incan_std` namespace rather than the source `std` spelling. Those are not project source and were never meant
/// to satisfy a source-only profile: `import std.async` legitimately reaches stdlib modules full of classes and
/// imports. Only the project's own modules are executed here, so only they are held to it.
#[must_use]
pub fn module_is_held_to_source_profile(module_path: &[String]) -> bool {
    !matches!(
        module_path.first().map(String::as_str),
        Some(incan_core::lang::stdlib::STDLIB_ROOT | incan_core::lang::stdlib::INCAN_STD_NAMESPACE)
    )
}

/// Whether one import names another module of the project currently being built.
///
/// #1260 executes a call that leaves the entry module, so an import naming a sibling of that module is inside the
/// profile: the session analyzes the whole source graph at once, and the execution graph resolves the callee's
/// canonical identity to the module that declares it. Everything reaching outside that graph stays refused -- the
/// standard library, a `pub::` package dependency, and every Rust or Python interop form each bring a boundary the
/// source-only profile has no evidence for, and each is owned by a separate child of #989.
fn is_local_module_import(import: &crate::frontend::ast::ImportDecl) -> bool {
    let path = match &import.kind {
        ImportKind::Module(path) | ImportKind::From { module: path, .. } => path,
        // `pub::` dependencies, Rust crates and Python modules are other packages by construction.
        _ => return false,
    };
    let Some(first) = path.segments.first() else {
        return false;
    };
    first.as_str() != incan_core::lang::stdlib::STDLIB_ROOT
}

/// Describe a Rust-interop import in terms of the boundary it crosses, when it is one.
///
/// A `rust::` import is refused for a different reason than an ordinary one, and #1262 requires the difference to be
/// visible: a reader has to be able to tell a construct this profile has not reached yet from a boundary that needs a
/// host it does not have. Reporting both as "import declaration" told them neither, and named the wrong thing to go
/// looking for.
///
/// The crate is named because it is the actionable part. Which crate a call would have entered is what a reader needs
/// to know, and it is the identity the eventual interop plan is selected against.
fn interop_boundary_description(import: &crate::frontend::ast::ImportDecl) -> Option<String> {
    let (crate_name, form) = match &import.kind {
        ImportKind::RustCrate { crate_name, .. } => (crate_name, "module import"),
        ImportKind::RustFrom { crate_name, .. } => (crate_name, "item import"),
        ImportKind::Python(module) => {
            return Some(format!(
                "Python interop import of `{module}`, which this backend has no host for"
            ));
        }
        _ => return None,
    };
    Some(format!(
        "Rust interop {form} of crate `{crate_name}`: executing it needs a Rust interop host, and this route has none"
    ))
}

/// Return the first source-module boundary this profile does not admit, with its original Incan source span.
///
/// The caller supplies one module's checked program and pairs any refusal with that module's file. Returning `None`
/// means every top-level declaration in this module is one the direct route can execute.
#[must_use]
pub fn source_profile_refusal(program: &crate::frontend::ast::Program) -> Option<ReplacementExecutionError> {
    if let Some(rust_module) = &program.rust_module_path {
        return Some(ReplacementExecutionError::unsupported_profile(
            "Rust interop `rust.module` directive",
            HirSourceSpan::new(rust_module.span.start, rust_module.span.end),
        ));
    }
    let mut async_activation_seen = false;
    for declaration in &program.declarations {
        // An alias is a binding to a declaration, not a declaration of its own: it introduces a second name for a
        // symbol that already exists and is admitted, or refused, on its own terms wherever it is declared. Refusing
        // the binding would refuse a name rather than any behavior, and the target is reached through the same
        // canonical identity either way -- which is the property #1261 exists to prove.
        if matches!(
            declaration.node,
            Declaration::Function(_) | Declaration::Docstring(_) | Declaration::Alias(_)
        ) || matches!(&declaration.node, Declaration::Model(model) if is_direct_replacement_plain_model(model))
            || matches!(&declaration.node, Declaration::Enum(enum_decl) if is_direct_replacement_fieldless_enum(enum_decl))
            || matches!(&declaration.node, Declaration::Enum(enum_decl) if is_direct_replacement_value_enum(enum_decl))
        {
            continue;
        }
        if let Declaration::Import(import) = &declaration.node {
            let exact_async_activation = matches!(
                (&import.visibility, &import.kind, &import.alias),
                (Visibility::Private, ImportKind::Module(path), None)
                    if !path.is_absolute
                        && path.parent_levels == 0
                        && path.segments == ["std", "async"]
            );
            if exact_async_activation && !async_activation_seen {
                async_activation_seen = true;
                continue;
            }
            if !exact_async_activation && is_local_module_import(import) {
                continue;
            }
            let description = if exact_async_activation {
                "duplicate `import std.async` replacement activation".to_string()
            } else if let Some(boundary) = interop_boundary_description(import) {
                boundary
            } else {
                "import declaration".to_string()
            };
            return Some(ReplacementExecutionError::unsupported_profile(
                description,
                HirSourceSpan::new(declaration.span.start, declaration.span.end),
            ));
        }
        return Some(ReplacementExecutionError::unsupported_profile(
            "non-function top-level declaration",
            HirSourceSpan::new(declaration.span.start, declaration.span.end),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{lexer, parser};

    /// Parse one module the way the profile receives it: checked source, not a fragment.
    fn program(source: &str) -> crate::frontend::ast::Program {
        let tokens = lexer::lex(source).unwrap_or_else(|errors| panic!("fixture did not lex: {errors:?}"));
        parser::parse(&tokens).unwrap_or_else(|errors| panic!("fixture did not parse: {errors:?}"))
    }

    /// Return the refusal description for a module the profile does not admit.
    fn refusal(source: &str) -> String {
        source_profile_refusal(&program(source))
            .unwrap_or_else(|| panic!("expected a refusal for:\n{source}"))
            .to_string()
    }

    /// Only the project's own modules answer to a source-only profile.
    ///
    /// The analysis pulls in the standard library the graph reached, under the generated `__incan_std` namespace as
    /// well as the source `std` spelling. Holding those to this profile would refuse `import std.async`, which is
    /// inside it, because the stdlib modules behind it are full of classes and imports.
    #[test]
    fn the_standard_library_is_not_held_to_the_source_profile() {
        assert!(module_is_held_to_source_profile(&["my_app".to_string()]));
        assert!(module_is_held_to_source_profile(&[
            "helpers".to_string(),
            "text".to_string()
        ]));

        assert!(!module_is_held_to_source_profile(&[
            incan_core::lang::stdlib::STDLIB_ROOT.to_string(),
            "io".to_string()
        ]));
        assert!(!module_is_held_to_source_profile(&[
            incan_core::lang::stdlib::INCAN_STD_NAMESPACE.to_string(),
            "async".to_string()
        ]));
    }

    /// The declarations the direct route can execute pass without a refusal.
    #[test]
    fn the_admitted_declaration_forms_raise_no_refusal() {
        assert!(source_profile_refusal(&program("def main() -> int:\n  return 42\n")).is_none());
        assert!(
            source_profile_refusal(&program("model Point:\n  x: int\n\ndef main() -> int:\n  return 1\n")).is_none()
        );
        assert!(
            source_profile_refusal(&program("enum Flag:\n  On\n  Off\n\ndef main() -> int:\n  return 1\n")).is_none()
        );

        // A sibling module of the project being built is inside the profile: the session analyzes the whole graph
        // and the execution graph resolves the callee to the module that declares it.
        assert!(
            source_profile_refusal(&program(
                "from helper import bump\n\ndef main() -> int:\n  return bump(1)\n"
            ))
            .is_none()
        );

        // One `import std.async` activates the async profile rather than crossing a boundary.
        assert!(source_profile_refusal(&program("import std.async\n\ndef main() -> int:\n  return 1\n")).is_none());
    }

    /// Each boundary names itself, so a reader can tell a missing host from an unreached construct.
    #[test]
    fn every_refused_boundary_names_what_it_crosses() {
        assert!(
            refusal("import rust::serde_json\n\ndef main() -> int:\n  return 1\n")
                .contains("Rust interop module import of crate `serde_json`")
        );
        assert!(
            refusal("from rust::incan_stdlib::text import normalize\n\ndef main() -> int:\n  return 1\n")
                .contains("Rust interop item import of crate `incan_stdlib`")
        );
        assert!(
            refusal("import python \"os\"\n\ndef main() -> int:\n  return 1\n")
                .contains("Python interop import of `os`")
        );
        assert!(
            refusal("rust.module(\"incan_stdlib::testing\")\n\ndef main() -> int:\n  return 1\n")
                .contains("Rust interop `rust.module` directive")
        );

        // A standard-library import is genuinely a construct this profile has not reached, and must not borrow an
        // interop host it never needed. The pair is what makes the distinction mean anything.
        let standard_library = refusal("import std.io\n\ndef main() -> int:\n  return 1\n");
        assert!(standard_library.contains("import declaration"), "{standard_library}");
        assert!(!standard_library.contains("interop host"), "{standard_library}");

        // Two exact activations. Reachable here and not through the CLI, where typechecking rejects the duplicate
        // binding first -- which is why this branch had no coverage until the rule moved somewhere testable.
        assert!(
            refusal("import std.async\nimport std.async\n\ndef main() -> int:\n  return 1\n")
                .contains("duplicate `import std.async` replacement activation")
        );

        // An aliased activation is not the activation form: it binds a name, so it crosses the boundary like any
        // other standard-library import.
        assert!(
            refusal("import std.async as runtime\n\ndef main() -> int:\n  return 1\n").contains("import declaration")
        );
        assert!(
            refusal("class Pair:\n  left: int\n\ndef main() -> int:\n  return 1\n")
                .contains("non-function top-level declaration")
        );
    }

    /// A refusal carries the span of the construct that caused it, not of the module.
    ///
    /// The span is what lets a caller point at the declaration in the file it came from, which is the whole reason
    /// the profile reports per module rather than per program.
    #[test]
    fn a_refusal_carries_the_offending_declarations_span() {
        let source = "def ok() -> int:\n  return 1\n\nclass Pair:\n  left: int\n";
        let error = source_profile_refusal(&program(source)).unwrap_or_else(|| panic!("expected a refusal"));
        let span = error
            .primary_span()
            .unwrap_or_else(|| panic!("a profile refusal must carry its span"));

        assert!(
            source[span.start..span.end].starts_with("class Pair"),
            "the refusal must point at the class, got `{}`",
            &source[span.start..span.end]
        );
    }
}
