//! RFC 023: `rust.module()` and `@rust.extern` validation.
//!
//! This module enforces the six diagnostic rules defined in RFC 023 for the `rust.module()` directive and
//! `@rust.extern` decorator:
//!
//! 1. Missing `rust.module()` when `@rust.extern` items exist → error
//! 2. `@rust.extern` with non-trivial body → error
//! 3. `@rust.extern` on instance method → error
//! 4. Duplicate `rust.module()` → error (handled at parse time)
//! 5. Unused `rust.module()` (no `@rust.extern` items) → warning
//! 6. Invalid `rust.module()` path (syntax or unresolved crate) → error

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;

use super::TypeChecker;
use super::collect::decorators::resolve_decorator_id;
use incan_core::lang::decorators::DecoratorId;

/// Regex-style validation of a Rust module path.
///
/// A valid path consists of one or more Rust identifiers separated by `::`.
/// Each segment must match `[a-zA-Z_][a-zA-Z0-9_]*`.
fn is_valid_rust_module_path(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }

    fn is_valid_segment(s: &str) -> bool {
        if s.is_empty() {
            return false;
        }
        let mut chars = s.chars();
        let first = match chars.next() {
            Some(c) => c,
            None => return false,
        };
        if !first.is_ascii_alphabetic() && first != '_' {
            return false;
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    path.split("::").all(is_valid_segment)
}

impl TypeChecker {
    /// Validate `rust.module()` and `@rust.extern` rules across the entire program (RFC 023).
    ///
    /// Called after both collection and checking passes are complete, so all declarations and their decorators are
    /// available for inspection.
    pub(crate) fn validate_rust_module_and_extern(&mut self, program: &Program) {
        let rust_module = &program.rust_module_path;

        // ---- Collect all @rust.extern items ----
        let mut rust_extern_items = Vec::new();
        self.collect_rust_extern_items(&program.declarations, &mut rust_extern_items);

        // ---- Rule 6: Invalid rust.module() path (syntax + crate resolution) ----
        if let Some(directive) = rust_module {
            if !is_valid_rust_module_path(&directive.node) {
                self.errors
                    .push(errors::invalid_rust_module_path(&directive.node, directive.span));
            } else {
                // Crate validation: first segment must be `incan_stdlib` or a declared dependency.
                let first_segment = directive.node.split("::").next().unwrap_or("");
                if first_segment != "incan_stdlib"
                    && let Some(ref crate_names) = self.declared_crate_names
                    && !crate_names.contains(first_segment)
                {
                    self.errors.push(errors::unresolved_rust_module_crate(
                        first_segment,
                        &directive.node,
                        directive.span,
                    ));
                }
            }
        }

        // ---- Rule 1: Missing rust.module() when @rust.extern items exist ----
        if rust_module.is_none() {
            for item in &rust_extern_items {
                self.errors
                    .push(errors::rust_extern_missing_rust_module(&item.name, item.span));
            }
        }

        // ---- Rule 5: Unused rust.module() (no @rust.extern items) ----
        if rust_module.is_some() && rust_extern_items.is_empty() {
            let directive_span = rust_module.as_ref().map_or(Span::default(), |d| d.span);
            self.errors.push(errors::unused_rust_module(directive_span));
        }
    }

    /// Scan declarations for `@rust.extern` items and validate per-item rules.
    ///
    /// Also enforces:
    /// - Rule 2: `@rust.extern` with non-trivial body → error
    /// - Rule 3: `@rust.extern` on instance method → error
    fn collect_rust_extern_items(&mut self, declarations: &[Spanned<Declaration>], items: &mut Vec<RustExternItem>) {
        for decl in declarations {
            match &decl.node {
                Declaration::Function(func) => {
                    if self.has_rust_extern_decorator(&func.decorators) {
                        items.push(RustExternItem {
                            name: func.name.clone(),
                            span: decl.span,
                        });

                        // ---- Rule 2: non-trivial body ----
                        if !is_trivial_body(&func.body) {
                            self.errors
                                .push(errors::rust_extern_non_trivial_body(&func.name, decl.span));
                        }
                    }
                }
                Declaration::Trait(tr) => {
                    for method in &tr.methods {
                        if self.has_rust_extern_decorator(&method.node.decorators) {
                            // RFC 023: `@rust.extern` is allowed on trait default methods.
                            // (It is disallowed on instance methods for classes/models/newtypes.)
                            items.push(RustExternItem {
                                name: method.node.name.clone(),
                                span: method.span,
                            });

                            // ---- Rule 2: non-trivial body ----
                            //
                            // Trait methods may be abstract (`body == None`), which is treated like a `...` stub.
                            // Only a *non-trivial* body is rejected for @rust.extern.
                            if let Some(body) = &method.node.body
                                && !is_trivial_body(body)
                            {
                                self.errors
                                    .push(errors::rust_extern_non_trivial_body(&method.node.name, method.span));
                            }
                        }
                    }
                }
                Declaration::Model(model) => {
                    self.check_methods_for_rust_extern(&model.methods, items);
                }
                Declaration::Class(class) => {
                    self.check_methods_for_rust_extern(&class.methods, items);
                }
                Declaration::Newtype(nt) => {
                    self.check_methods_for_rust_extern(&nt.methods, items);
                }
                _ => {}
            }
        }
    }

    /// Check methods on models/classes/newtypes for `@rust.extern` usage.
    ///
    /// Instance methods with `@rust.extern` are always an error (Rule 3).
    fn check_methods_for_rust_extern(&mut self, methods: &[Spanned<MethodDecl>], items: &mut Vec<RustExternItem>) {
        for method in methods {
            if self.has_rust_extern_decorator(&method.node.decorators) {
                // ---- Rule 3: @rust.extern on instance method ----
                if method.node.receiver.is_some() {
                    self.errors
                        .push(errors::rust_extern_on_instance_method(&method.node.name, method.span));
                } else {
                    items.push(RustExternItem {
                        name: method.node.name.clone(),
                        span: method.span,
                    });
                }

                // ---- Rule 2: non-trivial body ----
                if let Some(body) = &method.node.body
                    && !is_trivial_body(body)
                {
                    self.errors
                        .push(errors::rust_extern_non_trivial_body(&method.node.name, method.span));
                }
            }
        }
    }

    /// Check if a decorator list contains `@rust.extern`.
    fn has_rust_extern_decorator(&self, decorators: &[Spanned<Decorator>]) -> bool {
        decorators
            .iter()
            .any(|d| resolve_decorator_id(&d.node, &self.symbols) == Some(DecoratorId::RustExtern))
    }
}

/// A tracked `@rust.extern` item for cross-referencing with `rust.module()`.
struct RustExternItem {
    name: String,
    span: Span,
}

/// Check if a function body is "trivial" (`...` or `pass` or empty).
///
/// A trivial body contains only `Pass` statements (which the parser generates for `...` and `pass`), or is empty.
fn is_trivial_body(body: &[Spanned<Statement>]) -> bool {
    body.is_empty() || body.iter().all(|stmt| matches!(stmt.node, Statement::Pass))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_rust_module_paths() {
        assert!(is_valid_rust_module_path("incan_stdlib"));
        assert!(is_valid_rust_module_path("incan_stdlib::testing"));
        assert!(is_valid_rust_module_path("my_crate::sub::module"));
        assert!(is_valid_rust_module_path("_private::_inner"));
        assert!(is_valid_rust_module_path("crate1::mod2::func3"));
    }

    #[test]
    fn test_invalid_rust_module_paths() {
        assert!(!is_valid_rust_module_path(""));
        assert!(!is_valid_rust_module_path("my crate"));
        assert!(!is_valid_rust_module_path("my_crate;malicious()"));
        assert!(!is_valid_rust_module_path("123abc"));
        assert!(!is_valid_rust_module_path("::leading"));
        assert!(!is_valid_rust_module_path("trailing::"));
        assert!(!is_valid_rust_module_path("a::::b"));
        assert!(!is_valid_rust_module_path("a::b::"));
        assert!(!is_valid_rust_module_path("a..b"));
        assert!(!is_valid_rust_module_path("has spaces::here"));
        assert!(!is_valid_rust_module_path("semi;colon"));
        assert!(!is_valid_rust_module_path("paren("));
        assert!(!is_valid_rust_module_path("quote\""));
    }
}
