//! User-facing inspection and pruning for Incan-managed generated-build caches.

use crate::cli::{CacheCategory, CliError, CliResult, ExitCode};
use crate::generated_cache::{inspect_default_cache, prune_default_cache};

/// Inspect the managed generated-build cache in human-readable or JSON form.
pub fn inspect_generated_cache(category: CacheCategory, json: bool) -> CliResult<ExitCode> {
    let CacheCategory::GeneratedCargo = category;
    let inspection = inspect_default_cache()
        .map_err(|error| CliError::failure(format!("failed to inspect generated cache: {error}")))?;
    if json {
        let payload = serde_json::to_string_pretty(&inspection)
            .map_err(|error| CliError::failure(format!("failed to serialize generated cache report: {error}")))?;
        println!("{payload}");
        return Ok(ExitCode::SUCCESS);
    }

    println!("Generated Cargo cache: {}", inspection.root.display());
    println!(
        "Logical usage: {} across {} compatibility domain(s); soft limit {}",
        human_bytes(inspection.total_bytes),
        inspection.entries.len(),
        human_bytes(inspection.max_bytes)
    );
    for entry in inspection.entries {
        println!(
            "  {}  {}  {:>10}  {:<7}  profile={}  last_used_unix_seconds={}",
            entry.category,
            entry.identity,
            human_bytes(entry.bytes),
            if entry.active { "active" } else { "idle" },
            entry.profile,
            entry.last_used_unix_seconds,
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Prune inactive compatibility domains toward a soft limit or remove exact identities.
pub fn prune_generated_cache(
    category: CacheCategory,
    max_bytes: Option<u64>,
    dry_run: bool,
    identities: &[String],
    json: bool,
) -> CliResult<ExitCode> {
    let CacheCategory::GeneratedCargo = category;
    let report = prune_default_cache(max_bytes, dry_run, identities)
        .map_err(|error| CliError::failure(format!("failed to prune generated cache: {error}")))?;
    if json {
        let payload = serde_json::to_string_pretty(&report)
            .map_err(|error| CliError::failure(format!("failed to serialize generated cache report: {error}")))?;
        println!("{payload}");
        return Ok(ExitCode::SUCCESS);
    }

    let action = if dry_run {
        "Would reduce logical usage by"
    } else {
        "Reduced logical usage by"
    };
    println!(
        "{action} {} from {} ({} -> {}, limit {}).",
        human_bytes(report.removed_logical_bytes),
        report.root.display(),
        human_bytes(report.before_bytes),
        human_bytes(report.after_bytes),
        human_bytes(report.max_bytes)
    );
    let entry_action = prune_entry_action(dry_run);
    println!(
        "{entry_action} {} compatibility domain(s).",
        report.removed_entries.len()
    );
    for identity in &report.removed_entries {
        println!("  {} {identity}", entry_action.to_ascii_lowercase());
    }
    if !report.skipped_active_entries.is_empty() {
        println!(
            "Skipped {} active compatibility domain(s).",
            report.skipped_active_entries.len()
        );
    }
    for identity in &report.skipped_active_entries {
        println!("  active {identity}");
    }
    for identity in &report.not_found_identities {
        println!("  not found {identity}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Describe entry removal without claiming a dry-run changed storage.
fn prune_entry_action(dry_run: bool) -> &'static str {
    if dry_run { "Would remove" } else { "Removed" }
}

/// Render byte counts compactly without adding a formatting dependency to the compiler.
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::{human_bytes, prune_entry_action};

    #[test]
    fn formats_binary_byte_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024 * 2), "2.0 MiB");
    }

    #[test]
    fn dry_run_uses_predictive_removal_language() {
        assert_eq!(prune_entry_action(true), "Would remove");
        assert_eq!(prune_entry_action(false), "Removed");
    }
}
