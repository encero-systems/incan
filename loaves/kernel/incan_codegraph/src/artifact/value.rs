//! A self-describing postcard value for the tagged graph wire schema, with shared field names in the index.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::GraphArtifactError;

/// Postcard cannot deserialize serde's internally tagged graph records or arbitrary JSON values directly. These
/// external variants preserve every wire field while the artifact index interns repeated field names once.
#[derive(Serialize, Deserialize)]
pub(super) enum GraphValue {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Text(String),
    Array(Vec<GraphValue>),
    Object(Vec<(u32, GraphValue)>),
}

impl GraphValue {
    /// Encode a wire value without stringifying its payload or imposing a second semantic schema.
    pub(super) fn pack(value: Value, names: &mut BTreeMap<String, u32>) -> Result<Self, GraphArtifactError> {
        Ok(match value {
            Value::Null => Self::Null,
            Value::Bool(value) => Self::Bool(value),
            Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    Self::Signed(value)
                } else if let Some(value) = value.as_u64() {
                    Self::Unsigned(value)
                } else {
                    Self::Float(
                        value
                            .as_f64()
                            .ok_or_else(|| GraphArtifactError::Malformed("invalid number".into()))?,
                    )
                }
            }
            Value::String(value) => Self::Text(value),
            Value::Array(values) => Self::Array(
                values
                    .into_iter()
                    .map(|value| Self::pack(value, names))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(values) => {
                let mut fields = Vec::with_capacity(values.len());
                for (name, value) in values {
                    let next = u32::try_from(names.len())
                        .map_err(|_| GraphArtifactError::Malformed("too many field names".into()))?;
                    let key = *names.entry(name).or_insert(next);
                    fields.push((key, Self::pack(value, names)?));
                }
                Self::Object(fields)
            }
        })
    }

    /// Restore the exact wire value, refusing corrupt dictionary references and nonfinite numeric payloads.
    pub(super) fn unpack(self, names: &[String]) -> Result<Value, GraphArtifactError> {
        Ok(match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(value),
            Self::Signed(value) => Value::from(value),
            Self::Unsigned(value) => Value::from(value),
            Self::Float(value) => Value::Number(
                serde_json::Number::from_f64(value)
                    .ok_or_else(|| GraphArtifactError::Malformed("invalid floating-point payload".into()))?,
            ),
            Self::Text(value) => Value::String(value),
            Self::Array(values) => Value::Array(
                values
                    .into_iter()
                    .map(|value| value.unpack(names))
                    .collect::<Result<_, _>>()?,
            ),
            Self::Object(values) => {
                let mut fields = serde_json::Map::new();
                for (key, value) in values {
                    let index = usize::try_from(key)
                        .map_err(|_| GraphArtifactError::Malformed("invalid field index".into()))?;
                    let name = names
                        .get(index)
                        .ok_or_else(|| GraphArtifactError::Malformed("unknown field index".into()))?;
                    if fields.insert(name.clone(), value.unpack(names)?).is_some() {
                        return Err(GraphArtifactError::Malformed("duplicate record field".into()));
                    }
                }
                Value::Object(fields)
            }
        })
    }
}
