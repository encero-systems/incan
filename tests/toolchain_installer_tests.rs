use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

mod support;

static PREPARE_ASSETS_LOCK: Mutex<()> = Mutex::new(());
static ACTIVE_TOOLCHAIN_TEST_STAGING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
static TOOLCHAIN_TEST_STAGING_SWEEP: Mutex<()> = Mutex::new(());

const TOOLCHAIN_TEST_STAGING_ROOT: &str = "incan-toolchain-installer-tests";

/// Test-owned release staging with checked cleanup and abandoned-run recovery.
///
/// `tempfile::TempDir` deliberately ignores cleanup failures from `Drop`. That is a poor fit for these tests because
/// a single staging tree can contain several release archives and package-manager fixtures. Keep every tree below a
/// recognizable root, protect active trees with an OS-backed file lock, reclaim trees whose owner process exited,
/// and turn cleanup failures into test failures instead of silently filling temporary storage.
struct ToolchainTestStaging {
    path: PathBuf,
    tempdir: Option<tempfile::TempDir>,
    owner_lock: Option<File>,
}

impl ToolchainTestStaging {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(TOOLCHAIN_TEST_STAGING_ROOT);
        Self::new_in(&root)
    }

    fn new_in(root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let _thread_guard = TOOLCHAIN_TEST_STAGING_SWEEP
            .lock()
            .map_err(|_| "toolchain test staging sweep lock is poisoned")?;
        fs::create_dir_all(root)?;

        let sweep_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".sweep.lock"))?;
        sweep_lock.lock()?;
        reclaim_abandoned_toolchain_staging(root)?;

        let tempdir = tempfile::Builder::new().prefix("staging-").tempdir_in(root)?;
        let owner_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(tempdir.path().join(".owner.lock"))?;
        owner_lock.lock()?;
        let path = tempdir.path().to_path_buf();
        active_toolchain_test_staging()?.insert(path.clone());
        drop(sweep_lock);

        Ok(Self {
            path,
            tempdir: Some(tempdir),
            owner_lock: Some(owner_lock),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(&mut self) -> io::Result<()> {
        let _thread_guard = TOOLCHAIN_TEST_STAGING_SWEEP
            .lock()
            .map_err(|_| io::Error::other("toolchain test staging sweep lock is poisoned"))?;
        let mut active = active_toolchain_test_staging()?;
        let cleanup_result = match self.tempdir.take() {
            Some(tempdir) => tempdir.close(),
            None => Ok(()),
        };
        drop(self.owner_lock.take());
        active.remove(&self.path);
        cleanup_result.map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to remove toolchain test staging {}: {error}",
                    self.path.display()
                ),
            )
        })
    }
}

impl Drop for ToolchainTestStaging {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            if std::thread::panicking() {
                eprintln!("toolchain test staging cleanup also failed: {error}");
            } else {
                panic!("{error}");
            }
        }
    }
}

fn reclaim_abandoned_toolchain_staging(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || !entry.file_name().to_string_lossy().starts_with("staging-") {
            continue;
        }

        let staging = entry.path();
        if active_toolchain_test_staging()?.contains(&staging) {
            continue;
        }
        let owner_path = staging.join(".owner.lock");
        if !owner_path.exists() {
            fs::remove_dir_all(&staging)?;
            continue;
        }

        let owner_lock = OpenOptions::new().read(true).write(true).open(&owner_path)?;
        match owner_lock.try_lock() {
            Ok(()) => {
                drop(owner_lock);
                fs::remove_dir_all(&staging)?;
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    Ok(())
}

fn active_toolchain_test_staging() -> io::Result<std::sync::MutexGuard<'static, BTreeSet<PathBuf>>> {
    ACTIVE_TOOLCHAIN_TEST_STAGING
        .lock()
        .map_err(|_| io::Error::other("active toolchain test staging registry is poisoned"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn installer_script() -> PathBuf {
    repo_root().join("workspaces/release/install-incan.sh")
}

fn toolchain_package_archive_script() -> PathBuf {
    repo_root().join("workspaces/release/toolchain/package_archive.sh")
}

fn toolchain_prepare_assets_script() -> PathBuf {
    repo_root().join("workspaces/release/toolchain/prepare_assets.incn")
}

fn toolchain_local_smoke_script() -> PathBuf {
    repo_root().join("workspaces/release/toolchain/local_smoke.sh")
}

fn npm_prepare_package_script() -> PathBuf {
    repo_root().join("workspaces/release/npm/prepare_package.js")
}

fn npm_installer_wrapper() -> PathBuf {
    repo_root().join("workspaces/release/npm/bin/install-incan.js")
}

fn pip_prepare_package_script() -> PathBuf {
    repo_root().join("workspaces/release/pip/prepare_package.py")
}

fn pip_installer_wrapper() -> PathBuf {
    repo_root().join("workspaces/release/pip/src/incan_toolchain/cli.py")
}

fn sha256_hex(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let digest = Sha256::digest(&bytes);
    Ok(format!("{digest:x}"))
}

fn incan_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_incan") {
        return PathBuf::from(path);
    }
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = PathBuf::from(target_dir).join("debug").join("incan");
        if path.exists() {
            return path;
        }
    }
    repo_root().join("target").join("debug").join("incan")
}

fn prepare_toolchain_assets(
    dist: &Path,
    generated_at: &str,
    skip_homebrew: bool,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let _guard = PREPARE_ASSETS_LOCK.lock().map_err(|_| "prepare assets lock poisoned")?;
    let mut command = Command::new(incan_binary());
    command
        .args(["run"])
        .arg(toolchain_prepare_assets_script())
        .current_dir(repo_root())
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .env("INCAN_HOME", dist.join(".incan-home"))
        .env("INCAN_SOURCE_ROOT", repo_root())
        .env("INCAN_STDLIB", repo_root().join("crates/incan_stdlib/stdlib"))
        .env("INCAN_STDLIB_DIR", repo_root().join("crates/incan_stdlib/stdlib"))
        .env("INCAN_REPO_ROOT", repo_root())
        .env("INCAN_TOOLCHAIN_DIST_DIR", dist)
        .env("INCAN_TOOLCHAIN_GENERATED_AT", generated_at)
        .env(
            "INCAN_GENERATED_CARGO_TARGET_DIR",
            support::generated_cargo_target_dir(),
        );
    if skip_homebrew {
        command.env("INCAN_TOOLCHAIN_SKIP_HOMEBREW", "1");
    }
    Ok(command.output()?)
}

fn write_fixture_archive(root: &Path) -> Result<(PathBuf, String), Box<dyn std::error::Error>> {
    let payload = root.join("payload");
    let bin = payload.join("bin");
    fs::create_dir_all(&bin)?;
    fs::write(bin.join("incan"), "#!/usr/bin/env sh\nprintf 'incan fixture\\n'\n")?;
    fs::write(
        bin.join("incan-lsp"),
        "#!/usr/bin/env sh\nprintf 'incan-lsp fixture\\n'\n",
    )?;
    let sdk = payload.join("share/incan/sdk");
    fs::create_dir_all(&sdk)?;
    fs::write(
        sdk.join("sdk-inventory.json"),
        "{\"schema_version\":1,\"sdk_id\":\"fixture\",\"sdk_version\":\"0.5.0\",\"compiler_requirement\":\">=0.5.0,<0.6.0\",\"components\":{},\"profiles\":{}}\n",
    )?;
    let crates = payload.join("crates");
    fs::create_dir_all(&crates)?;
    fs::write(crates.join("Cargo.toml"), "[workspace]\nmembers = []\n")?;
    for support_crate in [
        "incan_core",
        "incan_derive",
        "incan_stdlib",
        "incan_vocab",
        "incan_web_macros",
    ] {
        let crate_dir = crates.join(support_crate);
        fs::create_dir_all(&crate_dir)?;
        fs::write(
            crate_dir.join("Cargo.toml"),
            format!("[package]\nname = \"{support_crate}\"\n"),
        )?;
    }

    let archive = root.join("incan-v0.4.0-test-x86_64-unknown-linux-gnu.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&payload)
        .arg(".")
        .status()?;
    assert!(status.success(), "tar fixture archive creation failed");

    let checksum = sha256_hex(&archive)?;
    Ok((archive, checksum))
}

fn make_executable(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn write_fixture_command(path: &Path, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(path, format!("#!/usr/bin/env sh\nprintf '{name} fixture\\n'\n"))?;
    make_executable(path)
}

fn write_executable(path: &Path, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(path, contents)?;
    make_executable(path)
}

fn write_fake_bash_arg_printer(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin)?;
    write_executable(
        &fake_bin.join("bash"),
        r#"#!/usr/bin/env sh
set -eu
for arg in "$@"; do
  printf '%s\n' "$arg"
done
"#,
    )?;
    Ok(fake_bin)
}

fn assert_printed_arg_pair(output: &[u8], name: &str, value: &str) {
    let args = String::from_utf8_lossy(output);
    let lines = args.lines().collect::<Vec<_>>();
    assert!(
        lines.windows(2).any(|pair| pair == [name, value]),
        "expected recorded args to contain {name} {value}, got:\n{args}"
    );
}

fn write_fixture_toolchain_commands(root: &Path) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let bin = root.join("commands");
    fs::create_dir_all(&bin)?;
    let incan = bin.join("incan");
    let incan_lsp = bin.join("incan-lsp");
    write_fixture_command(&incan, "incan")?;
    write_fixture_command(&incan_lsp, "incan-lsp")?;
    Ok((incan, incan_lsp))
}

fn write_fixture_sdk_provider_seed(root: &Path, profile: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let seed = root.join(format!("fixture-sdk-provider-seed-{profile}"));
    let components = [
        "stdlib-core",
        "stdlib-system",
        "stdlib-codecs",
        "stdlib-compression",
        "stdlib-data",
        "stdlib-async",
        "stdlib-observability",
        "stdlib-web",
        "stdlib-testing",
    ];
    let mut inventory_components = serde_json::Map::new();
    fs::create_dir_all(&seed)?;
    fs::write(seed.join("Cargo.lock"), "version = 4\n")?;
    for component in components {
        let available = profile != "minimal" || component == "stdlib-core";
        let provider_name = component.replace('-', "_");
        if available {
            let component_dir = seed.join("components").join(component);
            fs::create_dir_all(component_dir.join("src"))?;
            fs::write(
                component_dir.join(format!("{provider_name}.incnlib")),
                format!("{{\"name\":\"{provider_name}\",\"manifest_format\":2}}\n"),
            )?;
            fs::write(
                component_dir.join("Cargo.toml"),
                format!("[package]\nname = \"{provider_name}\"\nversion = \"0.5.0\"\n[lib]\npath = \"src/lib.rs\"\n"),
            )?;
            fs::write(component_dir.join("src/lib.rs"), "pub fn fixture() {}\n")?;
        }
        inventory_components.insert(
            component.to_string(),
            serde_json::json!({
                "version": "0.5.0",
                "mandatory": component == "stdlib-core",
                "available": available,
                "dependencies": [],
                "providers": []
            }),
        );
    }
    let inventory = serde_json::json!({
        "schema_version": 2,
        "sdk_id": "incan-fixture",
        "sdk_version": "0.5.0",
        "compiler_requirement": ">=0.5.0-dev.6,<0.6.0",
        "provider_codegen_revision": incan::version::SDK_PROVIDER_CODEGEN_REVISION,
        "components": inventory_components,
        "profiles": {
            "minimal": ["stdlib-core"],
            "default": components,
            "full": components
        }
    });
    fs::write(
        seed.join("sdk-inventory.json"),
        format!("{}\n", serde_json::to_string_pretty(&inventory)?),
    )?;
    Ok(seed)
}

/// Supply the archive-layout contract to fixture packagers without asking their shell-placeholder compiler to bake
/// a real native closure. End-to-end Loaf validation is covered by the compiler-owned Loaf tests instead.
fn write_fixture_loafs(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let loaf_root = root.join("fixture-oven-loafs");
    let generation = Path::new("generations/fixture-generation");
    let mut members = Vec::new();
    for compatibility_unit in ["base-release", "testing-debug"] {
        let relative = generation.join(format!("{compatibility_unit}.loaf/loaf.json"));
        let unit = loaf_root.join(relative.parent().ok_or("fixture Loaf path has no parent")?);
        fs::create_dir_all(&unit)?;
        fs::write(
            unit.join("loaf.json"),
            format!("{{\"fixture_compatibility_unit\":\"{compatibility_unit}\"}}\n"),
        )?;
        members.push(serde_json::json!({
            "label": compatibility_unit,
            "profile": if compatibility_unit == "base-release" { "release" } else { "debug" },
            "action": if compatibility_unit == "base-release" { "build" } else { "run" },
            "build_unit_identity": format!("sha256:{compatibility_unit}"),
            "path": relative,
        }));
    }
    fs::write(loaf_root.join(".envelope.lock"), "")?;
    fs::write(
        loaf_root.join("envelope.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "envelope": "release",
            "generation_identity": "sha256:fixture-generation",
            "evidence": {},
            "loafs": members,
        }))?,
    )?;
    Ok(loaf_root)
}

const NPM_PLATFORM_TARGETS: [(&str, &str, &str, &str); 3] = [
    ("x86_64-unknown-linux-gnu", "@incan/toolchain-linux-x64", "linux", "x64"),
    ("x86_64-apple-darwin", "@incan/toolchain-darwin-x64", "darwin", "x64"),
    (
        "aarch64-apple-darwin",
        "@incan/toolchain-darwin-arm64",
        "darwin",
        "arm64",
    ),
];

fn npm_platform_package_dir(dist: &Path, target: &str) -> PathBuf {
    dist.join("_npm-platform-packages").join(target)
}

fn current_npm_host_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

/// Sum each regular file exactly once for release-profile accounting assertions.
fn directory_logical_file_bytes(root: &Path) -> Result<u64, Box<dyn std::error::Error>> {
    let mut total = 0_u64;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            total = total.saturating_add(directory_logical_file_bytes(&entry.path())?);
        } else if file_type.is_file() {
            total = total.saturating_add(fs::metadata(entry.path())?.len());
        } else {
            return Err(format!("release fixture contains unsupported path: {}", entry.path().display()).into());
        }
    }
    Ok(total)
}

fn package_fixture_archive(
    root: &Path,
    target: &str,
    incan: &Path,
    incan_lsp: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    package_fixture_archive_with_profile(root, target, incan, incan_lsp, "full")
}

fn package_fixture_archive_with_profile(
    root: &Path,
    target: &str,
    incan: &Path,
    incan_lsp: &Path,
    profile: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let seed = write_fixture_sdk_provider_seed(root, profile)?;
    let loafs = write_fixture_loafs(root)?;
    let output = Command::new("bash")
        .arg(toolchain_package_archive_script())
        .arg(target)
        .args(["--out-dir", root.to_str().ok_or("output path is not UTF-8")?])
        .env("INCAN_BIN", incan)
        .env("INCAN_LSP_BIN", incan_lsp)
        .env("INCAN_SDK_PROVIDER_SEED_DIR", seed)
        .env("INCAN_OVEN_LOAF_DIR", loafs)
        .env("INCAN_OVEN_LOAF_OVERRIDE_TEST_ONLY", "1")
        .env("INCAN_SDK_DISTRIBUTION_PROFILE", profile)
        .current_dir(repo_root())
        .output()?;

    assert!(
        output.status.success(),
        "toolchain archive packaging failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn package_all_npm_fixture_archives(
    dist: &Path,
    incan: &Path,
    incan_lsp: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    for (target, _, _, _) in NPM_PLATFORM_TARGETS {
        package_fixture_archive(dist, target, incan, incan_lsp)?;
    }
    Ok(())
}

fn sha256_sidecar_path(archive: &Path) -> PathBuf {
    archive.with_file_name(format!(
        "{}.sha256",
        archive.file_name().and_then(|name| name.to_str()).unwrap_or_default()
    ))
}

fn profile_evidence_path(archive: &Path) -> PathBuf {
    archive.with_file_name(format!(
        "{}.profile.json",
        archive.file_name().and_then(|name| name.to_str()).unwrap_or_default()
    ))
}

/// Validate the support-crate workspace shape without asking the Cargo-free Oven suite to launch Cargo.
///
/// The package proof still uses `cargo metadata` in ordinary developer test runs. When Oven runs this integration
/// target, the archive has already been created by the named publisher boundary, so validate its complete workspace
/// declaration and every shipped member directly from the immutable extracted files instead.
fn assert_packaged_support_workspace_without_cargo(extracted: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let crates = extracted.join("crates");
    let workspace: toml::Value = toml::from_str(&fs::read_to_string(crates.join("Cargo.toml"))?)?;
    let workspace = workspace
        .get("workspace")
        .and_then(toml::Value::as_table)
        .ok_or("packaged support workspace has no [workspace] table")?;
    let expected_members = [
        "incan_core",
        "incan_derive",
        "incan_stdlib",
        "incan_vocab",
        "incan_web_macros",
    ];
    let members = workspace
        .get("members")
        .and_then(toml::Value::as_array)
        .ok_or("packaged support workspace has no workspace member list")?
        .iter()
        .map(|member| {
            member
                .as_str()
                .ok_or("packaged support workspace has a non-string member")
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(members, expected_members, "packaged support workspace members drifted");
    assert_eq!(workspace.get("resolver").and_then(toml::Value::as_str), Some("2"));
    let package = workspace
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or("packaged support workspace has no [workspace.package] table")?;
    for (field, expected) in [("edition", "2024"), ("rust-version", "1.93"), ("license", "Apache-2.0")] {
        assert_eq!(
            package.get(field).and_then(toml::Value::as_str),
            Some(expected),
            "packaged support workspace has an invalid {field}"
        );
    }
    assert!(
        fs::metadata(crates.join("Cargo.lock"))?.len() > 0,
        "packaged support workspace has an empty Cargo.lock"
    );
    for member in expected_members {
        let manifest: toml::Value = toml::from_str(&fs::read_to_string(crates.join(member).join("Cargo.toml"))?)?;
        let package = manifest
            .get("package")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| format!("packaged support crate {member} has no [package] table"))?;
        assert_eq!(
            package.get("name").and_then(toml::Value::as_str),
            Some(member),
            "packaged support crate {member} declares the wrong package name"
        );
    }
    Ok(())
}

fn oven_compiler_suite_is_active() -> bool {
    std::env::var_os("INCAN_OVEN_COMPILER_SUITE_RUSTC").is_some()
}

fn read_profile_evidence(archive: &Path) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&fs::read_to_string(profile_evidence_path(
        archive,
    ))?)?)
}

fn write_manifest(root: &Path, archive: &Path, checksum: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let manifest = root.join("manifest.json");
    fs::write(
        &manifest,
        format!(
            r#"{{
  "schema_version": 1,
  "toolchain_version": "0.4.0-test",
  "release": "v0.4.0-test",
  "channel": "dev",
  "rust_toolchain": {{
    "channel": "stable",
    "min_rust": "1.93",
    "targets": ["wasm32-wasip1"],
    "policy": "fixture"
  }},
  "commands": ["incan", "incan-lsp"],
  "hosts": {{
    "x86_64-unknown-linux-gnu": {{
      "archive_url": "file://{}",
      "archive_sha256": "{}",
      "archive_format": "tar.gz",
      "commands": {{
        "incan": "bin/incan",
        "incan-lsp": "bin/incan-lsp"
      }}
    }},
    "x86_64-apple-darwin": {{
      "archive_url": "file://{}",
      "archive_sha256": "{}",
      "archive_format": "tar.gz",
      "commands": {{
        "incan": "bin/incan",
        "incan-lsp": "bin/incan-lsp"
      }}
    }},
    "aarch64-apple-darwin": {{
      "archive_url": "file://{}",
      "archive_sha256": "{}",
      "archive_format": "tar.gz",
      "commands": {{
        "incan": "bin/incan",
        "incan-lsp": "bin/incan-lsp"
      }}
    }}
  }}
}}
"#,
            archive.display(),
            checksum,
            archive.display(),
            checksum,
            archive.display(),
            checksum
        ),
    )?;
    Ok(manifest)
}

fn assert_toolchain_install(incan_home: &Path, bin_dir: &Path) {
    assert!(incan_home.join("toolchains/0.4.0-test/bin/incan").exists());
    assert!(incan_home.join("toolchains/0.4.0-test/bin/incan-lsp").exists());
    assert!(
        incan_home
            .join("toolchains/0.4.0-test/share/incan/sdk/sdk-inventory.json")
            .exists()
    );
    assert!(incan_home.join("toolchains/0.4.0-test/crates/Cargo.toml").exists());
    assert!(
        incan_home
            .join("toolchains/0.4.0-test/crates/incan_stdlib/Cargo.toml")
            .exists()
    );
    assert!(incan_home.join("current").exists());
    assert!(bin_dir.join("incan").exists());
    assert!(bin_dir.join("incan-lsp").exists());
}

#[test]
fn toolchain_archive_packager_writes_archive_checksum_and_release_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let out_dir = tmp.path().join("toolchain");
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;

    package_fixture_archive(&out_dir, "x86_64-unknown-linux-gnu", &incan, &incan_lsp)?;

    let version = fs::read_to_string(out_dir.join("toolchain-version.txt"))?;
    let release = fs::read_to_string(out_dir.join("toolchain-release.txt"))?;
    assert!(!version.trim().is_empty());
    assert_eq!(release.trim(), format!("v{}", version.trim()));

    let archive = out_dir.join(format!("incan-{}-x86_64-unknown-linux-gnu.tar.gz", release.trim()));
    assert!(archive.exists(), "archive was not written: {}", archive.display());
    assert_eq!(
        fs::read_to_string(sha256_sidecar_path(&archive))?.trim(),
        sha256_hex(&archive)?
    );
    let evidence = read_profile_evidence(&archive)?;
    assert_eq!(evidence["sdk_profile"], serde_json::json!("full"));
    assert_eq!(evidence["sdk_component_count"], serde_json::json!(9));
    assert_eq!(evidence["archive_bytes"].as_u64(), Some(fs::metadata(&archive)?.len()));
    let package_root = out_dir
        .join("dist")
        .join(format!("incan-{}-x86_64-unknown-linux-gnu", release.trim()));
    assert_eq!(
        evidence["sdk_payload_bytes"].as_u64(),
        Some(directory_logical_file_bytes(&package_root.join("share/incan/sdk"))?)
    );
    assert_eq!(evidence["oven_loaf_count"].as_u64(), Some(2));
    assert_eq!(
        evidence["oven_loaf_logical_bytes"].as_u64(),
        Some(directory_logical_file_bytes(
            &package_root.join("share/incan/oven/loafs"),
        )?)
    );
    assert!(evidence.get("oven_loaf_payload_bytes").is_none());
    assert!(
        evidence["oven_loaf_physical_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
    );
    assert!(evidence.get("oven_loaf_max_bytes").is_none());

    let listing = Command::new("tar").arg("-tzf").arg(&archive).output()?;
    assert!(listing.status.success(), "tar listing failed");
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains("bin/incan"));
    assert!(listing.contains("bin/incan-lsp"));
    assert!(
        !listing.lines().any(|path| path.starts_with("./stdlib/")),
        "toolchain archive must not publish legacy top-level stdlib source:\n{listing}"
    );
    assert!(
        !listing
            .lines()
            .any(|path| path.starts_with("./crates/incan_stdlib/stdlib/")),
        "toolchain archive must not publish provider-owned stdlib source:\n{listing}"
    );
    assert!(
        !listing.lines().any(|path| path.contains(".cargo-target")),
        "toolchain archive must not publish Cargo build intermediates:\n{listing}"
    );
    assert!(
        !listing.lines().any(|path| path.contains("/target/incan_lock/")),
        "toolchain archive must not publish compiler inspection scratch state:\n{listing}"
    );
    assert!(listing.contains("share/incan/sdk/sdk-inventory.json"));
    assert!(listing.contains("share/incan/sdk/Cargo.lock"));
    for compatibility_unit in ["base-release", "testing-debug"] {
        assert!(listing.contains(&format!(
            "share/incan/oven/loafs/generations/fixture-generation/{compatibility_unit}.loaf/loaf.json"
        )));
    }
    for component in [
        "stdlib-core",
        "stdlib-system",
        "stdlib-codecs",
        "stdlib-compression",
        "stdlib-data",
        "stdlib-async",
        "stdlib-observability",
        "stdlib-web",
        "stdlib-testing",
    ] {
        assert!(
            listing.contains(&format!("share/incan/sdk/components/{component}/Cargo.toml")),
            "toolchain archive is missing SDK component {component}:\n{listing}"
        );
        assert!(
            listing.contains(&format!("share/incan/sdk/components/{component}/src/lib.rs")),
            "toolchain archive is missing generated Rust for SDK component {component}:\n{listing}"
        );
        assert!(
            !listing.contains(&format!("share/incan/sdk/components/{component}/Cargo.lock")),
            "SDK component {component} duplicates the shared lockfile:\n{listing}"
        );
    }
    assert!(listing.contains("crates/Cargo.toml"));
    assert!(listing.contains("crates/Cargo.lock"));
    assert!(listing.contains("crates/incan_core/Cargo.toml"));
    assert!(listing.contains("crates/incan_derive/Cargo.toml"));
    assert!(listing.contains("crates/incan_stdlib/Cargo.toml"));
    assert!(listing.contains("crates/incan_vocab/Cargo.toml"));
    assert!(listing.contains("crates/incan_web_macros/Cargo.toml"));

    let extracted = tmp.path().join("extracted-toolchain");
    fs::create_dir_all(&extracted)?;
    let extract = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive)
        .args(["-C"])
        .arg(&extracted)
        .status()?;
    assert!(extract.success(), "toolchain archive extraction failed");
    let shipped_inventory =
        incan::provider::SdkInventory::read_from_path(&extracted.join("share/incan/sdk/sdk-inventory.json"))?;
    shipped_inventory.validate_compiler_compatibility(
        incan::version::INCAN_VERSION,
        incan::version::SDK_PROVIDER_CODEGEN_REVISION,
    )?;
    if oven_compiler_suite_is_active() {
        assert_packaged_support_workspace_without_cargo(&extracted)?;
    } else {
        let metadata = Command::new("cargo")
            .args(["metadata", "--no-deps", "--format-version", "1", "--manifest-path"])
            .arg(extracted.join("crates/Cargo.toml"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            metadata.status.success(),
            "packaged support-crate workspace is invalid:\n{}",
            String::from_utf8_lossy(&metadata.stderr)
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn oven_alpha_benchmark_records_a_verified_cargo_guard_verdict() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let source = tmp.path().join("supported.incn");
    fs::write(&source, "def main() -> None:\n    pass\n")?;
    let clean_worktree_source = tmp.path().join("clean-checkout/supported.incn");
    fs::create_dir_all(
        clean_worktree_source
            .parent()
            .ok_or("clean-worktree fixture source has no parent")?,
    )?;
    fs::copy(&source, &clean_worktree_source)?;
    let incan = tmp.path().join("fixture-incan");
    let fixture_inspection = serde_json::json!({
        "limits": {
            "max_physical_bytes": incan::oven::DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
            "max_domain_physical_bytes": incan::oven::DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
            "max_domain_logical_bytes": incan::oven::DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        }
    });
    write_executable(
        &incan,
        &format!(
            "#!/usr/bin/env sh\nif [ \"$1\" = \"oven\" ]; then printf '%s\\n' '{}'; elif [ \"$1\" = \"--version\" ]; then printf 'incan fixture\\n'; fi\n",
            fixture_inspection
        ),
    )?;
    let rustc = tmp.path().join("fixture-rustc");
    write_executable(&rustc, "#!/usr/bin/env sh\nprintf 'rustc fixture 1.95.0\\n'\n")?;
    let guard_dir = tmp.path().join("cargo-guard");
    fs::create_dir_all(&guard_dir)?;
    write_executable(&guard_dir.join("cargo"), "#!/usr/bin/env sh\nexit 97\n")?;
    let output_dir = tmp.path().join("benchmark-evidence");

    let output = Command::new("bash")
        .arg(repo_root().join("scripts/bench_oven_alpha.sh"))
        .args([
            "--incan",
            incan.to_str().ok_or("fixture incan path is not UTF-8")?,
            "--release-identity",
            "fixture-release-artifact",
            "--rustc",
            rustc.to_str().ok_or("fixture rustc path is not UTF-8")?,
            "--checkout-revision",
            "fixture-revision",
            "--workload",
            "build",
            "--source",
            source.to_str().ok_or("fixture source path is not UTF-8")?,
            "--incan-home",
            tmp.path()
                .join("incan-home")
                .to_str()
                .ok_or("fixture home path is not UTF-8")?,
            "--output",
            output_dir.to_str().ok_or("fixture output path is not UTF-8")?,
            "--clean-worktree-source",
            clean_worktree_source
                .to_str()
                .ok_or("fixture clean-worktree source path is not UTF-8")?,
            "--cargo-guard-dir",
            guard_dir.to_str().ok_or("fixture guard path is not UTF-8")?,
            "--repetitions",
            "1",
        ])
        .current_dir(repo_root())
        .output()?;
    assert!(
        output.status.success(),
        "guarded benchmark fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_str(&fs::read_to_string(output_dir.join("report.json"))?)?;
    assert_eq!(report["cargo_guard"]["required"], serde_json::json!(true));
    assert_eq!(
        report["toolchain"]["rustc_identity"],
        serde_json::json!("rustc fixture 1.95.0")
    );
    assert_eq!(report["cargo_guard"]["probe_exit_code"], serde_json::json!(97));
    assert_eq!(
        report["cargo_guard"]["verdict"],
        serde_json::json!("successful normal stages imply that Cargo was not launched")
    );
    assert!(
        report["timing"]["wall_clock_ms"].as_u64().is_some(),
        "benchmark report must retain complete wall-clock timing"
    );
    assert!(
        report["timing"]["first_materialization_ms"].as_u64().is_some(),
        "benchmark report must retain cold materialization timing"
    );
    assert!(
        report["timing"]["warm_repeat_total_ms"].as_u64().is_some(),
        "benchmark report must retain prepared warm timing"
    );
    assert_eq!(
        report["toolchain"]["release_identity"],
        serde_json::json!("fixture-release-artifact")
    );
    assert_eq!(
        report["provenance"]["checkout_revision"],
        serde_json::json!("fixture-revision")
    );
    assert_eq!(
        report["workload"]["source_sha256"],
        report["workload"]["clean_worktree_source_sha256"]
    );
    assert_eq!(
        report["store"]["requested_limit_overrides"]["max_physical_bytes"],
        serde_json::Value::Null
    );
    assert_eq!(report["store"]["effective_limits"], fixture_inspection["limits"]);
    let storage_junctions = report["storage_junctions"]
        .as_array()
        .ok_or("benchmark report is missing storage junctions")?;
    let junction_names = storage_junctions
        .iter()
        .map(|junction| junction["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        junction_names,
        [
            "initial",
            "after_first_materialization",
            "after_warm_repeat_1",
            "after_clean_worktree_reuse",
        ]
    );
    for junction in storage_junctions {
        let reports = junction["reports"]
            .as_object()
            .ok_or("storage junction is missing report paths")?;
        for report_path in reports.values().filter_map(serde_json::Value::as_str) {
            assert!(
                output_dir.join(report_path).is_file(),
                "storage junction report is not retained: {report_path}"
            );
        }
    }
    Ok(())
}

#[test]
fn compiler_suite_action_composes_baker_guarded_runner_and_storage_evidence() -> Result<(), Box<dyn std::error::Error>>
{
    let action = fs::read_to_string(repo_root().join(".github/actions/run-oven-compiler-suite/action.yml"))?;
    for required in [
        "baker-result.json",
        "oven compiler-libtests",
        "cargo-guard",
        "consumer_toolchain",
        "rustup which --toolchain \"${{ inputs.consumer_toolchain }}\" rustc",
        "rustup which --toolchain \"${{ inputs.publisher_toolchain }}\" cargo",
        "--fixture-cargo \"$fixture_cargo_path\"",
        "fixture_invocation_count",
        "publisher[0].compiler_suite.store",
        "suite[0].store",
        "total_ms",
        "publisher_ms",
        "prepared_replay_ms",
        "raw_after_baker_kib",
        "raw-disk-usage-kib",
    ] {
        assert!(
            action.contains(required),
            "compiler-suite action must retain `{required}`"
        );
    }
    assert!(
        !action.contains("oven legacy-cargo"),
        "the Cargo-free suite action must consume the baker result rather than own another publisher"
    );
    assert!(
        !action.contains("oven store inspect"),
        "the suite action must consume product-owned baker/replay store reports instead of reconstructing them"
    );
    let makefile = fs::read_to_string(repo_root().join("Makefile"))?;
    assert!(
        makefile.contains("test-oven: test-prewarm-oven-loafs")
            && makefile.contains("oven legacy-cargo bake-loafs")
            && makefile.contains("--suite-store \"$(INCAN_TEST_OVEN_COMPILER_SUITE_STORE)\""),
        "the local and CI suite prewarm must use the typed Loaf baker"
    );
    assert!(
        makefile.contains("test-prewarm-oven-release-loafs: test-prewarm-sdk")
            && makefile.contains("--envelope release")
            && makefile.contains("INCAN_TEST_OVEN_RELEASE_TOOLCHAIN_ROOT"),
        "normal-command evidence must use a staged toolchain with the typed release Loaf envelope"
    );
    assert!(
        makefile.contains("INCAN_TEST_PUBLISHER_TOOLCHAIN ?= nightly-2026-03-24")
            && makefile.contains("INCAN_TEST_FIXTURE_CARGO_TOOLCHAIN ?= $(INCAN_TEST_PUBLISHER_TOOLCHAIN)")
            && makefile.contains("INCAN_TEST_SUITE_TOOLCHAIN ?= stable")
            && makefile
                .contains("--cargo \"$$(rustup which --toolchain \"$(INCAN_TEST_PUBLISHER_TOOLCHAIN)\" cargo)\"")
            && makefile.contains("--rustc \"$$(rustup which --toolchain \"$(INCAN_TEST_LOAF_TOOLCHAIN)\" rustc)\""),
        "the named publisher Cargo and direct-rustc consumer toolchains must remain separate"
    );
    let partition_target = makefile
        .split_once(".PHONY: test-oven-partition")
        .and_then(|(_, suffix)| suffix.split_once(".PHONY: test-oven-replay"))
        .map(|(target, _)| target)
        .ok_or("Makefile omitted the prepared Oven partition target")?;
    assert!(
        partition_target.contains("INCAN_TEST_OVEN_PARTITION_INDEX")
            && partition_target.contains("INCAN_TEST_OVEN_PARTITION_COUNT")
            && !partition_target.contains("test-prewarm-oven-loafs"),
        "a partition replay must require explicit partition coordinates and never silently prewarm or bake"
    );
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))?;
    let linux_prewarm = workflow
        .find("linux-reference-handoff:")
        .ok_or("pull-request CI is missing the Linux Oven prewarm handoff")?;
    let linux_prewarm_workflow = &workflow[linux_prewarm..];
    let compiler_build = linux_prewarm_workflow
        .find("- name: Build Linux compiler and reference generators")
        .ok_or("pull-request CI is missing Linux compiler build")?;
    let provider_restore = linux_prewarm_workflow
        .find("- uses: ./.github/actions/restore-sdk-provider-store")
        .ok_or("pull-request CI is missing SDK provider cache restore")?;
    let complete_suite = linux_prewarm_workflow
        .find("- name: Prewarm the complete Linux stable Oven suite")
        .ok_or("pull-request CI is missing complete Linux stable Oven suite")?;
    assert!(
        compiler_build < provider_restore && provider_restore < complete_suite,
        "pull-request CI must restore its exact SDK provider cache after compiling the identity-bearing compiler and before the complete Linux stable Oven suite"
    );
    assert!(
        workflow.contains("INCAN_OVEN_NATIVE_TEST_CASE_TIMINGS")
            && workflow.contains("INCAN_TEST_COMMAND_TIMINGS")
            && workflow.contains("INCAN_TEST_OVEN_COMPILER_SUITE_REPORT")
            && workflow.contains("oven-pr-linux-partition-${{ matrix.partition }}"),
        "the first stable Linux partition must retain case and nested-command timing evidence needed to investigate remaining native test costs"
    );
    let provider_cache_action =
        fs::read_to_string(repo_root().join(".github/actions/restore-sdk-provider-store/action.yml"))?;
    assert!(
        provider_cache_action.contains("oven sdk-provider-store-identity")
            && provider_cache_action.contains("rustc --version --verbose")
            && provider_cache_action.contains("incan-sdk-provider-v3"),
        "the SDK provider cache must use the compiler-owned source identity and retain a selected-rustc suffix"
    );
    assert!(
        !provider_cache_action.contains("shasum -a 256 target/debug/incan"),
        "a rebuilt development compiler executable must not force an SDK provider cache miss"
    );
    let evidence_workflow = fs::read_to_string(repo_root().join(".github/workflows/oven_evidence.yml"))?;
    for required in [
        "toolchain: 1.93.0",
        "consumer_toolchain: ${{ matrix.toolchain }}",
        "INCAN_TEST_LOAF_TOOLCHAIN=${{ matrix.toolchain }}",
        "INCAN_TEST_SUITE_TOOLCHAIN=${{ matrix.toolchain }}",
        "test-prewarm-oven-release-loafs",
        "src/oven/fixtures/release_core.incn",
        "target/oven-alpha-release-toolchain/bin/incan",
    ] {
        assert!(
            evidence_workflow.contains(required),
            "release evidence CI must retain `{required}`"
        );
    }
    assert!(
        workflow.contains("cancel-in-progress: true")
            && workflow.contains("make -s test-prewarm-oven-loafs")
            && workflow.contains("make -s test-oven-partition")
            && workflow.contains("make test-oven-pr-regressions")
            && workflow.contains("linux-reference-handoff")
            && workflow.contains("oven-platform-smoke")
            && workflow.contains("oven-linux-replay")
            && workflow.contains("actions/cache/save@v4")
            && workflow.contains("fail-on-cache-miss: true")
            && workflow.contains("partition: [0, 1, 2, 3]")
            && workflow.contains("TEST_ROOT=tests/cli_integration.rs")
            && workflow.matches("Install WASI target for vocab desugarers").count() == 3
            && !workflow.contains("make test-oven-focused"),
        "pull-request CI must cancel superseded runs, prewarm the complete stable Linux suite once, replay its four receipt partitions without rebaking, retain the platform CLI/C ABI root on macOS and MSRV, and retain Linux process-containment coverage"
    );
    assert!(
        evidence_workflow.contains("uses: ./.github/actions/run-oven-compiler-suite"),
        "complete compiler-suite correctness must remain in explicit release evidence"
    );
    let focused_target = makefile
        .split_once(".PHONY: test-oven-focused")
        .and_then(|(_, suffix)| suffix.split_once(".PHONY: test-oven-pr-regressions"))
        .map(|(target, _)| target)
        .ok_or("Makefile omitted the focused Oven target boundary")?;
    let focused_cargo_tests = focused_target
        .lines()
        .filter(|line| line.contains("cargo test"))
        .collect::<Vec<_>>();
    assert_eq!(focused_cargo_tests.len(), 4);
    assert!(
        focused_cargo_tests
            .iter()
            .all(|line| line.contains("CARGO_PROFILE_TEST_DEBUG=0")),
        "focused Oven tests must not pay the cold-link cost of unused test debug information"
    );
    assert!(
        !focused_target.contains("--features lsp"),
        "focused Oven tests must not compile the unrelated LSP feature graph"
    );
    assert!(
        makefile.contains(
            "CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo test --locked --features lsp --test oven_pr_regressions"
        ),
        "the bounded PR containment lane must suppress unused test debug information"
    );
    assert!(
        !workflow.contains("run-oven-compiler-suite") && !workflow.contains("bench_oven_alpha.sh"),
        "complete repository-suite and benchmark evidence must not run on every pull-request commit"
    );
    assert!(
        !repo_root().join("scripts/run_oven_compiler_suite.sh").exists(),
        "product-level compiler-suite orchestration must not live in shell"
    );
    Ok(())
}

#[test]
fn minimal_sdk_archive_physically_excludes_non_profile_components() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let out_dir = tmp.path().join("minimal-toolchain");
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;

    package_fixture_archive_with_profile(&out_dir, "x86_64-unknown-linux-gnu", &incan, &incan_lsp, "minimal")?;

    let release = fs::read_to_string(out_dir.join("toolchain-release.txt"))?;
    let archive = out_dir.join(format!("incan-{}-x86_64-unknown-linux-gnu.tar.gz", release.trim()));
    let evidence = read_profile_evidence(&archive)?;
    assert_eq!(evidence["sdk_profile"], serde_json::json!("minimal"));
    assert_eq!(evidence["sdk_component_count"], serde_json::json!(1));
    assert!(evidence["sdk_payload_bytes"].as_u64().is_some_and(|bytes| bytes > 0));
    assert_eq!(evidence["oven_loaf_count"].as_u64(), Some(2));
    let listing = Command::new("tar").arg("-tzf").arg(&archive).output()?;
    assert!(listing.status.success(), "minimal archive listing failed");
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains("share/incan/sdk/components/stdlib-core/"));
    for component in [
        "stdlib-system",
        "stdlib-codecs",
        "stdlib-compression",
        "stdlib-data",
        "stdlib-async",
        "stdlib-observability",
        "stdlib-web",
        "stdlib-testing",
    ] {
        assert!(
            !listing.contains(&format!("share/incan/sdk/components/{component}/")),
            "minimal archive unexpectedly contains {component}:\n{listing}"
        );
    }
    Ok(())
}

#[test]
fn default_sdk_archive_contains_every_default_profile_component() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let out_dir = tmp.path().join("default-toolchain");
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;

    package_fixture_archive_with_profile(&out_dir, "x86_64-unknown-linux-gnu", &incan, &incan_lsp, "default")?;

    let release = fs::read_to_string(out_dir.join("toolchain-release.txt"))?;
    let archive = out_dir.join(format!("incan-{}-x86_64-unknown-linux-gnu.tar.gz", release.trim()));
    let evidence = read_profile_evidence(&archive)?;
    assert_eq!(evidence["sdk_profile"], serde_json::json!("default"));
    assert_eq!(evidence["sdk_component_count"], serde_json::json!(9));
    assert!(evidence["sdk_payload_bytes"].as_u64().is_some_and(|bytes| bytes > 0));
    assert_eq!(evidence["oven_loaf_count"].as_u64(), Some(2));
    let listing = Command::new("tar").arg("-tzf").arg(&archive).output()?;
    assert!(listing.status.success(), "default archive listing failed");
    let listing = String::from_utf8_lossy(&listing.stdout);
    for component in [
        "stdlib-core",
        "stdlib-system",
        "stdlib-codecs",
        "stdlib-compression",
        "stdlib-data",
        "stdlib-async",
        "stdlib-observability",
        "stdlib-web",
        "stdlib-testing",
    ] {
        assert!(
            listing.contains(&format!("share/incan/sdk/components/{component}/")),
            "default archive is missing {component}:\n{listing}"
        );
    }
    Ok(())
}

#[test]
fn toolchain_release_assets_are_prepared_by_central_manifest_program() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let dist = tmp.path().join("toolchain");
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;

    for target in [
        "x86_64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ] {
        package_fixture_archive(&dist, target, &incan, &incan_lsp)?;
    }

    let output = prepare_toolchain_assets(&dist, "2026-06-06T00:00:00Z", false)?;

    assert!(
        output.status.success(),
        "toolchain asset preparation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(dist.join("manifest.json"))?)?;
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["generated_at"], "2026-06-06T00:00:00Z");
    assert_eq!(manifest["rust_toolchain"]["targets"][0], "wasm32-wasip1");
    assert!(
        manifest["rust_toolchain"]["policy"]
            .as_str()
            .unwrap_or_default()
            .contains("provisions stable Rust through rustup"),
        "manifest should document installer-managed Rust provisioning"
    );
    assert!(
        manifest["hosts"]["x86_64-unknown-linux-gnu"]["archive_url"]
            .as_str()
            .unwrap_or_default()
            .contains("/releases/download/")
    );
    assert!(dist.join("install.sh").exists());
    assert!(dist.join("toolchain-manifest.schema.v1.json").exists());
    let formula = fs::read_to_string(dist.join("incan.rb"))?;
    let version = env!("CARGO_PKG_VERSION");
    let release = format!("v{version}");
    let archive = dist.join(format!("incan-v{version}-x86_64-unknown-linux-gnu.tar.gz"));
    let archive_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("toolchain archive name was not valid UTF-8")?;
    let checksum = fs::read_to_string(sha256_sidecar_path(&archive))?.trim().to_string();
    assert!(formula.contains(&format!(r#"version "{version}""#)));
    assert!(formula.contains("npm and Homebrew install prebuilt Incan commands"));
    assert!(formula.contains(&format!(
        r#"url "https://github.com/encero-systems/incan/releases/download/{release}/{archive_name}""#
    )));
    assert!(formula.contains(&format!(r#"sha256 "{checksum}""#)));
    assert!(formula.contains("def staged_files"));
    assert!(formula.contains(r##"(Dir["#{buildpath}/**/*"] + Dir["**/*"]).uniq"##));
    assert!(formula.contains("def staged_binary(name)"));
    assert!(formula.contains("path = staged_files.find do |candidate|"));
    assert!(formula.contains("File.basename(candidate) == name && File.basename(File.dirname(candidate)) == \"bin\""));
    assert!(formula.contains("path.nil? ? nil : Pathname.new(path)"));
    assert!(formula.contains("def staged_file_sample"));
    assert!(formula.contains("incan_bin = staged_binary(\"incan\")"));
    assert!(formula.contains("incan_lsp_bin = staged_binary(\"incan-lsp\")"));
    assert!(formula.contains("sdk_inventory = Pathname.new(\"share/incan/sdk/sdk-inventory.json\")"));
    assert!(formula.contains(
        r#"odie "could not find incan binary in archive; staged files: #{staged_file_sample}" if incan_bin.nil?"#
    ));
    assert!(formula.contains(
        r#"odie "could not find SDK provider inventory in archive; staged files: #{staged_file_sample}" unless sdk_inventory.exist?"#
    ));
    assert!(formula.contains("could not find SDK provider inventory in archive"));
    assert!(formula.contains("libexec.install Dir[\"*\"]"));
    assert!(formula.contains("bin.write_exec_script libexec/\"bin/incan\""));
    assert!(formula.contains("bin.write_exec_script libexec/\"bin/incan-lsp\""));
    assert!(formula.contains("Incan builds supported projects with verified Oven direct-rustc plans"));
    assert!(!formula.contains("Incan builds projects through Cargo"));
    Ok(())
}

#[test]
fn toolchain_release_assets_can_be_prepared_for_single_host_smoke_without_homebrew()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let dist = tmp.path().join("toolchain");
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;

    package_fixture_archive(&dist, "aarch64-apple-darwin", &incan, &incan_lsp)?;

    let output = prepare_toolchain_assets(&dist, "2026-06-06T00:00:00Z", true)?;

    assert!(
        output.status.success(),
        "single-host toolchain asset preparation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(dist.join("manifest.json"))?)?;
    assert!(manifest["hosts"]["aarch64-apple-darwin"].is_object());
    assert!(dist.join("install.sh").exists());
    assert!(dist.join("toolchain-manifest.schema.v1.json").exists());
    assert!(!dist.join("incan.rb").exists());
    Ok(())
}

#[test]
fn package_prepare_scripts_stage_versions_and_shared_installer() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let dist = tmp.path().join("toolchain");
    fs::create_dir_all(&dist)?;
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;
    package_all_npm_fixture_archives(&dist, &incan, &incan_lsp)?;
    let npm_version = fs::read_to_string(dist.join("toolchain-version.txt"))?
        .trim()
        .to_string();

    let npm_output = Command::new("node")
        .arg(npm_prepare_package_script())
        .arg(&dist)
        .arg("--skip-pack")
        .output()?;
    assert!(
        npm_output.status.success(),
        "npm package preparation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&npm_output.stdout),
        String::from_utf8_lossy(&npm_output.stderr)
    );
    let npm_package: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dist.join("_npm-package/package.json"))?)?;
    assert_eq!(npm_package["version"], npm_version);
    assert_eq!(npm_package["homepage"], "https://incan.io");
    assert!(
        npm_package["files"]
            .as_array()
            .ok_or("npm files field must be an array")?
            .iter()
            .any(|entry| entry == "README.md")
    );
    assert!(
        npm_package
            .get("scripts")
            .and_then(|scripts| scripts.get("postinstall"))
            .is_none(),
        "default npm package must not declare postinstall"
    );
    let optional_dependencies = npm_package["optionalDependencies"]
        .as_object()
        .ok_or("npm optionalDependencies must be an object")?;
    for (target, package_name, os, cpu) in NPM_PLATFORM_TARGETS {
        assert_eq!(
            optional_dependencies
                .get(package_name)
                .and_then(serde_json::Value::as_str),
            Some(npm_version.as_str()),
            "top-level npm package must depend on {package_name}"
        );

        let platform_dir = npm_platform_package_dir(&dist, target);
        let platform_package: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(platform_dir.join("package.json"))?)?;
        assert_eq!(platform_package["name"], package_name);
        assert_eq!(platform_package["version"], npm_version);
        assert_eq!(platform_package["os"], serde_json::json!([os]));
        assert_eq!(platform_package["cpu"], serde_json::json!([cpu]));
        assert!(platform_dir.join("toolchain/bin/incan").exists());
        assert!(platform_dir.join("toolchain/bin/incan-lsp").exists());
        assert!(
            platform_dir
                .join("toolchain/share/incan/sdk/sdk-inventory.json")
                .exists()
        );
        assert!(platform_dir.join("toolchain/crates/Cargo.toml").exists());
    }
    assert!(fs::read_to_string(dist.join("_npm-package/README.md"))?.contains("https://incan.io"));
    assert!(dist.join("_npm-package/vendor/install-incan.sh").exists());

    fs::write(dist.join("toolchain-version.txt"), "0.4.0-dev.6\n")?;
    let pip_output = Command::new("python3")
        .arg(pip_prepare_package_script())
        .arg(&dist)
        .arg("--skip-build")
        .output()?;
    assert!(
        pip_output.status.success(),
        "pip package preparation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&pip_output.stdout),
        String::from_utf8_lossy(&pip_output.stderr)
    );
    let pip_project = fs::read_to_string(dist.join("_pip-package/pyproject.toml"))?;
    assert!(pip_project.contains(r#"version = "0.4.0.dev6""#));
    assert!(pip_project.contains(r#"Homepage = "https://incan.io""#));
    assert!(fs::read_to_string(dist.join("_pip-package/README.md"))?.contains("https://incan.io"));
    assert!(
        fs::read_to_string(dist.join("_pip-package/src/incan_toolchain/__init__.py"))?
            .contains(r#"__version__ = "0.4.0.dev6""#)
    );
    assert!(
        dist.join("_pip-package/src/incan_toolchain/vendor/install-incan.sh")
            .exists()
    );

    fs::write(dist.join("toolchain-version.txt"), "0.4.0-rc1\n")?;
    let pip_output = Command::new("python3")
        .arg(pip_prepare_package_script())
        .arg(&dist)
        .arg("--skip-build")
        .output()?;
    assert!(
        pip_output.status.success(),
        "pip rc package preparation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&pip_output.stdout),
        String::from_utf8_lossy(&pip_output.stderr)
    );
    assert!(fs::read_to_string(dist.join("_pip-package/pyproject.toml"))?.contains(r#"version = "0.4.0rc1""#));
    assert!(
        fs::read_to_string(dist.join("_pip-package/src/incan_toolchain/__init__.py"))?
            .contains(r#"__version__ = "0.4.0rc1""#)
    );
    Ok(())
}

#[test]
fn npm_command_wrappers_run_platform_package_without_installer() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let dist = tmp.path().join("toolchain");
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;
    package_all_npm_fixture_archives(&dist, &incan, &incan_lsp)?;

    let npm_output = Command::new("node")
        .arg(npm_prepare_package_script())
        .arg(&dist)
        .arg("--skip-pack")
        .output()?;
    assert!(
        npm_output.status.success(),
        "npm package preparation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&npm_output.stdout),
        String::from_utf8_lossy(&npm_output.stderr)
    );

    let package_root = dist.join("_npm-package");
    let node_modules_scope = package_root.join("node_modules/@incan");
    copy_dir_recursive(
        &npm_platform_package_dir(&dist, "x86_64-unknown-linux-gnu"),
        &node_modules_scope.join("toolchain-linux-x64"),
    )?;
    fs::remove_file(package_root.join("vendor/install-incan.sh"))?;

    let incan_output = Command::new("node")
        .arg(package_root.join("bin/incan.js"))
        .env("INCAN_NPM_HOST_TARGET", "x86_64-unknown-linux-gnu")
        .output()?;
    assert!(
        incan_output.status.success(),
        "incan npm shim failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&incan_output.stdout),
        String::from_utf8_lossy(&incan_output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&incan_output.stdout), "incan fixture\n");

    let incan_lsp_output = Command::new("node")
        .arg(package_root.join("bin/incan-lsp.js"))
        .arg("--help")
        .env("INCAN_NPM_HOST_TARGET", "x86_64-unknown-linux-gnu")
        .output()?;
    assert!(
        incan_lsp_output.status.success(),
        "incan-lsp npm shim failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&incan_lsp_output.stdout),
        String::from_utf8_lossy(&incan_lsp_output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&incan_lsp_output.stdout), "incan-lsp fixture\n");
    Ok(())
}

#[test]
fn npm_command_wrappers_report_unsupported_platforms() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let dist = tmp.path().join("toolchain");
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;
    package_all_npm_fixture_archives(&dist, &incan, &incan_lsp)?;

    let npm_output = Command::new("node")
        .arg(npm_prepare_package_script())
        .arg(&dist)
        .arg("--skip-pack")
        .output()?;
    assert!(
        npm_output.status.success(),
        "npm package preparation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&npm_output.stdout),
        String::from_utf8_lossy(&npm_output.stderr)
    );

    let package_root = dist.join("_npm-package");
    fs::remove_file(package_root.join("vendor/install-incan.sh"))?;

    let output = Command::new("node")
        .arg(package_root.join("bin/incan.js"))
        .env("INCAN_NPM_HOST_TARGET", "sparc64-sun-solaris")
        .output()?;
    assert!(
        !output.status.success(),
        "unsupported npm platform should fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported npm toolchain target: sparc64-sun-solaris"));
    assert!(stderr.contains("x86_64-unknown-linux-gnu"));
    assert!(stderr.contains("x86_64-apple-darwin"));
    assert!(stderr.contains("aarch64-apple-darwin"));
    Ok(())
}

#[test]
fn toolchain_installer_dry_run_selects_manifest_target_without_writing() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let (archive, checksum) = write_fixture_archive(tmp.path())?;
    let manifest = write_manifest(tmp.path(), &archive, &checksum)?;
    let incan_home = tmp.path().join("home");
    let bin_dir = tmp.path().join("bin");

    let output = Command::new("bash")
        .arg(installer_script())
        .args(["--manifest", manifest.to_str().ok_or("manifest path is not UTF-8")?])
        .args(["--target", "x86_64-unknown-linux-gnu"])
        .args(["--incan-home", incan_home.to_str().ok_or("home path is not UTF-8")?])
        .args(["--bin-dir", bin_dir.to_str().ok_or("bin path is not UTF-8")?])
        .arg("--dry-run")
        .output()?;

    assert!(
        output.status.success(),
        "installer dry-run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Incan toolchain 0.4.0-test"));
    assert!(stdout.contains("target:     x86_64-unknown-linux-gnu"));
    assert!(stdout.contains("Dry run only"));
    assert!(!incan_home.exists(), "dry-run must not create INCAN_HOME");
    assert!(!bin_dir.exists(), "dry-run must not create command bin directory");
    Ok(())
}

#[test]
fn toolchain_installer_verifies_checksum_and_links_commands() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let (archive, checksum) = write_fixture_archive(tmp.path())?;
    let manifest = write_manifest(tmp.path(), &archive, &checksum)?;
    let incan_home = tmp.path().join("home");
    let bin_dir = tmp.path().join("bin");

    let output = Command::new("bash")
        .arg(installer_script())
        .args(["--manifest", manifest.to_str().ok_or("manifest path is not UTF-8")?])
        .args(["--target", "x86_64-unknown-linux-gnu"])
        .args(["--archive", archive.to_str().ok_or("archive path is not UTF-8")?])
        .args(["--incan-home", incan_home.to_str().ok_or("home path is not UTF-8")?])
        .args(["--bin-dir", bin_dir.to_str().ok_or("bin path is not UTF-8")?])
        .env("INCAN_SKIP_RUST_INSTALL", "1")
        .output()?;

    assert!(
        output.status.success(),
        "installer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_toolchain_install(&incan_home, &bin_dir);
    Ok(())
}

#[test]
fn toolchain_installer_provisions_rust_backend_targets() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let (archive, checksum) = write_fixture_archive(tmp.path())?;
    let manifest = write_manifest(tmp.path(), &archive, &checksum)?;
    let incan_home = tmp.path().join("home");
    let bin_dir = tmp.path().join("bin");
    let fake_bin = tmp.path().join("fake-bin");
    fs::create_dir_all(&fake_bin)?;
    let rustup_log = tmp.path().join("rustup.log");

    write_executable(
        &fake_bin.join("rustup"),
        "#!/usr/bin/env sh\nprintf '%s\\n' \"$*\" >> \"$RUSTUP_LOG\"\n",
    )?;
    write_executable(
        &fake_bin.join("cargo"),
        "#!/usr/bin/env sh\nprintf 'cargo 1.96.0 fixture\\n'\n",
    )?;
    write_executable(
        &fake_bin.join("rustc"),
        "#!/usr/bin/env sh\nprintf 'rustc 1.96.0 fixture\\n'\n",
    )?;

    let current_path = std::env::var("PATH")?;
    let output = Command::new("bash")
        .arg(installer_script())
        .args(["--manifest", manifest.to_str().ok_or("manifest path is not UTF-8")?])
        .args(["--target", "x86_64-unknown-linux-gnu"])
        .args(["--archive", archive.to_str().ok_or("archive path is not UTF-8")?])
        .args(["--incan-home", incan_home.to_str().ok_or("home path is not UTF-8")?])
        .args(["--bin-dir", bin_dir.to_str().ok_or("bin path is not UTF-8")?])
        .env("PATH", format!("{}:{current_path}", fake_bin.display()))
        .env("RUSTUP_LOG", &rustup_log)
        .output()?;

    assert!(
        output.status.success(),
        "installer failed with fake Rust backend\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Rust backend:"));
    assert!(stdout.contains("target: wasm32-wasip1"));
    let rustup_log = fs::read_to_string(rustup_log)?;
    assert!(
        rustup_log.lines().any(|line| line == "target add wasm32-wasip1"),
        "expected installer to add manifest Rust target, got:\n{rustup_log}"
    );
    assert_toolchain_install(&incan_home, &bin_dir);
    Ok(())
}

#[test]
fn toolchain_installer_bootstraps_rustup_when_missing() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let (archive, checksum) = write_fixture_archive(tmp.path())?;
    let manifest = write_manifest(tmp.path(), &archive, &checksum)?;
    let incan_home = tmp.path().join("home");
    let bin_dir = tmp.path().join("bin");
    let fake_home = tmp.path().join("fake-home");
    fs::create_dir_all(&fake_home)?;
    let rustup_log = tmp.path().join("rustup-bootstrap.log");
    let rustup_init = tmp.path().join("rustup-init.sh");

    write_executable(
        &rustup_init,
        r#"#!/usr/bin/env sh
set -eu
mkdir -p "$HOME/.cargo/bin"
cat > "$HOME/.cargo/bin/rustup" <<'RUSTUP'
#!/usr/bin/env sh
printf '%s\n' "$*" >> "$RUSTUP_LOG"
RUSTUP
cat > "$HOME/.cargo/bin/cargo" <<'CARGO'
#!/usr/bin/env sh
printf 'cargo 1.96.0 fixture\n'
CARGO
cat > "$HOME/.cargo/bin/rustc" <<'RUSTC'
#!/usr/bin/env sh
printf 'rustc 1.96.0 fixture\n'
RUSTC
chmod +x "$HOME/.cargo/bin/rustup" "$HOME/.cargo/bin/cargo" "$HOME/.cargo/bin/rustc"
"#,
    )?;

    let output = Command::new("bash")
        .arg(installer_script())
        .args(["--manifest", manifest.to_str().ok_or("manifest path is not UTF-8")?])
        .args(["--target", "x86_64-unknown-linux-gnu"])
        .args(["--archive", archive.to_str().ok_or("archive path is not UTF-8")?])
        .args(["--incan-home", incan_home.to_str().ok_or("home path is not UTF-8")?])
        .args(["--bin-dir", bin_dir.to_str().ok_or("bin path is not UTF-8")?])
        .env("HOME", &fake_home)
        .env("CARGO_HOME", fake_home.join(".cargo"))
        .env("INCAN_RUSTUP_INIT", &rustup_init)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("RUSTUP_LOG", &rustup_log)
        .output()?;

    assert!(
        output.status.success(),
        "installer failed to bootstrap fake Rust backend\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Installing Rust backend with rustup (stable)"));
    assert!(stdout.contains("Rust backend:"));
    let rustup_log = fs::read_to_string(rustup_log)?;
    assert!(
        rustup_log.lines().any(|line| line == "target add wasm32-wasip1"),
        "expected bootstrapped rustup to add manifest Rust target, got:\n{rustup_log}"
    );
    assert_toolchain_install(&incan_home, &bin_dir);
    Ok(())
}

#[test]
fn homebrew_smoke_preserves_existing_platform_archives() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = PREPARE_ASSETS_LOCK.lock().map_err(|_| "prepare assets lock poisoned")?;
    let tmp = ToolchainTestStaging::new()?;
    let dist = tmp.path().join("toolchain");
    let fake_bin = tmp.path().join("fake-bin");
    fs::create_dir_all(&fake_bin)?;
    write_executable(
        &fake_bin.join("ruby"),
        "#!/usr/bin/env sh\nif [ \"$1\" = \"-c\" ]; then exit 0; fi\nexit 0\n",
    )?;
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;
    let targets = [
        "x86_64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ];

    for target in targets {
        package_fixture_archive(&dist, target, &incan, &incan_lsp)?;
    }

    let release = fs::read_to_string(dist.join("toolchain-release.txt"))?
        .trim()
        .to_string();
    let before = targets
        .iter()
        .map(|target| {
            let archive = dist.join(format!("incan-{release}-{target}.tar.gz"));
            let checksum = sha256_sidecar_path(&archive);
            Ok((
                target.to_string(),
                sha256_hex(&archive)?,
                fs::read_to_string(&checksum)?,
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;

    let path = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap_or_default());
    let output = Command::new("bash")
        .arg(toolchain_local_smoke_script())
        .arg("homebrew")
        .current_dir(repo_root())
        .env("PATH", path)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .env("INCAN_HOME", tmp.path().join("incan-home"))
        .env("TOOLCHAIN_DIST", &dist)
        .env("TOOLCHAIN_GENERATED_AT", "2026-06-06T00:00:00Z")
        .env("TOOLCHAIN_HOST_TARGET", "x86_64-unknown-linux-gnu")
        .env("TOOLCHAIN_INCAN_BIN", incan_binary())
        .output()?;

    assert!(
        output.status.success(),
        "homebrew smoke failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    for (target, archive_hash, checksum_contents) in before {
        let archive = dist.join(format!("incan-{release}-{target}.tar.gz"));
        let checksum = sha256_sidecar_path(&archive);
        assert_eq!(sha256_hex(&archive)?, archive_hash, "archive changed for {target}");
        assert_eq!(
            fs::read_to_string(&checksum)?,
            checksum_contents,
            "checksum sidecar changed for {target}"
        );
    }
    Ok(())
}

#[test]
fn npm_smoke_installs_platform_package_without_lifecycle_scripts() -> Result<(), Box<dyn std::error::Error>> {
    let Some(host_target) = current_npm_host_target() else {
        return Ok(());
    };
    let tmp = ToolchainTestStaging::new()?;
    let dist = tmp.path().join("toolchain");
    let (incan, incan_lsp) = write_fixture_toolchain_commands(tmp.path())?;
    package_all_npm_fixture_archives(&dist, &incan, &incan_lsp)?;

    let output = Command::new("bash")
        .arg(toolchain_local_smoke_script())
        .arg("npm")
        .current_dir(repo_root())
        .env("TOOLCHAIN_DIST", &dist)
        .env("TOOLCHAIN_HOST_TARGET", host_target)
        .output()?;

    assert!(
        output.status.success(),
        "npm smoke failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn npm_installer_wrapper_delegates_to_shared_toolchain_installer() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let (archive, checksum) = write_fixture_archive(tmp.path())?;
    let manifest = write_manifest(tmp.path(), &archive, &checksum)?;
    let incan_home = tmp.path().join("npm-home");
    let bin_dir = tmp.path().join("npm-bin");

    let output = Command::new("node")
        .arg(npm_installer_wrapper())
        .args(["--manifest", manifest.to_str().ok_or("manifest path is not UTF-8")?])
        .args(["--target", "x86_64-unknown-linux-gnu"])
        .args(["--archive", archive.to_str().ok_or("archive path is not UTF-8")?])
        .args(["--incan-home", incan_home.to_str().ok_or("home path is not UTF-8")?])
        .args(["--bin-dir", bin_dir.to_str().ok_or("bin path is not UTF-8")?])
        .env("INCAN_SKIP_RUST_INSTALL", "1")
        .output()?;

    assert!(
        output.status.success(),
        "npm wrapper failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_toolchain_install(&incan_home, &bin_dir);
    Ok(())
}

#[test]
fn npm_installer_wrapper_defaults_to_its_own_release_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let fake_bin = write_fake_bash_arg_printer(tmp.path())?;
    let current_path = std::env::var("PATH")?;
    let expected_manifest = "https://github.com/encero-systems/incan/releases/download/v0.4.0/manifest.json";

    let output = Command::new("node")
        .arg(npm_installer_wrapper())
        .arg("--package-install")
        .arg("--dry-run")
        .env("PATH", format!("{}:{current_path}", fake_bin.display()))
        .env_remove("INCAN_TOOLCHAIN_MANIFEST")
        .env_remove("INCAN_SKIP_NPM_INSTALL")
        .output()?;

    assert!(
        output.status.success(),
        "npm wrapper failed with fake bash\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_printed_arg_pair(&output.stdout, "--manifest", expected_manifest);
    Ok(())
}

#[test]
fn pip_installer_wrapper_delegates_to_shared_toolchain_installer() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let (archive, checksum) = write_fixture_archive(tmp.path())?;
    let manifest = write_manifest(tmp.path(), &archive, &checksum)?;
    let incan_home = tmp.path().join("pip-home");
    let bin_dir = tmp.path().join("pip-bin");

    let output = Command::new("python3")
        .arg(pip_installer_wrapper())
        .arg("install")
        .args(["--manifest", manifest.to_str().ok_or("manifest path is not UTF-8")?])
        .args(["--target", "x86_64-unknown-linux-gnu"])
        .args(["--archive", archive.to_str().ok_or("archive path is not UTF-8")?])
        .args(["--incan-home", incan_home.to_str().ok_or("home path is not UTF-8")?])
        .args(["--bin-dir", bin_dir.to_str().ok_or("bin path is not UTF-8")?])
        .env("INCAN_SKIP_RUST_INSTALL", "1")
        .output()?;

    assert!(
        output.status.success(),
        "pip wrapper failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_toolchain_install(&incan_home, &bin_dir);
    Ok(())
}

#[test]
fn pip_installer_wrapper_defaults_to_its_own_release_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = ToolchainTestStaging::new()?;
    let fake_bin = write_fake_bash_arg_printer(tmp.path())?;
    let current_path = std::env::var("PATH")?;
    let expected_manifest = "https://github.com/encero-systems/incan/releases/download/v0.4.0/manifest.json";

    let output = Command::new("python3")
        .arg(pip_installer_wrapper())
        .arg("install")
        .arg("--dry-run")
        .env("PATH", format!("{}:{current_path}", fake_bin.display()))
        .env_remove("INCAN_TOOLCHAIN_MANIFEST")
        .output()?;

    assert!(
        output.status.success(),
        "pip wrapper failed with fake bash\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_printed_arg_pair(&output.stdout, "--manifest", expected_manifest);
    Ok(())
}

fn return_error_after_creating_toolchain_staging(
    root: &Path,
    staging_path: &mut Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let staging = ToolchainTestStaging::new_in(root)?;
    *staging_path = Some(staging.path().to_path_buf());
    fs::write(staging.path().join("partial-release-asset"), "fixture")?;
    Err(io::Error::other("fixture subprocess failed").into())
}

#[test]
fn toolchain_test_staging_is_removed_after_a_successful_path() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let staging_path = {
        let staging = ToolchainTestStaging::new_in(root.path())?;
        let staging_path = staging.path().to_path_buf();
        fs::write(staging.path().join("release-asset"), "fixture")?;
        staging_path
    };

    assert!(
        !staging_path.exists(),
        "successful test path retained release staging: {}",
        staging_path.display()
    );
    Ok(())
}

#[test]
fn toolchain_test_staging_is_removed_after_a_failing_path() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let mut staging_path = None;
    let result = return_error_after_creating_toolchain_staging(root.path(), &mut staging_path);

    assert!(result.is_err(), "fixture failure must propagate");
    let staging_path = staging_path.ok_or("fixture did not report its staging path")?;
    assert!(
        !staging_path.exists(),
        "failed test path retained release staging: {}",
        staging_path.display()
    );
    Ok(())
}

#[test]
fn toolchain_test_staging_creation_and_cleanup_share_one_sweep_guard() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let root_path = Arc::new(root.path().to_path_buf());
    let mut workers = Vec::new();

    for worker in 0..8 {
        let root_path = Arc::clone(&root_path);
        workers.push(std::thread::spawn(move || -> Result<(), String> {
            for iteration in 0..32 {
                let mut staging = ToolchainTestStaging::new_in(&root_path).map_err(|error| error.to_string())?;
                fs::write(
                    staging.path().join(format!("release-asset-{worker}-{iteration}")),
                    "fixture",
                )
                .map_err(|error| error.to_string())?;
                staging.cleanup().map_err(|error| error.to_string())?;
            }
            Ok(())
        }));
    }

    for worker in workers {
        match worker.join() {
            Ok(result) => result.map_err(io::Error::other)?,
            Err(_) => return Err(io::Error::other("toolchain staging worker panicked").into()),
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn toolchain_test_staging_surfaces_cleanup_failures() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let mut staging = ToolchainTestStaging::new_in(root.path())?;
    let staging_path = staging.path().to_path_buf();
    fs::write(staging.path().join("release-asset"), "fixture")?;

    let original_permissions = fs::metadata(&staging_path)?.permissions();
    let mut blocked_permissions = original_permissions.clone();
    blocked_permissions.set_mode(0o500);
    fs::set_permissions(&staging_path, blocked_permissions)?;

    let cleanup_result = staging.cleanup();
    if staging_path.exists() {
        fs::set_permissions(&staging_path, original_permissions)?;
        fs::remove_dir_all(&staging_path)?;
    }

    let cleanup_error = cleanup_result
        .err()
        .ok_or("staging cleanup failure was silently ignored")?;
    let cleanup_diagnostic = cleanup_error.to_string();
    assert!(
        cleanup_diagnostic.contains("failed to remove toolchain test staging"),
        "cleanup failure was not actionable: {cleanup_diagnostic}"
    );
    assert!(
        cleanup_diagnostic.contains(staging_path.to_string_lossy().as_ref()),
        "cleanup failure did not include the staging path: {cleanup_diagnostic}"
    );
    Ok(())
}

#[test]
fn toolchain_test_staging_reclaims_an_abandoned_unlocked_run() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let mut abandoned = ToolchainTestStaging::new_in(root.path())?;
    let abandoned_path = abandoned.path().to_path_buf();
    fs::write(abandoned.path().join("partial-release-asset"), "fixture")?;

    let owner_lock = abandoned.owner_lock.take().ok_or("fixture owner lock is unavailable")?;
    owner_lock.unlock()?;
    drop(owner_lock);
    assert!(
        active_toolchain_test_staging()?.remove(&abandoned_path),
        "fixture staging was not registered as active"
    );
    let kept_path = abandoned
        .tempdir
        .take()
        .ok_or("fixture staging directory is unavailable")?
        .keep();
    drop(abandoned);
    assert_eq!(kept_path, abandoned_path);
    assert!(abandoned_path.exists(), "fixture must emulate abandoned staging");

    let active = ToolchainTestStaging::new_in(root.path())?;
    assert!(
        !abandoned_path.exists(),
        "a later toolchain test run did not reclaim abandoned staging: {}",
        abandoned_path.display()
    );
    assert!(
        active.path().exists(),
        "active staging must remain protected by its owner lock"
    );
    Ok(())
}
