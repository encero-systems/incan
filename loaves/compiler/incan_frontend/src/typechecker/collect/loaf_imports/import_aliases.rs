//! Symbol kind and resolved type remapping through import aliases.

use super::*;

impl TypeChecker {
    /// Rewrite imported semantic type references through type aliases from the source library manifest.
    pub(in crate::typechecker) fn remap_symbol_kind_with_import_aliases(
        &self,
        kind: &mut SymbolKind,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        if imported_type_aliases.is_empty() {
            return;
        }

        match kind {
            SymbolKind::Capability(info) => {
                for (_, ty) in &mut info.scope {
                    Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                }
            }
            SymbolKind::Variable(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.ty, imported_type_aliases);
            }
            SymbolKind::Static(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.ty, imported_type_aliases);
            }
            SymbolKind::Function(info) => {
                Self::remap_function_info_with_import_aliases(info, imported_type_aliases);
            }
            SymbolKind::FunctionOverloads(overloads) => {
                for overload in overloads {
                    Self::remap_function_info_with_import_aliases(&mut overload.info, imported_type_aliases);
                }
            }
            SymbolKind::Type(ty_info) => match ty_info {
                TypeInfo::Class(info) => {
                    Self::remap_class_info_with_import_aliases(info, imported_type_aliases);
                }
                TypeInfo::Model(info) => {
                    Self::remap_type_bounds_with_import_aliases(&mut info.trait_adoptions, imported_type_aliases);
                    for field in info.fields.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut field.ty, imported_type_aliases);
                    }
                    for property in info.properties.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut property.return_type, imported_type_aliases);
                    }
                    for method in info.methods.values_mut() {
                        Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                    }
                    for overloads in info.method_overloads.values_mut() {
                        for method in overloads {
                            Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                        }
                    }
                }
                TypeInfo::Newtype(info) => {
                    Self::remap_resolved_type_with_import_aliases(&mut info.underlying, imported_type_aliases);
                    Self::remap_type_bounds_with_import_aliases(&mut info.trait_adoptions, imported_type_aliases);
                    for method in info.methods.values_mut() {
                        Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                    }
                    for overloads in info.method_overloads.values_mut() {
                        for method in overloads {
                            Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                        }
                    }
                }
                TypeInfo::Enum(info) => {
                    Self::remap_type_bounds_with_import_aliases(&mut info.trait_adoptions, imported_type_aliases);
                    for fields in info.variant_fields.values_mut() {
                        for field in fields {
                            Self::remap_resolved_type_with_import_aliases(field, imported_type_aliases);
                        }
                    }
                    for method in info.methods.values_mut() {
                        Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                    }
                    for overloads in info.method_overloads.values_mut() {
                        for method in overloads {
                            Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                        }
                    }
                }
                TypeInfo::TypeAlias | TypeInfo::Builtin => {}
            },
            SymbolKind::Trait(info) => {
                for (_, type_args) in &mut info.supertraits {
                    for type_arg in type_args {
                        Self::remap_resolved_type_with_import_aliases(type_arg, imported_type_aliases);
                    }
                }
                for method in info.methods.values_mut() {
                    Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                }
                for (_, ty) in &mut info.requires {
                    Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                }
                for property in info.properties.values_mut() {
                    Self::remap_resolved_type_with_import_aliases(&mut property.return_type, imported_type_aliases);
                }
            }
            SymbolKind::Variant(info) => {
                if let Some(alias) = imported_type_aliases.get(&info.enum_name) {
                    info.enum_name = alias.clone();
                }
                for field in &mut info.fields {
                    Self::remap_resolved_type_with_import_aliases(field, imported_type_aliases);
                }
            }
            SymbolKind::Field(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.ty, imported_type_aliases);
            }
            SymbolKind::Property(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.return_type, imported_type_aliases);
            }
            SymbolKind::Module(_) | SymbolKind::RustItem(_) => {}
        }
    }

    /// Remap a compiled class's inheritance and member contracts into the import namespace. Method overloads keep
    /// their checked order; field, property and trait-bound types use the same recursive alias remapping rule.
    pub(super) fn remap_class_info_with_import_aliases(
        info: &mut ClassInfo,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        if let Some(extends) = &mut info.extends
            && let Some(alias) = imported_type_aliases.get(extends)
        {
            *extends = alias.clone();
        }
        Self::remap_type_bounds_with_import_aliases(&mut info.trait_adoptions, imported_type_aliases);
        for field in info.fields.values_mut() {
            Self::remap_resolved_type_with_import_aliases(&mut field.ty, imported_type_aliases);
        }
        for property in info.properties.values_mut() {
            Self::remap_resolved_type_with_import_aliases(&mut property.return_type, imported_type_aliases);
        }
        for method in info.methods.values_mut() {
            Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
        }
        for overloads in info.method_overloads.values_mut() {
            for method in overloads {
                Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
            }
        }
    }

    /// Rewrite all resolved carrier types in one imported function signature and its generic bounds.
    pub(in crate::typechecker::collect) fn remap_function_info_with_import_aliases(
        info: &mut FunctionInfo,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        for param in &mut info.params {
            Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
        }
        Self::remap_resolved_type_with_import_aliases(&mut info.return_type, imported_type_aliases);
        for bounds in info.type_param_bound_details.values_mut() {
            Self::remap_type_bounds_with_import_aliases(bounds, imported_type_aliases);
        }
    }

    /// Rewrite all resolved carrier types in one imported method signature and its generic or trait-target bounds.
    pub(in crate::typechecker::collect) fn remap_method_info_with_import_aliases(
        info: &mut MethodInfo,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        for param in &mut info.params {
            Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
        }
        Self::remap_resolved_type_with_import_aliases(&mut info.return_type, imported_type_aliases);
        for bounds in info.type_param_bound_details.values_mut() {
            Self::remap_type_bounds_with_import_aliases(bounds, imported_type_aliases);
        }
        if let Some(target) = info.trait_target.as_mut() {
            Self::remap_type_bound_with_import_aliases(target, imported_type_aliases);
        }
    }

    /// Rewrite resolved generic arguments carried by imported trait-bound metadata.
    pub(in crate::typechecker::collect) fn remap_type_bounds_with_import_aliases(
        bounds: &mut [TypeBoundInfo],
        imported_type_aliases: &HashMap<String, String>,
    ) {
        for bound in bounds {
            Self::remap_type_bound_with_import_aliases(bound, imported_type_aliases);
        }
    }

    /// Retain an imported bound's trait binding and generic arguments in the same canonical namespace as its signature.
    pub(in crate::typechecker::collect) fn remap_type_bound_with_import_aliases(
        bound: &mut TypeBoundInfo,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        if let Some(name) = imported_type_aliases.get(&bound.name) {
            bound.source_name.get_or_insert_with(|| bound.name.clone());
            bound.name = name.clone();
        }
        for type_arg in &mut bound.type_args {
            Self::remap_resolved_type_with_import_aliases(type_arg, imported_type_aliases);
        }
    }

    /// Rewrite resolved type names through import aliases after stdlib materialization.
    pub(in crate::typechecker) fn remap_resolved_type_with_import_aliases(
        ty: &mut ResolvedType,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        match ty {
            ResolvedType::Named(name) => {
                if let Some(alias) = imported_type_aliases.get(name) {
                    *name = alias.clone();
                }
            }
            ResolvedType::Generic(name, args) => {
                if let Some(alias) = imported_type_aliases.get(name) {
                    *name = alias.clone();
                }
                for arg in args {
                    Self::remap_resolved_type_with_import_aliases(arg, imported_type_aliases);
                }
            }
            ResolvedType::Function(params, return_type) => {
                for param in params {
                    Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
                }
                Self::remap_resolved_type_with_import_aliases(return_type, imported_type_aliases);
            }
            ResolvedType::Tuple(items) => {
                for item in items {
                    Self::remap_resolved_type_with_import_aliases(item, imported_type_aliases);
                }
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner)
            | ResolvedType::TypeToken(inner) => {
                Self::remap_resolved_type_with_import_aliases(inner, imported_type_aliases);
            }
            ResolvedType::FrozenDict(key, value) => {
                Self::remap_resolved_type_with_import_aliases(key, imported_type_aliases);
                Self::remap_resolved_type_with_import_aliases(value, imported_type_aliases);
            }
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
            | ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::Unknown => {}
        }
    }
}
