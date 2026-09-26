//! Pattern lowering: `match` arms (#1101), enum-variant, tuple, byte-string and or-patterns, the declared payload types
//! constructor patterns bind (#1245), `if let` / `while let` and destructuring comprehensions (#1161), `isinstance` as
//! a checked type test (#1281), and pattern, `raises` and panic-fact assertions.

use super::*;

// ---- #1101 B6: match ----

#[test]
fn lowers_a_literal_and_wildcard_match_as_a_single_structured_rvalue() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def classify(x: int) -> str:\n",
        "  match x:\n",
        "    case 0:\n",
        "      return \"zero\"\n",
        "    case _:\n",
        "      return \"other\"\n",
        "  return \"unreachable\"\n",
    );
    let module = build(source, &["m", "match_literal"])?;
    let snapshot_first = module.render_snapshot();
    let snapshot_second = build(source, &["m", "match_literal"])?.render_snapshot();
    assert_eq!(snapshot_first, snapshot_second, "lowering must be deterministic");

    assert!(
        snapshot_first.contains("match borrow(_0)"),
        "the scrutinee should be a single explicit read, not decomposed into ifs: {snapshot_first}"
    );
    assert!(
        snapshot_first.contains("const(0)"),
        "the literal pattern should render: {snapshot_first}"
    );
    assert!(
        snapshot_first.contains(" _ =>"),
        "the wildcard pattern should render: {snapshot_first}"
    );
    Ok(())
}

#[test]
fn nominal_pattern_without_checker_identity_stays_an_unresolved_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "model Point:\n",
        "  x: int\n",
        "\n",
        "def coordinate(point: Point) -> int:\n",
        "  match point:\n",
        "    case Point(x=value):\n",
        "      return value\n",
        "  return 0\n",
    );
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["m".to_string(), "missing_pattern_identity".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut type_info = checker.type_info().clone();
    type_info.references.resolved_identities.clear();

    let module = build_body_ir_module_v0(&program, &module_path, &type_info);
    let pattern = body_named(&module, "coordinate")?
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => arms.first().map(|arm| &arm.pattern),
            _ => None,
        })
        .ok_or("fixture must lower the match pattern")?;
    let bir::Pattern::Struct { canonical, .. } = pattern else {
        return Err(format!("missing identity must not admit a nominal target: {pattern:?}").into());
    };
    assert_eq!(canonical, &None);
    Ok(())
}

#[test]
fn lowers_an_enum_variant_pattern_that_binds_a_field() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def unwrap_or_zero(x: Option[int]) -> int:\n",
        "  match x:\n",
        "    case Some(value):\n",
        "      return value\n",
        "    case None:\n",
        "      return 0\n",
    );
    let module = build(source, &["m", "match_enum"])?;
    let snapshot = module.render_snapshot();

    // `Some`'s payload is the checker's recorded pattern type (#1245), so the binding is typed `int` and reads
    // `copy` rather than the non-Copy projected-read fallback an unresolved payload would force.
    assert!(
        snapshot.contains("Some(bind(_1, copy))"),
        "a positional constructor pattern should bind its typed field: {snapshot}"
    );
    assert!(
        snapshot.contains("const(none)"),
        "a bare `None` pattern is a literal, not a zero-field constructor: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_guarded_arm_with_the_guard_seeing_the_pattern_binding() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def sign(x: int) -> str:\n",
        "  match x:\n",
        "    case n if n > 0:\n",
        "      return \"positive\"\n",
        "    case n if n < 0:\n",
        "      return \"negative\"\n",
        "    case _:\n",
        "      return \"zero\"\n",
    );
    let module = build(source, &["m", "match_guard"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains(" if "),
        "a guarded arm should render its guard: {snapshot}"
    );
    // `n` binds `_1`/`_3` in the two arms; the guard should read that same pattern-bound local, not the
    // scrutinee's own `_0` -- confirming the guard sees the pattern binding, not a re-read of the scrutinee.
    assert!(
        snapshot.contains("bind(_1, copy) if { _2 = copy(_1) > const(0);"),
        "the first arm's guard should read the pattern-bound `n` (`_1`): {snapshot}"
    );
    assert!(
        snapshot.contains("bind(_3, copy) if { _4 = copy(_3) < const(0);"),
        "the second arm's guard should read its own pattern-bound `n` (`_3`): {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_nested_tuple_pattern_with_field_projected_bindings() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def sum_pair(pair: (int, int)) -> int:\n",
        "  match pair:\n",
        "    case (a, b):\n",
        "      return a + b\n",
    );
    let module = build(source, &["m", "match_tuple"])?;
    let snapshot = module.render_snapshot();

    // Unlike a `Struct`/`Enum` constructor pattern's fields (`Unknown`-typed, see the enum test above), a
    // `Tuple` pattern's element types are resolved precisely via the already-established `tuple_element_types`
    // helper (`BodyBuilder::lower_tuple_unpack`'s own precedent), so both bindings declare as real `int`s...
    assert!(snapshot.contains("local 1 a : int [binding]"));
    assert!(snapshot.contains("local 2 b : int [binding]"));
    // ...and, being Copy `int`s read through a non-empty (tuple-element) projection, read as `copy`, never
    // `move` -- a projected read never moves (see `ownership_fact_for_place`'s own docs).
    assert!(
        snapshot.contains("(bind(_1, copy), bind(_2, copy))"),
        "a tuple pattern should recursively bind each element as a copy: {snapshot}"
    );
    Ok(())
}

#[test]
fn byte_string_literal_pattern_is_refused_before_body_ir_issue1741() -> Result<(), Box<dyn std::error::Error>> {
    // `bir::Constant::Bytes` represents a byte value, but the closed `bir::Pattern` vocabulary does not model
    // byte-pattern matching semantics, and the build has no pattern form for a bytes literal either. The checker
    // refuses the pattern (#1741), so checked source never reaches Body IR's own placeholder for it.
    let source = concat!(
        "def check(data: bytes) -> str:\n",
        "  match data:\n",
        "    case b\"\\x00\":\n",
        "      return \"null\"\n",
        "    case _:\n",
        "      return \"other\"\n",
    );
    match build(source, &["m", "match_bytes"]) {
        Ok(module) => Err(format!(
            "a bytes literal pattern must be refused before Body IR: {}",
            module.render_snapshot()
        )
        .into()),
        Err(error)
            if error
                .to_string()
                .contains("bytes literal cannot be used as a match pattern") =>
        {
            Ok(())
        }
        Err(error) => Err(format!("expected the bytes-pattern refusal, got: {error}").into()),
    }
}

#[test]
fn or_pattern_alternatives_share_one_local_for_a_bound_name() -> Result<(), Box<dyn std::error::Error>> {
    // RFC 071 requires every `A(x) | B(x)` alternative to bind an identical name/type set, so Rust's own
    // compiled target has exactly one shared binding slot for `x`, not one per alternative -- `seen` in
    // `BodyBuilder::lower_match_pattern` reuses the same local for the second occurrence rather than declaring
    // a second one.
    let source = concat!(
        "enum Shape:\n",
        "  Circle(int)\n",
        "  Square(int)\n",
        "\n",
        "def get_size(s: Shape) -> int:\n",
        "  match s:\n",
        "    case Circle(x) | Square(x):\n",
        "      return x\n",
    );
    let module = build(source, &["m", "match_or_binding"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("Circle(bind(_1, copy)) canonical=")
            && snapshot.contains("Square(bind(_1, copy)) canonical="),
        "both canonical alternatives should bind the same shared local `_1`: {snapshot}"
    );
    Ok(())
}

// ---- #1245: constructor pattern bindings carry their declared payload type ----

/// The declared type and ownership fact of the sole local named `name`, as `(type, fact)` read off the one
/// `bind(..)` occurrence for it in the rendered snapshot.
fn bound_local_type_and_fact(
    module: &bir::BodyIrModule,
    body_name: &str,
    name: &str,
) -> Result<(IncanType, String), Box<dyn std::error::Error>> {
    let body = body_named(module, body_name)?;
    let local = sole_local_named(body, name)?;
    let ty = body
        .locals
        .get(local.index())
        .map(|decl| decl.ty.clone())
        .ok_or("the bound local must be present in the body's locals")?;
    let snapshot = module.render_snapshot();
    let needle = format!("bind(_{}, ", local.0);
    let fact = snapshot
        .split(&needle)
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| format!("no `{needle}` occurrence in the snapshot: {snapshot}"))?
        .to_string();
    Ok((ty, fact))
}

#[test]
fn a_user_enum_alternation_binds_its_declared_payload_type_in_a_match() -> Result<(), Box<dyn std::error::Error>> {
    // The `?`-typed binding #1245 reports: every constructor other than `Ok`/`Err` used to lower its payload
    // sub-patterns against `Unknown`, so `v` carried no type and an `Unknown` ownership fact.
    let source = concat!(
        "enum Shape:\n",
        "  Circle(int)\n",
        "  Square(int)\n",
        "\n",
        "def size(s: Shape) -> int:\n",
        "  match s:\n",
        "    case Shape.Circle(v) | Shape.Square(v):\n",
        "      return v\n",
    );
    let module = build(source, &["m", "enum_payload_match"])?;
    let (ty, fact) = bound_local_type_and_fact(&module, "size", "v")?;
    assert_eq!(
        ty,
        IncanType::Primitive(IncanPrimitiveType::Int),
        "`v` must carry the variant's declared `int` payload type: {}",
        module.render_snapshot()
    );
    assert_eq!(
        fact, "copy",
        "an `int` payload reads by copy, not through the unknown-type fallback"
    );
    Ok(())
}

#[test]
fn a_user_enum_string_payload_binds_as_a_borrowed_str() -> Result<(), Box<dyn std::error::Error>> {
    // A non-Copy payload proves the fact follows from the type: a projected read of a `str` borrows.
    let source = concat!(
        "enum Message:\n",
        "  Text(str)\n",
        "  Blank\n",
        "\n",
        "def body(m: Message) -> str:\n",
        "  match m:\n",
        "    case Message.Text(t):\n",
        "      return t\n",
        "    case Message.Blank:\n",
        "      return \"\"\n",
    );
    let module = build(source, &["m", "enum_str_payload"])?;
    let (ty, fact) = bound_local_type_and_fact(&module, "body", "t")?;
    assert_eq!(
        ty,
        IncanType::Primitive(IncanPrimitiveType::Str),
        "`t` must carry the variant's declared `str` payload type: {}",
        module.render_snapshot()
    );
    assert_eq!(fact, "borrow", "a `str` payload read through a projection borrows");
    Ok(())
}

#[test]
fn an_if_let_option_payload_binds_its_element_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def run(o: Option[int]) -> int:\n",
        "  mut total = 0\n",
        "  if let Some(v) = o:\n",
        "    total = v\n",
        "  return total\n",
    );
    let module = build(source, &["m", "if_let_option_payload"])?;
    let (ty, fact) = bound_local_type_and_fact(&module, "run", "v")?;
    assert_eq!(
        ty,
        IncanType::Primitive(IncanPrimitiveType::Int),
        "`Some(v)` over `Option[int]` must bind `v : int`: {}",
        module.render_snapshot()
    );
    assert_eq!(fact, "copy");
    Ok(())
}

#[test]
fn a_pattern_assertion_option_payload_binds_its_element_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def run(o: Option[str]) -> None:\n  assert o is Some(v)\n  print(v)\n";
    let module = build(source, &["m", "assert_option_payload"])?;
    let (ty, fact) = bound_local_type_and_fact(&module, "run", "v")?;
    assert_eq!(
        ty,
        IncanType::Primitive(IncanPrimitiveType::Str),
        "`Some(v)` over `Option[str]` must bind `v : str`: {}",
        module.render_snapshot()
    );
    assert_eq!(fact, "borrow");
    Ok(())
}

#[test]
fn a_multi_payload_user_enum_variant_binds_each_positional_type() -> Result<(), Box<dyn std::error::Error>> {
    // The checked fact is recorded per pattern node, so each positional payload carries its own declared type
    // rather than one shared guess for the whole constructor.
    let source = concat!(
        "enum Pair:\n",
        "  Both(int, str)\n",
        "\n",
        "def first(p: Pair) -> int:\n",
        "  match p:\n",
        "    case Pair.Both(n, label):\n",
        "      return n\n",
    );
    let module = build(source, &["m", "enum_multi_payload"])?;
    let (n_ty, n_fact) = bound_local_type_and_fact(&module, "first", "n")?;
    let (label_ty, label_fact) = bound_local_type_and_fact(&module, "first", "label")?;
    assert_eq!(n_ty, IncanType::Primitive(IncanPrimitiveType::Int));
    assert_eq!(n_fact, "copy");
    assert_eq!(label_ty, IncanType::Primitive(IncanPrimitiveType::Str));
    assert_eq!(label_fact, "borrow");
    Ok(())
}

/// Return the [`bir::LocalId`] of the single local named `name` in `body`, failing when the body declares none or
/// more than one.
///
/// Used by the `assert value is P` binding tests: the defect those cover is a *missing* declaration, so a test that
/// merely found "some local mentioning `v`" would pass against the broken lowering, where a later read of `v`
/// synthesizes an [`bir::LocalOrigin::External`] local under the same name.
fn sole_local_named(body: &bir::Body, name: &str) -> Result<bir::LocalId, Box<dyn std::error::Error>> {
    let matches: Vec<&bir::LocalDecl> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some(name))
        .collect();
    match matches.as_slice() {
        [local] => Ok(local.id),
        found => Err(format!("expected exactly one local named `{name}`, found {found:?}").into()),
    }
}

/// Return the [`bir::AssertionKind`]s of every assertion in `body`'s top-level block, in statement order.
fn assertion_kinds(body: &bir::Body) -> Vec<&bir::AssertionKind> {
    body.block
        .stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            bir::StatementKind::Assert { kind, .. } => Some(kind),
            _ => None,
        })
        .collect()
}

#[test]
fn a_pattern_assertion_binding_is_a_declared_local_read_by_the_statements_after_it()
-> Result<(), Box<dyn std::error::Error>> {
    // The defect #1167 closes: `assert o is Some(v)` used to lower to a bare placeholder, which dropped `v`
    // entirely. `print(v)` then lowered against a name this body never declared, and the unresolved-name recovery
    // path invented an `External` local for it -- so Body IR described a read of something outside the body rather
    // than of the value the assertion had just bound.
    let source = "def run(o: Option[str]) -> None:\n  assert o is Some(v)\n  print(v)\n";
    let module = build(source, &["m", "assert_pattern"])?;
    let body = body_named(&module, "run")?;

    let bound = sole_local_named(body, "v")?;
    let declaration = body
        .locals
        .get(bound.index())
        .ok_or("the bound local must be present in the body's locals")?;
    assert!(
        matches!(declaration.origin, bir::LocalOrigin::UserBinding),
        "the assertion must declare `v` as an ordinary source binding, not an external reference: {declaration:?}"
    );

    let [bir::AssertionKind::Pattern { pattern, .. }] = assertion_kinds(body).as_slice() else {
        return Err(format!("expected exactly one pattern assertion: {:?}", body.block.stmts).into());
    };
    let bir::Pattern::Enum {
        canonical,
        variant,
        fields,
        ..
    } = pattern.as_ref()
    else {
        return Err(format!("expected `Some(..)` to lower to a constructor pattern: {pattern:?}").into());
    };
    assert!(
        canonical.is_some(),
        "a successfully checked constructor pattern must retain its exact canonical target"
    );
    assert_eq!(
        variant,
        constructors::as_str(constructors::ConstructorId::Some),
        "the assertion must lower `Some(..)` to the registry's own constructor spelling",
    );
    let [bir::Pattern::Var(binding)] = fields.as_slice() else {
        return Err(format!("expected one `PatternBinding` payload: {fields:?}").into());
    };
    assert_eq!(
        binding.local, bound,
        "the pattern binding must name the same local the body declared"
    );

    // The read that follows the assertion resolves to that same local, and consumes it: `print(v)` is the only
    // read, so the last-use countdown seeded from the assertion's statement suffix must reach zero here.
    let snapshot = module.render_snapshot();
    assert!(
        snapshot.contains(&format!("call fn:print unbound(move(_{}, last_use))", bound.0)),
        "the following read must resolve to the bound local: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_pattern_assertion_over_a_result_resolves_its_payload_type_and_carries_a_message()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "def run(r: Result[int, str]) -> None:\n  assert r is Ok(n), \"needed a value\"\n  print(n)\n";
    let module = build(source, &["m", "assert_ok"])?;
    let body = body_named(&module, "run")?;
    let bound = sole_local_named(body, "n")?;
    assert_eq!(
        body.locals.get(bound.index()).map(|local| &local.ty),
        Some(&IncanType::Primitive(IncanPrimitiveType::Int)),
        "an intrinsic `Result` pattern resolves its payload type rather than falling back to unknown"
    );

    let snapshot = module.render_snapshot();
    assert!(
        snapshot.contains(&format!(
            "assert borrow(_0) is Result::ok(bind(_{}, copy)), const(\"needed a value\") may_panic",
            bound.0
        )),
        "the pattern form must carry its failure message alongside the binding: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_raises_assertion_retains_the_resolved_expected_error_rather_than_its_spelling()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "def boom() -> int:\n  return 1\n\ndef run() -> None:\n  assert boom() raises ValueError\n  assert boom() raises IndexError, \"wanted an index error\"\n";
    let module = build(source, &["m", "assert_raises"])?;
    let body = body_named(&module, "run")?;

    let expected: Vec<incan_lang::errors::ErrorKind> = assertion_kinds(body)
        .into_iter()
        .filter_map(|kind| match kind {
            bir::AssertionKind::Raises { expected_error, .. } => Some(*expected_error),
            _ => None,
        })
        .collect();
    assert_eq!(
        expected,
        vec![
            incan_lang::errors::ErrorKind::ValueError,
            incan_lang::errors::ErrorKind::IndexError
        ],
        "the expected error must be the resolved registry identity, not a source spelling"
    );

    let snapshot = module.render_snapshot();
    assert!(
        snapshot.contains("raises ValueError may_panic"),
        "a `raises` assertion without a message: {snapshot}"
    );
    assert!(
        snapshot.contains("raises IndexError, const(\"wanted an index error\") may_panic"),
        "a `raises` assertion with a message: {snapshot}"
    );
    Ok(())
}

#[test]
fn every_assert_form_records_a_panic_fact_and_the_panic_strategy_requirement() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "def boom() -> int:\n  return 1\n\ndef run(o: Option[str]) -> None:\n  assert 1 == 1\n  assert o is Some(v)\n  assert boom() raises ValueError\n  print(v)\n";
    let module = build(source, &["m", "assert_panic_facts"])?;
    let body = body_named(&module, "run")?;

    assert_eq!(
        body.panic_facts
            .iter()
            .filter(|fact| matches!(fact.reason, bir::PanicReason::AssertFailure))
            .count(),
        3,
        "all three assertion forms can panic: {:?}",
        body.panic_facts
    );
    assert!(
        body.runtime_requirements
            .contains(&AbiV0RuntimeRequirement::PanicStrategy),
        "an assertion of any form needs a panic strategy: {:?}",
        body.runtime_requirements
    );
    let snapshot = module.render_snapshot();
    assert!(
        !snapshot.contains("unsupported("),
        "no accepted assertion form may leave a placeholder behind: {snapshot}"
    );
    Ok(())
}

#[test]
fn an_unresolved_raises_error_type_refuses_by_naming_the_assert_form() -> Result<(), Box<dyn std::error::Error>> {
    // The source checker rejects an error type outside the builtin-exception registry, so lowering only reaches
    // this for a program that was already reported. What it must not do is fall back to the old shared
    // `assert pattern/raises form` label, which said nothing about which of the two forms was hit.
    let source = "def boom() -> int:\n  return 1\n\ndef run() -> None:\n  assert boom() raises NotAnError\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "assert_refusal"])?;
    assert!(
        diagnostics.iter().any(|message| message.contains("NotAnError")),
        "the source checker owns the diagnostic for an unknown error type: {diagnostics:?}"
    );

    let snapshot = module.render_snapshot();
    assert!(
        snapshot.contains("unsupported(assert `raises` form with an unresolved error type `NotAnError`)"),
        "the refusal must name the `raises` form and the type it could not resolve: {snapshot}"
    );
    assert!(
        !snapshot.contains("assert pattern/raises form"),
        "the shared label that could not distinguish the two forms is gone: {snapshot}"
    );
    Ok(())
}

// ---- Pattern conditions (#1161) ----

/// `if let` lowers to the single-arm match RFC 049 describes, with an implicit non-matching fallthrough.
#[test]
fn if_let_lowers_to_a_single_arm_match_with_an_empty_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def run(o: Option[int]) -> int:\n",
        "  mut total = 0\n",
        "  if let Some(v) = o:\n",
        "    total = v\n",
        "  return total\n",
    );
    let module = build(source, &["m", "if_let"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "`if let` must lower without a placeholder: {snapshot}"
    );
    assert!(
        snapshot.contains("Some(bind("),
        "the pattern must bind its payload: {snapshot}"
    );
    // RFC 049's own reading: a single-arm `match` plus an implicit `_ => pass`.
    assert!(
        snapshot.contains("_ => const(())"),
        "the non-matching path must be an explicit wildcard arm: {snapshot}"
    );
    Ok(())
}

/// A failed pattern condition is control flow, not a panic.
#[test]
fn if_let_records_no_panic_fact_for_a_non_matching_pattern() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def run(o: Option[int]) -> int:\n",
        "  mut total = 0\n",
        "  if let Some(v) = o:\n",
        "    total = v\n",
        "  return total\n",
    );
    let module = build(source, &["m", "if_let_panic"])?;
    let body = body_named(&module, "run")?;

    // `assert value is P` panics on the same shape; a pattern *condition* must not.
    assert!(
        body.panic_facts.is_empty(),
        "a non-matching `if let` is ordinary control flow, not a panic: {:?}",
        body.panic_facts
    );
    Ok(())
}

/// `while let` re-evaluates its subject each iteration and exits by breaking when the pattern stops matching.
#[test]
fn while_let_re_evaluates_its_subject_and_breaks_when_exhausted() -> Result<(), Box<dyn std::error::Error>> {
    // The subject is a call precisely so re-evaluation is observable: a hoisted subject would call `pop` once and
    // loop forever on the same value.
    let source = concat!(
        "def pop() -> Option[int]:\n",
        "  return None\n",
        "\n",
        "def run() -> int:\n",
        "  mut total = 0\n",
        "  while let Some(item) = pop():\n",
        "    total = total + item\n",
        "  return total\n",
    );
    let module = build(source, &["m", "while_let"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "`while let` must lower without a placeholder: {snapshot}"
    );
    let loop_at = snapshot.find("loop").ok_or("expected a loop statement")?;
    let call_at = snapshot.find("call fn:pop").ok_or("expected the subject call")?;
    assert!(
        call_at > loop_at,
        "the subject must be re-evaluated inside the loop, not hoisted above it: {snapshot}"
    );
    assert!(
        snapshot.contains("break"),
        "an exhausted pattern must break rather than panic: {snapshot}"
    );
    Ok(())
}

/// A destructuring comprehension binds the same facts the equivalent statement `for` does.
///
/// This is the parity the issue turns on: two spellings of one iteration must not differ in representability, and
/// the earlier gap was not the binding but its *type* -- the comprehension bound `a` as `?` where the statement
/// form bound it as `int`.
#[test]
fn a_destructuring_comprehension_binds_like_the_equivalent_statement_for() -> Result<(), Box<dyn std::error::Error>> {
    let comprehension = build(
        concat!(
            "def run(pairs: List[Tuple[int, int]]) -> List[int]:\n",
            "  return [a + b for a, b in pairs]\n",
        ),
        &["m", "comp"],
    )?;
    let statement_form = build(
        concat!(
            "def run(pairs: List[Tuple[int, int]]) -> int:\n",
            "  mut total = 0\n",
            "  for a, b in pairs:\n",
            "    total = total + a + b\n",
            "  return total\n",
        ),
        &["m", "stmt"],
    )?;

    let comp_snapshot = comprehension.render_snapshot();
    assert!(
        !comp_snapshot.contains("unsupported("),
        "a destructuring comprehension must lower without a placeholder: {comp_snapshot}"
    );

    for (name, module) in [("comprehension", &comprehension), ("statement for", &statement_form)] {
        let body = body_named(module, "run")?;
        for binding in ["a", "b"] {
            let local = sole_local_named(body, binding)?;
            let declared = body
                .locals
                .get(local.index())
                .ok_or_else(|| format!("{name}: `{binding}` is missing from the body's locals"))?;
            assert_eq!(
                declared.ty,
                IncanType::Primitive(IncanPrimitiveType::Int),
                "{name}: `{binding}` must carry the tuple element's resolved type, not `?`",
            );
        }
    }
    Ok(())
}

/// `if let` accepts pattern alternation (RFC 071), and lowering must not narrow that.
#[test]
fn if_let_lowers_an_alternated_pattern() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "enum Shape:\n",
        "  Circle(int)\n",
        "  Square(int)\n",
        "  Blank\n",
        "\n",
        "def run(s: Shape) -> int:\n",
        "  mut n = 0\n",
        "  if let Shape.Circle(v) | Shape.Square(v) = s:\n",
        "    n = v\n",
        "  return n\n",
    );
    let module = build(source, &["m", "if_let_alt"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "an alternated `if let` pattern must lower: {snapshot}"
    );
    // Both alternatives bind the same name, so it must be one declared local rather than two.
    let bindings: Vec<&str> = snapshot.lines().filter(|line| line.contains(" v : ")).collect();
    assert_eq!(bindings.len(), 1, "`v` must be a single declared local: {bindings:?}");
    assert!(
        bindings[0].contains("[binding]"),
        "`v` must be a source binding, not a temp or external: {}",
        bindings[0]
    );
    // This lowering reuses `lower_match_pattern` rather than reimplementing it, so it shares the payload typing a
    // statement `match` gets (#1245): `v` is the variant's declared `int`, not `?`.
    assert!(
        bindings[0].contains(" v : int "),
        "`v` must carry the alternated variants' shared payload type: {}",
        bindings[0]
    );
    Ok(())
}

// ---- #1281: `isinstance` is an explicit checked Body-IR type test ----

#[test]
fn lowers_isinstance_as_a_typed_test_without_a_runtime_type_operand() -> Result<(), Box<dyn std::error::Error>> {
    let source = "type Text = str\n\ndef probe(value: int | str) -> bool:\n  return isinstance(value, Text)\n";
    let module = build(source, &["m", "isinstance"])?;
    let snapshot = module.render_snapshot();
    let target_start = source.rfind("Text").ok_or("fixture must contain the target spelling")?;

    assert!(
        snapshot.contains(&format!(
            "isinstance(move(_0, last_use): Union[int, str], target=str@{target_start}..{}",
            target_start + 4
        )),
        "the typed test must retain the resolved target and its exact source span: {snapshot}"
    );
    assert!(
        !snapshot.contains("call builtin:isinstance"),
        "the target type must not be lowered as an ordinary runtime call argument: {snapshot}"
    );
    Ok(())
}

#[test]
fn missing_checked_isinstance_target_lowers_to_an_explicit_target_span_refusal()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "def probe(value: int | str) -> bool:\n  return isinstance(value, str)\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut type_info = checker.type_info().clone();
    let target = type_info
        .calls
        .isinstance_targets
        .values()
        .next()
        .cloned()
        .ok_or("fixture must record an isinstance target")?;
    type_info.calls.isinstance_targets.clear();
    let module_path = vec!["m".to_string(), "missing_isinstance_target".to_string()];
    let module = build_body_ir_module_v0(&program, &module_path, &type_info);
    let body = body_named(&module, "probe")?;
    let refusal_span = body
        .block
        .stmts
        .iter()
        .find_map(|statement| {
            matches!(
                &statement.kind,
                bir::StatementKind::Unsupported { description }
                    if description == "isinstance without checked target evidence"
            )
            .then_some(statement.span)
        })
        .ok_or("missing target evidence must lower to an explicit refusal")?;
    assert_eq!(
        refusal_span,
        HirSourceSpan::new(target.span.start, target.span.end),
        "missing evidence must refuse at the target expression rather than guessing from its spelling"
    );
    Ok(())
}
