//! Checking order from the compiler's transitive project import closures.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{KEY_TYPE, decode_graph_row, encode_graph_row, items, key_value, read_key, text};
use crate::hex_text::{hexlify, unhexlify};
use crate::record_scope::RecordKey;
use crate::store_record_source::encode_record_key;
use internal_models_bindings::{Description, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub const CHECKING_BASIS: &str = "incan.compiler.checking-basis/2";
const BASIS_SELECTOR: &[u8] = b"chbase\x00";
const ORDER_SELECTOR: &[u8] = b"nsrank\x00";
const COMPLETION_SELECTOR: &[u8] = b"chdone\x00";
pub const IMPORT_ORDER_FILE: &str = "import-closures.crumb";
const IMPORT_ORDER_ROW: &[u8] = b"imprnk\x00";

/// One completed worker value and its exact source receipt before the next stage.
fn completion_description() -> Description {
    model_description(
        COMPLETION_SELECTOR,
        "incan.compiler.checking-completion/1",
        &[
            ModelField::new(b"receip", "receipt", "binary"),
            ModelField::new(b"result", "value_root", "string"),
        ],
    )
}

/// Preserve the worker's produced value root together with its authenticated input receipt.
pub fn encode_checking_completion(value_root: &str, receipt: &[u8]) -> Result<Vec<u8>, String> {
    encode_graph_row(
        COMPLETION_SELECTOR,
        &completion_description(),
        vec![
            required_value(b"receip", Value::Binary(receipt.to_vec())),
            required_value(b"result", Value::Text(value_root.to_owned())),
        ],
    )
}

/// Require the exact earlier producer output before admitting its read evidence.
pub fn checking_completion_receipt(encoded: &[u8], value: &[u8]) -> Result<Vec<u8>, String> {
    let fields = decode_graph_row(encoded, COMPLETION_SELECTOR, &completion_description(), 2)?;
    if text(&fields[1].value)? != hexlify(&Sha256::digest(value)) {
        return Err("completed checking value differs from its producer root".into());
    }
    match &fields[0].value {
        Value::Binary(receipt) => Ok(receipt.clone()),
        _ => Err("completed checking value has no receipt".into()),
    }
}

/// Name the last completed supplier evidence for the exact requested checking output.
pub fn checking_basis_key(output: &RecordKey) -> Result<String, String> {
    if !matches!(output.family.as_str(), "local" | "final") {
        return Err("checking preparation requires a local or final output".into());
    }
    let key = encode_record_key(output)?;
    Ok(format!(
        "{CHECKING_BASIS}\t{}",
        hexlify(&Sha256::digest(key.as_bytes()))
    ))
}

fn basis_description() -> Description {
    model_description(
        BASIS_SELECTOR,
        "incan.compiler.checking-basis/2",
        &[
            ModelField::new(b"key___", "output", KEY_TYPE),
            ModelField::new(b"receip", "receipt_sha256", "binary"),
        ],
    )
}

/// Authenticate a local or final producer's exact completed receipt under its own key.
pub fn encode_checking_basis(output: &RecordKey, root: &str) -> Result<Vec<u8>, String> {
    let digest = unhexlify(root)?;
    if !matches!(output.family.as_str(), "local" | "final") || digest.len() != 32 || hexlify(&digest) != root {
        return Err("invalid checking basis".into());
    }
    encode_graph_row(
        BASIS_SELECTOR,
        &basis_description(),
        vec![
            required_value(b"key___", key_value(output)),
            required_value(b"receip", Value::Binary(digest)),
        ],
    )
}

/// Read the completed receipt digest recorded for exactly this output.
pub fn read_checking_basis(output: &RecordKey, encoded: &[u8]) -> Result<String, String> {
    let fields = decode_graph_row(encoded, BASIS_SELECTOR, &basis_description(), 2)?;
    if read_key(&fields[0].value)? != *output {
        return Err("checking basis belongs to another output".into());
    }
    match &fields[1].value {
        Value::Binary(digest) if digest.len() == 32 => Ok(hexlify(digest)),
        _ => Err("invalid checking basis receipt".into()),
    }
}

/// Bind the project identity, mutually importing namespaces and full predecessor closure.
fn order_description() -> Description {
    model_description(
        ORDER_SELECTOR,
        "incan.compiler.namespace-order/1",
        &[
            ModelField::new(b"closed", "closure", "list<string>"),
            ModelField::new(b"member", "members", "list<string>"),
            ModelField::new(b"origin", "origin", "string"),
        ],
    )
}

/// Strict predecessor count and rank evidence for one module or one mutually importing group.
///
/// Defaults: `rank` zero, `evidence` `"compiled-check"` (older and partial publications have no certificate for
/// independent scheduling), `members` and `certificate` empty.
#[derive(Clone, PartialEq, Eq)]
pub struct NamespaceOrder {
    pub rank: i64,
    pub evidence: String,
    pub members: Vec<String>,
    pub certificate: Vec<u8>,
}
crate::incan_debug::incan_debug!(NamespaceOrder {
    rank,
    evidence,
    members,
    certificate
});

impl Default for NamespaceOrder {
    /// The reference model's field defaults.
    fn default() -> Self {
        Self {
            rank: 0,
            evidence: "compiled-check".to_owned(),
            members: Vec::new(),
            certificate: Vec::new(),
        }
    }
}

impl NamespaceOrder {
    /// Whether this authenticated namespace includes every source consumed by a unit-level producer.
    pub fn covers(&self, sources: &HashSet<String>) -> Result<bool, String> {
        if sources.is_empty()
            || sources.len() as i64 > self.rank + self.members.len() as i64
            || self.certificate.is_empty()
        {
            return Ok(false);
        }
        let fields = decode_graph_row(&self.certificate, ORDER_SELECTOR, &order_description(), 3)?;
        let closure: HashSet<String> = namespace_names(&fields[0].value)?.into_iter().collect();
        Ok(sources.iter().all(|source| closure.contains(source)))
    }
}

/// Emit the canonical typed Crumb value used for both publication and verification.
fn encoded_order(origin: &str, members: &[String], closure: &[String]) -> Result<Vec<u8>, String> {
    let names = |names: &[String]| Value::List(names.iter().cloned().map(Value::Text).collect());
    encode_graph_row(
        ORDER_SELECTOR,
        &order_description(),
        vec![
            required_value(b"closed", names(closure)),
            required_value(b"member", names(members)),
            required_value(b"origin", Value::Text(origin.to_owned())),
        ],
    )
}

/// Require the canonical strictly increasing spelling of a nonempty namespace set.
fn namespace_names(value: &Value) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::new();
    for element in items(value)? {
        let name = text(element)?;
        if name.is_empty() || names.last().is_some_and(|last| name <= *last) {
            return Err("noncanonical certified namespace names".into());
        }
        names.push(name);
    }
    if names.is_empty() {
        return Err("empty certified namespace set".into());
    }
    Ok(names)
}

/// Recompute the canonical rank certificate and require its full origin-bound Crumb payload.
pub fn decode_namespace_order(certificate: &[u8], expected_origin: &str) -> Result<NamespaceOrder, String> {
    let fields = decode_graph_row(certificate, ORDER_SELECTOR, &order_description(), 3)?;
    let closure = namespace_names(&fields[0].value)?;
    let members = namespace_names(&fields[1].value)?;
    if text(&fields[2].value)? != expected_origin {
        return Err("certified namespace order belongs to another origin".into());
    }
    if members.iter().any(|member| !closure.contains(member)) {
        return Err("certified namespace group lies outside its closure".into());
    }
    if encoded_order(expected_origin, &members, &closure)? != certificate {
        return Err("noncanonical namespace order certificate".into());
    }
    Ok(NamespaceOrder {
        rank: closure.len() as i64 - members.len() as i64,
        evidence: format!("sha256:{}", hexlify(&Sha256::digest(certificate))),
        members,
        certificate: certificate.to_vec(),
    })
}

/// Collect each module's closed set, itself included, refusing malformed closures.
fn reachable_sets(closures: &HashMap<String, Vec<String>>) -> Result<HashMap<&str, HashSet<&str>>, String> {
    let mut reachable = HashMap::new();
    let mut modules: Vec<&String> = closures.keys().collect();
    modules.sort();
    for module in modules {
        let mut members = HashSet::new();
        for dependency in &closures[module] {
            if dependency.is_empty() || dependency == module || !members.insert(dependency.as_str()) {
                return Err(format!("malformed compiler namespace closure: {module}"));
            }
        }
        members.insert(module.as_str());
        reachable.insert(module.as_str(), members);
    }
    Ok(reachable)
}

/// The compiler supplies the full transitive import closure, excluding the module itself. Members of a cycle reach
/// one another and have identical closed sets once they include themselves. Every external predecessor contributes
/// to the shared rank. Incomplete closures cannot certify independent work.
pub fn certified_namespace_order(
    origin: &str,
    closures: &HashMap<String, Vec<String>>,
) -> Result<HashMap<String, NamespaceOrder>, String> {
    let reachable = reachable_sets(closures)?;
    let mut result = HashMap::new();
    let mut modules: Vec<&String> = closures.keys().collect();
    modules.sort();
    for module in modules {
        let closure = &reachable[module.as_str()];
        let mut group = vec![module.clone()];
        let mut complete = true;
        for dependency in &closures[module] {
            let Some(supplier) = reachable.get(dependency.as_str()) else {
                complete = false;
                break;
            };
            if supplier.iter().any(|predecessor| !closure.contains(predecessor)) {
                return Err(format!("inconsistent transitive compiler namespace closure: {module}"));
            }
            if supplier.contains(module.as_str()) {
                if supplier.len() != closure.len() {
                    return Err(format!("inconsistent joint compiler namespace closure: {module}"));
                }
                group.push(dependency.clone());
            }
        }
        if complete {
            group.sort();
            let mut dependencies: Vec<String> = closure.iter().map(|name| (*name).to_owned()).collect();
            dependencies.sort();
            let certificate = encoded_order(origin, &group, &dependencies)?;
            result.insert(
                module.clone(),
                NamespaceOrder {
                    rank: closure.len() as i64 - group.len() as i64,
                    evidence: format!("sha256:{}", hexlify(&Sha256::digest(&certificate))),
                    members: group,
                    certificate,
                },
            );
        }
    }
    Ok(result)
}

/// Keep the existing checked-namespace proof suffix recognizable to predecessor compilers.
pub fn ranked_output_suffix() -> String {
    ".ranked.cmpns5.graph-body-output".to_owned()
}

fn import_order_description() -> Description {
    model_description(
        IMPORT_ORDER_ROW,
        "incan.compiler.import-closures/1",
        &[ModelField::new(b"closed", "module_and_imports", "list<list<string>>")],
    )
}

/// Each row starts with its project source and lists the other project sources transitively imported.
pub fn encode_import_closures(rows: &[Vec<String>]) -> Result<Vec<u8>, String> {
    let value = Value::List(
        rows.iter()
            .map(|row| Value::List(row.iter().cloned().map(Value::Text).collect()))
            .collect(),
    );
    encode_graph_row(
        IMPORT_ORDER_ROW,
        &import_order_description(),
        vec![required_value(b"closed", value)],
    )
}

/// Require sorted, unique project-relative source names and a closed inventory from the collected unit.
pub fn read_import_closures(encoded: &[u8]) -> Result<HashMap<String, Vec<String>>, String> {
    let fields = decode_graph_row(encoded, IMPORT_ORDER_ROW, &import_order_description(), 1)?;
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    let mut previous = String::new();
    for value in items(&fields[0].value)? {
        let cells = items(value)?;
        let Some(first) = cells.first() else {
            return Err("empty compiler import closure".into());
        };
        let module = text(first)?;
        let segments: Vec<&str> = module.split('/').collect();
        if module <= previous || module.starts_with('/') || segments.contains(&"..") || segments.contains(&"") {
            return Err("noncanonical compiler import source".into());
        }
        previous = module.clone();
        let mut dependencies: Vec<String> = Vec::new();
        for item in &cells[1..] {
            let name = text(item)?;
            if name.is_empty() || name == module || dependencies.last().is_some_and(|last| name <= *last) {
                return Err("noncanonical compiler import dependency".into());
            }
            dependencies.push(name);
        }
        result.insert(module, dependencies);
    }
    if result
        .values()
        .flatten()
        .any(|dependency| !result.contains_key(dependency))
    {
        return Err("compiler import closure lacks its supplier".into());
    }
    Ok(result)
}
