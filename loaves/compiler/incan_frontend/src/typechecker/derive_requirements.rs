//! The compiler's one relation for the builtin derives `Clone`, `Debug`, `Eq` and `Hash`, and the refusals built on it.
//!
//! [`TypeChecker::derive_support`] answers whether a checked type implements one of those derives in the generated
//! program, reading the surface-type registry for runtime types ([`surface_types::derive_support`]), the derive
//! implication table for declared types ([`derives::implied_derives`]), and the automatic derives lowering gives a
//! `model`, `class`, `enum` or newtype. Generic bound checks (`type_satisfies_explicit_bound`) and the clone
//! requirement of collection methods (`is_clone_type`) consult it before their own fallbacks, and the refusals below
//! are built on it:
//!
//! - a `model` or `class` field, or an `enum` payload, whose type cannot carry the automatic `Clone` and `Debug`
//!   derives (`INCAN-T0113`, #1754);
//! - a set element or dict key type without `Eq` and `Hash`, including the type argument of a generic function or
//!   method whose body hashes its parameter, a requirement inferred into its signature by `hash_key_inference`
//!   (`INCAN-T0114`, #1758).
//!
//! The relation answers [`DeriveSupport::Unknown`] rather than guess: for a type parameter, a Rust-origin type, a
//! `rusttype`, a runtime type the registry records nothing for, and the `Eq` / `Hash` of a type declared in another
//! module, whose manifest may not list every derive. The refusals never refuse on an unknown answer.

use std::collections::HashMap;

use super::TypeChecker;
use crate::ast::{CallArg, DictEntry, Expr, ListEntry, ParamKind, Span, Spanned, Type};
use crate::diagnostics::CompileError;
use crate::diagnostics::errors::{self, DerivedMember, HashRemedy, HashedCollectionRole};
use crate::symbols::{CallableParam, NewtypeInfo, ResolvedType, SymbolKind, TypeInfo};
use crate::typechecker::helpers::collection_type_id;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::surface::types::{self as surface_types, SurfaceDeriveSupport, SurfaceTypeId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics;
use incan_semantics_core::CanonicalSymbolId;

/// What the compiler knows about one type's implementation of a builtin derive.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::typechecker) enum DeriveSupport {
    /// The type implements the derive in the generated program.
    Supported,
    /// The type does not implement it; the payload is the type inside it that lacks the derive (the type itself, or
    /// `JoinHandle[int]` inside `list[JoinHandle[int]]`).
    Missing(ResolvedType),
    /// The compiler cannot tell; each consumer applies its own policy for an unknown type.
    Unknown,
}

/// `@rust.derive(...)` facts about one nominal type declared in the module being checked.
///
/// Recorded during collection for every local `model`, `class`, `enum` and newtype, so the presence of an entry is
/// also what marks a declaration as local: the derive list of a local declaration is complete, while an imported one
/// comes from a manifest that may omit derives.
#[derive(Debug, Clone, Default)]
pub(in crate::typechecker) struct LocalDeriveFacts {
    /// Builtin derives spelled in `@rust.derive(...)`, by bare name (`Hash`) or by `std`/`core` path.
    pub rust_builtin_derives: Vec<DeriveId>,
    /// Whether `@rust.derive(...)` names a derive macro the compiler cannot classify, which may implement anything.
    pub has_unclassified_rust_derive: bool,
}

/// Whether `derive` is one the relation answers for.
fn is_relation_derive(derive: DeriveId) -> bool {
    matches!(
        derive,
        DeriveId::Clone | DeriveId::Debug | DeriveId::Eq | DeriveId::Hash
    )
}

impl TypeChecker {
    // ========================================================================
    // The relation
    // ========================================================================

    /// Return whether `ty` implements `derive` (`Clone`, `Debug`, `Eq` or `Hash`) in the generated program.
    ///
    /// Any other derive is [`DeriveSupport::Unknown`].
    pub(in crate::typechecker) fn derive_support(&self, ty: &ResolvedType, derive: DeriveId) -> DeriveSupport {
        if !is_relation_derive(derive) {
            return DeriveSupport::Unknown;
        }
        self.derive_support_assuming(ty, derive, &[], &mut Vec::new())
    }

    /// The relation with a set of type-parameter names assumed to implement `derive`, and the newtypes being visited.
    ///
    /// A derive on a generic declaration bounds each type parameter by the derived trait, so the automatic derives of
    /// a generic newtype are decided with its own parameters assumed to implement them. `visiting` stops a newtype
    /// whose underlying type names itself.
    fn derive_support_assuming(
        &self,
        ty: &ResolvedType,
        derive: DeriveId,
        assumed: &[String],
        visiting: &mut Vec<String>,
    ) -> DeriveSupport {
        let hashing = matches!(derive, DeriveId::Eq | DeriveId::Hash);
        match ty {
            ResolvedType::Int
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit => DeriveSupport::Supported,
            ResolvedType::Float => {
                if hashing {
                    DeriveSupport::Missing(ty.clone())
                } else {
                    DeriveSupport::Supported
                }
            }
            ResolvedType::Numeric(id) => {
                if hashing && numerics::is_binary_float(*id) {
                    DeriveSupport::Missing(ty.clone())
                } else {
                    DeriveSupport::Supported
                }
            }
            ResolvedType::Tuple(items) => self.all_derive_support(items, derive, assumed, visiting),
            // The frozen collections are baked slices: `Clone`, `Debug` and `Eq` follow their contents; none hashes.
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => {
                if derive == DeriveId::Hash {
                    DeriveSupport::Missing(ty.clone())
                } else {
                    self.derive_support_assuming(inner, derive, assumed, visiting)
                }
            }
            ResolvedType::FrozenDict(key, value) => {
                if derive == DeriveId::Hash {
                    DeriveSupport::Missing(ty.clone())
                } else {
                    let pair = [key.as_ref().clone(), value.as_ref().clone()];
                    self.all_derive_support(&pair, derive, assumed, visiting)
                }
            }
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
                    return DeriveSupport::Supported;
                }
                if let Some(collection) = collection_type_id(name) {
                    return match collection {
                        CollectionTypeId::Generator => DeriveSupport::Unknown,
                        CollectionTypeId::Set
                        | CollectionTypeId::Dict
                        | CollectionTypeId::FrozenList
                        | CollectionTypeId::FrozenSet
                        | CollectionTypeId::FrozenDict
                            if derive == DeriveId::Hash =>
                        {
                            DeriveSupport::Missing(ty.clone())
                        }
                        _ => self.all_derive_support(args, derive, assumed, visiting),
                    };
                }
                if let Some(surface_type) = self.surface_type_named(name) {
                    return self.surface_derive_support(ty, surface_type, args, derive, assumed, visiting);
                }
                self.declared_derive_support(ty, name, args, derive, assumed, visiting)
            }
            ResolvedType::Never
            | ResolvedType::Unknown
            | ResolvedType::CallSiteInfer
            | ResolvedType::RustPath(_)
            | ResolvedType::Function(_, _)
            | ResolvedType::Ref(_)
            | ResolvedType::RefMut(_)
            | ResolvedType::TypeToken(_)
            | ResolvedType::SelfType => DeriveSupport::Unknown,
        }
    }

    /// Combine the answers for types that must all implement `derive`: the first missing one wins, then any unknown.
    fn all_derive_support(
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

    /// Answer for a stdlib surface type from the registry.
    fn surface_derive_support(
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
    fn declared_derive_support(
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
            TypeInfo::Model(model) => self.automatic_nominal_derive(name, &model.derives, &model.traits, derive, false),
            TypeInfo::Class(class) => {
                // Lowering drops the automatic `Debug` of a private adapter class holding direct Rust state.
                let opaque_debug = name.starts_with('_') && derive == DeriveId::Debug;
                self.automatic_nominal_derive(name, &class.derives, &class.traits, derive, opaque_debug)
            }
            TypeInfo::Enum(en) => self.automatic_nominal_derive(name, &en.derives, &en.traits, derive, false),
            TypeInfo::Newtype(newtype) if newtype.is_rusttype => None,
            TypeInfo::Newtype(newtype) => self.newtype_derive(name, newtype, derive, visiting),
            TypeInfo::Builtin | TypeInfo::TypeAlias => None,
        };
        match own {
            Some(true) => self.all_derive_support(args, derive, assumed, visiting),
            Some(false) => DeriveSupport::Missing(ty.clone()),
            None => DeriveSupport::Unknown,
        }
    }

    /// Whether a `model`, `class` or `enum` implements `derive`: `Clone` and `Debug` always (`opaque_debug` marks the
    /// one case lowering drops `Debug`), `Eq` and `Hash` only when declared.
    fn automatic_nominal_derive(
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

    /// Whether a declared type provides `derive` through its own derive list, trait adoptions and `@rust.derive`.
    ///
    /// A derive counts together with what it implies (`Ord` implies `Eq`), under its own name or an import alias, and a
    /// trait adoption counts when it names the builtin trait. A type declared in another module, or with a
    /// `@rust.derive(...)` the compiler cannot classify, answers `None` when the derive is absent from what is known.
    fn declared_derive(
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
                    .filter_map(|spelling| self.builtin_derive_bound(spelling)),
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
    fn builtin_derive_named(&self, spelling: &str) -> Option<DeriveId> {
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
    fn newtype_derive(
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

    /// Whether lowering gives a newtype `derive` automatically: `Clone` when its underlying type implements it (or is
    /// `Copy`), and `Debug` unless its underlying type is known to lack it.
    ///
    /// The newtype's own type parameters count as implementing the derive, since the derive bounds them.
    fn newtype_automatic_derive(
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
    fn surface_type_named(&self, name: &str) -> Option<SurfaceTypeId> {
        self.active_surface_type_import(name).or_else(|| {
            surface_types::from_str(name)
                .filter(|_| matches!(self.lookup_type_info(name), None | Some(TypeInfo::Builtin)))
        })
    }

    // ========================================================================
    // #1754: automatic derives of a field's type
    // ========================================================================

    /// Refuse one member whose type cannot satisfy the automatic `Clone` and `Debug` derives its declaration carries.
    ///
    /// `owner_kind` is the declaration keyword (`model`, `class`, `enum`) and `owner_name` its name. The refusal names
    /// the innermost type that lacks the derives, so `list[JoinHandle[int]]` points at `JoinHandle[int]`.
    pub(in crate::typechecker) fn refuse_member_without_automatic_derives(
        &mut self,
        owner_kind: &str,
        owner_name: &str,
        member: DerivedMember<'_>,
        member_ty: &ResolvedType,
        span: Span,
    ) {
        let Some((holder, missing)) = self.first_missing_derives(member_ty, &[DeriveId::Clone, DeriveId::Debug]) else {
            return;
        };
        self.errors.push(errors::member_type_lacks_automatic_derives(
            owner_kind,
            owner_name,
            member,
            &member_ty.to_string(),
            &holder.to_string(),
            &missing,
            span,
        ));
    }

    /// Find the first of `required` that `ty` is known to lack, and every one of `required` that type inside it lacks.
    fn first_missing_derives(
        &self,
        ty: &ResolvedType,
        required: &[DeriveId],
    ) -> Option<(ResolvedType, Vec<&'static str>)> {
        let holder = required
            .iter()
            .find_map(|derive| match self.derive_support(ty, *derive) {
                DeriveSupport::Missing(holder) => Some(holder),
                DeriveSupport::Supported | DeriveSupport::Unknown => None,
            })?;
        let missing = required
            .iter()
            .filter(|derive| matches!(self.derive_support(&holder, **derive), DeriveSupport::Missing(_)))
            .map(|derive| derives::as_str(*derive))
            .collect::<Vec<_>>();
        Some((holder, missing))
    }

    // ========================================================================
    // #1758: set elements and dict keys implement Eq and Hash
    // ========================================================================

    /// Refuse the set element and dict key types an annotation names that lack `Eq` or `Hash` (#1758).
    ///
    /// The annotation and its resolved type are walked together so each refusal points at the element or key type as
    /// written; the interop `HashMap[K, V]` keys like a dict. A spelling that no longer lines up with its resolved type
    /// (a type alias) is not descended into: the alias's own declaration is where its target is checked.
    pub(in crate::typechecker) fn refuse_unhashable_collection_keys(
        &mut self,
        annotation: &Spanned<Type>,
        resolved: &ResolvedType,
    ) {
        match (&annotation.node, resolved) {
            (Type::Generic(_, args), ResolvedType::Generic(name, resolved_args))
                if args.len() == resolved_args.len() =>
            {
                let role = match collection_type_id(name) {
                    Some(CollectionTypeId::Set) => Some(HashedCollectionRole::SetElement),
                    Some(CollectionTypeId::Dict) => Some(HashedCollectionRole::DictKey),
                    _ if self.surface_type_named(name) == Some(SurfaceTypeId::HashMap) => {
                        Some(HashedCollectionRole::DictKey)
                    }
                    _ => None,
                };
                if let (Some(role), Some(key), Some(key_ty)) = (role, args.first(), resolved_args.first()) {
                    self.refuse_unhashable_collection_member(role, key_ty, key.span);
                }
                for (arg, resolved_arg) in args.iter().zip(resolved_args) {
                    self.refuse_unhashable_collection_keys(arg, resolved_arg);
                }
            }
            (Type::Generic(_, args), ResolvedType::FrozenList(element) | ResolvedType::FrozenSet(element)) => {
                if let [arg] = args.as_slice() {
                    self.refuse_unhashable_collection_keys(arg, element);
                }
            }
            (Type::Generic(_, args), ResolvedType::FrozenDict(key, value)) => {
                if let [key_arg, value_arg] = args.as_slice() {
                    self.refuse_unhashable_collection_keys(key_arg, key);
                    self.refuse_unhashable_collection_keys(value_arg, value);
                }
            }
            (Type::Tuple(items), ResolvedType::Tuple(resolved_items)) if items.len() == resolved_items.len() => {
                for (item, resolved_item) in items.iter().zip(resolved_items) {
                    self.refuse_unhashable_collection_keys(item, resolved_item);
                }
            }
            (Type::Function(params, ret), ResolvedType::Function(resolved_params, resolved_ret))
                if params.len() == resolved_params.len() =>
            {
                for (param, resolved_param) in params.iter().zip(resolved_params) {
                    self.refuse_unhashable_collection_keys(param, &resolved_param.ty);
                }
                self.refuse_unhashable_collection_keys(ret, resolved_ret);
            }
            _ => {}
        }
    }

    /// Collect the names of `type_params` that appear in `ty`.
    pub(in crate::typechecker) fn collect_named_type_params(
        ty: &ResolvedType,
        type_params: &[String],
        out: &mut Vec<String>,
    ) {
        match ty {
            ResolvedType::Named(name) | ResolvedType::TypeVar(name) => {
                if type_params.contains(name) && !out.contains(name) {
                    out.push(name.clone());
                }
            }
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                for arg in args {
                    Self::collect_named_type_params(arg, type_params, out);
                }
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => Self::collect_named_type_params(inner, type_params, out),
            ResolvedType::FrozenDict(key, value) => {
                Self::collect_named_type_params(key, type_params, out);
                Self::collect_named_type_params(value, type_params, out);
            }
            _ => {}
        }
    }

    /// Refuse one set element or dict key type that lacks `Eq` or `Hash`, once per source location (#1758).
    pub(in crate::typechecker) fn refuse_unhashable_collection_member(
        &mut self,
        role: HashedCollectionRole,
        member_ty: &ResolvedType,
        span: Span,
    ) {
        let Some((holder, missing)) = self.first_missing_derives(member_ty, &[DeriveId::Eq, DeriveId::Hash]) else {
            return;
        };
        let error = errors::collection_member_lacks_hash_derives(
            role,
            &member_ty.to_string(),
            &holder.to_string(),
            &missing,
            self.hash_remedy(&holder),
            span,
        );
        self.push_error_once(error);
    }

    /// Whether a destination annotation's element or key type lacks `Eq` or `Hash`, so its annotation was refused.
    ///
    /// A literal checked against such a destination is not refused again for the same element or key type; one
    /// checked against a type parameter or a hashable type still is, for its own elements.
    pub(in crate::typechecker) fn annotation_reports_unhashable_member(
        &self,
        expected_member: Option<&ResolvedType>,
    ) -> bool {
        expected_member.is_some_and(|ty| {
            self.first_missing_derives(ty, &[DeriveId::Eq, DeriveId::Hash])
                .is_some()
        })
    }

    /// Refuse a generic call whose type argument lacks `Eq` or `Hash` where the callee hashes that type parameter
    /// (#1758).
    ///
    /// The callee's hashed parameters are part of its signature, inferred when it was collected
    /// ([`Self::infer_hash_key_type_params`]), so this runs at the call whatever the declaration order, for functions
    /// and methods of this module and of source modules it imports; a compiled library's callee carries them as `Eq`
    /// and `Hash` bounds instead, which the ordinary bound check applies. `bindings` are the call's bindings closed by
    /// its literal arguments ([`Self::bindings_closed_by_literal_arguments`]). A binding to the caller's own type
    /// parameter is not refused: the caller's signature carries the requirement on to its callers.
    pub(in crate::typechecker) fn refuse_unhashable_type_arguments(
        &mut self,
        callee_name: &str,
        callee: Option<&CanonicalSymbolId>,
        type_params: &[String],
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        let Some(required) = callee
            .and_then(|identity| self.hash_key_type_params.get(identity))
            .cloned()
        else {
            return;
        };
        for type_param in required.iter().filter(|type_param| type_params.contains(type_param)) {
            let Some(bound) = bindings
                .get(type_param)
                .filter(|bound| !Self::is_open_binding(bound, type_params))
            else {
                continue;
            };
            self.refuse_unhashable_type_argument(callee_name, type_param, bound, span);
        }
    }

    /// Refuse one type argument bound to a hashed type parameter when it is known to lack `Eq` or `Hash` (#1758).
    ///
    /// Shared by the callees whose requirement is inferred in this checker and by those whose signature carries it as
    /// inferred `Eq` and `Hash` bounds (a compiled library's): either way an unknown answer admits the call.
    pub(in crate::typechecker) fn refuse_unhashable_type_argument(
        &mut self,
        callee_name: &str,
        type_param: &str,
        argument: &ResolvedType,
        span: Span,
    ) {
        let Some((holder, missing)) = self.first_missing_derives(argument, &[DeriveId::Eq, DeriveId::Hash]) else {
            return;
        };
        let error = errors::type_argument_lacks_hash_derives(
            callee_name,
            type_param,
            &argument.to_string(),
            &holder.to_string(),
            &missing,
            self.hash_remedy(&holder),
            span,
        );
        self.push_error_once(error);
    }

    /// Return a generic call's type-parameter bindings with each open one closed from a literal argument, for the
    /// checks of what the call instantiates its callee with.
    ///
    /// A literal argument checked against a parameter type that names a type parameter takes that placeholder as its
    /// own type (`[Tag.A]` against `list[T]` is a `list[T]`), so the binding stays open; the literal's elements say
    /// what the call instantiates the parameter with (`Tag`). A binding that is already closed is kept.
    pub(in crate::typechecker) fn bindings_closed_by_literal_arguments(
        &self,
        type_params: &[String],
        params: &[CallableParam],
        args: &[CallArg],
        bindings: &HashMap<String, ResolvedType>,
    ) -> HashMap<String, ResolvedType> {
        let mut literal_bindings = HashMap::new();
        for (expr, param) in Self::arguments_with_parameters(params, args) {
            if let Some(param) = param {
                self.literal_argument_bindings(&param.ty, expr, type_params, &mut literal_bindings);
            }
        }
        let mut closed = bindings.clone();
        for (type_param, ty) in literal_bindings {
            let open = closed
                .get(&type_param)
                .is_none_or(|bound| Self::is_open_binding(bound, type_params));
            if open {
                closed.insert(type_param, ty);
            }
        }
        closed
    }

    /// Return the declaration identity of the function a call by name reaches, unless it is an overload set, whose
    /// bindings belong to one overload.
    pub(in crate::typechecker) fn called_function_identity(&self, callee_name: &str) -> Option<CanonicalSymbolId> {
        let symbol_id = self.symbols.lookup(callee_name)?;
        match &self.symbols.get(symbol_id)?.kind {
            SymbolKind::Function(_) => self.symbols.identity_of(symbol_id).cloned(),
            _ => None,
        }
    }

    /// Whether a binding is still open: unresolved, or one of the callee's own type parameters.
    fn is_open_binding(bound: &ResolvedType, type_params: &[String]) -> bool {
        match bound {
            ResolvedType::Unknown | ResolvedType::CallSiteInfer => true,
            ResolvedType::TypeVar(name) | ResolvedType::Named(name) => type_params.contains(name),
            _ => false,
        }
    }

    /// Pair each argument with the parameter it binds: positional arguments in order over the ordinary parameters a
    /// call can bind positionally, named arguments by name. An unpacked argument ends the pairing, since it hides which
    /// parameters the arguments after it bind.
    pub(in crate::typechecker) fn arguments_with_parameters<'a, 'p>(
        params: &'p [CallableParam],
        args: &'a [CallArg],
    ) -> Vec<(&'a Spanned<Expr>, Option<&'p CallableParam>)> {
        let mut positional_params = params
            .iter()
            .filter(|param| param.kind == ParamKind::Normal && !param.is_partial_preset);
        let mut paired = Vec::new();
        for arg in args {
            match arg {
                CallArg::Positional(expr) => paired.push((expr, positional_params.next())),
                CallArg::Named(name, expr) => paired.push((
                    expr,
                    params.iter().find(|param| param.name() == Some(name.node.as_str())),
                )),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => break,
            }
        }
        paired
    }

    /// Recover type-parameter bindings from a literal argument's elements, following the parameter type's shape.
    ///
    /// Only a collection or tuple literal is read: any other argument's type is already what inference bound.
    fn literal_argument_bindings(
        &self,
        param_ty: &ResolvedType,
        expr: &Spanned<Expr>,
        type_params: &[String],
        out: &mut HashMap<String, ResolvedType>,
    ) {
        match (param_ty, &expr.node) {
            (_, Expr::Paren(inner)) => self.literal_argument_bindings(param_ty, inner, type_params, out),
            (ResolvedType::Generic(name, args), Expr::List(entries))
                if collection_type_id(name) == Some(CollectionTypeId::List) =>
            {
                if let (Some(element_ty), Some(ListEntry::Element(first))) = (args.first(), entries.first()) {
                    self.literal_element_bindings(element_ty, first, type_params, out);
                }
            }
            (ResolvedType::Generic(name, args), Expr::Set(elements))
                if collection_type_id(name) == Some(CollectionTypeId::Set) =>
            {
                if let (Some(element_ty), Some(first)) = (args.first(), elements.first()) {
                    self.literal_element_bindings(element_ty, first, type_params, out);
                }
            }
            (ResolvedType::Generic(name, args), Expr::Dict(entries))
                if collection_type_id(name) == Some(CollectionTypeId::Dict) =>
            {
                if let ([key_ty, value_ty], Some(DictEntry::Pair(key, value))) = (args.as_slice(), entries.first()) {
                    self.literal_element_bindings(key_ty, key, type_params, out);
                    self.literal_element_bindings(value_ty, value, type_params, out);
                }
            }
            (ResolvedType::Tuple(items), Expr::Tuple(elements)) if items.len() == elements.len() => {
                for (item_ty, element) in items.iter().zip(elements) {
                    self.literal_element_bindings(item_ty, element, type_params, out);
                }
            }
            _ => {}
        }
    }

    /// Recover a binding from one element of a literal: its own type where the element type is a type parameter, or
    /// the bindings of a nested literal.
    fn literal_element_bindings(
        &self,
        element_ty: &ResolvedType,
        element: &Spanned<Expr>,
        type_params: &[String],
        out: &mut HashMap<String, ResolvedType>,
    ) {
        match element_ty {
            ResolvedType::TypeVar(name) | ResolvedType::Named(name) if type_params.contains(name) => {
                if let Some(ty) = self.type_info.expr_type(element.span)
                    && !Self::is_open_binding(ty, type_params)
                {
                    out.entry(name.clone()).or_insert_with(|| ty.clone());
                }
            }
            _ => self.literal_argument_bindings(element_ty, element, type_params, out),
        }
    }

    /// Choose the remedy for the type that lacks `Eq` or `Hash`: a declared type adds derives, unless it defines
    /// `__eq__`, whose custom equality provides neither; a builtin cannot take derives.
    fn hash_remedy(&self, ty: &ResolvedType) -> HashRemedy {
        let (ResolvedType::Named(name) | ResolvedType::Generic(name, _)) = ty else {
            return HashRemedy::Builtin;
        };
        let methods = match self.lookup_semantic_type_info(name) {
            Some(TypeInfo::Model(info)) => &info.methods,
            Some(TypeInfo::Class(info)) => &info.methods,
            Some(TypeInfo::Enum(info)) => &info.methods,
            Some(TypeInfo::Newtype(info)) => &info.methods,
            Some(TypeInfo::Builtin | TypeInfo::TypeAlias) | None => return HashRemedy::Builtin,
        };
        if methods.contains_key("__eq__") {
            HashRemedy::CustomEquality
        } else {
            HashRemedy::AddDerives
        }
    }

    /// Push an error unless the same message was already reported at the same span.
    ///
    /// An annotation or expression can be checked more than once while its declaration is checked (a field and its
    /// default, for example); the refusal belongs to the source location, not to each pass over it.
    fn push_error_once(&mut self, error: CompileError) {
        if !self
            .errors
            .iter()
            .any(|existing| existing.span == error.span && existing.message == error.message)
        {
            self.errors.push(error);
        }
    }
}
