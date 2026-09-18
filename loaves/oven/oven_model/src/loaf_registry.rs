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
}

/// One index line, as `scripts/registry.py` in the registry writes it.
#[derive(Debug, Deserialize)]
struct IndexEntry {
    name: String,
    vers: String,
    cksum: String,
    manifest: String,
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
        }))
    }
}

impl LoafRegistryPackage {
    /// The record whose binding equals the selection exactly, if the package is adopted for it.
    pub fn fact_record(&self, selection: &RustFactSelection) -> Option<&RustFactRecord> {
        self.manifest.rust_facts.iter().find(|record| record.binds(selection))
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
                "{{\"cksum\":\"{CHECKSUM}\",\"manifest\":\"crates-io/serde/1.0.228/loaf.toml\",\"name\":\"serde\",\"source\":\"crates-io\",\"vers\":\"1.0.228\"}}\n"
            ),
        )?;
        Ok(())
    }

    /// Read the real registry checkout named by `INCAN_PUB_ROOT`, so the client is proven against the projection
    /// the registry actually renders rather than only against this module's fixtures.
    #[test]
    #[ignore = "needs INCAN_PUB_ROOT pointing at an incan.pub checkout"]
    fn the_real_registry_projection_is_readable() -> TestResult {
        let root = std::env::var_os("INCAN_PUB_ROOT").ok_or("INCAN_PUB_ROOT is unset")?;
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
