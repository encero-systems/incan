//! Consume and publish project layout companions from the selected compiler's checked modules.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use incan_lowering::RecordedLayouts;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::semantic_digest::{DigestError, enum_layout_witness, model_layout_witness};
use incan_semantics_core::{CanonicalSymbolId, encode_incan_symbol_identity};
use internal_models::NamespaceReference;
use internal_models_native::namespace_layout_output_directory;
use internal_models_native::namespace_layout_records::{
    LayoutWitness, NamespaceLayoutRecord, decode_namespace_layouts, encode_namespace_layouts,
};
use sha2::{Digest, Sha256};

use super::namespace_admission::FrameAccess;

#[derive(Debug)]
pub(super) struct LayoutPages {
    pub(super) layouts: Vec<NamespaceReference>,
    pub(super) previous: Vec<NamespaceReference>,
    pub(super) access: FrameAccess,
}

impl LayoutPages {
    /// Read the layout companion only when it names the exact checked namespace in the selected graph.
    pub(super) fn read(&self, source: &str, namespace: &NamespaceReference) -> Option<NamespaceLayoutRecord> {
        let reference = reference_for(&self.layouts, source)?;
        if reference.key.origin != namespace.key.origin {
            return None;
        }
        let view = self.access.read(reference).ok()?;
        let layout = decode_namespace_layouts(view.0.as_bytes()).ok()?;
        (layout.namespace == namespace.value_digest).then_some(layout)
    }

    /// Build a rechecked module's companion. Previous inputs are candidates until the complete declaration encoding
    /// agrees. A moved declaration or differently bound nominal therefore takes the ordinary producer path.
    pub(super) fn publish(
        &self,
        source: &str,
        ir: &BodyIrModule,
        digest: String,
        output: &Path,
        recorded: &mut RecordedLayouts,
    ) -> Result<(), String> {
        let mut previous = reference_for(&self.previous, source)
            .and_then(|reference| self.read(source, reference))
            .map(|record| {
                record
                    .layouts
                    .into_iter()
                    .map(|layout| (layout.identity.clone(), layout))
                    .collect()
            })
            .unwrap_or_default();
        let mut layouts = Vec::new();
        for declaration in &ir.nominal_declarations {
            if !declaration.type_parameters.is_empty() || !declaration.native_type_arguments.is_empty() {
                continue;
            }
            if let Some(layout) = checked_layout(source, &declaration.canonical, declaration, &mut previous, || {
                model_layout_witness(declaration)
            }) {
                recorded.record_model(declaration.canonical.clone(), layout.witness.clone());
                layouts.push(layout);
            }
        }
        for declaration in &ir.enum_declarations {
            if !declaration.type_parameters.is_empty() || !declaration.native_type_arguments.is_empty() {
                continue;
            }
            if let Some(layout) = checked_layout(source, &declaration.canonical, declaration, &mut previous, || {
                enum_layout_witness(declaration)
            }) {
                recorded.record_enum(declaration.canonical.clone(), layout.witness.clone());
                layouts.push(layout);
            }
        }
        if !layouts.is_empty() {
            layouts.sort_unstable_by(|left, right| left.identity.cmp(&right.identity));
            let bytes = encode_namespace_layouts(NamespaceLayoutRecord {
                namespace: digest,
                layouts,
            })?;
            let directory = output.join(namespace_layout_output_directory());
            std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            let key = hex::encode(Sha256::digest(source.as_bytes()));
            std::fs::write(directory.join(key), bytes).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// Resolve an exact, uniquely sorted namespace transport address.
pub(super) fn reference_for<'a>(references: &'a [NamespaceReference], source: &str) -> Option<&'a NamespaceReference> {
    references
        .binary_search_by(|entry| entry.key.identity.as_str().cmp(source))
        .ok()
        .map(|index| &references[index])
}

/// Author or restore a source layout. The compact declaration encoding keeps all checked inputs, including default
/// frames and their type bindings. Plain value equality would equate positive and negative floating-point zero.
fn checked_layout(
    source: &str,
    canonical: &CanonicalSymbolId,
    declaration: &impl serde::Serialize,
    previous: &mut BTreeMap<String, LayoutWitness>,
    compute: impl FnOnce() -> Result<(String, String), DigestError>,
) -> Option<LayoutWitness> {
    let identity = encode_incan_symbol_identity(canonical);
    let input_bytes = postcard::to_allocvec(declaration).ok()?;
    let input = format!("sha256:{}", hex::encode(Sha256::digest(input_bytes)));
    if let Some(old) = previous.remove(&identity)
        && old.input == input
    {
        return Some(old);
    }
    incan_lowering::emit_engine_trace("namespace_layout_witness", "", source);
    let witness = compute().ok()?;
    Some(LayoutWitness {
        identity,
        input,
        witness,
    })
}

/// Admit a record for the exact selected namespace and attach witnesses only to the project's checked declarations.
pub(super) fn admit_layouts(ir: &BodyIrModule, record: NamespaceLayoutRecord, recorded: &mut RecordedLayouts) {
    let mut witnesses: BTreeMap<_, _> = record
        .layouts
        .into_iter()
        .map(|row| (row.identity, row.witness))
        .collect();
    for model in &ir.nominal_declarations {
        if model.native_type_arguments.is_empty()
            && let Some(witness) = witnesses.remove(&encode_incan_symbol_identity(&model.canonical))
        {
            recorded.record_model(model.canonical.clone(), witness);
        }
    }
    for enumeration in &ir.enum_declarations {
        if enumeration.native_type_arguments.is_empty()
            && let Some(witness) = witnesses.remove(&encode_incan_symbol_identity(&enumeration.canonical))
        {
            recorded.record_enum(enumeration.canonical.clone(), witness);
        }
    }
}
