//! Focused checked-identity and Body-IR coverage for the selected #1256 string helpers.

use incan::frontend::ast;
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_core::lang::surface::string_methods::StringMethodId;
use incan_semantics_core::body_ir::{BodyIrModule, StatementKind};

const SELECTED_STRING_HELPERS_SOURCE: &str = r#"
def selected(text: str, csv_line: str, names: list[str], padded: str, sentence: str) -> None:
  upper = text.upper()
  lower = text.lower()
  split = csv_line.split(",")
  joined = ", ".join(names)
  stripped = padded.strip()
  contains = sentence.contains("quick")
  replaced = sentence.replace("fox", "dog")
"#;

/// Parse and type-check a self-contained source module for a focused Body-IR assertion.
fn checked_source(source: &str) -> Result<(ast::Program, TypeChecker, Vec<String>), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["checked_string_helpers".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok((program, checker, module_path))
}

/// Type-check one self-contained selected-helper fixture while retaining its checked facts after errors.
fn checked_source_with_error_messages(source: &str) -> Result<(TypeChecker, Vec<String>), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["checked_string_helpers".to_string()]));
    let errors = match checker.check_program(&program) {
        Ok(()) => Vec::new(),
        Err(errors) => errors.into_iter().map(|error| error.message).collect(),
    };
    Ok((checker, errors))
}

/// Return the full source span of a unique method-call expression in one fixture.
fn call_span(source: &str, call: &str) -> Result<ast::Span, Box<dyn std::error::Error>> {
    let start = source
        .find(call)
        .ok_or_else(|| format!("fixture is missing `{call}`"))?;
    Ok(ast::Span::new(start, start + call.len()))
}

/// Return one named Body-IR body, retaining its executable statement spans.
fn body_named<'module>(
    module: &'module BodyIrModule,
    name: &str,
) -> Result<&'module incan_semantics_core::body_ir::Body, Box<dyn std::error::Error>> {
    module
        .bodies
        .iter()
        .find(|body| body.name == name)
        .ok_or_else(|| format!("lowered module has no body named `{name}`").into())
}

/// Assert that one failed-closed helper lowering remains attached to its source call span.
fn assert_refusal_at_call_span(
    module: &BodyIrModule,
    body_name: &str,
    call_span: ast::Span,
) -> Result<(), Box<dyn std::error::Error>> {
    let refusal = body_named(module, body_name)?
        .block
        .stmts
        .iter()
        .find(|statement| matches!(statement.kind, StatementKind::Unsupported { .. }))
        .ok_or("selected string helper identity must lower to an explicit refusal")?;
    let StatementKind::Unsupported { description } = &refusal.kind else {
        return Err("expected selected string helper refusal".into());
    };
    if !description.contains("checked string helper") {
        return Err(format!("unexpected string-helper refusal: {description}").into());
    }
    if refusal.span.start != call_span.start || refusal.span.end != call_span.end {
        return Err(format!(
            "expected refusal at {}..{}, got {}..{}",
            call_span.start, call_span.end, refusal.span.start, refusal.span.end
        )
        .into());
    }
    Ok(())
}

/// Retain every selected registry identity at its complete source call span and exclude unadmitted methods.
#[test]
fn selected_string_helpers_retain_registry_ids_issue1256() -> Result<(), Box<dyn std::error::Error>> {
    let (program, checker, _) = checked_source(SELECTED_STRING_HELPERS_SOURCE)?;
    let type_info = checker.type_info();
    let expected = [
        ("text.upper()", StringMethodId::Upper),
        ("text.lower()", StringMethodId::Lower),
        ("csv_line.split(\",\")", StringMethodId::Split),
        ("\", \".join(names)", StringMethodId::Join),
        ("padded.strip()", StringMethodId::Strip),
        ("sentence.contains(\"quick\")", StringMethodId::Contains),
        ("sentence.replace(\"fox\", \"dog\")", StringMethodId::Replace),
    ];

    for (call, identity) in expected {
        let span = call_span(SELECTED_STRING_HELPERS_SOURCE, call)?;
        if type_info.resolved_string_helper_call(span) != Some(identity) {
            return Err(format!("expected `{call}` to retain {identity:?}").into());
        }
    }

    let unselected_source = "def render(text: str) -> str:\n  return text.to_string()\n";
    let (unselected_program, unselected_checker, _) = checked_source(unselected_source)?;
    let unselected_span = call_span(unselected_source, "text.to_string()")?;
    if unselected_checker
        .type_info()
        .resolved_string_helper_call(unselected_span)
        .is_some()
    {
        return Err("unselected string methods must not acquire a helper identity".into());
    }
    let module_path = vec!["checked_string_helpers".to_string()];
    let lowered_unselected = build_body_ir_module_v0(&unselected_program, &module_path, unselected_checker.type_info());
    if lowered_unselected.render_snapshot().contains("helper:str_") {
        return Err("unselected string methods must not lower as selected helpers".into());
    }

    let custom_method_source = r#"
class Marker:
  def upper(self) -> int:
    return 1

def render(marker: Marker) -> int:
  return marker.upper()
"#;
    let (custom_program, custom_checker, custom_path) = checked_source(custom_method_source)?;
    let custom_span = call_span(custom_method_source, "marker.upper()")?;
    if custom_checker
        .type_info()
        .resolved_string_helper_call(custom_span)
        .is_some()
    {
        return Err("a non-string method sharing a selected spelling must not retain string-helper identity".into());
    }
    let custom_snapshot =
        build_body_ir_module_v0(&custom_program, &custom_path, custom_checker.type_info()).render_snapshot();
    if custom_snapshot.contains("helper:str_upper") || custom_snapshot.contains("unsupported(") {
        return Err(format!("custom non-string upper() must keep ordinary method lowering: {custom_snapshot}").into());
    }

    let lowered = build_body_ir_module_v0(&program, &module_path, type_info);
    if lowered.render_snapshot().contains("unsupported(") {
        return Err("selected helper fixture must typecheck and lower without a refusal".into());
    }
    Ok(())
}

/// Lower selected identities to helper calls while preserving the string-contains receiver/needle order.
#[test]
fn selected_string_helpers_lower_to_canonical_helper_ops_issue1256() -> Result<(), Box<dyn std::error::Error>> {
    let (program, checker, module_path) = checked_source(SELECTED_STRING_HELPERS_SOURCE)?;
    let snapshot = build_body_ir_module_v0(&program, &module_path, checker.type_info()).render_snapshot();

    for helper in [
        "str_upper",
        "str_lower",
        "str_split",
        "str_join",
        "str_strip",
        "str_contains",
        "str_replace",
    ] {
        if !snapshot.contains(&format!("call helper:{helper}")) {
            return Err(format!("missing selected helper `{helper}`: {snapshot}").into());
        }
    }
    for raw_method in ["upper", "lower", "split", "join", "strip", "contains", "replace"] {
        if snapshot.contains(&format!("method:{raw_method}")) {
            return Err(format!("selected helper must not retain raw method `{raw_method}`: {snapshot}").into());
        }
    }

    let contains_source = "def contains(haystack: str, needle: str) -> bool:\n  return haystack.contains(needle)\n";
    let (contains_program, contains_checker, contains_path) = checked_source(contains_source)?;
    let contains_snapshot =
        build_body_ir_module_v0(&contains_program, &contains_path, contains_checker.type_info()).render_snapshot();
    let contains_call = contains_snapshot
        .lines()
        .find(|line| line.contains("call helper:str_contains"))
        .ok_or("missing explicit str_contains helper call")?;
    let receiver = contains_call
        .find("_0")
        .ok_or("contains helper call is missing the receiver operand")?;
    let needle = contains_call
        .find("_1")
        .ok_or("contains helper call is missing the needle operand")?;
    if receiver >= needle {
        return Err(format!("str_contains must keep (haystack, needle) order: {contains_call}").into());
    }
    Ok(())
}

/// Refuse missing or inconsistent selected identities rather than deriving a helper from method text.
#[test]
fn selected_string_helper_identity_gaps_refuse_at_the_original_call_span_issue1256()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "def transform(text: str) -> str:\n  return text.upper()\n";
    let (program, checker, module_path) = checked_source(source)?;
    let span = call_span(source, "text.upper()")?;

    let mut missing = checker.type_info().clone();
    if missing.resolved_string_helper_call(span) != Some(StringMethodId::Upper) {
        return Err("fixture must begin with the checked Upper identity".into());
    }
    missing
        .calls
        .resolved_string_helper_calls
        .remove(&(span.start, span.end));
    let missing_module = build_body_ir_module_v0(&program, &module_path, &missing);
    assert_refusal_at_call_span(&missing_module, "transform", span)?;

    let mut mismatched = checker.type_info().clone();
    mismatched
        .calls
        .resolved_string_helper_calls
        .insert((span.start, span.end), StringMethodId::Lower);
    let mismatched_module = build_body_ir_module_v0(&program, &module_path, &mismatched);
    assert_refusal_at_call_span(&mismatched_module, "transform", span)?;
    Ok(())
}

/// Keep parser-admitted call forms outside the selected helper contract until their binding semantics are explicit.
#[test]
fn selected_string_helper_no_identity_call_forms_refuse_at_the_original_span_issue1256()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "named arguments",
            "def transform(text: str) -> str:\n  return text.replace(old=\"a\", new=\"b\")\n",
            "text.replace(old=\"a\", new=\"b\")",
        ),
        (
            "positional unpack",
            "def transform(text: str, parts: list[str]) -> str:\n  return text.replace(*parts)\n",
            "text.replace(*parts)",
        ),
        (
            "keyword unpack",
            "def transform(text: str) -> str:\n  return text.replace(**{\"old\": \"a\", \"new\": \"b\"})\n",
            "text.replace(**{\"old\": \"a\", \"new\": \"b\"})",
        ),
        (
            "explicit type arguments",
            "def transform(text: str) -> str:\n  return text.upper[int]()\n",
            "text.upper[int]()",
        ),
    ];

    for (case_name, source, call) in cases {
        let (program, checker, module_path) = checked_source(source)?;
        let span = call_span(source, call)?;
        if checker.type_info().resolved_string_helper_call(span).is_some() {
            return Err(format!("{case_name}: no-identity call form must not retain a selected helper").into());
        }
        let module = build_body_ir_module_v0(&program, &module_path, checker.type_info());
        assert_refusal_at_call_span(&module, "transform", span)?;
    }
    Ok(())
}

/// Keep the selected helper subset aligned with the documented ordinary positional call forms.
#[test]
fn selected_string_helpers_validate_documented_positional_signatures_issue1256()
-> Result<(), Box<dyn std::error::Error>> {
    let valid = r#"
def valid(text: str, names: list[str]) -> None:
  upper = text.upper()
  lower = text.lower()
  stripped = text.strip()
  replaced = text.replace("before", "after")
  joined = ", ".join(names)
  contains = text.contains("needle")
  split_default = text.split()
  split_with_separator = text.split(",")
"#;
    let (_, checker, _) = checked_source(valid)?;
    let expected = [
        ("text.upper()", StringMethodId::Upper),
        ("text.lower()", StringMethodId::Lower),
        ("text.strip()", StringMethodId::Strip),
        ("text.replace(\"before\", \"after\")", StringMethodId::Replace),
        ("\", \".join(names)", StringMethodId::Join),
        ("text.contains(\"needle\")", StringMethodId::Contains),
        ("text.split()", StringMethodId::Split),
        ("text.split(\",\")", StringMethodId::Split),
    ];
    for (call, identity) in expected {
        let span = call_span(valid, call)?;
        if checker.type_info().resolved_string_helper_call(span) != Some(identity) {
            return Err(format!("expected `{call}` to retain {identity:?}").into());
        }
    }

    let invalid_cases = [
        (
            "upper extra argument",
            "def invalid(text: str) -> str:\n  return text.upper(\"extra\")\n",
            "text.upper(\"extra\")",
            "str.upper() expects 0 argument(s), got 1",
        ),
        (
            "lower extra argument",
            "def invalid(text: str) -> str:\n  return text.lower(\"extra\")\n",
            "text.lower(\"extra\")",
            "str.lower() expects 0 argument(s), got 1",
        ),
        (
            "strip extra argument",
            "def invalid(text: str) -> str:\n  return text.strip(\"extra\")\n",
            "text.strip(\"extra\")",
            "str.strip() expects 0 argument(s), got 1",
        ),
        (
            "replace missing second argument",
            "def invalid(text: str) -> str:\n  return text.replace(\"before\")\n",
            "text.replace(\"before\")",
            "str.replace() expects 2 argument(s), got 1",
        ),
        (
            "replace extra argument",
            "def invalid(text: str) -> str:\n  return text.replace(\"a\", \"b\", \"c\")\n",
            "text.replace(\"a\", \"b\", \"c\")",
            "str.replace() expects 2 argument(s), got 3",
        ),
        (
            "replace wrong second argument type",
            "def invalid(text: str) -> str:\n  return text.replace(\"a\", 1)\n",
            "text.replace(\"a\", 1)",
            "expected 'str', found 'int'",
        ),
        (
            "join wrong argument type",
            "def invalid(text: str) -> str:\n  return text.join(\"not a list\")\n",
            "text.join(\"not a list\")",
            "expected 'List[str]', found 'str'",
        ),
        (
            "join wrong item type",
            "def invalid(text: str) -> str:\n  return text.join([1])\n",
            "text.join([1])",
            "expected 'List[str]', found 'List[int]'",
        ),
        (
            "contains wrong argument type",
            "def invalid(text: str) -> bool:\n  return text.contains(1)\n",
            "text.contains(1)",
            "expected 'str', found 'int'",
        ),
        (
            "split wrong separator type",
            "def invalid(text: str) -> list[str]:\n  return text.split(1)\n",
            "text.split(1)",
            "expected 'str', found 'int'",
        ),
        (
            "split extra separator",
            "def invalid(text: str) -> list[str]:\n  return text.split(\",\", \";\")\n",
            "text.split(\",\", \";\")",
            "str.split() expects at most 1 argument(s), got 2",
        ),
    ];
    for (case_name, source, call, expected) in invalid_cases {
        let (checker, errors) = checked_source_with_error_messages(source)?;
        if !errors.iter().any(|error| error.contains(expected)) {
            return Err(format!("{case_name}: expected `{expected}`, got {errors:?}").into());
        }
        let span = call_span(source, call)?;
        if checker.type_info().resolved_string_helper_call(span).is_some() {
            return Err(format!("{case_name}: malformed source must not retain a string-helper identity").into());
        }
    }
    Ok(())
}
