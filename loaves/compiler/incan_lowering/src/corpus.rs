//! The golden lowering corpus: the reference lowering's inputs and answers, captured at the driver boundary.
//!
//! A corpus directory holds `blobs/<sha256>` (Body IR module bytes, their sidecars of serde-skipped facts, source
//! texts and product files, each stored once) and `cases/<sha256>.case`, one per unit: the layout request and every
//! lowering call in order, with its inputs by digest and the reference answer. The record shapes here are the
//! capture's own; postcard is positional, so their field order is the format.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, Function, INTERFACE_REPORT, NominalReference, Plan, PlanFunctionRecord,
    ProductNominal, TerminatorKind, decode_lowering_event, decode_native_interface, decode_plan_function_record,
    decode_product_parts, decode_record_plan, encode_plan_function_record, encode_product, plan_function_digest,
    product_identities,
};
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, NativeAttribute, NativeUnionDeclaration, RuntimeCallableBinding, StaticInitializer,
    TraitDefaultInstantiation, TraitFieldImplementation, TraitTypeArgument,
};
use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, IncanType};
use serde::{Deserialize, Serialize};

/// The serde-skipped facts of one Body.
#[derive(Serialize, Deserialize, Default)]
pub struct BodyExtras {
    pub nominal_instantiation: Vec<TraitTypeArgument>,
    pub trait_default: Option<(IncanType, Vec<TraitTypeArgument>)>,
    pub native_attributes: Vec<NativeAttribute>,
}

/// The serde-skipped facts of one module, its bodies in walk order.
#[derive(Serialize, Deserialize, Default)]
pub struct ModuleExtras {
    pub runtime_callable_bindings: Vec<RuntimeCallableBinding>,
    pub runtime_binding_scopes: Vec<(String, Vec<String>)>,
    pub program_entrypoints: Vec<CanonicalSymbolId>,
    pub published_callables: Vec<CanonicalSymbolId>,
    pub bodies: Vec<BodyExtras>,
    pub adoption_fields: Vec<Vec<TraitFieldImplementation>>,
    pub owned_callable_fields: Vec<Vec<usize>>,
    /// Each Loaf callable's serde-skipped declaration frame, with that frame's own skipped facts.
    pub declaration_frames: Vec<Option<(Body, BodyExtras)>>,
}

/// A runtime declaration request as captured.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Requirement {
    pub canonical: Option<CanonicalSymbolId>,
    pub path: String,
    /// postcard bytes of the plan's SourceSpan.
    pub span: Vec<u8>,
}

/// One answer of the lowering.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Outcome {
    Refused(String),
    NeedsRuntime(Vec<Requirement>),
    /// postcard bytes of the whole-program plan, or None for a unit without an entry.
    Complete(Option<Vec<u8>>),
    Census(Vec<String>),
}

/// One lowering call: its inputs by digest and its answer.
#[derive(Serialize, Deserialize)]
pub struct Step {
    pub call: String,
    pub modules: Vec<String>,
    pub sources: Vec<String>,
    pub files: Vec<String>,
    pub tests: Vec<String>,
    pub package_versions: BTreeMap<String, String>,
    pub overflow_checks: bool,
    pub scalar_products: Option<String>,
    pub scalar_bodies: Vec<(CompilerNodeId, String)>,
    pub record_report: Option<String>,
    pub entry: i64,
    pub kind: String,
    pub progress: bool,
    pub selection: String,
    pub outcome: Outcome,
    pub unions: Vec<NativeUnionDeclaration>,
    /// The product directory's files before the call, by name and blob digest.
    pub products_before: Vec<(String, String)>,
    /// The report directory's files after the call.
    pub report_after: Vec<(String, String)>,
    /// The product directory's files after the call.
    pub products_after: Vec<(String, String)>,
}

/// One unit's lowering sequence.
#[derive(Serialize, Deserialize, Default)]
pub struct Case {
    pub schema: String,
    pub layout_file: String,
    pub layout_modules: Vec<String>,
    pub layout: Vec<Requirement>,
    pub steps: Vec<Step>,
    pub faults: Vec<String>,
}

/// Visit every Body a module holds, in the capture's fixed order.
fn bodies_mut(module: &mut BodyIrModule, visit: &mut dyn FnMut(&mut Body)) {
    for body in &mut module.bodies {
        visit(body);
    }
    for binding in &mut module.decorated_bindings {
        visit(&mut binding.original_body);
        if let Some(body) = binding.initializer.as_deref_mut() {
            visit(body);
        }
    }
    for declaration in &mut module.static_declarations {
        if let StaticInitializer::Computed(body) = &mut declaration.initial {
            visit(body);
        }
    }
    for nominal in &mut module.nominal_declarations {
        if let Some(body) = nominal.field_default_body.as_deref_mut() {
            visit(body);
        }
        for partial in &mut nominal.method_partial_defaults {
            visit(&mut partial.frame);
        }
    }
}

/// Put one Body's skipped facts back.
fn restore_body(body: &mut Body, extra: BodyExtras) {
    body.nominal_instantiation = extra.nominal_instantiation;
    body.trait_default_instantiation =
        extra
            .trait_default
            .map(|(receiver_type, arguments)| TraitDefaultInstantiation {
                receiver_type,
                arguments,
            });
    body.native_attributes = extra.native_attributes;
}

/// Put captured skipped facts back into a decoded module.
pub fn inject(module: &mut BodyIrModule, extras: ModuleExtras) {
    module.runtime_callable_bindings = extras.runtime_callable_bindings;
    module.runtime_binding_scopes = extras.runtime_binding_scopes;
    module.program_entrypoints = extras.program_entrypoints;
    module.published_callables = extras.published_callables;
    let mut bodies = extras.bodies.into_iter();
    bodies_mut(module, &mut |body| {
        if let Some(extra) = bodies.next() {
            restore_body(body, extra);
        }
    });
    let mut frames = extras.declaration_frames.into_iter();
    for callable in &mut module.loaf_callables {
        callable.declaration_frame = frames.next().flatten().map(|(mut frame, extra)| {
            restore_body(&mut frame, extra);
            Box::new(frame)
        });
    }
    let mut owned = extras.owned_callable_fields.into_iter();
    let mut fields = extras.adoption_fields.into_iter();
    for nominal in &mut module.nominal_declarations {
        nominal.owned_callable_fields = owned.next().unwrap_or_default();
        for adoption in &mut nominal.trait_adoptions {
            adoption.fields = fields.next().unwrap_or_default();
        }
    }
}

/// A corpus directory and the blobs it shares between cases.
pub struct Corpus {
    root: PathBuf,
}

impl Corpus {
    pub fn open(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Every case file, in name order.
    pub fn cases(&self) -> Result<Vec<PathBuf>, String> {
        let mut cases: Vec<PathBuf> = std::fs::read_dir(self.root.join("cases"))
            .map_err(|error| format!("cannot list cases: {error}"))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "case"))
            .collect();
        cases.sort();
        Ok(cases)
    }

    pub fn case(&self, path: &Path) -> Result<Case, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        postcard::from_bytes(&bytes).map_err(|error| format!("cannot decode {}: {error}", path.display()))
    }

    pub fn blob(&self, digest: &str) -> Result<Vec<u8>, String> {
        std::fs::read(self.root.join("blobs").join(digest)).map_err(|error| format!("missing blob {digest}: {error}"))
    }

    pub fn text(&self, digest: &str) -> Result<String, String> {
        String::from_utf8(self.blob(digest)?).map_err(|error| format!("blob {digest} is not text: {error}"))
    }

    /// Decode one module reference (`<module digest>+<sidecar digest>`) into the module the reference lowered.
    pub fn module(&self, reference: &str) -> Result<BodyIrModule, String> {
        let (module, extras) = reference
            .split_once('+')
            .ok_or_else(|| format!("invalid module reference {reference}"))?;
        let mut decoded: BodyIrModule =
            postcard::from_bytes(&self.blob(module)?).map_err(|error| format!("module {module}: {error}"))?;
        let extras: ModuleExtras =
            postcard::from_bytes(&self.blob(extras)?).map_err(|error| format!("sidecar {extras}: {error}"))?;
        inject(&mut decoded, extras);
        Ok(decoded)
    }

    pub fn modules(&self, references: &[String]) -> Result<Vec<BodyIrModule>, String> {
        references.iter().map(|reference| self.module(reference)).collect()
    }

    pub fn texts(&self, digests: &[String]) -> Result<Vec<String>, String> {
        digests.iter().map(|digest| self.text(digest)).collect()
    }

    /// Restore a captured directory snapshot into `directory`.
    /// Make a directory's files exactly the captured ones; subdirectories, which a capture does not record, stay.
    pub fn reset(&self, files: &[(String, String)], directory: &Path) -> Result<(), String> {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned());
            if path.is_file() && !files.iter().any(|(file, _)| Some(file) == name.as_ref()) {
                std::fs::remove_file(&path).map_err(|error| error.to_string())?;
            }
        }
        self.restore(files, directory)
    }

    /// The report files the driver published for the plan with this digest, by name, when the capture holds them.
    pub fn publication(&self, digest: &str) -> Result<Option<Vec<(String, Vec<u8>)>>, String> {
        let path = self.root.join("publications").join(format!("{digest}.publication"));
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(None);
        };
        postcard::from_bytes(&bytes)
            .map(Some)
            .map_err(|error| format!("cannot decode {}: {error}", path.display()))
    }

    pub fn restore(&self, files: &[(String, String)], directory: &Path) -> Result<(), String> {
        self.restore_renamed(files, directory, &BTreeMap::new())
    }

    /// Make a directory's files exactly the captured ones, each reference product calling the module initializers
    /// `names` gives instead of the slot-named ones the reference wrote; subdirectories stay.
    pub fn reset_renamed(
        &self,
        files: &[(String, String)],
        directory: &Path,
        names: &BTreeMap<i64, String>,
    ) -> Result<(), String> {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned());
            if path.is_file() && !files.iter().any(|(file, _)| Some(file) == name.as_ref()) {
                std::fs::remove_file(&path).map_err(|error| error.to_string())?;
            }
        }
        self.restore_renamed(files, directory, names)
    }

    fn restore_renamed(
        &self,
        files: &[(String, String)],
        directory: &Path,
        names: &BTreeMap<i64, String>,
    ) -> Result<(), String> {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        for (name, digest) in files {
            let mut bytes = self.blob(digest)?;
            if name.ends_with(".incnfunction")
                && let Some(renamed) = renamed_product(&bytes, names)
            {
                bytes = renamed;
            }
            std::fs::write(directory.join(name), bytes).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Whether any product a case restores was written with a slot-named module initializer.
    pub fn restores_slot_names(&self, case: &Case) -> bool {
        case.steps.iter().any(|step| {
            step.products_before
                .iter()
                .any(|(_, digest)| self.blob(digest).is_ok_and(|bytes| has_slot_name(&bytes)))
        })
    }
}

/// The prefix of a module static initializer's planned name.
const MODULE_INITIALIZER: &str = "__incan_init_module_statics_";

/// Rename a reference plan's module static initializers to the names a produced plan gives the same modules.
///
/// The reference named a module's initializer by its first static slot, which another module's static can shift; this
/// lowering names it by the module's identity. Each reference name is matched to the produced initializer of the same
/// slot, read from the slot that initializer checks first, so a comparison of the renamed plan sees every other
/// difference. Returns the renamed plan's bytes and how many names changed, or nothing when no name changes.
pub fn renamed_initializers(expected: &[u8], produced: &[u8]) -> Option<(Vec<u8>, usize)> {
    let produced: Plan = postcard::from_bytes(produced).ok()?;
    let names = initializer_names(&produced);
    if names.is_empty() {
        return None;
    }
    let mut plan: Plan = postcard::from_bytes(expected).ok()?;
    let renamed = rename_plan(&mut plan, &names);
    if renamed == 0 {
        return None;
    }
    Some((postcard::to_allocvec(&plan).ok()?, renamed))
}

/// Rename a plan's slot-named module initializers and every call to them; returns how many names changed.
fn rename_plan(plan: &mut Plan, names: &BTreeMap<i64, String>) -> usize {
    let mut renamed = 0;
    for function in &mut plan.functions {
        if let Some(name) = slot_name(&function.name, names) {
            function.name = name;
            renamed += 1;
        }
        for block in &mut function.blocks {
            if let TerminatorKind::Call(callee, ..) = &mut block.terminator.kind
                && let CalleeKind::Planned(target) = &mut callee.kind
                && let Some(name) = slot_name(target, names)
            {
                *target = name;
                renamed += 1;
            }
        }
    }
    renamed
}

/// Whether two records report files differ only in the module initializer names their plan functions call: a product
/// event whose product differs only so, or a native interface whose plan does.
pub fn report_differs_only_by_initializers(
    file: &str,
    expected: &[u8],
    produced: &[u8],
    names: &BTreeMap<i64, String>,
) -> bool {
    if file.ends_with(".event") {
        let (Ok(mut want), Ok(mut have)) = (decode_lowering_event(expected), decode_lowering_event(produced)) else {
            return false;
        };
        let (want_product, have_product) = (std::mem::take(&mut want.product), std::mem::take(&mut have.product));
        return want == have
            && (want_product == have_product
                || product_differs_only_by_initializers(&want_product, &have_product, names));
    }
    if file == INTERFACE_REPORT {
        let (Ok(want), Ok(have)) = (decode_native_interface(expected), decode_native_interface(produced)) else {
            return false;
        };
        let (Ok(mut reference), Ok(current)) = (decode_record_plan(&want.plan), decode_record_plan(&have.plan)) else {
            return false;
        };
        rename_plan(&mut reference, names);
        return want.entry == have.entry && reference == current;
    }
    false
}

/// The produced name for a reference initializer name `__incan_init_module_statics_<slot>`.
fn slot_name(name: &str, names: &BTreeMap<i64, String>) -> Option<String> {
    let slot: i64 = name.strip_prefix(MODULE_INITIALIZER)?.parse().ok()?;
    names.get(&slot).cloned()
}

/// Each produced module initializer by the slot its completion probe checks.
fn initializer_names(plan: &Plan) -> BTreeMap<i64, String> {
    let mut names = BTreeMap::new();
    for function in &plan.functions {
        if !function.name.starts_with(MODULE_INITIALIZER) {
            continue;
        }
        for block in &function.blocks {
            if let TerminatorKind::Call(callee, ..) = &block.terminator.kind
                && let CalleeKind::ModuleInitialized(slot) = &callee.kind
            {
                names.insert(*slot, function.name.clone());
            }
        }
    }
    names
}

/// Each produced module initializer's name by the slot it initializes, read from a produced plan's bytes.
pub fn produced_initializers(produced: &[u8]) -> BTreeMap<i64, String> {
    match postcard::from_bytes::<Plan>(produced) {
        Ok(plan) => initializer_names(&plan),
        Err(_) => BTreeMap::new(),
    }
}

/// Rename every reference initializer a function's call terminators name.
fn rename_calls(function: &mut Function, names: &BTreeMap<i64, String>) {
    for block in &mut function.blocks {
        if let TerminatorKind::Call(callee, ..) = &mut block.terminator.kind
            && let CalleeKind::Planned(target) = &mut callee.kind
            && let Some(name) = slot_name(target, names)
        {
            *target = name;
        }
    }
}

/// Whether two keyed product records differ only in the module initializer names their functions call.
///
/// Both carry the same inputs, generic requests and named declarations. Their payloads decode, at the same placeholder
/// declaration positions, to a function, runtime demand and helper frames that are equal once the reference's
/// slot-named initializers take the produced names.
pub fn product_differs_only_by_initializers(expected: &[u8], produced: &[u8], names: &BTreeMap<i64, String>) -> bool {
    let (Ok(want), Ok(have)) = (
        decode_plan_function_record(expected),
        decode_plan_function_record(produced),
    ) else {
        return false;
    };
    if want.inputs != have.inputs || want.requests != have.requests {
        return false;
    }
    let (Ok(identities), Ok(produced_identities)) =
        (product_identities(&want.function), product_identities(&have.function))
    else {
        return false;
    };
    if identities != produced_identities {
        return false;
    }
    let references = placeholder_references(&identities);
    let (Ok(reference), Ok(current)) = (
        decode_product_parts(&want.function, &references),
        decode_product_parts(&have.function, &references),
    ) else {
        return false;
    };
    let (mut function, runtime, mut frames, _) = reference;
    rename_calls(&mut function, names);
    for frame in &mut frames {
        rename_calls(frame, names);
    }
    function == current.0 && runtime == current.1 && frames == current.2
}

/// Whether bytes hold a reference module initializer name, `__incan_init_module_statics_` and a decimal slot.
fn has_slot_name(bytes: &[u8]) -> bool {
    let prefix = MODULE_INITIALIZER.as_bytes();
    bytes
        .windows(prefix.len() + 1)
        .any(|window| window.starts_with(prefix) && window[prefix.len()].is_ascii_digit())
}

/// Placeholder declaration positions, one per named declaration in order, under which a product decodes alone.
fn placeholder_references(identities: &[ProductNominal]) -> Vec<NominalReference> {
    identities
        .iter()
        .enumerate()
        .map(|(index, nominal)| NominalReference {
            kind: nominal.kind.clone(),
            index: index as i64,
            name: String::new(),
        })
        .collect()
}

/// A product record with the same inputs and requests around a function, runtime demand and frames.
fn reencoded(
    record: &PlanFunctionRecord,
    parts: &(Function, Vec<ExternalFunction>, Vec<Function>),
    identities: &[ProductNominal],
) -> Option<Vec<u8>> {
    let payload = encode_product(&parts.0, &parts.1, identities, &parts.2).ok()?;
    let digest = plan_function_digest(&payload, &record.requests).ok()?;
    encode_plan_function_record(&PlanFunctionRecord {
        inputs: record.inputs.clone(),
        function: payload,
        requests: record.requests.clone(),
        digest,
    })
    .ok()
}

/// A reference product whose calls name the produced module initializers, or nothing when it names no slot-named
/// initializer or does not re-encode to its own bytes, in which case a rename could not be trusted.
pub fn renamed_product(encoded: &[u8], names: &BTreeMap<i64, String>) -> Option<Vec<u8>> {
    if names.is_empty() || !has_slot_name(encoded) {
        return None;
    }
    let record = decode_plan_function_record(encoded).ok()?;
    let identities = product_identities(&record.function).ok()?;
    let (function, runtime, frames, _) =
        decode_product_parts(&record.function, &placeholder_references(&identities)).ok()?;
    let mut parts = (function, runtime, frames);
    if reencoded(&record, &parts, &identities)? != encoded {
        return None;
    }
    rename_calls(&mut parts.0, names);
    for frame in &mut parts.2 {
        rename_calls(frame, names);
    }
    reencoded(&record, &parts, &identities)
}
