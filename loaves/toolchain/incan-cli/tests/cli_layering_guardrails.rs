//! Guard the CLI-to-compiler layering boundary against further erosion.
//!
//! The CLI is meant to orchestrate the compiler, not to reimplement it. Where that boundary slipped, one rule ended
//! up with two implementations that could disagree: RFC 120 records that "a check living in the CLI could not agree
//! with one living in the frontend by construction", and #1293 found a live instance -- manifest export projection
//! in `loaves/toolchain/incan-cli/src/commands/build.rs` disagreeing with the validator in the frontend's
//! `library_manifest` about which hop of a re-export chain a path described.
//!
//! This suite does not refactor anything and does not judge the existing reach-ins. It records them, so the set can
//! only shrink. Removing an entry is the work tracked by #1298; adding one fails here first.

use incan_test_support as support;
use support::repo_root;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;

const BASELINE_PATH: &str = "loaves/toolchain/incan-cli/tests/fixtures/cli_layering/cli_compiler_reach_in.json";
/// The toolchain ring's sources: both binaries' packages, since `oven-cli` carries the `oven`, `lock` and `tools`
/// handlers `incan` mounts and reaches the compiler through them the same way.
const TOOLCHAIN_ROOTS: [&str; 2] = ["loaves/toolchain/incan-cli/src", "loaves/toolchain/oven-cli/src"];

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One recorded file and the compiler modules it is currently allowed to reach.
#[derive(Debug, Deserialize)]
struct BaselineFile {
    path: String,
    reaches: Vec<String>,
}

/// The recorded CLI-to-compiler reach-in surface.
#[derive(Debug, Deserialize)]
struct Baseline {
    files: Vec<BaselineFile>,
}

/// Collect the distinct compiler-layer modules one source file names.
///
/// Matching is textual and deliberately coarse: it keys on `<layer>::<module>` rather than on resolved item paths,
/// because the concern is which compiler areas the CLI reaches into at all, not how many times or through which item.
/// A coarser key also keeps the baseline readable and its diffs meaningful. The layers are spelled as the ring
/// crates the CLI links: the frontend crate, the lowering and emission crates, and the backend the driver crate
/// carries — a reach-in spelled through `incan_ir::` or `incan_emit::` is the same reach-in that used to read
/// `crate::backend::`.
fn compiler_modules_named_by(source: &str) -> BTreeSet<String> {
    const LAYERS: [&str; 4] = [
        "incan_frontend::",
        "incan_ir::",
        "incan_emit::",
        "incan_driver::backend::",
    ];
    // The root crate re-exported these three at its top level as the toolchain's sanctioned entry points into the
    // frontend (`incan::compiler_stack`, `incan::library_manifest`, `incan::provider`); spelled through the ring
    // crate they are the same doors, not reach-ins past them.
    const SANCTIONED_FRONTEND_ENTRY_POINTS: [&str; 3] = ["compiler_stack", "library_manifest", "provider"];
    let mut modules = BTreeSet::new();
    for layer in LAYERS {
        for (index, _) in source.match_indices(layer) {
            if index > 0
                && source[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|previous| previous.is_ascii_alphanumeric() || previous == '_' || previous == ':')
            {
                continue;
            }
            let rest = &source[index + layer.len()..];
            let module: String = rest
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect();
            // Only a module segment counts. `incan_driver::backend::IrCodegen` names a type re-exported from the
            // layer root, which is the sanctioned entry point; `incan_driver::backend::ir` reaches past it into the
            // layer's internals, and that is the distinction this guard is about. Rust's naming convention separates
            // the two reliably.
            if !module.starts_with(|character: char| character.is_ascii_lowercase()) {
                continue;
            }
            if layer == "incan_frontend::" && SANCTIONED_FRONTEND_ENTRY_POINTS.contains(&module.as_str()) {
                continue;
            }
            modules.insert(format!("{layer}{module}"));
        }
    }
    modules
}

/// Walk the toolchain packages' sources and report the compiler modules each file reaches into.
fn observed_reach_in(root: &Path) -> Result<BTreeMap<String, BTreeSet<String>>, Box<dyn std::error::Error>> {
    let mut observed = BTreeMap::new();
    let mut pending = TOOLCHAIN_ROOTS
        .iter()
        .map(|toolchain_root| root.join(toolchain_root))
        .collect::<Vec<_>>();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }
            let modules = compiler_modules_named_by(&fs::read_to_string(&path)?);
            if modules.is_empty() {
                continue;
            }
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("a CLI source path was not valid UTF-8")?
                .replace('\\', "/");
            observed.insert(relative, modules);
        }
    }
    Ok(observed)
}

/// The CLI layer may not reach into compiler internals it has not already reached into.
///
/// A failure here is not a request to update the baseline. It means a CLI file gained a dependency on a compiler
/// area, which is the direction #1298 is trying to reverse. Move the logic behind the frontend or backend boundary
/// instead; regenerate the baseline only when an entry is being *removed*.
#[test]
fn cli_does_not_reach_further_into_the_compiler_than_recorded() -> TestResult {
    let root = repo_root();
    let baseline: Baseline = serde_json::from_str(&fs::read_to_string(root.join(BASELINE_PATH))?)?;
    let recorded: BTreeMap<String, BTreeSet<String>> = baseline
        .files
        .into_iter()
        .map(|file| (file.path, file.reaches.into_iter().collect()))
        .collect();
    let observed = observed_reach_in(&root)?;

    let mut added: Vec<String> = Vec::new();
    for (path, modules) in &observed {
        let allowed = recorded.get(path);
        for module in modules {
            if !allowed.is_some_and(|allowed| allowed.contains(module)) {
                added.push(format!("  {path} now reaches {module}"));
            }
        }
    }
    assert!(
        added.is_empty(),
        "the CLI layer reaches further into the compiler than {BASELINE_PATH} records:\n{}\n\nMove the logic behind \
         the frontend or backend boundary rather than widening the baseline. See #1298.",
        added.join("\n")
    );
    Ok(())
}

/// The recorded baseline must not describe reach-ins that no longer exist.
///
/// Keeping it exact is what makes the ratchet mean something: once a dependency is removed, the baseline shrinks with
/// it and cannot silently return later.
#[test]
fn the_cli_layering_baseline_records_no_stale_entries() -> TestResult {
    let root = repo_root();
    let baseline: Baseline = serde_json::from_str(&fs::read_to_string(root.join(BASELINE_PATH))?)?;
    let observed = observed_reach_in(&root)?;

    let mut stale: Vec<String> = Vec::new();
    for file in &baseline.files {
        match observed.get(&file.path) {
            None => stale.push(format!("  {} no longer reaches into the compiler at all", file.path)),
            Some(modules) => {
                for recorded in &file.reaches {
                    if !modules.contains(recorded) {
                        stale.push(format!("  {} no longer reaches {recorded}", file.path));
                    }
                }
            }
        }
    }
    assert!(
        stale.is_empty(),
        "{BASELINE_PATH} records reach-ins that no longer exist:\n{}\n\nRegenerate the baseline to lock the \
         improvement in.",
        stale.join("\n")
    );
    Ok(())
}
