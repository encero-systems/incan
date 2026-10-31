//! Diagnostic field projection through the existing recipe and current node bindings.

use super::{CheckedBodyView, DiagnosticPositions, Fields, NodeBindingError, Positions, Program, TypeChecker, next};
use crate::diagnostics::{CompileError, ErrorKind};

/// Store ordered diagnostic primitives and exact structural ranges. Detached, foreign and relative ranges remain
/// unsupported until this profile can prove their source binding; no supplement is silently dropped.
pub(super) fn project_diagnostics(
    checker: &TypeChecker,
    program: &Program,
    positions: &Positions,
    view: &CheckedBodyView,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    let mut current = DiagnosticPositions::new(program, checker.symbols.declaration_origin());
    for (family, values) in [("error", &view.diagnostics), ("warning", &view.warnings)] {
        for (index, value) in values.iter().enumerate() {
            let error = current.render(value).map_err(|_| NodeBindingError::MissingNode)?;
            if !error.related_declarations().is_empty() {
                return Err(NodeBindingError::Unsupported("related declaration diagnostic"));
            }
            let mut row = vec![
                "diagnostic".into(),
                positions.nodes.anchor(error.span)?.path,
                format!("{family}:{index:08}"),
                kind_name(error.kind).into(),
                error.message.clone(),
                error.stable_code().unwrap_or("").into(),
            ];
            row.push(error.notes().len().to_string());
            row.extend(error.notes().iter().cloned());
            row.push(error.hints().len().to_string());
            row.extend(error.hints().iter().cloned());
            match (error.expected(), error.actual()) {
                (Some(expected), Some(actual)) => {
                    row.extend(["expected-actual".into(), expected.into(), actual.into()])
                }
                (None, None) => row.push("no-expected-actual".into()),
                _ => {
                    return Err(NodeBindingError::Unsupported("partial expected/actual diagnostic"));
                }
            }
            row.push(error.related_spans().len().to_string());
            for related in error.related_spans() {
                row.extend([positions.nodes.anchor(related.span)?.path, related.label.clone()]);
            }
            fields.push(row);
        }
    }
    Ok(())
}

/// Reconstruct the recipe from its own primitives, then capture it against this revision's verified node ranges.
pub(super) fn read_diagnostic(
    program: &Program,
    positions: &Positions,
    row: &[String],
) -> Result<super::super::super::diagnostic_values::AnchoredDiagnostic, NodeBindingError> {
    let mut values = row[3..].iter();
    let kind = match next(&mut values)? {
        "error" => ErrorKind::Error,
        "syntax" => ErrorKind::Syntax,
        "type" => ErrorKind::Type,
        "warning" => ErrorKind::Warning,
        "lint" => ErrorKind::Lint,
        _ => return Err(NodeBindingError::MissingNode),
    };
    let message = next(&mut values)?.to_owned();
    let code = next(&mut values)?;
    let mut error = CompileError::new(message, positions.span(&row[1])?);
    error.kind = kind;
    if !code.is_empty() {
        let entry = crate::diagnostics::explain(code).ok_or(NodeBindingError::MissingNode)?;
        error = error.with_stable_code(entry.code);
    }
    for note in strings(&mut values)? {
        error = error.with_note(note);
    }
    for hint in strings(&mut values)? {
        error = error.with_hint(hint);
    }
    match next(&mut values)? {
        "expected-actual" => {
            let expected = next(&mut values)?.to_owned();
            error = error.with_expected_actual(expected, next(&mut values)?);
        }
        "no-expected-actual" => {}
        _ => return Err(NodeBindingError::MissingNode),
    }
    let count = count(&mut values)?;
    for _ in 0..count {
        let span = positions.span(next(&mut values)?)?;
        error = error.with_related_span(span, next(&mut values)?);
    }
    if values.next().is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    DiagnosticPositions::new(program, positions.nodes.owner().origin.clone())
        .capture(error)
        .map_err(|_| NodeBindingError::MissingNode)
}

/// The persistent severity tag is independent of native enum discriminants or Display prose.
fn kind_name(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Error => "error",
        ErrorKind::Syntax => "syntax",
        ErrorKind::Type => "type",
        ErrorKind::Warning => "warning",
        ErrorKind::Lint => "lint",
    }
}

/// Read a bounded list count before allocating or indexing any field payload.
fn count(values: &mut std::slice::Iter<'_, String>) -> Result<usize, NodeBindingError> {
    let count = next(values)?
        .parse::<usize>()
        .map_err(|_| NodeBindingError::MissingNode)?;
    if count > values.len() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(count)
}

/// Preserve note/hint order without consulting the current diagnostic oracle.
fn strings(values: &mut std::slice::Iter<'_, String>) -> Result<Vec<String>, NodeBindingError> {
    let count = count(values)?;
    Ok(values.take(count).cloned().collect())
}
