//! Deferred layouts of a compiled Loaf's declarations: what an install records instead of building each layout, and
//! the build a lookup runs the first time it reads one.

use super::*;
use crate::library_manifest::compiler_records::RecordFamily;
use crate::library_manifest::declaration_records::{qualified_name, restored_nominal_declaration};
use crate::library_manifest::install_record::LoafInstallRecord;
use crate::typechecker::TransitiveLayout;
use crate::typechecker::transitive_layouts::{LayoutDeclaration, LayoutRecipe, LayoutValue};
use serde::Deserialize;
use serde_json::Value;

/// The declaring-module scopes one install has read, by declaring module, so each is read once per install.
pub(in crate::typechecker::collect) type DeclaringScopes = HashMap<Vec<String>, Arc<HashMap<String, String>>>;

impl TypeChecker {
    /// The layout `recipe` builds, deferred until a lookup reads it.
    pub(in crate::typechecker::collect) fn deferred_layout<T: LayoutValue>(
        &self,
        recipe: LayoutRecipe,
    ) -> TransitiveLayout<T> {
        let layout = TransitiveLayout::deferred(recipe);
        #[cfg(test)]
        if self.dependency_interface_preparation_cache.eager_layouts {
            layout.get(self);
        }
        layout
    }

    /// Build one deferred layout's symbol kind exactly as its install would have: the declaration, each remapping in
    /// order, then the compiled class field provider. Installed layouts use their admitted records, with the root
    /// projection and the canonical declaration addressed independently. Legacy Loaves use their manifests.
    pub(in crate::typechecker) fn build_deferred_layout(&self, recipe: &LayoutRecipe) -> Option<SymbolKind> {
        let mut kind = match &recipe.declaration {
            LayoutDeclaration::Root { family, name } => self.build_root_layout(&recipe.library, *family, name)?,
            LayoutDeclaration::Api {
                qualified,
                module,
                declaration,
            } => self.build_authored_layout(&recipe.library, qualified, |manifest| {
                let api = manifest.contract_metadata.api.as_ref()?;
                api.modules.get(*module)?.declarations.get(*declaration)
            })?,
            LayoutDeclaration::ApiPath(path) => {
                let (module, name) = Self::normalized_api_declaration_target(path)?;
                let qualified = qualified_name(&module, &name);
                self.build_authored_layout(&recipe.library, &qualified, |manifest| {
                    Self::api_declaration_for_target_path(manifest, path)
                })?
            }
        };
        for remapping in &recipe.remappings {
            self.remap_symbol_kind_with_import_aliases(&mut kind, remapping.as_ref());
        }
        if let Some(library) = &recipe.field_provider {
            Self::mark_compiled_class_field_provider(&mut kind, library);
        }
        Some(kind)
    }

    /// Read one root projection by the name it is admitted under. Its member identities are carried by the
    /// projection, independently of that public spelling. Legacy Loaves use the same projection in their manifest.
    fn build_root_layout(&self, library: &str, family: RecordFamily, name: &str) -> Option<SymbolKind> {
        if let Some(values) = self.loaves.named_layout_records(library, family, name).ok()? {
            let [value] = values.as_slice() else {
                return None;
            };
            if value.get("name").and_then(Value::as_str) != Some(name) {
                return None;
            }
            return self.layout_from_root_record(family, value);
        }
        let exports = &self.loaves.manifest(library)?.exports;
        match family {
            RecordFamily::Model => {
                let export = exports.models.iter().find(|export| export.name == name)?;
                Some(SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export))))
            }
            RecordFamily::Class => {
                let export = exports.classes.iter().find(|export| export.name == name)?;
                Some(SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export))))
            }
            RecordFamily::Enum => {
                let export = exports.enums.iter().find(|export| export.name == name)?;
                Some(SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))))
            }
            RecordFamily::Newtype => {
                let export = exports.newtypes.iter().find(|export| export.name == name)?;
                Some(SymbolKind::Type(TypeInfo::Newtype(
                    self.newtype_info_from_manifest(export),
                )))
            }
            RecordFamily::Trait => {
                let export = exports.traits.iter().find(|export| export.name == name)?;
                Some(SymbolKind::Trait(self.trait_info_from_manifest(export)))
            }
            _ => None,
        }
    }

    /// Decode the published root projection into the same checker layout as a manifest export, preserving its
    /// visibility and materializable defaults. Same-package member partials and executable defaults enter elsewhere.
    pub(super) fn layout_from_root_record(&self, family: RecordFamily, value: &Value) -> Option<SymbolKind> {
        match family {
            RecordFamily::Model => Some(SymbolKind::Type(TypeInfo::Model(
                self.model_info_from_manifest(&Deserialize::deserialize(value).ok()?),
            ))),
            RecordFamily::Class => Some(SymbolKind::Type(TypeInfo::Class(
                self.class_info_from_manifest(&Deserialize::deserialize(value).ok()?),
            ))),
            RecordFamily::Enum => Some(SymbolKind::Type(TypeInfo::Enum(
                self.enum_info_from_manifest(&Deserialize::deserialize(value).ok()?),
            ))),
            RecordFamily::Newtype => Some(SymbolKind::Type(TypeInfo::Newtype(
                self.newtype_info_from_manifest(&Deserialize::deserialize(value).ok()?),
            ))),
            RecordFamily::Trait => Some(SymbolKind::Trait(
                self.trait_info_from_manifest(&Deserialize::deserialize(value).ok()?),
            )),
            _ => None,
        }
    }

    /// Read one authored declaration's facets. Only absence of an admitted signature selects the legacy lookup.
    fn build_authored_layout(
        &self,
        library: &str,
        qualified: &str,
        legacy_declaration: impl for<'a> FnOnce(&'a LibraryManifest) -> Option<&'a ApiDeclaration>,
    ) -> Option<SymbolKind> {
        let signatures = self
            .loaves
            .named_layout_records(library, RecordFamily::DeclarationSignature, qualified)
            .ok()?;
        let Some(signatures) = signatures else {
            let manifest = self.loaves.manifest(library)?;
            return self.symbol_kind_from_api_declaration(
                manifest.contract_metadata.api.as_ref(),
                legacy_declaration(manifest)?,
            );
        };
        let layouts = self
            .loaves
            .named_layout_records(library, RecordFamily::Layout, qualified)
            .ok()?;
        let defaults = self
            .loaves
            .named_layout_records(library, RecordFamily::Default, qualified)
            .ok()?;
        self.layout_from_declaration_records(
            signatures.as_slice(),
            layouts.as_deref().map_or(&[], Vec::as_slice),
            defaults.as_deref().map_or(&[], Vec::as_slice),
        )
    }

    /// Reassemble a nominal or trait layout with its authored name and identities. Each optional facet has at most
    /// one value. The signature is copied into the reconstruction so the named records stay immutable for readers.
    pub(super) fn layout_from_declaration_records(
        &self,
        signatures: &[Value],
        layouts: &[Value],
        defaults: &[Value],
    ) -> Option<SymbolKind> {
        let [signature] = signatures else {
            return None;
        };
        if layouts.len() > 1 || defaults.len() > 1 {
            return None;
        }
        let declaration = serde_json::from_value(restored_nominal_declaration(
            signature.clone(),
            layouts.first(),
            defaults.first(),
        ))
        .ok()?;
        self.symbol_kind_from_api_declaration(None, &declaration)
    }

    /// The scope of the module that declares `canonical` in the identity graph `graph` of `library`, as
    /// [`Self::remap_symbol_kind_through_declaring_module`] reads it, read once per declaring module into `scopes`.
    /// `None` when the identity graph names no declaring module, where that remap changes nothing.
    pub(in crate::typechecker::collect) fn declaring_module_scope(
        &self,
        library: &str,
        record: &LoafInstallRecord,
        canonical: Option<&CanonicalSymbolId>,
        scopes: &mut DeclaringScopes,
    ) -> Option<Arc<HashMap<String, String>>> {
        let source_path = record.identity_graph.declaration_source_path(canonical?)?;
        let (_, module_path) = source_path.split_last()?;
        if let Some(scope) = scopes.get(module_path) {
            return Some(Arc::clone(scope));
        }
        let scope = Arc::new(self.recorded_declaration_scope(library, record, module_path));
        scopes.insert(module_path.to_vec(), Arc::clone(&scope));
        Some(scope)
    }
}
