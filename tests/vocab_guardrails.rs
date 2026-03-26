use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use incan_core::lang::derives;
use incan_core::lang::types::collections;

/// Guardrail against reintroducing stringly-typed vocabulary checks.
///
/// This is intentionally a **coarse** safety net. It looks for suspicious patterns like `== "List"` or
/// `match name.as_str() { "List" => ... }` in Rust source files where we expect callers to go through
/// `incan_core::lang` registries instead.
///
/// Notes:
/// - We allow occurrences in `crates/incan_core/src/lang/**` (registries themselves), in docgen, and in tests/fixtures.
/// - This is not meant to be perfect; it’s meant to catch “oops I added a string match”.
#[test]
fn no_new_stringly_vocab_checks_in_rust_sources() {
    let root = repo_root();
    let spellings = tier_a_spellings();
    let mut offenders: Vec<(PathBuf, usize, String)> = Vec::new();

    let targets = [root.join("src"), root.join("crates")];
    for dir in targets {
        if dir.exists() {
            scan_dir(&root, &dir, &spellings, &mut offenders);
        }
    }

    if !offenders.is_empty() {
        let mut msg = String::new();
        msg.push_str("Found potential stringly-typed vocabulary checks. Prefer incan_core registries.\n\n");
        for (path, line_no, line) in offenders.into_iter().take(80) {
            msg.push_str(&format!(
                "- {}:{}: {}\n",
                path.strip_prefix(&root).unwrap_or(&path).display(),
                line_no,
                line.trim()
            ));
        }
        panic!("{msg}");
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn tier_a_spellings() -> Vec<&'static str> {
    // Tier A: high-signal, drift-prone vocabulary.
    // - Generic bases / builtin collection type names (and aliases)
    // - Derive names
    //
    // Tier B (optional): add keywords/operators/punctuation/builtins/surface names.
    let mut set: BTreeSet<&'static str> = BTreeSet::new();

    for t in collections::COLLECTION_TYPES {
        set.insert(t.canonical);
        for &a in t.aliases {
            set.insert(a);
        }
    }

    for d in derives::DERIVES {
        set.insert(d.canonical);
    }

    set.into_iter().collect()
}

fn is_allowed_file(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    if !rel.ends_with(".rs") {
        return true;
    }
    // Registries and interop policy define canonical spellings; allow them.
    if rel.starts_with("crates/incan_core/src/lang/") || rel.starts_with("crates/incan_core/src/interop/") {
        return true;
    }
    // Docgen inevitably contains spellings for headings, etc.
    if rel == "crates/incan_core/src/bin/generate_lang_reference.rs" {
        return true;
    }
    // Tests can mention spellings directly.
    if rel.starts_with("tests/") {
        return true;
    }
    false
}

fn scan_dir(root: &Path, dir: &Path, spellings: &[&'static str], offenders: &mut Vec<(PathBuf, usize, String)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(root, &path, spellings, offenders);
            continue;
        }
        if is_allowed_file(root, &path) {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in contents.lines().enumerate() {
            if is_suspicious_line(line, spellings) {
                offenders.push((path.clone(), idx + 1, line.to_string()));
            }
        }
    }
}

fn is_suspicious_line(line: &str, spellings: &[&'static str]) -> bool {
    // Avoid false positives in comments/docstrings.
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!") {
        return false;
    }

    // Only flag explicit equality checks or match arms for known vocabulary spellings.
    for s in spellings {
        // Patterns we consider "stringly vocab checks":
        // - `... == "Spelling"`
        // - `"Spelling" => ...`
        let eq = format!("== \"{s}\"");
        let arm = format!("\"{s}\" =>");
        if line.contains(&eq) || line.contains(&arm) {
            return true;
        }
    }

    false
}
