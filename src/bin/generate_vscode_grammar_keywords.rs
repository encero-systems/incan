//! Sync VS Code/TextMate keyword regexes from `incan_core::lang` registries.
//!
//! This updates only the keyword-related regex lines inside `workspaces/ide/vscode/incan.tmLanguage.json`. The grammar
//! file remains a checked-in artifact, but its keyword buckets are derived from stable `KeywordId`-based helpers.

use std::fs;
use std::io;
use std::path::PathBuf;

use incan_core::lang::highlighting;

/// Rewrite the checked-in VS Code grammar so keyword regexes match the canonical language registry.
///
/// This is intended for repository maintenance and should be run after changing `incan_core::lang::highlighting` or the
/// underlying keyword metadata.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let grammar_path = workspace_root().join("workspaces/ide/vscode/incan.tmLanguage.json");
    let contents = fs::read_to_string(&grammar_path)?;
    let updated = sync_vscode_grammar(&contents)?;

    if contents == updated {
        println!("VS Code grammar keywords already in sync.");
        return Ok(());
    }

    fs::write(&grammar_path, updated)?;
    println!("Updated {}", grammar_path.display());
    Ok(())
}

/// Return the repository root that contains the checked-in VS Code grammar artifact.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Apply every registry-backed keyword regex replacement to the provided grammar contents.
fn sync_vscode_grammar(contents: &str) -> Result<String, io::Error> {
    let mut updated = contents.to_string();
    for (pattern_name, regex) in highlighting::vscode_pattern_regexes() {
        updated = replace_named_pattern_match(&updated, pattern_name, &regex)?;
    }
    Ok(updated)
}

/// Replace the `"match"` line associated with a named TextMate pattern.
///
/// The generator only rewrites keyword regex buckets, so this performs a small textual update instead of reparsing and
/// reserializing the whole grammar JSON file.
fn replace_named_pattern_match(contents: &str, pattern_name: &str, regex: &str) -> Result<String, io::Error> {
    let name_needle = format!("\"name\": \"{pattern_name}\"");
    let mut cursor = 0usize;
    let mut selected_match_idx = None;

    while let Some(relative_name_idx) = contents[cursor..].find(&name_needle) {
        let name_idx = cursor + relative_name_idx;
        let search_start = name_idx.saturating_sub(240);
        let prefix = &contents[search_start..name_idx];
        if let Some(relative_match_idx) = prefix.rfind("\"match\":") {
            selected_match_idx = Some(search_start + relative_match_idx);
        }
        cursor = name_idx + name_needle.len();
    }

    let Some(match_idx) = selected_match_idx else {
        return Err(io::Error::other(format!(
            "pattern `{pattern_name}` with a `match` line was not found in VS Code grammar"
        )));
    };
    let line_start = contents[..match_idx].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let line_end = contents[match_idx..]
        .find('\n')
        .map(|idx| match_idx + idx)
        .unwrap_or(contents.len());

    let current_line = &contents[line_start..line_end];
    let indent_width = current_line.find('"').unwrap_or(0);
    let indent = &current_line[..indent_width];
    let regex_literal = serde_json::to_string(regex).map_err(io::Error::other)?;
    let replacement_line = format!("{indent}\"match\": {regex_literal},");

    let mut updated = String::with_capacity(contents.len() + replacement_line.len());
    updated.push_str(&contents[..line_start]);
    updated.push_str(&replacement_line);
    updated.push_str(&contents[line_end..]);
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_grammar_is_synced_with_registry_buckets() -> Result<(), Box<dyn std::error::Error>> {
        let grammar_path = workspace_root().join("workspaces/ide/vscode/incan.tmLanguage.json");
        let contents = fs::read_to_string(grammar_path)?;
        let updated = sync_vscode_grammar(&contents)?;
        assert_eq!(updated, contents);
        Ok(())
    }

    #[test]
    fn replacement_updates_named_pattern_match_line() -> Result<(), Box<dyn std::error::Error>> {
        let sample = r#"{
  "patterns": [],
  "repository": {
    "keywords": {
      "patterns": [
        {
          "match": "\\b(old)\\b",
          "name": "keyword.control.flow.incan"
        }
      ]
    }
  }
}
"#;

        let updated = replace_named_pattern_match(sample, "keyword.control.flow.incan", r"\b(new|old)\b")?;
        assert!(updated.contains(r#""match": "\\b(new|old)\\b","#));
        Ok(())
    }
}
