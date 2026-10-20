//! Compiler-backed checked C binding inspection.
//!
//! `incan inspect bindings` projects C declaration facts from the checked typechecker descriptor. The shared checked
//! analysis runs the ordinary host-target C verifier, but this report is not a reusable verification receipt and does
//! not resolve native artifacts or infer bridge and facade roles from source layout.

use std::env;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use incan_lang::lang::c_abi::{link_capability_as_str, scalar_type_as_str};
use serde::Serialize;
use std::fmt::Write as _;

use crate::{CliError, CliResult, ExitCode};
use incan_driver::features::FeatureSelection;
use incan_frontend::ParsedModule;
use incan_frontend::ast::Span;
use incan_frontend::typechecker::{
    CBindingBuffer, CBindingDescriptor, CBindingOutcome, CBindingType, COutputMode, CResourceAccess,
    c_binding_descriptor_identity,
};

use incan_driver::modules::collect_modules_detailed_with_session;
use incan_driver::session::{CompilationAnalysis, CompilationSession, DEFAULT_PROFILE};

/// Compatibility version for the checked C binding inspection report.
const BINDING_INSPECTION_SCHEMA_VERSION: u32 = 2;

/// Output format for `incan inspect bindings`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingInspectionFormat {
    /// Concise declaration summary for terminal use.
    Text,
    /// Deterministic structured declaration facts for tools.
    Json,
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

/// Inspect checked C binding declarations for one source file or project root.
///
/// The command runs the ordinary compiler collection and typecheck path exactly once, including host-target C
/// verification, then projects only descriptors produced by that pass. It is strict: invalid source returns ordinary
/// compiler diagnostics instead of a partial report whose facts could be mistaken for a checked contract.
pub fn inspect_bindings(
    path: &Path,
    format: BindingInspectionFormat,
    feature_selection: &FeatureSelection,
) -> CliResult<ExitCode> {
    let path = std::path::absolute(path).map_err(|error| CliError::failure(error.to_string()))?;
    let entry_path = resolve_binding_entry_path(&path)?;
    let session = CompilationSession::discover(&entry_path, feature_selection, DEFAULT_PROFILE)?;
    let modules = collect_modules_detailed_with_session(entry_path.clone(), &session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let analysis = session
        .analyze_modules(&modules)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let report = binding_inspection_report(&modules, &analysis)?;
    print!("{}", render_binding_inspection_report(&report, format)?);
    Ok(ExitCode::SUCCESS)
}

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
) -> CliResult<String> {
    match format {
        BindingInspectionFormat::Json => serde_json::to_string_pretty(report)
            .map(|text| format!("{text}\n"))
            .map_err(|error| CliError::failure(format!("failed to serialize binding inspection: {error}"))),
        BindingInspectionFormat::Text => render_binding_inspection_text(report),
    }
}

/// Render the human-readable declaration summary through the existing report contract.
fn render_binding_inspection_text(report: &BindingInspectionReport) -> CliResult<String> {
    let mut rendered = String::new();
    if report.bindings.is_empty() {
        writeln!(&mut rendered, "No checked C bindings.").map_err(|error| CliError::failure(error.to_string()))?;
        return Ok(rendered);
    }
    for binding in &report.bindings {
        writeln!(
            &mut rendered,
            "Binding {} ({})",
            binding.name,
            binding.module.join("::")
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
        writeln!(&mut rendered, "  identity: {}", binding.identity)
            .map_err(|error| CliError::failure(error.to_string()))?;
        writeln!(&mut rendered, "  header: {}", binding.header)
            .map_err(|error| CliError::failure(error.to_string()))?;
        writeln!(
            &mut rendered,
            "  link: c.{}(\"{}\")",
            binding.link_capability, binding.system_library
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
        writeln!(
            &mut rendered,
            "  source: {}:{}:{}",
            binding.source.file, binding.source.start_line, binding.source.start_column
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
        for resource in &binding.resources {
            writeln!(
                &mut rendered,
                "  resource {} [native: {}, release: {}]",
                resource.name, resource.native, resource.release
            )
            .map_err(|error| CliError::failure(error.to_string()))?;
        }
        for symbol in &binding.symbols {
            let parameters = symbol
                .parameters
                .iter()
                .map(|parameter| format!("{}: {}", parameter.name, format_binding_type(&parameter.ty)))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                &mut rendered,
                "  symbol {}({parameters}) -> {} [native: {}]",
                symbol.name,
                format_binding_type(&symbol.return_type),
                symbol.native
            )
            .map_err(|error| CliError::failure(error.to_string()))?;
            for buffer in &symbol.buffers {
                writeln!(
                    &mut rendered,
                    "    bounds {} -> {} [element: {}]",
                    buffer.pointer_parameter, buffer.length_parameter, buffer.element
                )
                .map_err(|error| CliError::failure(error.to_string()))?;
            }
            for outcome in &symbol.outcomes {
                writeln!(
                    &mut rendered,
                    "    outcome {} [initializes: {}, updates: {}, invalidates: {}]",
                    outcome.result,
                    format_output_names(&outcome.initializes),
                    format_output_names(&outcome.updates),
                    format_output_names(&outcome.invalidates)
                )
                .map_err(|error| CliError::failure(error.to_string()))?;
            }
        }
        for enumeration in &binding.enums {
            writeln!(&mut rendered, "  enum {}: {}", enumeration.name, enumeration.carrier)
                .map_err(|error| CliError::failure(error.to_string()))?;
            for variant in &enumeration.variants {
                writeln!(&mut rendered, "    {} = {}", variant.name, variant.native)
                    .map_err(|error| CliError::failure(error.to_string()))?;
            }
        }
        for structure in &binding.structs {
            writeln!(
                &mut rendered,
                "  struct {} [native: {}]",
                structure.name, structure.native
            )
            .map_err(|error| CliError::failure(error.to_string()))?;
            for field in &structure.fields {
                writeln!(&mut rendered, "    {}: {}", field.name, format_binding_type(&field.ty))
                    .map_err(|error| CliError::failure(error.to_string()))?;
            }
        }
    }
    Ok(rendered)
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
