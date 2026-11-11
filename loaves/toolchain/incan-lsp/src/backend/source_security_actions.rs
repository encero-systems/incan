//! Editor quick fixes for compiler-owned source-security diagnostics.

use super::*;

/// Build the deterministic replacement and removal actions for each exact raw bidirectional-control diagnostic.
pub(super) fn source_security_code_actions(
    uri: &Url,
    source: &str,
    diagnostics: &[Diagnostic],
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();
    for diagnostic in diagnostics {
        if !diagnostic_is_raw_bidirectional_control(diagnostic) {
            continue;
        }
        let Some(start) = position_to_offset(source, diagnostic.range.start) else {
            continue;
        };
        let Some(end) = position_to_offset(source, diagnostic.range.end) else {
            continue;
        };
        let Some(value) = source.get(start..end) else {
            continue;
        };
        let mut characters = value.chars();
        let Some(character) = characters.next() else {
            continue;
        };
        if characters.next().is_some() || incan_syntax::source_security::bidirectional_control_name(character).is_none()
        {
            continue;
        }
        let replacement = incan_syntax::source_security::unicode_scalar_escape(character);
        actions.push(replacement_action(
            uri,
            diagnostic,
            format!("Replace with {replacement}"),
            replacement,
            true,
        ));
        actions.push(replacement_action(
            uri,
            diagnostic,
            "Remove bidirectional control".to_string(),
            String::new(),
            false,
        ));
    }
    actions
}

/// Whether one LSP diagnostic carries the stable RFC 175 raw-control code.
fn diagnostic_is_raw_bidirectional_control(diagnostic: &Diagnostic) -> bool {
    matches!(
        diagnostic.code.as_ref(),
        Some(NumberOrString::String(code)) if code == "INCAN-P0101"
    )
}

/// Build one edit over the compiler's exact control range without reinterpreting the surrounding source.
fn replacement_action(
    uri: &Url,
    diagnostic: &Diagnostic,
    title: String,
    new_text: String,
    preferred: bool,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(HashMap::from([(
                uri.clone(),
                vec![TextEdit {
                    range: diagnostic.range,
                    new_text,
                }],
            )])),
            document_changes: None,
            change_annotations: None,
        }),
        command: None,
        is_preferred: Some(preferred),
        disabled: None,
        data: None,
    })
}

#[cfg(test)]
mod tests {
    use super::source_security_code_actions;
    use tower_lsp::lsp_types::{Diagnostic, NumberOrString, Position, Range, Url};

    /// The preferred fix exposes the scalar escape and the alternative removes exactly the diagnosed character.
    #[test]
    fn raw_control_actions_replace_or_remove_the_exact_span() -> Result<(), Box<dyn std::error::Error>> {
        let uri = Url::parse("file:///source.incn")?;
        let diagnostic = Diagnostic {
            range: Range::new(Position::new(0, 4), Position::new(0, 5)),
            code: Some(NumberOrString::String("INCAN-P0101".to_string())),
            ..Diagnostic::default()
        };
        let actions = source_security_code_actions(&uri, "left\u{202e}right", &[diagnostic]);
        assert_eq!(actions.len(), 2);
        let replacement = match &actions[0] {
            tower_lsp::lsp_types::CodeActionOrCommand::CodeAction(action) => action,
            tower_lsp::lsp_types::CodeActionOrCommand::Command(_) => return Err("expected code action".into()),
        };
        assert_eq!(replacement.title, "Replace with \\u{202e}");
        assert_eq!(replacement.is_preferred, Some(true));
        let edit = replacement.edit.as_ref().ok_or("missing workspace edit")?;
        let changes = edit.changes.as_ref().ok_or("missing changes")?;
        let edits = changes.get(&uri).ok_or("missing document edits")?;
        assert_eq!(edits.first().ok_or("missing replacement")?.new_text, "\\u{202e}");
        Ok(())
    }
}
