//! Derives a declaration's generated form requires of the types it holds.
//!
//! A `model`, `class` or `enum` always derives `Clone` and `Debug` (the language reference's *Automatic derives*), and
//! a derive holds only when every field type supports it. A field holding a runtime type that implements neither, such
//! as a `JoinHandle[T]`, therefore passed the checker and failed the generated program's build (#1754). The checker
//! refuses that field at the declaration instead, naming the field, its type and the derives it lacks. Which runtime
//! types lack the derives is registry data ([`surface_types::automatic_derive_support`]); a type the registry makes
//! no claim about is left to the build rather than guessed at.
//!
//! A set's elements and a dict's keys are compared and hashed, so their type must implement `Eq` and `Hash`. A
//! source-declared type implements them only through `@derive(Eq, Hash)` (or `Ord` for `Eq`), and an enum with no
//! derives passed the checker as a set element or dict key and failed the build (#1758). The checker refuses the
//! element or key type where it is written, naming the derives to add.

use super::TypeChecker;
use crate::ast::{Span, Spanned, Type};
use crate::diagnostics::errors::{self, DerivedMember, HashedCollectionRole};
use crate::symbols::{ResolvedType, TypeInfo};
use crate::typechecker::helpers::collection_type_id;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::surface::types::{self as surface_types, AutomaticDeriveSupport, SurfaceTypeId};
use incan_lang::lang::types::collections::CollectionTypeId;

impl TypeChecker {
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
        let Some((holder, missing)) = self.type_missing_automatic_derives(member_ty) else {
            return;
        };
        let missing = missing.iter().map(|id| derives::as_str(*id)).collect::<Vec<_>>();
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

    /// Find the first type inside `ty` whose runtime realization lacks the automatic derives, with the derives it
    /// lacks.
    ///
    /// Builtin collections, `Option`, `Result`, tuples and the interop `Vec` / `HashMap` implement a derive only when
    /// their element types do, so the walk descends into them; a source-declared generic `model`, `class`, `enum` or
    /// newtype derives with a bound on every type parameter, so it descends into those arguments too. It never
    /// descends into a function type (a callable value's own capabilities do not depend on its signature), a frozen
    /// collection, a Rust-origin type or a runtime handle the registry says implements the derives (`Mutex[T]` does
    /// whatever `T` is): the registry is the only authority on a runtime type, and a type it makes no claim about is
    /// left to the build.
    fn type_missing_automatic_derives<'t>(
        &self,
        ty: &'t ResolvedType,
    ) -> Option<(&'t ResolvedType, &'static [DeriveId])> {
        match ty {
            ResolvedType::Named(name) => match self
                .surface_type_named(name)
                .map(surface_types::automatic_derive_support)
            {
                Some(AutomaticDeriveSupport::Missing(missing)) => Some((ty, missing)),
                _ => None,
            },
            ResolvedType::Generic(name, args) => {
                if let Some(collection) = collection_type_id(name) {
                    if collection == CollectionTypeId::Generator {
                        return None;
                    }
                    return self.first_type_missing_automatic_derives(args);
                }
                if let Some(surface_type) = self.surface_type_named(name) {
                    return match surface_types::automatic_derive_support(surface_type) {
                        AutomaticDeriveSupport::Missing(missing) => Some((ty, missing)),
                        AutomaticDeriveSupport::FollowsTypeArguments => self.first_type_missing_automatic_derives(args),
                        AutomaticDeriveSupport::Implements => None,
                    };
                }
                if self.declared_type_derives_over_its_arguments(name) {
                    return self.first_type_missing_automatic_derives(args);
                }
                None
            }
            ResolvedType::Tuple(items) => self.first_type_missing_automatic_derives(items),
            _ => None,
        }
    }

    /// Return the first of `types` that holds a type lacking the automatic derives.
    fn first_type_missing_automatic_derives<'t>(
        &self,
        types: &'t [ResolvedType],
    ) -> Option<(&'t ResolvedType, &'static [DeriveId])> {
        types.iter().find_map(|ty| self.type_missing_automatic_derives(ty))
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

    /// Return whether a source-declared generic type implements its derives only for type arguments that do.
    ///
    /// A derive on a generic `model`, `class`, `enum` or newtype bounds every type parameter by the derived trait, so
    /// the walks above descend into such a type's arguments. A `rusttype` newtype is realized by its Rust type, whose
    /// implementations the checker does not know, so it is not descended into.
    fn declared_type_derives_over_its_arguments(&self, name: &str) -> bool {
        match self.lookup_semantic_type_info(name) {
            Some(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_)) => true,
            Some(TypeInfo::Newtype(info)) => !info.is_rusttype,
            Some(TypeInfo::Builtin | TypeInfo::TypeAlias) | None => false,
        }
    }

    /// Refuse the set element and dict key types an annotation names that lack `Eq` or `Hash` (#1758).
    ///
    /// The annotation and its resolved type are walked together so each refusal points at the element or key type as
    /// written. A spelling that no longer lines up with its resolved type (a type alias) is not descended into: the
    /// alias's own declaration is where its target is checked. `FrozenSet` and `FrozenDict` are baked slices rather
    /// than hashed tables, so their element and key types are not refused.
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
                    _ => None,
                };
                if let (Some(role), Some(key), Some(key_ty)) = (role, args.first(), resolved_args.first()) {
                    self.refuse_unhashable_collection_member(role, key_ty, key.span);
                }
                for (arg, resolved_arg) in args.iter().zip(resolved_args) {
                    self.refuse_unhashable_collection_keys(arg, resolved_arg);
                }
            }
            // A frozen collection is a baked slice with no hashing, so only its contents are walked.
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

    /// Refuse one set element or dict key type that lacks `Eq` or `Hash`, once per source location (#1758).
    ///
    /// An annotation can be resolved more than once while its declaration is checked (a field and its default, for
    /// example), so a refusal already recorded at the same span is not repeated.
    pub(in crate::typechecker) fn refuse_unhashable_collection_member(
        &mut self,
        role: HashedCollectionRole,
        member_ty: &ResolvedType,
        span: Span,
    ) {
        let Some((holder, missing)) = self.type_missing_hash_key_derives(member_ty) else {
            return;
        };
        let missing = missing.iter().map(|id| derives::as_str(*id)).collect::<Vec<_>>();
        let error = errors::collection_member_lacks_hash_derives(
            role,
            &member_ty.to_string(),
            &holder.to_string(),
            &missing,
            span,
        );
        if !self
            .errors
            .iter()
            .any(|existing| existing.span == error.span && existing.message == error.message)
        {
            self.errors.push(error);
        }
    }

    /// Find the type inside a set element or dict key type that lacks `Eq` or `Hash`, with the derives it lacks.
    ///
    /// Tuples, `list`, `Option` and `Result` implement `Eq` and `Hash` exactly when their contents do, so the walk
    /// descends into them, and into the arguments of a source-declared generic type that has both derives. Only a
    /// source-declared `model`, `class`, `enum` or newtype is ever refused, because its derives are all the compiler
    /// gives it; a builtin scalar, a type parameter, a Rust-origin type and a `rusttype` newtype are left to the build.
    fn type_missing_hash_key_derives<'t>(&self, ty: &'t ResolvedType) -> Option<(&'t ResolvedType, Vec<DeriveId>)> {
        match ty {
            ResolvedType::Tuple(items) => items.iter().find_map(|item| self.type_missing_hash_key_derives(item)),
            ResolvedType::Named(name) => self
                .declared_type_missing_hash_key_derives(name)
                .map(|missing| (ty, missing)),
            ResolvedType::Generic(name, args) => {
                if let Some(collection) = collection_type_id(name) {
                    if !matches!(
                        collection,
                        CollectionTypeId::List
                            | CollectionTypeId::Option
                            | CollectionTypeId::Result
                            | CollectionTypeId::Tuple
                    ) {
                        return None;
                    }
                    return args.iter().find_map(|arg| self.type_missing_hash_key_derives(arg));
                }
                if let Some(missing) = self.declared_type_missing_hash_key_derives(name) {
                    return Some((ty, missing));
                }
                if self.declared_type_derives_over_its_arguments(name) {
                    return args.iter().find_map(|arg| self.type_missing_hash_key_derives(arg));
                }
                None
            }
            _ => None,
        }
    }

    /// Return the derives among `Eq` and `Hash` a source-declared nominal type lacks, or `None` when it lacks neither
    /// or the compiler cannot tell.
    ///
    /// `Eq` is also satisfied by `Ord`, which implies it, and either derive by a `with Eq` / `with Hash` adoption,
    /// under its own name or an import alias. A type with an `__eq__` or `__hash__` method, or with a
    /// `@rust.derive(...)` list, is left to the build: its equality comes from a place the derive list does not
    /// show.
    fn declared_type_missing_hash_key_derives(&self, name: &str) -> Option<Vec<DeriveId>> {
        if self.local_rust_derive_paths.contains_key(name) {
            return None;
        }
        let (derive_names, trait_names, methods) = match self.lookup_semantic_type_info(name)? {
            TypeInfo::Model(info) => (&info.derives, &info.traits, &info.methods),
            TypeInfo::Class(info) => (&info.derives, &info.traits, &info.methods),
            TypeInfo::Enum(info) => (&info.derives, &info.traits, &info.methods),
            TypeInfo::Newtype(info) if !info.is_rusttype => (&info.derives, &info.traits, &info.methods),
            TypeInfo::Newtype(_) | TypeInfo::Builtin | TypeInfo::TypeAlias => return None,
        };
        if methods.contains_key("__eq__") || methods.contains_key("__hash__") {
            return None;
        }
        // A derive or trait imported under another name (`from std.derives.comparison import Hash as Hashed`) counts
        // as the builtin it names.
        let builtin_derive = |name: &String| {
            derives::from_str(name).or_else(|| {
                self.import_binding_path(name)
                    .and_then(<[String]>::last)
                    .and_then(|leaf| derives::from_str(leaf))
            })
        };
        let provides = |satisfying: &[DeriveId]| {
            derive_names
                .iter()
                .chain(trait_names)
                .any(|name| builtin_derive(name).is_some_and(|id| satisfying.contains(&id)))
        };
        let mut missing = Vec::new();
        if !provides(&[DeriveId::Eq, DeriveId::Ord]) {
            missing.push(DeriveId::Eq);
        }
        if !provides(&[DeriveId::Hash]) {
            missing.push(DeriveId::Hash);
        }
        (!missing.is_empty()).then_some(missing)
    }
}
