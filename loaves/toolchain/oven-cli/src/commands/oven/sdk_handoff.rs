//! Bound transport of one compiler-selected SDK provider between CI jobs.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use oven_store::process::{BoundedProcessLimits, BoundedProcessTermination, run_bounded_process};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cli::SdkHandoffCommand;

use super::{CliError, CliResult, ExitCode};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(270);
const COMMAND_OUTPUT_BYTES: usize = 1024 * 1024;
const CANARY: &str = "loaves/compiler/incan_test_support/fixtures/test_assert_canary.incn";

#[derive(Clone, Debug)]
struct HandoffCoordinates {
    provider_identity: String,
    compiler_sha256: String,
    rustc_identity: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct HandoffEnvelope {
    schema_version: u8,
    provider_identity: String,
    compiler_sha256: String,
    rustc_identity: String,
    payload_sha256: String,
    empty_directories: Vec<String>,
}

/// Stage or consume the exact SDK provider selected by the current compiler and Rust toolchain.
pub fn oven_sdk_handoff(command: SdkHandoffCommand) -> CliResult<ExitCode> {
    run_sdk_handoff(command)
        .map(|()| ExitCode::SUCCESS)
        .map_err(|error| CliError::failure(format!("SDK handoff failed: {error}")))
}

/// Execute one parsed handoff command within the I/O error boundary.
fn run_sdk_handoff(command: SdkHandoffCommand) -> io::Result<()> {
    match command {
        SdkHandoffCommand::Stage {
            workspace,
            compiler,
            rustc,
            artifact,
            store,
        } => {
            let coordinates = coordinates(&workspace, &compiler, &rustc)?;
            stage(&store, &artifact, &coordinates)
        }
        SdkHandoffCommand::Consume {
            workspace,
            compiler,
            rustc,
            artifact,
            path_file,
            env_file,
        } => {
            let coordinates = coordinates(&workspace, &compiler, &rustc)?;
            consume(&workspace, &compiler, &artifact, &path_file, &env_file, &coordinates)
        }
    }
}

/// Derive expected handoff coordinates independently of the downloaded envelope.
fn coordinates(workspace: &Path, compiler: &Path, rustc: &Path) -> io::Result<HandoffCoordinates> {
    let identity = run_command(
        Command::new(compiler)
            .args(["oven", "sdk-provider-store-identity", "--compiler-root"])
            .arg(workspace)
            .current_dir(workspace)
            .env("INCAN_NO_BANNER", "1")
            .env("INCAN_SOURCE_ROOT", workspace),
    )?;
    let provider_identity = String::from_utf8_lossy(&identity).trim().to_owned();
    if provider_identity.len() != 64
        || !provider_identity
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(io::Error::other("compiler returned an invalid SDK provider identity"));
    }
    let rustc_output = run_command(
        Command::new(rustc)
            .args(["--version", "--verbose"])
            .current_dir(workspace)
            .env("INCAN_NO_BANNER", "1")
            .env("INCAN_SOURCE_ROOT", workspace),
    )?;
    Ok(HandoffCoordinates {
        provider_identity,
        compiler_sha256: file_digest(compiler)?,
        rustc_identity: hex_digest(&rustc_output),
    })
}

/// Run one bounded child and retain its diagnostics on refusal.
fn run_command(command: &mut Command) -> io::Result<Vec<u8>> {
    let output = run_bounded_process(
        command,
        BoundedProcessLimits {
            stdout_bytes: COMMAND_OUTPUT_BYTES,
            stderr_bytes: COMMAND_OUTPUT_BYTES,
            timeout: Some(COMMAND_TIMEOUT),
        },
        None,
    )?;
    if output.termination != BoundedProcessTermination::Completed || !output.status.success() {
        let details = [output.stderr, output.stdout].concat();
        return Err(io::Error::other(format!(
            "command failed ({:?}): {}",
            output.termination,
            String::from_utf8_lossy(&details).trim()
        )));
    }
    Ok(output.stdout)
}

/// Stage only the selected provider, its transport digest, and its empty-directory manifest.
fn stage(store: &Path, artifact: &Path, coordinates: &HandoffCoordinates) -> io::Result<()> {
    let selected = store.join(&coordinates.provider_identity);
    require_inventory(&selected)?;
    require_real_directory(&selected)?;
    let payload_sha256 = payload_digest(&selected)?;
    let empty_directories = empty_directories(&selected)?;
    if artifact.exists() {
        return Err(io::Error::other(format!(
            "SDK handoff output already exists: {}",
            artifact.display()
        )));
    }
    let parent = artifact.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = tempfile::Builder::new().prefix("sdk-handoff-").tempdir_in(parent)?;
    let staged = temporary.path().join("artifact");
    fs::create_dir(&staged)?;
    let copied = staged.join(&coordinates.provider_identity);
    copy_regular_tree(&selected, &copied)?;
    if payload_digest(&copied)? != payload_sha256 {
        return Err(io::Error::other("SDK payload changed during handoff publication"));
    }
    let envelope = HandoffEnvelope {
        schema_version: 1,
        provider_identity: coordinates.provider_identity.clone(),
        compiler_sha256: coordinates.compiler_sha256.clone(),
        rustc_identity: coordinates.rustc_identity.clone(),
        payload_sha256: payload_sha256.clone(),
        empty_directories,
    };
    fs::write(
        staged.join("handoff.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&envelope).map_err(io::Error::other)?
        ),
    )?;
    fs::rename(&staged, artifact)?;
    println!(
        "Staged SDK handoff {} ({payload_sha256})",
        coordinates.provider_identity
    );
    Ok(())
}

/// Validate the envelope and payload before publishing any consumer environment paths.
fn consume(
    workspace: &Path,
    compiler: &Path,
    artifact: &Path,
    path_file: &Path,
    env_file: &Path,
    coordinates: &HandoffCoordinates,
) -> io::Result<()> {
    let envelope: HandoffEnvelope = serde_json::from_slice(&fs::read(artifact.join("handoff.json"))?)
        .map_err(|error| io::Error::other(format!("invalid handoff envelope: {error}")))?;
    if envelope.schema_version != 1 {
        return Err(io::Error::other("unsupported handoff schema"));
    }
    compare_coordinate(
        &envelope.provider_identity,
        &coordinates.provider_identity,
        "provider identity",
    )?;
    compare_coordinate(
        &envelope.compiler_sha256,
        &coordinates.compiler_sha256,
        "compiler digest",
    )?;
    compare_coordinate(&envelope.rustc_identity, &coordinates.rustc_identity, "rustc identity")?;
    let selected = artifact.join(&coordinates.provider_identity);
    require_real_directory(&selected)?;
    let inventory = require_inventory(&selected)?;
    for directory in &envelope.empty_directories {
        let relative = safe_relative_directory(directory)?;
        fs::create_dir_all(selected.join(relative))?;
    }
    if payload_digest(&selected)? != envelope.payload_sha256 {
        return Err(io::Error::other("SDK handoff payload digest mismatch"));
    }
    let output = run_command(
        Command::new(compiler)
            .args(["check", CANARY])
            .current_dir(workspace)
            .env("INCAN_NO_BANNER", "1")
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_SOURCE_ROOT", workspace)
            .env("INCAN_SDK_INVENTORY", &inventory),
    )
    .map_err(|error| io::Error::other(format!("SDK validation failed: {error}")))?;
    if payload_digest(&selected)? != envelope.payload_sha256 {
        return Err(io::Error::other("SDK validation modified the transferred payload"));
    }
    let variables = BTreeMap::from([
        ("INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE", path_file.display().to_string()),
        ("INCAN_INTERNAL_SDK_PROVIDER_STORE", artifact.display().to_string()),
        ("INCAN_SDK_INVENTORY", inventory.display().to_string()),
        ("INCAN_TEST_SDK_PROVIDER_PATH_FILE", path_file.display().to_string()),
        ("INCAN_TEST_SDK_PROVIDER_STORE", artifact.display().to_string()),
    ]);
    if variables.values().any(|value| value.contains(['\n', '\r'])) {
        return Err(io::Error::other("SDK handoff paths cannot contain line breaks"));
    }
    if let Some(parent) = path_file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path_file, format!("{}\n", selected.display()))?;
    if let Some(parent) = env_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut environment = OpenOptions::new().create(true).append(true).open(env_file)?;
    for (key, value) in variables {
        writeln!(environment, "{key}={value}")?;
    }
    io::stdout().write_all(&output)?;
    println!(
        "Validated SDK handoff {}; source publication was not requested",
        coordinates.provider_identity
    );
    Ok(())
}

/// Reject a coordinate mismatch using the stable diagnostic label.
fn compare_coordinate(actual: &str, expected: &str, label: &str) -> io::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::other(format!("SDK handoff {label} mismatch")))
    }
}

/// Require the selected provider's exact regular inventory file.
fn require_inventory(root: &Path) -> io::Result<PathBuf> {
    let path = root.join("sdk-inventory.json");
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        io::Error::other(format!(
            "SDK inventory is missing or not a regular file: {}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::other(format!(
            "SDK inventory is missing or not a regular file: {}",
            path.display()
        )));
    }
    Ok(path)
}

/// Require a directory that is not reached through a final-component symlink.
fn require_real_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::other(format!(
            "SDK handoff cannot contain a symlink: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Bind every relative path, entry kind, and regular-file digest below a provider root.
fn payload_digest(root: &Path) -> io::Result<String> {
    let mut paths = Vec::new();
    collect_paths(root, root, &mut paths)?;
    paths.sort();
    let mut digest = Sha256::new();
    for path in paths {
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        let name = relative.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
        digest.update(u64::try_from(name.len()).map_err(io::Error::other)?.to_be_bytes());
        digest.update(name.as_bytes());
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other(format!(
                "SDK handoff cannot contain a symlink: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            digest.update(b"directory\0");
        } else if metadata.is_file() {
            digest.update(b"file\0");
            digest.update(hex_to_bytes(&file_digest(&path)?)?);
        } else {
            return Err(io::Error::other(format!(
                "unsupported SDK handoff entry: {}",
                path.display()
            )));
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Collect every entry below a directory without following symlinks.
fn collect_paths(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> io::Result<()> {
    let _ = root;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        paths.push(path.clone());
        if entry.file_type()?.is_dir() {
            collect_paths(root, &path, paths)?;
        }
    }
    Ok(())
}

/// Hash one file without loading it into memory.
fn file_digest(path: &Path) -> io::Result<String> {
    let mut source = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Hash an in-memory command identity.
fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Decode one lowercase hexadecimal digest for the payload tree fold.
fn hex_to_bytes(value: &str) -> io::Result<Vec<u8>> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).map_err(io::Error::other)?;
            u8::from_str_radix(pair, 16).map_err(io::Error::other)
        })
        .collect()
}

/// Copy a provider tree while refusing links and special entries.
fn copy_regular_tree(source: &Path, destination: &Path) -> io::Result<()> {
    require_real_directory(source)?;
    fs::create_dir(destination)?;
    let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other(format!(
                "SDK handoff cannot contain a symlink: {}",
                source_path.display()
            )));
        }
        if metadata.is_dir() {
            copy_regular_tree(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path)?;
        } else {
            return Err(io::Error::other(format!(
                "unsupported SDK handoff entry: {}",
                source_path.display()
            )));
        }
    }
    Ok(())
}

/// Inventory empty directories using portable forward-slash paths.
fn empty_directories(root: &Path) -> io::Result<Vec<String>> {
    let mut paths = Vec::new();
    collect_paths(root, root, &mut paths)?;
    paths.sort();
    let mut empty = Vec::new();
    for path in paths.into_iter().filter(|path| path.is_dir()) {
        if fs::read_dir(&path)?.next().is_none() {
            empty.push(
                path.strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
            );
        }
    }
    Ok(empty)
}

/// Accept only a normalized relative directory manifest entry.
fn safe_relative_directory(value: &str) -> io::Result<&Path> {
    let path = Path::new(value);
    let safe = !value.is_empty()
        && value != "."
        && !path.is_absolute()
        && !value.contains('\\')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && path.to_string_lossy() == value;
    if safe {
        Ok(path)
    } else {
        Err(io::Error::other(
            "SDK directory manifest entry must remain inside the selected provider",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// Create one prepared provider and fixed coordinate set.
    fn fixture() -> io::Result<(tempfile::TempDir, PathBuf, PathBuf, HandoffCoordinates)> {
        let root = tempfile::tempdir()?;
        let store = root.path().join("producer store");
        let provider = store.join(IDENTITY);
        fs::create_dir_all(provider.join("components"))?;
        fs::write(provider.join("sdk-inventory.json"), b"{\"compatible\": true}")?;
        fs::write(
            provider.join("components/provider.incnlib"),
            b"immutable checked provider",
        )?;
        let artifact = root.path().join("handoff artifact");
        let coordinates = HandoffCoordinates {
            provider_identity: IDENTITY.to_owned(),
            compiler_sha256: "b".repeat(64),
            rustc_identity: "c".repeat(64),
        };
        Ok((root, store, artifact, coordinates))
    }

    /// Write executable compiler and rustc probes for end-to-end coordinate and canary checks.
    #[cfg(unix)]
    fn command_probes(workspace: &Path) -> io::Result<(PathBuf, PathBuf)> {
        use std::os::unix::fs::PermissionsExt;

        let compiler = workspace.join("incan");
        fs::write(
            &compiler,
            format!(
                "#!/bin/sh\nif [ \"$1\" = oven ]; then echo {IDENTITY}; exit 0; fi\nif [ \"$1\" = check ] && [ -f \"$INCAN_SDK_INVENTORY\" ]; then echo canary-ok; exit 0; fi\necho unexpected compiler command >&2\nexit 9\n"
            ),
        )?;
        fs::set_permissions(&compiler, fs::Permissions::from_mode(0o755))?;
        let rustc = workspace.join("rustc");
        fs::write(&rustc, "#!/bin/sh\necho rustc 1.98.0\necho host: test-target\n")?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;
        Ok((compiler, rustc))
    }

    #[test]
    fn stage_contains_only_selected_provider_and_envelope() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, store, artifact, coordinates) = fixture()?;
        fs::create_dir_all(store.join("d".repeat(64)))?;
        stage(&store, &artifact, &coordinates)?;
        let mut names = fs::read_dir(&artifact)?
            .map(|entry| entry.map(|item| item.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        assert_eq!(
            names,
            vec![
                std::ffi::OsString::from(IDENTITY),
                std::ffi::OsString::from("handoff.json")
            ]
        );
        Ok(())
    }

    #[test]
    fn stage_requires_inventory_and_refuses_existing_output() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, store, artifact, coordinates) = fixture()?;
        fs::remove_file(store.join(IDENTITY).join("sdk-inventory.json"))?;
        assert!(stage(&store, &artifact, &coordinates).is_err());
        fs::write(store.join(IDENTITY).join("sdk-inventory.json"), b"{}")?;
        fs::create_dir(&artifact)?;
        assert!(stage(&store, &artifact, &coordinates).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlink_payload_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let (_root, store, artifact, coordinates) = fixture()?;
        symlink(
            store.join(IDENTITY).join("sdk-inventory.json"),
            store.join(IDENTITY).join("outside"),
        )?;
        assert!(stage(&store, &artifact, &coordinates).is_err());
        assert!(!artifact.exists());
        Ok(())
    }

    #[test]
    fn payload_digest_tracks_paths_kinds_and_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, store, _artifact, _coordinates) = fixture()?;
        let provider = store.join(IDENTITY);
        let initial = payload_digest(&provider)?;
        fs::write(provider.join("components/provider.incnlib"), b"changed")?;
        assert_ne!(payload_digest(&provider)?, initial);
        fs::write(
            provider.join("components/provider.incnlib"),
            b"immutable checked provider",
        )?;
        fs::create_dir(provider.join("empty"))?;
        assert_ne!(payload_digest(&provider)?, initial);
        Ok(())
    }

    #[test]
    fn empty_directory_manifest_restores_file_only_transport() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, store, artifact, coordinates) = fixture()?;
        fs::create_dir_all(store.join(IDENTITY).join("empty/nested"))?;
        stage(&store, &artifact, &coordinates)?;
        let nested = artifact.join(IDENTITY).join("empty/nested");
        fs::remove_dir_all(artifact.join(IDENTITY).join("empty"))?;
        let envelope: HandoffEnvelope = serde_json::from_slice(&fs::read(artifact.join("handoff.json"))?)?;
        for value in envelope.empty_directories {
            fs::create_dir_all(artifact.join(IDENTITY).join(safe_relative_directory(&value)?))?;
        }
        assert!(nested.is_dir());
        assert_eq!(payload_digest(&artifact.join(IDENTITY))?, envelope.payload_sha256);
        Ok(())
    }

    #[test]
    fn empty_directory_manifest_cannot_escape_provider() -> Result<(), Box<dyn std::error::Error>> {
        for value in ["../../escaped", "/absolute", ".", "", "a/../b", "a//b", "a\\b"] {
            assert!(safe_relative_directory(value).is_err(), "accepted {value:?}");
        }
        assert_eq!(safe_relative_directory("empty/nested")?, Path::new("empty/nested"));
        Ok(())
    }

    #[test]
    fn coordinate_mismatches_use_stable_diagnostics() {
        for label in ["provider identity", "compiler digest", "rustc identity"] {
            let error = compare_coordinate("tampered", "expected", label)
                .err()
                .map(|error| error.to_string());
            assert_eq!(error.as_deref(), Some(format!("SDK handoff {label} mismatch").as_str()));
        }
    }

    #[test]
    fn malformed_provider_identity_is_rejected_before_transport() {
        for identity in [
            "short",
            "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "gaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            let valid = identity.len() == 64
                && identity
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
            assert!(!valid);
        }
    }

    #[cfg(unix)]
    #[test]
    fn relocated_handoff_validates_before_publishing_consumer_paths() -> Result<(), Box<dyn std::error::Error>> {
        let (root, store, artifact, _) = fixture()?;
        let workspace = root.path().join("source checkout");
        fs::create_dir(&workspace)?;
        let (compiler, rustc) = command_probes(&workspace)?;
        let coordinates = coordinates(&workspace, &compiler, &rustc)?;
        stage(&store, &artifact, &coordinates)?;
        let relocated = root.path().join("consumer checkout/sdk");
        fs::create_dir_all(relocated.parent().ok_or("relocated handoff needs parent")?)?;
        fs::rename(&artifact, &relocated)?;
        fs::remove_dir_all(&store)?;
        let path_file = root.path().join("provider path");
        let env_file = root.path().join("github env");
        consume(&workspace, &compiler, &relocated, &path_file, &env_file, &coordinates)?;
        let selected = relocated.join(IDENTITY);
        assert_eq!(fs::read_to_string(&path_file)?, format!("{}\n", selected.display()));
        let environment = fs::read_to_string(&env_file)?;
        assert!(environment.contains(&format!(
            "INCAN_SDK_INVENTORY={}\n",
            selected.join("sdk-inventory.json").display()
        )));
        assert!(!store.exists());
        Ok(())
    }

    #[test]
    fn modified_payload_and_unknown_schema_are_refused_before_path_publication()
    -> Result<(), Box<dyn std::error::Error>> {
        let (root, store, artifact, coordinates) = fixture()?;
        stage(&store, &artifact, &coordinates)?;
        let path_file = root.path().join("provider path");
        let env_file = root.path().join("github env");
        fs::write(artifact.join(IDENTITY).join("components/provider.incnlib"), b"tampered")?;
        assert!(
            consume(
                root.path(),
                Path::new("unused"),
                &artifact,
                &path_file,
                &env_file,
                &coordinates
            )
            .is_err()
        );
        assert!(!path_file.exists());
        assert!(!env_file.exists());
        let mut envelope: serde_json::Value = serde_json::from_slice(&fs::read(artifact.join("handoff.json"))?)?;
        envelope["schema_version"] = 2.into();
        fs::write(artifact.join("handoff.json"), serde_json::to_vec(&envelope)?)?;
        assert!(
            consume(
                root.path(),
                Path::new("unused"),
                &artifact,
                &path_file,
                &env_file,
                &coordinates
            )
            .is_err()
        );
        Ok(())
    }
}
