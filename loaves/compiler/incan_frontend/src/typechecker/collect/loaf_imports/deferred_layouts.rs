//! Deferred layouts of a compiled Loaf's declarations: what an install records instead of building each layout, and
//! the build a lookup runs the first time it reads one.

use super::*;
use crate::typechecker::TransitiveLayout;
use crate::typechecker::transitive_layouts::{LayoutDeclaration, LayoutRecipe, LayoutValue};

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
    /// order, then the compiled class field provider. `None` when the declaration is no longer where the recipe names
    /// it, which an immutable manifest never allows.
    pub(in crate::typechecker) fn build_deferred_layout(&self, recipe: &LayoutRecipe) -> Option<SymbolKind> {
        let manifest = recipe.manifest.as_ref();
        let exports = &manifest.exports;
        let mut kind = match &recipe.declaration {
            LayoutDeclaration::Model(index) => SymbolKind::Type(TypeInfo::Model(
                self.model_info_from_manifest(exports.models.get(*index)?),
            )),
            LayoutDeclaration::Class(index) => SymbolKind::Type(TypeInfo::Class(
                self.class_info_from_manifest(exports.classes.get(*index)?),
            )),
            LayoutDeclaration::Enum(index) => {
                SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(exports.enums.get(*index)?)))
            }
            LayoutDeclaration::Newtype(index) => SymbolKind::Type(TypeInfo::Newtype(
                self.newtype_info_from_manifest(exports.newtypes.get(*index)?),
            )),
            LayoutDeclaration::Trait(index) => {
                SymbolKind::Trait(self.trait_info_from_manifest(exports.traits.get(*index)?))
            }
            LayoutDeclaration::Export(name) => SymbolKind::Type(self.manifest_nominal_type_info(manifest, name)?),
            LayoutDeclaration::Api { module, declaration } => {
                let api = manifest.contract_metadata.api.as_ref()?;
                let declaration = api.modules.get(*module)?.declarations.get(*declaration)?;
                self.symbol_kind_from_api_declaration(Some(api), declaration)?
            }
            LayoutDeclaration::ApiPath(path) => {
                let declaration = Self::api_declaration_for_target_path(manifest, path)?;
                self.symbol_kind_from_api_declaration(manifest.contract_metadata.api.as_ref(), declaration)?
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

    /// The scope of the module that declares `canonical` in the identity graph `graph` of `library`, as
    /// [`Self::remap_symbol_kind_through_declaring_module`] reads it, read once per declaring module into `scopes`.
    /// `None` when the identity graph names no declaring module, where that remap changes nothing.
    pub(in crate::typechecker::collect) fn declaring_module_scope(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        graph: &crate::library_manifest::LibraryIdentityGraph,
        canonical: Option<&CanonicalSymbolId>,
        scopes: &mut DeclaringScopes,
    ) -> Option<Arc<HashMap<String, String>>> {
        let source_path = graph.declaration_source_path(canonical?)?;
        let (_, module_path) = source_path.split_last()?;
        if let Some(scope) = scopes.get(module_path) {
            return Some(Arc::clone(scope));
        }
        let scope = Arc::new(self.pub_library_declaration_scope(library, manifest, module_path));
        scopes.insert(module_path.to_vec(), Arc::clone(&scope));
        Some(scope)
    }
}
