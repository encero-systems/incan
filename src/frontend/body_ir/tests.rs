//! Tests for Body IR lowering.
//!
//! Moved verbatim out of the parent module: `use super::*` keeps every item these tests reach, so the split is a
//! pure relocation with no visibility change.

use super::defaults::*;
use super::*;
use crate::frontend::typechecker::TypeChecker;
use crate::frontend::{lexer, parser};
use incan_core::lang::surface::constructors;

/// Lower a module that imports from other modules, declaring each dependency's flattened cache name *and* its
/// real path segments.
///
/// Both are required because the flattened name is not injective: `("pkg_helpers", &["pkg", "helpers"], ..)` and
/// `("pkg_helpers", &["pkg_helpers"], ..)` are different modules sharing one cache key, and only the segments say
/// which one a fixture means.
fn build_with_imports(
    source: &str,
    module_path: &[&str],
    imports: &[(&str, &[&str], &str)],
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let mut import_programs = Vec::new();
    for (name, segments, import_source) in imports {
        let tokens = lexer::lex(import_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        import_programs.push((*name, *segments, program));
    }
    let import_refs: Vec<(&str, &ast::Program)> = import_programs
        .iter()
        .map(|(name, _, program)| (*name, program))
        .collect();

    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    for (name, segments, _) in &import_programs {
        checker.register_dependency_module_path_segments(
            name,
            segments.iter().map(|segment| segment.to_string()).collect(),
        );
    }
    checker
        .check_with_imports(&program, &import_refs)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Collect the named-callable targets called directly in one lowered body, in statement order.
fn named_targets<'module>(
    module: &'module bir::BodyIrModule,
    body_name: &str,
) -> Vec<&'module bir::NamedCallableTarget> {
    module
        .bodies
        .iter()
        .filter(|body| body.name == body_name)
        .flat_map(|body| &body.block.stmts)
        .filter_map(|stmt| match &stmt.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } => Some(target),
            _ => None,
        })
        .collect()
}

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

#[test]
fn imported_callable_without_call_site_identity_does_not_recover_authority_from_its_binding_name()
-> Result<(), Box<dyn std::error::Error>> {
    let dependency_source = "pub def helper() -> int:\n  return 42\n";
    let source = "from helpers import helper\n\ndef main() -> int:\n  return helper()\n";
    let dependency_tokens =
        lexer::lex(dependency_source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let dependency =
        parser::parse(&dependency_tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_with_imports(&program, &[("helpers", &dependency)])
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    assert!(
        checker.type_info().resolved_import_identity("helper").is_some(),
        "the fixture must retain import-binding metadata independently of the call-site fact"
    );
    let mut type_info = checker.type_info().clone();
    type_info.references.resolved_identities.clear();

    let module = build_body_ir_module_v0(&program, &module_path, &type_info);
    let target = named_targets(&module, "main")
        .into_iter()
        .next()
        .ok_or("missing imported helper call")?;
    assert_eq!(target.canonical, None);
    assert_eq!(target.direct_call_id, None);
    assert_eq!(target.builtin, None);
    Ok(())
}

#[test]
fn canonical_local_and_global_roots_survive_body_ir_lowering() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
const LIMIT: int = 3
static COUNT: int = 0

def update(value: int) -> int:
  mut local = value
  COUNT += local
  return LIMIT + COUNT
"#;
    let module = build(source, &["app"])?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "update")
        .ok_or("missing update body")?;
    assert!(
        body.locals
            .iter()
            .filter(|local| !matches!(local.origin, bir::LocalOrigin::Temporary))
            .all(|local| local.identity.is_some()),
        "every source local and parameter must retain its canonical declaration identity: {:?}",
        body.locals
    );
    assert!(
        !body
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "resolved globals must not degrade to External locals: {:?}",
        body.locals
    );
    let snapshot = body.render_snapshot();
    assert!(
        snapshot.contains("@const:app::LIMIT@"),
        "missing canonical const root: {snapshot}"
    );
    assert!(
        snapshot.contains("@static:app::COUNT@"),
        "missing canonical static root: {snapshot}"
    );
    Ok(())
}

#[test]
fn imported_alias_globals_keep_the_provider_identity() -> Result<(), Box<dyn std::error::Error>> {
    let provider = r#"
pub const LIMIT: int = 3
pub static COUNT: int = 1
"#;
    let consumer = r#"
from provider import LIMIT as maximum, COUNT as current

def read() -> int:
  return maximum + current
"#;
    let module = build_with_imports(consumer, &["consumer"], &[("provider", &["provider"], provider)])?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "read")
        .ok_or("missing read body")?;
    let snapshot = body.render_snapshot();
    assert!(
        snapshot.contains("@const:provider::LIMIT@"),
        "alias lost provider const identity: {snapshot}"
    );
    assert!(
        snapshot.contains("@static:provider::COUNT@"),
        "alias lost provider static identity: {snapshot}"
    );
    assert!(
        !snapshot.contains("maximum") && !snapshot.contains("current"),
        "global identity must not be reconstructed from the consumer alias: {snapshot}"
    );
    Ok(())
}

#[test]
fn rejected_const_write_keeps_its_canonical_target_and_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
const LIMIT: int = 3

def overwrite() -> int:
  LIMIT = 4
  return LIMIT
"#;
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["app", "const_write"])?;
    assert!(
        diagnostics.iter().any(|message| message.contains("LIMIT")),
        "the source checker must reject the const write: {diagnostics:?}"
    );
    let body = body_named(&module, "overwrite")?;
    assert!(
        !body
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "a rejected canonical const target must not degrade to an External local: {:?}",
        body.locals
    );
    assert!(
        body.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Unsupported { description }
                if description.contains("const:app::const_write::LIMIT")
                    && description.contains("not writable")
        )),
        "the rejected write must retain the const identity in its explicit refusal: {}",
        body.render_snapshot()
    );
    Ok(())
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

    let body = body_named(&module, "add")?;
    assert_eq!(
        body.decl_id, body.direct_call_id,
        "top-level Body IR and declaration HIR must correlate by the same span-derived node id"
    );
    assert!(snapshot_first.contains("body add decl:m::arith#decl."));
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

// ========================================================================
// #1160 -- power, bitwise, shift, membership, and identity operators
// ========================================================================

/// Lower one `def f(...)` body over `source` and return its rendered snapshot.
///
/// The operator tests below all assert against a single function body, so folding "build, find `f`, render" into
/// one call keeps each test's body about the operator rather than about the scaffolding.
fn rendered_f(source: &str, module_leaf: &str) -> Result<String, Box<dyn std::error::Error>> {
    let module = build(source, &["m", module_leaf])?;
    Ok(body_named(&module, "f")?.render_snapshot())
}

#[test]
fn lowers_the_power_operator_as_a_primitive_keeping_the_checked_float_promotion()
-> Result<(), Box<dyn std::error::Error>> {
    // A dynamic `int ** int` exponent resolves `float`: the typechecker owns that promotion and lowering must carry
    // its answer onto the assigned temporary rather than re-deriving a result type from the operator. A
    // non-negative integer-literal exponent is the separate `int` case.
    let rendered = rendered_f("def f(a: int, b: int) -> float:\n  return a ** b\n", "pow")?;

    assert!(
        rendered.contains("_2 = copy(_0) ** copy(_1)"),
        "`**` must lower as a primitive binary op: {rendered}"
    );
    assert!(
        rendered.contains("local 2 <tmp> : float"),
        "the checked `float` result of `int ** int` must survive onto the temporary: {rendered}"
    );
    Ok(())
}

#[test]
fn exact_binary_float_arithmetic_keeps_the_checked_body_ir_width() -> Result<(), Box<dyn std::error::Error>> {
    for kind in ["f32", "f64"] {
        let source = format!("def f(left: {kind}, right: {kind}) -> {kind}:\n  return left * right\n");
        let rendered = rendered_f(&source, &format!("exact_{kind}"))?;
        assert!(
            rendered.contains(&format!("local 2 <tmp> : {kind}")),
            "{kind} multiplication must retain its checked exact result in Body IR: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn lowers_the_bitwise_and_shift_operators_as_primitives_keeping_the_checked_int_result()
-> Result<(), Box<dyn std::error::Error>> {
    for (spelling, module_leaf) in [
        ("&", "bitand"),
        ("|", "bitor"),
        ("^", "bitxor"),
        ("<<", "shl"),
        (">>", "shr"),
    ] {
        let source = format!("def f(a: int, b: int) -> int:\n  return a {spelling} b\n");
        let rendered = rendered_f(&source, module_leaf)?;

        assert!(
            rendered.contains(&format!("_2 = copy(_0) {spelling} copy(_1)")),
            "`{spelling}` must lower as a primitive binary op over both operands: {rendered}"
        );
        assert!(
            rendered.contains("local 2 <tmp> : int"),
            "`int {spelling} int` must keep its checked `int` result: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn lowers_identity_operators_distinctly_from_equality() -> Result<(), Box<dyn std::error::Error>> {
    // The Rust-emission backend currently emits `is`/`is not` exactly like `==`/`!=`. Body IR must still record
    // which operator the source wrote -- it is the representation any later identity/equality split gets decided
    // against, and a collapsed `is` leaves nothing to decide from.
    let is_rendered = rendered_f("def f(a: int, b: int) -> bool:\n  return a is b\n", "is_op")?;
    assert!(
        is_rendered.contains("_2 = copy(_0) is copy(_1)"),
        "`is` must lower as its own operator: {is_rendered}"
    );
    assert!(
        !is_rendered.contains("=="),
        "`is` must not be collapsed into equality: {is_rendered}"
    );

    let is_not_rendered = rendered_f("def f(a: int, b: int) -> bool:\n  return a is not b\n", "is_not_op")?;
    assert!(
        is_not_rendered.contains("_2 = copy(_0) is not copy(_1)"),
        "`is not` must lower as its own operator: {is_not_rendered}"
    );
    assert!(
        !is_not_rendered.contains("!="),
        "`is not` must not be collapsed into inequality: {is_not_rendered}"
    );
    Ok(())
}

#[test]
fn lowers_string_membership_as_an_explicit_helper_call_with_its_runtime_requirement()
-> Result<(), Box<dyn std::error::Error>> {
    // This is the Body IR representation `parity-987-0003` was missing. That row records string `in` as
    // `Preserved` -- the runtime helper's substring policy -- but evaluates through the stdlib-runtime evidence
    // lane, so nothing proved the behavior was expressible here at all. An explicit `Callee::Helper` call with a
    // recorded runtime requirement is what closes that gap: the substring choice is now a represented fact rather
    // than something a reader has to infer from the operand types.
    let rendered = rendered_f(
        "def f(hay: str, needle: str) -> bool:\n  return needle in hay\n",
        "str_in",
    )?;

    // Membership is the one string operator whose surface order is the reverse of its helper's signature. The call
    // is emitted haystack-first to match `incan_core::strings::str_contains`, so a backend can bind every string
    // helper positionally without knowing that one of them disagrees with the rest.
    assert!(
        rendered.contains("_2 = call helper:str_contains(move(_0, last_use), move(_1, last_use))"),
        "string `in` must lower to a helper call carrying the haystack then the needle: {rendered}"
    );
    assert!(
        rendered.contains("runtime_helper(str_contains)"),
        "the helper call must record its runtime requirement: {rendered}"
    );
    assert!(
        !rendered.contains("unsupported("),
        "string membership must not fall back to a placeholder: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_negated_string_membership_as_its_own_helper_rather_than_a_wrapped_negation()
-> Result<(), Box<dyn std::error::Error>> {
    // One source operator stays one Body IR operation, following the `str_eq`/`str_ne` pair: a consumer reading
    // this call knows the source wrote `not in` without having to recognize a negation wrapper around `in`.
    let rendered = rendered_f(
        "def f(hay: str, needle: str) -> bool:\n  return needle not in hay\n",
        "str_not_in",
    )?;

    assert!(
        rendered.contains("_2 = call helper:str_not_contains(move(_0, last_use), move(_1, last_use))"),
        "string `not in` must lower to its own helper call, haystack first: {rendered}"
    );
    assert!(
        rendered.contains("runtime_helper(str_not_contains)"),
        "the negated membership helper must record its own runtime requirement: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_collection_membership_as_a_helper_call_naming_its_own_container() -> Result<(), Box<dyn std::error::Error>> {
    // Each collection names its own helper rather than sharing one `contains`. A single variant would leave a
    // consumer to re-derive list-versus-set-versus-dict from operand types, which is the inference Body IR exists
    // to replace with a represented fact -- and it is why `str in str` keeps its own helper too.
    //
    // The call is emitted haystack-first to match `str_contains` and every `contains` in Rust, while the source
    // spelling reads needle-first, so a backend can bind every membership helper positionally.
    for (container, module_leaf, helper) in [
        ("xs: List[int], v: int", "list_in", "list_contains"),
        ("xs: Set[int], v: int", "set_in", "set_contains"),
        ("xs: Dict[int, str], v: int", "dict_in", "dict_contains_key"),
    ] {
        let source = format!("def f({container}) -> bool:\n  return v in xs\n");
        let rendered = rendered_f(&source, module_leaf)?;

        assert!(
            rendered.contains(&format!("call helper:{helper}(move(_0, last_use)")),
            "`in` over {container} must lower to {helper} with the container first: {rendered}"
        );
        assert!(
            rendered.contains(&format!("runtime_helper({helper})")),
            "{helper} must record its runtime requirement: {rendered}"
        );
        assert!(
            !rendered.contains("unsupported("),
            "collection membership must not fall back to a placeholder: {rendered}"
        );
        assert!(
            !rendered.contains("str_contains"),
            "collection membership must not borrow the string substring policy: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn lowers_negated_collection_membership_as_its_own_helper_per_container() -> Result<(), Box<dyn std::error::Error>> {
    // One source operator stays one Body IR operation, following the `str_contains`/`str_not_contains` pair: a
    // consumer reading this call knows the source wrote `not in` without recognizing a negation wrapper.
    for (container, module_leaf, helper) in [
        ("xs: List[int], v: int", "list_not_in", "list_not_contains"),
        ("xs: Set[int], v: int", "set_not_in", "set_not_contains"),
        ("xs: Dict[int, str], v: int", "dict_not_in", "dict_not_contains_key"),
    ] {
        let source = format!("def f({container}) -> bool:\n  return v not in xs\n");
        let rendered = rendered_f(&source, module_leaf)?;

        assert!(
            rendered.contains(&format!("call helper:{helper}(move(_0, last_use)")),
            "`not in` over {container} must lower to {helper}, container first: {rendered}"
        );
        assert!(
            rendered.contains(&format!("runtime_helper({helper})")),
            "{helper} must record its own runtime requirement: {rendered}"
        );
        assert!(
            !rendered.contains("un_op") && !rendered.contains("Not("),
            "`not in` must be its own operation, not a negation wrapped around `in`: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn dict_membership_names_key_lookup_rather_than_element_lookup() -> Result<(), Box<dyn std::error::Error>> {
    // Dict membership tests keys while its sibling collections test elements. Leaving that to be inferred from the
    // receiver type would make key-versus-value a backend convention; naming it in the operation makes it a fact.
    let rendered = rendered_f(
        "def f(d: Dict[str, int], k: str) -> bool:\n  return k in d\n",
        "dict_key_in",
    )?;

    assert!(
        rendered.contains("call helper:dict_contains_key("),
        "dict `in` must name key lookup: {rendered}"
    );
    assert!(
        !rendered.contains("helper:dict_contains("),
        "dict membership must not use an element-lookup spelling: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_list_concatenation_as_a_helper_call_rather_than_a_primitive_addition()
-> Result<(), Box<dyn std::error::Error>> {
    // The regression this test exists for: `+` on two lists used to satisfy `binary_op_is_supported` through the
    // numeric mapping and lower to `BinOp::Add`, a machine addition over two heap containers, with no
    // `Unsupported` marker for a consumer to notice. The typechecker accepts list `+` through a builtin branch
    // that records no operator dispatch, so nothing downstream marked it as a call -- which is exactly the
    // wrong-representation failure `lower_operator_dispatch` guards user-defined `__add__` against.
    let rendered = rendered_f(
        "def f(xs: List[int], ys: List[int]) -> List[int]:\n  return xs + ys\n",
        "list_concat",
    )?;

    assert!(
        rendered.contains("call helper:list_concat(move(_0, last_use), move(_1, last_use))"),
        "list `+` must lower to the concatenation helper in source order: {rendered}"
    );
    assert!(
        rendered.contains("runtime_helper(list_concat)"),
        "list concatenation must record its runtime requirement: {rendered}"
    );
    assert!(
        !rendered.contains(") + "),
        "list `+` must not lower to a primitive addition: {rendered}"
    );
    Ok(())
}

#[test]
fn compound_list_assignment_routes_through_the_same_concatenation_helper() -> Result<(), Box<dyn std::error::Error>> {
    // `lower_compound_assignment` shares both the admission gate and the emission path with `lower_binary`, so
    // giving list `+` a helper silently changed `xs += ys` too. That is the behaviour the Rust-emission backend
    // already has -- `determine_binop_plan` sees `Add` over two lists whichever spelling produced it -- but a
    // shared path that changed without coverage is exactly where a later divergence would hide.
    let rendered = rendered_f(
        "def f(ys: List[int]) -> List[int]:\n  mut xs = [1, 2]\n  xs += ys\n  return xs\n",
        "list_concat_assign",
    )?;

    assert!(
        rendered.contains("call helper:list_concat("),
        "`+=` on lists must reuse the concatenation helper rather than a primitive addition: {rendered}"
    );
    assert!(
        rendered.contains("runtime_helper(list_concat)"),
        "the compound form must record the same runtime requirement as the binary form: {rendered}"
    );
    Ok(())
}

#[test]
fn list_equality_stays_a_primitive_because_that_is_what_the_other_backend_emits()
-> Result<(), Box<dyn std::error::Error>> {
    // The counterweight to the concatenation test above, and the reason closing the admission hole is not "refuse
    // every primitive over a collection". `determine_binop_plan` in the Rust-emission backend routes list `+` to
    // `incan_stdlib::collections::list_concat` -- so calling it `BinOp::Add` contradicted that backend -- but it
    // emits comparisons as an infix operator, which on two `Vec`s resolves to Rust's `PartialEq` and matches
    // Python's element-wise `==`. Both backends therefore agree that this one *is* an operator, and refusing it
    // here would manufacture a divergence instead of closing one.
    let rendered = rendered_f(
        "def f(xs: List[int], ys: List[int]) -> bool:\n  return xs == ys\n",
        "list_eq",
    )?;

    assert!(
        !rendered.contains("unsupported("),
        "list equality must keep lowering, matching the Rust-emission backend: {rendered}"
    );
    assert!(
        rendered.contains(" == "),
        "list equality must stay a primitive comparison rather than becoming a helper call: {rendered}"
    );
    Ok(())
}

#[test]
fn an_unresolved_binary_operand_refuses_before_either_expression_is_lowered() -> Result<(), Box<dyn std::error::Error>>
{
    let binary = "def f(value: int) -> int:\n  return value & missing\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(binary, &["m", "unknown_binary"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the source checker must reject the unknown operand before Body IR lowers it"
    );
    assert!(
        rendered.contains("unsupported(binary operator BitAnd)"),
        "an unresolved primitive operand must refuse at the binary expression: {rendered}"
    );
    assert!(
        !rendered.contains("copy(_0)") && !rendered.contains("missing"),
        "the refusal must precede both operand reads, not materialize an external unknown: {rendered}"
    );

    let compound = "def f() -> int:\n  mut value = 1\n  value &= missing\n  return value\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(compound, &["m", "unknown_compound"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the source checker must reject the unknown compound right operand before Body IR lowers it"
    );
    assert!(
        rendered.contains("unsupported(compound assignment operator BitAnd)"),
        "an unresolved compound operand must refuse at the compound statement: {rendered}"
    );
    assert!(
        !rendered.contains(" & ") && !rendered.contains("missing"),
        "the compound refusal must not synthesize a primitive operation or external read: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_every_bitwise_and_shift_compound_assignment_as_a_read_modify_write() -> Result<(), Box<dyn std::error::Error>>
{
    // Each `<op>=` form must produce exactly the operator its binary spelling produces, plus a write back to the
    // same local -- the compound path shares `lower_binary_from_operands` precisely so the two cannot drift.
    let source = "def f() -> int:\n  mut v = 8\n  v &= 3\n  v |= 4\n  v ^= 1\n  v <<= 2\n  v >>= 1\n  return v\n";
    let rendered = rendered_f(source, "compound_bits")?;

    for (index, (spelling, operand)) in [("&", 3), ("|", 4), ("^", 1), ("<<", 2), (">>", 1)].iter().enumerate() {
        let temp = index + 1;
        assert!(
            rendered.contains(&format!("_{temp} = copy(_0) {spelling} const({operand})")),
            "`{spelling}=` must combine the current value with the right operand: {rendered}"
        );
        assert!(
            rendered.contains(&format!("_0 = copy(_{temp}, last_use)")),
            "`{spelling}=` must write the result back to the assigned local: {rendered}"
        );
    }
    assert!(
        !rendered.contains("unsupported("),
        "no bitwise or shift compound form may fall back: {rendered}"
    );
    Ok(())
}

#[test]
fn a_compound_assignment_through_an_operator_hook_is_refused_by_name() -> Result<(), Box<dyn std::error::Error>> {
    // `v &= w` on a type with `__and__` is a method call. Now that `&` has a primitive `BinOp`, combining the
    // operands here would claim a machine operation the source never asked for -- the wrong-representation
    // failure `lower_binary` already guards against for the binary spelling. Body IR has no place-targeted
    // dispatch form yet, so the refusal names the hook it would have to call.
    let source = "model Box:\n  value: int\n\n  def __and__(self, other: Box) -> Box:\n    return Box(value=self.value & other.value)\n\ndef f() -> int:\n  mut v = Box(value=1)\n  v &= Box(value=2)\n  return v.value\n";
    let rendered = rendered_f(source, "hook_compound")?;

    assert!(
        rendered.contains("unsupported(compound assignment through operator hook `__and__`)"),
        "a hooked compound assignment must refuse by naming the method it would dispatch to: {rendered}"
    );
    assert!(
        !rendered.contains("copy(_1) & "),
        "it must not fall through to the primitive bitwise operator: {rendered}"
    );
    Ok(())
}

#[test]
fn protocol_hook_operators_reach_lowering_as_resolved_method_calls() -> Result<(), Box<dyn std::error::Error>> {
    // #1160's refusal-boundary question, answered: `@` and both pipes are protocol hooks with no primitive form.
    // The typechecker resolves them through `__matmul__` / `__pipe_forward__` / `__pipe_backward__` and rejects
    // the expression outright when no hook resolves, so a well-typed program always arrives here with a recorded
    // dispatch. They need no operator-table entry and carry no refusal -- and so no `Disposition::Unsupported`
    // corpus row, which would otherwise have needed an owner.
    let source = "model OpBox:\n  value: int\n\n  def __matmul__(self, other: OpBox) -> OpBox:\n    return other\n\n  def __pipe_forward__(self, other: OpBox) -> OpBox:\n    return other\n\n  def __pipe_backward__(self, other: OpBox) -> OpBox:\n    return other\n\ndef matmul(a: OpBox, b: OpBox) -> OpBox:\n  return a @ b\n\ndef forward(a: OpBox, b: OpBox) -> OpBox:\n  return a |> b\n\ndef backward(a: OpBox, b: OpBox) -> OpBox:\n  return a <| b\n";
    let module = build(source, &["m", "protocol_hooks"])?;

    for (body_name, method, spelling) in [
        ("matmul", "__matmul__", "@"),
        ("forward", "__pipe_forward__", "|>"),
        ("backward", "__pipe_backward__", "<|"),
    ] {
        let rendered = body_named(&module, body_name)?.render_snapshot();
        assert!(
            rendered.contains(&format!("call method:{method} unbound(borrow(_0), move(_1, last_use))")),
            "`{spelling}` must lower as the method the typechecker resolved, receiver borrowed: {rendered}"
        );
        assert!(
            !rendered.contains("unsupported("),
            "`{spelling}` must not reach the operator table at all: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn the_shift_and_power_operators_record_no_panic_fact() -> Result<(), Box<dyn std::error::Error>> {
    // A stated decision, not an omission. `**` and the shifts can only trap by exceeding the result width, which
    // is the same arithmetic-overflow class as `+`, `-`, and `*` -- none of which record a fact either. Recording
    // one here would claim these three operators fail in a way ordinary arithmetic does not.
    let rendered = rendered_f(
        "def f(a: int, b: int) -> int:\n  return (a << b) + (a >> b)\n",
        "shift_panics",
    )?;
    assert!(
        !rendered.contains("panic_facts:"),
        "shifts must not record a panic fact: {rendered}"
    );

    // The contrast that gives that decision meaning: floor division, whose divisor may be zero on every build
    // profile, still records one.
    let divide = rendered_f("def f(a: int, b: int) -> int:\n  return a // b\n", "div_panics")?;
    assert!(
        divide.contains("division_or_modulo"),
        "division must still record its panic fact: {divide}"
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
fn unsupported_constructs_lower_to_an_explicit_placeholder_instead_of_panicking()
-> Result<(), Box<dyn std::error::Error>> {
    // This case was the refusal's own pin until #1161: a destructuring generator clause used to refuse the whole
    // expression rather than bind its pattern. It now lowers, so the test asserts the positive behaviour instead of
    // freezing a hole -- the clause's names become real bindings projected out of the polled item, exactly as the
    // equivalent statement `for` produces them.
    let source = "def pick(x: int) -> int:\n  gen = (left + right for left, right in [(1, 2)])\n  return x\n";
    let module = build(source, &["m", "unsupported"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a destructuring generator clause must lower rather than refuse: {snapshot}"
    );
    for binding in ["left", "right"] {
        assert!(
            snapshot.contains(&format!(" {binding} : int [binding]")),
            "the clause must bind `{binding}` with the tuple element's resolved type: {snapshot}"
        );
    }
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
    let body = body_named(&module, "keep_outer")?;
    let identities: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("x"))
        .map(|local| local.identity.as_ref())
        .collect();
    let [Some(parameter), Some(comprehension_binding)] = identities.as_slice() else {
        return Err(
            format!("expected canonical identities for the parameter and comprehension binding: {body:?}").into(),
        );
    };
    assert_ne!(
        parameter, comprehension_binding,
        "scoped bindings need distinct identities"
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
        IncanType::Primitive(IncanPrimitiveType::Numeric(
            incan_core::lang::types::numerics::NumericTypeId::U8
        ))
    );
    let bir::CallableParamDefault::Source(limit_default) = &limit.default else {
        return Err("a checked literal default must become a deferred Body-IR computation".into());
    };
    let limit_start = source.find("7,").ok_or("missing literal default source spelling")?;
    assert_eq!(limit_default.span, HirSourceSpan::new(limit_start, limit_start + 1));
    assert!(limit_default.stmts.is_empty());
    assert_eq!(
        limit_default.result,
        bir::Operand::Constant(bir::Constant::TypedNumeric(bir::TypedNumericConstant::Unsigned {
            kind: incan_core::lang::types::numerics::NumericTypeId::U8,
            value: 7,
        }))
    );

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
fn byte_string_default_is_a_deferred_body_ir_constant_at_its_own_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep(payload: bytes = b\"x\") -> bytes:\n  return payload\n";
    let module = build(source, &["m", "unsupported_default"])?;
    let keep = module.bodies.first().ok_or("expected the keep function Body IR")?;
    let payload = keep.params.first().ok_or("expected the payload parameter")?;

    let bir::CallableParamDefault::Source(default) = &payload.default else {
        return Err("bytes defaults must retain their representable Body-IR constant".into());
    };
    let default_start = source.find("b\"x\"").ok_or("missing bytes default spelling")?;
    assert_eq!(
        default.span,
        HirSourceSpan::new(default_start, default_start + "b\"x\"".len()),
        "the deferred computation must retain the default expression's exact source span"
    );
    assert!(default.stmts.is_empty());
    assert_eq!(
        default.result,
        bir::Operand::Constant(bir::Constant::Bytes(b"x".to_vec()))
    );
    assert_eq!(
        keep.locals.len(),
        1,
        "a literal default must not allocate a speculative temporary or external local: {keep:?}"
    );
    assert!(
        !keep
            .block
            .stmts
            .iter()
            .any(|statement| matches!(&statement.kind, bir::StatementKind::Unsupported { .. })),
        "the deferred default belongs to parameter metadata, not the normal function body: {keep:?}"
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
    // Lowering the partial moves one of its synthesized forwarding locals before the unsupported binary refuses.
    // The transaction must discard that move before `second` reuses the local id in the normal body, or the
    // required root-scope drop would silently disappear.
    let source = "def route(method: str) -> str:\n  return method\n\ndef choose(value: str = (partial route(method=\"GET\")) + missing) -> str:\n  first = \"first\"\n  second = \"second\"\n  return first\n";
    let (module, _diagnostics) = build_after_expected_typecheck_errors(source, &["m", "default_ownership_rollback"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let value = choose.params.first().ok_or("expected choose's value parameter")?;
    let bir::CallableParamDefault::Unsupported { description, .. } = &value.default else {
        return Err(format!("the unsupported default must remain a refusal: {:?}", value.default).into());
    };
    assert!(
        description.contains("binary operator Add"),
        "the refusal must name the unsupported default operation: {description}"
    );
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
    // `bir::Constant::Bytes` represents a byte value, but the closed `bir::Pattern` vocabulary does not yet model
    // byte-pattern matching semantics. Refuse before lowering the scrutinee rather than silently mis-rendering
    // the pattern as a catch-all wildcard the existing Rust-emission backend's own `lower_pattern` would emit.
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
        snapshot.contains("Circle(bind(_1, borrow)) canonical=")
            && snapshot.contains("Square(bind(_1, borrow)) canonical="),
        "both canonical alternatives should bind the same shared local `_1`: {snapshot}"
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
        // A provider operation's declaration slots are described by its plan's inputs, not by an argument binding.
        bir::Callee::ProviderOperation(_) => Err("a provider operation carries its own input facts".into()),
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
    assert_eq!(
        declaration.canonical.kind,
        SemanticSourceTargetKind::Model,
        "the retained physical declaration must carry its compiler-owned model identity"
    );
    assert_eq!(declaration.canonical.declaration_name, "Pair");
    assert_eq!(declaration.field_identities.len(), declaration.fields.len());
    assert_eq!(
        declaration
            .field_identities
            .iter()
            .map(|identity| (identity.kind.clone(), identity.declaration_name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (SemanticSourceTargetKind::Field, "left"),
            (SemanticSourceTargetKind::Field, "right")
        ],
        "the physical field layout must retain the checked member identity for every slot"
    );

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
    assert_eq!(target.canonical.as_ref(), Some(&declaration.canonical));
    assert_eq!(
        target.direct_declaration_id.as_ref(),
        Some(&declaration.direct_declaration_id)
    );
    assert_eq!(
        target.canonical_field_layout.as_deref(),
        Some(declaration.fields.as_slice()),
        "the constructor must retain the checked layout independently from the mutable module declaration"
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
    let declaration = module
        .value_enum_declarations
        .first()
        .ok_or("the value enum must retain its declaration record")?;
    let variant = declaration
        .variants
        .iter()
        .find(|variant| variant.name == "NotFound")
        .ok_or("the value enum must retain NotFound")?;
    assert_eq!(declaration.canonical.kind, SemanticSourceTargetKind::Enum);
    assert_eq!(variant.canonical.kind, SemanticSourceTargetKind::Variant);
    let selected = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::ValueEnumVariant(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("the selected member must retain its value-enum target")?;
    assert_eq!(selected.enum_canonical, declaration.canonical);
    assert_eq!(selected.variant_canonical, variant.canonical);
    let body = body_named(&module, "main")?;
    let value_method = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("the value-enum scalar projection must lower as a method call")?;
    let canonical = value_method
        .canonical
        .as_ref()
        .ok_or("the checked value-enum method must retain its canonical target")?;
    assert_eq!(canonical.namespace, incan_semantics_core::SymbolNamespace::Member);
    assert_eq!(canonical.kind, SemanticSourceTargetKind::Method);
    assert_eq!(canonical.declaration_name, "value");
    assert_eq!(
        canonical.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["m".to_string(), "value_enum_identity".to_string()])
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
    let declaration = module
        .fieldless_enum_declarations
        .first()
        .ok_or("the fieldless enum must retain its declaration record")?;
    let selected = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::FieldlessEnumVariant(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("the selected member must retain its fieldless-enum target")?;
    let variant = declaration
        .variants
        .iter()
        .find(|variant| variant.name == selected.variant_name)
        .ok_or("the selected variant must exist in the retained registry")?;
    assert_eq!(selected.enum_canonical, declaration.canonical);
    assert_eq!(selected.variant_canonical, variant.canonical);
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
        "{ASYNC_PRELUDE}async def f(flag: bool) -> int:\n  mut total = 0\n  if flag:\n    total = await fast()\n  else:\n    total = 7\n  return total\n"
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
        "{ASYNC_PRELUDE}async def f() -> int:\n  mut total = 0\n  mut i = 0\n  while i < 3:\n    total = total + await fast()\n    i = i + 1\n  return total\n"
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
    let body = body_named(&module, "f")?;
    let rendered = body.render_snapshot();

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
    // Each arm owns a type-refined local, but both locals are projections of the one authored race-header binding.
    assert_eq!(
        rendered.matches("value : int [binding]").count(),
        2,
        "each arm must bind its own local rather than sharing one: {rendered}"
    );
    let identities: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("value"))
        .filter_map(|local| local.identity.as_ref())
        .collect();
    let [first_arm, second_arm] = identities.as_slice() else {
        return Err(format!("both arm bindings need canonical identities: {body:?}").into());
    };
    assert_eq!(first_arm, second_arm, "both arm locals must retain the header identity");
    let header = source.find("value:").ok_or("missing race header binding")?;
    for local in body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("value"))
    {
        assert_eq!(
            local.span,
            incan_semantics_core::HirSourceSpan::new(header, header + "value".len()),
            "each arm local must stay anchored to the exact race header token"
        );
    }
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
    //
    // The arm takes its block form so it can hold the shared by-design stand-in refusal. Its previous stand-in was
    // collection membership, which #1246 represented -- see STAND_IN_REFUSAL_LABEL for why a gap is never a safe
    // choice here.
    let source = format!(
        "{ASYNC_PRELUDE}async def f() -> bool:\n  race for value:\n    await fast() => value == 1\n    await slow() =>\n{}      value == 2\n",
        stand_in_refusal_stmt("      ")
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
    let provider_operations = ProviderOperationCatalog::new();
    let lowering_facts = BodyIrLoweringFacts {
        type_info: &type_info,
        function_default_sources: &function_default_sources,
        local_function_declarations: &local_function_declarations,
        local_nominal_declarations: &local_nominal_declarations,
        local_fieldless_enum_declarations: &local_fieldless_enum_declarations,
        local_value_enum_declarations: &local_value_enum_declarations,
        module_identity: "m",
        provider_operations: &provider_operations,
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

/// RFC 120's guide-level example: one declaration reached four ways is one identity.
///
/// A local call, a plain import, an import alias, and a re-export through a facade are all *bindings to* one
/// declaration. None of them creates a second identity for the thing it names, and the facade in particular must
/// not be recorded as an owner of what it merely re-exports.
#[test]
fn one_declaration_keeps_one_identity_across_local_imported_aliased_and_reexported_calls()
-> Result<(), Box<dyn std::error::Error>> {
    let helpers_source = r#"
pub def render() -> int:
  return 1

def use_local() -> int:
  return render()
"#;
    let facade_source = r#"
from helpers import render
"#;
    let app_source = r#"
from helpers import render
from helpers import render as draw
from facade import render as relayed

def use_imported() -> int:
  return render()

def use_alias() -> int:
  return draw()

def use_reexport() -> int:
  return relayed()
"#;
    let helpers = build(helpers_source, &["helpers"])?;
    let app = build_with_imports(
        app_source,
        &["app"],
        &[
            ("helpers", &["helpers"], helpers_source),
            ("facade", &["facade"], facade_source),
        ],
    )?;

    let mut facts = Vec::new();
    for (module, body) in [
        (&helpers, "use_local"),
        (&app, "use_imported"),
        (&app, "use_alias"),
        (&app, "use_reexport"),
    ] {
        let targets = named_targets(module, body);
        let [target] = targets.as_slice() else {
            return Err(Box::from(format!(
                "expected one named call in `{body}`, got {}",
                targets.len()
            )));
        };
        let Some(fact) = &target.canonical else {
            return Err(Box::from(format!("`{body}` must carry a canonical identity")));
        };
        facts.push((body, target.name.clone(), fact.clone()));
    }

    // One declaration, one identity, however each call site spelled it.
    let (_, _, first) = &facts[0];
    for (body, _, fact) in &facts {
        assert_eq!(fact, first, "`{body}` must resolve to the one declaration identity");
    }

    // The identity describes the declaration, never the reference.
    assert_eq!(first.declaration_name, "render");
    assert_eq!(first.kind, SemanticSourceTargetKind::Function);
    assert_eq!(first.namespace, incan_semantics_core::SymbolNamespace::OrdinaryLexical);
    assert_eq!(
        first.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["helpers".to_string()]),
        "the origin is the declaring module, never the importing or re-exporting one"
    );
    assert_eq!(
        first.scope_discriminant, None,
        "a module-level declaration is unique within its origin"
    );

    // It anchors to the one declaration site, not to any call site.
    let render_body = helpers
        .bodies
        .iter()
        .find(|body| body.name == "render")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("lowered `render` body missing"))?;
    assert_eq!(first.declaration_span, render_body.span);

    // The call-site spellings genuinely differ; only the identity collapses them.
    let spellings: Vec<&str> = facts.iter().map(|(_, name, _)| name.as_str()).collect();
    assert_eq!(spellings, vec!["render", "render", "draw", "relayed"]);
    Ok(())
}

/// A nested facade resolves its own relative re-export against *its* module, not the consumer's.
///
/// `pkg.facade` writing `from helpers import render` means `pkg.helpers`, because a sibling-relative candidate is
/// tried before the bare one from `pkg.facade`. Resolving that link from the consumer instead binds the root
/// `helpers`, and since the cache and the identity both resolved it, they would agree on the wrong declaration.
/// Distinguished by arity so the binding itself proves which module won.
#[test]
fn a_nested_facade_reexport_resolves_against_the_facade_not_the_consumer() -> Result<(), Box<dyn std::error::Error>> {
    let root_helpers = r#"
pub def render(first: int, second: int) -> int:
  return first + second
"#;
    let nested_helpers = r#"
pub def render() -> int:
  return 1
"#;
    let facade_source = r#"
from helpers import render
"#;
    let app_source = r#"
from pkg.facade import render

def run() -> int:
  return render()
"#;
    let app = build_with_imports(
        app_source,
        &["app"],
        &[
            ("helpers", &["helpers"], root_helpers),
            ("pkg_helpers", &["pkg", "helpers"], nested_helpers),
            ("pkg_facade", &["pkg", "facade"], facade_source),
        ],
    )?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from("the re-exported call must carry an identity".to_string()));
    };

    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["pkg".to_string(), "helpers".to_string()]),
        "the facade's own module decides what its relative re-export means"
    );

    // The span must be the nested declaration's, not the root's identically-named one.
    let nested = build(nested_helpers, &["pkg", "helpers"])?;
    let nested_render = nested
        .bodies
        .iter()
        .find(|body| body.name == "render")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("nested `render` body missing"))?;
    assert_eq!(fact.declaration_span, nested_render.span);

    let root = build(root_helpers, &["helpers"])?;
    if let Some(root_render) = root.bodies.iter().find(|body| body.name == "render") {
        assert_ne!(
            fact.declaration_span, root_render.span,
            "the root module's `render` is a different declaration"
        );
    }
    Ok(())
}

/// Same-named declarations cross-imported along an acyclic chain: `a` <- `b` <- `c` <- `d`.
///
/// Module `c` ends up with three `make` declarations in scope at once — its own, `a`'s, and `b`'s — reached under
/// three spellings. Each must resolve to its own declaration, and `d` must agree with `c` about which is which.
#[test]
fn same_named_declarations_cross_imported_along_a_chain_stay_distinct() -> Result<(), Box<dyn std::error::Error>> {
    let a_source = r#"
pub def make() -> int:
  return 1
"#;
    let b_source = r#"
from a import make as make_a

pub def make() -> int:
  return 2

def use_all() -> int:
  return make() + make_a()
"#;
    let c_source = r#"
from a import make as make_a
from b import make as make_b

pub def make() -> int:
  return 3

def use_all() -> int:
  return make() + make_a() + make_b()
"#;
    let d_source = r#"
from a import make as make_a
from b import make as make_b
from c import make as make_c

def use_all() -> int:
  return make_a() + make_b() + make_c()
"#;

    fn origins(
        module: &bir::BodyIrModule,
        body: &str,
    ) -> Result<Vec<incan_semantics_core::SymbolOrigin>, Box<dyn std::error::Error>> {
        let mut found = Vec::new();
        for target in named_targets(module, body) {
            let Some(fact) = &target.canonical else {
                return Err(Box::from(format!("a call in `{body}` carried no identity")));
            };
            assert_eq!(fact.declaration_name, "make");
            found.push(fact.origin.clone());
        }
        found.sort();
        Ok(found)
    }
    let module_origin = |name: &str| incan_semantics_core::SymbolOrigin::Module(vec![name.to_string()]);

    let b = build_with_imports(b_source, &["b"], &[("a", &["a"], a_source)])?;
    assert_eq!(
        origins(&b, "use_all")?,
        vec![module_origin("a"), module_origin("b")],
        "`b` sees its own `make` and `a`'s as different declarations"
    );

    let c = build_with_imports(c_source, &["c"], &[("a", &["a"], a_source), ("b", &["b"], b_source)])?;
    assert_eq!(
        origins(&c, "use_all")?,
        vec![module_origin("a"), module_origin("b"), module_origin("c")],
        "`c` holds three same-named declarations at once and must keep them apart"
    );

    let d = build_with_imports(
        d_source,
        &["d"],
        &[
            ("a", &["a"], a_source),
            ("b", &["b"], b_source),
            ("c", &["c"], c_source),
        ],
    )?;
    assert_eq!(
        origins(&d, "use_all")?,
        vec![module_origin("a"), module_origin("b"), module_origin("c")],
        "a consumer that only imports must agree with `c` about which declaration is which"
    );
    Ok(())
}

/// Three modules with byte-identical contents, consumed together.
///
/// Because the sources are identical, `declaration_name`, `kind`, and `declaration_span` are identical across all
/// three `make` declarations, so `origin` is the only field that can separate them. If origin were dropped, wrong,
/// or recovered from a spelling, the three would collapse into one identity — and a consumer would dispatch a call
/// on `a` to the declaration in `c`.
///
/// It also pins the converse: one declaration reached from two modules under two spellings stays one identity.
#[test]
fn identical_modules_consumed_together_keep_three_distinct_identities() -> Result<(), Box<dyn std::error::Error>> {
    // One source text, used verbatim for `a`, `b`, and `c`.
    let shared_source = r#"
pub model Item:
  value: int

pub def make() -> int:
  return 1

def use_local() -> int:
  return make()
"#;
    let consumer_source = r#"
from a import make as make_a
from b import make as make_b
from c import make as make_c

def use_a() -> int:
  return make_a()

def use_b() -> int:
  return make_b()

def use_c() -> int:
  return make_c()
"#;
    let consumer = build_with_imports(
        consumer_source,
        &["d"],
        &[
            ("a", &["a"], shared_source),
            ("b", &["b"], shared_source),
            ("c", &["c"], shared_source),
        ],
    )?;

    let mut facts = Vec::new();
    for body in ["use_a", "use_b", "use_c"] {
        let targets = named_targets(&consumer, body);
        let [target] = targets.as_slice() else {
            return Err(Box::from(format!(
                "expected one named call in `{body}`, got {}",
                targets.len()
            )));
        };
        let Some(fact) = &target.canonical else {
            return Err(Box::from(format!("`{body}` must carry an identity")));
        };
        facts.push(fact.clone());
    }

    // The premise: everything except origin is identical, so origin alone carries the distinction.
    for fact in &facts {
        assert_eq!(fact.declaration_name, "make");
        assert_eq!(fact.kind, SemanticSourceTargetKind::Function);
    }
    assert_eq!(facts[0].declaration_span, facts[1].declaration_span);
    assert_eq!(facts[1].declaration_span, facts[2].declaration_span);

    for (fact, module) in facts.iter().zip(["a", "b", "c"]) {
        assert_eq!(
            fact.origin,
            incan_semantics_core::SymbolOrigin::Module(vec![module.to_string()]),
            "each call must name the module it imported from"
        );
    }
    assert_ne!(facts[0], facts[1]);
    assert_ne!(facts[1], facts[2]);
    assert_ne!(facts[0], facts[2]);

    // One declaration reached two ways — locally in `a`, and through `d`'s alias — stays one identity.
    let a_module = build(shared_source, &["a"])?;
    let local = named_targets(&a_module, "use_local");
    let [local] = local.as_slice() else {
        return Err(Box::from("expected one named call in `use_local`".to_string()));
    };
    let Some(local_fact) = &local.canonical else {
        return Err(Box::from("the local call must carry an identity".to_string()));
    };
    assert_eq!(
        *local_fact, facts[0],
        "`a`'s own `make` and `d`'s `make_a` are one declaration"
    );

    // And the seam refuses an identity this module does not own, despite the identical spelling and span.
    assert!(a_module.body_for_canonical_target(&facts[0]).is_some());
    assert!(
        a_module.body_for_canonical_target(&facts[1]).is_none(),
        "`a` must not answer for `b`'s identically-spelled, identically-spanned declaration"
    );
    Ok(())
}

/// A local declaration beside an explicitly aliased import is identified as the *local* declaration.
///
/// RFC 120 rejects an implicit same-spelling replacement of an import. The explicit alias is the valid spelling for
/// keeping both bindings active, and the local call must still carry the local declaration's identity.
#[test]
fn a_local_declaration_beside_an_aliased_import_is_identified_locally() -> Result<(), Box<dyn std::error::Error>> {
    let helpers_source = r#"
pub def render() -> int:
  return 1
"#;
    let app_source = r#"
from helpers import render as imported_render

def render(value: int) -> int:
  return value

def run() -> int:
  return render(7)
"#;
    let app = build_with_imports(app_source, &["app"], &[("helpers", &["helpers"], helpers_source)])?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from(
            "the call bound a local declaration and must carry its identity".to_string(),
        ));
    };

    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["app".to_string()]),
        "the local declaration owns this call; the explicitly aliased import is a separate binding"
    );
    // The two facts on one target must never name different declarations.
    let resolved = app
        .body_for_canonical_target(fact)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("this module owns the declaration and must resolve it"))?;
    assert_eq!(resolved.name, "render");
    assert_eq!(
        Some(&resolved.direct_call_id),
        target.direct_call_id.as_ref(),
        "the canonical identity and the span identity must select one declaration"
    );
    Ok(())
}

/// Bodies do not carry owner-qualified names, so one module can hold a class method `render` and a free function
/// `render`. The consumer seam must separate them by declaration span; matching on the declared name would hand
/// back whichever body came first, silently, for an identity that names the other one.
#[test]
fn the_consumer_seam_separates_same_named_bodies_by_declaration_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
class Canvas:
  def render(self) -> int:
    return 1

def render() -> int:
  return 2

def run() -> int:
  return render()
"#;
    let module = build(source, &["app"])?;

    let same_named: Vec<&bir::Body> = module.bodies.iter().filter(|body| body.name == "render").collect();
    assert_eq!(
        same_named.len(),
        2,
        "this fixture is only meaningful while the module really holds two bodies named `render`"
    );

    let targets = named_targets(&module, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from("the free function call must carry an identity".to_string()));
    };

    let resolved = module
        .body_for_canonical_target(fact)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("the owning module must resolve its own identity"))?;
    assert_eq!(resolved.span, fact.declaration_span);
    assert_eq!(
        resolved.block.stmts.len(),
        module
            .bodies
            .iter()
            .find(|body| body.span == fact.declaration_span)
            .map(|body| body.block.stmts.len())
            .unwrap_or_default()
    );
    // The method body shares the spelling and must not be what the seam returns.
    let method_span = same_named
        .iter()
        .map(|body| body.span)
        .find(|span| *span != fact.declaration_span)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("expected a second, differently-spanned `render`"))?;
    assert_ne!(resolved.span, method_span);
    Ok(())
}

/// A re-export chain longer than one hop, with a rename in the middle. Exercises the recursion in
/// `dependency_member_identity_from` and proves a rename never leaks into `declaration_name`.
#[test]
fn a_renamed_multi_hop_re_export_still_resolves_to_the_original_declaration() -> Result<(), Box<dyn std::error::Error>>
{
    let helpers_source = r#"
pub def render() -> int:
  return 1
"#;
    let inner_source = r#"
from helpers import render as painted
"#;
    let facade_source = r#"
from inner import painted
"#;
    let app_source = r#"
from facade import painted as relayed

def run() -> int:
  return relayed()
"#;
    let app = build_with_imports(
        app_source,
        &["app"],
        &[
            ("helpers", &["helpers"], helpers_source),
            ("inner", &["inner"], inner_source),
            ("facade", &["facade"], facade_source),
        ],
    )?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from(
            "a multi-hop re-export resolves to a declaration and must carry an identity".to_string(),
        ));
    };
    assert_eq!(
        fact.declaration_name, "render",
        "neither `painted` nor `relayed` may become the declared name"
    );
    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["helpers".to_string()]),
        "the origin is the declaring module, not either facade"
    );
    assert_eq!(target.name, "relayed", "the call site keeps its own spelling");
    Ok(())
}

/// The dependency cache key is the flattened, underscore-joined module name, which also names the emitted Rust
/// module and is therefore not injective: the path `pkg.helpers` and a module literally named `pkg_helpers` are
/// one key. Registering the real segments makes the identity name the module that answered rather than the
/// spelling that asked — and note a leading-underscore segment like `pkg._helpers` is exactly what defeats an
/// escaping scheme, which is why the real segments are carried instead of being encoded into one string.
#[test]
fn a_registered_module_path_wins_over_the_matching_candidate_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let helpers_source = r#"
pub def render() -> int:
  return 1
"#;
    let app_source = r#"
from helpers import render

def run() -> int:
  return render()
"#;
    let helpers_tokens = lexer::lex(helpers_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let helpers_program = parser::parse(&helpers_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let tokens = lexer::lex(app_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let module_path = vec!["pkg".to_string(), "app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    // One real module, a single segment literally named `pkg_helpers`, reached from `pkg.app` as sibling
    // `helpers`. Without the registration the matching candidate would spell a `pkg::helpers` that does not exist.
    checker.register_dependency_module_path_segments("pkg_helpers", vec!["pkg_helpers".to_string()]);
    checker
        .check_with_imports(&program, &[("pkg_helpers", &helpers_program)])
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let app = build_body_ir_module_v0(&program, &module_path, checker.type_info());

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from(
            "the import resolves to a declaration and must carry an identity".to_string(),
        ));
    };
    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["pkg_helpers".to_string()]),
        "the origin must be the module that answered, not the candidate spelling that matched its flattened name"
    );
    Ok(())
}

/// Import resolution tries the sibling-relative candidate before the bare one, so the path written at an import is
/// not necessarily the module it bound. An identity built from the written path would name the root module's
/// declaration here — a different function that merely shares the name.
#[test]
fn a_sibling_relative_import_is_owned_by_the_module_resolution_actually_selected()
-> Result<(), Box<dyn std::error::Error>> {
    // Distinguishable by arity: the zero-argument call below only typechecks against the sibling.
    let root_helpers = r#"
pub def render(first: int, second: int) -> int:
  return first + second
"#;
    let sibling_helpers = r#"
pub def render() -> int:
  return 1
"#;
    let app_source = r#"
from helpers import render

def run() -> int:
  return render()
"#;
    let app = build_with_imports(
        app_source,
        &["pkg", "app"],
        &[
            ("helpers", &["helpers"], root_helpers), /* The sibling is genuinely the nested module
                                                      * `pkg.helpers`, not a module named `pkg_helpers`. */
            ("pkg_helpers", &["pkg", "helpers"], sibling_helpers),
        ],
    )?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from(
            "a sibling-relative import resolves to a proven declaration and must carry an identity".to_string(),
        ));
    };

    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["pkg".to_string(), "helpers".to_string()]),
        "the origin must be the module resolution selected, not the path the import spelled"
    );
    assert_ne!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["helpers".to_string()]),
        "naming the written path would collide with the root module's unrelated `render`"
    );
    // Origin alone would still pass if a name-keyed span lookup picked the wrong file's declaration.
    assert_eq!(
        fact.declaration_span,
        HirSourceSpan::new(1, 37),
        "the span must be the sibling's zero-argument declaration, not the root's two-argument one"
    );
    Ok(())
}

/// An imported overload is selected at the call site, so its identity must carry that checked selection through an
/// ordinary import and an alias instead of falling back to the overloaded spelling.
#[test]
fn imported_overload_calls_retain_the_selected_canonical_identity() -> Result<(), Box<dyn std::error::Error>> {
    let helpers_source = r#"
pub def render(value: int) -> int:
  return value

pub def render(value: str) -> int:
  return 1
"#;
    let app_source = r#"
from helpers import render
from helpers import render as draw

def use_imported() -> int:
  return render(2)

def use_alias() -> int:
  return draw(3)
"#;
    let app = build_with_imports(app_source, &["app"], &[("helpers", &["helpers"], helpers_source)])?;

    let helpers = build(helpers_source, &["helpers"])?;
    let overload_spans = helpers
        .bodies
        .iter()
        .filter(|body| body.name == "render")
        .map(|body| body.span)
        .collect::<Vec<_>>();
    let [int_overload_span, str_overload_span] = overload_spans.as_slice() else {
        return Err(format!("expected both helper overload bodies, got {overload_spans:?}").into());
    };
    let mut selected = Vec::new();
    for body in ["use_imported", "use_alias"] {
        let targets = named_targets(&app, body);
        let [target] = targets.as_slice() else {
            return Err(Box::from(format!(
                "expected one named call in `{body}`, got {}",
                targets.len()
            )));
        };
        let canonical = target
            .canonical
            .as_ref()
            .ok_or_else(|| format!("`{body}` must retain the overload selected by typechecking"))?;
        assert_eq!(canonical.declaration_span, *int_overload_span);
        assert_ne!(canonical.declaration_span, *str_overload_span);
        assert_eq!(canonical.declaration_name, "render");
        assert_eq!(canonical.kind, SemanticSourceTargetKind::Function);
        assert_eq!(
            canonical.origin,
            incan_semantics_core::SymbolOrigin::Module(vec!["helpers".to_string()])
        );
        selected.push(canonical.clone());
    }
    assert_eq!(
        selected[0], selected[1],
        "the alias must preserve the selected declaration identity"
    );
    Ok(())
}

/// The consumer seam: an identity resolves to a declaration, or to nothing. It must never be satisfied by a
/// same-named declaration that happens to live in the consuming module.
#[test]
fn a_canonical_identity_resolves_to_its_declaration_only_in_the_owning_module() -> Result<(), Box<dyn std::error::Error>>
{
    let helpers_source = r#"
pub def render() -> int:
  return 1
"#;
    // `app` declares its own same-named `render`, so a seam keyed on the spelling would wrongly match it.
    let app_source = r#"
from helpers import render as draw

def render() -> int:
  return 2

def run() -> int:
  return draw()
"#;
    let helpers = build(helpers_source, &["helpers"])?;
    let app = build_with_imports(app_source, &["app"], &[("helpers", &["helpers"], helpers_source)])?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from("the aliased import must carry an identity".to_string()));
    };

    let owning = helpers
        .body_for_canonical_target(fact)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("owning module must resolve the identity"))?;
    assert_eq!(owning.name, "render");
    assert!(
        app.body_for_canonical_target(fact).is_none(),
        "the consuming module's own same-named `render` must not satisfy an identity it does not own"
    );
    Ok(())
}

/// Two same-name declarations in one module get two identities, because the identity anchors to a declaration
/// span rather than to the spelling. The *spelling* cannot separate overloads; the identity can, and the
/// typechecker's per-call-site overload selection is what tells them apart.
#[test]
fn each_local_overload_gets_its_own_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def render(value: int) -> int:
  return value

def render(value: str) -> int:
  return 1

def use_int() -> int:
  return render(2)

def use_str() -> int:
  return render("x")
"#;
    let module = build(source, &["app"])?;

    let mut facts = Vec::new();
    for body in ["use_int", "use_str"] {
        let targets = named_targets(&module, body);
        let [target] = targets.as_slice() else {
            return Err(Box::from(format!(
                "expected one named call in `{body}`, got {}",
                targets.len()
            )));
        };
        let Some(fact) = &target.canonical else {
            return Err(Box::from(format!(
                "`{body}` selected one overload and must carry its identity"
            )));
        };
        // Refusing to name an overload must not cost the span dispatch, and the two must agree.
        assert!(target.direct_call_id.is_some());
        facts.push(fact.clone());
    }

    assert_ne!(
        facts[0], facts[1],
        "two overloads are two declarations and must not collapse to one identity"
    );
    assert_eq!(facts[0].declaration_name, "render");
    assert_eq!(facts[1].declaration_name, "render");

    // Each identity resolves to the declaration whose signature that call actually selected.
    for fact in &facts {
        let resolved = module
            .body_for_canonical_target(fact)
            .ok_or_else(|| Box::<dyn std::error::Error>::from("this module owns both overloads"))?;
        assert_eq!(resolved.name, "render");
        assert_eq!(resolved.span, fact.declaration_span);
    }
    Ok(())
}

/// Newtype and enum methods lower like any other owner's methods.
///
/// The body count is asserted explicitly so a future regression shows up as a mismatch rather than a silent
/// absence, which is how this gap went unnoticed: nothing failed when these bodies were simply never produced.
#[test]
fn newtype_and_enum_methods_contribute_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Meters = newtype int:
    def value(self) -> int:
        return self.0

    def scale(mut self, factor: int) -> None:
        self.0 = self.0 * factor

enum Signal:
    Idle
    Active(int)

    def code(self) -> int:
        return 0

def run() -> int:
    return 1
"#;
    let module = build(source, &["app"])?;

    let names: Vec<&str> = module.bodies.iter().map(|body| body.name.as_str()).collect();
    assert!(
        names.contains(&"value"),
        "newtype method `value` must lower, got {names:?}"
    );
    assert!(
        names.contains(&"scale"),
        "newtype `mut self` method must lower, got {names:?}"
    );
    assert!(names.contains(&"code"), "enum method `code` must lower, got {names:?}");
    assert!(names.contains(&"run"));
    assert_eq!(
        module.bodies.len(),
        4,
        "one body per method plus the free function, and nothing else: {names:?}"
    );

    // Each method's identity is scoped under its owning declaration, so two owners may share a method name.
    for (owner, method) in [("Meters", "value"), ("Meters", "scale"), ("Signal", "code")] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == method)
            .ok_or_else(|| Box::<dyn std::error::Error>::from(format!("`{method}` body missing")))?;
        assert!(
            body.decl_id.path().ends_with(&format!("{owner}::{method}")),
            "`{method}` must be scoped under `{owner}`, got {}",
            body.decl_id.path()
        );
    }
    Ok(())
}

/// An enum method that dispatches on `self`, including a payload variant, lowers a real body rather than an
/// empty or placeholder one.
#[test]
fn an_enum_method_dispatching_on_self_lowers_its_match() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
    Idle
    Active(int)

    def level(self) -> int:
        match self:
            case Signal.Idle:
                return 0
            case Signal.Active(amount):
                return amount
"#;
    let module = build(source, &["app"])?;

    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "level")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("enum method `level` must lower"))?;

    assert!(!body.block.stmts.is_empty(), "the method must lower a real body");
    // A refused construct also produces statements, so an empty check proves nothing on its own. The snapshot
    // carrying no `unsupported(` node is what distinguishes a lowered match from a placeholder.
    let rendered = module.render_snapshot();
    assert!(
        !rendered.contains("unsupported("),
        "the match and its payload binding must lower, not refuse:\n{rendered}"
    );
    Ok(())
}

/// A newtype method reading its wrapped value lowers through the nominal receiver.
#[test]
fn a_newtype_method_reads_its_wrapped_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Meters = newtype int:
    def doubled(self) -> int:
        return self.0 * 2
"#;
    let module = build(source, &["app"])?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "doubled")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("newtype method `doubled` must lower"))?;
    assert!(!body.block.stmts.is_empty());
    assert!(
        !module.render_snapshot().contains("unsupported("),
        "reading the wrapped value must lower, not refuse"
    );
    assert!(
        body.decl_id.path().ends_with("Meters::doubled"),
        "identity is scoped under the newtype, got {}",
        body.decl_id.path()
    );
    Ok(())
}

/// A newtype method's receiver is the newtype itself, and `mut self` stays a mutable receiver.
#[test]
fn newtype_method_receivers_are_receiver_locals() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Meters = newtype int:
    def value(self) -> int:
        return self.0

    def scale(mut self, factor: int) -> None:
        self.0 = self.0 * factor
"#;
    let module = build(source, &["m", "newtype_receiver"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("local 0 self : Meters [receiver]"), "{snapshot}");
    assert!(snapshot.contains("local 0 self : Meters [receiver_mut]"), "{snapshot}");
    assert!(
        !snapshot.contains("unsupported("),
        "newtype receiver reads and mutation must lower without a placeholder: {snapshot}"
    );
    Ok(())
}

/// An enum method's receiver is the enum value it dispatches on.
#[test]
fn enum_method_receiver_is_a_receiver_local() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
    Idle
    Active(int)

    def code(self) -> int:
        return 0
"#;
    let module = build(source, &["m", "enum_receiver"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("local 0 self : Signal [receiver]"), "{snapshot}");
    Ok(())
}

/// A bodyless newtype or enum method never reaches lowering: the source checker rejects it first.
///
/// Only a trait declaration admits a bodyless method, so the "abstract method contributes nothing" boundary sits
/// upstream for these two kinds rather than in the lowering walk. Lowering still has to stay total for the rejected
/// program, which is what this pins — no body, and no `Unsupported` placeholder standing in for one.
#[test]
fn a_bodyless_newtype_or_enum_method_is_a_source_error_and_lowers_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
    Idle

    def label(self) -> str: ...
"#;
    let (module, errors) = build_after_expected_typecheck_errors(source, &["m", "bodyless"])?;

    assert!(
        errors
            .iter()
            .any(|error| error.contains("must have a body outside trait declarations")),
        "the source checker owns this refusal: {errors:?}"
    );
    assert!(
        module.bodies.is_empty(),
        "a rejected bodyless method must produce neither a body nor a placeholder: {:?}",
        module.bodies.iter().map(|body| body.name.as_str()).collect::<Vec<_>>()
    );
    Ok(())
}

/// Every declaration kind that carries a `methods` field lowers its bodies.
///
/// A skipped kind is the one coverage failure this module cannot make visible — it produces no `Body` at all rather
/// than an `Unsupported` marker — so the exhaustive set is pinned here rather than left to the walk's `_ => ` arm.
#[test]
fn every_declaration_kind_that_carries_methods_lowers_its_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
trait Describable:
    def describe(self) -> int:
        return 0

model Point:
    x: int

    def get_x(self) -> int:
        return self.x

class Counter:
    total: int

    def total_now(self) -> int:
        return self.total

type Meters = newtype int:
    def raw(self) -> int:
        return self.0

enum Signal:
    Idle

    def code(self) -> int:
        return 1
"#;
    let module = build(source, &["m", "all_owners"])?;
    let names: Vec<&str> = module.bodies.iter().map(|body| body.name.as_str()).collect();

    for method in ["describe", "get_x", "total_now", "raw", "code"] {
        assert!(names.contains(&method), "`{method}` must lower, got {names:?}");
    }
    assert_eq!(
        module.bodies.len(),
        5,
        "one body per methods-carrying declaration kind and nothing else: {names:?}"
    );
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

// ============================================================================
// Provider-operation plans (#1213)
// ============================================================================

/// Construct the selected provider plan from the checked declaration metadata produced by `source`.
///
/// This intentionally follows the production direction: the declaration-side decorator resolves a capability,
/// publication persists that checked pair into the provider manifest, and consumer lowering projects the selected
/// manifest through its provider plan. Tests must not manually fill `ProviderOperationCatalog`, because that would
/// bypass the compiler-owned producer contract #1213 adds.
pub(crate) fn provider_plan_from_checked_source(
    type_info: &TypeCheckInfo,
    state: bir::ProviderActivationState,
) -> Result<ProviderPlan, Box<dyn std::error::Error>> {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use crate::frontend::library_manifest_index::LibraryManifestIndex;
    use crate::library_manifest::{CompiledProviderMetadata, LibraryManifest, ProviderOperationMetadata};
    use crate::provider::{NamespaceAuthority, ProviderIdentity, ProviderProvenance, ProviderRecord};

    let operation_descriptors = type_info
        .declarations
        .provider_operations
        .values()
        .map(|operation| ProviderOperationMetadata {
            operation: operation.operation.clone(),
            required_capability: operation.required_capability.clone(),
            runtime_requirements: operation.runtime_requirements.clone(),
        })
        .collect::<Vec<_>>();
    if operation_descriptors.is_empty() {
        return Err("fixture source declared no checked provider operation".into());
    }

    let namespace_claims = operation_descriptors
        .iter()
        .map(|descriptor| {
            descriptor
                .operation
                .module_path()
                .map(ToOwned::to_owned)
                .ok_or("provider operation identity must name a module declaration")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut manifest = LibraryManifest::new("fixture_provider", "0.1.0");
    manifest.contract_metadata.provider = CompiledProviderMetadata {
        operation_descriptors,
        ..CompiledProviderMetadata::default()
    };
    let (enabled, available) = match state {
        bir::ProviderActivationState::Active => (true, true),
        bir::ProviderActivationState::Disabled => (false, true),
        bir::ProviderActivationState::Unavailable => (true, false),
    };
    ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![ProviderRecord {
            identity: ProviderIdentity {
                name: "fixture_provider".to_string(),
                version: "0.1.0".to_string(),
                digest: "fixture:provider-operation".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Compiler,
            authority: NamespaceAuthority::Compiler,
            namespace_claims: namespace_claims.clone(),
            available,
            enabled,
            // The selected metadata remains available for a known-but-unavailable fixture so lowering can issue
            // its explicit source-span refusal instead of treating the operation as an ordinary call.
            manifest: Some(Arc::new(manifest)),
            artifact: None,
            implementation_facets: Vec::new(),
        }],
        namespace_claims,
    )
    .map_err(|error| error.to_string().into())
}

/// Lower `source` through its checked provider manifest and the selected `ProviderPlan`.
fn build_with_provider_operation(
    source: &str,
    module_path: &[&str],
    state: bir::ProviderActivationState,
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let provider_plan = provider_plan_from_checked_source(checker.type_info(), state)?;
    build_body_ir_module_v0_with_provider_plan(&program, &module_path, checker.type_info(), &provider_plan)
        .map_err(Into::into)
}

/// Collect the provider-operation plans lowered into one body, in statement order.
fn provider_plans<'module>(
    module: &'module bir::BodyIrModule,
    body_name: &str,
) -> Vec<&'module bir::ProviderOperationPlan> {
    module
        .bodies
        .iter()
        .filter(|body| body.name == body_name)
        .flat_map(|body| &body.block.stmts)
        .filter_map(|stmt| match &stmt.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::ProviderOperation(plan),
                ..
            } => Some(plan.as_ref()),
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

    let expected: Vec<incan_core::errors::ErrorKind> = assertion_kinds(body)
        .into_iter()
        .filter_map(|kind| match kind {
            bir::AssertionKind::Raises { expected_error, .. } => Some(*expected_error),
            _ => None,
        })
        .collect();
    assert_eq!(
        expected,
        vec![
            incan_core::errors::ErrorKind::ValueError,
            incan_core::errors::ErrorKind::IndexError
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

/// Collect the explicit refusal descriptions recorded in one body, in statement order.
fn refusal_descriptions<'module>(module: &'module bir::BodyIrModule, body_name: &str) -> Vec<&'module str> {
    module
        .bodies
        .iter()
        .filter(|body| body.name == body_name)
        .flat_map(|body| &body.block.stmts)
        .filter_map(|stmt| match &stmt.kind {
            bir::StatementKind::Unsupported { description } => Some(description.as_str()),
            _ => None,
        })
        .collect()
}

const PROVIDER_FIXTURE_SOURCE: &str = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, amount: int) -> int:
  return amount

def run() -> int:
  return charge("acct-1", 250)
"#;

/// An admitted call reaches Body IR as a plan carrying every checked fact #1156 needs to execute it.
#[test]
fn an_admitted_provider_operation_lowers_to_a_checked_execution_plan() -> Result<(), Box<dyn std::error::Error>> {
    let module =
        build_with_provider_operation(PROVIDER_FIXTURE_SOURCE, &["app"], bir::ProviderActivationState::Active)?;

    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from(format!(
            "expected one provider-operation plan, got {}",
            plans.len()
        )));
    };

    assert_eq!(plan.operation.declaration_name, "charge");
    assert_eq!(
        plan.operation.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["app".to_string()]),
        "the plan must name the declaration the call resolved to, not the call-site spelling"
    );
    assert_eq!(plan.provider.state, bir::ProviderActivationState::Active);
    assert_eq!(
        plan.provider.provider_key,
        "fixture_provider@0.1.0#fixture:provider-operation[]"
    );
    assert_eq!(
        plan.required_capability.kind,
        SemanticSourceTargetKind::Capability,
        "the plan's required authority must name a capability declaration"
    );
    assert!(
        plan.runtime_requirements.is_empty(),
        "the first provider-operation contract does not infer runtime requirements from a source spelling"
    );

    // The declaration's own body must still lower, and the plan must name it rather than shadow it.
    let declaration = module
        .body_for_canonical_target(&plan.operation)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("this module owns the operation and must resolve it"))?;
    assert_eq!(declaration.name, "charge");
    Ok(())
}

/// Consumer lowering uses the provider manifest's declaration identity, even though the caller only wrote an import.
#[test]
fn an_imported_provider_operation_lowers_from_the_selected_provider_manifest() -> Result<(), Box<dyn std::error::Error>>
{
    let provider_source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
pub def charge(account: str, amount: int) -> int:
  return amount
"#;
    let consumer_source = r#"
from provider import charge

def run() -> int:
  return charge("acct-1", 250)
"#;
    let provider_tokens = lexer::lex(provider_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let provider_program =
        parser::parse(&provider_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut provider_checker = TypeChecker::new();
    provider_checker.set_current_module_path(Some(vec!["provider".to_string()]));
    provider_checker
        .check_program(&provider_program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let provider_plan =
        provider_plan_from_checked_source(provider_checker.type_info(), bir::ProviderActivationState::Active)?;

    let consumer_tokens = lexer::lex(consumer_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let consumer_program =
        parser::parse(&consumer_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut consumer_checker = TypeChecker::new();
    let consumer_module_path = vec!["app".to_string()];
    consumer_checker.set_current_module_path(Some(consumer_module_path.clone()));
    consumer_checker.register_dependency_module_path_segments("provider", vec!["provider".to_string()]);
    consumer_checker
        .check_with_imports(&consumer_program, &[("provider", &provider_program)])
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    assert!(
        consumer_checker
            .type_info()
            .declarations
            .function_bindings
            .contains_key("charge"),
        "an imported function must retain its checked signature for argument planning"
    );

    let module = build_body_ir_module_v0_with_provider_plan(
        &consumer_program,
        &consumer_module_path,
        consumer_checker.type_info(),
        &provider_plan,
    )?;
    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from(format!(
            "expected one imported provider plan, got {}; lowered body: {}",
            plans.len(),
            module.render_snapshot()
        )));
    };
    assert_eq!(
        plan.operation.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["provider".to_string()]),
        "the plan must preserve the provider declaration identity, not the consumer import spelling"
    );
    assert_eq!(plan.required_capability.declaration_name, "charge_card");
    assert_eq!(plan.provider.module_path, vec!["provider".to_string()]);
    Ok(())
}

/// The plan describes each evaluated input by slot, evaluation order, checked type, and its own span.
#[test]
fn a_plan_records_every_evaluated_input_fact() -> Result<(), Box<dyn std::error::Error>> {
    let module =
        build_with_provider_operation(PROVIDER_FIXTURE_SOURCE, &["app"], bir::ProviderActivationState::Active)?;

    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from("expected one provider-operation plan".to_string()));
    };
    let [account, amount] = plan.inputs.as_slice() else {
        return Err(Box::from(format!(
            "expected two evaluated inputs, got {}",
            plan.inputs.len()
        )));
    };

    assert_eq!((account.slot, account.written_position), (0, 0));
    assert_eq!((amount.slot, amount.written_position), (1, 1));
    assert_eq!(account.ty, IncanType::Primitive(IncanPrimitiveType::Str));
    assert_eq!(amount.ty, IncanType::Primitive(IncanPrimitiveType::Int));
    assert!(
        account.span.start < amount.span.start && amount.span.end <= plan.call_span.end,
        "each input must carry its own argument span inside the call: {:?}",
        plan.inputs
    );
    Ok(())
}

/// Named provider arguments preserve source evaluation order even though their operands are emitted by declaration
/// slot. The plan is the bridge between those two orders, so losing either fact would let a consumer reorder effects
/// or ownership decisions.
#[test]
fn a_provider_plan_preserves_reversed_named_argument_evaluation_and_ownership() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, memo: str) -> int:
  return 1

def first() -> str:
  return "first"

def second() -> str:
  return "second"

def run() -> int:
  return charge(memo=first(), account=second())
"#;
    let module = build_with_provider_operation(source, &["app"], bir::ProviderActivationState::Active)?;
    let body = body_named(&module, "run")?;
    let rendered = body.render_snapshot();
    let first_at = rendered
        .find("call fn:first(")
        .ok_or("missing first argument evaluation")?;
    let second_at = rendered
        .find("call fn:second(")
        .ok_or("missing second argument evaluation")?;
    assert!(
        first_at < second_at,
        "named arguments must evaluate in written source order: {rendered}"
    );

    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from("expected one provider-operation plan".to_string()));
    };
    assert_eq!(
        plan.inputs
            .iter()
            .map(|input| (input.slot, input.written_position))
            .collect::<Vec<_>>(),
        vec![(1, 0), (0, 1)],
        "the plan must retain written order separately from declaration slots: {:?}",
        plan.inputs
    );

    let provider_call = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::ProviderOperation(_),
                args,
                ..
            } => Some(args),
            _ => None,
        })
        .ok_or("missing provider-operation call")?;
    let [
        bir::ArgumentElement::One(bir::Operand::Place(account)),
        bir::ArgumentElement::One(bir::Operand::Place(memo)),
    ] = provider_call.as_slice()
    else {
        return Err(Box::from(format!(
            "expected two declared-slot provider operands, got {provider_call:?}"
        )));
    };
    assert_eq!(
        (account.fact, account.last_use, memo.fact, memo.last_use),
        (bir::OwnershipFact::Move, true, bir::OwnershipFact::Move, true),
        "the declaration-slot operands must retain the ownership facts from their written evaluations"
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

/// The plan phrases the RFC 104 authority question at the invocation, which is what #1156 executes from.
#[test]
fn a_plan_yields_an_authority_request_for_the_invocation() -> Result<(), Box<dyn std::error::Error>> {
    let module =
        build_with_provider_operation(PROVIDER_FIXTURE_SOURCE, &["app"], bir::ProviderActivationState::Active)?;

    let plans = provider_plans(&module, "run");
    let [plan] = plans.as_slice() else {
        return Err(Box::from("expected one provider-operation plan".to_string()));
    };
    let request = plan.authority_request();

    assert_eq!(request.capability, plan.required_capability);
    assert_eq!(request.operation, plan.operation);
    assert_eq!(request.request_span, plan.call_span);
    assert_eq!(request.suggested_grant, "app.charge_card");
    Ok(())
}

/// Invoking an admitted operation preserves the runtime requirements published by its descriptor.
#[test]
fn invoking_a_provider_operation_preserves_its_published_runtime_requirements_on_the_caller()
-> Result<(), Box<dyn std::error::Error>> {
    let module =
        build_with_provider_operation(PROVIDER_FIXTURE_SOURCE, &["app"], bir::ProviderActivationState::Active)?;

    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "run")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("the calling body must lower"))?;

    assert!(
        caller.runtime_requirements.is_empty(),
        "the fixture publishes no runtime requirement, so lowering must not invent one: {:?}",
        caller.runtime_requirements
    );
    Ok(())
}

/// A call the catalog does not admit stays an ordinary named call: nothing may become a plan by spelling alone.
#[test]
fn an_operation_the_catalog_does_not_admit_stays_an_ordinary_call() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(PROVIDER_FIXTURE_SOURCE, &["app"])?;

    assert!(
        provider_plans(&module, "run").is_empty(),
        "an empty catalog must admit nothing"
    );
    assert_eq!(
        named_targets(&module, "run").len(),
        1,
        "the call must still lower normally"
    );
    assert!(refusal_descriptions(&module, "run").is_empty());
    Ok(())
}

/// A disabled provider stops the operation at its source span, with no plan and so nothing to execute.
#[test]
fn an_operation_whose_provider_is_disabled_refuses_before_execution() -> Result<(), Box<dyn std::error::Error>> {
    let module = build_with_provider_operation(
        PROVIDER_FIXTURE_SOURCE,
        &["app"],
        bir::ProviderActivationState::Disabled,
    )?;

    assert!(
        provider_plans(&module, "run").is_empty(),
        "a refused operation must produce no plan, and so no execution and no receipt"
    );
    let refusals = refusal_descriptions(&module, "run");
    assert!(
        refusals.iter().any(|description| description.contains("not enabled")),
        "the refusal must name why the operation cannot run: {refusals:?}"
    );
    Ok(())
}

/// Provider admission is decided before lowering an argument expression. Otherwise a disabled operation could
/// still run source-observable work merely while constructing an execution plan that will never exist.
#[test]
fn a_disabled_provider_refuses_at_the_call_span_before_lowering_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, amount: int) -> int:
  return amount

def side_effect() -> str:
  return "acct-1"

def run() -> int:
  return charge(side_effect(), 250)
"#;
    let module = build_with_provider_operation(source, &["app"], bir::ProviderActivationState::Disabled)?;
    let body = body_named(&module, "run")?;
    let refusal = body
        .block
        .stmts
        .iter()
        .find(|statement| matches!(statement.kind, bir::StatementKind::Unsupported { .. }))
        .ok_or("disabled provider call must lower to an explicit refusal")?;
    let expected_start = source.find("charge(side_effect()").ok_or("fixture call missing")?;
    assert_eq!(
        refusal.span.start, expected_start,
        "the refusal must keep the provider call's source span rather than an argument span"
    );
    assert!(
        named_targets(&module, "run")
            .iter()
            .all(|target| target.name != "side_effect"),
        "argument expressions must not be lowered after provider admission refused: {}",
        body.render_snapshot()
    );
    assert!(provider_plans(&module, "run").is_empty());
    Ok(())
}

/// An unavailable provider is refused separately from a disabled one, because the two have different remedies.
#[test]
fn an_operation_whose_provider_is_unavailable_refuses_with_its_own_reason() -> Result<(), Box<dyn std::error::Error>> {
    let module = build_with_provider_operation(
        PROVIDER_FIXTURE_SOURCE,
        &["app"],
        bir::ProviderActivationState::Unavailable,
    )?;

    assert!(provider_plans(&module, "run").is_empty());
    let refusals = refusal_descriptions(&module, "run");
    assert!(
        refusals
            .iter()
            .any(|description| description.contains("no locally available artifact")),
        "an unavailable provider must not be reported as a disabled one: {refusals:?}"
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

// ---- Bytes literals and range values (#1165) ----

#[test]
fn bytes_literals_lower_to_their_own_constant_rather_than_a_string() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def send(payload: bytes) -> int:\n",
        "  return 1\n",
        "\n",
        "def keep() -> bytes:\n",
        "  greeting = b\"hi\"\n",
        "  return greeting\n",
        "\n",
        "def run() -> int:\n",
        "  return send(b\"\\x00\\xff\")\n",
    );
    let module = build(source, &["m", "bytes_literal"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a byte-string literal must lower to a real constant: {snapshot}"
    );
    assert!(
        snapshot.contains("const(b\"\\x68\\x69\")"),
        "a bound byte-string literal must render as its own bytes constant: {snapshot}"
    );
    assert!(
        !snapshot.contains("const(\"hi\")"),
        "a byte-string literal must never be represented as the string constant it is not: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:send(const(b\"\\x00\\xff\"))"),
        "a byte-string literal must survive as a constant in argument position: {snapshot}"
    );
    assert!(
        snapshot.contains(" greeting : bytes [binding]"),
        "the bound local must keep its checked `bytes` type: {snapshot}"
    );

    // The owned-buffer representation is what makes this read a move rather than a copy: `bytes` reports
    // `AbiV0Ownership::Owned`, so its last read transfers ownership exactly as a `str` local's would.
    let greeting = local_for_binding(&snapshot, "greeting").ok_or("expected a local for `greeting`")?;
    assert!(
        snapshot.contains(&format!("return move({greeting}, last_use)")),
        "the last read of an owned bytes local must be a move: {snapshot}"
    );
    Ok(())
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

/// Corrupt provider metadata whose authority is not a capability is rejected before lowering can mint a plan.
#[test]
fn corrupt_provider_metadata_with_a_non_capability_authority_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(PROVIDER_FIXTURE_SOURCE).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut provider_plan =
        provider_plan_from_checked_source(checker.type_info(), bir::ProviderActivationState::Active)?;
    let provider = provider_plan
        .records()
        .next()
        .ok_or("fixture provider plan must have one record")?;
    let mut manifest = provider
        .manifest
        .as_deref()
        .cloned()
        .ok_or("fixture provider must have a manifest")?;
    manifest.contract_metadata.provider.operation_descriptors[0]
        .required_capability
        .kind = SemanticSourceTargetKind::Function;

    use std::collections::BTreeSet;
    use std::sync::Arc;

    use crate::frontend::library_manifest_index::LibraryManifestIndex;
    use crate::provider::{NamespaceAuthority, ProviderIdentity, ProviderProvenance, ProviderRecord};

    provider_plan = ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![ProviderRecord {
            identity: ProviderIdentity {
                name: "fixture_provider".to_string(),
                version: "0.1.0".to_string(),
                digest: "fixture:provider-operation".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Compiler,
            authority: NamespaceAuthority::Compiler,
            namespace_claims: BTreeSet::from([vec!["app".to_string()]]),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(manifest)),
            artifact: None,
            implementation_facets: Vec::new(),
        }],
        [vec!["app".to_string()]],
    )?;
    let error = ProviderOperationCatalog::from_provider_plan(&provider_plan)
        .expect_err("corrupt metadata must not create a lowering catalog");
    assert!(
        error.contains("unsupported declaration kinds"),
        "unexpected error: {error}"
    );
    Ok(())
}

/// A selected provider cannot publish one callable identity twice. Refusing at plan projection makes the result
/// independent of manifest traversal order and prevents a Body-IR call from silently choosing one provider record.
#[test]
fn duplicate_provider_operation_identities_refuse_before_body_ir_building() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use crate::frontend::library_manifest_index::LibraryManifestIndex;

    let tokens = lexer::lex(PROVIDER_FIXTURE_SOURCE).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let provider_plan = provider_plan_from_checked_source(checker.type_info(), bir::ProviderActivationState::Active)?;
    let mut provider = provider_plan
        .records()
        .next()
        .cloned()
        .ok_or("fixture provider plan must have one provider")?;
    let manifest = Arc::make_mut(
        provider
            .manifest
            .as_mut()
            .ok_or("fixture provider must have a manifest")?,
    );
    let duplicate = manifest.contract_metadata.provider.operation_descriptors[0].clone();
    manifest
        .contract_metadata
        .provider
        .operation_descriptors
        .push(duplicate);
    let namespace_claims = provider.namespace_claims.clone();
    let colliding_plan = ProviderPlan::new(LibraryManifestIndex::default(), vec![provider], namespace_claims)?;

    let error =
        build_body_ir_module_v0_with_provider_plan(&program, &module_path, checker.type_info(), &colliding_plan)
            .expect_err("a duplicate canonical operation identity must not pick one manifest entry");
    assert!(
        error.contains("duplicate provider operation identity `charge`"),
        "unexpected collision refusal: {error}"
    );
    Ok(())
}

/// An omitted parameter has no evaluated input, so the plan cannot honestly describe what would execute.
#[test]
fn an_operation_called_with_an_omitted_default_refuses() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, amount: int = 1) -> int:
  return amount

def run() -> int:
  return charge("acct-1")
"#;
    let module = build_with_provider_operation(source, &["app"], bir::ProviderActivationState::Active)?;

    assert!(provider_plans(&module, "run").is_empty());
    let refusals = refusal_descriptions(&module, "run");
    assert!(
        refusals
            .iter()
            .any(|description| description.contains("omitted parameter")),
        "unexpected refusal: {refusals:?}"
    );
    Ok(())
}

/// A callee the typechecker never resolved has no identity, so it cannot become a plan by sharing a spelling.
///
/// The catalog holds a real entry throughout, and the call site writes a name the module does not declare. Nothing
/// about the two spellings is allowed to bring them together: admission runs on canonical identity, and an
/// unresolved callee has none.
///
/// The refusal here is genuinely *source-owned*: the typechecker rejects the program before lowering runs. Body IR
/// deliberately does not raise a second diagnostic for the same gap — see
/// [`BodyBuilder::declared_slots_for_direct_call`], which keeps the ordinary named-call representation with no proven
/// identity so the executor refuses it at the original call span. What this pins is that no plan is minted, so nothing
/// exists that could execute or report.
#[test]
fn an_unresolved_operation_refuses_at_its_source_span_and_produces_no_plan() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
capability charge_card:
  description = "Charge one approved card"

@provider_operation(charge_card)
def charge(account: str, amount: int) -> int:
  return amount

def run() -> int:
  return charge_now("acct-1", 250)
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    let diagnostics = checker
        .check_program(&program)
        .err()
        .ok_or("the fixture calls an undeclared name and must be rejected by the source checker")?;
    assert!(!diagnostics.is_empty(), "the refusal must be source-owned first");

    let provider_plan = provider_plan_from_checked_source(checker.type_info(), bir::ProviderActivationState::Active)?;
    let module =
        build_body_ir_module_v0_with_provider_plan(&program, &module_path, checker.type_info(), &provider_plan)?;

    assert!(
        provider_plans(&module, "run").is_empty(),
        "an unresolved callee must never be admitted as an operation the catalog holds under another identity"
    );
    let targets = named_targets(&module, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    assert!(
        target.canonical.is_none() && target.direct_call_id.is_none(),
        "an unresolved callee must carry no identity for anything downstream to admit it by"
    );
    Ok(())
}

// ============================================================================
// Statement-position `loop:` and the `unsafe:` boundary (#1162)
// ============================================================================

/// The indentation `render_block` gives a statement nested one block below a body's top-level statements.
///
/// A body renders its own block at depth 1, so a top-level statement carries two spaces and anything inside that
/// statement's nested block carries four. Tests that care about *where* a statement landed compare against this
/// rather than merely finding the text somewhere in the body, which would also pass if the statement had escaped
/// into the enclosing block.
/// The label fragment every by-design refusal used as a test stand-in renders with.
///
/// Tests that need *some* refusal in a given position -- to prove a refusal stays nested inside the construct
/// containing it, say -- must not reach for whichever construct happens to be unrepresentable that week. Four have
/// now gone vacuous exactly that way: `value ** 2` (#1160), a byte-string literal (#1165), a callable default
/// (#1240), and collection membership (#1246, twice over). Each was a *gap*, and #1101 exists to close gaps, so
/// every such choice decays the moment its owning sibling lands.
///
/// `unsafe:` is categorically different. It is refused because Body IR v0 cannot carry the acknowledgement a
/// consumer would need to admit the region deliberately -- a stated disposition, not pending work. Reversing it
/// means designing the acknowledgement fact first, so it will not quietly become representable underneath a test.
///
/// If [`the_shared_stand_in_refusal_is_still_refused_by_design`] ever fails, every test below that uses this has
/// gone vacuous: pick another *by-design* refusal, update these two items, and do not substitute a gap.
const STAND_IN_REFUSAL_LABEL: &str = "`unsafe:` acknowledgement region";

/// Source lines for the shared stand-in refusal, indented to sit inside an enclosing block.
fn stand_in_refusal_stmt(indent: &str) -> String {
    format!("{indent}unsafe:\n{indent}  pass\n")
}

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

// ---- Body IR input contract (#1166) ----

/// Parse `source`, project it through `active_features`, then typecheck and lower **the projection**.
///
/// This is the shape [`build_body_ir_module_v0`]'s input contract requires of every caller: the checker and the
/// lowering both see the feature-projected program, never the full parse tree. [`build`] deliberately skips the
/// projection step, so the two helpers together show what projection is worth rather than only asserting it ran.
fn build_projected(
    source: &str,
    module_path: &[&str],
    active_features: &[&str],
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let parsed = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let active = active_features
        .iter()
        .map(|feature| (*feature).to_string())
        .collect::<std::collections::BTreeSet<String>>();
    let program = parsed.projected_for_features(&active);
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Lower `source` after appending `injected` to its first top-level function body, **after** typechecking.
///
/// Vocab and scoped-DSL nodes cannot arrive through [`parser::parse`]: they need a library vocabulary that an
/// import activates, and every pipeline that has a desugar pass removes them before lowering. Splicing the node in
/// after the checker has run is therefore the only way to reach Body IR's input-contract safety net, and the state
/// it produces is exactly the one a caller that skipped the desugar pass would hand over.
fn build_with_statement_injected_after_typecheck(
    source: &str,
    module_path: &[&str],
    injected: ast::Statement,
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let function = program
        .declarations
        .iter_mut()
        .find_map(|decl| match &mut decl.node {
            ast::Declaration::Function(function) => Some(function),
            _ => None,
        })
        .ok_or("expected a top-level function to inject the undesugared node into")?;
    function.body.push(ast::Spanned::new(injected, ast::Span::new(0, 1)));

    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Lower `source` after adding a raw top-level declaration, **after** typechecking.
///
/// The source parser only builds vocabulary declarations after an imported vocabulary is active, and a healthy
/// compiler desugars them before typechecking. Adding one after checking isolates the lowerer's final safety net:
/// every executable body must refuse the raw declaration rather than silently dropping it during top-level
/// collection.
fn build_with_top_level_declaration_injected_after_typecheck(
    source: &str,
    module_path: &[&str],
    injected: ast::Spanned<ast::Declaration>,
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    program.declarations.push(injected);
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Every `Unsupported` refusal description in `body_name`'s top-level statement list.
fn unsupported_descriptions(module: &bir::BodyIrModule, body_name: &str) -> Vec<String> {
    module
        .bodies
        .iter()
        .filter(|body| body.name == body_name)
        .flat_map(|body| &body.block.stmts)
        .filter_map(|stmt| match &stmt.kind {
            bir::StatementKind::Unsupported { description } => Some(description.clone()),
            _ => None,
        })
        .collect()
}

/// A scoped-DSL surface owner naming a library that this compilation never loaded.
fn fixture_scoped_surface_owner() -> ast::ScopedSurfaceOwner {
    ast::ScopedSurfaceOwner {
        declaration: "query".to_string(),
        clause: None,
        call: None,
    }
}

/// A top-level vocabulary declaration whose source meaning has not been desugared.
fn fixture_top_level_vocab_declaration() -> ast::Spanned<ast::Declaration> {
    ast::Spanned::new(
        ast::Declaration::VocabBlock(ast::VocabBlockStmt {
            keyword: "query".to_string(),
            keyword_binding: ast::VocabKeywordBinding {
                is_declaration_owned_clause: false,
                dependency_key: "demo.query".to_string(),
                activation_namespace: "demo".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::FunctionDecl,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
                clause_body_kind: None,
            },
            decorators: Vec::new(),
            signature_head: None,
            header_args: Vec::new(),
            body: Vec::new(),
            body_item_trailing_commas: Vec::new(),
        }),
        ast::Span::new(40, 60),
    )
}

#[test]
fn an_undesugared_top_level_vocab_declaration_refuses_every_executable_body() -> Result<(), Box<dyn std::error::Error>>
{
    let module = build_with_top_level_declaration_injected_after_typecheck(
        "def main() -> int:\n    return 1\n\ndef helper() -> int:\n    return 2\n",
        &["m"],
        fixture_top_level_vocab_declaration(),
    )?;

    for body in &module.bodies {
        let Some(statement) = body.block.stmts.first() else {
            return Err(Box::from(format!("expected a contract refusal in `{}`", body.name)));
        };
        let bir::StatementKind::Unsupported { description } = &statement.kind else {
            return Err(Box::from(format!(
                "the first statement of `{}` must reject the raw top-level declaration: {statement:?}",
                body.name
            )));
        };
        assert!(description.contains("top-level vocab block"), "{description}");
        assert!(
            description.contains("Body IR input-contract violation"),
            "{description}"
        );
        assert_eq!(statement.span, HirSourceSpan::new(40, 60));
    }

    let error = crate::backend::replacement::prepare_free_function_execution(&module, "main", &[])
        .err()
        .ok_or("a raw top-level vocabulary declaration must stop direct-execution preparation")?;
    let crate::backend::replacement::ReplacementExecutionError::Unsupported { description, span, .. } = error else {
        return Err(Box::from(format!("unexpected direct-execution result: {error}")));
    };
    assert!(description.contains("top-level vocab block"), "{description}");
    assert_eq!(span, HirSourceSpan::new(40, 60));
    Ok(())
}

#[test]
fn an_undesugared_vocab_expression_item_is_refused_as_a_caller_contract_violation()
-> Result<(), Box<dyn std::error::Error>> {
    // A vocab expression item is body content of a raw `vocab:` block. The desugar pass owns what it means, so one
    // reaching lowering is a broken caller rather than a construct Body IR has yet to model, and the refusal has to
    // say which of the two it is.
    let module = build_with_statement_injected_after_typecheck(
        "def run() -> int:\n    return 1\n",
        &["m"],
        ast::Statement::VocabExpressionItem(ast::VocabExpressionItemStmt {
            expr: ast::Spanned::new(ast::Expr::Literal(ast::Literal::Bool(true)), ast::Span::new(0, 1)),
            alias: None,
            modifiers: Vec::new(),
        }),
    )?;

    let descriptions = unsupported_descriptions(&module, "run");
    let [description] = descriptions.as_slice() else {
        return Err(Box::from(format!("expected exactly one refusal, got {descriptions:?}")));
    };
    assert!(
        description.contains("vocab expression item"),
        "the refusal must still name the node a program actually hit: {description}"
    );
    assert!(
        description.contains("Body IR input-contract violation") && description.contains("desugar pass"),
        "a vocab node must read as a caller contract violation, not an unmodeled construct: {description}"
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

#[test]
fn an_undesugared_scoped_dsl_symbol_call_is_refused_as_a_caller_contract_violation()
-> Result<(), Box<dyn std::error::Error>> {
    // `sum(value)` is one of `ScopedDslSurfaces`' canonical forms. Reaching lowering as surface syntax means no
    // desugarer ever gave it a meaning, so the backend must not invent one -- and must not report the absence as a
    // language gap either.
    let module = build_with_statement_injected_after_typecheck(
        "def run() -> int:\n    return 1\n",
        &["m"],
        ast::Statement::Expr(ast::Spanned::new(
            ast::Expr::Surface(Box::new(ast::SurfaceExpr {
                key: SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key: "demo.query".to_string(),
                    descriptor_key: "aggregate".to_string(),
                },
                payload: ast::SurfaceExprPayload::ScopedSymbolCall {
                    symbol: "sum".to_string(),
                    args: Vec::new(),
                    owner: fixture_scoped_surface_owner(),
                },
            })),
            ast::Span::new(0, 1),
        )),
    )?;

    let descriptions = unsupported_descriptions(&module, "run");
    let [description] = descriptions.as_slice() else {
        return Err(Box::from(format!("expected exactly one refusal, got {descriptions:?}")));
    };
    assert!(
        description.contains("scoped DSL symbol call"),
        "the refusal must name the scoped-DSL form rather than the bare label `expression`: {description}"
    );
    assert!(
        description.contains("Body IR input-contract violation"),
        "a scoped-DSL node must read as a caller contract violation: {description}"
    );
    Ok(())
}

#[test]
fn an_unsafe_region_refuses_under_a_named_permanent_boundary() -> Result<(), Box<dyn std::error::Error>> {
    // #1162's second half. The refusal is a decided disposition, not a missing dispatch arm: an `unsafe:` region
    // introduces no Incan scope, so inlining its statements would be trivial -- and would erase the
    // acknowledgement the region exists to record.
    let source = concat!(
        "def probe(x: int) -> int:\n",
        "  return x\n",
        "\n",
        "def touch(value: int) -> int:\n",
        "  mut total = 0\n",
        "  unsafe:\n",
        "    total = probe(value)\n",
        "  return total\n",
    );
    let module = build(source, &["m", "unsafe_region"])?;
    let snapshot = body_named(&module, "touch")?.render_snapshot();

    assert!(
        snapshot.contains("unsupported(`unsafe:` acknowledgement region:"),
        "the refusal must name the construct rather than reading as a generic placeholder: {snapshot}"
    );
    assert!(
        snapshot.contains("refused by design") && snapshot.contains("#1162"),
        "the refusal must state that it is a decided boundary and name its owner: {snapshot}"
    );
    // The region's own statements must not quietly become statements of the enclosing block, which is the
    // silent-execution outcome the refusal exists to prevent.
    assert!(
        !snapshot.contains("probe"),
        "an acknowledged region's statements must not be inlined into the enclosing block: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_soft_keyword_surface_statement_stays_an_unmodeled_construct_rather_than_a_contract_violation()
-> Result<(), Box<dyn std::error::Error>> {
    // `SurfaceStmtPayload::KeywordArgs` carries both a library's scoped-DSL statement and the stdlib-registered
    // soft keywords (`assert` today). Only the first is a pipeline fault; blaming the caller for the second would
    // send its author looking for a desugar pass that was never supposed to run.
    let module = build_with_statement_injected_after_typecheck(
        "def run() -> int:\n    return 1\n",
        &["m"],
        ast::Statement::Surface(ast::SurfaceStmt {
            key: SurfaceFeatureKey::SoftKeyword(KeywordId::Assert),
            payload: ast::SurfaceStmtPayload::KeywordArgs(vec![ast::Spanned::new(
                ast::Expr::Literal(ast::Literal::Bool(true)),
                ast::Span::new(0, 1),
            )]),
        }),
    )?;

    let descriptions = unsupported_descriptions(&module, "run");
    let [description] = descriptions.as_slice() else {
        return Err(Box::from(format!("expected exactly one refusal, got {descriptions:?}")));
    };
    assert!(
        description.contains("assert"),
        "a soft-keyword surface statement must name its keyword: {description}"
    );
    assert!(
        !description.contains("input-contract violation"),
        "real language surface must not be reported as a caller contract violation: {description}"
    );
    Ok(())
}

#[test]
fn a_body_behind_an_inactive_feature_is_not_lowered() -> Result<(), Box<dyn std::error::Error>> {
    // Feature projection is part of the input contract, not an optimization. A body the compilation does not
    // contain must produce no `bir::Body` at all -- not an empty one, and not one an executor could be asked to run.
    let source =
        "when feature(\"beta\"):\n    def gated() -> int:\n        return 7\n\ndef always() -> int:\n    return 1\n";

    let projected = build_projected(source, &["m"], &[])?;
    let lowered: Vec<&str> = projected.bodies.iter().map(|body| body.name.as_str()).collect();
    assert_eq!(
        lowered,
        ["always"],
        "a declaration behind an inactive feature must not reach lowering"
    );

    // The same program without the projection step does lower it, which is what makes the step load-bearing rather
    // than incidentally satisfied by this fixture.
    let unprojected = build(source, &["m"])?;
    assert!(
        unprojected.bodies.iter().any(|body| body.name == "gated"),
        "the fixture must actually carry a gated body, or the assertion above proves nothing"
    );
    Ok(())
}

#[test]
fn projection_through_an_active_feature_lowers_exactly_as_an_ungated_program_does()
-> Result<(), Box<dyn std::error::Error>> {
    // The other half of the contract: projection removes inactive declarations and changes nothing else. With the
    // feature active the projected program and the raw parse tree must lower identically, spans included, so a
    // caller that applies the contract cannot silently perturb a body that was always part of the compilation.
    let source =
        "when feature(\"beta\"):\n    def gated() -> int:\n        return 7\n\ndef always() -> int:\n    return 1\n";

    let active = build_projected(source, &["m"], &["beta"])?;
    let unprojected = build(source, &["m"])?;
    assert_eq!(
        active.render_snapshot(),
        unprojected.render_snapshot(),
        "an active feature must make projection a no-op"
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
    // Its type is `?` here, and that is deliberately *not* asserted as a defect of this change: an equivalent
    // statement `match` over the same alternated constructor pattern produces exactly the same `?`. This lowering
    // reuses `lower_match_pattern` rather than reimplementing it, so it inherits that gap rather than widening it.
    // Narrowing the generic constructor path's field types belongs with the same #1101 work that leaves
    // `assert o is Some(v)` typed `?`.
    Ok(())
}

// ---- #1072: preserve the typechecker's lexical assignment decision in Body IR ----

#[test]
fn plain_assignment_reuses_its_active_body_ir_local() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def run() -> int:\n  mut x = 1\n  x = 2\n  return x\n",
        &["m", "plain_reassign"],
    )?;
    let body = body_named(&module, "run")?;
    let names: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("x"))
        .collect();

    assert_eq!(
        names.len(),
        1,
        "plain reassignment must write the original local rather than declare a duplicate: {}",
        module.render_snapshot()
    );
    Ok(())
}

#[test]
fn branch_shadowing_does_not_leak_into_following_body_ir_reads() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def run() -> int:\n  let x = 1\n  if true:\n    let x = 2\n  return x\n",
        &["m", "branch_shadow"],
    )?;
    let body = body_named(&module, "run")?;
    let x_locals: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("x"))
        .collect();
    let outer = x_locals.first().ok_or("fixture must declare an outer `x`")?.id;
    assert_eq!(x_locals.len(), 2, "the explicit `let` shadow must retain both locals");

    let returned_local = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Return {
                value: Some(bir::Operand::Place(operand)),
            } => operand.place.local_id(),
            _ => None,
        })
        .ok_or("fixture must return a local place")?;
    assert_eq!(
        returned_local, outer,
        "a read after the branch must resolve to the enclosing local, not the branch-only shadow"
    );
    Ok(())
}

#[test]
fn plain_multi_target_assignment_reuses_active_body_ir_locals() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def run() -> int:\n  mut left = 0\n  mut right = 0\n  left, right = (1, 2)\n  return left + right\n",
        &["m", "plain_multi_reassign"],
    )?;
    let body = body_named(&module, "run")?;
    for name in ["left", "right"] {
        let count = body
            .locals
            .iter()
            .filter(|local| local.name.as_deref() == Some(name))
            .count();
        assert_eq!(
            count,
            1,
            "plain multi-target assignment must reuse `{name}` rather than create a duplicate: {}",
            module.render_snapshot()
        );
    }
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
