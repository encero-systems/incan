//! Conditional automatic adoption of the primitive typed-value protocol (RFC 173, #2519).
//!
//! This checked relation decides availability, not serialization. It never derives a persistent identity or reads
//! display metadata. Runtime hooks are separately generated from the selected declaration and concrete types.

use std::collections::HashMap;

use super::TypeChecker;
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::{ResolvedType, TypeBoundInfo, TypeInfo};
use incan_lang::lang::types::{UNION_TYPE_NAME, collections::CollectionTypeId, numerics};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

/// The two ordinary primitive traits for which RFC 173 supplies conditional generated implementations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValueProtocol {
    Walk,
    Reconstruct,
}

impl ValueProtocol {
    /// Classify only a checked trait declaration from the primitive reflection provider. Source aliases preserve
    /// that declaration; a same-spelled trait in another provider never acquires generated behavior.
    fn from_identity(identity: &CanonicalSymbolId) -> Option<Self> {
        if identity.kind != SemanticSourceTargetKind::Trait {
            return None;
        }
        let SymbolOrigin::Package { library, module_path } = &identity.origin else {
            return None;
        };
        if library != "incan_stdlib_primitives" || module_path.as_slice() != ["primitives", "reflection"] {
            return None;
        }
        match identity.declaration_name.as_str() {
            "Walk" => Some(Self::Walk),
            "Reconstruct" => Some(Self::Reconstruct),
            _ => None,
        }
    }
}

impl TypeChecker {
    /// Answer an authenticated protocol bound from the concrete stored types. Returning None leaves every other
    /// trait to its ordinary relation; missing checked ownership is never accepted by its short name.
    pub(super) fn typed_value_protocol_bound(&self, ty: &ResolvedType, bound: &TypeBoundInfo) -> Option<bool> {
        if !bound.type_args.is_empty() {
            return None;
        }
        let identity = self
            .symbols
            .lookup(&bound.name)
            .and_then(|symbol| self.symbols.identity_of(symbol))
            .cloned()
            .or_else(|| {
                let (path, name) = self.type_bound_identity(bound)?;
                self.package_trait_declaration(&path, &name)
            })?;
        let protocol = ValueProtocol::from_identity(&identity)?;
        let ty = self.expand_type_aliases(ty.clone());
        Some(self.typed_value_protocol_support(&ty, bound, protocol, &mut Vec::new()))
    }

    /// Check one concrete value recursively. Generic arguments are checked before the declaration recursion guard,
    /// so revisiting a recursive nominal never hides a newly introduced unsupported actual argument.
    pub(in crate::typechecker) fn typed_value_protocol_support(
        &self,
        ty: &ResolvedType,
        bound: &TypeBoundInfo,
        protocol: ValueProtocol,
        visiting: &mut Vec<String>,
    ) -> bool {
        if let Some(name) = self.active_type_param_name(ty) {
            return self.active_type_param_satisfies_bound_info(name, bound, &HashMap::new());
        }
        let all = |types: &[ResolvedType], visiting: &mut Vec<String>| {
            types
                .iter()
                .all(|ty| self.typed_value_protocol_support(ty, bound, protocol, visiting))
        };
        match ty {
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit => true,
            ResolvedType::Tuple(items) => all(items, visiting),
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => {
                self.typed_value_protocol_support(inner, bound, protocol, visiting)
            }
            ResolvedType::FrozenDict(key, value) => {
                self.typed_value_protocol_support(key, bound, protocol, visiting)
                    && self.typed_value_protocol_support(value, bound, protocol, visiting)
            }
            ResolvedType::Named(name) => self.nominal_value_protocol_support(name, &[], bound, protocol, visiting),
            ResolvedType::Generic(name, arguments) => {
                if numerics::decimal_constructor_from_str(name).is_some() {
                    return true;
                }
                if name == UNION_TYPE_NAME {
                    return !arguments.is_empty() && all(arguments, visiting);
                }
                if let Some(collection) = super::helpers::collection_type_id(name) {
                    let arity = match collection {
                        CollectionTypeId::Generator => return false,
                        CollectionTypeId::Dict | CollectionTypeId::FrozenDict | CollectionTypeId::Result => Some(2),
                        CollectionTypeId::Tuple => None,
                        _ => Some(1),
                    };
                    return arity.is_none_or(|arity| arguments.len() == arity) && all(arguments, visiting);
                }
                self.nominal_value_protocol_support(name, arguments, bound, protocol, visiting)
            }
            // A view is not an owned stored value. An explicit authored adopter must describe its own chosen form;
            // runtime resources, unknown facts and unresolved templates do not receive automatic protocol authority.
            ResolvedType::Never
            | ResolvedType::Unknown
            | ResolvedType::CallSiteInfer
            | ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::Ref(_)
            | ResolvedType::RefMut(_)
            | ResolvedType::RustPath(_)
            | ResolvedType::TypeToken(_)
            | ResolvedType::Trait(_)
            | ResolvedType::Function(_, _) => false,
        }
    }

    /// Inspect all stored members, including private ones, with the selected nominal's concrete substitutions.
    /// Classes and rusttypes require ordinary explicit adoption. An authored protocol hook prevents mixing a custom
    /// stored form with generated hooks; ordinary trait conformance remains responsible for its complete signature.
    fn nominal_value_protocol_support(
        &self,
        name: &str,
        arguments: &[ResolvedType],
        bound: &TypeBoundInfo,
        protocol: ValueProtocol,
        visiting: &mut Vec<String>,
    ) -> bool {
        let Some(info) = self.lookup_semantic_type_info(name) else {
            return false;
        };
        let (parameters, methods, automatic) = match info {
            TypeInfo::Model(info) => (&info.type_params, &info.methods, true),
            TypeInfo::Enum(info) => (&info.type_params, &info.methods, true),
            TypeInfo::Newtype(info) => (&info.type_params, &info.methods, !info.is_rusttype),
            TypeInfo::Class(info) => (&info.type_params, &info.methods, false),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return false,
        };
        if parameters.len() != arguments.len() {
            return false;
        }
        let custom = ["__walk__", "__shape__", "__reconstruct__"]
            .iter()
            .any(|name| methods.contains_key(*name));
        if !automatic || custom {
            return self.type_implements_trait_with_args(name, arguments, &bound.name, &[]);
        }
        if !arguments
            .iter()
            .all(|argument| self.typed_value_protocol_support(argument, bound, protocol, visiting))
        {
            return false;
        }
        if visiting.iter().any(|owner| owner == name) {
            return true;
        }
        visiting.push(name.to_owned());
        let substitutions = type_param_subst_map(parameters, arguments);
        let supports = |ty: &ResolvedType, visiting: &mut Vec<String>| {
            let concrete = self.expand_type_aliases(substitute_resolved_type(ty, &substitutions));
            self.typed_value_protocol_support(&concrete, bound, protocol, visiting)
        };
        let supported = match info {
            TypeInfo::Model(info) => info
                .field_order
                .iter()
                .all(|name| info.fields.get(name).is_some_and(|field| supports(&field.ty, visiting))),
            TypeInfo::Enum(info) => info
                .variant_fields
                .values()
                .all(|fields| fields.iter().all(|field| supports(field, visiting))),
            TypeInfo::Newtype(info) => supports(&info.underlying, visiting),
            _ => false,
        };
        visiting.pop();
        supported
    }
}
