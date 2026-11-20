//! Public compiler row helpers retaining the standard library's general logical model.
use internal_models_bindings::{
    AbsentPolicy, Budget, Description, DescriptionMode, KindRegistry, Metadata, MetadataEntry, RowRecord, Value,
    ValueField, binary_message, complete_view, create_budget, crumb_message, empty_registry, mapped_bytes,
    read_record_at, read_rows, row_count, write_rows,
};

/// A required model field with its stable identity and canonical projection policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelField {
    pub identity: Vec<u8>,
    pub name: String,
    pub storage: String,
    pub canonical: bool,
}

/// Create the shared bound for one compiler-record operation.
pub fn operation_budget() -> Result<Budget, String> {
    create_budget(67_108_864, 4_194_304, 128).map_err(|error| crumb_message(&error))
}

/// Keep metadata keys and values in the original declaration order.
fn entry(key: &str, value: Metadata) -> MetadataEntry {
    MetadataEntry {
        key: key.to_owned(),
        value,
    }
}

/// Construct one logical description from either general fields or static compiler schema fields.
pub(crate) fn description_from_fields<'a>(
    selector: &[u8],
    path: &str,
    fields: impl Iterator<Item = (&'a [u8], &'a str, &'a str, bool)>,
) -> Description {
    let metadata = fields
        .map(|(identity, name, storage, canonical)| {
            Metadata::Map(vec![
                entry("id", Metadata::Binary(identity.to_vec())),
                entry("name", Metadata::Text(name.to_owned())),
                entry("type", Metadata::Text(storage.to_owned())),
                entry("canonical", Metadata::Boolean(canonical)),
                entry("absent", Metadata::Text("required".into())),
                entry("properties", Metadata::Map(Vec::new())),
            ])
        })
        .collect();
    Description {
        metadata: Metadata::Map(vec![
            entry("kind", Metadata::Text("Record".into())),
            entry("nominal", Metadata::Binary(selector.to_vec())),
            entry("path", Metadata::Text(path.to_owned())),
            entry("properties", Metadata::Map(Vec::new())),
            entry("fields", Metadata::List(metadata)),
        ]),
        mode: DescriptionMode::Reference,
    }
}

/// Build a referenced Crumb description in declaration order.
pub fn model_description(selector: &[u8], path: &str, fields: &[ModelField]) -> Description {
    description_from_fields(
        selector,
        path,
        fields
            .iter()
            .map(|f| (f.identity.as_slice(), f.name.as_str(), f.storage.as_str(), f.canonical)),
    )
}

/// Move an already owned compiler value directly into its required field.
pub(crate) fn owned_field(identity: &[u8], value: Value, canonical: bool) -> ValueField {
    ValueField {
        identity: identity.to_vec(),
        canonical,
        absent: AbsentPolicy::Required,
        value,
    }
}

/// Bind a borrowed logical value to a present, required field.
pub fn required_value(identity: &[u8], value: &Value, canonical: bool) -> ValueField {
    // The returned field must retain the logical value after this borrow ends.
    owned_field(identity, value.clone(), canonical)
}

/// Use the original byte and element limits and create the empty local kind registry.
fn row_budget(maximum: i64) -> Result<(Budget, KindRegistry), String> {
    let mut budget = create_budget(maximum.wrapping_mul(4), 4_194_304, 128).map_err(|error| crumb_message(&error))?;
    let registry = empty_registry(&mut budget).map_err(|error| crumb_message(&error))?;
    Ok((budget, registry))
}

/// Encode one model's rows under the public Crumb budget and caller's final byte limit.
pub fn encode_rows(description: &Description, records: &Vec<RowRecord>, maximum: i64) -> Result<Vec<u8>, String> {
    let (mut budget, registry) = row_budget(maximum)?;
    let encoded = write_rows(
        // The existing Incan row writer requires a borrowed Vec of owned descriptions.
        &vec![description.clone()],
        records,
        &registry,
        false,
        &mut budget,
    )
    .map_err(|error| crumb_message(&error))?;
    if i64::try_from(encoded.len()).map_or(true, |size| size > maximum) {
        return Err("encoded Crumb rows exceed their byte boundary".into());
    }
    Ok(encoded)
}

/// Restore each logical row in order under the same allowance used for admission.
pub fn decode_rows(encoded: &Vec<u8>, descriptions: &Vec<Description>, maximum: i64) -> Result<Vec<RowRecord>, String> {
    if i64::try_from(encoded.len()).map_or(true, |size| size > maximum) {
        return Err("encoded Crumb rows exceed their byte boundary".into());
    }
    let (mut budget, registry) = row_budget(maximum)?;
    let owner = mapped_bytes(encoded)?;
    let view = complete_view(&owner).ok_or("Crumb row envelope has no complete view")?;
    let rows = read_rows(&view, descriptions, &registry, false, &mut budget).map_err(|error| binary_message(&error))?;
    (0..row_count(&rows))
        .map(|index| read_record_at(&rows, index, &mut budget).map_err(|error| binary_message(&error)))
        .collect()
}
