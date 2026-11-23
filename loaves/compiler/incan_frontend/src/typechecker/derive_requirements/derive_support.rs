//! Derive support for checked types: native, collection, surface, declared, nominal and newtype derives.

use super::*;

impl TypeChecker {
    // ========================================================================
    // The relation
    // ========================================================================

    /// Return whether `ty` implements `derive` in the generated program.
    ///
    /// A derive the relation does not answer for ([`is_relation_derive`]) is [`DeriveSupport::Unknown`].
    pub(in crate::typechecker) fn derive_support(&self, ty: &ResolvedType, derive: DeriveId) -> DeriveSupport {
        if !is_relation_derive(derive) {
            return DeriveSupport::Unknown;
        }
        self.derive_support_assuming(ty, derive, &[], &mut Vec::new())
    }

    /// Return the relation's answer for `ty` with a declaration's own type parameters taken to implement `derive`, as
    /// a derive on a generic declaration bounds them.
    pub(in crate::typechecker) fn derive_support_assuming_type_params(
        &self,
        ty: &ResolvedType,
        derive: DeriveId,
        type_params: &[String],
    ) -> DeriveSupport {
        if !is_relation_derive(derive) {
            return DeriveSupport::Unknown;
        }
        self.derive_support_assuming(ty, derive, type_params, &mut Vec::new())
    }

    /// The relation with a set of type-parameter names assumed to implement `derive`, and the newtypes being visited.
    ///
    /// A derive on a generic declaration bounds each type parameter by the derived trait, so the automatic derives of
    /// a generic newtype are decided with its own parameters assumed to implement them. `visiting` stops a newtype
    /// whose underlying type names itself.
    pub(super) fn derive_support_assuming(
        &self,
        ty: &ResolvedType,
        derive: DeriveId,
        assumed: &[String],
        visiting: &mut Vec<String>,
    ) -> DeriveSupport {
        match ty {
            ResolvedType::Int | ResolvedType::Bool | ResolvedType::Unit => DeriveSupport::Supported,
            // `String` and `Vec<u8>` own their contents, so they are not `Copy`.
            ResolvedType::Str | ResolvedType::Bytes => {
                if derive == DeriveId::Copy {
                    DeriveSupport::Missing(ty.clone())
                } else {
                    DeriveSupport::Supported
                }
            }
            // The baked `'static` string and bytes are `Copy` and ordered; whether they have a `Default` is not
            // recorded.
            ResolvedType::FrozenStr | ResolvedType::FrozenBytes => {
                if derive == DeriveId::Default {
                    DeriveSupport::Unknown
                } else {
                    DeriveSupport::Supported
                }
            }
            ResolvedType::Float => {
                if needs_total_comparison(derive) {
                    DeriveSupport::Missing(ty.clone())
                } else {
                    DeriveSupport::Supported
                }
            }
            ResolvedType::Numeric(id) => {
                if needs_total_comparison(derive) && numerics::is_binary_float(*id) {
                    DeriveSupport::Missing(ty.clone())
                } else {
                    DeriveSupport::Supported
                }
            }
            ResolvedType::Tuple(items) => self.all_derive_support(items, derive, assumed, visiting),
            // The frozen collections are baked slices: `Clone`, `Debug`, `PartialEq` and `Eq` follow their contents;
            // none hashes, and the rest is not recorded.
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => match derive {
                DeriveId::Hash => DeriveSupport::Missing(ty.clone()),
                DeriveId::Clone | DeriveId::Debug | DeriveId::PartialEq | DeriveId::Eq => {
                    self.derive_support_assuming(inner, derive, assumed, visiting)
                }
                _ => DeriveSupport::Unknown,
            },
            ResolvedType::FrozenDict(key, value) => match derive {
                DeriveId::Hash => DeriveSupport::Missing(ty.clone()),
                DeriveId::Clone | DeriveId::Debug | DeriveId::PartialEq | DeriveId::Eq => {
                    let pair = [key.as_ref().clone(), value.as_ref().clone()];
                    self.all_derive_support(&pair, derive, assumed, visiting)
                }
                _ => DeriveSupport::Unknown,
            },
            ResolvedType::TypeVar(name) => {
                if assumed.contains(name) {
                    DeriveSupport::Supported
                } else {
                    DeriveSupport::Unknown
                }
            }
            ResolvedType::Named(name) => {
                if assumed.contains(name) {
                    return DeriveSupport::Supported;
                }
                if self.generic_placeholder_name(ty).is_some() || collection_type_id(name).is_some() {
                    return DeriveSupport::Unknown;
                }
                if let Some(surface_type) = self.surface_type_named(name) {
                    return self.surface_derive_support(ty, surface_type, &[], derive, assumed, visiting);
                }
                self.declared_derive_support(ty, name, &[], derive, assumed, visiting)
            }
            ResolvedType::Generic(name, args) => {
                if numerics::decimal_constructor_from_str(name).is_some() {
                    return match derive {
                        DeriveId::PartialOrd | DeriveId::Ord | DeriveId::Default => DeriveSupport::Unknown,
                        _ => DeriveSupport::Supported,
                    };
                }
                if let Some(collection) = collection_type_id(name) {
                    return self.collection_derive_support(ty, collection, args, derive, assumed, visiting);
                }
                if let Some(surface_type) = self.surface_type_named(name) {
                    return self.surface_derive_support(ty, surface_type, args, derive, assumed, visiting);
                }
                self.declared_derive_support(ty, name, args, derive, assumed, visiting)
            }
            ResolvedType::RustPath(_) => self.native_derive_support(ty, derive),
            ResolvedType::Never
            | ResolvedType::Unknown
            | ResolvedType::CallSiteInfer
            | ResolvedType::Function(_, _)
            | ResolvedType::Ref(_)
            | ResolvedType::RefMut(_)
            | ResolvedType::TypeToken(_)
            | ResolvedType::Trait(_)
            | ResolvedType::SelfType => DeriveSupport::Unknown,
        }
    }

    /// Admit native Clone and Copy only from a complete metadata record's unconditional implementation. Conditional
    /// templates and absent facts remain unknown; a type name or the presence of a similarly named method is never
    /// proof.
    pub(super) fn native_derive_support(&self, ty: &ResolvedType, derive: DeriveId) -> DeriveSupport {
        let paths = derives::rust_ownership_trait_paths(derive);
        if paths.is_empty() {
            return DeriveSupport::Unknown;
        }
        let Some((path, _, _, _)) = self.rust_identity_for_type(ty) else {
            return DeriveSupport::Unknown;
        };
        let Some(metadata) = self.rust_item_metadata_for_complete_type(&path) else {
            return DeriveSupport::Unknown;
        };
        let proven = paths.iter().any(|path| {
            let definition = self.rust_definition_for_path(path);
            Self::rust_type_metadata_implements_trait(&metadata, path, definition.as_deref())
        });
        if proven {
            DeriveSupport::Supported
        } else {
            DeriveSupport::Unknown
        }
    }

    /// Report whether available native templates include an owned Clone candidate for a foreign field or rusttype.
    /// Missing or partial metadata remains unknown. No candidate is a storage-admission refusal, never proof that
    /// the Rust type lacks Clone: extraction may omit unsupported implementation shapes.
    pub(in crate::typechecker) fn native_field_clone_candidate(&self, ty: &ResolvedType) -> Option<bool> {
        let underlying = self.rusttype_native_target(ty);
        let native = match (ty, underlying.as_ref()) {
            (ResolvedType::RustPath(_), _) => ty,
            (_, Some(native)) => native,
            _ => return None,
        };
        let (path, _, _, _) = self.rust_identity_for_type(native)?;
        let metadata = self.rust_item_metadata_for_complete_type(&path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        if !info.metadata_completeness.has_trait_impls() {
            return None;
        }
        Some(derives::rust_ownership_trait_paths(DeriveId::Clone).iter().any(|path| {
            let definition = self.rust_definition_for_path(path);
            let suffix = Self::rust_trait_path_suffix(path);
            info.implemented_traits.iter().any(|implementation| {
                (implementation.path == *path
                    || Some(implementation.path.as_str()) == definition.as_deref()
                    || Self::rust_trait_path_suffix(&implementation.path) == suffix)
                    && self.rust_impl_self_bindings(implementation, native).is_some()
            })
        }))
    }

    /// Combine the answers for types that must all implement `derive`: the first missing one wins, then any unknown.
    pub(super) fn all_derive_support(
        &self,
        types: &[ResolvedType],
        derive: DeriveId,
        assumed: &[String],
        visiting: &mut Vec<String>,
    ) -> DeriveSupport {
        let mut unknown = false;
        for ty in types {
            match self.derive_support_assuming(ty, derive, assumed, visiting) {
                DeriveSupport::Supported => {}
                DeriveSupport::Missing(holder) => return DeriveSupport::Missing(holder),
                DeriveSupport::Unknown => unknown = true,
            }
        }
        if unknown {
            DeriveSupport::Unknown
        } else {
            DeriveSupport::Supported
        }
    }

    /// Answer for a builtin collection from what its generated Rust type implements.
    ///
    /// A `list` is a `Vec`, a `set` a `HashSet` and a `dict` a `HashMap`: none is `Copy`, each has an empty `Default`,
    /// and the hashed ones are neither ordered nor hashable. `Option`, `Result` and tuples follow their type arguments,
    /// except that only `Option` has a `Default`. The frozen collections are baked slices that record `Clone`,
    /// `Debug`, `PartialEq` and `Eq` only.
    pub(super) fn collection_derive_support(
        &self,
        ty: &ResolvedType,
        collection: CollectionTypeId,
        args: &[ResolvedType],
        derive: DeriveId,
        assumed: &[String],
        visiting: &mut Vec<String>,
    ) -> DeriveSupport {
        let follows =
            |checker: &Self, visiting: &mut Vec<String>| checker.all_derive_support(args, derive, assumed, visiting);
        match collection {
            CollectionTypeId::Generator => DeriveSupport::Unknown,
            CollectionTypeId::List => match derive {
                DeriveId::Copy => DeriveSupport::Missing(ty.clone()),
                DeriveId::Default => DeriveSupport::Supported,
                _ => follows(self, visiting),
            },
            CollectionTypeId::Set | CollectionTypeId::Dict => match derive {
                DeriveId::Copy | DeriveId::Hash | DeriveId::PartialOrd | DeriveId::Ord => {
                    DeriveSupport::Missing(ty.clone())
                }
                DeriveId::Default => DeriveSupport::Supported,
                _ => follows(self, visiting),
            },
            CollectionTypeId::Option => match derive {
                DeriveId::Default => DeriveSupport::Supported,
                _ => follows(self, visiting),
            },
            CollectionTypeId::Result => match derive {
                DeriveId::Default => DeriveSupport::Missing(ty.clone()),
                _ => follows(self, visiting),
            },
            CollectionTypeId::Tuple => follows(self, visiting),
            CollectionTypeId::FrozenList | CollectionTypeId::FrozenSet | CollectionTypeId::FrozenDict => match derive {
                DeriveId::Hash => DeriveSupport::Missing(ty.clone()),
                DeriveId::Clone | DeriveId::Debug | DeriveId::PartialEq | DeriveId::Eq => follows(self, visiting),
                _ => DeriveSupport::Unknown,
            },
        }
    }

    /// Answer for a stdlib surface type from the registry.
    pub(super) fn surface_derive_support(
        &self,
        ty: &ResolvedType,
        surface_type: SurfaceTypeId,
        args: &[ResolvedType],
        derive: DeriveId,
        assumed: &[String],
        visiting: &mut Vec<String>,
    ) -> DeriveSupport {
        match surface_types::derive_support(surface_type, derive) {
            SurfaceDeriveSupport::Implements => DeriveSupport::Supported,
            SurfaceDeriveSupport::FollowsTypeArguments => self.all_derive_support(args, derive, assumed, visiting),
            SurfaceDeriveSupport::Missing => DeriveSupport::Missing(ty.clone()),
            SurfaceDeriveSupport::NotRecorded => DeriveSupport::Unknown,
        }
    }

    /// Answer for a declared `model`, `class`, `enum` or newtype, then for its type arguments.
    ///
    /// A derive on a generic declaration bounds every type parameter, so a declaration that implements `derive` does so
    /// for type arguments that implement it too.
    pub(super) fn declared_derive_support(
        &self,
        ty: &ResolvedType,
        name: &str,
        args: &[ResolvedType],
        derive: DeriveId,
        assumed: &[String],
        visiting: &mut Vec<String>,
    ) -> DeriveSupport {
        let Some(info) = self.lookup_semantic_type_info(name) else {
            return DeriveSupport::Unknown;
        };
        let own = match info {
            TypeInfo::Model(model) => self
                .automatic_nominal_derive(name, &model.derives, &model.traits, derive, false)
                .map(|derived| derived || Self::dunder_provides(&model.methods, derive)),
            TypeInfo::Class(class) => {
                let opaque_debug = self.class_omits_automatic_debug(name, class);
                self.automatic_nominal_derive(name, &class.derives, &class.traits, derive, opaque_debug)
                    .map(|derived| derived || Self::dunder_provides(&class.methods, derive))
            }
            TypeInfo::Enum(en) => {
                let automatic = Self::enum_automatic_derives(en).contains(&derive);
                if automatic {
                    Some(true)
                } else {
                    self.automatic_nominal_derive(name, &en.derives, &en.traits, derive, false)
                        .map(|derived| derived || Self::dunder_provides(&en.methods, derive))
                }
            }
            TypeInfo::Newtype(newtype) if newtype.is_rusttype => None,
            TypeInfo::Newtype(newtype) => self
                .newtype_derive(name, newtype, derive, visiting)
                .map(|derived| derived || Self::dunder_provides(&newtype.methods, derive)),
            TypeInfo::Builtin | TypeInfo::TypeAlias => None,
        };
        match own {
            Some(true) => self.all_derive_support(args, derive, assumed, visiting),
            Some(false) => DeriveSupport::Missing(ty.clone()),
            None => DeriveSupport::Unknown,
        }
    }

    /// Whether a private adapter class with direct Rust state omits automatic Debug. Explicit or implied Debug stays
    /// selected and rustc verifies the native fields' implementations; a private source-only class keeps its derive.
    pub(in crate::typechecker) fn class_omits_automatic_debug(&self, name: &str, class: &ClassInfo) -> bool {
        name.starts_with('_')
            && self.declared_derive(name, &class.derives, &class.traits, DeriveId::Debug) != Some(true)
            && class
                .fields
                .values()
                .any(|field| crate::symbols::resolved_type_contains_rust_path(&field.ty))
    }

    /// Whether a nominal implements `derive` through the base automatic `Clone` and `Debug` set, or through a declared
    /// or implied derive.
    ///
    /// Enum-specific automatic equality is added by [`Self::enum_automatic_derives`] before this fallback is queried.
    pub(super) fn automatic_nominal_derive(
        &self,
        name: &str,
        derive_names: &[String],
        trait_names: &[String],
        derive: DeriveId,
        opaque_debug: bool,
    ) -> Option<bool> {
        match derive {
            DeriveId::Debug if opaque_debug => None,
            DeriveId::Clone | DeriveId::Debug => Some(true),
            _ => self.declared_derive(name, derive_names, trait_names, derive),
        }
    }

    /// Whether a type's own dunder gives it `derive` in the generated program: `__eq__` is emitted as its
    /// `PartialEq` implementation.
    pub(super) fn dunder_provides(methods: &HashMap<String, MethodInfo>, derive: DeriveId) -> bool {
        derive == DeriveId::PartialEq && methods.contains_key(magic_methods::as_str(MagicMethodId::Eq))
    }

    /// Return the derives lowering adds to an enum automatically.
    ///
    /// Every enum has `Debug` and `Clone`. Without an authored `__eq__`, payloads that support structural equality add
    /// `PartialEq`, and those that also exclude floating point add `Eq`. This set is the single authority used by
    /// native derive selection, operator checking and derive-member validation.
    pub(in crate::typechecker) fn enum_automatic_derives(info: &EnumInfo) -> Vec<DeriveId> {
        let mut automatic = vec![DeriveId::Debug, DeriveId::Clone];
        if info.methods.contains_key(magic_methods::as_str(MagicMethodId::Eq)) {
            return automatic;
        }
        if info
            .variant_fields
            .values()
            .flatten()
            .all(|payload| payload_defaults_equality(payload, DeriveId::PartialEq))
        {
            automatic.push(DeriveId::PartialEq);
        }
        if info
            .variant_fields
            .values()
            .flatten()
            .all(|payload| payload_defaults_equality(payload, DeriveId::Eq))
        {
            automatic.push(DeriveId::Eq);
        }
        automatic
    }

    /// Whether a comparison operator on `ty` is backed by a derive rather than a dunder: `derive` (`PartialEq` for
    /// `==` and `!=`, `PartialOrd` for the orderings) declared or implied on the type, spelled in `@rust.derive(...)`,
    /// or through the enum's automatic equality set.
    ///
    /// A dunder defines only its own operator, so the operator check dispatches a type's dunders separately and asks
    /// this only for the derived implementation.
    pub(in crate::typechecker) fn comparison_is_derived(&self, ty: &ResolvedType, derive: DeriveId) -> bool {
        let (ResolvedType::Named(name) | ResolvedType::Generic(name, _)) = ty else {
            return false;
        };
        let Some(info) = self.lookup_semantic_type_info(name) else {
            return false;
        };
        let declared =
            |derives: &[String], traits: &[String]| self.declared_derive(name, derives, traits, derive) == Some(true);
        match info {
            TypeInfo::Model(model) => declared(&model.derives, &model.traits),
            TypeInfo::Class(class) => declared(&class.derives, &class.traits),
            TypeInfo::Enum(en) => {
                declared(&en.derives, &en.traits) || Self::enum_automatic_derives(en).contains(&derive)
            }
            TypeInfo::Newtype(newtype) => !newtype.is_rusttype && declared(&newtype.derives, &newtype.traits),
            TypeInfo::Builtin | TypeInfo::TypeAlias => false,
        }
    }

    /// Whether a declared type provides `derive` through its own derive list, trait adoptions and `@rust.derive`.
    ///
    /// A derive counts together with what it implies (`Ord` implies `Eq`), under its own name or an import alias, and a
    /// trait adoption counts when it names the builtin trait, except `Hash`: a set or dict hashes a type only through
    /// the derive, and the `__hash__` an adopter of `std.derives.comparison.Hash` defines is an ordinary method
    /// (#1822). A type declared in another module, or with a `@rust.derive(...)` the compiler cannot classify,
    /// answers `None` when the derive is absent from what is known.
    pub(super) fn declared_derive(
        &self,
        name: &str,
        derive_names: &[String],
        trait_names: &[String],
        derive: DeriveId,
    ) -> Option<bool> {
        let local = self.local_derive_facts.get(name);
        let mut provided = derive_names
            .iter()
            .filter_map(|spelling| self.builtin_derive_named(spelling))
            .chain(
                trait_names
                    .iter()
                    .filter_map(|spelling| self.builtin_derive_bound(spelling))
                    .filter(|adopted| *adopted != DeriveId::Hash),
            )
            .collect::<Vec<_>>();
        if let Some(facts) = local {
            provided.extend(facts.rust_builtin_derives.iter().copied());
        }
        let implied = provided
            .iter()
            .flat_map(|id| derives::implied_derives(*id).iter().copied())
            .collect::<Vec<_>>();
        if provided.contains(&derive) || implied.contains(&derive) {
            return Some(true);
        }
        match local {
            Some(facts) if !facts.has_unclassified_rust_derive => Some(false),
            _ => None,
        }
    }

    /// Resolve an `@derive(...)` spelling, including an import alias, to the builtin derive it names.
    ///
    /// Lowering takes a builtin derive name as the builtin derive whatever else it could name, so the relation does
    /// too; a trait adoption is resolved strictly instead ([`Self::builtin_derive_bound`]).
    pub(in crate::typechecker) fn builtin_derive_named(&self, spelling: &str) -> Option<DeriveId> {
        derives::from_str(spelling).or_else(|| {
            self.import_binding_path(spelling)
                .and_then(<[String]>::last)
                .and_then(|leaf| derives::from_str(leaf))
        })
    }

    /// Whether a non-`rusttype` newtype implements `derive`: its explicit derives first, then what lowering adds.
    ///
    /// A missing `Clone` or `Debug` is known only for a newtype declared in this module without an unclassified
    /// `@rust.derive(...)`; elsewhere the answer is unknown.
    pub(super) fn newtype_derive(
        &self,
        name: &str,
        info: &NewtypeInfo,
        derive: DeriveId,
        visiting: &mut Vec<String>,
    ) -> Option<bool> {
        if self.declared_derive(name, &info.derives, &info.traits, derive) == Some(true) {
            return Some(true);
        }
        match derive {
            DeriveId::Copy if Self::newtype_underlying_is_copy(&info.underlying) => Some(true),
            DeriveId::Clone | DeriveId::Debug => {
                if self.newtype_automatic_derive(name, info, derive, visiting) {
                    return Some(true);
                }
                // A newtype declared elsewhere may carry the derive through `@rust.derive(...)`, which its manifest
                // does not record; only a local declaration's derive list is complete.
                let complete = self
                    .local_derive_facts
                    .get(name)
                    .is_some_and(|facts| !facts.has_unclassified_rust_derive);
                complete.then_some(false)
            }
            _ => self.declared_derive(name, &info.derives, &info.traits, derive),
        }
    }

    /// Whether lowering derives `Copy` (and `Clone`) for a newtype automatically: when its underlying type is a
    /// number, `bool`, `None`, a frozen string or bytes, or a tuple, `Option` or `Result` of those.
    ///
    /// This is the checked form of the rule lowering applies to the lowered underlying type (`IrType::is_copy`); a
    /// declared type that derives `Copy` does not count there, so it does not count here either.
    pub(super) fn newtype_underlying_is_copy(underlying: &ResolvedType) -> bool {
        match underlying {
            ResolvedType::Unit
            | ResolvedType::Bool
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Ref(_)
            | ResolvedType::RefMut(_)
            | ResolvedType::TypeToken(_) => true,
            ResolvedType::Tuple(items) => items.iter().all(Self::newtype_underlying_is_copy),
            ResolvedType::Generic(name, args) => {
                if numerics::decimal_constructor_from_str(name).is_some() {
                    return true;
                }
                match collection_type_id(name) {
                    Some(CollectionTypeId::Option | CollectionTypeId::Result | CollectionTypeId::Tuple) => {
                        !args.is_empty() && args.iter().all(Self::newtype_underlying_is_copy)
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Whether lowering gives a newtype `derive` automatically: `Clone` when its underlying type implements it (or is
    /// `Copy`), and `Debug` unless its underlying type is known to lack it.
    ///
    /// The newtype's own type parameters count as implementing the derive, since the derive bounds them.
    pub(super) fn newtype_automatic_derive(
        &self,
        name: &str,
        info: &NewtypeInfo,
        derive: DeriveId,
        visiting: &mut Vec<String>,
    ) -> bool {
        if visiting.iter().any(|visited| visited == name) {
            return false;
        }
        visiting.push(name.to_string());
        let support = self.derive_support_assuming(&info.underlying, derive, &info.type_params, visiting);
        visiting.pop();
        match derive {
            DeriveId::Clone => support == DeriveSupport::Supported || self.is_copy_type(&info.underlying),
            DeriveId::Debug => !matches!(support, DeriveSupport::Missing(_)),
            _ => false,
        }
    }

    /// Select the serde derives a nominal's trait adoptions imply, walking each adopted trait's supertraits.
    ///
    /// Adopting the standard library's JSON `Serialize` or `Deserialize`, directly or through a supertrait, gives the
    /// adopter the matching serde derive, as legacy emission forwards it. The walk keys on each trait's canonical
    /// identity through the current import bindings, never on its basename, and visits each trait name once.
    pub(in crate::typechecker) fn adopted_serde_derives(
        &self,
        adoptions: &[Spanned<crate::ast::TraitBound>],
    ) -> Vec<String> {
        let mut pending = adoptions
            .iter()
            .map(|adoption| adoption.node.name.clone())
            .collect::<std::collections::VecDeque<_>>();
        let mut visited = std::collections::HashSet::new();
        let mut selected = Vec::new();
        while let Some(name) = pending.pop_front() {
            if !visited.insert(name.clone()) {
                continue;
            }
            if let Some(info) = self.lookup_trait_info(&name) {
                pending.extend(info.supertraits.iter().map(|(supertrait, _)| supertrait.clone()));
            }
            let path = self.derive_trait_path(&name);
            let protocol = path.split_last().and_then(|(trait_name, module)| {
                incan_lang::lang::stdlib::stdlib_json_trait_id_for_identity(module, trait_name)
            });
            let derive = match protocol {
                Some(incan_lang::lang::stdlib::StdlibJsonTraitId::Serialize) => "serde::Serialize",
                Some(incan_lang::lang::stdlib::StdlibJsonTraitId::Deserialize) => "serde::Deserialize",
                None => continue,
            };
            if !selected.iter().any(|existing| existing == derive) {
                selected.push(derive.to_owned());
            }
        }
        selected
    }

    /// Append builtin derive implications in legacy normalization order without duplicating checked selections.
    pub(in crate::typechecker) fn append_implied_derives(selected: &mut Vec<String>) {
        for (derive, implied) in derives::DERIVE_IMPLICATIONS {
            if selected.iter().any(|name| name == derives::as_str(*derive)) {
                for implied in *implied {
                    let name = derives::as_str(*implied).to_owned();
                    if !selected.contains(&name) {
                        selected.push(name);
                    }
                }
            }
        }
    }

    /// Append checked Rust derives selected by the local type's `@rust.derive(...)`, without duplicating an automatic
    /// or explicit selection. Builtins use native names; dependency macros retain their inspected exported paths.
    ///
    /// Returns whether the selection now covers that decorator completely: `false` when it also names a derive macro
    /// checking cannot resolve, or when collection recorded no facts for the type. This grants passthrough only;
    /// no expansion-dependent trait implementation is inferred.
    pub(in crate::typechecker) fn append_native_rust_derives(
        &mut self,
        type_name: &str,
        selected: &mut Vec<String>,
    ) -> bool {
        let Some(facts) = self.local_derive_facts.get(type_name) else {
            return false;
        };
        for derive in &facts.rust_builtin_derives {
            let name = derives::as_str(*derive).to_owned();
            if !selected.contains(&name) {
                selected.push(name);
            }
        }
        if let Some(paths) = self.local_rust_derive_paths.get(type_name) {
            for path in paths {
                if Self::builtin_rust_derive_path(path).is_some() {
                    continue;
                }
                let native_macro = self.checked_native_derive_macro(path);
                if native_macro.is_some() {
                    self.type_info.derivations.native_macro_paths.insert(path.clone());
                }
                if (matches!(path.as_str(), "serde::Serialize" | "serde::Deserialize") || native_macro.is_some())
                    && !selected.contains(path)
                {
                    selected.push(path.clone());
                }
            }
        }
        !facts.has_unclassified_rust_derive
    }

    /// Return the derive names lowering adds to a newtype automatically, in emission order (`Debug`, then `Clone`).
    ///
    /// Recorded for lowering with the newtype's construction facts, so lowering and this relation agree on what a
    /// newtype implements.
    pub(in crate::typechecker) fn newtype_automatic_derive_names(&self, name: &str, info: &NewtypeInfo) -> Vec<String> {
        [DeriveId::Debug, DeriveId::Clone]
            .into_iter()
            .filter(|derive| self.newtype_automatic_derive(name, info, *derive, &mut Vec::new()))
            .map(|derive| derives::as_str(derive).to_string())
            .collect()
    }

    /// Return the stdlib surface type a type name refers to, unless a source declaration owns the spelling.
    ///
    /// An imported surface type is found under its local binding, so an aliased import still counts. The canonical
    /// spelling counts only while it names no source declaration: a program's own `type JoinHandle[T] = rusttype ...`
    /// is not the runtime handle.
    pub(super) fn surface_type_named(&self, name: &str) -> Option<SurfaceTypeId> {
        self.active_surface_type_import(name).or_else(|| {
            surface_types::from_str(name)
                .filter(|_| matches!(self.lookup_type_info(name), None | Some(TypeInfo::Builtin)))
        })
    }
}
