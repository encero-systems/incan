//! Read and atomically retain native plan products under their producer's complete input witnesses.
//!
//! A product is one `<digest>.incnfunction` file in the request's product directory, named by the semantic digest of
//! its complete producer inputs. Records-mode reporting (product events and refusals for Bakery admission) refuses by
//! name until it is in Rust; an ordinary keyed request reports nothing.

use std::path::{Path, PathBuf};

use incan_mir_plan_native::{
    ExternalFunction, Function, NominalReference, PlanFunctionRecord, ProductNominal, SourceSpan, UnitNominals,
    decode_plan_function_record, decode_product_parts, encode_plan_function_record, encode_product,
    plan_function_digest, product_nominals, restore_named_product,
};
use incan_semantics_core::semantic_digest::semantic_digest;

use crate::pending::untranslated;
use crate::record_positions::{
    index_positions, indexed_product, place_positions, positioned_product, relocate_product,
};

/// A record whose key is its checked meaning; its spans name Body IR nodes by ordinal through a request-local position
/// map.
pub const RECORD_FUNCTION: &str = "incan.record-function/7";

/// The first input of an ordinary keyed product.
const SCALAR_FUNCTION: &str = "incan.scalar-function/9";

/// A restored product: its function, runtime demand, resolved declarations, helper frames and its still-encoded
/// generic instance requests.
pub type RetainedProduct = (
    Function,
    Vec<ExternalFunction>,
    Vec<NominalReference>,
    Vec<Function>,
    Vec<Vec<u8>>,
);

/// Bind a native product to its checked body, consumed signatures, physical frame and diagnostic provenance.
pub fn scalar_inputs(body: &str, name: &str, span: &SourceSpan, overflow_checks: bool, reads: &str) -> Vec<String> {
    vec![
        SCALAR_FUNCTION.to_owned(),
        body.to_owned(),
        name.to_owned(),
        span.file.to_string(),
        span.line.to_string(),
        span.column.to_string(),
        overflow_checks.to_string(),
        reads.to_owned(),
    ]
}

/// Key a record by its checked meaning and its Body IR node count; node positions are a separate map.
///
/// The count keeps a position map aligned with the nodes it was captured from, as when a docstring adds a node the
/// meaning digest leaves out.
pub fn record_scalar_inputs(
    body: &str,
    name: &str,
    span: &SourceSpan,
    overflow: bool,
    reads: &str,
    nodes: usize,
) -> Vec<String> {
    vec![
        RECORD_FUNCTION.to_owned(),
        body.to_owned(),
        name.to_owned(),
        span.file.to_string(),
        overflow.to_string(),
        reads.to_owned(),
        nodes.to_string(),
    ]
}

/// Read a complete native function whose full producer inputs and named declarations still match.
///
/// 1. Read the content-addressed private record.
/// 2. Validate its input witness and payload digest, and resolve each declaration it names in the current unit.
/// 3. Restore the complete native function at the current declaration and node positions, or return a cache miss.
///
/// The function returns with the generic instance requests its lowering made, still encoded.
pub fn retained_scalar(
    directory: &str,
    inputs: &[String],
    unit: &UnitNominals,
    span: Option<&SourceSpan>,
    positions: &[(i64, i64)],
) -> Option<RetainedProduct> {
    // 1. Missing, corrupt and concurrently replaced entries all remain ordinary misses.
    let path = scalar_path(directory, inputs).ok()?;
    let encoded = std::fs::read(path).ok()?;
    // 2. A changed or missing named declaration is a miss, never a stale position.
    let (payload, requests) = record_payload(&encoded, inputs, span, positions)?;
    let (function, runtime, resolved, frames) = restore_named_product(&payload, unit)?;
    Some((function, runtime, resolved, frames, requests))
}

/// Publish a loss-checked native function without changing compilation's verdict.
///
/// 1. Encode and authenticate every function field and named declaration against its producer inputs.
/// 2. Write a complete temporary record and atomically replace its destination.
#[allow(clippy::too_many_arguments)]
pub fn retain_scalar(
    directory: &str,
    inputs: &[String],
    function: &Function,
    runtime: &[ExternalFunction],
    nominals: &[ProductNominal],
    positions: &[(i64, i64)],
    frames: &[Function],
    requests: &[Vec<u8>],
) -> Result<(), String> {
    // 1. Records mode must preserve an unsupported native shape as an explicit producer refusal.
    let encoded = encoded_record(inputs, function, runtime, nominals, positions, frames, requests)?;
    // 2. The request-local transport is required input to Bakery publication, so failed writes refuse the producer.
    publish_record(directory, inputs, &encoded)
}

/// Publish one bounded product event for Bakery admission; an ordinary keyed request reports nothing.
pub fn report_scalar(directory: Option<&str>) -> Result<(), String> {
    match directory {
        None => Ok(()),
        Some(_) => Err(untranslated("scalar_store::report_scalar")),
    }
}

/// Retain an explicit record refusal without changing the ordinary lowering verdict; only a records request has one.
pub fn report_scalar_refusal(directory: Option<&str>) -> Result<(), String> {
    match directory {
        None => Ok(()),
        Some(_) => Err(untranslated("scalar_store::report_scalar_refusal")),
    }
}

/// Whether this request already reported a product under these inputs, as a retried instance pass does.
pub fn reported(directory: Option<&str>, inputs: &[String]) -> bool {
    let Some(root) = directory else {
        return false;
    };
    match fingerprint(inputs) {
        Ok(digest) => Path::new(root)
            .join(format!("{}.event", digest.replace("sha256:", "")))
            .is_file(),
        Err(_) => false,
    }
}

/// Authenticate the entire native payload after the codec proves an exact structural round trip.
///
/// A keyed record's spans are owner-relative, and each span that locates a Body IR node names that node's ordinal, so
/// the record holds no line a moved comment or docstring can change.
fn encoded_record(
    inputs: &[String],
    function: &Function,
    runtime: &[ExternalFunction],
    nominals: &[ProductNominal],
    positions: &[(i64, i64)],
    frames: &[Function],
    requests: &[Vec<u8>],
) -> Result<Vec<u8>, String> {
    let mut payload = encode_product(function, runtime, nominals, frames)?;
    if inputs.first().map(String::as_str) == Some(RECORD_FUNCTION) {
        payload = indexed_record_payload(payload, function, runtime, nominals, positions, frames)?;
    }
    let digest = plan_function_digest(&payload, requests)?;
    let record = PlanFunctionRecord {
        inputs: inputs.to_vec(),
        function: payload,
        requests: requests.to_vec(),
        digest,
    };
    encode_plan_function_record(&record)
}

/// Index a records product's spans by Body IR node ordinal, proving the indexing loses no native fact.
///
/// A noncanonical spelling must still pass the original codec's independent relocation and index proofs.
fn indexed_record_payload(
    payload: Vec<u8>,
    function: &Function,
    runtime: &[ExternalFunction],
    nominals: &[ProductNominal],
    positions: &[(i64, i64)],
    frames: &[Function],
) -> Result<Vec<u8>, String> {
    let file = function.span.file.to_string();
    let line = function.span.line;
    let indexed = indexed_product(&payload, positions, &file, line)?;
    let restored = positioned_product(&indexed, positions, &file, line, &mut Vec::new())?;
    if restored != payload {
        let relative = relocate_product(&payload, "<owner>", 1)?;
        let original = relocate_product(&relative, &file, line)?;
        if original != payload {
            let references = product_nominals(function, runtime, frames)?;
            let (restored_function, restored_runtime, restored_frames, _identities) =
                decode_product_parts(&original, &references)?;
            if encode_product(&restored_function, &restored_runtime, nominals, &restored_frames)? != payload {
                return Err("plan-function position round trip changed native facts".into());
            }
        }
        if place_positions(&indexed, positions)? != relative || indexed != index_positions(&relative, positions)? {
            return Err("plan-function position map changed native facts".into());
        }
    }
    Ok(indexed)
}

/// Verify one decoded record and hand its positioned payload and generic requests to the caller.
fn record_payload(
    encoded: &[u8],
    inputs: &[String],
    span: Option<&SourceSpan>,
    positions: &[(i64, i64)],
) -> Option<(Vec<u8>, Vec<Vec<u8>>)> {
    let record = decode_plan_function_record(encoded).ok()?;
    if record.inputs != inputs {
        return None;
    }
    if plan_function_digest(&record.function, &record.requests).ok()? != record.digest {
        return None;
    }
    let mut payload = record.function;
    if inputs.first().map(String::as_str) == Some(RECORD_FUNCTION)
        && let Some(current) = span
    {
        // Node ordinals take this body's positions; a body without those nodes is a miss.
        payload = positioned_product(
            &payload,
            positions,
            &current.file.to_string(),
            current.line,
            &mut Vec::new(),
        )
        .ok()?;
    }
    Some((payload, record.requests))
}

/// Name a native product by the existing semantic digest owner's unambiguous framed serialization.
pub fn scalar_path(directory: &str, inputs: &[String]) -> Result<PathBuf, String> {
    let digest = fingerprint(inputs)?;
    Ok(Path::new(directory).join(format!("{}.incnfunction", digest.replace("sha256:", ""))))
}

/// Use the existing digest owner; these fingerprints never define a second declaration identity.
fn fingerprint(inputs: &[String]) -> Result<String, String> {
    semantic_digest(&inputs.to_vec()).map_err(|error| error.to_string())
}

/// Publish one complete record with a process-local temporary name under the producer's locked build.
fn publish_record(directory: &str, inputs: &[String], encoded: &[u8]) -> Result<(), String> {
    let path = scalar_path(directory, inputs)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("cannot create scalar cache directory {error}"))?;
    }
    let temporary = PathBuf::from(format!("{}.{}.pending", path.display(), std::process::id()));
    std::fs::write(&temporary, encoded).map_err(|error| format!("cannot write scalar cache {error}"))?;
    std::fs::rename(&temporary, &path).map_err(|error| format!("cannot replace scalar cache {error}"))
}

#[cfg(test)]
mod tests {
    use incan_mir_plan_native::{Function, Local, PlanType, TerminatorKind, block, source_span, unit_nominals};

    use super::{retain_scalar, retained_scalar, scalar_inputs, scalar_path};

    /// A plan function returning unit, with one block that returns.
    fn returning_unit(name: &str) -> Function {
        let span = source_span("src/main.incn", 3, 1);
        let locals = vec![Local {
            ty: PlanType::Unit,
            span: span.clone(),
        }];
        let blocks = vec![block(Vec::new(), TerminatorKind::Return, &span)];
        Function::new(name.to_owned(), Vec::new(), PlanType::Unit, locals, blocks, span)
    }

    /// A fresh private product directory for one test.
    fn product_directory(test: &str) -> String {
        let directory = std::env::temp_dir().join(format!("incan-lowering-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory.to_string_lossy().into_owned()
    }

    #[test]
    fn scalar_inputs_spell_the_reference_key() {
        let inputs = scalar_inputs(
            "sha256:ab",
            "main",
            &source_span("src/main.incn", 3, 1),
            false,
            "witness",
        );
        assert_eq!(
            inputs,
            [
                "incan.scalar-function/9",
                "sha256:ab",
                "main",
                "src/main.incn",
                "3",
                "1",
                "false",
                "witness"
            ]
        );
    }

    #[test]
    fn retained_product_restores_what_was_retained() {
        let directory = product_directory("restore");
        let span = source_span("src/main.incn", 3, 1);
        let inputs = scalar_inputs("sha256:ab", "main", &span, false, "witness");
        let function = returning_unit("main");
        let requests = vec![b"request".to_vec()];
        retain_scalar(&directory, &inputs, &function, &[], &[], &[], &[], &requests).expect("retain");
        let path = scalar_path(&directory, &inputs).expect("path");
        assert!(path.is_file(), "the product is one file named by its inputs");
        assert_eq!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("incnfunction")
        );
        let unit = unit_nominals(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (restored, runtime, references, frames, encoded) =
            retained_scalar(&directory, &inputs, &unit, Some(&span), &[]).expect("a retained product is reused");
        assert_eq!(restored, function);
        assert!(runtime.is_empty() && references.is_empty() && frames.is_empty());
        assert_eq!(encoded, requests);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn changed_inputs_and_damaged_products_are_misses() {
        let directory = product_directory("miss");
        let span = source_span("src/main.incn", 3, 1);
        let inputs = scalar_inputs("sha256:ab", "main", &span, false, "witness");
        retain_scalar(&directory, &inputs, &returning_unit("main"), &[], &[], &[], &[], &[]).expect("retain");
        let unit = unit_nominals(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let changed = scalar_inputs("sha256:ab", "main", &span, true, "witness");
        assert!(retained_scalar(&directory, &changed, &unit, Some(&span), &[]).is_none());
        let path = scalar_path(&directory, &inputs).expect("path");
        std::fs::write(&path, b"not a product").expect("damage");
        assert!(retained_scalar(&directory, &inputs, &unit, Some(&span), &[]).is_none());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
