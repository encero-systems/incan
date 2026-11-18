//! Fixed-width streaming delimiter for a compiled body frame.

use crate::CodecFault;

/// Read the little-endian payload length. Oven admits the size before allocating the frame.
pub fn compiled_body_frame_size(prefix: &[u8]) -> Result<i64, CodecFault> {
    let octets = prefix.try_into().map_err(|_| CodecFault::Malformed)?;
    match i64::from_le_bytes(octets) {
        size @ 1..=67_108_864 => Ok(size),
        _ => Err(CodecFault::Limit),
    }
}
