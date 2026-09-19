//! The Loaf registry as a registered compatible source (RFC 117), in the shape RFC 125 gives it.
//!
//! A registry is read through a static sparse index: one file per package name in the crates.io path scheme, one
//! JSON line per published version naming the source checksum and the manifest that describes it. This module reads
//! that shape from a local checkout, the interim transport the toolchain pins by commit; a signed HTTPS index later
//! changes only how the same files arrive. Nothing here executes package content, and a lookup is fail-closed: a
//! registry that describes a different source than the one being built is a refusal, never a silent miss.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::digest::digest_bytes;
use crate::manifest::{
    ManifestError, ProjectManifest, RustFactOut, RustFactRecord, RustFactSelection, is_sha256_identity,
};

/// A registered Loaf registry read from a local checkout.
#[derive(Debug, Clone)]
pub struct LoafRegistry {
    root: PathBuf,
}

/// The status the registry publishes for one bound fact: `harvested` until an attestation proves the Oven-baked
/// unit equivalent to the Cargo-built one, then `attested`.
pub const LOAF_REGISTRY_STATUS_HARVESTED: &str = "harvested";

/// One package version the registry describes, with its manifest verified against the source it names.
#[derive(Debug, Clone)]
pub struct LoafRegistryPackage {
    /// Package name as published.
    pub name: String,
    /// Exact published version.
    pub version: String,
    /// Canonical `sha256:` checksum of the published archive.
    pub checksum: String,
    /// The verified manifest.
    pub manifest: ProjectManifest,
    /// Directory holding the manifest and its committed generated inputs.
    pub manifest_root: PathBuf,
    /// The exact index line this package was selected from.
    pub index_line: String,
    /// The bindings the index line lists, each with the status the registry publishes for it.
    pub index_facts: Vec<LoafRegistryIndexFact>,
}

/// One binding as the sparse index line states it, beside the record the manifest carries for it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LoafRegistryIndexFact {
    pub toolchain: String,
    pub target: String,
    pub profile: String,
    #[serde(default)]
    pub features: Vec<String>,
    /// `harvested` or `attested`.
    pub status: String,
}

/// Why a registry lookup refused.
#[derive(Debug, thiserror::Error)]
pub enum LoafRegistryError {
    #[error("Loaf registry root {path} has no index directory")]
    NoIndex { path: PathBuf },
    #[error("could not read Loaf registry file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Loaf registry index {path} is invalid: {message}")]
    Index { path: PathBuf, message: String },
    #[error("Loaf registry manifest {path} is invalid: {source}")]
    Manifest {
        path: PathBuf,
        #[source]
        source: ManifestError,
    },
    #[error("Loaf registry describes `{name}` {version} differently from the selected source: {message}")]
    Mismatch {
        name: String,
        version: String,
        message: String,
    },
    /// The checkout is not at the commit the caller pinned, or its revision could not be read.
    #[error("Loaf registry checkout {path} is not at the pinned index commit: {message}")]
    Pin { path: PathBuf, message: String },
}

/// One index line, as `incan-pub build` renders it. Keys this reader does not consume are ignored.
#[derive(Debug, Deserialize)]
struct IndexEntry {
    name: String,
    vers: String,
    cksum: String,
    manifest: String,
    #[serde(default)]
    facts: Vec<LoafRegistryIndexFact>,
}

/// The crates.io sparse-index location for one package name.
pub fn sparse_index_path(name: &str) -> PathBuf {
    let lower = name.to_ascii_lowercase();
    let mut path = PathBuf::from("index");
    match lower.len() {
        1 => path.push("1"),
        2 => path.push("2"),
        3 => {
            path.push("3");
            path.push(&lower[..1]);
        }
        _ => {
            path.push(&lower[..2]);
            path.push(&lower[2..4]);
        }
    }
    path.push(&lower);
    path
}

/// Canonicalize a checksum given either bare or `sha256:`-prefixed lowercase hex.
pub fn canonical_checksum(checksum: &str) -> Option<String> {
    let bare = checksum.strip_prefix("sha256:").unwrap_or(checksum);
    let canonical = format!("sha256:{bare}");
    is_sha256_identity(&canonical).then_some(canonical)
}

impl LoafRegistry {
    /// Open a registry checkout; the index directory must exist.
    pub fn open(root: &Path) -> Result<Self, LoafRegistryError> {
        if !root.join("index").is_dir() {
            return Err(LoafRegistryError::NoIndex {
                path: root.to_path_buf(),
            });
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// Open a registry checkout only if its git `HEAD` resolves to `index_commit`.
    ///
    /// The toolchain manifest pins the `index` commit a release was settled against; a consumer with the same pin
    /// resolves the same records. The revision is read from the checkout's own `.git` metadata (a directory, or the
    /// `gitdir:` file a worktree carries) without running git, so the check is the same on a host without it.
    pub fn open_pinned(root: &Path, index_commit: &str) -> Result<Self, LoafRegistryError> {
        let expected = index_commit.trim().to_ascii_lowercase();
        if !is_commit_id(&expected) {
            return Err(LoafRegistryError::Pin {
                path: root.to_path_buf(),
                message: format!("`{index_commit}` is not a commit id"),
            });
        }
        let actual = checkout_head_commit(root)?;
        if actual != expected {
            return Err(LoafRegistryError::Pin {
                path: root.to_path_buf(),
                message: format!("HEAD is {actual}, the pin is {expected}"),
            });
        }
        Self::open(root)
    }

    /// The checkout root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Find the manifest the registry publishes for one exact package source.
    ///
    /// `Ok(None)` means the registry does not describe this name or version. An index line for the version whose
    /// checksum differs from `checksum` is a refusal: the registry describes a different source, and applying its
    /// facts to this one would be a guess.
    pub fn package(
        &self,
        name: &str,
        version: &str,
        checksum: &str,
    ) -> Result<Option<LoafRegistryPackage>, LoafRegistryError> {
        let checksum = canonical_checksum(checksum).ok_or_else(|| LoafRegistryError::Mismatch {
            name: name.to_string(),
            version: version.to_string(),
            message: "the selected source checksum is not a sha256 identity".to_string(),
        })?;
        let index_path = self.root.join(sparse_index_path(name));
        let index = match fs::read_to_string(&index_path) {
            Ok(index) => index,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(LoafRegistryError::Io {
                    path: index_path,
                    source,
                });
            }
        };
        let mut selected = None;
        for line in index.lines().filter(|line| !line.trim().is_empty()) {
            let entry: IndexEntry = serde_json::from_str(line).map_err(|error| LoafRegistryError::Index {
                path: index_path.clone(),
                message: error.to_string(),
            })?;
            if entry.name != name {
                return Err(LoafRegistryError::Index {
                    path: index_path.clone(),
                    message: format!("line names `{}` in the file for `{name}`", entry.name),
                });
            }
            if entry.vers != version {
                continue;
            }
            if selected.replace((entry, line.to_string())).is_some() {
                return Err(LoafRegistryError::Index {
                    path: index_path.clone(),
                    message: format!("version {version} is listed more than once"),
                });
            }
        }
        let Some((entry, index_line)) = selected else {
            return Ok(None);
        };
        let mismatch = |message: String| LoafRegistryError::Mismatch {
            name: name.to_string(),
            version: version.to_string(),
            message,
        };
        if canonical_checksum(&entry.cksum).as_deref() != Some(checksum.as_str()) {
            return Err(mismatch("the index names a different source checksum".to_string()));
        }
        let manifest_relative = Path::new(&entry.manifest);
        if manifest_relative.is_absolute()
            || manifest_relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(LoafRegistryError::Index {
                path: index_path,
                message: format!("manifest path `{}` leaves the registry", entry.manifest),
            });
        }
        let manifest_path = self.root.join(manifest_relative);
        let text = fs::read_to_string(&manifest_path).map_err(|source| LoafRegistryError::Io {
            path: manifest_path.clone(),
            source,
        })?;
        let manifest =
            ProjectManifest::from_str(&text, &manifest_path).map_err(|source| LoafRegistryError::Manifest {
                path: manifest_path.clone(),
                source,
            })?;
        let project = manifest.project.as_ref();
        if project.and_then(|project| project.name.as_deref()) != Some(name)
            || project.and_then(|project| project.version.as_deref()) != Some(version)
        {
            return Err(mismatch("the manifest names another package or version".to_string()));
        }
        let source = manifest
            .source
            .as_ref()
            .ok_or_else(|| mismatch("the manifest declares no [source] publication binding".to_string()))?;
        if source.checksum != checksum {
            return Err(mismatch("the manifest binds a different source checksum".to_string()));
        }
        let manifest_root = manifest_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| mismatch("the manifest has no directory".to_string()))?;
        for record in &manifest.rust_facts {
            for out in &record.out {
                let committed = manifest_root.join(&out.path);
                let bytes = fs::read(&committed).map_err(|source| LoafRegistryError::Io {
                    path: committed.clone(),
                    source,
                })?;
                if digest_bytes(&bytes) != out.digest {
                    return Err(mismatch(format!(
                        "committed generated input `{}` does not match its declared digest",
                        out.name
                    )));
                }
            }
        }
        Ok(Some(LoafRegistryPackage {
            name: name.to_string(),
            version: version.to_string(),
            checksum,
            manifest,
            manifest_root,
            index_line,
            index_facts: entry.facts,
        }))
    }
}

/// The `HEAD` commit of the nearest git checkout enclosing `path`, or `None` when no ancestor is one.
///
/// A harvest notes the checkout it ran from; a path outside any checkout notes nothing rather than guessing.
pub fn enclosing_checkout_head_commit(path: &Path) -> Option<String> {
    path.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .and_then(|root| checkout_head_commit(root).ok())
}

/// Whether `value` is a lowercase hex git object id (SHA-1 or SHA-256 repositories).
fn is_commit_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Resolve the commit a checkout's `HEAD` names by reading its git metadata, never by running git.
///
/// `.git` is a directory in an ordinary clone and a `gitdir: <path>` file in a worktree; a worktree keeps `HEAD`
/// in its own directory and shares refs through the `commondir` it names. A symbolic `HEAD` is followed to its
/// loose ref, then to `packed-refs`; a detached `HEAD` is the commit itself. `root` must be the checkout root
/// itself; see [`enclosing_checkout_head_commit`] for a path somewhere inside one.
pub fn checkout_head_commit(root: &Path) -> Result<String, LoafRegistryError> {
    let pin_error = |message: String| LoafRegistryError::Pin {
        path: root.to_path_buf(),
        message,
    };
    let read = |path: &Path| -> Result<String, LoafRegistryError> {
        fs::read_to_string(path).map_err(|source| LoafRegistryError::Io {
            path: path.to_path_buf(),
            source,
        })
    };
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else if dot_git.is_file() {
        let pointer = read(&dot_git)?;
        let target = pointer
            .trim()
            .strip_prefix("gitdir:")
            .map(str::trim)
            .filter(|target| !target.is_empty())
            .ok_or_else(|| pin_error("`.git` names no gitdir".to_string()))?;
        let target = Path::new(target);
        if target.is_absolute() {
            target.to_path_buf()
        } else {
            root.join(target)
        }
    } else {
        return Err(pin_error(
            "no `.git` directory or file; the registry must be a git checkout".to_string(),
        ));
    };
    let head = read(&git_dir.join("HEAD"))?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref:").map(str::trim) else {
        let detached = head.to_ascii_lowercase();
        return if is_commit_id(&detached) {
            Ok(detached)
        } else {
            Err(pin_error(format!("HEAD `{head}` is neither a ref nor a commit")))
        };
    };
    // ---- A symbolic HEAD: loose ref in this git dir, then the shared common dir, then packed-refs ----
    let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => {
            let common = Path::new(common.trim());
            if common.is_absolute() {
                common.to_path_buf()
            } else {
                git_dir.join(common)
            }
        }
        Err(_) => git_dir.clone(),
    };
    for candidate in [git_dir.join(reference), common_dir.join(reference)] {
        if candidate.is_file() {
            let commit = read(&candidate)?.trim().to_ascii_lowercase();
            if is_commit_id(&commit) {
                return Ok(commit);
            }
            return Err(pin_error(format!("ref `{reference}` holds `{commit}`, not a commit")));
        }
    }
    let packed = common_dir.join("packed-refs");
    if packed.is_file() {
        for line in read(&packed)?.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with('^') {
                continue;
            }
            if let Some((commit, name)) = line.split_once(' ')
                && name.trim() == reference
            {
                let commit = commit.trim().to_ascii_lowercase();
                if is_commit_id(&commit) {
                    return Ok(commit);
                }
            }
        }
    }
    Err(pin_error(format!(
        "HEAD names `{reference}`, which resolves to no commit"
    )))
}

impl LoafRegistryPackage {
    /// The record whose binding equals the selection exactly, if the package is adopted for it.
    pub fn fact_record(&self, selection: &RustFactSelection) -> Option<&RustFactRecord> {
        self.manifest.rust_facts.iter().find(|record| record.binds(selection))
    }

    /// The status the index line publishes for the binding equal to `selection`.
    ///
    /// A line that lists the binding says `harvested` or `attested`; a line that does not list it has attested
    /// nothing about it, which is `harvested`, the status every admitted fact starts with.
    pub fn binding_status(&self, selection: &RustFactSelection) -> String {
        let mut features = selection.features.clone();
        features.sort();
        features.dedup();
        self.index_facts
            .iter()
            .find(|fact| {
                let mut listed = fact.features.clone();
                listed.sort();
                listed.dedup();
                fact.toolchain == selection.toolchain
                    && fact.target == selection.target
                    && fact.profile == selection.profile
                    && listed == features
            })
            .map(|fact| fact.status.clone())
            .unwrap_or_else(|| LOAF_REGISTRY_STATUS_HARVESTED.to_string())
    }

    /// The committed file one `out` entry names.
    pub fn committed_out(&self, out: &RustFactOut) -> PathBuf {
        self.manifest_root.join(&out.path)
    }

    /// Content identity of the exact index line this package was selected from, for generation evidence.
    pub fn index_line_digest(&self) -> String {
        digest_bytes(self.index_line.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const CHECKSUM: &str = "sha256:9a8e94ea7f378bd32cbbd37198a4a91436180c5bb472411e48b5ec2e2124ae9e";

    fn registry_fixture(root: &Path, private_rs: &[u8], manifest_checksum: &str) -> TestResult {
        let manifest_dir = root.join("crates-io/serde/1.0.228");
        fs::create_dir_all(manifest_dir.join("out"))?;
        fs::write(manifest_dir.join("out/private.rs"), private_rs)?;
        let digest = digest_bytes(b"#[doc(hidden)]\npub mod __private228 {}\n");
        fs::write(
            manifest_dir.join("loaf.toml"),
            format!(
                "[project]\nname = \"serde\"\nversion = \"1.0.228\"\n\n[source]\nregistry = \"https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{manifest_checksum}\"\n\n[[rust.facts]]\ntoolchain = \"rustc 1.98.0 (88d9e12ae 2026-08-18)\"\ntarget = \"aarch64-apple-darwin\"\nprofile = \"release\"\nfeatures = [\"default\", \"std\"]\ncfg = [\"if_docsrs_then_no_serde_core\"]\nout = [{{ name = \"private.rs\", path = \"out/private.rs\", digest = \"{digest}\" }}]\n"
            ),
        )?;
        let index_dir = root.join("index/se/rd");
        fs::create_dir_all(&index_dir)?;
        fs::write(
            index_dir.join("serde"),
            format!(
                "{{\"adopter\":\"fixture\",\"assets\":0,\"cksum\":\"{CHECKSUM}\",\"facts\":[{{\"features\":[\"default\",\"std\"],\"profile\":\"release\",\"status\":\"attested\",\"target\":\"aarch64-apple-darwin\",\"toolchain\":\"rustc 1.98.0 (88d9e12ae 2026-08-18)\"}}],\"manifest\":\"crates-io/serde/1.0.228/loaf.toml\",\"name\":\"serde\",\"source\":\"crates-io\",\"vers\":\"1.0.228\",\"yanked\":false}}\n"
            ),
        )?;
        Ok(())
    }

    /// Lay out the git metadata of a checkout whose `HEAD` is `commit`, as a clone (`symbolic`) or detached.
    fn git_checkout(root: &Path, commit: &str, symbolic: bool, packed: bool) -> TestResult {
        let git_dir = root.join(".git");
        fs::create_dir_all(git_dir.join("refs/heads"))?;
        if symbolic {
            fs::write(git_dir.join("HEAD"), "ref: refs/heads/index\n")?;
            if packed {
                // An annotated tag packs as its own line plus a `^` peeled line; both must be passed over on the way
                // to the branch.
                let tag = "a".repeat(40);
                let peeled = "b".repeat(40);
                fs::write(
                    git_dir.join("packed-refs"),
                    format!(
                        "# pack-refs with: peeled fully-peeled sorted\n{tag} refs/tags/v1\n^{peeled}\n{commit} refs/heads/index\n"
                    ),
                )?;
            } else {
                fs::write(git_dir.join("refs/heads/index"), format!("{commit}\n"))?;
            }
        } else {
            fs::write(git_dir.join("HEAD"), format!("{commit}\n"))?;
        }
        Ok(())
    }

    /// Read the real registry checkout named by `INCAN_PUB_ROOT`, so the client is proven against the projection
    /// the registry actually renders rather than only against this module's fixtures. With `INCAN_PUB_COMMIT` set
    /// to the checkout's `HEAD`, the pinned open is proven against real git metadata (a worktree, in practice).
    #[test]
    #[ignore = "needs INCAN_PUB_ROOT pointing at an incan.pub checkout"]
    fn the_real_registry_projection_is_readable() -> TestResult {
        let root = std::env::var_os("INCAN_PUB_ROOT").ok_or("INCAN_PUB_ROOT is unset")?;
        if let Some(commit) = std::env::var_os("INCAN_PUB_COMMIT") {
            LoafRegistry::open_pinned(Path::new(&root), &commit.to_string_lossy())?;
            assert!(matches!(
                LoafRegistry::open_pinned(Path::new(&root), &"0".repeat(40)),
                Err(LoafRegistryError::Pin { .. })
            ));
        }
        let registry = LoafRegistry::open(Path::new(&root))?;
        let package = registry
            .package(
                "libm",
                "0.2.16",
                "b6d2cec3eae94f9f509c767b45932f1ada8350c4bdb85af2fcab4a3c14807981",
            )?
            .ok_or("the registry must describe libm 0.2.16")?;
        let release = package
            .fact_record(&RustFactSelection {
                toolchain: "rustc 1.98.0 (88d9e12ae 2026-08-18)".to_string(),
                target: "aarch64-apple-darwin".to_string(),
                profile: "release".to_string(),
                features: vec!["default".to_string(), "arch".to_string()],
            })
            .ok_or("the release record must bind")?;
        assert_eq!(release.cfg, ["arch_enabled", "optimizations_enabled"]);
        let serde_core = registry
            .package(
                "serde_core",
                "1.0.228",
                "sha256:41d385c7d4ca58e59fc732af25c3983b67ac852c1a25000afe1175de458b67ad",
            )?
            .ok_or("the registry must describe serde_core 1.0.228")?;
        assert!(
            serde_core
                .manifest
                .rust_facts
                .iter()
                .all(|record| { record.out.iter().all(|out| serde_core.committed_out(out).is_file()) })
        );
        Ok(())
    }

    #[test]
    fn a_pinned_registry_opens_only_at_its_index_commit() -> TestResult {
        let commit = "8d40e1d".to_string() + &"0".repeat(33);
        let other = "f".repeat(40);
        for (symbolic, packed) in [(true, false), (true, true), (false, false)] {
            let root = tempfile::tempdir()?;
            registry_fixture(root.path(), b"#[doc(hidden)]\npub mod __private228 {}\n", CHECKSUM)?;
            git_checkout(root.path(), &commit, symbolic, packed)?;
            assert!(LoafRegistry::open_pinned(root.path(), &commit).is_ok());
            assert!(
                LoafRegistry::open_pinned(root.path(), &commit.to_ascii_uppercase()).is_ok(),
                "a pin is compared as an object id, not as text"
            );
            assert!(matches!(
                LoafRegistry::open_pinned(root.path(), &other),
                Err(LoafRegistryError::Pin { .. })
            ));
            assert!(matches!(
                LoafRegistry::open_pinned(root.path(), "not-a-commit"),
                Err(LoafRegistryError::Pin { .. })
            ));
        }
        // A worktree: `.git` is a file naming the worktree's git dir, whose refs live in the common dir.
        let main = tempfile::tempdir()?;
        git_checkout(main.path(), &commit, true, false)?;
        let worktree_git = main.path().join(".git/worktrees/index");
        fs::create_dir_all(&worktree_git)?;
        fs::write(worktree_git.join("HEAD"), "ref: refs/heads/index\n")?;
        fs::write(worktree_git.join("commondir"), "../..\n")?;
        let worktree = tempfile::tempdir()?;
        registry_fixture(worktree.path(), b"#[doc(hidden)]\npub mod __private228 {}\n", CHECKSUM)?;
        fs::write(
            worktree.path().join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )?;
        assert!(LoafRegistry::open_pinned(worktree.path(), &commit).is_ok());
        assert!(matches!(
            LoafRegistry::open_pinned(worktree.path(), &other),
            Err(LoafRegistryError::Pin { .. })
        ));
        // The nearest enclosing checkout is found from a path inside it, and nothing is found outside one.
        let inside = main.path().join("crates-io/serde/1.0.228");
        fs::create_dir_all(&inside)?;
        assert_eq!(
            enclosing_checkout_head_commit(&inside).as_deref(),
            Some(commit.as_str())
        );
        assert_eq!(enclosing_checkout_head_commit(Path::new("/")), None);
        // Not a checkout at all.
        let plain = tempfile::tempdir()?;
        registry_fixture(plain.path(), b"#[doc(hidden)]\npub mod __private228 {}\n", CHECKSUM)?;
        assert!(matches!(
            LoafRegistry::open_pinned(plain.path(), &commit),
            Err(LoafRegistryError::Pin { .. })
        ));
        Ok(())
    }

    #[test]
    fn sparse_index_paths_follow_the_crates_io_scheme() {
        assert_eq!(sparse_index_path("a"), Path::new("index/1/a"));
        assert_eq!(sparse_index_path("io"), Path::new("index/2/io"));
        assert_eq!(sparse_index_path("hex"), Path::new("index/3/h/hex"));
        assert_eq!(sparse_index_path("serde"), Path::new("index/se/rd/serde"));
        assert_eq!(sparse_index_path("Proc-Macro2"), Path::new("index/pr/oc/proc-macro2"));
    }

    #[test]
    fn a_registry_lookup_selects_verifies_and_binds_one_package() -> TestResult {
        let root = tempfile::tempdir()?;
        registry_fixture(root.path(), b"#[doc(hidden)]\npub mod __private228 {}\n", CHECKSUM)?;
        let registry = LoafRegistry::open(root.path())?;
        let package = registry
            .package("serde", "1.0.228", CHECKSUM.trim_start_matches("sha256:"))?
            .ok_or("serde must be described")?;
        assert_eq!(package.checksum, CHECKSUM, "a bare checksum is canonicalized");
        let selection = RustFactSelection {
            toolchain: "rustc 1.98.0 (88d9e12ae 2026-08-18)".to_string(),
            target: "aarch64-apple-darwin".to_string(),
            profile: "release".to_string(),
            features: vec!["std".to_string(), "default".to_string()],
        };
        let record = package.fact_record(&selection).ok_or("the release record must bind")?;
        assert_eq!(record.cfg, ["if_docsrs_then_no_serde_core"]);
        assert_eq!(
            package.binding_status(&selection),
            "attested",
            "the index line's status for the exact binding"
        );
        assert_eq!(
            package.binding_status(&RustFactSelection {
                profile: "debug".to_string(),
                ..selection.clone()
            }),
            LOAF_REGISTRY_STATUS_HARVESTED,
            "a binding the line does not list has been attested by nothing"
        );
        assert!(package.committed_out(&record.out[0]).is_file());
        assert!(
            package
                .fact_record(&RustFactSelection {
                    profile: "debug".to_string(),
                    ..selection
                })
                .is_none()
        );
        assert!(registry.package("serde", "1.0.227", CHECKSUM)?.is_none());
        assert!(registry.package("absent", "1.0.0", CHECKSUM)?.is_none());
        assert!(is_sha256_identity(&package.index_line_digest()));
        Ok(())
    }

    #[test]
    fn a_registry_that_describes_another_source_refuses_rather_than_misses() -> TestResult {
        let root = tempfile::tempdir()?;
        registry_fixture(root.path(), b"#[doc(hidden)]\npub mod __private228 {}\n", CHECKSUM)?;
        let registry = LoafRegistry::open(root.path())?;
        let other = format!("sha256:{}", "f".repeat(64));
        assert!(matches!(
            registry.package("serde", "1.0.228", &other),
            Err(LoafRegistryError::Mismatch { .. })
        ));
        let tampered = tempfile::tempdir()?;
        registry_fixture(tampered.path(), b"// not the committed bytes\n", CHECKSUM)?;
        assert!(matches!(
            LoafRegistry::open(tampered.path())?.package("serde", "1.0.228", CHECKSUM),
            Err(LoafRegistryError::Mismatch { .. })
        ));
        let unbound = tempfile::tempdir()?;
        registry_fixture(unbound.path(), b"#[doc(hidden)]\npub mod __private228 {}\n", &other)?;
        assert!(matches!(
            LoafRegistry::open(unbound.path())?.package("serde", "1.0.228", CHECKSUM),
            Err(LoafRegistryError::Mismatch { .. })
        ));
        assert!(matches!(
            LoafRegistry::open(&root.path().join("nowhere")),
            Err(LoafRegistryError::NoIndex { .. })
        ));
        Ok(())
    }
}
