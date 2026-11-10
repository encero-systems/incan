//! Unicode identifier and normalization facts used by Crumb names.

/// Return whether one complete name follows UAX #31's identifier profile with the specified underscore extension.
pub fn is_xid_name(value: &str) -> bool {
    let mut characters = value.chars();
    let first = match characters.next() {
        Some(character) => character,
        None => return false,
    };
    if first != '_' && !unicode_ident::is_xid_start(first) {
        return false;
    }
    characters.all(|character| unicode_ident::is_xid_continue(character))
}

/// Return the first scalar index that differs from NFC, or `-1` when the input is already NFC.
pub fn first_nfc_difference(value: &str) -> i64 {
    let normalizer = icu_normalizer::ComposingNormalizer::new_nfc();
    if normalizer.is_normalized(value) {
        return -1;
    }
    let mut source = value.chars();
    let normalized_text = normalizer.normalize(value);
    let mut normalized = normalized_text.chars();
    let mut index = 0i64;
    loop {
        match (source.next(), normalized.next()) {
            (None, None) => return -1,
            (Some(left), Some(right)) if left == right => index += 1,
            _ => return index,
        }
    }
}
