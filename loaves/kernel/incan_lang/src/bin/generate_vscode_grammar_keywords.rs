//! Sync VS Code/TextMate keyword regexes from `incan_lang::lang` registries.
//!
//! This updates only the keyword-related regex lines inside `workspaces/ide/vscode/incan.tmLanguage.json`. The grammar
//! file remains a checked-in artifact, but its keyword buckets are derived from stable `KeywordId`-based helpers.

use std::fs;
use std::io;
use std::path::PathBuf;

use incan_lang::lang::highlighting;

/// Rewrite the checked-in VS Code grammar so keyword regexes match the canonical language registry.
///
/// This is intended for repository maintenance and should be run after changing `incan_lang::lang::highlighting` or the
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

/// Locate the checkout whose grammar this tool syncs: an explicit `INCAN_SOURCE_ROOT`, the current directory when it
/// is the checkout, or the checkout three levels above this kernel crate.
fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("INCAN_SOURCE_ROOT").filter(|path| !path.is_empty()) {
        return PathBuf::from(root);
    }
    if let Ok(current_dir) = std::env::current_dir()
        && current_dir.join("Cargo.toml").is_file()
        && current_dir.join("workspaces/ide/vscode").is_dir()
    {
        return current_dir;
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(3)
        .map(std::path::Path::to_path_buf)
        .unwrap_or(manifest_dir)
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
    let regex_literal = json_string_literal(regex);
    let replacement_line = format!("{indent}\"match\": {regex_literal},");

    let mut updated = String::with_capacity(contents.len() + replacement_line.len());
    updated.push_str(&contents[..line_start]);
    updated.push_str(&replacement_line);
    updated.push_str(&contents[line_end..]);
    Ok(updated)
}

/// Render one string as a JSON string literal: quotes, backslashes and control characters escaped, everything else
/// verbatim. The grammar file is JSON, and a regex is mostly backslashes, so this is the one encoder the tool needs
/// and the kernel crate stays without a JSON dependency for it.
fn json_string_literal(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('"');
    for character in value.chars() {
        match character {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            control if control.is_control() => {
                literal.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => literal.push(other),
        }
    }
    literal.push('"');
    literal
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
