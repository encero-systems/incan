//! The grammar each checker record field's values follow, family by family; native objects never enter it.

use crate::call_artifact_codecs::{CallArtifactCodec, call_artifact_codec};
pub use crate::checker_record_models::{CanonicalField, CodecFault};
use crate::checker_record_rust_calls::rust_artifact_end;
pub(crate) use crate::checker_record_types::{checked_type_end as type_end, parameters_end};
use crate::input_fields::{input_field_end, method_call_end};
use CodecFault::Malformed;

/// The complete checked identifier classification shared with native record restoration.
const IDENTIFIER_KINDS: [&str; 8] = [
    "value",
    "static",
    "type-name",
    "variant",
    "module",
    "rust-import",
    "rust-value",
    "trait",
];

/// Accept only a bounded unsigned canonical decimal spelling, including the one-byte zero.
pub fn decimal(text: &str) -> Result<i64, CodecFault> {
    crate::checker_record_types::canonical_decimal(text)
}

/// Decode an unsigned count for indexing retained fields.
pub(crate) fn number(s: &str) -> Result<usize, CodecFault> {
    usize::try_from(decimal(s)?).map_err(|_| Malformed)
}

/// Read the validated scope count or the validated binding ordinal for the record-wide range check.
pub fn allocated_scope_index(field: &CanonicalField) -> Result<i64, CodecFault> {
    let index = if field.family == "scope" { 0 } else { 1 };
    decimal(at(&field.values, index)?)
}

/// A normalized node root carries the full SHA-256 spelling, not a shortened hint or nonempty placeholder.
pub fn full_digest(value: &str) -> bool {
    value.chars().count() == 71
        && value
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Get a borrowed token at a wire offset, retaining the schema's typed refusal on truncation.
pub(crate) fn at(values: &[String], index: usize) -> Result<&str, CodecFault> {
    values.get(index).map(String::as_str).ok_or(Malformed)
}

/// Read the optional diagnostic label and expectation after two counted note lists.
fn diagnostic_end(values: &[String]) -> Result<usize, CodecFault> {
    if values.len() < 7 || !matches!(values[0].as_str(), "error" | "syntax" | "type" | "warning" | "lint") {
        return Err(Malformed);
    }
    let mut end = 3;
    for _ in 0..2 {
        if end >= values.len() {
            return Err(Malformed);
        }
        end += 1 + number(&values[end])?;
        if end >= values.len() {
            return Err(Malformed);
        }
    }
    end += match values[end].as_str() {
        "primary-label" => 2,
        "no-primary-label" => 1,
        _ => return Err(Malformed),
    };
    end += match at(values, end)? {
        "expected-actual" => 3,
        "no-expected-actual" => 1,
        _ => return Err(Malformed),
    };
    let count = number(at(values, end)?)?;
    Ok(end + 1 + count * 2)
}

/// Explicit access modes, type component paths and stable source slots survive independently of source spelling.
fn ownership_end(v: &[String]) -> Result<usize, CodecFault> {
    if v.len() < 3 || v[0] != "1" {
        return Err(Malformed);
    }
    let count = number(&v[1])?;
    if count > v.len() {
        return Err(Malformed);
    }
    let mut end = 2;
    for _ in 0..count {
        if end + 3 >= v.len() || !matches!(v[end].as_str(), "Borrowed" | "BorrowedMut" | "Owned" | "Reference") {
            return Err(Malformed);
        }
        if v[end + 1] != "result" {
            decimal(&v[end + 1])?;
        }
        let size = number(&v[end + 2])?;
        end += 3;
        if size > v.len() - end {
            return Err(Malformed);
        }
        for component in &v[end..end + size] {
            decimal(component)?;
        }
        end = type_end(v, end + size, 0)?;
    }
    match at(v, end)? {
        "none" if count == 0 => return Err(Malformed),
        "none" => return Ok(end + 1),
        "parameter" => {
            decimal(at(v, end + 1)?)?;
            end += 2;
        }
        "receiver" => end += 1,
        _ => return Err(Malformed),
    }
    let size = number(at(v, end)?)?;
    end += 1;
    if size > v.len() - end || v[end..end + size].iter().any(String::is_empty) {
        return Err(Malformed);
    }
    Ok(end + size)
}

/// A constructor's argument-to-field binding: its field count, its counted argument slots and its counted,
/// ascending defaulted slots, which together occupy every field once. A requirement names one checked field and
/// its owner.
fn constructor_field_end(field: &CanonicalField) -> Result<usize, CodecFault> {
    let values = &field.values;
    if field.node.is_empty() {
        return Err(Malformed);
    }
    if field.family == "requirement" {
        if field.selector != "constructor-field" || values.len() != 2 || values.iter().any(String::is_empty) {
            return Err(Malformed);
        }
        return Ok(2);
    }
    if field.selector != "field-binding" || values.len() < 3 {
        return Err(Malformed);
    }
    let counts = values
        .iter()
        .map(|value| number(value))
        .collect::<Result<Vec<_>, _>>()?;
    let defaults_at = counts[1] + 2;
    if defaults_at >= counts.len() || counts[defaults_at] != counts.len() - defaults_at - 1 {
        return Err(Malformed);
    }
    let defaulted = &counts[defaults_at + 1..];
    if defaulted.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(Malformed);
    }
    let mut slots: Vec<usize> = counts[2..defaults_at].iter().chain(defaulted).copied().collect();
    slots.sort_unstable();
    if !slots.iter().copied().eq(0..counts[0]) {
        return Err(Malformed);
    }
    Ok(values.len())
}

/// Accept the canonical root marker or at most 32 dot-separated canonical component indices.
fn validate_type_identity_path(path: &str) -> Result<(), CodecFault> {
    if path == "root" {
        return Ok(());
    }
    let components: Vec<&str> = path.split('.').collect();
    if components.len() > 32 {
        return Err(Malformed);
    }
    for component in components {
        decimal(component)?;
    }
    Ok(())
}

/// The profile defines each primitive field grammar; unknown selectors or extra primitives are unsupported.
pub fn validate_field(field: &CanonicalField) -> Result<(), CodecFault> {
    let end = field_end(field)?;
    if end != field.values.len() {
        return Err(Malformed);
    }
    Ok(())
}

/// Each family's values follow that family's grammar; return where it stops.
fn field_end(field: &CanonicalField) -> Result<usize, CodecFault> {
    let values = field.values.as_slice();
    let selector = field.selector.as_str();
    let node = !field.node.is_empty();
    let unsupported = |name: &str| Err(CodecFault::Unsupported(name.to_owned()));
    match field.family.as_str() {
        "type" => {
            if !matches!(selector, "expr" | "body-expr" | "assignment" | "destination") {
                return unsupported(selector);
            }
            type_end(values, 0, 0)
        }
        "type-identity" => {
            if !node || values.len() != 1 || values[0].is_empty() {
                return Err(Malformed);
            }
            validate_type_identity_path(selector)?;
            Ok(1)
        }
        "identity-context" => {
            if !node
                || selector != "reference"
                || values.len() != 5
                || values[0].is_empty()
                || values[1].is_empty()
                || !matches!(values[3].as_str(), "field" | "method" | "property" | "variant")
                || values[4].is_empty()
            {
                return Err(Malformed);
            }
            decimal(&values[2])?;
            Ok(5)
        }
        "binding-type" => {
            if !node || values.is_empty() || values[0].is_empty() {
                return Err(Malformed);
            }
            match selector {
                "body" => type_end(values, 1, 0),
                "parameter-requirement" => match at(values, 1)? {
                    "none" => Ok(2),
                    "some" => type_end(values, 2, 0),
                    _ => Err(Malformed),
                },
                _ => unsupported(selector),
            }
        }
        "mutation" => mutation_end(field),
        "input-call" | "input-local" | "input-flow" | "input-copy" => input_field_end(field),
        "ownership" => {
            if selector != "contract" || !node {
                return Err(Malformed);
            }
            ownership_end(values)
        }
        "signature" => signature_end(values, selector),
        "constructor" | "requirement" => constructor_field_end(field),
        "builtin" => match (selector, values) {
            ("call", [function]) if !function.is_empty() => Ok(1),
            ("method", [owner, method]) if !owner.is_empty() && !method.is_empty() => Ok(2),
            _ => Err(Malformed),
        },
        "scope" => scope_end(field),
        "binding" if values.len() == 2 => {
            if selector != "declaration" || !node || values[0].is_empty() || decimal(&values[1])? > 2047 {
                return Err(Malformed);
            }
            Ok(2)
        }
        "binding" | "reference" | "write" | "default" => {
            if values.len() != 1 || values[0].is_empty() {
                return Err(Malformed);
            }
            Ok(1)
        }
        "parameter" => type_end(values, 0, 0),
        "identifier" => {
            if selector != "kind" || values.len() != 1 || !IDENTIFIER_KINDS.contains(&values[0].as_str()) {
                return unsupported("identifier");
            }
            Ok(1)
        }
        "artifact-protocol" => artifact_protocol_end(field),
        "artifact-call" => {
            if !node {
                return Err(Malformed);
            }
            match call_artifact_codec(selector) {
                Some(CallArtifactCodec::SpanType) => type_end(values, 0, 0),
                Some(CallArtifactCodec::Method) => method_call_end(values),
                Some(codec) if codec.admits_plain_values(values) => Ok(values.len()),
                Some(_) => Err(Malformed),
                None => unsupported(selector),
            }
        }
        "target" => {
            if selector != "source" || values.len() < 2 {
                return Err(Malformed);
            }
            Ok(values.len())
        }
        "status" => {
            if node || !matches!(selector, "refused" | "pending") || values.len() != 1 {
                return Err(Malformed);
            }
            if !matches!(values[0].as_str(), "true" | "false") {
                return Err(Malformed);
            }
            Ok(1)
        }
        "effect" => {
            if node || selector.is_empty() || values != ["checked"] {
                return Err(Malformed);
            }
            Ok(1)
        }
        "native-call" => match selector {
            "argument-type" | "result-type" if node => type_end(values, 0, 0),
            "empty" if !node && values == ["true"] => Ok(1),
            _ => unsupported("native-call"),
        },
        "native-value" => {
            if !node || selector != "type" || values.len() < 2 || values[0].is_empty() {
                return Err(Malformed);
            }
            type_end(values, 1, 0)
        }
        "final-type" => match selector {
            "empty" if values == ["true"] => Ok(1),
            "expression" => type_end(values, 0, 0),
            "binding" if node => type_end(values, 0, 0),
            _ => Err(Malformed),
        },
        "diagnostic" => diagnostic_end(values),
        "artifact-rust" => rust_artifact_end(field),
        family => unsupported(family),
    }
}

/// Signatures: a declaration's flag, parameters and result type; a call's parameters; or a counted list of types,
/// its monomorphs or type arguments.
fn signature_end(values: &[String], selector: &str) -> Result<usize, CodecFault> {
    match selector {
        "declaration" => {
            if values.is_empty() || !matches!(values[0].as_str(), "true" | "false") {
                return Err(Malformed);
            }
            let end = parameters_end(values, 1, 0)?;
            type_end(values, end, 0)
        }
        "call" | "body-call" => parameters_end(values, 0, 0),
        "monomorphs" | "type-arguments" => {
            if values.is_empty() {
                return Err(Malformed);
            }
            let count = number(&values[0])?;
            if count > values.len() {
                return Err(Malformed);
            }
            let mut end = 1;
            for _ in 0..count {
                end = type_end(values, end, 0)?;
            }
            Ok(end)
        }
        _ => Err(CodecFault::Unsupported(selector.to_owned())),
    }
}

/// What a node changes: `reassigned` is just `true`; `changed` counts the names it changes, strictly sorted.
fn mutation_end(field: &CanonicalField) -> Result<usize, CodecFault> {
    let values = field.values.as_slice();
    if field.node.is_empty() {
        return Err(Malformed);
    }
    match field.selector.as_str() {
        "reassigned" => {
            if values != ["true"] {
                return Err(Malformed);
            }
            Ok(1)
        }
        "changed" => {
            if values.len() < 2 || values[0].is_empty() {
                return Err(Malformed);
            }
            if number(&values[1])? != values.len() - 2 {
                return Err(Malformed);
            }
            let mut previous = "";
            for name in &values[2..] {
                if name.as_str() <= previous {
                    return Err(Malformed);
                }
                previous = name;
            }
            Ok(values.len())
        }
        _ => Err(Malformed),
    }
}

/// One allocation inventory: each scope's parent ordinal, never later than itself, and its kind.
fn scope_end(field: &CanonicalField) -> Result<usize, CodecFault> {
    let values = field.values.as_slice();
    if !field.node.is_empty() || field.selector != "allocation" || values.is_empty() {
        return Err(Malformed);
    }
    let count = number(&values[0])?;
    if count > 2047 || values.len() != 1 + count * 2 {
        return Err(Malformed);
    }
    for index in 0..count {
        if number(&values[1 + index * 2])? > index || !matches!(values[2 + index * 2].as_str(), "block" | "function") {
            return Err(Malformed);
        }
    }
    Ok(values.len())
}

/// Generated protocol artifact rows retain one checked receiver identity at its source node.
fn artifact_protocol_end(field: &CanonicalField) -> Result<usize, CodecFault> {
    let values = field.values.as_slice();
    let selector = field.selector.as_str();
    if field.node.is_empty() || values.len() != 1 {
        return Err(Malformed);
    }
    if selector == "body_ir_truthiness_receivers" && values[0].is_empty() {
        return Err(Malformed);
    }
    if matches!(
        selector,
        "item_taking_iterations" | "in_place_match_scrutinees" | "loan_free_scrutinees"
    ) && values != ["true"]
    {
        return Err(Malformed);
    }
    if selector == "comprehension_source_consumption" && !matches!(values[0].as_str(), "true" | "false") {
        return Err(Malformed);
    }
    if !matches!(
        selector,
        "body_ir_truthiness_receivers"
            | "comprehension_source_consumption"
            | "item_taking_iterations"
            | "in_place_match_scrutinees"
            | "loan_free_scrutinees"
    ) {
        return Err(CodecFault::Unsupported(selector.to_owned()));
    }
    Ok(1)
}
