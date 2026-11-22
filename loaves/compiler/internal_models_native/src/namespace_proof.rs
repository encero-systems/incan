//! The compiler's physical checked namespace identity, retained with its engine-selected page.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, items, text};
use crate::namespace_rows::NamespaceIdentity;
use internal_models_bindings::{Description, Value};
use sha2::{Digest, Sha256};

const SELECTOR: &[u8] = b"nsid01\x00";
const MAX_PROOF_BYTES: usize = 65_536;

/// Oven requests a proof-capable output directory. Older compilers may publish their existing rows there.
pub fn namespace_proof_output_suffix() -> String {
    ".cmpns5.graph-body-output".to_owned()
}

fn description() -> Description {
    model_description(
        SELECTOR,
        "incan.checked-namespace-identity/1",
        &[
            ModelField::new(b"digest", "physical_binding", "binary"),
            ModelField::new(b"name__", "module_name", "string"),
            ModelField::new(b"path__", "module_path", "list<string>"),
        ],
    )
}

/// Each variable-width identity contributes one fixed-size SHA-256 input in declared order.
fn binding(source: &str, schema: u32, name: &str, path: &[String], object_digest: &[u8]) -> Result<Vec<u8>, String> {
    if source.is_empty() || name.is_empty() || schema == 0 || object_digest.len() != 32 {
        return Err("missing checked namespace physical identity".into());
    }
    let mut hasher = Sha256::new();
    hasher.update(b"incan.checked-namespace-identity/1");
    hasher.update(Sha256::digest(source.as_bytes()));
    hasher.update(Sha256::digest(schema.to_string().as_bytes()));
    hasher.update(Sha256::digest(name.as_bytes()));
    for segment in path {
        if segment.is_empty() {
            return Err("empty segment in checked namespace physical identity".into());
        }
        hasher.update(Sha256::digest(segment.as_bytes()));
    }
    hasher.update(object_digest);
    Ok(hasher.finalize().to_vec())
}

/// Bind a validated compiler declaration to the exact positioned bytes emitted by its writer.
pub fn encode_namespace_proof(
    source: &str,
    schema: u32,
    name: &str,
    path: &[String],
    object_digest: &[u8],
) -> Result<Vec<u8>, String> {
    let digest = binding(source, schema, name, path, object_digest)?;
    let encoded = encode_graph_row(
        SELECTOR,
        &description(),
        vec![
            required_value(b"digest", Value::Binary(digest)),
            required_value(b"name__", Value::Text(name.to_owned())),
            required_value(b"path__", Value::List(path.iter().cloned().map(Value::Text).collect())),
        ],
    )?;
    if encoded.len() > MAX_PROOF_BYTES {
        return Err("checked namespace physical identity exceeds its byte limit".into());
    }
    Ok(encoded)
}

/// Authenticate the compiler's recorded identity against the requested source, reader schema and physical object.
pub fn read_namespace_proof(
    encoded: &[u8],
    source: &str,
    schema: u32,
    object_digest: &[u8],
) -> Result<NamespaceIdentity, String> {
    let (identity, recorded) = proof_fields(encoded)?;
    let expected = binding(source, schema, &identity.name, &identity.path, object_digest)?;
    if recorded == expected {
        Ok(identity)
    } else {
        Err("checked namespace physical identity differs".into())
    }
}

/// The module name and path a proof records, read without binding them to the object they name. A reader that admits
/// a namespace this way authenticates the proof with [`read_namespace_proof`] before it uses the object.
pub fn namespace_proof_identity(encoded: &[u8]) -> Result<NamespaceIdentity, String> {
    proof_fields(encoded).map(|(identity, _)| identity)
}

/// A proof's recorded identity and the binding digest it carries.
fn proof_fields(encoded: &[u8]) -> Result<(NamespaceIdentity, Vec<u8>), String> {
    if encoded.len() > MAX_PROOF_BYTES {
        return Err("checked namespace physical identity exceeds its byte limit".into());
    }
    let [digest, name, path]: [_; 3] = decode_graph_row(encoded, SELECTOR, &description(), 3)?
        .try_into()
        .map_err(|_| "checked namespace physical identity has the wrong field count".to_owned())?;
    let name = text(&name.value)?;
    let path = items(&path.value)?.iter().map(text).collect::<Result<Vec<_>, _>>()?;
    match digest.value {
        Value::Binary(recorded) => Ok((NamespaceIdentity { name, path }, recorded)),
        _ => Err("checked namespace physical identity differs".into()),
    }
}
