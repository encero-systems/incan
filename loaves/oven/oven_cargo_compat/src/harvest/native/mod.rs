//! Native trace matching, archive reconstruction, and link-work composition.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use oven_model::manifest::{
    RustFactLibrary, RustFactLibraryKind, RustFactLinkLanguage, RustFactProducerRole, RustFactWorkObservation,
};

use super::super::{
    CargoInvocationOutput, OvenLegacyCargoError, OvenLegacyCargoSelectedUnitCapture, OvenLegacyNativeInvocation,
    OvenLegacyNativeProbeEvidence, digest_bytes,
};

mod argv;
mod authority;
mod environment;
mod sources;

pub(in crate::harvest) use authority::NativeCompiler;
use authority::{CompilerAuthority, common_ancestor, native_resource_directory};
use environment::{CommonCompileEnvironment, common_environment_path, resolve_observed_path};
use sources::NativeSources;

/// Convert and attach native compiler/archive traces to the exact build-script edges that emitted their library.
pub(crate) fn attach_native_link_records(
    capture: &mut OvenLegacyCargoSelectedUnitCapture,
    outputs: &[CargoInvocationOutput],
    compiler: &Path,
    cxx: &Path,
    sysroot: &Path,
) -> Result<(), OvenLegacyCargoError> {
    let mut native = Vec::new();
    for output in outputs {
        for line in output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if matches!(
                value.get("reason").and_then(serde_json::Value::as_str),
                Some("incan-native-compile-invocation" | "incan-native-archive-invocation")
            ) {
                native.push(
                    serde_json::from_value::<OvenLegacyNativeInvocation>(value).map_err(|error| {
                        OvenLegacyCargoError::Plan(format!("invalid native invocation record: {error}"))
                    })?,
                );
            }
        }
    }
    if native.is_empty() {
        return Ok(());
    }
    let compiler = fs::canonicalize(compiler).map_err(|source| OvenLegacyCargoError::Io {
        path: compiler.to_path_buf(),
        source,
    })?;
    let cxx = fs::canonicalize(cxx).map_err(|source| OvenLegacyCargoError::Io {
        path: cxx.to_path_buf(),
        source,
    })?;
    let sysroot = fs::canonicalize(sysroot).map_err(|source| OvenLegacyCargoError::Io {
        path: sysroot.to_path_buf(),
        source,
    })?;
    let compiler_resource = native_resource_directory(&compiler, "C compiler")?;
    let cxx_resource = native_resource_directory(&cxx, "C++ compiler")?;
    let compilers = [
        NativeCompiler {
            executable: &compiler,
            resource_dir: &compiler_resource,
            sysroot: &sysroot,
        },
        NativeCompiler {
            executable: &cxx,
            resource_dir: &cxx_resource,
            sysroot: &sysroot,
        },
    ];
    for unit in &mut capture.units {
        for dependency in &mut unit.dependencies {
            let Some(facts) = dependency.build_script.as_mut() else {
                continue;
            };
            if facts.linked_libraries.is_empty() {
                continue;
            }
            let out_dir = facts.out_dir.to_string_lossy();
            let matching = native
                .iter()
                .filter(|invocation| {
                    invocation
                        .environment
                        .get("OUT_DIR")
                        .is_some_and(|value| value == out_dir.as_ref())
                })
                .cloned()
                .collect::<Vec<_>>();
            match native_link_work_from_observations(&matching, &facts.linked_libraries, &compilers) {
                Ok(conversion) => {
                    facts.publisher_work = conversion.work;
                    facts.publisher_native_probes = conversion.probes;
                    facts.publisher_work_refusal = None;
                }
                Err(reason) => facts.publisher_work_refusal = Some(reason),
            }
        }
    }
    Ok(())
}

/// Converted native work and the compiler probes deliberately excluded from replay.
pub(super) struct NativeLinkConversion {
    pub(super) work: Vec<RustFactWorkObservation>,
    pub(super) probes: Vec<OvenLegacyNativeProbeEvidence>,
}

/// Convert complete compiler and archiver traces into one link record per observed archive path.
pub(super) fn native_link_work_from_observations(
    invocations: &[OvenLegacyNativeInvocation],
    linked_libraries: &[String],
    compilers: &[NativeCompiler<'_>],
) -> Result<NativeLinkConversion, String> {
    let compiles = invocations
        .iter()
        .filter(|invocation| invocation.reason == "incan-native-compile-invocation")
        .collect::<Vec<_>>();
    if compiles.is_empty() {
        return Err("native adoption observed no compiler invocation".to_string());
    }
    let library_names = linked_libraries
        .iter()
        .filter_map(|library| library.strip_prefix("static="))
        .collect::<BTreeSet<_>>();
    let mut archives = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.reason == "incan-native-archive-invocation")
    {
        let (archive, members) = archive_members(invocation)?;
        archives.entry(archive).or_default().extend(members);
    }
    if archives.is_empty() {
        return Err("native adoption observed no archive path".to_string());
    }
    let mut parsed_compiles = BTreeMap::<PathBuf, (&OvenLegacyNativeInvocation, PlainCompile)>::new();
    let mut probes = Vec::new();
    let archived_paths = archives
        .values()
        .flat_map(|members| members.iter().cloned())
        .collect::<BTreeSet<_>>();
    for invocation in compiles {
        match plain_compile(invocation) {
            Ok(parsed) if archived_paths.contains(&parsed.output) => {
                if parsed_compiles
                    .insert(parsed.output.clone(), (invocation, parsed))
                    .is_some()
                {
                    return Err("one archived object has more than one observed compile".to_string());
                }
            }
            Ok(parsed)
                if parsed
                    .source
                    .starts_with(common_environment_path(&[invocation], "CARGO_MANIFEST_DIR")?) =>
            {
                let name = parsed
                    .output
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or("non-UTF-8 object");
                return Err(format!("compiled object `{name}` was never archived"));
            }
            Ok(_) | Err(_) => probes.push(native_probe_evidence(invocation, compilers)?),
        }
    }
    let mut work = Vec::new();
    for (archive, members) in archives {
        let archive_name = archive
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| "archive path has no portable UTF-8 file name".to_string())?;
        let library = archive_name
            .strip_prefix("lib")
            .and_then(|name| name.strip_suffix(".a"))
            .filter(|name| library_names.contains(name))
            .ok_or_else(|| format!("archive path `{archive_name}` has no matching `static=<name>` link library"))?;
        let member_compiles = members
            .iter()
            .map(|member| {
                parsed_compiles.get(member).ok_or_else(|| {
                    let name = member
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or("non-UTF-8 object");
                    format!("archive member `{name}` was never compiled")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if members.iter().collect::<BTreeSet<_>>().len() != members.len() {
            return Err(format!("archive `{archive_name}` contains one object more than once"));
        }
        work.push(native_link_work_for_archive(&member_compiles, library, compilers)?);
    }
    if parsed_compiles.len() != archived_paths.len() {
        return Err("the rebuilt archive member set does not equal the non-probe compile outputs".to_string());
    }
    let matched = work
        .iter()
        .filter_map(|observation| observation.library.as_ref().map(|library| library.name.as_str()))
        .collect::<BTreeSet<_>>();
    if matched != library_names {
        let missing = library_names.difference(&matched).next().copied().unwrap_or("unknown");
        return Err(format!("static link library `{missing}` has no matching archive path"));
    }
    Ok(NativeLinkConversion { work, probes })
}

/// Convert one reconstructed archive by composing environment, authority, and source adoption.
fn native_link_work_for_archive(
    member_compiles: &[&(&OvenLegacyNativeInvocation, PlainCompile)],
    library: &str,
    compilers: &[NativeCompiler<'_>],
) -> Result<RustFactWorkObservation, String> {
    let invocations = member_compiles
        .iter()
        .map(|(invocation, _)| *invocation)
        .collect::<Vec<_>>();
    let environment = CommonCompileEnvironment::from_invocations(&invocations)?;
    let authority = CompilerAuthority::from_invocations(&invocations, compilers)?;
    let mut sources = NativeSources::new(&environment, &authority);
    for (invocation, compile) in member_compiles.iter().copied() {
        sources.adopt_compile(invocation, &compile.source, &compile.output)?;
    }
    let (objects, inputs, owner_paths) = sources.finish();
    let executable = authority.executable(&owner_paths)?;
    Ok(RustFactWorkObservation {
        role: RustFactProducerRole::Link,
        name: library.to_string(),
        target: environment.target,
        executable: Some(executable),
        objects,
        arguments: Vec::new(),
        environment: Vec::new(),
        inputs,
        outputs: Vec::new(),
        library: Some(RustFactLibrary {
            name: library.to_string(),
            kind: RustFactLibraryKind::Static,
        }),
    })
}

/// Parsed plain `-c SOURCE -o OBJECT` invocation.
struct PlainCompile {
    source: PathBuf,
    output: PathBuf,
}

/// Require exactly one source and one output from an ordinary compile invocation.
fn plain_compile(invocation: &OvenLegacyNativeInvocation) -> Result<PlainCompile, String> {
    let value_after = |flag: &str| {
        invocation
            .arguments
            .windows(2)
            .filter(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str())
            .collect::<Vec<_>>()
    };
    let sources = value_after("-c");
    let outputs = value_after("-o");
    let ([source], [output]) = (sources.as_slice(), outputs.as_slice()) else {
        return Err(
            "native invocation is not a plain compile with exactly one `-c` source and `-o` object".to_string(),
        );
    };
    Ok(PlainCompile {
        source: resolve_observed_path(&invocation.working_directory, source)?,
        output: resolve_observed_path(&invocation.working_directory, output)?,
    })
}

/// Normalize one non-archived compiler invocation into stable probe evidence.
fn native_probe_evidence(
    invocation: &OvenLegacyNativeInvocation,
    compilers: &[NativeCompiler<'_>],
) -> Result<OvenLegacyNativeProbeEvidence, String> {
    let manifest_dir = common_environment_path(&[invocation], "CARGO_MANIFEST_DIR")?;
    let out_dir = common_environment_path(&[invocation], "OUT_DIR")?;
    let executable = fs::canonicalize(&invocation.executable)
        .map_err(|error| format!("cannot resolve observed probe compiler: {error}"))?;
    let authority = compilers
        .iter()
        .find(|candidate| fs::canonicalize(candidate.executable).ok().as_ref() == Some(&executable))
        .ok_or_else(|| "native probe used a compiler other than explicit --cc or --cxx".to_string())?;
    let resource = fs::canonicalize(authority.resource_dir)
        .map_err(|error| format!("cannot resolve probe compiler resource directory: {error}"))?;
    let sysroot =
        fs::canonicalize(authority.sysroot).map_err(|error| format!("cannot resolve probe C sysroot: {error}"))?;
    let owner_root = common_ancestor(&[executable.as_path(), resource.as_path(), sysroot.as_path()])
        .ok_or_else(|| "probe compiler, resource directory, and sysroot have no common owner root".to_string())?;
    let normalize = |value: &str| {
        let mut normalized = value.to_string();
        for (root, marker) in [
            (&manifest_dir, "<package>"),
            (&out_dir, "<out>"),
            (&owner_root, "<owner>"),
        ] {
            normalized = normalized.replace(root.to_string_lossy().as_ref(), marker);
        }
        normalized
    };
    let normalized = (
        "incan.oven.native-compiler-probe/1",
        normalize(&invocation.executable),
        invocation
            .arguments
            .iter()
            .map(|argument| normalize(argument))
            .collect::<Vec<_>>(),
        invocation.environment.get("TARGET"),
    );
    let encoded = serde_json::to_vec(&normalized)
        .map_err(|error| format!("cannot encode native compiler probe evidence: {error}"))?;
    Ok(OvenLegacyNativeProbeEvidence {
        digest: digest_bytes(&encoded),
        output: invocation.output.clone(),
    })
}

/// Return one archive operation's path and ordered object members; index-only operations have no members.
fn archive_members(invocation: &OvenLegacyNativeInvocation) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let mut archive = None;
    let mut members = Vec::new();
    for argument in &invocation.arguments {
        if archive.is_none() && argument.ends_with(".a") {
            archive = Some(resolve_observed_path(&invocation.working_directory, argument)?);
            continue;
        }
        if archive.is_some() && argument.ends_with(".o") {
            members.push(resolve_observed_path(&invocation.working_directory, argument)?);
        }
    }
    let Some(archive) = archive else {
        return Err("archive invocation did not name an archive path".to_string());
    };
    Ok((archive, members))
}

/// Remove cc-rs's hexadecimal object prefix while retaining a portable `.o` member name.
pub(super) fn stable_object_name(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| "object path has no portable UTF-8 file name".to_string())?;
    if !name.ends_with(".o") {
        return Err(format!("native compile output `{name}` is not an object file"));
    }
    let stable = name
        .split_once('-')
        .filter(|(prefix, _)| prefix.len() >= 8 && prefix.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map_or(name, |(_, suffix)| suffix);
    Ok(stable.to_string())
}

/// Map a source suffix to the manifest's native-link language vocabulary.
fn source_language(path: &Path) -> Result<RustFactLinkLanguage, String> {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("c") => Ok(RustFactLinkLanguage::C),
        Some("cc" | "cpp" | "cxx") => Ok(RustFactLinkLanguage::Cpp),
        Some("s" | "S" | "asm") => Ok(RustFactLinkLanguage::Assembly),
        _ => Err("cannot classify native source language from its extension".to_string()),
    }
}
