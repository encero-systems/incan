//! Producer-side vocab companion crate extraction for `incan build --lib`.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::{CliError, CliResult};
use crate::library_manifest::{SoftKeywordActivation, VocabDesugarerArtifact, VocabExports};
use crate::manifest::ProjectManifest;
use sha2::{Digest, Sha256};
use wasmtime::{Config, Engine, ExternType, Module, ValType};

pub(crate) struct LibraryVocabExtraction {
    pub(crate) payload: VocabExports,
    pub(crate) compatibility_activations: Vec<SoftKeywordActivation>,
    pub(crate) pending_desugarer_artifact: Option<PendingDesugarerArtifact>,
}

pub(crate) struct PendingDesugarerArtifact {
    pub(crate) metadata: VocabDesugarerArtifact,
    pub(crate) source_path: PathBuf,
}

pub(crate) fn collect_library_vocab_metadata(
    manifest: &ProjectManifest,
    project_root: &Path,
) -> CliResult<Option<LibraryVocabExtraction>> {
    let Some(vocab) = manifest.vocab() else {
        return Ok(None);
    };

    let declared_crate_path = vocab
        .crate_path
        .clone()
        .ok_or_else(|| CliError::failure("`[vocab]` section requires a `crate` field in incan.toml".to_string()))?;
    let declared_crate_path = declared_crate_path.trim().to_string();
    if declared_crate_path.is_empty() {
        return Err(CliError::failure("`[vocab].crate` cannot be empty".to_string()));
    }

    let companion_crate_root = resolve_companion_crate_root(project_root, &declared_crate_path);
    validate_companion_crate_root(&companion_crate_root)?;
    let cargo_manifest_path = companion_crate_root.join("Cargo.toml");
    let package_name = read_companion_package_name(&cargo_manifest_path)?;

    let metadata = extract_vocab_metadata_from_library_entrypoint(&companion_crate_root, &package_name)?;
    ensure_supported_vocab_metadata_version(&metadata, &companion_crate_root)?;
    if let Some(desugarer) = metadata.desugarer.as_ref() {
        ensure_companion_supports_cdylib(&cargo_manifest_path)?;
        ensure_rust_target_installed(&desugarer.target)?;
        run_cargo_build_for_target(&cargo_manifest_path, &desugarer.target, &desugarer.profile)?;
    }
    let compatibility_activations = project_soft_keyword_activations(&metadata.keyword_registrations);
    let pending_desugarer_artifact =
        build_pending_desugarer_artifact(&companion_crate_root, &package_name, metadata.desugarer.as_ref())?;

    Ok(Some(LibraryVocabExtraction {
        payload: VocabExports {
            crate_path: declared_crate_path,
            package_name,
            keyword_registrations: metadata.keyword_registrations,
            dsl_surfaces: metadata.dsl_surfaces,
            provider_manifest: metadata.library_manifest,
            desugarer_artifact: pending_desugarer_artifact
                .as_ref()
                .map(|artifact| artifact.metadata.clone()),
        },
        compatibility_activations,
        pending_desugarer_artifact,
    }))
}

fn resolve_companion_crate_root(project_root: &Path, declared_crate_path: &str) -> PathBuf {
    let crate_path = PathBuf::from(declared_crate_path);
    if crate_path.is_absolute() {
        crate_path
    } else {
        project_root.join(crate_path)
    }
}

fn validate_companion_crate_root(crate_root: &Path) -> CliResult<()> {
    if !crate_root.exists() {
        return Err(CliError::failure(format!(
            "`[vocab].crate` does not exist: {}",
            crate_root.display()
        )));
    }
    if !crate_root.is_dir() {
        return Err(CliError::failure(format!(
            "`[vocab].crate` must point to a directory: {}",
            crate_root.display()
        )));
    }

    let cargo_toml = crate_root.join("Cargo.toml");
    if !cargo_toml.is_file() {
        return Err(CliError::failure(format!(
            "vocab companion crate is missing Cargo.toml: {}",
            cargo_toml.display()
        )));
    }

    let lib_rs = crate_root.join("src").join("lib.rs");
    if !lib_rs.is_file() {
        return Err(CliError::failure(format!(
            "vocab companion crate is missing src/lib.rs: {}",
            lib_rs.display()
        )));
    }

    Ok(())
}

fn read_companion_package_name(cargo_manifest_path: &Path) -> CliResult<String> {
    let content = std::fs::read_to_string(cargo_manifest_path)
        .map_err(|err| CliError::failure(format!("failed to read {}: {err}", cargo_manifest_path.display())))?;
    let cargo_toml = toml::from_str::<toml::Value>(&content)
        .map_err(|err| CliError::failure(format!("failed to parse {}: {err}", cargo_manifest_path.display())))?;

    let package_name = cargo_toml
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|pkg| pkg.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CliError::failure(format!(
                "vocab companion crate {} is missing [package].name",
                cargo_manifest_path.display()
            ))
        })?;

    Ok(package_name.to_string())
}

fn run_cargo_build_for_target(cargo_manifest_path: &Path, target: &str, profile: &str) -> CliResult<()> {
    let mut command = Command::new("cargo");
    command.arg("build").arg("--manifest-path").arg(cargo_manifest_path);
    if profile == "release" {
        command.arg("--release");
    }
    command.arg("--target").arg(target).arg("--quiet");

    let output = command
        .output()
        .map_err(|err| CliError::failure(format!("failed to run cargo build for vocab desugarer target: {err}")))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(CliError::failure(format!(
        "vocab companion crate failed to build desugarer target `{target}` profile `{profile}` ({}):\n{}",
        cargo_manifest_path.display(),
        stderr.trim()
    )))
}

fn ensure_companion_supports_cdylib(cargo_manifest_path: &Path) -> CliResult<()> {
    let content = fs::read_to_string(cargo_manifest_path)
        .map_err(|err| CliError::failure(format!("failed to read {}: {err}", cargo_manifest_path.display())))?;
    let cargo_toml = toml::from_str::<toml::Value>(&content)
        .map_err(|err| CliError::failure(format!("failed to parse {}: {err}", cargo_manifest_path.display())))?;
    let has_cdylib = cargo_toml
        .get("lib")
        .and_then(toml::Value::as_table)
        .and_then(|lib| lib.get("crate-type"))
        .and_then(toml::Value::as_array)
        .map(|crate_types| {
            crate_types
                .iter()
                .filter_map(toml::Value::as_str)
                .any(|crate_type| crate_type == "cdylib")
        })
        .unwrap_or(false);
    if has_cdylib {
        Ok(())
    } else {
        Err(CliError::failure(format!(
            "vocab companion crate `{}` must declare `[lib].crate-type` including `cdylib` to package a desugarer (example: `crate-type = [\"rlib\", \"cdylib\"]`)",
            cargo_manifest_path.display()
        )))
    }
}

fn ensure_rust_target_installed(target: &str) -> CliResult<()> {
    let output = Command::new("rustup")
        .arg("target")
        .arg("list")
        .arg("--installed")
        .output()
        .map_err(|err| {
            CliError::failure(format!(
                "failed to check installed Rust targets for vocab desugarer build: {err}"
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::failure(format!(
            "failed to list installed Rust targets for vocab desugarer build:\n{}",
            stderr.trim()
        )));
    }
    let installed = parse_installed_rust_targets(&String::from_utf8_lossy(&output.stdout));
    if installed.contains(target) {
        return Ok(());
    }
    Err(CliError::failure(format!(
        "vocab desugarer target `{target}` is not installed in the Rust toolchain. Install it with `rustup target add {target}`."
    )))
}

fn parse_installed_rust_targets(stdout: &str) -> HashSet<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(std::string::ToString::to_string)
        .collect()
}

fn extract_vocab_metadata_from_library_entrypoint(
    companion_crate_root: &Path,
    package_name: &str,
) -> CliResult<incan_vocab::VocabMetadata> {
    let extraction_dir = create_extraction_workspace_dir()?;
    let helper_root = extraction_dir.join("runner");
    fs::create_dir_all(helper_root.join("src")).map_err(|err| {
        CliError::failure(format!(
            "failed to create vocab extraction workspace {}: {err}",
            helper_root.display()
        ))
    })?;
    write_extraction_runner_manifest(&helper_root, companion_crate_root, package_name)?;
    write_extraction_runner_source(&helper_root)?;

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(helper_root.join("Cargo.toml"))
        .output()
        .map_err(|err| CliError::failure(format!("failed to run vocab extraction helper: {err}")))?;

    let metadata_result = if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str::<incan_vocab::VocabMetadata>(stdout.trim()).map_err(|err| {
            CliError::failure(format!(
                "failed to parse metadata extracted from `library_vocab()` in {}: {err}",
                companion_crate_root.display()
            ))
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CliError::failure(format!(
            "failed to extract vocab metadata from companion crate via `library_vocab()` ({}):\n{}",
            companion_crate_root.display(),
            stderr.trim()
        )))
    };

    let _ = fs::remove_dir_all(&extraction_dir);
    metadata_result
}

fn ensure_supported_vocab_metadata_version(
    metadata: &incan_vocab::VocabMetadata,
    companion_crate_root: &Path,
) -> CliResult<()> {
    if metadata.metadata_version == 0 {
        return Err(CliError::failure(format!(
            "companion crate `{}` produced invalid vocab metadata version 0",
            companion_crate_root.display()
        )));
    }
    if metadata.metadata_version > incan_vocab::VOCAB_METADATA_VERSION {
        return Err(CliError::failure(format!(
            "companion crate `{}` produced vocab metadata version {} but this compiler supports up to {}",
            companion_crate_root.display(),
            metadata.metadata_version,
            incan_vocab::VOCAB_METADATA_VERSION
        )));
    }
    Ok(())
}

fn create_extraction_workspace_dir() -> CliResult<PathBuf> {
    static EXTRACTION_COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| CliError::failure(format!("failed to compute extraction workspace timestamp: {err}")))?
            .as_nanos(),
        EXTRACTION_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let dir = env::temp_dir().join(format!("incan_vocab_extract_{nonce}"));
    fs::create_dir_all(&dir).map_err(|err| {
        CliError::failure(format!(
            "failed to create temporary vocab extraction directory {}: {err}",
            dir.display()
        ))
    })?;
    Ok(dir)
}

/// Write the temporary Cargo package that calls the companion crate's `library_vocab()` entrypoint.
fn write_extraction_runner_manifest(
    helper_root: &Path,
    companion_crate_root: &Path,
    package_name: &str,
) -> CliResult<()> {
    let helper_manifest = helper_root.join("Cargo.toml");
    let escaped_companion_path = escape_cargo_toml_string(companion_crate_root);
    let escaped_package_name = package_name.replace('\\', "\\\\").replace('"', "\\\"");
    let manifest = format!(
        "[package]\nname = \"incan_vocab_extraction_runner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncompanion = {{ package = \"{escaped_package_name}\", path = \"{escaped_companion_path}\" }}\nserde_json = \"1.0\"\n"
    );
    fs::write(&helper_manifest, manifest).map_err(|err| {
        CliError::failure(format!(
            "failed to write vocab extraction helper manifest {}: {err}",
            helper_manifest.display()
        ))
    })?;
    copy_workspace_lockfile_to_extraction_runner(helper_root)
}

/// Seed the temporary helper with the repo lockfile so path-only vocab tests do not re-resolve crates.io.
fn copy_workspace_lockfile_to_extraction_runner(helper_root: &Path) -> CliResult<()> {
    let workspace_lockfile = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock");
    if !workspace_lockfile.is_file() {
        return Ok(());
    }

    let helper_lockfile = helper_root.join("Cargo.lock");
    fs::copy(&workspace_lockfile, &helper_lockfile).map_err(|err| {
        CliError::failure(format!(
            "failed to copy workspace lockfile {} to vocab extraction helper {}: {err}",
            workspace_lockfile.display(),
            helper_lockfile.display()
        ))
    })?;
    Ok(())
}

/// Write the Rust entrypoint for the temporary Cargo package that prints serialized vocab metadata.
fn write_extraction_runner_source(helper_root: &Path) -> CliResult<()> {
    let source_path = helper_root.join("src").join("main.rs");
    let source = "fn main() {\n    let registration = companion::library_vocab();\n    let metadata = registration.metadata();\n    let text = match serde_json::to_string_pretty(&metadata) {\n        Ok(text) => text,\n        Err(err) => {\n            eprintln!(\"failed to serialize registration metadata: {err}\");\n            std::process::exit(1);\n        }\n    };\n    print!(\"{text}\");\n}\n";
    fs::write(&source_path, source).map_err(|err| {
        CliError::failure(format!(
            "failed to write vocab extraction helper source {}: {err}",
            source_path.display()
        ))
    })
}

fn escape_cargo_toml_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

fn build_pending_desugarer_artifact(
    companion_crate_root: &Path,
    package_name: &str,
    desugarer: Option<&incan_vocab::DesugarerMetadata>,
) -> CliResult<Option<PendingDesugarerArtifact>> {
    let Some(desugarer) = desugarer else {
        return Ok(None);
    };

    let artifact_kind = desugarer.artifact_kind;
    if !matches!(artifact_kind, incan_vocab::DesugarerArtifactKind::WasmModule) {
        return Err(CliError::failure(
            "unsupported vocab desugarer artifact kind (expected WasmModule)".to_string(),
        ));
    }

    let artifact_file_name = desugarer
        .file_name
        .clone()
        .unwrap_or_else(|| format!("{}.wasm", package_name.replace('-', "_")));
    let source_path = companion_crate_root
        .join("target")
        .join(&desugarer.target)
        .join(&desugarer.profile)
        .join(&artifact_file_name);

    if !source_path.is_file() {
        return Err(CliError::failure(format!(
            "vocab desugarer artifact not found at {} (build companion crate for target `{}` profile `{}` first)",
            source_path.display(),
            desugarer.target,
            desugarer.profile
        )));
    }

    let bytes = fs::read(&source_path).map_err(|err| {
        CliError::failure(format!(
            "failed to read vocab desugarer artifact at {}: {err}",
            source_path.display()
        ))
    })?;
    validate_wasm_desugarer_entrypoint(&source_path, &bytes, &desugarer.entrypoint)?;
    let sha256 = hex::encode(Sha256::digest(&bytes));

    Ok(Some(PendingDesugarerArtifact {
        metadata: VocabDesugarerArtifact {
            artifact_kind,
            abi_version: desugarer.abi_version,
            relative_path: format!("desugarers/{artifact_file_name}"),
            target: desugarer.target.clone(),
            profile: desugarer.profile.clone(),
            entrypoint: desugarer.entrypoint.clone(),
            sha256,
        },
        source_path,
    }))
}

fn validate_wasm_desugarer_entrypoint(path: &Path, bytes: &[u8], entrypoint: &str) -> CliResult<()> {
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config)
        .map_err(|err| CliError::failure(format!("failed to initialize wasm validation engine: {err}")))?;
    let module = Module::new(&engine, bytes).map_err(|err| {
        CliError::failure(format!(
            "failed to compile vocab desugarer artifact `{}` as wasm: {err}",
            path.display()
        ))
    })?;
    validate_wasm_memory_export(&module, path)?;
    validate_wasm_func_export(&module, path, entrypoint, Some(ValType::I32))?;
    validate_wasm_func_export(&module, path, incan_vocab::WASM_DESUGAR_INIT_ENTRYPOINT, None)?;
    for &global_name in incan_vocab::WASM_DESUGAR_REQUIRED_I32_GLOBAL_EXPORTS {
        validate_wasm_i32_global_export(&module, path, global_name)?;
    }
    Ok(())
}

fn validate_wasm_memory_export(module: &Module, path: &Path) -> CliResult<()> {
    let Some(export) = module.get_export(incan_vocab::WASM_DESUGAR_MEMORY_EXPORT) else {
        return Err(CliError::failure(format!(
            "vocab desugarer artifact `{}` is missing exported memory `{}`",
            path.display(),
            incan_vocab::WASM_DESUGAR_MEMORY_EXPORT
        )));
    };
    if matches!(export, ExternType::Memory(_)) {
        Ok(())
    } else {
        Err(CliError::failure(format!(
            "vocab desugarer export `{}` in `{}` is not a memory export",
            incan_vocab::WASM_DESUGAR_MEMORY_EXPORT,
            path.display()
        )))
    }
}

fn validate_wasm_func_export(
    module: &Module,
    path: &Path,
    export_name: &str,
    expected_result: Option<ValType>,
) -> CliResult<()> {
    let Some(export) = module.get_export(export_name) else {
        return Err(CliError::failure(format!(
            "vocab desugarer artifact `{}` is missing exported function `{export_name}`",
            path.display()
        )));
    };
    let ExternType::Func(func_ty) = export else {
        return Err(CliError::failure(format!(
            "vocab desugarer export `{export_name}` in `{}` is not a function",
            path.display()
        )));
    };
    let params_ok = func_ty.params().next().is_none();
    let mut results = func_ty.results();
    let result_ok = match expected_result {
        Some(ValType::I32) => matches!(results.next(), Some(ValType::I32)) && results.next().is_none(),
        None => results.next().is_none(),
        Some(_) => false,
    };
    if params_ok && result_ok {
        Ok(())
    } else {
        Err(CliError::failure(format!(
            "vocab desugarer export `{export_name}` in `{}` has an invalid function signature",
            path.display()
        )))
    }
}

fn validate_wasm_i32_global_export(module: &Module, path: &Path, export_name: &str) -> CliResult<()> {
    let Some(export) = module.get_export(export_name) else {
        return Err(CliError::failure(format!(
            "vocab desugarer artifact `{}` is missing exported global `{export_name}`",
            path.display()
        )));
    };
    let ExternType::Global(global_ty) = export else {
        return Err(CliError::failure(format!(
            "vocab desugarer export `{export_name}` in `{}` is not a global",
            path.display()
        )));
    };
    if matches!(global_ty.content(), ValType::I32) {
        Ok(())
    } else {
        Err(CliError::failure(format!(
            "vocab desugarer global `{export_name}` in `{}` must have type `i32`",
            path.display()
        )))
    }
}

fn project_soft_keyword_activations(registrations: &[incan_vocab::KeywordRegistration]) -> Vec<SoftKeywordActivation> {
    let mut dedup = HashSet::new();
    let mut projected = Vec::new();

    for registration in registrations {
        let incan_vocab::KeywordActivation::OnImport { namespace } = &registration.activation else {
            continue;
        };
        for keyword in &registration.keywords {
            let Some(id) = incan_core::lang::keywords::from_str(&keyword.name) else {
                continue;
            };
            if !incan_core::lang::keywords::is_soft(id) {
                continue;
            }

            let key = (namespace.clone(), keyword.name.clone());
            if dedup.insert(key.clone()) {
                projected.push(SoftKeywordActivation {
                    namespace: key.0,
                    keyword: key.1,
                });
            }
        }
    }

    projected.sort_by(|left, right| {
        left.namespace
            .cmp(&right.namespace)
            .then(left.keyword.cmp(&right.keyword))
    });
    projected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ProjectManifest;
    use std::fs;

    fn write_vocab_companion_crate(
        project_root: &Path,
        crate_dir: &str,
        package_name: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let crate_root = project_root.join(crate_dir);
        fs::create_dir_all(crate_root.join("src"))?;
        fs::write(
            crate_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("crates")
                    .join("incan_vocab")
                    .display()
            ),
        )?;
        fs::write(
            crate_root.join("src/lib.rs"),
            "pub fn library_vocab() -> incan_vocab::VocabRegistration {\n    incan_vocab::VocabRegistration::new().with_keyword_registration(\n        incan_vocab::KeywordRegistration {\n            activation: incan_vocab::KeywordActivation::OnImport {\n                namespace: \"widgets.dsl\".to_string(),\n            },\n            keywords: vec![incan_vocab::KeywordSpec::new(\n                \"await\",\n                incan_vocab::KeywordSurfaceKind::ControlFlow,\n            )],\n            valid_decorators: Vec::new(),\n        }\n    )\n}\n",
        )?;
        Ok(crate_root)
    }

    #[test]
    fn projects_import_activated_soft_keywords() {
        let registrations = vec![
            incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "mylib.dsl".to_string(),
                },
                keywords: vec![
                    incan_vocab::KeywordSpec::new("await", incan_vocab::KeywordSurfaceKind::ControlFlow),
                    incan_vocab::KeywordSpec::new("def", incan_vocab::KeywordSurfaceKind::FunctionDecl),
                ],
                valid_decorators: Vec::new(),
            },
            incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::Always,
                keywords: vec![incan_vocab::KeywordSpec::new(
                    "await",
                    incan_vocab::KeywordSurfaceKind::ControlFlow,
                )],
                valid_decorators: Vec::new(),
            },
        ];

        let projected = project_soft_keyword_activations(&registrations);
        assert_eq!(
            projected,
            vec![SoftKeywordActivation {
                namespace: "mylib.dsl".to_string(),
                keyword: "await".to_string(),
            }]
        );
    }

    #[test]
    fn resolve_companion_crate_root_uses_project_root_for_relative_paths() {
        let project_root = PathBuf::from("/tmp/incan_project");
        let resolved = resolve_companion_crate_root(&project_root, "crates/mylib_vocab");
        assert_eq!(resolved, project_root.join("crates/mylib_vocab"));
    }

    #[test]
    fn validate_companion_crate_root_rejects_missing_src_lib() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let crate_root = temp.path().join("vocab_companion");
        fs::create_dir_all(&crate_root)?;
        fs::write(
            crate_root.join("Cargo.toml"),
            "[package]\nname = \"vocab_companion\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;

        let err = validate_companion_crate_root(&crate_root)
            .err()
            .ok_or("expected validation failure")?;
        assert!(err.to_string().contains("missing src/lib.rs"));
        Ok(())
    }

    #[test]
    fn read_companion_package_name_reads_package_name() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let cargo_toml = temp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"widgets_vocab_companion\"\nversion = \"0.1.0\"\n",
        )?;

        let package_name = read_companion_package_name(&cargo_toml)?;
        assert_eq!(package_name, "widgets_vocab_companion");
        Ok(())
    }

    #[test]
    fn ensure_companion_supports_cdylib_accepts_cdylib_crate_type() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let cargo_toml = temp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"widgets_vocab_companion\"\nversion = \"0.1.0\"\n\n[lib]\ncrate-type = [\"rlib\", \"cdylib\"]\n",
        )?;
        ensure_companion_supports_cdylib(&cargo_toml)?;
        Ok(())
    }

    #[test]
    fn ensure_companion_supports_cdylib_rejects_missing_cdylib_crate_type() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let cargo_toml = temp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"widgets_vocab_companion\"\nversion = \"0.1.0\"\n\n[lib]\npath = \"src/lib.rs\"\n",
        )?;
        let err = match ensure_companion_supports_cdylib(&cargo_toml) {
            Ok(()) => return Err("expected missing cdylib to fail".into()),
            Err(err) => err,
        };
        assert!(err.to_string().contains("cdylib"));
        Ok(())
    }

    #[test]
    fn extract_vocab_metadata_from_library_entrypoint_parses_valid_payload() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let crate_root = write_vocab_companion_crate(temp.path(), "vocab_companion", "widgets_vocab_companion")?;
        let parsed = extract_vocab_metadata_from_library_entrypoint(&crate_root, "widgets_vocab_companion")?;
        assert_eq!(parsed.keyword_registrations.len(), 1);
        assert_eq!(
            parsed.keyword_registrations[0].activation,
            incan_vocab::KeywordActivation::OnImport {
                namespace: "widgets.dsl".to_string()
            }
        );
        Ok(())
    }

    #[test]
    fn extraction_runner_manifest_reuses_workspace_lockfile() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let helper_root = temp.path().join("runner");
        let companion_root = temp.path().join("vocab_companion");
        fs::create_dir_all(helper_root.join("src"))?;
        fs::create_dir_all(&companion_root)?;

        write_extraction_runner_manifest(&helper_root, &companion_root, "widgets_vocab_companion")?;

        let manifest = fs::read_to_string(helper_root.join("Cargo.toml"))?;
        assert!(manifest.contains("serde_json = \"1.0\""));
        assert!(
            helper_root.join("Cargo.lock").is_file(),
            "helper runner should inherit the workspace lockfile"
        );
        Ok(())
    }

    #[test]
    fn collect_library_vocab_metadata_requires_library_vocab_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project_root = temp.path().join("project");
        fs::create_dir_all(&project_root)?;
        let crate_root = project_root.join("vocab_companion");
        fs::create_dir_all(crate_root.join("src"))?;
        fs::write(
            crate_root.join("Cargo.toml"),
            "[package]\nname = \"widgets_vocab_companion\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
        )?;
        fs::write(crate_root.join("src/lib.rs"), "pub fn register_vocab() {}\n")?;

        let manifest_path = project_root.join("incan.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"widgets\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        let manifest = ProjectManifest::from_str(&fs::read_to_string(&manifest_path)?, &manifest_path)?;

        let err = collect_library_vocab_metadata(&manifest, &project_root)
            .err()
            .ok_or("expected vocab metadata extraction to fail without library_vocab entrypoint")?;
        let message = err.to_string();
        assert!(message.contains("library_vocab"), "unexpected error: {message}");
        Ok(())
    }

    #[test]
    fn collect_library_vocab_metadata_extracts_payload_and_projection() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project_root = temp.path().join("project");
        fs::create_dir_all(&project_root)?;
        write_vocab_companion_crate(&project_root, "vocab_companion", "widgets_vocab_companion")?;

        let manifest_path = project_root.join("incan.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"widgets\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        let manifest = ProjectManifest::from_str(&fs::read_to_string(&manifest_path)?, &manifest_path)?;

        let extraction = collect_library_vocab_metadata(&manifest, &project_root)?
            .ok_or("expected vocab metadata extraction to return payload")?;
        assert_eq!(extraction.payload.crate_path, "vocab_companion");
        assert_eq!(extraction.payload.package_name, "widgets_vocab_companion");
        assert_eq!(extraction.payload.keyword_registrations.len(), 1);
        assert_eq!(
            extraction.compatibility_activations,
            vec![SoftKeywordActivation {
                namespace: "widgets.dsl".to_string(),
                keyword: "await".to_string(),
            }]
        );
        Ok(())
    }

    #[test]
    fn parse_installed_rust_targets_ignores_empty_lines() {
        let parsed = parse_installed_rust_targets("wasm32-wasip1\n\nx86_64-apple-darwin\n");
        assert!(parsed.contains("wasm32-wasip1"));
        assert!(parsed.contains("x86_64-apple-darwin"));
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn ensure_supported_vocab_metadata_version_rejects_newer_version() {
        let metadata = incan_vocab::VocabMetadata {
            metadata_version: incan_vocab::VOCAB_METADATA_VERSION + 1,
            ..incan_vocab::VocabMetadata::default()
        };
        let err = match ensure_supported_vocab_metadata_version(&metadata, Path::new("/tmp/companion")) {
            Ok(()) => panic!("expected metadata version mismatch"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("metadata version"));
    }
}
