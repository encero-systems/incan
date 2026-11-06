//! Linux publisher confinement through bubblewrap, with no ambient filesystem or loader discovery.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::PublisherReadClosure;
use crate::process::{BoundedProcessLimits, BoundedProcessTermination, run_bounded_process};

/// Execute the admitted publisher with its exact runtime files and a private writable product mount.
///
/// Runtime basenames are loader names. Their parent directories deterministically supply `LD_LIBRARY_PATH`; a
/// declared GNU loader is also mounted at the host architecture's kernel interpreter path. The runtime coordinate
/// list and its bytes are bound by the caller's owner identity and receipt. No shell, library tree or host root is
/// implicitly mounted. An unavailable primitive or unsupported loader arrangement refuses before publisher effects.
pub(super) fn run_hermetic_process(
    executable: &Path,
    closure: PublisherReadClosure<'_>,
    arguments: &[OsString],
    environment: &BTreeMap<String, OsString>,
    inputs: &[PathBuf],
    product_root: &Path,
    limits: BoundedProcessLimits,
) -> Result<(), String> {
    let mut command = sandbox_command(
        Path::new("/usr/bin/bwrap"),
        executable,
        closure,
        arguments,
        environment,
        inputs,
        product_root,
    )?;
    complete_publisher_process(&mut command, limits)
}

/// Construct the exact sandbox command; an explicit primitive path permits a missing-tool refusal control.
fn sandbox_command(
    sandbox: &Path,
    executable: &Path,
    closure: PublisherReadClosure<'_>,
    arguments: &[OsString],
    environment: &BTreeMap<String, OsString>,
    inputs: &[PathBuf],
    product_root: &Path,
) -> Result<Command, String> {
    // ---- Exact filesystem and loader authority ----
    if environment.keys().any(|name| name.starts_with("LD_")) {
        return Err("publisher LD_* environment is reserved for its declared runtime files".to_string());
    }
    let product_root =
        std::fs::canonicalize(product_root).map_err(|error| format!("could not resolve product root: {error}"))?;
    let executable = std::fs::canonicalize(executable)
        .map_err(|error| format!("could not resolve publisher executable: {error}"))?;
    let mut reads = BTreeSet::new();
    for path in closure
        .owner
        .iter()
        .map(|entry| entry.physical.as_path())
        .chain(inputs.iter().map(PathBuf::as_path))
    {
        let path = std::fs::canonicalize(path)
            .map_err(|error| format!("could not resolve publisher input {}: {error}", path.display()))?;
        if path.starts_with(&product_root) || product_root.starts_with(&path) {
            return Err("publisher read and writable product mounts overlap".to_string());
        }
        reads.insert(path);
    }
    if !reads
        .iter()
        .any(|path| path == &executable || path.is_dir() && executable.starts_with(path))
    {
        return Err("publisher executable is outside its admitted read closure".to_string());
    }
    let mut runtime_names = BTreeSet::new();
    let mut runtime_directories = BTreeSet::new();
    let mut runtime = Vec::new();
    for file in closure.runtime {
        let file = std::fs::canonicalize(file)
            .map_err(|error| format!("could not resolve publisher runtime file: {error}"))?;
        if !reads.contains(&file) || !file.is_file() {
            return Err("publisher runtime file is outside its exact admitted read closure".to_string());
        }
        let name = file.file_name().ok_or("publisher runtime file has no name")?.to_owned();
        if !runtime_names.insert(name) {
            return Err("publisher runtime files have ambiguous loader basenames".to_string());
        }
        runtime_directories.insert(
            file.parent()
                .ok_or("publisher runtime file has no parent")?
                .to_path_buf(),
        );
        runtime.push(file);
    }
    if runtime_directories.iter().any(|directory| {
        directory
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .any(|byte| matches!(byte, b':' | b';' | b'$'))
    }) {
        return Err("publisher runtime directory contains GNU loader path metacharacters (:, ; or $)".to_string());
    }
    let library_path = std::env::join_paths(runtime_directories)
        .map_err(|error| format!("invalid publisher runtime directories: {error}"))?;

    // ---- Private namespaces, exact read-only mounts, one writable product root ----
    let mut command = Command::new(sandbox);
    command.args([
        "--unshare-user",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-ipc",
        "--unshare-uts",
        "--disable-userns",
        "--cap-drop",
        "ALL",
        "--new-session",
        "--die-with-parent",
        "--clearenv",
    ]);
    for path in &reads {
        command.arg("--ro-bind").arg(path).arg(path);
    }
    let (loader_name, loader_path) = gnu_loader()?;
    for file in &runtime {
        if file.file_name().is_some_and(|name| name == loader_name) {
            command.arg("--ro-bind").arg(file).arg(loader_path);
        }
    }
    command
        .arg("--bind")
        .arg(&product_root)
        .arg(&product_root)
        .arg("--remount-ro")
        .arg("/")
        .arg("--chdir")
        .arg(&product_root);
    for (name, value) in environment {
        command.arg("--setenv").arg(name).arg(value);
    }
    if !runtime.is_empty() {
        command.arg("--setenv").arg("LD_LIBRARY_PATH").arg(library_path);
    }
    command
        .arg("--")
        .arg(&executable)
        .args(arguments)
        .current_dir(&product_root)
        .env_clear();

    Ok(command)
}

/// Apply the existing timeout, bounded output capture and process-tree supervision to the prepared command.
fn complete_publisher_process(command: &mut Command, limits: BoundedProcessLimits) -> Result<(), String> {
    let output = run_bounded_process(command, limits, None)
        .map_err(|error| format!("could not launch publisher confinement: {error}"))?;
    if output.termination != BoundedProcessTermination::Completed || !output.status.success() {
        return Err(format!(
            "termination {:?}, status {}, stderr {}",
            output.termination,
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Select only explicitly supported GNU host interpreter spellings; this is not dependency discovery.
fn gnu_loader() -> Result<(&'static str, &'static str), String> {
    if cfg!(all(target_arch = "x86_64", target_env = "gnu")) {
        Ok(("ld-linux-x86-64.so.2", "/lib64/ld-linux-x86-64.so.2"))
    } else if cfg!(all(target_arch = "aarch64", target_env = "gnu")) {
        Ok(("ld-linux-aarch64.so.1", "/lib/ld-linux-aarch64.so.1"))
    } else {
        Err("publisher confinement has no supported GNU loader contract on this host".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BTreeMap, BoundedProcessLimits, Command, OsString, Path, PathBuf, PublisherReadClosure,
        complete_publisher_process, gnu_loader, sandbox_command,
    };
    use crate::publisher_owner::PublisherOwnerClosurePath;
    use std::error::Error;
    use std::fs;
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    const PROBE: &str = "publisher_execution::linux::tests::publisher_linux_probe";

    /// Execute only when explicitly selected inside the tested sandbox; normal discovery never runs this worker.
    #[test]
    #[ignore = "subprocess worker selected by the Linux confinement controls"]
    fn publisher_linux_probe() -> Result<(), Box<dyn Error>> {
        let mode = std::env::var("OVEN_PUBLISHER_PROBE")?;
        let output = PathBuf::from(std::env::var_os("OVEN_PUBLISHER_OUTPUT").ok_or("missing output")?);
        match mode.as_str() {
            "access" => {
                let input = std::env::var_os("OVEN_PUBLISHER_INPUT").ok_or("missing input")?;
                assert_eq!(fs::read(&input)?, b"admitted input");
                assert!(fs::write(&input, b"tampered input").is_err());
                assert!(fs::read(std::env::var_os("OVEN_PUBLISHER_SECRET").ok_or("missing secret")?).is_err());
                assert!(
                    fs::write(
                        std::env::var_os("OVEN_PUBLISHER_OUTSIDE").ok_or("missing outside")?,
                        b"escape"
                    )
                    .is_err()
                );
                assert!(fs::write("/unowned-write", b"escape").is_err());
                let endpoint: std::net::SocketAddr = std::env::var("OVEN_PUBLISHER_ENDPOINT")?.parse()?;
                assert!(TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)).is_err());
                fs::write(output, b"confined")?;
            }
            "descendant" => {
                let mut child = Command::new(std::env::var_os("OVEN_PUBLISHER_EXE").ok_or("missing probe executable")?);
                child
                    .args(["--exact", PROBE, "--ignored", "--nocapture"])
                    .env("OVEN_PUBLISHER_PROBE", "delayed");
                let _child = child.spawn()?;
                std::thread::sleep(Duration::from_secs(5));
            }
            "delayed" => {
                std::thread::sleep(Duration::from_millis(600));
                fs::write(output, b"escaped descendant")?;
            }
            "overflow" => print!("{}", "overflow".repeat(4096)),
            _ => return Err("unknown publisher probe mode".into()),
        }
        Ok(())
    }

    /// A real copied GNU runtime is explicitly admitted, never discovered by the production launcher.
    struct Fixture {
        root: tempfile::TempDir,
        executable: PathBuf,
        closure: Vec<PublisherOwnerClosurePath>,
        runtime: Vec<PathBuf>,
        input: PathBuf,
        products: PathBuf,
    }

    impl Fixture {
        /// Prepare the test harness and regular-file GNU runtime bundle for the supported Debian CI host.
        fn new() -> Result<Self, Box<dyn Error>> {
            let root = tempfile::tempdir()?;
            let owner = root.path().join("owner");
            fs::create_dir(&owner)?;
            let executable = owner.join("probe");
            let current = std::env::current_exe()?;
            if fs::hard_link(&current, &executable).is_err() {
                fs::copy(&current, &executable)?;
            }
            let multiarch = if cfg!(target_arch = "x86_64") {
                "x86_64-linux-gnu"
            } else {
                "aarch64-linux-gnu"
            };
            let (loader, _) = gnu_loader()?;
            let mut runtime = Vec::new();
            for name in [loader, "libc.so.6", "libgcc_s.so.1", "libm.so.6"] {
                let source = fs::canonicalize(Path::new("/lib").join(multiarch).join(name))?;
                let destination = owner.join(name);
                fs::copy(source, &destination)?;
                runtime.push(destination);
            }
            let closure = std::iter::once(&executable)
                .chain(runtime.iter())
                .map(|path| PublisherOwnerClosurePath {
                    relative: path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    physical: path.clone(),
                    directory: false,
                })
                .collect();
            let input = root.path().join("input");
            fs::write(&input, b"admitted input")?;
            let products = root.path().join("products");
            fs::create_dir(&products)?;
            Ok(Self {
                root,
                executable,
                closure,
                runtime,
                input,
                products,
            })
        }

        /// Invoke exactly the probe case while preserving normal launcher supervision and limits.
        fn run(
            &self,
            sandbox: &Path,
            mode: &str,
            extra: BTreeMap<String, OsString>,
            limits: BoundedProcessLimits,
        ) -> Result<(), String> {
            let mut environment = BTreeMap::from([
                ("OVEN_PUBLISHER_PROBE".to_string(), OsString::from(mode)),
                (
                    "OVEN_PUBLISHER_EXE".to_string(),
                    self.executable.clone().into_os_string(),
                ),
                (
                    "OVEN_PUBLISHER_OUTPUT".to_string(),
                    self.products.join("result").into_os_string(),
                ),
                ("OVEN_PUBLISHER_INPUT".to_string(), self.input.clone().into_os_string()),
            ]);
            environment.extend(extra);
            let arguments = ["--exact", PROBE, "--ignored", "--nocapture"].map(OsString::from);
            let mut command = sandbox_command(
                sandbox,
                &self.executable,
                PublisherReadClosure {
                    owner: &self.closure,
                    runtime: &self.runtime,
                },
                &arguments,
                &environment,
                std::slice::from_ref(&self.input),
                &self.products,
            )?;
            complete_publisher_process(&mut command, limits)
        }
    }

    /// Keep both diagnostic streams and wall time physically bounded for every sandbox control.
    fn limits() -> BoundedProcessLimits {
        BoundedProcessLimits {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            timeout: Some(Duration::from_secs(5)),
        }
    }

    /// Admitted reads and output succeed; host reads, all other writes, and host loopback access are denied.
    #[test]
    fn linux_publisher_confines_exact_files_and_network() -> Result<(), Box<dyn Error>> {
        let fixture = Fixture::new()?;
        let secret = fixture.root.path().join("secret");
        fs::write(&secret, b"private")?;
        let outside = fixture.root.path().join("outside");
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let environment = BTreeMap::from([
            ("OVEN_PUBLISHER_SECRET".to_string(), secret.into_os_string()),
            ("OVEN_PUBLISHER_OUTSIDE".to_string(), outside.clone().into_os_string()),
            (
                "OVEN_PUBLISHER_ENDPOINT".to_string(),
                listener.local_addr()?.to_string().into(),
            ),
        ]);
        fixture.run(Path::new("/usr/bin/bwrap"), "access", environment, limits())?;
        assert_eq!(fs::read(fixture.products.join("result"))?, b"confined");
        assert_eq!(fs::read(&fixture.input)?, b"admitted input");
        assert!(!outside.exists());
        Ok(())
    }

    /// A timed-out publisher's descendant cannot outlive the private PID namespace or write a late product.
    #[test]
    fn linux_publisher_timeout_closes_descendants() -> Result<(), Box<dyn Error>> {
        let fixture = Fixture::new()?;
        let mut limits = limits();
        limits.timeout = Some(Duration::from_millis(200));
        let error = fixture
            .run(Path::new("/usr/bin/bwrap"), "descendant", BTreeMap::new(), limits)
            .err()
            .ok_or("timeout accepted")?;
        assert!(error.contains("TimedOut"), "{error}");
        std::thread::sleep(Duration::from_millis(800));
        assert!(!fixture.products.join("result").exists());
        Ok(())
    }

    /// Physical owner roots must be representable literally in the GNU loader search path.
    #[test]
    fn linux_publisher_refuses_loader_path_metacharacters() -> Result<(), Box<dyn Error>> {
        for name in ["owner;copy", "$ORIGIN", "owner:copy"] {
            let mut fixture = Fixture::new()?;
            let old_owner = fixture
                .executable
                .parent()
                .ok_or("fixture executable has no parent")?
                .to_path_buf();
            let owner = fixture.root.path().join(name);
            fs::rename(&old_owner, &owner)?;
            fixture.executable = owner.join("probe");
            for entry in &mut fixture.closure {
                entry.physical = owner.join(entry.physical.strip_prefix(&old_owner)?);
            }
            for file in &mut fixture.runtime {
                *file = owner.join(file.strip_prefix(&old_owner)?);
            }
            let error = fixture
                .run(Path::new("/usr/bin/bwrap"), "overflow", BTreeMap::new(), limits())
                .err()
                .ok_or("loader path metacharacters accepted")?;
            assert!(error.contains("GNU loader path metacharacters"), "{error}");
            assert!(!fixture.products.join("result").exists());
        }
        Ok(())
    }

    /// Output overflow, missing confinement and conflicting loader variables fail instead of running unconfined.
    #[test]
    fn linux_publisher_refuses_overflow_missing_sandbox_and_loader_override() -> Result<(), Box<dyn Error>> {
        let fixture = Fixture::new()?;
        let error = fixture
            .run(Path::new("/usr/bin/bwrap"), "overflow", BTreeMap::new(), limits())
            .err()
            .ok_or("overflow accepted")?;
        assert!(error.contains("StdoutLimitExceeded"), "{error}");
        let missing = fixture.root.path().join("absent-bwrap");
        let error = fixture
            .run(&missing, "access", BTreeMap::new(), limits())
            .err()
            .ok_or("missing sandbox accepted")?;
        assert!(error.contains("could not launch publisher confinement"));
        let overrides = BTreeMap::from([("LD_PRELOAD".to_string(), OsString::from("untrusted"))]);
        let error = fixture
            .run(Path::new("/usr/bin/bwrap"), "access", overrides, limits())
            .err()
            .ok_or("loader override accepted")?;
        assert!(error.contains("LD_*"));
        assert!(!fixture.products.join("result").exists());
        Ok(())
    }
}
