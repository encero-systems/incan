//! Layout witnesses published beside one checked editable namespace.

use crate::checker_field_grammar::full_digest;
use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, text_value};
use internal_models_bindings::{Description, Value};

const ROW: &[u8] = b"nslay1\x00";

/// Inputs and witnesses carried by a successful checked namespace.
pub struct NamespaceLayoutRecord {
    /// Digest of the complete checked namespace value produced alongside these layouts.
    pub namespace: String,
    /// Source layouts in identity order, with their checked input identities for rechecking this namespace.
    pub layouts: Vec<LayoutWitness>,
}

/// One model or enum and its witness, with the exact declaration serialization that can prove it again.
pub struct LayoutWitness {
    /// The compiler's canonical symbol token. Also identifies whether this declaration is a model or enum.
    pub identity: String,
    /// Digest of the exact producer input, used only when checking rebuilt the enclosing namespace.
    pub input: String,
    /// The lowering's specialization and layout version digests.
    pub witness: (String, String),
}

/// Describe the durable Crumb layout companion, without imposing a schema on the compiler's checked declaration.
fn description() -> Description {
    model_description(
        ROW,
        "incan.compiler.namespace-layouts/1",
        &[
            ModelField::new(b"layout", "layouts", "list<tuple<string,string,string,string>>"),
            ModelField::new(b"ns____", "namespace", "string"),
        ],
    )
}

/// A layout companion can stand beside one exact namespace and name each admitted declaration once.
fn validate(record: &NamespaceLayoutRecord) -> Result<(), String> {
    if !full_digest(&format!("sha256:{}", record.namespace)) || record.layouts.is_empty() {
        return Err("invalid checked namespace layout companion".into());
    }
    let mut previous = "";
    for row in &record.layouts {
        let (specialization, version) = &row.witness;
        if row.identity.as_str() <= previous
            || !full_digest(&row.input)
            || !full_digest(specialization)
            || !full_digest(version)
        {
            return Err("invalid or unordered checked layout witness".into());
        }
        previous = &row.identity;
    }
    Ok(())
}

/// Encode the checked layout companion once; its caller publishes it under the same receipt as its namespace.
pub fn encode_namespace_layouts(record: NamespaceLayoutRecord) -> Result<Vec<u8>, String> {
    validate(&record)?;
    let layouts = record
        .layouts
        .into_iter()
        .map(|row| {
            let (specialization, version) = row.witness;
            Value::Tuple(vec![
                Value::Text(row.identity),
                Value::Text(row.input),
                Value::Text(specialization),
                Value::Text(version),
            ])
        })
        .collect();
    encode_graph_row(
        ROW,
        &description(),
        vec![
            required_value(b"layout", Value::List(layouts)),
            required_value(b"ns____", Value::Text(record.namespace)),
        ],
    )
}

/// Read a bounded companion. Its namespace digest still needs to be matched to the selected producer value.
pub fn decode_namespace_layouts(bytes: &[u8]) -> Result<NamespaceLayoutRecord, String> {
    let [layouts, namespace]: [_; 2] = decode_graph_row(bytes, ROW, &description(), 2)?
        .try_into()
        .map_err(|_| "invalid checked namespace layout fields")?;
    let Value::List(layouts) = layouts.value else {
        return Err("invalid checked namespace layout inventory".into());
    };
    let layouts = layouts
        .into_iter()
        .map(|row| {
            let Value::Tuple(row) = row else {
                return Err("expected a checked layout tuple".into());
            };
            let [identity, input, specialization, version]: [_; 4] =
                row.try_into().map_err(|_| "invalid checked layout tuple")?;
            Ok(LayoutWitness {
                identity: text_value(identity)?,
                input: text_value(input)?,
                witness: (text_value(specialization)?, text_value(version)?),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let record = NamespaceLayoutRecord {
        namespace: text_value(namespace.value)?,
        layouts,
    };
    validate(&record)?;
    Ok(record)
}
