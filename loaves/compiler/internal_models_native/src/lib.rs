//! Side-by-side Rust implementation of the compiler record contracts.

#[cfg(test)]
mod tests {
    use super::{CodecFault, compiled_body_frame_size};

    /// Oven reads exactly eight little-endian bytes before allocating a compiled body frame.
    #[test]
    fn compiled_body_frame_size_keeps_wire_limits() {
        assert_eq!(compiled_body_frame_size(&[1, 0, 0, 0, 0, 0, 0, 0]), Ok(1));
        assert_eq!(compiled_body_frame_size(&[0, 0, 0, 4, 0, 0, 0, 0]), Ok(67_108_864));
        for prefix in [&[][..], &[1; 7][..], &[1; 9][..]] {
            assert_eq!(compiled_body_frame_size(prefix), Err(CodecFault::Malformed));
        }
        for prefix in [[0; 8], [1, 0, 0, 4, 0, 0, 0, 0], [0xff; 8]] {
            assert_eq!(compiled_body_frame_size(&prefix), Err(CodecFault::Limit));
        }
    }
}
