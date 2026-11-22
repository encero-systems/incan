//! Validate canonical checker-record source types independently of native object layouts.

use crate::checker_record_models::CodecFault;
use CodecFault::Malformed;

/// Accept only a bounded unsigned canonical decimal spelling, including the one-byte zero.
pub fn canonical_decimal(text: &str) -> Result<i64, CodecFault> {
    let bytes = text.as_bytes();
    // The reference measures the spelling in Unicode scalar values; a non-ASCII scalar is refused below anyway.
    let scalars = text.chars().count();
    if text.is_empty() || scalars > 5 || (scalars > 1 && text.starts_with('0')) {
        return Err(Malformed);
    }
    let mut value = 0_i64;
    for &digit in bytes {
        if !digit.is_ascii_digit() {
            return Err(Malformed);
        }
        value = value * 10 + i64::from(digit - b'0');
    }
    Ok(value)
}

/// The token at one position, or the schema's refusal for a truncated field.
fn at(values: &[String], index: usize) -> Result<&str, CodecFault> {
    values.get(index).map(String::as_str).ok_or(Malformed)
}

/// Return the first field after one prefix-encoded source type.
pub fn checked_type_end(values: &[String], offset: usize, depth: i64) -> Result<usize, CodecFault> {
    if depth > 32 || offset >= values.len() {
        return Err(Malformed);
    }
    let tag = values[offset].as_str();
    match tag {
        "never" | "int" | "float" | "bool" | "str" | "bytes" | "frozen-str" | "frozen-bytes" | "unit" | "self-type"
        | "call-site-infer" | "unknown" => Ok(offset + 1),
        "named" | "type-var" | "numeric" | "rust-path" => {
            if at(values, offset + 1)?.is_empty() {
                return Err(Malformed);
            }
            Ok(offset + 2)
        }
        "result" | "frozen-dict" => {
            let after = checked_type_end(values, offset + 1, depth + 1)?;
            checked_type_end(values, after, depth + 1)
        }
        "option" | "list" | "frozen-list" | "frozen-set" | "type-token" | "ref" | "ref-mut" => {
            checked_type_end(values, offset + 1, depth + 1)
        }
        "ownership" => {
            if !matches!(
                at(values, offset + 1)?,
                "Borrowed" | "BorrowedMut" | "Owned" | "Reference"
            ) {
                return Err(Malformed);
            }
            checked_type_end(values, offset + 2, depth + 1)
        }
        "callable" => {
            let after = parameters_end(values, offset + 1, depth + 1)?;
            checked_type_end(values, after, depth + 1)
        }
        "generic" | "tuple" => {
            if offset + 1 >= values.len() {
                return Err(Malformed);
            }
            let mut end = offset + 1;
            if tag == "generic" {
                if end + 1 >= values.len() || values[end].is_empty() {
                    return Err(Malformed);
                }
                end += 1;
            }
            let count = canonical_decimal(&values[end])?;
            end += 1;
            if count > (values.len() - end) as i64 {
                return Err(Malformed);
            }
            for _ in 0..count {
                end = checked_type_end(values, end, depth + 1)?;
            }
            Ok(end)
        }
        _ => Err(CodecFault::Unsupported(format!("type:{tag}"))),
    }
}

/// Return the first field after canonical callable parameters and their checked source types.
pub fn parameters_end(values: &[String], offset: usize, depth: i64) -> Result<usize, CodecFault> {
    if offset >= values.len() {
        return Err(Malformed);
    }
    let count = canonical_decimal(&values[offset])?;
    if count > values.len() as i64 {
        return Err(Malformed);
    }
    let mut end = offset + 1;
    for _ in 0..count {
        if end + 6 >= values.len() {
            return Err(Malformed);
        }
        if !matches!(values[end].as_str(), "named" | "unnamed") {
            return Err(Malformed);
        }
        if values[end] == "unnamed" && !values[end + 1].is_empty() {
            return Err(Malformed);
        }
        if !matches!(values[end + 2].as_str(), "normal" | "rest" | "keywords") {
            return Err(Malformed);
        }
        if !values[end + 3..end + 6]
            .iter()
            .all(|bit| bit == "true" || bit == "false")
        {
            return Err(Malformed);
        }
        end = checked_type_end(values, end + 6, depth + 1)?;
    }
    Ok(end)
}
