//! The golden lowering corpus: the reference lowering's inputs and answers, captured at the driver boundary.
//!
//! A corpus directory holds `blobs/<sha256>` (Body IR module bytes, their sidecars of serde-skipped facts, source
//! texts and product files, each stored once) and `cases/<sha256>.case`, one per unit: the layout request and every
//! lowering call in order, with its inputs by digest and the reference answer. The record shapes here are the
//! capture's own; postcard is positional, so their field order is the format.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    pub fn restore(&self, files: &[(String, String)], directory: &Path) -> Result<(), String> {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        for (name, digest) in files {
            std::fs::write(directory.join(name), self.blob(digest)?).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
