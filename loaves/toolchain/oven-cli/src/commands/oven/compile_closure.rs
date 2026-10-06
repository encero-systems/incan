//! Pinned adopted-closure compilation through the shared SDK executor.

use std::collections::BTreeMap;
use std::path::PathBuf;

use oven_rustc::sdk_closure::{
    ClosureCompileRequest, SdkCompiledClosure, SdkLockedUnit, prepare_closure, validate_locked_root_features,
};
use serde::{Deserialize, Serialize};

use crate::cli::CompileClosureArgs;
use crate::{CliError, CliResult, ExitCode};

/// One index-line dependency declaration requested as a root.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Root {
    name: String,
    loaf: String,
    req: String,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default = "enabled")]
    default_features: bool,
    #[serde(default)]
    optional: bool,
    #[serde(default)]
    target: Option<String>,
}

/// Index declarations enable defaults unless explicitly disabled.
fn enabled() -> bool {
    true
}

/// The public roots exchange binds both compilation domains.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Roots {
    roots: Vec<Root>,
    target: String,
    host: String,
}

/// Resolution units retain the source authority on the result wire.
#[derive(Deserialize)]
struct Lock {
    schema: String,
    units: Vec<SdkLockedUnit>,
}

/// Root artifact and its exact compiled-unit identity.
#[derive(Serialize)]
struct Extern {
    path: PathBuf,
    unit: String,
}

/// A selected unit that the executor could not provide.
#[derive(Serialize)]
struct Refused {
    loaf: String,
    version: String,
    profile: String,
    features: Vec<String>,
    reason: String,
}

/// One compiled unit of the closure with its binding, so a registry can publish every unit, not only the roots.
#[derive(Serialize)]
struct CompiledUnitRecord {
    loaf: String,
    version: String,
    domain: String,
    features: Vec<String>,
    profile: String,
    /// Receipt identity, the same value `externs` reports as `unit`.
    unit: String,
    /// Content-addressed Oven store entry holding the compiled output.
    entry: String,
}

/// The registry-facing closure result, retaining the original resolution document.
#[derive(Serialize)]
struct ResultDocument {
    schema: &'static str,
    lock: serde_json::Value,
    externs: BTreeMap<String, Extern>,
    units: Vec<CompiledUnitRecord>,
    refused: Vec<Refused>,
}

/// Compile the explicit resolution and write named root outcomes while retaining leases through publication.
pub fn run(arguments: CompileClosureArgs) -> CliResult<ExitCode> {
    execute(arguments).map_err(|error| CliError::failure(format!("compile-closure: {error}")))
}

/// Validate request domains and resolution bindings before crossing the shared compiler boundary.
fn execute(arguments: CompileClosureArgs) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let roots: Roots = serde_json::from_slice(&std::fs::read(&arguments.roots)?)?;
    let lock_path = arguments.lock.as_ref().ok_or(
        "resolution unavailable: baking core_engine requires a sealed Oven inspection source authority for Rust dependency blake2; use --lock with an Incan resolution document",
    )?;
    let lock_value: serde_json::Value = serde_json::from_slice(&std::fs::read(lock_path)?)?;
    let lock: Lock = serde_json::from_value(lock_value.clone())?;
    if lock.schema != "incan.oven.loaf-resolution/1" {
        return Err("unsupported resolution schema".into());
    }
    let rustc = match arguments.rustc {
        Some(path) => path,
        None => oven_rustc::rustc::resolve_active_rustc()?,
    };
    if roots.host != oven_rustc::rustc::rustc_host_target(&rustc)? || roots.target != roots.host {
        return Err("closure currently requires target = host = selected rustc host".into());
    }
    let selected = selected_roots(&roots, &lock)?;
    for root in &roots.roots {
        let binding = selected
            .get(&root.name.replace('-', "_"))
            .ok_or("selected root disappeared")?;
        validate_locked_root_features(
            &arguments.pin,
            &arguments.index_commit,
            binding,
            &root.features,
            root.default_features,
        )?;
    }
    let blobs = arguments
        .blobs
        .or_else(|| std::env::var_os("INCAN_OVEN_BLOBS").map(PathBuf::from))
        .ok_or("set INCAN_OVEN_BLOBS or --blobs to the admitted archive directory")?;
    let parent = arguments
        .out
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let output = parent.join("closure-store");
    let closure = prepare_closure(&ClosureCompileRequest {
        lock: lock_path,
        blobs: &blobs,
        output: &output,
        rustc: &rustc,
        index: &arguments.pin,
        index_commit: &arguments.index_commit,
        target: &roots.target,
        profile: &arguments.profile,
    })?;
    let document = result_document(lock_value, &lock, selected, &closure, &arguments.profile);
    std::fs::write(&arguments.out, serde_json::to_vec_pretty(&document)?)?;
    eprintln!(
        "closure: {} compiled, {} reused, {} missing facts, {} failed",
        closure.report().compiled.len(),
        closure.report().reused.len(),
        closure.report().refused.len(),
        closure.report().failed.len()
    );
    Ok(ExitCode::SUCCESS)
}

/// Project retained root artifacts and every unavailable selected binding into the public closure exchange.
fn result_document(
    lock_value: serde_json::Value,
    lock: &Lock,
    selected: BTreeMap<String, &SdkLockedUnit>,
    closure: &SdkCompiledClosure,
    profile: &str,
) -> ResultDocument {
    let mut externs = BTreeMap::new();
    for (name, binding) in selected {
        if let Some(unit) = closure
            .units()
            .iter()
            .find(|unit| same_binding(unit.binding(), binding))
        {
            externs.insert(
                name,
                Extern {
                    path: unit.output().to_path_buf(),
                    unit: unit.compiled_identity().to_string(),
                },
            );
        }
    }
    let mut refused = Vec::new();
    for binding in &lock.units {
        if closure.units().iter().any(|unit| same_binding(unit.binding(), binding)) {
            continue;
        }
        let label = format!(
            "({}, {}, {}, {:?}, {})",
            binding.loaf, binding.version, profile, binding.features, binding.domain
        );
        let reason = closure
            .report()
            .failed
            .iter()
            .find(|failure| failure.starts_with(&label))
            .cloned()
            .unwrap_or_else(|| "no fact for the exact toolchain, target, profile and feature binding".to_string());
        refused.push(Refused {
            loaf: binding.loaf.clone(),
            version: binding.version.clone(),
            profile: profile.to_string(),
            features: binding.features.clone(),
            reason,
        });
    }
    let units = closure
        .units()
        .iter()
        .map(|unit| CompiledUnitRecord {
            loaf: unit.binding().loaf.clone(),
            version: unit.binding().version.clone(),
            domain: unit.binding().domain.clone(),
            features: unit.binding().features.clone(),
            profile: profile.to_string(),
            unit: unit.compiled_identity().to_string(),
            entry: unit.entry_identity().to_string(),
        })
        .collect();
    ResultDocument {
        schema: "incan.oven.closure/1",
        lock: lock_value,
        externs,
        units,
        refused,
    }
}

/// Match each alias to one compatible target root; unsupported conditional roots fail closed.
fn selected_roots<'a>(
    roots: &Roots,
    lock: &'a Lock,
) -> Result<BTreeMap<String, &'a SdkLockedUnit>, Box<dyn std::error::Error>> {
    let mut selected = BTreeMap::new();
    for root in &roots.roots {
        if root.name.is_empty()
            || !root
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("root extern must be a nonempty identifier".into());
        }
        if root.optional || root.target.is_some() {
            return Err(format!(
                "root {}: conditional or optional roots require the Incan resolver",
                root.name
            )
            .into());
        }
        let requirement = semver::VersionReq::parse(&root.req)?;
        let compatible = |unit: &&SdkLockedUnit, domain: &str| {
            unit.loaf == root.loaf
                && unit.domain == domain
                && semver::Version::parse(&unit.version).is_ok_and(|version| requirement.matches(&version))
                && root.features.iter().all(|feature| unit.features.contains(feature))
        };
        // A root is normally a target unit; a procedural-macro root resolves only in the host domain, so it is
        // looked up there when the lock has no target binding for it.
        let mut candidates: Vec<_> = lock.units.iter().filter(|unit| compatible(unit, "target")).collect();
        if candidates.is_empty() {
            candidates = lock.units.iter().filter(|unit| compatible(unit, "host")).collect();
        }
        let [binding] = candidates.as_slice() else {
            return Err(format!("root {}: expected one compatible locked binding", root.name).into());
        };
        if selected.insert(root.name.replace('-', "_"), *binding).is_some() {
            return Err(format!("duplicate root extern {}", root.name).into());
        }
    }
    Ok(selected)
}

/// Compare a compiled binding's package, archive, domain and feature selection.
fn same_binding(left: &SdkLockedUnit, right: &SdkLockedUnit) -> bool {
    left.loaf == right.loaf
        && left.version == right.version
        && left.domain == right.domain
        && left.archive_digest == right.archive_digest
        && left.features == right.features
}

#[cfg(test)]
mod tests {
    use super::{Lock, Roots, selected_roots};

    /// Root matching must reject requested features absent from an otherwise compatible resolution.
    #[test]
    fn root_features_and_aliases_are_checked() -> Result<(), Box<dyn std::error::Error>> {
        let lock: Lock = serde_json::from_str(
            r#"{"schema":"incan.oven.loaf-resolution/1","units":[{"loaf":"crates-io/cc","version":"1.5.1","archive_digest":"sha256:fixture","domain":"target","features":["parallel"],"target_predicates":[]}]}"#,
        )?;
        let mut roots: Roots = serde_json::from_str(
            r#"{"roots":[{"name":"my-cc","loaf":"crates-io/cc","req":"^1.0.45","features":["parallel"]}],"target":"aarch64-apple-darwin","host":"aarch64-apple-darwin"}"#,
        )?;
        assert!(selected_roots(&roots, &lock)?.contains_key("my_cc"));
        roots.roots[0].features.push("missing".to_string());
        assert!(selected_roots(&roots, &lock).is_err());
        roots.roots[0].features.pop();
        roots.roots[0].name = "".to_string();
        assert!(selected_roots(&roots, &lock).is_err());
        Ok(())
    }

    /// Duplicate normalized aliases cannot silently replace a root artifact.
    #[test]
    fn duplicate_normalized_roots_are_refused() -> Result<(), Box<dyn std::error::Error>> {
        let lock: Lock = serde_json::from_str(
            r#"{"schema":"incan.oven.loaf-resolution/1","units":[{"loaf":"crates-io/cc","version":"1.5.1","archive_digest":"sha256:fixture","domain":"target","features":[],"target_predicates":[]}]}"#,
        )?;
        let roots: Roots = serde_json::from_str(
            r#"{"roots":[{"name":"my-cc","loaf":"crates-io/cc","req":"^1"},{"name":"my_cc","loaf":"crates-io/cc","req":"^1"}],"target":"aarch64-apple-darwin","host":"aarch64-apple-darwin"}"#,
        )?;
        assert!(selected_roots(&roots, &lock).is_err());
        Ok(())
    }
}
