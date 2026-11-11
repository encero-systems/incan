use super::TypeChecker;
use crate::diagnostics::{CompileError, DiagnosticPhase, code_for_error, format_error};

/// Check source that must produce exactly one type mismatch, keeping its complete structured diagnostic.
fn one_type_mismatch(source: &str) -> Result<CompileError, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("expected one type mismatch")?;
    assert_eq!(errors.len(), 1, "the mismatch has one owning diagnostic");
    errors.into_iter().next().ok_or_else(|| "missing type mismatch".into())
}

/// Plain enum construction keeps source type names in non-generic and generic mismatch diagnostics (#2763).
#[test]
fn plain_enum_construction_keeps_source_names_in_mismatch_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "Color",
            "enum Color:\n    Red(int)\n    Blue\n\ndef name(text: str) -> str:\n    return text\n\ndef main() -> None:\n    color = Color.Red(1)\n    println(name(color))\n",
            "Argument 'text' of 'name' has type mismatch: expected 'Str', found 'Color'",
        ),
        (
            "RecordState[Tombstone]",
            "model Page:\n    number: int\n\nmodel Tombstone:\n    serial: int\n\nenum RecordState[T]:\n    Present(T)\n    Deleted(T)\n\ndef read(state: RecordState[Page]) -> None:\n    pass\n\ndef main() -> None:\n    state = RecordState.Deleted(Tombstone(serial=1))\n    read(state)\n",
            "Argument 'state' of 'read' has type mismatch: expected 'RecordState[Page]', found 'RecordState[Tombstone]'",
        ),
    ];

    for (source_type, source, message) in cases {
        let error = one_type_mismatch(source)?;
        assert_eq!(code_for_error(&error, DiagnosticPhase::Typecheck), "INCAN-T0001");
        assert_eq!(error.message, message);
        assert!(!error.message.contains("__incan_carrier_"));
        assert_eq!(error.actual(), Some(source_type));
        assert!(
            !error.notes().is_empty(),
            "the parameter declaration explains the mismatch"
        );
        assert!(!error.hints().is_empty(), "the diagnostic names a source-level fix");
        let rendered = format_error("case.incn", source, &error);
        assert!(rendered.contains("case.incn"));
        assert!(rendered.contains('^'));
        assert!(rendered.contains("= hint:"));
        assert!(rendered.contains(source_type));
    }
    Ok(())
}
