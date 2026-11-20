//! The native runtime every compiled Incan program links.
//!
//! Lowering names each operation by its public path, `incan_native_runtime::<name>`, and the rustc driver verifies the
//! plan's declared signature against this crate's metadata before building a call. A non-generic operation therefore
//! takes exactly the arguments its call sites declare; a generic one keeps the type-parameter order of its checked
//! source declaration, so a plan's explicit type arguments close it unchanged.

#![forbid(unsafe_code)]

#[cfg(test)]
mod behavior_tests;
mod async_;
mod builtins;
mod carriers;
mod checked_json;
mod collections;
mod decimals;
mod diverging_errors;
mod frozen_collections;
mod frozen_text;
mod generators;
mod generic_decorators;
mod integer_helpers;
mod lists;
mod model_values;
mod newtype_hooks;
mod numerics;
mod panic_hook;
mod pattern_assertions;
mod race;
mod raises_assertions;
mod static_collections;
mod statics;
mod strings;
mod text_codecs;
mod tuple_iterators;
mod validation_errors;

use std::io::Write;

pub use async_::{run_async_entry, run_future_entry, run_result_future_entry};
pub use builtins::{
    absolute_int, boolean_and, boolean_or, maximum_bool, maximum_float, maximum_int, maximum_text, minimum_bool,
    minimum_float, minimum_int, minimum_text, power_f32, power_f64, power_i64, read_file_text, sum_bool, sum_int,
    truth_bool, truth_float, truth_int, truth_list, truth_text, write_file_text,
};
pub use carriers::{clone_carrier_payload, format_carrier};
pub use checked_json::checked_json_result;
pub use collections::{
    borrow_mut_dict, clone_dict, clone_set, contains_dict, contains_set, default_dict, empty_dict, empty_set, equal_dict,
    equal_set, extend_dict, extend_list_set, extend_set, get_dict, insert_dict, insert_set, keys_dict, length_dict,
    length_set, nth_dict, nth_set, optional_dict, unequal_dict, unequal_set, values_dict,
};
pub use decimals::{
    decimal_from_parts, equal_decimal, format_decimal, greater_decimal, greater_equal_decimal, less_decimal,
    less_equal_decimal, unequal_decimal,
};
pub use diverging_errors::raise_value_error;
pub use frozen_collections::{
    freeze_list, frozen_dict_contains, frozen_dict_get, frozen_dict_keys, frozen_dict_length, frozen_dict_length_ref,
    frozen_dict_text_contains, frozen_dict_text_get, frozen_dict_text_keys, frozen_list_byte_items, frozen_list_items,
    frozen_list_length, frozen_list_length_ref, frozen_list_text_items, frozen_set_items, frozen_set_length,
    frozen_set_length_ref, frozen_set_text_items, own_static_bytes,
};
pub use frozen_text::{copy_frozen_bytes, format_frozen, length_frozen_bytes};
pub use generators::{
    all_int_generator, any_int_generator, any_text_callable_generator, batch_generator, bool_callable_step_generator,
    bool_step_generator, chain_generators, count_generator, empty_generator, enumerate_generator,
    filter_bool_generator, filter_float_generator, filter_int_generator, find_int_generator, flat_map_int_generator,
    flat_map_int_generators, flat_map_int_set_generator, flat_map_text_callable_generator, float_callable_step_generator,
    float_step_generator, fold_int_generator, for_each_int_generator, int_callable_step_generator, int_step_generator,
    item_callable_step_generator, list_generator, map_bool_generator, map_float_generator, map_int_generator,
    map_text_callable_generator, once_generator, poll_generator, range_generator, reduce_int_generator, set_generator,
    skip_generator, skip_while_int_generator, str_callable_step_generator, sum_int_generator, take_generator,
    take_while_int_generator, zip_generators,
};
pub use generic_decorators::{callable_name, initialize_binding, register_callable_name};
pub use incan_std_primitives::frozen::{FrozenBytes as FrozenBytesCarrier, FrozenStr as FrozenCarrier};
pub use incan_std_primitives::num::Decimal128 as DecimalCarrier;
pub use incan_std_primitives::validation::ValidationError as ValidationErrorCarrier;
pub use integer_helpers::{
    checked_add_u8, checked_mul_u8, checked_pow_u8, checked_sub_u8, saturating_add_u8, saturating_mul_u8,
    saturating_pow_u8, saturating_sub_u8, wrapping_add_u8, wrapping_mul_u8, wrapping_pow_u8, wrapping_sub_u8,
};
pub use lists::{
    append_list, borrow_list_mut, clone_list, concat_list, contains_list, count_list, empty_list, equal_list,
    extend_list, get_list, index_list, length_list, pop_list, remove_list, repeat_list, set_list, slice_list,
    sorted_float, sorted_list, swap_list, unequal_list,
};
pub use model_values::{
    debug_model, debug_value, deserialize_model, display_type_parameter, display_value, equal_model, greater_equal_model,
    greater_model, less_equal_model, less_model, serialize_bool, serialize_float, serialize_int, serialize_model,
    serialize_string, unequal_model,
};
pub use newtype_hooks::{
    add_newtype_sum, checked_newtype_field_result, checked_newtype_result, checked_sum_result, parse_newtype_int,
    validate_newtype_constraint,
};
pub use numerics::*;
pub use pattern_assertions::{
    missing_err, missing_err_with, missing_ok, missing_ok_with, missing_some, missing_some_with, unexpected_some,
    unexpected_some_with, unwrap_err, unwrap_none,
};
pub use race::race_pair;
pub use raises_assertions::assert_raises_deferred;
pub use static_collections::{
    read_static_int_set, read_static_text_int_dict, write_static_int_set, write_static_text_int_dict,
};
pub use statics::{
    begin_module_initialization, end_module_initialization, read_static_bool, read_static_float, read_static_int,
    read_static_text, write_static_bool, write_static_float, write_static_int, write_static_text,
};
pub use strings::{
    concat_text, contains_text, ends_text, equal_text, greater_equal_text, greater_text, join_text, length_text,
    less_equal_text, less_text, lower_text, not_contains_text, replace_text, slice_text, slice_text_bounds,
    split_default_text, split_text, split_whitespace_text, starts_text, strip_text, unequal_text, upper_text,
};
pub use text_codecs::{
    decode_replacement_policy, invalid_decoder_options, invalid_utf8_error, text_codec_encoding_valid,
    text_codec_errors_valid, validate_encoder_label,
};
pub use tuple_iterators::{
    ZipLists, clone_zip, enumerate_bytes, enumerate_list, enumerate_text, poll_zip, text_items, zip_lists,
    zip_lists_generator,
};
pub use validation_errors::{format_validation_error, make_coded_validation_error, make_validation_error};

/// Format an integer with the same conversion used by language string interpolation.
pub fn format_int(value: i64) -> String {
    value.to_string()
}

/// Preserve the language's float spelling, including visibly integral floats.
pub fn format_float(value: f64) -> String {
    incan_std_primitives::strings::float_to_string(value)
}

/// Format a boolean with the language's scalar conversion.
pub fn format_bool(value: bool) -> String {
    if value { "true".to_owned() } else { "false".to_owned() }
}

/// Print one line: retain the UTF-8 bytes, append a newline, and write the whole line or raise its I/O failure.
pub fn println_text(value: String) {
    let mut bytes = value.into_bytes();
    bytes.push(b'\n');
    if let Err(error) = std::io::stdout().write_all(&bytes) {
        std::panic::panic_any(format!("failed printing to stdout: {error}"));
    }
}

/// Create the owned display text for an already formatted borrowed string.
pub fn clone_text(value: &String) -> String {
    value.clone()
}

/// Use the range constructor's error behavior before a normalized counting loop.
pub fn validate_range(start: i64, end: i64, step: i64) {
    let _ = incan_std_primitives::iter::range(start, end, step);
}

/// Raise an assertion panic: the empty message keeps the bare `AssertionError` payload.
pub fn assert_failure(message: String) {
    if message.is_empty() {
        std::panic::panic_any("AssertionError".to_owned());
    }
    std::panic::panic_any(format!("AssertionError: {message}"));
}

/// Initialize canonical language panic reporting before executing a native main body.
pub fn initialize_panic_hook() {
    panic_hook::initialize_panic_hook();
}

/// Preserve the tuple element debug spelling of an integer.
pub fn debug_int(value: i64) -> String {
    format!("{value:?}")
}

/// Preserve the tuple element debug spelling of a float.
pub fn debug_float(value: f64) -> String {
    format!("{value:?}")
}

/// Preserve the tuple element debug spelling of a boolean.
pub fn debug_bool(value: bool) -> String {
    format!("{value:?}")
}

/// Preserve the empty tuple spelling of a unit element.
pub fn debug_unit(value: ()) -> String {
    format!("{value:?}")
}

/// Quote and escape text with the tuple debug formatter.
pub fn debug_text(value: &String) -> String {
    format!("{value:?}")
}

