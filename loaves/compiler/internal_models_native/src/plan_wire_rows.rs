//! The graph's native plan profile uses compiler-owned Crumb descriptions for its stored functions and layouts.

use crate::crumb_rows::{ModelField, admit_rows, description_field, entry, operation_budget};
use crate::graph_crumb_fields::graph_read_budget;
use internal_models_bindings::{
    Budget, Description, DescriptionMode, MappedView, Metadata, RowAdmission, RowRecord, RowRequest, RowSet, Value,
    binary_message, build_value, charge_bytes, charge_elements, codec_allowance, crumb_message, depth_allowance,
    empty_registry, encode_metadata, read_record_at, refusal, row_count, row_descriptions, settle, view_len,
    view_request, write_rows,
};

const MAX_WIRE_BYTES: i64 = 67_108_864;

/// Write a schema-checked row under the same byte, element and nesting limits as other graph records.
pub fn write_wire(key: &[u8], descriptions: &Vec<Description>, value: Value) -> Result<Vec<u8>, String> {
    let mut budget = graph_read_budget()?;
    let registry = empty_registry(&mut budget).map_err(|error| crumb_message(&error))?;
    let rows = vec![RowRecord {
        key: key.to_vec(),
        value,
    }];
    let encoded =
        write_rows(descriptions, &rows, &registry, false, &mut budget).map_err(|error| crumb_message(&error))?;
    if encoded.len() as i64 > MAX_WIRE_BYTES {
        return Err("native plan wire exceeds its byte limit".into());
    }
    Ok(encoded)
}

/// Authenticate every reachable schema and the sole root selector before interpreting the native payload.
pub fn read_wire(encoded: &[u8], key: &[u8], supplied: &Vec<Description>) -> Result<Value, String> {
    let mut budget = graph_read_budget()?;
    let rows = admit_wire(encoded, supplied, &mut budget)?;
    let row = read_record_at(&rows, 0, &mut budget).map_err(|error| binary_message(&error))?;
    if row.key != key {
        return Err("unexpected native plan root selector".into());
    }
    Ok(row.value)
}

/// Admit the complete envelope and require exactly the supplied referenced schema closure.
pub fn admit_wire(encoded: &[u8], supplied: &Vec<Description>, budget: &mut Budget) -> Result<RowSet, String> {
    let rows = admit_rows(encoded, supplied, MAX_WIRE_BYTES, budget)?;
    let dictionary = row_descriptions(&rows, budget).map_err(|error| binary_message(&error))?;
    if row_count(&rows) != 1 || dictionary.len() != supplied.len() {
        return Err("native plan wire requires one row and its exact declaration closure".into());
    }
    if dictionary.iter().any(|entry| entry.mode != DescriptionMode::Reference) {
        return Err("native plan wire requires referenced compiler schemas".into());
    }
    Ok(rows)
}

/// Retain an exact reference dictionary for repeated field reads of one compiler-owned root.
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedWireReader {
    pub(crate) root: Vec<u8>,
    pub(crate) dictionary: Vec<Vec<u8>>,
    pub(crate) name: String,
}
crate::incan_debug::incan_debug!(PreparedWireReader { root, dictionary, name });

impl PreparedWireReader {
    /// Authenticate complete owned bytes against the compiler's closed reference dictionary. `slots` receives the
    /// supplied-description position of each dictionary entry.
    pub fn admit(&self, encoded: &[u8], budget: &mut Budget, slots: &mut Vec<i64>) -> Result<RowAdmission, String> {
        if encoded.len() as i64 > MAX_WIRE_BYTES {
            return Err(format!("{} exceeds its byte limit", self.name));
        }
        self.admit_request(RowRequest::new(encoded.to_vec(), false, true), budget, slots)
    }

    /// Authenticate one bounded envelope already anchored in immutable storage.
    pub fn admit_view(&self, view: &MappedView, budget: &mut Budget) -> Result<RowAdmission, String> {
        if view_len(view) > MAX_WIRE_BYTES {
            return Err(format!("{} exceeds its byte limit", self.name));
        }
        let mut slots = Vec::new();
        self.admit_request(view_request(view, false, true), budget, &mut slots)
    }

    /// Apply the same closed reference dictionary and budget to either backing.
    fn admit_request(
        &self,
        mut request: RowRequest,
        budget: &mut Budget,
        slots: &mut Vec<i64>,
    ) -> Result<RowAdmission, String> {
        for entry in &self.dictionary {
            request.supply(entry.clone());
        }
        let (remaining_bytes, remaining_elements) = codec_allowance(budget);
        request.allow(remaining_bytes, remaining_elements, depth_allowance(budget));
        let row = request.admit();
        charge_bytes(budget, row.bytes_charged).map_err(|error| crumb_message(&error))?;
        charge_elements(budget, row.elements_charged).map_err(|error| crumb_message(&error))?;
        if !row.admitted() {
            let error = refusal(
                row.category.clone(),
                row.offset,
                row.field.clone(),
                row.variant.clone(),
                row.cause.clone(),
            );
            return Err(binary_message(&error));
        }
        if row.records != 1 || row.key(0) != self.root || row.description_count() != self.dictionary.len() as i64 {
            return Err(format!(
                "{} requires one root and its complete declaration dictionary",
                self.name
            ));
        }
        for index in 0..row.description_count() {
            let slot = row.reference_index(index);
            if slot < 0 {
                return Err(format!("{} requires referenced compiler descriptions", self.name));
            }
            slots.push(slot);
        }
        Ok(row)
    }

    /// Encode the closed reference dictionary once for every later admission.
    pub fn create(root: &[u8], descriptions: &[Description], name: &str) -> Result<Self, String> {
        let mut boundary = operation_budget(0)?;
        let dictionary = descriptions
            .iter()
            .map(|description| encode_metadata(&description.metadata, &mut boundary).map_err(|e| crumb_message(&e)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            root: root.to_vec(),
            dictionary,
            name: name.to_owned(),
        })
    }
}

/// Read and build one admitted field of the root row under the caller's remaining allowance.
pub fn wire_field(row: &RowAdmission, selector: &[u8], budget: &mut Budget) -> Result<Value, String> {
    let (remaining_bytes, remaining_elements) = codec_allowance(budget);
    let field = row.read_field(
        0,
        selector.to_vec(),
        remaining_bytes,
        remaining_elements,
        depth_allowance(budget),
    );
    settle(&field, budget).map_err(|error| binary_message(&error))?;
    build_value(field).map_err(|error| binary_message(&error))
}

/// Describe a closed native alternative and its identity-addressed positional payload.
pub fn variant_schema(identity: &[u8], name: &str, fields: &[ModelField]) -> Metadata {
    Metadata::Map(vec![
        entry("id", Metadata::Binary(identity.to_vec())),
        entry("name", Metadata::Text(name.to_owned())),
        entry("fields", Metadata::List(fields.iter().map(description_field).collect())),
        entry("properties", Metadata::Map(Vec::new())),
    ])
}

/// A graph-local sum type with stable member identities and explicit positional types.
pub fn enum_schema(key: &[u8], name: &str, variants: Vec<Metadata>) -> Description {
    Description {
        mode: DescriptionMode::Reference,
        metadata: Metadata::Map(vec![
            entry("kind", Metadata::Text("Enum".into())),
            entry("nominal", Metadata::Binary(key.to_vec())),
            entry("path", Metadata::Text(name.to_owned())),
            entry("properties", Metadata::Map(Vec::new())),
            entry("variants", Metadata::List(variants)),
        ]),
    }
}
