//! Generated-Rust regressions for `match` arms: ownership across mutually exclusive arms, and the arm and pattern
//! shapes lowering hands the emitter for patterns that the source spells in one arm.

use incan_emit::IrCodegen;
use incan_frontend::{lexer, parser};
use incan_semantics_core::SemanticSourceTargetKind;

use incan_test_support::canonical_projection;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lower one source fixture through the ordinary native pipeline.
fn generate_rust(source: &str) -> Result<String, std::io::Error> {
    let tokens =
        lexer::lex(source).map_err(|errors| std::io::Error::other(format!("fixture did not lex: {errors:?}")))?;
    let program =
        parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("fixture did not parse: {errors:?}")))?;
    IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| std::io::Error::other(format!("fixture did not codegen: {error:?}")))
}

/// Return generated Rust plus a whitespace-free copy for stable ownership assertions.
fn generated_and_compact_rust(source: &str) -> Result<(String, String), std::io::Error> {
    let generated = generate_rust(source)?;
    let compact = generated
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    Ok((generated, compact))
}

#[test]
fn generic_value_moves_in_every_terminal_match_arm_without_clone_bound() -> TestResult {
    let (generated, rust) = generated_and_compact_rust(
        r#"
pub def select[T](value: T, choose_first: bool) -> T:
    match choose_first:
        true => return value
        false => return value
"#,
    )?;
    let select = canonical_projection::projected_name(&generated, "select", SemanticSourceTargetKind::Function);

    assert!(
        rust.contains(&format!("pubfn{select}<T,>(value:T,choose_first:bool)->T")),
        "terminal match arms must not narrow the generic signature with Clone:\n{rust}"
    );
    assert_eq!(
        rust.matches("returnvalue;").count(),
        2,
        "each mutually exclusive terminal arm must move the same value exactly once:\n{rust}"
    );
    assert!(
        !rust.contains("value.clone()"),
        "terminal match arms must not clone a value that no path uses afterwards:\n{rust}"
    );
    Ok(())
}

#[test]
fn generic_value_clones_in_match_arms_when_used_after_match() -> TestResult {
    let (generated, rust) = generated_and_compact_rust(
        r#"
pub def preserve[T](value: T, choose_first: bool) -> T:
    match choose_first:
        true =>
            first = value
        false =>
            second = value
    return value
"#,
    )?;
    let preserve = canonical_projection::projected_name(&generated, "preserve", SemanticSourceTargetKind::Function);

    assert!(
        rust.contains(&format!("pubfn{preserve}<T:Clone,>(value:T,choose_first:bool)->T")),
        "a value used after the match must retain the required Clone bound:\n{rust}"
    );
    assert_eq!(
        rust.matches("=value.clone();").count(),
        2,
        "each arm must preserve the value for its post-match use:\n{rust}"
    );
    assert!(
        rust.contains("returnvalue;"),
        "the post-match last use must still move the original value:\n{rust}"
    );
    Ok(())
}

/// A guard decides whether its arm body runs; once the body runs, a `return` inside it is that path's last use of
/// every local it reads (#1489). The guard expression itself keeps the conservative counter, because a guard can read
/// a value, fail, and let a later arm execute.
#[test]
fn guarded_match_arm_returns_move_once_the_guard_admits_the_arm() -> TestResult {
    let (generated, rust) = generated_and_compact_rust(
        r#"
pub def guarded[T](value: T, choose_first: bool, admit_first: bool) -> T:
    match choose_first:
        case true if admit_first: return value
        case _: return value
"#,
    )?;
    let guarded = canonical_projection::projected_name(&generated, "guarded", SemanticSourceTargetKind::Function);

    assert!(
        rust.contains(&format!(
            "pubfn{guarded}<T,>(value:T,choose_first:bool,admit_first:bool)->T"
        )),
        "returning from each arm consumes the value on that path alone, so no Clone bound is needed:\n{rust}"
    );
    assert_eq!(
        rust.matches("returnvalue;").count(),
        2,
        "both the guarded arm and the fallback arm return the value by move:\n{rust}"
    );
    assert!(
        !rust.contains("value.clone()"),
        "a return inside an admitted guarded arm must not clone:\n{rust}"
    );
    Ok(())
}

/// A guard that reads a non-Copy value keeps the conservative read: the guard can fail and a later arm still needs
/// the value, so the read inside the guard clones while the admitted arm's `return` moves.
#[test]
fn guard_expression_reads_stay_conservative_while_the_admitted_return_moves() -> TestResult {
    let (_, rust) = generated_and_compact_rust(
        r#"
def admits(value: list[str]) -> bool:
    return len(value) > 0

pub def guarded(value: list[str], choose_first: bool) -> list[str]:
    match choose_first:
        case true if admits(value): return value
        case _: return value
"#,
    )?;

    assert!(
        rust.contains("(value.clone(),)=>{returnvalue;}"),
        "the guard may fail, so its read must not consume the value:\n{rust}"
    );
    assert_eq!(
        rust.matches("returnvalue;").count(),
        2,
        "each arm body returns the value by move once it runs:\n{rust}"
    );
    Ok(())
}

/// #1739: the issue's or-pattern, whose alternatives carry nested `str` literals, prints one guarded arm per
/// alternative. Printed as written, each literal would be a `&str` pattern token in an owned `String` position, and the
/// guard hoisting the literal serves one arm only; the guard shared by both alternatives reads `reviewer` without
/// consuming it, because it may run once per alternative.
#[test]
fn or_pattern_with_nested_str_literals_prints_one_guarded_arm_per_alternative_issue1739() -> TestResult {
    let (_, rust) = generated_and_compact_rust(
        r#"
def approved_by(reviewer: str) -> bool:
    return len(reviewer) > 0

pub def classify(pair: tuple[int, str], reviewer: str) -> str:
    match pair:
        (0, "a") | (1, "b") if approved_by(reviewer) => return "known"
        _ => return "other"
"#,
    )?;

    for (number, literal) in [(0, "a"), (1, "b")] {
        assert!(
            rust.contains(&format!(
                "({number},_,)ifmatch(&pair){{(_,__incan_match_member_0,)ifincan_std_core::strings::str_eq(&__incan_match_member_0,&\"{literal}\")=>true,_=>false,}}&&"
            )),
            "the alternative `({number}, \"{literal}\")` must print as its own arm testing its own literal on the scrutinee:\n{rust}"
        );
        assert!(
            !rust.contains(&format!("({number},\"{literal}\"")),
            "no alternative may keep its `str` literal as a pattern token:\n{rust}"
        );
    }
    assert_eq!(
        rust.matches("(reviewer.to_string(),)").count(),
        2,
        "each copy of the shared guard reads `reviewer` without consuming it:\n{rust}"
    );
    assert!(
        !rust.contains("(reviewer,)"),
        "no copy of the shared guard may move `reviewer`:\n{rust}"
    );
    Ok(())
}

/// #1740: a partial pattern whose unnamed fields include a private one prints `..` after the fields it names, the
/// only spelling that compiles where the model's module keeps that field private; the private field is never named.
/// A partial pattern whose unnamed fields are all public keeps spelling them as wildcards (#1708).
#[test]
fn partial_pattern_leaving_a_private_field_unnamed_prints_a_rest_marker_issue1740() -> TestResult {
    let (_, rust) = generated_and_compact_rust(
        r#"
pub model Account:
    pub kind: str
    pub tier: int
    _secret: int

pub model Plan:
    pub name: str
    pub seats: int

pub def describe(account: Account) -> str:
    match account:
        Account(kind="premium") => return "premium"
        _ => return "other"

pub def plan_name(plan: Plan) -> str:
    match plan:
        Plan(name="team") => return "team"
        _ => return "other"
"#,
    )?;

    assert!(
        rust.contains("{kind:_,..}ifmatch(&account){") && rust.contains("{kind:__incan_match_member_0,..}if"),
        "the pattern names `kind` and leaves the rest, private field included, to `..`, and the literal is tested on \
         the scrutinee through a pattern that names only `kind`:\n{rust}"
    );
    assert!(
        !rust.contains("_secret:_"),
        "the private field must not be named in a pattern outside its model's methods:\n{rust}"
    );
    assert!(
        rust.contains("{name:_,seats:_,}ifmatch(&plan){"),
        "a rest of public fields is still spelled as wildcards:\n{rust}"
    );
    Ok(())
}

/// #1739: under a guard, the arm for a later alternative that can match the same value as an earlier one tests that
/// the earlier one did not match, ahead of the guard, so the guard runs once and only for the first matching
/// alternative. A nested alternation that binds no names prints as one membership `match` on the scrutinee, so its
/// arm's body is printed once. A top-level `"a" | _` over a `str` value prints as a literal arm and a wildcard arm that
/// tests the literal did not match, never as the `"a" | _` token pattern.
#[test]
fn guarded_alternations_print_first_match_and_membership_tests_issue1739() -> TestResult {
    let (_, rust) = generated_and_compact_rust(
        r#"
def accept(name: str) -> bool:
    return name == "a"

pub def first_match(pair: tuple[str, str]) -> str:
    match pair:
        (x, "a") | ("b", x) if accept(x) => return x
        _ => return "none"

pub def tag(pair: tuple[int, Option[str]]) -> str:
    match pair:
        (n, Some("a") | None) if n > 0 => return "tagged"
        _ => return "other"

pub def long_word(value: str) -> str:
    match value:
        "a" | _ if len(value) > 3 => return "a or long"
        _ => return "short"
"#,
    )?;

    assert!(
        rust.contains(
            "(_,x,)ifmatch(&pair){(__incan_match_member_0,_,)ifincan_std_core::strings::str_eq(&__incan_match_member_0,&\"b\")=>true,_=>false,}&&!{match(&pair){(_,__incan_match_member_1,)if"
        ),
        "the second alternative's arm tests that the first did not match, ahead of the guard:\n{rust}"
    );
    assert!(
        rust.contains("(n,_,)ifmatch(&pair){") && rust.contains("(_,None)=>true,_=>false,}&&n>0=>"),
        "the nested alternation is one membership test on the scrutinee, ahead of the guard:\n{rust}"
    );
    assert_eq!(
        rust.matches("return\"tagged\".to_string();").count(),
        1,
        "the membership test keeps one arm, so the body is printed once:\n{rust}"
    );
    assert!(
        rust.contains("_if!{incan_std_core::strings::str_eq(&value,&\"a\")}&&"),
        "the wildcard alternative tests on the scrutinee that the literal did not match:\n{rust}"
    );
    assert!(
        !rust.contains("\"a\"|_"),
        "a `str` literal is never printed as a pattern token beside a wildcard:\n{rust}"
    );
    Ok(())
}

/// #1739: over a local `Result` scrutinee used after the `match`, a guarded alternation binds nothing new, so the
/// scrutinee keeps its value for the later `match`. A later alternative whose position holds a whole alternation that
/// binds names tests the scrutinee; over a temporary, that alternation is split and each piece tests its bound
/// position.
#[test]
fn first_match_tests_read_places_and_split_over_temporaries_issue1739() -> TestResult {
    let (_, rust) = generated_and_compact_rust(
        r#"
pub def ok_after_match(res: Result[Option[str], int], verbose: bool) -> str:
    match res:
        Ok(None) | Ok(_) if verbose => println("ok value")
        _ => println("other value")
    match res:
        Ok(_) => return "still ok"
        Err(code) => return f"error {code}"

pub def sign_of(pair: tuple[Result[int, int], int]) -> str:
    match pair:
        (Ok(1), n) | (Ok(n) | Err(n), _) if n != 0 => return f"{n}"
        _ => return "other"

pub def sign_of_parts(result: Result[int, int], count: int) -> str:
    match (result, count):
        (Ok(1), n) | (Ok(n) | Err(n), _) if n != 0 => return f"{n}"
        _ => return "other"
"#,
    )?;

    assert!(
        rust.contains("Ok(_,)if!{match(&res){Ok(None)=>true,_=>false,}}&&verbose=>"),
        "the `Ok(_)` arm tests the scrutinee through a shared view:\n{rust}"
    );
    assert!(
        !rust.contains("__incan_match_pos_"),
        "no arm binds a position of a local scrutinee:\n{rust}"
    );
    assert!(
        rust.contains("(Ok(n)|Err(n),_,)if!{match(&pair){(Ok(1),_)=>true,_=>false,}}&&n!=0=>"),
        "the whole alternation stays and its arm tests that `(Ok(1), n)` did not match the scrutinee:\n{rust}"
    );
    assert!(
        rust.contains("(Ok(n),_,)if!{match(&n){1=>true,_=>false,}}&&n!=0=>") && rust.contains("(Err(n),_)ifn!=0=>"),
        "over a temporary the alternation is split and the `Ok(n)` piece tests its bound position:\n{rust}"
    );
    Ok(())
}
