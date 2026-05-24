//! Experimental architecture-advice command.
//!
//! The first deterministic signal is intentionally narrow: find repeated match
//! dispatch over the same apparent domain and report the design pressure with
//! concrete source evidence. The signal is backed by `incan_codegraph` body
//! facts so future architecture rules can share the same graph substrate.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use incan_codegraph::{CodegraphDocument, CodegraphEdgeKind, CodegraphNode, CodegraphNodeKind};
use incan_core::lang::stdlib;
use serde_json::json;

use crate::cli::{CliError, CliResult, ExitCode};

use super::tools::collect_codegraph_document_suppressing_diagnostics;

/// Output format for `incan architect`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchitectFormat {
    /// Human-readable advisory report.
    Text,
    /// Machine-readable report for tools and agents.
    Json,
}

/// Run an experimental architecture scan over an Incan source file or project.
pub fn architect_project(path: &Path, format: ArchitectFormat) -> CliResult<ExitCode> {
    let scan_path = resolve_architect_scan_path(path)?;
    let document = collect_codegraph_document_suppressing_diagnostics(&scan_path, true)?;
    let report = ArchitectureReport::from_codegraph(&document);

    match format {
        ArchitectFormat::Text => print_text_report(&report),
        ArchitectFormat::Json => print_json_report(&report)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Resolve a source file or directory used as the scan input.
fn resolve_architect_scan_path(path: &Path) -> CliResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path)
    };

    if absolute.is_file() {
        return Ok(absolute);
    }
    if absolute.is_dir() {
        return Ok(absolute);
    }

    Err(CliError::failure(format!(
        "architect scan requires an Incan source file or directory: {}",
        absolute.display()
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchitectureReport {
    findings: Vec<ArchitectureFinding>,
}

impl ArchitectureReport {
    fn from_codegraph(document: &CodegraphDocument) -> Self {
        let mut sites = document
            .nodes
            .iter()
            .filter(|node| node.kind == CodegraphNodeKind::MatchDispatch)
            .filter(|node| !is_stdlib_node(node))
            .filter_map(match_dispatch_site_from_node)
            .collect::<Vec<_>>();
        deduplicate_match_dispatch_sites(&mut sites);

        let mut findings = fail_fast_call_findings(document);
        findings.extend(repeated_dispatch_findings(&sites));
        deduplicate_findings(&mut findings);
        sort_findings(&mut findings);

        Self { findings }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchitectureFinding {
    code: &'static str,
    priority: &'static str,
    title: String,
    pressure: String,
    suggestions: Vec<String>,
    risks: Vec<String>,
    shared_patterns: Vec<String>,
    shared_pattern_count: usize,
    largest_pattern_count: usize,
    default_arm_site_count: usize,
    evidence: Vec<MatchEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchDispatchSite {
    domain_key: String,
    domain_label: String,
    patterns: BTreeSet<PatternLabel>,
    explicit_pattern_count: usize,
    has_default_arm: bool,
    evidence: MatchEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchEvidence {
    file_path: String,
    owner: String,
    line: usize,
    column: usize,
    summary: String,
    arm_labels: Vec<String>,
    explicit_arm_count: Option<usize>,
    source_arm_count: Option<usize>,
    has_default_arm: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PatternLabel {
    family: PatternFamily,
    label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PatternFamily {
    Constructor,
    StringLiteral,
    Literal,
}

type EvidenceDedupeKey = (
    String,
    String,
    usize,
    usize,
    String,
    Vec<String>,
    Option<usize>,
    Option<usize>,
    Option<bool>,
);

type FindingDedupeKey = (&'static str, &'static str, String, Vec<String>, Vec<EvidenceDedupeKey>);

type MatchDispatchSiteDedupeKey = (String, String, Vec<PatternLabel>, usize, bool, EvidenceDedupeKey);

fn is_stdlib_node(node: &CodegraphNode) -> bool {
    node.module_path
        .first()
        .is_some_and(|segment| segment == stdlib::INCAN_STD_NAMESPACE)
}

fn match_dispatch_site_from_node(node: &CodegraphNode) -> Option<MatchDispatchSite> {
    let domain_key = node.facts.get("domain_key")?.clone();
    let domain_label = node.facts.get("domain_label")?.clone();
    let owner = node
        .facts
        .get("owner")
        .cloned()
        .unwrap_or_else(|| "<unknown>".to_string());
    let pattern_labels = parse_json_string_list(node.facts.get("pattern_labels")?)?;
    let pattern_families = node
        .facts
        .get("pattern_families")
        .and_then(|value| parse_json_string_list(value))
        .unwrap_or_default();

    if pattern_labels.len() < 2 {
        return None;
    }

    let patterns = pattern_labels
        .iter()
        .enumerate()
        .map(|(idx, label)| PatternLabel {
            family: pattern_families
                .get(idx)
                .and_then(|family| pattern_family_from_fact(family))
                .unwrap_or_else(|| infer_pattern_family(label)),
            label: label.clone(),
        })
        .collect::<BTreeSet<_>>();
    let arm_labels = patterns.iter().map(|pattern| pattern.label.clone()).collect::<Vec<_>>();

    Some(MatchDispatchSite {
        domain_key,
        domain_label: domain_label.clone(),
        explicit_pattern_count: node
            .facts
            .get("explicit_pattern_count")
            .and_then(|count| count.parse().ok())
            .unwrap_or(patterns.len()),
        has_default_arm: node.facts.get("has_default_arm").is_some_and(|value| value == "true"),
        patterns,
        evidence: MatchEvidence {
            file_path: node.file_path.clone().unwrap_or_else(|| "<unknown>".to_string()),
            owner,
            line: node.facts.get("line").and_then(|line| line.parse().ok()).unwrap_or(1),
            column: node
                .facts
                .get("column")
                .and_then(|column| column.parse().ok())
                .unwrap_or(1),
            summary: format!("match over `{domain_label}`"),
            arm_labels,
            explicit_arm_count: Some(
                node.facts
                    .get("explicit_pattern_count")
                    .and_then(|count| count.parse().ok())
                    .unwrap_or(pattern_labels.len()),
            ),
            source_arm_count: Some(
                node.facts
                    .get("arm_count")
                    .and_then(|count| count.parse().ok())
                    .unwrap_or(pattern_labels.len()),
            ),
            has_default_arm: Some(node.facts.get("has_default_arm").is_some_and(|value| value == "true")),
        },
    })
}

fn parse_json_string_list(value: &str) -> Option<Vec<String>> {
    serde_json::from_str(value).ok()
}

fn pattern_family_from_fact(value: &str) -> Option<PatternFamily> {
    match value {
        "constructor" => Some(PatternFamily::Constructor),
        "string_literal" => Some(PatternFamily::StringLiteral),
        "literal" => Some(PatternFamily::Literal),
        _ => None,
    }
}

fn infer_pattern_family(label: &str) -> PatternFamily {
    if label.starts_with('"') {
        PatternFamily::StringLiteral
    } else if label.ends_with("(...)") {
        PatternFamily::Constructor
    } else {
        PatternFamily::Literal
    }
}

fn fail_fast_call_findings(document: &CodegraphDocument) -> Vec<ArchitectureFinding> {
    let declarations = document
        .nodes
        .iter()
        .filter(|node| node.kind == CodegraphNodeKind::Declaration)
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let declaration_by_body_fact = document
        .edges
        .iter()
        .filter(|edge| edge.kind == CodegraphEdgeKind::Contains)
        .filter(|edge| declarations.contains_key(edge.source_id.as_str()))
        .map(|edge| (edge.target_id.as_str(), edge.source_id.as_str()))
        .collect::<BTreeMap<_, _>>();

    let mut findings = Vec::new();
    for call in document
        .nodes
        .iter()
        .filter(|node| node.kind == CodegraphNodeKind::CallSite)
    {
        let Some(call_kind) = fail_fast_call_kind(call) else {
            continue;
        };
        let Some(declaration_id) = declaration_by_body_fact.get(call.id.as_str()) else {
            continue;
        };
        let Some(declaration) = declarations.get(declaration_id) else {
            continue;
        };
        findings.push(fail_fast_call_finding(call, declaration, call_kind));
    }
    findings
}

fn fail_fast_call_kind(call: &CodegraphNode) -> Option<&'static str> {
    let callee_key = call.facts.get("callee_key")?.as_str();
    let callee_label = call.facts.get("callee_label").map_or("", String::as_str);
    match (callee_key, callee_label) {
        ("method:unwrap", _) | (_, ".unwrap()") => Some("unwrap"),
        ("method:expect", _) | (_, ".expect()") => Some("expect"),
        ("call:ident:panic", _) | (_, "panic(...)") => Some("panic"),
        ("call:ident:todo", _) | (_, "todo(...)") => Some("todo"),
        ("call:ident:unreachable", _) | (_, "unreachable(...)") => Some("unreachable"),
        _ => None,
    }
}

fn fail_fast_call_finding(call: &CodegraphNode, declaration: &CodegraphNode, call_kind: &str) -> ArchitectureFinding {
    let is_public_boundary = declaration_is_public(declaration);
    let priority = if is_public_boundary { "P1" } else { "P2" };
    let boundary_phrase = if is_public_boundary {
        "a public API boundary"
    } else {
        "an internal boundary"
    };
    let owner = call
        .facts
        .get("owner")
        .cloned()
        .unwrap_or_else(|| declaration.label.clone());
    let callee_label = call
        .facts
        .get("callee_label")
        .cloned()
        .unwrap_or_else(|| call.label.clone());
    let pressure = format!(
        "`{}` calls fail-fast `{callee_label}` inside {boundary_phrase}; recoverable errors can become process-level failures.",
        declaration.label
    );
    let title = if is_public_boundary {
        format!("Public API boundary calls fail-fast `{call_kind}`")
    } else {
        format!("Internal boundary calls fail-fast `{call_kind}`")
    };

    ArchitectureFinding {
        code: "arch.fail_fast_boundary_call",
        priority,
        title,
        pressure,
        suggestions: vec![
            "Return a typed Result/Error value across the boundary instead of failing inside it.".to_string(),
            "Move fail-fast handling to the executable edge if this is intentionally unrecoverable.".to_string(),
        ],
        risks: vec![
            "Do not replace fail-fast calls blindly when an invariant violation really should abort.".to_string(),
            "Check whether the caller can usefully recover before widening the error surface.".to_string(),
        ],
        shared_patterns: Vec::new(),
        shared_pattern_count: 0,
        largest_pattern_count: 0,
        default_arm_site_count: 0,
        evidence: vec![body_fact_evidence(call, owner, format!("callee: {callee_label}"))],
    }
}

fn declaration_is_public(declaration: &CodegraphNode) -> bool {
    declaration
        .facts
        .get("visibility")
        .is_some_and(|visibility| visibility == "public")
        || declaration.facts.contains_key("checked_api_anchor_id")
}

fn body_fact_evidence(node: &CodegraphNode, owner: String, summary: String) -> MatchEvidence {
    MatchEvidence {
        file_path: node.file_path.clone().unwrap_or_else(|| "<unknown>".to_string()),
        owner,
        line: node.facts.get("line").and_then(|line| line.parse().ok()).unwrap_or(1),
        column: node
            .facts
            .get("column")
            .and_then(|column| column.parse().ok())
            .unwrap_or(1),
        summary,
        arm_labels: Vec::new(),
        explicit_arm_count: None,
        source_arm_count: None,
        has_default_arm: None,
    }
}

fn deduplicate_findings(findings: &mut Vec<ArchitectureFinding>) {
    let mut seen = BTreeSet::new();
    findings.retain(|finding| seen.insert(finding_dedupe_key(finding)));
}

fn finding_dedupe_key(finding: &ArchitectureFinding) -> FindingDedupeKey {
    let mut evidence = finding.evidence.iter().map(evidence_dedupe_key).collect::<Vec<_>>();
    evidence.sort();
    (
        finding.code,
        finding.priority,
        finding.title.clone(),
        finding.shared_patterns.clone(),
        evidence,
    )
}

fn deduplicate_match_dispatch_sites(sites: &mut Vec<MatchDispatchSite>) {
    let mut seen = BTreeSet::new();
    sites.retain(|site| seen.insert(match_dispatch_site_dedupe_key(site)));
}

fn match_dispatch_site_dedupe_key(site: &MatchDispatchSite) -> MatchDispatchSiteDedupeKey {
    (
        site.domain_key.clone(),
        site.domain_label.clone(),
        site.patterns.iter().cloned().collect(),
        site.explicit_pattern_count,
        site.has_default_arm,
        evidence_dedupe_key(&site.evidence),
    )
}

fn evidence_dedupe_key(evidence: &MatchEvidence) -> EvidenceDedupeKey {
    (
        evidence.file_path.clone(),
        evidence.owner.clone(),
        evidence.line,
        evidence.column,
        evidence.summary.clone(),
        evidence.arm_labels.clone(),
        evidence.explicit_arm_count,
        evidence.source_arm_count,
        evidence.has_default_arm,
    )
}

fn repeated_dispatch_findings(sites: &[MatchDispatchSite]) -> Vec<ArchitectureFinding> {
    let mut groups: BTreeMap<&str, Vec<&MatchDispatchSite>> = BTreeMap::new();
    for site in sites {
        groups.entry(&site.domain_key).or_default().push(site);
    }

    let mut findings = Vec::new();
    for grouped_sites in groups.into_values() {
        if grouped_sites.len() < 2 {
            continue;
        }
        let shared_patterns = shared_patterns(&grouped_sites);
        if shared_patterns.len() < 2 {
            continue;
        }
        if is_mechanical_wrapper_pattern_set(&shared_patterns) {
            continue;
        }
        if is_low_signal_default_overlap(&shared_patterns, &grouped_sites) {
            continue;
        }
        let Some(first) = grouped_sites.first() else {
            continue;
        };
        findings.push(repeated_dispatch_finding(
            first.domain_label.clone(),
            shared_patterns,
            grouped_sites,
        ));
    }
    findings
}

fn repeated_dispatch_finding(
    domain_label: String,
    shared_patterns: Vec<PatternLabel>,
    sites: Vec<&MatchDispatchSite>,
) -> ArchitectureFinding {
    let pattern_labels = shared_patterns
        .iter()
        .map(|pattern| pattern.label.clone())
        .collect::<Vec<_>>();
    let largest_pattern_count = largest_pattern_count(&sites);
    let default_arm_site_count = sites.iter().filter(|site| site.has_default_arm).count();
    let pattern_family = dominant_pattern_family(&shared_patterns);
    let priority = repeated_dispatch_priority(pattern_family, sites.len(), shared_patterns.len());
    let suggestions = suggestions_for(pattern_family, sites.len());
    let risks = vec![
        "Do not introduce a registry if the matched domain is intentionally closed and exhaustiveness matters."
            .to_string(),
        "Do not introduce a visitor/table abstraction if the repeated matches are clearer as local domain logic."
            .to_string(),
    ];
    let pressure = format!(
        "{} match expressions dispatch over `{}` and share {}/{} explicit arms: {}",
        sites.len(),
        domain_label,
        pattern_labels.len(),
        largest_pattern_count,
        pattern_labels.join(", ")
    );

    ArchitectureFinding {
        code: "arch.repeated_match_dispatch",
        priority,
        title: format!("Repeated match dispatch over `{domain_label}`"),
        pressure,
        suggestions,
        risks,
        shared_patterns: pattern_labels,
        shared_pattern_count: shared_patterns.len(),
        largest_pattern_count,
        default_arm_site_count,
        evidence: sites.into_iter().map(|site| site.evidence.clone()).collect(),
    }
}

fn repeated_dispatch_priority(
    pattern_family: PatternFamily,
    site_count: usize,
    shared_pattern_count: usize,
) -> &'static str {
    if pattern_family == PatternFamily::StringLiteral && site_count >= 3 && shared_pattern_count >= 3 {
        "P2"
    } else {
        "P3"
    }
}

fn largest_pattern_count(sites: &[&MatchDispatchSite]) -> usize {
    sites.iter().map(|site| site.explicit_pattern_count).max().unwrap_or(0)
}

fn is_low_signal_default_overlap(shared_patterns: &[PatternLabel], sites: &[&MatchDispatchSite]) -> bool {
    let largest_pattern_count = largest_pattern_count(sites);
    if largest_pattern_count == 0 {
        return false;
    }
    let shared_count = shared_patterns.len();
    let overlap_is_tiny = shared_count * 10 <= largest_pattern_count * 3;
    let default_subset_site = sites
        .iter()
        .any(|site| site.has_default_arm && site.explicit_pattern_count <= shared_count);

    overlap_is_tiny && default_subset_site
}

fn shared_patterns(sites: &[&MatchDispatchSite]) -> Vec<PatternLabel> {
    let Some((first, rest)) = sites.split_first() else {
        return Vec::new();
    };
    let mut shared = first.patterns.clone();
    for site in rest {
        shared = shared.intersection(&site.patterns).cloned().collect();
    }
    shared.into_iter().collect()
}

fn is_mechanical_wrapper_pattern_set(patterns: &[PatternLabel]) -> bool {
    let labels = patterns
        .iter()
        .map(|pattern| pattern.label.as_str())
        .collect::<Vec<_>>();
    matches!(
        labels.as_slice(),
        ["Err(...)", "Ok(...)"] | ["None", "Some(...)"] | ["Some(...)", "None"]
    )
}

fn dominant_pattern_family(patterns: &[PatternLabel]) -> PatternFamily {
    let mut counts: BTreeMap<PatternFamily, usize> = BTreeMap::new();
    for pattern in patterns {
        *counts.entry(pattern.family).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(family, _)| family)
        .unwrap_or(PatternFamily::Literal)
}

fn suggestions_for(pattern_family: PatternFamily, site_count: usize) -> Vec<String> {
    let mut suggestions = Vec::new();
    match pattern_family {
        PatternFamily::StringLiteral => {
            suggestions.push(
                "Consider replacing the string-literal domain with an enum or value enum if the set is closed."
                    .to_string(),
            );
            suggestions.push(
                "If libraries should add cases, consider a registry keyed by the parsed enum/value object instead of raw strings."
                    .to_string(),
            );
        }
        PatternFamily::Constructor => {
            suggestions.push(
                "Decide whether this is intentionally exhaustive local logic or a growing operation boundary."
                    .to_string(),
            );
            suggestions.push(
                "If it is a growing operation boundary, prefer a visitor/table/registry adapter outside the domain type when the operation belongs to another subsystem."
                    .to_string(),
            );
            suggestions.push(
                "Keep local exhaustive matches when they are clearer than an abstraction and the case set changes rarely."
                    .to_string(),
            );
        }
        PatternFamily::Literal => {
            suggestions.push(
                "Consider naming the literal domain with an enum/newtype before adding more branch sites.".to_string(),
            );
        }
    }
    if site_count >= 3 {
        suggestions.push(
            "Because this appears in three or more places, check whether adding one case requires shotgun edits."
                .to_string(),
        );
    }
    suggestions
}

fn sort_findings(findings: &mut [ArchitectureFinding]) {
    findings.sort_by(|left, right| {
        priority_rank(left.priority)
            .cmp(&priority_rank(right.priority))
            .then_with(|| left.code.cmp(right.code))
            .then_with(|| left.title.cmp(&right.title))
    });
}

fn priority_rank(priority: &str) -> usize {
    match priority {
        "P1" => 0,
        "P2" => 1,
        "P3" => 2,
        _ => 3,
    }
}

fn print_text_report(report: &ArchitectureReport) {
    if report.findings.is_empty() {
        println!("Architecture Findings");
        println!();
        println!("No deterministic architecture findings.");
        return;
    }

    println!("Architecture Findings");
    for finding in &report.findings {
        println!();
        println!("[{}] {}", finding.priority, finding.title);
        println!("Pressure: {}", finding.pressure);
        if finding.default_arm_site_count > 0 {
            println!(
                "Default context: {} of {} sites include a wildcard/default arm.",
                finding.default_arm_site_count,
                finding.evidence.len()
            );
        }
        println!("Suggestions:");
        for suggestion in &finding.suggestions {
            println!("  - {suggestion}");
        }
        println!("Risks:");
        for risk in &finding.risks {
            println!("  - {risk}");
        }
        println!("Evidence:");
        for evidence in &finding.evidence {
            print!(
                "  - {}:{}:{} in {}",
                evidence.file_path, evidence.line, evidence.column, evidence.owner
            );
            if !evidence.summary.is_empty() {
                print!(" ({})", evidence.summary);
            }
            if let (Some(explicit_arm_count), Some(source_arm_count), Some(has_default_arm)) = (
                evidence.explicit_arm_count,
                evidence.source_arm_count,
                evidence.has_default_arm,
            ) {
                print!(
                    " (explicit arms: {explicit_arm_count}/{source_arm_count}; fallback: {}; arms: {})",
                    if has_default_arm { "yes" } else { "no" },
                    evidence.arm_labels.join(", ")
                );
            }
            println!();
        }
    }
}

fn print_json_report(report: &ArchitectureReport) -> CliResult<()> {
    let findings = report
        .findings
        .iter()
        .map(|finding| {
            json!({
                "code": finding.code,
                "priority": finding.priority,
                "title": finding.title,
                "pressure": finding.pressure,
                "suggestions": finding.suggestions,
                "risks": finding.risks,
                "shared_patterns": finding.shared_patterns,
                "shared_pattern_count": finding.shared_pattern_count,
                "largest_pattern_count": finding.largest_pattern_count,
                "default_arm_site_count": finding.default_arm_site_count,
                "evidence": finding.evidence.iter().map(|evidence| {
                    json!({
                        "file_path": evidence.file_path,
                        "owner": evidence.owner,
                        "line": evidence.line,
                        "column": evidence.column,
                        "summary": evidence.summary,
                        "arm_labels": evidence.arm_labels,
                        "explicit_arm_count": evidence.explicit_arm_count,
                        "source_arm_count": evidence.source_arm_count,
                        "has_default_arm": evidence.has_default_arm,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let output = serde_json::to_string_pretty(&json!({ "findings": findings }))
        .map_err(|error| CliError::failure(format!("failed to serialize architecture report: {error}")))?;
    println!("{output}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use incan_codegraph::{CodegraphEdge, CodegraphEdgeKind, CodegraphNode, CodegraphSpan};

    use super::*;

    fn report_for_source(source: &str) -> Result<ArchitectureReport, Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("architect_severity.incn");
        fs::write(&path, source)?;
        let document = collect_codegraph_document_suppressing_diagnostics(&path, true)?;
        Ok(ArchitectureReport::from_codegraph(&document))
    }

    fn report_for_directory(entries: &[(&str, &str)]) -> Result<ArchitectureReport, Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        for (path, source) in entries {
            let file_path = tmp.path().join(path);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(file_path, source)?;
        }
        let document =
            collect_codegraph_document_suppressing_diagnostics(&resolve_architect_scan_path(tmp.path())?, true)?;
        Ok(ArchitectureReport::from_codegraph(&document))
    }

    fn contains_edge(source_id: &str, target_id: &str) -> CodegraphEdge {
        let mut facts = BTreeMap::new();
        facts.insert("relation".to_string(), "test_contains".to_string());
        CodegraphEdge {
            id: format!("edge:{source_id}:{target_id}"),
            kind: CodegraphEdgeKind::Contains,
            source_id: source_id.to_string(),
            target_id: target_id.to_string(),
            span: None,
            facts,
        }
    }

    fn declaration_node(id: &str, label: &str, visibility: &str) -> CodegraphNode {
        let mut facts = BTreeMap::new();
        facts.insert("declaration_kind".to_string(), "function".to_string());
        facts.insert("visibility".to_string(), visibility.to_string());
        CodegraphNode {
            id: id.to_string(),
            kind: CodegraphNodeKind::Declaration,
            label: label.to_string(),
            file_path: Some("src/api.incn".to_string()),
            module_path: vec!["src".to_string(), "api".to_string()],
            span: Some(CodegraphSpan { start: 0, end: 10 }),
            facts,
        }
    }

    fn call_site_node(id: &str, line: usize) -> CodegraphNode {
        let mut facts = BTreeMap::new();
        facts.insert("owner".to_string(), "public_boundary".to_string());
        facts.insert("line".to_string(), line.to_string());
        facts.insert("column".to_string(), "12".to_string());
        facts.insert("callee_key".to_string(), "method:unwrap".to_string());
        facts.insert("callee_label".to_string(), ".unwrap()".to_string());
        CodegraphNode {
            id: id.to_string(),
            kind: CodegraphNodeKind::CallSite,
            label: ".unwrap()".to_string(),
            file_path: Some("src/api.incn".to_string()),
            module_path: vec!["src".to_string(), "api".to_string()],
            span: Some(CodegraphSpan { start: 10, end: 18 }),
            facts,
        }
    }

    fn match_dispatch_node(
        id: &str,
        owner: &str,
        domain_key: &str,
        domain_label: &str,
        patterns: &[&str],
        families: &[&str],
        has_default_arm: bool,
    ) -> CodegraphNode {
        let mut facts = BTreeMap::new();
        facts.insert("owner".to_string(), owner.to_string());
        facts.insert("line".to_string(), "3".to_string());
        facts.insert("column".to_string(), "5".to_string());
        facts.insert("domain_key".to_string(), domain_key.to_string());
        facts.insert("domain_label".to_string(), domain_label.to_string());
        facts.insert("explicit_pattern_count".to_string(), patterns.len().to_string());
        facts.insert(
            "arm_count".to_string(),
            (patterns.len() + usize::from(has_default_arm)).to_string(),
        );
        facts.insert("has_default_arm".to_string(), has_default_arm.to_string());
        facts.insert("pattern_labels".to_string(), json!(patterns).to_string());
        facts.insert("pattern_families".to_string(), json!(families).to_string());

        CodegraphNode {
            id: id.to_string(),
            kind: CodegraphNodeKind::MatchDispatch,
            label: format!("match {domain_label}"),
            file_path: Some("demo.incn".to_string()),
            module_path: vec!["demo".to_string()],
            span: Some(CodegraphSpan { start: 0, end: 10 }),
            facts,
        }
    }

    #[test]
    fn repeated_string_match_dispatch_reports_finding() {
        let mut document = CodegraphDocument::new(None);
        document.push_node(match_dispatch_node(
            "body-fact:match:label",
            "label",
            "ident:kind",
            "kind",
            &["\"create\"", "\"update\""],
            &["string_literal", "string_literal"],
            true,
        ));
        document.push_node(match_dispatch_node(
            "body-fact:match:severity",
            "severity",
            "ident:kind",
            "kind",
            &["\"create\"", "\"update\""],
            &["string_literal", "string_literal"],
            true,
        ));

        let report = ArchitectureReport::from_codegraph(&document);

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "arch.repeated_match_dispatch");
        assert_eq!(
            report.findings[0].shared_patterns,
            vec!["\"create\"".to_string(), "\"update\"".to_string()]
        );
    }

    #[test]
    fn single_match_does_not_report_finding() {
        let mut document = CodegraphDocument::new(None);
        document.push_node(match_dispatch_node(
            "body-fact:match:label",
            "label",
            "ident:kind",
            "kind",
            &["\"create\"", "\"update\""],
            &["string_literal", "string_literal"],
            true,
        ));

        let report = ArchitectureReport::from_codegraph(&document);

        assert!(report.findings.is_empty());
    }

    #[test]
    fn tiny_overlap_with_default_fallback_is_suppressed() {
        let mut document = CodegraphDocument::new(None);
        document.push_node(match_dispatch_node(
            "body-fact:match:catalog",
            "core_scenario",
            "ident:key",
            "key",
            &[
                "CoreScenarioKey.ReferenceRelSharedSubplan(...)",
                "CoreScenarioKey.SetRelOperations(...)",
                "CoreScenarioKey.AggregateGroupingSets(...)",
                "CoreScenarioKey.CrossRelCartesian(...)",
                "CoreScenarioKey.FetchRelLimitOffset(...)",
                "CoreScenarioKey.FilterRows(...)",
                "CoreScenarioKey.JoinRelVariants(...)",
                "CoreScenarioKey.ProjectComputedColumns(...)",
                "CoreScenarioKey.ReadLocalFiles(...)",
                "CoreScenarioKey.ReadNamedTable(...)",
                "CoreScenarioKey.ReadVirtualTable(...)",
                "CoreScenarioKey.SortRelOrdering(...)",
            ],
            &[
                "constructor",
                "constructor",
                "constructor",
                "constructor",
                "constructor",
                "constructor",
                "constructor",
                "constructor",
                "constructor",
                "constructor",
                "constructor",
                "constructor",
            ],
            false,
        ));
        document.push_node(match_dispatch_node(
            "body-fact:match:invariants",
            "scenario_matches_key_invariants",
            "ident:key",
            "key",
            &[
                "CoreScenarioKey.ReferenceRelSharedSubplan(...)",
                "CoreScenarioKey.SetRelOperations(...)",
            ],
            &["constructor", "constructor"],
            true,
        ));

        let report = ArchitectureReport::from_codegraph(&document);

        assert!(
            report.findings.is_empty(),
            "expected low-overlap default-heavy repeated dispatch to be treated as noise"
        );
    }

    #[test]
    fn constructor_dispatch_suggestion_frames_closed_domain_as_decision() {
        let mut document = CodegraphDocument::new(None);
        document.push_node(match_dispatch_node(
            "body-fact:match:register",
            "register_backend",
            "ident:source_kind",
            ".source_kind",
            &["SourceKind.Arrow(...)", "SourceKind.Csv(...)"],
            &["constructor", "constructor"],
            false,
        ));
        document.push_node(match_dispatch_node(
            "body-fact:match:schema",
            "infer_schema",
            "ident:source_kind",
            ".source_kind",
            &["SourceKind.Arrow(...)", "SourceKind.Csv(...)"],
            &["constructor", "constructor"],
            false,
        ));

        let report = ArchitectureReport::from_codegraph(&document);

        assert_eq!(report.findings.len(), 1);
        let suggestions = report.findings[0].suggestions.join("\n");
        assert!(
            suggestions.contains("intentionally exhaustive local logic or a growing operation boundary"),
            "expected constructor-domain suggestion to frame the choice as a decision: {suggestions}"
        );
        assert!(
            !suggestions.contains("moving repeated operations onto the enum"),
            "constructor-domain suggestion should not imply subsystem behavior belongs on the enum: {suggestions}"
        );
    }

    #[test]
    fn directory_scan_includes_unimported_project_sources() -> Result<(), Box<dyn std::error::Error>> {
        let report = report_for_directory(&[
            (
                "src/main.incn",
                r#"
pub def main_value() -> int:
    return 1
"#,
            ),
            (
                "src/adapter.incn",
                r#"
pub def public_boundary(raw: Result[int, str]) -> int:
    return raw.unwrap()
"#,
            ),
        ])?;

        assert!(
            report.findings.iter().any(|finding| {
                finding.code == "arch.fail_fast_boundary_call"
                    && finding.priority == "P1"
                    && finding
                        .evidence
                        .iter()
                        .any(|evidence| evidence.file_path == "src/adapter.incn")
            }),
            "expected directory architect scan to include unimported source files: {report:#?}"
        );
        Ok(())
    }

    #[test]
    fn duplicate_body_facts_do_not_duplicate_findings() {
        let mut document = CodegraphDocument::new(None);
        document.push_node(declaration_node(
            "decl:api::public_boundary",
            "public_boundary",
            "public",
        ));
        document.push_node(call_site_node("body-fact:call:first", 7));
        document.push_node(call_site_node("body-fact:call:duplicate", 7));
        document.push_edge(contains_edge("decl:api::public_boundary", "body-fact:call:first"));
        document.push_edge(contains_edge("decl:api::public_boundary", "body-fact:call:duplicate"));

        let report = ArchitectureReport::from_codegraph(&document);
        let fail_fast_findings = report
            .findings
            .iter()
            .filter(|finding| finding.code == "arch.fail_fast_boundary_call")
            .collect::<Vec<_>>();

        assert_eq!(
            fail_fast_findings.len(),
            1,
            "expected identical fail-fast evidence to be reported once: {report:#?}"
        );
    }

    #[test]
    fn deliberately_bad_source_flags_p1_and_p2_findings() -> Result<(), Box<dyn std::error::Error>> {
        let report = report_for_source(
            r#"
pub def public_boundary(raw: Result[int, str]) -> int:
    return raw.unwrap()

def label(kind: str) -> str:
    match kind:
        "create" => "Create"
        "update" => "Update"
        "delete" => "Delete"
        _ => "Other"

def severity(kind: str) -> int:
    match kind:
        "create" => 1
        "update" => 2
        "delete" => 3
        _ => 0

def route(kind: str) -> str:
    match kind:
        "create" => "/new"
        "update" => "/edit"
        "delete" => "/remove"
        _ => "/"
"#,
        )?;

        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == "arch.fail_fast_boundary_call" && finding.priority == "P1"),
            "expected public fail-fast boundary call to be flagged as P1: {report:#?}"
        );
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == "arch.repeated_match_dispatch"
                    && finding.priority == "P2"
                    && finding.shared_patterns.len() == 3),
            "expected repeated raw string dispatch across three sites to be flagged as P2: {report:#?}"
        );
        Ok(())
    }
}
