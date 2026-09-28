//! The JSON form of a declaration's members: a type that provides a `std.serde.json` trait needs it of every member.
//!
//! A `model`, `class`, `enum` or newtype that derives or adopts `Serialize` (or `Deserialize`) serializes (or parses)
//! each field, variant payload or underlying value, so every one of those types must provide the trait as well. A
//! `model` or `class` member that neither derives nor adopts it used to pass the check and fail the build instead
//! (#1886, #1867). This module refuses that member at check time.
//!
//! `decimal` and the frozen types have no JSON form at all, so a member that holds one is refused too (#1561).
//!
//! The relation is [`TypeChecker::type_without_json_form`]'s, extended in the one way a declaration's members differ
//! from a route payload: an `enum` or newtype member serializes through its contents, since lowering gives such a
//! member the serde derive of the declaration that holds it, so the search goes on into its payloads or underlying
//! value. Only a `model` or `class` that certainly lacks the trait is refused; a type the checker cannot classify is
//! left to the build.

use crate::ast::Span;
use crate::diagnostics::errors::{self, DerivedMember};
use crate::symbols::{ResolvedType, TypeBoundInfo, TypeInfo};
use incan_lang::lang::stdlib::{self, StdlibJsonTraitId};
use incan_lang::lang::types::numerics;

use super::TypeChecker;
use super::helpers::collection_type_id;

/// The type inside a member type that has no JSON form for a protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonFormGap {
    /// A `model` or `class` that neither derives nor adopts the protocol trait, by name.
    Nominal(String),
    /// `decimal` or a frozen type, which no protocol serializes, as the checker spells it (#1561).
    Builtin(String),
}

/// Return the spelling of a builtin type with no JSON form, `decimal` or a frozen type (#1561), or `None` for any other
/// type.
///
/// Neither the decimal runtime type nor the frozen types implement the JSON protocols, so a declaration that holds one
/// and provides `Serialize` or `Deserialize` cannot build. Their contents are not searched: the type itself has no
/// form.
pub(in crate::typechecker) fn builtin_type_without_json_form(ty: &ResolvedType) -> Option<String> {
    match ty {
        ResolvedType::FrozenStr
        | ResolvedType::FrozenBytes
        | ResolvedType::FrozenList(_)
        | ResolvedType::FrozenSet(_)
        | ResolvedType::FrozenDict(_, _) => Some(ty.to_string()),
        ResolvedType::Generic(name, _) if numerics::decimal_constructor_from_str(name).is_some() => {
            Some(ty.to_string())
        }
        _ => None,
    }
}

/// The source name of a `std.serde.json` protocol trait, for diagnostics.
fn protocol_name(protocol: StdlibJsonTraitId) -> &'static str {
    match protocol {
        StdlibJsonTraitId::Serialize => "Serialize",
        StdlibJsonTraitId::Deserialize => "Deserialize",
    }
}

impl TypeChecker {
    /// Return whether one trait spelling names the `std.serde.json` `protocol` trait.
    ///
    /// The spelling is resolved to its declaring module and source name as a bound is
    /// ([`Self::trait_bound_module_path`], [`Self::trait_bound_source_name`]); a known spelling of the stdlib trait
    /// (`Serialize`, `json.Serialize`) also counts, so a facade re-export the resolution does not follow is never
    /// taken for a missing trait.
    fn names_json_protocol(
        &self,
        spelling: &str,
        module_path: Option<&[String]>,
        source_name: Option<&str>,
        protocol: StdlibJsonTraitId,
    ) -> bool {
        let module_path = module_path
            .map(<[String]>::to_vec)
            .or_else(|| self.trait_bound_module_path(spelling));
        let source_name = source_name
            .map(str::to_string)
            .or_else(|| self.trait_bound_source_name(spelling))
            .unwrap_or_else(|| spelling.to_string());
        let by_identity = module_path
            .as_deref()
            .and_then(|path| stdlib::stdlib_json_trait_id_for_identity(path, &source_name));
        by_identity == Some(protocol) || stdlib::stdlib_json_trait_id(spelling) == Some(protocol)
    }

    /// Return whether a type's trait adoptions, which include what its derives adopt, provide the `std.serde.json`
    /// `protocol` trait, directly or through a supertrait of an adopted trait.
    pub(in crate::typechecker) fn adoptions_provide_json_protocol(
        &self,
        adoptions: &[TypeBoundInfo],
        protocol: StdlibJsonTraitId,
    ) -> bool {
        adoptions.iter().any(|adoption| {
            self.names_json_protocol(
                &adoption.name,
                adoption.module_path.as_deref(),
                adoption.source_name.as_deref(),
                protocol,
            ) || self
                .semantic_supertrait_closure(&adoption.name)
                .iter()
                .any(|(supertrait, _)| self.names_json_protocol(supertrait, None, None, protocol))
        })
    }

    /// Return whether a `@derive(...)` spelling is one the compiler classifies: a builtin derive, or a `std.serde.json`
    /// trait or the `std.serde.json` module, whose adoptions the type's trait adoptions record.
    pub(in crate::typechecker) fn derive_is_classified(&self, derive: &str) -> bool {
        if incan_lang::lang::derives::from_str(derive).is_some() {
            return true;
        }
        if self
            .module_path_for_imported_name(derive)
            .is_some_and(|path| stdlib::is_stdlib_json_trait_module_path(&path))
        {
            return true;
        }
        let path = self.derive_trait_path(derive);
        path.split_last().is_some_and(|(trait_name, module_path)| {
            stdlib::stdlib_json_trait_id_for_identity(module_path, trait_name).is_some()
        })
    }

    /// Return the first type inside a member type of a declaration that provides `protocol` which certainly has no JSON
    /// form for it, or `None` when every part has, or may have, one.
    ///
    /// `decimal` and the frozen types have none ([`builtin_type_without_json_form`]). Collections, `Option`, `Result`
    /// and tuples carry their elements, and a generic nominal needs its type arguments to provide the trait too. A
    /// `model` or `class` is decided by [`Self::nominal_certainly_lacks_json_form`]. An `enum` or newtype that does
    /// not provide the trait itself is searched through its payloads or underlying value, which it serializes; one
    /// that provides it is checked at its own declaration. `visiting` stops a type that contains itself.
    fn member_type_without_json_form(
        &self,
        ty: &ResolvedType,
        protocol: StdlibJsonTraitId,
        visiting: &mut Vec<String>,
    ) -> Option<JsonFormGap> {
        if let Some(builtin) = builtin_type_without_json_form(ty) {
            return Some(JsonFormGap::Builtin(builtin));
        }
        match ty {
            ResolvedType::Named(name) => self.nominal_member_without_json_form(name, &[], protocol, visiting),
            ResolvedType::Generic(name, args) => {
                if collection_type_id(name).is_some() {
                    return args
                        .iter()
                        .find_map(|arg| self.member_type_without_json_form(arg, protocol, visiting));
                }
                self.nominal_member_without_json_form(name, args, protocol, visiting)
            }
            ResolvedType::Tuple(items) => items
                .iter()
                .find_map(|item| self.member_type_without_json_form(item, protocol, visiting)),
            _ => None,
        }
    }

    /// The nominal case of [`Self::member_type_without_json_form`]: the nominal itself, then its type arguments.
    fn nominal_member_without_json_form(
        &self,
        name: &str,
        args: &[ResolvedType],
        protocol: StdlibJsonTraitId,
        visiting: &mut Vec<String>,
    ) -> Option<JsonFormGap> {
        if self.nominal_certainly_lacks_json_form(name, protocol) {
            return Some(JsonFormGap::Nominal(name.to_string()));
        }
        let contents = match self.lookup_type_info(name) {
            Some(TypeInfo::Enum(info)) if !self.adoptions_provide_json_protocol(&info.trait_adoptions, protocol) => {
                info.variant_fields.values().flatten().cloned().collect::<Vec<_>>()
            }
            Some(TypeInfo::Newtype(info))
                if !info.is_rusttype && !self.adoptions_provide_json_protocol(&info.trait_adoptions, protocol) =>
            {
                vec![info.underlying.clone()]
            }
            _ => Vec::new(),
        };
        if !contents.is_empty() && !visiting.iter().any(|visited| visited == name) {
            visiting.push(name.to_string());
            let found = contents
                .iter()
                .find_map(|content| self.member_type_without_json_form(content, protocol, visiting));
            visiting.pop();
            if found.is_some() {
                return found;
            }
        }
        args.iter()
            .find_map(|arg| self.member_type_without_json_form(arg, protocol, visiting))
    }

    /// Refuse each member of a declaration that provides a `std.serde.json` trait whose type has no JSON form for it
    /// (#1886, #1867).
    ///
    /// `adoptions` are the declaration's trait adoptions, those its derives adopt included; `members` pair each
    /// member with its resolved type and the span of its declaration.
    pub(in crate::typechecker) fn refuse_members_without_json_form(
        &mut self,
        owner_kind: &str,
        owner_name: &str,
        adoptions: &[TypeBoundInfo],
        members: &[(DerivedMember<'_>, ResolvedType, Span)],
    ) {
        for protocol in [StdlibJsonTraitId::Serialize, StdlibJsonTraitId::Deserialize] {
            if !self.adoptions_provide_json_protocol(adoptions, protocol) {
                continue;
            }
            for (member, member_ty, span) in members {
                let error = match self.member_type_without_json_form(member_ty, protocol, &mut Vec::new()) {
                    None => continue,
                    Some(JsonFormGap::Nominal(holder)) => errors::member_type_lacks_json_protocol(
                        owner_kind,
                        owner_name,
                        *member,
                        &member_ty.to_string(),
                        &holder,
                        protocol_name(protocol),
                        *span,
                    ),
                    Some(JsonFormGap::Builtin(holder)) => errors::member_type_has_no_json_form(
                        owner_kind,
                        owner_name,
                        *member,
                        &member_ty.to_string(),
                        &holder,
                        protocol_name(protocol),
                        *span,
                    ),
                };
                self.errors.push(error);
            }
        }
    }

    /// Return the spelling of the first type inside the type of a value `json_stringify` writes that certainly has no
    /// `Serialize` form, or `None` when every part has, or may have, one.
    ///
    /// Collections, `Option`, `Result`, tuples and the frozen collections carry their elements, and a generic nominal
    /// its type arguments. A `model` or `class` is decided by [`Self::nominal_certainly_lacks_json_form`]. A type
    /// parameter of the body has a `Serialize` form only through its bounds, and a trait-typed value only through the
    /// trait or a supertrait of it. Any other type, an `enum` or newtype included, is left to the build.
    pub(in crate::typechecker) fn value_type_without_serialize_form(&self, ty: &ResolvedType) -> Option<String> {
        let protocol = StdlibJsonTraitId::Serialize;
        if let Some(name) = self.active_type_param_name(ty) {
            let bounds = self
                .current_type_param_bound_details
                .iter()
                .rev()
                .find_map(|frame| frame.get(name))?;
            return (!self.adoptions_provide_json_protocol(bounds, protocol)).then(|| name.to_string());
        }
        match ty {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.value_type_without_serialize_form(inner),
            ResolvedType::Named(name) if self.lookup_trait_info(name).is_some() => {
                let trait_bound = TypeBoundInfo {
                    name: name.clone(),
                    source_name: None,
                    type_args: Vec::new(),
                    module_path: None,
                    implementation_type_params: Vec::new(),
                    inferred: false,
                };
                (!self.adoptions_provide_json_protocol(std::slice::from_ref(&trait_bound), protocol))
                    .then(|| name.clone())
            }
            ResolvedType::Named(name) => self
                .nominal_certainly_lacks_json_form(name, protocol)
                .then(|| name.clone()),
            ResolvedType::Generic(name, args) => {
                if collection_type_id(name).is_none() && self.nominal_certainly_lacks_json_form(name, protocol) {
                    return Some(name.clone());
                }
                args.iter().find_map(|arg| self.value_type_without_serialize_form(arg))
            }
            ResolvedType::Tuple(items) => items
                .iter()
                .find_map(|item| self.value_type_without_serialize_form(item)),
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => {
                self.value_type_without_serialize_form(inner)
            }
            ResolvedType::FrozenDict(key, value) => self
                .value_type_without_serialize_form(key)
                .or_else(|| self.value_type_without_serialize_form(value)),
            _ => None,
        }
    }

    /// Return the trait adoptions recorded for the nominal `name`, those its derives adopt included.
    pub(in crate::typechecker) fn nominal_trait_adoptions(&self, name: &str) -> Vec<TypeBoundInfo> {
        match self.lookup_type_info(name) {
            Some(TypeInfo::Model(info)) => info.trait_adoptions.clone(),
            Some(TypeInfo::Class(info)) => info.trait_adoptions.clone(),
            Some(TypeInfo::Enum(info)) => info.trait_adoptions.clone(),
            Some(TypeInfo::Newtype(info)) => info.trait_adoptions.clone(),
            _ => Vec::new(),
        }
    }
}
