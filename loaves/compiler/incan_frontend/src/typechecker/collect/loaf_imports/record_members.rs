//! Imports from an installed Loaf settled from its own records (RFC 179): a public namespace by its path and a
//! function, constant or static by its module-qualified name, so checking a program reads what it uses rather than the
//! Loaf's whole checked API. Whatever these records cannot settle exactly is left to the Loaf's manifest, as before.

use serde_json::Value;

use super::record_types::RecordSpellings;
use super::*;
use crate::library_manifest::declaration_records::{DeclarationFacet, OVERLOAD_FIELD, restored_declaration};
use crate::library_manifest::{CanonicalIdentityExport, ExportIdentity, LibraryIdentityGraph};

impl TypeChecker {
    /// Bind `from <loaf>[.<path>] import <namespace>, …` from the Loaf's namespace records when every item names one of
    /// its public namespaces and nothing else that import could bind. Returns `false`, binding nothing, when the
    /// records cannot settle that, and the manifest then decides the import.
    pub(super) fn collect_record_namespace_imports(
        &mut self,
        import: &LoafImport,
        items: &[ImportItem],
        span: Span,
    ) -> bool {
        let library = import.key.as_str();
        if self.loaf_is_not_prepared(library) {
            return false;
        }
        let Some(admitted) = self.loaves.admitted(library) else {
            return false;
        };
        if !items
            .iter()
            .all(|item| self.record_namespace_item(library, &import.module, &item.name))
        {
            return false;
        }
        let mut source_module = admitted.binding_root.clone();
        source_module.extend(import.module.iter().cloned());
        for item in items {
            let local_name = Self::import_item_local_name(item);
            if self.validate_protected_builtin_binding(&local_name, span) {
                continue;
            }
            self.validate_root_namespace(&local_name, span);
            if !self.loaf_import_needs_binding(&local_name, &source_module, &item.name, span) {
                continue;
            }
            let mut child = import.module.clone();
            child.push(item.name.clone());
            let mut namespace_path = admitted.binding_root.clone();
            namespace_path.extend(child.iter().cloned());
            let target_identity = self.loaf_module_identity(library, &child);
            self.define_import_symbol(local_name, namespace_path, false, target_identity, span);
        }
        true
    }

    /// Bind `from <loaf>.<path> import <member>, …` from the Loaf's records when every item is a member they settle
    /// exactly (see [`Self::record_module_member`]); a member takes precedence over a child namespace of its name, as
    /// on the manifest route. Returns `false`, binding nothing, when one is not, and the manifest then decides the
    /// import.
    pub(super) fn collect_record_member_imports(
        &mut self,
        import: &LoafImport,
        items: &[ImportItem],
        span: Span,
    ) -> bool {
        let library = import.key.as_str();
        if import.module.is_empty() || self.loaf_is_not_prepared(library) {
            return false;
        }
        let Some(admitted) = self.loaves.admitted(library) else {
            return false;
        };
        let mut members = Vec::with_capacity(items.len());
        for item in items {
            let Some(resolved) = self.record_module_member(library, &import.module, &item.name) else {
                return false;
            };
            let Some(source_module_path) = resolved
                .source_module_path
                .strip_prefix(admitted.binding_root.as_slice())
            else {
                return false;
            };
            let marker_free = self.record_proves_no_testing_marker(library, source_module_path, &resolved.source_name);
            members.push((
                PublicModuleMember {
                    kind: resolved.kind,
                    canonical: resolved.canonical,
                    source_module_path: source_module_path.to_vec(),
                    source_name: resolved.source_name,
                    type_alias: None,
                    partial_projection: None,
                },
                marker_free,
            ));
        }
        let mut source_module = admitted.binding_root.clone();
        source_module.extend(import.module.iter().cloned());
        for (item, (member, marker_free)) in items.iter().zip(members) {
            let local_name = Self::import_item_local_name(item);
            if self.validate_protected_builtin_binding(&local_name, span) {
                continue;
            }
            self.validate_root_namespace(&local_name, span);
            if !self.loaf_import_needs_binding(&local_name, &source_module, &item.name, span) {
                continue;
            }
            let mut binding_path = source_module.clone();
            binding_path.push(item.name.clone());
            self.define_pub_module_import_symbol(library, local_name.clone(), binding_path, member, span);
            self.record_standard_surface_import(
                &admitted.binding_root,
                &import.module,
                item,
                &local_name,
                span,
                marker_free,
            );
        }
        true
    }

    /// Whether the signature records of `name`, declared in the module `module` of `library`, prove it is no
    /// `std.testing` marker: none of its declarations carries the marker decorator. `false` when no record says.
    fn record_proves_no_testing_marker(&self, library: &str, module: &[String], name: &str) -> bool {
        let qualified = module
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(name))
            .collect::<Vec<_>>()
            .join("::");
        let Ok(Some(signatures)) = self
            .loaves
            .declaration_facts(library, DeclarationFacet::Signature, &qualified)
        else {
            return false;
        };
        signatures.iter().all(|signature| {
            signature
                .get("decorators")
                .and_then(Value::as_array)
                .is_some_and(|decorators| {
                    decorators.iter().all(|decorator| {
                        decorator.get("path").and_then(Value::as_array).is_none_or(|path| {
                            path.len() != 1
                                || path[0].as_str() != Some(crate::testing_markers::TESTING_MARKER_DECORATOR)
                        })
                    })
                })
        })
    }

    /// Whether the records prove `item`, imported from the namespace `module` of `library`, is one of its public
    /// namespaces and no declaration: no package-root export for a root import, no member of the parent namespace
    /// otherwise.
    fn record_namespace_item(&self, library: &str, module: &[String], item: &str) -> bool {
        let mut path = module.to_vec();
        path.push(item.to_owned());
        if !matches!(self.loaves.public_namespace(library, &path), Ok(Some(_))) {
            return false;
        }
        if module.is_empty() {
            return matches!(self.loaves.may_export_at_root(library, item), Ok(Some(false)));
        }
        self.record_proves_no_member(library, module, item)
    }

    /// Whether the records prove that the public namespace `module_path` of `library` publishes no member named
    /// `member`, so resolving that member finds nothing, exactly as the manifest's checked API would.
    pub(in crate::typechecker) fn record_proves_no_member(
        &self,
        library: &str,
        module_path: &[String],
        member: &str,
    ) -> bool {
        self.record_namespace_members(library, module_path, member)
            .is_some_and(|members| members.is_empty())
    }

    /// The members the public namespace `module_path` of `library` publishes under `member`, from its records. When
    /// `member` is also a child namespace, that child's record names them, so the parent's record, which carries every
    /// member of its child modules, is not read. `None` when the records cannot tell.
    fn record_namespace_members(&self, library: &str, module_path: &[String], member: &str) -> Option<Vec<Value>> {
        if module_path.is_empty() {
            return None;
        }
        let mut child = module_path.to_vec();
        child.push(member.to_owned());
        if let Ok(Some(namespace)) = self.loaves.public_namespace(library, &child)
            && let Some(members) = namespace.get("parent_members").and_then(Value::as_array)
        {
            return Some(members.clone());
        }
        let namespace = self.loaves.public_namespace(library, module_path).ok()??;
        Some(namespace_members(&namespace, member).into_iter().cloned().collect())
    }

    /// Resolve a function, constant or static published by a public namespace from the namespace's record and the
    /// member's own declaration records; an overloaded function is resolved from each overload's records. `None` when
    /// they cannot settle it exactly: no records, another kind of member, an alias, a bounded type parameter, a type
    /// its declaring module's records do not settle (see [`Self::record_type_remapping`]), a default record that names
    /// no overload, or no single declaration identity; the manifest then resolves the member.
    pub(in crate::typechecker) fn record_module_member(
        &mut self,
        library: &str,
        module_path: &[String],
        member: &str,
    ) -> Option<ResolvedPublicModuleSymbol> {
        let admitted = self.loaves.admitted(library)?;
        let members = self.record_namespace_members(library, module_path, member)?;
        let (found, overloads) = members.split_first()?;
        let source_path: Vec<String> = serde_json::from_value(found.get("source_path")?.clone()).ok()?;
        if overloads
            .iter()
            .any(|overload| overload.get("source_path") != found.get("source_path"))
        {
            return None;
        }
        let (source_name, source_module) = source_path.split_last()?;
        let qualified = source_path.join("::");
        let signatures = self
            .loaves
            .declaration_facts(library, DeclarationFacet::Signature, &qualified)
            .ok()??;
        let defaults = self
            .loaves
            .declaration_facts(library, DeclarationFacet::Defaults, &qualified)
            .ok()??;
        let declarations = restored_declarations(&signatures, &defaults)?;
        let mut remapping = HashMap::new();
        for declaration in &declarations {
            let spellings = RecordSpellings::of_member(declaration)?;
            remapping.extend(self.record_type_remapping(library, source_module, &spellings, &mut Vec::new())?);
        }
        let public_path = std::iter::once(admitted.name.clone())
            .chain(module_path.iter().cloned())
            .chain(std::iter::once(member.to_owned()))
            .collect::<Vec<_>>();
        let namespace = self.loaves.public_namespace(library, module_path).ok()??;
        let exports: Vec<ExportIdentity> = serde_json::from_value(namespace.get("identities")?.clone()).ok()?;
        let (mut kind, canonical) = match declarations.as_slice() {
            [declaration] => {
                let matching = exports
                    .iter()
                    .filter(|identity| identity.public_path == public_path && identity.source_path == source_path)
                    .collect::<Vec<_>>();
                let [identity] = matching.as_slice() else {
                    return None;
                };
                let canonical = identity.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate)?;
                (
                    self.symbol_kind_from_api_declaration(None, declaration)?,
                    Some(canonical),
                )
            }
            overloads => self.record_overloads(overloads, exports, &public_path, &source_path)?,
        };
        self.remap_symbol_kind_with_import_aliases(&mut kind, &remapping);
        Self::mark_compiled_class_field_provider(&mut kind, library);
        let mut source_module_path = admitted.binding_root;
        source_module_path.extend(source_module.iter().cloned());
        Some(ResolvedPublicModuleSymbol {
            kind,
            canonical,
            source_module_path,
            source_name: source_name.clone(),
        })
    }

    /// The overload set the function declarations `overloads` publish at `public_path`, each candidate with its own
    /// identity among `exports`, the namespace's identities, in declaration order, and the set's one identity when
    /// every candidate shares it, as the manifest route selects them. `None` when a declaration is no function.
    fn record_overloads(
        &self,
        overloads: &[ApiDeclaration],
        exports: Vec<ExportIdentity>,
        public_path: &[String],
        source_path: &[String],
    ) -> Option<(SymbolKind, Option<CanonicalSymbolId>)> {
        let graph = LibraryIdentityGraph {
            exports,
            ..Default::default()
        };
        let identities = graph.identities_for_public_source_path(public_path, source_path);
        let mut candidates = Vec::with_capacity(overloads.len());
        for (index, declaration) in overloads.iter().enumerate() {
            if !matches!(declaration, ApiDeclaration::Function(_)) {
                return None;
            }
            let SymbolKind::Function(info) = self.symbol_kind_from_api_declaration(None, declaration)? else {
                return None;
            };
            candidates.push(FunctionOverloadInfo {
                info,
                span: Span::default(),
                identity: identities.get(index).cloned().flatten(),
            });
        }
        let canonical = identities
            .first()
            .cloned()
            .flatten()
            .filter(|first| identities.iter().all(|identity| identity.as_ref() == Some(first)))
            .or_else(|| graph.canonical_for_public_path(public_path));
        Some((SymbolKind::FunctionOverloads(candidates), canonical))
    }
}

/// The declarations the `signatures` records of one name describe, in declaration order, each with the defaults of its
/// own `defaults` record put back. An overload's default record names the overload it belongs to; `None` when one
/// names none, as in a set converted before it did, or when a record does not decode.
fn restored_declarations(signatures: &[Value], defaults: &[Value]) -> Option<Vec<ApiDeclaration>> {
    if let [signature] = signatures {
        if defaults.len() > 1 {
            return None;
        }
        let declaration = serde_json::from_value(restored_declaration(signature.clone(), defaults.first())).ok()?;
        return Some(vec![declaration]);
    }
    let mut by_overload = std::collections::BTreeMap::new();
    for record in defaults {
        let entries = record.as_array()?;
        let overload = entries.first()?.get(OVERLOAD_FIELD)?.as_u64()?;
        if entries
            .iter()
            .any(|entry| entry.get(OVERLOAD_FIELD).and_then(Value::as_u64) != Some(overload))
            || overload >= signatures.len() as u64
            || by_overload.insert(overload, record).is_some()
        {
            return None;
        }
    }
    signatures
        .iter()
        .zip(0u64..)
        .map(|(signature, overload)| {
            serde_json::from_value(restored_declaration(
                signature.clone(),
                by_overload.get(&overload).copied(),
            ))
            .ok()
        })
        .collect()
}

/// The members a namespace record publishes under `name`.
fn namespace_members<'a>(namespace: &'a Value, name: &str) -> Vec<&'a Value> {
    namespace
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|member| member.get("name").and_then(Value::as_str) == Some(name))
        .collect()
}
