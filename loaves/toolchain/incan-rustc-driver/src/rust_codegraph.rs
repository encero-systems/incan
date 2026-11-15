//! Observe a Rust unit after rustc analysis using Oven's exact invocation. Inspection stops before codegen;
//! compilation continues and publishes the same facts beside the native Loaf.

use incan_codegraph::{
    CODEGRAPH_SCHEMA_VERSION, CodegraphHeaderRecord, CodegraphLanguage, CodegraphMode, CodegraphPackage,
    CodegraphRecord,
};
use rustc_interface::interface;
use rustc_middle::ty::TyCtxt;
use rustc_middle::ty::print::{with_no_trimmed_paths, with_no_visible_paths, with_resolve_crate_name};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Analyze a Rust invocation and publish only a complete checked graph. Failed analysis never creates a cache entry.
pub fn export(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    run(arguments, false)
}

/// Capture the same checked Rust facts while continuing through the unit's ordinary native compilation.
pub fn compile(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    run(arguments, true)
}

/// One observer serves inspection and compilation; the latter publishes a graph only after code generation succeeds.
fn run(arguments: &[String], codegen: bool) -> Result<(), Box<dyn std::error::Error>> {
    let [output, root, rustc_arguments @ ..] = arguments else {
        return Err("usage: --rust-codegraph OUTPUT ROOT RUSTC_ARGS...".into());
    };
    let mut args = vec!["incan-rustc-driver".to_owned()];
    args.extend_from_slice(rustc_arguments);
    let mut callbacks = Observer {
        root: root.clone(),
        records: Vec::new(),
        codegen,
    };
    let capture = crate::native_timing::Capture::begin();
    let analyzed = rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut callbacks));
    drop(capture);
    analyzed.map_err(|_| "Rust analysis failed; no graph was published")?;
    if codegen {
        canonicalize_rlib(rustc_arguments)?;
    }
    if callbacks.records.is_empty() {
        return Err("Rust analysis produced no graph".into());
    }
    write_graph(
        output,
        &incan_codegraph::artifact::encode_graph(&callbacks.records, std::path::Path::new(root))?,
    )?;
    Ok(())
}

/// Give rustc's incremental code-generation members content-derived names before the rlib is fingerprinted or baked.
fn canonicalize_rlib(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = rlib_output(arguments)? else {
        return Ok(());
    };
    canonicalize_archive(&path)
        .map_err(|error| format!("cannot canonicalize Rust archive {}: {error}", path.display()).into())
}

/// Resolve the one rlib named by the ordinary rustc output arguments used for a local Rust library.
fn rlib_output(arguments: &[String]) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let mut crate_name = None;
    let mut crate_type = None;
    let mut output_directory = None;
    let mut extra_filename = String::new();
    let mut previous = "";
    for argument in arguments {
        match previous {
            "--crate-name" => crate_name = Some(argument.as_str()),
            "--crate-type" => crate_type = Some(argument.as_str()),
            "--out-dir" => output_directory = Some(argument.as_str()),
            "-C" if argument.starts_with("extra-filename=") => {
                extra_filename = argument["extra-filename=".len()..].to_owned();
            }
            _ => {}
        }
        if let Some(value) = argument.strip_prefix("--crate-name=") {
            crate_name = Some(value);
        } else if let Some(value) = argument.strip_prefix("--crate-type=") {
            crate_type = Some(value);
        } else if let Some(value) = argument.strip_prefix("--out-dir=") {
            output_directory = Some(value);
        } else if let Some(value) = argument.strip_prefix("-Cextra-filename=") {
            extra_filename = value.to_owned();
        }
        previous = argument;
    }
    if !crate_type.is_some_and(|kind| kind.split(',').any(|kind| matches!(kind, "lib" | "rlib"))) {
        return Ok(None);
    }
    let crate_name = crate_name.ok_or("Rust library compilation names no crate")?;
    let output_directory = output_directory.ok_or("Rust library compilation names no output directory")?;
    Ok(Some(
        Path::new(output_directory).join(format!("lib{crate_name}{extra_filename}.rlib")),
    ))
}

/// Rename BSD extended-name code-generation members in place without changing archive offsets or payload bytes.
fn canonicalize_archive(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut archive = OpenOptions::new().read(true).write(true).open(path)?;
    let mut magic = [0_u8; 8];
    archive.read_exact(&mut magic)?;
    if magic != *b"!<arch>\n" {
        return Err("rlib has no archive magic".into());
    }
    loop {
        let mut header = [0_u8; 60];
        if archive.read(&mut header[..1])? == 0 {
            break;
        }
        archive.read_exact(&mut header[1..])?;
        if header[58..] != *b"`\n" {
            return Err("rlib has a malformed member header".into());
        }
        let size = std::str::from_utf8(&header[48..58])?.trim().parse::<u64>()?;
        let data_start = archive.stream_position()?;
        let name_field = std::str::from_utf8(&header[..16])?.trim();
        if let Some(length) = name_field.strip_prefix("#1/") {
            let length = length.trim().parse::<usize>()?;
            if length as u64 > size {
                return Err("rlib member name exceeds its member size".into());
            }
            let mut name = vec![0_u8; length];
            archive.read_exact(&mut name)?;
            let visible = name
                .iter()
                .rposition(|byte| *byte != 0)
                .map_or(&name[..0], |end| &name[..=end]);
            if visible.ends_with(b".rcgu.o") {
                let digest = member_digest(&mut archive, size - length as u64)?;
                let canonical = canonical_member_name(length, &digest)?;
                if name != canonical {
                    archive.seek(SeekFrom::Start(data_start))?;
                    archive.write_all(&canonical)?;
                }
            }
        }
        let next = data_start
            .checked_add(size)
            .and_then(|end| end.checked_add(size % 2))
            .ok_or("rlib member offset overflow")?;
        archive.seek(SeekFrom::Start(next))?;
    }
    archive.sync_all()?;
    Ok(())
}

/// Hash exactly one archive member payload, excluding its unstable extended filename.
fn member_digest(archive: &mut std::fs::File, mut remaining: u64) -> Result<[u8; 32], std::io::Error> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let length = remaining.min(buffer.len() as u64) as usize;
        archive.read_exact(&mut buffer[..length])?;
        digest.update(&buffer[..length]);
        remaining -= length as u64;
    }
    Ok(digest.finalize().into())
}

/// Fit a content-derived object name into the producer's existing extended-name allocation.
fn canonical_member_name(length: usize, digest: &[u8; 32]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    const PREFIX: &[u8] = b"incan-";
    const SUFFIX: &[u8] = b".rcgu.o";
    if length <= PREFIX.len() + SUFFIX.len() {
        return Err("rlib code-generation member name is too short to canonicalize".into());
    }
    let encoded = hex::encode(digest);
    let variable = length - PREFIX.len() - SUFFIX.len();
    let mut name = Vec::with_capacity(length);
    name.extend_from_slice(PREFIX);
    name.extend(encoded.as_bytes().iter().copied().cycle().take(variable));
    name.extend_from_slice(SUFFIX);
    Ok(name)
}

/// Publish encoded Rust graph bytes and retain the requested path in any filesystem refusal.
fn write_graph(output: &str, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write(output, bytes).map_err(|error| format!("cannot write Rust codegraph {output}: {error}").into())
}

/// Retain records outside rustc's arena; the caller decides whether this invocation continues through codegen.
struct Observer {
    root: String,
    records: Vec<CodegraphRecord>,
    /// Compilation continues only for an ordinary unit build; inspection remains analysis-only.
    codegen: bool,
}

impl rustc_driver::Callbacks for Observer {
    /// Give hand-written Rust invocations the same opt-in phase diagnostics as Incan's native boundary.
    fn config(&mut self, config: &mut interface::Config) {
        crate::native_timing::Timing::requested().configure(config);
        if self.codegen {
            config.override_queries = Some(|_session, providers| crate::native_timing::provide_codegen(providers));
        }
    }

    /// Extract only after name resolution and type checking succeeded; method targets come from typeck results.
    fn after_analysis(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        self.records.push(CodegraphRecord::Header(CodegraphHeaderRecord {
            schema_version: CODEGRAPH_SCHEMA_VERSION,
            compiler_version: incan_lang::version::INCAN_VERSION.to_owned(),
            mode: CodegraphMode::Strict,
            root_path: self.root.clone(),
            languages: vec![CodegraphLanguage::Rust],
            package: Some(CodegraphPackage {
                name: std::env::var("CARGO_PKG_NAME")
                    .ok()
                    .or_else(|| Some(tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).to_string())),
                version: std::env::var("CARGO_PKG_VERSION").ok(),
                root_path: Some(self.root.clone()),
            }),
            semantic_contexts: Vec::new(),
            degraded: false,
        }));
        // Identities must print identically in a defining crate and a consumer, regardless of reexports or aliases.
        with_resolve_crate_name!(with_no_trimmed_paths!(with_no_visible_paths!({
            crate::rust_graph_records::declarations(tcx, &mut self.records);
            crate::rust_graph_bodies::references(tcx, &mut self.records);
        })));
        if self.codegen {
            rustc_driver::Compilation::Continue
        } else {
            rustc_driver::Compilation::Stop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::write_graph;

    /// A graph publication failure identifies the exact output that could not be written.
    #[test]
    fn write_refusal_names_output_path() -> Result<(), Box<dyn std::error::Error>> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let output = std::env::current_dir()?
            .join(format!("incan-absent-codegraph-parent-{unique}"))
            .join("unit.incngraph")
            .display()
            .to_string();
        match write_graph(&output, b"graph") {
            Err(error) if error.to_string().contains(&output) => Ok(()),
            Err(error) => Err(format!("write refusal omitted `{output}`: {error}").into()),
            Ok(()) => Err(format!("unexpectedly wrote graph to `{output}`").into()),
        }
    }
}
