//! Shared executable evidence for RFC 120's pinned release-artifact projection.
//!
//! Both the focused #1174 integration test and the #987 parity corpus call this exact probe. Keeping the native
//! build and symbol-table inspection here prevents the corpus from treating a pointer to some other test as proof.

use std::collections::HashSet;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, ScopeDiscriminant, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
    decode_incan_identity_from_demangled_symbol, encode_incan_symbol_identity,
};
use rustc_demangle::try_demangle;
use sha2::{Digest, Sha256};

pub(crate) const SELECTED_RUST: &str = "1.98.0";
const FIXTURE_CRATE_NAME: &str = "incan_symbol_fixture";
const FIXTURE_RUSTC_ARGS: &[&str] = &[
    "--crate-name",
    FIXTURE_CRATE_NAME,
    "--edition=2024",
    "-Copt-level=3",
    "-Cdebuginfo=0",
    "-Ccodegen-units=1",
    "-Csymbol-mangling-version=v0",
    "-Clink-dead-code=yes",
];

/// Verifiable facts recovered from one real optimized Rust-v0 artifact.
#[derive(Debug, Clone)]
pub(crate) struct ArtifactProjectionEvidence {
    pub(crate) fixture_input_identity: String,
    pub(crate) artifact_content_identity: String,
    pub(crate) recovered_observation_identity: String,
    pub(crate) recovered_identities: Vec<CanonicalSymbolId>,
    pub(crate) saw_generic_u64_specialization: bool,
    pub(crate) saw_non_incan_host_symbol: bool,
    pub(crate) baseline_bytes: u64,
    pub(crate) projected_bytes: u64,
    pub(crate) baseline_identifier_bytes: usize,
    pub(crate) projected_identifier_bytes: usize,
}

/// Compile and inspect the exact DD-0002 artifact fixture, refusing any missing or misclassified identity.
pub(crate) fn verify_pinned_release_artifact() -> Result<ArtifactProjectionEvidence, Box<dyn Error>> {
    if !matches!(std::env::consts::OS, "linux" | "macos") {
        return Err(format!(
            "the incan-v1 artifact fixture needs an nm adapter for supported CI platform `{}`",
            std::env::consts::OS
        )
        .into());
    }

    let (rustc, rustc_version) = selected_rustc()?;
    let identities = fixture_identities();
    let projected_names = identities.iter().map(encode_incan_symbol_identity).collect::<Vec<_>>();
    let baseline_names = ["ordinary", "generic", "method", "storage"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let projected_source = fixture_source(&projected_names)?;
    let baseline_source = fixture_source(&baseline_names)?;
    let rustc_args = FIXTURE_RUSTC_ARGS.join("\0");
    let fixture_input_identity = content_identity(&[
        rustc_version.as_bytes(),
        std::env::consts::OS.as_bytes(),
        std::env::consts::ARCH.as_bytes(),
        rustc_args.as_bytes(),
        projected_source.as_bytes(),
        baseline_source.as_bytes(),
    ]);

    let temporary = tempfile::tempdir()?;
    let projected = compile_fixture(&rustc, temporary.path(), "projected", &projected_source)?;
    let baseline = compile_fixture(&rustc, temporary.path(), "baseline", &baseline_source)?;

    let symbols = native_symbols(&projected)?;
    let mut recovered = HashSet::new();
    let mut saw_generic_u64_specialization = false;
    let mut saw_non_incan_host_symbol = false;
    for raw in symbols.lines().filter_map(|line| line.split_whitespace().last()) {
        let raw = raw.strip_prefix('_').unwrap_or(raw);
        let Ok(symbol) = try_demangle(raw) else {
            continue;
        };
        let demangled = format!("{symbol:#}");
        let fixture_symbol = FixtureDemangledSymbol::parse(&demangled);
        if fixture_symbol
            .as_ref()
            .is_some_and(|symbol| symbol.matches(FIXTURE_CRATE_NAME, "host_bridge", None))
        {
            saw_non_incan_host_symbol = true;
            if decode_incan_identity_from_demangled_symbol(&demangled)?.is_some() {
                return Err("a non-Incan host frame decoded as an Incan source declaration".into());
            }
        }
        if fixture_symbol
            .as_ref()
            .is_some_and(|symbol| symbol.matches(FIXTURE_CRATE_NAME, &projected_names[1], Some("u64")))
        {
            saw_generic_u64_specialization = true;
        }
        if let Some(identity) = decode_incan_identity_from_demangled_symbol(&demangled)? {
            recovered.insert(identity);
        }
    }

    let expected = identities.into_iter().collect::<HashSet<_>>();
    if recovered != expected {
        return Err(format!(
            "demangled artifact symbols did not recover the exact canonical identities: expected {expected:?}, got {recovered:?}"
        )
        .into());
    }
    if !saw_generic_u64_specialization {
        return Err("Rust v0 demangling lost the generic `<u64>` specialization".into());
    }
    if !saw_non_incan_host_symbol {
        return Err("fixture did not retain its non-Incan host symbol".into());
    }

    let projected_bytes = fs::metadata(&projected)?.len();
    let baseline_bytes = fs::metadata(&baseline)?.len();
    let projected_content = fs::read(&projected)?;
    let baseline_content = fs::read(&baseline)?;
    let artifact_content_identity = content_identity(&[&projected_content, &baseline_content]);
    let projected_identifier_bytes = projected_names.iter().map(String::len).sum::<usize>();
    let baseline_identifier_bytes = baseline_names.iter().map(String::len).sum::<usize>();
    if projected_identifier_bytes <= baseline_identifier_bytes {
        return Err("fixture did not measure the projection-name cost".into());
    }

    let mut recovered_identities = recovered.into_iter().collect::<Vec<_>>();
    recovered_identities.sort();
    let recovered_rendered = recovered_identities
        .iter()
        .map(CanonicalSymbolId::render_compact)
        .collect::<Vec<_>>()
        .join("\n");
    let generic_observation: &[u8] = if saw_generic_u64_specialization {
        b"generic_u64=true"
    } else {
        b"generic_u64=false"
    };
    let host_observation: &[u8] = if saw_non_incan_host_symbol {
        b"non_incan_host=true"
    } else {
        b"non_incan_host=false"
    };
    let recovered_observation_identity = content_identity(&[
        symbols.as_bytes(),
        recovered_rendered.as_bytes(),
        generic_observation,
        host_observation,
    ]);
    Ok(ArtifactProjectionEvidence {
        fixture_input_identity,
        artifact_content_identity,
        recovered_observation_identity,
        recovered_identities,
        saw_generic_u64_specialization,
        saw_non_incan_host_symbol,
        baseline_bytes,
        projected_bytes,
        baseline_identifier_bytes,
        projected_identifier_bytes,
    })
}

/// The exact path shape emitted by the small Rust-v0 fixture.
///
/// This parser is intentionally narrower than a general Rust demangler. It separates the crate path, item path, and
/// final generic argument list so fixture evidence cannot be satisfied by a host/item lookalike substring.
#[derive(Debug, PartialEq, Eq)]
struct FixtureDemangledSymbol<'a> {
    crate_name: &'a str,
    item_path: &'a str,
    generic_args: Option<&'a str>,
}

impl<'a> FixtureDemangledSymbol<'a> {
    fn parse(demangled: &'a str) -> Option<Self> {
        let (path, generic_args) = if let Some((path, generic_args)) = demangled.rsplit_once("::<") {
            let generic_args = generic_args.strip_suffix('>')?;
            if path.contains('<') || path.contains('>') || generic_args.contains('<') || generic_args.contains('>') {
                return None;
            }
            (path, Some(generic_args))
        } else {
            if demangled.contains('<') || demangled.contains('>') {
                return None;
            }
            (demangled, None)
        };
        let (crate_name, item_path) = path.split_once("::")?;
        if crate_name.is_empty() || item_path.is_empty() {
            return None;
        }
        Some(Self {
            crate_name,
            item_path,
            generic_args,
        })
    }

    fn matches(&self, crate_name: &str, item_path: &str, generic_args: Option<&str>) -> bool {
        self.crate_name == crate_name && self.item_path == item_path && self.generic_args == generic_args
    }
}

#[cfg(test)]
mod fixture_demangled_symbol_tests {
    use super::{FIXTURE_CRATE_NAME, FixtureDemangledSymbol, content_identity};

    #[test]
    fn content_identity_binds_bytes_and_chunk_boundaries() {
        assert_ne!(content_identity(&[b"artifact-a"]), content_identity(&[b"artifact-b"]));
        assert_ne!(content_identity(&[b"ab", b"c"]), content_identity(&[b"a", b"bc"]));
    }

    #[test]
    fn host_bridge_match_rejects_path_and_identifier_lookalikes() {
        let exact = FixtureDemangledSymbol::parse("incan_symbol_fixture::host_bridge");
        assert!(exact.is_some_and(|symbol| symbol.matches(FIXTURE_CRATE_NAME, "host_bridge", None)));

        for lookalike in [
            "incan_symbol_fixture::host_bridge_adapter",
            "incan_symbol_fixture::nested::host_bridge",
            "incan_symbol_fixture_lookalike::host_bridge",
            "incan_symbol_fixture::host_bridge::<u64>",
        ] {
            let parsed = FixtureDemangledSymbol::parse(lookalike);
            assert!(
                !parsed.is_some_and(|symbol| symbol.matches(FIXTURE_CRATE_NAME, "host_bridge", None)),
                "lookalike `{lookalike}` must not satisfy host-symbol evidence"
            );
        }
    }

    #[test]
    fn generic_match_requires_exact_item_path_and_u64_suffix() {
        let projected = "__incan_v1_0102";
        let exact = FixtureDemangledSymbol::parse("incan_symbol_fixture::__incan_v1_0102::<u64>");
        assert!(exact.is_some_and(|symbol| symbol.matches(FIXTURE_CRATE_NAME, projected, Some("u64"))));

        for lookalike in [
            "incan_symbol_fixture::__incan_v1_0102_adapter::<u64>",
            "incan_symbol_fixture::nested::__incan_v1_0102::<u64>",
            "incan_symbol_fixture::__incan_v1_0102::<u64x>",
            "incan_symbol_fixture::__incan_v1_0102::<Vec<u64>>",
            "incan_symbol_fixture::__incan_v1_0102_u64",
        ] {
            let parsed = FixtureDemangledSymbol::parse(lookalike);
            assert!(
                !parsed.is_some_and(|symbol| symbol.matches(FIXTURE_CRATE_NAME, projected, Some("u64"))),
                "lookalike `{lookalike}` must not satisfy generic-specialization evidence"
            );
        }
    }
}

fn selected_rustc() -> Result<(PathBuf, String), Box<dyn Error>> {
    let output = Command::new("rustup")
        .args(["which", "--toolchain", SELECTED_RUST, "rustc"])
        .output()
        .map_err(|error| format!("DD-0002 requires rustup and Rust {SELECTED_RUST}: {error}"))?;
    ensure_success("locate the DD-0002 Rust toolchain", &output)?;
    let path = String::from_utf8(output.stdout)?.trim().to_string();
    if path.is_empty() {
        return Err(format!("rustup returned no rustc path for required toolchain {SELECTED_RUST}").into());
    }
    let rustc = PathBuf::from(path);
    let version = Command::new(&rustc).arg("-Vv").output()?;
    ensure_success("query the DD-0002 rustc version", &version)?;
    let version = String::from_utf8(version.stdout)?;
    let release = version.lines().find_map(|line| line.strip_prefix("release: "));
    if release != Some(SELECTED_RUST) {
        return Err(format!(
            "DD-0002 artifact fixture requires rustc release {SELECTED_RUST}, found {}",
            release.unwrap_or("an unparseable rustc -Vv response")
        )
        .into());
    }
    Ok((rustc, version))
}

fn compile_fixture(rustc: &Path, directory: &Path, variant: &str, source: &str) -> Result<PathBuf, Box<dyn Error>> {
    let source_path = directory.join(format!("{variant}.rs"));
    let artifact_path = directory.join(format!("{variant}{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source_path, source)?;
    let output = Command::new(rustc)
        .args(FIXTURE_RUSTC_ARGS)
        .arg(&source_path)
        .arg("-o")
        .arg(&artifact_path)
        .output()?;
    ensure_success("compile the incan-v1 release artifact fixture", &output)?;
    Ok(artifact_path)
}

fn content_identity(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.len().to_le_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut identity = String::with_capacity("sha256:".len() + digest.len() * 2);
    identity.push_str("sha256:");
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        identity.push(char::from(HEX[usize::from(byte >> 4)]));
        identity.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    identity
}

fn native_symbols(artifact: &Path) -> Result<String, Box<dyn Error>> {
    let output = Command::new("nm")
        .arg(artifact)
        .output()
        .map_err(|error| format!("incan-v1 artifact inspection requires the platform `nm` tool: {error}"))?;
    ensure_success("inspect the release artifact symbol table", &output)?;
    Ok(String::from_utf8(output.stdout)?)
}

fn ensure_success(action: &str, output: &Output) -> Result<(), Box<dyn Error>> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "failed to {action}: status={}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

fn fixture_source(names: &[String]) -> Result<String, Box<dyn Error>> {
    let [ordinary, generic, method, storage] = names else {
        return Err("fixture requires exactly four symbol names".into());
    };
    let mut source = String::new();
    writeln!(
        source,
        r#"
#[inline(never)]
fn {ordinary}(value: u64) -> u64 {{ std::hint::black_box(value.wrapping_add(1)) }}

#[inline(never)]
fn {generic}<T: Copy>(value: T) -> T {{ std::hint::black_box(value) }}

struct Fixture;

static {storage}: u64 = 17;

impl Fixture {{
    #[inline(never)]
    fn {method}(&self, value: u64) -> u64 {{ std::hint::black_box(value.wrapping_mul(2)) }}
}}

#[inline(never)]
fn host_bridge(value: u64) -> u64 {{ std::hint::black_box(value.wrapping_sub(1)) }}

fn main() {{
    let value = {ordinary}(std::hint::black_box(41));
    let value = {generic}::<u64>(value);
    let value = Fixture.{method}(value);
    let value = value.wrapping_add(std::hint::black_box({storage}));
    std::hint::black_box(host_bridge(value));
}}
"#
    )?;
    Ok(source)
}

fn fixture_identities() -> Vec<CanonicalSymbolId> {
    vec![
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(vec!["fixture".to_string()]),
            declaration_name: "ordinary".to_string(),
            kind: SemanticSourceTargetKind::Function,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(10, 31),
        },
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Package {
                library: "fixture-package".to_string(),
                module_path: vec!["generics".to_string()],
            },
            declaration_name: "generic".to_string(),
            kind: SemanticSourceTargetKind::Function,
            scope_discriminant: Some(ScopeDiscriminant(7)),
            declaration_span: HirSourceSpan::new(40, 73),
        },
        CanonicalSymbolId {
            namespace: SymbolNamespace::Member,
            origin: SymbolOrigin::Module(vec!["fixture".to_string(), "Fixture".to_string()]),
            declaration_name: "method".to_string(),
            kind: SemanticSourceTargetKind::Method,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(90, 130),
        },
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(vec!["fixture".to_string()]),
            declaration_name: "storage".to_string(),
            kind: SemanticSourceTargetKind::Static,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(140, 167),
        },
    ]
}
