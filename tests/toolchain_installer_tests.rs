use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

static PREPARE_ASSETS_LOCK: Mutex<()> = Mutex::new(());

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
        .env("INCAN_REPO_ROOT", repo_root())
        .env("INCAN_TOOLCHAIN_DIST_DIR", dist)
        .env("INCAN_TOOLCHAIN_GENERATED_AT", generated_at)
        .env(
            "INCAN_GENERATED_CARGO_TARGET_DIR",
            repo_root().join("target/incan_generated_shared_target"),
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
    let stdlib = payload.join("stdlib");
    fs::create_dir_all(&stdlib)?;
    fs::write(stdlib.join("testing.incn"), "# fixture std.testing source\n")?;
    let crates = payload.join("crates");
    fs::create_dir_all(&crates)?;
    fs::write(crates.join("Cargo.toml"), "[workspace]\nmembers = []\n")?;
    for support_crate in ["incan_core", "incan_derive", "incan_stdlib", "incan_web_macros"] {
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

fn write_fake_bash_recorder(root: &Path) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin)?;
    let log = root.join("bash-args.log");
    write_executable(
        &fake_bin.join("bash"),
        r#"#!/usr/bin/env sh
set -eu
: > "$FAKE_BASH_LOG"
for arg in "$@"; do
  printf '%s\n' "$arg" >> "$FAKE_BASH_LOG"
done
"#,
    )?;
    Ok((fake_bin, log))
}

fn assert_recorded_arg_pair(log: &Path, name: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    let args = fs::read_to_string(log)?;
    let lines = args.lines().collect::<Vec<_>>();
    assert!(
        lines.windows(2).any(|pair| pair == [name, value]),
        "expected recorded args to contain {name} {value}, got:\n{args}"
    );
    Ok(())
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

fn package_fixture_archive(
    root: &Path,
    target: &str,
    incan: &Path,
    incan_lsp: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("bash")
        .arg(toolchain_package_archive_script())
        .arg(target)
        .args(["--out-dir", root.to_str().ok_or("output path is not UTF-8")?])
        .env("INCAN_BIN", incan)
        .env("INCAN_LSP_BIN", incan_lsp)
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
    assert!(incan_home.join("toolchains/0.4.0-test/stdlib/testing.incn").exists());
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
    let tmp = tempfile::tempdir()?;
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

    let listing = Command::new("tar").arg("-tzf").arg(&archive).output()?;
    assert!(listing.status.success(), "tar listing failed");
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains("bin/incan"));
    assert!(listing.contains("bin/incan-lsp"));
    assert!(listing.contains("stdlib/testing.incn"));
    assert!(listing.contains("stdlib/prelude.incn"));
    assert!(listing.contains("crates/Cargo.toml"));
    assert!(listing.contains("crates/incan_core/Cargo.toml"));
    assert!(listing.contains("crates/incan_derive/Cargo.toml"));
    assert!(listing.contains("crates/incan_stdlib/Cargo.toml"));
    assert!(listing.contains("crates/incan_web_macros/Cargo.toml"));
    Ok(())
}

#[test]
fn toolchain_release_assets_are_prepared_by_central_manifest_script() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
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
    assert!(formula.contains("def staged_binary(name)"));
    assert!(formula.contains("could not find stdlib/testing.incn in archive"));
    assert!(formula.contains("libexec.install Dir[\"*\"]"));
    assert!(formula.contains("bin.write_exec_script libexec/\"bin/incan\""));
    assert!(formula.contains("bin.write_exec_script libexec/\"bin/incan-lsp\""));
    Ok(())
}

#[test]
fn toolchain_release_assets_can_be_prepared_for_single_host_smoke_without_homebrew()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
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
        assert!(platform_dir.join("toolchain/stdlib/testing.incn").exists());
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
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
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
fn homebrew_formula_is_rendered_from_the_toolchain_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
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
        "formula rendering failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let version = env!("CARGO_PKG_VERSION");
    let target = "x86_64-unknown-linux-gnu";
    let release = format!("v{version}");
    let archive_name = format!("incan-v{version}-{target}.tar.gz");
    let checksum = fs::read_to_string(dist.join(format!("{archive_name}.sha256")))?
        .trim()
        .to_string();
    let formula = fs::read_to_string(dist.join("incan.rb"))?;
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
    assert!(formula.contains("stdlib_dir = Pathname.new(\"stdlib\")"));
    assert!(formula.contains(
        r#"odie "could not find incan binary in archive; staged files: #{staged_file_sample}" if incan_bin.nil?"#
    ));
    assert!(formula.contains(
        r#"odie "could not find stdlib/testing.incn in archive; staged files: #{staged_file_sample}" unless (stdlib_dir/"testing.incn").exist?"#
    ));
    assert!(formula.contains("libexec.install Dir[\"*\"]"));
    assert!(formula.contains("bin.write_exec_script libexec/\"bin/incan\""));
    assert!(formula.contains("bin.write_exec_script libexec/\"bin/incan-lsp\""));
    Ok(())
}

#[test]
fn homebrew_smoke_preserves_existing_platform_archives() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = PREPARE_ASSETS_LOCK.lock().map_err(|_| "prepare assets lock poisoned")?;
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
    let (fake_bin, log) = write_fake_bash_recorder(tmp.path())?;
    let current_path = std::env::var("PATH")?;
    let expected_manifest = "https://github.com/encero-systems/incan/releases/download/v0.4.0/manifest.json";

    let output = Command::new("node")
        .arg(npm_installer_wrapper())
        .arg("--package-install")
        .arg("--dry-run")
        .env("PATH", format!("{}:{current_path}", fake_bin.display()))
        .env("FAKE_BASH_LOG", &log)
        .env_remove("INCAN_TOOLCHAIN_MANIFEST")
        .env_remove("INCAN_SKIP_NPM_INSTALL")
        .output()?;

    assert!(
        output.status.success(),
        "npm wrapper failed with fake bash\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_recorded_arg_pair(&log, "--manifest", expected_manifest)?;
    Ok(())
}

#[test]
fn pip_installer_wrapper_delegates_to_shared_toolchain_installer() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
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
    let tmp = tempfile::tempdir()?;
    let (fake_bin, log) = write_fake_bash_recorder(tmp.path())?;
    let current_path = std::env::var("PATH")?;
    let expected_manifest = "https://github.com/encero-systems/incan/releases/download/v0.4.0/manifest.json";

    let output = Command::new("python3")
        .arg(pip_installer_wrapper())
        .arg("install")
        .arg("--dry-run")
        .env("PATH", format!("{}:{current_path}", fake_bin.display()))
        .env("FAKE_BASH_LOG", &log)
        .env_remove("INCAN_TOOLCHAIN_MANIFEST")
        .output()?;

    assert!(
        output.status.success(),
        "pip wrapper failed with fake bash\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_recorded_arg_pair(&log, "--manifest", expected_manifest)?;
    Ok(())
}
