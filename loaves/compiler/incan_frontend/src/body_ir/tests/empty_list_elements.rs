//! Empty list initializers retain agreed validated append elements without changing legacy inference.

use super::{TypeChecker, body_named, build, build_after_expected_typecheck_errors, lexer, parser};
use crate::symbols::ResolvedType;

/// The unchanged original closes its empty task list from the checked fresh handle it appends.
#[test]
fn complete_fresh_task_append_retains_closed_list() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_collections_and_strings/list_append_takes_a_fresh_task_handle.incn"
    );
    let module = build(source, &["m", "fresh_task_list"])?;
    let body = body_named(&module, "main")?;
    let locals = serde_json::to_string(&body.locals)?;
    assert!(
        !locals.contains("\"Unknown\"") && !locals.contains("\"TypeVar\""),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// Matching validated appends give the exact empty initializer a single concrete element type.
#[test]
fn matching_scalar_appends_close_empty_initializer() -> Result<(), Box<dyn std::error::Error>> {
    assert_list_closed(
        "def main() -> None:\n    mut values = []\n    values.append(1)\n    values.append(2)\n    println(len(values))\n",
        "main",
        true,
    )
}

/// Conflicting validated appends cannot publish a closed initializer or leave stale private read types.
#[test]
fn conflicting_appends_keep_empty_initializer_open() -> Result<(), Box<dyn std::error::Error>> {
    assert_list_closed(
        "def main() -> None:\n    mut values = []\n    values.append(1)\n    println(len(values))\n    values.append(\"other\")\n    println(len(values))\n",
        "main",
        false,
    )
}

/// A list with no validated append retains its unknown element instead of guessing a default.
#[test]
fn absent_append_keeps_empty_initializer_open() -> Result<(), Box<dyn std::error::Error>> {
    assert_list_closed(
        "def main() -> None:\n    mut values = []\n    println(len(values))\n",
        "main",
        false,
    )
}

/// Same-spelled bindings in separate bodies retain independent checked element requirements.
#[test]
fn separate_bindings_retain_independent_list_elements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def first() -> None:\n    mut values = []\n    values.append(1)\n    println(len(values))\n\ndef second() -> None:\n    mut values = []\n    values.append(\"text\")\n    println(len(values))\n";
    let module = build(source, &["m", "independent_empty_lists"])?;
    for (name, element) in [("first", "int"), ("second", "str")] {
        let body = body_named(&module, name)?;
        let local = body
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some("values"))
            .ok_or("missing list binding")?;
        assert_eq!(
            local.ty.to_string(),
            format!("List[{element}]"),
            "{}",
            body.render_snapshot()
        );
    }
    Ok(())
}

/// A mutating call refused on an immutable receiver cannot close the empty initializer.
#[test]
fn invalid_append_keeps_empty_initializer_open() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def main() -> None:\n    values = []\n    values.append(1)\n";
    let (module, errors) = build_after_expected_typecheck_errors(source, &["m", "invalid_list_append"])?;
    assert!(!errors.is_empty());
    assert!(serde_json::to_string(&body_named(&module, "main")?.locals)?.contains("\"Unknown\""));
    Ok(())
}

/// An alias escape without a proven element contract preserves the original empty initializer's refusal.
#[test]
fn alias_escape_keeps_empty_initializer_open() -> Result<(), Box<dyn std::error::Error>> {
    assert_list_closed(
        "def main() -> None:\n    mut values = []\n    values.append(1)\n    alias = values\n    println(len(alias))\n",
        "main",
        false,
    )
}

/// An index assignment is not a validated append and prevents a closed empty-initializer contract.
#[test]
fn other_mutation_keeps_empty_initializer_open() -> Result<(), Box<dyn std::error::Error>> {
    assert_list_closed(
        "def main() -> None:\n    mut values = []\n    values.append(1)\n    values[0] = \"other\"\n    println(len(values))\n",
        "main",
        false,
    )
}

/// Reassigning the binding invalidates its original initializer even when later appends agree.
#[test]
fn reassignment_keeps_original_empty_initializer_open() -> Result<(), Box<dyn std::error::Error>> {
    assert_list_closed(
        "def main() -> None:\n    mut values = []\n    values.append(1)\n    values = []\n    values.append(1)\n    println(len(values))\n",
        "main",
        false,
    )
}

/// A closure capture is an escape even when its body only reads the list's length.
#[test]
fn captured_length_read_keeps_empty_initializer_open() -> Result<(), Box<dyn std::error::Error>> {
    assert_list_closed(
        "def main() -> None:\n    mut values = []\n    values.append(1)\n    length = () => len(values)\n    println(length())\n",
        "main",
        false,
    )
}

/// Private element facts must leave the ordinary initializer's List[Unknown] snapshot untouched.
#[test]
fn append_element_requirement_preserves_legacy_initializer() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def main() -> None:\n    mut values = []\n    values.append(1)\n    println(len(values))\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".into(), "empty_list_snapshot".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let start = source.find("[]").ok_or("missing empty initializer")?;
    let key = (start, start + 2);
    let Some(ResolvedType::Generic(_, elements)) = checker.type_info().expressions.expr_types.get(&key) else {
        return Err("missing ordinary list initializer".into());
    };
    assert_eq!(elements, &[ResolvedType::Unknown]);
    let Some(ResolvedType::Generic(_, elements)) = checker.type_info().expressions.body_ir_expr_types.get(&key) else {
        return Err("missing private closed list initializer".into());
    };
    assert_eq!(elements, &[ResolvedType::Int]);
    Ok(())
}

/// A spawned source task retains its private integer output even when the surrounding empty list expects Unknown.
#[test]
fn contextual_task_append_retains_checked_spawn_output() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.async import spawn\nasync def work() -> int:\n    return 1\nasync def main() -> None:\n    mut values = []\n    values.append(spawn(work()))\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".into(), "contextual_task_append".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let facts = checker.type_info();
    let start = source.find("spawn(work())").ok_or("missing task call")?;
    let key = (start, start + "spawn(work())".len());
    assert_eq!(
        facts.calls.body_ir_call_result_types.get(&key),
        Some(&ResolvedType::Generic("JoinHandle".into(), vec![ResolvedType::Int])),
        "checked private arguments={:?}; private call results={:?}",
        facts.calls.body_ir_call_type_arguments,
        facts.calls.body_ir_call_result_types,
    );
    assert_eq!(
        facts.expressions.expr_types.get(&key),
        Some(&ResolvedType::Generic("JoinHandle".into(), vec![ResolvedType::Unknown]))
    );
    Ok(())
}

/// Compare the presence of an open element at the exact list binding, without depending on frame numbering.
fn assert_list_closed(source: &str, body_name: &str, closed: bool) -> Result<(), Box<dyn std::error::Error>> {
    let module = build(source, &["m", "empty_list_elements"])?;
    let body = body_named(&module, body_name)?;
    let local = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("values"))
        .ok_or("missing list binding")?;
    assert_eq!(
        !serde_json::to_string(&local.ty)?.contains("\"Unknown\""),
        closed,
        "{}",
        body.render_snapshot()
    );
    Ok(())
}
