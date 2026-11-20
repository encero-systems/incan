//! Bounded public-Crumb row setup shared by the compiler's record models.
use internal_models_bindings::{
    AbsentPolicy, Budget, Description, DescriptionMode, KindRegistry, MappedView, Metadata, MetadataEntry, RowRecord,
    RowSet, Value, ValueField, binary_message, complete_view, create_budget, crumb_message, empty_registry,
    mapped_bytes, read_record_at, read_rows, row_count, view_len, write_rows,
};

const OPERATION_BYTES: i64 = 67_108_864;
const OPERATION_ELEMENTS: i64 = 4_194_304;
const OPERATION_DEPTH: i64 = 128;

/// One required Crumb model field with its stable identity and canonical projection policy. `canonical` defaults to
/// true.
#[derive(Clone, PartialEq, Eq)]
pub struct ModelField {
    pub identity: Vec<u8>,
    pub name: String,
    pub storage: String,
    pub canonical: bool,
}
crate::incan_debug::incan_debug!(ModelField {
    identity,
    name,
    storage,
    canonical
});

impl ModelField {
    /// A canonical field, the reference default.
    pub fn new(identity: &[u8], name: &str, storage: &str) -> Self {
        Self {
            identity: identity.to_vec(),
            name: name.to_owned(),
            storage: storage.to_owned(),
            canonical: true,
        }
    }

    /// A field excluded from the canonical projection.
    pub fn non_canonical(identity: &[u8], name: &str, storage: &str) -> Self {
        Self {
            canonical: false,
            ..Self::new(identity, name, storage)
        }
    }
}

/// Bound cumulative Crumb work, including repeated passes over the same logical fields. Pass zero for the reference
/// default minimum.
pub fn operation_budget(minimum_bytes: i64) -> Result<Budget, String> {
    create_budget(OPERATION_BYTES.max(minimum_bytes), OPERATION_ELEMENTS, OPERATION_DEPTH)
        .map_err(|error| crumb_message(&error))
}

/// Keep metadata keys and values in the original declaration order.
pub(crate) fn entry(key: &str, value: Metadata) -> MetadataEntry {
    MetadataEntry {
        key: key.to_owned(),
        value,
    }
}

/// Build one complete referenced Crumb model description in declaration order.
pub fn model_description(selector: &[u8], path: &str, fields: &[ModelField]) -> Description {
    Description {
        metadata: Metadata::Map(vec![
            entry("kind", Metadata::Text("Record".into())),
            entry("nominal", Metadata::Binary(selector.to_vec())),
            entry("path", Metadata::Text(path.to_owned())),
            entry("properties", Metadata::Map(Vec::new())),
            entry("fields", Metadata::List(fields.iter().map(description_field).collect())),
        ]),
        mode: DescriptionMode::Reference,
    }
}

/// Bind one present logical value to a required field contract.
pub fn required_value(identity: &[u8], value: Value) -> ValueField {
    field_value(identity, value, true)
}

/// Bind one present logical value to a required field contract with an explicit canonical projection policy.
pub fn field_value(identity: &[u8], value: Value, canonical: bool) -> ValueField {
    ValueField {
        identity: identity.to_vec(),
        canonical,
        absent: AbsentPolicy::Required,
        value,
    }
}

/// Encode one model's rows through the public Crumb API and enforce the caller's byte boundary.
pub fn encode_rows(description: &Description, records: Vec<RowRecord>, maximum: i64) -> Result<Vec<u8>, String> {
    let mut budget = operation_budget(maximum * 4)?;
    let registry = empty_registry(&mut budget).map_err(|error| crumb_message(&error))?;
    // The standard row writer takes its description list by reference.
    let descriptions = vec![description.clone()];
    let encoded =
        write_rows(&descriptions, &records, &registry, false, &mut budget).map_err(|error| crumb_message(&error))?;
    if encoded.len() as i64 > maximum {
        return Err(format!(
            "encoded Crumb rows need {} bytes, limit {maximum}",
            encoded.len()
        ));
    }
    Ok(encoded)
}

/// Admit one complete envelope and materialize only its bounded logical rows.
pub fn decode_rows(encoded: &[u8], descriptions: &Vec<Description>, maximum: i64) -> Result<Vec<RowRecord>, String> {
    let mut budget = operation_budget(maximum * 4)?;
    let rows = admit_rows(encoded, descriptions, maximum, &mut budget)?;
    materialize_rows(&rows, &mut budget)
}

/// Project each admitted row under the original remaining budget.
fn materialize_rows(rows: &RowSet, budget: &mut Budget) -> Result<Vec<RowRecord>, String> {
    (0..row_count(rows))
        .map(|index| read_record_at(rows, index, budget).map_err(|error| binary_message(&error)))
        .collect()
}

/// Encode one required field's complete metadata.
pub fn description_field(field: &ModelField) -> Metadata {
    Metadata::Map(vec![
        entry("id", Metadata::Binary(field.identity.clone())),
        entry("name", Metadata::Text(field.name.clone())),
        entry("type", Metadata::Text(field.storage.clone())),
        entry("canonical", Metadata::Boolean(field.canonical)),
        entry("absent", Metadata::Text("required".into())),
        entry("properties", Metadata::Map(Vec::new())),
    ])
}

/// Retain the validated logical row view so callers may read a field without decoding its siblings.
pub fn admit_rows(
    encoded: &[u8],
    descriptions: &Vec<Description>,
    maximum: i64,
    budget: &mut Budget,
) -> Result<RowSet, String> {
    if encoded.len() as i64 > maximum {
        return Err("encoded Crumb rows exceed their byte boundary".into());
    }
    let owner = mapped_bytes(&encoded.to_vec())?;
    let view = complete_view(&owner).ok_or("Crumb row envelope has no complete view")?;
    admit_rows_view(&view, descriptions, maximum, budget)
}

/// Admit a borrowed store range with the same envelope and budget checks as owned input.
pub fn admit_rows_view(
    view: &MappedView,
    descriptions: &Vec<Description>,
    maximum: i64,
    budget: &mut Budget,
) -> Result<RowSet, String> {
    if view_len(view) > maximum {
        return Err("encoded Crumb rows exceed their byte boundary".into());
    }
    let registry: KindRegistry = empty_registry(budget).map_err(|error| crumb_message(&error))?;
    read_rows(view, descriptions, &registry, false, budget).map_err(|error| binary_message(&error))
}
