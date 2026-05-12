pub fn u32_to_le_bytes(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn u64_to_le_bytes(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn u128_to_le_bytes(value: u128) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_outputs_are_little_endian_bytes() {
        assert_eq!(u32_to_le_bytes(0x01020304), vec![4, 3, 2, 1]);
        assert_eq!(u64_to_le_bytes(0x0102030405060708), vec![8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(
            u128_to_le_bytes(0x0102030405060708090a0b0c0d0e0f10),
            vec![16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1],
        );
    }
}
