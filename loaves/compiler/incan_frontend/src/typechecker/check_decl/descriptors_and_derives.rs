//! Descriptor derive validation, registry descriptor types, and native and JSON derive selection.

use super::*;

impl TypeChecker {
    /// Keep `@derive(Descriptor)` scoped to structural models; classes, enums, and newtypes have incompatible
    /// construction semantics for a checked descriptor snapshot.
    pub(super) fn validate_descriptor_derive_attachment(&mut self, decorators: &[Spanned<Decorator>], kind: &str) {
        for decorator in decorators {
            if self.decorator_id(&decorator.node) != Some(DecoratorId::Derive) {
                continue;
            }
            for argument in &decorator.node.args {
                let DecoratorArg::Positional(value) = argument else {
                    continue;
                };
                if matches!(&value.node, Expr::Ident(name) if derives::from_str(name) == Some(DeriveId::Descriptor)) {
                    self.errors.push(CompileError::type_error(
                        format!("@derive(Descriptor) is only supported on models, not {kind}s"),
                        value.span,
                    ));
                }
            }
        }
    }

    /// Validate the field contract that `@derive(Descriptor)` opts into.
    ///
    /// A registry snapshot is a typed, non-executing value. Mutable containers, functions, open generics, Rust
    /// handles, and recursive descriptor graphs cannot meet that promise. Keeping this validation on the model derive
    /// prevents a registry use site from being the first place a domain author learns that its descriptor is not
    /// serializable and replay-safe.
    pub(super) fn validate_descriptor_model_shape(&mut self, model: &ModelDecl, derives: &[String]) {
        if !derives
            .iter()
            .any(|derive_| derives::from_str(derive_) == Some(DeriveId::Descriptor))
        {
            return;
        }
        if !model.type_params.is_empty() {
            self.errors.push(CompileError::type_error(
                "@derive(Descriptor) does not support generic models; use a concrete descriptor model".to_string(),
                model.fields.first().map(|field| field.span).unwrap_or_default(),
            ));
        }
        for field in &model.fields {
            let ty = self.resolve_type_checked(&field.node.ty);
            let mut visiting = HashSet::new();
            if let Err(reason) = self.registry_descriptor_value_type_is_valid(&ty, &mut visiting) {
                self.errors.push(CompileError::type_error(
                    format!(
                        "@derive(Descriptor) field `{}.{}` has unsupported type `{ty}`: {reason}",
                        model.name, field.node.name
                    ),
                    field.span,
                ));
            }
        }
    }

    /// Return whether a descriptor field can be preserved as a finite structural snapshot.
    pub(super) fn registry_descriptor_value_type_is_valid(
        &self,
        ty: &ResolvedType,
        visiting: &mut HashSet<String>,
    ) -> Result<(), String> {
        match ty {
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit => Ok(()),
            ResolvedType::FrozenList(element) => self.registry_descriptor_value_type_is_valid(element, visiting),
            ResolvedType::FrozenDict(key, value) => {
                self.registry_descriptor_dict_key_type_is_valid(key, visiting)?;
                self.registry_descriptor_value_type_is_valid(value, visiting)
            }
            ResolvedType::FrozenSet(_) => {
                Err("FrozenSet values do not yet have an RFC 113 structural encoding".to_string())
            }
            ResolvedType::Named(name) => self.registry_descriptor_named_type_is_valid(name, visiting),
            ResolvedType::Trait(_) => Err("traits have no standalone descriptor value representation".to_string()),
            ResolvedType::Generic(name, arguments) => match collection_type_id(name) {
                Some(CollectionTypeId::Option) if arguments.len() == 1 => {
                    self.registry_descriptor_value_type_is_valid(&arguments[0], visiting)
                }
                Some(CollectionTypeId::FrozenList) if arguments.len() == 1 => {
                    self.registry_descriptor_value_type_is_valid(&arguments[0], visiting)
                }
                Some(CollectionTypeId::FrozenDict) if arguments.len() == 2 => {
                    self.registry_descriptor_dict_key_type_is_valid(&arguments[0], visiting)?;
                    self.registry_descriptor_value_type_is_valid(&arguments[1], visiting)
                }
                Some(CollectionTypeId::List | CollectionTypeId::Dict | CollectionTypeId::Set) => {
                    Err("mutable collections are not descriptor snapshots; use FrozenList or FrozenDict".to_string())
                }
                Some(CollectionTypeId::Tuple | CollectionTypeId::Result | CollectionTypeId::Generator) => {
                    Err("this collection shape has no RFC 113 structural encoding".to_string())
                }
                Some(CollectionTypeId::FrozenSet) => {
                    Err("FrozenSet values do not yet have an RFC 113 structural encoding".to_string())
                }
                Some(kind) => Err(format!("collection `{kind:?}` has an invalid descriptor arity")),
                None => Err(format!("generic type `{name}` is not a concrete descriptor value")),
            },
            ResolvedType::Tuple(_) => Err("tuple values do not yet have an RFC 113 structural encoding".to_string()),
            ResolvedType::Function(_, _) => {
                Err("functions require runtime execution and cannot be snapshotted".to_string())
            }
            ResolvedType::TypeToken(inner) => self.registry_type_reference_is_valid(inner),
            ResolvedType::Never => Err("Never has no value that can be preserved in a descriptor snapshot".to_string()),
            ResolvedType::TypeVar(_) | ResolvedType::SelfType => {
                Err("open type parameters cannot be structural snapshots".to_string())
            }
            ResolvedType::Ref(_) | ResolvedType::RefMut(_) => {
                Err("borrowed values cannot escape into descriptor snapshots".to_string())
            }
            ResolvedType::RustPath(_) => Err("Rust handles cannot be descriptor snapshots".to_string()),
            ResolvedType::CallSiteInfer | ResolvedType::Unknown => {
                Err("the field type could not be resolved to a structural value".to_string())
            }
        }
    }

    /// Validate a concrete Incan type token retained inside a descriptor snapshot.
    ///
    /// Type references are source identities, not Rust handles or an invitation to execute reflection. The token must
    /// therefore be fully concrete and stay within the Incan type universe that the compiler can render stably.
    pub(super) fn registry_type_reference_is_valid(&self, ty: &ResolvedType) -> Result<(), String> {
        match ty {
            ResolvedType::Never
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::Named(_)
            | ResolvedType::Trait(_) => Ok(()),
            ResolvedType::Generic(_, arguments) | ResolvedType::Tuple(arguments) => {
                for argument in arguments {
                    self.registry_type_reference_is_valid(argument)?;
                }
                Ok(())
            }
            ResolvedType::FrozenList(element)
            | ResolvedType::TypeToken(element)
            | ResolvedType::Ref(element)
            | ResolvedType::RefMut(element) => self.registry_type_reference_is_valid(element),
            ResolvedType::FrozenDict(key, value) => {
                self.registry_type_reference_is_valid(key)?;
                self.registry_type_reference_is_valid(value)
            }
            ResolvedType::FrozenSet(element) => self.registry_type_reference_is_valid(element),
            ResolvedType::Function(_, _) => Err("function type tokens are not descriptor snapshots".to_string()),
            ResolvedType::TypeVar(_) | ResolvedType::SelfType => {
                Err("open type parameters cannot be descriptor type references".to_string())
            }
            ResolvedType::RustPath(_) => Err("Rust type handles cannot be descriptor type references".to_string()),
            ResolvedType::CallSiteInfer | ResolvedType::Unknown => {
                Err("the type token could not be resolved to a concrete Incan type".to_string())
            }
        }
    }

    /// Restrict frozen-map keys to deterministic scalar identities rather than arbitrary descriptor objects.
    pub(super) fn registry_descriptor_dict_key_type_is_valid(
        &self,
        ty: &ResolvedType,
        visiting: &mut HashSet<String>,
    ) -> Result<(), String> {
        match ty {
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes => Ok(()),
            ResolvedType::Named(name) => match self.lookup_semantic_type_info(name) {
                Some(TypeInfo::Enum(info)) if info.variant_fields.values().all(Vec::is_empty) => Ok(()),
                Some(TypeInfo::Newtype(info)) if !info.is_rusttype => {
                    self.registry_descriptor_value_type_is_valid(&info.underlying, visiting)
                }
                _ => Err("map keys must be primitives, fieldless enums, or structural newtypes".to_string()),
            },
            _ => Err("map keys must be deterministic scalar identities".to_string()),
        }
    }

    /// Validate nominal descriptor components while detecting recursive snapshot graphs. An imported model's field
    /// types name their declarations by carrier, so each component resolves through its declaration identity (#2605).
    pub(super) fn registry_descriptor_named_type_is_valid(
        &self,
        name: &str,
        visiting: &mut HashSet<String>,
    ) -> Result<(), String> {
        let Some(type_info) = self.lookup_semantic_type_info(name) else {
            return Err(format!("unknown nominal type `{name}`"));
        };
        match type_info {
            TypeInfo::Enum(info) => info
                .variant_fields
                .values()
                .all(Vec::is_empty)
                .then_some(())
                .ok_or_else(|| "enums with payload variants are not structural descriptor values".to_string()),
            TypeInfo::Newtype(info) => {
                if info.is_rusttype {
                    return Err("rusttype aliases cannot be descriptor snapshots".to_string());
                }
                if !info.type_params.is_empty() {
                    return Err("generic newtypes cannot be descriptor snapshots".to_string());
                }
                let visit = format!("newtype:{name}");
                if !visiting.insert(visit.clone()) {
                    return Err("recursive newtype snapshots are not supported".to_string());
                }
                let result = self.registry_descriptor_value_type_is_valid(&info.underlying, visiting);
                visiting.remove(&visit);
                result
            }
            TypeInfo::Model(info) => {
                if !info
                    .derives
                    .iter()
                    .any(|derive_| derives::from_str(derive_) == Some(DeriveId::Descriptor))
                {
                    return Err("nested models must also use @derive(Descriptor)".to_string());
                }
                if !info.type_params.is_empty() {
                    return Err("generic descriptor models are not supported".to_string());
                }
                let visit = format!("model:{name}");
                if !visiting.insert(visit.clone()) {
                    return Err("recursive descriptor model snapshots are not supported".to_string());
                }
                let result = info.field_order.iter().try_for_each(|field_name| {
                    let field = info
                        .fields
                        .get(field_name)
                        .ok_or_else(|| format!("descriptor model field `{field_name}` is missing"))?;
                    self.registry_descriptor_value_type_is_valid(&field.ty, visiting)
                });
                visiting.remove(&visit);
                result
            }
            TypeInfo::Class(_) => Err("classes are mutable and cannot be descriptor snapshots".to_string()),
            TypeInfo::Builtin | TypeInfo::TypeAlias => Err(format!(
                "builtin or alias type `{name}` is not a structural descriptor value"
            )),
        }
    }

    /// Select the native derives of a model or class from its explicit derive names, in legacy emission order.
    ///
    /// Stdlib JSON protocol selections map to their canonical serde paths, then `declared` adds the derives the
    /// declaration itself implies, such as a model's adopted serde derives. The derives those selections imply follow,
    /// and the automatic Debug, Clone, FieldInfo and IncanClass derives every model and class receives come last.
    /// Derived Display stays a semantic selection that consumers must not emit as a Rust macro.
    pub(super) fn native_derive_selection(&self, derives: &[String], declared: Vec<String>) -> Vec<String> {
        let mut native_derives = self.json_derive_selections(derives);
        for name in declared {
            if !native_derives.contains(&name) {
                native_derives.push(name);
            }
        }
        Self::append_implied_derives(&mut native_derives);
        for derive in derives::plain_model_derives() {
            if !native_derives.iter().any(|name| name == derive) {
                native_derives.push(derive.to_owned());
            }
        }
        native_derives
    }

    /// Select the native derives a type's `@derive(...)` names stand for, in order and without repeats.
    ///
    /// A std.serde.json protocol, named by its trait or through the module, selects serde's own derive path, as legacy
    /// derive emission does; every other selection keeps its name.
    pub(super) fn json_derive_selections(&self, derives: &[String]) -> Vec<String> {
        let mut native_derives = Vec::new();
        for derive in derives {
            let selected = if self
                .module_path_for_imported_name(derive)
                .is_some_and(|path| incan_lang::lang::stdlib::is_stdlib_json_trait_module_path(&path))
            {
                vec!["serde::Serialize".to_owned(), "serde::Deserialize".to_owned()]
            } else {
                let path = self.derive_trait_path(derive);
                let protocol = path.split_last().and_then(|(name, module)| {
                    incan_lang::lang::stdlib::stdlib_json_trait_id_for_identity(module, name)
                });
                match protocol {
                    Some(incan_lang::lang::stdlib::StdlibJsonTraitId::Serialize) => {
                        vec!["serde::Serialize".to_owned()]
                    }
                    Some(incan_lang::lang::stdlib::StdlibJsonTraitId::Deserialize) => {
                        vec!["serde::Deserialize".to_owned()]
                    }
                    None => vec![derive.clone()],
                }
            };
            for name in selected {
                if !native_derives.contains(&name) {
                    native_derives.push(name);
                }
            }
        }
        native_derives
    }

    /// Expand checked module and trait derive selections for Body IR without altering legacy's selection table.
    ///
    /// The checker owns import resolution and RFC 024 metadata. Body IR must retain the selected Rust paths here,
    /// including TOML bundles and aliases, instead of compensating for their source spellings in plan lowering.
    /// Descriptor checks structural snapshots but emits no Rust macro. A derive with no expansion stays named so
    /// the direct admission predicate refuses it rather than silently discarding behavior.
    pub(super) fn body_ir_native_derive_selection(&mut self, selection: &[String]) -> Vec<String> {
        let mut result = Vec::new();
        for name in selection {
            if name == "Descriptor" {
                continue;
            }
            if derives::from_str(name).is_some() {
                if !result.contains(name) {
                    result.push(name.clone());
                }
                continue;
            }
            let mut paths = Vec::new();
            if let Some(module) = self.module_path_for_imported_name(name)
                && let Some(traits) = self.lookup_derivable_traits(&module)
            {
                for trait_name in traits {
                    paths.extend(self.body_ir_trait_derive_paths(&module, &trait_name));
                }
            } else {
                let path = self.derive_trait_path(name);
                if let Some((trait_name, module)) = path.split_last() {
                    paths = self.body_ir_trait_derive_paths(module, trait_name);
                }
            }
            if paths.is_empty() {
                let imported = self.rust_import_path_for_local_name(name);
                let candidate = imported.as_deref().unwrap_or(name);
                if let Some(path) = self.checked_native_derive_macro(candidate) {
                    paths.push(path);
                } else {
                    paths.push(name.clone());
                }
            }
            for path in paths {
                if self.checked_native_derive_macro(&path).is_some() {
                    self.type_info.derivations.native_macro_paths.insert(path.clone());
                }
                if !result.contains(&path) {
                    result.push(path);
                }
            }
        }
        result
    }

    /// Return the Rust derive metadata of one checked trait, from its source module or its Loaf, in declaration order.
    pub(super) fn body_ir_trait_derive_paths(&mut self, module: &[String], trait_name: &str) -> Vec<String> {
        let key = format!("{}.{}", module.join("."), trait_name);
        if let Some(paths) = self.dependency_trait_rust_derive_paths.get(&key) {
            return paths.clone();
        }
        self.loaf_trait_rust_derive_paths(module, trait_name)
            .unwrap_or_default()
    }
}
