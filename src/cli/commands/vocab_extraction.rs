//! Producer-side vocab companion crate extraction for `incan build --lib`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::{CliError, CliResult};
use crate::library_manifest::{SoftKeywordActivation, VocabDesugarerArtifact, VocabExports};
use crate::manifest::ProjectManifest;
use sha2::{Digest, Sha256};

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

    // Ensure the declared companion crate at least compiles before metadata extraction.
    run_cargo_build(&cargo_manifest_path)?;

    let metadata_path = companion_crate_root.join("vocab_metadata.json");
    let metadata = read_vocab_metadata(&metadata_path)?;
    if let Some(desugarer) = metadata.desugarer.as_ref() {
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

fn run_cargo_build(cargo_manifest_path: &Path) -> CliResult<()> {
    let output = Command::new("cargo")
        .arg("build")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(cargo_manifest_path)
        .output()
        .map_err(|err| CliError::failure(format!("failed to run cargo build: {err}")))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(CliError::failure(format!(
        "vocab companion crate failed to build ({}):\n{}",
        cargo_manifest_path.display(),
        stderr.trim()
    )))
}

fn run_cargo_build_for_target(cargo_manifest_path: &Path, target: &str, profile: &str) -> CliResult<()> {
    let mut command = Command::new("cargo");
    command.arg("build").arg("--manifest-path").arg(cargo_manifest_path);
    if profile == "release" {
        command.arg("--release");
    }
    command.arg("--target").arg(target);
    command.arg("--quiet");

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

fn read_vocab_metadata(metadata_path: &Path) -> CliResult<incan_vocab::VocabMetadata> {
    let content = std::fs::read_to_string(metadata_path).map_err(|err| {
        CliError::failure(format!(
            "failed to read vocab metadata at {}: {err}",
            metadata_path.display()
        ))
    })?;

    serde_json::from_str::<incan_vocab::VocabMetadata>(&content).map_err(|err| {
        CliError::failure(format!(
            "failed to parse vocab metadata at {}: {err}",
            metadata_path.display()
        ))
    })
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
    let sha256 = hex::encode(Sha256::digest(&bytes));

    Ok(Some(PendingDesugarerArtifact {
        metadata: VocabDesugarerArtifact {
            artifact_kind,
            relative_path: format!("desugarers/{artifact_file_name}"),
            target: desugarer.target.clone(),
            profile: desugarer.profile.clone(),
            entrypoint: desugarer.entrypoint.clone(),
            sha256,
        },
        source_path,
    }))
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
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n"
            ),
        )?;
        fs::write(crate_root.join("src/lib.rs"), "pub fn register_vocab() {}\n")?;
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
    fn read_vocab_metadata_parses_valid_payload() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let metadata_path = temp.path().join("vocab_metadata.json");
        let metadata = incan_vocab::VocabMetadata {
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "widgets.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec::new(
                    "await",
                    incan_vocab::KeywordSurfaceKind::ControlFlow,
                )],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: Vec::new(),
            library_manifest: incan_vocab::LibraryManifest::default(),
            desugarer: None,
        };
        fs::write(&metadata_path, serde_json::to_string_pretty(&metadata)?)?;

        let parsed = read_vocab_metadata(&metadata_path)?;
        assert_eq!(parsed, metadata);
        Ok(())
    }

    #[test]
    fn characterization_collect_library_vocab_metadata_requires_vocab_metadata_sidecar()
    -> Result<(), Box<dyn std::error::Error>> {
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

        let err = collect_library_vocab_metadata(&manifest, &project_root)
            .err()
            .ok_or("expected vocab metadata extraction to fail without vocab_metadata.json")?;
        let message = err.to_string();
        assert!(
            message.contains("failed to read vocab metadata"),
            "unexpected error: {message}"
        );
        assert!(message.contains("vocab_metadata.json"), "unexpected error: {message}");
        Ok(())
    }

    #[test]
    fn collect_library_vocab_metadata_extracts_payload_and_projection() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project_root = temp.path().join("project");
        fs::create_dir_all(&project_root)?;
        write_vocab_companion_crate(&project_root, "vocab_companion", "widgets_vocab_companion")?;

        let metadata = incan_vocab::VocabMetadata {
            keyword_registrations: vec![
                incan_vocab::KeywordRegistration {
                    activation: incan_vocab::KeywordActivation::OnImport {
                        namespace: "widgets.dsl".to_string(),
                    },
                    keywords: vec![incan_vocab::KeywordSpec::new(
                        "await",
                        incan_vocab::KeywordSurfaceKind::ControlFlow,
                    )],
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
            ],
            dsl_surfaces: Vec::new(),
            library_manifest: incan_vocab::LibraryManifest::default(),
            desugarer: None,
        };
        fs::write(
            project_root.join("vocab_companion").join("vocab_metadata.json"),
            serde_json::to_string_pretty(&metadata)?,
        )?;

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
        assert_eq!(extraction.payload.keyword_registrations.len(), 2);
        assert_eq!(
            extraction.compatibility_activations,
            vec![SoftKeywordActivation {
                namespace: "widgets.dsl".to_string(),
                keyword: "await".to_string(),
            }]
        );
        Ok(())
    }
}
