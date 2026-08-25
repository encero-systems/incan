//! Body IR lowering tests.
//!
//! These are integration-shaped rather than unit-shaped: each builds a whole module through
//! lex -> parse -> typecheck -> lower and asserts on the rendered snapshot, so there is no submodule any given
//! test naturally belongs to. They stay in one sibling file, following `src/frontend/typechecker/tests.rs`.

use super::*;
use crate::frontend::typechecker::TypeChecker;
use crate::frontend::{lexer, parser};

fn build(source: &str, module_path: &[&str]) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Lower an intentionally-invalid source program after recording its typecheck diagnostics.
///
/// Positive coverage must go through [`build`], which requires ordinary typechecking. This helper is only for
/// Body IR's fail-closed assertions: after the source checker correctly rejects a program, lowering must still
/// make its unsupported representation explicit rather than approximating it.
fn build_after_expected_typecheck_errors(
    source: &str,
    module_path: &[&str],
) -> Result<(bir::BodyIrModule, Vec<String>), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    let diagnostics = checker
        .check_program(&program)
        .err()
        .ok_or("expected the intentionally invalid source program to produce a diagnostic")?
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();
    Ok((
        build_body_ir_module_v0(&program, &module_path, checker.type_info()),
        diagnostics,
    ))
}

/// Build a Body IR module from `source` after rewriting its first `for a, b in ...:` header into the nested
/// `for a, (b, c) in ...:` shape the parser has no spelling for (see
/// `nested_tuple_for_patterns_have_no_source_spelling_yet`). The rewrite happens *before* typechecking, so the
/// nested pattern flows through `TypeChecker::define_for_pattern_bindings`' own recursion and reaches lowering
/// with real resolved element types, exactly as a future parser-supported nesting would.
fn build_with_nested_for_pattern(
    source: &str,
    module_path: &[&str],
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let for_stmt = program
        .declarations
        .iter_mut()
        .find_map(|decl| match &mut decl.node {
            ast::Declaration::Function(function) => function.body.iter_mut().find_map(|stmt| match &mut stmt.node {
                ast::Statement::For(for_stmt) => Some(for_stmt),
                _ => None,
            }),
            _ => None,
        })
        .ok_or("expected a top-level function containing a `for` statement")?;
    let ast::Pattern::Tuple(items) = &mut for_stmt.pattern.node else {
        return Err("expected a flat tuple loop pattern to nest".into());
    };
    let second = items.pop().ok_or("expected a two-item tuple loop pattern")?;
    let span = second.span;
    let third = ast::Spanned::new(ast::Pattern::Binding("c".to_string()), span);
    items.push(ast::Spanned::new(ast::Pattern::Tuple(vec![second, third]), span));

    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Build a Body IR module from `source` after rewriting its first `for x in ...:` header into a two-name tuple
/// pattern **after** typechecking, leaving the recorded item type as the original non-tuple element type.
///
/// This reaches lowering's defence-in-depth path directly: the typechecker rejects such a program
/// (`for_pattern_expects_tuple_item`), so no ordinary `build` could ever produce this state, yet lowering must
/// still refuse rather than project `.0`/`.1` out of a value with no such fields.
fn build_with_for_pattern_widened_after_typecheck(
    source: &str,
    module_path: &[&str],
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let for_stmt = program
        .declarations
        .iter_mut()
        .find_map(|decl| match &mut decl.node {
            ast::Declaration::Function(function) => function.body.iter_mut().find_map(|stmt| match &mut stmt.node {
                ast::Statement::For(for_stmt) => Some(for_stmt),
                _ => None,
            }),
            _ => None,
        })
        .ok_or("expected a top-level function containing a `for` statement")?;
    let span = for_stmt.pattern.span;
    let first = std::mem::replace(&mut for_stmt.pattern.node, ast::Pattern::Wildcard);
    for_stmt.pattern.node = ast::Pattern::Tuple(vec![
        ast::Spanned::new(first, span),
        ast::Spanned::new(ast::Pattern::Binding("second".to_string()), span),
    ]);

    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

#[test]
fn lowers_arithmetic_with_a_copy_last_use_and_a_move_return() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(x: int, y: int) -> int:\n  return x + y\n";
    let module = build(source, &["m", "arith"])?;
    let snapshot_first = module.render_snapshot();
    let snapshot_second = build(source, &["m", "arith"])?.render_snapshot();
    assert_eq!(snapshot_first, snapshot_second, "lowering must be deterministic");

    assert!(snapshot_first.contains("body add decl:m::arith::add"));
    assert!(snapshot_first.contains("local 0 x : int [param]"));
    assert!(snapshot_first.contains("local 1 y : int [param]"));
    // x is not the last read (y is), so x is Copy either way (int is a Copy type); both reads should be `copy`.
    assert!(snapshot_first.contains("copy(_0)"));
    assert!(snapshot_first.contains("copy(_1"));
    // `int` is a Copy-shaped type, so even a freshly created temporary reads as `copy`, not `move`.
    assert!(snapshot_first.contains("return copy(_2, last_use)"));
    Ok(())
}

#[test]
fn lowers_string_concat_as_an_explicit_helper_call_with_runtime_requirements() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "def greet(name: str) -> str:\n  return \"hi \" + name\n";
    let module = build(source, &["m", "strs"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("call helper:str_concat"));
    assert!(snapshot.contains("runtime_requirements:"));
    assert!(snapshot.contains("runtime_helper(str_concat)"));
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn lowers_a_non_copy_binding_and_drops_it_when_never_moved() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make() -> None:\n  s = \"hello\"\n  return\n";
    let module = build(source, &["m", "drop"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("local 0 s : str [binding]"));
    assert!(snapshot.contains("drop _0"));
    Ok(())
}

#[test]
fn lowers_a_non_copy_binding_and_skips_the_drop_when_moved_via_return() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make() -> str:\n  s = \"hello\"\n  return s\n";
    let module = build(source, &["m", "moved"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("return move(_0, last_use)"));
    assert!(
        !snapshot.contains("drop _0"),
        "a moved-out local must not also be dropped: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_clone_when_a_non_copy_binding_is_read_more_than_once() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def dup(s: str) -> str:\n  first = s\n  return s\n";
    let module = build(source, &["m", "clone"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("clone(_0)"),
        "the first, non-last read of `s` should clone: {snapshot}"
    );
    assert!(snapshot.contains("return move(_0, last_use)"));
    Ok(())
}

#[test]
fn lowers_if_while_and_for_into_normalized_control_flow() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def run(n: int) -> int:\n  total = 0\n  for i in 0..n:\n    if i > 2:\n      total = total + i\n  while total > 100:\n    total = total - 1\n  return total\n";
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
fn unsupported_constructs_lower_to_an_explicit_placeholder_instead_of_panicking()
-> Result<(), Box<dyn std::error::Error>> {
    // #1123 supports lazy generator expressions with simple binding clauses. A destructuring clause still needs
    // a generator-specific binding/poll representation, so it must refuse the complete expression rather than
    // partly lowering it as an eager list or silently dropping the pattern.
    let source = "def pick(x: int) -> int:\n  gen = (left + right for left, right in [(1, 2)])\n  return x\n";
    let module = build(source, &["m", "unsupported"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("unsupported(generator for-clause pattern is not a simple binding)"),
        "should record an explicit placeholder rather than panicking: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_an_immutable_receiver_read_through_a_field_projection() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Counter:\n  value: int\n\n  def get(self) -> int:\n    return self.value\n";
    let module = build(source, &["m", "receiver_read"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body get decl:m::receiver_read::Counter::get"));
    assert!(snapshot.contains("local 0 self : Counter [receiver]"));
    // `self.value` is a projected read of an `int` (Copy) field, so it reads `copy`, never `move` or `clone`.
    assert!(snapshot.contains("return copy(_0.value)"));

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
fn mut_self_receiver_origin_is_mutable_and_field_mutation_lowers() -> Result<(), Box<dyn std::error::Error>> {
    // `mut self` must remain a mutable receiver when its field assignment is lowered.
    let source = "model Counter:\n  value: int\n\n  def bump(mut self) -> None:\n    self.value = self.value + 1\n";
    let module = build(source, &["m", "receiver_mut"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body bump decl:m::receiver_mut::Counter::bump"));
    assert!(snapshot.contains("local 0 self : Counter [receiver_mut]"));
    assert!(
        !snapshot.contains("unsupported("),
        "mutable receiver field assignment should lower without a placeholder: {snapshot}"
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
fn lowers_a_list_comprehension_into_a_push_loop() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def doubled(items: list[int]) -> list[int]:\n  return [x * 2 for x in items]\n";
    let module = build(source, &["m", "list_comp"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("list[]"),
        "should start from an empty list aggregate: {snapshot}"
    );
    assert!(
        snapshot.contains("call method:push unbound(mut_borrow("),
        "should grow the list via a synthesized push call: {snapshot}"
    );
    assert!(
        snapshot.contains("iter_next("),
        "should desugar into the shared iteration primitive: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_filtered_list_comprehension_with_a_guarding_if() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def evens(items: list[int]) -> list[int]:\n  return [x for x in items if x % 2 == 0]\n";
    let module = build(source, &["m", "list_comp_filter"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("call method:push unbound("),
        "filtered comprehension should still push accepted elements: {snapshot}"
    );
    assert!(
        snapshot.contains("if "),
        "the filter clause should lower to a guarding If: {snapshot}"
    );
    Ok(())
}

#[test]
fn comprehension_bindings_do_not_escape_the_expression_scope() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep_outer(x: int, items: list[int]) -> int:\n  doubled = [x * 2 for x in items]\n  return x\n";
    let module = build(source, &["m", "comprehension_scope"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("return copy(_0)"),
        "the trailing read must resolve the enclosing parameter, not the comprehension binding: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_dict_comprehension_into_an_insert_loop() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def doubled(items: list[int]) -> dict[int, int]:\n  return {x: x * 2 for x in items}\n";
    let module = build(source, &["m", "dict_comp"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("dict[]"),
        "should start from an empty dict aggregate: {snapshot}"
    );
    assert!(
        snapshot.contains("call method:insert unbound(mut_borrow("),
        "should grow the dict via a synthesized insert call: {snapshot}"
    );
    Ok(())
}

#[test]
fn generator_expression_keeps_its_multi_clause_body_lazy_and_captures_its_environment()
-> Result<(), Box<dyn std::error::Error>> {
    // Mirrors the multi-clause fixture from `test_rfc006_generator_expression_infers_element_type` in
    // `src/frontend/typechecker/tests.rs`, but also reads `offset` from both the filter and element. The Body IR
    // value must capture that enclosing local once at construction; it must not materialize the chain or run
    // either filter/element in the enclosing body.
    let source = "def positives(offset: int, xs: list[int], ys: list[int]) -> Generator[int]:\n  return (x * offset for x in xs if x > offset for y in ys if y > x)\n";
    let module = build(source, &["m", "generator_expr"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("generator(source="),
        "generator construction must be represented as a distinct lazy rvalue: {snapshot}"
    );
    assert!(
        snapshot.contains("captures=["),
        "the deferred body must receive explicit construction-time captures: {snapshot}"
    );
    assert!(
        !snapshot.contains("list[]"),
        "a generator expression must not materialize an eager list while claiming Generator[T]: {snapshot}"
    );
    assert!(
        snapshot.contains("yield "),
        "the element must be suspended in the generator body: {snapshot}"
    );
    assert!(
        snapshot.contains("iter_next("),
        "for clauses must remain deferred iteration operations: {snapshot}"
    );
    assert!(
        snapshot.contains("if "),
        "filters must remain deferred guard operations: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "a valid generator expression must not leave an unsupported placeholder: {snapshot}"
    );
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "positives")
        .ok_or("generator fixture must lower its function body")?;
    assert!(
        body.block.stmts.iter().all(|statement| !matches!(
            statement.kind,
            bir::StatementKind::IterNext { .. } | bir::StatementKind::Yield { .. }
        )),
        "polling and yield must stay inside the generator rvalue, not the enclosing body: {snapshot}"
    );
    let (source, captured_operands, generator_body) = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue:
                    bir::Rvalue::Generator {
                        source,
                        captured_operands,
                        body,
                    },
                ..
            } => Some((source, captured_operands, body)),
            _ => None,
        })
        .ok_or("generator fixture must assign a Generator rvalue")?;
    assert!(
        matches!(source, bir::Operand::Place(_)),
        "the first for source must be captured as a construction-time operand: {source:?}"
    );
    assert_eq!(
        captured_operands.len(),
        2,
        "offset and ys are the deferred free captures"
    );
    let capture_names: Vec<_> = generator_body
        .capture_locals
        .iter()
        .map(|local| body.locals[local.index()].name.as_deref())
        .collect();
    assert_eq!(capture_names, vec![Some("offset"), Some("ys")]);
    assert!(
        matches!(
            body.locals[generator_body.source_local.index()].origin,
            bir::LocalOrigin::Captured
        ),
        "the construction-time source needs a generator-owned local"
    );
    assert!(
        generator_body
            .capture_locals
            .iter()
            .all(|local| matches!(body.locals[local.index()].origin, bir::LocalOrigin::Captured)),
        "each deferred free value must bind through an explicit captured local"
    );
    Ok(())
}

#[test]
fn generator_expression_evaluates_only_its_outer_source_before_construction() -> Result<(), Box<dyn std::error::Error>>
{
    let source = concat!(
        "def source() -> list[int]:\n",
        "  return [1, 2]\n\n",
        "def lazy() -> Generator[int]:\n",
        "  return (item for item in source())\n"
    );
    let module = build(source, &["m", "generator_source_timing"])?;
    let snapshot = module.render_snapshot();
    let source_call = snapshot
        .find("call fn:source()")
        .ok_or("outer generator source call must lower at construction")?;
    let generator = snapshot
        .find("generator(source=")
        .ok_or("generator construction must have a distinct rvalue")?;
    assert!(
        source_call < generator,
        "the first for source must be evaluated before generator construction: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "a supported outer source must not leave an unsupported marker: {snapshot}"
    );
    Ok(())
}

#[test]
fn generator_expression_captures_an_outer_value_without_leaking_its_clause_binding()
-> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def preserve(prefix: str, values: list[str]) -> str:\n",
        "  generated = (prefix + value for value in values)\n",
        "  return prefix\n"
    );
    let module = build(source, &["m", "generator_capture_scope"])?;
    let snapshot = module.render_snapshot();
    assert!(
        snapshot.contains("captures=[clone(_0)"),
        "the generator must own a construction-time clone while the enclosing binding remains live: {snapshot}"
    );
    assert!(
        snapshot.contains("return move(_0, last_use)"),
        "the trailing source read must resolve the outer prefix, not a generator-local capture: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "captured generator values must lower without an unsupported placeholder: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_dict_literal_as_a_dict_aggregate_with_paired_operands() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make() -> dict[str, int]:\n  return {\"a\": 1, \"b\": 2}\n";
    let module = build(source, &["m", "dict_lit"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("dict[const(\"a\"): const(1), const(\"b\"): const(2)]"),
        "dict aggregate should render key/value pairs: {snapshot}"
    );
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn lowers_a_set_literal_as_a_set_aggregate() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make() -> set[str]:\n  return {\"a\", \"b\"}\n";
    let module = build(source, &["m", "set_lit"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("set[const(\"a\"), const(\"b\")]"),
        "set aggregate should render as a flat element list: {snapshot}"
    );
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn lowers_a_slice_expression_as_a_slice_projected_place_read() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def middle(s: str) -> str:\n  return s[1:3]\n";
    let module = build(source, &["m", "slice"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("[const(1):const(3)]"),
        "slice projection should render start/end operands: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_tuple_unpack_into_field_projected_reads_off_a_materialized_tuple() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def sum_pair() -> int:\n  pair = (1, 2)\n  a, b = pair\n  return a + b\n";
    let module = build(source, &["m", "tuple_unpack"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains(".0") && snapshot.contains(".1"),
        "tuple unpack should project each element by index: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "tuple unpack should not fall back: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_method_call_on_self_with_a_borrowed_receiver_argument() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Counter:\n  value: int\n\n  def get(self) -> int:\n    return self.value\n\n  def get_twice(self) -> int:\n    return self.get() + self.get()\n";
    let module = build(source, &["m", "method_call"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body get_twice decl:m::method_call::Counter::get_twice"));
    // Method-call receivers borrow, mirroring how any other method call's receiver already lowers.
    assert!(snapshot.contains("call method:get(borrow(_0))"));
    Ok(())
}

#[test]
fn abstract_trait_method_produces_no_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Greeter:\n  def greet(self) -> str: ...\n";
    let module = build(source, &["m", "abstract_method"])?;

    assert!(
        module.bodies.is_empty(),
        "an abstract method has no body to lower, and must not produce an Unsupported placeholder body either: {:?}",
        module.bodies
    );

    Ok(())
}

#[test]
fn lowers_tuple_assign_swap_with_correct_evaluation_order() -> Result<(), Box<dyn std::error::Error>> {
    // `arr[i], arr[j] = (arr[j], arr[i])` must read both original values before writing either target, or the
    // swap would clobber `arr[i]` before `arr[j]`'s read observes it. A leading plain-identifier target (`a, b
    // = ...`) always parses as `TupleUnpackStmt` instead (new bindings, possibly shadowing) -- lvalue index/
    // field targets are what actually reaches `TupleAssignStmt`, matching the parser's own routing
    // (`crates/incan_syntax/src/parser/stmts.rs`'s `assignment_or_expr_stmt`).
    let source =
        "def swap(mut arr: list[int], i: int, j: int) -> int:\n  arr[i], arr[j] = (arr[j], arr[i])\n  return arr[i]\n";
    let module = build(source, &["m", "tuple_assign"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "tuple assign should not fall back: {snapshot}"
    );
    // Both targets should end up written via a plain `Assign` into an `[index]`-projected place, not
    // `Unsupported`.
    assert!(
        snapshot.matches("] = ").count() >= 2,
        "both index-projected targets should be assigned: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_default_trait_method_with_a_self_typed_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Identity:\n  def identity(self) -> Self:\n    return self\n";
    let module = build(source, &["m", "trait_default"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body identity decl:m::trait_default::Identity::identity"));
    assert!(snapshot.contains("local 0 self : Self [receiver]"));
    assert!(snapshot.contains("return clone(_0)"));

    Ok(())
}

#[test]
fn lowers_chained_assignment_right_to_left() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def chain() -> int:\n  x = y = z = 5\n  return x + y + z\n";
    let module = build(source, &["m", "chained"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "chained assignment should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains("const(5)"),
        "the rightmost target reads the literal value: {snapshot}"
    );
    Ok(())
}

#[test]
fn static_method_lowers_like_a_free_function_with_no_receiver_local() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Counter:\n  value: int\n\n  def zero() -> Counter:\n    return Counter(value=0)\n";
    let module = build(source, &["m", "static_method"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body zero decl:m::static_method::Counter::zero"));
    assert!(
        !snapshot.contains("[receiver"),
        "a static/associated method (receiver: None) must not declare a receiver local: {snapshot}"
    );

    Ok(())
}

#[test]
fn method_parameter_type_is_recorded_from_the_checked_callable_signature() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "model Counter:\n  value: int\n\n  def add(self, amount: int) -> int:\n    return self.value + amount\n";
    let module = build(source, &["m", "method_param"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body add decl:m::method_param::Counter::add"));
    assert!(
        snapshot.contains("local 1 amount : int [param]"),
        "an ordinary method parameter must declare with its checked resolved type, not Unknown: {snapshot}"
    );

    Ok(())
}

#[test]
fn top_level_defaults_lower_to_deferred_source_computations() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fallback() -> int:\n  return 2\n\ndef choose(limit: u8 = 7, value: int = fallback()) -> int:\n  return value\n";
    let module = build(source, &["m", "top_level_default"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let limit = choose.params.first().ok_or("expected choose's limit parameter")?;
    let value = choose.params.get(1).ok_or("expected choose's value parameter")?;

    assert_eq!(limit.local, bir::LocalId(0));
    assert_eq!(limit.name, "limit");
    assert_eq!(
        limit.ty,
        IncanType::Primitive(IncanPrimitiveType::Numeric("u8".to_string()))
    );
    let bir::CallableParamDefault::Source(limit_default) = &limit.default else {
        return Err("a checked literal default must become a deferred Body-IR computation".into());
    };
    let limit_start = source.find("7,").ok_or("missing literal default source spelling")?;
    assert_eq!(limit_default.span, HirSourceSpan::new(limit_start, limit_start + 1));
    assert!(limit_default.stmts.is_empty());
    assert_eq!(limit_default.result, bir::Operand::Constant(bir::Constant::Int(7)));

    assert_eq!(value.local, bir::LocalId(1));
    assert_eq!(value.name, "value");
    let bir::CallableParamDefault::Source(value_default) = &value.default else {
        return Err("a checked function default call must become a deferred Body-IR computation".into());
    };
    let call_start = source.rfind("fallback()").ok_or("missing default source spelling")?;
    assert_eq!(
        value_default.span,
        HirSourceSpan::new(call_start, call_start + "fallback()".len()),
        "the direct consumer must receive the default expression's exact source span"
    );
    let [call] = value_default.stmts.as_slice() else {
        return Err("the deferred function default should contain one call statement".into());
    };
    let bir::StatementKind::Call {
        destination: Some(destination),
        callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
        args,
        may_panic,
    } = &call.kind
    else {
        return Err("the deferred default must retain a direct named call".into());
    };
    assert_eq!(target.name, "fallback");
    assert!(target.type_args.is_empty());
    assert!(args.is_empty());
    assert!(!may_panic);
    let bir::Operand::Place(result) = &value_default.result else {
        return Err("the deferred default call must return its computed temporary".into());
    };
    assert_eq!(&result.place, destination);
    assert!(
        !choose.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        )),
        "the default call must not be appended to the ordinary function body: {choose:?}"
    );
    assert!(
        !choose
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "a refused source default must not retain an implicit frontend lookup: {choose:?}"
    );

    Ok(())
}

#[test]
fn generic_method_defaults_use_the_shared_parameter_contract_after_self() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fallback() -> str:\n  return \"label\"\n\nmodel Shelf[T]:\n  def label[U](self, owner_items: list[T] = [], method_items: list[U] = [], suffix: str = \"\", fallback_label: str = fallback()) -> str:\n    return suffix\n";
    let module = build(source, &["m", "method_default"])?;
    let label = module
        .bodies
        .iter()
        .find(|body| body.name == "label")
        .ok_or("expected the label method Body IR")?;
    let self_param = label.params.first().ok_or("expected the self parameter")?;
    let owner_items = label.params.get(1).ok_or("expected the owner-generic parameter")?;
    let method_items = label.params.get(2).ok_or("expected the method-generic parameter")?;
    let suffix = label.params.get(3).ok_or("expected the literal default parameter")?;
    let fallback_label = label.params.get(4).ok_or("expected the call default parameter")?;

    assert_eq!(self_param.local, bir::LocalId(0));
    assert!(
        self_param.span.start < self_param.span.end,
        "the synthetic receiver must carry its documented declaration-span fallback"
    );
    assert!(matches!(&self_param.default, bir::CallableParamDefault::Required));
    assert!(matches!(&owner_items.default, bir::CallableParamDefault::Source(_)));
    assert!(matches!(&method_items.default, bir::CallableParamDefault::Source(_)));
    let bir::CallableParamDefault::Source(suffix_default) = &suffix.default else {
        return Err("a checked method literal default must become a deferred computation".into());
    };
    let literal_start = source.find("\"\"").ok_or("missing method literal default spelling")?;
    assert_eq!(
        suffix_default.span,
        HirSourceSpan::new(literal_start, literal_start + "\"\"".len())
    );
    assert!(suffix_default.stmts.is_empty());
    assert_eq!(
        suffix_default.result,
        bir::Operand::Constant(bir::Constant::Str(String::new()))
    );
    let bir::CallableParamDefault::Source(fallback_default) = &fallback_label.default else {
        return Err("a checked method call default must become a deferred computation".into());
    };
    let call_start = source
        .rfind("fallback()")
        .ok_or("missing method call default spelling")?;
    assert_eq!(
        fallback_default.span,
        HirSourceSpan::new(call_start, call_start + "fallback()".len())
    );
    assert_eq!(fallback_default.stmts.len(), 1);
    assert!(
        !label.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        )),
        "the method default call must not be appended to the ordinary method body: {label:?}"
    );
    assert_eq!(label.param_locals.len(), 5);

    Ok(())
}

#[test]
fn trait_method_default_uses_a_deferred_source_computation() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fallback() -> str:\n  return \"hello\"\n\ntrait Greeter:\n  def greet(self, greeting: str = fallback()) -> str:\n    return greeting\n";
    let module = build(source, &["m", "trait_method_default"])?;
    let greet = module
        .bodies
        .iter()
        .find(|body| body.name == "greet")
        .ok_or("expected the greet trait-method Body IR")?;
    let self_param = greet.params.first().ok_or("expected the self parameter")?;
    let greeting = greet.params.get(1).ok_or("expected the greeting parameter")?;

    assert!(matches!(&self_param.default, bir::CallableParamDefault::Required));
    let bir::CallableParamDefault::Source(default) = &greeting.default else {
        return Err("a checked trait-method default must become a deferred computation".into());
    };
    let default_start = source
        .rfind("fallback()")
        .ok_or("missing trait-method default source spelling")?;
    assert_eq!(
        default.span,
        HirSourceSpan::new(default_start, default_start + "fallback()".len())
    );
    let [call] = default.stmts.as_slice() else {
        return Err("the deferred trait-method default should contain one call statement".into());
    };
    assert!(matches!(
        &call.kind,
        bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            ..
        } if target.name == "fallback"
    ));
    assert!(
        !greet.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        )),
        "the trait-method default call must not be appended to the ordinary method body: {greet:?}"
    );

    Ok(())
}

#[test]
fn unrepresentable_default_is_a_parameter_refusal_at_its_own_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep(payload: bytes = b\"x\") -> bytes:\n  return payload\n";
    let module = build(source, &["m", "unsupported_default"])?;
    let keep = module.bodies.first().ok_or("expected the keep function Body IR")?;
    let payload = keep.params.first().ok_or("expected the payload parameter")?;

    let bir::CallableParamDefault::Unsupported { span, description } = &payload.default else {
        return Err("bytes defaults must refuse instead of pretending to be executable".into());
    };
    assert_eq!(description, "bytes literal");
    let default_start = source.find("b\"x\"").ok_or("missing bytes default spelling")?;
    assert_eq!(
        *span,
        HirSourceSpan::new(default_start, default_start + "b\"x\"".len()),
        "the refusal must retain the unsupported default expression's exact source span"
    );
    assert_eq!(
        keep.locals.len(),
        1,
        "refused speculative lowering must not leak a default temporary or external local: {keep:?}"
    );
    assert!(
        !keep
            .block
            .stmts
            .iter()
            .any(|statement| matches!(&statement.kind, bir::StatementKind::Unsupported { .. })),
        "the refusal belongs to the parameter contract, not the normal function body: {keep:?}"
    );

    Ok(())
}

#[test]
fn unsupported_race_arm_in_a_default_is_found_at_its_nested_source_span() {
    // A race remains a structured Body-IR node even when one arm has an unsupported construct. Callable
    // defaults have the stricter contract: the whole deferred computation must be executable, so the default
    // boundary must find that nested refusal and retain the nested construct's span for direct consumers.
    let unsupported_span = HirSourceSpan::new(24, 34);
    let statements = vec![bir::Statement {
        kind: bir::StatementKind::Race {
            destination: None,
            arms: vec![bir::RaceArm {
                awaitable: bir::Operand::Constant(bir::Constant::Int(1)),
                binding: bir::LocalId(0),
                body: bir::Block {
                    scope: bir::ScopeId(1),
                    stmts: vec![bir::Statement {
                        kind: bir::StatementKind::Unsupported {
                            description: "power operator".to_string(),
                        },
                        span: unsupported_span,
                    }],
                },
                result: bir::Operand::Constant(bir::Constant::Int(0)),
            }],
        },
        span: HirSourceSpan::new(10, 40),
    }];

    assert_eq!(
        first_unsupported_default_statement(&statements),
        Some((unsupported_span, "power operator".to_string())),
        "a direct consumer must refuse the nested construct rather than accept a partially unsupported default"
    );
}

#[test]
fn unsupported_rvalue_bodies_in_a_default_are_found_at_their_nested_source_spans() {
    // A source default can construct a closure or generator, or evaluate a match, whose structured Body IR owns
    // more statements than the outer assignment exposes. Those statement sequences are still part of the direct
    // default contract: a consumer must receive their original refusal span instead of a misleading `Source`.
    let unsupported = |span: HirSourceSpan, description: &str| bir::Statement {
        kind: bir::StatementKind::Unsupported {
            description: description.to_string(),
        },
        span,
    };
    let assignment = |rvalue| bir::Statement {
        kind: bir::StatementKind::Assign {
            place: bir::Place::from_local(bir::LocalId(0)),
            rvalue,
        },
        span: HirSourceSpan::new(0, 80),
    };
    let result = bir::Operand::Constant(bir::Constant::Int(0));
    let closure_span = HirSourceSpan::new(10, 20);
    let generator_span = HirSourceSpan::new(21, 31);
    let guard_span = HirSourceSpan::new(32, 42);
    let body_span = HirSourceSpan::new(43, 53);
    let cases = vec![
        (
            vec![assignment(bir::Rvalue::Closure {
                params: Vec::new(),
                captured_operands: Vec::new(),
                body: Box::new(bir::ClosureBody {
                    capture_locals: Vec::new(),
                    stmts: vec![unsupported(closure_span, "closure body")],
                    result: result.clone(),
                }),
            })],
            closure_span,
            "closure body",
        ),
        (
            vec![assignment(bir::Rvalue::Generator {
                source: bir::Operand::Constant(bir::Constant::Int(1)),
                captured_operands: Vec::new(),
                body: Box::new(bir::GeneratorBody {
                    source_local: bir::LocalId(1),
                    capture_locals: Vec::new(),
                    stmts: vec![unsupported(generator_span, "generator body")],
                }),
            })],
            generator_span,
            "generator body",
        ),
        (
            vec![assignment(bir::Rvalue::Match {
                scrutinee: bir::Operand::Constant(bir::Constant::Int(1)),
                arms: vec![bir::MatchArm {
                    pattern: bir::Pattern::Wildcard,
                    guard_stmts: vec![unsupported(guard_span, "match guard")],
                    guard: Some(bir::Operand::Constant(bir::Constant::Bool(true))),
                    body_stmts: Vec::new(),
                    result: result.clone(),
                }],
            })],
            guard_span,
            "match guard",
        ),
        (
            vec![assignment(bir::Rvalue::Match {
                scrutinee: bir::Operand::Constant(bir::Constant::Int(1)),
                arms: vec![bir::MatchArm {
                    pattern: bir::Pattern::Wildcard,
                    guard_stmts: Vec::new(),
                    guard: None,
                    body_stmts: vec![unsupported(body_span, "match body")],
                    result,
                }],
            })],
            body_span,
            "match body",
        ),
    ];

    for (statements, span, description) in cases {
        assert_eq!(
            first_unsupported_default_statement(&statements),
            Some((span, description.to_string())),
            "a nested {description} refusal must prevent an incomplete default computation from becoming Source"
        );
    }
}

#[test]
fn invalid_default_is_rejected_before_body_ir_is_built() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def choose(value: int = \"wrong\") -> int:\n  return value\n";
    let error = build(source, &["m", "invalid_default"])
        .err()
        .ok_or("a mismatched callable default must be rejected before Body IR construction")?;
    assert!(
        error.to_string().contains("Type mismatch: expected 'int', found 'str'"),
        "the source typechecker must reject the mismatched default before a Body-IR consumer sees it: {error}"
    );

    Ok(())
}

#[test]
fn refused_default_restores_ownership_state_before_local_ids_are_reused() -> Result<(), Box<dyn std::error::Error>> {
    // Lowering the partial moves one of its synthesized forwarding locals before the bytes literal refuses.
    // The transaction must discard that move before `second` reuses the local id in the normal body, or the
    // required root-scope drop would silently disappear.
    let source = "def route(method: str) -> str:\n  return method\n\ndef choose(value: str = (partial route(method=\"GET\")) + b\"x\") -> str:\n  first = \"first\"\n  second = \"second\"\n  return first\n";
    let (module, _diagnostics) = build_after_expected_typecheck_errors(source, &["m", "default_ownership_rollback"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let value = choose.params.first().ok_or("expected choose's value parameter")?;
    assert!(matches!(
        &value.default,
        bir::CallableParamDefault::Unsupported { description, .. } if description == "bytes literal"
    ));
    let second = choose
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("second"))
        .ok_or("expected second binding after refused default")?;
    assert!(
        choose.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Drop { local } if *local == second.id
        )),
        "a stale speculative move must not suppress second's required drop: {choose:?}"
    );

    Ok(())
}

#[test]
fn invalid_callable_defaults_remain_body_ir_refusals_without_implicit_captures()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "earlier parameter",
            "def choose(first: str, second: str = first) -> str:\n  return second\n",
            "first",
        ),
        (
            "receiver",
            "model Label:\n  text: str\n\n  def choose(self, value: str = self.text) -> str:\n    return value\n",
            "self.text",
        ),
        (
            "bare field",
            "model Label:\n  text: str\n\n  def choose(self, value: str = text) -> str:\n    return value\n",
            "text",
        ),
        (
            "bare property",
            "model Label:\n  text: str\n\n  property display -> str:\n    return self.text\n\n  def choose(self, value: str = display) -> str:\n    return value\n",
            "display",
        ),
    ];

    for (case, source, default_spelling) in cases {
        let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "default_capture"])?;
        let rejected_name = match default_spelling.split('.').next() {
            Some(name) => name,
            None => default_spelling,
        };
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.contains(rejected_name)),
            "the source checker must reject the {case} default before Body IR: {diagnostics:?}"
        );
        let choose = module
            .bodies
            .iter()
            .find(|body| body.name == "choose")
            .ok_or("expected the choose Body IR")?;
        let parameter = choose.params.last().ok_or("expected the defaulted parameter")?;
        let bir::CallableParamDefault::Unsupported { span, description } = &parameter.default else {
            return Err(format!("a {case} default must not fabricate a callable-frame or instance capture").into());
        };
        let default_start = source
            .rfind(default_spelling)
            .ok_or("missing invalid default source spelling")?;
        assert_eq!(
            *span,
            HirSourceSpan::new(default_start, default_start + default_spelling.len()),
            "the {case} refusal must preserve the whole default expression span"
        );
        assert!(description.contains(rejected_name));
        assert!(
            !choose
                .locals
                .iter()
                .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
            "a direct consumer must not need an implicit lexical lookup for the refused {case} default: {choose:?}"
        );
    }

    Ok(())
}

#[test]
fn validated_newtype_default_remains_a_visible_body_ir_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let source = "type Attempts = newtype int:\n  def from_underlying(n: int) -> Result[Attempts, ValidationError]:\n    return Ok(Attempts(n))\n\ndef choose(value: Attempts = 3) -> Attempts:\n  return value\n";
    let module = build(source, &["m", "newtype_default"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let value = choose.params.first().ok_or("expected the newtype default parameter")?;
    let bir::CallableParamDefault::Unsupported { span, description } = &value.default else {
        return Err("a default requiring validated-newtype coercion must not become a raw source computation".into());
    };
    let default_start = source.rfind("3)").ok_or("missing newtype default spelling")?;
    assert_eq!(*span, HirSourceSpan::new(default_start, default_start + 1));
    assert_eq!(
        description,
        "default requires a validated-newtype coercion Body IR does not yet represent"
    );
    assert_eq!(choose.locals.len(), 1);
    assert!(
        !choose
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "the newtype refusal must not leave a hidden source lookup in the callable body: {choose:?}"
    );

    Ok(())
}

#[test]
fn aliased_method_parameter_type_retains_the_checked_callable_type() -> Result<(), Box<dyn std::error::Error>> {
    // `UserId` is a type alias for `int` (RFC-style `type X = Y`). A naive re-parse of the raw `id: UserId`
    // annotation inside Body IR (with no alias table of its own) could only produce `Named("UserId")`; the
    // checked callable type resolves the alias all the way through, so the local must show `int`.
    let source = "type UserId = int\n\nmodel Account:\n  balance: int\n\n  def credit(self, id: UserId, amount: int) -> int:\n    return self.balance + amount\n";
    let module = build(source, &["m", "aliased_param"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 id : int [param]"),
        "an aliased parameter type must resolve through the alias like any other checked expression, not stay \
         the raw `UserId` annotation spelling: {snapshot}"
    );

    Ok(())
}

#[test]
fn generic_method_parameter_type_retains_the_owner_type_variable() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class Box[T]:\n  value: T\n\n  def replace(mut self, other: T) -> None:\n    self.value = other\n\n  def wrap(mut self, items: list[T]) -> None:\n    pass\n";
    let module = build(source, &["m", "generic_param"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 other : T [param]"),
        "a bare owner type-variable parameter must retain the checked type variable: {snapshot}"
    );
    assert!(
        snapshot.contains("local 1 items : List[T] [param]"),
        "a generic collection parameter must retain its checked element type variable: {snapshot}"
    );

    Ok(())
}

#[test]
fn static_method_parameter_types_are_recorded_like_ordinary_methods() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "model Counter:\n  value: int\n\n  def from_value(amount: int) -> Counter:\n    return Counter(value=amount)\n";
    let module = build(source, &["m", "static_param"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body from_value decl:m::static_param::Counter::from_value"));
    assert!(
        !snapshot.contains("[receiver"),
        "a static/associated method (receiver: None) must not declare a receiver local: {snapshot}"
    );
    assert!(
        snapshot.contains("local 0 amount : int [param]"),
        "a static method's ordinary parameters must resolve the same way an instance method's do: {snapshot}"
    );

    Ok(())
}

#[test]
fn overloaded_method_declarations_retain_distinct_parameter_types_by_declaration_span()
-> Result<(), Box<dyn std::error::Error>> {
    // Two `add` methods on the same owner, distinguished only by adopting two instantiations of the same
    // generic trait (RFC 042 multi-instantiation) -- the language surface's one legitimate way to declare
    // same-name, same-owner method overloads with genuinely different parameter types. If the checked binding
    // table were keyed by `(owner, method_name)` alone (like `decorated_method_bindings`), the second
    // declaration would silently overwrite the first and both bodies would report the same parameter type.
    let source = "trait Adder[T]:\n  def add(self, x: T) -> T: ...\n\nmodel Calc with Adder[int], Adder[str]:\n  count: int\n\n  def add(self, x: int) -> int:\n    return x\n\n  def add(self, x: str) -> str:\n    return x\n";
    let module = build(source, &["m", "overload_param"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 x : int [param]"),
        "the int-instantiated overload must keep its own checked parameter type: {snapshot}"
    );
    assert!(
        snapshot.contains("local 1 x : str [param]"),
        "the str-instantiated overload must keep its own distinct checked parameter type, not collide with the \
         int overload recorded under the same method name: {snapshot}"
    );

    Ok(())
}

#[test]
fn method_parameter_type_falls_back_to_unknown_only_when_the_typechecker_binding_is_absent()
-> Result<(), Box<dyn std::error::Error>> {
    // A successful typecheck always populates `method_bindings_by_span` for every method Body IR actually
    // lowers a body for (see `TypeChecker::check_method_with_self_ty`), so the only way to observe the
    // fallback honestly is to simulate the checked fact genuinely being absent -- exercising the same
    // defence-in-depth path `lower_method_body` falls back to, rather than asserting on a state ordinary
    // typechecking can never produce.
    let source =
        "model Counter:\n  value: int\n\n  def add(self, amount: int) -> int:\n    return self.value + amount\n";
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = vec!["m".to_string(), "fallback_param".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let mut type_info = checker.type_info().clone();
    type_info.declarations.method_bindings_by_span.clear();

    let module = build_body_ir_module_v0(&program, &module_path, &type_info);
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 amount : ? [param]"),
        "with no recorded checked binding for this declaration, the parameter must fall back to the explicit \
         Unknown type rather than guessing from the raw annotation: {snapshot}"
    );

    Ok(())
}

#[test]
fn lowers_compound_assignment_as_a_read_modify_write() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def accumulate(step: int) -> int:\n  mut total = 0\n  total += step\n  return total\n";
    let module = build(source, &["m", "compound"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "compound assignment should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains(" + "),
        "compound assignment should desugar through a binary op: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_compound_string_assignment_through_the_string_concat_helper() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def greet(name: str) -> str:\n  mut out = \"hi \"\n  out += name\n  return out\n";
    let module = build(source, &["m", "compound_str"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("call helper:str_concat"),
        "string compound assignment should route through the same helper as `+`: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_field_assignment_on_a_mutable_model_parameter() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "model Counter:\n  count: int\n\ndef bump(mut c: Counter) -> int:\n  c.count = c.count + 1\n  return c.count\n";
    let module = build(source, &["m", "field_assign"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "field assignment should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains(".count = "),
        "should assign into the `.count` projection: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_index_assignment_on_a_mutable_list_parameter() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def set_first(mut items: list[int], value: int) -> None:\n  items[0] = value\n  return\n";
    let module = build(source, &["m", "index_assign"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "index assignment should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains("[const(0)] = "),
        "should assign into the `[0]` projection: {snapshot}"
    );
    Ok(())
}

#[test]
fn index_assignment_evaluates_object_before_index() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make_items() -> list[int]:\n  return [1]\n\ndef make_index() -> int:\n  return 0\n\ndef assign() -> None:\n  make_items()[make_index()] = 7\n  return\n";
    let module = build(source, &["m", "index_assignment_order"])?;
    let snapshot = module.render_snapshot();
    let object_call = snapshot
        .find("call fn:make_items()")
        .ok_or("missing index-assignment object call")?;
    let index_call = snapshot
        .find("call fn:make_index()")
        .ok_or("missing index-assignment index call")?;

    assert!(
        object_call < index_call,
        "index assignment must evaluate its object before its index: {snapshot}"
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

#[test]
fn a_local_callable_named_ok_shadows_the_intrinsic_result_constructor() -> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Failure:\n  Shadowed\n\ndef main(Ok: (int) -> Result[int, Failure]) -> Result[int, Failure]:\n  return Ok(42)\n";
    let module = build(source, &["m", "result_constructor_shadow"])?;
    let main = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("the main body must be retained")?;
    let call = single_call(main)?;
    let bir::StatementKind::Call {
        callee: bir::Callee::Function(bir::CallableTarget::Local(target)),
        ..
    } = call
    else {
        return Err("a callable parameter named Ok must remain a local Body-IR call".into());
    };
    let parameter = main
        .param_locals
        .first()
        .ok_or("the callable parameter must retain a local id")?;
    assert_eq!(target.operand.place, bir::Place::from_local(*parameter));
    Ok(())
}

#[test]
fn lowers_an_fstring_into_a_format_rvalue_with_literal_and_display_parts() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def greet(name: str) -> str:\n  return f\"hello {name}\"\n";
    let module = build(source, &["m", "fstring_display"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("fstring(lit(\"hello \"), move(_0, last_use):display"),
        "f-string should lower to an explicit Format rvalue with literal and display parts: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_an_fstring_debug_interpolation_using_the_debug_style() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def show(n: int) -> str:\n  return f\"n={n:?}\"\n";
    let module = build(source, &["m", "fstring_debug"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains(":debug"),
        "`{{n:?}}` should lower to a Debug-styled format part: {snapshot}"
    );
    assert!(
        !snapshot.contains(":display"),
        "a debug interpolation should not also render as display: {snapshot}"
    );
    Ok(())
}

#[test]
fn fstring_records_the_fstring_runtime_helper_and_allocator_requirements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def label(x: int) -> str:\n  return f\"x={x}\"\n";
    let module = build(source, &["m", "fstring_reqs"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("runtime_requirements:"));
    assert!(snapshot.contains("runtime_helper(fstring)"));
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn fstring_embedded_expression_participates_in_last_use_tracking() -> Result<(), Box<dyn std::error::Error>> {
    // `s` is read twice: once as a plain binding RHS and once inside the f-string. The f-string's embedded read
    // must still count toward `s`'s last-use countdown (see `count_reads_in_expr`'s `ast::Expr::FString` arm),
    // so the first (non-last) read clones and only the f-string's read -- the true last use -- moves.
    let source = "def dup(s: str) -> str:\n  first = s\n  return f\"value={s}\"\n";
    let module = build(source, &["m", "fstring_last_use"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("clone(_0)"),
        "the first, non-last read of `s` should clone: {snapshot}"
    );
    assert!(
        snapshot.contains("fstring(lit(\"value=\"), move(_0, last_use):display"),
        "the f-string's embedded read is the true last use and should move: {snapshot}"
    );
    Ok(())
}

#[test]
fn comprehension_embedded_expression_participates_in_last_use_tracking() -> Result<(), Box<dyn std::error::Error>> {
    // Mirrors `fstring_embedded_expression_participates_in_last_use_tracking`'s regression shape for the same
    // class of bug: `count_reads_in_expr` must recurse into `ast::Expr::ListComp`'s element expression, or the
    // earlier, non-comprehension read of `s` on the first line would be miscounted as the last use (`Move`)
    // even though the list comprehension on the next line reads `s` again -- an unsound move, not merely an
    // imprecise clone. `s` is read twice: once as a plain binding RHS, once inside the comprehension's element.
    let source = "def dup(s: str, items: list[int]) -> list[str]:\n  first = s\n  return [s for n in items]\n";
    let module = build(source, &["m", "comp_last_use"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("clone(_0)"),
        "the first, non-last read of `s` should clone because the comprehension reads it again: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_closure_capturing_nothing_with_an_empty_capture_list() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make(step: int) -> int:\n  add: (int) -> int = (x) => x + 1\n  return add(step)\n";
    let module = build(source, &["m", "closure_no_capture"])?;
    let snapshot = module.render_snapshot();
    let make = module.bodies.first().ok_or("expected the make function Body IR")?;
    let closure_params = make
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { params, .. },
                ..
            } => Some(params),
            _ => None,
        })
        .ok_or("expected the closure literal")?;
    let x = closure_params.first().ok_or("expected the closure parameter")?;

    assert!(
        !snapshot.contains("unsupported("),
        "a closure literal should lower fully, not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains("captures=[]"),
        "a closure that reads no outer variable should capture nothing: {snapshot}"
    );
    assert!(
        snapshot.contains("closure(params=[x: int local=_"),
        "the closure's own parameter should be recorded: {snapshot}"
    );
    assert_eq!(x.name, "x");
    assert_eq!(x.local, bir::LocalId(1));
    assert_eq!(
        x.span,
        HirSourceSpan::new(
            source.find("(x)").ok_or("missing closure parameter spelling")? + 1,
            source.find("(x)").ok_or("missing closure parameter spelling")? + 2,
        )
    );
    assert!(matches!(&x.default, bir::CallableParamDefault::Required));
    Ok(())
}

#[test]
fn source_closure_default_syntax_is_refused_before_body_ir_exists() -> Result<(), Box<dyn std::error::Error>> {
    // Closure parameter parsing deliberately accepts identifiers only. Keeping this source-level failure explicit
    // means #1172 does not invent an executable local-closure default from parser-unrepresentable syntax.
    let source = "def make() -> int:\n  value: (int) -> int = (x = 1) => x\n  return value(2)\n";
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let errors = match parser::parse(&tokens) {
        Ok(_) => return Err("closure-default source syntax must not parse into a Body-IR input".into()),
        Err(errors) => errors,
    };
    let parameter_start = source
        .find("x = 1")
        .ok_or("missing closure default parameter spelling")?;
    let parameter_end = parameter_start + "x = 1".len();

    assert!(
        errors
            .iter()
            .any(|error| { error.span.start >= parameter_start && error.span.end <= parameter_end }),
        "the parser must refuse the closure-default spelling at its own source parameter range: {errors:?}"
    );

    Ok(())
}

#[test]
fn lowers_a_closure_capturing_an_outer_variable_with_a_real_clone_fact() -> Result<(), Box<dyn std::error::Error>> {
    // `name` is read once inside the closure (a capture) and again afterward by `return name`, so the capture
    // is not the last use: it must clone, not move -- a real Duckborrower fact, not a placeholder.
    let source = "def greet(name: str) -> str:\n  make_msg: () -> str = () => name\n  return name\n";
    let module = build(source, &["m", "closure_capture_clone"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("captures=[clone(_0)]"),
        "capturing `name` before its last use should clone: {snapshot}"
    );
    assert!(snapshot.contains("local 1 name : str [captured]"));
    Ok(())
}

#[test]
fn lowers_a_closure_capturing_an_outer_variable_at_its_last_use() -> Result<(), Box<dyn std::error::Error>> {
    // `name` is read once, inside the closure, and never again -- the capture itself is `name`'s last use, so
    // it should move rather than clone.
    let source = "def greet(name: str) -> str:\n  make_msg: () -> str = () => name\n  return make_msg()\n";
    let module = build(source, &["m", "closure_capture_move"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("captures=[move(_0, last_use)]"),
        "capturing `name` at its only/last use should move: {snapshot}"
    );
    Ok(())
}

#[test]
fn invokes_a_stored_closure_through_its_local_operand_and_preserves_its_capture_ownership()
-> Result<(), Box<dyn std::error::Error>> {
    // The local `decorate` is a value with a lexical environment, not a declaration named `decorate`.
    // Its call target must therefore retain the closure-local read (including its ownership fact) rather than
    // being approximated as a direct function call and losing the relationship to the captured `prefix`.
    let source = "def greet(prefix: str) -> str:\n  decorate: (str) -> str = (suffix) => prefix + suffix\n  return decorate(\"!\")\n";
    let module = build(source, &["m", "stored_closure_call"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("captures=[move(_0, last_use)]"),
        "the closure must own its last-use capture explicitly: {snapshot}"
    );
    assert!(
        snapshot.contains("call local:move(_"),
        "the stored closure must be invoked through its local operand: {snapshot}"
    );
    assert!(
        !snapshot.contains("call fn:decorate("),
        "a stored closure must never be misrepresented as a named function: {snapshot}"
    );
    Ok(())
}

#[test]
fn closure_body_can_still_read_its_capture_after_lowering_restores_outer_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    // The closure's own capture-binding local must resolve inside the closure body (via `result:`), and the
    // enclosing function's own read of `step` afterward must resolve back to the *outer* local, not the
    // closure's capture -- i.e. `Self::lower_closure`'s save/restore of `self.bindings` must round-trip.
    let source = "def make(step: int) -> int:\n  add: () -> int = () => step\n  return step\n";
    let module = build(source, &["m", "closure_capture_restore"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("result: copy(_1)"),
        "the closure body should read its own capture-binding local for `step` (an `int`, so `copy`): {snapshot}"
    );
    assert!(
        snapshot.contains("return copy(_0)"),
        "the function's own trailing `return step` must resolve back to the *outer* local `_0`, not the \
         closure's capture-binding local `_1`, proving the save/restore round-trips: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "nothing here should fall back: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_partial_callable_into_a_forwarding_closure() -> Result<(), Box<dyn std::error::Error>> {
    // A local partial retains every target parameter in its callable surface. The captured `method` is a
    // defaulted, overrideable slot, while `path` remains required and `content_type` keeps the target default.
    // `method` is read again after construction, so the non-Copy preset capture must be a real clone fact.
    let source = "def route(method: str, path: str, content_type: str = \"text\") -> str:\n  return method + path + content_type\n\ndef make(method: str) -> str:\n  get = partial route(method=method)\n  named = get(method=\"POST\", path=\"/named\")\n  return method + get(\"/health\")\n";
    let module = build(source, &["m", "partial_callable"])?;
    let snapshot = module.render_snapshot();
    let make = module
        .bodies
        .iter()
        .find(|body| body.name == "make")
        .ok_or("expected the make function Body IR")?;
    let (partial_params, captured_operands, closure_body) = make
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue:
                    bir::Rvalue::Closure {
                        params,
                        captured_operands,
                        body,
                    },
                ..
            } => Some((params, captured_operands, body)),
            _ => None,
        })
        .ok_or("expected the synthesized partial closure")?;
    let method = partial_params
        .iter()
        .find(|param| param.name == "method")
        .ok_or("expected the captured method parameter")?;
    let content_type = partial_params
        .iter()
        .find(|param| param.name == "content_type")
        .ok_or("expected the target default parameter")?;

    assert!(matches!(
        &method.default,
        bir::CallableParamDefault::PartialPreset { .. }
    ));
    let bir::CallableParamDefault::PartialPreset { capture } = &method.default else {
        return Err("the partial preset must retain its capture local".into());
    };
    assert_eq!(closure_body.capture_locals, vec![*capture]);
    assert!(matches!(
        captured_operands.first(),
        Some(bir::Operand::Place(bir::PlaceOperand {
            fact: bir::OwnershipFact::Clone,
            ..
        }))
    ));
    let bir::CallableParamDefault::Source(content_type_default) = &content_type.default else {
        return Err("the unpresetted checked default must remain a deferred source computation".into());
    };
    let content_type_start = source.find("\"text\"").ok_or("missing content_type default spelling")?;
    assert_eq!(
        content_type_default.span,
        HirSourceSpan::new(content_type_start, content_type_start + "\"text\"".len())
    );
    assert!(content_type_default.stmts.is_empty());
    assert_eq!(
        content_type_default.result,
        bir::Operand::Constant(bir::Constant::Str("text".to_string()))
    );
    let supplied_slots: Vec<Vec<usize>> = make
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Local(target)),
                ..
            } => match &target.binding {
                bir::ArgumentBinding::Resolved { arguments, .. } => {
                    Some(arguments.iter().map(|argument| argument.slot).collect())
                }
                bir::ArgumentBinding::UnresolvedPositional => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        supplied_slots.iter().any(|slots| slots.as_slice() == [0, 1]),
        "a named argument must override the captured preset in its declaration slot: {make:?}"
    );
    assert!(
        supplied_slots.iter().any(|slots| slots.as_slice() == [1]),
        "a positional residual argument must omit the preset and trailing source default by declaration slot: {make:?}"
    );
    assert!(
        snapshot.contains("call local:move(_"),
        "a stored partial must be invoked through its local operand: {snapshot}"
    );
    assert!(
        !snapshot.contains("call fn:get("),
        "a stored partial must never be misrepresented as a named function: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:route("),
        "the synthesized closure body should forward into a call to the target function: {snapshot}"
    );
    Ok(())
}

#[test]
fn stored_partial_refuses_too_few_or_too_many_residual_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let too_few = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(9)\n";
    let (too_few_module, diagnostics) = build_after_expected_typecheck_errors(too_few, &["m", "partial_too_few"])?;
    let too_few_snapshot = too_few_module.render_snapshot();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("Missing required argument 'c'")),
        "the source checker must diagnose the missing residual parameter: {diagnostics:?}"
    );
    assert!(
        too_few_snapshot
        .contains("unsupported(local callable `add_with_one` expects at least 2 required arguments, got 1; missing required parameter `c`)"),
        "a partial invocation may not omit a required residual argument: {too_few_snapshot}"
    );

    let too_many = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(9, 2, 3)\n";
    let (too_many_module, diagnostics) = build_after_expected_typecheck_errors(too_many, &["m", "partial_too_many"])?;
    let too_many_snapshot = too_many_module.render_snapshot();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("expects 2 argument(s), got 3")),
        "the source checker must use the residual arity: {diagnostics:?}"
    );
    assert!(
        too_many_snapshot
            .contains("unsupported(local callable `add_with_one` expects at most 2 positional arguments, got 3)"),
        "a partial invocation may not provide more residual positional arguments than its target accepts: {too_many_snapshot}"
    );
    assert!(
        !too_many_snapshot.contains("call fn:add_with_one("),
        "invalid residual arity must not be approximated as a named-function call: {too_many_snapshot}"
    );
    Ok(())
}

#[test]
fn stored_partial_passes_positional_residual_arguments_in_target_declaration_order()
-> Result<(), Box<dyn std::error::Error>> {
    // Positional calls skip the defaulted preset `a`, while Body IR records their target slots explicitly.
    let source = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(9, 2)\n";
    let module = build(source, &["m", "partial_order"])?;
    let snapshot = module.render_snapshot();
    let local_call = snapshot
        .lines()
        .find(|line| line.contains("call local:"))
        .ok_or("stored partial call missing from Body IR snapshot")?;
    assert!(
        local_call.contains("const(9), const(2)"),
        "residual positional arguments must remain b/c ordered while the preset default stays captured: {local_call}"
    );
    assert!(
        local_call.contains("slots=[1, 2]"),
        "positional residual arguments must map to their target declaration slots: {local_call}"
    );
    assert!(
        local_call.contains("defaults=[0]"),
        "the skipped preset slot must be recorded as a defaulted slot rather than left implicit: {local_call}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "the residual Body IR call itself should be executable once admitted by the typechecker: {snapshot}"
    );
    Ok(())
}

#[test]
fn stored_partial_allows_a_named_preset_override() -> Result<(), Box<dyn std::error::Error>> {
    // The construction-time capture remains the default, but a named argument replaces it for this invocation.
    let source = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(a=7, b=9, c=2)\n";
    let module = build(source, &["m", "partial_named_override"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a named preset override must lower as an ordinary local callable invocation: {snapshot}"
    );
    assert!(
        snapshot.contains("const(7), const(9), const(2)"),
        "the local invocation must retain the explicit target slots for the override and residual values: {snapshot}"
    );
    // An absent slot/defaults suffix is the identity binding: every declared slot filled, in declaration order.
    // That is precisely what distinguishes a named override from the positional call above, which skips the
    // preset and therefore renders `slots=[1, 2] defaults=[0]`.
    let local_call = snapshot
        .lines()
        .find(|line| line.contains("call local:"))
        .ok_or("stored partial call missing from Body IR snapshot")?;
    assert!(
        !local_call.contains("slots=") && !local_call.contains("defaults="),
        "a named override must occupy the captured preset's declaration slot rather than skipping it: {local_call}"
    );
    Ok(())
}

#[test]
fn partial_callable_restores_enclosing_bindings_after_lowering() -> Result<(), Box<dyn std::error::Error>> {
    // `partial join(prefix="hi ")` synthesizes a residual closure parameter called `suffix`, but that internal
    // binding must not replace the enclosing function parameter of the same name. The trailing return must read
    // the original function parameter (`_0`), not the closure-only parameter allocated while lowering the
    // partial expression.
    let source = "def join(prefix: str, suffix: str) -> str:\n  return prefix + suffix\n\ndef keep_outer(suffix: str) -> str:\n  formatter = partial join(prefix=\"hi \")\n  return suffix\n";
    let module = build(source, &["m", "partial_binding_restore"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("return move(_0, last_use)"),
        "the trailing return must resolve the enclosing `suffix` parameter, not a synthesized partial local: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_single_yield_and_marks_the_body_a_generator() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def numbers() -> Generator[int]:\n  yield 1\n";
    let module = build(source, &["m", "single_yield"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("yield const(1)"),
        "yield should lower to an explicit Yield statement: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "statement-position yield with a value must not fall back to Unsupported: {snapshot}"
    );
    let body = module
        .bodies
        .iter()
        .find(|b| b.name == "numbers")
        .ok_or("numbers body missing from module")?;
    assert!(
        body.is_generator(),
        "a body containing a yield must report is_generator()"
    );
    Ok(())
}

#[test]
fn lowers_multiple_yields_across_control_flow_inside_a_loop() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def counter(n: int) -> Generator[int]:\n  mut i = 0\n  while i < n:\n    yield i\n    i = i + 1\n  yield -1\n";
    let module = build(source, &["m", "loop_yield"])?;
    let snapshot = module.render_snapshot();

    // Two yields: one nested inside the normalized `loop:` the `while` desugars into, one at the top level
    // after the loop.
    assert_eq!(
        snapshot.matches("yield ").count(),
        2,
        "expected exactly two yield statements: {snapshot}"
    );
    assert!(
        snapshot.contains("loop:"),
        "while should still desugar to a normalized loop: {snapshot}"
    );
    let body = module
        .bodies
        .iter()
        .find(|b| b.name == "counter")
        .ok_or("counter body missing from module")?;
    assert!(
        body.is_generator(),
        "a yield nested inside a loop must still be found by is_generator()"
    );
    Ok(())
}

#[test]
fn a_non_generator_function_is_not_reported_as_a_generator() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(x: int, y: int) -> int:\n  return x + y\n";
    let module = build(source, &["m", "not_a_generator"])?;
    let body = module
        .bodies
        .iter()
        .find(|b| b.name == "add")
        .ok_or("add body missing from module")?;
    assert!(
        !body.is_generator(),
        "an ordinary function body must not be reported as a generator"
    );
    Ok(())
}

#[test]
fn yield_records_the_generator_runtime_requirements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def numbers() -> Generator[int]:\n  yield 1\n";
    let module = build(source, &["m", "yield_requirements"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("runtime_requirements:"));
    assert!(snapshot.contains("runtime_helper(generator)"));
    assert!(snapshot.contains("hosted_std"));
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn yielded_expression_participates_in_last_use_tracking() -> Result<(), Box<dyn std::error::Error>> {
    // `s` is read once, inside the yielded value, and never again afterward -- it should read as a last-use
    // `move`, not fall back to an undercounted `clone`/`borrow` the way #1101's f-string bucket found and fixed
    // for embedded f-string reads (`count_reads_in_expr`'s `FString` arm); `Yield` needed the same fix.
    let source = "def one(s: str) -> Generator[str]:\n  yield s\n";
    let module = build(source, &["m", "yield_last_use"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("yield move(_0, last_use)"),
        "the yielded value should be a last-use move: {snapshot}"
    );
    Ok(())
}

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

    // `Some`'s field type is not resolved (v0 does not mirror the existing backend's constructor field-type
    // projection -- see `Pattern`'s own docs), so the binding reads through the conservative
    // non-Copy/projected-read fallback (`borrow`, never `move`) even though `value`'s actual type is `int`.
    assert!(
        snapshot.contains("Some(bind(_1, borrow))"),
        "a positional constructor pattern should bind its field: {snapshot}"
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
fn byte_string_literal_pattern_lowers_to_an_explicit_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    // `bir::Constant` has no byte-string variant (mirrors `lower_literal`'s own gap for a plain literal
    // *expression*), so a match with an unrepresentable arm bails the whole expression to `Unsupported` before
    // lowering the scrutinee, rather than silently mis-rendering the pattern as a catch-all wildcard the way
    // the existing Rust-emission backend's own `lower_pattern` does.
    let source = concat!(
        "def check(data: bytes) -> str:\n",
        "  match data:\n",
        "    case b\"\\x00\":\n",
        "      return \"null\"\n",
        "    case _:\n",
        "      return \"other\"\n",
    );
    let module = build(source, &["m", "match_bytes"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("unsupported(match arm with a byte-string literal pattern)"),
        "should record an explicit placeholder rather than mis-rendering the pattern: {snapshot}"
    );
    Ok(())
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
        snapshot.contains("Circle(bind(_1, borrow)) | Square(bind(_1, borrow))"),
        "both alternatives should bind the same shared local `_1`: {snapshot}"
    );
    Ok(())
}

/// Extract the `_N` place a loop's `IterNext` writes each produced item into, so a destructuring test can assert
/// on projections off that exact local without hard-coding a local number unrelated lowering changes would churn.
fn iter_next_destination(snapshot: &str) -> Option<String> {
    snapshot.lines().find_map(|line| {
        let (destination, _) = line.trim().split_once(" = iter_next(")?;
        Some(destination.to_string())
    })
}

/// Find the `_N` spelling of the local declared for source binding `name`, so a test can assert on reads of that
/// binding without pinning a local number.
fn local_for_binding(snapshot: &str, name: &str) -> Option<String> {
    snapshot.lines().find_map(|line| {
        let (id, tail) = line.trim().strip_prefix("local ")?.split_once(' ')?;
        tail.starts_with(&format!("{name} : ")).then(|| format!("_{id}"))
    })
}

#[test]
fn lowers_a_wildcard_for_pattern_without_declaring_a_binding() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def count(items: list[int]) -> int:\n  mut n = 0\n  for _ in items:\n    n = n + 1\n  return n\n";
    let module = build(source, &["m", "wildcard_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a wildcard loop pattern must lower, not fall back to a placeholder: {snapshot}"
    );
    assert!(
        snapshot.contains(", builtin)"),
        "wildcard iteration still polls the builtin protocol: {snapshot}"
    );
    assert!(
        !snapshot.contains(" _ : "),
        "`_` binds nothing, so it must not become a named local: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_wildcard_for_pattern_over_a_range() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def count(n: int) -> int:\n  mut total = 0\n  for _ in 0..n:\n    total = total + 1\n  return total\n";
    let module = build(source, &["m", "wildcard_range_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a wildcard range loop must keep the normalized counting-loop shape: {snapshot}"
    );
    assert!(
        snapshot.contains("loop:") && snapshot.contains("break"),
        "the range path still desugars to a normalized loop: {snapshot}"
    );
    assert!(
        !snapshot.contains(" _ : "),
        "`_` binds nothing over a range either: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_tuple_for_pattern_into_one_binding_per_element() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def total(pairs: list[tuple[int, int]]) -> int:\n  mut acc = 0\n  for a, b in pairs:\n    acc = acc + a + b\n  return acc\n";
    let module = build(source, &["m", "tuple_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a tuple loop pattern must lower to real bindings: {snapshot}"
    );
    assert!(
        snapshot.contains(" a : int [binding]"),
        "`a` must be a real source binding carrying its resolved element type: {snapshot}"
    );
    assert!(
        snapshot.contains(" b : int [binding]"),
        "`b` must be a real source binding carrying its resolved element type: {snapshot}"
    );

    let destination = iter_next_destination(&snapshot).ok_or("expected an IterNext statement")?;
    assert!(
        snapshot.contains(&format!("copy({destination}.0)")),
        "`a` must bind the produced item's first tuple field: {snapshot}"
    );
    assert!(
        snapshot.contains(&format!("copy({destination}.1)")),
        "`b` must bind the produced item's second tuple field: {snapshot}"
    );
    Ok(())
}

#[test]
fn tuple_for_pattern_bindings_are_readable_inside_the_loop_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def total(pairs: list[tuple[int, int]]) -> int:\n  mut acc = 0\n  for a, b in pairs:\n    acc = acc + a + b\n  return acc\n";
    let module = build(source, &["m", "tuple_for_reads"])?;
    let snapshot = module.render_snapshot();

    for name in ["a", "b"] {
        let local =
            local_for_binding(&snapshot, name).ok_or_else(|| format!("expected a local for `{name}`: {snapshot}"))?;
        assert!(
            snapshot.contains(&format!("copy({local})")),
            "the loop body must read `{name}` through its own binding {local}: {snapshot}"
        );
    }
    Ok(())
}

#[test]
fn lowers_a_tuple_for_pattern_over_a_user_defined_iteration_protocol() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model PairIter:\n  value: int\n\n  def __next__(self) -> Option[tuple[int, int]]:\n    return Some((self.value, self.value))\n\nmodel Pairs:\n  def __iter__(self) -> PairIter:\n    return PairIter(value=0)\n\ndef total() -> int:\n  mut acc = 0\n  for a, b in Pairs():\n    acc = acc + a + b\n  return acc\n";
    let module = build(source, &["m", "protocol_tuple_for"])?;
    let snapshot = module.render_snapshot();

    // Scoped to the loop-pattern refusal specifically: this source's `PairIter(value=0)` constructor also
    // trips Body IR's separate, pre-existing "call with named or unpack arguments" gap, which #1125 does not own.
    assert!(
        !snapshot.contains("unsupported(for-loop pattern"),
        "protocol-driven tuple iteration must lower to real bindings: {snapshot}"
    );
    assert!(
        snapshot.contains("user_defined(__next__)"),
        "the resolved protocol must still drive the poll: {snapshot}"
    );
    assert!(
        snapshot.contains(" a : int [binding]") && snapshot.contains(" b : int [binding]"),
        "both tuple elements must bind with their resolved types: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_nested_tuple_for_pattern_through_projected_subfields() -> Result<(), Box<dyn std::error::Error>> {
    // `for_binding_pattern_item` (`crates/incan_syntax/src/parser/stmts.rs`) admits only `_` or a bare
    // identifier, so a nested loop pattern has no source spelling yet -- see
    // `nested_tuple_for_patterns_have_no_source_spelling_yet`. The typechecker's own
    // `define_for_pattern_bindings` already recurses through nested `Pattern::Tuple` specifically so a
    // hand-built AST cannot reach lowering with a shape lowering does not understand, so this test builds that
    // AST directly and drives the real typecheck-then-lower pipeline over it.
    let source = "def total(pairs: list[tuple[int, tuple[int, int]]]) -> int:\n  mut acc = 0\n  for a, b in pairs:\n    acc = acc + a + b + c\n  return acc\n";
    let module = build_with_nested_for_pattern(source, &["m", "nested_tuple_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a nested tuple loop pattern must lower to real bindings: {snapshot}"
    );
    for name in ["a", "b", "c"] {
        assert!(
            snapshot.contains(&format!(" {name} : int [binding]")),
            "`{name}` must be a real source binding carrying its resolved element type: {snapshot}"
        );
    }

    let destination = iter_next_destination(&snapshot).ok_or("expected an IterNext statement")?;
    assert!(
        snapshot.contains(&format!("copy({destination}.0)")),
        "`a` must bind the outer tuple's first field: {snapshot}"
    );
    assert!(
        snapshot.contains(&format!("copy({destination}.1.0)")),
        "`b` must bind through the nested tuple's first field: {snapshot}"
    );
    assert!(
        snapshot.contains(&format!("copy({destination}.1.1)")),
        "`c` must bind through the nested tuple's second field: {snapshot}"
    );
    Ok(())
}

#[test]
fn nested_tuple_for_patterns_have_no_source_spelling_yet() -> Result<(), Box<dyn std::error::Error>> {
    // Pins the boundary `lowers_a_nested_tuple_for_pattern_through_projected_subfields` works around: Body IR
    // lowers nested loop patterns structurally, but no source syntax produces one today, in a `for` statement or
    // in a comprehension `for` clause (both parse their header through `for_binding_pattern`). #1125 explicitly
    // does not add new source syntax, so this stays a parser-surface gap rather than a lowering gap. When the
    // parser does learn this spelling, this test fails and the nested case can move onto the ordinary `build`
    // path.
    let source = "def total(pairs: list[tuple[int, tuple[int, int]]]) -> int:\n  for a, (b, c) in pairs:\n    pass\n  return 0\n";
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    assert!(
        parser::parse(&tokens).is_err(),
        "a parenthesized nested loop pattern is not part of the source surface yet"
    );
    Ok(())
}

#[test]
fn destructured_for_pattern_bindings_do_not_escape_the_loop_scope() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def keep_outer(a: int, pairs: list[tuple[int, int]]) -> int:\n  for a, b in pairs:\n    pass\n  return a\n";
    let module = build(source, &["m", "tuple_for_scope"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("return copy(_0)"),
        "the trailing read must resolve the enclosing parameter, not the destructured loop local: {snapshot}"
    );
    Ok(())
}

#[test]
fn destructured_for_pattern_bindings_carry_ownership_and_drop_facts() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def widths(pairs: list[tuple[str, str]]) -> int:\n  mut n = 0\n  for head, tail in pairs:\n    n = n + len(head)\n  return n\n";
    let module = build(source, &["m", "tuple_for_drops"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains(" head : str [binding]") && snapshot.contains(" tail : str [binding]"),
        "non-Copy tuple elements must still bind carrying their resolved element type: {snapshot}"
    );

    let destination = iter_next_destination(&snapshot).ok_or("expected an IterNext statement")?;
    assert!(
        snapshot.contains(&format!("borrow({destination}.0)")),
        "a non-Copy element read through a projection borrows rather than moving: {snapshot}"
    );

    // `head` is its call argument's recorded last use and therefore moves; unread `tail` remains live and owes
    // one loop-scope drop. Count exact ids so the enclosing parameter's root-scope drop is not conflated with
    // either binding.
    let body = module.bodies.first().ok_or("expected the widths Body IR")?;
    let head = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("head"))
        .ok_or("missing loop binding `head`")?;
    let tail = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("tail"))
        .ok_or("missing loop binding `tail`")?;
    assert!(snapshot.contains(&format!("move(_{}", head.id.0)));
    assert_eq!(snapshot.matches(&format!("drop _{}", head.id.0)).count(), 0);
    assert_eq!(snapshot.matches(&format!("drop _{}", tail.id.0)).count(), 1);
    Ok(())
}

#[test]
fn a_closure_does_not_capture_names_a_nested_destructuring_pattern_binds() -> Result<(), Box<dyn std::error::Error>> {
    // `a` and `b` are bound by the comprehension's own `for` clause, so they are *not* free variables of the
    // enclosing closure and must never be captured from the enclosing scope -- where they do not exist at all.
    // Before #1125 the free-variable walk only treated a plain `Pattern::Binding` as binding a name, so a
    // destructuring clause pattern left both names looking free.
    let source = "def outer(pairs: list[tuple[int, int]]) -> int:\n  sums: () -> list[int] = () => [a + b for a, b in pairs]\n  return 0\n";
    let module = build(source, &["m", "closure_pattern_capture"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains(" a : ") && !snapshot.contains(" b : "),
        "clause-bound names must not become captured locals of the enclosing closure: {snapshot}"
    );
    assert!(
        snapshot.contains("[captured]"),
        "the closure should still capture the one name it really reads from the enclosing scope: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_tuple_for_pattern_over_a_non_tuple_item_type_is_a_type_error() -> Result<(), Box<dyn std::error::Error>> {
    // Regression for the P1 on #1125: this used to typecheck silently, binding both names as `Unknown`, and
    // Body IR then projected `.0`/`.1` out of an `int`.
    let source = "def total(items: list[int]) -> int:\n  for left, right in items:\n    pass\n  return 0\n";
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".to_string(), "non_tuple_for".to_string()]));

    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("destructuring a non-tuple iteration item must be rejected, not silently bound as Unknown")?;
    let rendered = format!("{errors:?}");
    assert!(
        rendered.contains("Cannot destructure 2 values from iteration item of type 'int'"),
        "the diagnostic should name the offending item type: {rendered}"
    );
    Ok(())
}

#[test]
fn a_tuple_for_pattern_over_a_mismatched_arity_item_type_is_a_type_error() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def total(pairs: list[tuple[int, int]]) -> int:\n  for a, b, c in pairs:\n    pass\n  return 0\n";
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".to_string(), "arity_for".to_string()]));

    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("a wrong-arity tuple loop pattern must be rejected")?;
    let rendered = format!("{errors:?}");
    assert!(
        rendered.contains("Cannot unpack 3 values from tuple with 2 elements"),
        "the arity mismatch should be reported: {rendered}"
    );
    Ok(())
}

#[test]
fn lowering_fails_closed_on_a_tuple_pattern_whose_item_type_is_not_a_tuple() -> Result<(), Box<dyn std::error::Error>> {
    // Defence in depth for the same P1: the typechecker rejects this program, so lowering should only ever see
    // it from a hand-built AST -- and must refuse rather than project `.0`/`.1` out of an `int`.
    let source = "def total(items: list[int]) -> int:\n  for value in items:\n    pass\n  return 0\n";
    let module = build_with_for_pattern_widened_after_typecheck(source, &["m", "fail_closed_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("unsupported(for-loop tuple pattern over non-tuple item type `int`)"),
        "lowering must refuse, naming the item type it cannot destructure: {snapshot}"
    );
    assert!(
        !snapshot.contains(".0)") && !snapshot.contains(".1)"),
        "lowering must not emit tuple-field projections into a non-tuple value: {snapshot}"
    );
    assert!(
        !snapshot.contains(" second : "),
        "no binding may be declared for a refused pattern: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_tuple_for_pattern_over_an_unconstrained_type_variable_is_a_type_error() -> Result<(), Box<dyn std::error::Error>> {
    // An unconstrained `T` can be instantiated as `int`, and Incan has no tuple-shaped bound that could
    // promise otherwise, so this can never be proven safe.
    let source = "def total[T](items: list[T]) -> int:\n  for left, right in items:\n    pass\n  return 0\n";
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".to_string(), "typevar_for".to_string()]));

    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("destructuring an unconstrained type variable must be rejected")?;
    let rendered = format!("{errors:?}");
    assert!(
        rendered.contains("Cannot destructure 2 values from iteration item of type"),
        "the diagnostic should name the underdetermined item type: {rendered}"
    );
    Ok(())
}

#[test]
fn a_tuple_for_pattern_over_type_variable_elements_still_binds() -> Result<(), Box<dyn std::error::Error>> {
    // The shape `crates/incan_stdlib/stdlib/collections.incn` actually uses: the *item* is a tuple, and only
    // its elements are type variables. Rejecting bare type variables must not catch this too.
    let source = "def keys[K, V](items: list[Tuple[K, V]]) -> int:\n  mut n = 0\n  for key, value in items:\n    n = n + 1\n  return n\n";
    let module = build(source, &["m", "typevar_elements_for"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported(for-loop"),
        "a tuple item whose elements are type variables must still bind: {snapshot}"
    );
    assert!(
        snapshot.contains(" key : ") && snapshot.contains(" value : "),
        "both names must bind as real locals: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowering_fails_closed_on_a_tuple_pattern_over_an_unconstrained_type_variable()
-> Result<(), Box<dyn std::error::Error>> {
    // Lowering must apply the same rule the typechecker does, so the two stages cannot disagree about which
    // programs are bindable.
    let source = "def total[T](items: list[T]) -> int:\n  for value in items:\n    pass\n  return 0\n";
    let module = build_with_for_pattern_widened_after_typecheck(source, &["m", "fail_closed_typevar"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("unsupported(for-loop tuple pattern over non-tuple item type"),
        "lowering must refuse an unconstrained type variable, matching the typechecker: {snapshot}"
    );
    assert!(
        !snapshot.contains(".0)") && !snapshot.contains(".1)"),
        "lowering must not emit tuple-field projections into a type variable: {snapshot}"
    );
    Ok(())
}
// ========================================================================
// #1158 -- named, defaulted, and explicitly generic call arguments
// ========================================================================

/// Return the single `Call` statement in `body`, failing when there is not exactly one.
///
/// The #1158 tests assert on one call's resolved binding, so a body that lowered to several calls would make a
/// positional "first call" assertion silently test the wrong statement.
fn single_call(body: &bir::Body) -> Result<&bir::StatementKind, Box<dyn std::error::Error>> {
    let calls: Vec<&bir::StatementKind> = body
        .block
        .stmts
        .iter()
        .map(|stmt| &stmt.kind)
        .filter(|kind| matches!(kind, bir::StatementKind::Call { .. }))
        .collect();
    match calls.as_slice() {
        [only] => Ok(only),
        other => Err(format!("expected exactly one call statement, found {}", other.len()).into()),
    }
}

/// Return the resolved argument binding carried by a call statement's callee.
fn call_binding(kind: &bir::StatementKind) -> Result<&bir::ArgumentBinding, Box<dyn std::error::Error>> {
    let bir::StatementKind::Call { callee, .. } = kind else {
        return Err("not a call statement".into());
    };
    match callee {
        bir::Callee::Function(bir::CallableTarget::Named(target)) => Ok(&target.binding),
        bir::Callee::Function(bir::CallableTarget::Local(target)) => Ok(&target.binding),
        bir::Callee::Method(target) => Ok(&target.binding),
        bir::Callee::Helper(_) => Err("a helper call carries no declared argument binding".into()),
    }
}

/// A resolved binding's two lists: the per-operand records, and the slots left to a default.
type ResolvedBindingParts<'a> = (&'a [bir::BoundArgument], &'a [usize]);

/// Return a call's resolved argument binding, failing when the call recorded no declared-slot binding.
///
/// Insisting on [`bir::ArgumentBinding::Resolved`] is the point: a test that accepted
/// `UnresolvedPositional` would silently pass against an implementation that stopped binding named arguments.
fn resolved_binding(kind: &bir::StatementKind) -> Result<ResolvedBindingParts<'_>, Box<dyn std::error::Error>> {
    match call_binding(kind)? {
        bir::ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        } => Ok((arguments, defaulted_slots)),
        bir::ArgumentBinding::UnresolvedPositional => {
            Err("expected a resolved declared-slot binding, found an unresolved positional call".into())
        }
    }
}

/// Return the named body from a lowered module.
fn body_named<'a>(module: &'a bir::BodyIrModule, name: &str) -> Result<&'a bir::Body, Box<dyn std::error::Error>> {
    module
        .bodies
        .iter()
        .find(|body| body.name == name)
        .ok_or_else(|| format!("body `{name}` missing from the lowered module").into())
}

#[test]
fn named_construction_lowers_to_a_constructor_aggregate_with_a_resolved_field_binding()
-> Result<(), Box<dyn std::error::Error>> {
    // The canonical README spelling. Before #1158 this was the *only* accepted construction spelling and it
    // lowered to `unsupported`, so no nominal value was representable in Body IR at all.
    let source = "model P:\n  x: int\n  y: int\n\ndef make() -> P:\n  return P(x=1, y=2)\n";
    let module = build(source, &["m", "ctor"])?;
    let snapshot = module.render_snapshot();
    assert_eq!(
        snapshot,
        build(source, &["m", "ctor"])?.render_snapshot(),
        "lowering must be deterministic"
    );

    assert!(
        !snapshot.contains("unsupported("),
        "named construction must lower to real Body IR: {snapshot}"
    );
    assert!(
        snapshot.contains("constructor(P)[const(1), const(2)]"),
        "construction must lower to a constructor aggregate in declared field order: {snapshot}"
    );
    Ok(())
}

#[test]
fn out_of_order_named_construction_binds_by_field_and_records_written_order() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "model P:\n  x: int\n  y: int\n\ndef make() -> P:\n  return P(y=2, x=1)\n";
    let module = build(source, &["m", "ctor_order"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "out-of-order named construction must lower: {snapshot}"
    );
    // Operands follow declared field order (`x` then `y`) even though the source wrote `y` first, and the
    // written order is recorded rather than discarded.
    assert!(
        snapshot.contains("constructor(P) written=[1, 0][const(1), const(2)]"),
        "field binding must reorder operands while preserving the written order fact: {snapshot}"
    );
    Ok(())
}

#[test]
fn construction_records_an_omitted_field_default_as_an_explicit_slot() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model P:\n  x: int\n  y: int = 5\n\ndef make() -> P:\n  return P(x=1)\n";
    let module = build(source, &["m", "ctor_default"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "construction omitting a defaulted field must lower: {snapshot}"
    );
    // The default's *computation* stays owned by the declaration; the call site records only that slot 1 took it.
    assert!(
        snapshot.contains("constructor(P) defaults=[1][const(1)]"),
        "an omitted field must be recorded as a defaulted slot, not left implicit: {snapshot}"
    );
    Ok(())
}

/// Retain the exact local model layout a direct executor needs instead of treating a constructor spelling as an
/// identity.
#[test]
fn source_local_model_construction_retains_its_declaration_identity_and_canonical_field_layout()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "model Pair:\n  left: int\n  right: int\n\ndef main() -> int:\n  pair = Pair(right=2, left=40)\n  return pair.left + pair.right\n";
    let module = build(source, &["m", "nominal_identity"])?;
    let declaration = match module.nominal_declarations.as_slice() {
        [declaration] => declaration,
        declarations => {
            return Err(format!("expected one retained local model declaration, found {declarations:?}").into());
        }
    };
    assert_eq!(declaration.name, "Pair");
    assert_eq!(declaration.fields, vec!["left", "right"]);
    assert_eq!(declaration.type_parameter_count, 0);

    let body = body_named(&module, "main")?;
    let target = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("the local model construction must lower as a constructor aggregate")?;
    assert_eq!(target.name, "Pair");
    assert_eq!(
        target.direct_declaration_id.as_ref(),
        Some(&declaration.direct_declaration_id)
    );
    let bir::ArgumentBinding::Resolved {
        arguments,
        defaulted_slots,
    } = &target.binding
    else {
        return Err("local model construction must retain its resolved field binding".into());
    };
    assert!(defaulted_slots.is_empty());
    assert_eq!(
        arguments
            .iter()
            .map(|argument| (argument.slot, argument.written_position))
            .collect::<Vec<_>>(),
        vec![(0, 1), (1, 0)],
        "constructor operands retain declaration slots while written positions retain source evaluation order"
    );
    Ok(())
}

/// Retain the exact local value-enum member selected by source lowering rather than recovering it from a
/// qualified spelling in a direct runtime.
#[test]
fn source_local_value_enum_member_retains_exact_enum_and_variant_identities() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "enum HttpStatus(int):\n  Ok = 200\n  NotFound = 404\n\ndef main() -> int:\n  return HttpStatus.NotFound.value()\n";
    let module = build(source, &["m", "value_enum_identity"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("value_enum HttpStatus id=decl:m::value_enum_identity#decl."),
        "the module must retain the source-local enum declaration identity: {snapshot}"
    );
    assert!(
        snapshot.contains("variant NotFound id=decl:m::value_enum_identity#decl."),
        "the module must retain the source-local member declaration identity: {snapshot}"
    );
    assert!(
        snapshot.contains("value_enum_variant(HttpStatus::NotFound"),
        "the member expression must lower to an identity-bearing rvalue instead of an external field place: {snapshot}"
    );
    Ok(())
}

/// Retain the exact local fieldless normal-enum member selected by source lowering rather than treating a
/// qualified spelling as a value any backend may recover.
#[test]
fn source_local_fieldless_enum_member_retains_exact_enum_and_variant_identities()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Signal:\n  Ready\n  Stop\n\ndef main() -> bool:\n  return Signal.Ready == Signal.Stop\n";
    let module = build(source, &["m", "fieldless_enum_identity"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("fieldless_enum Signal id=decl:m::fieldless_enum_identity#decl."),
        "the module must retain the source-local enum declaration identity: {snapshot}"
    );
    assert!(
        snapshot.contains("variant Ready id=decl:m::fieldless_enum_identity#decl."),
        "the module must retain the source-local member declaration identity: {snapshot}"
    );
    assert!(
        snapshot.contains("fieldless_enum_variant(Signal::Ready"),
        "the member expression must lower to an identity-bearing rvalue instead of an external field place: {snapshot}"
    );
    Ok(())
}

#[test]
fn mixed_positional_and_named_call_arguments_bind_to_declared_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(1, b=2)\n";
    let module = build(source, &["m", "mixed"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a mixed positional/named call must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:add(const(1), const(2))"),
        "a mixed call binding in declaration order needs no slot map: {snapshot}"
    );
    // The rendered string alone would also match an implementation that ignored named binding entirely and
    // lowered arguments in written order, so assert the resolved binding itself.
    let (arguments, defaulted_slots) = resolved_binding(single_call(body_named(&module, "use")?)?)?;
    assert!(defaulted_slots.is_empty(), "nothing was omitted: {defaulted_slots:?}");
    assert_eq!(
        arguments
            .iter()
            .map(|argument| (argument.slot, argument.written_position))
            .collect::<Vec<_>>(),
        vec![(0, 0), (1, 1)],
        "`b=2` must resolve to declared slot 1 rather than being taken positionally: {arguments:?}"
    );
    Ok(())
}

#[test]
fn out_of_order_named_call_arguments_evaluate_in_written_source_order() -> Result<(), Box<dyn std::error::Error>> {
    // The effect-ordering contract: `g()` is written first, so it must be *called* first, even though its value
    // binds to the later declared parameter. A consumer executing operands in slot order would swap the effects.
    let source = "def f() -> int:\n  return 1\n\ndef g() -> int:\n  return 2\n\ndef add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(b=g(), a=f())\n";
    let module = build(source, &["m", "written_order"])?;
    let snapshot = module.render_snapshot();
    let use_body = body_named(&module, "use")?;
    let rendered = use_body.render_snapshot();

    let g_at = rendered.find("call fn:g(").ok_or("missing call to g")?;
    let f_at = rendered.find("call fn:f(").ok_or("missing call to f")?;
    assert!(
        g_at < f_at,
        "argument sub-expressions must be evaluated in written source order: {rendered}"
    );
    assert!(
        rendered.contains("written=[1, 0]"),
        "the written order must be recorded on the call, not merely implied by statement order: {rendered}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "no part of this program should refuse: {snapshot}"
    );
    Ok(())
}

#[test]
fn an_omitted_defaulted_argument_is_recorded_as_a_defaulted_slot() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(a: int, b: int = 2) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(1)\n";
    let module = build(source, &["m", "call_default"])?;
    let use_body = body_named(&module, "use")?;
    let (arguments, defaulted_slots) = resolved_binding(single_call(use_body)?)?;

    assert_eq!(
        defaulted_slots,
        [1],
        "an omitted default must be an explicit call-site fact: {defaulted_slots:?}"
    );
    assert_eq!(
        arguments.len(),
        1,
        "only the supplied argument gets an operand: {arguments:?}"
    );
    Ok(())
}

#[test]
fn an_omitted_interior_default_binds_without_compacting_later_arguments() -> Result<(), Box<dyn std::error::Error>> {
    // #1124 had to refuse this: a flat operand vector could not say that `9` fills slot 2 rather than slot 1.
    // The recorded binding is exactly that sparse argument map, so the call is now representable.
    let source = "def at(a: int, b: int = 2, c: int = 3) -> int:\n  return a + b + c\n\ndef use() -> int:\n  return at(1, c=9)\n";
    let module = build(source, &["m", "interior_default"])?;
    let snapshot = module.render_snapshot();
    let use_body = body_named(&module, "use")?;
    let (arguments, defaulted_slots) = resolved_binding(single_call(use_body)?)?;

    assert!(
        !snapshot.contains("unsupported("),
        "an interior default hole must now lower: {snapshot}"
    );
    assert_eq!(
        defaulted_slots,
        [1],
        "slot 1 takes its declared default: {defaulted_slots:?}"
    );
    assert_eq!(
        arguments.iter().map(|argument| argument.slot).collect::<Vec<_>>(),
        vec![0, 2],
        "the supplied operands must keep their real declaration slots: {arguments:?}"
    );
    Ok(())
}

#[test]
fn method_call_named_arguments_bind_after_the_borrowed_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class C:\n  def add(self, a: int, b: int) -> int:\n    return a + b\n\ndef use(c: C) -> int:\n  return c.add(b=2, a=1)\n";
    let module = build(source, &["m", "method_named"])?;
    let snapshot = module.render_snapshot();
    let use_body = body_named(&module, "use")?;
    let (arguments, _) = resolved_binding(single_call(use_body)?)?;

    assert!(
        !snapshot.contains("unsupported("),
        "a named method call must lower: {snapshot}"
    );
    // The receiver stays `args[0]` and is deliberately outside the binding, whose slots index the method's own
    // declared parameters.
    assert_eq!(
        arguments.iter().map(|argument| argument.slot).collect::<Vec<_>>(),
        vec![0, 1],
        "method argument slots must index declared parameters, not the receiver: {arguments:?}"
    );
    assert_eq!(
        arguments
            .iter()
            .map(|argument| argument.written_position)
            .collect::<Vec<_>>(),
        vec![1, 0],
        "the written order of `b=2, a=1` must survive the reorder into declaration order: {arguments:?}"
    );
    assert!(
        use_body.render_snapshot().contains("borrow(_0)"),
        "the receiver must still lower as a borrowed first argument: {snapshot}"
    );
    Ok(())
}

#[test]
fn explicit_call_site_type_arguments_survive_lowering() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def pick[T](v: T) -> T:\n  return v\n\ndef use() -> int:\n  return pick[int](1)\n";
    let module = build(source, &["m", "generic_call"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "an explicitly generic call must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:pick[int](const(1))"),
        "resolved call-site type arguments belong to the callee's identity: {snapshot}"
    );
    Ok(())
}

#[test]
fn explicit_method_call_type_arguments_survive_lowering() -> Result<(), Box<dyn std::error::Error>> {
    // The other half of `CallSiteGenerics`' canonical surface: `session.read_csv[Order](path)`. The typechecker
    // substitutes the receiver's generics before recording the signature, so the method's slots are already
    // concrete here and the resolved type argument still has to reach the callee.
    let source =
        "class S:\n  def read[T](self, v: T) -> T:\n    return v\n\ndef use(s: S) -> int:\n  return s.read[int](1)\n";
    let module = build(source, &["m", "generic_method"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "an explicitly generic method call must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call method:read[int]("),
        "a method call's resolved type arguments belong to its callee identity: {snapshot}"
    );
    Ok(())
}

#[test]
fn direct_and_local_named_binding_go_through_one_mechanism() -> Result<(), Box<dyn std::error::Error>> {
    // #1158's "one mechanism" criterion: the direct `Callee::Function` path and the #1124 local-callable path
    // must produce the same binding facts for the same spelling, not merely both succeed.
    // Deliberately an out-of-order spelling: its binding is *not* the identity, so this cannot be satisfied by
    // two independent mechanisms that merely agree on the trivial case, nor by a path that never bound at all.
    let direct = "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(b=2, a=1)\n";
    let local =
        "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  g = add\n  return g(b=2, a=1)\n";

    let direct_module = build(direct, &["m", "one_direct"])?;
    let local_module = build(local, &["m", "one_local"])?;
    let direct_binding = call_binding(single_call(body_named(&direct_module, "use")?)?)?;
    let local_binding = call_binding(single_call(body_named(&local_module, "use")?)?)?;

    assert_eq!(
        direct_binding, local_binding,
        "a direct call and a local-callable call must resolve one spelling identically"
    );
    let bir::ArgumentBinding::Resolved { arguments, .. } = direct_binding else {
        return Err("the shared mechanism must produce a resolved binding, not a positional fallback".into());
    };
    assert_eq!(
        arguments
            .iter()
            .map(|argument| (argument.slot, argument.written_position))
            .collect::<Vec<_>>(),
        vec![(0, 1), (1, 0)],
        "the shared binding must be the non-trivial one this spelling implies: {arguments:?}"
    );
    Ok(())
}

#[test]
fn an_overloaded_call_binds_against_the_declaration_the_typechecker_selected() -> Result<(), Box<dyn std::error::Error>>
{
    // Regression: `function_bindings` is keyed by bare name, so it holds only one of two same-name
    // declarations. Binding against the wrong overload's parameter *names* silently swaps the arguments --
    // a wrong answer where the previous refusal was at least honest.
    let source = "def pick(a: int, b: int) -> int:\n  return a - b\n\ndef pick(b: str, a: str) -> str:\n  return a\n\ndef use() -> int:\n  return pick(a=10, b=1)\n";
    let module = build(source, &["m", "overload"])?;
    let use_body = body_named(&module, "use")?;
    let rendered = use_body.render_snapshot();
    let (arguments, _) = resolved_binding(single_call(use_body)?)?;

    // The selected overload is `pick(a: int, b: int)`, so `a=10` fills slot 0 and `b=1` fills slot 1. Binding
    // against the *second* declaration would map `a` to slot 1 and emit the operands as `const(1), const(10)`.
    assert_eq!(
        arguments
            .iter()
            .map(|argument| (argument.slot, argument.written_position))
            .collect::<Vec<_>>(),
        vec![(0, 0), (1, 1)],
        "the call must bind against the overload the typechecker selected: {arguments:?}"
    );
    assert!(
        rendered.contains("call fn:pick(const(10), const(1))"),
        "operands must follow the selected overload's declaration order: {rendered}"
    );
    Ok(())
}

#[test]
fn an_overloaded_call_retains_the_typechecker_selected_same_module_declaration_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "def pick(a: int, b: int) -> int:\n  return a - b\n\ndef pick(b: str, a: str) -> str:\n  return a\n\ndef use() -> int:\n  return pick(a=10, b=1)\n";
    let module = build(source, &["m", "overload_identity"])?;
    let use_body = body_named(&module, "use")?;
    let bir::StatementKind::Call {
        callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
        ..
    } = single_call(use_body)?
    else {
        return Err("expected an identity-selected named function call".into());
    };
    let target_id = target
        .direct_call_id
        .as_ref()
        .ok_or("same-module overloaded call must retain a direct declaration identity")?;
    let selected = module
        .bodies
        .iter()
        .find(|body| body.direct_call_id == *target_id)
        .ok_or("direct call identity must select a Body-IR declaration")?;

    assert_eq!(selected.name, "pick");
    assert!(
        selected.render_snapshot().contains("local 0 a : int [param]"),
        "the direct identity must select the integer overload: {}",
        selected.render_snapshot()
    );
    Ok(())
}

#[test]
fn an_overload_set_that_changes_arity_does_not_refuse_a_valid_call() -> Result<(), Box<dyn std::error::Error>> {
    // The other half of the same defect: with the two-parameter declaration written first, a name-keyed lookup
    // could resolve `pick(1, 2)` against the one-parameter overload and refuse a call the typechecker accepted.
    let source = "def pick(a: int, b: int) -> int:\n  return a + b\n\ndef pick(a: str) -> str:\n  return a\n\ndef use() -> int:\n  return pick(1, 2)\n";
    let module = build(source, &["m", "overload_arity"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a call the typechecker accepted must not be refused by overload confusion: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_rest_parameter_callee_still_lowers_its_positional_arguments() -> Result<(), Box<dyn std::error::Error>> {
    // Variadics are a delivered language capability. Routing the direct path through the shared planner must not
    // silently narrow what Body IR represents; the call keeps lowering, it simply makes no declared-slot claim.
    let source = "def total(a: int, *xs: int) -> int:\n  return a\n\ndef use() -> int:\n  return total(1, 2, 3)\n";
    let module = build(source, &["m", "rest"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a positional call into a rest-parameter signature must still lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:total unbound(const(1), const(2), const(3))"),
        "a rest signature has no one-to-one declared slots, so the binding must say so: {snapshot}"
    );
    Ok(())
}

#[test]
fn argument_ownership_facts_are_sequenced_by_written_order_not_operand_index() -> Result<(), Box<dyn std::error::Error>>
{
    // The invariant `ArgumentBinding` documents: operands are reordered into declaration order, but their
    // ownership facts were decided in written order. Read left to right this vector moves `_0` and then clones
    // it; `written=[1, 0]` is what tells a consumer the clone happened first.
    let source = "def two(p: str, q: str) -> str:\n  return p + q\n\ndef use(a: str) -> str:\n  return two(q=a, p=a)\n";
    let module = build(source, &["m", "own_order"])?;
    let rendered = body_named(&module, "use")?.render_snapshot();

    assert!(
        rendered.contains("call fn:two written=[1, 0](move(_0, last_use), clone(_0))"),
        "ownership facts must stay sequenced by written order: {rendered}"
    );
    Ok(())
}

#[test]
fn class_construction_binds_inherited_fields_in_declared_layout_order() -> Result<(), Box<dyn std::error::Error>> {
    // Constructor ABI order puts the parent's fields first. A subclass construction must bind against that
    // flattened order, not against the subclass's own declarations alone.
    let source = "class Base:\n  a: int\n\nclass Sub extends Base:\n  b: int = 7\n\ndef make() -> Sub:\n  return Sub(b=1, a=2)\n";
    let module = build(source, &["m", "subclass"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "subclass construction must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("constructor(Sub) written=[1, 0][const(2), const(1)]"),
        "inherited fields come first in constructor layout order: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_construction_the_checker_declined_to_bind_is_refused_as_a_construction() -> Result<(), Box<dyn std::error::Error>>
{
    // A duplicate field leaves no recorded binding. Falling through to the direct-call path would refuse this as
    // a call to an unknown function, naming the wrong construct entirely.
    let source = "model P:\n  x: int = 1\n  y: int = 2\n\ndef make() -> P:\n  return P(x=1, x=2)\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "dup_field"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the typechecker must reject a duplicated field first"
    );
    assert!(
        snapshot.contains("construction of `P` with an unresolved field layout"),
        "a refused construction must be named as a construction: {snapshot}"
    );
    Ok(())
}

#[test]
fn an_argument_spread_is_refused_by_name_rather_than_as_a_generic_call_failure()
-> Result<(), Box<dyn std::error::Error>> {
    // The typechecker rejects these first; lowering must stay fail-closed and name the specific spelling, since
    // #1159 owns spread representation while #1158 owns named binding.
    let source =
        "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use(xs: List[int]) -> int:\n  return add(*xs)\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the source checker must reject an unmatched positional spread first"
    );
    assert!(
        snapshot.contains("positional argument spread"),
        "a spread must be refused as a spread, not as a named-argument failure: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_named_argument_with_no_matching_parameter_is_refused_by_name() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(a=1, zz=2)\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "bad_named"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the typechecker must reject an unknown parameter name first"
    );
    assert!(
        snapshot.contains("has no parameter `zz`"),
        "lowering must name the unresolvable parameter rather than accepting it silently: {snapshot}"
    );
    Ok(())
}

// ========================================================================
// #1164 -- `await` and `race for`
// ========================================================================

const ASYNC_PRELUDE: &str =
    "import std.async\n\nasync def fast() -> int:\n  return 1\n\nasync def slow() -> int:\n  return 2\n\n";

#[test]
fn lowers_await_as_an_explicit_suspension_point_with_a_destination() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!("{ASYNC_PRELUDE}async def f() -> int:\n  v = await fast()\n  return v\n");
    let module = build(&source, &["m", "await_one"])?;
    let snapshot = module.render_snapshot();
    assert_eq!(
        snapshot,
        build(&source, &["m", "await_one"])?.render_snapshot(),
        "lowering must be deterministic"
    );

    assert!(!snapshot.contains("unsupported("), "await must lower: {snapshot}");
    // The suspension carries a destination and the awaited operand's own ownership fact -- the two facts that
    // distinguish it from a generator `yield`, which produces outward and has no destination.
    assert!(
        snapshot.contains("_1 = await copy(_0, last_use)"),
        "await must record its destination and the awaited read's ownership fact: {snapshot}"
    );
    assert!(
        !snapshot.contains("yield"),
        "await must not be represented as a generator yield: {snapshot}"
    );
    Ok(())
}

#[test]
fn records_the_async_runtime_requirement_on_the_awaiting_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!("{ASYNC_PRELUDE}async def f() -> int:\n  return await fast()\n");
    let module = build(&source, &["m", "await_req"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        rendered.contains("async_runtime"),
        "the requirement must be recorded on the awaiting body itself, not merely somewhere in the module: {rendered}"
    );
    Ok(())
}

#[test]
fn an_async_body_without_any_await_is_still_marked_async() -> Result<(), Box<dyn std::error::Error>> {
    // The reason `is_async` is a stored declaration fact rather than derived the way `is_generator` is: this
    // body contains no `await` at all, yet its caller still gets an awaitable. Deriving async-ness by scanning
    // for a suspension point would report this body as synchronous.
    let source = "import std.async\n\nasync def f() -> int:\n  return 1\n";
    let module = build(source, &["m", "async_plain"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("body async f"),
        "an `async def` with no await must still be marked async: {snapshot}"
    );
    assert!(
        !snapshot.contains("await "),
        "this body genuinely contains no suspension point, so the async fact cannot have been derived from one: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_synchronous_body_is_not_marked_async() -> Result<(), Box<dyn std::error::Error>> {
    let module = build("def f() -> int:\n  return 1\n", &["m", "sync"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("body f "),
        "a plain function must render unmarked: {snapshot}"
    );
    assert!(
        !snapshot.contains("body async"),
        "a synchronous body must not be marked async: {snapshot}"
    );
    Ok(())
}

#[test]
fn sequential_awaits_keep_their_effect_ordering_across_suspension() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        format!("{ASYNC_PRELUDE}async def f() -> int:\n  x = await fast()\n  y = await slow()\n  return x + y\n");
    let module = build(&source, &["m", "await_seq"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(!rendered.contains("unsupported("), "both awaits must lower: {rendered}");
    let first = rendered.find("call fn:fast(").ok_or("missing first awaitable")?;
    let first_await = rendered.find("await ").ok_or("missing first suspension")?;
    let second = rendered.find("call fn:slow(").ok_or("missing second awaitable")?;
    assert!(
        first < first_await && first_await < second,
        "statements before a suspension must precede it and statements after must follow it: {rendered}"
    );
    assert_eq!(
        rendered.matches("await ").count(),
        2,
        "each source `await` must produce its own suspension point: {rendered}"
    );
    Ok(())
}

#[test]
fn await_inside_a_branch_stays_inside_that_branch() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{ASYNC_PRELUDE}async def f(flag: bool) -> int:\n  total = 0\n  if flag:\n    total = await fast()\n  else:\n    total = 7\n  return total\n"
    );
    let module = build(&source, &["m", "await_branch"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !rendered.contains("unsupported("),
        "await in a branch must lower: {rendered}"
    );
    let branch_line = rendered
        .lines()
        .find(|line| line.contains("await "))
        .ok_or("missing suspension")?;
    assert!(
        branch_line.starts_with("    "),
        "the suspension must stay nested inside the branch block: {rendered}"
    );
    Ok(())
}

#[test]
fn await_inside_a_loop_stays_inside_the_loop_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  total = 0\n  i = 0\n  while i < 3:\n    total = total + await fast()\n    i = i + 1\n  return total\n"
    );
    let module = build(&source, &["m", "await_loop"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !rendered.contains("unsupported("),
        "await in a loop must lower: {rendered}"
    );
    let await_line = rendered
        .lines()
        .find(|line| line.contains("await "))
        .ok_or("missing suspension")?;
    assert!(
        await_line.starts_with("    "),
        "the suspension must stay inside the loop body: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_a_two_arm_race_with_per_arm_bindings_and_pre_selection_awaitables() -> Result<(), Box<dyn std::error::Error>>
{
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  race for value:\n    await fast() => value\n    await slow() => value\n"
    );
    let module = build(&source, &["m", "race_two"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !rendered.contains("unsupported("),
        "a two-arm race must lower: {rendered}"
    );
    // Every awaitable is evaluated before selection, in source order -- observable here as both calls being
    // emitted ahead of the race statement rather than inside an arm.
    let fast_at = rendered.find("call fn:fast(").ok_or("missing first awaitable")?;
    let slow_at = rendered.find("call fn:slow(").ok_or("missing second awaitable")?;
    let race_at = rendered.find("race:").ok_or("missing race statement")?;
    assert!(
        fast_at < slow_at && slow_at < race_at,
        "all arm awaitables must be evaluated, in source order, before selection: {rendered}"
    );
    // The source spells one binding name, but each arm re-scopes it, so each arm owns its own local.
    assert_eq!(
        rendered.matches("value : int [binding]").count(),
        2,
        "each arm must bind its own local rather than sharing one: {rendered}"
    );
    Ok(())
}

#[test]
fn a_race_arm_binding_does_not_escape_its_arm() -> Result<(), Box<dyn std::error::Error>> {
    // The arm binding shadows an enclosing name only for the duration of its own arm. Restoring it is the same
    // discipline `lower_closure` follows, and getting it wrong is silent: reads after the race would resolve to
    // the last arm's local, so the body would compute the wrong value with no unsupported node to show for it.
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  value = 100\n  winner = race for value:\n    await fast() => value\n    await slow() => value\n  return value + winner\n"
    );
    let module = build(&source, &["m", "race_shadow"])?;
    let body = body_named(&module, "f")?;
    let rendered = body.render_snapshot();

    // `value` is declared first, so it is local 0; the trailing `value + winner` must read exactly that local.
    let outer = local_for_binding(&rendered, "value").ok_or("missing outer binding")?;
    assert_eq!(
        outer, "_0",
        "the outer binding should be the first declared local: {rendered}"
    );
    let sum_line = rendered
        .lines()
        .find(|line| line.contains(" + "))
        .ok_or("missing the trailing sum")?;
    assert!(
        sum_line.contains("copy(_0)"),
        "a read after the race must resolve to the enclosing binding, not an arm local: {rendered}"
    );
    Ok(())
}

#[test]
fn a_block_arm_local_does_not_leak_past_its_arm() -> Result<(), Box<dyn std::error::Error>> {
    // A block arm lowers ordinary statements, so `let total = ...` inside it declares a lexical local through
    // the same path any assignment uses. Restoring only the shared race binding would leave that arm-local
    // installed, and the trailing read of `total` would silently resolve to it instead of the outer binding --
    // a wrong value with no unsupported node to show for it.
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  let total = 100\n  winner = race for value:\n    await fast() => value\n    await slow() =>\n      let total = value * 2\n      total\n  return total + winner\n"
    );
    let module = build(&source, &["m", "race_arm_local"])?;
    let body = body_named(&module, "f")?;
    let rendered = body.render_snapshot();

    // The outer binding is distinct from the arm's `total`, and the post-race expression must use that outer
    // local regardless of earlier parameters or temporaries that might affect local numbering.
    let outer = local_for_binding(&rendered, "total").ok_or("missing outer binding")?;
    assert!(
        rendered.matches("total : int [binding]").count() >= 2,
        "the arm must declare its own `total` rather than reusing the outer one: {rendered}"
    );
    let sum_line = rendered
        .lines()
        .find(|line| line.contains(" + ") && !line.starts_with("      "))
        .ok_or("missing the trailing sum")?;
    assert!(
        sum_line.contains(&format!("copy({outer})")),
        "a read after the race must resolve to the enclosing local, not one an arm body declared: {rendered}"
    );
    Ok(())
}

#[test]
fn a_race_arm_block_body_lowers_its_statements_and_trailing_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  race for value:\n    await fast() => value\n    await slow() =>\n      doubled = value * 2\n      doubled\n"
    );
    let module = build(&source, &["m", "race_block"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !rendered.contains("unsupported("),
        "a block arm body must lower: {rendered}"
    );
    // The arm body's statements live inside the arm, indented under it -- only the winning arm runs, so they
    // must not be hoisted into the enclosing block alongside the awaitables.
    let arm_stmt = rendered
        .lines()
        .find(|line| line.contains("* const(2)"))
        .ok_or("missing the arm body computation")?;
    assert!(
        arm_stmt.starts_with("      "),
        "an arm body statement must stay nested inside its arm: {rendered}"
    );
    // The block's trailing expression becomes the arm's result, not merely a statement inside it.
    assert!(
        rendered.contains("-> copy(_5)"),
        "the block's trailing expression must become the arm's result operand: {rendered}"
    );
    Ok(())
}

#[test]
fn an_unsupported_construct_in_a_race_arm_does_not_collapse_the_whole_race() -> Result<(), Box<dyn std::error::Error>> {
    // The issue's explicit requirement: a construct Body IR cannot represent keeps its own node *inside* a
    // represented race, so a consumer loses only that construct rather than the entire expression.
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> int:\n  race for value:\n    await fast() => value\n    await slow() => value ** 2\n"
    );
    let module = build(&source, &["m", "race_partial"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        rendered.contains("race:"),
        "the race itself must still be represented: {rendered}"
    );
    // Asserting the node exists somewhere in the body would also pass if it had been hoisted into the enclosing
    // block -- the exact regression this test exists to catch -- so require it to be indented inside an arm.
    let refusal = rendered
        .lines()
        .find(|line| line.contains("unsupported("))
        .ok_or("missing the refusal for the unrepresentable arm construct")?;
    assert!(
        refusal.starts_with("      "),
        "the refusal must stay inside its arm rather than collapsing or escaping the race: {rendered}"
    );
    Ok(())
}

#[test]
fn an_async_method_body_is_marked_async() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import std.async\n\nclass C:\n  async def m(self) -> int:\n    return 1\n";
    let module = build(source, &["m", "async_method"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("body async m"),
        "an async method body must carry the same async fact as an async function: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_prefix_surface_keyword_that_is_not_await_is_refused_rather_than_treated_as_a_suspension() {
    // `SurfaceExprPayload::PrefixUnary` is generic over any prefix soft keyword; `await` is merely the only one
    // registered today. Dispatching on the payload alone would silently lower a future prefix keyword as a
    // suspension point, so lowering matches the surface *key*. This pins that.
    let type_info = TypeCheckInfo::default();
    let function_default_sources = FunctionDefaultSources::new();
    let local_function_declarations = LocalFunctionDeclarations::new();
    let local_nominal_declarations = LocalNominalDeclarations::new();
    let local_fieldless_enum_declarations = LocalFieldlessEnumDeclarations::new();
    let local_value_enum_declarations = LocalValueEnumDeclarations::new();
    let lowering_facts = BodyIrLoweringFacts {
        type_info: &type_info,
        function_default_sources: &function_default_sources,
        local_function_declarations: &local_function_declarations,
        local_nominal_declarations: &local_nominal_declarations,
        local_fieldless_enum_declarations: &local_fieldless_enum_declarations,
        local_value_enum_declarations: &local_value_enum_declarations,
        module_identity: "m",
    };
    let mut builder = BodyBuilder::new(&lowering_facts, IncanType::Unknown);
    let scope = builder.new_scope(None, HirSourceSpan::new(0, 1));
    let mut out = Vec::new();
    let surface = ast::SurfaceExpr {
        key: SurfaceFeatureKey::SoftKeyword(KeywordId::Async),
        payload: ast::SurfaceExprPayload::PrefixUnary(Box::new(ast::Spanned::new(
            ast::Expr::Ident("placeholder".to_string()),
            ast::Span::new(0, 1),
        ))),
    };

    let _ = builder.lower_surface_expr(&surface, ast::Span::new(0, 1), scope, &mut out);

    assert!(
        out.iter().any(|stmt| matches!(
            &stmt.kind,
            bir::StatementKind::Unsupported { description } if description.contains("prefix-keyword")
        )),
        "a non-`await` prefix keyword must keep the generic surface refusal, not become an await: {out:?}"
    );
    assert!(
        !out.iter()
            .any(|stmt| matches!(&stmt.kind, bir::StatementKind::Await { .. })),
        "no suspension point may be emitted for a keyword that is not `await`: {out:?}"
    );
}

// ========================================================================
// #1159 -- spread arguments and spread aggregate elements
// ========================================================================

#[test]
fn a_leading_spread_splices_before_its_fixed_elements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(xs: list[int]) -> None:\n  out = [*xs, 1]\n  return\n";
    let module = build(source, &["m", "spread_trailing"])?;
    let snapshot = module.render_snapshot();
    assert_eq!(
        snapshot,
        build(source, &["m", "spread_trailing"])?.render_snapshot(),
        "lowering must be deterministic"
    );

    assert!(
        !snapshot.contains("unsupported("),
        "a list spread must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("list[*move(_0, last_use), const(1)]"),
        "the spread must keep its written position and carry its own ownership fact: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_trailing_spread_splices_after_its_fixed_elements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(xs: list[int]) -> None:\n  out = [1, *xs]\n  return\n";
    let module = build(source, &["m", "spread_after"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a trailing spread must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("list[const(1), *move(_0, last_use)]"),
        "a spread written last must stay last: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_statically_shaped_spread_binds_as_an_ordinary_fixed_arity_call() -> Result<(), Box<dyn std::error::Error>> {
    // `add(*(1, 2))` really is `add(1, 2)`: the typechecker proves the arity before lowering, so this belongs
    // on the declaration-slot path, not on the runtime-arity path a genuine spread needs. Its operands must
    // land in declared slots with no spread element and no `unbound` marker.
    for (label, source) in [
        (
            "tuple",
            "def add(a: int, b: int) -> int:\n  return a + b\n\ndef m() -> int:\n  return add(*(1, 2))\n",
        ),
        (
            "list",
            "def add(a: int, b: int) -> int:\n  return a + b\n\ndef m() -> int:\n  return add(*[1, 2])\n",
        ),
        (
            "dict",
            "def add(a: int, b: int) -> int:\n  return a + b\n\ndef m() -> int:\n  return add(**{\"a\": 1, \"b\": 2})\n",
        ),
    ] {
        let module = build(source, &["m", "shaped"])?;
        let rendered = body_named(&module, "m")?.render_snapshot();

        assert!(
            !rendered.contains("unsupported("),
            "{label} spread must lower: {rendered}"
        );
        assert!(
            rendered.contains("call fn:add(const(1), const(2))"),
            "a {label} spread with a proven shape must bind to declared slots: {rendered}"
        );
        assert!(
            !rendered.contains("unbound") && !rendered.contains("*const"),
            "a proven-shape spread must not be represented as runtime-arity: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn a_spread_with_no_proven_shape_stays_on_the_runtime_arity_path() -> Result<(), Box<dyn std::error::Error>> {
    // The contrast case for the test above: a list *variable* has no statically visible arity, so it must keep
    // its spread element rather than being expanded into slots that cannot be counted.
    let source = "def log(*items: int) -> None:\n  return\n\ndef m(xs: list[int]) -> None:\n  log(*xs)\n  return\n";
    let module = build(source, &["m", "unshaped"])?;
    let rendered = body_named(&module, "m")?.render_snapshot();

    assert!(
        rendered.contains("call fn:log unbound(*move(_0, last_use))"),
        "an unproven spread must stay a spread element on the unresolved-arity path: {rendered}"
    );
    Ok(())
}

#[test]
fn a_standalone_keyword_spread_call_lowers() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def log(**fields: int) -> None:\n  return\n\ndef m(kw: dict[str, int]) -> None:\n  log(**kw)\n  return\n";
    let module = build(source, &["m", "kw_spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a keyword spread call must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:log unbound(**move(_0, last_use))"),
        "a keyword spread must render with its own marker and ownership fact: {snapshot}"
    );
    Ok(())
}

#[test]
fn fixed_elements_keep_their_positions_on_both_sides_of_a_spread() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(xs: list[int]) -> None:\n  out = [1, *xs, 2]\n  return\n";
    let module = build(source, &["m", "spread_middle"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("list[const(1), *move(_0, last_use), const(2)]"),
        "surrounding fixed elements must keep their positions relative to the spread: {snapshot}"
    );
    Ok(())
}

#[test]
fn multiple_spreads_each_keep_their_own_element() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(xs: list[int], ys: list[int]) -> None:\n  out = [*xs, *ys]\n  return\n";
    let module = build(source, &["m", "spread_multi"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "multiple spreads must lower: {snapshot}"
    );
    // Counting `list[*` would pass against an implementation that silently dropped the second spread, since it
    // only observes that the aggregate *begins* with one. Assert the whole rendering so a dropped, reordered,
    // or differently-owned second spread all fail.
    assert!(
        snapshot.contains("list[*move(_0, last_use), *move(_1, last_use)]"),
        "both spreads must survive, in written order, each with its own ownership fact: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_dict_spread_keeps_its_written_position_before_an_overriding_key() -> Result<(), Box<dyn std::error::Error>> {
    // The override rule is what makes this meaningful: entries take effect in order and a later entry wins,
    // so the spread must stay *before* the literal key rather than being reordered or merged.
    let source = "def m(d: dict[str, int]) -> None:\n  out = {**d, \"a\": 1}\n  return\n";
    let module = build(source, &["m", "dict_spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a dict spread must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("dict[**move(_0, last_use), const(\"a\"): const(1)]"),
        "the spread must precede the overriding key and stay a distinct entry: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_dict_spread_after_a_literal_key_keeps_that_order() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(d: dict[str, int]) -> None:\n  out = {\"a\": 1, **d}\n  return\n";
    let module = build(source, &["m", "dict_spread_after"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("dict[const(\"a\"): const(1), **move(_0, last_use)]"),
        "written entry order decides precedence, so it must survive lowering: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_positional_call_spread_lowers_without_a_declared_slot_claim() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def log(*items: int) -> None:\n  return\n\ndef m(xs: list[int]) -> None:\n  log(*xs)\n  return\n";
    let module = build(source, &["m", "call_spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a call spread must lower: {snapshot}"
    );
    // A spread makes the arity a runtime fact, so the call must record no declared-slot binding rather than
    // asserting an identity slot map nobody checked.
    assert!(
        snapshot.contains("call fn:log unbound(*move(_0, last_use))"),
        "a spread call must be unbound and carry the spliced source's ownership fact: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_mixed_call_keeps_every_written_argument_form() -> Result<(), Box<dyn std::error::Error>> {
    // The issue's combined form. A named argument here has no declared slot to bind to, because the spread
    // makes the arity a runtime fact -- but discarding its name would lose source information.
    let source = "def log(a: int, b: int, *items: int, **fields: int) -> None:\n  return\n\ndef m(xs: list[int], kw: dict[str, int]) -> None:\n  log(1, *xs, b=2, **kw)\n  return\n";
    let module = build(source, &["m", "call_mixed"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "the combined call form must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:log unbound(const(1), *move(_0, last_use), b=const(2), **move(_1, last_use))"),
        "positional, spread, named, and keyword-spread arguments must each keep their written form and order: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_method_call_spread_lowers_after_the_borrowed_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class C:\n  def take(self, *items: int) -> None:\n    return\n\ndef m(c: C, xs: list[int]) -> None:\n  c.take(*xs)\n  return\n";
    let module = build(source, &["m", "method_spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a method call spread must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call method:take unbound(borrow(_0), *move(_1, last_use))"),
        "the receiver stays args[0] and is never spliced: {snapshot}"
    );
    Ok(())
}

#[test]
fn set_literals_have_no_spread_spelling_to_represent() -> Result<(), Box<dyn std::error::Error>> {
    // Documenting a finding rather than adding surface: the source language rejects set spread in every
    // position, and `ast::Expr::Set` has no entry enum that could carry one. RFC 038 excludes it deliberately.
    for source in [
        "def m(xs: list[int]) -> None:\n  out = {*xs}\n  return\n",
        "def m(xs: list[int]) -> None:\n  out = {1, *xs}\n  return\n",
    ] {
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let errors = parser::parse(&tokens)
            .err()
            .ok_or("the parser must reject set spread rather than Body IR having to refuse it")?;
        // `is_err()` alone would pass for any unrelated parse failure, including one this fixture introduced.
        assert!(
            errors
                .iter()
                .any(|error| error.message.to_lowercase().contains("spread")),
            "the rejection must name spread rather than being any parse failure: {errors:?}"
        );
    }
    Ok(())
}

// ========================================================================
// RFC 028 -- user-defined operator dispatch
// ========================================================================

const VEC2_SRC: &str = "@derive(Debug)\nmodel Vec2:\n  x: int\n  y: int\n\n  def __add__(self, other: Vec2) -> Vec2:\n    return Vec2(x=self.x + other.x, y=self.y + other.y)\n\n";

#[test]
fn a_user_defined_operator_lowers_to_the_method_the_typechecker_resolved() -> Result<(), Box<dyn std::error::Error>> {
    // Representing this as `BinOp::Add` would claim a primitive machine operation where the source calls a
    // method -- a wrong representation rather than an honest refusal, with no marker for a consumer to notice.
    let source = format!("{VEC2_SRC}def f(a: Vec2, b: Vec2) -> Vec2:\n  return a + b\n");
    let module = build(&source, &["m", "user_op"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        rendered.contains("call method:__add__ unbound(borrow(_0),"),
        "a user-defined operator must dispatch to its resolved method, with the left operand borrowed as the \
         receiver: {rendered}"
    );
    assert!(
        !rendered.contains("copy(_0) + copy(_1)") && !rendered.contains("move(_0, last_use) + "),
        "it must not also lower as a primitive operation: {rendered}"
    );
    Ok(())
}

#[test]
fn primitive_operators_are_unaffected_by_operator_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    // The typechecker records no dispatch for primitives, so these must keep their existing representations.
    let ints = build("def f(a: int, b: int) -> int:\n  return a + b\n", &["m", "prim_int"])?;
    assert!(
        body_named(&ints, "f")?
            .render_snapshot()
            .contains("copy(_0) + copy(_1)"),
        "integer addition must stay a primitive binary operation"
    );

    let strings = build("def f(a: str, b: str) -> str:\n  return a + b\n", &["m", "prim_str"])?;
    assert!(
        body_named(&strings, "f")?
            .render_snapshot()
            .contains("call helper:str_concat("),
        "string concatenation must stay a compiler-owned helper call"
    );
    Ok(())
}

#[cfg(test)]
mod tuple_destructure_interop_tests {
    use super::{IncanType, unsupported_tuple_destructure};

    /// Lowering must apply the same accepted-shape rule as the typechecker to interop values (#1132).
    ///
    /// A blanket `RustInteropPath` exemption here would leave the original defect reachable through interop: an
    /// opaque Rust value would lower to a `.0`/`.1` projection and fail as raw `rustc` output.
    #[test]
    fn opaque_rust_interop_values_refuse_to_lower_a_tuple_destructure() {
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("String".to_string()), 2).is_some(),
            "an opaque Rust value must not lower to a tuple field projection"
        );
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("std::vec::Vec<u8>".to_string()), 2).is_some(),
            "a Rust generic that is not a tuple must not lower to a tuple field projection"
        );
        // `(String)` is a parenthesised `String`, not a one-element tuple, so a single-name destructure must not
        // lower to `.0` against it.
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("(String)".to_string()), 1).is_some(),
            "a parenthesised Rust type has no `.0` field and must refuse to lower"
        );
        // The genuine one-element spelling still lowers.
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("(String,)".to_string()), 1).is_none(),
            "`(String,)` is a real one-element tuple and must keep lowering"
        );
    }

    /// The readable tuple spelling the stdlib relies on must still lower, so the refusal stays narrow.
    #[test]
    fn readable_rust_tuple_values_still_lower_a_tuple_destructure() {
        assert!(
            unsupported_tuple_destructure(
                &IncanType::RustInteropPath("(String,incan_stdlib::json::JsonValue)".to_string()),
                2
            )
            .is_none(),
            "`std.json` destructures a `rust::HashMap` item and must keep lowering"
        );
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("(String,JsonValue)".to_string()), 3).is_some(),
            "a Rust tuple of the wrong arity must still be refused"
        );
    }
}
