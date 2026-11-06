//! Control flow: `if` / `while` / `for` normalization, division and assert panic facts, `for` over builtin lists,
//! user-defined and fallible iteration protocols, loop-scoped bindings, expression-position `if`, value-carrying `loop`
//! expressions, `try` propagation, range values and the facts a bound range shares with an inline one (#1165), and
//! statement-position `loop:` bodies (#1162).

use super::*;

/// An unsafe region retains its source receipt and exact nested operations before any consumer admits it.
#[test]
fn unsafe_region_retains_acknowledgment_receipt_and_operations() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def probe(value: int) -> int:\n  return value\n\ndef touch(value: int) -> int:\n  mut total = 0\n  unsafe:\n    total = probe(value)\n  return total\n",
        &["m", "unsafe_receipt"],
    )?;
    let body = body_named(&module, "touch")?;
    let value = serde_json::to_value(body)?;
    let statements = value["block"]["stmts"].as_array().ok_or("missing body statements")?;
    let statement = statements
        .iter()
        .find(|statement| statement["kind"].get("UnsafeRegion").is_some())
        .ok_or("missing unsafe acknowledgment region")?;
    let region = &statement["kind"]["UnsafeRegion"];
    assert_eq!(region["acknowledgment"]["receipt"]["kind"], "Statement");
    assert_eq!(region["acknowledgment"]["span"], statement["span"]);
    assert_eq!(region["acknowledgment"]["scope"], value["block"]["scope"]);
    assert_eq!(region["body"]["scope"], value["block"]["scope"]);
    assert!(
        region["body"]["stmts"]
            .as_array()
            .is_some_and(|operations| operations.len() >= 2)
    );
    Ok(())
}

/// A declaration inside an acknowledged region remains in the enclosing Incan scope, as legacy flattening requires.
#[test]
fn unsafe_region_keeps_enclosing_scope_and_later_reads() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def main() -> None:\n  unsafe:\n    value = \"retained\"\n  println(value)\n",
        &["m", "unsafe_scope"],
    )?;
    let body = body_named(&module, "main")?;
    assert_eq!(body.scopes.len(), 1);
    assert!(body.locals.iter().any(|local| local.name.as_deref() == Some("value")));
    let value = serde_json::to_value(body)?;
    assert!(
        value["block"]["stmts"].as_array().is_some_and(|statements| statements
            .iter()
            .any(|statement| statement["kind"].get("UnsafeRegion").is_some())),
        "{}",
        body.render_snapshot()
    );
    assert!(
        !body.render_snapshot().contains("unsupported"),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

#[test]
fn lowers_if_while_and_for_into_normalized_control_flow() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def run(n: int) -> int:\n  mut total = 0\n  for i in 0..n:\n    if i > 2:\n      total = total + i\n  while total > 100:\n    total = total - 1\n  return total\n";
    let module = build(source, &["m", "control"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("loop:"),
        "for/while should desugar to a normalized loop: {snapshot}"
    );
    assert!(snapshot.contains("if "));
    assert!(snapshot.contains("break"));
    Ok(())
}

#[test]
fn lowers_division_and_assert_as_explicit_panic_facts() -> Result<(), Box<dyn std::error::Error>> {
    // Floor division keeps an `int` result (true division promotes to `float`), so this stays a same-type return.
    let source = "def div(a: int, b: int) -> int:\n  assert b != 0\n  return a // b\n";
    let module = build(source, &["m", "panics"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("panic_facts:"));
    assert!(snapshot.contains("assert_failure"));
    assert!(snapshot.contains("division_or_modulo"));
    assert!(snapshot.contains("panic_strategy"));
    Ok(())
}

#[test]
fn lowers_for_over_a_builtin_list_using_the_builtin_iter_protocol() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def total(items: list[int]) -> int:\n  mut acc = 0\n  for x in items:\n    acc = acc + x\n  return acc\n";
    let module = build(source, &["m", "builtin_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("iter_next(mut_borrow("),
        "builtin for should poll via IterNext: {snapshot}"
    );
    assert!(
        snapshot.contains(", builtin)"),
        "builtin collection iteration should use IterProtocol::Builtin: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "should not fall back to Unsupported: {snapshot}"
    );
    Ok(())
}

#[test]
fn for_pattern_bindings_do_not_escape_the_loop_scope() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep_outer(x: int, items: list[int]) -> int:\n  for x in items:\n    pass\n  return x\n";
    let module = build(source, &["m", "for_scope"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("return copy(_0)"),
        "the trailing read must resolve the enclosing parameter, not the for-pattern local: {snapshot}"
    );
    let body = body_named(&module, "keep_outer")?;
    let identities: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("x"))
        .map(|local| local.identity.as_ref())
        .collect();
    let [Some(parameter), Some(loop_binding)] = identities.as_slice() else {
        return Err(format!("expected canonical identities for the parameter and loop binding: {body:?}").into());
    };
    assert_ne!(parameter, loop_binding, "shadowed bindings need distinct identities");
    Ok(())
}

/// A checked structural iterator calls its selected hooks without fallible propagation.
#[test]
fn lowers_for_over_a_user_defined_iteration_protocol() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model CounterIter:\n  value: int\n  limit: int\n\n  def __next__(self) -> Option[int]:\n    if self.value < self.limit:\n      return Some(self.value)\n    return None\n\nmodel Counter:\n  limit: int\n\n  def __iter__(self) -> CounterIter:\n    return CounterIter(value=0, limit=self.limit)\n\ndef total() -> int:\n  mut acc = 0\n  for item in Counter(limit=3):\n    acc = acc + item\n  return acc\n";
    let module = build(source, &["m", "protocol_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("call method:__iter__"),
        "should call the resolved __iter__ method to obtain an iterator: {snapshot}"
    );
    assert!(
        snapshot.contains("call method:__next__") && !snapshot.contains("try?("),
        "should poll via the resolved __next__ method, non-fallible: {snapshot}"
    );
    Ok(())
}

/// A checked fallible iterator preserves polling and propagates its checked error explicitly.
#[test]
fn lowers_fallible_for_iteration_with_an_implicit_try_propagate_semantic() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model ChunkStream:\n  def __iter__(self) -> ChunkStream:\n    return self\n\n  def __next__(self) -> Result[Option[int], str]:\n    return Ok(None)\n\ndef total() -> Result[int, str]:\n  mut acc = 0\n  for chunk in ChunkStream()?:\n    acc = acc + chunk\n  return Ok(acc)\n";
    let module = build(source, &["m", "fallible_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("call method:__next__") && snapshot.contains("try?("),
        "fallible protocol iteration must retain checked polling and propagation: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_expression_position_if_as_unit_typed() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def maybe_print(flag: bool) -> None:\n  if flag:\n    pass\n  else:\n    pass\n  return\n";
    // `if` used purely as a statement already covers the statement-position path; this test instead exercises
    // the expression-position path via a plain expression statement wrapping an `if` expression's value.
    let source_expr = "def maybe(flag: bool) -> None:\n  _ = if flag:\n    pass\n  else:\n    pass\n  return\n";
    let _ = build(source, &["m", "if_stmt"])?; // sanity: statement-position if still works unchanged
    let module = build(source_expr, &["m", "if_expr"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "expression-position if should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains("const(())"),
        "an if-expression's value should be the Unit constant: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_loop_expression_break_value_into_a_merged_result_place() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def find(flag: bool) -> int:\n  return loop:\n    if flag:\n      break 42\n    break 7\n";
    let module = build(source, &["m", "loop_expr"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "loop-expression should not fall back: {snapshot}"
    );
    // Both `break 42` and `break 7` should have been rewritten into an assignment to the shared result local
    // followed by a plain, valueless `break`, rather than carrying a value on `Break` itself.
    assert!(snapshot.contains("const(42)"));
    assert!(snapshot.contains("const(7)"));
    assert!(
        !snapshot.contains("break const"),
        "break value should be assigned into the result place, not carried on `break`: {snapshot}"
    );
    Ok(())
}

/// A loop expression's result belongs to its enclosing scope and survives the loop body's scope-exit drops.
#[test]
fn loop_expression_result_outlives_the_loop_scope() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def find(flag: bool) -> Option[str]:\n  return loop:\n    if flag:\n      break None\n    break Some(\"kept\")\n";
    let module = build(source, &["m", "loop_result_scope"])?;
    let body = body_named(&module, "find")?;
    let bir::StatementKind::Loop { body: loop_body } = &body.block.stmts[0].kind else {
        return Err(format!("expected a normalized loop: {body:?}").into());
    };
    let bir::StatementKind::Return {
        value: Some(bir::Operand::Place(read)),
    } = &body.block.stmts[1].kind
    else {
        return Err(format!("expected the loop result to be returned: {body:?}").into());
    };
    let result = read.place.local_id().ok_or("loop result is not a local")?;
    assert_eq!(body.locals[result.index()].scope, body.block.scope);
    assert_ne!(body.locals[result.index()].scope, loop_body.scope);
    Ok(())
}

#[test]
fn nested_while_break_inside_a_loop_expression_does_not_target_the_outer_loop() -> Result<(), Box<dyn std::error::Error>>
{
    // A plain `break` inside a nested `while` must exit the `while`, not accidentally get rewritten into an
    // assignment to the outer `loop:` expression's result place.
    let source = "def find(limit: int) -> int:\n  return loop:\n    mut i = 0\n    while i < limit:\n      if i == 5:\n        break\n      i = i + 1\n    break i\n";
    let module = build(source, &["m", "nested_loop"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "nested while/loop should not fall back: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_try_into_an_explicit_try_propagate_statement() -> Result<(), Box<dyn std::error::Error>> {
    let source = "enum E:\n  Bad\n\ndef half(x: int) -> Result[int, E]:\n  if x % 2 != 0:\n    return Err(E.Bad)\n  return Ok(x // 2)\n\ndef quarter(x: int) -> Result[int, E]:\n  h = half(x)?\n  return half(h)\n";
    let module = build(source, &["m", "try_expr"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("= try?("),
        "`?` should lower to an explicit try-propagate statement: {snapshot}"
    );
    assert!(
        snapshot.contains("same_error_type=E") && snapshot.contains("result_ok(") && snapshot.contains("result_err("),
        "Result constructors and exact error routing must stay explicit in Body IR: {snapshot}"
    );
    Ok(())
}

/// Find the last binding with `name`, used where a later same-name assignment shadows the earlier value.
fn last_local_for_binding(snapshot: &str, name: &str) -> Option<String> {
    snapshot
        .lines()
        .filter_map(|line| {
            let (id, tail) = line.trim().strip_prefix("local ")?.split_once(' ')?;
            tail.starts_with(&format!("{name} : ")).then(|| format!("_{id}"))
        })
        .next_back()
}

#[test]
fn a_range_bound_to_a_local_lowers_to_a_range_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def build_ranges() -> int:\n",
        "  half_open = 0..10\n",
        "  closed = 1..=5\n",
        "  return 0\n",
    );
    let module = build(source, &["m", "range_value"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a range in value position must lower to a real operand: {snapshot}"
    );
    assert!(
        snapshot.contains("range[const(0), const(10), const(1), const(false)]"),
        "an exclusive range must carry its bounds, unit step, and `false` inclusivity: {snapshot}"
    );
    assert!(
        snapshot.contains("range[const(1), const(5), const(1), const(true)]"),
        "an inclusive range must differ from the exclusive one only in its inclusivity operand: {snapshot}"
    );
    assert!(
        snapshot.contains(" half_open : Range[int] [binding]"),
        "the bound local must keep the checked range type: {snapshot}"
    );
    Ok(())
}

/// The facts two range spellings must agree on, read out of a lowered body's single `Loop` statement.
///
/// Deliberately not a snapshot comparison: a bound range reads its bounds off a value while an inline header
/// lowers them from the AST, so the two bodies cannot be textually identical. What must match is how iteration
/// proceeds -- counting rather than polling, one conditional exit, an item bound from the index by copy, and one
/// arithmetic advance per iteration.
#[derive(Debug, PartialEq)]
struct RangeIterationFacts {
    /// Whether any iteration in the body is an iterator poll rather than a counting step.
    polls_an_iterator: bool,
    /// How many `if <cond>: break` exits guard the loop.
    conditional_breaks: usize,
    /// Declared type of the local the loop pattern binds.
    item_binding_ty: String,
    /// Ownership fact the per-iteration item write reads the index with.
    item_read_fact: String,
    /// Operator the index is advanced with at the end of each iteration.
    advance_op: String,
}

/// Extract [`RangeIterationFacts`] from the named body's single loop, over the binding `item_name`.
fn range_iteration_facts(
    module: &bir::BodyIrModule,
    body_name: &str,
    item_name: &str,
) -> Result<RangeIterationFacts, Box<dyn std::error::Error>> {
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == body_name)
        .ok_or("expected the loop body")?;
    let item_local = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some(item_name))
        .ok_or("expected a local for the loop binding")?;
    let loop_stmts = body
        .block
        .stmts
        .iter()
        .find_map(|stmt| match &stmt.kind {
            bir::StatementKind::Loop { body } => Some(&body.stmts),
            _ => None,
        })
        .ok_or("expected a normalized loop")?;

    let polls_an_iterator = loop_stmts
        .iter()
        .any(|stmt| matches!(&stmt.kind, bir::StatementKind::IterNext { .. }));
    let conditional_breaks = loop_stmts
        .iter()
        .filter(|stmt| match &stmt.kind {
            bir::StatementKind::If { then_block, .. } => {
                matches!(then_block.stmts.as_slice(), [only] if matches!(&only.kind, bir::StatementKind::Break { .. }))
            }
            _ => false,
        })
        .count();
    let item_read_fact = loop_stmts
        .iter()
        .find_map(|stmt| match &stmt.kind {
            bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } if place.local_id() == Some(item_local.id) && place.projection.is_empty() => {
                Some(format!("{:?}", read.fact))
            }
            _ => None,
        })
        .ok_or("expected the per-iteration item write")?;
    let advance = loop_stmts
        .get(loop_stmts.len().wrapping_sub(2))
        .ok_or("expected an index advance before the loop ends")?;
    let bir::StatementKind::Assign {
        rvalue: bir::Rvalue::BinaryOp(advance_op, _, _),
        ..
    } = &advance.kind
    else {
        return Err("the statement before the index write must compute the advanced index".into());
    };

    Ok(RangeIterationFacts {
        polls_an_iterator,
        conditional_breaks,
        item_binding_ty: item_local.ty.to_string(),
        item_read_fact,
        advance_op: format!("{advance_op:?}"),
    })
}

#[test]
fn a_bound_range_iterates_with_the_same_facts_as_the_inline_range() -> Result<(), Box<dyn std::error::Error>> {
    let bound_source = concat!(
        "def total() -> int:\n",
        "  r = 0..10\n",
        "  mut acc = 0\n",
        "  for i in r:\n",
        "    acc = acc + i\n",
        "  return acc\n",
    );
    let inline_source = concat!(
        "def total() -> int:\n",
        "  mut acc = 0\n",
        "  for i in 0..10:\n",
        "    acc = acc + i\n",
        "  return acc\n",
    );
    let bound = build(bound_source, &["m", "bound_range_for"])?;
    let inline = build(inline_source, &["m", "inline_range_for"])?;

    assert!(
        !bound.render_snapshot().contains("unsupported("),
        "iterating a bound range must not fall back to a placeholder: {}",
        bound.render_snapshot()
    );
    let bound_facts = range_iteration_facts(&bound, "total", "i")?;
    assert!(
        !bound_facts.polls_an_iterator,
        "a bound range must keep the counting-loop shape rather than degrading to an iterator poll: {bound_facts:?}"
    );
    assert_eq!(
        bound_facts,
        range_iteration_facts(&inline, "total", "i")?,
        "a bound range must iterate with the same facts as the inline range it was bound from"
    );
    Ok(())
}

#[test]
fn a_bound_range_loop_drives_itself_from_the_range_value_fields() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def total() -> int:\n",
        "  mut r = 0..5\n",
        "  r = 1..=5\n",
        "  mut acc = 0\n",
        "  for i in r:\n",
        "    acc = acc + i\n",
        "  return acc\n",
    );
    let module = build(source, &["m", "bound_range_fields"])?;
    let snapshot = module.render_snapshot();
    let range = last_local_for_binding(&snapshot, "r").ok_or("expected a local for `r`")?;
    // The loop evaluates the range once, as legacy's native range does, so it iterates a snapshot taken before it.
    let snapshotted = snapshot
        .lines()
        .find_map(|line| {
            let (place, value) = line.trim().split_once(" = ")?;
            ["copy", "move", "clone"]
                .iter()
                .any(|fact| {
                    [format!("{fact}({range})"), format!("{fact}({range}, last_use)")].contains(&value.to_string())
                })
                .then(|| place.to_string())
        })
        .ok_or_else(|| format!("the loop must snapshot the range before iterating: {snapshot}"))?;

    for field in bir::AggregateKind::RANGE_FIELDS {
        assert!(
            snapshot.contains(&format!("copy({snapshotted}.{field})")),
            "the loop must read the range's own `{field}` field rather than re-deriving it: {snapshot}"
        );
    }
    assert!(
        !snapshot.contains("iter_next("),
        "a bound range must not be polled as a general iterable: {snapshot}"
    );
    for operator in [">", ">=", "not", " and ", " or "] {
        assert!(
            snapshot.contains(operator),
            "the loop must derive its stop condition from the bound value's dynamic inclusivity: {snapshot}"
        );
    }
    Ok(())
}

#[test]
fn a_range_returned_by_a_local_callable_refuses_before_field_projection() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def identity[T](value: T) -> T:\n",
        "  return value\n",
        "\n",
        "def total() -> int:\n",
        "  values = identity(0..4)\n",
        "  for value in values:\n",
        "    return value\n",
        "  return 0\n",
    );
    let module = build(source, &["m", "opaque_range_parameter"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("unsupported(range value without a source-local Body IR range aggregate)"),
        "a type spelling alone must not invent a Range aggregate layout: {snapshot}"
    );
    for field in bir::AggregateKind::RANGE_FIELDS {
        assert!(
            !snapshot.contains(&format!("values.{field}")),
            "a callable result with no local range aggregate must never acquire a synthetic `{field}` projection: {snapshot}"
        );
    }
    Ok(())
}

/// Deferred or zero-or-more expression scopes cannot make an enclosing call result into a source-local range
/// aggregate. Closures and comprehension elements are expression-only in the current grammar, so assignments cannot
/// occur in those bodies; these source fixtures instead pin the reachable boundary: capturing or yielding the opaque
/// `Range[int]` value does not authorize later range-field projections from the enclosing binding.
#[test]
fn nested_or_deferred_range_uses_do_not_authorize_an_outer_range_projection() -> Result<(), Box<dyn std::error::Error>>
{
    let prefix = concat!(
        "def identity[T](value: T) -> T:\n",
        "  return value\n",
        "\n",
        "def total() -> int:\n",
        "  mut r = identity(0..4)\n",
    );
    let suffix = concat!("  for value in r:\n", "    return value\n", "  return 0\n",);
    let nested_bodies = [
        "  unused = () => r\n",
        "  unused = (r for ignored in [])\n",
        "  unused = [r for ignored in []]\n",
    ];

    for (index, nested_body) in nested_bodies.iter().enumerate() {
        let source = format!("{prefix}{nested_body}{suffix}");
        let module = build(&source, &["m", "nested_range_provenance"])?;
        let snapshot = module.render_snapshot();
        assert!(
            snapshot.contains("unsupported(range value without a source-local Body IR range aggregate)"),
            "nested fixture {index} must leave the outer call result unproven: {snapshot}"
        );
        for field in bir::AggregateKind::RANGE_FIELDS {
            assert!(
                !snapshot.contains(&format!("r.{field}")),
                "nested fixture {index} must not project a range field from the outer call result: {snapshot}"
            );
        }
    }
    Ok(())
}

/// The indentation `render_block` gives a statement nested one block below a body's top-level statements.
///
/// A body renders its own block at depth 1, so a top-level statement carries two spaces and anything inside that
/// statement's nested block carries four. The shared stand-in is an undesugared vocab item injected after checking;
/// its refusal is a caller input-contract violation, independent of valid language constructs gaining representation.
const STAND_IN_REFUSAL_LABEL: &str = "Body IR input-contract violation";

/// The injected stand-in must still exercise the caller-contract refusal before nesting assertions rely on it.
#[test]
fn the_shared_stand_in_refusal_is_still_refused_by_design() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!("def f() -> int:\n{}  return 1\n", stand_in_refusal_stmt("  "));
    let module = build_with_stand_in_refusal(&source, &["m", "stand_in_guard"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        rendered.contains(STAND_IN_REFUSAL_LABEL),
        "the injected input-contract violation must stay refused: {rendered}"
    );
    Ok(())
}

const NESTED_BLOCK_INDENT: &str = "    ";

#[test]
fn a_statement_position_loop_lowers_to_the_same_loop_the_expression_spelling_produces()
-> Result<(), Box<dyn std::error::Error>> {
    // `bir::StatementKind::Loop` already existed and the expression spelling already emitted it; only
    // `lower_stmt_into`'s dispatch was missing, so the plain statement form -- the more common one -- refused.
    let source = concat!(
        "def count_to(limit: int) -> int:\n",
        "  mut i = 0\n",
        "  loop:\n",
        "    if i >= limit:\n",
        "      break\n",
        "    i = i + 1\n",
        "  return i\n",
    );
    let module = build(source, &["m", "loop_stmt"])?;
    let snapshot = body_named(&module, "count_to")?.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a statement-position `loop:` must lower rather than refuse: {snapshot}"
    );
    assert!(
        snapshot.lines().any(|line| line.trim() == "loop:"),
        "it must lower to Body IR's one normalized loop shape: {snapshot}"
    );
    // The statement spelling produces no value, so its `break` stays a plain valueless exit rather than acquiring
    // the result place a `loop:` expression's `break value` is rewritten into.
    assert!(
        snapshot.lines().any(|line| line.trim() == "break"),
        "the loop must be exited by a valueless break: {snapshot}"
    );
    Ok(())
}

/// A literal-true loop retains only its body-controlled exits, so a value-returning body needs no trailing return.
#[test]
fn a_while_true_loop_lowers_to_the_statement_loop_without_an_exit_test() -> Result<(), Box<dyn std::error::Error>> {
    // Legacy emits Rust's `loop` for a literal `true` condition, so only the body's own `return` leaves this loop and
    // the value-returning function needs no return after it.
    let source = concat!(
        "def first_even(start: int) -> int:\n",
        "  mut i = start\n",
        "  while true:\n",
        "    if i % 2 == 0:\n",
        "      return i\n",
        "    i = i + 1\n",
    );
    let module = build(source, &["m", "while_true"])?;
    let snapshot = body_named(&module, "first_even")?.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a `while true:` loop must lower rather than refuse: {snapshot}"
    );
    assert!(
        snapshot.lines().any(|line| line.trim() == "loop:"),
        "it must lower to Body IR's one normalized loop shape: {snapshot}"
    );
    assert!(
        !snapshot.lines().any(|line| line.trim() == "break"),
        "a literal `true` condition must not lower an exit test that breaks out of the loop: {snapshot}"
    );
    Ok(())
}

#[test]
fn continue_inside_a_statement_loop_behaves_as_it_does_in_while_and_for() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def odd_count(limit: int) -> int:\n",
        "  mut i = 0\n",
        "  mut odds = 0\n",
        "  loop:\n",
        "    if i >= limit:\n",
        "      break\n",
        "    i = i + 1\n",
        "    if i % 2 == 0:\n",
        "      continue\n",
        "    odds = odds + 1\n",
        "  return odds\n",
    );
    let module = build(source, &["m", "loop_stmt_continue"])?;
    let snapshot = body_named(&module, "odd_count")?.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a statement `loop:` carrying a `continue` must lower whole: {snapshot}"
    );
    assert!(
        snapshot.lines().any(|line| line.trim() == "continue"),
        "`continue` must lower to the shared continue statement: {snapshot}"
    );
    Ok(())
}

#[test]
fn nested_statement_loops_each_get_their_own_loop_block() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def grid(rows: int, cols: int) -> int:\n",
        "  mut cells = 0\n",
        "  mut r = 0\n",
        "  loop:\n",
        "    if r >= rows:\n",
        "      break\n",
        "    mut c = 0\n",
        "    loop:\n",
        "      if c >= cols:\n",
        "        break\n",
        "      c = c + 1\n",
        "      cells = cells + 1\n",
        "    r = r + 1\n",
        "  return cells\n",
    );
    let module = build(source, &["m", "nested_loop_stmt"])?;
    let snapshot = body_named(&module, "grid")?.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "nested statement loops must lower whole: {snapshot}"
    );
    let loop_indents: Vec<&str> = snapshot
        .lines()
        .filter(|line| line.trim() == "loop:")
        .map(|line| &line[..line.len() - line.trim_start().len()])
        .collect();
    // Asserting only that two loops exist would also pass if the inner one had been hoisted out of the outer
    // one's body, so require the second to be nested inside the first.
    assert_eq!(loop_indents.len(), 2, "both loops must be represented: {snapshot}");
    assert!(
        loop_indents[1].len() > loop_indents[0].len(),
        "the inner loop must stay nested inside the outer loop's body: {snapshot}"
    );
    Ok(())
}

#[test]
fn an_unsupported_statement_inside_a_statement_loop_keeps_its_own_refusal() -> Result<(), Box<dyn std::error::Error>> {
    // A caller-contract violation stays inside the loop; it must not collapse or escape its containing construct.
    let source = format!(
        concat!(
            "def scan(values: list[int]) -> int:\n",
            "  mut i = 0\n",
            "  loop:\n",
            "    if i >= 3:\n",
            "      break\n",
            "{}",
            "    i = i + 1\n",
            "  return i\n",
        ),
        stand_in_refusal_stmt("    ")
    );
    let module = build_with_stand_in_refusal(&source, &["m", "loop_stmt_partial"])?;
    let snapshot = body_named(&module, "scan")?.render_snapshot();

    assert!(
        snapshot.lines().any(|line| line.trim() == "loop:"),
        "the loop itself must still be represented: {snapshot}"
    );
    let refusal = snapshot
        .lines()
        .find(|line| line.contains(STAND_IN_REFUSAL_LABEL))
        .ok_or("missing the refusal for the unrepresentable loop-body statement")?;
    assert!(
        refusal.starts_with(NESTED_BLOCK_INDENT),
        "the refusal must stay inside the loop body rather than collapsing or escaping the loop: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_value_carrying_break_in_a_statement_loop_is_not_merged_into_an_enclosing_loop_expression()
-> Result<(), Box<dyn std::error::Error>> {
    // The typechecker owns this rule and already rejects the program (`break_value_requires_loop_expression`), so
    // lowering's job is only to not invent a second rule. A statement `loop:` pushes no break target, which is
    // what stops the value from being rewritten into the *enclosing* `loop:` expression's result place -- an
    // assignment the source never wrote.
    let source = concat!(
        "def find(limit: int) -> int:\n",
        "  return loop:\n",
        "    mut i = 0\n",
        "    loop:\n",
        "      if i >= limit:\n",
        "        break 1\n",
        "      i = i + 1\n",
        "    break i\n",
    );
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "loop_stmt_break_value"])?;
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("`break <value>` is only valid inside `loop:` expressions")),
        "the source checker must reject a value-carrying break in a statement loop: {diagnostics:?}"
    );

    let snapshot = body_named(&module, "find")?.render_snapshot();
    assert!(
        snapshot.lines().any(|line| line.trim() == "break const(1)"),
        "the rejected value must stay on the `break` statement rather than being assigned into the outer loop's \
         result place: {snapshot}"
    );
    Ok(())
}

/// The last statement of `block` that is not a scope drop.
fn last_non_drop(block: &bir::Block) -> Option<&bir::Statement> {
    block
        .stmts
        .iter()
        .rev()
        .find(|stmt| !matches!(stmt.kind, bir::StatementKind::Drop { .. }))
}

/// Whether `stmt` returns a value, directly or, for an `if`/`else`, from the tail of every branch.
fn returns_a_value(stmt: &bir::Statement) -> bool {
    match &stmt.kind {
        bir::StatementKind::Return { value: Some(_) } => true,
        bir::StatementKind::If {
            then_block,
            else_block: Some(else_block),
            ..
        } => {
            last_non_drop(then_block).is_some_and(returns_a_value)
                && last_non_drop(else_block).is_some_and(returns_a_value)
        }
        _ => false,
    }
}

#[test]
fn a_value_returning_function_returns_its_trailing_expression_issue2025() -> Result<(), Box<dyn std::error::Error>> {
    // The checker accepts each body because its trailing expression is the function's result. Body IR must say so
    // with a `return`, not record a discarded expression statement.
    let cases = [
        (
            "pick",
            "def pick(n: int) -> str:\n  match n:\n    1 => \"one\"\n    _ => \"other\"\n",
        ),
        ("next", "def next(n: int) -> int:\n  n + 1\n"),
        (
            "choose",
            "def choose(n: int) -> str:\n  if n == 1:\n    \"one\"\n  else:\n    \"other\"\n",
        ),
        // A branch with an owned local ends in its scope's drop, after the value.
        (
            "first",
            "def first(n: int) -> int:\n  if n == 1:\n    mut xs = [1]\n    xs[0]\n  else:\n    2\n",
        ),
    ];
    for (name, source) in cases {
        let module = build(source, &["m", "trailing"])?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing body")?;
        let tail = last_non_drop(&body.block).ok_or("empty body")?;
        assert!(
            returns_a_value(tail),
            "`{name}` must return its trailing value: {}",
            module.render_snapshot()
        );
    }
    Ok(())
}

#[test]
fn a_unit_function_keeps_its_trailing_expression_statement_issue2025() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def report(n: int) -> None:\n  println(n)\n";
    let module = build(source, &["m", "trailing_unit"])?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "report")
        .ok_or("missing body")?;
    let tail = last_non_drop(&body.block).ok_or("empty body")?;
    assert!(
        matches!(tail.kind, bir::StatementKind::Expr { .. }),
        "{}",
        module.render_snapshot()
    );
    Ok(())
}

/// Counting-loop continue edges advance the index, while an inner ordinary loop keeps its own header semantics.
#[test]
fn range_continue_retains_increment_without_leaking_into_nested_loops() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def f() -> int:\n",
        "    mut total = 0\n",
        "    for i in 0..3:\n",
        "        if i == 1:\n",
        "            continue\n",
        "        while false:\n",
        "            continue\n",
        "        total += i\n",
        "    return total\n",
    );
    let module = build(source, &["m", "range_continue"])?;
    let body = module.bodies.first().ok_or("missing function body")?;
    let counting = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .ok_or("missing counting loop")?;
    let continued = counting
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::If { then_block, .. }
                if then_block
                    .stmts
                    .iter()
                    .any(|statement| matches!(statement.kind, bir::StatementKind::Continue)) =>
            {
                Some(then_block)
            }
            _ => None,
        })
        .ok_or("missing continue branch")?;
    let before_continue = continued.stmts.as_slice();
    assert!(
        matches!(
            before_continue,
            [
                ..,
                bir::Statement {
                    kind: bir::StatementKind::Assign {
                        rvalue: bir::Rvalue::BinaryOp(bir::BinOp::Add, _, _),
                        ..
                    },
                    ..
                },
                bir::Statement {
                    kind: bir::StatementKind::Assign { .. },
                    ..
                },
                bir::Statement {
                    kind: bir::StatementKind::Continue,
                    ..
                }
            ]
        ),
        "continue branch: {before_continue:#?}"
    );
    let inner = counting
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .ok_or("missing inner loop")?;
    let continue_position = inner
        .stmts
        .iter()
        .position(|statement| matches!(statement.kind, bir::StatementKind::Continue))
        .ok_or("missing inner continue")?;
    assert!(!inner.stmts[..continue_position].iter().any(|statement| matches!(
        statement.kind,
        bir::StatementKind::Assign {
            rvalue: bir::Rvalue::BinaryOp(bir::BinOp::Add, _, _),
            ..
        }
    )));
    Ok(())
}

/// Builtin identity, retained argument timing, and signed exhaustion are facts of the checked producer.
#[test]
fn builtin_range_retains_bounds_validation_and_signed_exhaustion() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def total(end: int) -> int:\n  mut acc = 0\n  for i in range(5, end, -2):\n    if i == 3:\n      continue\n    acc += i\n  return acc\n",
        &["m", "builtin_range"],
    )?;
    let body = body_named(&module, "total")?;
    let validation = body
        .block
        .stmts
        .iter()
        .position(|statement| {
            matches!(
                statement.kind,
                bir::StatementKind::Call {
                    callee: bir::Callee::Helper(bir::HelperOp::RangeValidate),
                    may_panic: true,
                    ..
                }
            )
        })
        .ok_or("builtin range must retain constructor validation")?;
    let iteration = body
        .block
        .stmts
        .iter()
        .position(|statement| matches!(statement.kind, bir::StatementKind::Loop { .. }))
        .ok_or("builtin range must normalize to a counting loop")?;
    assert!(validation < iteration);
    let snapshot = module.render_snapshot();
    assert!(!snapshot.contains("iter_next("), "{snapshot}");
    assert!(
        snapshot.contains(" >= "),
        "positive steps must retain exclusive exhaustion: {snapshot}"
    );
    assert!(
        snapshot.contains(" <= "),
        "negative steps must retain exclusive exhaustion: {snapshot}"
    );
    assert!(snapshot.contains("range_validate"), "{snapshot}");
    Ok(())
}

/// A source-local function named range grants no builtin normalization authority.
#[test]
fn same_spelled_range_function_keeps_its_checked_iteration_target() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def range(end: int) -> list[int]:\n  return [end]\n\ndef main() -> None:\n  for item in range(3):\n    println(item)\n",
        &["m", "shadowed_range"],
    )?;
    let snapshot = module.render_snapshot();
    assert!(!snapshot.contains("range_validate"), "{snapshot}");
    assert!(snapshot.contains("iter_next("), "{snapshot}");
    Ok(())
}

/// An inclusive source range ends at the maximum integer on both advancement edges, as a header and as a bound value,
/// while a builtin range keeps its own runtime advancement.
#[test]
fn source_ranges_stop_at_the_maximum_integer_before_advancing() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def run(start: int, end: int, skip: bool) -> None:\n    values = start..=end\n    for item in values:\n        if skip:\n            continue\n        println(item)\n    for other in start..=end:\n        if skip:\n            continue\n        println(other)\n    for counted in range(start, end):\n        if skip:\n            continue\n        println(counted)\n",
        &["m", "range_maximum"],
    )?;
    let snapshot = body_named(&module, "run")?.render_snapshot();
    assert_eq!(
        snapshot.matches("9223372036854775807").count(),
        4,
        "each source range loop must stop at the maximum on its fallthrough and continue edges: {snapshot}"
    );
    assert_eq!(
        snapshot.matches("continue").count(),
        3,
        "every loop must exercise a continue edge: {snapshot}"
    );
    Ok(())
}

/// A range header evaluates its end once, before the loop, as legacy's native range does.
#[test]
fn range_header_end_is_evaluated_once_before_the_loop() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def end() -> int:\n    return 3\n\ndef run() -> None:\n    for i in 0..end():\n        println(i)\n",
        &["m", "range_header_end"],
    )?;
    let snapshot = body_named(&module, "run")?.render_snapshot();
    let (before, inside) = snapshot.split_once("loop:").ok_or("missing counting loop")?;
    let calls_end = |line: &str| line.contains("call ") && line.contains("end");
    assert!(
        before.lines().any(calls_end),
        "the end bound must be evaluated before the loop: {snapshot}"
    );
    assert!(
        !inside.lines().any(calls_end),
        "the loop must not re-evaluate its end bound: {snapshot}"
    );
    Ok(())
}

// ---- #1561: a loop whose body changes its item in place iterates its list exclusively ----

/// The iterator of `body_name`'s only builtin loop, read off the statement that materializes it from the iterable.
fn iterable_read(module: &bir::BodyIrModule, body_name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let body = body_named(module, body_name)?;
    let loop_index = body
        .block
        .stmts
        .iter()
        .position(|statement| matches!(statement.kind, bir::StatementKind::Loop { .. }))
        .ok_or("the body must contain a builtin loop")?;
    let initializer = loop_index
        .checked_sub(1)
        .and_then(|index| body.block.stmts.get(index))
        .ok_or("the loop must follow its iterator's initializer")?;
    match &initializer.kind {
        bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            ..
        } => Ok(format!("{:?}", read.fact)),
        other => Err(format!("unexpected iterator initializer {other:?}").into()),
    }
}

/// In-place item mutations require an exclusive iterable borrow; a loop whose item is only passed to a reader keeps a
/// shared borrow.
#[test]
fn a_loop_that_changes_its_item_in_place_borrows_its_list_exclusively() -> Result<(), Box<dyn std::error::Error>> {
    // Legacy iterates with `iter_mut`, so `row.append(3)` changes the caller's list; `show(row)` only passes a copy.
    let source = concat!(
        "model Cell:\n",
        "  pub tags: list[int]\n",
        "\n",
        "  def tag(mut self, value: int) -> None:\n",
        "    self.tags.append(value)\n",
        "\n",
        "def show(mut row: list[int]) -> int:\n",
        "  return len(row)\n",
        "\n",
        "def grow(mut items: list[list[int]]) -> None:\n",
        "  for row in items:\n",
        "    row.append(3)\n",
        "\n",
        "def tag_all(mut cells: list[Cell]) -> None:\n",
        "  for cell in cells:\n",
        "    cell.tag(9)\n",
        "\n",
        "def measure(mut items: list[list[int]]) -> int:\n",
        "  mut total = 0\n",
        "  for row in items:\n",
        "    total += show(row)\n",
        "  return total\n",
    );
    let module = build(source, &["m", "in_place_iteration"])?;
    assert_eq!(iterable_read(&module, "grow")?, "MutBorrow");
    assert_eq!(iterable_read(&module, "tag_all")?, "MutBorrow");
    assert_eq!(iterable_read(&module, "measure")?, "Borrow");
    Ok(())
}

/// Direct enumeration polls the original list and binds its changed component exclusively, without an owned
/// enumeration materialization that would disconnect the write from the source.
#[test]
fn enumeration_retains_the_original_list_and_changed_pattern_binding() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def grow(mut rows: list[list[int]]) -> None:\n  for i, row in enumerate(rows):\n    row.append(i)\n",
        &["m", "enumeration_source"],
    )?;
    let body = body_named(&module, "grow")?;
    assert_eq!(iterable_read(&module, "grow")?, "MutBorrow");
    assert!(!body.block.stmts.iter().any(|statement| matches!(&statement.kind,
        bir::StatementKind::Call { callee: bir::Callee::Function(bir::CallableTarget::Named(target)), .. }
            if target.builtin == Some(bir::BuiltinFnId::Enumerate))));
    let loop_body = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .ok_or("missing enumeration loop")?;
    let polled = loop_body
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::IterNext { destination, .. } => destination.local_id(),
            _ => None,
        })
        .ok_or("missing source item")?;
    let row = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("row"))
        .ok_or("missing row binding")?
        .id;
    assert!(loop_body.stmts.iter().any(|statement| matches!(&statement.kind,
        bir::StatementKind::Assign { place, rvalue: bir::Rvalue::Use(bir::Operand::Place(read)) }
            if place.local_id() == Some(row) && read.place.local_id() == Some(polled) && read.fact == bir::OwnershipFact::MutBorrow)));
    Ok(())
}

/// Changing an ordinary value alias in the body does not turn the loop's pattern into an exclusive source binding.
#[test]
fn enumeration_keeps_ordinary_body_aliases_as_values() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def copied(mut rows: list[list[int]]) -> None:\n  for i, row in enumerate(rows):\n    mut alias = row\n    alias.append(i)\n",
        &["m", "enumeration_alias"],
    )?;
    let body = body_named(&module, "copied")?;
    assert_eq!(iterable_read(&module, "copied")?, "Borrow");
    let alias = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("alias"))
        .ok_or("missing alias")?
        .id;
    let snapshot = module.render_snapshot();
    assert!(!snapshot.contains("unsupported("), "{snapshot}");
    let loop_body = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .ok_or("missing enumeration loop")?;
    assert!(loop_body.stmts.iter().any(|statement| matches!(&statement.kind,
        bir::StatementKind::Assign { place, rvalue: bir::Rvalue::Use(bir::Operand::Place(read)) }
            if place.local_id() == Some(alias) && read.fact != bir::OwnershipFact::MutBorrow)));
    Ok(())
}

/// A same-named user function keeps its checked call and ordinary tuple iteration; builtin source normalization
/// requires canonical identity rather than a name match.
#[test]
fn enumeration_named_user_function_keeps_its_checked_call() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def enumerate(values: list[int]) -> list[tuple[int, int]]:\n  return [(7, values[0])]\ndef shadowed(values: list[int]) -> int:\n  mut total = 0\n  for i, value in enumerate(values):\n    total += i + value\n  return total\n";
    let module = build(source, &["m", "enumeration_shadow"])?;
    let body = body_named(&module, "shadowed")?;
    let calls: Vec<_> = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } => Some(target.builtin),
            _ => None,
        })
        .collect();
    assert_eq!(calls, vec![None]);
    Ok(())
}

/// Direct zip retains separate original source polls and exclusive ownership only for the changed operand.
#[test]
fn zip_retains_original_sources_and_operand_specific_ownership() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def grow(mut rows: list[list[int]], extra: list[int]) -> None:\n  for row, n in zip(rows, extra):\n    row.append(n)\n",
        &["m", "zip_sources"],
    )?;
    let body = body_named(&module, "grow")?;
    let rows = body.params[0].local;
    let extra = body.params[1].local;
    let reads: Vec<_> = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
                ..
            } => Some((read.place.local_id(), read.fact)),
            _ => None,
        })
        .collect();
    assert!(reads.contains(&(Some(rows), bir::OwnershipFact::MutBorrow)));
    assert!(reads.contains(&(Some(extra), bir::OwnershipFact::Borrow)));
    let loop_body = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .ok_or("missing zip loop")?;
    assert_eq!(
        loop_body
            .stmts
            .iter()
            .filter(|statement| matches!(statement.kind, bir::StatementKind::IterNext { .. }))
            .count(),
        2
    );
    Ok(())
}

/// Direct dictionary values poll original keys and retain the changed value as an exclusive indexed place.
#[test]
fn dictionary_values_retain_the_original_indexed_value() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def grow(mut table: dict[str, list[int]]) -> None:\n  for value in table.values():\n    value.append(3)\n",
        &["m", "dictionary_values"],
    )?;
    let body = body_named(&module, "grow")?;
    assert!(!body.block.stmts.iter().any(|statement| matches!(&statement.kind,
        bir::StatementKind::Call { callee: bir::Callee::Method(target), .. }
            if target.canonical.as_ref().is_some_and(|identity| identity.declaration_name == "Dict.values"))));
    let loop_body = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .ok_or("missing value loop")?;
    assert!(loop_body.stmts.iter().any(|statement| matches!(&statement.kind,
        bir::StatementKind::Assign { rvalue: bir::Rvalue::Use(bir::Operand::Place(read)), .. }
            if read.fact == bir::OwnershipFact::MutBorrow && matches!(read.place.projection.first(), Some(bir::PlaceElem::Index(_))))));
    Ok(())
}
