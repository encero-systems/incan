//! Editor-mediated compile-time trust actions backed by Oven's user-configuration command.

use super::*;

use std::os::unix::process::CommandExt;

/// One exact trust entry offered by Oven after unattended preparation stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TrustEntry {
    /// Oven's entry kind: publisher, Loaf, or project.
    kind: String,
    /// The publisher scope, Loaf identity, or absolute project root.
    value: String,
}

/// Read the machine-invocable command from each stable missing-trust stop without re-deciding policy in the editor.
pub(super) fn trust_entries_from_stop(stop: &str) -> Vec<TrustEntry> {
    let mut entries = Vec::new();
    for line in stop.lines() {
        let Some(command) = line
            .trim()
            .strip_prefix("trust with `oven trust add ")
            .and_then(|value| value.strip_suffix('`'))
        else {
            continue;
        };
        let Some((kind, value)) = command.split_once(' ') else {
            continue;
        };
        if !matches!(kind, "publisher" | "loaf" | "project") || value.is_empty() {
            continue;
        }
        let entry = TrustEntry {
            kind: kind.to_string(),
            value: value.to_string(),
        };
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    entries
}

/// Whether a diagnostic is the unattended compile-time trust refusal that can offer a trust action.
pub(super) fn diagnostic_is_not_trusted(diagnostic: &Diagnostic) -> bool {
    matches!(
        diagnostic.code.as_ref(),
        Some(NumberOrString::String(code)) if code == "INCAN-T0134"
    )
}

/// Build the standard LSP quick fix whose command carries only the entry Oven already chose.
pub(super) fn trust_code_action(entry: &TrustEntry) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Trust {} {}", entry.kind, entry.value),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: None,
        edit: None,
        command: Some(Command {
            title: "Add compile-time trust entry".to_string(),
            command: ADD_TRUST_COMMAND.to_string(),
            arguments: Some(vec![json!(entry.kind), json!(entry.value)]),
        }),
        is_preferred: Some(false),
        disabled: None,
        data: None,
    })
}

impl IncanLanguageServer {
    /// Confirm and apply one exact Oven trust entry. No project file is edited: `oven trust add` owns the user policy.
    pub(super) async fn execute_add_trust(
        &self,
        arguments: Vec<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        let entry = match trust_entry_from_arguments(&arguments) {
            Ok(entry) => entry,
            Err(error) => return Ok(Some(json!({ "error": error }))),
        };
        let confirmation = "Add trust entry".to_string();
        let selected = self
            .client
            .show_message_request(
                MessageType::WARNING,
                format!(
                    "Trust {} {} to run its compile-time code during unattended editor analysis?",
                    entry.kind, entry.value
                ),
                Some(vec![MessageActionItem {
                    title: confirmation.clone(),
                    properties: HashMap::new(),
                }]),
            )
            .await?;
        if !selected.is_some_and(|action| action.title == confirmation) {
            return Ok(Some(json!({ "applied": false })));
        }
        let applied = tokio::task::spawn_blocking(move || run_trust_add(&entry)).await;
        match applied {
            Ok(Ok(message)) => Ok(Some(json!({ "applied": true, "message": message }))),
            Ok(Err(error)) => Ok(Some(json!({ "error": error }))),
            Err(error) => Ok(Some(json!({ "error": format!("trust update failed: {error}") }))),
        }
    }
}

/// Decode the fixed command arguments and reject callers that try to smuggle Oven options through the value.
fn trust_entry_from_arguments(arguments: &[serde_json::Value]) -> std::result::Result<TrustEntry, String> {
    let [kind, value] = arguments else {
        return Err(format!("{ADD_TRUST_COMMAND} expects [kind, value]"));
    };
    let kind = kind
        .as_str()
        .filter(|kind| matches!(*kind, "publisher" | "loaf" | "project"))
        .ok_or_else(|| "trust kind must be publisher, loaf, or project".to_string())?;
    let value = value
        .as_str()
        .filter(|value| !value.is_empty() && !value.starts_with('-'))
        .ok_or_else(|| "trust value must be a non-option string".to_string())?;
    Ok(TrustEntry {
        kind: kind.to_string(),
        value: value.to_string(),
    })
}

/// Run the sibling compiler under Oven's executable name so the command edits the ordinary user policy.
fn run_trust_add(entry: &TrustEntry) -> std::result::Result<String, String> {
    let incan = std::env::current_exe()
        .map_err(|error| format!("cannot locate the language server: {error}"))?
        .with_file_name("incan");
    let output = std::process::Command::new(&incan)
        .arg0("oven")
        .arg("trust")
        .arg("add")
        .arg(&entry.kind)
        .arg(&entry.value)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", incan.display()))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stable stops yield deduplicated exact entries and ignore unrelated prose.
    #[test]
    fn parses_offered_trust_entries() {
        let stop = "error[INCAN-T0134]: stopped\n  trust with `oven trust add publisher acme`\nnoise\n  trust with `oven trust add project /work/demo`\n  trust with `oven trust add publisher acme`";
        assert_eq!(
            trust_entries_from_stop(stop),
            vec![
                TrustEntry {
                    kind: "publisher".to_string(),
                    value: "acme".to_string(),
                },
                TrustEntry {
                    kind: "project".to_string(),
                    value: "/work/demo".to_string(),
                },
            ]
        );
    }

    /// Command arguments admit only the three policy entry kinds and reject option-shaped values.
    #[test]
    fn validates_trust_command_arguments() {
        assert!(trust_entry_from_arguments(&[json!("loaf"), json!("acme/tools")]).is_ok());
        assert!(trust_entry_from_arguments(&[json!("publisher"), json!("--help")]).is_err());
        assert!(trust_entry_from_arguments(&[json!("mode"), json!("observe")]).is_err());
    }
}
