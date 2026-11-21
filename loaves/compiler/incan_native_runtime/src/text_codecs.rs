//! Surface policy and error adaptation for primitive-owned UTF-8 conversion.

use incan_lang::lang::text_codecs::{is_utf8_encoding_label, normalize_encoding_label};
use incan_std_primitives::errors::raise_value_error;
use incan_std_primitives::validation::ValidationError;

/// Validate the normalized encoding before a computed receiver or error policy is evaluated.
pub fn text_codec_encoding_valid(encoding: &String) -> bool {
    is_utf8_encoding_label(&normalize_encoding_label(encoding))
}

/// Admit only the decoder's strict and replacement policies.
pub fn text_codec_errors_valid(errors: &String) -> bool {
    errors == "strict" || errors == "replace"
}

/// Refuse a non-UTF-8 encoder label with its normalized spelling, before the receiver is read.
pub fn validate_encoder_label(encoding: &String) {
    let label = normalize_encoding_label(encoding);
    if !is_utf8_encoding_label(&label) {
        raise_value_error(&format!(
            "str.encode() supports only utf-8 in this release, got encoding '{label}'"
        ));
    }
}

/// Validate decoder options in surface order and return `(accepted, replace_invalid)`.
pub fn decode_replacement_policy(encoding: &String, errors: &String) -> (bool, bool) {
    if !is_utf8_encoding_label(&normalize_encoding_label(encoding)) {
        return (false, false);
    }
    if errors == "replace" {
        return (true, true);
    }
    if !text_codec_errors_valid(errors) {
        return (false, false);
    }
    (true, false)
}

/// Adapt one rejected decoder option pair to its established ordered surface error.
pub fn invalid_decoder_options(encoding: &String, errors: &String) -> ValidationError {
    let label = normalize_encoding_label(encoding);
    if !is_utf8_encoding_label(&label) {
        return ValidationError::with_code(
            format!("bytes.decode() supports only utf-8 in this release, got encoding '{label}'"),
            "unknown-encoding",
        );
    }
    ValidationError::with_code(
        format!("bytes.decode() errors must be \"strict\" or \"replace\", got '{errors}'"),
        "unknown-errors-policy",
    )
}

/// Adapt primitive UTF-8 validation detail to the established coded decoder error.
pub fn invalid_utf8_error(detail: String) -> ValidationError {
    ValidationError::with_code(format!("'utf-8' codec can't decode bytes: {detail}"), "invalid-utf8")
}
