//! Published executable-representation inspection.
//!
//! `incan inspect representation` answers the two questions RFC 123's Tooling layer names and that nothing else
//! surfaced: which encoded version a package's representation carries, and which of its public declarations that
//! representation actually covers. It reads the published sidecar and nothing else -- no compilation, no execution,
//! and no fragment decoding, because coverage is declared in the index rather than inferred from content.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::executable_representation::{
    CoverageReason, DeclarationCoverage, EXECUTABLE_REPRESENTATION_VERSION, SurfaceIndex, SurfaceReader,
    representation_version,
};
use serde::Serialize;

use crate::{CliError, CliResult, ExitCode};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest::public_identities::public_executable_identities;
use incan_frontend::loaf_index::{EXECUTABLE_REPRESENTATION_EXTENSION, LIBRARY_MANIFEST_EXTENSION};

/// Output format for `incan inspect representation`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepresentationInspectionFormat {
    /// Version, coverage totals, and per-declaration coverage for terminal use.
    Text,
    /// Deterministic structured report for tools.
    Json,
}

/// Whether this build can interpret a published representation, and the declarations it covers.
#[derive(Debug, Serialize)]
struct RepresentationReport {
    /// Package that published the manifest this report was resolved from.
    library: String,
    /// Package version from that manifest.
    version: String,
    /// Manifest the sidecar was located through.
    manifest_path: PathBuf,
    /// Published representation, absent for a package that ships none.
    representation: Option<PublishedRepresentation>,
    /// Public identities the manifest declares that the representation's index does not mention at all.
    ///
    /// Distinct from an `Uncovered` declaration, which the publisher considered and refused. An identity missing
    /// from the index entirely means the manifest and the sidecar disagree about the public surface.
    unindexed_public_identities: Vec<CanonicalSymbolId>,
}

/// One published sidecar's interpretable version and declared coverage.
#[derive(Debug, Serialize)]
struct PublishedRepresentation {
    /// Sidecar the coverage was read from.
    path: PathBuf,
    /// Encoded-shape version carried inside the sidecar.
    version: u32,
    /// Highest version this build can interpret.
    supported_version: u32,
    /// Whether this build can interpret the sidecar at all.
    interpretable: bool,
    /// Declaring package recorded inside the index, for comparison with the manifest.
    index_library: Option<String>,
    /// Package version recorded inside the index, for comparison with the manifest.
    index_package_version: Option<String>,
    /// Coverage totals by state.
    totals: CoverageTotals,
    /// Declared coverage for every identity the index names.
    declarations: Vec<DeclarationReport>,
}

/// Counts for each coverage state the index can declare.
#[derive(Debug, Default, Serialize)]
struct CoverageTotals {
    /// Declarations with an addressable fragment.
    covered: usize,
    /// Declarations that execute through a separately addressed owning type.
    type_context: usize,
    /// Declarations the publisher deliberately did not cover, by reason.
    uncovered: BTreeMap<String, usize>,
}

/// One indexed declaration and the coverage state the index declares for it.
#[derive(Debug, Serialize)]
struct DeclarationReport {
    /// Canonical identity as published; serialized whole so this command introduces no second identity spelling.
    identity: CanonicalSymbolId,
    /// `covered`, `type_context`, or `uncovered`.
    state: &'static str,
    /// Refusal reason, present only for an uncovered declaration.
    reason: Option<&'static str>,
    /// Direct canonical execution requirements, present only for a covered public declaration.
    requirements: Vec<CanonicalSymbolId>,
    /// Owning public type, present only for a member that executes through its type context.
    type_context_owner: Option<CanonicalSymbolId>,
}

/// Inspect the executable representation published beside one package's other products.
pub fn inspect_representation(path: &Path, format: RepresentationInspectionFormat) -> CliResult<ExitCode> {
    let manifest_path = resolve_manifest_path(path)?;
    let manifest = LibraryManifest::read_from_path(&manifest_path)
        .map_err(|error| CliError::failure(format!("failed to read `{}`: {error}", manifest_path.display())))?;
    let report = build_report(&manifest_path, &manifest)?;
    render(&report, format)
}

/// Locate the `.incnlib` manifest at `path`: the manifest itself, or the Loaf artifact directory holding it (DD-0007).
fn resolve_manifest_path(path: &Path) -> CliResult<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(CliError::failure(format!(
            "no such path `{}`; pass a Loaf manifest or its artifact directory",
            path.display()
        )));
    }
    sole_manifest_in(path)?.ok_or_else(|| {
        CliError::failure(format!(
            "no `.incnlib` manifest in `{}`; pass a Loaf manifest or its artifact directory",
            path.display()
        ))
    })
}

/// Return the one manifest in `root`, or a refusal when several are present and none can be preferred.
fn sole_manifest_in(root: &Path) -> CliResult<Option<PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let entries = fs::read_dir(root)
        .map_err(|error| CliError::failure(format!("failed to inspect `{}`: {error}", root.display())))?;
    let mut manifests = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| CliError::failure(format!("failed to inspect `{}`: {error}", root.display())))?;
        let candidate = entry.path();
        if candidate
            .extension()
            .is_some_and(|extension| extension == LIBRARY_MANIFEST_EXTENSION)
        {
            manifests.push(candidate);
        }
    }
    manifests.sort();
    match manifests.len() {
        0 => Ok(None),
        1 => Ok(manifests.pop()),
        _ => Err(CliError::failure(format!(
            "`{}` holds several package manifests; name the one to inspect",
            root.display()
        ))),
    }
}

/// Read the manifest's declared surface and, when one is published, the sidecar's version and coverage.
fn build_report(manifest_path: &Path, manifest: &LibraryManifest) -> CliResult<RepresentationReport> {
    let public = public_executable_identities(manifest);
    // A Loaf's representation sits beside its manifest as `<name>.incnbody` (DD-0007).
    let surface_path = manifest_path.with_extension(EXECUTABLE_REPRESENTATION_EXTENSION);
    if manifest.contract_metadata.executable_representation.is_none() {
        return Ok(RepresentationReport {
            library: manifest.name.clone(),
            version: manifest.version.clone(),
            manifest_path: manifest_path.to_path_buf(),
            representation: None,
            unindexed_public_identities: public.into_iter().collect(),
        });
    }
    let bytes = fs::read(&surface_path).map_err(|error| {
        CliError::failure(format!(
            "the manifest names a representation at `{}` that cannot be read: {error}",
            surface_path.display()
        ))
    })?;
    let version = representation_version(&bytes).map_err(|error| {
        CliError::failure(format!(
            "`{}` does not carry a readable representation version: {error}",
            surface_path.display()
        ))
    })?;

    // The version is reported even when this build cannot interpret it, because "which version is this?" is the
    // question a reader facing an unsupported package most needs answered. Only the index read is skipped.
    let index = match SurfaceReader::open(&bytes) {
        Ok(reader) => Some(reader.index().clone()),
        Err(_) if version != EXECUTABLE_REPRESENTATION_VERSION => None,
        Err(error) => {
            return Err(CliError::failure(format!(
                "`{}` declares a supported version but its index cannot be read: {error}",
                surface_path.display()
            )));
        }
    };

    let mut unindexed = Vec::new();
    let declarations = match index.as_ref() {
        Some(index) => {
            for identity in &public {
                if !index.declarations.contains_key(identity) {
                    unindexed.push(identity.clone());
                }
            }
            index
                .declarations
                .iter()
                .map(|(identity, coverage)| declaration_report(identity, coverage))
                .collect()
        }
        None => Vec::new(),
    };

    Ok(RepresentationReport {
        library: manifest.name.clone(),
        version: manifest.version.clone(),
        manifest_path: manifest_path.to_path_buf(),
        representation: Some(PublishedRepresentation {
            path: surface_path,
            version,
            supported_version: EXECUTABLE_REPRESENTATION_VERSION,
            interpretable: index.is_some(),
            index_library: index.as_ref().map(|index| index.library.clone()),
            index_package_version: index.as_ref().map(|index| index.package_version.clone()),
            totals: totals_for(index.as_ref()),
            declarations,
        }),
        unindexed_public_identities: unindexed,
    })
}

/// Project one indexed declaration into its reportable shape.
fn declaration_report(identity: &CanonicalSymbolId, coverage: &DeclarationCoverage) -> DeclarationReport {
    match coverage {
        DeclarationCoverage::Covered { requirements, .. } => DeclarationReport {
            identity: identity.clone(),
            state: "covered",
            reason: None,
            requirements: requirements.clone(),
            type_context_owner: None,
        },
        DeclarationCoverage::Uncovered(reason) => DeclarationReport {
            identity: identity.clone(),
            state: "uncovered",
            reason: Some(reason_label(*reason)),
            requirements: Vec::new(),
            type_context_owner: None,
        },
        DeclarationCoverage::TypeContext { owner } => DeclarationReport {
            identity: identity.clone(),
            state: "type_context",
            reason: None,
            requirements: Vec::new(),
            type_context_owner: Some(owner.clone()),
        },
    }
}

/// Count each coverage state, keeping uncovered declarations separated by the reason the publisher recorded.
fn totals_for(index: Option<&SurfaceIndex>) -> CoverageTotals {
    let mut totals = CoverageTotals::default();
    let Some(index) = index else {
        return totals;
    };
    for coverage in index.declarations.values() {
        match coverage {
            DeclarationCoverage::Covered { .. } => totals.covered += 1,
            DeclarationCoverage::TypeContext { .. } => totals.type_context += 1,
            DeclarationCoverage::Uncovered(reason) => {
                *totals.uncovered.entry(reason_label(*reason).to_string()).or_default() += 1;
            }
        }
    }
    totals
}

/// Stable wire spelling for one refusal reason.
///
/// Written out rather than derived from the variant name so the reported vocabulary is a deliberate contract that a
/// rename of the Rust variant cannot silently change under a consumer.
fn reason_label(reason: CoverageReason) -> &'static str {
    match reason {
        CoverageReason::UnsupportedConstruct => "unsupported_construct",
        CoverageReason::PrivateDependency => "private_dependency",
        CoverageReason::UnresolvedReference => "unresolved_reference",
        CoverageReason::RequiredDeclarationUnavailable => "required_declaration_unavailable",
        CoverageReason::NoExecutableDeclaration => "no_executable_declaration",
    }
}

/// Emit the report in the requested format.
fn render(report: &RepresentationReport, format: RepresentationInspectionFormat) -> CliResult<ExitCode> {
    match format {
        RepresentationInspectionFormat::Json => {
            let text = serde_json::to_string_pretty(report)
                .map_err(|error| CliError::failure(format!("failed to encode the report: {error}")))?;
            println!("{text}");
        }
        RepresentationInspectionFormat::Text => render_text(report),
    }
    Ok(ExitCode::SUCCESS)
}

/// Write the terminal summary: identity, version, totals, then one line per declaration.
fn render_text(report: &RepresentationReport) {
    println!("package    {} {}", report.library, report.version);
    println!("manifest   {}", report.manifest_path.display());
    let Some(published) = report.representation.as_ref() else {
        println!("representation  none published");
        println!(
            "\nThis package links without an executable representation. {} public declaration(s) are therefore \
             unavailable to the non-linking route.",
            report.unindexed_public_identities.len()
        );
        return;
    };
    println!("sidecar    {}", published.path.display());
    let support = if published.interpretable {
        "supported"
    } else {
        "NOT interpretable by this build"
    };
    println!(
        "version    {} ({support}; this build reads version {})",
        published.version, published.supported_version
    );
    if let (Some(library), Some(package_version)) = (
        published.index_library.as_ref(),
        published.index_package_version.as_ref(),
    ) && (library != &report.library || package_version != &report.version)
    {
        println!("MISMATCH   the sidecar declares {library} {package_version}");
    }
    if !published.interpretable {
        return;
    }

    println!(
        "\ncoverage   {} covered, {} through type context, {} uncovered",
        published.totals.covered,
        published.totals.type_context,
        published.totals.uncovered.values().sum::<usize>()
    );
    for (reason, count) in &published.totals.uncovered {
        println!("             {count} {reason}");
    }
    if !report.unindexed_public_identities.is_empty() {
        println!(
            "             {} public declaration(s) absent from the index entirely",
            report.unindexed_public_identities.len()
        );
    }

    println!("\ndeclarations");
    for declaration in &published.declarations {
        let suffix = match (declaration.reason, declaration.requirements.len()) {
            (Some(reason), _) => format!(" ({reason})"),
            (None, 0) => String::new(),
            (None, count) => format!(" ({count} requirement(s))"),
        };
        println!(
            "  {:<13} {:<10} {}{suffix}",
            declaration.state,
            declaration.identity.kind.as_str(),
            declaration.identity.declaration_name
        );
    }
}
