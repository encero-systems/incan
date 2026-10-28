//! Private checked-source snapshots selected by Oven, never public Loaf executable representations.
//!
//! Publication deliberately omits invocation-only facts. These binary snapshots supplement that serialization and
//! verify an exact structural round trip before publication. An unfamiliar or corrupt snapshot is a cache miss.

use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use incan_driver::unit::{UnitDescription, UnitKind};
use incan_frontend::library_manifest::LibraryManifest;
use incan_lang::interop::RustItemMetadata;
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, NativeAttribute, StaticInitializer, TraitDefaultInstantiation, TraitFieldImplementation,
    TraitTypeArgument,
};
use incan_semantics_core::{CanonicalSymbolId, IncanType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::CheckedProgram;

mod validation;

/// Private binary layout; Oven also binds every snapshot to its complete compiler generation.
const SCHEMA: &str = "incan.checked-source/3";

/// Borrow a complete checked program without copying its bodies or graph during serialization.
#[derive(Serialize)]
struct ProgramSnapshot<'a> {
    modules: Vec<ModuleSnapshot<'a>>,
    manifest: Option<String>,
    rust_items: Option<&'a [RustItemMetadata]>,
    sources: &'a [String],
    files: &'a [String],
    entry: usize,
    own: usize,
    graph: &'a [u8],
}

/// Owned twin of `ProgramSnapshot`, in the same positional order, for restoring one private compilation.
#[derive(Deserialize)]
struct StoredProgram {
    modules: Vec<StoredModule>,
    manifest: Option<String>,
    rust_items: Option<Vec<RustItemMetadata>>,
    sources: Vec<String>,
    files: Vec<String>,
    entry: usize,
    own: usize,
    graph: Vec<u8>,
}

/// Preserve the public body representation and supplement every field publication intentionally skips.
#[derive(Serialize)]
struct ModuleSnapshot<'a> {
    module: &'a BodyIrModule,
    frames: Vec<BodyFacts<'a>>,
    callables: Vec<Option<BodySnapshot<'a>>>,
    nominals: Vec<NominalSnapshot<'a>>,
    entries: &'a [CanonicalSymbolId],
    runtime: Vec<(&'a str, &'a CanonicalSymbolId)>,
}

/// Owned twin of `ModuleSnapshot`; no checked identity is reconstructed from a source spelling.
#[derive(Deserialize)]
struct StoredModule {
    module: BodyIrModule,
    frames: Vec<StoredBodyFacts>,
    callables: Vec<Option<StoredBody>>,
    nominals: Vec<StoredNominal>,
    entries: Vec<CanonicalSymbolId>,
    runtime: Vec<(String, CanonicalSymbolId)>,
}

/// Borrow the callable instantiation and macro facts excluded from public executable fragments.
#[derive(Serialize)]
struct BodyFacts<'a> {
    nominal: &'a [TraitTypeArgument],
    default_scope: Option<(&'a IncanType, &'a [TraitTypeArgument])>,
    attributes: &'a [NativeAttribute],
}

/// Owned twin of `BodyFacts`, including source attributes already expanded by a compiled public provider.
#[derive(Deserialize)]
struct StoredBodyFacts {
    nominal: Vec<TraitTypeArgument>,
    default_scope: Option<(IncanType, Vec<TraitTypeArgument>)>,
    attributes: Vec<NativeAttribute>,
}

/// Linked declaration frames are wholly skipped by publication, so retain their ordinary fields once too.
#[derive(Serialize)]
struct BodySnapshot<'a> {
    body: &'a Body,
    facts: BodyFacts<'a>,
}

/// Owned twin of `BodySnapshot` for a linked declaration frame.
#[derive(Deserialize)]
struct StoredBody {
    body: Body,
    facts: StoredBodyFacts,
}

/// Retain closed callable storage and required-field witnesses excluded from public nominal fragments.
#[derive(Serialize)]
struct NominalSnapshot<'a> {
    owned_callable_fields: &'a [usize],
    trait_fields: Vec<&'a [TraitFieldImplementation]>,
}

/// Owned twin of `NominalSnapshot` in the same positional order.
#[derive(Deserialize)]
struct StoredNominal {
    owned_callable_fields: Vec<usize>,
    trait_fields: Vec<Vec<TraitFieldImplementation>>,
}

/// Read only the explicitly supplied artifact, accepting a matching witness and intact complete payload.
pub(super) fn read(unit: &UnitDescription) -> Option<CheckedProgram> {
    let _span = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::checked_source_read").entered();
    if unit.kind != UnitKind::Library {
        return None;
    }
    let bytes = std::fs::read(unit.checked_artifact.as_ref()?).ok()?;
    let boundary = bytes.iter().position(|byte| *byte == b'\n')?;
    let record: Value = serde_json::from_slice(&bytes[..boundary]).ok()?;
    let key = unit.checked_key.as_deref()?;
    if record["schema"] != SCHEMA || record["key"].as_str()? != key {
        return None;
    }
    let payload = &bytes[boundary + 1..];
    if record["digest"].as_str()? != hex::encode(Sha256::digest(payload)) {
        return None;
    }
    let program = decode(payload).ok()?;
    // Fresh descriptions belong to Oven. Also reject a stale direct invocation after an authored source edit.
    if program.files.len() != program.sources.len()
        || program.files.len() != program.modules.len()
        || program.own > program.modules.len()
        || program.entry >= program.modules.len()
        || program.files[program.own..]
            .iter()
            .zip(&program.sources[program.own..])
            .any(|(file, source)| std::fs::read_to_string(file).ok().as_ref() != Some(source))
    {
        return None;
    }
    tracing::debug!(target: "incan::invocation", "reused checked source");
    Some(program)
}

/// Publish only exact round trips after successful checking. Cache failures never change compilation's verdict.
pub(super) fn write(unit: &UnitDescription, program: &mut CheckedProgram) {
    let _span = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::checked_source_write").entered();
    let (Some(path), Some(key)) = (&unit.checked_artifact, &unit.checked_key) else {
        return;
    };
    if unit.kind != UnitKind::Library || !program.tests.is_empty() {
        return;
    }
    let result = atomic_write(path, key, program);
    if let Err(error) = result {
        tracing::debug!(target: "incan::invocation", %error, "checked source was not retained");
    }
}

/// Encode domain maps with the existing binary codec, which admits structured keys without JSON coercions.
fn encode<W: Write>(program: &mut CheckedProgram, writer: W) -> Result<W, Box<dyn std::error::Error>> {
    let manifest = manifest_header(program)?;
    let modules = program.modules.iter().map(module_snapshot).collect();
    let snapshot = ProgramSnapshot {
        modules,
        manifest,
        rust_items: program
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.rust_abi.as_ref())
            .map(|abi| abi.items.as_slice()),
        sources: &program.sources,
        files: &program.files,
        entry: program.entry,
        own: program.own,
        graph: &program.graph,
    };
    Ok(postcard::to_io(&snapshot, writer)?)
}

/// Keep the manifest's tagged JSON header small while borrowing Rust facts through their existing binary serde
/// representation. Restore the temporarily detached vector before propagating any serialization failure.
fn manifest_header(program: &mut CheckedProgram) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(manifest) = program.manifest.as_mut() else {
        return Ok(None);
    };
    let items = manifest.rust_abi.as_mut().map(|abi| std::mem::take(&mut abi.items));
    let header = manifest.to_json_string();
    if let (Some(abi), Some(items)) = (manifest.rust_abi.as_mut(), items) {
        abi.items = items;
    }
    Ok(Some(header?))
}

/// Restore checked state without reinterpreting an identity or reading a native implementation from source.
fn decode(bytes: &[u8]) -> Result<CheckedProgram, Box<dyn std::error::Error>> {
    let snapshot: StoredProgram = postcard::from_bytes(bytes)?;
    let mut manifest = snapshot
        .manifest
        .as_deref()
        .map(LibraryManifest::from_json_str)
        .transpose()?;
    match (
        manifest.as_mut().and_then(|manifest| manifest.rust_abi.as_mut()),
        snapshot.rust_items,
    ) {
        (Some(abi), Some(items)) => abi.items = items,
        (None, None) => {}
        _ => return Err("checked Rust fact inventory differs".into()),
    }
    Ok(CheckedProgram {
        modules: snapshot
            .modules
            .into_iter()
            .map(restore_module)
            .collect::<Result<_, _>>()?,
        manifest,
        sources: snapshot.sources,
        files: snapshot.files,
        entry: snapshot.entry,
        own: snapshot.own,
        graph: snapshot.graph,
        tests: Vec::new(),
    })
}

/// Borrow ordinary fields once and append the invocation-only witnesses in stable structural order.
fn module_snapshot(module: &BodyIrModule) -> ModuleSnapshot<'_> {
    ModuleSnapshot {
        module,
        frames: frames(module).map(body_facts).collect(),
        callables: module
            .loaf_callables
            .iter()
            .map(|item| {
                item.declaration_frame.as_deref().map(|body| BodySnapshot {
                    body,
                    facts: body_facts(body),
                })
            })
            .collect(),
        nominals: module
            .nominal_declarations
            .iter()
            .map(|item| NominalSnapshot {
                owned_callable_fields: &item.owned_callable_fields,
                trait_fields: item
                    .trait_adoptions
                    .iter()
                    .map(|adoption| adoption.fields.as_slice())
                    .collect(),
            })
            .collect(),
        entries: &module.program_entrypoints,
        runtime: module
            .runtime_callable_bindings
            .iter()
            .map(|item| (item.path.as_str(), &item.canonical))
            .collect(),
    }
}

/// Restore publication-skipped fields in their original slots, refusing mismatched sidecar inventories.
fn restore_module(snapshot: StoredModule) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let mut module = snapshot.module;
    module.program_entrypoints = snapshot.entries;
    module.runtime_callable_bindings = snapshot
        .runtime
        .into_iter()
        .map(|(path, canonical)| incan_semantics_core::body_ir::RuntimeCallableBinding { path, canonical })
        .collect();
    if snapshot.callables.len() != module.loaf_callables.len()
        || snapshot.nominals.len() != module.nominal_declarations.len()
        || snapshot.frames.len() != frames(&module).count()
    {
        return Err("checked module inventory differs".into());
    }
    for (callable, frame) in module.loaf_callables.iter_mut().zip(snapshot.callables) {
        callable.declaration_frame = frame.map(|mut frame| {
            restore_body_facts(&mut frame.body, frame.facts);
            Box::new(frame.body)
        });
    }
    for (nominal, extra) in module.nominal_declarations.iter_mut().zip(snapshot.nominals) {
        nominal.owned_callable_fields = extra.owned_callable_fields;
        if extra.trait_fields.len() != nominal.trait_adoptions.len() {
            return Err("checked trait inventory differs".into());
        }
        for (adoption, fields) in nominal.trait_adoptions.iter_mut().zip(extra.trait_fields) {
            adoption.fields = fields;
        }
    }
    for (frame, saved) in frames_mut(&mut module).zip(snapshot.frames) {
        restore_body_facts(frame, saved);
    }
    Ok(module)
}

/// Borrow only facts the public body representation skips; ordinary source bodies are serialized exactly once.
fn body_facts(body: &Body) -> BodyFacts<'_> {
    BodyFacts {
        nominal: &body.nominal_instantiation,
        default_scope: body
            .trait_default_instantiation
            .as_ref()
            .map(|scope| (&scope.receiver_type, scope.arguments.as_slice())),
        attributes: &body.native_attributes,
    }
}

/// Restore skipped instantiation and macro facts into the already decoded body without another body copy.
fn restore_body_facts(body: &mut Body, facts: StoredBodyFacts) {
    body.nominal_instantiation = facts.nominal;
    body.native_attributes = facts.attributes;
    body.trait_default_instantiation =
        facts
            .default_scope
            .map(|(receiver_type, arguments)| TraitDefaultInstantiation {
                receiver_type,
                arguments,
            });
}

/// Enumerate ordinary body-bearing slots in stable structural order; linked frames have their own inventory.
fn frames(module: &BodyIrModule) -> impl Iterator<Item = &Body> {
    module
        .bodies
        .iter()
        .chain(
            module
                .decorated_bindings
                .iter()
                .flat_map(|binding| std::iter::once(&binding.original_body).chain(binding.initializer.as_deref())),
        )
        .chain(
            module
                .static_declarations
                .iter()
                .filter_map(|item| match &item.initial {
                    StaticInitializer::Computed(body) => Some(body.as_ref()),
                    _ => None,
                }),
        )
        .chain(module.nominal_declarations.iter().flat_map(|item| {
            item.field_default_body
                .as_deref()
                .into_iter()
                .chain(item.method_partial_defaults.iter().map(|item| item.frame.as_ref()))
        }))
}

/// Visit the same body-bearing slots mutably after the ordinary public wire fields have been decoded.
fn frames_mut(module: &mut BodyIrModule) -> impl Iterator<Item = &mut Body> {
    module
        .bodies
        .iter_mut()
        .chain(
            module.decorated_bindings.iter_mut().flat_map(|binding| {
                std::iter::once(&mut binding.original_body).chain(binding.initializer.as_deref_mut())
            }),
        )
        .chain(
            module
                .static_declarations
                .iter_mut()
                .filter_map(|item| match &mut item.initial {
                    StaticInitializer::Computed(body) => Some(body.as_mut()),
                    _ => None,
                }),
        )
        .chain(module.nominal_declarations.iter_mut().flat_map(|item| {
            item.field_default_body
                .as_deref_mut()
                .into_iter()
                .chain(item.method_partial_defaults.iter_mut().map(|item| item.frame.as_mut()))
        }))
}

/// Publish a fully validated snapshot with bounded I/O buffers, replacing no source or native output.
fn atomic_write(path: &Path, key: &str, program: &mut CheckedProgram) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let directory = path.parent().ok_or("checked artifact has no directory")?;
    std::fs::create_dir_all(directory)?;
    let temporary = path.with_extension(format!("{}.pending", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&temporary)?;
    let result = write_snapshot(file, key, program).and_then(|()| {
        std::fs::rename(&temporary, path)?;
        Ok(())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    Ok(())
}

/// Hash the payload while writing it and witness the largest serialized slice required by postcard's reader.
struct PayloadWriter<W> {
    target: W,
    digest: Sha256,
    largest: usize,
}

impl<W: Write> Write for PayloadWriter<W> {
    /// Hash only successfully written bytes; the original slice length bounds the decoder's temporary borrow.
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.target.write(bytes)?;
        self.digest.update(&bytes[..written]);
        self.largest = self.largest.max(bytes.len());
        Ok(written)
    }

    /// Forward flushing so validation never reads buffered or incomplete payload bytes.
    fn flush(&mut self) -> std::io::Result<()> {
        self.target.flush()
    }
}

/// Stream the borrowed program to disk and compare its complete decoded facts before finalizing its header.
fn write_snapshot(
    mut file: std::fs::File,
    key: &str,
    program: &mut CheckedProgram,
) -> Result<(), Box<dyn std::error::Error>> {
    // ---- Fixed-width checksum placeholder and borrowed payload ----
    let placeholder = header(key, &"0".repeat(64))?;
    let (digest, largest) = {
        let mut target = BufWriter::new(&mut file);
        target.write_all(&placeholder)?;
        target.write_all(b"\n")?;
        let PayloadWriter {
            mut target,
            digest,
            largest,
        } = encode(
            program,
            PayloadWriter {
                target,
                digest: Sha256::new(),
                largest: 10,
            },
        )?;
        target.flush()?;
        (hex::encode(digest.finalize()), largest)
    };
    let mut scratch = vec![0; largest];

    // ---- Exact round trip with one destination record and bounded string scratch ----
    let offset = u64::try_from(placeholder.len())?
        .checked_add(1)
        .ok_or("checked header is too large")?;
    file.seek(SeekFrom::Start(offset))?;
    validation::verify(program, BufReader::new(&mut file), &mut scratch)?;
    let complete = header(key, &digest)?;
    if complete.len() != placeholder.len() {
        return Err("checked header changed width".into());
    }
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&complete)?;
    Ok(())
}

/// Encode only scalar metadata; checked domain maps keep their binary representation.
fn header(key: &str, digest: &str) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&json!({"schema": SCHEMA, "key": key, "digest": digest}))
}
