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

use std::collections::HashMap;

use crate::frontend::ast::{Declaration, Decorator, ImportKind, ImportPath, Program};
use crate::frontend::symbols::{SymbolKind, SymbolTable};
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
        let id = self.lookup(leading_segment)?;
        let sym = self.get(id)?;
        match &sym.kind {
            SymbolKind::Module(info) => Some(info.path.as_slice()),
            _ => None,
        }
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
    for decl in &program.declarations {
        if let Declaration::Import(import) = &decl.node {
            match &import.kind {
                ImportKind::Module(path) => {
                    if let Some(name) = import.alias.as_ref().cloned().or_else(|| path.segments.last().cloned()) {
                        aliases.insert(name, path.segments.clone());
                    }
                }
                ImportKind::From { module, items } => {
                    for item in items {
                        let name = item.alias.as_ref().cloned().unwrap_or_else(|| item.name.clone());
                        let mut resolved = module.segments.clone();
                        resolved.push(item.name.clone());
                        aliases.insert(name, resolved);
                    }
                }
                _ => {}
            }
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
