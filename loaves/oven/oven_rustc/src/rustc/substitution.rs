//! Base-artifact substitution for Cargo-driven bakes: deciding when a `rustc` invocation may be answered from the
//! store instead of compiled.
//!
//! A bounded compatibility bake still runs Cargo, and Cargo consults only its own target directory, so a fresh target
//! directory recompiles every registry unit the store already holds as an admitted artifact. This module is the pure
//! half of the answer. It reads one `rustc` invocation the way Cargo issues it, derives the unit identity that
//! invocation asks for, and matches that identity against a manifest of base units an admitted closure provides.
//! Nothing here spawns a process or touches the filesystem beyond what a caller hands in; the wrapper entry point
//! that copies artifacts and falls back to the real compiler is a separate, later step.
//!
//! The contract that keeps a substituted artifact valid is SVH consistency: an rlib is only usable by a consumer if
//! every unit it was compiled against is the unit the consumer links. That is enforced structurally rather than by
//! inspecting metadata — a unit is substituted only when every `--extern` it names resolves to a unit already
//! substituted in the same target directory. Cargo compiles in topological order, so by the time a unit is requested
//! each of its dependencies is either a path this wrapper wrote or a path it did not, and one lookup decides.
#![allow(
    dead_code,
    reason = "the wrapper entry point and the manifest producer land in the following increments; this pure core \
              is exercised by its own tests first"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Schema tag written into every base-unit manifest so a stale producer cannot be read as a current one.
pub const BASE_UNIT_MANIFEST_SCHEMA: &str = "incan-oven-base-units/1";

/// Environment variable naming the base-unit manifest a wrapped Cargo run may substitute from.
pub const BASE_UNITS_ENV: &str = "INCAN_OVEN_BASE_UNITS";

/// Crate types this increment may substitute. Host-side proc-macro and build-script units are deliberately
/// excluded: they are dynamic libraries with different failure modes and are never the expensive part of a closure.
const SUBSTITUTABLE_CRATE_TYPES: &[&str] = &["lib", "rlib"];

/// One `rustc` invocation as Cargo issues it, reduced to the facts that identify the unit it compiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustcUnitRequest {
    /// The single positional Rust source input passed to rustc.
    pub source: PathBuf,
    /// `--crate-name`, the identifier rustc will embed in the artifact.
    pub crate_name: String,
    /// `CARGO_PKG_NAME`, the package the unit belongs to.
    pub package: String,
    /// `CARGO_PKG_VERSION`, the exact package version.
    pub version: String,
    /// Every `--cfg feature="…"` value, sorted and deduplicated.
    pub features: BTreeSet<String>,
    /// `--edition`, when present.
    pub edition: Option<String>,
    /// Every `--crate-type` value, sorted.
    pub crate_types: BTreeSet<String>,
    /// `--target`, or `None` for a host compile.
    pub target: Option<String>,
    /// `-C opt-level=…`, when present.
    pub opt_level: Option<String>,
    /// `-C debuginfo=…`, when present.
    pub debuginfo: Option<String>,
    /// `-C panic=…`, when present.
    pub panic: Option<String>,
    /// `--out-dir`, where Cargo expects the artifacts.
    pub out_dir: PathBuf,
    /// `-C extra-filename=…`, the suffix Cargo expects on every artifact name.
    pub extra_filename: String,
    /// Every `--extern name=path`, keyed by the extern name. A bare `--extern name` maps to `None`.
    pub externs: BTreeMap<String, Option<PathBuf>>,
}

/// Why an invocation could not be read as a substitutable unit request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustcUnitRequestError {
    /// A required argument or environment value was absent.
    Missing(&'static str),
    /// An argument that must take a value ended the argument list.
    DanglingOption(String),
    /// A value was not valid UTF-8 where the request needs to read it.
    NotUtf8(String),
}

impl std::fmt::Display for RustcUnitRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(what) => write!(f, "rustc invocation carries no {what}"),
            Self::DanglingOption(option) => write!(f, "rustc option `{option}` has no value"),
            Self::NotUtf8(option) => write!(f, "rustc option `{option}` has a value that is not valid UTF-8"),
        }
    }
}

impl std::error::Error for RustcUnitRequestError {}

impl RustcUnitRequest {
    /// Read one rustc invocation. `args` are the arguments after the compiler path, exactly as Cargo passed them to
    /// the wrapper; `env` is the wrapper's environment, from which the package coordinates are taken.
    ///
    /// Both `--flag value` and `--flag=value` spellings are accepted for every option Cargo is known to use. Unknown
    /// options are ignored: they cannot make a substitution unsafe on their own, because the identity match below
    /// requires the codegen facts that matter to agree, and everything else is the compiler's business.
    pub fn parse<A, E>(args: &[A], env: E) -> Result<Self, RustcUnitRequestError>
    where
        A: AsRef<OsStr>,
        E: Fn(&str) -> Option<OsString>,
    {
        let mut crate_name = None;
        let mut source = None;
        let mut features = BTreeSet::new();
        let mut edition = None;
        let mut crate_types = BTreeSet::new();
        let mut target = None;
        let mut opt_level = None;
        let mut debuginfo = None;
        let mut panic = None;
        let mut out_dir = None;
        let mut extra_filename = None;
        let mut externs = BTreeMap::new();

        let mut index = 0;
        while index < args.len() {
            let raw = args[index].as_ref();
            index += 1;
            let Some(arg) = raw.to_str() else {
                continue;
            };
            let (option, inline_value) = match arg.split_once('=') {
                Some((option, value)) if option.starts_with("--") => (option, Some(value.to_string())),
                _ => (arg, None),
            };
            // Options that take a value: read it inline or from the next argument.
            let take_value = |index: &mut usize| -> Result<String, RustcUnitRequestError> {
                if let Some(value) = inline_value.clone() {
                    return Ok(value);
                }
                let Some(next) = args.get(*index) else {
                    return Err(RustcUnitRequestError::DanglingOption(option.to_string()));
                };
                *index += 1;
                next.as_ref()
                    .to_str()
                    .map(str::to_string)
                    .ok_or_else(|| RustcUnitRequestError::NotUtf8(option.to_string()))
            };
            match option {
                "--crate-name" => crate_name = Some(take_value(&mut index)?),
                "--edition" => edition = Some(take_value(&mut index)?),
                "--crate-type" => {
                    for kind in take_value(&mut index)?.split(',') {
                        crate_types.insert(kind.to_string());
                    }
                }
                "--target" => target = Some(take_value(&mut index)?),
                "--out-dir" => out_dir = Some(PathBuf::from(take_value(&mut index)?)),
                "--cfg" => {
                    let value = take_value(&mut index)?;
                    if let Some(feature) = feature_cfg(&value) {
                        features.insert(feature.to_string());
                    }
                }
                "--extern" => {
                    let value = take_value(&mut index)?;
                    match value.split_once('=') {
                        Some((name, path)) => externs.insert(name.to_string(), Some(PathBuf::from(path))),
                        None => externs.insert(value, None),
                    };
                }
                "-C" | "--codegen" => {
                    let value = take_value(&mut index)?;
                    let (key, setting) = value.split_once('=').unwrap_or((value.as_str(), ""));
                    match key {
                        "opt-level" => opt_level = Some(setting.to_string()),
                        "debuginfo" => debuginfo = Some(setting.to_string()),
                        "panic" => panic = Some(setting.to_string()),
                        "extra-filename" => extra_filename = Some(setting.to_string()),
                        _ => {}
                    }
                }
                _ => {
                    // `-Cfoo=bar` without a space is legal rustc spelling; Cargo does not use it, but read it anyway.
                    if let Some(value) = arg.strip_prefix("-C")
                        && !value.is_empty()
                    {
                        let (key, setting) = value.split_once('=').unwrap_or((value, ""));
                        match key {
                            "opt-level" => opt_level = Some(setting.to_string()),
                            "debuginfo" => debuginfo = Some(setting.to_string()),
                            "panic" => panic = Some(setting.to_string()),
                            "extra-filename" => extra_filename = Some(setting.to_string()),
                            _ => {}
                        }
                    } else if !arg.starts_with('-') && arg.ends_with(".rs") && source.is_none() {
                        source = Some(PathBuf::from(arg));
                    }
                }
            }
        }

        let env_string = |name: &'static str| -> Result<String, RustcUnitRequestError> {
            env(name)
                .ok_or(RustcUnitRequestError::Missing(name))?
                .into_string()
                .map_err(|_| RustcUnitRequestError::NotUtf8(name.to_string()))
        };

        let crate_name = crate_name.ok_or(RustcUnitRequestError::Missing("--crate-name"))?;
        let package = env_string("CARGO_PKG_NAME")?;
        let version = env_string("CARGO_PKG_VERSION")?;
        let source = source.ok_or(RustcUnitRequestError::Missing("Rust source input"))?;
        Ok(Self {
            source,
            crate_name,
            package,
            version,
            features,
            edition,
            crate_types,
            target,
            opt_level,
            debuginfo,
            panic,
            out_dir: out_dir.ok_or(RustcUnitRequestError::Missing("--out-dir"))?,
            extra_filename: extra_filename.ok_or(RustcUnitRequestError::Missing("-C extra-filename"))?,
            externs,
        })
    }

    /// Whether this unit is of a kind this increment substitutes at all.
    pub fn is_substitutable_kind(&self) -> bool {
        !self.crate_types.is_empty()
            && self
                .crate_types
                .iter()
                .all(|kind| SUBSTITUTABLE_CRATE_TYPES.contains(&kind.as_str()))
    }

    /// The artifact file name Cargo expects for one crate type, e.g. `libsyn-0123abcd.rlib`.
    pub fn expected_artifact_name(&self, crate_type: &str) -> Option<String> {
        match crate_type {
            "lib" | "rlib" => Some(format!("lib{}{}.rlib", self.crate_name, self.extra_filename)),
            _ => None,
        }
    }
}

/// Extract the feature name from a `--cfg feature="name"` value, in either quoting Cargo has used.
fn feature_cfg(value: &str) -> Option<&str> {
    let rest = value.strip_prefix("feature=")?;
    Some(rest.trim_matches('"'))
}

/// One admitted unit an injected bake may substitute for a matching compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseUnit {
    /// Registry package name.
    pub package: String,
    /// Exact package version.
    pub version: String,
    /// Crate identifier embedded in the artifact.
    pub crate_name: String,
    /// Features the artifact was compiled with. A request must ask for exactly this set: a subset would link an
    /// artifact carrying code the consumer did not ask for, and a superset would be missing code it did.
    pub features: BTreeSet<String>,
    /// Target triple the artifact was compiled for, or `None` for the publisher's host.
    pub target: Option<String>,
    /// `opt-level` the artifact was compiled with.
    pub opt_level: String,
    /// `debuginfo` the artifact was compiled with.
    pub debuginfo: String,
    /// `panic` strategy the artifact was compiled with, when one was set explicitly.
    pub panic: Option<String>,
    /// The immutable `.rlib` this unit provides.
    pub rlib: PathBuf,
    /// The `.rmeta` beside it, when the base recorded one.
    pub rmeta: Option<PathBuf>,
    /// Content digest of the rlib, re-verified before it is copied.
    pub rlib_digest: String,
}

/// Every base unit one admitted closure offers to a wrapped Cargo run, plus the compiler it was sealed by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseUnitManifest {
    /// Must equal [`BASE_UNIT_MANIFEST_SCHEMA`].
    pub schema: String,
    /// Exact `rustc -vV` identity of the compiler that produced every unit here. A wrapper serving a different
    /// compiler must not substitute anything, whatever else matches.
    pub rustc_identity: String,
    /// The units, in no particular order.
    pub units: Vec<BaseUnit>,
}

/// Why a request was not answered from the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubstitutionMiss {
    /// The manifest was written by a different schema or for a different compiler.
    IncompatibleManifest,
    /// The unit's crate type is not one this increment substitutes.
    CrateType,
    /// No base unit has this package, version, crate name, and feature set.
    NoMatchingUnit,
    /// A base unit matched on identity but was compiled for a different target or with different codegen settings.
    CodegenMismatch,
    /// One of the request's dependencies was compiled fresh in this target directory, so a base artifact built
    /// against the base's own dependency would not be the unit the consumer links.
    FreshDependency(String),
}

/// Paths this wrapper has already written into a target directory, keyed by the exact artifact path Cargo will
/// hand to consumers as `--extern`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstitutionLedger {
    /// Artifact path → the base unit's rlib digest that was placed there.
    pub placed: BTreeMap<PathBuf, String>,
}

impl SubstitutionLedger {
    /// Whether an extern path is one this wrapper placed from the base.
    pub fn placed_here(&self, path: &Path) -> bool {
        self.placed.contains_key(path)
    }
}

/// Decide whether one request may be answered from the base, and by which unit.
///
/// Every check is a reason to refuse, never a reason to guess: an unmatched request is compiled by the real
/// compiler, which is always correct and merely slower.
pub fn match_unit<'m>(
    request: &RustcUnitRequest,
    manifest: &'m BaseUnitManifest,
    rustc_identity: &str,
    ledger: &SubstitutionLedger,
) -> Result<&'m BaseUnit, SubstitutionMiss> {
    if manifest.schema != BASE_UNIT_MANIFEST_SCHEMA || manifest.rustc_identity != rustc_identity {
        return Err(SubstitutionMiss::IncompatibleManifest);
    }
    if !request.is_substitutable_kind() {
        return Err(SubstitutionMiss::CrateType);
    }
    let candidate = manifest
        .units
        .iter()
        .find(|unit| {
            unit.package == request.package
                && unit.version == request.version
                && unit.crate_name == request.crate_name
                && unit.features == request.features
        })
        .ok_or(SubstitutionMiss::NoMatchingUnit)?;
    let codegen_matches = candidate.target == request.target
        && request.opt_level.as_deref() == Some(candidate.opt_level.as_str())
        && request.debuginfo.as_deref() == Some(candidate.debuginfo.as_str())
        && request.panic == candidate.panic;
    if !codegen_matches {
        return Err(SubstitutionMiss::CodegenMismatch);
    }
    for (name, path) in &request.externs {
        match path {
            // A bare `--extern name` resolves from the sysroot or `-L` search paths; the standard library is the
            // only such case Cargo produces for registry units, and it is the same for the base and the request.
            None => {}
            Some(path) if ledger.placed_here(path) => {}
            Some(_) => return Err(SubstitutionMiss::FreshDependency(name.clone())),
        }
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn env_with<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    fn cargo_style_argv() -> Vec<String> {
        [
            "--crate-name",
            "syn",
            "--edition=2021",
            "/registry/syn-2.0.117/src/lib.rs",
            "--error-format=json",
            "--crate-type",
            "lib",
            "--emit=dep-info,metadata,link",
            "-C",
            "embed-bitcode=no",
            "-C",
            "debuginfo=2",
            "--cfg",
            "feature=\"clone-impls\"",
            "--cfg",
            "feature=\"derive\"",
            "--cfg",
            "feature=\"proc-macro\"",
            "-C",
            "metadata=0123456789abcdef",
            "-C",
            "extra-filename=-0123456789abcdef",
            "--out-dir",
            "/target/debug/deps",
            "-C",
            "opt-level=0",
            "-L",
            "dependency=/target/debug/deps",
            "--extern",
            "proc_macro2=/target/debug/deps/libproc_macro2-aaaa.rlib",
            "--extern",
            "quote=/target/debug/deps/libquote-bbbb.rlib",
            "--extern",
            "unicode_ident=/target/debug/deps/libunicode_ident-cccc.rlib",
            "--cap-lints",
            "allow",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn syn_request() -> Result<RustcUnitRequest, RustcUnitRequestError> {
        RustcUnitRequest::parse(
            &cargo_style_argv(),
            env_with(&[("CARGO_PKG_NAME", "syn"), ("CARGO_PKG_VERSION", "2.0.117")]),
        )
    }

    fn syn_base_unit() -> BaseUnit {
        BaseUnit {
            package: "syn".to_string(),
            version: "2.0.117".to_string(),
            crate_name: "syn".to_string(),
            features: ["clone-impls", "derive", "proc-macro"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            target: None,
            opt_level: "0".to_string(),
            debuginfo: "2".to_string(),
            panic: None,
            rlib: PathBuf::from("/store/entries/sha256-abc/artifacts/build/deps/libsyn-ffff.rlib"),
            rmeta: None,
            rlib_digest: "sha256:abc".to_string(),
        }
    }

    fn manifest_with(units: Vec<BaseUnit>) -> BaseUnitManifest {
        BaseUnitManifest {
            schema: BASE_UNIT_MANIFEST_SCHEMA.to_string(),
            rustc_identity: "rustc 1.98.0 (88d9e12ae 2026-08-18)".to_string(),
            units,
        }
    }

    fn ledger_with(paths: &[&str]) -> SubstitutionLedger {
        SubstitutionLedger {
            placed: paths
                .iter()
                .map(|path| (PathBuf::from(path), "sha256:dep".to_string()))
                .collect(),
        }
    }

    const ALL_DEPS_PLACED: &[&str] = &[
        "/target/debug/deps/libproc_macro2-aaaa.rlib",
        "/target/debug/deps/libquote-bbbb.rlib",
        "/target/debug/deps/libunicode_ident-cccc.rlib",
    ];

    #[test]
    fn parses_a_cargo_issued_invocation_in_both_option_spellings() -> TestResult {
        let request = syn_request()?;
        assert_eq!(request.crate_name, "syn");
        assert_eq!(request.package, "syn");
        assert_eq!(request.version, "2.0.117");
        assert_eq!(request.edition.as_deref(), Some("2021"));
        assert_eq!(
            request.features,
            ["clone-impls", "derive", "proc-macro"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        assert_eq!(request.crate_types, BTreeSet::from(["lib".to_string()]));
        assert_eq!(request.target, None);
        assert_eq!(request.opt_level.as_deref(), Some("0"));
        assert_eq!(request.debuginfo.as_deref(), Some("2"));
        assert_eq!(request.panic, None);
        assert_eq!(request.out_dir, PathBuf::from("/target/debug/deps"));
        assert_eq!(request.extra_filename, "-0123456789abcdef");
        assert_eq!(request.externs.len(), 3);
        assert_eq!(
            request.externs.get("quote"),
            Some(&Some(PathBuf::from("/target/debug/deps/libquote-bbbb.rlib")))
        );
        assert_eq!(
            request.expected_artifact_name("lib").as_deref(),
            Some("libsyn-0123456789abcdef.rlib")
        );
        Ok(())
    }

    #[test]
    fn parse_reports_the_missing_fact_rather_than_guessing() {
        let missing_name = RustcUnitRequest::parse(
            &["--out-dir", "/t", "-C", "extra-filename=-x"],
            env_with(&[("CARGO_PKG_NAME", "syn"), ("CARGO_PKG_VERSION", "2.0.117")]),
        );
        assert_eq!(missing_name, Err(RustcUnitRequestError::Missing("--crate-name")));
        let missing_package = RustcUnitRequest::parse(
            &["--crate-name", "syn", "--out-dir", "/t", "-C", "extra-filename=-x"],
            env_with(&[("CARGO_PKG_VERSION", "2.0.117")]),
        );
        assert_eq!(missing_package, Err(RustcUnitRequestError::Missing("CARGO_PKG_NAME")));
        let dangling = RustcUnitRequest::parse(&["--crate-name"], env_with(&[]));
        assert_eq!(
            dangling,
            Err(RustcUnitRequestError::DanglingOption("--crate-name".to_string()))
        );
    }

    #[test]
    fn matches_the_base_unit_when_identity_codegen_and_dependencies_agree() -> TestResult {
        let request = syn_request()?;
        let manifest = manifest_with(vec![syn_base_unit()]);
        let unit = match_unit(
            &request,
            &manifest,
            &manifest.rustc_identity,
            &ledger_with(ALL_DEPS_PLACED),
        )
        .map_err(|miss| format!("{miss:?}"))?;
        assert_eq!(unit.rlib_digest, "sha256:abc");
        Ok(())
    }

    #[test]
    fn refuses_a_manifest_for_another_compiler_or_schema() -> TestResult {
        let request = syn_request()?;
        let ledger = ledger_with(ALL_DEPS_PLACED);
        let manifest = manifest_with(vec![syn_base_unit()]);
        assert_eq!(
            match_unit(&request, &manifest, "rustc 1.97.0 (deadbeef 2026-06-01)", &ledger),
            Err(SubstitutionMiss::IncompatibleManifest)
        );
        let mut stale = manifest.clone();
        stale.schema = "incan-oven-base-units/0".to_string();
        assert_eq!(
            match_unit(&request, &stale, &manifest.rustc_identity, &ledger),
            Err(SubstitutionMiss::IncompatibleManifest)
        );
        Ok(())
    }

    #[test]
    fn refuses_a_feature_subset_or_superset() -> TestResult {
        let request = syn_request()?;
        let ledger = ledger_with(ALL_DEPS_PLACED);
        let mut fewer = syn_base_unit();
        fewer.features.remove("derive");
        let mut more = syn_base_unit();
        more.features.insert("full".to_string());
        let manifest = manifest_with(vec![fewer, more]);
        assert_eq!(
            match_unit(&request, &manifest, &manifest.rustc_identity, &ledger),
            Err(SubstitutionMiss::NoMatchingUnit)
        );
        Ok(())
    }

    #[test]
    fn refuses_different_codegen_or_target() -> TestResult {
        let request = syn_request()?;
        let ledger = ledger_with(ALL_DEPS_PLACED);
        let mut release = syn_base_unit();
        release.opt_level = "3".to_string();
        let manifest = manifest_with(vec![release]);
        assert_eq!(
            match_unit(&request, &manifest, &manifest.rustc_identity, &ledger),
            Err(SubstitutionMiss::CodegenMismatch)
        );
        let mut cross = syn_base_unit();
        cross.target = Some("wasm32-wasip1".to_string());
        let manifest = manifest_with(vec![cross]);
        assert_eq!(
            match_unit(&request, &manifest, &manifest.rustc_identity, &ledger),
            Err(SubstitutionMiss::CodegenMismatch)
        );
        Ok(())
    }

    #[test]
    fn refuses_when_any_dependency_was_compiled_fresh() -> TestResult {
        let request = syn_request()?;
        let manifest = manifest_with(vec![syn_base_unit()]);
        let partial = ledger_with(&[
            "/target/debug/deps/libproc_macro2-aaaa.rlib",
            "/target/debug/deps/libunicode_ident-cccc.rlib",
        ]);
        assert_eq!(
            match_unit(&request, &manifest, &manifest.rustc_identity, &partial),
            Err(SubstitutionMiss::FreshDependency("quote".to_string()))
        );
        Ok(())
    }

    #[test]
    fn a_leaf_with_no_externs_matches_on_identity_alone() -> TestResult {
        let request = RustcUnitRequest::parse(
            &[
                "--crate-name",
                "unicode_ident",
                "--crate-type",
                "lib",
                "-C",
                "opt-level=0",
                "-C",
                "debuginfo=2",
                "-C",
                "extra-filename=-cccc",
                "--out-dir",
                "/target/debug/deps",
            ],
            env_with(&[("CARGO_PKG_NAME", "unicode-ident"), ("CARGO_PKG_VERSION", "1.0.18")]),
        )?;
        let unit = BaseUnit {
            package: "unicode-ident".to_string(),
            version: "1.0.18".to_string(),
            crate_name: "unicode_ident".to_string(),
            features: BTreeSet::new(),
            target: None,
            opt_level: "0".to_string(),
            debuginfo: "2".to_string(),
            panic: None,
            rlib: PathBuf::from("/store/libunicode_ident-ffff.rlib"),
            rmeta: None,
            rlib_digest: "sha256:leaf".to_string(),
        };
        let manifest = manifest_with(vec![unit]);
        let matched = match_unit(
            &request,
            &manifest,
            &manifest.rustc_identity,
            &SubstitutionLedger::default(),
        )
        .map_err(|miss| format!("{miss:?}"))?;
        assert_eq!(matched.rlib_digest, "sha256:leaf");
        Ok(())
    }

    #[test]
    fn proc_macro_and_bin_units_are_never_substituted() -> TestResult {
        let mut argv = cargo_style_argv();
        let position = argv
            .iter()
            .position(|arg| arg == "lib")
            .ok_or("fixture lost its crate type")?;
        argv[position] = "proc-macro".to_string();
        let request = RustcUnitRequest::parse(
            &argv,
            env_with(&[("CARGO_PKG_NAME", "syn"), ("CARGO_PKG_VERSION", "2.0.117")]),
        )?;
        let manifest = manifest_with(vec![syn_base_unit()]);
        assert_eq!(
            match_unit(
                &request,
                &manifest,
                &manifest.rustc_identity,
                &ledger_with(ALL_DEPS_PLACED)
            ),
            Err(SubstitutionMiss::CrateType)
        );
        Ok(())
    }

    #[test]
    fn manifest_round_trips_through_json() -> TestResult {
        let manifest = manifest_with(vec![syn_base_unit()]);
        let text = serde_json::to_string(&manifest)?;
        let decoded: BaseUnitManifest = serde_json::from_str(&text)?;
        assert_eq!(decoded, manifest);
        Ok(())
    }
}
