//! Match-arm patterns whose shape decides how many arms and which fields the lowered pattern spells. An alternation
//! whose alternatives need tests of their own lowers to one arm per alternative, the first matching alternative
//! deciding the arm, and a nested alternation that binds no names becomes one membership test (#1739). A partial
//! model pattern that leaves a private field unnamed ends in a rest marker instead of naming it (#1740).

use crate::decl::IrDeclKind;
use crate::expr::{BinOp, IrExprKind, MatchArm, Pattern, UnaryOp, VarAccess};
use crate::lower::AstLowering;
use crate::stmt::IrStmtKind;
use crate::types::IrType;
use crate::{IrProgram, TypedExpr};
use incan_frontend::{lexer, parser, typechecker::TypeChecker};

type TestResult = Result<(), String>;

/// Lex, check and lower one source module, refusing a program the checker does not accept.
fn lower_checked(source: &str) -> Result<IrProgram, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("typechecker failed: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    lowering
        .lower_program(&program)
        .map_err(|errors| format!("lowering failed: {errors:?}"))
}

/// The arms of the first `match` (or lowered `if let`) in the body of the named function.
fn match_arms(program: &IrProgram, function: &str) -> Result<Vec<MatchArm>, String> {
    let body = program
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(func) if func.name == function => Some(&func.body),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{function}`"))?;
    body.iter()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::Match { arms, .. } => Some(arms.clone()),
            IrStmtKind::Expr(expr) | IrStmtKind::Return(Some(expr)) => match &expr.kind {
                IrExprKind::Match { arms, .. } => Some(arms.clone()),
                _ => None,
            },
            _ => None,
        })
        .ok_or_else(|| format!("`{function}` lowers no match"))
}

/// Return `(binding, value)` when `guard` is one hoisted string-literal test `binding == "value"`.
fn string_literal_test(guard: &TypedExpr) -> Option<(String, String)> {
    let IrExprKind::BinOp {
        op: BinOp::Eq,
        left,
        right,
    } = &guard.kind
    else {
        return None;
    };
    let (IrExprKind::Var { name, .. }, IrExprKind::String(value)) = (&left.kind, &right.kind) else {
        return None;
    };
    (left.ty == IrType::String && right.ty == IrType::String).then(|| (name.clone(), value.clone()))
}

/// Return the integer a literal pattern matches, if it is one.
fn int_literal(pattern: &Pattern) -> Option<i64> {
    match pattern {
        Pattern::Literal(literal) => match literal.kind {
            IrExprKind::Int(value) => Some(value),
            _ => None,
        },
        _ => None,
    }
}

/// Split one arm's tuple pattern into its items, or explain what it was instead.
fn tuple_items<'a>(arm: &'a MatchArm, what: &str) -> Result<&'a [Pattern], String> {
    match &arm.pattern {
        Pattern::Tuple(items) => Ok(items.as_slice()),
        other => Err(format!("{what} must lower to a tuple pattern, got {other:?}")),
    }
}

/// Collect the access of every read of `name` inside `expr`, looking through operators and call arguments.
fn collect_reads_of(expr: &TypedExpr, name: &str, reads: &mut Vec<VarAccess>) {
    match &expr.kind {
        IrExprKind::Var { name: read, access, .. } if read == name => reads.push(*access),
        IrExprKind::BinOp { left, right, .. } => {
            collect_reads_of(left, name, reads);
            collect_reads_of(right, name, reads);
        }
        IrExprKind::UnaryOp { operand, .. } => collect_reads_of(operand, name, reads),
        IrExprKind::Call { func, args, .. } => {
            collect_reads_of(func, name, reads);
            for arg in args {
                collect_reads_of(&arg.expr, name, reads);
            }
        }
        IrExprKind::BuiltinCall { args, .. } => {
            for arg in args {
                collect_reads_of(arg, name, reads);
            }
        }
        _ => {}
    }
}

/// The conditions of a guard joined with `and`, in the order they run.
fn conjuncts(guard: &TypedExpr) -> Vec<&TypedExpr> {
    match &guard.kind {
        IrExprKind::BinOp {
            op: BinOp::And,
            left,
            right,
        } => {
            let mut tests = conjuncts(left);
            tests.extend(conjuncts(right));
            tests
        }
        _ => vec![guard],
    }
}

/// The expression a grouping block holds, or the expression itself when it is not one.
fn ungrouped(expr: &TypedExpr) -> &TypedExpr {
    match &expr.kind {
        IrExprKind::Block {
            stmts,
            value: Some(value),
        } if stmts.is_empty() => value,
        _ => expr,
    }
}

/// Whether `expr` is a shared view of the binding `name`.
fn is_view_of(expr: &TypedExpr, name: &str) -> bool {
    match &expr.kind {
        IrExprKind::UnaryOp {
            op: UnaryOp::Ref,
            operand,
        } => matches!(&operand.kind, IrExprKind::Var { name: read, .. } if read == name),
        _ => false,
    }
}

/// The literal a membership `match` over a shared view of `name` tests at its one `str` position, if `test` is one.
fn member_literal_test(test: &TypedExpr, name: &str) -> Option<String> {
    let IrExprKind::Match { scrutinee, arms } = &test.kind else {
        return None;
    };
    if !is_view_of(scrutinee, name) {
        return None;
    }
    let [member, fallback] = arms.as_slice() else {
        return None;
    };
    if !matches!(fallback.pattern, Pattern::Wildcard) {
        return None;
    }
    member
        .guard
        .as_ref()
        .and_then(string_literal_test)
        .map(|(_, value)| value)
}

/// Whether `pattern` binds a name the arm planning generates: a hoisted literal or a captured position.
fn binds_generated_name(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Var(name) => name.starts_with("__incan_match_str_") || name.starts_with("__incan_match_pos_"),
        Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } | Pattern::Or(items) => {
            items.iter().any(binds_generated_name)
        }
        Pattern::Struct { fields, .. } => fields.iter().any(|(_, item)| binds_generated_name(item)),
        Pattern::Wildcard | Pattern::Literal(_) => false,
    }
}

/// The fields of an arm's variant pattern, or an explanation of what the pattern was instead.
fn enum_fields(arm: &MatchArm) -> Result<&[Pattern], String> {
    match &arm.pattern {
        Pattern::Enum { fields, .. } => Ok(fields.as_slice()),
        other => Err(format!("expected a variant pattern, got {other:?}")),
    }
}

/// The dotted place a shared view reads (`res`, `job.status`), if `expr` is a view of a place.
fn view_text(expr: &TypedExpr) -> Option<String> {
    let IrExprKind::UnaryOp {
        op: UnaryOp::Ref,
        operand,
    } = &expr.kind
    else {
        return None;
    };
    place_text(operand)
}

/// The dotted spelling of a place expression: a name, or a field path starting at one.
fn place_text(expr: &TypedExpr) -> Option<String> {
    match &expr.kind {
        IrExprKind::Var { name, .. } => Some(name.clone()),
        IrExprKind::Field { object, field } => place_text(object).map(|object| format!("{object}.{field}")),
        _ => None,
    }
}

/// Whether `test` is the negation of a membership `match` over a shared view of `name`.
fn negated_membership_of(test: &TypedExpr, name: &str) -> bool {
    match &test.kind {
        IrExprKind::UnaryOp {
            op: UnaryOp::Not,
            operand,
        } => matches!(
            &ungrouped(operand).kind,
            IrExprKind::Match { scrutinee, .. } if is_view_of(scrutinee, name)
        ),
        _ => false,
    }
}

/// Whether `pattern` binds a position of the matched value for a test.
fn binds_position(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Var(name) => name.starts_with("__incan_match_pos_"),
        Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } | Pattern::Or(items) => {
            items.iter().any(binds_position)
        }
        Pattern::Struct { fields, .. } => fields.iter().any(|(_, item)| binds_position(item)),
        Pattern::Wildcard | Pattern::Literal(_) => false,
    }
}

/// #1739: the issue's program. A top-level alternation of tuple patterns, each with a nested `str` literal, lowers to
/// one arm per alternative in source order: each arm tests its own literal in its own guard, on the scrutinee, and runs
/// the same body, and the arm after the alternation is untouched.
#[test]
fn or_pattern_with_nested_str_literals_lowers_one_arm_per_alternative_issue1739() -> TestResult {
    let source = r#"
def classify(pair: tuple[int, str]) -> str:
    match pair:
        (0, "a") | (1, "b") => return "known"
        _ => return "other"
"#;
    let arms = match_arms(&lower_checked(source)?, "classify")?;
    let [first, second, rest] = arms.as_slice() else {
        return Err(format!(
            "expected two split arms and the wildcard, got {} arms",
            arms.len()
        ));
    };
    for (arm, number, literal) in [(first, 0, "a"), (second, 1, "b")] {
        let [item_number, Pattern::Wildcard] = tuple_items(arm, "each alternative")? else {
            return Err(format!(
                "the literal of alternative `{literal}` must become a wildcard, got {:?}",
                arm.pattern
            ));
        };
        assert_eq!(int_literal(item_number), Some(number), "each arm keeps its own number");
        let guard = arm
            .guard
            .as_ref()
            .ok_or("each split arm tests its own literal in its guard")?;
        assert_eq!(
            member_literal_test(guard, "pair"),
            Some(literal.to_string()),
            "the guard tests exactly this alternative's literal, on the scrutinee"
        );
    }
    assert_eq!(
        format!("{:?}", first.body),
        format!("{:?}", second.body),
        "both alternatives run the one lowered body"
    );
    assert!(
        matches!(rest.pattern, Pattern::Wildcard) && rest.guard.is_none(),
        "the arm after the alternation is unchanged, got {:?}",
        rest.pattern
    );
    Ok(())
}

/// #1739: a nested alternation that binds no names and holds a `str` literal, such as `Some("a") | None` inside a
/// tuple, keeps the arm whole. Over a local scrutinee, its position becomes a wildcard and the arm's guard tests the
/// scrutinee, through a shared view, against the alternation at that position, ahead of the arm's own guard: the arm's
/// pattern binds nothing new, so nothing moves out of the scrutinee.
#[test]
fn nested_nameless_alternation_becomes_one_membership_test_issue1739() -> TestResult {
    let source = r#"
def tag(pair: tuple[int, Option[str]]) -> str:
    match pair:
        (n, Some("a") | None) if n > 0 => return "tagged"
        _ => return "other"
"#;
    let arms = match_arms(&lower_checked(source)?, "tag")?;
    let [tagged, _] = arms.as_slice() else {
        return Err(format!("expected one arm and the wildcard, got {} arms", arms.len()));
    };
    let [Pattern::Var(n), Pattern::Wildcard] = tuple_items(tagged, "the arm")? else {
        return Err(format!(
            "the alternation's position must become a wildcard, got {:?}",
            tagged.pattern
        ));
    };
    assert_eq!(n, "n");
    let tests = conjuncts(tagged.guard.as_ref().ok_or("the arm must carry a guard")?);
    let [membership, user_guard] = tests.as_slice() else {
        return Err(format!("expected the membership test and the guard, got {tests:?}"));
    };
    let IrExprKind::Match {
        scrutinee,
        arms: member_arms,
    } = &membership.kind
    else {
        return Err(format!("the membership test must be a match, got {membership:?}"));
    };
    assert!(
        is_view_of(scrutinee, "pair"),
        "the membership test reads the scrutinee, got {scrutinee:?}"
    );
    let [some_member, none_member, no_member] = member_arms.as_slice() else {
        return Err(format!(
            "expected one member arm per alternative and a fallback, got {member_arms:?}"
        ));
    };
    let [Pattern::Wildcard, Pattern::Enum { fields, .. }] = tuple_items(some_member, "the `Some` member")? else {
        return Err(format!(
            "the member arm keeps only the alternation's position, got {:?}",
            some_member.pattern
        ));
    };
    let [Pattern::Var(member)] = fields.as_slice() else {
        return Err(format!("the payload literal must become a binding, got {fields:?}"));
    };
    assert_eq!(
        some_member.guard.as_ref().and_then(string_literal_test),
        Some((member.clone(), "a".to_string()))
    );
    assert!(
        none_member.guard.is_none()
            && matches!(no_member.pattern, Pattern::Wildcard)
            && matches!(no_member.body.kind, IrExprKind::Bool(false))
            && [some_member, none_member]
                .iter()
                .all(|arm| matches!(arm.body.kind, IrExprKind::Bool(true))),
        "member arms answer `true` and the fallback answers `false`"
    );
    assert!(
        matches!(user_guard.kind, IrExprKind::BinOp { op: BinOp::Gt, .. }),
        "the arm's own guard follows, got {user_guard:?}"
    );
    Ok(())
}

/// #1739: over a temporary scrutinee, which no guard can read again, the same alternation's position becomes a binding
/// and the membership test reads that binding.
#[test]
fn nested_nameless_alternation_over_a_temporary_binds_its_position_issue1739() -> TestResult {
    let source = r#"
def tag(n: int, label: Option[str]) -> str:
    match (n, label):
        (count, Some("a") | None) if count > 0 => return "tagged"
        _ => return "other"
"#;
    let arms = match_arms(&lower_checked(source)?, "tag")?;
    let [tagged, _] = arms.as_slice() else {
        return Err(format!("expected one arm and the wildcard, got {} arms", arms.len()));
    };
    let [Pattern::Var(_), Pattern::Var(position)] = tuple_items(tagged, "the arm")? else {
        return Err(format!(
            "the alternation's position must become a binding, got {:?}",
            tagged.pattern
        ));
    };
    let tests = conjuncts(tagged.guard.as_ref().ok_or("the arm must carry a guard")?);
    assert!(
        tests.iter().any(|test| matches!(
            &test.kind,
            IrExprKind::Match { scrutinee, .. } if is_view_of(scrutinee, position)
        )),
        "the membership test reads the bound position, got {tests:?}"
    );
    Ok(())
}

/// #1739: four nested alternations that bind no names still lower to one arm: each is one membership test, so the
/// body is never copied per combination of alternatives.
#[test]
fn nested_nameless_alternations_never_multiply_the_arm_issue1739() -> TestResult {
    let source = r#"
def count(quad: tuple[Option[str], Option[str], Option[str], Option[str]]) -> int:
    match quad:
        (Some("a") | None, Some("b") | None, Some("c") | None, Some("d") | None) => return 1
        _ => return 0
"#;
    let arms = match_arms(&lower_checked(source)?, "count")?;
    let [quad_arm, _] = arms.as_slice() else {
        return Err(format!("expected one arm and the wildcard, got {} arms", arms.len()));
    };
    let items = tuple_items(quad_arm, "the arm")?;
    assert!(
        items.len() == 4 && items.iter().all(|item| matches!(item, Pattern::Wildcard)),
        "every alternation's position becomes a wildcard, got {items:?}"
    );
    let guard = quad_arm
        .guard
        .as_ref()
        .ok_or("the arm must carry the membership tests")?;
    let tests = conjuncts(guard);
    assert!(
        tests.len() == 4
            && tests.iter().all(|test| matches!(
                &test.kind,
                IrExprKind::Match { scrutinee, arms } if is_view_of(scrutinee, "quad") && arms.len() == 3
            )),
        "one membership test per alternation, each with one member arm per alternative, got {guard:?}"
    );
    Ok(())
}

/// #1739: alternations that need no guard of their own stay in one arm: an alternation of plain values beside a nested
/// literal, a nested alternation of `str` literals (one binding tested with `or`), and a top-level alternation of `str`
/// literals, which the backend matches directly.
#[test]
fn alternations_that_need_no_guard_of_their_own_stay_one_arm_issue1739() -> TestResult {
    let source = r#"
def low(number: int, word: str) -> str:
    match (number, word):
        (0 | 1, "a") => return "low a"
        (n, "yes" | "no") => return "answered"
        _ => return "other"

def word(value: str) -> str:
    match value:
        "a" | "b" => return "early"
        _ => return "late"
"#;
    let program = lower_checked(source)?;
    let arms = match_arms(&program, "low")?;
    let [low_a, answered, _] = arms.as_slice() else {
        return Err(format!("expected three arms, got {}", arms.len()));
    };
    let [Pattern::Or(numbers), Pattern::Var(binding)] = tuple_items(low_a, "the plain-value alternation")? else {
        return Err(format!("`0 | 1` must stay an alternation, got {:?}", low_a.pattern));
    };
    assert_eq!(
        numbers.iter().map(int_literal).collect::<Vec<_>>(),
        vec![Some(0), Some(1)]
    );
    assert_eq!(
        low_a.guard.as_ref().and_then(string_literal_test),
        Some((binding.clone(), "a".to_string()))
    );
    let [Pattern::Var(_), Pattern::Var(_)] = tuple_items(answered, "the literal alternation")? else {
        return Err(format!(
            "`\"yes\" | \"no\"` must share one binding, got {:?}",
            answered.pattern
        ));
    };
    assert!(
        answered
            .guard
            .as_ref()
            .is_some_and(|guard| matches!(guard.kind, IrExprKind::BinOp { op: BinOp::Or, .. })),
        "the shared binding is tested with `or`, got {:?}",
        answered.guard
    );

    let arms = match_arms(&program, "word")?;
    let [early, _] = arms.as_slice() else {
        return Err(format!("expected two arms, got {}", arms.len()));
    };
    assert!(
        matches!(&early.pattern, Pattern::Or(items) if items.len() == 2) && early.guard.is_none(),
        "a top-level alternation of `str` literals stays one unguarded arm, got {:?}",
        early.pattern
    );
    Ok(())
}

/// #1739: `if let` with the issue's alternation lowers to one arm per alternative ahead of its fallback arm.
#[test]
fn if_let_or_pattern_with_nested_str_literals_lowers_one_arm_per_alternative_issue1739() -> TestResult {
    let source = r#"
def check(number: int, word: str) -> str:
    if let (0, "a") | (1, "b") = (number, word):
        return "known"
    return "other"
"#;
    let arms = match_arms(&lower_checked(source)?, "check")?;
    let [first, second, fallback] = arms.as_slice() else {
        return Err(format!(
            "expected two split arms and the fallback, got {} arms",
            arms.len()
        ));
    };
    for (arm, literal) in [(first, "a"), (second, "b")] {
        let [_, Pattern::Var(binding)] = tuple_items(arm, "each alternative")? else {
            return Err(format!(
                "the literal `{literal}` must become a binding, got {:?}",
                arm.pattern
            ));
        };
        assert_eq!(
            arm.guard.as_ref().and_then(string_literal_test),
            Some((binding.clone(), literal.to_string()))
        );
    }
    assert!(
        matches!(fallback.pattern, Pattern::Wildcard),
        "the fallback stays last, got {:?}",
        fallback.pattern
    );
    Ok(())
}

/// #1739: the arm's own guard runs once per matching alternative once the arm is split, so no read in it may consume
/// the value it reads: every copy of the guard reads `expected` without moving it.
#[test]
fn guard_shared_by_split_arms_never_consumes_what_it_reads_issue1739() -> TestResult {
    let source = r#"
def accepts(value: str) -> bool:
    return len(value) > 0

def check(pair: tuple[int, str], expected: str) -> bool:
    match pair:
        (0, "a") | (1, "b") if accepts(expected) => return true
        _ => return false
"#;
    let arms = match_arms(&lower_checked(source)?, "check")?;
    let [first, second, _] = arms.as_slice() else {
        return Err(format!(
            "expected two split arms and the wildcard, got {} arms",
            arms.len()
        ));
    };
    for arm in [first, second] {
        let guard = arm.guard.as_ref().ok_or("each split arm carries the arm's guard")?;
        let mut reads = Vec::new();
        collect_reads_of(guard, "expected", &mut reads);
        assert!(!reads.is_empty(), "the guard reads `expected`, got {guard:?}");
        assert!(
            reads.iter().all(|access| !matches!(access, VarAccess::Move)),
            "a guard shared by split arms must not move `expected`, got {reads:?}"
        );
    }
    Ok(())
}

/// #1739: alternatives that bind the same name at different positions each get an arm that binds it where that
/// alternative does; with no guard, the first alternative that matches decides, as the arms are tried in order.
#[test]
fn alternatives_binding_a_name_at_different_positions_each_bind_it_issue1739() -> TestResult {
    let source = r#"
def either_side(left: str, right: str) -> str:
    match (left, right):
        (x, "a") | ("b", x) => return x
        _ => return "none"
"#;
    let arms = match_arms(&lower_checked(source)?, "either_side")?;
    let [first, second, _] = arms.as_slice() else {
        return Err(format!("expected two arms and the wildcard, got {} arms", arms.len()));
    };
    let [Pattern::Var(x), Pattern::Var(literal)] = tuple_items(first, "the first alternative")? else {
        return Err(format!("expected `(x, <literal>)`, got {:?}", first.pattern));
    };
    assert_eq!(x, "x");
    assert_eq!(
        first.guard.as_ref().and_then(string_literal_test),
        Some((literal.clone(), "a".to_string()))
    );
    let [Pattern::Var(literal), Pattern::Var(x)] = tuple_items(second, "the second alternative")? else {
        return Err(format!("expected `(<literal>, x)`, got {:?}", second.pattern));
    };
    assert_eq!(x, "x");
    assert_eq!(
        second.guard.as_ref().and_then(string_literal_test),
        Some((literal.clone(), "b".to_string())),
        "without a guard the later alternative tests only its own literal"
    );
    Ok(())
}

/// #1739: under a guard, the first alternative that matches decides the arm and the guard runs once. The arm for the
/// second alternative of `(x, "a") | ("b", x) if accept(x)` also tests, on the scrutinee, that the first alternative
/// did not match, ahead of the guard; the first alternative's arm needs no such test.
#[test]
fn overlapping_alternatives_under_a_guard_take_the_first_match_only_issue1739() -> TestResult {
    let source = r#"
def accept(name: str) -> bool:
    return name == "a"

def first_match(pair: tuple[str, str]) -> str:
    match pair:
        (x, "a") | ("b", x) if accept(x) => return x
        _ => return "none"
"#;
    let arms = match_arms(&lower_checked(source)?, "first_match")?;
    let [first, second, _] = arms.as_slice() else {
        return Err(format!("expected two arms and the wildcard, got {} arms", arms.len()));
    };
    let first_tests = conjuncts(first.guard.as_ref().ok_or("the first arm carries a guard")?);
    assert!(
        !first_tests
            .iter()
            .any(|test| matches!(test.kind, IrExprKind::UnaryOp { .. })),
        "nothing precedes the first alternative, got {first_tests:?}"
    );
    assert!(
        matches!(first_tests.last().map(|test| &test.kind), Some(IrExprKind::Call { .. })),
        "the arm's own guard runs last, got {first_tests:?}"
    );

    let second_tests = conjuncts(second.guard.as_ref().ok_or("the second arm carries a guard")?);
    let [literal_test, not_first, user_guard] = second_tests.as_slice() else {
        return Err(format!(
            "expected the literal test, the first-match test and the guard, got {second_tests:?}"
        ));
    };
    assert_eq!(member_literal_test(literal_test, "pair"), Some("b".to_string()));
    assert!(
        negated_membership_of(not_first, "pair"),
        "the second arm is taken only if the first alternative does not match the scrutinee, got {not_first:?}"
    );
    assert!(
        matches!(user_guard.kind, IrExprKind::Call { .. }),
        "the arm's own guard runs last, got {user_guard:?}"
    );
    Ok(())
}

/// #1739: over a temporary scrutinee the first-match test reads positions the later arm binds: the second
/// alternative's `x` is the first alternative's `"a"` position.
#[test]
fn overlapping_alternatives_over_a_temporary_test_bound_positions_issue1739() -> TestResult {
    let source = r#"
def accept(name: str) -> bool:
    return name == "a"

def first_match(left: str, right: str) -> str:
    match (left, right):
        (x, "a") | ("b", x) if accept(x) => return x
        _ => return "none"
"#;
    let arms = match_arms(&lower_checked(source)?, "first_match")?;
    let [_, second, _] = arms.as_slice() else {
        return Err(format!("expected two arms and the wildcard, got {} arms", arms.len()));
    };
    let second_tests = conjuncts(second.guard.as_ref().ok_or("the second arm carries a guard")?);
    let [_, not_first, _] = second_tests.as_slice() else {
        return Err(format!(
            "expected the literal test, the first-match test and the guard, got {second_tests:?}"
        ));
    };
    let IrExprKind::UnaryOp {
        op: UnaryOp::Not,
        operand,
    } = &not_first.kind
    else {
        return Err(format!("expected a negated test, got {not_first:?}"));
    };
    assert_eq!(
        string_literal_test(ungrouped(operand)),
        Some(("x".to_string(), "a".to_string())),
        "the second arm is taken only if its `x` does not hold the first alternative's `\"a\"`"
    );
    Ok(())
}

/// #1739: a later alternative whose position holds a whole alternation that binds names still gets its first-match
/// test. Over a local scrutinee the test reads the scrutinee, so the alternation stays whole; over a temporary the
/// alternation is split first, so each piece's bound position can be read. `(Ok(1), 0)` then reaches neither the
/// second alternative nor its guard.
#[test]
fn first_match_reads_through_a_whole_binding_alternation_issue1739() -> TestResult {
    let source = r#"
def sign_of(pair: tuple[Result[int, int], int]) -> str:
    match pair:
        (Ok(1), n) | (Ok(n) | Err(n), _) if n != 0 => return f"{n}"
        _ => return "other"

def sign_of_parts(result: Result[int, int], count: int) -> str:
    match (result, count):
        (Ok(1), n) | (Ok(n) | Err(n), _) if n != 0 => return f"{n}"
        _ => return "other"
"#;
    let program = lower_checked(source)?;
    let arms = match_arms(&program, "sign_of")?;
    let [_, whole, _] = arms.as_slice() else {
        return Err(format!("expected two arms and the wildcard, got {} arms", arms.len()));
    };
    let [Pattern::Or(_), Pattern::Wildcard] = tuple_items(whole, "the second alternative")? else {
        return Err(format!("the alternation stays whole, got {:?}", whole.pattern));
    };
    let tests = conjuncts(whole.guard.as_ref().ok_or("the second arm carries a guard")?);
    assert!(
        tests.iter().any(|test| negated_membership_of(test, "pair")),
        "the second arm tests that the first alternative did not match, got {tests:?}"
    );

    let arms = match_arms(&program, "sign_of_parts")?;
    let [_, ok_piece, err_piece, _] = arms.as_slice() else {
        return Err(format!("expected three arms and the wildcard, got {} arms", arms.len()));
    };
    let ok_tests = conjuncts(ok_piece.guard.as_ref().ok_or("the `Ok` piece carries a guard")?);
    assert!(
        ok_tests
            .iter()
            .any(|test| matches!(test.kind, IrExprKind::UnaryOp { op: UnaryOp::Not, .. })),
        "the `Ok(n)` piece tests that `(Ok(1), _)` did not match, got {ok_tests:?}"
    );
    let err_tests = conjuncts(err_piece.guard.as_ref().ok_or("the `Err` piece carries a guard")?);
    assert!(
        !err_tests
            .iter()
            .any(|test| matches!(test.kind, IrExprKind::UnaryOp { .. })),
        "`Err(n)` cannot overlap an `Ok` alternative, got {err_tests:?}"
    );
    Ok(())
}

/// #1739: a guarded alternation over a `Result` scrutinee that is used after the `match` binds nothing new, so the
/// scrutinee keeps its value: the first-match test reads it through a shared view.
#[test]
fn guarded_alternation_leaves_a_result_scrutinee_whole_issue1739() -> TestResult {
    let source = r#"
def ok_after_match(res: Result[Option[str], int], verbose: bool) -> str:
    match res:
        Ok(None) | Ok(_) if verbose => println("ok value")
        _ => println("other value")
    match res:
        Ok(_) => return "still ok"
        Err(code) => return f"error {code}"
"#;
    let arms = match_arms(&lower_checked(source)?, "ok_after_match")?;
    let [_, any_ok, _] = arms.as_slice() else {
        return Err(format!("expected two arms and the wildcard, got {} arms", arms.len()));
    };
    assert!(
        arms.iter().all(|arm| !binds_position(&arm.pattern)),
        "no arm binds a position of the scrutinee, got {arms:?}"
    );
    let tests = conjuncts(any_ok.guard.as_ref().ok_or("the `Ok(_)` arm carries a guard")?);
    assert!(
        tests.iter().any(|test| negated_membership_of(test, "res")),
        "the `Ok(_)` arm tests on the scrutinee that `Ok(None)` did not match, got {tests:?}"
    );
    Ok(())
}

/// #1739: a top-level alternation that mixes a `str` literal with a wildcard splits, since the backend matches a `str`
/// value against literals only when every alternative is one. Under a guard, the wildcard's arm tests on the scrutinee
/// that the literal did not match.
#[test]
fn top_level_alternation_of_a_str_literal_and_a_wildcard_splits_issue1739() -> TestResult {
    let source = r#"
def any_word(value: str) -> str:
    match value:
        "a" | _ => return "any"

def long_word(value: str) -> str:
    match value:
        "a" | _ if len(value) > 3 => return "a or long"
        _ => return "short"
"#;
    let program = lower_checked(source)?;
    let arms = match_arms(&program, "any_word")?;
    let [literal, rest] = arms.as_slice() else {
        return Err(format!("expected two arms, got {}", arms.len()));
    };
    assert!(
        matches!(&literal.pattern, Pattern::Literal(expr) if matches!(expr.kind, IrExprKind::String(_)))
            && matches!(rest.pattern, Pattern::Wildcard)
            && literal.guard.is_none()
            && rest.guard.is_none(),
        "the alternatives become a literal arm and a wildcard arm, got {:?} and {:?}",
        literal.pattern,
        rest.pattern
    );

    let arms = match_arms(&program, "long_word")?;
    let [_, wildcard, _] = arms.as_slice() else {
        return Err(format!("expected two arms and the fallback, got {}", arms.len()));
    };
    assert!(
        matches!(wildcard.pattern, Pattern::Wildcard),
        "the wildcard's arm binds nothing, got {:?}",
        wildcard.pattern
    );
    let tests = conjuncts(wildcard.guard.as_ref().ok_or("the wildcard's arm carries a guard")?);
    let [not_literal, _] = tests.as_slice() else {
        return Err(format!("expected the first-match test and the guard, got {tests:?}"));
    };
    let IrExprKind::UnaryOp {
        op: UnaryOp::Not,
        operand,
    } = &not_literal.kind
    else {
        return Err(format!("expected a negated test, got {not_literal:?}"));
    };
    assert_eq!(
        string_literal_test(ungrouped(operand)),
        Some(("value".to_string(), "a".to_string()))
    );
    Ok(())
}

/// #1739: a guard after an alternation that stays whole may run once per alternative the backend tries, so no read in
/// it consumes the value it reads.
#[test]
fn guard_after_a_whole_alternation_never_consumes_what_it_reads_issue1739() -> TestResult {
    let source = r#"
def consume(value: str) -> bool:
    return len(value) > 0

def pick(n: int, s: str) -> str:
    match n:
        1 | 2 if consume(s) => return "small"
        _ => return "other"
"#;
    let arms = match_arms(&lower_checked(source)?, "pick")?;
    let [small, _] = arms.as_slice() else {
        return Err(format!("expected one arm and the wildcard, got {} arms", arms.len()));
    };
    assert!(
        matches!(small.pattern, Pattern::Or(_)),
        "`1 | 2` stays one alternation, got {:?}",
        small.pattern
    );
    let guard = small.guard.as_ref().ok_or("the arm carries its guard")?;
    let mut reads = Vec::new();
    collect_reads_of(guard, "s", &mut reads);
    assert!(
        !reads.is_empty() && reads.iter().all(|access| !matches!(access, VarAccess::Move)),
        "the guard reads `s` without moving it, got {reads:?}"
    );
    Ok(())
}

/// #1739: `while let` goes through the same arm planning as `match` and `if let`: a nested `str` literal becomes a
/// guarded binding ahead of the arm that ends the loop. A pattern alternation never reaches it, because the parser
/// accepts alternations only in `match` arms and `if let`.
#[test]
fn while_let_plans_its_pattern_like_a_match_arm_issue1739() -> TestResult {
    let source = r#"
def stage(step: int) -> tuple[int, str]:
    if step == 0:
        return (0, "a")
    return (1, "b")

def count_stages() -> int:
    mut step = 0
    while let (0, "a") = stage(step):
        step += 1
    return step
"#;
    let program = lower_checked(source)?;
    let body = program
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(func) if func.name == "count_stages" => Some(&func.body),
            _ => None,
        })
        .ok_or("missing function `count_stages`")?;
    let arms = body
        .iter()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::Loop { body, .. } => body.iter().find_map(|stmt| match &stmt.kind {
                IrStmtKind::Match { arms, .. } => Some(arms),
                _ => None,
            }),
            _ => None,
        })
        .ok_or("`count_stages` lowers no loop over a match")?;
    let [stage_arm, stop] = arms.as_slice() else {
        return Err(format!(
            "expected the pattern's arm and the loop's end, got {} arms",
            arms.len()
        ));
    };
    let [item_number, Pattern::Var(binding)] = tuple_items(stage_arm, "the pattern's arm")? else {
        return Err(format!(
            "the literal must become a binding, got {:?}",
            stage_arm.pattern
        ));
    };
    assert_eq!(int_literal(item_number), Some(0));
    assert_eq!(
        stage_arm.guard.as_ref().and_then(string_literal_test),
        Some((binding.clone(), "a".to_string()))
    );
    assert!(matches!(stop.pattern, Pattern::Wildcard), "the loop's end stays last");

    let alternation = source.replace("while let (0, \"a\") =", "while let (0, \"a\") | (1, \"b\") =");
    let tokens = lexer::lex(&alternation).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    assert!(
        parser::parse(&tokens).is_err(),
        "a `while let` alternation is refused before lowering"
    );
    Ok(())
}

/// #1739: over a place, a nested `str` literal is tested on the place rather than hoisted into a binding, so a
/// `Result` local or field matched again after a guarded alternation still holds its `Err` payload.
#[test]
fn nested_str_literal_over_a_place_leaves_the_place_whole_issue1739() -> TestResult {
    let source = r#"
model Job:
    status: Result[int, str]

def retry_after(res: Result[int, str], retry: bool) -> str:
    match res:
        Err("timeout") | Err(_) if retry => println("retrying")
        _ => println("giving up")
    match res:
        Ok(value) => return f"value {value}"
        Err(reason) => return f"error {reason}"

def job_state(job: Job, retry: bool) -> str:
    match job.status:
        Err("timeout") | Err(_) if retry => println("retrying")
        _ => println("giving up")
    match job.status:
        Ok(value) => return f"value {value}"
        Err(reason) => return f"error {reason}"
"#;
    let program = lower_checked(source)?;
    for (function, view) in [("retry_after", "res"), ("job_state", "job.status")] {
        let arms = match_arms(&program, function)?;
        let [timeout, _, _] = arms.as_slice() else {
            return Err(format!(
                "expected two arms and the wildcard in `{function}`, got {}",
                arms.len()
            ));
        };
        assert!(
            arms.iter().all(|arm| !binds_generated_name(&arm.pattern)),
            "no arm of `{function}` binds a hoisted literal or a position, got {arms:?}"
        );
        let [Pattern::Wildcard] = enum_fields(timeout)? else {
            return Err(format!(
                "the literal's position is a wildcard, got {:?}",
                timeout.pattern
            ));
        };
        let tests = conjuncts(timeout.guard.as_ref().ok_or("the timeout arm carries a guard")?);
        assert!(
            tests.iter().any(|test| matches!(
                &test.kind,
                IrExprKind::Match { scrutinee, .. } if view_text(scrutinee).as_deref() == Some(view)
            )),
            "the literal is tested through a shared view of `{view}`, got {tests:?}"
        );
    }
    Ok(())
}

/// #1740: a partial pattern over a model outside the model's own methods, whose unnamed fields include a private one,
/// spells only the fields it names and ends in a rest marker; the private field is never named, not even as a
/// wildcard. A partial pattern whose unnamed fields are all public keeps spelling them as wildcards (#1708).
#[test]
fn partial_pattern_leaving_a_private_field_unnamed_ends_in_a_rest_marker_issue1740() -> TestResult {
    let source = r#"
pub model Account:
    pub kind: str
    pub tier: int
    _secret: int

pub model Plan:
    pub name: str
    pub seats: int

def describe(account: Account) -> str:
    match account:
        Account(kind="premium") => return "premium"
        _ => return "other"

def plan_name(plan: Plan) -> str:
    match plan:
        Plan(name="team") => return "team"
        _ => return "other"
"#;
    let program = lower_checked(source)?;

    let arms = match_arms(&program, "describe")?;
    let premium = arms.first().ok_or("expected a premium arm")?;
    let Pattern::Struct { name, fields, rest } = &premium.pattern else {
        return Err(format!("expected a struct pattern, got {:?}", premium.pattern));
    };
    assert_eq!(name, "Account");
    assert!(*rest, "a rest holding a private field must end in a rest marker");
    let [(field, Pattern::Wildcard)] = fields.as_slice() else {
        return Err(format!("only the named field may be spelled, got {fields:?}"));
    };
    assert_eq!(field, "kind");
    let guard = premium.guard.as_ref().ok_or("the premium arm carries a guard")?;
    let IrExprKind::Match {
        scrutinee,
        arms: member_arms,
    } = &guard.kind
    else {
        return Err(format!(
            "the named field's literal is tested on the place, got {guard:?}"
        ));
    };
    assert!(is_view_of(scrutinee, "account"));
    let Some(Pattern::Struct { fields, rest, .. }) = member_arms.first().map(|arm| &arm.pattern) else {
        return Err(format!("expected a struct member arm, got {member_arms:?}"));
    };
    assert!(
        *rest && fields.len() == 1 && fields.iter().all(|(field, _)| field == "kind"),
        "the test names only the public field it tests, got {fields:?}"
    );

    let arms = match_arms(&program, "plan_name")?;
    let team = arms.first().ok_or("expected a team arm")?;
    let Pattern::Struct { fields, rest, .. } = &team.pattern else {
        return Err(format!("expected a struct pattern, got {:?}", team.pattern));
    };
    assert!(
        !rest,
        "a rest of public fields is spelled out, not left to a rest marker"
    );
    assert_eq!(
        fields.iter().map(|(field, _)| field.as_str()).collect::<Vec<_>>(),
        vec!["name", "seats"],
        "the public omitted field is spelled as a wildcard after the named one"
    );
    assert!(
        matches!(fields.last(), Some((_, Pattern::Wildcard))),
        "the omitted field is a wildcard, got {fields:?}"
    );
    Ok(())
}
