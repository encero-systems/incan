//! `@rust_extern` declarations: where they are, what they claim, and the identities their outputs are keyed by.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::backend::selection::digest_output;
use incan_frontend::ParsedModule;
use incan_frontend::ast::{Declaration, Decorator, Span, Spanned};

#[derive(Debug, Clone)]
pub struct RustExternDeclContext {
    #[allow(dead_code)]
    pub file_path: PathBuf,
    #[allow(dead_code)]
    pub source: String,
    pub item_name: String,
    pub rust_module_path: String,
    #[allow(dead_code)]
    pub span: Span,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustExternBuildFailureKind {
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
pub fn collect_rust_extern_contexts(modules: &[ParsedModule]) -> Vec<RustExternDeclContext> {
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
pub fn rust_extern_report_paths(contexts: &[RustExternDeclContext]) -> Vec<String> {
    let mut paths = contexts
        .iter()
        .map(|context| format!("{}::{}", context.rust_module_path, context.item_name))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

/// Resolve an ordinary source publisher's extern ABI demand to the native item its existing backend actually calls.
///
/// Canonical `Deserialize.from_json` is a compiler-provided trait default calling `parse_or_error`, not a free
/// function in `rust.module`. Only the original Data source issuer, its registered JSON member and the exact
/// parsed trait method can select that helper. Diagnostic labels and legacy ABI collection remain unchanged.
pub(crate) fn ordinary_rust_extern_abi_path(
    context: &RustExternDeclContext,
    modules: &[ParsedModule],
    plan: &incan_provider::ProviderPlan,
) -> crate::error::CliResult<String> {
    use incan_lang::lang::stdlib::{self, StdlibJsonTraitId};

    let original = format!("{}::{}", context.rust_module_path, context.item_name);
    let Some(owner) = plan.standard_source_publication() else {
        return Ok(original);
    };
    if context.rust_module_path != format!("{}::serde", stdlib::facets::DATA)
        || context.item_name != "from_json"
        || !owner.namespace_roots().contains(&stdlib::STDLIB_SERDE)
    {
        return Ok(original);
    }
    let module_path = [stdlib::STDLIB_ROOT, stdlib::STDLIB_SERDE, stdlib::STDLIB_JSON].map(str::to_string);
    let member = owner
        .open_source_module(&module_path)
        .map_err(|error| crate::error::CliError::failure(error.to_string()))?;
    if context.file_path != member.path() {
        return Ok(original);
    }
    if context.source.as_bytes()
        != member
            .verified_bytes()
            .map_err(|error| crate::error::CliError::failure(error.to_string()))?
    {
        return Err(crate::error::CliError::failure(
            "ordinary JSON extern source differs from its original owned member",
        ));
    }
    let canonical_method = modules
        .iter()
        .filter(|module| module.file_path == context.file_path)
        .any(|module| {
            module.ast.declarations.iter().any(|declaration| {
                let Declaration::Trait(trait_decl) = &declaration.node else {
                    return false;
                };
                stdlib::stdlib_json_trait_id_for_identity(&module_path, &trait_decl.name)
                    == Some(StdlibJsonTraitId::Deserialize)
                    && trait_decl.methods.iter().any(|method| {
                        method.span == context.span
                            && method.node.name == context.item_name
                            && has_rust_extern_decorator(&method.node.decorators)
                    })
            })
        });
    if canonical_method {
        Ok(format!("{}::json::__private::parse_or_error", stdlib::facets::DATA))
    } else {
        Ok(original)
    }
}

/// Content-derived identity of the source modules about to be compiled, computed before codegen runs.
///
/// Ordered by file path rather than collection order so the identity does not depend on module discovery order. Used as
/// a [`BackendSelection`]'s `source_identity`.
pub fn module_source_identity(modules: &[ParsedModule]) -> String {
    let mut ordered: Vec<&ParsedModule> = modules.iter().collect();
    ordered.sort_by(|left, right| left.file_path.cmp(&right.file_path));
    let parts: Vec<&str> = ordered.iter().map(|module| module.source.as_str()).collect();
    digest_output(&parts)
}

/// Content-derived identity of multi-file generated Rust output, used as a backend execution receipt's
/// `output_identity`.
///
/// `rust_modules` is a `HashMap`, so entries are sorted by module path before digesting; the identity must not depend
/// on `HashMap` iteration order.
pub fn multi_file_output_identity(main_code: &str, rust_modules: &HashMap<Vec<String>, String>) -> String {
    let mut sorted: Vec<(&Vec<String>, &String)> = rust_modules.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(right.0));
    let mut parts: Vec<&str> = vec![main_code];
    parts.extend(sorted.into_iter().map(|(_, code)| code.as_str()));
    digest_output(&parts)
}
