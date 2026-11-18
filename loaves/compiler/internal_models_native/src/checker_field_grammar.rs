//! Primitive grammars retained by canonical checker rows.

use crate::call_artifact_codecs::{CallArtifactCodec, call_artifact_codec};
pub use crate::codec_fault::CodecFault;
use crate::input_fields::input_field_end;
use CodecFault::Malformed;

/// A schema-owned field: node is a structural role or stable binding reference; values follow its typed grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalField {
    pub family: String,
    pub node: String,
    pub selector: String,
    pub values: Vec<String>,
}

/// Parse the canonical unsigned decimal grammar of up to five digits.
pub fn decimal(text: &str) -> Result<i64, CodecFault> {
    let bytes = text.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 5
        || (bytes.len() > 1 && bytes[0] == b'0')
        || !bytes.iter().all(u8::is_ascii_digit)
    {
        return Err(Malformed);
    }
    Ok(bytes
        .iter()
        .fold(0_i64, |value, digit| value * 10 + i64::from(digit - b'0')))
}

/// Decode an unsigned count for indexing retained fields.
pub(crate) fn number(s: &str) -> Result<usize, CodecFault> {
    usize::try_from(decimal(s)?).map_err(|_| Malformed)
}

/// Require the complete 64-nybble lowercase digest behind its named algorithm.
pub fn full_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Get a borrowed token at a wire offset, retaining the schema's typed refusal on truncation.
pub(crate) fn at(values: &[String], index: usize) -> Result<&str, CodecFault> {
    values.get(index).map(String::as_str).ok_or(Malformed)
}

/// Return the first offset after one prefix-encoded type.
pub(crate) fn type_end(values: &[String], offset: usize, depth: u32) -> Result<usize, CodecFault> {
    if depth > 32 {
        return Err(Malformed);
    }
    let tag = at(values, offset)?;
    let next = offset + 1;
    match tag {
        "never" | "int" | "float" | "bool" | "str" | "bytes" | "frozen-str" | "frozen-bytes" | "unit" | "self-type"
        | "call-site-infer" | "unknown" => Ok(next),
        "named" | "type-var" | "numeric" | "rust-path" => {
            if at(values, next)?.is_empty() {
                return Err(Malformed);
            }
            Ok(next + 1)
        }
        "result" | "frozen-dict" => type_end(values, type_end(values, next, depth + 1)?, depth + 1),
        "option" | "list" | "frozen-list" | "frozen-set" | "type-token" | "ref" | "ref-mut" => {
            type_end(values, next, depth + 1)
        }
        "ownership" => {
            if !mode(at(values, next)?) {
                return Err(Malformed);
            }
            type_end(values, next + 1, depth + 1)
        }
        "callable" => type_end(values, parameters_end(values, next, depth + 1)?, depth + 1),
        "generic" | "tuple" => {
            let first = at(values, next)?;
            // Round 112 calls the standard list accessor when a named generic is missing its arity.
            let (arity, mut end) = if tag == "generic" {
                if first.is_empty() {
                    return Err(Malformed);
                }
                (
                    internal_models_bindings::list_get(values, (next + 1) as i64).as_str(),
                    next + 2,
                )
            } else {
                (first, next + 1)
            };
            let count = number(arity)?;
            if count > values.len() - end {
                return Err(Malformed);
            }
            for _ in 0..count {
                end = type_end(values, end, depth + 1)?;
            }
            Ok(end)
        }
        _ => Err(CodecFault::Unsupported(format!("type:{tag}"))),
    }
}

/// Validate explicit ownership-mode spellings shared by the declaration and flow grammars.
pub(crate) fn mode(value: &str) -> bool {
    matches!(value, "Borrowed" | "BorrowedMut" | "Owned" | "Reference")
}

/// Validate the binding marker, source shape, three flags and type in every callable parameter.
pub(crate) fn parameters_end(values: &[String], offset: usize, depth: u32) -> Result<usize, CodecFault> {
    let count = number(at(values, offset)?)?;
    if count > values.len() {
        return Err(Malformed);
    }
    let mut end = offset + 1;
    for _ in 0..count {
        let head = values.get(end..end + 6).ok_or(Malformed)?;
        if !matches!(head[0].as_str(), "named" | "unnamed")
            || (head[0] == "unnamed" && !head[1].is_empty())
            || !matches!(head[2].as_str(), "normal" | "rest" | "keywords")
            || !head[3..].iter().all(|v| matches!(v.as_str(), "true" | "false"))
        {
            return Err(Malformed);
        }
        end = type_end(values, end + 6, depth + 1)?;
    }
    Ok(end)
}

/// Read the optional diagnostic label and expectation after two counted note lists.
fn diagnostic_end(values: &[String]) -> Result<usize, CodecFault> {
    if values.len() < 7 || !matches!(at(values, 0)?, "error" | "syntax" | "type" | "warning" | "lint") {
        return Err(Malformed);
    }
    let mut end = 3;
    for _ in 0..2 {
        end += 1 + number(at(values, end)?)?;
    }
    end += match at(values, end)? {
        "primary-label" => 2,
        "no-primary-label" => 1,
        _ => return Err(Malformed),
    };
    end += match at(values, end)? {
        "expected-actual" => 3,
        "no-expected-actual" => 1,
        _ => return Err(Malformed),
    };
    Ok(end + 1 + number(at(values, end)?)? * 2)
}

/// Read component paths, source slots and the return reference of one ownership declaration.
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
        if end + 3 >= v.len() || !mode(&v[end]) {
            return Err(Malformed);
        }
        if v[end + 1] != "result" {
            number(&v[end + 1])?;
        }
        let n = number(&v[end + 2])?;
        end += 3;
        for component in v.get(end..end + n).ok_or(Malformed)? {
            number(component)?;
        }
        end = type_end(v, end + n, 0)?;
    }
    match at(v, end)? {
        "none" if count > 0 => return Ok(end + 1),
        "parameter" => {
            number(at(v, end + 1)?)?;
            end += 2;
        }
        "receiver" => end += 1,
        _ => return Err(Malformed),
    }
    let n = number(at(v, end)?)?;
    end += 1;
    if v.get(end..end + n).ok_or(Malformed)?.iter().any(String::is_empty) {
        return Err(Malformed);
    }
    Ok(end + n)
}

/// Verify that argument and default slots are disjoint and cover the field inventory.
fn constructor_end(field: &CanonicalField) -> Result<usize, CodecFault> {
    let v = &field.values;
    if field.node.is_empty() {
        return Err(Malformed);
    }
    if field.family == "requirement" {
        return if field.selector == "constructor-field" && v.len() == 2 && v.iter().all(|s| !s.is_empty()) {
            Ok(2)
        } else {
            Err(Malformed)
        };
    }
    if field.selector != "field-binding" || v.len() < 3 {
        return Err(Malformed);
    }
    let fields = number(&v[0])?;
    let supplied = number(&v[1])?;
    let divider = supplied + 2;
    let defaults = number(at(v, divider)?)?;
    if defaults != v.len() - divider - 1 || supplied + defaults != fields {
        return Err(Malformed);
    }
    let mut used = vec![false; fields];
    let mut previous = None;
    for (i, text) in v.iter().enumerate().skip(2) {
        if i == divider {
            continue;
        }
        let slot = number(text)?;
        let taken = used.get_mut(slot).ok_or(Malformed)?;
        if *taken || (i > divider && previous.is_some_and(|n| n >= slot)) {
            return Err(Malformed);
        }
        *taken = true;
        if i > divider {
            previous = Some(slot);
        }
    }
    Ok(v.len())
}

/// Validate shape, tag and complete consumption. Unknown producer capabilities retain their own refusal.
pub fn validate_field(field: &CanonicalField) -> Result<(), CodecFault> {
    let family = field.family.as_str();
    let sel = field.selector.as_str();
    let node = !field.node.is_empty();
    let v = &field.values[..];
    let unsupported = |name: &str| CodecFault::Unsupported(name.to_owned());
    let end = match family {
        "type" => {
            if !matches!(sel, "expr" | "body-expr" | "assignment" | "destination") {
                return Err(unsupported(sel));
            }
            type_end(v, 0, 0)?
        }
        "type-identity" | "identity-context" => identity_field_end(field)?,
        "binding-type" => {
            if !node || at(v, 0)?.is_empty() {
                return Err(Malformed);
            }
            match sel {
                "body" => type_end(v, 1, 0)?,
                "parameter-requirement" => match at(v, 1)? {
                    "none" => 2,
                    "some" => type_end(v, 2, 0)?,
                    _ => return Err(Malformed),
                },
                _ => return Err(unsupported(sel)),
            }
        }
        "mutation" => mutation_end(field)?,
        "input-call" | "input-local" | "input-flow" | "input-copy" => input_field_end(field)?,
        "ownership" if sel == "contract" && node => ownership_end(v)?,
        "signature" => match sel {
            "declaration" if matches!(at(v, 0)?, "true" | "false") => type_end(v, parameters_end(v, 1, 0)?, 0)?,
            "declaration" => return Err(Malformed),
            "call" | "body-call" => parameters_end(v, 0, 0)?,
            "monomorphs" | "type-arguments" => {
                let n = number(at(v, 0)?)?;
                if n > v.len() {
                    return Err(Malformed);
                }
                let mut end = 1;
                for _ in 0..n {
                    end = type_end(v, end, 0)?;
                }
                end
            }
            _ => return Err(unsupported(sel)),
        },
        "constructor" | "requirement" => constructor_end(field)?,
        "builtin"
            if ((sel == "call" && v.len() == 1) || (sel == "method" && v.len() == 2))
                && v.iter().all(|s| !s.is_empty()) =>
        {
            v.len()
        }
        "binding" | "reference" | "write" | "default" if matches!(v,[s] if !s.is_empty()) => 1,
        "parameter" => type_end(v, 0, 0)?,
        "identifier" => {
            if sel != "kind" || !(v == ["value"] || v == ["type-name"]) {
                return Err(unsupported("identifier"));
            }
            1
        }
        "artifact-protocol" => {
            if !node || v.len() != 1 {
                return Err(Malformed);
            }
            match sel {
                "body_ir_truthiness_receivers" if !v[0].is_empty() => 1,
                "item_taking_iterations" | "in_place_match_scrutinees" | "loan_free_scrutinees" if v == ["true"] => 1,
                "comprehension_source_consumption" if v == ["true"] || v == ["false"] => 1,
                "body_ir_truthiness_receivers"
                | "item_taking_iterations"
                | "in_place_match_scrutinees"
                | "loan_free_scrutinees"
                | "comprehension_source_consumption" => return Err(Malformed),
                _ => return Err(unsupported(sel)),
            }
        }
        "artifact-call" => {
            if !node {
                return Err(Malformed);
            }
            match call_artifact_codec(sel).ok_or_else(|| unsupported(sel))? {
                CallArtifactCodec::SpanType => type_end(v, 0, 0)?,
                codec if codec.admits_plain_values(v) => v.len(),
                _ => return Err(Malformed),
            }
        }
        "target" if sel == "source" && v.len() >= 2 => v.len(),
        "status" if !node && matches!(sel, "refused" | "pending") && (v == ["true"] || v == ["false"]) => 1,
        "effect" if !node && !sel.is_empty() && v == ["checked"] => 1,
        "native-call" => match sel {
            "argument-type" | "result-type" if node => type_end(v, 0, 0)?,
            "empty" if !node && v == ["true"] => 1,
            _ => return Err(unsupported("native-call")),
        },
        "final-type" if sel == "empty" && v == ["true"] => 1,
        "final-type" if sel == "expression" || (sel == "binding" && node) => type_end(v, 0, 0)?,
        "diagnostic" => diagnostic_end(v)?,
        "ownership" | "builtin" | "binding" | "reference" | "write" | "default" | "target" | "status" | "effect"
        | "final-type" => return Err(Malformed),
        _ => return Err(unsupported(family)),
    };
    if end == v.len() { Ok(()) } else { Err(Malformed) }
}

/// Validate the component path and the stable identity of a nested member.
fn identity_field_end(field: &CanonicalField) -> Result<usize, CodecFault> {
    let node = !field.node.is_empty();
    let sel = field.selector.as_str();
    let v = field.values.as_slice();
    Ok(match field.family.as_str() {
        "type-identity" => {
            if !node || !matches!(v,[s] if !s.is_empty()) {
                return Err(Malformed);
            }
            if sel != "root" {
                for (i, part) in sel.split('.').enumerate() {
                    if i >= 32 {
                        return Err(Malformed);
                    }
                    number(part)?;
                }
            }
            1
        }
        "identity-context" => {
            if !node
                || sel != "reference"
                || v.len() != 5
                || v[0].is_empty()
                || v[1].is_empty()
                || !matches!(v[3].as_str(), "field" | "method" | "property" | "variant")
                || v[4].is_empty()
            {
                return Err(Malformed);
            }
            number(&v[2])?;
            5
        }
        _ => return Err(Malformed),
    })
}
/// Validate one mutation marker or its strictly ordered changed names.
fn mutation_end(field: &CanonicalField) -> Result<usize, CodecFault> {
    let node = !field.node.is_empty();
    let sel = field.selector.as_str();
    let v = field.values.as_slice();
    let unsupported = |name: &str| CodecFault::Unsupported(name.to_owned());

    if !node {
        return Err(Malformed);
    }
    Ok(match sel {
        "reassigned" if v == ["true"] => 1,
        "changed"
            if v.len() >= 2
                && !v[0].is_empty()
                && number(&v[1])? == v.len() - 2
                && v.get(2).is_none_or(|s| !s.is_empty())
                && v[2..].windows(2).all(|p| p[0] < p[1]) =>
        {
            v.len()
        }
        "changed" | "reassigned" => return Err(Malformed),
        _ => return Err(unsupported(sel)),
    })
}
