//! A local function called from inside a closure must emit the identity it emits anywhere else.
//!
//! See #1492. The trigger is narrower than the issue states, and the narrow case is the one with no coverage: it
//! is not closures generally, and it is not emission. A closure bound to a local, passed to a stdlib combinator,
//! capturing an enclosing binding, or passed to a user-defined method taking a `Callable` all resolve correctly.
//! The callee's canonical path only goes missing when the closure is an argument to a **Rust-interop** method,
//! which is a different lowering path.
//!
//! The emitter is not at fault. `emit_call_expr` takes the canonical-path branch whenever the fact is present and
//! falls through to `emit_expr` when it is absent, so a bare name is a missing fact rather than a wrong choice.
//! Patching that fallback would hide the absence instead of supplying the fact.

use incan_emit::IrCodegen;
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};

/// The one local declaration these fixtures call, so the assertions need no hard-coded mangled form.
const DECLARATION: &str = "describe";

/// Parse, check and generate, returning the emitted Rust.
fn generated(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| std::io::Error::other(format!("generation failed: {error}")).into())
}

/// A closure argument to a Rust-interop method keeps its callee's resolved identity.
///
/// Asserted as agreement between two call sites in one function rather than against a literal mangled string,
/// which would make this a change detector for RFC 120's projection format. What must hold is that the direct
/// call and the call inside the closure name the same thing: the bare spelling is not declared anywhere in the
/// generated Rust, so emitting it does not merely look wrong, it fails to compile.
#[test]
fn a_closure_argument_to_a_rust_method_keeps_its_callee_identity_issue1492() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "from rust::std::option import Option as RustOption\n",
        "\n",
        "def describe(code: int) -> str:\n",
        "    return f\"code {code}\"\n",
        "\n",
        "pub def run(value: RustOption[int]) -> RustOption[str]:\n",
        "    direct = describe(1)\n",
        "    println(direct)\n",
        "    return value.map((code) => describe(code))\n",
    );
    let rust = generated(source)?;

    let projected = rust
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .find(|token| token.starts_with("__incan_v1_") && rust.matches(token).count() > 1)
        .ok_or("the direct call should emit a projected identity to compare against")?
        .to_string();

    assert_eq!(
        rust.matches(&format!("{DECLARATION}(")).count(),
        0,
        "`{DECLARATION}` was called by its bare source name, which is not declared in the generated Rust.\n\
         The direct call in the same function resolved to `{projected}`, so the two sites disagree.\n\n{rust}"
    );
    // The closure site must name the same projection as the direct call, not merely some projected identity: the
    // closure body is the text between `map(` and the end of the function, and the declaration's projection has
    // to appear inside it.
    let closure_body = rust
        .split_once(".map(")
        .map(|(_, rest)| rest)
        .ok_or("the closure should be emitted as a `map` argument")?;
    assert!(
        closure_body.contains(&projected),
        "the closure site must call `{projected}`, the identity the direct call resolved to:\n{closure_body}"
    );
    Ok(())
}

/// The shape the issue reports: a closure handed to a method of a type imported from a Rust crate that carries no
/// inspection metadata, capturing an enclosing local, beside a direct call of the same function.
///
/// The `rust::std` case above proves the fact survives the deferral; this one proves it on the path the original
/// bake took. With no metadata the receiver's method signature is never selected, so the closure is never checked
/// again contextually -- the deferred check is the only pass that records the callee's identity, and lowering has
/// to find it there. A capture is included because the closure's environment is what distinguishes a closure body
/// from a plain nested call. Both sites must name one projection, and no site may emit the bare source name.
#[test]
fn a_closure_argument_to_a_metadata_free_crate_method_keeps_its_callee_identity_issue1492()
-> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "from rust::cfg_expr import Expression, Predicate\n",
        "\n",
        "model CfgSnapshot:\n",
        "    target: str\n",
        "\n",
        "def cfg_predicate(predicate: &Predicate, snapshot: CfgSnapshot) -> Option[bool]:\n",
        "    return Some(true)\n",
        "\n",
        "pub def evaluate(expression: Expression, predicate: &Predicate, snapshot: CfgSnapshot) -> Option[bool]:\n",
        "    if cfg_predicate(predicate, snapshot) == None:\n",
        "        return None\n",
        "    return expression.eval((predicate) => cfg_predicate(predicate, snapshot))\n",
    );
    let rust = generated(source)?;

    let projected = rust
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|token| token.starts_with("__incan_v1_"))
        .find(|token| rust.matches(token).count() > 1)
        .ok_or("the direct call should emit a projected identity to compare against")?
        .to_string();
    assert_eq!(
        rust.matches("cfg_predicate(").count(),
        0,
        "`cfg_predicate` was called by its bare source name inside the closure:\n{rust}"
    );
    let closure_body = rust
        .split_once(".eval(")
        .map(|(_, rest)| rest)
        .ok_or("the closure should be emitted as an `eval` argument")?;
    assert!(
        closure_body.contains(&projected),
        "the closure site must call `{projected}`, the identity the direct call resolved to:\n{closure_body}"
    );
    Ok(())
}

/// The shapes that already worked must keep working, so the fix cannot be a blanket change to closure lowering.
///
/// Each of these resolves correctly today. They are pinned because the natural over-broad repair — forcing a
/// canonical path onto every call inside every closure — would also reach them, and a regression here would
/// otherwise be invisible.
#[test]
fn closure_shapes_that_already_resolved_are_unchanged_issue1492() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "bound to a local",
            concat!(
                "def describe(code: int) -> str:\n",
                "    return f\"code {code}\"\n",
                "\n",
                "pub def run(seed: int) -> str:\n",
                "    f = (code) => describe(code)\n",
                "    return f(seed)\n",
            ),
        ),
        (
            "argument to a user-defined method",
            concat!(
                "def describe(code: int) -> str:\n",
                "    return f\"code {code}\"\n",
                "\n",
                "model Expression:\n",
                "    seed: int\n",
                "\n",
                "    def eval(self, render: Callable[int, str]) -> str:\n",
                "        return render(self.seed)\n",
                "\n",
                "pub def run(expression: Expression) -> str:\n",
                "    return expression.eval((code) => describe(code))\n",
            ),
        ),
    ];
    for (name, source) in cases {
        let rust = generated(source)?;
        assert_eq!(
            rust.matches(&format!("{DECLARATION}(")).count(),
            0,
            "the `{name}` shape regressed: `{DECLARATION}` emitted its bare source name\n\n{rust}"
        );
    }
    Ok(())
}

/// A string literal passed to a generic stdlib method converts to the parameter's declared type.
///
/// See #1494. `list[str].append` already converts, because `CollectionMethodKind::Append` extracts the element
/// type and emits through `ValueUseSite::CollectionElement`. `Deque` is Incan-authored, so its `append` is an
/// ordinary call whose declared parameter drives the same `str` to `String` conversion.
///
/// This in-process check takes the module path (`SymbolOrigin::Module`) and passed while the packaged path still
/// failed: the defect was upstream, in `resolve_type_index_expression` dropping the type application's argument so
/// the receiver reached emission as `Deque[Unknown]` (fixed in #1529). The real-build cover for the packaged path is
/// `string_literals_reach_collection_methods_owned_issue1494` in `cli_language_regression_tests`.
#[test]
fn a_string_literal_converts_for_a_generic_stdlib_method_issue1494() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "from std.collections import Deque\n",
        "\n",
        "pub def probe(requested: list[str]) -> None:\n",
        "    mut pending = Deque[str].from_iter(requested)\n",
        "    pending.append(\"default\")\n",
    );
    let rust = generated(source)?;
    let appended = rust
        .lines()
        .find(|line| line.contains("\"default\""))
        .map(str::trim)
        .ok_or("the literal should appear in the generated Rust")?;
    assert!(
        appended.contains("to_string") || appended.contains("String::from") || appended.contains("into()"),
        "the literal reached a `String` parameter unconverted, so the generated Rust will not compile:\n  \
         {appended}\n\n{rust}"
    );
    Ok(())
}
