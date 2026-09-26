//! Check collection literals (tuple, list, dict, and set).
//!
//! These helpers validate collection literal expressions and compute container element types using the current
//! checker's compatibility rules.

use crate::ast::*;
use crate::diagnostics::errors;
use crate::symbols::{ResolvedType, TypeInfo};
use crate::typechecker::helpers::{collection_type_id, dict_ty, list_ty, set_ty};
use incan_lang::lang::types::collections::CollectionTypeId;

use super::TypeChecker;

impl TypeChecker {
    /// Return the type a collection literal builds for the destination type `expected`, looking through the `Option`
    /// and union wrappers the destination puts around it; `is_kind` accepts the literal's own kind of type.
    ///
    /// `Option[list[int]]` gives `list[int]`: an `[]` assigned to it is a `list[int]`, which the destination wraps in
    /// `Some`. A union gives its one member of the literal's kind when every other member is a type no collection
    /// literal of another kind builds (see [`Self::never_built_by_other_literal`]), so `list[int] | str` gives
    /// `list[int]`. A union with two members of the kind (`list[int] | list[str]`), or with a member a literal might
    /// build (a type parameter, a newtype, a trait), gives nothing, and the literal is checked on its own.
    fn collection_literal_destination<'ty>(
        &self,
        expected: &'ty ResolvedType,
        is_kind: &dyn Fn(&ResolvedType) -> bool,
    ) -> Option<&'ty ResolvedType> {
        if is_kind(expected) {
            return Some(expected);
        }
        if let Some(inner) = expected.option_inner_type() {
            return self.collection_literal_destination(inner, is_kind);
        }
        let mut destination = None;
        for member in expected.union_members()? {
            match self.collection_literal_destination(member, is_kind) {
                Some(found) if destination.is_none() => destination = Some(found),
                Some(_) => return None,
                None if self.never_built_by_other_literal(member) => {}
                None => return None,
            }
        }
        destination
    }

    /// Return whether no value of `ty` is built by a collection literal of a kind `ty` is not: a scalar, a tuple, a
    /// builtin list, dict, set, `Result` or generator, a model, class or enum, or an `Option` or union of such types.
    ///
    /// A type parameter, a newtype (which may accept its underlying collection), a trait, a frozen collection and an
    /// unknown type may be built by such a literal, so they answer `false`.
    fn never_built_by_other_literal(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::Tuple(_) => true,
            ResolvedType::Named(name) => self.names_model_class_or_enum(name),
            ResolvedType::Generic(name, args) => match collection_type_id(name.as_str()) {
                Some(CollectionTypeId::Option) => args.iter().all(|arg| self.never_built_by_other_literal(arg)),
                Some(
                    CollectionTypeId::List
                    | CollectionTypeId::Dict
                    | CollectionTypeId::Set
                    | CollectionTypeId::Tuple
                    | CollectionTypeId::Result
                    | CollectionTypeId::Generator,
                ) => true,
                Some(CollectionTypeId::FrozenList | CollectionTypeId::FrozenDict | CollectionTypeId::FrozenSet) => {
                    false
                }
                None if ty.is_union() => args.iter().all(|arg| self.never_built_by_other_literal(arg)),
                None => self.names_model_class_or_enum(name),
            },
            _ => false,
        }
    }

    /// Return whether `name` resolves to a model, class or enum declaration.
    fn names_model_class_or_enum(&self, name: &str) -> bool {
        matches!(
            self.lookup_type_info(name),
            Some(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_))
        )
    }

    /// Return the element type of a `list[T]` type.
    fn list_element_type(ty: &ResolvedType) -> Option<&ResolvedType> {
        match ty {
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) && args.len() == 1 =>
            {
                args.first()
            }
            _ => None,
        }
    }

    /// Return the key and value types of a `dict[K, V]` type.
    fn dict_entry_types(ty: &ResolvedType) -> Option<(&ResolvedType, &ResolvedType)> {
        match ty {
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict) && args.len() == 2 =>
            {
                Some((&args[0], &args[1]))
            }
            _ => None,
        }
    }

    /// Return the element types of a tuple type, spelled either `tuple[...]` or as a tuple literal's own type.
    fn tuple_element_types(ty: &ResolvedType) -> Option<&[ResolvedType]> {
        match ty {
            ResolvedType::Tuple(items) => Some(items.as_slice()),
            ResolvedType::Generic(name, items)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                Some(items.as_slice())
            }
            _ => None,
        }
    }

    /// Extract the element type from the list type an expected destination gives a list literal, if one is known.
    ///
    /// The destination may wrap the list in `Option` or a union (see [`Self::collection_literal_destination`]), so
    /// `[]` and `[None]` take the element type of an `Option[list[...]]` or `list[...] | str` destination too.
    fn list_expected_element_type(&self, expected: Option<&ResolvedType>) -> Option<ResolvedType> {
        let destination =
            self.collection_literal_destination(expected?, &|ty| Self::list_element_type(ty).is_some())?;
        Self::list_element_type(destination).cloned()
    }

    /// Extract key/value types from the dict type an expected destination gives a dict literal, if one is known.
    ///
    /// The destination may wrap the dict in `Option` or a union, as for a list literal.
    fn dict_expected_entry_types(
        &self,
        expected: Option<&ResolvedType>,
    ) -> (Option<ResolvedType>, Option<ResolvedType>) {
        let destination = expected.and_then(|expected| {
            self.collection_literal_destination(expected, &|ty| Self::dict_entry_types(ty).is_some())
        });
        match destination.and_then(Self::dict_entry_types) {
            Some((key, value)) => (Some(key.clone()), Some(value.clone())),
            None => (None, None),
        }
    }

    /// Extract the element type from a statically known list spread operand.
    fn list_spread_element_type(ty: &ResolvedType) -> Option<ResolvedType> {
        Self::list_element_type(ty).cloned()
    }

    /// Extract key/value types from a statically known dict spread operand.
    fn dict_spread_entry_types(ty: &ResolvedType) -> Option<(ResolvedType, ResolvedType)> {
        Self::dict_entry_types(ty).map(|(key, value)| (key.clone(), value.clone()))
    }

    /// Merge one observed collection member type into the literal's candidate member type.
    fn merge_collection_member_type(&mut self, member_ty: &mut ResolvedType, value_ty: ResolvedType, span: Span) {
        if matches!(member_ty, ResolvedType::Unknown) {
            *member_ty = value_ty;
            return;
        }

        if self.types_compatible(&value_ty, member_ty) {
            return;
        }

        if self.types_compatible(member_ty, &value_ty) {
            *member_ty = value_ty;
            return;
        }

        self.errors.push(errors::type_mismatch(
            &member_ty.to_string(),
            &value_ty.to_string(),
            span,
        ));
    }

    /// Validate a collection member against a contextual element type, or merge it into the inferred member type.
    fn check_collection_member_type(
        &mut self,
        hinted_ty: Option<&ResolvedType>,
        member_ty: &mut ResolvedType,
        value_ty: ResolvedType,
        span: Span,
    ) {
        if let Some(expected) = hinted_ty {
            if !self.types_compatible(&value_ty, expected) {
                self.errors.push(errors::type_mismatch(
                    &expected.to_string(),
                    &value_ty.to_string(),
                    span,
                ));
            }
            return;
        }
        self.merge_collection_member_type(member_ty, value_ty, span);
    }

    /// Refine only list element holes originating in the first member's empty literal.
    ///
    /// Compatibility alone accepts `list[Unknown]` against `list[str]`, but retaining that hole loses the owned
    /// string representation downstream. The source witness prevents peer observations from refining unrelated
    /// unknown call results or nominal generic arguments. Existing concrete leaves are never widened here.
    fn refine_empty_list_member(member: &mut ResolvedType, observed: &ResolvedType, seed: &Expr) {
        let Expr::List(entries) = seed else {
            return;
        };
        let (ResolvedType::Generic(name, args), ResolvedType::Generic(other_name, other_args)) = (member, observed)
        else {
            return;
        };
        if collection_type_id(name) != Some(CollectionTypeId::List)
            || collection_type_id(other_name) != Some(CollectionTypeId::List)
            || args.len() != 1
            || other_args.len() != 1
        {
            return;
        }
        if entries.is_empty() {
            if matches!(args[0], ResolvedType::Unknown) {
                args[0] = other_args[0].clone();
            }
        } else if let Some(ListEntry::Element(first)) = entries.first() {
            Self::refine_empty_list_member(&mut args[0], &other_args[0], &first.node);
        }
    }

    /// Type-check a list literal with an optional destination-type hint.
    ///
    /// When a surrounding context already expects `List[T]`, empty lists adopt `T` directly and non-empty lists
    /// validate each element against that hinted type. Without a hint, the first element still seeds the candidate
    /// element type, but later elements must remain compatible instead of being ignored.
    pub(in crate::typechecker::check_expr) fn check_list_with_expected(
        &mut self,
        elems: &[ListEntry],
        expected: Option<&ResolvedType>,
    ) -> ResolvedType {
        let hinted_elem_ty = self.list_expected_element_type(expected);
        let mut elem_ty = hinted_elem_ty.clone().unwrap_or(ResolvedType::Unknown);
        let first_member = match elems.first() {
            Some(ListEntry::Element(value)) => Some(&value.node),
            _ => None,
        };

        for elem in elems {
            match elem {
                ListEntry::Element(value) => {
                    let value_ty = self.check_expr_with_expected(value, hinted_elem_ty.as_ref());
                    if hinted_elem_ty.is_none()
                        && let Some(seed) = first_member
                        && self.types_compatible(&value_ty, &elem_ty)
                    {
                        Self::refine_empty_list_member(&mut elem_ty, &value_ty, seed);
                    }
                    self.check_collection_member_type(hinted_elem_ty.as_ref(), &mut elem_ty, value_ty, value.span);
                }
                ListEntry::Spread(value) => {
                    let expected_spread = hinted_elem_ty.clone().map(list_ty);
                    let spread_ty = self.check_expr_with_expected(value, expected_spread.as_ref());
                    if let ResolvedType::Tuple(item_types) = spread_ty {
                        for item_ty in item_types {
                            self.check_collection_member_type(
                                hinted_elem_ty.as_ref(),
                                &mut elem_ty,
                                item_ty,
                                value.span,
                            );
                        }
                    } else if let ResolvedType::Generic(name, item_types) = &spread_ty
                        && collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple)
                    {
                        for item_ty in item_types {
                            self.check_collection_member_type(
                                hinted_elem_ty.as_ref(),
                                &mut elem_ty,
                                item_ty.clone(),
                                value.span,
                            );
                        }
                    } else if let Some(value_ty) = Self::list_spread_element_type(&spread_ty) {
                        self.check_collection_member_type(hinted_elem_ty.as_ref(), &mut elem_ty, value_ty, value.span);
                    } else {
                        self.errors.push(errors::type_mismatch(
                            "List[_] or tuple[...]",
                            &spread_ty.to_string(),
                            value.span,
                        ));
                    }
                }
            }
        }

        list_ty(elem_ty)
    }

    /// Type-check a tuple literal.
    pub(in crate::typechecker::check_expr) fn check_tuple(&mut self, elems: &[Spanned<Expr>]) -> ResolvedType {
        let elem_types: Vec<_> = elems.iter().map(|e| self.check_expr(e)).collect();
        ResolvedType::Tuple(elem_types)
    }

    /// Type-check a tuple literal against the tuple type its destination expects.
    ///
    /// The destination's tuple type of the literal's length (found through `Option` and union wrappers, see
    /// [`Self::collection_literal_destination`]) gives each element its expected type, as a list literal's element
    /// type does, so `None`, `Ok(...)`, `Err(...)`, an empty collection and an integer literal in a float slot are
    /// checked against it. Each element of the literal's type is the expected element type when the element is
    /// compatible with it and that type is fully known, which is what `(None, 1)` needs to be an `Option[str]` in a
    /// `tuple[Option[str], int]`; otherwise it is the element's own type, and the destination's own check reports a
    /// mismatch. Without a tuple destination of that length the literal is checked on its own.
    pub(in crate::typechecker::check_expr) fn check_tuple_with_expected(
        &mut self,
        elems: &[Spanned<Expr>],
        expected: &ResolvedType,
    ) -> ResolvedType {
        let arity = elems.len();
        let Some(expected_elems) = self
            .collection_literal_destination(expected, &|ty| {
                Self::tuple_element_types(ty).is_some_and(|items| items.len() == arity)
            })
            .and_then(Self::tuple_element_types)
            .map(<[ResolvedType]>::to_vec)
        else {
            return self.check_tuple(elems);
        };
        let elem_types = elems
            .iter()
            .zip(expected_elems)
            .map(|(elem, expected_elem)| {
                let elem_ty = self.check_expr_with_expected(elem, Some(&expected_elem));
                if self.is_fully_known_type(&expected_elem) && self.types_compatible(&elem_ty, &expected_elem) {
                    expected_elem
                } else {
                    elem_ty
                }
            })
            .collect();
        ResolvedType::Tuple(elem_types)
    }

    /// Type-check a list literal.
    pub(in crate::typechecker::check_expr) fn check_list(&mut self, elems: &[ListEntry]) -> ResolvedType {
        self.check_list_with_expected(elems, None)
    }

    /// Type-check a dict literal.
    pub(in crate::typechecker::check_expr) fn check_dict(&mut self, entries: &[DictEntry]) -> ResolvedType {
        self.check_dict_with_expected(entries, None)
    }

    /// Type-check a dict literal with an optional destination-type hint.
    pub(in crate::typechecker::check_expr) fn check_dict_with_expected(
        &mut self,
        entries: &[DictEntry],
        expected: Option<&ResolvedType>,
    ) -> ResolvedType {
        let (hinted_key_ty, hinted_value_ty) = self.dict_expected_entry_types(expected);
        let mut key_ty = hinted_key_ty.clone().unwrap_or(ResolvedType::Unknown);
        let mut val_ty = hinted_value_ty.clone().unwrap_or(ResolvedType::Unknown);

        for entry in entries {
            match entry {
                DictEntry::Pair(key, value) => {
                    let observed_key_ty = self.check_expr_with_expected(key, hinted_key_ty.as_ref());
                    let observed_value_ty = self.check_expr_with_expected(value, hinted_value_ty.as_ref());
                    self.check_collection_member_type(hinted_key_ty.as_ref(), &mut key_ty, observed_key_ty, key.span);
                    self.check_collection_member_type(
                        hinted_value_ty.as_ref(),
                        &mut val_ty,
                        observed_value_ty,
                        value.span,
                    );
                }
                DictEntry::Spread(value) => {
                    let expected_spread = match (hinted_key_ty.clone(), hinted_value_ty.clone()) {
                        (Some(key), Some(value)) => Some(dict_ty(key, value)),
                        _ => None,
                    };
                    let spread_ty = self.check_expr_with_expected(value, expected_spread.as_ref());
                    let Some((observed_key_ty, observed_value_ty)) = Self::dict_spread_entry_types(&spread_ty) else {
                        self.errors
                            .push(errors::type_mismatch("Dict[_, _]", &spread_ty.to_string(), value.span));
                        continue;
                    };
                    self.check_collection_member_type(hinted_key_ty.as_ref(), &mut key_ty, observed_key_ty, value.span);
                    self.check_collection_member_type(
                        hinted_value_ty.as_ref(),
                        &mut val_ty,
                        observed_value_ty,
                        value.span,
                    );
                }
            }
        }

        dict_ty(key_ty, val_ty)
    }

    /// Type-check a set literal.
    pub(in crate::typechecker::check_expr) fn check_set(&mut self, elems: &[Spanned<Expr>]) -> ResolvedType {
        let elem_ty = if let Some(first) = elems.first() {
            self.check_expr(first)
        } else {
            ResolvedType::Unknown
        };

        for elem in elems.iter().skip(1) {
            self.check_expr(elem);
        }

        set_ty(elem_ty)
    }

    /// Return whether `ty` has no part left for inference to fill: no unknown type, type parameter, `Self` or call-site
    /// `_`.
    ///
    /// A tuple literal takes an expected element type only when it is fully known, so a `tuple[T, int]` destination
    /// does not turn an element's own type into the placeholder `T`.
    fn is_fully_known_type(&self, ty: &ResolvedType) -> bool {
        if self.is_generic_placeholder_type(ty) {
            return false;
        }
        match ty {
            ResolvedType::Unknown | ResolvedType::CallSiteInfer | ResolvedType::SelfType => false,
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                args.iter().all(|arg| self.is_fully_known_type(arg))
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => self.is_fully_known_type(inner),
            ResolvedType::FrozenDict(key, value) => self.is_fully_known_type(key) && self.is_fully_known_type(value),
            ResolvedType::Function(params, ret) => {
                params.iter().all(|param| self.is_fully_known_type(&param.ty)) && self.is_fully_known_type(ret)
            }
            _ => true,
        }
    }
}
