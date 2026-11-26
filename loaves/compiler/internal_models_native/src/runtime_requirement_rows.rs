//! Typed graph recipes for the ordered runtime demands of a successful native unit.

use crate::compiler_records::PlanSourceSpanRecord;
use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{
    decode_graph_row, encode_graph_row, integer, items_value, number_value, text_value, tuple_items_value,
};
use internal_models_bindings::{Description, Value};

const ROW: &[u8] = b"rtreq2\x00";
const BATCHES: &[u8] = b"batch_";

/// One selected runtime lookup in the exact order planning requested it.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeRequirementRow {
    /// The exact provider lookup path, including any encoded spelling chosen by lowering.
    pub path: String,
    /// A complete canonical symbol token when planning already selected the declaration.
    pub canonical: Option<String>,
    /// The saved diagnostic location. Replay still uses the current requesting source site.
    pub span: PlanSourceSpanRecord,
}
crate::incan_debug::incan_debug!(RuntimeRequirementRow { path, canonical, span });

/// Describe the ordered batches without projecting any worker or native plan object into the schema.
fn description() -> Description {
    model_description(
        ROW,
        "incan.engine.runtime-requirements/2",
        &[ModelField::new(
            BATCHES,
            "batches",
            "list<list<tuple<string,option<string>,tuple<string,i64,i64>>>>",
        )],
    )
}

/// Publish the complete ordered recipe, including an empty recipe when a successful plan demanded nothing.
pub fn encode_runtime_requirement_batches(batches: Vec<Vec<RuntimeRequirementRow>>) -> Result<Vec<u8>, String> {
    let mut ordered = Vec::with_capacity(batches.len());
    for batch in batches {
        if batch.is_empty() {
            return Err("empty runtime requirement batch".into());
        }
        let rows = batch.into_iter().map(wire_requirement).collect::<Result<Vec<_>, _>>()?;
        ordered.push(Value::List(rows));
    }
    encode_graph_row(ROW, &description(), vec![required_value(BATCHES, Value::List(ordered))])
}

/// Move the selected path, explicit option, and diagnostic coordinates into their typed tuple positions.
fn wire_requirement(requirement: RuntimeRequirementRow) -> Result<Value, String> {
    let canonical = match requirement.canonical {
        None => Value::NoneValue,
        Some(token) if !token.is_empty() => Value::Some(vec![Value::Text(token)]),
        Some(_) => return Err("empty runtime canonical identity".into()),
    };
    if requirement.path.is_empty() {
        return Err("empty runtime lookup path".into());
    }
    Ok(Value::Tuple(vec![
        Value::Text(requirement.path),
        canonical,
        Value::Tuple(vec![
            Value::Text(requirement.span.file),
            integer(requirement.span.line),
            integer(requirement.span.column),
        ]),
    ]))
}

/// Admit a single referenced recipe under the graph limits and retain the original nested order.
/// The driver still decodes each canonical token and validates the corresponding current Loaf demand.
pub fn decode_runtime_requirement_batches(bytes: &[u8]) -> Result<Vec<Vec<RuntimeRequirementRow>>, String> {
    let [batches]: [_; 1] = decode_graph_row(bytes, ROW, &description(), 1)?
        .try_into()
        .map_err(|_| "invalid runtime requirement recipe fields")?;
    let batches = items_value(batches.value)?;
    let mut ordered = Vec::with_capacity(batches.len());
    for batch in batches {
        let rows = items_value(batch)?;
        if rows.is_empty() {
            return Err("empty runtime requirement batch".into());
        }
        ordered.push(rows.into_iter().map(read_requirement).collect::<Result<Vec<_>, _>>()?);
    }
    Ok(ordered)
}

/// Interpret the exact typed tuple and reject invalid options or empty lookup identities.
fn read_requirement(value: Value) -> Result<RuntimeRequirementRow, String> {
    let [path, canonical, span]: [_; 3] = tuple_items_value(value, 3)?
        .try_into()
        .map_err(|_| "invalid runtime requirement tuple")?;
    let [file, line, column]: [_; 3] = tuple_items_value(span, 3)?
        .try_into()
        .map_err(|_| "invalid runtime source coordinates")?;
    let path = text_value(path)?;
    if path.is_empty() {
        return Err("empty runtime lookup path".into());
    }
    let canonical = match canonical {
        Value::NoneValue => None,
        Value::Some(values) => {
            let [value]: [_; 1] = values
                .try_into()
                .map_err(|_| "invalid runtime canonical identity option")?;
            let token = text_value(value)?;
            if token.is_empty() {
                return Err("empty runtime canonical identity".into());
            }
            Some(token)
        }
        _ => return Err("invalid runtime canonical identity option".into()),
    };
    Ok(RuntimeRequirementRow {
        path,
        canonical,
        span: PlanSourceSpanRecord {
            file: text_value(file)?,
            line: number_value(line)?,
            column: number_value(column)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use incan_semantics_core::{
        CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, decode_incan_symbol_identity,
        encode_incan_symbol_identity,
    };

    use super::{
        BATCHES, PlanSourceSpanRecord, ROW, RuntimeRequirementRow, Value, decode_runtime_requirement_batches,
        description, encode_graph_row, encode_runtime_requirement_batches, required_value,
    };

    /// A diagnostic location that can differ even for an otherwise identical runtime demand.
    fn requirement(path: &str, canonical: Option<String>, line: i64) -> RuntimeRequirementRow {
        RuntimeRequirementRow {
            path: path.to_owned(),
            canonical,
            span: PlanSourceSpanRecord {
                file: "src/runtime.incn".to_owned(),
                line,
                column: 3,
            },
        }
    }

    /// Persist complete ordered demands and distinguish an encoded path from a selected canonical declaration.
    #[test]
    fn runtime_recipe_round_trip_preserves_order_and_source_identity() -> Result<(), Box<dyn std::error::Error>> {
        let authored = CanonicalSymbolId::module_declaration(
            vec!["codec".to_owned()],
            "read",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(7, 12),
        );
        let mut generated = authored.generated_member("shim", SemanticSourceTargetKind::Method);
        generated.declaration_span = generated.declaration_span.with_expansion(Some([3; 32]));
        let authored_token = encode_incan_symbol_identity(&authored);
        let generated_token = encode_incan_symbol_identity(&generated);
        let encoded_path = format!("primitives::{generated_token}");
        let batches = vec![
            vec![requirement("primitives::read", Some(authored_token), 8)],
            vec![
                requirement(&encoded_path, None, 17),
                requirement("primitives::shim", Some(generated_token), 19),
            ],
        ];
        let bytes = encode_runtime_requirement_batches(batches.clone())?;
        assert!(bytes.starts_with(b"CRMB"));
        let decoded = decode_runtime_requirement_batches(&bytes)?;
        assert_eq!(decoded, batches);
        let exact = decoded
            .iter()
            .flatten()
            .filter_map(|row| row.canonical.as_deref())
            .map(decode_incan_symbol_identity)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(exact, [Some(authored), Some(generated)]);
        assert_eq!(encode_runtime_requirement_batches(decoded)?, bytes);

        let empty = encode_runtime_requirement_batches(Vec::new())?;
        assert!(decode_runtime_requirement_batches(&empty)?.is_empty());
        assert!(encode_runtime_requirement_batches(vec![Vec::new()]).is_err());
        assert!(decode_runtime_requirement_batches(&bytes[..bytes.len() - 1]).is_err());
        assert!(decode_runtime_requirement_batches(b"incan.plan.runtime-requirements/1\n\0").is_err());
        let wrong = encode_graph_row(
            ROW,
            &description(),
            vec![required_value(BATCHES, Value::List(vec![Value::List(vec![])]))],
        )?;
        assert!(decode_runtime_requirement_batches(&wrong).is_err());
        Ok(())
    }
}
