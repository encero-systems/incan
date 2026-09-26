//! Match-arm planning for patterns the backend cannot take as written.
//!
//! The backend prints an IR pattern as a Rust pattern and an IR guard as a Rust guard. Three source shapes need more
//! than that:
//!
//! - A `str` literal below the top of a pattern is a `&str` token that an owned `String` position refuses, so it
//!   becomes a binding whose equality test runs in the arm's guard (#1707).
//! - A pattern alternation whose alternatives need tests of their own cannot stay one Rust alternation: one guard
//!   cannot know which alternative matched (#1739). That covers alternatives with nested `str` literals and a top-level
//!   alternation that mixes `str` literals with other patterns.
//! - A pattern alternation under a guard whose alternatives can match the same value. The first alternative that
//!   matches binds the names and the guard runs once for it; when the guard is false the arm is skipped. A Rust
//!   alternation would run the guard again for a later alternative (#1739).
//!
//! [`AstLowering::plan_arm_alternatives`] turns one source pattern into the patterns of the IR arms that carry it, each
//! with the tests its guard runs ahead of the source arm's own guard. A top-level alternation becomes one IR arm per
//! alternative. A nested alternation that binds no names becomes one membership test, so it never multiplies the arm's
//! body. A nested alternation that binds names stays one Rust alternation unless one of its alternatives needs a test
//! that applies to that alternative alone (a nested `str` literal, or a first-match test under a guard): a Rust
//! alternation cannot attach a test to one of its alternatives, so such an alternation is split into combinations.
//!
//! The tests read the matched value without taking it. When the scrutinee is a place, such as a local or a field
//! path, they read that place through a shared view: nested `str` literals, alternations of them, and alternations that
//! bind no names become wildcards tested on the place, so the arm's pattern binds only the names the source spells and
//! a scrutinee used after the `match` keeps every part those names do not take. When the scrutinee is a temporary, they
//! read positions the arm's pattern binds, and a nested `str` literal is hoisted into a binding (#1707); moving a part
//! out of a temporary costs nothing.

use super::super::super::TypedExpr;
use super::super::super::expr::{BinOp, IrExprKind, MatchArm, Pattern, UnaryOp, VarAccess, VarRefKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use super::grouped_unary_operand;
use incan_frontend::ast::{self, Spanned};
use std::collections::VecDeque;

/// Prefix of the binding a `str` literal nested in an arm's own pattern is hoisted into.
const ARM_STRING_BINDING_PREFIX: &str = "__incan_match_str_";
/// Prefix of the binding that captures one position of the matched value for a test in the arm's guard.
const POSITION_BINDING_PREFIX: &str = "__incan_match_pos_";
/// Prefix of the binding a `str` literal inside a membership test is hoisted into.
const MEMBER_STRING_BINDING_PREFIX: &str = "__incan_match_member_";

/// Whether a pattern matches, as far as one IR arm's guard can tell: decided without a test, or by one.
enum PatternTest {
    /// The pattern cannot match wherever the arm's own pattern matches.
    Never,
    /// The pattern matches wherever the arm's own pattern matches.
    Always,
    /// The pattern matches exactly when this boolean test holds.
    Test(Box<TypedExpr>),
}

impl PatternTest {
    /// Whether either of two pattern tests holds.
    fn or(self, other: PatternTest) -> PatternTest {
        match (self, other) {
            (PatternTest::Always, _) | (_, PatternTest::Always) => PatternTest::Always,
            (PatternTest::Never, test) | (test, PatternTest::Never) => test,
            (PatternTest::Test(left), PatternTest::Test(right)) => {
                PatternTest::Test(Box::new(bool_binop(BinOp::Or, *left, *right)))
            }
        }
    }

    /// Whether both of two pattern tests hold.
    fn and(self, other: PatternTest) -> PatternTest {
        match (self, other) {
            (PatternTest::Never, _) | (_, PatternTest::Never) => PatternTest::Never,
            (PatternTest::Always, test) | (test, PatternTest::Always) => test,
            (PatternTest::Test(left), PatternTest::Test(right)) => {
                PatternTest::Test(Box::new(bool_binop(BinOp::And, *left, *right)))
            }
        }
    }

    /// Record this test as one condition of an arm's guard, where the arm's pattern already holds.
    fn push_into(self, tests: &mut Vec<TypedExpr>) {
        match self {
            PatternTest::Always => {}
            PatternTest::Never => tests.push(bool_literal(false)),
            PatternTest::Test(test) => tests.push(*test),
        }
    }
}

/// Fresh names for the bindings one IR arm's tests introduce.
///
/// Every IR arm is its own scope, so the numbering restarts per arm.
#[derive(Default)]
struct ArmTestNames {
    positions: usize,
    members: usize,
}

impl ArmTestNames {
    /// The next binding name for a captured position of the matched value.
    fn position(&mut self) -> String {
        let name = format!("{POSITION_BINDING_PREFIX}{}", self.positions);
        self.positions += 1;
        name
    }

    /// The next binding name for a `str` literal hoisted inside a membership test.
    fn member(&mut self) -> String {
        let name = format!("{MEMBER_STRING_BINDING_PREFIX}{}", self.members);
        self.members += 1;
        name
    }
}

/// A boolean operation over two boolean tests.
fn bool_binop(op: BinOp, left: TypedExpr, right: TypedExpr) -> TypedExpr {
    TypedExpr::new(
        IrExprKind::BinOp {
            op,
            left: Box::new(left),
            right: Box::new(right),
        },
        IrType::Bool,
    )
}

/// A boolean literal.
fn bool_literal(value: bool) -> TypedExpr {
    TypedExpr::new(IrExprKind::Bool(value), IrType::Bool)
}

/// The negation of a boolean test, with an operator- or `match`-shaped operand grouped so `not` covers all of it.
fn negated(test: TypedExpr) -> TypedExpr {
    TypedExpr::new(
        IrExprKind::UnaryOp {
            op: UnaryOp::Not,
            operand: Box::new(grouped_unary_operand(test)),
        },
        IrType::Bool,
    )
}

/// A read of a binding the arm's pattern introduced, for a test in the arm's guard.
fn binding_var(binding: &str) -> TypedExpr {
    TypedExpr::new(
        IrExprKind::Var {
            name: binding.to_string(),
            access: VarAccess::Read,
            ref_kind: VarRefKind::Value,
        },
        IrType::Unknown,
    )
}

/// A shared view of `value` (a place or a binding of the arm), for a membership test in the arm's guard.
///
/// The test inspects the value without taking it: a guard only has shared access to the matched value and to the
/// arm's bindings, and the arm's body, or code after the `match`, still owns them. The view's type is left open so the
/// backend prints the view as written.
fn view_of(value: &TypedExpr) -> TypedExpr {
    TypedExpr::new(
        IrExprKind::UnaryOp {
            op: UnaryOp::Ref,
            operand: Box::new(value.clone()),
        },
        IrType::Unknown,
    )
}

/// The equality test of the `str` value `value` against the literal `literal`, recorded like a source `value == "…"`.
fn string_equality(value: &TypedExpr, literal: &str) -> TypedExpr {
    let mut left = value.clone();
    left.ty = IrType::String;
    let right = TypedExpr::new(IrExprKind::String(literal.to_string()), IrType::String);
    bool_binop(BinOp::Eq, left, right)
}

/// A later alternative's position holds a whole alternation that binds names, which the first-match test cannot read,
/// so the alternative must be split at that alternation first.
struct NeedsSplit;

/// The last `::` segment of a variant path, which names the variant whatever its qualification.
fn variant_name(path: &str) -> &str {
    path.rsplit_once("::").map_or(path, |(_, name)| name)
}

impl AstLowering {
    /// Plan the IR arms that carry one source arm's pattern: each IR arm's pattern and the tests its guard runs ahead
    /// of the source arm's own guard, in the order the arms must be tried.
    ///
    /// `guarded` says whether the source arm has a guard. Under a guard, an IR arm for a later alternative also tests
    /// that no earlier alternative that could match the same value did match: the first alternative that matches
    /// decides the arm, and the guard runs once, for that alternative's names. When the guard is false, matching moves
    /// on to the next source arm. Alternatives that cannot match the same value, such as `(0, "a") | (1, "b")`, need no
    /// such test. `scrutinee` is the matched value; when it is a place the tests read it directly (see the module
    /// documentation).
    ///
    /// A pattern that needs none of this comes back as its single alternative with no test.
    pub(in crate::lower) fn plan_arm_alternatives(
        pattern: Pattern,
        guarded: bool,
        scrutinee: &TypedExpr,
    ) -> Vec<(Pattern, Option<TypedExpr>)> {
        let subject = Self::readable_subject(scrutinee, &pattern);
        let mut pending: VecDeque<(Pattern, bool)> = Self::split_top_level_alternation(pattern, guarded)
            .into_iter()
            .flat_map(|alternative| Self::split_binding_alternations(alternative, guarded))
            .map(|alternative| (alternative, true))
            .collect();
        let mut earlier = Vec::new();
        let mut planned = Vec::new();
        while let Some((alternative, may_split)) = pending.pop_front() {
            match Self::plan_alternative(&alternative, &earlier, guarded, subject, may_split) {
                Ok(arm) => {
                    planned.push(arm);
                    earlier.push(alternative);
                }
                Err(NeedsSplit) => {
                    for piece in Self::split_every_binding_alternation(alternative).into_iter().rev() {
                        pending.push_front((piece, false));
                    }
                }
            }
        }
        planned
    }

    /// Plan one IR arm for `alternative`, given the `earlier` alternatives of the same source arm.
    ///
    /// Fails with [`NeedsSplit`] when a first-match test must read a position of `alternative` that holds a whole
    /// alternation binding names; the caller splits that alternation and plans the pieces, with `may_split` false so a
    /// piece that still cannot be read falls back to no test rather than splitting again.
    fn plan_alternative(
        alternative: &Pattern,
        earlier: &[Pattern],
        guarded: bool,
        subject: Option<&TypedExpr>,
        may_split: bool,
    ) -> Result<(Pattern, Option<TypedExpr>), NeedsSplit> {
        let mut names = ArmTestNames::default();
        let mut tests = Vec::new();
        let mut pattern = match subject {
            Some(subject) => {
                let mut skeletons = Vec::new();
                let pattern =
                    Self::drop_guard_tested_positions(alternative.clone(), guarded, &|pattern| pattern, &mut skeletons);
                for skeleton in &skeletons {
                    Self::member_test(skeleton, subject, &mut names).push_into(&mut tests);
                }
                pattern
            }
            None => Self::bind_nameless_alternations(alternative.clone(), guarded, &mut names, &mut tests),
        };
        if guarded {
            for earlier in earlier {
                if !Self::patterns_may_overlap(earlier, alternative) {
                    continue;
                }
                let test = match subject {
                    Some(subject) => Self::member_test(earlier, subject, &mut names),
                    None => match Self::earlier_alternative_test(earlier, &mut pattern, &mut names, &mut tests) {
                        Ok(test) => test,
                        Err(NeedsSplit) if may_split => return Err(NeedsSplit),
                        Err(NeedsSplit) => PatternTest::Never,
                    },
                };
                match test {
                    PatternTest::Never => {}
                    PatternTest::Always => tests.push(bool_literal(false)),
                    PatternTest::Test(test) => tests.push(negated(*test)),
                }
            }
        }
        let mut next_string = 0;
        let (pattern, literal_guard) =
            Self::hoist_nested_string_literals(pattern, ARM_STRING_BINDING_PREFIX, &mut next_string);
        let test = tests.into_iter().fold(literal_guard, |joined, test| {
            Self::conjoin_match_guards(joined, Some(test))
        });
        Ok((pattern, test))
    }

    /// The scrutinee, when it is a place an arm's guard can read and no alternative of `pattern` rebinds its root name.
    ///
    /// A place is a local, a parameter, or a field path starting at one. A binding of the same name in the pattern
    /// would shadow the place inside the guard, so such a pattern reads positions instead.
    fn readable_subject<'a>(scrutinee: &'a TypedExpr, pattern: &Pattern) -> Option<&'a TypedExpr> {
        let root = Self::place_root(scrutinee)?;
        (!Self::pattern_binds_name(pattern, root)).then_some(scrutinee)
    }

    /// The local at the root of a place expression: a local or parameter read, or a field path starting at one.
    fn place_root(expr: &TypedExpr) -> Option<&str> {
        match &expr.kind {
            IrExprKind::Var {
                name,
                ref_kind: VarRefKind::Value,
                ..
            } => Some(name),
            IrExprKind::Field { object, .. } => Self::place_root(object),
            _ => None,
        }
    }

    /// Build the IR arms for one source arm from its planned `alternatives`.
    ///
    /// Each arm runs its alternative's tests ahead of the source arm's own `guard`, so the guard runs only for the
    /// alternative that decides the arm, and each arm carries a copy of the one lowered `body`. The arms are tried in
    /// order and at most one body runs, so the copies never both execute and a value the body consumes is consumed
    /// once.
    pub(in crate::lower) fn match_arms_for_alternatives(
        alternatives: Vec<(Pattern, Option<TypedExpr>)>,
        guard: Option<TypedExpr>,
        body: TypedExpr,
    ) -> Vec<MatchArm> {
        alternatives
            .into_iter()
            .map(|(pattern, test)| MatchArm {
                pattern,
                bindings: Vec::new(),
                guard: Self::conjoin_match_guards(test, guard.clone()),
                body: body.clone(),
            })
            .collect()
    }

    /// Lower an arm's own guard, noting whether the backend may evaluate the lowered guard more than once.
    ///
    /// That is the case for a guard copied onto several IR arms, and for a guard after a pattern alternation, which
    /// the backend's alternation evaluates once per alternative it tries. Such a guard is lowered as code that may run
    /// more than once: none of its reads consumes the value it reads, or a later evaluation would find it gone.
    pub(in crate::lower) fn lower_match_arm_guard(
        &mut self,
        guard: Option<&Spanned<ast::Expr>>,
        may_repeat: bool,
    ) -> Result<Option<TypedExpr>, LoweringError> {
        let Some(guard) = guard else {
            return Ok(None);
        };
        if !may_repeat {
            return self.lower_expr_spanned(guard).map(Some);
        }
        self.non_linear_context_depth += 1;
        let lowered = self.lower_expr_spanned(guard);
        self.non_linear_context_depth -= 1;
        lowered.map(Some)
    }

    /// Whether `pattern` holds a pattern alternation anywhere, at its top or nested.
    pub(in crate::lower) fn pattern_holds_alternation(pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Or(_) => true,
            Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } => {
                items.iter().any(Self::pattern_holds_alternation)
            }
            Pattern::Struct { fields, .. } => fields.iter().any(|(_, item)| Self::pattern_holds_alternation(item)),
            Pattern::Wildcard | Pattern::Var(_) | Pattern::Literal(_) => false,
        }
    }

    // ============================================================================
    // Splitting alternations into arms
    // ============================================================================

    /// Split a top-level alternation into its alternatives when it cannot stay one Rust alternation, flattening
    /// alternations grouped inside it.
    fn split_top_level_alternation(pattern: Pattern, guarded: bool) -> Vec<Pattern> {
        match pattern {
            Pattern::Or(items) if Self::top_level_alternation_needs_arms(&items, guarded) => items
                .into_iter()
                .flat_map(|item| Self::split_top_level_alternation(item, guarded))
                .collect(),
            other => vec![other],
        }
    }

    /// Whether a top-level alternation needs one arm per alternative.
    ///
    /// It does when an alternative nests a `str` literal (its test belongs to that alternative's arm alone), when it
    /// mixes `str` literals with other patterns (the backend matches a `str` value against literals only when every
    /// alternative is one), and under a guard when alternatives, or an alternation inside one of them, can match the
    /// same value.
    fn top_level_alternation_needs_arms(items: &[Pattern], guarded: bool) -> bool {
        items.iter().any(Self::pattern_nests_string_literal)
            || (items.iter().any(Self::is_string_literal_alternation)
                && !items.iter().all(Self::is_string_literal_alternation))
            || (guarded && Self::alternation_overlaps(items))
    }

    /// Whether an alternation below the top of a pattern needs tests the backend's alternation cannot run.
    ///
    /// An alternation of `str` literals only never does: it becomes one binding tested against each literal. Any other
    /// alternation that holds a `str` literal does, and so does one whose alternatives can match the same value when
    /// the arm has a guard.
    fn nested_alternation_needs_tests(items: &[Pattern], guarded: bool) -> bool {
        !Self::all_string_literals(items)
            && (items.iter().any(Self::pattern_contains_string_literal)
                || (guarded && Self::alternation_overlaps(items)))
    }

    /// Split the nested alternations that bind names and need tests into separate patterns, returning every
    /// combination in the order the alternations try them.
    ///
    /// Such an alternation has an alternative whose test applies to it alone, which a Rust alternation cannot carry,
    /// and its names come from whichever alternative matched, so the test cannot move out of the alternation either. A
    /// nested alternation that binds no names stays whole here; it becomes a membership test instead.
    fn split_binding_alternations(pattern: Pattern, guarded: bool) -> Vec<Pattern> {
        match pattern {
            Pattern::Or(items)
                if items.iter().any(Self::pattern_binds_names)
                    && Self::nested_alternation_needs_tests(&items, guarded) =>
            {
                items
                    .into_iter()
                    .flat_map(|item| Self::split_binding_alternations(item, guarded))
                    .collect()
            }
            other => Self::structural_combinations(other, &|item| Self::split_binding_alternations(item, guarded)),
        }
    }

    /// Split every nested alternation that binds names, returning every combination in the order the alternations try
    /// them; used when a first-match test must read a position such an alternation holds.
    fn split_every_binding_alternation(pattern: Pattern) -> Vec<Pattern> {
        match pattern {
            Pattern::Or(items) if items.iter().any(Self::pattern_binds_names) => items
                .into_iter()
                .flat_map(Self::split_every_binding_alternation)
                .collect(),
            other => Self::structural_combinations(other, &Self::split_every_binding_alternation),
        }
    }

    /// Split every alternation that holds a `str` literal alongside other patterns, for the arms of a membership test.
    ///
    /// A membership test's arms only answer `true`, so splitting them copies no body.
    fn split_every_string_alternation(pattern: Pattern) -> Vec<Pattern> {
        match pattern {
            Pattern::Or(items)
                if items.iter().any(Self::pattern_contains_string_literal) && !Self::all_string_literals(&items) =>
            {
                items
                    .into_iter()
                    .flat_map(Self::split_every_string_alternation)
                    .collect()
            }
            other => Self::structural_combinations(other, &Self::split_every_string_alternation),
        }
    }

    /// Rebuild a tuple, struct or enum pattern once for every combination of its items' `split` alternatives, the first
    /// item varying slowest; any other pattern comes back as it is.
    fn structural_combinations(pattern: Pattern, split: &dyn Fn(Pattern) -> Vec<Pattern>) -> Vec<Pattern> {
        match pattern {
            Pattern::Tuple(items) => Self::item_combinations(items, split)
                .into_iter()
                .map(Pattern::Tuple)
                .collect(),
            Pattern::Struct { name, fields, rest } => {
                let (field_names, items): (Vec<String>, Vec<Pattern>) = fields.into_iter().unzip();
                Self::item_combinations(items, split)
                    .into_iter()
                    .map(|items| Pattern::Struct {
                        name: name.clone(),
                        fields: field_names.iter().cloned().zip(items).collect(),
                        rest,
                    })
                    .collect()
            }
            Pattern::Enum { name, variant, fields } => Self::item_combinations(fields, split)
                .into_iter()
                .map(|fields| Pattern::Enum {
                    name: name.clone(),
                    variant: variant.clone(),
                    fields,
                })
                .collect(),
            other => vec![other],
        }
    }

    /// Every combination of the items' `split` alternatives, the first item varying slowest.
    ///
    /// Items that do not split contribute their one pattern, so items with nothing to split come back as their single
    /// original list.
    fn item_combinations(items: Vec<Pattern>, split: &dyn Fn(Pattern) -> Vec<Pattern>) -> Vec<Vec<Pattern>> {
        let mut combinations: Vec<Vec<Pattern>> = vec![Vec::new()];
        for item in items {
            let alternatives = split(item);
            let mut extended = Vec::with_capacity(combinations.len() * alternatives.len());
            for prefix in &combinations {
                for alternative in &alternatives {
                    let mut combination = prefix.clone();
                    combination.push(alternative.clone());
                    extended.push(combination);
                }
            }
            combinations = extended;
        }
        combinations
    }

    // ============================================================================
    // Membership tests
    // ============================================================================

    /// Replace each nested alternation that binds no names and needs tests with a binding of its position, pushing the
    /// alternation's membership test on that binding into `tests`.
    ///
    /// The arm keeps one pattern and one body however many such alternations it holds, and the test runs once.
    fn bind_nameless_alternations(
        pattern: Pattern,
        guarded: bool,
        names: &mut ArmTestNames,
        tests: &mut Vec<TypedExpr>,
    ) -> Pattern {
        match pattern {
            Pattern::Tuple(items) => Pattern::Tuple(
                items
                    .into_iter()
                    .map(|item| Self::bind_nameless_alternation(item, guarded, names, tests))
                    .collect(),
            ),
            Pattern::Struct { name, fields, rest } => Pattern::Struct {
                name,
                fields: fields
                    .into_iter()
                    .map(|(field, item)| (field, Self::bind_nameless_alternation(item, guarded, names, tests)))
                    .collect(),
                rest,
            },
            Pattern::Enum { name, variant, fields } => Pattern::Enum {
                name,
                variant,
                fields: fields
                    .into_iter()
                    .map(|item| Self::bind_nameless_alternation(item, guarded, names, tests))
                    .collect(),
            },
            other => other,
        }
    }

    /// Rewrite one nested sub-pattern for [`Self::bind_nameless_alternations`].
    fn bind_nameless_alternation(
        pattern: Pattern,
        guarded: bool,
        names: &mut ArmTestNames,
        tests: &mut Vec<TypedExpr>,
    ) -> Pattern {
        match pattern {
            Pattern::Or(items)
                if !items.iter().any(Self::pattern_binds_names)
                    && Self::nested_alternation_needs_tests(&items, guarded) =>
            {
                let binding = names.position();
                Self::member_test(&Pattern::Or(items), &binding_var(&binding), names).push_into(tests);
                Pattern::Var(binding)
            }
            other @ Pattern::Or(_) => other,
            other => Self::bind_nameless_alternations(other, guarded, names, tests),
        }
    }

    /// Replace each nested position the arm's guard tests on the place instead of binding it with a wildcard,
    /// collecting for each the pattern that tests it on the whole matched value: the position's sub-pattern, with a
    /// wildcard everywhere else.
    ///
    /// Those positions are a `str` literal, an alternation of `str` literals, and an alternation that binds no names
    /// and needs tests. Hoisting a `str` literal into a binding would move that part out of the place, so a scrutinee
    /// used after the `match` would lose it. `focus` rebuilds the test pattern around a sub-pattern at the current
    /// position. A struct position keeps only its own field and a rest marker, so the test names no other field.
    fn drop_guard_tested_positions(
        pattern: Pattern,
        guarded: bool,
        focus: &dyn Fn(Pattern) -> Pattern,
        skeletons: &mut Vec<Pattern>,
    ) -> Pattern {
        match pattern {
            Pattern::Tuple(items) => {
                let arity = items.len();
                Pattern::Tuple(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(index, item)| {
                            let focus_item = |inner: Pattern| {
                                let mut inner = Some(inner);
                                focus(Pattern::Tuple(
                                    (0..arity)
                                        .map(|position| match position == index {
                                            true => inner.take().unwrap_or(Pattern::Wildcard),
                                            false => Pattern::Wildcard,
                                        })
                                        .collect(),
                                ))
                            };
                            Self::drop_guard_tested_position(item, guarded, &focus_item, skeletons)
                        })
                        .collect(),
                )
            }
            Pattern::Struct { name, fields, rest } => Pattern::Struct {
                fields: fields
                    .into_iter()
                    .map(|(field, item)| {
                        let focus_item = |inner: Pattern| {
                            focus(Pattern::Struct {
                                name: name.clone(),
                                fields: vec![(field.clone(), inner)],
                                rest: true,
                            })
                        };
                        let item = Self::drop_guard_tested_position(item, guarded, &focus_item, skeletons);
                        (field, item)
                    })
                    .collect(),
                name,
                rest,
            },
            Pattern::Enum { name, variant, fields } => {
                let arity = fields.len();
                Pattern::Enum {
                    fields: fields
                        .into_iter()
                        .enumerate()
                        .map(|(index, item)| {
                            let focus_item = |inner: Pattern| {
                                let mut inner = Some(inner);
                                focus(Pattern::Enum {
                                    name: name.clone(),
                                    variant: variant.clone(),
                                    fields: (0..arity)
                                        .map(|position| match position == index {
                                            true => inner.take().unwrap_or(Pattern::Wildcard),
                                            false => Pattern::Wildcard,
                                        })
                                        .collect(),
                                })
                            };
                            Self::drop_guard_tested_position(item, guarded, &focus_item, skeletons)
                        })
                        .collect(),
                    name,
                    variant,
                }
            }
            other => other,
        }
    }

    /// Rewrite one nested sub-pattern for [`Self::drop_guard_tested_positions`].
    fn drop_guard_tested_position(
        pattern: Pattern,
        guarded: bool,
        focus: &dyn Fn(Pattern) -> Pattern,
        skeletons: &mut Vec<Pattern>,
    ) -> Pattern {
        match pattern {
            Pattern::Or(items)
                if Self::all_string_literals(&items)
                    || (!items.iter().any(Self::pattern_binds_names)
                        && Self::nested_alternation_needs_tests(&items, guarded)) =>
            {
                skeletons.push(focus(Pattern::Or(items)));
                Pattern::Wildcard
            }
            other @ Pattern::Or(_) => other,
            literal @ Pattern::Literal(_) if Self::is_string_literal_pattern(&literal) => {
                skeletons.push(focus(literal));
                Pattern::Wildcard
            }
            other => Self::drop_guard_tested_positions(other, guarded, focus, skeletons),
        }
    }

    /// Test whether `value` (a place, or a binding of the arm) matches `pattern`, whose own names play no part.
    ///
    /// A `str` literal, or an alternation of them, is an equality test. Anything else is a `match` over a shared view
    /// of the value with one arm per alternative answering `true` and a final arm answering `false`.
    fn member_test(pattern: &Pattern, value: &TypedExpr, names: &mut ArmTestNames) -> PatternTest {
        match pattern {
            Pattern::Wildcard | Pattern::Var(_) => return PatternTest::Always,
            Pattern::Literal(_) | Pattern::Or(_) if Self::is_string_literal_alternation(pattern) => {
                return Self::string_literal_alternation_test(value, pattern);
            }
            _ => {}
        }

        let mut arms = Vec::new();
        for alternative in Self::split_every_string_alternation(Self::erase_pattern_bindings(pattern)) {
            if matches!(alternative, Pattern::Wildcard) {
                return PatternTest::Always;
            }
            let (arm_pattern, guard) = if Self::is_string_literal_alternation(&alternative) {
                let member = names.member();
                let test = Self::string_literal_alternation_test(&binding_var(&member), &alternative);
                (Pattern::Var(member), Self::pattern_test_guard(test))
            } else {
                Self::hoist_nested_string_literals(alternative, MEMBER_STRING_BINDING_PREFIX, &mut names.members)
            };
            arms.push(MatchArm {
                pattern: arm_pattern,
                bindings: Vec::new(),
                guard,
                body: bool_literal(true),
            });
        }
        arms.push(MatchArm {
            pattern: Pattern::Wildcard,
            bindings: Vec::new(),
            guard: None,
            body: bool_literal(false),
        });
        PatternTest::Test(Box::new(TypedExpr::new(
            IrExprKind::Match {
                scrutinee: Box::new(view_of(value)),
                arms,
            },
            IrType::Bool,
        )))
    }

    /// The equality test of the `str` value `value` against a `str` literal, or against each literal of an alternation
    /// of them joined with `or`.
    fn string_literal_alternation_test(value: &TypedExpr, pattern: &Pattern) -> PatternTest {
        let literals = match pattern {
            Pattern::Or(items) => items.as_slice(),
            single => std::slice::from_ref(single),
        };
        literals
            .iter()
            .filter_map(|literal| match literal {
                Pattern::Literal(literal) => match &literal.kind {
                    IrExprKind::String(literal) => Some(PatternTest::Test(Box::new(string_equality(value, literal)))),
                    _ => None,
                },
                _ => None,
            })
            .fold(PatternTest::Never, PatternTest::or)
    }

    /// The guard an arm carries for a pattern test: none when the test always holds.
    fn pattern_test_guard(test: PatternTest) -> Option<TypedExpr> {
        match test {
            PatternTest::Always => None,
            PatternTest::Never => Some(bool_literal(false)),
            PatternTest::Test(test) => Some(*test),
        }
    }

    // ============================================================================
    // First match under a guard
    // ============================================================================

    /// Test, in the guard of the arm whose pattern is `current`, whether the `earlier` alternative also matches the
    /// value.
    ///
    /// The two patterns are walked together. Where both have the same structure the test descends into it; where they
    /// name different variants, tuple lengths or literals the earlier alternative cannot match. Where `current` has a
    /// name or a wildcard, or a sub-pattern without names, that position is bound (a wildcard or a sub-pattern becomes
    /// a fresh binding, the sub-pattern's own test moving into `tests`) and the earlier alternative's sub-pattern is
    /// tested on the binding. A position that holds a whole alternation binding names cannot be bound: the test fails
    /// with [`NeedsSplit`] so the alternation is split first. Any other position `current` cannot bind counts as one
    /// the earlier alternative does not match.
    fn earlier_alternative_test(
        earlier: &Pattern,
        current: &mut Pattern,
        names: &mut ArmTestNames,
        tests: &mut Vec<TypedExpr>,
    ) -> Result<PatternTest, NeedsSplit> {
        match earlier {
            Pattern::Wildcard | Pattern::Var(_) => return Ok(PatternTest::Always),
            Pattern::Or(items) => {
                let mut any = PatternTest::Never;
                for item in items {
                    any = any.or(Self::earlier_alternative_test(item, current, names, tests)?);
                }
                return Ok(any);
            }
            _ => {}
        }
        if let Some(decided) = Self::decided_without_test(current, earlier) {
            return Ok(decided);
        }
        match (current, earlier) {
            (Pattern::Tuple(actual), Pattern::Tuple(expected)) if actual.len() == expected.len() => {
                let mut all = PatternTest::Always;
                for (actual, expected) in actual.iter_mut().zip(expected) {
                    all = all.and(Self::earlier_alternative_test(expected, actual, names, tests)?);
                }
                Ok(all)
            }
            (
                Pattern::Enum {
                    variant: actual_variant,
                    fields: actual,
                    ..
                },
                Pattern::Enum {
                    variant: expected_variant,
                    fields: expected,
                    ..
                },
            ) if variant_name(actual_variant) == variant_name(expected_variant) && actual.len() == expected.len() => {
                let mut all = PatternTest::Always;
                for (actual, expected) in actual.iter_mut().zip(expected) {
                    all = all.and(Self::earlier_alternative_test(expected, actual, names, tests)?);
                }
                Ok(all)
            }
            (Pattern::Struct { fields: actual, .. }, Pattern::Struct { fields: expected, .. }) => {
                let mut all = PatternTest::Always;
                for (field, expected) in expected {
                    let index = match actual.iter().position(|(name, _)| name == field) {
                        Some(index) => index,
                        None => {
                            actual.push((field.clone(), Pattern::Wildcard));
                            actual.len() - 1
                        }
                    };
                    all = all.and(Self::earlier_alternative_test(
                        expected,
                        &mut actual[index].1,
                        names,
                        tests,
                    )?);
                }
                Ok(all)
            }
            (current, _) => {
                let alternation = matches!(current, Pattern::Or(_));
                match Self::bind_position(current, names, tests) {
                    Some(binding) => Ok(Self::member_test(earlier, &binding_var(&binding), names)),
                    None if alternation => Err(NeedsSplit),
                    None => Ok(PatternTest::Never),
                }
            }
        }
    }

    /// The outcome of [`Self::earlier_alternative_test`] when the two patterns decide it without a test: literals
    /// that are or are not equal, `None` against `Some(...)` or `None`, and different variants or tuple lengths.
    fn decided_without_test(current: &Pattern, earlier: &Pattern) -> Option<PatternTest> {
        let known = |matches: bool| {
            if matches {
                PatternTest::Always
            } else {
                PatternTest::Never
            }
        };
        match (current, earlier) {
            (Pattern::Literal(actual), Pattern::Literal(expected)) => Self::literals_equal(actual, expected).map(known),
            (Pattern::Literal(literal), Pattern::Enum { variant, fields, .. })
            | (Pattern::Enum { variant, fields, .. }, Pattern::Literal(literal)) => {
                Self::literal_matches_variant(literal, variant, fields).map(known)
            }
            (Pattern::Tuple(actual), Pattern::Tuple(expected)) if actual.len() != expected.len() => {
                Some(PatternTest::Never)
            }
            (
                Pattern::Enum {
                    variant: actual_variant,
                    fields: actual,
                    ..
                },
                Pattern::Enum {
                    variant: expected_variant,
                    fields: expected,
                    ..
                },
            ) if variant_name(actual_variant) != variant_name(expected_variant) || actual.len() != expected.len() => {
                Some(PatternTest::Never)
            }
            _ => None,
        }
    }

    /// Bind the position `current` stands for so a test can inspect it, returning the binding's name.
    ///
    /// A name is used as it is. A wildcard, or a sub-pattern that binds no names, becomes a fresh binding; the
    /// sub-pattern's own test moves into `tests`, so the arm still matches exactly what it matched. A sub-pattern that
    /// binds names cannot be replaced and yields `None`.
    fn bind_position(current: &mut Pattern, names: &mut ArmTestNames, tests: &mut Vec<TypedExpr>) -> Option<String> {
        if let Pattern::Var(name) = current {
            return Some(name.clone());
        }
        if Self::pattern_binds_names(current) {
            return None;
        }
        let binding = names.position();
        let replaced = std::mem::replace(current, Pattern::Var(binding.clone()));
        Self::member_test(&replaced, &binding_var(&binding), names).push_into(tests);
        Some(binding)
    }

    /// Whether two patterns can match one value; `true` whenever that cannot be ruled out from the patterns alone.
    fn patterns_may_overlap(left: &Pattern, right: &Pattern) -> bool {
        match (left, right) {
            (Pattern::Wildcard | Pattern::Var(_), _) | (_, Pattern::Wildcard | Pattern::Var(_)) => true,
            (Pattern::Or(items), other) | (other, Pattern::Or(items)) => {
                items.iter().any(|item| Self::patterns_may_overlap(item, other))
            }
            (Pattern::Literal(left), Pattern::Literal(right)) => Self::literals_equal(left, right) != Some(false),
            (Pattern::Tuple(left), Pattern::Tuple(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| Self::patterns_may_overlap(left, right))
            }
            (
                Pattern::Enum {
                    variant: left_variant,
                    fields: left,
                    ..
                },
                Pattern::Enum {
                    variant: right_variant,
                    fields: right,
                    ..
                },
            ) => {
                variant_name(left_variant) == variant_name(right_variant)
                    && left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| Self::patterns_may_overlap(left, right))
            }
            (Pattern::Struct { fields: left, .. }, Pattern::Struct { fields: right, .. }) => {
                left.iter().all(|(field, left)| {
                    right
                        .iter()
                        .find(|(other, _)| other == field)
                        .is_none_or(|(_, right)| Self::patterns_may_overlap(left, right))
                })
            }
            (Pattern::Literal(literal), Pattern::Enum { variant, fields, .. })
            | (Pattern::Enum { variant, fields, .. }, Pattern::Literal(literal)) => {
                Self::literal_matches_variant(literal, variant, fields) != Some(false)
            }
            _ => true,
        }
    }

    /// Whether two alternatives of an alternation can match one value, or an alternation nested in one of them can.
    fn alternation_overlaps(items: &[Pattern]) -> bool {
        items.iter().enumerate().any(|(index, item)| {
            items[..index]
                .iter()
                .any(|earlier| Self::patterns_may_overlap(earlier, item))
        }) || items.iter().any(Self::holds_overlapping_alternation)
    }

    /// Whether `pattern` holds an alternation, other than one of `str` literals only, whose alternatives can match one
    /// value.
    fn holds_overlapping_alternation(pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Or(items) => !Self::all_string_literals(items) && Self::alternation_overlaps(items),
            Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } => {
                items.iter().any(Self::holds_overlapping_alternation)
            }
            Pattern::Struct { fields, .. } => fields.iter().any(|(_, item)| Self::holds_overlapping_alternation(item)),
            Pattern::Wildcard | Pattern::Var(_) | Pattern::Literal(_) => false,
        }
    }

    /// Whether two literal patterns match the same constant; `None` when their kinds cannot be compared here.
    fn literals_equal(left: &TypedExpr, right: &TypedExpr) -> Option<bool> {
        match (&left.kind, &right.kind) {
            (IrExprKind::Int(left), IrExprKind::Int(right)) => Some(left == right),
            (IrExprKind::IntLiteral(left), IrExprKind::IntLiteral(right)) => Some(left == right),
            (IrExprKind::Bool(left), IrExprKind::Bool(right)) => Some(left == right),
            (IrExprKind::String(left), IrExprKind::String(right)) => Some(left == right),
            (IrExprKind::None, IrExprKind::None) | (IrExprKind::Unit, IrExprKind::Unit) => Some(true),
            _ => None,
        }
    }

    /// Whether a literal pattern and a variant pattern match the same value: a `None` literal matches `None` and not
    /// `Some(...)`; `None` for anything else.
    fn literal_matches_variant(literal: &TypedExpr, variant: &str, fields: &[Pattern]) -> Option<bool> {
        if !matches!(literal.kind, IrExprKind::None) {
            return None;
        }
        match variant_name(variant) {
            "None" => Some(fields.is_empty()),
            "Some" => Some(false),
            _ => None,
        }
    }

    // ============================================================================
    // Pattern shape queries
    // ============================================================================

    /// Whether `pattern` is a string literal pattern.
    fn is_string_literal_pattern(pattern: &Pattern) -> bool {
        matches!(pattern, Pattern::Literal(literal) if matches!(literal.kind, IrExprKind::String(_)))
    }

    /// Whether every alternative in `items` is a string literal pattern (and there is at least one).
    fn all_string_literals(items: &[Pattern]) -> bool {
        !items.is_empty() && items.iter().all(Self::is_string_literal_pattern)
    }

    /// Whether `pattern` is a string literal or an alternation of string literals only.
    fn is_string_literal_alternation(pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Or(items) => Self::all_string_literals(items),
            other => Self::is_string_literal_pattern(other),
        }
    }

    /// Whether `pattern` is, or contains at any depth, a string literal pattern.
    fn pattern_contains_string_literal(pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Literal(_) => Self::is_string_literal_pattern(pattern),
            Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } | Pattern::Or(items) => {
                items.iter().any(Self::pattern_contains_string_literal)
            }
            Pattern::Struct { fields, .. } => fields
                .iter()
                .any(|(_, item)| Self::pattern_contains_string_literal(item)),
            Pattern::Wildcard | Pattern::Var(_) => false,
        }
    }

    /// Whether a string literal sits below the top of `pattern`: inside one of its tuple, struct or enum patterns.
    ///
    /// A top-level string literal, or an alternation of them, does not count: the backend matches those against a
    /// `str` scrutinee directly.
    fn pattern_nests_string_literal(pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } => {
                items.iter().any(Self::pattern_contains_string_literal)
            }
            Pattern::Struct { fields, .. } => fields
                .iter()
                .any(|(_, item)| Self::pattern_contains_string_literal(item)),
            Pattern::Or(items) => items.iter().any(Self::pattern_nests_string_literal),
            Pattern::Wildcard | Pattern::Var(_) | Pattern::Literal(_) => false,
        }
    }

    /// Whether `pattern` binds a name anywhere.
    fn pattern_binds_names(pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Var(_) => true,
            Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } | Pattern::Or(items) => {
                items.iter().any(Self::pattern_binds_names)
            }
            Pattern::Struct { fields, .. } => fields.iter().any(|(_, item)| Self::pattern_binds_names(item)),
            Pattern::Wildcard | Pattern::Literal(_) => false,
        }
    }

    /// Whether `pattern` binds `name` anywhere.
    fn pattern_binds_name(pattern: &Pattern, name: &str) -> bool {
        match pattern {
            Pattern::Var(bound) => bound == name,
            Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } | Pattern::Or(items) => {
                items.iter().any(|item| Self::pattern_binds_name(item, name))
            }
            Pattern::Struct { fields, .. } => fields.iter().any(|(_, item)| Self::pattern_binds_name(item, name)),
            Pattern::Wildcard | Pattern::Literal(_) => false,
        }
    }

    /// `pattern` with every binding replaced by a wildcard, for a test that only asks whether it matches.
    fn erase_pattern_bindings(pattern: &Pattern) -> Pattern {
        match pattern {
            Pattern::Var(_) => Pattern::Wildcard,
            Pattern::Wildcard | Pattern::Literal(_) => pattern.clone(),
            Pattern::Tuple(items) => Pattern::Tuple(items.iter().map(Self::erase_pattern_bindings).collect()),
            Pattern::Or(items) => Pattern::Or(items.iter().map(Self::erase_pattern_bindings).collect()),
            Pattern::Struct { name, fields, rest } => Pattern::Struct {
                name: name.clone(),
                fields: fields
                    .iter()
                    .map(|(field, item)| (field.clone(), Self::erase_pattern_bindings(item)))
                    .collect(),
                rest: *rest,
            },
            Pattern::Enum { name, variant, fields } => Pattern::Enum {
                name: name.clone(),
                variant: variant.clone(),
                fields: fields.iter().map(Self::erase_pattern_bindings).collect(),
            },
        }
    }

    // ============================================================================
    // Hoisting nested `str` literals
    // ============================================================================

    /// The binding name a hoisted nested string literal takes.
    fn hoisted_string_literal_binding_name(prefix: &str, index: usize) -> String {
        format!("{prefix}{index}")
    }

    /// The literal test a hoisted string literal contributes to its arm's guard: `binding == "value"`.
    ///
    /// The comparison is recorded as an ordinary `BinOp::Eq` over two `str` operands, the same fact a source
    /// `word == "answer"` records, so the backend prints its borrowed string comparison for it and every string
    /// carrier the position may hold (owned, borrowed or static) compares through one path.
    fn hoisted_string_literal_test(binding: &str, value: &str) -> TypedExpr {
        let left = TypedExpr::new(
            IrExprKind::Var {
                name: binding.to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::String,
        );
        let right = TypedExpr::new(IrExprKind::String(value.to_string()), IrType::String);
        bool_binop(BinOp::Eq, left, right)
    }

    /// Join two optional boolean guards with `and`, keeping whichever exists when the other is absent.
    fn conjoin_match_guards(first: Option<TypedExpr>, second: Option<TypedExpr>) -> Option<TypedExpr> {
        Self::join_match_guards(first, second, BinOp::And)
    }

    /// Join two optional boolean guards with `op`, keeping whichever exists when the other is absent.
    fn join_match_guards(first: Option<TypedExpr>, second: Option<TypedExpr>, op: BinOp) -> Option<TypedExpr> {
        match (first, second) {
            (Some(first), Some(second)) => Some(bool_binop(op, first, second)),
            (first, None) => first,
            (None, second) => second,
        }
    }

    /// Rewrite the string literals nested inside a pattern into bindings tested by the arm's guard.
    ///
    /// A string literal at the top of a pattern is matched against the scrutinee by the backend's own `str` handling,
    /// and stays as written. A string literal *inside* a tuple, struct or enum pattern has no such path: printed as a
    /// pattern token it is a `&str` that an owned `String` position refuses (#1707). The shape the backend does consume
    /// is a binding plus a guard, so each nested literal becomes a fresh `<prefix><n>` binding (numbered from
    /// `next_index`) and its equality test is returned as the guard to conjoin ahead of the arm's own: the literal
    /// decides whether the arm applies, and the user's guard runs only once it does. An alternation whose alternatives
    /// are all string literals shares one binding and tests them with `or`, the nested twin of the backend's
    /// top-level alternation handling. [`Self::plan_arm_alternatives`] removes every other alternation holding a string
    /// literal before this runs.
    fn hoist_nested_string_literals(
        pattern: Pattern,
        prefix: &str,
        next_index: &mut usize,
    ) -> (Pattern, Option<TypedExpr>) {
        let mut guard = None;
        let pattern = match pattern {
            Pattern::Tuple(items) => Pattern::Tuple(
                items
                    .into_iter()
                    .map(|item| Self::hoist_string_literal_subpattern(item, prefix, next_index, &mut guard))
                    .collect(),
            ),
            Pattern::Struct { name, fields, rest } => Pattern::Struct {
                name,
                fields: fields
                    .into_iter()
                    .map(|(field, item)| {
                        (
                            field,
                            Self::hoist_string_literal_subpattern(item, prefix, next_index, &mut guard),
                        )
                    })
                    .collect(),
                rest,
            },
            Pattern::Enum { name, variant, fields } => Pattern::Enum {
                name,
                variant,
                fields: fields
                    .into_iter()
                    .map(|item| Self::hoist_string_literal_subpattern(item, prefix, next_index, &mut guard))
                    .collect(),
            },
            other => other,
        };
        (pattern, guard)
    }

    /// Rewrite one nested sub-pattern for [`Self::hoist_nested_string_literals`], accumulating the guard.
    fn hoist_string_literal_subpattern(
        pattern: Pattern,
        prefix: &str,
        next_index: &mut usize,
        guard: &mut Option<TypedExpr>,
    ) -> Pattern {
        match pattern {
            Pattern::Literal(literal) => match &literal.kind {
                IrExprKind::String(value) => {
                    let binding = Self::hoisted_string_literal_binding_name(prefix, *next_index);
                    *next_index += 1;
                    let test = Self::hoisted_string_literal_test(&binding, value);
                    *guard = Self::conjoin_match_guards(guard.take(), Some(test));
                    Pattern::Var(binding)
                }
                _ => Pattern::Literal(literal),
            },
            Pattern::Or(items) if Self::all_string_literals(&items) => {
                let binding = Self::hoisted_string_literal_binding_name(prefix, *next_index);
                *next_index += 1;
                let alternatives = items
                    .iter()
                    .filter_map(|item| match item {
                        Pattern::Literal(literal) => match &literal.kind {
                            IrExprKind::String(value) => Some(Self::hoisted_string_literal_test(&binding, value)),
                            _ => None,
                        },
                        _ => None,
                    })
                    .fold(None, |joined, test| {
                        Self::join_match_guards(joined, Some(test), BinOp::Or)
                    });
                *guard = Self::conjoin_match_guards(guard.take(), alternatives);
                Pattern::Var(binding)
            }
            Pattern::Or(items) => Pattern::Or(items),
            Pattern::Tuple(items) => Pattern::Tuple(
                items
                    .into_iter()
                    .map(|item| Self::hoist_string_literal_subpattern(item, prefix, &mut *next_index, &mut *guard))
                    .collect(),
            ),
            Pattern::Struct { name, fields, rest } => Pattern::Struct {
                name,
                fields: fields
                    .into_iter()
                    .map(|(field, item)| {
                        (
                            field,
                            Self::hoist_string_literal_subpattern(item, prefix, &mut *next_index, &mut *guard),
                        )
                    })
                    .collect(),
                rest,
            },
            Pattern::Enum { name, variant, fields } => Pattern::Enum {
                name,
                variant,
                fields: fields
                    .into_iter()
                    .map(|item| Self::hoist_string_literal_subpattern(item, prefix, &mut *next_index, &mut *guard))
                    .collect(),
            },
            other @ (Pattern::Wildcard | Pattern::Var(_)) => other,
        }
    }
}
