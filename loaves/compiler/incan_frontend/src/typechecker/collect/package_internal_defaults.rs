//! A same-package unit's view of its project library's package-internal section (DD-0007).

use super::*;
use crate::library_manifest::{LibraryManifest, PackageInternalExports};

impl TypeChecker {
    /// Mark the parameters a unit of the library's own package may omit, when this unit is one. The public family left
    /// them required because their defaults are no values it can carry; the callee's own default frame computes them
    /// at the omitted call site, as a linked Loaf call's defaults always run.
    pub(in crate::typechecker::collect) fn open_package_internal_defaults(
        &self,
        manifest: &LibraryManifest,
        canonical: Option<&CanonicalSymbolId>,
        kind: &mut SymbolKind,
    ) {
        let Some(section) = manifest.package_internal.as_ref() else {
            return;
        };
        if !PackageInternalExports::opened_by(self.project_library.as_deref(), &manifest.name) {
            return;
        }
        match kind {
            SymbolKind::Function(info) => {
                if let Some(identity) = canonical {
                    mark_source_defaults(section, identity, info);
                }
            }
            SymbolKind::FunctionOverloads(overloads) => {
                for overload in overloads {
                    if let Some(identity) = overload.identity.as_ref() {
                        mark_source_defaults(section, identity, &mut overload.info);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Give each parameter of `info` the section lists for `identity` its default.
fn mark_source_defaults(section: &PackageInternalExports, identity: &CanonicalSymbolId, info: &mut FunctionInfo) {
    for parameter in section.omittable_parameters(identity) {
        if let Some(param) = info.params.get_mut(parameter) {
            param.has_default = true;
        }
    }
}
