//! Shared helper utilities for resolving decorator paths.
//!
//! Decorators like `@std.web.route` can also be referenced through local aliases:
//!
//! - `import std.web as web` → `@web.route(...)`
//! - `from std.web import route` → `@route(...)`
//!
//! Multiple compiler subsystems need consistent resolution:
//! - the typechecker (via the `SymbolTable`)
//! - scanner passes (via collected import aliases)
//! - LSP and CLI utilities (also via collected import aliases)
//!
//! This module centralizes that logic so the behavior stays in sync.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::{Declaration, Decorator, ImportKind, ImportPath, Program};
use crate::frontend::symbols::SymbolTable;
use incan_core::lang::builtins::{self, BuiltinFnId};
use incan_core::lang::decorators;
use incan_core::lang::stdlib;

/// A lookup source for resolving the first decorator path segment as an import alias.
///
/// If `@alias.something` is used, a lookup provides the module path segments for `alias`.
pub trait DecoratorPrefixLookup {
    /// Return the path segments to substitute for the given leading segment, if it is an alias.
    fn prefix_segments(&self, leading_segment: &str) -> Option<&[String]>;
}

impl DecoratorPrefixLookup for HashMap<String, Vec<String>> {
    fn prefix_segments(&self, leading_segment: &str) -> Option<&[String]> {
        self.get(leading_segment).map(|v| v.as_slice())
    }
}

impl DecoratorPrefixLookup for SymbolTable {
    fn prefix_segments(&self, leading_segment: &str) -> Option<&[String]> {
        self.import_binding_path(leading_segment)
    }
}

/// Helper function to add `crate` / `super` prefixes to path segments.
pub fn path_segments_with_prefix(path: &ImportPath) -> Vec<String> {
    let mut segments = Vec::new();
    if path.is_absolute {
        segments.push("crate".to_string());
    } else {
        for _ in 0..path.parent_levels {
            segments.push("super".to_string());
        }
    }
    segments.extend(path.segments.iter().cloned());
    segments
}

/// Collect import aliases from the program.
///
/// This collects:
/// - `import foo.bar as baz` → `baz` maps to `["foo", "bar"]`
/// - `from foo.bar import qux as q` → `q` maps to `["foo", "bar", "qux"]`
pub fn collect_import_aliases(program: &Program) -> HashMap<String, Vec<String>> {
    let mut aliases = HashMap::new();
    let mut occupied = std::iter::once(builtins::as_str(BuiltinFnId::Print).to_string())
        .chain(
            builtins::aliases(BuiltinFnId::Print)
                .iter()
                .map(|name| (*name).to_string()),
        )
        .collect::<HashSet<_>>();
    for decl in &program.declarations {
        match &decl.node {
            Declaration::Import(import) => match &import.kind {
                ImportKind::Module(path) => {
                    if let Some(name) = import.alias.as_ref().cloned().or_else(|| path.segments.last().cloned())
                        && occupied.insert(name.clone())
                    {
                        aliases.insert(name, path.segments.clone());
                    }
                }
                ImportKind::From { module, items } => {
                    for item in items {
                        let name = item.alias.as_ref().cloned().unwrap_or_else(|| item.name.clone());
                        let mut resolved = module.segments.clone();
                        resolved.push(item.name.clone());
                        if occupied.insert(name.clone()) {
                            aliases.insert(name, resolved);
                        }
                    }
                }
                ImportKind::PubLibrary { library, path } => {
                    let name = import
                        .alias
                        .clone()
                        .or_else(|| path.last().cloned())
                        .unwrap_or_else(|| library.clone());
                    let mut resolved = vec!["pub".to_string(), library.clone()];
                    resolved.extend(path.iter().cloned());
                    if occupied.insert(name.clone()) {
                        aliases.insert(name, resolved);
                    }
                }
                ImportKind::PubFrom { library, path, items } => {
                    for item in items {
                        let name = item.alias.as_ref().cloned().unwrap_or_else(|| item.name.clone());
                        let mut resolved = vec!["pub".to_string(), library.clone()];
                        resolved.extend(path.iter().cloned());
                        resolved.push(item.name.clone());
                        if occupied.insert(name.clone()) {
                            aliases.insert(name, resolved);
                        }
                    }
                }
                ImportKind::RustCrate { crate_name, path, .. } => {
                    let name = import
                        .alias
                        .as_ref()
                        .cloned()
                        .or_else(|| path.last().cloned())
                        .unwrap_or_else(|| crate_name.clone());
                    occupied.insert(name);
                }
                ImportKind::RustFrom { items, .. } => {
                    occupied.extend(
                        items
                            .iter()
                            .map(|item| item.alias.as_ref().cloned().unwrap_or_else(|| item.name.clone())),
                    );
                }
                ImportKind::Python(_) => {
                    if let Some(name) = &import.alias {
                        occupied.insert(name.clone());
                    }
                }
            },
            Declaration::Const(item) if item.name != "__derives__" => {
                occupied.insert(item.name.clone());
            }
            Declaration::Static(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Model(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Class(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Trait(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Alias(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Partial(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::TypeAlias(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Newtype(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Enum(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Function(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Capability(item) => {
                occupied.insert(item.name.clone());
            }
            Declaration::Const(_)
            | Declaration::Docstring(_)
            | Declaration::TestModule(_)
            | Declaration::VocabBlock(_) => {}
        }
    }
    aliases
}

/// Collect aliases for direct Rust imports.
///
/// This intentionally stays separate from [`collect_import_aliases`] because `rust::...` imports are not Incan module
/// paths. Lowering uses this for Rust derive macro passthrough such as
/// `from rust::serde @ "1.0" import Deserialize` → `serde::Deserialize`.
pub fn collect_rust_import_aliases(program: &Program) -> HashMap<String, Vec<String>> {
    let mut aliases = HashMap::new();
    for decl in &program.declarations {
        let Declaration::Import(import) = &decl.node else {
            continue;
        };

        match &import.kind {
            ImportKind::RustCrate { crate_name, path, .. } => {
                let mut resolved = vec![crate_name.clone()];
                resolved.extend(path.iter().cloned());
                let name = import
                    .alias
                    .as_ref()
                    .cloned()
                    .or_else(|| path.last().cloned())
                    .unwrap_or_else(|| crate_name.clone());
                aliases.insert(name, resolved);
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } => {
                for item in items {
                    let name = item.alias.as_ref().cloned().unwrap_or_else(|| item.name.clone());
                    let mut resolved = vec![crate_name.clone()];
                    resolved.extend(path.iter().cloned());
                    resolved.push(item.name.clone());
                    aliases.insert(name, resolved);
                }
            }
            _ => {}
        }
    }
    aliases
}

/// Resolve a decorator path to a module path.
///
/// Rules:
/// - absolute/parented paths keep their `crate`/`super` prefix
/// - known decorator namespace roots (`std`, `rust`) are already-canonical and returned as-is
/// - otherwise, if the leading segment is an alias, it is substituted and the remaining segments are appended
pub fn resolve_decorator_path(dec: &Decorator, lookup: &impl DecoratorPrefixLookup) -> Vec<String> {
    if dec.path.is_absolute || dec.path.parent_levels > 0 {
        return path_segments_with_prefix(&dec.path);
    }

    let segments = dec.path.segments.clone();
    if segments.is_empty() {
        return segments;
    }

    // Known decorator namespace roots (`std`, `rust`) are already canonical — don't rewrite them.
    if segments[0] == stdlib::STDLIB_ROOT || decorators::is_known_decorator_namespace(&segments[0]) {
        return segments;
    }

    if let Some(prefix) = lookup.prefix_segments(&segments[0]) {
        let mut resolved: Vec<String> = prefix.to_vec();
        resolved.extend(segments.iter().skip(1).cloned());
        return resolved;
    }

    segments
}
