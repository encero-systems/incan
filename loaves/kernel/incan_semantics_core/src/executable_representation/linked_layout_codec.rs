//! A linked-layout witness travels in its declaration's authenticated range.

use std::sync::Arc;

use super::{ExecutableDeclaration, ExecutableRepresentationError, malformed};

/// Zero cannot start an existing declaration trailer, which encodes a nonempty trait-field binding list.
const HEADER: &[u8] = &[0, 1];
const DIGEST_BYTES: usize = 64;

/// Bytes contributed to a declaration range by its layout witness.
pub const LINKED_LAYOUT_WITNESS_BYTES: usize = HEADER.len() + 2 * DIGEST_BYTES;

/// Append the producer's digest pair before any existing trait-field trailer.
pub(super) fn append(
    bytes: &mut Vec<u8>,
    specialization: &str,
    version: &str,
) -> Result<(), ExecutableRepresentationError> {
    checked_digest(specialization.as_bytes())?;
    checked_digest(version.as_bytes())?;
    bytes.extend(HEADER);
    bytes.extend(specialization.as_bytes());
    bytes.extend(version.as_bytes());
    Ok(())
}

/// Attach a witness to its owning declaration. Legacy ranges return their original remainder unchanged.
pub(super) fn restore<'a>(
    declaration: &mut ExecutableDeclaration,
    bytes: &'a [u8],
) -> Result<&'a [u8], ExecutableRepresentationError> {
    if bytes.first() != HEADER.first() {
        return Ok(bytes);
    }
    let rest = bytes
        .strip_prefix(HEADER)
        .ok_or_else(|| malformed("unsupported linked-layout witness"))?;
    let (specialization, rest) = rest
        .split_at_checked(DIGEST_BYTES)
        .ok_or_else(|| malformed("truncated linked-layout specialization"))?;
    let (version, rest) = rest
        .split_at_checked(DIGEST_BYTES)
        .ok_or_else(|| malformed("truncated linked-layout version"))?;
    let witness = Arc::new((
        checked_digest(specialization)?.to_owned(),
        checked_digest(version)?.to_owned(),
    ));
    if let Some(model) = declaration.nominal_mut() {
        model.linked_layout_witness = Some(witness);
    } else if let Some(enumeration) = declaration.enum_layout_mut() {
        enumeration.linked_layout_witness = Some(witness);
    } else {
        return Err(malformed("linked-layout witness has no layout owner"));
    }
    Ok(rest)
}

/// Read exactly one canonical lowercase hexadecimal SHA-256 digest.
fn checked_digest(bytes: &[u8]) -> Result<&str, ExecutableRepresentationError> {
    if bytes.len() != DIGEST_BYTES
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(malformed("invalid linked-layout digest"));
    }
    std::str::from_utf8(bytes).map_err(|error| malformed(error.to_string()))
}
