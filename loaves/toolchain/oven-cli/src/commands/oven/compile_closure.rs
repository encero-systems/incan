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
#[derive(Deserialize, Serialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

/// Index declarations enable defaults unless explicitly disabled.
fn enabled() -> bool {
    true
}

/// The public roots exchange binds both compilation domains.
#[derive(Deserialize, Serialize)]
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
    name: String,
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

/// Compile roots from an explicit or Incan-resolved lock, retaining named outcomes and leases through publication.
pub fn run(arguments: CompileClosureArgs) -> CliResult<ExitCode> {
    execute(arguments).map_err(|error| CliError::failure(format!("compile-closure: {error}")))
}

/// Validate request domains and resolution bindings before crossing the shared compiler boundary.
fn execute(arguments: CompileClosureArgs) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let mut roots: Roots = serde_json::from_slice(&std::fs::read(&arguments.roots)?)?;
    let staging = tempfile::tempdir()?;
    let generated_lock;
    let lock_path = match &arguments.lock {
        Some(path) => path,
        None => {
            generated_lock = resolve_lock(&arguments, staging.path())?;
            &generated_lock
        }
    };
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
    let named_refusals = retain_resolved_roots(&mut roots, &lock_value, &arguments.profile);
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
    let compile_lock = staging.path().join("compile-lock.json");
    std::fs::write(
        &compile_lock,
        serde_json::to_vec(&serde_json::json!({"schema": lock.schema, "units": lock.units}))?,
    )?;
    let closure = prepare_closure(&ClosureCompileRequest {
        lock: &compile_lock,
        blobs: &blobs,
        output: &output,
        rustc: &rustc,
        index: &arguments.pin,
        index_commit: &arguments.index_commit,
        target: &roots.target,
        profile: &arguments.profile,
    })?;
    let mut document = result_document(lock_value, &lock, selected, &closure, &arguments.profile);
    for root in &roots.roots {
        let name = root.name.replace('-', "_");
        if !document.externs.contains_key(&name) {
            document.refused.push(Refused {
                name: root.name.clone(),
                loaf: root.loaf.clone(),
                version: root.req.clone(),
                profile: arguments.profile.clone(),
                features: root.features.clone(),
                reason: "root closure unavailable; see refused unit bindings".to_string(),
            });
        }
    }
    document.refused.extend(named_refusals);
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

/// Retain roots accepted by the Incan resolver and project each named resolution refusal onto the result wire.
fn retain_resolved_roots(roots: &mut Roots, lock_value: &serde_json::Value, profile: &str) -> Vec<Refused> {
    let resolution_refusals = lock_value.get("refused").and_then(serde_json::Value::as_array);
    let mut named_refusals = Vec::new();
    if let Some(refusals) = resolution_refusals {
        roots.roots.retain(|root| {
            let Some(refusal) = refusals
                .iter()
                .find(|refusal| refusal.get("name").and_then(serde_json::Value::as_str) == Some(&root.name))
            else {
                return true;
            };
            named_refusals.push(Refused {
                name: root.name.clone(),
                loaf: root.loaf.clone(),
                version: root.req.clone(),
                profile: profile.to_string(),
                features: root.features.clone(),
                reason: refusal
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("resolution refused")
                    .to_string(),
            });
            false
        });
    }
    named_refusals
}

/// Stage only committed index lines and compiler cfg, then run the shared Incan lock driver once for all roots.
fn resolve_lock(
    arguments: &CompileClosureArgs,
    staging: &std::path::Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let commit = git_bytes(
        arguments,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", arguments.index_commit),
        ],
    )?;
    if String::from_utf8(commit)?.trim() != arguments.index_commit {
        return Err("index commit must be the full immutable revision".into());
    }
    let listing = String::from_utf8(git_bytes(
        arguments,
        &["ls-tree", "-r", "--name-only", &arguments.index_commit, "--", "index/"],
    )?)?;
    let mut names = String::new();
    for relative in listing.lines() {
        let path = std::path::Path::new(relative);
        if path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("index snapshot contains an unsafe path".into());
        }
        let destination = staging.join(path);
        std::fs::create_dir_all(destination.parent().ok_or("index path has no parent")?)?;
        std::fs::write(
            destination,
            git_bytes(arguments, &["show", &format!("{}:{relative}", arguments.index_commit)])?,
        )?;
        names.push_str(relative.strip_prefix("index/").ok_or("index path has no prefix")?);
        names.push('\n');
    }
    std::fs::write(staging.join("names.txt"), names)?;
    std::fs::write(staging.join("revision"), &arguments.index_commit)?;
    let rustc = match &arguments.rustc {
        Some(path) => path.clone(),
        None => oven_rustc::rustc::resolve_active_rustc()?,
    };
    let cfg = std::process::Command::new(rustc).args(["--print", "cfg"]).output()?;
    if !cfg.status.success() {
        return Err("selected rustc could not report cfg".into());
    }
    std::fs::write(staging.join("target.cfg"), cfg.stdout)?;
    let source_root = std::env::var_os("INCAN_SOURCE_ROOT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let driver = source_root.join("workspaces/oven/src/loaf_lock_main.incn");
    let compiler = std::env::current_exe()?.with_file_name("incan");
    let roots: Roots = serde_json::from_slice(&std::fs::read(&arguments.roots)?)?;
    let roots_file = staging.join("roots.json");
    std::fs::write(&roots_file, serde_json::to_vec(&roots)?)?;
    let resolved = std::process::Command::new(compiler)
        .arg("run")
        .arg(driver)
        .arg("--")
        .arg(staging)
        .arg(staging.join("names.txt"))
        .arg(staging.join("target.cfg"))
        .arg(roots_file)
        .arg(&arguments.index_commit)
        .output()?;
    if !resolved.status.success() {
        return Err(format!(
            "Incan resolution failed: {}{}",
            String::from_utf8_lossy(&resolved.stdout),
            String::from_utf8_lossy(&resolved.stderr)
        )
        .into());
    }
    let lock = staging.join("lock.json");
    let _: Lock = serde_json::from_slice(&resolved.stdout)?;
    std::fs::write(&lock, resolved.stdout)?;
    Ok(lock)
}

/// Read one immutable Git object without consulting the index checkout's working files.
fn git_bytes(arguments: &CompileClosureArgs, operation: &[&str]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&arguments.pin)
        .args(operation)
        .output()?;
    if !output.status.success() {
        return Err(format!("pinned index read failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(output.stdout)
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
            name: binding.loaf.clone(),
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
    use super::{Lock, Roots, retain_resolved_roots, selected_roots};

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

    /// A named resolution refusal removes only that root, preserving the remaining shared closure requests.
    #[test]
    fn resolution_refusals_preserve_admitted_roots() -> Result<(), Box<dyn std::error::Error>> {
        let mut roots: Roots = serde_json::from_str(
            r#"{"roots":[{"name":"valid","loaf":"crates-io/memchr","req":"=2.8.3"},{"name":"missing","loaf":"crates-io/absent","req":"^1"}],"target":"aarch64-apple-darwin","host":"aarch64-apple-darwin"}"#,
        )?;
        let lock =
            serde_json::json!({"refused":[{"name":"missing","loaf":"crates-io/absent","reason":"no candidate"}]});
        let refusals = retain_resolved_roots(&mut roots, &lock, "debug");
        assert_eq!(roots.roots.len(), 1);
        assert_eq!(roots.roots[0].name, "valid");
        assert_eq!(refusals.len(), 1);
        assert_eq!(refusals[0].name, "missing");
        assert_eq!(refusals[0].reason, "no candidate");
        let normalized = serde_json::to_value(&roots)?;
        assert_eq!(normalized["roots"][0]["default-features"], true);
        assert_eq!(normalized["roots"][0]["optional"], false);
        assert_eq!(normalized["roots"][0]["features"], serde_json::json!([]));
        assert!(normalized["roots"][0].get("target").is_none());
        Ok(())
    }
}
