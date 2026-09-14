//! `@rust_extern` declarations: where they are, what they claim, and the identities their outputs are keyed by.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::backend::selection::digest_output;
use crate::frontend::ParsedModule;
use crate::frontend::ast::{Declaration, Decorator, Span, Spanned};

#[derive(Debug, Clone)]
pub(crate) struct RustExternDeclContext {
    #[allow(dead_code)]
    pub(crate) file_path: PathBuf,
    #[allow(dead_code)]
    pub(crate) source: String,
    pub(crate) item_name: String,
    pub(crate) rust_module_path: String,
    #[allow(dead_code)]
    pub(crate) span: Span,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RustExternBuildFailureKind {
    UnresolvedBackingItem,
    SignatureMismatch,
    FeatureGatedBackingPath,
}

/// Return whether a declaration's decorators include `rust.extern`.
fn has_rust_extern_decorator(decorators: &[Spanned<Decorator>]) -> bool {
    decorators
        .iter()
        .any(|d| d.node.path.segments.join(".") == "rust.extern")
}

/// Collect source contexts for Rust extern declarations in Rust-backed modules.
pub(crate) fn collect_rust_extern_contexts(modules: &[ParsedModule]) -> Vec<RustExternDeclContext> {
    let mut contexts = Vec::new();
    for module in modules {
        let Some(rust_module) = module.ast.rust_module_path.as_ref().map(|p| p.node.clone()) else {
            continue;
        };
        for decl in &module.ast.declarations {
            match &decl.node {
                Declaration::Function(func) if has_rust_extern_decorator(&func.decorators) => {
                    contexts.push(RustExternDeclContext {
                        file_path: module.file_path.clone(),
                        source: module.source.clone(),
                        item_name: func.name.clone(),
                        rust_module_path: rust_module.clone(),
                        span: decl.span,
                    });
                }
                Declaration::Trait(tr) => {
                    for method in &tr.methods {
                        if has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                Declaration::Model(model) => {
                    for method in &model.methods {
                        if method.node.receiver.is_none() && has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                Declaration::Class(class) => {
                    for method in &class.methods {
                        if method.node.receiver.is_none() && has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                Declaration::Newtype(nt) => {
                    for method in &nt.methods {
                        if method.node.receiver.is_none() && has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    contexts
}

/// Return stable `rust.module::item` labels for Rust extern declarations that influenced this generated build.
pub(crate) fn rust_extern_report_paths(contexts: &[RustExternDeclContext]) -> Vec<String> {
    let mut paths = contexts
        .iter()
        .map(|context| format!("{}::{}", context.rust_module_path, context.item_name))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

/// Content-derived identity of the source modules about to be compiled, computed before codegen runs.
///
/// Ordered by file path rather than collection order so the identity does not depend on module
/// discovery order. Used as a [`BackendSelection`]'s `source_identity`.
pub(crate) fn module_source_identity(modules: &[ParsedModule]) -> String {
    let mut ordered: Vec<&ParsedModule> = modules.iter().collect();
    ordered.sort_by(|left, right| left.file_path.cmp(&right.file_path));
    let parts: Vec<&str> = ordered.iter().map(|module| module.source.as_str()).collect();
    digest_output(&parts)
}

/// Content-derived identity of multi-file generated Rust output, used as a backend execution
/// receipt's `output_identity`.
///
/// `rust_modules` is a `HashMap`, so entries are sorted by module path before digesting; the
/// identity must not depend on `HashMap` iteration order.
pub(crate) fn multi_file_output_identity(main_code: &str, rust_modules: &HashMap<Vec<String>, String>) -> String {
    let mut sorted: Vec<(&Vec<String>, &String)> = rust_modules.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(right.0));
    let mut parts: Vec<&str> = vec![main_code];
    parts.extend(sorted.into_iter().map(|(_, code)| code.as_str()));
    digest_output(&parts)
}
