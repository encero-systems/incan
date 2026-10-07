//! Check `match` expressions, patterns, and exhaustiveness.
//!
//! This module validates `match` expressions by type-checking each arm, binding pattern variables, and ensuring
//! exhaustiveness for enums, `Result`, and `Option`.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostics::errors;
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::*;
use incan_lang::interop::RustItemKind;
use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::stdlib;
use incan_lang::lang::surface::constructors;
use incan_lang::lang::surface::constructors::ConstructorId;
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_lang::lang::types::numerics;
use incan_semantics_core::SymbolOrigin;

use super::TypeChecker;
use super::match_coverage::{coverage_row_head_is_wild, expand_coverage_heads};
use crate::typechecker::check_stmt::{TupleShape, classify_tuple_shape};

#[derive(Clone)]
struct PatternBinding {
    ty: ResolvedType,
    span: Span,
}

/// Return a stable name list for binding-set comparison and diagnostics.
fn sorted_binding_names(bindings: &HashMap<String, PatternBinding>) -> Vec<String> {
    let mut names: Vec<_> = bindings.keys().cloned().collect();
    names.sort();
    names
}

/// Default payload binding mode after matching through explicit Rust references.
#[derive(Clone, Copy)]
pub(super) enum PatternBorrow {
    Shared,
    Mutable,
}

/// Peel reference layers for constructor lookup while preserving Rust match ergonomics for payload bindings. A shared
/// reference fixes shared binding mode even when another reference layer is mutable.
pub(super) fn borrowed_pattern_subject(mut subject: &ResolvedType) -> (&ResolvedType, Option<PatternBorrow>) {
    let mut borrow = None;
    loop {
        match subject {
            ResolvedType::Ref(inner) => {
                borrow = Some(PatternBorrow::Shared);
                subject = inner;
            }
            ResolvedType::RefMut(inner) => {
                if borrow.is_none() {
                    borrow = Some(PatternBorrow::Mutable);
                }
                subject = inner;
            }
            _ => return (subject, borrow),
        }
    }
}

/// The family of a match position's type that a literal pattern is compared against (#1741).
///
/// A literal spells one of a few scalar types, so a position of one of these families can say for certain whether a
/// literal can ever match there. Following the literal rules of the numeric reference, an integer literal is an `int`
/// and matches only an integer position, whose width then bounds its value; a float literal matches only a float
/// position; `None` matches only an `Option` position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiteralPatternFamily {
    Integer,
    Float,
    Str,
    Bool,
    Option,
}

impl LiteralPatternFamily {
    /// Return the family of a position type, or `None` when the type is of no literal family.
    fn of_position(ty: &ResolvedType) -> Option<Self> {
        match ty {
            ResolvedType::Int => Some(Self::Integer),
            ResolvedType::Float => Some(Self::Float),
            ResolvedType::Numeric(id) if numerics::is_integer(*id) => Some(Self::Integer),
            ResolvedType::Numeric(id) if numerics::is_binary_float(*id) => Some(Self::Float),
            ResolvedType::Numeric(_) | ResolvedType::Bool => Some(Self::Bool),
            ResolvedType::Str | ResolvedType::FrozenStr => Some(Self::Str),
            _ if ty.is_option() => Some(Self::Option),
            _ => None,
        }
    }

    /// Whether a position type of no literal family is known well enough to say that no literal matches a value of it:
    /// `bytes`, a tuple, a collection, a model, class, enum or newtype, a union, a decimal and a type parameter are. An
    /// unresolved type, a Rust type the checker does not see into and `Self` are not, and are left to the checks that
    /// own them.
    fn position_is_known(ty: &ResolvedType) -> bool {
        !matches!(
            ty,
            ResolvedType::Unknown
                | ResolvedType::CallSiteInfer
                | ResolvedType::Never
                | ResolvedType::RustPath(_)
                | ResolvedType::SelfType
                | ResolvedType::Ref(_)
                | ResolvedType::RefMut(_)
        )
    }

    /// Whether a literal of this spelling can match a value of this family.
    fn admits(self, literal: &Literal) -> bool {
        match literal {
            Literal::Int(_) => self == Self::Integer,
            Literal::Float(_) => self == Self::Float,
            Literal::String(_) => self == Self::Str,
            Literal::Bool(_) => self == Self::Bool,
            Literal::None => self == Self::Option,
            Literal::Decimal(_) | Literal::Bytes(_) => false,
        }
    }
}

/// Apply the inherited match binding mode to a payload before checking its nested pattern.
fn borrowed_pattern_payload(field: ResolvedType, borrow: Option<PatternBorrow>) -> ResolvedType {
    match borrow {
        Some(PatternBorrow::Shared) => ResolvedType::Ref(Box::new(field)),
        Some(PatternBorrow::Mutable) => ResolvedType::RefMut(Box::new(field)),
        None => field,
    }
}

impl TypeChecker {
    /// Split a constructor pattern name into its optional enum qualifier and variant segment.
    ///
    /// The parser normalizes qualified surface patterns like `Color.Red` to `Color::Red`, while bare constructors
    /// such as `Some` and `Ok` keep the unqualified spelling. Match checking needs both pieces separately so
    /// qualifier validation and variant symbol lookup stay consistent.
    pub(super) fn split_pattern_constructor_name(name: &str) -> (Option<&str>, &str) {
        match name.rsplit_once("::") {
            Some((qualifier, variant)) => (Some(qualifier), variant),
            None => (None, name),
        }
    }

    /// Whether a type spelled `written` in a pattern names the subject's type `subject_name`.
    ///
    /// The spellings agree, or they are two spellings of one compiled-library declaration: a consumer's import of the
    /// type and the provider-qualified key a dependency signature carries when the signature was imported before the
    /// type, or two public paths of the type. Both carry the declaration's identity, so the pattern names the subject's
    /// type.
    fn pattern_type_spelling_names_subject(&self, written: &str, subject_name: &str) -> bool {
        written == subject_name
            || matches!(
                (
                    self.public_library_type_identities.get(written),
                    self.public_library_type_identities.get(subject_name),
                ),
                (Some(written_identity), Some(subject_identity)) if written_identity == subject_identity
            )
    }

    /// Whether an explicit pattern qualifier names the same enum-like scrutinee type being matched.
    fn pattern_qualifier_matches_expected_type(expected_ty: &ResolvedType, qualifier: &str) -> bool {
        match expected_ty {
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => qualifier == type_name,
            _ => false,
        }
    }

    /// Resolve the semantic type captured by a union type pattern.
    ///
    /// A constructor pattern over a union may name either one concrete member (`A(value)`) or a transparent alias whose
    /// expanded union members are a subset of the scrutinee union (`Base(value)` where `Input = Union[Base, int]`).
    fn union_pattern_target_type(&self, expected_ty: &ResolvedType, name: &str) -> Option<ResolvedType> {
        let target_ty = self.union_member_target_spelling(
            self.expand_type_aliases(resolve_type(&Type::Simple(name.to_string()), &self.symbols)),
        );
        let members = Self::expected_union_members(expected_ty)?;

        if let Some(target_members) = target_ty.union_members()
            && target_members.iter().all(|target| {
                members
                    .iter()
                    .any(|member| self.match_union_member_matches(member, target))
            })
        {
            return Some(union_ty(target_members.to_vec()));
        }

        members
            .iter()
            .find(|member| self.match_union_member_matches(member, &target_ty))
            .cloned()
    }

    /// Return the active union member set for a match subject, including `Option[Union[...]]`.
    fn expected_union_members(expected_ty: &ResolvedType) -> Option<&[ResolvedType]> {
        if let Some(members) = expected_ty.union_members() {
            Some(members)
        } else if let Some(inner) = expected_ty.option_inner_type() {
            inner.union_members()
        } else {
            None
        }
    }

    /// Type-check a `match` expression and return its resolved type: the one type its arms unify to.
    ///
    /// Every arm produces the match's value, in statement position too, since the generated match needs one type
    /// across its arms. An expression arm is checked against `expected`, the type of the place the match is written to
    /// when there is one, so a literal arm takes that type as it does at the place itself. A block arm produces no
    /// value (`None`): it takes part only when it can complete, and a block that ends in `return`, `break` or
    /// `continue` cannot. The arm types unify as [`Self::unify_branch_value_types`] states: a narrower numeric arm is
    /// widened and a payload arm is wrapped in `Some` in the arm itself, and arms that share no type are refused with
    /// a type mismatch. Each arm is written to the expected type directly when that type is numeric or an `Option`
    /// that holds no union, the adaptations lowering makes in an arm; otherwise the arms unify among themselves and
    /// the whole match is written to the place, since an arm is not made a union member or any other type in place.
    pub(in crate::typechecker::check_expr) fn check_match(
        &mut self,
        subject: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
        _span: Span,
        expected: Option<&ResolvedType>,
    ) -> ResolvedType {
        let subject_ty = self.check_expr(subject);
        let subject_ty = self.settle_open_constructor_sides_in_place(subject, subject_ty);
        let subject_binding = if let Expr::Ident(name) = &subject.node {
            self.lookup_variable_info(name)
                .cloned()
                .map(|info| (name.clone(), info, subject.span))
        } else {
            None
        };
        let mut remaining_union_members = subject_ty.union_members().map(|members| members.to_vec());
        let view_param = self.pattern_view_param(subject);

        self.check_match_exhaustiveness(&subject_ty, arms, _span);

        let mut arm_values = Vec::new();

        for arm in arms {
            let narrowed_subject_ty = remaining_union_members
                .as_ref()
                .and_then(|remaining| self.match_arm_remainder_type(&arm.node.pattern, remaining));
            let expected_ty = narrowed_subject_ty.as_ref().unwrap_or(&subject_ty);

            self.symbols.enter_scope(ScopeKind::Block);
            self.check_pattern(&arm.node.pattern, expected_ty);
            let views = self.enter_pattern_views(&arm.node.pattern.node, view_param.clone());
            if let (Some((name, info, span)), Some(ty)) = (&subject_binding, narrowed_subject_ty.clone()) {
                self.symbols.define_refined_binding(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty,
                        is_mutable: info.is_mutable,
                        is_used: false,
                    }),
                    span: *span,
                    scope: 0,
                });
                if info.is_mutable {
                    self.mutable_bindings.insert(name.clone());
                }
            }

            if let Some(guard) = &arm.node.guard {
                let guard_ty = self.check_expr(guard);
                self.validate_truthiness_condition(&guard_ty, guard.span);
            }

            match &arm.node.body {
                MatchBody::Expr(e) => {
                    let arm_ty = match expected {
                        Some(expected) => self.check_expr_with_expected(e, Some(expected)),
                        None => self.check_expr(e),
                    };
                    arm_values.push((arm_ty, e.span));
                }
                MatchBody::Block(stmts) => {
                    self.check_statement_block(stmts);
                    if !block_cannot_complete(stmts) {
                        arm_values.push((ResolvedType::Unit, arm.span));
                    }
                }
            }

            self.exit_pattern_views(views);
            self.symbols.exit_scope();

            if arm.node.guard.is_none()
                && let Some(remaining) = remaining_union_members.as_mut()
            {
                self.remove_covered_union_members(remaining, &arm.node.pattern, &subject_ty);
            }
        }
        self.note_dict_lookup_match(subject, arms);

        let arm_destination = expected
            .map(|expected| self.expand_type_aliases(expected.clone()))
            .filter(|expected| {
                (expected.is_option() || crate::typechecker::numeric_type_id_for_compat(expected).is_some())
                    && !Self::type_holds_union(expected)
            });
        self.unify_branch_value_types(&arm_values, arm_destination.as_ref())
            .or_else(|| arm_values.first().map(|(ty, _)| ty.clone()))
            .unwrap_or(ResolvedType::Unit)
    }

    /// Return the type represented by the as-yet-uncovered union members for wildcard and binding arms.
    fn match_arm_remainder_type(&self, pattern: &Spanned<Pattern>, remaining: &[ResolvedType]) -> Option<ResolvedType> {
        match &pattern.node {
            Pattern::Wildcard | Pattern::Binding(_) if !remaining.is_empty() => {
                Some(Self::localize_union_member(union_ty(remaining.to_vec())))
            }
            Pattern::Group(inner) => self.match_arm_remainder_type(inner, remaining),
            _ => None,
        }
    }

    /// Compare exact checked union alternatives for both narrowing and exhaustiveness.
    ///
    /// Public nominal aliases share their selected artifact and canonical declaration. Assignment conversions do
    /// not prove coverage: even mutually compatible numeric types remain separate union alternatives.
    fn match_union_member_matches(&self, member: &ResolvedType, target: &ResolvedType) -> bool {
        match (
            self.public_library_type_identity_for_type(member),
            self.public_library_type_identity_for_type(target),
        ) {
            (Some((left, left_args)), Some((right, right_args))) => {
                return left == right
                    && left_args.len() == right_args.len()
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.match_union_member_matches(left, right));
            }
            (Some(_), None) | (None, Some(_)) => return false,
            (None, None) => {}
        }
        match (member, target) {
            (ResolvedType::Generic(left, left_args), ResolvedType::Generic(right, right_args)) => {
                left == right
                    && left_args.len() == right_args.len()
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.match_union_member_matches(left, right))
            }
            (ResolvedType::Tuple(left), ResolvedType::Tuple(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| self.match_union_member_matches(left, right))
            }
            (ResolvedType::FrozenList(left), ResolvedType::FrozenList(right))
            | (ResolvedType::FrozenSet(left), ResolvedType::FrozenSet(right))
            | (ResolvedType::TypeToken(left), ResolvedType::TypeToken(right))
            | (ResolvedType::Ref(left), ResolvedType::Ref(right))
            | (ResolvedType::RefMut(left), ResolvedType::RefMut(right)) => self.match_union_member_matches(left, right),
            (ResolvedType::FrozenDict(left_key, left_value), ResolvedType::FrozenDict(right_key, right_value)) => {
                self.match_union_member_matches(left_key, right_key)
                    && self.match_union_member_matches(left_value, right_value)
            }
            _ => member == target,
        }
    }

    /// Remove the union members covered by a pattern from the remaining-arm accumulator.
    fn remove_covered_union_members(
        &self,
        remaining: &mut Vec<ResolvedType>,
        pattern: &Spanned<Pattern>,
        subject_ty: &ResolvedType,
    ) {
        match &pattern.node {
            Pattern::Constructor(name, sub_patterns) => {
                let (enum_qualifier_opt, ctor_name) = Self::split_pattern_constructor_name(name.node.as_str());
                if enum_qualifier_opt.is_none()
                    && let Some(member_ty) = self.union_pattern_target_type(subject_ty, ctor_name)
                {
                    if let Some(target_members) = member_ty.union_members() {
                        remaining.retain(|member| {
                            !target_members.iter().any(|target| {
                                self.match_union_member_matches(member, target)
                                    && self.union_type_pattern_payload_is_exhaustive(sub_patterns, target)
                            })
                        });
                    } else {
                        remaining.retain(|member| {
                            !self.match_union_member_matches(member, &member_ty)
                                || !self.union_type_pattern_payload_is_exhaustive(sub_patterns, &member_ty)
                        });
                    }
                }
            }
            Pattern::Or(alternatives) => {
                for alternative in alternatives {
                    self.remove_covered_union_members(remaining, alternative, subject_ty);
                }
            }
            Pattern::Group(inner) => self.remove_covered_union_members(remaining, inner, subject_ty),
            Pattern::Wildcard | Pattern::Binding(_) => remaining.clear(),
            _ => {}
        }
    }

    /// Return whether a union type pattern covers every value of the member it names.
    ///
    /// `int(n)` covers the `int` member, while `int(0)` leaves every other integer uncovered. The nested pattern is
    /// the payload stored by the generated union wrapper, so it is judged by the same pattern-matrix walk used for an
    /// enum variant payload rather than treating the outer type name as proof of complete coverage (#1876).
    fn union_type_pattern_payload_is_exhaustive(&self, sub_patterns: &[PatternArg], member_ty: &ResolvedType) -> bool {
        let rows = sub_patterns
            .iter()
            .find_map(|arg| match arg {
                PatternArg::Positional(pattern) => Some(vec![vec![Some(&pattern.node)]]),
                PatternArg::Named(_, _) => None,
            })
            .unwrap_or_default();
        self.coverage_rows_exhaustive(rows, std::slice::from_ref(member_ty), 0)
    }

    /// Check the one positional payload accepted by `Some`, `Ok`, and `Err` patterns.
    fn check_single_payload_constructor_pattern(
        &mut self,
        constructor: &Spanned<String>,
        sub_patterns: &[PatternArg],
        payload_ty: &ResolvedType,
    ) {
        if sub_patterns.len() != 1 {
            self.errors.push(errors::builtin_arity(
                &constructor.node,
                1,
                sub_patterns.len(),
                constructor.span,
            ));
        }
        let mut checked_positional = false;
        for arg in sub_patterns {
            match arg {
                PatternArg::Positional(pattern) if !checked_positional => {
                    self.check_pattern(pattern, payload_ty);
                    checked_positional = true;
                }
                PatternArg::Positional(_) => {}
                PatternArg::Named(_, pattern) => self
                    .errors
                    .push(errors::named_pattern_not_supported(&constructor.node, pattern.span)),
            }
        }
    }

    /// Record the canonical fields a model or class destructuring pattern leaves unnamed.
    ///
    /// `field_order` is the nominal's complete field list in declaration order and `provided` the canonical names
    /// the pattern spelled (aliases already resolved). The difference is what a `..` would cover in Rust; lowering
    /// spells it out as one wildcard per field (#1708). A pattern that names every field leaves no record, so a
    /// consumer reads absence as "nothing to add".
    ///
    /// When the rest includes a field this pattern may not name (a private field of `type_name` matched outside its
    /// owner's methods, by the same rule that refuses naming it), the pattern is also recorded as one whose rest must
    /// stay unspelled, and lowering covers it with a rest marker instead (#1740). A field missing from `fields`
    /// counts as unnameable: a rest marker covers any field, while a spelled one must exist and be visible.
    fn record_pattern_rest_fields(
        &mut self,
        span: Span,
        type_name: &str,
        fields: &HashMap<String, FieldInfo>,
        field_order: &[String],
        provided: &HashSet<String>,
    ) {
        let rest: Vec<String> = field_order
            .iter()
            .filter(|field| !provided.contains(*field))
            .cloned()
            .collect();
        if rest.is_empty() {
            return;
        }
        let rest_has_private_field = rest.iter().any(|field| {
            fields
                .get(field)
                .is_none_or(|info| self.private_field_is_inaccessible(type_name, info))
        });
        if rest_has_private_field {
            self.type_info
                .expressions
                .pattern_rests_with_private_fields
                .insert((span.start, span.end));
        }
        self.type_info
            .expressions
            .pattern_rest_fields
            .insert((span.start, span.end), rest);
    }

    /// Record a constructor pattern that resolved through the active lexical binding.
    pub(in crate::typechecker) fn record_pattern_lexical_identity(&mut self, name: &str, span: Span) {
        let identity = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.identity_of(symbol_id))
            .cloned();
        if let Some(identity) = identity {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Record the source enum variant selected by a checked constructor pattern.
    fn record_incan_enum_pattern_identity(&mut self, expected_ty: &ResolvedType, variant: &str, span: Span) {
        let enum_name = match expected_ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return,
        };
        let identity = match self.lookup_semantic_type_info(enum_name) {
            Some(TypeInfo::Enum(info)) => info.variant_identities.get(variant).cloned(),
            _ => None,
        };
        if let Some(identity) = identity {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Record the enum-qualified canonical variant a checked variant pattern over an Incan enum names.
    ///
    /// The subject's enum qualifies the variant, and a variant alias resolves to the variant it names, so a bare
    /// `Filled(n)` and an aliased `Full(n)` or `Shape.Full(n)` are spelled `Shape::Filled` by lowering, as a qualified
    /// pattern over the canonical variant is. A module that matches a value of an enum another project module declares
    /// without binding the enum's name spells it from the crate root (`crate::shapes::Shape::Filled`), and a value of a
    /// dependency's enum typed by its provider-qualified key (`pub::recall::Outcome`) spells it from the dependency's
    /// crate, as lowering spells that type (`recall::Outcome::Found`).
    fn record_incan_enum_pattern_path(&mut self, expected_ty: &ResolvedType, variant: &str, span: Span) {
        let enum_name = match expected_ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return,
        };
        let Some(TypeInfo::Enum(info)) = self.lookup_semantic_type_info(enum_name) else {
            return;
        };
        let canonical = info.variant_aliases.get(variant).map_or(variant, String::as_str);
        let enum_binds_here = self
            .lookup_symbol(enum_name)
            .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Type(TypeInfo::Enum(_))));
        let owner = match info.variant_identities.get(canonical).map(|identity| &identity.origin) {
            Some(SymbolOrigin::Module(module_path))
                if !enum_binds_here && module_path.first().is_some_and(|root| root != stdlib::STDLIB_ROOT) =>
            {
                format!(
                    "{}::{}::{enum_name}",
                    keywords::as_str(KeywordId::Crate),
                    module_path.join("::")
                )
            }
            _ => match crate::typechecker::split_canonical_public_library_type_name(enum_name) {
                Some((library, public_name)) => format!("{library}::{public_name}"),
                None => enum_name.clone(),
            },
        };
        let path = format!("{owner}::{canonical}");
        self.type_info
            .expressions
            .pattern_variant_paths
            .insert((span.start, span.end), path);
    }

    /// Type-check a pattern against an expected type, defining bindings in the current scope.
    ///
    /// Every pattern node's checked type is recorded at its own span before it is dispatched on, the way a `for`
    /// pattern's item type is (#1125). Lowering has no way to rebuild a payload type on its own -- a user enum's
    /// variant payloads, an imported or Rust-backed enum's, a model field's declared type, or the borrow wrapper
    /// [`borrowed_pattern_payload`] applies -- so this recorded fact is what lets a destructured binding carry its
    /// declared type rather than an unresolved one (#1245). The record is unconditional: a node checked against
    /// [`ResolvedType::Unknown`] records that honestly, and a consumer treats it as "no fact" rather than as a type.
    /// Union constructors also record their selected target at the constructor-name span so Body IR can retain member
    /// selection independently of spelling.
    pub(in crate::typechecker) fn check_pattern(&mut self, pattern: &Spanned<Pattern>, expected_ty: &ResolvedType) {
        self.record_expr_type(pattern.span, expected_ty.clone());
        match &pattern.node {
            Pattern::Wildcard => {}
            Pattern::Binding(name) => {
                self.validate_protected_builtin_binding(name, pattern.span);
                self.symbols.define(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: expected_ty.clone(),
                        is_mutable: false,
                        is_used: false,
                    }),
                    span: pattern.span,
                    scope: 0,
                });
                self.record_write_target_identity(pattern.span, name);
            }
            Pattern::Group(inner) => {
                self.check_pattern(inner, expected_ty);
            }
            Pattern::Or(alternatives) => {
                self.check_or_pattern(alternatives, expected_ty);
            }
            Pattern::Literal(literal) => self.check_literal_pattern(literal, expected_ty, pattern.span),
            Pattern::Constructor(name, sub_patterns) => {
                let (subject_ty, borrow) = borrowed_pattern_subject(expected_ty);
                let (enum_qualifier_opt, ctor_name) = Self::split_pattern_constructor_name(name.node.as_str());
                if enum_qualifier_opt.is_none()
                    && let Some(member_ty) = self.union_pattern_target_type(expected_ty, ctor_name)
                {
                    self.record_pattern_lexical_identity(ctor_name, name.span);
                    self.record_expr_type(name.span, member_ty.clone());
                    let mut positional = None;
                    for arg in sub_patterns {
                        match arg {
                            PatternArg::Positional(pat) => {
                                positional = Some(pat);
                                break;
                            }
                            PatternArg::Named(_, pat) => {
                                self.errors
                                    .push(errors::named_pattern_not_supported(&name.node, pat.span));
                            }
                        }
                    }
                    if let Some(pat) = positional {
                        let written_ty = resolve_type(&Type::Simple(ctor_name.to_string()), &self.symbols);
                        self.check_pattern(pat, &Self::narrowed_union_member_type(&member_ty, &written_ty));
                    }
                    return;
                }

                let qualifier_matches_expected = enum_qualifier_opt
                    .is_none_or(|qualifier| Self::pattern_qualifier_matches_expected_type(subject_ty, qualifier));

                if qualifier_matches_expected && let Some(cid) = constructors::from_str(ctor_name) {
                    match cid {
                        ConstructorId::Ok => {
                            if let ResolvedType::Generic(type_name, args) = subject_ty
                                && type_name == collections::as_str(CollectionTypeId::Result)
                                && !args.is_empty()
                            {
                                self.record_pattern_lexical_identity(ctor_name, name.span);
                                let payload_ty = borrowed_pattern_payload(args[0].clone(), borrow);
                                self.check_single_payload_constructor_pattern(name, sub_patterns, &payload_ty);
                                return;
                            }
                        }
                        ConstructorId::Err => {
                            if let ResolvedType::Generic(type_name, args) = subject_ty
                                && type_name == collections::as_str(CollectionTypeId::Result)
                                && args.len() >= 2
                            {
                                self.record_pattern_lexical_identity(ctor_name, name.span);
                                let payload_ty = borrowed_pattern_payload(args[1].clone(), borrow);
                                self.check_single_payload_constructor_pattern(name, sub_patterns, &payload_ty);
                                return;
                            }
                        }
                        ConstructorId::Some => {
                            if let ResolvedType::Generic(type_name, args) = subject_ty
                                && type_name == collections::as_str(CollectionTypeId::Option)
                                && !args.is_empty()
                            {
                                self.record_pattern_lexical_identity(ctor_name, name.span);
                                let payload_ty = borrowed_pattern_payload(args[0].clone(), borrow);
                                self.check_single_payload_constructor_pattern(name, sub_patterns, &payload_ty);
                                return;
                            }
                        }
                        ConstructorId::None => {
                            self.record_pattern_lexical_identity(ctor_name, name.span);
                            return;
                        }
                    }
                }

                let ctor_name = if name.node.contains("::") {
                    name.node.split("::").last().unwrap_or(&name.node)
                } else {
                    name.node.as_str()
                };

                // A record pattern names the subject's model or class, generic ones included: each field it names
                // is checked against the field's type under the subject's type arguments.
                let model_or_class_fields = match subject_ty {
                    ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _)
                        if self.pattern_type_spelling_names_subject(ctor_name, type_name) =>
                    {
                        let type_args = match subject_ty {
                            ResolvedType::Generic(_, type_args) => type_args.as_slice(),
                            _ => &[],
                        };
                        // A dependency's type the module reaches only through a signature has no lexical symbol.
                        self.lookup_semantic_type_info(type_name)
                            .and_then(|type_info| match type_info {
                                TypeInfo::Model(model_info) => Some((
                                    model_info.fields.clone(),
                                    model_info.field_order.clone(),
                                    type_param_subst_map(&model_info.type_params, type_args),
                                )),
                                TypeInfo::Class(class_info) => Some((
                                    class_info.fields.clone(),
                                    class_info.field_order.clone(),
                                    type_param_subst_map(&class_info.type_params, type_args),
                                )),
                                _ => None,
                            })
                            .map(|(fields, field_order, substitutions)| (type_name, fields, field_order, substitutions))
                    }
                    _ => None,
                };

                if let Some((type_name, fields, field_order, substitutions)) = model_or_class_fields {
                    self.record_pattern_lexical_identity(type_name, name.span);
                    let mut provided = HashSet::new();
                    for arg in sub_patterns {
                        match arg {
                            PatternArg::Positional(pat) => {
                                self.errors
                                    .push(errors::positional_pattern_not_supported(type_name, pat.span));
                            }
                            PatternArg::Named(field_name, pat) => {
                                let Some((canonical_name, info)) =
                                    self.resolve_field_info(&fields, &field_name.node, true, true)
                                else {
                                    self.errors.push(errors::missing_field(
                                        type_name,
                                        &field_name.node,
                                        field_name.span,
                                    ));
                                    continue;
                                };

                                if let Some(identity) = info.identity.clone() {
                                    self.type_info.record_resolved_identity(field_name.span, identity);
                                }

                                if self.private_field_is_inaccessible(type_name, info) {
                                    self.errors.push(errors::private_field(
                                        type_name,
                                        &field_name.node,
                                        field_name.span,
                                    ));
                                    continue;
                                }

                                if !provided.insert(canonical_name.clone()) {
                                    self.errors.push(errors::duplicate_pattern_field(
                                        type_name,
                                        canonical_name.as_str(),
                                        pat.span,
                                    ));
                                    continue;
                                }
                                let field_ty = substitute_resolved_type(&info.ty, &substitutions);
                                self.check_pattern(pat, &borrowed_pattern_payload(field_ty, borrow));
                            }
                        }
                    }
                    self.record_pattern_rest_fields(name.span, type_name, &fields, &field_order, &provided);
                    return;
                }

                let variant_name = ctor_name;

                let positional_count = sub_patterns
                    .iter()
                    .filter(|a| matches!(a, PatternArg::Positional(_)))
                    .count();

                let incan_resolution =
                    self.incan_enum_constructor_payload_types(expected_ty, variant_name, enum_qualifier_opt);
                let rust_resolution =
                    self.rust_enum_constructor_payload_types(expected_ty, name.node.as_str(), positional_count);
                let field_types: Option<Vec<ResolvedType>> =
                    incan_resolution.clone().or_else(|| match rust_resolution.as_ref() {
                        Some(RustEnumPatternResolution::PayloadTypes(fields)) => Some(fields.clone()),
                        Some(RustEnumPatternResolution::QualifierMismatch) | None => None,
                    });

                match field_types {
                    Some(fields) => {
                        if incan_resolution.is_some() {
                            self.record_incan_enum_pattern_identity(expected_ty, variant_name, name.span);
                            self.record_incan_enum_pattern_path(expected_ty, variant_name, name.span);
                            // One sub-pattern per payload value (#1561); a named sub-pattern is refused on its own.
                            let all_positional =
                                sub_patterns.iter().all(|arg| matches!(arg, PatternArg::Positional(_)));
                            if all_positional && positional_count != fields.len() {
                                self.errors.push(errors::pattern_arity_mismatch(
                                    &format!("The pattern '{}'", name.node),
                                    "payload value",
                                    fields.len(),
                                    positional_count,
                                    pattern.span,
                                ));
                            }
                        }
                        self.check_constructor_subpatterns_enum_like(
                            name.node.as_str(),
                            sub_patterns,
                            Some(fields.as_slice()),
                            self.pattern_subject_is_rust_backed(expected_ty),
                        );
                    }
                    None => {
                        let permissive = self.match_subject_allows_unknown_rust_enum_payloads(
                            expected_ty,
                            name.node.as_str(),
                            rust_resolution.as_ref(),
                        );
                        if !permissive && !matches!(expected_ty, ResolvedType::Unknown) {
                            self.errors.push(errors::unknown_match_constructor_pattern(
                                name.node.as_str(),
                                &expected_ty.to_string(),
                                pattern.span,
                            ));
                        }
                        self.check_constructor_subpatterns_enum_like(
                            name.node.as_str(),
                            sub_patterns,
                            None,
                            self.pattern_subject_is_rust_backed(expected_ty),
                        );
                    }
                }
            }
            Pattern::Tuple(sub_patterns) => {
                // A tuple subject arrives in two spellings: a tuple literal or a `(A, B)` annotation infers
                // `ResolvedType::Tuple`, while a written `tuple[A, B]` resolves through the collection registry as
                // `Generic("Tuple", …)`. Both destructure the same way, and the classification `for` and unpack
                // already use is the one rule for it; matching only the first spelling left the sub-patterns of the
                // second unvisited, so their names were never bound (#1714).
                let (subject_ty, borrow) = borrowed_pattern_subject(expected_ty);
                if let TupleShape::Tuple(elem_types) = classify_tuple_shape(subject_ty) {
                    // One sub-pattern per element (#1561).
                    if sub_patterns.len() != elem_types.len() {
                        self.errors.push(errors::pattern_arity_mismatch(
                            "A tuple pattern",
                            "element",
                            elem_types.len(),
                            sub_patterns.len(),
                            pattern.span,
                        ));
                    }
                    for (pat, elem_ty) in sub_patterns.iter().zip(elem_types.iter()) {
                        self.check_pattern(pat, &borrowed_pattern_payload(elem_ty.clone(), borrow));
                    }
                }
            }
        }
    }

    /// Refuse a literal pattern whose value can never match the position it is in (#1741).
    ///
    /// A decimal or bytes literal has no pattern form at all. Any other literal is compared with the position's type
    /// (the scrutinee, a tuple element, a variant payload or a field) after peeling the borrow wrappers match
    /// ergonomics add: a literal matches only a position of its own family (`LiteralPatternFamily`), so one in a known
    /// position of no family (a type parameter, a union, a nominal, a collection, a tuple) is refused too, and only an
    /// unresolved or Rust-only position type is left to the checks that own it. A numeric literal that has the
    /// position's family is then held to the position's width and range by the same rules a value literal of that type
    /// follows; a suffixed one is held to its suffix's range, and its suffix must name the position's exact type.
    fn check_literal_pattern(&mut self, literal: &Literal, expected_ty: &ResolvedType, span: Span) {
        let unmatchable = match literal {
            Literal::Decimal(_) => Some("decimal"),
            Literal::Bytes(_) => Some("bytes"),
            _ => None,
        };
        if let Some(kind) = unmatchable {
            self.errors.push(errors::pattern_literal_not_matchable(kind, span));
            return;
        }
        let (position_ty, _) = borrowed_pattern_subject(expected_ty);
        let family = LiteralPatternFamily::of_position(position_ty);
        if family.is_none() && !LiteralPatternFamily::position_is_known(position_ty) {
            return;
        }
        if !family.is_some_and(|family| family.admits(literal)) {
            let found = match literal {
                Literal::None => constructors::as_str(ConstructorId::None).to_string(),
                _ => self.check_literal(literal, span).to_string(),
            };
            self.errors.push(errors::pattern_literal_type_mismatch(
                &position_ty.to_string(),
                &found,
                span,
            ));
            return;
        }
        let position_ty = position_ty.clone();
        if matches!(literal, Literal::Int(value) if value.suffix.is_some())
            || matches!(literal, Literal::Float(value) if value.suffix.is_some())
        {
            // A suffix names the literal's type: its value is held to that type's range, and a pattern literal is
            // compared without conversion, so the position must be that exact type.
            let errors_before = self.errors.len();
            let literal_ty = self.check_literal(literal, span);
            if self.errors.len() == errors_before
                && super::super::numeric_type_id_for_compat(&literal_ty)
                    != super::super::numeric_type_id_for_compat(&position_ty)
            {
                self.errors.push(errors::pattern_literal_type_mismatch(
                    &position_ty.to_string(),
                    &literal_ty.to_string(),
                    span,
                ));
            }
            return;
        }
        let literal_expr = Spanned::new(Expr::Literal(literal.clone()), span);
        match literal {
            Literal::Int(_) => {
                self.check_int_literal_with_expected(&literal_expr, &position_ty);
            }
            Literal::Float(_) => {
                self.check_float_literal_with_expected(&literal_expr, &position_ty);
            }
            _ => {}
        }
    }

    /// Type-check alternatives in isolated scopes, then define only the agreed binding set in the surrounding arm
    /// scope.
    ///
    /// Without the isolation step, `A(x) | B(y)` would accidentally leak both `x` and `y` into the branch body even
    /// though no single successful match can provide both names. RFC 071 requires every alternative to bind the same
    /// names with the same types before any branch-local binding is made visible.
    fn check_or_pattern(&mut self, alternatives: &[Spanned<Pattern>], expected_ty: &ResolvedType) {
        let mut binding_sets = Vec::new();

        for alternative in alternatives {
            let before = self.symbols.all_symbols().len();
            self.symbols.enter_scope(ScopeKind::Block);
            self.check_pattern(alternative, expected_ty);
            let bindings = self.collect_pattern_bindings_since(before);
            self.symbols.exit_scope();
            binding_sets.push((alternative.span, bindings));
        }

        let Some((_, first_bindings)) = binding_sets.first() else {
            return;
        };

        let expected_names = sorted_binding_names(first_bindings);
        let mut agreement_ok = true;

        for (span, bindings) in binding_sets.iter().skip(1) {
            let found_names = sorted_binding_names(bindings);
            if found_names != expected_names {
                self.errors.push(errors::pattern_alternation_binding_mismatch(
                    &expected_names,
                    &found_names,
                    *span,
                ));
                agreement_ok = false;
                continue;
            }

            for name in &expected_names {
                let Some(expected) = first_bindings.get(name) else {
                    continue;
                };
                let Some(found) = bindings.get(name) else {
                    continue;
                };
                if expected.ty != found.ty {
                    self.errors.push(errors::pattern_alternation_binding_type_mismatch(
                        name,
                        &expected.ty.to_string(),
                        &found.ty.to_string(),
                        found.span,
                    ));
                    agreement_ok = false;
                }
            }
        }

        if !agreement_ok {
            return;
        }

        for name in expected_names {
            let Some(binding) = first_bindings.get(&name) else {
                continue;
            };
            self.symbols.define(Symbol {
                name: name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty: binding.ty.clone(),
                    is_mutable: false,
                    is_used: false,
                }),
                span: binding.span,
                scope: 0,
            });
            for (_, bindings) in &binding_sets {
                if let Some(alternative_binding) = bindings.get(&name) {
                    // Every syntactic write in a valid OR-pattern introduces the one binding visible to the arm
                    // body. Overwrite the provisional isolated-scope facts with that final binding identity.
                    self.record_write_target_identity(alternative_binding.span, &name);
                }
            }
        }
    }

    /// Collect variable bindings defined while checking one isolated alternation alternative.
    fn collect_pattern_bindings_since(&self, start: usize) -> HashMap<String, PatternBinding> {
        self.symbols
            .all_symbols()
            .iter()
            .skip(start)
            .filter_map(|symbol| match &symbol.kind {
                SymbolKind::Variable(info) => Some((
                    symbol.name.clone(),
                    PatternBinding {
                        ty: info.ty.clone(),
                        span: symbol.span,
                    },
                )),
                _ => None,
            })
            .collect()
    }

    /// Positional sub-patterns for enum-like constructor patterns: known payload types per index, or all
    /// [`ResolvedType::Unknown`] when `known_fields` is `None` (Rust interop best-effort).
    fn check_constructor_subpatterns_enum_like(
        &mut self,
        ctor_label: &str,
        sub_patterns: &[PatternArg],
        known_fields: Option<&[ResolvedType]>,
        rust_backed: bool,
    ) {
        let mut idx = 0usize;
        for arg in sub_patterns {
            match arg {
                PatternArg::Positional(pat) => {
                    if let Some(fields) = known_fields {
                        if let Some(field_ty) = fields.get(idx) {
                            self.check_pattern(pat, field_ty);
                        }
                    } else {
                        self.check_pattern(pat, &ResolvedType::Unknown);
                    }
                    idx += 1;
                }
                PatternArg::Named(_, pat) => {
                    // A Rust enum may use struct variants, whose fields are named and cannot be destructured
                    // positionally. Rust variant metadata records payload shapes in declaration order but not field
                    // names, so the payload is checked permissively here and `rustc` validates the names, matching
                    // how the rest of Rust-interop payload checking already behaves.
                    if rust_backed {
                        self.check_pattern(pat, &ResolvedType::Unknown);
                    } else {
                        self.errors
                            .push(errors::named_pattern_not_supported(ctor_label, pat.span));
                    }
                }
            }
        }
    }

    /// Return whether a match subject is backed by a Rust type, directly or through a `rusttype` newtype.
    ///
    /// Rust-backed subjects get permissive payload treatment throughout pattern checking, because Rust metadata
    /// records payload shapes without the detail an Incan declaration would carry.
    fn pattern_subject_is_rust_backed(&self, expected_ty: &ResolvedType) -> bool {
        let (expected_ty, _) = borrowed_pattern_subject(expected_ty);
        match expected_ty {
            ResolvedType::RustPath(_) => true,
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => {
                self.lookup_type_info(type_name).is_some_and(|info| {
                    matches!(
                        info,
                        TypeInfo::Newtype(nt) if nt.is_rusttype && matches!(&nt.underlying, ResolvedType::RustPath(_))
                    )
                })
            }
            _ => false,
        }
    }

    /// When constructor payload types are missing, only Rust-backed match subjects use permissive
    /// [`ResolvedType::Unknown`] payload checking, and only when the written constructor does not already prove the
    /// pattern is invalid (for example an explicit mismatched rusttype qualifier).
    fn match_subject_allows_unknown_rust_enum_payloads(
        &self,
        expected_ty: &ResolvedType,
        pattern_full_name: &str,
        rust_resolution: Option<&RustEnumPatternResolution>,
    ) -> bool {
        if !self.pattern_subject_is_rust_backed(expected_ty) {
            return false;
        }

        match rust_resolution {
            Some(RustEnumPatternResolution::PayloadTypes(_)) => true,
            Some(RustEnumPatternResolution::QualifierMismatch) => false,
            None => !pattern_full_name.contains("::"),
        }
    }

    /// Payload types for a source-defined enum variant, using the enum type's own metadata under the subject's type
    /// arguments.
    ///
    /// Qualified patterns such as `Color.Red` should not depend on a module-level `Red` symbol being importable or
    /// winning same-scope shadowing. The scrutinee already tells us which enum is being matched, so resolve the
    /// variant from that enum's table. The caller compares the pattern's sub-pattern count with the payload count.
    fn incan_enum_constructor_payload_types(
        &self,
        expected_ty: &ResolvedType,
        variant_name: &str,
        enum_qualifier_opt: Option<&str>,
    ) -> Option<Vec<ResolvedType>> {
        let enum_name = match expected_ty {
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => type_name,
            _ => return None,
        };
        if enum_qualifier_opt.is_some_and(|qualifier| !self.pattern_type_spelling_names_subject(qualifier, enum_name)) {
            return None;
        }
        let Some(TypeInfo::Enum(enum_info)) = self.lookup_semantic_type_info(enum_name) else {
            return None;
        };
        let canonical_variant = enum_info
            .variant_aliases
            .get(variant_name)
            .map(String::as_str)
            .unwrap_or(variant_name);
        if !enum_info.variants.iter().any(|variant| variant == canonical_variant) {
            return None;
        }
        // A generic enum's payloads are typed under the subject's type arguments, as a generic record's fields are.
        let substitutions = match expected_ty {
            ResolvedType::Generic(_, type_args) => type_param_subst_map(&enum_info.type_params, type_args),
            _ => HashMap::new(),
        };
        Some(
            enum_info
                .variant_fields
                .get(canonical_variant)
                .map(|fields| {
                    fields
                        .iter()
                        .map(|field| substitute_resolved_type(field, &substitutions))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        )
    }

    /// Tuple-variant payload types for `match` patterns on Rust-backed enum surfaces.
    ///
    /// Incan registers [`SymbolKind::Variant`] for source/manifest enums; imported Rust enums and prost-style oneofs
    /// are usually spelled as a `rusttype` / [`TypeInfo::Newtype`] wrapper over [`ResolvedType::RustPath`]. For a
    /// single positional sub-pattern, the payload type is `RustPath("{backing}::{variant}")`, consistent with Rust
    /// field/member path composition in `check_expr::access`. Multiple positional patterns without precise metadata use
    /// [`ResolvedType::Unknown`] per slot.
    ///
    /// When the scrutinee is already [`ResolvedType::RustPath`], any `Type::Variant` prefix in the pattern is not
    /// validated against that path (unlike [`ResolvedType::Named`] rusttypes, where the prefix must match the Incan
    /// type name). Payload typing still uses `{scrutinee_rust_path}::{variant}`.
    fn rust_enum_constructor_payload_types(
        &self,
        expected_ty: &ResolvedType,
        pattern_full_name: &str,
        positional_count: usize,
    ) -> Option<RustEnumPatternResolution> {
        let (expected_ty, borrow) = borrowed_pattern_subject(expected_ty);
        let bind_payload = |field| borrowed_pattern_payload(field, borrow);
        let (enum_qualifier_opt, variant_segment) = match pattern_full_name.rsplit_once("::") {
            Some((e, v)) => (Some(e), v),
            None => (None, pattern_full_name),
        };

        let base_rust_path: String = match expected_ty {
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _) => {
                if let Some(q) = enum_qualifier_opt
                    && q != type_name.as_str()
                {
                    return Some(RustEnumPatternResolution::QualifierMismatch);
                }
                let info = self.lookup_type_info(type_name)?;
                match info {
                    TypeInfo::Newtype(nt) if nt.is_rusttype => match &nt.underlying {
                        ResolvedType::RustPath(p) => p.clone(),
                        _ => return None,
                    },
                    _ => return None,
                }
            }
            ResolvedType::RustPath(p) => p.clone(),
            _ => return None,
        };

        let (metadata_rust_path, _) = self.rust_path_base_and_args(base_rust_path.as_str());
        if let Some(meta) = self.known_rust_metadata_for_path(metadata_rust_path.as_str())
            && let RustItemKind::Type(info) = meta.kind
            && let Some(variant) = info.variants.iter().find(|variant| variant.name == variant_segment)
        {
            let fields: Vec<ResolvedType> = variant
                .fields
                .iter()
                .map(|field| bind_payload(self.resolved_type_from_rust_shape(field)))
                .collect();
            return Some(RustEnumPatternResolution::payloads(fields));
        }

        let fields = match positional_count {
            0 => vec![],
            1 => vec![ResolvedType::RustPath(format!("{base_rust_path}::{variant_segment}"))],
            n => (0..n).map(|_| ResolvedType::Unknown).collect(),
        };
        Some(RustEnumPatternResolution::payloads(
            fields.into_iter().map(bind_payload).collect(),
        ))
    }

    /// Check that a match expression covers all possible cases.
    ///
    /// For enums, `Result`, and `Option`, verifies every variant is handled. Wildcards (`_`) satisfy all remaining
    /// cases. A variant counts as handled only when the unguarded arms that name it also cover its payload, which the
    /// pattern-matrix walk in `match_coverage` decides (#1741). Emits a
    /// [`non_exhaustive_match`](errors::non_exhaustive_match) error naming each missing variant, spelled with wildcard
    /// payloads (`Some(_)`) when arms name it but leave part of its payload uncovered. Every other subject is held to
    /// the same walk and reported as missing `_`: literal arms over an `int` or a `str` never cover it on their own.
    fn check_match_exhaustiveness(&mut self, subject_ty: &ResolvedType, arms: &[Spanned<MatchArm>], span: Span) {
        if let Some(members) = Self::expected_union_members(subject_ty) {
            let mut remaining = members.to_vec();
            let mut option_variants = HashSet::new();
            let mut has_wildcard = false;
            for arm in arms.iter().filter(|arm| arm.node.guard.is_none()) {
                self.remove_covered_union_members(&mut remaining, &arm.node.pattern, subject_ty);
                if subject_ty.is_option() {
                    self.collect_pattern_coverage(
                        &arm.node.pattern.node,
                        subject_ty,
                        &mut option_variants,
                        &mut has_wildcard,
                    );
                }
            }
            let mut missing = remaining.iter().map(ToString::to_string).collect::<Vec<_>>();
            let none = constructors::as_str(ConstructorId::None);
            if subject_ty.is_option() && !has_wildcard && !option_variants.contains(none) {
                missing.push(none.to_string());
            }
            if !missing.is_empty() {
                self.errors.push(errors::non_exhaustive_match(&missing, span));
            }
            return;
        }
        let rows = expand_coverage_heads(
            arms.iter()
                .filter(|arm| arm.node.guard.is_none())
                .map(|arm| vec![Some(&arm.node.pattern.node)])
                .collect(),
        );
        if rows.iter().any(coverage_row_head_is_wild) {
            return;
        }
        let Some(variant_constructors) = self.match_subject_variant_constructors(subject_ty) else {
            // Any other subject (a scalar, a tuple, a model): the arms must cover every value, so arms that are only
            // literals over an open type need a wildcard (#1741).
            if !self.coverage_rows_exhaustive(rows, std::slice::from_ref(subject_ty), 0) {
                self.errors.push(errors::non_exhaustive_match(&["_".to_string()], span));
            }
            return;
        };
        // A variant is covered when the arms that name it cover its payload too: `Some(0)` alone leaves `Some(_)`
        // open, while `Ok(Some(x))` beside `Ok(None)` covers `Ok` (#1741).
        let missing: Vec<String> = variant_constructors
            .iter()
            .filter_map(|constructor| {
                let payload_rows = self.specialize_coverage_rows(&rows, constructor);
                let named = !payload_rows.is_empty();
                (!self.coverage_rows_exhaustive(payload_rows, constructor.payload_types(), 0))
                    .then(|| constructor.missing_label(named))
            })
            .collect();
        if !missing.is_empty() {
            self.errors.push(errors::non_exhaustive_match(&missing, span));
        }
    }

    /// Add the variants covered by a pattern to the match-exhaustiveness accumulator.
    fn collect_pattern_coverage(
        &self,
        pattern: &Pattern,
        subject_ty: &ResolvedType,
        covered: &mut HashSet<String>,
        has_wildcard: &mut bool,
    ) {
        match pattern {
            Pattern::Wildcard | Pattern::Binding(_) => {
                *has_wildcard = true;
            }
            Pattern::Literal(Literal::None) if subject_ty.is_option() => {
                covered.insert(constructors::as_str(ConstructorId::None).to_string());
            }
            Pattern::Constructor(name, _) => {
                let variant_name = if name.node.contains("::") {
                    name.node.split("::").last().unwrap_or(&name.node).to_string()
                } else {
                    name.node.clone()
                };
                covered.insert(variant_name);
            }
            Pattern::Or(alternatives) => {
                for alternative in alternatives {
                    self.collect_pattern_coverage(&alternative.node, subject_ty, covered, has_wildcard);
                }
            }
            Pattern::Group(inner) => {
                self.collect_pattern_coverage(&inner.node, subject_ty, covered, has_wildcard);
            }
            _ => {}
        }
    }
}

enum RustEnumPatternResolution {
    PayloadTypes(Vec<ResolvedType>),
    QualifierMismatch,
}

impl RustEnumPatternResolution {
    fn payloads(fields: Vec<ResolvedType>) -> Self {
        Self::PayloadTypes(fields)
    }
}

/// Whether a statement block cannot complete, because its last statement leaves it: a `return`, `break` or `continue`.
///
/// A `match` arm with such a body produces no value of its own, so it does not take part in unifying the arm types.
fn block_cannot_complete(stmts: &[Spanned<Statement>]) -> bool {
    stmts.last().is_some_and(|stmt| {
        matches!(
            stmt.node,
            Statement::Return(_) | Statement::Break(_) | Statement::Continue
        )
    })
}

impl TypeChecker {
    /// Whether a type is or holds an anonymous union anywhere inside it, as an `Option` payload, a collection element
    /// or a tuple item.
    fn type_holds_union(ty: &ResolvedType) -> bool {
        match ty {
            _ if ty.is_union() => true,
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => args.iter().any(Self::type_holds_union),
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => Self::type_holds_union(inner),
            ResolvedType::FrozenDict(key, value) => Self::type_holds_union(key) || Self::type_holds_union(value),
            _ => false,
        }
    }
}
