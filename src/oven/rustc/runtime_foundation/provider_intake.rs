//! The provider-intake exchange with the Incan-authored control plane: the wire request and response, the build-script
//! directives a provider's captured output is parsed and bound into, and the receipt each provider seals.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::super::{
    OvenMaterializedRustFacetEnvironmentValue, OvenRustcError, OvenSelectedRustFacetEnvironmentValue,
    OvenSelectedRustFacetGeneratedInput, OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetUnit,
    ValidatedOvenSelectedRustFacetGraph, digest_bytes,
};
use super::{
    OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION, OVEN_RUNTIME_FOUNDATION_PROVIDER_STDOUT_LIMIT,
    OvenRuntimeFoundationNativeLinkState, OvenRuntimeFoundationProviderDeclaration,
    OvenRuntimeFoundationProviderDirectives, OvenRuntimeFoundationProviderEffects,
    OvenRuntimeFoundationProviderHostDependency, OvenRuntimeFoundationProviderPackageSource,
    OvenRuntimeFoundationProviderRecord, OvenRuntimeFoundationProviderState, runtime_foundation_invalid,
    runtime_foundation_provider_package_contains_path, validate_runtime_foundation_provider_declaration,
    validate_runtime_foundation_provider_host_dependencies, validate_runtime_foundation_provider_package_source,
};

/// Exact response schema emitted by the Incan-authored provider-intake exchange.
pub(crate) const OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_REQUEST_SCHEMA: &str =
    "incan.oven.runtime-foundation-provider-intake-request/2";
pub(super) const OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_SCHEMA: &str =
    "incan.oven.runtime-foundation-provider-intake/2";

/// One original manifest build-dependency slot retained only for the host-to-source intake request.
///
/// The source bridge checks that `declaration_index` names a literal `build-dependencies` declaration before it
/// reduces this record to the alias/unit pair returned in a provider declaration. Keeping the slot in request evidence
/// prevents a host from reconstructing that source correlation from a selected graph, which deliberately contains no
/// manifest parser or dependency-role projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenRuntimeFoundationProviderIntakeHostDependency {
    /// Stable ordinal of the original selected manifest dependency declaration.
    pub(crate) declaration_index: u32,
    /// Rust-facing alias from that original declaration.
    pub(crate) alias: String,
    /// Existing selected host-domain unit identity bound by the source producer.
    pub(crate) unit: String,
}

/// Exact source request encoded only from host-validated evidence.
#[derive(Serialize)]
struct OvenRuntimeFoundationProviderIntakeWireRequest<'a> {
    schema: &'static str,
    request: OvenRuntimeFoundationProviderIntakeWireRequestBody<'a>,
}

/// Closed request body shared only with the Incan provider-intake source contract.
#[derive(Serialize)]
struct OvenRuntimeFoundationProviderIntakeWireRequestBody<'a> {
    selected_identity: &'a str,
    manifest: &'a OvenSelectedRustFacetSourceMember,
    manifest_source: &'a str,
    package_members: &'a [OvenSelectedRustFacetSourceMember],
    host_dependencies: &'a [OvenRuntimeFoundationProviderIntakeHostDependency],
}

/// Closed wire response accepted only as a source declaration candidate. It has no execution authority.
#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum OvenRuntimeFoundationProviderIntakeWireResponse {
    /// The checked source declaration selected no build script.
    NoBuildScript { schema: String, selected_identity: String },
    /// The checked source declaration selected one build script and its explicitly supplied package inventory.
    BuildScript {
        schema: String,
        selected_identity: String,
        declaration: OvenRuntimeFoundationProviderIntakeWireDeclaration,
    },
    /// Source parsing or declaration binding refused; this cannot become a no-provider outcome by omission.
    Refused {
        schema: String,
        selected_identity: String,
        error: OvenRuntimeFoundationProviderIntakeWireRefusal,
    },
}

/// Source-only declaration facts which still require selected-graph authentication by the Rust host.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OvenRuntimeFoundationProviderIntakeWireDeclaration {
    entrypoint: OvenRuntimeFoundationProviderIntakeWireSourceMember,
    package_members: Vec<OvenRuntimeFoundationProviderIntakeWireSourceMember>,
    host_dependencies: Vec<OvenRuntimeFoundationProviderIntakeWireHostDependency>,
}

/// One portable package member as represented by the Incan source exchange.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OvenRuntimeFoundationProviderIntakeWireSourceMember {
    path: String,
    digest: String,
}

/// One source-validated build-dependency slot reduced to the selected alias/unit edge.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OvenRuntimeFoundationProviderIntakeWireHostDependency {
    alias: String,
    unit: String,
}

/// Host-retained evidence for one exact provider-intake Engine request.
///
/// The Engine command path must construct this only after it has rehashed the retained package manifest and complete
/// package inventory, bound declaration slots to selected host units, and issued the exact request permit. The Incan
/// response carries no roots or manifest bytes; this record restores those facts without a second graph, filesystem
/// probe or Cargo metadata query.
#[derive(Debug, Clone)]
pub(crate) struct OvenRuntimeFoundationProviderIntakeEvidence {
    /// Existing selected Rust unit which this source declaration may describe.
    selected_identity: String,
    /// Complete physical package evidence retained by the host request.
    pub(super) package: OvenRuntimeFoundationProviderPackageSource,
    /// Source-declared build-dependency slots already correlated to selected host units by the producer.
    host_dependencies: Vec<OvenRuntimeFoundationProviderIntakeHostDependency>,
}

/// Closed refusal details are decoded only to keep malformed source output from becoming an implicit no-provider state.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OvenRuntimeFoundationProviderIntakeWireRefusal {
    kind: String,
    fields: Vec<String>,
}

/// Seal every captured receipt from its complete typed effect record after canonical presentation ordering is fixed.
///
/// This runs only on the publisher-side constructor. Admission recomputes and compares these values; it must never
/// repair a descriptor whose effect record and asserted receipt identity disagree.
pub(super) fn seal_runtime_foundation_provider_receipts(
    providers: &mut [OvenRuntimeFoundationProviderRecord],
) -> Result<(), OvenRustcError> {
    for record in providers {
        let OvenRuntimeFoundationProviderState::Captured { receipt } = &mut record.state else {
            continue;
        };
        if receipt.schema_version != OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider receipt schema",
                format!(
                    "expected schema {OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION}, found {}",
                    receipt.schema_version
                ),
            ));
        }
        receipt.effect_digest = runtime_foundation_provider_effect_digest(&receipt.effects)?;
        receipt.provider_receipt_identity = runtime_foundation_provider_receipt_identity(
            &record.selected_identity,
            &record.declaration,
            &receipt.effects,
        )?;
    }
    Ok(())
}

/// Derive the digest of the entire normalized build-script effect vocabulary.
pub(super) fn runtime_foundation_provider_effect_digest(
    effects: &OvenRuntimeFoundationProviderEffects,
) -> Result<String, OvenRustcError> {
    let bytes = serde_json::to_vec(&(
        "incan.oven.runtime-foundation-provider-effects/1",
        OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION,
        effects,
    ))
    .map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation provider effect digest",
            format!("cannot encode canonical provider effects: {error}"),
        )
    })?;
    Ok(digest_bytes(&bytes))
}

/// Derive one provider receipt identity from its selected unit, declared script input and complete effect record.
pub(super) fn runtime_foundation_provider_receipt_identity(
    selected_identity: &str,
    declaration: &OvenRuntimeFoundationProviderDeclaration,
    effects: &OvenRuntimeFoundationProviderEffects,
) -> Result<String, OvenRustcError> {
    let bytes = serde_json::to_vec(&(
        "incan.oven.runtime-foundation-provider-receipt/1",
        OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION,
        selected_identity,
        declaration,
        effects,
    ))
    .map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation provider receipt identity",
            format!("cannot encode canonical provider receipt: {error}"),
        )
    })?;
    Ok(digest_bytes(&bytes))
}

/// Parse a bounded build-script stdout stream without letting unsupported output become an unrecorded compiler fact.
///
/// This recognizes both Cargo's legacy `cargo:` spelling and its current `cargo::` spelling, but it does not invoke
/// Cargo or infer a dependency graph. Raw environment values remain in this short-lived result until a later caller
/// proves they equal the selected graph's declared environment representation.
pub(super) fn parse_runtime_foundation_provider_directives(
    stdout: &[u8],
) -> Result<OvenRuntimeFoundationProviderDirectives, OvenRustcError> {
    if stdout.len() > OVEN_RUNTIME_FOUNDATION_PROVIDER_STDOUT_LIMIT {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider stdout",
            format!(
                "exceeds the {} byte provider-output limit",
                OVEN_RUNTIME_FOUNDATION_PROVIDER_STDOUT_LIMIT
            ),
        ));
    }
    let stdout = std::str::from_utf8(stdout)
        .map_err(|_| runtime_foundation_invalid("runtime foundation provider stdout", "contains non-UTF-8 bytes"))?;
    let mut emitted_cfg = Vec::new();
    let mut checked_cfg = Vec::new();
    let mut emitted_environment = BTreeMap::new();
    let mut rerun_paths = Vec::new();
    let mut rerun_environment = Vec::new();
    let mut native_directives = BTreeSet::new();
    for line in stdout.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let directive = line
            .strip_prefix("cargo::")
            .or_else(|| line.strip_prefix("cargo:"))
            .ok_or_else(|| {
                runtime_foundation_invalid(
                    "runtime foundation provider stdout",
                    "contains a non-directive output line",
                )
            })?;
        let (name, value) = directive.split_once('=').ok_or_else(|| {
            runtime_foundation_invalid("runtime foundation provider directive", "has no `=` separator")
        })?;
        match name {
            "rustc-cfg" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                emitted_cfg.push(value.to_string());
            }
            "rustc-check-cfg" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                checked_cfg.push(value.to_string());
            }
            "rustc-env" => {
                let (environment_name, environment_value) = value.split_once('=').ok_or_else(|| {
                    runtime_foundation_invalid(
                        "runtime foundation provider rustc-env",
                        "has no environment-value separator",
                    )
                })?;
                if environment_name.trim().is_empty()
                    || environment_name.bytes().any(|byte| byte == b'=' || byte == b'\0')
                    || emitted_environment
                        .insert(environment_name.to_string(), environment_value.to_string())
                        .is_some()
                {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation provider rustc-env",
                        "has an empty, malformed or repeated environment name",
                    ));
                }
            }
            "rerun-if-changed" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                rerun_paths.push(value.to_string());
            }
            "rerun-if-env-changed" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                rerun_environment.push(value.to_string());
            }
            "rustc-link-lib" | "rustc-link-search" | "rustc-link-arg" | "rustc-cdylib-link-arg" => {
                require_nonempty_runtime_foundation_provider_directive_value(name, value)?;
                native_directives.insert(name);
            }
            _ => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation provider directive",
                    format!("does not support `{name}`"),
                ));
            }
        }
    }
    let native_link = if native_directives.is_empty() {
        OvenRuntimeFoundationNativeLinkState::NoNativeLink
    } else {
        OvenRuntimeFoundationNativeLinkState::Unsupported {
            reason: format!(
                "provider emitted native-link directive(s): {}",
                native_directives.into_iter().collect::<Vec<_>>().join(", ")
            ),
        }
    };
    Ok(OvenRuntimeFoundationProviderDirectives {
        emitted_cfg,
        checked_cfg,
        emitted_environment,
        rerun_paths,
        rerun_environment,
        native_link,
    })
}

/// Reject an absent or whitespace-only directive value without including the possibly sensitive value in diagnostics.
fn require_nonempty_runtime_foundation_provider_directive_value(
    directive: &str,
    value: &str,
) -> Result<(), OvenRustcError> {
    if value.trim().is_empty() {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider directive",
            format!("`{directive}` has an empty value"),
        ));
    }
    Ok(())
}

/// Bind parsed build-script output to the selected unit and its admitted physical environment.
///
/// This is deliberately a comparison, never an inference. The caller supplies generated-input facts from its owned
/// provider output staging and the physical environment from selected-unit materialization. Every retained value is
/// converted back to the selected graph's portable representation before it can enter a receipt.
pub(super) fn bind_runtime_foundation_provider_directives(
    unit: &OvenSelectedRustFacetUnit,
    declaration: &OvenRuntimeFoundationProviderDeclaration,
    materialized_environment: &BTreeMap<String, OvenMaterializedRustFacetEnvironmentValue>,
    generated_inputs: Vec<OvenSelectedRustFacetGeneratedInput>,
    directives: OvenRuntimeFoundationProviderDirectives,
) -> Result<OvenRuntimeFoundationProviderEffects, OvenRustcError> {
    let OvenRuntimeFoundationProviderDeclaration::BuildScript { package, .. } = declaration else {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider directives",
            format!("unit {} has no declared build-script source closure", unit.crate_name),
        ));
    };
    if generated_inputs != unit.generated_inputs {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider generated inputs",
            format!(
                "unit {} provider-generated members do not exactly match the selected graph",
                unit.crate_name
            ),
        ));
    }
    let mut emitted_environment = BTreeMap::new();
    for (name, value) in directives.emitted_environment {
        let selected = unit.environment.get(&name).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider environment",
                format!(
                    "unit {} emitted an environment name absent from the selected graph",
                    unit.crate_name
                ),
            )
        })?;
        let materialized = materialized_environment.get(&name).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider environment",
                format!(
                    "unit {} has no admitted physical environment for its emitted name",
                    unit.crate_name
                ),
            )
        })?;
        let matches = match (selected, materialized) {
            (
                OvenSelectedRustFacetEnvironmentValue::Text { value: expected },
                OvenMaterializedRustFacetEnvironmentValue::Text(materialized),
            ) => expected == &value && materialized == expected,
            (
                OvenSelectedRustFacetEnvironmentValue::Path { .. },
                OvenMaterializedRustFacetEnvironmentValue::Path(materialized),
            ) => materialized.to_str() == Some(value.as_str()),
            (OvenSelectedRustFacetEnvironmentValue::SensitiveDigest { .. }, _) => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation provider environment",
                    format!(
                        "unit {} emitted a redacted environment value without a separately admitted value provider",
                        unit.crate_name
                    ),
                ));
            }
            _ => false,
        };
        if !matches {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider environment",
                format!(
                    "unit {} emitted an environment value that differs from its admitted selection",
                    unit.crate_name
                ),
            ));
        }
        emitted_environment.insert(name, selected.clone());
    }
    let mut effects = OvenRuntimeFoundationProviderEffects {
        generated_inputs,
        emitted_cfg: directives.emitted_cfg,
        checked_cfg: directives.checked_cfg,
        emitted_environment,
        rerun_paths: directives.rerun_paths,
        rerun_environment: directives.rerun_environment,
        native_link: directives.native_link,
    };
    effects.emitted_cfg.sort();
    effects.checked_cfg.sort();
    effects.rerun_paths.sort();
    effects.rerun_environment.sort();
    if effects.emitted_cfg.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .emitted_cfg
            .iter()
            .any(|cfg| cfg.trim().is_empty() || !unit.cfg.contains(cfg))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider cfg",
            format!(
                "unit {} emitted cfg values absent from or repeated in the selected graph",
                unit.crate_name
            ),
        ));
    }
    if effects.checked_cfg.windows(2).any(|pair| pair[0] >= pair[1])
        || effects.checked_cfg.iter().any(|cfg| cfg.trim().is_empty())
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider checked cfg",
            format!("unit {} emitted empty or repeated check-cfg values", unit.crate_name),
        ));
    }
    if effects.rerun_paths.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .rerun_paths
            .iter()
            .any(|path| !runtime_foundation_provider_package_contains_path(package, path))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider rerun paths",
            format!(
                "unit {} emitted rerun paths absent from its declared package source members",
                unit.crate_name
            ),
        ));
    }
    if effects.rerun_environment.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .rerun_environment
            .iter()
            .any(|name| name.trim().is_empty() || !unit.environment.contains_key(name))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider rerun environment",
            format!(
                "unit {} emitted rerun environment names absent from the selected graph",
                unit.crate_name
            ),
        ));
    }
    Ok(effects)
}

impl OvenRuntimeFoundationProviderIntakeEvidence {
    /// Bind one source-producer record to the existing selected graph before it can become an Engine request.
    ///
    /// The caller still has to obtain package bytes from an immutable retained owner and rehash them before launch.
    /// This constructor proves the portable record's graph relationships first: it cannot name an unknown unit, a
    /// package root outside that unit's owner/compiler root, or a host dependency absent from its direct selected
    /// edges. The original declaration slot remains available only in the request sent to the source bridge.
    pub(crate) fn new(
        selected: &ValidatedOvenSelectedRustFacetGraph,
        selected_identity: String,
        package: OvenRuntimeFoundationProviderPackageSource,
        mut host_dependencies: Vec<OvenRuntimeFoundationProviderIntakeHostDependency>,
    ) -> Result<Self, OvenRustcError> {
        host_dependencies.sort_by_key(|dependency| dependency.declaration_index);
        let evidence = Self {
            selected_identity,
            package,
            host_dependencies,
        };
        evidence.validate_against(selected)?;
        Ok(evidence)
    }

    /// Return the selected unit correlation named by this retained source evidence.
    pub(crate) fn selected_identity(&self) -> &str {
        &self.selected_identity
    }

    /// Borrow the physical package evidence whose exact bytes later bind the request and publication.
    pub(crate) fn package(&self) -> &OvenRuntimeFoundationProviderPackageSource {
        &self.package
    }

    /// Validate this evidence against the one selected graph it is allowed to describe.
    fn validate_against(&self, selected: &ValidatedOvenSelectedRustFacetGraph) -> Result<(), OvenRustcError> {
        let units = runtime_foundation_selected_units(selected);
        let unit = units.get(self.selected_identity.as_str()).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider intake",
                "names no unit in the validated selected Rust graph",
            )
        })?;
        validate_runtime_foundation_provider_package_source(unit, &self.package)?;
        let host_dependencies = self.foundation_host_dependencies()?;
        validate_runtime_foundation_provider_host_dependencies(unit, &units, &host_dependencies)
    }

    /// Project source-request slots into the final alias/unit representation after checking canonical slot order.
    fn foundation_host_dependencies(&self) -> Result<Vec<OvenRuntimeFoundationProviderHostDependency>, OvenRustcError> {
        if self
            .host_dependencies
            .windows(2)
            .any(|pair| pair[0].declaration_index >= pair[1].declaration_index)
        {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider intake evidence",
                "host dependency declaration indices must be sorted and unique",
            ));
        }
        let mut dependencies = self
            .host_dependencies
            .iter()
            .map(|dependency| OvenRuntimeFoundationProviderHostDependency {
                alias: dependency.alias.clone(),
                unit: dependency.unit.clone(),
            })
            .collect::<Vec<_>>();
        dependencies.sort_by(|left, right| left.alias.cmp(&right.alias).then_with(|| left.unit.cmp(&right.unit)));
        Ok(dependencies)
    }
}

/// Encode the exact source request for one already authenticated provider-package evidence record.
///
/// This is intentionally only an encoder. It neither issues an Engine permit nor launches a child process. The
/// caller must hold the retained owner responsible for the manifest bytes and use the resulting exact request digest
/// in a dedicated one-use Engine operation. Supplying a manifest string with different bytes than the typed manifest
/// record refuses before the source layer can infer a false no-build-script declaration.
pub(crate) fn encode_runtime_foundation_provider_intake_request(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    evidence: &OvenRuntimeFoundationProviderIntakeEvidence,
    manifest_source: &str,
) -> Result<Vec<u8>, OvenRustcError> {
    evidence.validate_against(selected)?;
    let actual = digest_bytes(manifest_source.as_bytes());
    if actual != evidence.package.manifest.digest {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake manifest",
            "source bytes do not match the retained typed manifest digest",
        ));
    }
    serde_json::to_vec(&OvenRuntimeFoundationProviderIntakeWireRequest {
        schema: OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_REQUEST_SCHEMA,
        request: OvenRuntimeFoundationProviderIntakeWireRequestBody {
            selected_identity: &evidence.selected_identity,
            manifest: &evidence.package.manifest,
            manifest_source,
            package_members: &evidence.package.members,
            host_dependencies: &evidence.host_dependencies,
        },
    })
    .map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation provider intake request",
            format!("cannot encode closed source request: {error}"),
        )
    })
}

/// Index the one selected graph without inventing a second dependency relation for provider intake.
fn runtime_foundation_selected_units(
    selected: &ValidatedOvenSelectedRustFacetGraph,
) -> BTreeMap<&str, &OvenSelectedRustFacetUnit> {
    selected
        .graph()
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect()
}

/// Authenticate one Incan-authored provider declaration against retained request evidence and the selected Rust graph.
///
/// The source exchange decides only how `[package].build` and the literal `build-dependencies` declarations bind to
/// an explicitly supplied inventory. This adapter does not execute a script, discover a manifest, select a child,
/// admit a path, or create provider effects. The caller must already hold a matching Engine permit and request digest;
/// this function restores roots, manifest bytes and source inventory only from the opaque host evidence, then requires
/// every returned build-script member and host dependency to match it exactly.
pub(crate) fn decode_runtime_foundation_provider_intake(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    evidence: &OvenRuntimeFoundationProviderIntakeEvidence,
    response: &[u8],
) -> Result<OvenRuntimeFoundationProviderDeclaration, OvenRustcError> {
    let response =
        serde_json::from_slice::<OvenRuntimeFoundationProviderIntakeWireResponse>(response).map_err(|_| {
            runtime_foundation_invalid(
                "runtime foundation provider intake",
                "does not use the closed Incan provider-intake response schema",
            )
        })?;
    let (schema, returned_selected_identity, declaration, refused) = match response {
        OvenRuntimeFoundationProviderIntakeWireResponse::NoBuildScript {
            schema,
            selected_identity,
        } => (schema, selected_identity, None, false),
        OvenRuntimeFoundationProviderIntakeWireResponse::BuildScript {
            schema,
            selected_identity,
            declaration,
        } => (schema, selected_identity, Some(declaration), false),
        OvenRuntimeFoundationProviderIntakeWireResponse::Refused {
            schema,
            selected_identity,
            error,
        } => {
            let _ = (error.kind, error.fields);
            (schema, selected_identity, None, true)
        }
    };
    if schema != OVEN_RUNTIME_FOUNDATION_PROVIDER_INTAKE_SCHEMA {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "uses an unsupported source response schema",
        ));
    }
    if returned_selected_identity != evidence.selected_identity {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "does not match the host-requested selected unit",
        ));
    }
    evidence.validate_against(selected)?;
    let units = runtime_foundation_selected_units(selected);
    let unit = units.get(evidence.selected_identity.as_str()).ok_or_else(|| {
        runtime_foundation_invalid(
            "runtime foundation provider intake",
            "names no unit in the validated selected Rust graph",
        )
    })?;
    if refused {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "the source declaration was refused and cannot become a provider state",
        ));
    }
    let expected_host_dependencies = evidence.foundation_host_dependencies()?;
    let Some(declaration) = declaration else {
        if !expected_host_dependencies.is_empty() {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider intake",
                "a no-build-script response cannot discard retained host dependency evidence",
            ));
        }
        return Ok(OvenRuntimeFoundationProviderDeclaration::NoBuildScript {
            package: evidence.package.clone(),
        });
    };
    let mut package_members = declaration
        .package_members
        .into_iter()
        .map(|member| OvenSelectedRustFacetSourceMember {
            path: member.path,
            digest: member.digest,
        })
        .collect::<Vec<_>>();
    package_members.sort();
    if package_members != evidence.package.members {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "does not return the exact host-requested package source inventory",
        ));
    }
    let mut host_dependencies = declaration
        .host_dependencies
        .into_iter()
        .map(|dependency| OvenRuntimeFoundationProviderHostDependency {
            alias: dependency.alias,
            unit: dependency.unit,
        })
        .collect::<Vec<_>>();
    host_dependencies.sort_by(|left, right| left.alias.cmp(&right.alias).then_with(|| left.unit.cmp(&right.unit)));
    if host_dependencies != expected_host_dependencies {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider intake",
            "does not return the exact host-requested build dependency bindings",
        ));
    }
    let declaration = OvenRuntimeFoundationProviderDeclaration::BuildScript {
        package: evidence.package.clone(),
        entrypoint: declaration.entrypoint.path,
        digest: declaration.entrypoint.digest,
        edition: unit.edition.clone(),
        host_dependencies,
    };
    validate_runtime_foundation_provider_declaration(unit, &units, &declaration)?;
    Ok(declaration)
}
