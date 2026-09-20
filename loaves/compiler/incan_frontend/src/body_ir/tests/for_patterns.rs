//! Destructuring `for` patterns: wildcard, tuple and nested tuple patterns over lists, ranges and user-defined
//! iteration protocols, the ownership and drop facts their bindings carry, loop-scoped visibility, and the type errors
//! and fail-closed lowering for non-tuple, mismatched-arity and unconstrained item types.

use super::*;

/// Build a Body IR module from `source` after rewriting its first `for a, b in ...:` header into the nested `for a, (b,
/// c) in ...:` shape the parser has no spelling for (see `nested_tuple_for_patterns_have_no_source_spelling_yet`). The
/// rewrite happens *before* typechecking, so the nested pattern flows through
/// `TypeChecker::define_for_pattern_bindings`' own recursion and reaches lowering with real resolved element types,
/// exactly as a future parser-supported nesting would.
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

/// Extract the `_N` place a loop's `IterNext` writes each produced item into, so a destructuring test can assert
/// on projections off that exact local without hard-coding a local number unrelated lowering changes would churn.
fn iter_next_destination(snapshot: &str) -> Option<String> {
    snapshot.lines().find_map(|line| {
        let (destination, _) = line.trim().split_once(" = iter_next(")?;
        Some(destination.to_string())
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
    let body = body_named(&module, "total")?;
    for name in ["a", "b"] {
        let binding = body
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some(name))
            .ok_or_else(|| format!("missing `{name}` binding"))?;
        assert!(
            binding.identity.is_some(),
            "`{name}` must retain its canonical identity: {binding:?}"
        );
    }

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
    // `for_binding_pattern_item` (`loaves/kernel/incan_syntax/src/parser/stmts.rs`) admits only `_` or a bare
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

    // Asserting the names are absent entirely only held while a destructuring clause was *refused*; #1161 lowers
    // one, so `a` and `b` now exist as the clause's own bindings. The property this test is actually for is
    // narrower and unchanged: neither may be captured from an enclosing scope where it does not exist.
    for binding in [" a : ", " b : "] {
        for line in snapshot.lines().filter(|line| line.contains(binding)) {
            assert!(
                !line.contains("[captured]"),
                "a clause-bound name must not be captured from the enclosing closure: {line}"
            );
        }
    }
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
    // The shape `loaves/stdlib/data/src/collections.incn` actually uses: the *item* is a tuple, and only
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
