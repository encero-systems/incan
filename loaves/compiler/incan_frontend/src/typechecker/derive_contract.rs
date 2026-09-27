//! Declaration checks of the derive contract (#1871).
//!
//! A `@derive(...)` on a `model`, `class`, `enum` or newtype passes to the generated program as the matching Rust
//! derive, which holds only when the declaration meets what the derive needs of it. These checks refuse at check time,
//! with `INCAN-T0001`, the declarations whose build used to fail:
//!
//! - a derive whose trait a member type does not implement, as the derive relation answers it: `@derive(Copy)` over a
//!   `str` field, `@derive(Eq)` over a `float` field, `@derive(Default)` over a field that declares no default and
//!   whose type has none; a derive counts together with what it implies, so `@derive(Ord)` needs `Eq` of its members
//!   too;
//! - `@derive(PartialOrd)` on a type that does not provide `PartialEq`;
//! - a derive on a declaration kind it does not apply to: `Validate` off a model, `Default` on an enum;
//! - a comparison dunder or `__str__` whose declaration does not have the signature its operator or builtin calls it
//!   with.
//!
//! A type that defines a dunder is also refused when it derives what provides the same behavior (#1872): `__str__`
//! with `Display`, `__eq__` or `__ne__` with `Eq` or `PartialEq`, and an ordering dunder with `Ord` or `PartialOrd`,
//! each also through a derive that implies it or through `@rust.derive(...)`. The generated program would implement
//! the trait twice, or give the dunder's operator a different meaning from the derived ones.
//!
//! The automatic `Clone` and `Debug` of a model, class or enum are refused separately (`INCAN-T0113`), and a member
//! type the relation cannot decide is never refused.

use std::collections::HashMap;

use super::TypeChecker;
use super::collect::decorators::{decorators_named, positional_derive_names};
use super::derive_requirements::DeriveSupport;
use crate::ast::{Decorator, MethodDecl, Receiver, Span, Spanned};
use crate::diagnostics::errors::{self, DerivedMember};
use crate::symbols::{MethodInfo, ResolvedType, TypeInfo};
use incan_lang::lang::decorators::DecoratorId;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::keywords::{self, KeywordId};

/// The declaration kinds a derive is written on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::typechecker) enum DerivedKind {
    /// A `model` declaration.
    Model,
    /// A `class` declaration.
    Class,
    /// An `enum` declaration.
    Enum,
    /// A `newtype` declaration (not a `rusttype`).
    Newtype,
}

impl DerivedKind {
    /// The declaration keyword, as diagnostics name the kind.
    fn keyword(self) -> &'static str {
        keywords::as_str(match self {
            Self::Model => KeywordId::Model,
            Self::Class => KeywordId::Class,
            Self::Enum => KeywordId::Enum,
            Self::Newtype => KeywordId::Newtype,
        })
    }

    /// Whether the kind carries the automatic `Clone` and `Debug` derives whose member requirements `INCAN-T0113`
    /// already refuses.
    fn has_automatic_clone_and_debug(self) -> bool {
        !matches!(self, Self::Newtype)
    }
}

/// One member whose type a derive's trait must hold for: a field, an enum payload or a newtype's underlying type.
struct DerivedDeclarationMember<'a> {
    /// Which member it is, as diagnostics name it.
    member: DerivedMember<'a>,
    /// The member's checked type.
    ty: ResolvedType,
    /// Whether the member declares a default, which `Default` takes instead of the type's own.
    has_default: bool,
    span: Span,
}

/// A builtin derive as written in `@derive(...)`, with its spelling and where it is written.
struct WrittenDerive {
    /// The builtin derive the spelling names, through an import alias too.
    id: DeriveId,
    /// The derive as the source spells it, bare or module-qualified, for diagnostics.
    spelling: String,
    /// Where the derive is written.
    span: Span,
}

/// Each dunder that defines behavior a derive also provides, with those derives (#1872).
const DUNDER_DERIVE_CONFLICTS: &[(&str, &[DeriveId])] = &[
    ("__str__", &[DeriveId::Display]),
    ("__eq__", &[DeriveId::Eq, DeriveId::PartialEq]),
    ("__ne__", &[DeriveId::Eq, DeriveId::PartialEq]),
    ("__lt__", &[DeriveId::Ord, DeriveId::PartialOrd]),
    ("__le__", &[DeriveId::Ord, DeriveId::PartialOrd]),
    ("__gt__", &[DeriveId::Ord, DeriveId::PartialOrd]),
    ("__ge__", &[DeriveId::Ord, DeriveId::PartialOrd]),
];

/// The signature a dunder must have: its parameter count after `self` and its return type.
struct DunderSignature {
    /// Whether the dunder takes one `other: Self` parameter (a comparison) or none (`__str__`).
    takes_other: bool,
    /// The return type the operator or builtin reads: `bool` for a comparison, `str` for `__str__`.
    returns: ResolvedType,
    /// The signature as diagnostics spell it after the dunder's name.
    display: &'static str,
}

/// Return the signature the operator or builtin that calls `dunder` calls it with, when it is one the checker holds
/// to a signature.
fn dunder_signature(dunder: &str) -> Option<DunderSignature> {
    match dunder {
        "__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__" => Some(DunderSignature {
            takes_other: true,
            returns: ResolvedType::Bool,
            display: "(self, other: Self) -> bool",
        }),
        "__str__" => Some(DunderSignature {
            takes_other: false,
            returns: ResolvedType::Str,
            display: "(self) -> str",
        }),
        _ => None,
    }
}

impl TypeChecker {
    /// Check the derive contract of one declaration: its derives' member requirements, their applicability, and its
    /// dunder signatures (#1871).
    ///
    /// `name` is the declared type, whose checked [`TypeInfo`] supplies the member types (inherited class fields
    /// included) and the methods; the AST supplies where each member and method is written. `member_spans` is keyed by
    /// field or variant name, and a newtype's underlying type by the empty name.
    pub(in crate::typechecker) fn check_derive_contract(
        &mut self,
        kind: DerivedKind,
        name: &str,
        decorators: &[Spanned<Decorator>],
        member_spans: &HashMap<&str, Span>,
        methods: &[Spanned<MethodDecl>],
    ) {
        let Some(info) = self.lookup_type_info(name).cloned() else {
            return;
        };
        let written = self.written_builtin_derives(decorators);
        let fallback_span = written.first().map_or_else(Span::default, |derive| derive.span);
        let members = Self::derived_declaration_members(&info, member_spans, fallback_span);
        let type_params = match &info {
            TypeInfo::Model(model) => model.type_params.clone(),
            TypeInfo::Class(class) => class.type_params.clone(),
            TypeInfo::Enum(en) => en.type_params.clone(),
            TypeInfo::Newtype(newtype) => newtype.type_params.clone(),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return,
        };
        for derive in &written {
            if !self.refuse_inapplicable_derive(kind, name, derive) {
                self.refuse_unmet_member_requirements(kind, name, derive, &members, &type_params);
            }
        }
        self.refuse_partial_order_without_equality(kind, name, &written, &type_params);
        self.refuse_mismatched_dunder_signatures(name, &info, methods, &type_params);
        self.refuse_dunders_beside_their_derives(kind, name, &written, methods);
    }

    /// Refuse each dunder the type defines beside a derive that provides the same behavior (#1872).
    ///
    /// A written derive counts together with what it implies, so `__eq__` beside `@derive(Ord)` is refused; a builtin
    /// derive spelled in `@rust.derive(...)` counts as written.
    fn refuse_dunders_beside_their_derives(
        &mut self,
        kind: DerivedKind,
        name: &str,
        written: &[WrittenDerive],
        methods: &[Spanned<MethodDecl>],
    ) {
        let rust_derives = self
            .local_derive_facts
            .get(name)
            .map(|facts| facts.rust_builtin_derives.clone())
            .unwrap_or_default();
        let providers = written
            .iter()
            .map(|derive| (derive.spelling.as_str(), derive.id))
            .chain(rust_derives.iter().map(|id| (derives::as_str(*id), *id)))
            .collect::<Vec<_>>();
        for method in methods {
            let dunder = method.node.name.as_str();
            let Some((_, conflicting)) = DUNDER_DERIVE_CONFLICTS
                .iter()
                .find(|(candidate, _)| *candidate == dunder)
            else {
                continue;
            };
            let provider = providers.iter().find(|(_, id)| {
                std::iter::once(*id)
                    .chain(derives::implied_derives(*id).iter().copied())
                    .any(|provided| conflicting.contains(&provided))
            });
            if let Some((spelling, _)) = provider {
                self.errors.push(errors::dunder_conflicts_with_derive(
                    kind.keyword(),
                    name,
                    dunder,
                    spelling,
                    method.span,
                ));
            }
        }
    }

    /// Return the builtin derives written in the declaration's `@derive(...)` decorators, in order.
    fn written_builtin_derives(&self, decorators: &[Spanned<Decorator>]) -> Vec<WrittenDerive> {
        decorators_named(decorators, &self.symbols, DecoratorId::Derive)
            .flat_map(|decorator| positional_derive_names(&decorator.node.args))
            .filter_map(|(spelling, span)| {
                self.builtin_derive_named(&spelling)
                    .map(|id| WrittenDerive { id, spelling, span })
            })
            .collect()
    }

    /// Return the members a derive's trait must hold for, in declaration order, from the checked type.
    fn derived_declaration_members<'a>(
        info: &'a TypeInfo,
        member_spans: &HashMap<&str, Span>,
        fallback_span: Span,
    ) -> Vec<DerivedDeclarationMember<'a>> {
        let span_of = |name: &str| member_spans.get(name).copied().unwrap_or(fallback_span);
        let fields = |order: &'a [String], fields: &'a HashMap<String, crate::symbols::FieldInfo>| {
            order
                .iter()
                .filter_map(|field| fields.get(field).map(|info| (field, info)))
                .map(|(field, info)| DerivedDeclarationMember {
                    member: DerivedMember::Field(field),
                    ty: info.ty.clone(),
                    has_default: info.has_default,
                    span: span_of(field),
                })
                .collect::<Vec<_>>()
        };
        match info {
            TypeInfo::Model(model) => fields(&model.field_order, &model.fields),
            TypeInfo::Class(class) => fields(&class.field_order, &class.fields),
            TypeInfo::Enum(en) => en
                .variants
                .iter()
                .flat_map(|variant| {
                    en.variant_fields
                        .get(variant)
                        .into_iter()
                        .flatten()
                        .map(move |ty| (variant, ty))
                })
                .map(|(variant, ty)| DerivedDeclarationMember {
                    member: DerivedMember::VariantPayload(variant),
                    ty: ty.clone(),
                    has_default: false,
                    span: span_of(variant),
                })
                .collect(),
            TypeInfo::Newtype(newtype) => vec![DerivedDeclarationMember {
                member: DerivedMember::Underlying,
                ty: newtype.underlying.clone(),
                has_default: false,
                span: span_of(""),
            }],
            TypeInfo::Builtin | TypeInfo::TypeAlias => Vec::new(),
        }
    }

    /// Refuse a derive on a declaration kind it does not apply to; returns whether it was refused.
    ///
    /// `Validate` builds a model's validated constructor, and `Default` on an enum needs a default variant Incan cannot
    /// name.
    fn refuse_inapplicable_derive(&mut self, kind: DerivedKind, name: &str, derive: &WrittenDerive) -> bool {
        let applies_to = match derive.id {
            DeriveId::Validate if kind != DerivedKind::Model => "a model",
            DeriveId::Default if kind == DerivedKind::Enum => "a model, class or newtype",
            _ => return false,
        };
        self.errors.push(errors::derive_not_applicable(
            derive.spelling.as_str(),
            kind.keyword(),
            name,
            applies_to,
            derive.span,
        ));
        true
    }

    /// Refuse each member whose type does not implement what one written derive needs of it, the derive together with
    /// what it implies.
    ///
    /// `Default` needs its trait only of a member that declares no default. The declaration's own type parameters are
    /// taken to implement the derive, which bounds them.
    fn refuse_unmet_member_requirements(
        &mut self,
        kind: DerivedKind,
        name: &str,
        derive: &WrittenDerive,
        members: &[DerivedDeclarationMember<'_>],
        type_params: &[String],
    ) {
        let required = std::iter::once(derive.id)
            .chain(derives::implied_derives(derive.id).iter().copied())
            .filter(|required| {
                !(kind.has_automatic_clone_and_debug() && matches!(required, DeriveId::Clone | DeriveId::Debug))
            })
            .collect::<Vec<_>>();
        for member in members {
            if derive.id == DeriveId::Default && member.has_default {
                continue;
            }
            let mut holder = None;
            let mut missing = Vec::new();
            for required in &required {
                if let DeriveSupport::Missing(lacking) =
                    self.derive_support_assuming_type_params(&member.ty, *required, type_params)
                {
                    holder.get_or_insert(lacking);
                    missing.push(derives::as_str(*required));
                }
            }
            let Some(holder) = holder else {
                continue;
            };
            self.errors.push(errors::derive_member_lacks_derive(
                derive.spelling.as_str(),
                kind.keyword(),
                name,
                member.member,
                &member.ty.to_string(),
                &holder.to_string(),
                &missing,
                member.span,
            ));
        }
    }

    /// Refuse `@derive(PartialOrd)` on a type that provides no `PartialEq`, which the derived order builds on and
    /// lowering does not add for it.
    fn refuse_partial_order_without_equality(
        &mut self,
        kind: DerivedKind,
        name: &str,
        written: &[WrittenDerive],
        type_params: &[String],
    ) {
        let Some(partial_ord) = written.iter().find(|derive| derive.id == DeriveId::PartialOrd) else {
            return;
        };
        let owner = if type_params.is_empty() {
            ResolvedType::Named(name.to_string())
        } else {
            ResolvedType::Generic(
                name.to_string(),
                type_params.iter().cloned().map(ResolvedType::TypeVar).collect(),
            )
        };
        // The owner's own answer decides: a generic owner that provides `PartialEq` answers for its type arguments.
        if !matches!(
            self.derive_support_assuming_type_params(&owner, DeriveId::PartialEq, type_params),
            DeriveSupport::Missing(_)
        ) {
            return;
        }
        self.errors.push(errors::derive_requires_derive(
            partial_ord.spelling.as_str(),
            derives::as_str(DeriveId::PartialEq),
            kind.keyword(),
            name,
            partial_ord.span,
        ));
    }

    /// Refuse a comparison dunder or `__str__` whose declaration does not have the signature its operator or builtin
    /// calls it with.
    ///
    /// The generated program implements `==` through `__eq__` as `eq(&self, other: &Self)` and the display of
    /// `str(value)` through `__str__(&self)`, and the other comparison operators call their dunder with one operand of
    /// the same type, so each takes a `self` receiver, `other: Self` for a comparison, and returns `bool` or `str`.
    /// `other` may be written as `Self` or as the declared type with its own type parameters.
    fn refuse_mismatched_dunder_signatures(
        &mut self,
        name: &str,
        info: &TypeInfo,
        methods: &[Spanned<MethodDecl>],
        type_params: &[String],
    ) {
        let (method_infos, method_overloads) = match info {
            TypeInfo::Model(model) => (&model.methods, &model.method_overloads),
            TypeInfo::Class(class) => (&class.methods, &class.method_overloads),
            TypeInfo::Enum(en) => (&en.methods, &en.method_overloads),
            TypeInfo::Newtype(newtype) => (&newtype.methods, &newtype.method_overloads),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return,
        };
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for method in methods {
            let dunder = method.node.name.as_str();
            let Some(signature) = dunder_signature(dunder) else {
                continue;
            };
            let index = seen.entry(dunder).or_default();
            let checked = method_overloads
                .get(dunder)
                .and_then(|overloads| overloads.get(*index))
                .or_else(|| (*index == 0).then(|| method_infos.get(dunder)).flatten());
            *index += 1;
            let Some(checked) = checked else {
                continue;
            };
            if !Self::dunder_matches_signature(checked, &signature, name, type_params) {
                self.errors.push(errors::dunder_signature_mismatch(
                    name,
                    dunder,
                    signature.display,
                    method.span,
                ));
            }
        }
    }

    /// Whether a checked dunder has the signature its operator or builtin calls it with.
    fn dunder_matches_signature(
        checked: &MethodInfo,
        signature: &DunderSignature,
        owner: &str,
        type_params: &[String],
    ) -> bool {
        if checked.receiver != Some(Receiver::Immutable) || checked.return_type != signature.returns {
            return false;
        }
        match (signature.takes_other, checked.params.as_slice()) {
            (false, []) => true,
            (true, [other]) => is_owner_type(&other.ty, owner, type_params),
            _ => false,
        }
    }
}

/// Whether `ty` is the declaring type itself: `Self`, or the declared name with its own type parameters in order.
fn is_owner_type(ty: &ResolvedType, owner: &str, type_params: &[String]) -> bool {
    match ty {
        ResolvedType::SelfType => true,
        ResolvedType::Named(name) => name == owner && type_params.is_empty(),
        ResolvedType::Generic(name, args) => {
            name == owner
                && args.len() == type_params.len()
                && args.iter().zip(type_params).all(|(arg, param)| {
                    matches!(arg, ResolvedType::Named(arg_name) | ResolvedType::TypeVar(arg_name) if arg_name == param)
                })
        }
        _ => false,
    }
}
