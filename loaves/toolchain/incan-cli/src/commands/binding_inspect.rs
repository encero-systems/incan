//! Compiler-backed checked C binding inspection.
//!
//! `incan inspect bindings` projects C declaration facts from the checked typechecker descriptor. The shared checked
//! analysis runs the ordinary host-target C verifier, but this report is not a reusable verification receipt and does
//! not resolve native artifacts or infer bridge and facade roles from source layout.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use incan_lang::lang::c_abi::{link_capability_as_str, scalar_type_as_str};
use serde::Serialize;

use crate::{CliError, CliResult, ExitCode};
use incan_driver::interop_plan::locked_interop_plan_target;
use incan_frontend::ParsedModule;
use incan_frontend::ast::{Span, Visibility};
use incan_frontend::typechecker::{
    CBindingBuffer, CBindingDescriptor, CBindingOutcome, CBindingType, COutputMode, CResourceAccess,
    c_binding_descriptor_identity,
};
use incan_provider::FeatureSelection;
use oven_model::oven_interop::locked_interop_target_identity;
use oven_rustc::interop::{
    default_interop_execution_receipt_path, load_interop_execution_receipt, validate_interop_execution_receipt,
};

use incan_driver::modules::collect_modules_detailed_with_session;
use incan_driver::session::{CompilationAnalysis, CompilationSession};

/// Compatibility version for the checked C binding inspection report.
const BINDING_INSPECTION_SCHEMA_VERSION: u32 = 2;

/// Compatibility version for the redaction-safe checked binding-use receipt.
const BINDING_USAGE_RECEIPT_SCHEMA_VERSION: u32 = 2;

/// Output format for `incan inspect bindings`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingInspectionFormat {
    /// Concise declaration summary for terminal use.
    Text,
    /// Deterministic structured declaration facts for tools.
    Json,
    /// Redaction-safe receipt of checked binding and raw-call usage, optionally joined to a locked Oven target.
    Receipt,
}

/// Stable checked C binding declaration report.
#[derive(Debug, Serialize)]
struct BindingInspectionReport {
    schema_version: u32,
    bindings: Vec<BindingReport>,
}

/// One C binding declaration as retained by successful typechecking.
#[derive(Debug, Serialize)]
struct BindingReport {
    module: Vec<String>,
    /// Relocation-stable compiler identity for the complete checked descriptor contract.
    identity: String,
    name: String,
    header: String,
    system_library: String,
    link_capability: String,
    source: BindingSourceSpan,
    resources: Vec<ResourceReport>,
    symbols: Vec<SymbolReport>,
    enums: Vec<EnumReport>,
    structs: Vec<StructReport>,
}

/// One nominal opaque resource and its binding-local release association.
#[derive(Debug, Serialize)]
struct ResourceReport {
    name: String,
    native: String,
    release: String,
}

/// One declaration-level C symbol contract.
#[derive(Debug, Serialize)]
struct SymbolReport {
    name: String,
    native: String,
    parameters: Vec<ParameterReport>,
    return_type: BindingTypeReport,
    buffers: Vec<BufferReport>,
    outcomes: Vec<OutcomeReport>,
}

/// One named C parameter contract.
#[derive(Debug, Serialize)]
struct ParameterReport {
    name: String,
    #[serde(rename = "type")]
    ty: BindingTypeReport,
}

/// One descriptor-owned checked pointer-to-length association for a bounded span.
#[derive(Debug, Serialize)]
struct BufferReport {
    pointer_parameter: String,
    length_parameter: String,
    element: String,
}

/// One declared result path and its output-slot state transitions.
#[derive(Debug, Serialize)]
struct OutcomeReport {
    result: String,
    initializes: Vec<String>,
    updates: Vec<String>,
    invalidates: Vec<String>,
}

/// One C enum declaration and its target-verified carrier contract.
#[derive(Debug, Serialize)]
struct EnumReport {
    name: String,
    carrier: String,
    variants: Vec<EnumVariantReport>,
}

/// One declared native enum constant spelling.
#[derive(Debug, Serialize)]
struct EnumVariantReport {
    name: String,
    native: String,
}

/// One plain C structure declaration.
#[derive(Debug, Serialize)]
struct StructReport {
    name: String,
    native: String,
    fields: Vec<StructFieldReport>,
}

/// One declared plain C structure field.
#[derive(Debug, Serialize)]
struct StructFieldReport {
    name: String,
    #[serde(rename = "type")]
    ty: BindingTypeReport,
}

/// A C type in the checked binding vocabulary, represented structurally rather than as generated Rust text.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BindingTypeReport {
    Scalar {
        spelling: String,
    },
    Pointer {
        mutable: bool,
        pointee: Box<BindingTypeReport>,
    },
    Struct {
        name: String,
    },
    Resource {
        access: ResourceAccessReport,
        resource: String,
    },
    Output {
        mode: OutputModeReport,
        value: Box<BindingTypeReport>,
    },
    Nullable {
        value: Box<BindingTypeReport>,
    },
    Void,
}

/// Stable inspection spelling for an opaque resource's checked access mode.
#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ResourceAccessReport {
    Owned,
    Borrowed,
    BorrowedMut,
}

/// Stable inspection spelling for one compiler-managed output position.
#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum OutputModeReport {
    Out,
    InOut,
}

/// A byte- and editor-addressable source span for one binding declaration.
#[derive(Debug, Serialize)]
struct BindingSourceSpan {
    file: String,
    start: usize,
    end: usize,
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
}

/// Portable receipt containing only checked C binding usage facts safe to retain outside a source checkout.
#[derive(Debug, Serialize)]
struct BindingUsageReceipt {
    schema_version: u32,
    compatibility: BindingUsageCompatibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<BindingUsageTarget>,
    bindings: Vec<BindingUsageBinding>,
    calls: Vec<BindingUsageCall>,
    facades: Vec<BindingUsageFacade>,
}

/// Explicit v0.5 comparison policy for retained checked C binding provenance.
///
/// The initial policy is intentionally exact: a different descriptor or locked target identity is a changed ABI,
/// ownership, target, or artifact contract until a later compiler-owned compatibility rule can prove otherwise.
#[derive(Debug, Serialize)]
struct BindingUsageCompatibility {
    binding_contract: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_contract: Option<&'static str>,
}

/// Target selection identity joined to a checked binding-use receipt without serializing a local package path.
#[derive(Debug, Serialize)]
struct BindingUsageTarget {
    target: String,
    locked_target_identity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_execution_identity: Option<String>,
    /// Explicit target-artifact correspondence keyed by the checked binding declaration identity in source terms.
    #[serde(skip)]
    binding_artifacts: BTreeMap<(Vec<String>, String), Vec<String>>,
}

/// One compiler-owned checked descriptor identity used by the receipt.
#[derive(Debug, Serialize)]
struct BindingUsageBinding {
    module: Vec<String>,
    name: String,
    identity: String,
    /// Package-authored target-artifact names retained only when the selected target declares this exact binding.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    target_artifacts: Vec<String>,
}

/// One checked direct native symbol use without source offsets, values, paths, or process-local addresses.
#[derive(Debug, Serialize)]
struct BindingUsageCall {
    binding_identity: String,
    symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<BindingUsageOwner>,
}

/// Stable callable identity associated with a checked raw call.
#[derive(Debug, Serialize)]
struct BindingUsageOwner {
    name: String,
    visibility: String,
}

/// One compiler-proven public facade and private raw bridge, without source locations or local paths.
#[derive(Debug, Serialize)]
struct BindingUsageFacade {
    facade: BindingUsageOwner,
    bridge: BindingUsageOwner,
    calls: Vec<BindingUsageFacadeCall>,
}

/// One direct raw C call that the facade's bridge owns.
#[derive(Debug, Serialize)]
struct BindingUsageFacadeCall {
    binding_identity: String,
    symbol: String,
}

/// Inspect checked C binding declarations for one source file or project root.
///
/// The command runs the ordinary compiler collection and typecheck path exactly once, including host-target C
/// verification, then projects only descriptors produced by that pass. It is strict: invalid source returns ordinary
/// compiler diagnostics instead of a partial report whose facts could be mistaken for a checked contract.
pub fn inspect_bindings(
    path: &Path,
    format: BindingInspectionFormat,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    target: Option<&str>,
) -> CliResult<ExitCode> {
    if target.is_some() && format != BindingInspectionFormat::Receipt {
        return Err(CliError::failure(
            "`incan inspect bindings --target` requires `--format receipt`",
        ));
    }
    let entry_path = resolve_binding_entry_path(path)?;
    let session = CompilationSession::discover_with_selections(&entry_path, feature_selection, sdk_profile_override)?;
    let modules = collect_modules_detailed_with_session(entry_path.clone(), &session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let analysis = session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            None,
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    if format == BindingInspectionFormat::Receipt {
        let target = target
            .map(|target| binding_usage_target(&entry_path, target))
            .transpose()?;
        let receipt = binding_usage_receipt(&modules, &analysis, target)?;
        return render_binding_usage_receipt(&receipt);
    }
    let report = binding_inspection_report(&modules, &analysis)?;
    render_binding_inspection_report(&report, format)
}

/// Join an optional lock-fresh Oven target to a binding-use receipt without retaining project-local paths.
fn binding_usage_target(entry_path: &Path, target: &str) -> CliResult<BindingUsageTarget> {
    let locked = locked_interop_plan_target(entry_path, target)?;
    let receipt_path = default_interop_execution_receipt_path(&locked.project_root, &locked.target.target);
    let selected_execution_identity = if receipt_path.is_file() {
        let receipt = load_interop_execution_receipt(&receipt_path).map_err(CliError::failure)?;
        validate_interop_execution_receipt(&locked.target, &receipt).map_err(CliError::failure)?;
        Some(receipt.identity)
    } else {
        None
    };
    Ok(BindingUsageTarget {
        target: locked.target.target.clone(),
        locked_target_identity: locked_interop_target_identity(&locked.target).map_err(CliError::failure)?,
        selected_execution_identity,
        binding_artifacts: locked
            .target
            .bindings
            .iter()
            .map(|binding| {
                (
                    (binding.module.clone(), binding.name.clone()),
                    binding.artifacts.clone(),
                )
            })
            .collect(),
    })
}

/// Project checked descriptors and raw calls into a deliberately redacted, deterministic receipt.
fn binding_usage_receipt(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    target: Option<BindingUsageTarget>,
) -> CliResult<BindingUsageReceipt> {
    let mut bindings = Vec::new();
    let mut identities_by_module_and_name = BTreeMap::new();
    let target_binding_artifacts = target
        .as_ref()
        .map(|target| target.binding_artifacts.clone())
        .unwrap_or_default();
    for module in modules {
        let type_info = analysis
            .type_info_for_module_path(&module.path_segments)
            .ok_or_else(|| CliError::failure(format!("missing session analysis for {}", module.file_path.display())))?;
        for descriptor in type_info.c_abi.bindings.values() {
            let identity = c_binding_descriptor_identity(&module.path_segments, descriptor);
            let key = (module.path_segments.clone(), descriptor.class_name.clone());
            if let Some(existing) = identities_by_module_and_name.insert(key.clone(), identity.clone())
                && existing != identity
            {
                return Err(CliError::failure(format!(
                    "checked C binding `{}` has conflicting descriptor identities in module `{}`",
                    descriptor.class_name,
                    module.path_segments.join("::")
                )));
            }
            bindings.push(BindingUsageBinding {
                module: module.path_segments.clone(),
                name: descriptor.class_name.clone(),
                identity,
                target_artifacts: target_binding_artifacts.get(&key).cloned().unwrap_or_default(),
            });
        }
    }
    for (module, name) in target_binding_artifacts.keys() {
        if !identities_by_module_and_name.contains_key(&(module.clone(), name.clone())) {
            return Err(CliError::failure(format!(
                "locked Oven interop target declares binding-artifact correspondence for `{}::{name}` but compilation did not produce that checked binding",
                module.join("::")
            )));
        }
    }
    let mut calls = Vec::new();
    let mut facades = Vec::new();
    for module in modules {
        let type_info = analysis
            .type_info_for_module_path(&module.path_segments)
            .ok_or_else(|| CliError::failure(format!("missing session analysis for {}", module.file_path.display())))?;
        for raw_call in &type_info.c_abi.raw_calls {
            let identity = identities_by_module_and_name
                .get(&(module.path_segments.clone(), raw_call.binding.clone()))
                .ok_or_else(|| {
                    CliError::failure(format!(
                        "checked C raw call `{}.{}` has no descriptor identity in module `{}`",
                        raw_call.binding,
                        raw_call.symbol,
                        module.path_segments.join("::")
                    ))
                })?;
            calls.push(BindingUsageCall {
                binding_identity: identity.clone(),
                symbol: raw_call.symbol.clone(),
                owner: raw_call.owner.as_ref().map(binding_usage_owner),
            });
        }
        for facade in &type_info.c_abi.facades {
            let mut facade_calls = Vec::new();
            for raw_call in type_info
                .c_abi
                .raw_calls
                .iter()
                .filter(|raw_call| raw_call.owner.as_ref() == Some(&facade.bridge))
            {
                let identity = identities_by_module_and_name
                    .get(&(module.path_segments.clone(), raw_call.binding.clone()))
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "checked C facade bridge `{}.{}` has no descriptor identity in module `{}`",
                            raw_call.binding,
                            raw_call.symbol,
                            module.path_segments.join("::")
                        ))
                    })?;
                facade_calls.push(BindingUsageFacadeCall {
                    binding_identity: identity.clone(),
                    symbol: raw_call.symbol.clone(),
                });
            }
            facade_calls.sort_by(|left, right| {
                left.binding_identity
                    .cmp(&right.binding_identity)
                    .then_with(|| left.symbol.cmp(&right.symbol))
            });
            facade_calls
                .dedup_by(|left, right| left.binding_identity == right.binding_identity && left.symbol == right.symbol);
            if facade_calls.is_empty() {
                continue;
            }
            facades.push(BindingUsageFacade {
                facade: binding_usage_owner(&facade.facade),
                bridge: binding_usage_owner(&facade.bridge),
                calls: facade_calls,
            });
        }
    }
    bindings.sort_by(|left, right| left.module.cmp(&right.module).then_with(|| left.name.cmp(&right.name)));
    calls.sort_by(|left, right| {
        left.binding_identity
            .cmp(&right.binding_identity)
            .then_with(|| left.symbol.cmp(&right.symbol))
            .then_with(|| {
                left.owner
                    .as_ref()
                    .map(|owner| &owner.name)
                    .cmp(&right.owner.as_ref().map(|owner| &owner.name))
            })
    });
    facades.sort_by(|left, right| {
        left.facade
            .name
            .cmp(&right.facade.name)
            .then_with(|| left.bridge.name.cmp(&right.bridge.name))
            .then_with(|| left.calls.len().cmp(&right.calls.len()))
    });
    Ok(BindingUsageReceipt {
        schema_version: BINDING_USAGE_RECEIPT_SCHEMA_VERSION,
        compatibility: BindingUsageCompatibility {
            binding_contract: "exact_descriptor_identity",
            target_contract: target.as_ref().map(|_| "exact_locked_target_identity"),
        },
        target,
        bindings,
        calls,
        facades,
    })
}

/// Serialize the redaction-safe receipt without adding a second text contract for provenance tooling.
fn render_binding_usage_receipt(receipt: &BindingUsageReceipt) -> CliResult<ExitCode> {
    println!(
        "{}",
        serde_json::to_string_pretty(receipt)
            .map_err(|error| CliError::failure(format!("failed to serialize binding usage receipt: {error}")))?
    );
    Ok(ExitCode::SUCCESS)
}

/// Render one source-visibility fact without requiring receipt consumers to parse compiler syntax.
fn binding_owner_visibility(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Private => "private",
        Visibility::Public => "public",
    }
}

/// Project a compiler-owned raw-call owner without retaining its source location.
fn binding_usage_owner(owner: &incan_frontend::typechecker::CBindingRawCallOwner) -> BindingUsageOwner {
    BindingUsageOwner {
        name: owner.name.clone(),
        visibility: binding_owner_visibility(owner.visibility).to_string(),
    }
}

/// Resolve either an explicit Incan file or the ordinary package entrypoint used for binding inspection.
fn resolve_binding_entry_path(path: &Path) -> CliResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path)
    };
    if absolute.is_file() {
        return Ok(absolute);
    }
    if absolute.is_dir() {
        for candidate in [absolute.join("src/main.incn"), absolute.join("src/lib.incn")] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        return Err(CliError::failure(format!(
            "binding inspection requires an Incan source file, or a project directory with `src/main.incn` or `src/lib.incn`: {}",
            absolute.display()
        )));
    }
    Err(CliError::failure(format!(
        "binding inspection path does not exist: {}",
        absolute.display()
    )))
}

/// Project all checked binding descriptors from one successful shared compilation analysis.
fn binding_inspection_report(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
) -> CliResult<BindingInspectionReport> {
    let mut bindings = Vec::new();
    for module in modules {
        let type_info = analysis
            .type_info_for_module_path(&module.path_segments)
            .ok_or_else(|| CliError::failure(format!("missing session analysis for {}", module.file_path.display())))?;
        for descriptor in type_info.c_abi.bindings.values() {
            bindings.push(binding_report(module, descriptor));
        }
    }
    bindings.sort_by(|left, right| {
        left.module
            .cmp(&right.module)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.source.file.cmp(&right.source.file))
    });
    Ok(BindingInspectionReport {
        schema_version: BINDING_INSPECTION_SCHEMA_VERSION,
        bindings,
    })
}

/// Convert one checked descriptor into the stable tooling projection without reinterpreting the source AST.
fn binding_report(module: &ParsedModule, descriptor: &CBindingDescriptor) -> BindingReport {
    BindingReport {
        module: module.path_segments.clone(),
        identity: c_binding_descriptor_identity(&module.path_segments, descriptor),
        name: descriptor.class_name.clone(),
        header: descriptor.header.clone(),
        system_library: descriptor.system_library.clone(),
        link_capability: link_capability_as_str(descriptor.link_capability).to_string(),
        source: binding_source_span(&module.file_path, &module.source, descriptor.span),
        resources: descriptor
            .resources
            .iter()
            .map(|resource| ResourceReport {
                name: resource.name.clone(),
                native: resource.native.clone(),
                release: resource.release.clone(),
            })
            .collect(),
        symbols: descriptor
            .symbols
            .iter()
            .map(|symbol| SymbolReport {
                name: symbol.name.clone(),
                native: symbol.native.clone(),
                parameters: symbol
                    .parameters
                    .iter()
                    .map(|parameter| ParameterReport {
                        name: parameter.name.clone(),
                        ty: binding_type_report(&parameter.ty),
                    })
                    .collect(),
                return_type: binding_type_report(&symbol.return_type),
                buffers: symbol.buffers.iter().map(buffer_report).collect(),
                outcomes: symbol.outcomes.iter().map(outcome_report).collect(),
            })
            .collect(),
        enums: descriptor
            .enums
            .iter()
            .map(|enumeration| EnumReport {
                name: enumeration.name.clone(),
                carrier: scalar_type_as_str(enumeration.carrier).to_string(),
                variants: enumeration
                    .variants
                    .iter()
                    .map(|variant| EnumVariantReport {
                        name: variant.name.clone(),
                        native: variant.native.clone(),
                    })
                    .collect(),
            })
            .collect(),
        structs: descriptor
            .structs
            .iter()
            .map(|structure| StructReport {
                name: structure.name.clone(),
                native: structure.native.clone(),
                fields: structure
                    .fields
                    .iter()
                    .map(|field| StructFieldReport {
                        name: field.name.clone(),
                        ty: binding_type_report(&field.ty),
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Project one compiler-owned pointer-to-length association without inferring it from names or generated Rust.
fn buffer_report(buffer: &CBindingBuffer) -> BufferReport {
    BufferReport {
        pointer_parameter: buffer.pointer_parameter.clone(),
        length_parameter: buffer.length_parameter.clone(),
        element: scalar_type_as_str(buffer.element).to_string(),
    }
}

/// Project one checked output-state declaration without reinterpreting its result spelling.
fn outcome_report(outcome: &CBindingOutcome) -> OutcomeReport {
    OutcomeReport {
        result: outcome.result.clone(),
        initializes: outcome.initializes.clone(),
        updates: outcome.updates.clone(),
        invalidates: outcome.invalidates.clone(),
    }
}

/// Translate the checked typechecker representation into a stable, source-vocabulary-oriented report value.
fn binding_type_report(ty: &CBindingType) -> BindingTypeReport {
    match ty {
        CBindingType::Scalar(scalar) => BindingTypeReport::Scalar {
            spelling: scalar_type_as_str(*scalar).to_string(),
        },
        CBindingType::Pointer { mutable, pointee } => BindingTypeReport::Pointer {
            mutable: *mutable,
            pointee: Box::new(binding_type_report(pointee)),
        },
        CBindingType::Struct(name) => BindingTypeReport::Struct { name: name.clone() },
        CBindingType::Resource { access, resource } => BindingTypeReport::Resource {
            access: resource_access_report(*access),
            resource: resource.clone(),
        },
        CBindingType::Output { mode, value } => BindingTypeReport::Output {
            mode: output_mode_report(*mode),
            value: Box::new(binding_type_report(value)),
        },
        CBindingType::Nullable(value) => BindingTypeReport::Nullable {
            value: Box::new(binding_type_report(value)),
        },
        CBindingType::Void => BindingTypeReport::Void,
    }
}

/// Project a resource argument's checked access mode into its stable report enum.
fn resource_access_report(access: CResourceAccess) -> ResourceAccessReport {
    match access {
        CResourceAccess::Owned => ResourceAccessReport::Owned,
        CResourceAccess::Borrowed => ResourceAccessReport::Borrowed,
        CResourceAccess::BorrowedMut => ResourceAccessReport::BorrowedMut,
    }
}

/// Project a compiler-managed output position into its stable report enum.
fn output_mode_report(mode: COutputMode) -> OutputModeReport {
    match mode {
        COutputMode::Out => OutputModeReport::Out,
        COutputMode::InOut => OutputModeReport::InOut,
    }
}

/// Render one stable report as either machine-readable JSON or a concise terminal summary.
fn render_binding_inspection_report(
    report: &BindingInspectionReport,
    format: BindingInspectionFormat,
) -> CliResult<ExitCode> {
    match format {
        BindingInspectionFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(report)
                .map_err(|error| CliError::failure(format!("failed to serialize binding inspection: {error}")))?
        ),
        BindingInspectionFormat::Text => render_binding_inspection_text(report),
        BindingInspectionFormat::Receipt => {
            return Err(CliError::failure(
                "binding usage receipts must be rendered before declaration reports",
            ));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Print the human-readable declaration summary without creating a second serialization contract.
fn render_binding_inspection_text(report: &BindingInspectionReport) {
    if report.bindings.is_empty() {
        println!("No checked C bindings.");
        return;
    }
    for binding in &report.bindings {
        println!("Binding {} ({})", binding.name, binding.module.join("::"));
        println!("  identity: {}", binding.identity);
        println!("  header: {}", binding.header);
        println!("  link: c.{}(\"{}\")", binding.link_capability, binding.system_library);
        println!(
            "  source: {}:{}:{}",
            binding.source.file, binding.source.start_line, binding.source.start_column
        );
        for resource in &binding.resources {
            println!(
                "  resource {} [native: {}, release: {}]",
                resource.name, resource.native, resource.release
            );
        }
        for symbol in &binding.symbols {
            let parameters = symbol
                .parameters
                .iter()
                .map(|parameter| format!("{}: {}", parameter.name, format_binding_type(&parameter.ty)))
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "  symbol {}({parameters}) -> {} [native: {}]",
                symbol.name,
                format_binding_type(&symbol.return_type),
                symbol.native
            );
            for buffer in &symbol.buffers {
                println!(
                    "    bounds {} -> {} [element: {}]",
                    buffer.pointer_parameter, buffer.length_parameter, buffer.element
                );
            }
            for outcome in &symbol.outcomes {
                println!(
                    "    outcome {} [initializes: {}, updates: {}, invalidates: {}]",
                    outcome.result,
                    format_output_names(&outcome.initializes),
                    format_output_names(&outcome.updates),
                    format_output_names(&outcome.invalidates)
                );
            }
        }
        for enumeration in &binding.enums {
            println!("  enum {}: {}", enumeration.name, enumeration.carrier);
            for variant in &enumeration.variants {
                println!("    {} = {}", variant.name, variant.native);
            }
        }
        for structure in &binding.structs {
            println!("  struct {} [native: {}]", structure.name, structure.native);
            for field in &structure.fields {
                println!("    {}: {}", field.name, format_binding_type(&field.ty));
            }
        }
    }
}

/// Format output-position names for the concise terminal report.
fn format_output_names(names: &[String]) -> String {
    if names.is_empty() {
        return "-".to_string();
    }
    names.join(", ")
}

/// Format one structured binding type with the corresponding Incan C vocabulary spelling for terminal output.
fn format_binding_type(ty: &BindingTypeReport) -> String {
    match ty {
        BindingTypeReport::Scalar { spelling } => spelling.clone(),
        BindingTypeReport::Pointer { mutable, pointee } => {
            let constructor = if *mutable { "c.MutPtr" } else { "c.ConstPtr" };
            format!("{constructor}[{}]", format_binding_type(pointee))
        }
        BindingTypeReport::Struct { name } => name.clone(),
        BindingTypeReport::Resource { access, resource } => {
            let constructor = match access {
                ResourceAccessReport::Owned => "c.Owned",
                ResourceAccessReport::Borrowed => "c.Borrowed",
                ResourceAccessReport::BorrowedMut => "c.BorrowedMut",
            };
            format!("{constructor}[{resource}]")
        }
        BindingTypeReport::Output { mode, value } => {
            let constructor = match mode {
                OutputModeReport::Out => "c.Out",
                OutputModeReport::InOut => "c.InOut",
            };
            format!("{constructor}[{}]", format_binding_type(value))
        }
        BindingTypeReport::Nullable { value } => format!("Option[{}]", format_binding_type(value)),
        BindingTypeReport::Void => "None".to_string(),
    }
}

/// Convert the compiler's byte span into a deterministic source anchor for external tooling.
fn binding_source_span(path: &Path, source: &str, span: Span) -> BindingSourceSpan {
    let start = span.start.min(source.len());
    let end = span.end.min(source.len()).max(start);
    let (start_line, start_column) = line_column_for_offset(source, start);
    let (end_line, end_column) = line_column_for_offset(source, end);
    BindingSourceSpan {
        file: path.to_string_lossy().to_string(),
        start,
        end,
        start_line,
        start_column,
        end_line,
        end_column,
    }
}

/// Convert a byte offset into 1-based display coordinates while preserving byte offsets for precise tool anchors.
fn line_column_for_offset(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let mut line = 1usize;
    let mut column = 1usize;
    for (index, character) in source.char_indices() {
        if index >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}
