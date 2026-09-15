//! Identity and lifecycle of the SDK provider store: what makes one compiled-SDK tree distinct from another, the
//! locks that serialize its preparation, and the digests that key it.
//!
//! The identity folds the compiler executable and the effect digest of the sources the compiler was built from, so
//! a rebuilt frontend never reads yesterday's providers back.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::{env, fs};

use sha2::{Digest, Sha256};

use crate::provider::effect_digest::{COMPILER_RUST_EFFECT_ROOTS, COMPILER_STDLIB_ROOT, compiler_effect_digest};
use crate::provider::error::{ProviderError, ProviderResult};
static SDK_PROVIDER_COMPILER_DIGESTS: LazyLock<Mutex<HashMap<PathBuf, [u8; 32]>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Internal provider-store override used by isolated compiler and packaging tests.
pub(crate) const INTERNAL_SDK_PROVIDER_STORE_ENV: &str = "INCAN_INTERNAL_SDK_PROVIDER_STORE";

/// Internal file through which release packaging receives the exact immutable SDK provider root.
pub(crate) const INTERNAL_SDK_PROVIDER_PATH_FILE_ENV: &str = "INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE";

/// Internal SDK distribution profile used by release packaging to omit component payloads physically.
pub(crate) const INTERNAL_SDK_DISTRIBUTION_PROFILE_ENV: &str = "INCAN_INTERNAL_SDK_DISTRIBUTION_PROFILE";

/// Internal path override for the Cargo.lock payload used while producing a compiler-owned artifact.
pub(crate) const INTERNAL_CARGO_LOCK_PAYLOAD_PATH_ENV: &str = "INCAN_INTERNAL_CARGO_LOCK_PAYLOAD_PATH";

/// Select the Incan CLI executable that prepares SDK provider artifacts.
///
/// Cargo integration tests and development utilities do not run inside the `incan` CLI. Tests receive the real binary
/// through `CARGO_BIN_EXE_incan`; utility binaries use the sibling CLI built in the same target directory. Returning an
/// error is important: executing a generator with CLI arguments can exit successfully without publishing an artifact.
pub(crate) fn sdk_provider_builder_executable(
    cargo_test_binary: Option<PathBuf>,
    current_executable: PathBuf,
) -> ProviderResult<PathBuf> {
    if let Some(executable) = cargo_test_binary.as_ref().filter(|path| path.is_file()) {
        return Ok(executable.clone());
    }

    let binary_dir = current_executable.parent().unwrap_or_else(|| Path::new("."));
    let mut sibling = binary_dir.join("incan");
    sibling.set_extension(std::env::consts::EXE_EXTENSION);
    if sibling.is_file() {
        return Ok(sibling);
    }

    let mut parent_sibling = binary_dir.parent().unwrap_or_else(|| Path::new(".")).join("incan");
    parent_sibling.set_extension(std::env::consts::EXE_EXTENSION);
    if parent_sibling.is_file() {
        return Ok(parent_sibling);
    }

    let supplied = cargo_test_binary
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unset".to_string());
    Err(ProviderError::failure(format!(
        "SDK provider publication requires the incan CLI executable at {} or {}; CARGO_BIN_EXE_incan={supplied}, current executable={}; build that binary before running compiler-backed utilities",
        sibling.display(),
        parent_sibling.display(),
        current_executable.display(),
    )))
}

/// Find the verified workspace Cargo.lock available to a development SDK provider build.
///
/// A standalone artifact crate otherwise resolves its own newest compatible versions, which can differ from the
/// compiler workspace's verified offline cache. Installed SDK layouts need not contain a workspace lockfile, so they
/// deliberately retain normal Cargo resolution.
pub(crate) fn sdk_provider_workspace_lock(stdlib_root: &Path) -> Option<PathBuf> {
    stdlib_root
        .ancestors()
        .skip(1)
        .map(|parent| parent.join("Cargo.lock"))
        .find(|path| path.is_file())
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
}

/// Pass the verified SDK lock to the child that owns generated output publication.
///
/// The child's lock resolver and project generator materialize this payload inside its library transaction. Writing
/// Cargo.lock into the output beforehand would create a nonempty directory without generated-library ownership.
pub(crate) fn configure_sdk_provider_workspace_lock(command: &mut Command, workspace_lock: Option<&Path>) {
    let Some(workspace_lock) = workspace_lock else {
        return;
    };
    command.env(INTERNAL_CARGO_LOCK_PAYLOAD_PATH_ENV, workspace_lock);
}

/// Keep the bootstrap artifact lock alive for the whole preparation/publish transaction.
///
/// The compiler cannot call its own Incan `std.fs` artifact before that artifact exists. This is therefore a
/// deliberately narrow native bootstrap boundary, mirroring RFC 112's advisory-lock contract while the compiler
/// produces the first Incan-owned stdlib artifact.
pub(crate) struct SdkProviderStoreLock {
    _file: fs::File,
}

/// Acquire the artifact-store lock that serializes all bootstrap builds and publications.
pub(crate) fn acquire_sdk_provider_store_lock(store_root: &Path) -> ProviderResult<SdkProviderStoreLock> {
    fs::create_dir_all(store_root).map_err(|error| {
        ProviderError::failure(format!(
            "failed to create SDK provider store {}: {error}",
            store_root.display()
        ))
    })?;
    let lock_path = store_root.join(".incan.lock");
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            ProviderError::failure(format!("failed to open artifact lock {}: {error}", lock_path.display()))
        })?;
    // Try first, and say something before settling in to wait. Serializing the store is correct — two processes
    // publishing providers at once is what this lock exists to prevent — but an unannounced block is
    // indistinguishable from a hang, and preparing providers can take minutes. A command that has stopped printing
    // for no stated reason gets misread as a compiler defect on whatever project happened to be open. See #1514.
    if file.try_lock().is_err() {
        eprintln!(
            "Waiting for the SDK provider store at {}. Another Incan process holds its lock; this command \
             continues as soon as that one releases it.",
            store_root.display()
        );
        file.lock().map_err(|error| {
            ProviderError::failure(format!(
                "failed to acquire artifact lock {}: {error}",
                lock_path.display()
            ))
        })?;
    }
    Ok(SdkProviderStoreLock { _file: file })
}

/// Hash one sorted provider source subtree while excluding generated build output.
fn hash_sdk_provider_source_tree(root: &Path, current: &Path, hasher: &mut Sha256) -> ProviderResult<()> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| {
            ProviderError::failure(format!(
                "failed to read stdlib source directory {}: {error}",
                current.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            ProviderError::failure(format!(
                "failed to enumerate stdlib source directory {}: {error}",
                current.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|error| {
            ProviderError::failure(format!(
                "failed to make stdlib source path {} relative: {error}",
                path.display()
            ))
        })?;
        if relative.components().any(|component| component.as_os_str() == "target") {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            ProviderError::failure(format!(
                "failed to inspect stdlib source path {}: {error}",
                path.display()
            ))
        })?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        if file_type.is_dir() {
            hasher.update(b"directory\0");
            hash_sdk_provider_source_tree(root, &path, hasher)?;
        } else if file_type.is_file() {
            hasher.update(b"file\0");
            let bytes = fs::read(&path).map_err(|error| {
                ProviderError::failure(format!("failed to read stdlib source file {}: {error}", path.display()))
            })?;
            hasher.update(bytes);
        } else if file_type.is_symlink() {
            hasher.update(b"symlink\0");
            let target = fs::read_link(&path).map_err(|error| {
                ProviderError::failure(format!(
                    "failed to read stdlib source symlink {}: {error}",
                    path.display()
                ))
            })?;
            hasher.update(target.to_string_lossy().as_bytes());
        }
        hasher.update([0xff]);
    }
    Ok(())
}

/// Derive the immutable provider-store identity from every input that can change generated Rust or its dependency
/// closure. The identity is content based, so a stale provider set is never accepted because a directory exists.
///
/// Development binaries are rebuilt when test-only Rust changes, and their raw bytes are not a stable description of
/// compiler behavior. A checkout therefore contributes an effect digest — what this compiler *produces* for this
/// standard library — while an installed toolchain, which has no source to inspect, falls back to the executable
/// digest.
///
/// # Why the effect digest replaced the source closure
///
/// Through v3 a checkout folded a hash of its whole `src/`, `crates/` and `tests/` tree, so editing the language
/// server, a CLI command or an inspection module rebuilt all ten SDK components at a cost of roughly seventeen
/// minutes (#1495). Hashing the source answers "did the compiler change", and what the cache needs to know is
/// "would this compiler produce different components", which is a different question for almost every edit anyone
/// makes. [`compiler_effect_digest`] answers the second one directly.
///
/// [`crate::version::SDK_PROVIDER_CODEGEN_REVISION`] is folded alongside it. Publication code can change the shape
/// of the store without changing any component's content, and that constant is the declared mechanism for saying
/// so; the inventory already validates it on every cache hit, and folding it here means a bump also partitions the
/// store rather than only rejecting what is in it.
pub(crate) fn sdk_provider_store_identity(
    stdlib_root: &Path,
    executable: &Path,
    workspace_lock: Option<&Path>,
    distribution_profile: &str,
) -> ProviderResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"incan-sdk-provider-store-v4\0");
    hasher.update(b"compiler-version\0");
    hasher.update(crate::version::INCAN_VERSION.as_bytes());
    hasher.update(b"provider-codegen-revision\0");
    hasher.update(crate::version::SDK_PROVIDER_CODEGEN_REVISION.to_le_bytes());
    hasher.update(b"distribution-profile\0");
    hasher.update(distribution_profile.as_bytes());

    let executable = fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    if let Some(checkout_root) = sdk_provider_compiler_checkout_root(stdlib_root) {
        hasher.update(b"compiler-effect\0");
        hasher.update(sdk_provider_effect_digest(&checkout_root)?.as_bytes());
    } else {
        hash_sdk_provider_source_tree(stdlib_root, stdlib_root, &mut hasher)?;
        hasher.update(b"compiler-executable-content\0");
        hasher.update(sdk_provider_compiler_digest(&executable)?);
    }

    hasher.update(b"workspace-lock\0");
    if let Some(workspace_lock) = workspace_lock {
        hasher.update(fs::read(workspace_lock).map_err(|error| {
            ProviderError::failure(format!(
                "failed to read workspace lock {}: {error}",
                workspace_lock.display()
            ))
        })?);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Digest what the compiler in `checkout_root` produces for its standard library, memoized on its inputs' bytes.
///
/// The digest itself costs about 1.8 seconds over all 104 standard-library sources, which is three orders of
/// magnitude below the rebuild it prevents but far too much to pay on every command that resolves the provider
/// store. So it is computed once per distinct input content and read back afterwards.
///
/// The memo key is a plain byte hash of the same roots the digest reads, folded with a stamp of the compiler
/// executable doing the reading. The digest is a pure function of those bytes *and* of the frontend that lexes,
/// parses, checks and lowers them, so a key over the bytes alone would let a rebuilt compiler read the previous
/// compiler's answer back from disk and leave the store identity where it was. Any edit to the roots — even one the
/// digest would forgive, like a comment — and any rebuild of the compiler miss the memo and recompute rather than
/// returning a stale answer; a compiler that cannot stamp its own executable does not memoize at all. Entries are
/// published by rename, because `make -j` puts many processes on one cache and a half-written digest is still a
/// well-formed cache key.
///
/// # The memo changes the cost, never the value
///
/// An environment with nowhere to put the memo recomputes the digest each time and gets the same answer. That is
/// deliberate rather than an omission: the identity must be a function of the compiler and its standard library
/// alone. Substituting the cheaper byte hash where no cache exists would make two machines with identical source
/// publish to two different store paths, which is exactly what
/// [`sdk_provider_store_identity_for_compiler_root`] exists to prevent.
fn sdk_provider_effect_digest(checkout_root: &Path) -> ProviderResult<String> {
    let cached_path = match running_compiler_stamp() {
        Some(compiler_stamp) => {
            let content_key = sdk_provider_effect_input_key(checkout_root, &compiler_stamp)?;
            sdk_provider_effect_digest_cache_root().map(|root| root.join(&content_key))
        }
        None => None,
    };
    if let Some(cached_path) = &cached_path
        && let Ok(cached) = fs::read_to_string(cached_path)
        && cached.starts_with("sha256:")
    {
        return Ok(cached.trim().to_string());
    }

    let root = checkout_root.to_path_buf();
    let digest = crate::compiler_stack::run_on_compiler_stack(move || {
        compiler_effect_digest(&root).map_err(|error| error.to_string())
    })
    .map_err(|message| {
        ProviderError::failure(format!(
            "failed to digest compiler effect for the standard library: {message}"
        ))
    })?;

    if let Some(cached_path) = &cached_path {
        write_effect_digest_memo(cached_path, &digest);
    }
    Ok(digest)
}

/// Publish one memoized digest by rename, so a concurrent reader never sees a partially written one.
///
/// `make -j` runs many `incan` processes against one cache, and every one of them computes this digest on a miss.
/// A plain write can be read mid-flight, and a truncated digest is still a well-formed cache key — it would
/// partition the store under a value no compiler will ever produce again. Writing beside the target and renaming
/// makes the file appear whole or not at all; two processes racing write byte-identical content, so whichever
/// rename lands last is correct either way.
///
/// Every failure here is ignored deliberately. This is a cache: an unwritable directory, a full disk or a losing
/// race costs the next command a recomputation, which the caller already handles.
fn write_effect_digest_memo(cached_path: &Path, digest: &str) {
    let Some(parent) = cached_path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Some(name) = cached_path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let staged = parent.join(format!(".{name}.{}.staged", std::process::id()));
    if fs::write(&staged, digest).is_ok() && fs::rename(&staged, cached_path).is_err() {
        let _ = fs::remove_file(&staged);
    }
}

/// Resolve where effect digests are memoized, or `None` when this environment has nowhere durable to put them.
///
/// A test harness that redirects the provider store gets its memo redirected with it, and a unit test gets a
/// target-directory memo of its own, so neither ever reads or writes the developer's own cache.
fn sdk_provider_effect_digest_cache_root() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/incan_test_effect_digest"));
    }
    if let Some(store) = env::var_os(INTERNAL_SDK_PROVIDER_STORE_ENV).filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(store).join(".effect-digest-v1"));
    }
    env::var_os("INCAN_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .or_else(|| env::var_os("USERPROFILE"))
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".incan"))
        })
        .map(|root| root.join("cache").join("effect-digest-v1"))
}

/// Stamp the compiler executable computing the effect digest: its path, length and modification time.
///
/// This is the same observed-stamp shape the `rustc -vV` probe and the artifact digest memo use. A rebuilt
/// compiler has a new length or a new mtime, so the stamp moves with it; identical checkouts on two machines
/// produce different stamps and different memo entries, which costs each machine one digest and never changes
/// the digest's value. `None` means the executable cannot be observed, and the caller then does not memoize.
fn running_compiler_stamp() -> Option<String> {
    let executable = env::current_exe().ok()?;
    let metadata = fs::metadata(&executable).ok()?;
    let modified = metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(format!(
        "{}:{}:{}.{:09}",
        executable.display(),
        metadata.len(),
        modified.as_secs(),
        modified.subsec_nanos()
    ))
}

/// Hash the bytes of every root the effect digest reads, folded with the computing compiler's stamp, as the memo
/// key for its result.
///
/// Roots are folded in their declared order under their declared labels so two checkouts with the same content
/// agree, and a root that is absent is recorded as absent rather than skipped — a missing tree is a different
/// compiler, not the same one. The compiler stamp is folded last, so a rebuilt compiler misses every entry the
/// previous one wrote.
fn sdk_provider_effect_input_key(checkout_root: &Path, compiler_stamp: &str) -> ProviderResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"incan-effect-inputs-v2\0");
    let stdlib_root = checkout_root.join(COMPILER_STDLIB_ROOT);
    hasher.update(b"stdlib\0");
    hash_sdk_provider_source_tree(&stdlib_root, &stdlib_root, &mut hasher)?;
    for (label, relative) in COMPILER_RUST_EFFECT_ROOTS {
        let root = checkout_root.join(relative);
        hasher.update(label.as_bytes());
        hasher.update([0]);
        if root.is_dir() {
            hash_sdk_provider_source_tree(&root, &root, &mut hasher)?;
        } else {
            hasher.update(b"absent\0");
        }
    }
    hasher.update(b"compiler\0");
    hasher.update(compiler_stamp.as_bytes());
    hasher.update([0]);
    Ok(hex::encode(hasher.finalize()))
}

/// Return the compiler-owned SDK provider identity for one source checkout.
///
/// This is intentionally exposed only to repository automation after it has built the matching CLI. The cache key
/// must follow the same source closure as provider publication; hashing development executable bytes would make
/// identical source checkouts miss after unrelated test builds.
pub(crate) fn sdk_provider_store_identity_for_compiler_root(compiler_root: &Path) -> ProviderResult<String> {
    let stdlib_root = fs::canonicalize(compiler_root.join("crates/incan_stdlib/stdlib")).map_err(|error| {
        ProviderError::failure(format!(
            "failed to canonicalize built-in stdlib source directory below {}: {error}",
            compiler_root.display()
        ))
    })?;
    let executable = env::current_exe()
        .map_err(|error| ProviderError::failure(format!("failed to resolve current incan executable: {error}")))?;
    let executable = sdk_provider_builder_executable(None, executable)?;
    let workspace_lock = sdk_provider_workspace_lock(&stdlib_root);
    let distribution_profile = env::var(INTERNAL_SDK_DISTRIBUTION_PROFILE_ENV)
        .ok()
        .filter(|profile| !profile.is_empty())
        .unwrap_or_else(|| "full".to_string());
    sdk_provider_store_identity(
        &stdlib_root,
        &executable,
        workspace_lock.as_deref(),
        &distribution_profile,
    )
}

/// Resolve the source checkout that owns a discovered SDK tree, if this is a development layout.
fn sdk_provider_compiler_checkout_root(stdlib_root: &Path) -> Option<PathBuf> {
    let explicit = env::var_os("INCAN_SOURCE_ROOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    explicit
        .into_iter()
        .chain(stdlib_root.ancestors().map(Path::to_path_buf))
        .find(|candidate| is_sdk_provider_compiler_checkout(candidate, stdlib_root))
}

/// Check the exact source layout before treating a directory tree as compiler authority.
fn is_sdk_provider_compiler_checkout(candidate: &Path, stdlib_root: &Path) -> bool {
    if !candidate.join("Cargo.toml").is_file() || !candidate.join("src").is_dir() {
        return false;
    }
    let expected_stdlib_root = candidate.join("crates/incan_stdlib/stdlib");
    fs::canonicalize(&expected_stdlib_root).ok() == fs::canonicalize(stdlib_root).ok()
}

/// Hash the running compiler once per process with SHA-256, the hash family every other identity in the toolchain uses,
/// independent of its path.
fn sdk_provider_compiler_digest(executable: &Path) -> ProviderResult<[u8; 32]> {
    if let Some(digest) = SDK_PROVIDER_COMPILER_DIGESTS
        .lock()
        .map_err(|_| ProviderError::failure("failed to lock the compiler-content digest cache"))?
        .get(executable)
        .copied()
    {
        return Ok(digest);
    }

    let mut executable_file = fs::File::open(executable).map_err(|error| {
        ProviderError::failure(format!(
            "failed to read compiler executable {}: {error}",
            executable.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = executable_file.read(&mut buffer).map_err(|error| {
            ProviderError::failure(format!(
                "failed to read compiler executable {}: {error}",
                executable.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    SDK_PROVIDER_COMPILER_DIGESTS
        .lock()
        .map_err(|_| ProviderError::failure("failed to lock the compiler-content digest cache"))?
        .insert(executable.to_path_buf(), digest);
    Ok(digest)
}

/// Select one user-shared development cache instead of duplicating identical provider artifacts in every checkout.
pub(crate) fn default_sdk_provider_store(
    stdlib_root: &Path,
    incan_home: Option<std::ffi::OsString>,
    user_home: Option<std::ffi::OsString>,
) -> PathBuf {
    incan_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            user_home
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".incan"))
        })
        .map(|root| root.join("cache").join("providers").join("sdk-v2"))
        .unwrap_or_else(|| stdlib_root.join("target").join("incan_sdk_components"))
}

/// Flush every staged artifact file and directory before atomic publication.
pub(crate) fn sync_sdk_provider_tree(path: &Path) -> ProviderResult<()> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| {
            ProviderError::failure(format!(
                "failed to read staged artifact directory {}: {error}",
                path.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            ProviderError::failure(format!(
                "failed to enumerate staged artifact directory {}: {error}",
                path.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let entry_path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            ProviderError::failure(format!(
                "failed to inspect staged artifact path {}: {error}",
                entry_path.display()
            ))
        })?;
        if file_type.is_dir() {
            sync_sdk_provider_tree(&entry_path)?;
        } else if file_type.is_file() {
            fs::File::open(&entry_path)
                .and_then(|file| file.sync_all())
                .map_err(|error| {
                    ProviderError::failure(format!(
                        "failed to synchronize staged artifact file {}: {error}",
                        entry_path.display()
                    ))
                })?;
        }
    }
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            ProviderError::failure(format!(
                "failed to synchronize staged artifact directory {}: {error}",
                path.display()
            ))
        })
}

/// Flush the artifact store after publishing a new immutable artifact directory.
pub(crate) fn sync_sdk_provider_store(store_root: &Path) -> ProviderResult<()> {
    fs::File::open(store_root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            ProviderError::failure(format!(
                "failed to synchronize artifact store {}: {error}",
                store_root.display()
            ))
        })
}

/// Allocate a unique private staging directory for one artifact identity.
pub(crate) fn staged_sdk_provider_root(store_root: &Path, identity: &str) -> ProviderResult<PathBuf> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| ProviderError::failure(format!("system clock predates Unix epoch: {error}")))?;
    Ok(store_root.join(format!(
        ".staging-{identity}-{}-{}",
        std::process::id(),
        elapsed.as_nanos()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_provider_builder_selects_the_real_cli_for_tests_and_utilities() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let cargo_cli = temp_dir.path().join("incan-cli");
        fs::write(&cargo_cli, "test binary")?;
        assert_eq!(
            sdk_provider_builder_executable(Some(cargo_cli.clone()), PathBuf::from("/tmp/integration-test"),)?,
            cargo_cli
        );

        let target_dir = temp_dir.path().join("target/debug");
        fs::create_dir_all(&target_dir)?;
        let mut sibling_cli = target_dir.join("incan");
        sibling_cli.set_extension(std::env::consts::EXE_EXTENSION);
        fs::write(&sibling_cli, "cli binary")?;
        assert_eq!(
            sdk_provider_builder_executable(None, target_dir.join("generate_feature_inventory"))?,
            sibling_cli
        );

        let direct_cli = temp_dir.path().join("incan");
        fs::write(&direct_cli, "installed cli")?;
        assert_eq!(
            sdk_provider_builder_executable(Some(PathBuf::from("/tmp/stale-incan-cli")), direct_cli.clone(),)?,
            direct_cli
        );

        let deps_dir = target_dir.join("deps");
        fs::create_dir_all(&deps_dir)?;
        assert_eq!(
            sdk_provider_builder_executable(None, deps_dir.join("incan-abc123"))?,
            sibling_cli
        );
        Ok(())
    }

    #[test]
    fn sdk_provider_builder_rejects_a_utility_without_a_sibling_cli() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let utility = temp_dir.path().join("generate_feature_inventory");
        fs::write(&utility, "utility binary")?;

        let error = match sdk_provider_builder_executable(None, utility) {
            Err(error) => error,
            Ok(path) => return Err(format!("missing sibling CLI unexpectedly resolved to {}", path.display()).into()),
        };
        assert!(error.message.contains("requires the incan CLI executable"));
        Ok(())
    }

    #[test]
    fn sdk_provider_build_uses_enclosing_workspace_lock() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let workspace = tmp.path().join("workspace");
        let stdlib_root = workspace.join("crates/incan_stdlib/stdlib");
        let artifact_root = stdlib_root.join("target/lib");
        fs::create_dir_all(&stdlib_root)?;
        fs::write(workspace.join("Cargo.lock"), "workspace lock payload")?;

        let workspace_lock = sdk_provider_workspace_lock(&stdlib_root);
        let mut command = Command::new("incan");
        configure_sdk_provider_workspace_lock(&mut command, workspace_lock.as_deref());
        let selected = command
            .get_envs()
            .find(|(name, _)| *name == INTERNAL_CARGO_LOCK_PAYLOAD_PATH_ENV)
            .and_then(|(_, path)| path)
            .ok_or("SDK lock authority absent from child command")?;
        assert_eq!(fs::read_to_string(selected)?, "workspace lock payload");
        assert!(
            !artifact_root.exists(),
            "the child's output transaction owns lock materialization"
        );
        Ok(())
    }

    #[test]
    fn sdk_provider_store_identity_tracks_source_and_lock_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let stdlib_root = temp_dir.path().join("stdlib");
        fs::create_dir_all(stdlib_root.join("nested"))?;
        fs::write(stdlib_root.join("loaf.toml"), "[project]\nname = \"stdlib\"\n")?;
        fs::write(
            stdlib_root.join("nested").join("module.incn"),
            "pub def value() -> int:\n  return 1\n",
        )?;
        let workspace_lock = temp_dir.path().join("Cargo.lock");
        fs::write(&workspace_lock, "first lock closure")?;
        let executable = temp_dir.path().join("compiler-a");
        fs::write(&executable, "compiler payload")?;

        let initial = sdk_provider_store_identity(&stdlib_root, &executable, Some(&workspace_lock), "full")?;
        let relocated_executable = temp_dir.path().join("relocated").join("compiler-b");
        fs::create_dir_all(
            relocated_executable
                .parent()
                .ok_or("relocated compiler had no parent")?,
        )?;
        fs::copy(&executable, &relocated_executable)?;
        let relocated =
            sdk_provider_store_identity(&stdlib_root, &relocated_executable, Some(&workspace_lock), "full")?;
        assert_eq!(
            initial, relocated,
            "identical compiler bytes must reuse provider artifacts across paths"
        );
        let changed_executable = temp_dir.path().join("compiler-changed");
        fs::write(&changed_executable, "different compiler payload")?;
        let compiler_changed =
            sdk_provider_store_identity(&stdlib_root, &changed_executable, Some(&workspace_lock), "full")?;
        assert_ne!(
            initial, compiler_changed,
            "changing compiler bytes must invalidate provider artifacts"
        );
        fs::write(
            stdlib_root.join("nested").join("module.incn"),
            "pub def value() -> int:\n  return 2\n",
        )?;
        let source_changed = sdk_provider_store_identity(&stdlib_root, &executable, Some(&workspace_lock), "full")?;
        assert_ne!(
            initial, source_changed,
            "changing a stdlib source must invalidate its artifact identity"
        );

        fs::write(&workspace_lock, "second lock closure")?;
        let lock_changed = sdk_provider_store_identity(&stdlib_root, &executable, Some(&workspace_lock), "full")?;
        assert_ne!(
            source_changed, lock_changed,
            "changing the resolved Cargo closure must invalidate its artifact identity"
        );
        let minimal = sdk_provider_store_identity(&stdlib_root, &executable, Some(&workspace_lock), "minimal")?;
        assert_ne!(
            lock_changed, minimal,
            "distribution profiles must not share provider-store identities"
        );
        Ok(())
    }

    /// The memo key over this repository's own effect roots stays cheap enough to pay on every command.
    ///
    /// This is the cost the fix actually charges. The digest behind it costs seconds and is paid once per distinct
    /// input content; what a warm command pays is this byte hash, and it reads a small named set of roots rather
    /// than the whole `src/`, `crates/` and `tests/` tree v3 walked.
    #[test]
    fn the_effect_memo_key_over_this_checkout_stays_cheap() -> Result<(), Box<dyn std::error::Error>> {
        let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let compiler_stamp = running_compiler_stamp().ok_or("the test executable must be stampable")?;
        let started = std::time::Instant::now();
        let key = sdk_provider_effect_input_key(&checkout, &compiler_stamp)?;
        let elapsed = started.elapsed();
        println!("EFFECT-MEMO-KEY {key} in {} ms", elapsed.as_millis());
        assert_eq!(key.len(), 64, "the memo key is a hex sha256");
        assert_eq!(
            key,
            sdk_provider_effect_input_key(&checkout, &compiler_stamp)?,
            "the memo key must not depend on directory iteration order"
        );
        assert_ne!(
            key,
            sdk_provider_effect_input_key(&checkout, "another-compiler-build")?,
            "a rebuilt compiler must miss the memo the previous build wrote"
        );
        // Generous enough to survive a loaded machine and a cold page cache, tight enough to fail if the key ever
        // starts walking the whole checkout again.
        assert!(
            elapsed.as_secs() < 5,
            "the memo key took {} ms, which is no longer a per-command cost",
            elapsed.as_millis()
        );
        Ok(())
    }

    /// The store identity keys on what the compiler produces, not on what the compiler is made of (#1495).
    ///
    /// Each assertion below is one row of that contract. The two that changed in v4 are the ones that used to cost
    /// seventeen minutes: an edit to a compiler subsystem no component's compilation can reach now reuses the
    /// store, and a comment added to a standard-library source does too.
    #[test]
    fn sdk_provider_store_identity_keys_on_what_the_compiler_produces() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let checkout = temp_dir.path().join("checkout");
        let stdlib_root = checkout.join("crates/incan_stdlib/stdlib");
        fs::create_dir_all(checkout.join("src"))?;
        fs::create_dir_all(stdlib_root.join("components"))?;
        fs::write(checkout.join("Cargo.toml"), "[workspace]\nmembers = []\n")?;
        fs::write(checkout.join("Cargo.lock"), "first lock closure")?;
        fs::write(checkout.join("src/compiler.rs"), "pub fn compile() {}\n")?;
        fs::write(
            stdlib_root.join("components/core.incn"),
            "pub def core() -> int:\n  return 1\n",
        )?;
        let executable = checkout.join("target/debug/incan");
        fs::create_dir_all(executable.parent().ok_or("compiler executable had no parent")?)?;
        fs::write(&executable, "first development compiler bytes")?;

        let initial =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        let rebuilt_executable = checkout.join("target/rebuilt/incan");
        fs::create_dir_all(
            rebuilt_executable
                .parent()
                .ok_or("rebuilt compiler executable had no parent")?,
        )?;
        fs::write(&rebuilt_executable, "rebuilt development compiler bytes")?;
        let rebuilt_executable = sdk_provider_store_identity(
            &stdlib_root,
            &rebuilt_executable,
            Some(&checkout.join("Cargo.lock")),
            "full",
        )?;
        assert_eq!(
            initial, rebuilt_executable,
            "a rebuilt development executable with unchanged compiler source must reuse SDK providers"
        );

        fs::create_dir_all(checkout.join("tests"))?;
        fs::write(checkout.join("tests/only_test.rs"), "#[test]\nfn regression() {}\n")?;
        let changed_test =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        assert_eq!(
            initial, changed_test,
            "test-only source must not republish SDK providers"
        );

        let workflow_dir = checkout.join(".github/workflows");
        fs::create_dir_all(&workflow_dir)?;
        fs::write(workflow_dir.join("ci.yml"), "name: original CI\n")?;
        let with_workflow =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        assert_eq!(
            initial, with_workflow,
            "CI configuration does not change SDK compilation inputs"
        );
        fs::write(workflow_dir.join("ci.yml"), "name: reordered CI\n")?;
        fs::rename(workflow_dir.join("ci.yml"), workflow_dir.join("renamed.yml"))?;
        let changed_workflow =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        assert_eq!(
            initial, changed_workflow,
            "CI edits and administrative path changes must reuse SDK providers"
        );

        // The seventeen minutes. `src/compiler.rs` stands for every subsystem outside the effect roots — the
        // language server, an inspection module, a CLI command — none of which can change what a component
        // contains, and all of which rebuilt all ten components through v3.
        fs::write(
            checkout.join("src/compiler.rs"),
            "pub fn compile() { let changed = true; }\n",
        )?;
        let unreachable_subsystem =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        assert_eq!(
            initial, unreachable_subsystem,
            "an edit no component's compilation can reach must reuse the provider store"
        );

        // A comment cannot change what the compiler emits, so it cannot change the store. This is the property the
        // byte hash could not express at any granularity, and the reason the key is a digest of meaning.
        fs::write(
            stdlib_root.join("components/core.incn"),
            "# a comment the compiler cannot emit\npub def core() -> int:\n  return 1\n",
        )?;
        let commented_stdlib =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        assert_eq!(
            initial, commented_stdlib,
            "a comment in a standard-library source must reuse the provider store"
        );

        fs::write(
            stdlib_root.join("components/core.incn"),
            "pub def core() -> int:\n  return 2\n",
        )?;
        let changed_stdlib =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        assert_ne!(
            initial, changed_stdlib,
            "a changed standard-library body is a changed component"
        );

        // The transitional half. Lowering and emission change generated Rust without moving any HIR, so their
        // source is folded until direct-HIR removes the need. Emission lives in its own crate now; an edit written
        // under the old `src/backend` path would prove nothing.
        let backend = checkout.join("loaves/compiler/incan_emit/src/emit");
        fs::create_dir_all(&backend)?;
        fs::write(backend.join("decls.rs"), "pub fn emit() -> u8 { 1 }\n")?;
        let with_backend =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        fs::write(backend.join("decls.rs"), "pub fn emit() -> u8 { 2 }\n")?;
        let changed_backend =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        assert_ne!(
            with_backend, changed_backend,
            "an emission change alters generated Rust without moving any HIR"
        );

        // The Rust half is mandatory rather than an enhancement: every component links this runtime, so an
        // Incan-only key would report a hit for a change the consumer can observe.
        let runtime = checkout.join("crates/incan_stdlib/src");
        fs::create_dir_all(&runtime)?;
        fs::write(runtime.join("frozen.rs"), "pub fn limit() -> u8 { 1 }\n")?;
        let with_runtime =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        fs::write(runtime.join("frozen.rs"), "pub fn limit() -> u8 { 2 }\n")?;
        let changed_runtime =
            sdk_provider_store_identity(&stdlib_root, &executable, Some(&checkout.join("Cargo.lock")), "full")?;
        assert_ne!(
            with_runtime, changed_runtime,
            "every component links the Rust runtime, so a change to it must invalidate"
        );
        Ok(())
    }

    #[test]
    fn sdk_provider_store_defaults_to_the_shared_incan_cache() {
        let stdlib_root = Path::new("/workspace/stdlib");
        assert_eq!(
            default_sdk_provider_store(stdlib_root, Some("/opt/incan-home".into()), Some("/home/user".into())),
            Path::new("/opt/incan-home/cache/providers/sdk-v2")
        );
        assert_eq!(
            default_sdk_provider_store(stdlib_root, None, Some("/home/user".into())),
            Path::new("/home/user/.incan/cache/providers/sdk-v2")
        );
        assert_eq!(
            default_sdk_provider_store(stdlib_root, None, None),
            Path::new("/workspace/stdlib/target/incan_sdk_components")
        );
    }

    /// The store lock serializes, and a waiter is a waiter rather than a failure.
    ///
    /// This pins the behaviour the diagnostic sits on top of: a second acquirer blocks and then succeeds, instead
    /// of failing or taking the lock. Timing distinguishes blocking from erroring; it does not assert a duration,
    /// since the wait ends when the holder releases.
    ///
    /// It does **not** cover the message itself. Replacing the `try_lock` guard with `if false` leaves this test
    /// passing, because the outer `lock()` blocks either way — so the diagnostic is verified by reading it, not by
    /// this test. Covering it would mean routing one `eprintln!` through an injectable sink, which is more
    /// structure than a single diagnostic earns. Recorded so the next reader does not assume otherwise.
    #[test]
    fn a_held_store_lock_makes_the_next_acquirer_wait_rather_than_fail() -> Result<(), Box<dyn std::error::Error>> {
        let store = tempfile::tempdir()?;
        let store_root = store.path().to_path_buf();

        let held = acquire_sdk_provider_store_lock(&store_root)?;

        let waiting_root = store_root.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let acquired = acquire_sdk_provider_store_lock(&waiting_root);
            let _ = tx.send(acquired.is_ok());
        });

        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(250)).is_err(),
            "a second acquirer must wait while the first holds the lock, not take it"
        );

        drop(held);
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(10)),
            Ok(true),
            "releasing the lock must let the waiter through"
        );
        waiter
            .join()
            .map_err(|_| std::io::Error::other("lock waiter panicked"))?;
        Ok(())
    }

    /// An uncontended lock is the common path and must stay silent and immediate.
    #[test]
    fn an_uncontended_store_lock_is_acquired_without_waiting() -> Result<(), Box<dyn std::error::Error>> {
        let store = tempfile::tempdir()?;
        let first = acquire_sdk_provider_store_lock(store.path())?;
        drop(first);
        let second = acquire_sdk_provider_store_lock(store.path())?;
        drop(second);
        Ok(())
    }
}
