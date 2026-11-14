//! Imports from an installed Loaf settled from its own records (RFC 179): a public namespace by its path and a
//! function or constant by its module-qualified name, so checking a program reads what it uses rather than the Loaf's
//! whole checked API. Whatever these records cannot settle exactly is left to the Loaf's manifest, as before.

use incan_lang::lang::types::builtin_type_name;
use serde_json::Value;

use super::*;
use crate::library_manifest::declaration_records::{DeclarationFacet, restored_declaration};
use crate::library_manifest::{CanonicalIdentityExport, ExportIdentity, TypeRef};

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

    /// Resolve a function or a constant published by a public namespace from the namespace's record and the member's
    /// own declaration records. `None` when they cannot settle it exactly: no records, another kind of member, an
    /// alias, overloads, a signature naming anything but built-in types, or no single declaration identity; the
    /// manifest then resolves the member.
    pub(in crate::typechecker) fn record_module_member(
        &mut self,
        library: &str,
        module_path: &[String],
        member: &str,
    ) -> Option<ResolvedPublicModuleSymbol> {
        let admitted = self.loaves.admitted(library)?;
        let members = self.record_namespace_members(library, module_path, member)?;
        let [found] = members.as_slice() else {
            return None;
        };
        let source_path: Vec<String> = serde_json::from_value(found.get("source_path")?.clone()).ok()?;
        let (source_name, source_module) = source_path.split_last()?;
        let qualified = source_path.join("::");
        let signatures = self
            .loaves
            .declaration_facts(library, DeclarationFacet::Signature, &qualified)
            .ok()??;
        let [signature] = signatures.as_slice() else {
            return None;
        };
        let defaults = self
            .loaves
            .declaration_facts(library, DeclarationFacet::Defaults, &qualified)
            .ok()??;
        if defaults.len() > 1 {
            return None;
        }
        let declaration: ApiDeclaration =
            serde_json::from_value(restored_declaration(signature.clone(), defaults.first())).ok()?;
        let built_in = match &declaration {
            ApiDeclaration::Function(function) => {
                function.type_params.is_empty()
                    && function.params.iter().all(|param| built_in_type(&param.ty))
                    && built_in_type(&function.return_type)
            }
            ApiDeclaration::Const(constant) => built_in_type(&constant.ty),
            _ => false,
        };
        if !built_in {
            return None;
        }
        let public_path = std::iter::once(admitted.name.clone())
            .chain(module_path.iter().cloned())
            .chain(std::iter::once(member.to_owned()))
            .collect::<Vec<_>>();
        let namespace = self.loaves.public_namespace(library, module_path).ok()??;
        let identities: Vec<ExportIdentity> = serde_json::from_value(namespace.get("identities")?.clone()).ok()?;
        let matching = identities
            .iter()
            .filter(|identity| identity.public_path == public_path && identity.source_path == source_path)
            .collect::<Vec<_>>();
        let [identity] = matching.as_slice() else {
            return None;
        };
        let canonical = identity.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate)?;
        let mut kind = self.symbol_kind_from_api_declaration(None, &declaration)?;
        Self::mark_compiled_class_field_provider(&mut kind, library);
        let mut source_module_path = admitted.binding_root;
        source_module_path.extend(source_module.iter().cloned());
        Some(ResolvedPublicModuleSymbol {
            kind,
            canonical: Some(canonical),
            source_module_path,
            source_name: source_name.clone(),
        })
    }
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

/// Whether `ty` names built-in types only, which no provider's type remapping can change.
fn built_in_type(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Named { name, origin } => origin.is_none() && builtin_type_name(name).is_some(),
        TypeRef::Applied { name, args, origin } => {
            origin.is_none() && builtin_type_name(name).is_some() && args.iter().all(built_in_type)
        }
        TypeRef::Function { params, return_type } => params.iter().all(built_in_type) && built_in_type(return_type),
        _ => false,
    }
}
