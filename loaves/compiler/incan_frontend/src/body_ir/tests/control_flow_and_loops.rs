//! Control flow: `if` / `while` / `for` normalization, division and assert panic facts, `for` over builtin lists,
//! user-defined and fallible iteration protocols, loop-scoped bindings, expression-position `if`, value-carrying `loop`
//! expressions, `try` propagation, range values and the facts a bound range shares with an inline one (#1165), and
//! statement-position `loop:` bodies (#1162).

use super::*;

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
        snapshot.contains("user_defined(__next__)"),
        "should poll via the resolved __next__ method, non-fallible: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_fallible_for_iteration_with_an_implicit_try_propagate_semantic() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model ChunkStream:\n  def __iter__(self) -> ChunkStream:\n    return self\n\n  def __next__(self) -> Result[Option[int], str]:\n    return Ok(None)\n\ndef total() -> Result[int, str]:\n  mut acc = 0\n  for chunk in ChunkStream()?:\n    acc = acc + chunk\n  return Ok(acc)\n";
    let module = build(source, &["m", "fallible_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("user_defined(__next__, fallible)"),
        "fallible protocol iteration should mark IterNext as fallible: {snapshot}"
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

    for field in bir::AggregateKind::RANGE_FIELDS {
        assert!(
            snapshot.contains(&format!("copy({range}.{field})")),
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
/// statement's nested block carries four. Tests that care about *where* a statement landed compare against this rather
/// than merely finding the text somewhere in the body, which would also pass if the statement had escaped into the
/// enclosing block. The label fragment every by-design refusal used as a test stand-in renders with.
///
/// Tests that need *some* refusal in a given position -- to prove a refusal stays nested inside the construct
/// containing it, say -- must not reach for whichever construct happens to be unrepresentable that week. Four have
/// now gone vacuous exactly that way: `value ** 2` (#1160), a byte-string literal (#1165), a callable default
/// (#1240), and collection membership (#1246, twice over). Each was a *gap*, and #1101 exists to close gaps, so
/// every such choice decays the moment its owning sibling lands.
///
/// `unsafe:` is categorically different. It is refused because Body IR v0 cannot carry the acknowledgment a
/// consumer would need to admit the region deliberately -- a stated disposition, not pending work. Reversing it
/// means designing the acknowledgment fact first, so it will not quietly become representable underneath a test.
///
/// If [`the_shared_stand_in_refusal_is_still_refused_by_design`] ever fails, every test below that uses this has
/// gone vacuous: pick another *by-design* refusal, update these two items, and do not substitute a gap.
const STAND_IN_REFUSAL_LABEL: &str = "`unsafe:` acknowledgment region";

#[test]
fn the_shared_stand_in_refusal_is_still_refused_by_design() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!("def f() -> int:\n{}  return 1\n", stand_in_refusal_stmt("  "));
    let rendered = rendered_f(&source, "stand_in_guard")?;

    assert!(
        rendered.contains(STAND_IN_REFUSAL_LABEL),
        "the shared stand-in refusal is no longer refused, so every test using it now asserts nothing. Pick \
         another refusal that is refused *by design* rather than merely unimplemented, and update \
         STAND_IN_REFUSAL_LABEL and stand_in_refusal_stmt together: {rendered}"
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
    // The loop must not swallow a construct it happens to contain: a consumer loses only that statement, not the
    // whole loop. The original stand-in was collection membership, chosen because #1160 deliberately left it
    // refused, and with the explicit note that "a stand-in that later becomes representable turns its test
    // vacuous" -- which is exactly what #1246 then did to it. Both this test and
    // `an_unsupported_construct_in_a_race_arm_does_not_collapse_the_whole_race` now share one by-design refusal
    // instead, so the next representation cannot quietly hollow them out. See STAND_IN_REFUSAL_LABEL.
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
    let module = build(&source, &["m", "loop_stmt_partial"])?;
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
