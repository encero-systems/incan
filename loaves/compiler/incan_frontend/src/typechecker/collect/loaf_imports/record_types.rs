//! The types of an installed Loaf a declaration's signature names, settled from the Loaf's own records (RFC 179): the
//! declaring module's type scope says which declaration each spelling names, and each named model is registered from
//! its own signature, layout and default records, so checking a call reads the types it uses rather than the Loaf's
//! whole checked API. Whatever these records cannot settle exactly is left to the Loaf's manifest.

use std::collections::BTreeSet;

use incan_lang::lang::types::builtin_type_name;

use super::*;
use crate::api_metadata::ApiModel;
use crate::library_manifest::declaration_records::{DeclarationFacet, qualified_name, restored_nominal_declaration};
use crate::library_manifest::type_scope::DeclaredType;
use crate::library_manifest::{CanonicalIdentityExport, TypeRef};

/// What a declaration spells beyond built-in types and its own type parameters.
#[derive(Default)]
pub(super) struct RecordSpellings {
    /// The type spellings, each of which must name a model the Loaf's records settle.
    types: BTreeSet<String>,
    /// The trait spellings, which stay as written: they must name nothing of the Loaf's own.
    traits: BTreeSet<String>,
}

impl RecordSpellings {
    /// The spellings of the function, constant or static `declaration`. `None` for another kind of declaration, a
    /// bounded type parameter, or a type the record route does not settle.
    pub(super) fn of_member(declaration: &ApiDeclaration) -> Option<Self> {
        let mut spellings = Self::default();
        let settled = match declaration {
            ApiDeclaration::Function(function) => {
                function.type_params.iter().all(|param| param.bounds.is_empty())
                    && function.params.iter().all(|param| spellings.take(&param.ty))
                    && spellings.take(&function.return_type)
            }
            ApiDeclaration::Const(constant) => spellings.take(&constant.ty),
            ApiDeclaration::Static(item) => spellings.take(&item.ty),
            _ => false,
        };
        settled.then_some(spellings)
    }

    /// The spellings of `model`: its fields, properties, methods and trait adoptions. `None` for a generic model, a
    /// generic method, or a type the record route does not settle.
    fn of_model(model: &ApiModel) -> Option<Self> {
        let mut spellings = Self::default();
        let settled = model.type_params.is_empty()
            && model.fields.iter().all(|field| spellings.take(&field.ty))
            && model
                .properties
                .iter()
                .all(|property| spellings.take(&property.return_type))
            && model.methods.iter().all(|method| {
                method.type_params.is_empty()
                    && method.params.iter().all(|param| spellings.take(&param.ty))
                    && spellings.take(&method.return_type)
            })
            && model.trait_adoptions.iter().all(|adoption| {
                spellings.traits.insert(adoption.name.clone());
                adoption.implementation_type_params.is_empty()
                    && adoption.type_args.iter().all(|argument| spellings.take(argument))
            });
        settled.then_some(spellings)
    }

    /// Whether no spelling needs the declaring module's type scope.
    pub(super) fn is_empty(&self) -> bool {
        self.types.is_empty() && self.traits.is_empty()
    }

    /// Take the spellings of `ty`. `false` when it holds anything but built-in types, type parameters, function types
    /// and names without a foreign origin, which is all the record route settles.
    fn take(&mut self, ty: &TypeRef) -> bool {
        match ty {
            TypeRef::Named { name, origin: None } => {
                if ResolvedType::builtin_simple(name).is_none() {
                    self.types.insert(name.clone());
                }
                true
            }
            TypeRef::Applied {
                name,
                args,
                origin: None,
            } => {
                if builtin_type_name(name).is_none() {
                    self.types.insert(name.clone());
                }
                args.iter().all(|argument| self.take(argument))
            }
            TypeRef::Function { params, return_type } => {
                params.iter().all(|param| self.take(param)) && self.take(return_type)
            }
            TypeRef::TypeParam { .. } => true,
            _ => false,
        }
    }
}

impl TypeChecker {
    /// The remapping of `spellings` as the module `module` of `library` spells them: each type spelling mapped to the
    /// key of the model it names, which is registered from its own records on the way. `registering` holds the keys
    /// of the models being registered around this one, so a model that names itself or its user ends the walk.
    ///
    /// `None`, and the manifest then decides, when the module has no type scope record that followed its spellings as
    /// this index binds the Loaf, when a type spelling names anything but a model the declaring module scopes and the
    /// records settle, or when a trait spelling may name a declaration of the Loaf: one of its module's type scope, or
    /// a name its package root may export.
    pub(super) fn record_type_remapping(
        &mut self,
        library: &str,
        module: &[String],
        spellings: &RecordSpellings,
        registering: &mut Vec<String>,
    ) -> Option<HashMap<String, String>> {
        if spellings.is_empty() {
            return Some(HashMap::new());
        }
        let admitted = self.loaves.admitted(library)?;
        let scope = self.loaves.type_scope(library, module).ok()??;
        if scope.standard != (admitted.binding_root == ["std"]) {
            return None;
        }
        for name in &spellings.traits {
            if scope.names.contains_key(name)
                || !matches!(self.loaves.may_export_at_root(library, name), Ok(Some(false)))
            {
                return None;
            }
        }
        let mut remapping = HashMap::new();
        for spelling in &spellings.types {
            let declared = scope.names.get(spelling).filter(|declared| declared.scoped)?;
            let key = Self::declaration_type_key(library, &declared.module, &declared.name);
            if !registering.contains(&key) {
                self.register_record_model(library, declared, &key, registering)?;
            }
            remapping.insert(spelling.clone(), key);
        }
        Some(remapping)
    }

    /// Register the model `declared` names under `key`, its declaration key, as the manifest route registers every
    /// declaration of the Loaf: its identity, and its members with their type names read in the declaring module's
    /// scope. A key already registered keeps its entry. `None` when the records do not settle the declaration: it is
    /// no model, its type scope names no identity for it, or its members name what
    /// [`Self::record_type_remapping`] refuses.
    fn register_record_model(
        &mut self,
        library: &str,
        declared: &DeclaredType,
        key: &str,
        registering: &mut Vec<String>,
    ) -> Option<()> {
        if self.public_library_type_identities.contains_key(key) && self.transitive_pub_types.contains_key(key) {
            return Some(());
        }
        let canonical = declared.identity.as_ref().and_then(CanonicalIdentityExport::hydrate)?;
        let qualified = qualified_name(&declared.module, &declared.name);
        let mut facts = Vec::with_capacity(3);
        for facet in [
            DeclarationFacet::Signature,
            DeclarationFacet::Layout,
            DeclarationFacet::Defaults,
        ] {
            facts.push(self.loaves.declaration_facts(library, facet, &qualified).ok()??);
        }
        let [signatures, layouts, defaults] = facts.as_slice() else {
            return None;
        };
        let [signature] = signatures.as_slice() else {
            return None;
        };
        if layouts.len() > 1 || defaults.len() > 1 {
            return None;
        }
        let restored = restored_nominal_declaration(signature.clone(), layouts.first(), defaults.first());
        let declaration: ApiDeclaration = serde_json::from_value(restored).ok()?;
        let ApiDeclaration::Model(model) = &declaration else {
            return None;
        };
        let spellings = RecordSpellings::of_model(model)?;
        registering.push(key.to_owned());
        let remapping = self.record_type_remapping(library, &declared.module, &spellings, registering);
        registering.pop();
        let remapping = remapping?;
        let mut kind = self.symbol_kind_from_api_declaration(None, &declaration)?;
        self.remap_symbol_kind_with_import_aliases(&mut kind, &remapping);
        let SymbolKind::Type(info) = kind else {
            return None;
        };
        let mut source_path = declared.module.clone();
        source_path.push(declared.name.clone());
        let mut identity = PublicLibraryTypeIdentity::new(library, &source_path);
        identity.canonical = Some(canonical.clone());
        identity.selected_provider = self
            .loaves
            .identities()
            .find(|(indexed, _)| *indexed == library)
            .map(|(_, provider)| provider.clone());
        self.public_library_type_identities.insert(key.to_owned(), identity);
        self.cache_retained_nominal_layout(Some(&canonical), &info);
        self.transitive_pub_types.entry(key.to_owned()).or_default().push(info);
        Some(())
    }
}
