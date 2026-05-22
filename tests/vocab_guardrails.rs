use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use incan_core::lang::derives;
use incan_core::lang::types::collections;
use serde::Deserialize;

const SEMANTIC_STRING_AUDIT_PATH: &str = "tests/fixtures/vocab_guardrails/semantic_string_audit.json";

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

#[derive(Debug)]
struct AuditedSemanticStringFile {
    path: String,
    category: String,
    expected_count: usize,
    expected_fingerprint: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SemanticStringAudit {
    files: Vec<RawAuditedSemanticStringFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAuditedSemanticStringFile {
    path: String,
    category: String,
    expected_count: usize,
    expected_fingerprint: String,
}

/// Guardrail for semantic string comparisons that remain in high-risk compiler paths.
///
/// A semantic string comparison is not automatically wrong. Some strings are source names, manifest keys, Rust display
/// fragments, or quarantined metadata-free compatibility policy. The point of this test is that these comparisons must
/// be visible and classified instead of silently growing in typechecking, lowering, emission, dependency resolution, or
/// Rust inspection.
#[test]
fn semantic_string_checks_are_classified() {
    let root = repo_root();
    let audit_entries = audited_semantic_string_files(&root);
    let scan_files = semantic_string_scan_files(&root);
    let scanned_paths: BTreeSet<String> = scan_files.iter().map(|path| rel_path(&root, path)).collect();
    let mut offenders = Vec::new();

    for path in &scan_files {
        let sites = semantic_string_sites(path);
        if sites.is_empty() {
            continue;
        }
        let rel = rel_path(&root, path);
        let actual_count = sites.len();
        let actual_fingerprint = fingerprint_sites(&sites);
        match audit_entries.iter().find(|entry| entry.path == rel) {
            Some(entry) if entry.expected_count == actual_count && entry.expected_fingerprint == actual_fingerprint => {
            }
            Some(entry) => offenders.push(format!(
                "{} changed in `{}`: expected {} sites/{:016x}, found {} sites/{:016x}",
                entry.category, rel, entry.expected_count, entry.expected_fingerprint, actual_count, actual_fingerprint
            )),
            None => offenders.push(format!(
                "unclassified semantic string checks in `{rel}`: {} sites/{actual_fingerprint:016x}",
                actual_count
            )),
        }
    }

    let mut audited_paths: BTreeSet<&str> = BTreeSet::new();
    let mut previous_audited_path: Option<&str> = None;
    for entry in &audit_entries {
        if let Some(previous) = previous_audited_path
            && previous > entry.path.as_str()
        {
            offenders.push(format!(
                "semantic string audit paths are not sorted: `{previous}` appears before `{}`",
                entry.path
            ));
        }
        previous_audited_path = Some(entry.path.as_str());
        if !audited_paths.insert(entry.path.as_str()) {
            offenders.push(format!("duplicate semantic string audit entry: `{}`", entry.path));
        }
        let path = root.join(&entry.path);
        if !path.exists() {
            offenders.push(format!(
                "audited semantic string file no longer exists: `{}` ({})",
                entry.path, entry.category
            ));
        } else if !scanned_paths.contains(&entry.path) {
            offenders.push(format!(
                "audited semantic string file is outside scanned roots: `{}` ({})",
                entry.path, entry.category
            ));
        }
    }

    if !offenders.is_empty() {
        let mut msg = String::new();
        msg.push_str(
            "Semantic string checks changed. Move behavior behind a registry when possible; otherwise classify the file in the semantic string audit fixture.\n\n",
        );
        for offender in offenders {
            msg.push_str("- ");
            msg.push_str(&offender);
            msg.push('\n');
        }
        msg.push_str("\nCurrent scanned sites:\n");
        for path in &scan_files {
            let sites = semantic_string_sites(path);
            if sites.is_empty() {
                continue;
            }
            let rel = rel_path(&root, path);
            msg.push_str(&format!(
                "\n{rel} ({} sites/{:016x})\n",
                sites.len(),
                fingerprint_sites(&sites)
            ));
            for site in sites {
                msg.push_str(&format!("  {}\n", site.trim()));
            }
        }
        panic!("{msg}");
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn audited_semantic_string_files(root: &Path) -> Vec<AuditedSemanticStringFile> {
    let audit_path = root.join(SEMANTIC_STRING_AUDIT_PATH);
    let contents = fs::read_to_string(&audit_path)
        .unwrap_or_else(|err| panic!("failed to read semantic string audit `{}`: {err}", audit_path.display()));
    let audit: SemanticStringAudit = serde_json::from_str(&contents).unwrap_or_else(|err| {
        panic!(
            "failed to parse semantic string audit `{}`: {err}",
            audit_path.display()
        )
    });

    if audit.files.is_empty() {
        panic!(
            "semantic string audit `{}` must classify at least one file",
            audit_path.display()
        );
    }

    audit
        .files
        .into_iter()
        .map(|entry| {
            let expected_fingerprint =
                parse_expected_fingerprint(&audit_path, &entry.path, &entry.expected_fingerprint);
            AuditedSemanticStringFile {
                path: entry.path,
                category: entry.category,
                expected_count: entry.expected_count,
                expected_fingerprint,
            }
        })
        .collect()
}

fn parse_expected_fingerprint(audit_path: &Path, entry_path: &str, value: &str) -> u64 {
    let hex = value.strip_prefix("0x").unwrap_or_else(|| {
        panic!(
            "semantic string audit `{}` entry `{entry_path}` has non-hex expected_fingerprint `{value}`",
            audit_path.display()
        )
    });
    u64::from_str_radix(hex, 16).unwrap_or_else(|err| {
        panic!(
            "semantic string audit `{}` entry `{entry_path}` has invalid expected_fingerprint `{value}`: {err}",
            audit_path.display()
        )
    })
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

fn semantic_string_scan_files(root: &Path) -> Vec<PathBuf> {
    const ROOTS: &[&str] = &[
        "crates/incan_core/src/interop",
        "crates/rust_inspect/src",
        "src/backend/ir",
        "src/cli/commands/common.rs",
        "src/dependency_resolver.rs",
        "src/frontend/testing_markers.rs",
        "src/frontend/typechecker",
    ];

    let mut files = Vec::new();
    for root_path in ROOTS {
        collect_rust_files(&root.join(root_path), &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn collect_rust_files(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        if is_semantic_string_scan_file(path) {
            files.push(path.to_path_buf());
        }
        return;
    }

    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if is_semantic_string_scan_file(&path) {
            files.push(path);
        }
    }
}

fn is_semantic_string_scan_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("rs")
        && path.file_name().and_then(|file| file.to_str()) != Some("tests.rs")
        && !path
            .components()
            .any(|component| component.as_os_str().to_str() == Some("tests"))
}

fn semantic_string_sites(path: &Path) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut sites = Vec::new();
    let mut brace_depth = 0usize;
    let mut pending_cfg_test = false;
    let mut skip_until_depth: Option<usize> = None;

    for line in contents.lines() {
        let code = strip_line_comment(line).trim();
        if let Some(target_depth) = skip_until_depth {
            brace_depth = update_brace_depth(brace_depth, code);
            if brace_depth <= target_depth {
                skip_until_depth = None;
            }
            continue;
        }

        if code.starts_with("#[cfg(test)]") {
            pending_cfg_test = true;
            brace_depth = update_brace_depth(brace_depth, code);
            continue;
        }
        if pending_cfg_test && code.contains("mod tests") && code.contains('{') {
            let target_depth = brace_depth;
            brace_depth = update_brace_depth(brace_depth, code);
            if brace_depth > target_depth {
                skip_until_depth = Some(target_depth);
            }
            pending_cfg_test = false;
            continue;
        }
        if pending_cfg_test && !code.starts_with("#[") && !code.is_empty() {
            pending_cfg_test = false;
        }

        if semantic_string_line(code) {
            sites.push(code.to_string());
        }
        brace_depth = update_brace_depth(brace_depth, code);
    }

    sites
}

fn update_brace_depth(current: usize, code: &str) -> usize {
    let mut depth = current;
    let mut in_string = false;
    let mut escaped = false;
    for byte in code.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth = depth.saturating_add(1),
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

fn strip_line_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    while idx + 1 < bytes.len() {
        let byte = bytes[idx];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            idx += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
        } else if byte == b'/' && bytes[idx + 1] == b'/' {
            return &line[..idx];
        }
        idx += 1;
    }
    line
}

fn semantic_string_line(code: &str) -> bool {
    if code.is_empty() || !code.contains('"') {
        return false;
    }
    if code.starts_with("#[")
        || code.starts_with("assert!")
        || code.starts_with("assert_eq!")
        || code.starts_with("assert_ne!")
        || code.starts_with("panic!")
        || code.starts_with("format!")
        || code.starts_with("write!")
        || code.starts_with("writeln!")
    {
        return false;
    }

    line_has_string_comparison(code)
        || line_has_string_matches_macro(code)
        || line_has_string_match_arm(code)
        || line_has_semantic_string_table(code)
}

fn line_has_string_comparison(code: &str) -> bool {
    code.contains("== \"")
        || code.contains("!= \"")
        || code.contains("== &\"")
        || code.contains("!= &\"")
        || code.contains(".as_deref() == Some(\"")
        || code.contains(".as_deref() != Some(\"")
        || code.contains("== Some(\"")
        || code.contains("!= Some(\"")
}

fn line_has_string_matches_macro(code: &str) -> bool {
    code.contains("matches!(") && code.contains('"')
}

fn line_has_string_match_arm(code: &str) -> bool {
    let Some(arrow_idx) = code.find("=>") else {
        return false;
    };
    let before_arrow = code[..arrow_idx].trim_start();
    before_arrow.starts_with('"') || before_arrow.starts_with("| \"") || before_arrow.starts_with("(\"")
}

fn line_has_semantic_string_table(code: &str) -> bool {
    code.contains("methods: &[") || code.contains("expected: &[") || code.contains("features: &[")
}

fn fingerprint_sites(sites: &[String]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for site in sites {
        for byte in site.as_bytes().iter().chain(std::iter::once(&b'\n')) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}
