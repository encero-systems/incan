//! Target-aware Clang verification for the bounded checked C binding foundation.
//!
//! The frontend owns C binding semantics. This module deliberately receives only a checked descriptor and renders a
//! non-executable C translation unit for one selected target. It never reads headers to discover symbols, guesses a
//! signature, or searches the host for a matching library.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::frontend::typechecker::{CBindingDescriptor, CBindingEnum, CBindingStruct, CBindingType};
use crate::oven_interop::{InteropTargetPlatform, IosTargetKind, OvenInteropTarget, ios_target_kind};
use incan_core::lang::c_abi::ScalarTypeId;

type EnumValueProbeRequest = (String, String, String);

/// A Clang-compatible target supplied by the checked C foundation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CAbiTarget {
    /// GNU-compatible Linux x86-64 ABI.
    LinuxX86_64,
    /// Apple arm64 ABI.
    MacosArm64,
    /// Android arm64 ABI with its NDK API level.
    AndroidArm64 {
        /// Android API level selected for the Clang target triple.
        api_level: u32,
    },
    /// iOS arm64 ABI with its minimum deployment target and device/simulator SDK selection.
    IosArm64 {
        /// Minimum iOS version selected for the Clang target triple.
        deployment_target: String,
        /// Whether verification targets the simulator ABI rather than a physical device ABI.
        simulator: bool,
    },
}

impl CAbiTarget {
    /// Stable target triple passed to Clang.
    pub(crate) fn triple(&self) -> String {
        match self {
            Self::LinuxX86_64 => "x86_64-unknown-linux-gnu".to_string(),
            Self::MacosArm64 => "arm64-apple-macos11".to_string(),
            Self::AndroidArm64 { api_level } => format!("aarch64-linux-android{api_level}"),
            Self::IosArm64 {
                deployment_target,
                simulator: false,
            } => format!("arm64-apple-ios{deployment_target}"),
            Self::IosArm64 {
                deployment_target,
                simulator: true,
            } => format!("arm64-apple-ios{deployment_target}-simulator"),
        }
    }

    /// Target that matches the compiler host running this invocation.
    pub(crate) fn host() -> Option<Self> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            return Some(Self::MacosArm64);
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            return Some(Self::LinuxX86_64);
        }
        #[allow(unreachable_code)]
        None
    }

    /// Translate one checked Oven interop target declaration into Clang's exact ABI target spelling.
    fn from_interop_target(interop_target: &OvenInteropTarget) -> Result<Self, String> {
        match (&interop_target.target[..], interop_target.platform.as_ref()) {
            ("x86_64-unknown-linux-gnu", None) => Ok(Self::LinuxX86_64),
            ("aarch64-apple-darwin", None) => Ok(Self::MacosArm64),
            ("aarch64-linux-android", Some(InteropTargetPlatform::Android { api_level })) => {
                Ok(Self::AndroidArm64 { api_level: *api_level })
            }
            (_, Some(InteropTargetPlatform::Ios { deployment_target })) => {
                let Some(kind) = ios_target_kind(&interop_target.target) else {
                    return Err(format!(
                        "Oven interop target `{}` has iOS platform facts but is not an iOS arm64 target",
                        interop_target.target
                    ));
                };
                Ok(Self::IosArm64 {
                    deployment_target: deployment_target.clone(),
                    simulator: matches!(kind, IosTargetKind::Simulator),
                })
            }
            (_, Some(InteropTargetPlatform::Android { .. })) => Err(format!(
                "Oven interop target `{}` has Android platform facts but is not an Android arm64 target",
                interop_target.target
            )),
            _ => Err(format!(
                "Oven interop target `{}` is not supported by checked C ABI verification; declare one of `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `aarch64-linux-android` with Android platform facts, or `aarch64-apple-ios`/`aarch64-apple-ios-sim` with iOS platform facts",
                interop_target.target
            )),
        }
    }

    /// Return whether this target needs Android's NDK-owned Clang wrapper rather than a host toolchain.
    fn is_android(&self) -> bool {
        matches!(self, Self::AndroidArm64 { .. })
    }

    /// Return whether this target needs the iPhoneOS SDK sysroot.
    #[cfg(target_os = "macos")]
    fn is_ios(&self) -> bool {
        matches!(self, Self::IosArm64 { .. })
    }

    /// Return whether this iOS target must select Xcode's simulator SDK rather than the device SDK.
    #[cfg(target_os = "macos")]
    fn is_ios_simulator(&self) -> bool {
        matches!(self, Self::IosArm64 { simulator: true, .. })
    }
}

/// Checked C ABI verification inputs selected by either the host or one declared Oven interop target.
///
/// This is deliberately narrower than a build target: it supplies only the ABI target and preprocessor facts needed
/// to syntax-check a source-owned C declaration. It neither selects Rust's compilation target nor stages a native
/// artifact for a mobile package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CAbiVerificationPlan {
    target: CAbiTarget,
    definitions: Vec<String>,
    toolchain_identity: Option<String>,
}

impl CAbiVerificationPlan {
    /// Select host-target verification without importing any package-specific physical inputs.
    pub(crate) fn host() -> Option<Self> {
        CAbiTarget::host().map(|target| Self {
            target,
            definitions: Vec::new(),
            toolchain_identity: None,
        })
    }

    /// Select ABI verification from one manifest interop target whose package requirements have already been validated.
    pub(crate) fn from_interop_target(interop_target: &OvenInteropTarget) -> Result<Self, String> {
        Ok(Self {
            target: CAbiTarget::from_interop_target(interop_target)?,
            definitions: interop_target.definitions.clone(),
            toolchain_identity: interop_target
                .toolchain
                .as_ref()
                .map(|requirement| requirement.capability.clone()),
        })
    }

    /// Return the exact Clang target selected for this verifier pass.
    pub(crate) fn target(&self) -> &CAbiTarget {
        &self.target
    }

    /// Return explicit target-local C definitions supplied by the checked manifest.
    fn definitions(&self) -> &[String] {
        &self.definitions
    }

    /// Return the declared toolchain capability when this plan came from an Oven interop target declaration.
    fn toolchain_identity(&self) -> Option<&str> {
        self.toolchain_identity.as_deref()
    }
}

/// Explicit Clang executable used for one verifier invocation.
///
/// Oven will eventually provide this capability through a selected Loaf target. The first checked-binding slice
/// keeps that policy out of source declarations while accepting an explicit test/CI override and the platform's
/// Clang-compatible toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClangToolchain {
    executable: PathBuf,
    arguments: Vec<OsString>,
}

impl ClangToolchain {
    /// Select the current platform's Clang-compatible compiler without consulting binding names or headers.
    pub(crate) fn discover(plan: &CAbiVerificationPlan) -> Result<Self, CAbiVerificationError> {
        if let Some(executable) = env::var_os("INCAN_C_ABI_CLANG").filter(|value| !value.is_empty()) {
            return Ok(Self {
                executable: PathBuf::from(executable),
                arguments: Vec::new(),
            });
        }
        if plan.target().is_android() {
            return Err(CAbiVerificationError::toolchain(android_toolchain_message(plan)));
        }
        #[cfg(target_os = "macos")]
        {
            let sdk = if plan.target().is_ios_simulator() {
                "iphonesimulator"
            } else if plan.target().is_ios() {
                "iphoneos"
            } else {
                "macosx"
            };
            let executable = xcrun_value(Some(sdk), &["--find", "clang"], "Xcode Clang")?;
            let sysroot_description = if plan.target().is_ios_simulator() {
                "iPhoneSimulator SDK"
            } else if plan.target().is_ios() {
                "iPhoneOS SDK"
            } else {
                "macOS SDK"
            };
            let sysroot = xcrun_value(Some(sdk), &["--show-sdk-path"], sysroot_description)?;
            let arguments = vec![OsString::from("-isysroot"), OsString::from(sysroot)];
            Ok(Self {
                executable: PathBuf::from(executable),
                arguments,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Self {
                executable: PathBuf::from("clang"),
                arguments: Vec::new(),
            })
        }
    }

    /// Construct a test-only toolchain with an explicit executable path.
    #[cfg(test)]
    fn at(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            arguments: Vec::new(),
        }
    }
}

/// Read one Xcode-selected path or capability without treating its value as a package declaration.
#[cfg(target_os = "macos")]
fn xcrun_value(sdk: Option<&str>, arguments: &[&str], description: &str) -> Result<String, CAbiVerificationError> {
    let mut command = Command::new("xcrun");
    if let Some(sdk) = sdk {
        command.args(["--sdk", sdk]);
    }
    let output = command
        .args(arguments)
        .output()
        .map_err(|error| CAbiVerificationError::toolchain(format!("could not select {description}: {error}")))?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !value.is_empty() {
            return Ok(value);
        }
    }
    Err(CAbiVerificationError::toolchain(format!(
        "could not select {description}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Explain the current explicit Android toolchain boundary without inventing an ambient NDK discovery policy.
fn android_toolchain_message(plan: &CAbiVerificationPlan) -> String {
    let identity = plan.toolchain_identity().unwrap_or("Android NDK toolchain");
    format!(
        "declared Android toolchain capability `{identity}` is not provisioned by `incan check` yet; set `INCAN_C_ABI_CLANG` to its NDK Clang executable for this verification. Oven will resolve compatible toolchain requirements in its interop build path."
    )
}

/// One source-anchorable verifier failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CAbiVerificationError {
    /// Binding that could not be checked, when known.
    pub(crate) binding: Option<String>,
    /// Human-readable reason safe to present as an Incan diagnostic.
    pub(crate) message: String,
}

/// Target-verified C enum values consumed by the ordinary Incan lowering path.
///
/// The C probe is authoritative: generated Rust receives only the folded scalar after target verification, never a
/// guessed header spelling or macro expansion from the binding source.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct CAbiVerificationReceipt {
    enum_values: BTreeMap<(String, String), i64>,
}

impl CAbiVerificationReceipt {
    /// Return one verified value by its binding-local enum and variant names.
    #[cfg(test)]
    pub(crate) fn enum_value(&self, enumeration: &str, variant: &str) -> Option<i64> {
        self.enum_values
            .get(&(enumeration.to_string(), variant.to_string()))
            .copied()
    }

    /// Iterate over every verified enum value in stable declaration-key order.
    pub(crate) fn enum_values(&self) -> impl Iterator<Item = (&(String, String), &i64)> {
        self.enum_values.iter()
    }
}

impl CAbiVerificationError {
    /// Construct a verifier failure that belongs to one checked binding.
    fn binding(binding: &CBindingDescriptor, message: impl Into<String>) -> Self {
        Self {
            binding: Some(binding.class_name.clone()),
            message: message.into(),
        }
    }

    /// Construct a verifier failure that cannot be associated with one binding.
    fn toolchain(message: impl Into<String>) -> Self {
        Self {
            binding: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for CAbiVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(binding) = &self.binding {
            write!(formatter, "C binding `{binding}` verification failed: {}", self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl std::error::Error for CAbiVerificationError {}

/// Verify declared C symbols and plain layouts for one selected target.
///
/// This is deliberately syntax-only: it validates the resolved header's declarations before Rust project generation
/// and does not perform ambient linker probing. Linking remains a separate selected-artifact concern in #942.
pub(crate) fn verify_checked_c_binding(
    toolchain: &ClangToolchain,
    plan: &CAbiVerificationPlan,
    binding: &CBindingDescriptor,
) -> Result<CAbiVerificationReceipt, CAbiVerificationError> {
    let source = render_verification_probe(binding)?;
    let mut command = verifier_command(toolchain, plan);
    command
        .args(["-fsyntax-only", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        CAbiVerificationError::binding(
            binding,
            format!(
                "could not start selected Clang toolchain `{}` for target `{}`: {error}",
                toolchain.executable.display(),
                plan.target().triple()
            ),
        )
    })?;
    let Some(stdin) = child.stdin.as_mut() else {
        return Err(CAbiVerificationError::binding(
            binding,
            "selected Clang toolchain did not expose standard input for the verifier probe",
        ));
    };
    stdin.write_all(source.as_bytes()).map_err(|error| {
        CAbiVerificationError::binding(binding, format!("could not write C verifier probe: {error}"))
    })?;
    let output = child.wait_with_output().map_err(|error| {
        CAbiVerificationError::binding(binding, format!("could not wait for C verifier probe: {error}"))
    })?;
    if output.status.success() {
        return verify_enum_values(toolchain, plan, binding);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(CAbiVerificationError::binding(
        binding,
        format!(
            "Clang rejected the declared signature or layout for target `{}`:\n{}",
            plan.target().triple(),
            stderr.trim()
        ),
    ))
}

/// Start one verifier process with the exact selected ABI target and manifest-owned definitions.
fn verifier_command(toolchain: &ClangToolchain, plan: &CAbiVerificationPlan) -> Command {
    let mut command = Command::new(&toolchain.executable);
    command.args(&toolchain.arguments);
    for definition in plan.definitions() {
        command.arg(format!("-D{definition}"));
    }
    command.args(["-std=c11", "-Werror", "-x", "c", "-target"]);
    command.arg(plan.target().triple());
    command
}

/// Extract every native enum expression as an `i64` from Clang's target AST.
///
/// This syntax-only probe uses an anonymous enum to make Clang fold each macro or constant expression with the selected
/// ABI; its JSON AST then reports the exact value without linking or executing target code.
fn verify_enum_values(
    toolchain: &ClangToolchain,
    plan: &CAbiVerificationPlan,
    binding: &CBindingDescriptor,
) -> Result<CAbiVerificationReceipt, CAbiVerificationError> {
    let (source, requested) = render_enum_value_probe(binding)?;
    if requested.is_empty() {
        return Ok(CAbiVerificationReceipt::default());
    }
    let mut command = verifier_command(toolchain, plan);
    command
        .args(["-fsyntax-only", "-Xclang", "-ast-dump=json", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        CAbiVerificationError::binding(
            binding,
            format!(
                "could not start selected Clang toolchain `{}` for enum values on target `{}`: {error}",
                toolchain.executable.display(),
                plan.target().triple()
            ),
        )
    })?;
    let Some(stdin) = child.stdin.as_mut() else {
        return Err(CAbiVerificationError::binding(
            binding,
            "selected Clang toolchain did not expose standard input for the enum verifier probe",
        ));
    };
    stdin.write_all(source.as_bytes()).map_err(|error| {
        CAbiVerificationError::binding(binding, format!("could not write C enum verifier probe: {error}"))
    })?;
    let output = child.wait_with_output().map_err(|error| {
        CAbiVerificationError::binding(binding, format!("could not wait for C enum verifier probe: {error}"))
    })?;
    if !output.status.success() {
        return Err(CAbiVerificationError::binding(
            binding,
            format!(
                "Clang could not evaluate declared enum constants for target `{}`:\n{}",
                plan.target().triple(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let ast = serde_json::from_slice::<serde_json::Value>(&output.stdout).map_err(|error| {
        CAbiVerificationError::binding(binding, format!("Clang returned an invalid enum AST: {error}"))
    })?;
    let mut enum_values = BTreeMap::new();
    for (enumeration, variant, generated_name) in requested {
        let Some(value) = ast_enum_constant_value(&ast, &generated_name) else {
            return Err(CAbiVerificationError::binding(
                binding,
                format!("Clang did not report a value for `{enumeration}.{variant}`"),
            ));
        };
        let value = value.parse::<i64>().map_err(|error| {
            CAbiVerificationError::binding(
                binding,
                format!("C enum value for `{enumeration}.{variant}` is outside Incan `int`: {error}"),
            )
        })?;
        enum_values.insert((enumeration, variant), value);
    }
    Ok(CAbiVerificationReceipt { enum_values })
}

/// Render a C source unit whose named anonymous-enum constants Clang can fold.
fn render_enum_value_probe(
    binding: &CBindingDescriptor,
) -> Result<(String, Vec<EnumValueProbeRequest>), CAbiVerificationError> {
    let mut probe = format!(
        "/* Generated by the Incan checked C ABI verifier. */\n#include \"{}\"\n\n",
        escape_c_include(&binding.header)
    );
    let mut requested = Vec::new();
    for enumeration in &binding.enums {
        for variant in &enumeration.variants {
            let native = checked_c_identifier(binding, &variant.native, "native enum constant")?;
            let generated_name = format!(
                "__incan_c_value_{}_{}_{}",
                c_identifier_component(&binding.class_name),
                c_identifier_component(&enumeration.name),
                c_identifier_component(&variant.name),
            );
            probe.push_str(&format!("enum {{ {generated_name} = ({native}) }};\n"));
            requested.push((enumeration.name.clone(), variant.name.clone(), generated_name));
        }
    }
    Ok((probe, requested))
}

/// Find one generated enum constant's folded integer value in Clang's JSON AST.
fn ast_enum_constant_value<'a>(value: &'a serde_json::Value, expected_name: &str) -> Option<&'a str> {
    let object = value.as_object()?;
    if object.get("kind").and_then(serde_json::Value::as_str) == Some("EnumConstantDecl")
        && object.get("name").and_then(serde_json::Value::as_str) == Some(expected_name)
    {
        return ast_constant_value(value);
    }
    object
        .get("inner")
        .and_then(serde_json::Value::as_array)
        .and_then(|children| {
            children
                .iter()
                .find_map(|child| ast_enum_constant_value(child, expected_name))
        })
}

/// Find the first folded constant expression nested beneath an enum declaration.
fn ast_constant_value(value: &serde_json::Value) -> Option<&str> {
    let object = value.as_object()?;
    if object.get("kind").and_then(serde_json::Value::as_str) == Some("ConstantExpr")
        && let Some(value) = object.get("value").and_then(serde_json::Value::as_str)
    {
        return Some(value);
    }
    object
        .get("inner")
        .and_then(serde_json::Value::as_array)
        .and_then(|children| children.iter().find_map(ast_constant_value))
}

/// Render a deterministic, non-executable C probe from one checked descriptor.
fn render_verification_probe(binding: &CBindingDescriptor) -> Result<String, CAbiVerificationError> {
    let mut probe = format!(
        "/* Generated by the Incan checked C ABI verifier. */\n#include \"{}\"\n\n",
        escape_c_include(&binding.header)
    );
    for symbol in &binding.symbols {
        let native = checked_c_identifier(binding, &symbol.native, "native symbol")?;
        let parameters = if symbol.parameters.is_empty() {
            "void".to_string()
        } else {
            symbol
                .parameters
                .iter()
                .map(|parameter| c_type_spelling(binding, &parameter.ty, false))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        };
        let result = c_type_spelling(binding, &symbol.return_type, true)?;
        probe.push_str(&format!(
            "_Static_assert(_Generic(&{native}, {result} (*)({parameters}): 1, default: 0), \"Incan C signature mismatch: {}.{}\");\n",
            binding.class_name, symbol.name
        ));
    }
    for structure in &binding.structs {
        render_structure_layout_probe(&mut probe, binding, structure)?;
    }
    for enumeration in &binding.enums {
        render_enum_carrier_probes(&mut probe, binding, enumeration)?;
    }
    Ok(probe)
}

/// Render one carrier check for every source-visible C enum constant.
///
/// The declaration's `c.*` carrier is an explicit ABI promise: C offers no portable enum-value reflection, so the probe
/// uses `_Generic` after macro expansion to reject missing constants and physical carrier mismatches without
/// inventing a source-level spelling for the platform's enum representation.
fn render_enum_carrier_probes(
    probe: &mut String,
    binding: &CBindingDescriptor,
    enumeration: &CBindingEnum,
) -> Result<(), CAbiVerificationError> {
    let carrier = c_scalar_spelling(enumeration.carrier);
    for variant in &enumeration.variants {
        let native = checked_c_identifier(binding, &variant.native, "native enum constant")?;
        probe.push_str(&format!(
            "_Static_assert(_Generic(({native}), {carrier}: 1, default: 0), \"Incan C enum carrier mismatch: {}.{}.{}\");\n",
            binding.class_name, enumeration.name, variant.name
        ));
    }
    Ok(())
}

/// Render layout equivalence checks for one explicitly listed plain C structure.
fn render_structure_layout_probe(
    probe: &mut String,
    binding: &CBindingDescriptor,
    structure: &CBindingStruct,
) -> Result<(), CAbiVerificationError> {
    let native = checked_c_type_name(binding, &structure.native, "plain structure native type")?;
    let expected = format!(
        "__incan_expected_{}_{}",
        c_identifier_component(&binding.class_name),
        c_identifier_component(&structure.name)
    );
    probe.push_str(&format!("typedef struct {expected} {{\n"));
    for field in &structure.fields {
        let ty = c_type_spelling(binding, &field.ty, false)?;
        let field_name = checked_c_identifier(binding, &field.name, "plain structure field")?;
        probe.push_str(&format!("    {ty} {field_name};\n"));
    }
    probe.push_str(&format!("}} {expected};\n"));
    probe.push_str(&format!(
        "_Static_assert(sizeof({native}) == sizeof({expected}), \"Incan C layout size mismatch: {}.{}\");\n",
        binding.class_name, structure.name
    ));
    probe.push_str(&format!(
        "_Static_assert(_Alignof({native}) == _Alignof({expected}), \"Incan C layout alignment mismatch: {}.{}\");\n",
        binding.class_name, structure.name
    ));
    for field in &structure.fields {
        let field_name = checked_c_identifier(binding, &field.name, "plain structure field")?;
        probe.push_str(&format!(
            "_Static_assert(__builtin_offsetof({native}, {field_name}) == __builtin_offsetof({expected}, {field_name}), \"Incan C layout field offset mismatch: {}.{}.{}\");\n",
            binding.class_name, structure.name, field.name
        ));
    }
    Ok(())
}

/// Render one compiler-known C type without allowing source text to introduce arbitrary C fragments.
fn c_type_spelling(
    binding: &CBindingDescriptor,
    ty: &CBindingType,
    allow_void: bool,
) -> Result<String, CAbiVerificationError> {
    match ty {
        CBindingType::Void if allow_void => Ok("void".to_string()),
        CBindingType::Void => Err(CAbiVerificationError::binding(
            binding,
            "`None` is valid only as a C function return type",
        )),
        CBindingType::Scalar(scalar) => Ok(c_scalar_spelling(*scalar).to_string()),
        CBindingType::Pointer { mutable, pointee } => {
            let pointee = c_type_spelling(binding, pointee, false)?;
            let qualifier = if *mutable { "" } else { "const " };
            Ok(format!("{qualifier}{pointee} *"))
        }
        CBindingType::Resource { resource, .. } => {
            let Some(resource) = binding.resources.iter().find(|candidate| candidate.name == *resource) else {
                return Err(CAbiVerificationError::binding(
                    binding,
                    format!("C resource `{resource}` is not declared by this binding"),
                ));
            };
            let native = checked_c_type_name(binding, &resource.native, "opaque resource native type")?;
            Ok(format!("{native} *"))
        }
        CBindingType::Output { value, .. } => {
            let value = c_type_spelling(binding, value, false)?;
            Ok(format!("{value} *"))
        }
        CBindingType::Nullable(value) => c_type_spelling(binding, value, allow_void),
        CBindingType::Struct(name) => {
            let Some(structure) = binding.structs.iter().find(|structure| structure.name == *name) else {
                return Err(CAbiVerificationError::binding(
                    binding,
                    format!("C structure `{name}` is not declared by this binding"),
                ));
            };
            checked_c_type_name(binding, &structure.native, "plain structure native type")
        }
    }
}

/// Return a target-aware builtin spelling for an exact C scalar category.
fn c_scalar_spelling(scalar: ScalarTypeId) -> &'static str {
    match scalar {
        ScalarTypeId::I8 => "__INT8_TYPE__",
        ScalarTypeId::U8 => "__UINT8_TYPE__",
        ScalarTypeId::I16 => "__INT16_TYPE__",
        ScalarTypeId::U16 => "__UINT16_TYPE__",
        ScalarTypeId::I32 => "__INT32_TYPE__",
        ScalarTypeId::U32 => "__UINT32_TYPE__",
        ScalarTypeId::I64 => "__INT64_TYPE__",
        ScalarTypeId::U64 => "__UINT64_TYPE__",
        ScalarTypeId::I128 => "__int128",
        ScalarTypeId::U128 => "unsigned __int128",
        ScalarTypeId::F32 => "float",
        ScalarTypeId::F64 => "double",
        ScalarTypeId::Size => "__SIZE_TYPE__",
        ScalarTypeId::CChar => "char",
        ScalarTypeId::CInt => "int",
    }
}

/// Require a C identifier rather than permitting unstructured probe injection.
fn checked_c_identifier<'a>(
    binding: &CBindingDescriptor,
    value: &'a str,
    label: &str,
) -> Result<&'a str, CAbiVerificationError> {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err(CAbiVerificationError::binding(
            binding,
            format!("{label} cannot be empty"),
        ));
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(CAbiVerificationError::binding(
            binding,
            format!("{label} `{value}` is not a supported C identifier"),
        ));
    }
    Ok(value)
}

/// Admit only a C identifier or `struct <identifier>` for declared plain layouts.
fn checked_c_type_name(
    binding: &CBindingDescriptor,
    value: &str,
    label: &str,
) -> Result<String, CAbiVerificationError> {
    if let Some(tag) = value.strip_prefix("struct ") {
        checked_c_identifier(binding, tag, label)?;
        return Ok(value.to_string());
    }
    checked_c_identifier(binding, value, label)?;
    Ok(value.to_string())
}

/// Escape an explicit header path for one generated C include directive.
fn escape_c_include(header: &str) -> String {
    header.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Convert source names into a private generated C identifier component.
fn c_identifier_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{CAbiTarget, CAbiVerificationPlan, ClangToolchain, verify_checked_c_binding};
    use crate::frontend::typechecker::{
        CBindingDescriptor, CBindingEnum, CBindingEnumVariant, CBindingParameter, CBindingStruct, CBindingStructField,
        CBindingSymbol, CBindingType,
    };
    use crate::oven_interop::{CapabilityRequirement, InteropTargetPlatform, OvenInteropTarget};
    use incan_core::lang::c_abi::{LinkCapabilityId, ScalarTypeId};

    fn fixture_binding(header: String) -> CBindingDescriptor {
        CBindingDescriptor {
            span: crate::frontend::ast::Span::default(),
            class_name: "Fixture".to_string(),
            header,
            system_library: "fixture".to_string(),
            link_capability: LinkCapabilityId::SystemLibrary,
            resources: Vec::new(),
            symbols: vec![CBindingSymbol {
                name: "absolute".to_string(),
                native: "fixture_abs".to_string(),
                parameters: vec![CBindingParameter {
                    name: "value".to_string(),
                    ty: CBindingType::Scalar(ScalarTypeId::I32),
                }],
                return_type: CBindingType::Scalar(ScalarTypeId::I32),
                buffers: Vec::new(),
                outcomes: Vec::new(),
            }],
            enums: vec![CBindingEnum {
                name: "Status".to_string(),
                carrier: ScalarTypeId::I32,
                variants: vec![CBindingEnumVariant {
                    name: "OK".to_string(),
                    native: "FIXTURE_OK".to_string(),
                }],
            }],
            structs: vec![CBindingStruct {
                name: "Pair".to_string(),
                native: "fixture_pair".to_string(),
                fields: vec![
                    CBindingStructField {
                        name: "left".to_string(),
                        ty: CBindingType::Scalar(ScalarTypeId::I32),
                    },
                    CBindingStructField {
                        name: "right".to_string(),
                        ty: CBindingType::Scalar(ScalarTypeId::I32),
                    },
                ],
            }],
        }
    }

    fn host_clang(plan: &CAbiVerificationPlan) -> Option<ClangToolchain> {
        ClangToolchain::discover(plan).ok()
    }

    fn declared_interop_target(
        target: &str,
        platform: InteropTargetPlatform,
        definitions: Vec<&str>,
    ) -> OvenInteropTarget {
        OvenInteropTarget {
            target: target.to_string(),
            toolchain: Some(CapabilityRequirement {
                capability: "fixture-clang".to_string(),
                version: None,
            }),
            sdk: Some(CapabilityRequirement {
                capability: match (&platform, target) {
                    (InteropTargetPlatform::Android { .. }, _) => "android",
                    (InteropTargetPlatform::Ios { .. }, _) => match crate::oven_interop::ios_target_kind(target) {
                        Some(kind) => kind.sdk_capability(),
                        None => "invalid-ios-target",
                    },
                }
                .to_string(),
                version: None,
            }),
            platform: Some(platform),
            headers: Vec::new(),
            definitions: definitions.into_iter().map(str::to_string).collect(),
            artifacts: Vec::new(),
            bindings: Vec::new(),
            shims: Vec::new(),
        }
    }

    #[test]
    fn host_clang_verifies_signature_and_plain_layout() -> Result<(), Box<dyn std::error::Error>> {
        let Some(plan) = CAbiVerificationPlan::host() else {
            return Ok(());
        };
        let Some(toolchain) = host_clang(&plan) else {
            return Ok(());
        };
        let temporary = tempfile::tempdir()?;
        let header = temporary.path().join("fixture.h");
        std::fs::write(
            &header,
            "typedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint fixture_abs(int value);\n",
        )?;
        let receipt = verify_checked_c_binding(
            &toolchain,
            &plan,
            &fixture_binding(header.to_string_lossy().into_owned()),
        )?;
        assert_eq!(receipt.enum_value("Status", "OK"), Some(0));
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn host_clang_selects_the_macos_sdk_sysroot() -> Result<(), Box<dyn std::error::Error>> {
        let Some(plan) = CAbiVerificationPlan::host() else {
            return Ok(());
        };
        let toolchain = ClangToolchain::discover(&plan)?;
        assert!(
            toolchain
                .arguments
                .windows(2)
                .any(|arguments| arguments[0] == "-isysroot" && !arguments[1].is_empty()),
            "macOS C ABI verification must provide the Xcode SDK sysroot"
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn simulator_profile_verifies_with_the_iphone_simulator_sdk() -> Result<(), Box<dyn std::error::Error>> {
        let plan = CAbiVerificationPlan::from_interop_target(&declared_interop_target(
            "aarch64-apple-ios-sim",
            InteropTargetPlatform::Ios {
                deployment_target: "13.0".to_string(),
            },
            Vec::new(),
        ))?;
        let toolchain = ClangToolchain::discover(&plan)?;
        assert!(
            toolchain.arguments.windows(2).any(|arguments| {
                arguments[0] == "-isysroot"
                    && arguments[1].to_string_lossy().contains("iPhoneSimulator")
                    && arguments[1].to_string_lossy().ends_with(".sdk")
            }),
            "simulator C ABI verification must select the iPhoneSimulator SDK"
        );
        let temporary = tempfile::tempdir()?;
        let header = temporary.path().join("fixture.h");
        std::fs::write(
            &header,
            "typedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint fixture_abs(int value);\n",
        )?;
        let receipt = verify_checked_c_binding(
            &toolchain,
            &plan,
            &fixture_binding(header.to_string_lossy().into_owned()),
        )?;
        assert_eq!(receipt.enum_value("Status", "OK"), Some(0));
        Ok(())
    }

    #[test]
    fn clang_syntax_verifies_the_foundation_fixture_for_linux_and_macos_targets()
    -> Result<(), Box<dyn std::error::Error>> {
        let Some(plan) = CAbiVerificationPlan::host() else {
            return Ok(());
        };
        let Some(toolchain) = host_clang(&plan) else {
            return Ok(());
        };
        let temporary = tempfile::tempdir()?;
        let header = temporary.path().join("fixture.h");
        std::fs::write(
            &header,
            "typedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint fixture_abs(int value);\n",
        )?;
        let binding = fixture_binding(header.to_string_lossy().into_owned());
        for target in [CAbiTarget::LinuxX86_64, CAbiTarget::MacosArm64] {
            let plan = CAbiVerificationPlan {
                target,
                definitions: Vec::new(),
                toolchain_identity: None,
            };
            verify_checked_c_binding(&toolchain, &plan, &binding)?;
        }
        Ok(())
    }

    #[test]
    fn verifier_reports_mismatched_checked_signature() -> Result<(), Box<dyn std::error::Error>> {
        let Some(plan) = CAbiVerificationPlan::host() else {
            return Ok(());
        };
        let Some(toolchain) = host_clang(&plan) else {
            return Ok(());
        };
        let temporary = tempfile::tempdir()?;
        let header = temporary.path().join("fixture.h");
        std::fs::write(
            &header,
            "typedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nlong fixture_abs(int value);\n",
        )?;
        let error = match verify_checked_c_binding(
            &toolchain,
            &plan,
            &fixture_binding(header.to_string_lossy().into_owned()),
        ) {
            Err(error) => error,
            Ok(_) => panic!("mismatched C return type must be rejected"),
        };
        assert!(
            error.message.contains("Clang rejected"),
            "unexpected verifier error: {error}"
        );
        assert!(
            error.message.contains("Incan C signature mismatch"),
            "unexpected verifier error: {error}"
        );
        Ok(())
    }

    #[test]
    fn verifier_rejects_an_enum_constant_with_the_wrong_carrier() -> Result<(), Box<dyn std::error::Error>> {
        let Some(plan) = CAbiVerificationPlan::host() else {
            return Ok(());
        };
        let Some(toolchain) = host_clang(&plan) else {
            return Ok(());
        };
        let temporary = tempfile::tempdir()?;
        let header = temporary.path().join("fixture.h");
        std::fs::write(
            &header,
            "typedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint fixture_abs(int value);\n",
        )?;
        let mut binding = fixture_binding(header.to_string_lossy().into_owned());
        binding.enums[0].carrier = ScalarTypeId::U32;
        let error = match verify_checked_c_binding(&toolchain, &plan, &binding) {
            Err(error) => error,
            Ok(_) => panic!("an enum carrier mismatch must be rejected"),
        };
        assert!(
            error.message.contains("Incan C enum carrier mismatch"),
            "unexpected verifier error: {error}"
        );
        Ok(())
    }

    #[test]
    fn declared_mobile_target_profiles_select_exact_clang_abi_triples() -> Result<(), Box<dyn std::error::Error>> {
        let android = CAbiVerificationPlan::from_interop_target(&declared_interop_target(
            "aarch64-linux-android",
            InteropTargetPlatform::Android { api_level: 34 },
            vec!["FIXTURE=1"],
        ))?;
        assert_eq!(android.target().triple(), "aarch64-linux-android34");
        assert_eq!(android.definitions(), ["FIXTURE=1"]);

        let ios = CAbiVerificationPlan::from_interop_target(&declared_interop_target(
            "aarch64-apple-ios",
            InteropTargetPlatform::Ios {
                deployment_target: "13.0".to_string(),
            },
            Vec::new(),
        ))?;
        assert_eq!(ios.target().triple(), "arm64-apple-ios13.0");

        let ios_simulator = CAbiVerificationPlan::from_interop_target(&declared_interop_target(
            "aarch64-apple-ios-sim",
            InteropTargetPlatform::Ios {
                deployment_target: "13.0".to_string(),
            },
            Vec::new(),
        ))?;
        assert_eq!(ios_simulator.target().triple(), "arm64-apple-ios13.0-simulator");
        Ok(())
    }

    #[test]
    fn host_target_is_available_only_for_supported_abi_verifiers() {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert_eq!(CAbiTarget::host(), Some(CAbiTarget::LinuxX86_64));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(CAbiTarget::host(), Some(CAbiTarget::MacosArm64));
        #[cfg(not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )))]
        assert_eq!(CAbiTarget::host(), None);
    }

    #[test]
    fn verifier_applies_declared_target_definitions() -> Result<(), Box<dyn std::error::Error>> {
        let Some(target) = CAbiTarget::host() else {
            return Ok(());
        };
        let plan = CAbiVerificationPlan {
            target,
            definitions: vec!["INCAN_FIXTURE_FEATURE=1".to_string()],
            toolchain_identity: None,
        };
        let Some(toolchain) = host_clang(&plan) else {
            return Ok(());
        };
        let temporary = tempfile::tempdir()?;
        let header = temporary.path().join("fixture.h");
        std::fs::write(
            &header,
            "#ifndef INCAN_FIXTURE_FEATURE\n#error expected target definition\n#endif\ntypedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint fixture_abs(int value);\n",
        )?;

        verify_checked_c_binding(
            &toolchain,
            &plan,
            &fixture_binding(header.to_string_lossy().into_owned()),
        )?;
        Ok(())
    }

    #[test]
    fn target_catalogue_names_linux_and_macos_abi_triples() {
        assert_eq!(CAbiTarget::LinuxX86_64.triple(), "x86_64-unknown-linux-gnu");
        assert_eq!(CAbiTarget::MacosArm64.triple(), "arm64-apple-macos11");
    }

    #[test]
    fn test_toolchain_constructor_is_explicit() {
        let toolchain = ClangToolchain::at("/fixture/clang");
        assert_eq!(toolchain.executable, std::path::PathBuf::from("/fixture/clang"));
    }
}
