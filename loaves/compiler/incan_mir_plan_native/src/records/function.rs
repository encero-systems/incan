//! Round-trip complete native functions through the plan owner's explicitly bounded product codec.
//!
//! A product stores every plan type through the layout type codec, with each model and enum position replaced by a
//! product-local ordinal. The product names each ordinal's declaration by stable identity, so it never retains a
//! position in the unit's tables. Places, operands, operations and statements cross [`crate::records::body`].

use std::collections::BTreeSet;

use incan_semantics_core::body_ir::loaf_item_path;
use incan_semantics_core::{SemanticSourceTargetKind, SymbolNamespace, decode_incan_symbol_identity};

use crate::body::{
    BasicBlock, Callee, CalleeKind, Local, Operand, Parameter, Place, Terminator, TerminatorKind, Unwind,
};
use crate::nominal::{
    NominalKind, NominalReference, NominalRemap, ProductNominal, UnitNominals, binding_remap, capture_remap,
    preserving_remap, resolve_nominals,
};
use crate::plan::{ExternalFunction, Function, GenericLoan};
use crate::positions::{PositionChange, WirePositions};
use crate::records::body::{
    restore_binary, restore_operand, restore_operands, restore_place, restore_span, restore_statement, restore_type,
    restore_types, store_binary, store_operand, store_operands, store_place, store_span, store_statement, store_type,
    store_types,
};
use crate::records::wire_body_records::{StoredOperand, StoredPlace, StoredSpan, read_stored_span};
use crate::records::wire_function_records::{
    StoredBlock, StoredExit, StoredExternal, StoredFunction, StoredLoan, StoredParameter, StoredProduct, StoredSlot,
    StoredTarget, StoredUnwind, read_stored_external, read_stored_function, read_stored_product, wire_stored_external,
    wire_stored_function, wire_stored_product,
};
use crate::records::wire_schema::{STORED_EXTERNAL_SCHEMA, STORED_FUNCTION_SCHEMA, STORED_PRODUCT_SCHEMA};
use crate::types::{PlanType, SourceSpan};
use crate::wire::{Wire, read_wire, write_wire};

/// The root selector of a keyed function product.
const PRODUCT_ROOT: &[u8] = b"G\xc7\x7f$\xca!\x00";
/// The root selector of one runtime dependency signature.
const EXTERNAL_ROOT: &[u8] = b"t\xaa^\x89\x16D\x00";
/// The root selector of a bare function record that names no model or enum.
const FUNCTION_ROOT: &[u8] = b"\x03\xa6%\x81$\xed\x00";
/// The private product schema version.
const SCHEMA: i64 = 7;
/// Source import prefixes retained by the scalar product.
const RUST_BINDING: &str = "rust::";
const PACKAGE_BINDING: &str = "pub::";
/// The one host function of a crate other than Rust's own that the bounded identities name.
const JSON_HOST_FUNCTION: &str = "incan_std_data::json::__private::stringify_or_raise";
/// Why an external has no identity this codec can store.
const UNBOUND_EXTERNAL: &str = "scalar function products refuse an external without an Incan binding identity";

/// A restored product: its function, runtime demand, resolved positions and helper frames.
pub type NamedProduct = (Function, Vec<ExternalFunction>, Vec<NominalReference>, Vec<Function>);
/// A completely restored product: its function, runtime demand, helper frames and named declarations.
pub type ProductParts = (Function, Vec<ExternalFunction>, Vec<Function>, Vec<ProductNominal>);
/// A product placed at this request's positions: its bytes, runtime demand and named declarations.
pub type PlacedProduct = (Vec<u8>, Vec<ExternalFunction>, Vec<ProductNominal>);

/// Name the unit positions a product of this function would retain, in the product's first-seen ordinal order.
pub fn product_nominals(
    function: &Function,
    runtime: &[ExternalFunction],
    frames: &[Function],
) -> Result<Vec<NominalReference>, String> {
    let mut remap = capture_remap();
    stored_product(function, runtime, frames, &mut remap)?;
    Ok(remap.references)
}

/// Retain a complete function, its synthesized helper frames and its dependency requirements, naming every model and
/// enum by declaration identity.
///
/// 1. Loss-check the function, its frames and every external signature field with product-local nominal ordinals.
/// 2. Require one declaration identity for each ordinal, in first-seen order.
/// 3. Restore the product at the captured positions and require exact equality before publication.
pub fn encode_product(
    function: &Function,
    runtime: &[ExternalFunction],
    nominals: &[ProductNominal],
    frames: &[Function],
) -> Result<Vec<u8>, String> {
    // 1. Unsupported carriers still refuse capture rather than omit dependency facts.
    let mut remap = capture_remap();
    let mut stored = stored_product(function, runtime, frames, &mut remap)?;
    // 2. An ordinal without an identity would only be meaningful in the producing unit.
    if nominals.len() != remap.references.len() {
        return Err("plan product names a model or enum without a declaration identity".to_owned());
    }
    if nominals
        .iter()
        .zip(&remap.references)
        .any(|(nominal, reference)| nominal.kind != reference.kind)
    {
        return Err("plan product declaration identity names another table".to_owned());
    }
    stored.nominals = nominals.to_vec();
    let text = write_wire(PRODUCT_ROOT, &STORED_PRODUCT_SCHEMA, wire_stored_product(&stored)?)?;
    // 3. Requirements and positions are part of the native product, not an assumed side effect of lowering again.
    let (restored_function, restored, restored_frames, _) = decode_product_parts(&text, &remap.references)?;
    if restored_function != *function {
        return Err("scalar function product round trip changed native facts".to_owned());
    }
    if restored != runtime {
        return Err("native product round trip changed its dependency requirements".to_owned());
    }
    if restored_frames != frames {
        return Err("native product round trip changed its helper frames".to_owned());
    }
    Ok(text)
}

/// Compare the whole plan's exact encoded function, helper frames and demand with an admitted capture.
pub fn matches_product(
    encoded: &[u8],
    function: &Function,
    runtime: &[ExternalFunction],
    nominals: &[ProductNominal],
    frames: &[Function],
    references: &[NominalReference],
) -> Result<bool, String> {
    let mut remap = capture_remap();
    let mut stored = stored_product(function, runtime, frames, &mut remap)?;
    if remap.references != references || nominals.len() != references.len() {
        return Ok(false);
    }
    stored.nominals = nominals.to_vec();
    let candidate = write_wire(PRODUCT_ROOT, &STORED_PRODUCT_SCHEMA, wire_stored_product(&stored)?)?;
    Ok(candidate == encoded)
}

/// Read the declarations a product names, so its consumer can resolve each one in the current unit.
pub fn product_identities(text: &[u8]) -> Result<Vec<ProductNominal>, String> {
    let stored = decode_product_record(text)?;
    if stored.schema != SCHEMA {
        return Err("unknown native product schema".to_owned());
    }
    Ok(stored.nominals)
}

/// Name a product's helper frames in emission order, without resolving any declaration.
pub fn product_frame_names(text: &[u8]) -> Result<Vec<String>, String> {
    let stored = decode_product_record(text)?;
    if stored.schema != SCHEMA {
        return Err("unknown native product schema".to_owned());
    }
    Ok(stored.frames.into_iter().map(|frame| frame.name).collect())
}

/// Restore a product's helper frames, binding their ordinals exactly as the product's function binds them.
pub fn decode_frames(text: &[u8], references: &[NominalReference]) -> Result<Vec<Function>, String> {
    restore_frames(&decode_product_record(text)?, references)
}

/// Restore the helper frames of one admitted product.
fn restore_frames(stored: &StoredProduct, references: &[NominalReference]) -> Result<Vec<Function>, String> {
    restore_noted_frames(stored, references).map(|(frames, _)| frames)
}

/// Restore the helper frames of one admitted product, with the linked crates their bindings name.
fn restore_noted_frames(
    stored: &StoredProduct,
    references: &[NominalReference],
) -> Result<(Vec<Function>, Vec<String>), String> {
    if stored.schema != SCHEMA {
        return Err("unknown native product schema".to_owned());
    }
    if references.len() != stored.nominals.len() {
        return Err("plan product declaration identities differ from their resolved positions".to_owned());
    }
    let mut remap = binding_remap(references.to_vec());
    let frames = stored
        .frames
        .iter()
        .map(|frame| restore_function(frame, &mut remap))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((frames, remap.crates))
}

/// Restore both parts of a native product without repeating dependency selection.
///
/// 1. Decode and validate the producer's private schema.
/// 2. Bind each product-local ordinal to the unit position its consumer resolved for that declaration.
/// 3. Restore the complete function and every dependency signature.
pub fn decode_product(
    text: &[u8],
    references: &[NominalReference],
) -> Result<(Function, Vec<ExternalFunction>), String> {
    // 1. An unknown schema remains an ordinary cache miss at the store boundary.
    restore_product(&decode_product_record(text)?, references)
}

/// Restore an admitted product's function and runtime demand at the resolved positions.
fn restore_product(
    stored: &StoredProduct,
    references: &[NominalReference],
) -> Result<(Function, Vec<ExternalFunction>), String> {
    restore_noted_product(stored, references).map(|(function, runtime, _)| (function, runtime))
}

/// Restore an admitted product's function and runtime demand, with the linked crates their bindings name.
fn restore_noted_product(
    stored: &StoredProduct,
    references: &[NominalReference],
) -> Result<(Function, Vec<ExternalFunction>, Vec<String>), String> {
    if stored.schema != SCHEMA {
        return Err("unknown native product schema".to_owned());
    }
    // 2. The consumer resolves declarations; this codec only checks that every ordinal has a position.
    if references.len() != stored.nominals.len() {
        return Err("plan product declaration identities differ from their resolved positions".to_owned());
    }
    if references
        .iter()
        .zip(&stored.nominals)
        .any(|(reference, nominal)| reference.kind != nominal.kind)
    {
        return Err("plan product resolved a declaration in another table".to_owned());
    }
    let mut remap = binding_remap(references.to_vec());
    // 3. Both parts cross the private product boundary explicitly.
    let function = restore_function(&stored.function, &mut remap)?;
    let mut restored = Vec::with_capacity(stored.runtime.len());
    for value in &stored.runtime {
        restored.push(restore_external(value, &mut remap)?);
    }
    Ok((function, restored, remap.crates))
}

/// Restore a product for a unit that links `linked`: its function, runtime demand and helper frames, refused when a
/// function or constant it names belongs to another crate.
fn restore_for_unit(
    stored: &StoredProduct,
    references: &[NominalReference],
    linked: &BTreeSet<String>,
) -> Result<(Function, Vec<ExternalFunction>, Vec<Function>), String> {
    let (function, runtime, mut crates) = restore_noted_product(stored, references)?;
    let (frames, frame_crates) = restore_noted_frames(stored, references)?;
    crates.extend(frame_crates);
    if let Some(name) = crates.iter().find(|name| !linked.contains(*name)) {
        return Err(format!(
            "plan product names crate {name}, which this unit does not link"
        ));
    }
    Ok((function, runtime, frames))
}

/// Bind a retained product to this unit and restore its function and helper frames from one typed decode.
pub fn restore_named_product(encoded: &[u8], unit: &UnitNominals) -> Option<NamedProduct> {
    let stored = decode_product_record(encoded).ok()?;
    let resolved = resolve_nominals(&stored.nominals, unit)?;
    let (function, runtime, frames) = restore_for_unit(&stored, &resolved, &BTreeSet::new()).ok()?;
    Some((function, runtime, resolved, frames))
}

/// Validate the native function against its own product-local nominal order after admitting the schema once.
pub fn valid_ordinal_product(encoded: &[u8]) -> bool {
    let Ok(stored) = decode_product_record(encoded) else {
        return false;
    };
    let ordinals: Vec<NominalReference> = stored
        .nominals
        .iter()
        .enumerate()
        .map(|(ordinal, nominal)| NominalReference::unnamed(nominal.kind, ordinal as i64))
        .collect();
    restore_product(&stored, &ordinals).is_ok()
}

/// Restore the complete native product after one admission of its shared schema and bytes.
pub fn decode_product_parts(encoded: &[u8], references: &[NominalReference]) -> Result<ProductParts, String> {
    let stored = decode_product_record(encoded)?;
    let (function, runtime) = restore_product(&stored, references)?;
    let frames = restore_frames(&stored, references)?;
    Ok((function, runtime, frames, stored.nominals))
}

/// Encode a unit's resolved runtime inventory, loss-checked; its positions index the unit's own tables.
pub fn encode_runtime(runtime: &[ExternalFunction]) -> Result<Vec<Vec<u8>>, String> {
    let mut remap = preserving_remap();
    let mut encoded = Vec::with_capacity(runtime.len());
    for value in runtime {
        let stored = store_external(value, &mut remap)?;
        // A unit's inventory travels in its native interface without the product's linked-crate restore check.
        if linked_crate(&stored.path).is_some() {
            return Err(UNBOUND_EXTERNAL.to_owned());
        }
        encoded.push(write_wire(
            EXTERNAL_ROOT,
            &STORED_EXTERNAL_SCHEMA,
            wire_stored_external(&stored)?,
        )?);
    }
    if decode_runtime(&encoded)? != runtime {
        return Err("native runtime inventory round trip changed its signatures".to_owned());
    }
    Ok(encoded)
}

/// Restore a unit's resolved runtime inventory in its recorded order.
pub fn decode_runtime(encoded: &[Vec<u8>]) -> Result<Vec<ExternalFunction>, String> {
    let mut remap = preserving_remap();
    encoded
        .iter()
        .map(|text| restore_external(&decode_external_record(text)?, &mut remap))
        .collect()
}

/// Retain one complete function that names no model or enum; the enclosing product retains its requirements.
pub fn encode_scalar(function: &Function) -> Result<Vec<u8>, String> {
    let mut remap = capture_remap();
    let record = store_function(function, &mut remap)?;
    let text = write_wire(FUNCTION_ROOT, &STORED_FUNCTION_SCHEMA, wire_stored_function(&record)?)?;
    if !remap.references.is_empty() {
        return Err("a bare function record refuses models and enums; retain it as a product".to_owned());
    }
    // A loss in any native field refuses capture instead of becoming an accepted cache hit.
    if decode_scalar(&text)? != *function {
        return Err("scalar function product round trip changed native facts".to_owned());
    }
    Ok(text)
}

/// Restore a bare function record, which names no model or enum.
pub fn decode_scalar(text: &[u8]) -> Result<Function, String> {
    let mut remap = binding_remap(Vec::new());
    let value = read_wire(text, FUNCTION_ROOT, &STORED_FUNCTION_SCHEMA)?;
    restore_function(&read_stored_function(&value, &WirePositions::preserve())?, &mut remap)
}

/// Rebuild only native span positions while retaining every other typed field and all helper frames.
pub fn transform_product_positions(
    encoded: &[u8],
    positions: &WirePositions,
    frame_names: &mut Vec<String>,
) -> Result<Vec<u8>, String> {
    let stored = positioned_record(encoded, positions)?;
    frame_names.extend(stored.frames.iter().map(|frame| frame.name.clone()));
    write_wire(PRODUCT_ROOT, &STORED_PRODUCT_SCHEMA, wire_stored_product(&stored)?)
}

/// Bind the complete native value and its dependencies in the same decode that places its stored spans.
pub fn positioned_product_requirements(
    encoded: &[u8],
    positions: &WirePositions,
    references: &[NominalReference],
) -> Result<PlacedProduct, String> {
    let stored = positioned_record(encoded, positions)?;
    let (_, runtime) = restore_product(&stored, references)?;
    restore_frames(&stored, references)?;
    let positioned = write_wire(PRODUCT_ROOT, &STORED_PRODUCT_SCHEMA, wire_stored_product(&stored)?)?;
    Ok((positioned, runtime, stored.nominals))
}

/// A product read once and placed at this request's positions, kept typed until its consumer has resolved the
/// declarations it names.
///
/// A restored body needs the declarations first and its function, runtime demand and helper frames after. Both come
/// from this one decode: the placed product is not written back to bytes, and those bytes are not read again.
pub struct PlacedRecord {
    stored: StoredProduct,
}

impl PlacedRecord {
    /// The declarations the product names, in its ordinal order.
    pub fn nominals(&self) -> &[ProductNominal] {
        &self.stored.nominals
    }

    /// Restore the function, its runtime demand and its helper frames with each ordinal bound to the unit position
    /// `references` resolved for it.
    ///
    /// A product that names a function of a crate outside `linked`, the crates the restoring unit links, is refused.
    pub fn restore(
        &self,
        references: &[NominalReference],
        linked: &BTreeSet<String>,
    ) -> Result<(Function, Vec<ExternalFunction>, Vec<Function>), String> {
        restore_for_unit(&self.stored, references, linked)
    }

    /// Whether the product restores with every declaration bound to its own product-local ordinal.
    pub fn restores_at_own_ordinals(&self) -> bool {
        let ordinals: Vec<NominalReference> = self
            .stored
            .nominals
            .iter()
            .enumerate()
            .map(|(ordinal, nominal)| NominalReference::unnamed(nominal.kind, ordinal as i64))
            .collect();
        restore_product(&self.stored, &ordinals).is_ok()
    }
}

/// Read one stored product and apply `positions` to its spans in the same decode, keeping the typed value.
///
/// This is [`transform_product_positions`] without the final encoding: restoring the returned record equals decoding
/// the bytes that function returns.
pub fn place_product_record(encoded: &[u8], positions: &WirePositions) -> Result<PlacedRecord, String> {
    Ok(PlacedRecord {
        stored: positioned_record(encoded, positions)?,
    })
}

/// Apply the selected coordinate mapping to one fully admitted typed product.
fn positioned_record(encoded: &[u8], positions: &WirePositions) -> Result<StoredProduct, String> {
    let value = read_wire(encoded, PRODUCT_ROOT, &STORED_PRODUCT_SCHEMA)?;
    let mapping = match &positions.change {
        PositionChange::Relocate(_, _, file, line) => {
            let root = product_root_span(&value, &WirePositions::preserve())?;
            WirePositions::new(PositionChange::Relocate(root.file, root.line, file.clone(), *line))
        }
        PositionChange::PlaceAt(nodes, _, _, file, line) => {
            let placing = WirePositions::new(PositionChange::Place(nodes.clone()));
            let root = product_root_span(&value, &placing)?;
            WirePositions::new(PositionChange::PlaceAt(
                nodes.clone(),
                root.file,
                root.line,
                file.clone(),
                *line,
            ))
        }
        _ => positions.clone(),
    };
    let stored = read_stored_product(&value, &mapping)?;
    if stored.schema != SCHEMA {
        return Err("unknown native product schema".to_owned());
    }
    Ok(stored)
}

/// Read only the declaration span needed to place a product. The subsequent full read validates every frame.
fn product_root_span(value: &Wire, positions: &WirePositions) -> Result<StoredSpan, String> {
    if let Wire::Record(product) = value
        && product.len() == 5
        && let Wire::Record(function) = &product[2].value
        && function.len() == 10
    {
        return read_stored_span(&function[2].value, positions);
    }
    Err("native product has no declaration span".to_owned())
}

/// Admit and read one product record exactly as stored.
fn decode_product_record(encoded: &[u8]) -> Result<StoredProduct, String> {
    let value = read_wire(encoded, PRODUCT_ROOT, &STORED_PRODUCT_SCHEMA)?;
    read_stored_product(&value, &WirePositions::preserve())
}

/// Admit and read one runtime signature record exactly as stored.
fn decode_external_record(encoded: &[u8]) -> Result<StoredExternal, String> {
    let value = read_wire(encoded, EXTERNAL_ROOT, &STORED_EXTERNAL_SCHEMA)?;
    read_stored_external(&value, &WirePositions::preserve())
}

/// Encode the function, its requirements and then its frames, assigning nominal ordinals in that first-seen order.
fn stored_product(
    function: &Function,
    runtime: &[ExternalFunction],
    frames: &[Function],
    remap: &mut NominalRemap,
) -> Result<StoredProduct, String> {
    let encoded = store_function(function, remap)?;
    let mut externals = Vec::with_capacity(runtime.len());
    for value in runtime {
        externals.push(store_external(value, remap)?);
    }
    let mut stored_frames = Vec::with_capacity(frames.len());
    for frame in frames {
        stored_frames.push(store_function(frame, remap)?);
    }
    Ok(StoredProduct {
        schema: SCHEMA,
        function: encoded,
        runtime: externals,
        nominals: Vec::new(),
        frames: stored_frames,
    })
}

/// Encode one complete function's native storage, control flow and source locations.
///
/// 1. Refuse attributes and coroutine state this codec cannot preserve.
/// 2. Encode every plan field explicitly, a closed instance's generic loans included, translating nominal positions
///    through `remap`.
fn store_function(function: &Function, remap: &mut NominalRemap) -> Result<StoredFunction, String> {
    // 1. No invocation-only function field may disappear through a default during replay.
    if function.is_async || function.future_callee.is_some() {
        return Err("scalar function products refuse coroutine state".to_owned());
    }
    if !function.attributes.is_empty() {
        return Err("scalar function products refuse attributes".to_owned());
    }
    // 2. Every plan field crosses the codec explicitly.
    let mut parameters = Vec::with_capacity(function.parameters.len());
    for value in &function.parameters {
        parameters.push(store_parameter(value, remap)?);
    }
    let mut slots = Vec::with_capacity(function.locals.len());
    for value in &function.locals {
        slots.push(store_slot(&value.ty, &value.span, remap)?);
    }
    let mut blocks = Vec::with_capacity(function.blocks.len());
    for value in &function.blocks {
        blocks.push(store_block(value, remap)?);
    }
    let mut loans = Vec::with_capacity(function.generic_loans.len());
    for loan in &function.generic_loans {
        loans.push(StoredLoan {
            slot: loan.slot,
            path: loan.path.clone(),
            parameter: loan.parameter,
            ty: store_type(&loan.ty, remap)?,
        });
    }
    Ok(StoredFunction {
        schema: SCHEMA,
        name: function.name.clone(),
        parameters,
        result: store_type(&function.return_type, remap)?,
        locals: slots,
        blocks,
        span: store_span(&function.span),
        lifetimes: function.signature_lifetimes.clone(),
        loans,
        retained_result_source: function.retained_result_source,
    })
}

/// Restore a function only from a recognized complete record, binding nominal ordinals through `remap`.
fn restore_function(stored: &StoredFunction, remap: &mut NominalRemap) -> Result<Function, String> {
    // Malformed or unfamiliar records are cache misses at the caller.
    if stored.schema != SCHEMA {
        return Err("unknown scalar function product schema".to_owned());
    }
    let mut parameters = Vec::with_capacity(stored.parameters.len());
    for value in &stored.parameters {
        parameters.push(restore_parameter(value, remap)?);
    }
    let mut slots = Vec::with_capacity(stored.locals.len());
    for value in &stored.locals {
        slots.push(Local {
            ty: restore_type(&value.ty, remap)?,
            span: restore_span(&value.span),
        });
    }
    let mut blocks = Vec::with_capacity(stored.blocks.len());
    for value in &stored.blocks {
        blocks.push(restore_block(value, remap)?);
    }
    let mut loans = Vec::with_capacity(stored.loans.len());
    for value in &stored.loans {
        loans.push(GenericLoan {
            slot: value.slot,
            path: value.path.clone(),
            parameter: value.parameter,
            ty: restore_type(&value.ty, remap)?,
        });
    }
    let mut function = Function::new(
        stored.name.clone(),
        parameters,
        restore_type(&stored.result, remap)?,
        slots,
        blocks,
        restore_span(&stored.span),
    );
    function.signature_lifetimes = stored.lifetimes.clone();
    function.generic_loans = loans;
    function.retained_result_source = stored.retained_result_source;
    Ok(function)
}

/// Retain the parameter name, layout type and independent source provenance.
fn store_parameter(value: &Parameter, remap: &mut NominalRemap) -> Result<StoredParameter, String> {
    Ok(StoredParameter {
        name: value.name.clone(),
        slot: store_slot(&value.ty, &value.span, remap)?,
    })
}

/// Restore one complete parameter in its original declaration order.
fn restore_parameter(value: &StoredParameter, remap: &mut NominalRemap) -> Result<Parameter, String> {
    Ok(Parameter {
        name: value.name.clone(),
        ty: restore_type(&value.slot.ty, remap)?,
        span: restore_span(&value.slot.span),
    })
}

/// Retain one local or parameter layout type with its independent diagnostic location.
fn store_slot(ty: &PlanType, span: &SourceSpan, remap: &mut NominalRemap) -> Result<StoredSlot, String> {
    Ok(StoredSlot {
        ty: store_type(ty, remap)?,
        span: store_span(span),
    })
}

/// Retain the Incan binding identity; the native adapter keeps its implementation path private.
fn store_external(value: &ExternalFunction, remap: &mut NominalRemap) -> Result<StoredExternal, String> {
    Ok(StoredExternal {
        path: store_binding(&value.path)?,
        arguments: store_types(&value.type_arguments, remap)?,
        parameters: store_types(&value.parameters, remap)?,
        result: store_type(&value.return_type, remap)?,
        span: store_span(&value.span),
    })
}

/// Select the native implementation only after the Incan-facing record has been accepted.
fn restore_external(value: &StoredExternal, remap: &mut NominalRemap) -> Result<ExternalFunction, String> {
    Ok(ExternalFunction {
        path: restore_named_binding(&value.path, remap)?,
        type_arguments: restore_types(&value.arguments, remap)?,
        parameters: restore_types(&value.parameters, remap)?,
        return_type: restore_type(&value.result, remap)?,
        span: restore_span(&value.span),
    })
}

/// Translate the bounded scalar adapter's implementation paths to stable Incan identities.
pub(crate) fn store_binding(path: &str) -> Result<String, String> {
    if path.contains("::__incan_v") {
        let parts: Vec<&str> = path.split("::").collect();
        let [_, symbol] = parts.as_slice() else {
            return Err("invalid native path for an emitted package callable".to_owned());
        };
        if package_item(symbol)? != path {
            return Err("native path disagrees with its emitted package identity".to_owned());
        }
        return Ok(format!("{PACKAGE_BINDING}{symbol}"));
    }
    match path {
        "incan_native_runtime::initialize_panic_hook" => return Ok("builtin::runtime_initialize".to_owned()),
        "incan_native_runtime::println_text" => return Ok("builtin::print".to_owned()),
        // The native model serializer imports this host function at this exact Rust path.
        JSON_HOST_FUNCTION => return Ok(format!("{RUST_BINDING}{path}")),
        _ => {}
    }
    if let Some(rest) = path.strip_prefix("incan_native_runtime::") {
        return Ok(format!("builtin::runtime::{rest}"));
    }
    if let Some(rest) = path
        .strip_prefix("incan_std_primitives::")
        .or_else(|| path.strip_prefix("incan_stdlib::"))
    {
        return Ok(format!("std::{rest}"));
    }
    // A Rust standard-library function keeps the identity Incan source imports it by, as a runtime generic's
    // `from rust::std::vec import Vec` does. A function of another crate the unit links, as a Loaf's Rust facet
    // exports one, keeps the path that crate exports it at: a product is restored only under the producer profile
    // that keyed it, which fingerprints every dependency interface and Rust facet, and only by a unit that links the
    // crate (`PlacedRecord::restore`).
    if native_crate(path).is_some() {
        return Ok(format!("{RUST_BINDING}{path}"));
    }
    Err(UNBOUND_EXTERNAL.to_owned())
}

/// The crate a native item path `<crate>::<item>` names, when its first segment is a Rust crate name.
pub fn native_crate(path: &str) -> Option<&str> {
    let (name, item) = path.split_once("::")?;
    let mut characters = name.chars();
    let leading = characters.next()?;
    let valid = (leading.is_ascii_alphabetic() || leading == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_');
    (valid && !item.is_empty()).then_some(name)
}

/// The crate a stored binding identity names when it is a crate the unit links, rather than Rust's own crates, the
/// runtime, the standard library or an emitted package; `None` for every other identity.
fn linked_crate(identity: &str) -> Option<&str> {
    let path = identity.strip_prefix(RUST_BINDING)?;
    let name = native_crate(path)?;
    (path != JSON_HOST_FUNCTION && !["std", "core", "alloc"].contains(&name)).then_some(name)
}

/// Recover the package's native item from the compiler's complete canonical symbol projection.
fn package_item(symbol: &str) -> Result<String, String> {
    let canonical = decode_incan_symbol_identity(symbol)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "package binding has no emitted Incan symbol".to_owned())?;
    match (&canonical.namespace, &canonical.kind) {
        (SymbolNamespace::OrdinaryLexical, SemanticSourceTargetKind::Function)
        | (SymbolNamespace::Member, SemanticSourceTargetKind::Method) => {}
        _ => return Err("emitted Incan binding is not a supported callable".to_owned()),
    }
    let path = loaf_item_path(&canonical).ok_or_else(|| "emitted Incan binding has no package provider".to_owned())?;
    if !path.ends_with(&format!("::{symbol}")) {
        return Err("noncanonical emitted Incan package binding".to_owned());
    }
    Ok(path)
}

/// Select the native implementation of an admitted binding identity, noting in `remap` the linked crate it names.
pub(crate) fn restore_named_binding(identity: &str, remap: &mut NominalRemap) -> Result<String, String> {
    if let Some(name) = linked_crate(identity) {
        remap.crates.push(name.to_owned());
    }
    restore_binding(identity)
}

/// Select one native implementation for an admitted Incan binding identity at adapter time.
fn restore_binding(identity: &str) -> Result<String, String> {
    if let Some(symbol) = identity.strip_prefix(PACKAGE_BINDING) {
        return package_item(symbol);
    }
    match identity {
        "builtin::runtime_initialize" => return Ok("incan_native_runtime::initialize_panic_hook".to_owned()),
        "builtin::print" => return Ok("incan_native_runtime::println_text".to_owned()),
        _ => {}
    }
    if let Some(rest) = identity.strip_prefix(RUST_BINDING) {
        return Ok(rest.to_owned());
    }
    if let Some(rest) = identity.strip_prefix("builtin::runtime::") {
        return Ok(format!("incan_native_runtime::{rest}"));
    }
    if let Some(suffix) = identity.strip_prefix("std::") {
        // Standard-library scalar adapters are currently implemented by the primitives crate. The legacy fstring
        // target is the only pre-primitives identity still accepted by this bounded product schema.
        if suffix == "fstring" {
            return Ok("incan_stdlib::fstring".to_owned());
        }
        return Ok(format!("incan_std_primitives::{suffix}"));
    }
    Err("unknown Incan scalar binding identity".to_owned())
}

/// Retain each native unwind continuation exactly.
fn store_unwind(unwind: &Unwind) -> StoredUnwind {
    match unwind {
        Unwind::Continue => StoredUnwind::Continue,
        Unwind::Cleanup(block) => StoredUnwind::Cleanup(*block),
        Unwind::Terminate => StoredUnwind::Terminate,
    }
}

/// Recover cleanup propagation without choosing a new unwind policy.
fn restore_unwind(unwind: &StoredUnwind) -> Unwind {
    match unwind {
        StoredUnwind::Continue => Unwind::Continue,
        StoredUnwind::Cleanup(block) => Unwind::Cleanup(*block),
        StoredUnwind::Terminate => Unwind::Terminate,
    }
}

/// Retain branches, direct calls, explicit drops and overflow checks.
fn store_exit(exit: &TerminatorKind, remap: &mut NominalRemap) -> Result<StoredExit, String> {
    Ok(match exit {
        TerminatorKind::Goto(block) => StoredExit::Goto(*block),
        TerminatorKind::SwitchBool(operand, no, yes) => StoredExit::Switch(store_operand(operand, remap)?, *no, *yes),
        TerminatorKind::Call(callee, arguments, destination, target, unwind) => {
            return store_call(callee, arguments, destination, *target, unwind, remap);
        }
        TerminatorKind::Drop(place, target, unwind) => {
            StoredExit::Drop(store_place(place, remap)?, *target, store_unwind(unwind))
        }
        TerminatorKind::AssertOverflow(flag, op, left, right, target, unwind) => StoredExit::Overflow(
            store_operand(flag, remap)?,
            store_binary(op),
            store_operand(left, remap)?,
            store_operand(right, remap)?,
            *target,
            store_unwind(unwind),
        ),
        TerminatorKind::Return => StoredExit::Return,
        TerminatorKind::Unreachable => StoredExit::Unreachable,
        TerminatorKind::UnwindResume => StoredExit::Resume,
        _ => return Err("scalar function products refuse this native terminator".to_owned()),
    })
}

/// Restore the producer's exact branch, call, drop and overflow control flow.
fn restore_exit(exit: &StoredExit, remap: &mut NominalRemap) -> Result<TerminatorKind, String> {
    Ok(match exit {
        StoredExit::Goto(block) => TerminatorKind::Goto(*block),
        StoredExit::Switch(operand, no, yes) => TerminatorKind::SwitchBool(restore_operand(operand, remap)?, *no, *yes),
        StoredExit::Call(callee, span, arguments, destination, target, unwind) => {
            return restore_call(callee, span, arguments, destination, *target, unwind, remap);
        }
        StoredExit::Drop(place, target, unwind) => {
            TerminatorKind::Drop(restore_place(place, remap)?, *target, restore_unwind(unwind))
        }
        StoredExit::Overflow(flag, op, left, right, target, unwind) => TerminatorKind::AssertOverflow(
            restore_operand(flag, remap)?,
            restore_binary(*op)?,
            restore_operand(left, remap)?,
            restore_operand(right, remap)?,
            *target,
            restore_unwind(unwind),
        ),
        StoredExit::Return => TerminatorKind::Return,
        StoredExit::Unreachable => TerminatorKind::Unreachable,
        StoredExit::Resume => TerminatorKind::UnwindResume,
    })
}

/// Retain every call field; its external requirements are separately retained by the complete product.
fn store_call(
    callee: &Callee,
    arguments: &[Operand],
    destination: &Place,
    target: i64,
    unwind: &Unwind,
    remap: &mut NominalRemap,
) -> Result<StoredExit, String> {
    Ok(StoredExit::Call(
        store_target(&callee.kind, remap)?,
        store_span(&callee.span),
        store_operands(arguments, remap)?,
        store_place(destination, remap)?,
        target,
        store_unwind(unwind),
    ))
}

/// Restore the exact planned target, arguments, destination, successors and source provenance.
fn restore_call(
    callee: &StoredTarget,
    span: &StoredSpan,
    arguments: &[StoredOperand],
    destination: &StoredPlace,
    target: i64,
    unwind: &StoredUnwind,
    remap: &mut NominalRemap,
) -> Result<TerminatorKind, String> {
    Ok(TerminatorKind::Call(
        Callee {
            kind: restore_target(callee, remap)?,
            span: restore_span(span),
        },
        restore_operands(arguments, remap)?,
        restore_place(destination, remap)?,
        target,
        restore_unwind(unwind),
    ))
}

/// Retain direct references, checked instantiations and nominal clones by their owner's product-local ordinal.
fn store_target(target: &CalleeKind, remap: &mut NominalRemap) -> Result<StoredTarget, String> {
    Ok(match target {
        CalleeKind::Planned(name) => StoredTarget::Planned(name.clone()),
        CalleeKind::External(path) => StoredTarget::External(store_binding(path)?),
        CalleeKind::Instantiated(path, arguments) => {
            StoredTarget::Instantiated(store_binding(path)?, store_types(arguments, remap)?)
        }
        CalleeKind::CloneModel(owner, name) => {
            StoredTarget::CloneModel(remap.slot(NominalKind::Model, *owner, name)?.index)
        }
        CalleeKind::CloneEnum(owner, name) => {
            StoredTarget::CloneEnum(remap.slot(NominalKind::Enum, *owner, name)?.index)
        }
        // The formatted carrier names its model or enum by product-local ordinal, like any stored type.
        CalleeKind::FormatValue(ty, debug) => StoredTarget::FormatValue(store_type(ty, remap)?, *debug),
        // A static cell is named by its declaration, through a product-local ordinal like a model or enum.
        CalleeKind::StaticLoad(slot) => StoredTarget::StaticLoad(remap.slot(NominalKind::Static, *slot, "")?.index),
        CalleeKind::StaticStore(slot) => StoredTarget::StaticStore(remap.slot(NominalKind::Static, *slot, "")?.index),
        CalleeKind::StaticForce(slot) => StoredTarget::StaticForce(remap.slot(NominalKind::Static, *slot, "")?.index),
        CalleeKind::StaticMethod(slot, name) => {
            StoredTarget::StaticMethod(remap.slot(NominalKind::Static, *slot, "")?.index, name.clone())
        }
        _ => return store_closure_target(target, remap),
    })
}

/// Retain calls into lifted closure frames and callable values; the frames travel in the same product.
fn store_closure_target(target: &CalleeKind, remap: &mut NominalRemap) -> Result<StoredTarget, String> {
    Ok(match target {
        CalleeKind::ClosureBody(name, captures) => StoredTarget::ClosureBody(name.clone(), *captures),
        CalleeKind::ClosureBodyOnce(name, captures) => StoredTarget::ClosureBodyOnce(name.clone(), *captures),
        CalleeKind::CallClosure(signature) => StoredTarget::CallClosure(store_types(signature, remap)?),
        CalleeKind::CallClosureOnce(signature) => StoredTarget::CallClosureOnce(store_types(signature, remap)?),
        CalleeKind::CallBorrowingClosure(signature) => {
            StoredTarget::CallBorrowingClosure(store_types(signature, remap)?)
        }
        CalleeKind::SharedCallableBody(signature) => StoredTarget::SharedCallableBody(store_types(signature, remap)?),
        CalleeKind::CloneClosure(signature) => StoredTarget::CloneClosure(store_types(signature, remap)?),
        CalleeKind::Value(operand) => StoredTarget::Value(store_operand(operand, remap)?),
        _ => return Err("native products refuse dynamic or owner-dependent callees".to_owned()),
    })
}

/// Restore the exact reference kind, selecting its native implementation in this adapter.
fn restore_target(target: &StoredTarget, remap: &mut NominalRemap) -> Result<CalleeKind, String> {
    Ok(match target {
        StoredTarget::Planned(name) => CalleeKind::Planned(name.clone()),
        StoredTarget::External(identity) => CalleeKind::External(restore_named_binding(identity, remap)?),
        StoredTarget::Instantiated(identity, arguments) => CalleeKind::Instantiated(
            restore_named_binding(identity, remap)?,
            restore_types(arguments, remap)?,
        ),
        StoredTarget::CloneModel(ordinal) => {
            let slot = remap.slot(NominalKind::Model, *ordinal, "")?;
            CalleeKind::CloneModel(slot.index, slot.name)
        }
        StoredTarget::CloneEnum(ordinal) => {
            let slot = remap.slot(NominalKind::Enum, *ordinal, "")?;
            CalleeKind::CloneEnum(slot.index, slot.name)
        }
        StoredTarget::ClosureBody(name, captures) => CalleeKind::ClosureBody(name.clone(), *captures),
        StoredTarget::ClosureBodyOnce(name, captures) => CalleeKind::ClosureBodyOnce(name.clone(), *captures),
        StoredTarget::CallClosure(signature) => CalleeKind::CallClosure(restore_types(signature, remap)?),
        StoredTarget::CallClosureOnce(signature) => CalleeKind::CallClosureOnce(restore_types(signature, remap)?),
        StoredTarget::CallBorrowingClosure(signature) => {
            CalleeKind::CallBorrowingClosure(restore_types(signature, remap)?)
        }
        StoredTarget::SharedCallableBody(signature) => CalleeKind::SharedCallableBody(restore_types(signature, remap)?),
        StoredTarget::CloneClosure(signature) => CalleeKind::CloneClosure(restore_types(signature, remap)?),
        StoredTarget::Value(operand) => CalleeKind::Value(restore_operand(operand, remap)?),
        StoredTarget::FormatValue(ty, debug) => CalleeKind::FormatValue(restore_type(ty, remap)?, *debug),
        StoredTarget::StaticLoad(ordinal) => {
            CalleeKind::StaticLoad(remap.slot(NominalKind::Static, *ordinal, "")?.index)
        }
        StoredTarget::StaticStore(ordinal) => {
            CalleeKind::StaticStore(remap.slot(NominalKind::Static, *ordinal, "")?.index)
        }
        StoredTarget::StaticForce(ordinal) => {
            CalleeKind::StaticForce(remap.slot(NominalKind::Static, *ordinal, "")?.index)
        }
        StoredTarget::StaticMethod(ordinal, name) => {
            CalleeKind::StaticMethod(remap.slot(NominalKind::Static, *ordinal, "")?.index, name.clone())
        }
    })
}

/// Retain the complete block, including its terminator and both block and exit locations.
fn store_block(block: &BasicBlock, remap: &mut NominalRemap) -> Result<StoredBlock, String> {
    let mut statements = Vec::with_capacity(block.statements.len());
    for value in &block.statements {
        statements.push(store_statement(value, remap)?);
    }
    Ok(StoredBlock {
        statements,
        exit: store_exit(&block.terminator.kind, remap)?,
        exit_span: store_span(&block.terminator.span),
        cleanup: block.cleanup,
        span: store_span(&block.span),
    })
}

/// Restore every block field without synthesizing control flow or source provenance.
fn restore_block(block: &StoredBlock, remap: &mut NominalRemap) -> Result<BasicBlock, String> {
    let mut statements = Vec::with_capacity(block.statements.len());
    for value in &block.statements {
        statements.push(restore_statement(value, remap)?);
    }
    Ok(BasicBlock {
        statements,
        terminator: Terminator {
            kind: restore_exit(&block.exit, remap)?,
            span: restore_span(&block.exit_span),
        },
        cleanup: block.cleanup,
        span: restore_span(&block.span),
    })
}
