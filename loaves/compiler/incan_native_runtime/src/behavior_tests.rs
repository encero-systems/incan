//! Behavior of the runtime's public surface, pinned to the reference toolchain (release run 6 on `f0fa3b405d`).
//!
//! Expected values come from programs the reference home ran (`run1/golden` in the runtime lane: `ref1-main.incn`,
//! `ref2-main.incn` and `errs-cases.txt` with their captured output) and from the reference Incan runtime's own
//! source. Each test calls the runtime exactly as a plan does: through the public path and the call-site types.

use crate::*;
use std::collections::{HashMap, HashSet};

/// The unit whose cells the tests outside the statics module use; every unit keys its own cells.
const TEST_UNIT: u64 = 0x5465_7374_0000_0001;

/// Read a caught panic's text payload, as the language panic hook and `std.testing` read it.
fn panic_message<R>(run: impl FnOnce() -> R + std::panic::UnwindSafe) -> Option<String> {
    let payload = std::panic::catch_unwind(run).err()?;
    match payload.downcast_ref::<String>() {
        Some(text) => Some(text.clone()),
        None => payload.downcast_ref::<&str>().map(|text| (*text).to_owned()),
    }
}

fn text(value: &str) -> String {
    value.to_owned()
}

mod scalars {
    use super::*;

    #[test]
    fn scalar_formatting_keeps_the_language_spelling() {
        assert_eq!(format_int(-42), "-42");
        assert_eq!(format_float(3.0), "3.0");
        assert_eq!(format_float(1.0e16), "1e+16");
        assert_eq!(format_float(1.0e15), "1000000000000000.0");
        assert_eq!(format_float(-0.0), "-0.0");
        assert_eq!(format_float(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(format_float(1.8446744073709552e19), "1.8446744073709552e+19");
        assert_eq!(format_bool(true), "true");
        assert_eq!(format_bool(false), "false");
    }

    #[test]
    fn debug_spellings_match_tuple_display() {
        assert_eq!(debug_int(1), "1");
        assert_eq!(debug_float(2.0), "2.0");
        assert_eq!(debug_bool(true), "true");
        assert_eq!(debug_unit(()), "()");
        assert_eq!(debug_text(&text("x\n")), "\"x\\n\"");
    }

    #[test]
    fn assertion_failures_carry_the_language_payload() {
        assert_eq!(
            panic_message(|| assert_failure(String::new())).as_deref(),
            Some("AssertionError")
        );
        assert_eq!(
            panic_message(|| assert_failure(text("numbers differ"))).as_deref(),
            Some("AssertionError: numbers differ")
        );
    }

    #[test]
    fn range_validation_rejects_a_zero_step() {
        validate_range(0, 5, 1);
        assert_eq!(
            panic_message(|| validate_range(0, 5, 0)).as_deref(),
            Some("ValueError: range() arg 3 must not be zero")
        );
    }

    #[test]
    fn truth_follows_the_language_rules() {
        assert!(!truth_int(0) && truth_int(-1));
        assert!(!truth_float(0.0) && !truth_float(-0.0) && truth_float(f64::NAN));
        assert!(truth_bool(true) && !truth_bool(false));
        assert!(!truth_text(&String::new()) && truth_text(&text("x")));
        assert!(!truth_list::<i64>(&Vec::new()) && truth_list(&vec![1]));
        assert!(boolean_and(true, true) && !boolean_and(true, false));
        assert!(boolean_or(false, true) && !boolean_or(false, false));
    }

    #[test]
    fn absolute_and_sum_keep_their_overflow_errors() {
        assert_eq!(absolute_int(-7), 7);
        assert_eq!(absolute_int(7), 7);
        assert_eq!(
            panic_message(|| absolute_int(i64::MIN)).as_deref(),
            Some("ValueError: integer overflow in builtin `abs`")
        );
        assert_eq!(sum_int(&vec![1, 2, 3]), 6);
        assert_eq!(sum_bool(&vec![true, true, false]), 2);
        assert_eq!(
            panic_message(|| sum_int(&vec![i64::MAX, 1])).as_deref(),
            Some("ValueError: integer overflow in builtin `sum`")
        );
        assert_eq!(sum_int(&vec![i64::MIN, -0]), i64::MIN);
    }

    #[test]
    fn extrema_borrow_their_lists() {
        assert_eq!(minimum_int(&vec![3, 1, 2]), 1);
        assert_eq!(maximum_int(&vec![3, 1, 2]), 3);
        assert_eq!(minimum_float(&vec![3.5, 1.25]), 1.25);
        assert_eq!(maximum_float(&vec![3.5, 1.25]), 3.5);
        assert!(maximum_bool(&vec![true, false]));
        assert!(!minimum_bool(&vec![true, false]));
        assert_eq!(maximum_text(&vec![text("b"), text("a")]), "b");
        assert_eq!(minimum_text(&vec![text("b"), text("a")]), "a");
        assert_eq!(
            panic_message(|| minimum_int(&Vec::new())).as_deref(),
            Some("ValueError: min() arg is an empty sequence")
        );
    }

    #[test]
    fn integer_power_wraps_like_the_release_runtime() {
        assert_eq!(power_i64(2, 62), 4611686018427387904);
        assert_eq!(power_i64(3, 40), -6289078614652622815);
        assert_eq!(power_i64(5, 0), 1);
        assert_eq!(power_i8(3, 5), -13);
        assert_eq!(power_u8(2, 9), 0);
        assert_eq!(power_i32(-2, 3), -8);
        assert_eq!(power_u128(10, 20), 100_000_000_000_000_000_000);
        // Only the low 32 exponent bits count: -1 selects 4294967295, an odd exponent of -1 stays -1.
        assert_eq!(power_i64(-1, -1), -1);
        assert_eq!(power_i64(7, 1 << 32), 1);
    }

    #[test]
    fn float_power_uses_the_native_operation() {
        assert_eq!(power_f64(2.0, 0.5), 2.0_f64.powf(0.5));
        assert_eq!(power_f64(2.0, 2.0), 4.0);
        assert_eq!(power_f32(2.0, 3.0), 8.0);
    }

    #[test]
    fn files_report_their_native_errors() {
        let directory = std::env::temp_dir().join(format!("incan-runtime-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join("note.txt").to_string_lossy().into_owned();
        assert_eq!(write_file_text(&path, &text("héllo")), Ok(()));
        assert_eq!(read_file_text(&path), Ok(text("héllo")));
        let missing = directory.join("missing.txt").to_string_lossy().into_owned();
        assert_eq!(
            read_file_text(&missing),
            Err(std::fs::read_to_string(&missing)
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default())
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}

mod numbers {
    use super::*;

    #[test]
    fn exact_widths_cast_like_the_language() {
        assert_eq!(int_u64(u64::MAX), -1);
        assert_eq!(int_i128(i128::MAX), -1);
        assert_eq!(int_f64(1e30), i64::MAX);
        assert_eq!(int_f64(f64::NAN), 0);
        assert_eq!(int_f32(-2.75), -2);
        assert_eq!(int_u8(250), 250);
        assert_eq!(float_u64(u64::MAX), 1.8446744073709552e19);
        assert_eq!(float_i8(-3), -3.0);
        assert_eq!(float_f32(1.5), 1.5);
        assert_eq!(int_isize(-9), -9);
        assert_eq!(float_usize(4), 4.0);
    }

    #[test]
    fn exact_widths_format_with_their_display() {
        assert_eq!(format_i8(-13), "-13");
        assert_eq!(format_u128(u128::MAX), "340282366920938463463374607431768211455");
        assert_eq!(format_f32(1.5), "1.5");
        assert_eq!(format_f32(3.0), "3.0");
        assert_eq!(format_f32(0.0000001), "0.0000001");
        assert_eq!(format_f64(3.0), "3.0");
        assert_eq!(format_isize(-1), "-1");
        assert_eq!(format_usize(7), "7");
    }

    #[test]
    fn python_division_keeps_flooring_and_zero_errors() {
        assert_eq!(py_floor_div_i8(-7, 3), -3);
        assert_eq!(py_mod_i8(-7, 3), 2);
        assert_eq!(py_floor_div_i128(-7, 2), -4);
        assert_eq!(py_mod_i128(-7, 3), 2);
        assert_eq!(py_floor_div_u8(124, 30), 4);
        assert_eq!(py_mod_f64(-1.0, 3.0), 2.0);
        assert_eq!(py_floor_div_f32(7.0, 2.0), 3.0);
        assert_eq!(
            panic_message(|| py_mod_i32(5, 0)).as_deref(),
            Some("ZeroDivisionError: integer division or modulo by zero")
        );
    }

    #[test]
    fn negation_wraps_at_the_minimum() {
        assert_eq!(negate_i8(5), -5);
        assert_eq!(negate_i8(i8::MIN), i8::MIN);
        assert_eq!(negate_i128(-3), 3);
        assert_eq!(negate_isize(isize::MIN), isize::MIN);
    }

    #[test]
    fn parsing_keeps_the_language_errors() {
        assert_eq!(parse_int(&text("42")), 42);
        assert_eq!(parse_float(&text("1.5")), 1.5);
        assert_eq!(
            panic_message(|| parse_int(&text("4x"))).as_deref(),
            Some("ValueError: cannot convert '4x' to int")
        );
        assert_eq!(
            panic_message(|| parse_int(&text("  42 "))).as_deref(),
            Some("ValueError: cannot convert '  42 ' to int")
        );
        assert_eq!(
            panic_message(|| parse_float(&text("abc"))).as_deref(),
            Some("ValueError: cannot convert 'abc' to float")
        );
    }

    #[test]
    fn float_classification_uses_the_carrier() {
        assert!(is_finite_f64(1.0) && !is_finite_f64(f64::INFINITY));
        assert!(is_nan_f32(f32::NAN) && !is_nan_f32(0.0));
        assert!(is_finite_f32(1.0) && is_nan_f64(f64::NAN));
    }

    #[test]
    fn u8_policies_are_the_native_ones() {
        assert_eq!(checked_add_u8(250, 10), None);
        assert_eq!(checked_sub_u8(5, 3), Some(2));
        assert_eq!(checked_mul_u8(16, 16), None);
        assert_eq!(checked_pow_u8(2, 7), Some(128));
        assert_eq!(wrapping_add_u8(250, 10), 4);
        assert_eq!(wrapping_sub_u8(0, 1), 255);
        assert_eq!(wrapping_mul_u8(16, 16), 0);
        assert_eq!(wrapping_pow_u8(2, 8), 0);
        assert_eq!(saturating_add_u8(250, 10), 255);
        assert_eq!(saturating_sub_u8(0, 1), 0);
        assert_eq!(saturating_mul_u8(16, 16), 255);
        assert_eq!(saturating_pow_u8(2, 8), 255);
    }

    #[test]
    fn decimals_compare_by_value() {
        let one = decimal_from_parts(10, 1);
        let same = decimal_from_parts(100, 2);
        let more = decimal_from_parts(15, 1);
        assert_eq!(format_decimal(one), "1.0");
        assert_eq!(format_decimal(same), "1.00");
        assert!(equal_decimal(one, same) && !unequal_decimal(one, same));
        assert!(less_decimal(one, more) && less_equal_decimal(one, same));
        assert!(greater_decimal(more, one) && greater_equal_decimal(one, same));
    }
}

mod texts {
    use super::*;

    #[test]
    fn text_operations_keep_their_legacy_results() {
        let sample = text("  Héllo, World  ");
        assert_eq!(strip_text(&sample), "Héllo, World");
        assert_eq!(upper_text(&sample), "  HÉLLO, WORLD  ");
        assert_eq!(lower_text(&sample), "  héllo, world  ");
        assert_eq!(length_text(&text("héllo")), 5);
        assert_eq!(slice_text(&text("héllo"), true, 1, true, 3, false, 0), "él");
        assert_eq!(slice_text(&text("héllo"), true, -2, false, 0, false, 0), "lo");
        assert_eq!(slice_text(&text("abcdef"), false, 0, false, 0, true, 2), "ace");
        assert_eq!(slice_text_bounds(&text("héllo"), 1, 3), "él");
        assert_eq!(replace_text(&text("abc"), &text("b"), &text("BB")), "aBBc");
        assert_eq!(concat_text(&text("ab"), &text("cd")), "abcd");
    }

    #[test]
    fn text_predicates_and_comparisons() {
        assert!(contains_text(&text("hello"), &text("ell")));
        assert!(not_contains_text(&text("hello"), &text("xyz")));
        assert!(starts_text(&text("héllo"), &text("hé")));
        assert!(ends_text(&text("héllo"), &text("lo")));
        assert!(less_text(&text("abc"), &text("abd")));
        assert!(less_equal_text(&text("abc"), &text("abc")));
        assert!(greater_text(&text("b"), &text("a")));
        assert!(greater_equal_text(&text("b"), &text("b")));
        assert!(equal_text(&text("a"), &text("a")) && unequal_text(&text("a"), &text("b")));
    }

    #[test]
    fn splitting_and_joining() {
        assert_eq!(split_text(&text("a,b,,c"), &text(",")), vec!["a", "b", "", "c"]);
        assert_eq!(split_default_text(&text("a b  c")), vec!["a b  c"]);
        assert_eq!(split_whitespace_text(&text(" a b\u{3000}c ")), vec!["a", "b", "c"]);
        assert_eq!(
            join_text(&text(", "), &vec![text("x"), text("y"), text("z")]),
            "x, y, z"
        );
    }

    #[test]
    fn text_iteration_yields_scalars() {
        assert_eq!(text_items(&text("hé!")), vec!["h", "é", "!"]);
        assert_eq!(text_items(&String::new()), Vec::<String>::new());
        assert_eq!(enumerate_text(&text("hé")), vec![(0, text("h")), (1, text("é"))]);
        assert_eq!(enumerate_bytes(&vec![7_u8, 255]), vec![(0, 7), (1, 255)]);
        assert_eq!(
            enumerate_list(&vec![text("p"), text("q")]),
            vec![(0, text("p")), (1, text("q"))]
        );
    }

    #[test]
    fn text_codec_policies_and_errors() {
        assert!(text_codec_encoding_valid(&text(" UTF_8 ")));
        assert!(!text_codec_encoding_valid(&text("latin-1")));
        assert!(text_codec_errors_valid(&text("replace")) && !text_codec_errors_valid(&text("ignore")));
        assert_eq!(
            decode_replacement_policy(&text("utf-8"), &text("replace")),
            (true, true)
        );
        assert_eq!(
            decode_replacement_policy(&text("utf-8"), &text("strict")),
            (true, false)
        );
        assert_eq!(
            decode_replacement_policy(&text("utf-8"), &text("ignore")),
            (false, false)
        );
        assert_eq!(
            decode_replacement_policy(&text("latin-1"), &text("strict")),
            (false, false)
        );
        assert_eq!(
            invalid_decoder_options(&text("Latin_1"), &text("strict")).to_string(),
            "unknown-encoding: bytes.decode() supports only utf-8 in this release, got encoding 'latin-1'"
        );
        assert_eq!(
            invalid_decoder_options(&text("utf-8"), &text("ignore")).to_string(),
            "unknown-errors-policy: bytes.decode() errors must be \"strict\" or \"replace\", got 'ignore'"
        );
        assert_eq!(
            invalid_utf8_error(text("bad byte")).to_string(),
            "invalid-utf8: 'utf-8' codec can't decode bytes: bad byte"
        );
        validate_encoder_label(&text("utf8"));
        assert_eq!(
            panic_message(|| validate_encoder_label(&text("Latin-1"))).as_deref(),
            Some("ValueError: str.encode() supports only utf-8 in this release, got encoding 'latin-1'")
        );
    }

    #[test]
    fn frozen_text_observations() {
        assert_eq!(format_frozen(FrozenCarrier::new("frozen")), "frozen");
        assert_eq!(length_frozen_bytes(FrozenBytesCarrier::new(b"abc")), 3);
        assert_eq!(copy_frozen_bytes(FrozenBytesCarrier::new(b"abc")), b"abc".to_vec());
    }
}

mod lists_and_collections {
    use super::*;

    #[test]
    fn list_operations_keep_python_indexing() {
        let mut values = empty_list::<i64>();
        append_list(&mut values, 3);
        append_list(&mut values, 1);
        append_list(&mut values, 2);
        assert_eq!(length_list(&values), 3);
        assert_eq!(get_list(&values, -1), 2);
        assert_eq!(sorted_list(&values), vec![1, 2, 3]);
        assert_eq!(sorted_float(&vec![2.5, -1.0, 0.0]), vec![-1.0, 0.0, 2.5]);
        assert_eq!(slice_list(&values, true, 0, true, 2, false, 0), vec![3, 1]);
        assert_eq!(count_list(&values, 1), 1);
        assert_eq!(index_list(&values, 2), 2);
        assert!(contains_list(&values, 3) && !contains_list(&values, 9));
        assert_eq!(concat_list(&values, &vec![9]), vec![3, 1, 2, 9]);
        assert_eq!(repeat_list(0, 3), vec![0, 0, 0]);
        assert!(equal_list(&values, &vec![3, 1, 2]) && unequal_list(&values, &vec![3]));
        set_list(&mut values, 0, 7);
        *borrow_list_mut(&mut values, -1) = 8;
        swap_list(&mut values, 0, -1);
        assert_eq!(values, vec![8, 1, 7]);
        extend_list(&mut values, &vec![4]);
        remove_list(&mut values, 1);
        assert_eq!(pop_list(&mut values), 4);
        assert_eq!(clone_list(&values), vec![8, 7]);
    }

    #[test]
    fn stored_exclusive_references_are_reborrowed_to_their_referent() {
        let mut first = String::from("ada");
        let mut second = String::from("grace");
        let mut views: Vec<&mut String> = vec![&mut first, &mut second];
        reborrow_list_mut(&mut views, -1).push('!');
        *reborrow_list_mut(&mut views, 0) = String::from("Leiden");
        assert_eq!((first.as_str(), second.as_str()), ("Leiden", "grace!"));
    }

    #[test]
    fn list_errors_keep_their_messages() {
        assert_eq!(
            panic_message(|| get_list(&vec![1, 2], 5)).as_deref(),
            Some("IndexError: index 5 out of range for list of length 2")
        );
        assert_eq!(
            panic_message(|| pop_list(&mut Vec::<i64>::new())).as_deref(),
            Some("IndexError: pop from empty list")
        );
        assert_eq!(
            panic_message(|| index_list(&vec![1, 2], 5)).as_deref(),
            Some("ValueError: value not found in list")
        );
    }

    #[test]
    fn dictionaries_copy_values_and_report_missing_keys() {
        let mut values = empty_dict::<String, i64>();
        insert_dict(&mut values, text("a"), 1);
        insert_dict(&mut values, text("b"), 2);
        assert_eq!(length_dict(&values), 2);
        assert!(contains_dict(&values, text("a")) && !contains_dict(&values, text("zz")));
        assert_eq!(get_dict(&values, text("a")), 1);
        assert_eq!(default_dict(&values, text("zz"), 7), 7);
        assert_eq!(optional_dict(&values, text("a")), Some(1));
        assert_eq!(optional_dict(&values, text("zz")), None);
        *borrow_mut_dict(&mut values, text("b")) += 10;
        assert_eq!(get_dict(&values, text("b")), 12);
        let mut keys = keys_dict(&values);
        keys.sort();
        assert_eq!(keys, vec![text("a"), text("b")]);
        let mut items = values_dict(&values);
        items.sort();
        assert_eq!(items, vec![1, 12]);
        assert!(equal_dict(&values, &clone_dict(&values)));
        assert!(unequal_dict(&values, &empty_dict()));
        let first = nth_dict(&values, 0);
        assert_eq!(keys_dict(&values)[0], first);
        assert_eq!(
            panic_message(|| get_dict(&values, text("zz"))).as_deref(),
            Some("KeyError: 'zz' not found in dict")
        );
        let mut target = empty_dict::<String, i64>();
        extend_dict(&mut target, &values);
        assert_eq!(target, values);
    }

    #[test]
    fn sets_keep_membership_and_cursors() {
        let mut values = empty_set::<i64>();
        insert_set(&mut values, 1);
        insert_set(&mut values, 2);
        insert_set(&mut values, 2);
        assert_eq!(length_set(&values), 2);
        assert!(contains_set(&values, 2) && !contains_set(&values, 3));
        assert!(equal_set(&values, &clone_set(&values)) && unequal_set(&values, &empty_set()));
        let first = nth_set(&values, 0);
        assert_eq!(values.iter().next().copied(), Some(first));
        assert_eq!(
            panic_message(|| nth_set(&values, 5)).as_deref(),
            Some("IndexError: set cursor outside collection")
        );
        let mut grown = empty_set::<i64>();
        extend_list_set(&mut grown, &vec![3, 3, 4]);
        extend_set(&mut grown, &values);
        assert_eq!(grown, HashSet::from([1, 2, 3, 4]));
    }

    #[test]
    fn zip_cursors_poll_left_before_right() {
        let mut zipped = zip_lists(vec![1, 2, 3], vec![text("a"), text("b")]);
        let mut output = Vec::new();
        assert!(poll_zip(&mut zipped, &mut output));
        let copy = clone_zip(&zipped);
        assert!(poll_zip(&mut zipped, &mut output));
        assert!(!poll_zip(&mut zipped, &mut output));
        assert_eq!(output, vec![(1, text("a")), (2, text("b"))]);
        assert_eq!(zipped.left_index, 3);
        assert_eq!(zipped.right_index, 2);
        let rest: Vec<(i64, String)> = zip_lists_generator(copy).collect();
        assert_eq!(rest, vec![(2, text("b"))]);
    }
}

mod statics_and_names {
    use super::*;

    /// The unit these cells belong to; every unit keys its own cells.
    const UNIT: u64 = 0x4c6f_6166_0000_0001;

    #[test]
    fn static_cells_persist_across_calls() {
        assert_eq!(read_static_int(UNIT, 9001, 5), 5);
        write_static_int(UNIT, 9001, 6);
        assert_eq!(read_static_int(UNIT, 9001, 5), 6);
        write_static_text(UNIT, 9002, text("t"));
        assert_eq!(read_static_text(UNIT, 9002, text("x")), "t");
        assert_eq!(read_static_float(UNIT, 9003, 1.5), 1.5);
        write_static_bool(UNIT, 9004, true);
        assert!(read_static_bool(UNIT, 9004, false));
        write_static_float(UNIT, 9005, 2.5);
        assert_eq!(read_static_float(UNIT, 9005, 0.0), 2.5);
        assert_eq!(read_static_int_set(UNIT, 9006, HashSet::from([1])), HashSet::from([1]));
        write_static_int_set(UNIT, 9006, HashSet::from([2]));
        assert_eq!(read_static_int_set(UNIT, 9006, HashSet::new()), HashSet::from([2]));
        write_static_text_int_dict(UNIT, 9007, HashMap::from([(text("k"), 1)]));
        assert_eq!(
            read_static_text_int_dict(UNIT, 9007, HashMap::new()),
            HashMap::from([(text("k"), 1)])
        );
    }

    #[test]
    fn module_initialization_is_claimed_once() {
        assert!(begin_module_initialization(UNIT, 77));
        assert!(!begin_module_initialization(UNIT, 77));
        end_module_initialization(UNIT, 77);
        assert!(!begin_module_initialization(UNIT, 77));
    }

    /// Two units each number their statics from 0; their slot 0 are two cells.
    #[test]
    fn the_same_slot_in_two_units_is_two_cells() {
        let (loaf, program) = (0x9f3a_51c2_7e10_44d8, 0x2b71_e0aa_3c95_1f06);
        write_static_int(loaf, 0, 2);
        assert_eq!(read_static_int(program, 0, 100), 100);
        assert_eq!(read_static_int(loaf, 0, 0), 2);
        write_static_text(program, 0, text("program"));
        assert_eq!(read_static_text(loaf, 0, text("loaf")), "loaf");
        write_static_int_set(loaf, 0, HashSet::from([3]));
        assert_eq!(read_static_int_set(program, 0, HashSet::new()), HashSet::new());
    }

    /// A unit's module initialization claim is its own; another unit's module key 0 is still unclaimed.
    #[test]
    fn module_initialization_is_claimed_per_unit() {
        let (loaf, program) = (0x51c2_9f3a_44d8_7e10, 0xe0aa_2b71_1f06_3c95);
        assert!(begin_module_initialization(loaf, 0));
        assert!(!begin_module_initialization(loaf, 0));
        assert!(begin_module_initialization(program, 0));
        end_module_initialization(loaf, 0);
        assert!(!begin_module_initialization(loaf, 0));
    }

    #[test]
    fn callable_names_are_scoped_by_module() {
        assert_eq!(callable_name(text("m"), 41), "<callable>");
        register_callable_name(text("m"), 41, text("first"));
        register_callable_name(text("m"), 41, text("renamed"));
        assert_eq!(callable_name(text("m"), 41), "renamed");
        assert_eq!(callable_name(text("other"), 41), "<callable>");
    }

    #[test]
    fn decorated_bindings_publish_once() {
        fn publish() {
            write_static_int(UNIT, 9100, read_static_int(UNIT, 9100, 0) + 1);
        }
        let address = publish as fn() as usize;
        initialize_binding(text("m"), text("inst"), publish, address, 1, text("decorated"));
        initialize_binding(text("m"), text("inst"), publish, address, 1, text("decorated"));
        assert_eq!(read_static_int(UNIT, 9100, 0), 1);
        assert_eq!(callable_name(text("m"), 1), "decorated");
    }

    #[test]
    fn recursive_decorated_binding_is_refused() {
        fn reenter() {
            initialize_binding(
                text("m2"),
                text("loop"),
                reenter,
                reenter as fn() as usize,
                2,
                text("loop"),
            );
        }
        assert_eq!(
            panic_message(|| {
                initialize_binding(
                    text("m2"),
                    text("loop"),
                    reenter,
                    reenter as fn() as usize,
                    2,
                    text("loop"),
                )
            })
            .as_deref(),
            Some("ValueError: recursive initialization of decorated callable 'loop'")
        );
    }
}

mod failures {
    use super::*;

    #[test]
    fn unwraps_keep_the_standard_messages() {
        assert_eq!(
            panic_message(unwrap_none).as_deref(),
            Some("called `Option::unwrap()` on a `None` value")
        );
        assert_eq!(
            panic_message(|| unwrap_err(text("boom"))).as_deref(),
            Some("called `Result::unwrap()` on an `Err` value: \"boom\"")
        );
    }

    #[test]
    fn pattern_assertions_keep_std_testing_messages() {
        assert_eq!(
            panic_message(missing_some).as_deref(),
            Some("assertion failed: expected Some, got None")
        );
        assert_eq!(
            panic_message(missing_ok).as_deref(),
            Some("assertion failed: expected Ok, got Err")
        );
        assert_eq!(
            panic_message(missing_err).as_deref(),
            Some("assertion failed: expected Err, got Ok")
        );
        assert_eq!(
            panic_message(unexpected_some).as_deref(),
            Some("assertion failed: expected None, got Some")
        );
        assert_eq!(panic_message(|| missing_some_with(text("why"))).as_deref(), Some("why"));
    }

    #[test]
    fn raises_assertions_match_the_error_category() {
        fn value_error() {
            incan_std_primitives::errors::raise_value_error("bad");
        }
        fn quiet() {}
        assert_raises_deferred(value_error, text("ValueError"), String::new());
        assert_eq!(
            panic_message(|| assert_raises_deferred(quiet, text("ValueError"), String::new())).as_deref(),
            Some("AssertionError: expected ValueError to be raised")
        );
        assert_eq!(
            panic_message(|| assert_raises_deferred(value_error, text("KeyError"), String::new())).as_deref(),
            Some("AssertionError: expected KeyError to be raised, got ValueError: bad")
        );
        assert_eq!(
            panic_message(|| assert_raises_deferred(quiet, text("KeyError"), text("custom"))).as_deref(),
            Some("AssertionError: custom")
        );
    }

    #[test]
    fn newtype_and_validation_helpers() {
        assert_eq!(parse_newtype_int(text("12")), Ok(12));
        assert_eq!(parse_newtype_int(text("x")), Err(text("invalid digit found in string")));
        assert_eq!(add_newtype_sum(i64::MAX, 1), i64::MIN);
        assert_eq!(checked_newtype_result::<i64, String>(Ok(3), text("T"), text("hook")), 3);
        assert_eq!(checked_sum_result::<i64, String>(Ok(4), text("ctx")), 4);
        assert_eq!(
            panic_message(|| checked_sum_result::<i64, String>(Err(text("cause")), text("ctx"))).as_deref(),
            Some("ctx: cause")
        );
        validate_newtype_constraint(true, text("Port"), text("ge=1"));
        assert_eq!(checked_json_result::<i64, String>(Err(text("bad"))), Err(text("bad")));
        assert_eq!(format_validation_error(&make_validation_error(text("m"))), "m");
        assert_eq!(
            format_validation_error(&make_coded_validation_error(text("m"), text("c"))),
            "c: m"
        );
    }

    #[test]
    fn carriers_use_their_native_traits() {
        assert_eq!(format_carrier(&Some(3)), "Some(3)");
        assert_eq!(clone_carrier_payload(&text("p")), "p");
        assert_eq!(raise_value_error_message(), Some(text("ValueError: stop")));
    }

    fn raise_value_error_message() -> Option<String> {
        panic_message(|| raise_value_error(text("stop")))
    }
}

mod models {
    use super::*;

    #[derive(Debug, Clone, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
    struct Point {
        x: i64,
        label: String,
    }

    #[test]
    fn model_operations_use_the_derived_traits() {
        let left = Point { x: 1, label: text("a") };
        let right = Point { x: 2, label: text("b") };
        assert_eq!(debug_model(&left), "Point { x: 1, label: \"a\" }");
        assert_eq!(debug_value(text("q")), "\"q\"");
        assert_eq!(display_value(2.0_f64), "2");
        assert_eq!(display_type_parameter(5_u8), "5");
        assert!(equal_model(&left, &left.clone()) && unequal_model(&left, &right));
        assert!(less_model(&left, &right) && less_equal_model(&left, &left));
        assert!(greater_model(&right, &left) && greater_equal_model(&right, &right));
        assert_eq!(serialize_model(&left), "{\"x\":1,\"label\":\"a\"}");
        assert_eq!(deserialize_model::<Point>(&text("{\"x\":1,\"label\":\"a\"}")), Ok(left));
        assert!(deserialize_model::<Point>(&text("{")).is_err());
        assert_eq!(serialize_int(3), "3");
        assert_eq!(serialize_float(1.5), "1.5");
        assert_eq!(serialize_bool(true), "true");
        assert_eq!(serialize_string(&text("a\"b")), "\"a\\\"b\"");
    }
}

mod frozen {
    use super::*;

    #[test]
    fn frozen_lists_and_sets_read_in_baked_order() {
        let list = freeze_list(vec![text("b"), text("a")]);
        assert_eq!(frozen_list_length(list), 2);
        assert_eq!(frozen_list_length_ref(&list), 2);
        assert_eq!(frozen_list_items(list), vec![text("b"), text("a")]);
        assert_eq!(
            frozen_list_text_items(freeze_list(vec![1, 2])),
            vec![text("1"), text("2")]
        );
        let bytes: incan_std_primitives::frozen::FrozenList<&'static [u8]> =
            incan_std_primitives::frozen::FrozenList::new(&[b"ab", b"c"]);
        assert_eq!(frozen_list_byte_items(bytes), vec![b"ab".to_vec(), b"c".to_vec()]);
        assert_eq!(own_static_bytes::<&'static [u8]>(b"xy"), b"xy".to_vec());
        let set = incan_std_primitives::frozen::FrozenSet::new(&[3_i64, 1]);
        assert_eq!(frozen_set_length(set), 2);
        assert_eq!(frozen_set_length_ref(&set), 2);
        assert_eq!(frozen_set_items(set), vec![3, 1]);
        assert_eq!(frozen_set_text_items(set), vec![text("3"), text("1")]);
    }

    #[test]
    fn frozen_dicts_find_first_matching_keys() {
        let dict = incan_std_primitives::frozen::FrozenDict::new(&[("a", 1_i64), ("b", 2), ("a", 3)]);
        assert_eq!(frozen_dict_length(dict), 3);
        assert_eq!(frozen_dict_length_ref(&dict), 3);
        assert_eq!(frozen_dict_keys(dict), vec!["a", "b", "a"]);
        assert_eq!(frozen_dict_text_keys(dict), vec![text("a"), text("b"), text("a")]);
        assert_eq!(frozen_dict_text_get(dict, &text("b")), 2);
        assert!(frozen_dict_text_contains(dict, &text("a")) && !frozen_dict_text_contains(dict, &text("z")));
        let numbers = incan_std_primitives::frozen::FrozenDict::new(&NUMBERS);
        assert_eq!(frozen_dict_get(numbers, 2), "two");
        assert!(frozen_dict_contains(numbers, 1) && !frozen_dict_contains(numbers, 3));
        assert_eq!(
            panic_message(|| frozen_dict_get(numbers, 9)).as_deref(),
            Some("KeyError: '9' not found in dict")
        );
        assert_eq!(
            panic_message(|| frozen_dict_text_get(dict, &text("zz"))).as_deref(),
            Some("KeyError: 'zz' not found in dict")
        );
    }

    static NUMBERS: [(i64, &str); 2] = [(1, "one"), (2, "two")];
}

mod generator_chains {
    use super::*;
    use incan_std_primitives::iter::Generator;

    fn ints(values: Vec<i64>) -> Generator<i64> {
        list_generator(values)
    }

    fn collect<T>(values: Generator<T>) -> Vec<T> {
        values.collect()
    }

    #[test]
    fn sources_poll_lazily() {
        let mut source = range_generator(0, 3, 1);
        let mut output = Vec::new();
        assert!(poll_generator(&mut source, &mut output));
        assert!(poll_generator(&mut source, &mut output));
        assert!(poll_generator(&mut source, &mut output));
        assert!(!poll_generator(&mut source, &mut output));
        assert_eq!(output, vec![0, 1, 2]);
        assert_eq!(collect(once_generator(5)), vec![5]);
        assert_eq!(collect(empty_generator::<i64>()), Vec::<i64>::new());
        let mut from_set = collect(set_generator(HashSet::from([1, 2])));
        from_set.sort();
        assert_eq!(from_set, vec![1, 2]);
    }

    #[test]
    fn adapters_keep_their_order_and_limits() {
        assert_eq!(collect(take_generator(ints(vec![1, 2, 3]), 2)), vec![1, 2]);
        assert_eq!(collect(take_generator(ints(vec![1, 2, 3]), -1)), Vec::<i64>::new());
        assert_eq!(collect(skip_generator(ints(vec![1, 2, 3]), 1)), vec![2, 3]);
        assert_eq!(
            collect(batch_generator(ints(vec![1, 2, 3]), 2)),
            vec![vec![1, 2], vec![3]]
        );
        assert_eq!(collect(chain_generators(ints(vec![1]), ints(vec![2]))), vec![1, 2]);
        assert_eq!(collect(enumerate_generator(ints(vec![7, 8]))), vec![(0, 7), (1, 8)]);
        assert_eq!(
            collect(zip_generators(ints(vec![1, 2, 3]), ints(vec![4, 5]))),
            vec![(1, 4), (2, 5)]
        );
        assert_eq!(
            collect(take_while_int_generator(ints(vec![1, 2, 5, 1]), |v| v < 3)),
            vec![1, 2]
        );
        assert_eq!(
            collect(skip_while_int_generator(ints(vec![1, 2, 5, 1]), |v| v < 3)),
            vec![5, 1]
        );
        assert_eq!(
            collect(filter_int_generator(ints(vec![1, 2, 3, 4]), |v| v % 2 == 0)),
            vec![2, 4]
        );
        assert_eq!(
            collect(filter_float_generator(list_generator(vec![1.0, -1.0]), |v| v > 0.0)),
            vec![1.0]
        );
        assert_eq!(
            collect(filter_bool_generator(list_generator(vec![true, false]), |v| v)),
            vec![true]
        );
        assert_eq!(collect(map_int_generator(ints(vec![1, 2]), |v| v * 10)), vec![10, 20]);
        assert_eq!(
            collect(map_float_generator(list_generator(vec![1.5]), |v| v * 2.0)),
            vec![3.0]
        );
        assert_eq!(
            collect(map_bool_generator(list_generator(vec![true]), |v| !v)),
            vec![false]
        );
    }

    #[test]
    fn expansions_draw_each_step_when_polled() {
        assert_eq!(
            collect(int_step_generator(ints(vec![1, 2]), |v| ints(vec![v, v]))),
            vec![1, 1, 2, 2]
        );
        assert_eq!(
            collect(float_step_generator(list_generator(vec![1.0]), once_generator)),
            vec![1.0]
        );
        assert_eq!(
            collect(bool_step_generator(list_generator(vec![true]), once_generator)),
            vec![true]
        );
        assert_eq!(
            collect(flat_map_int_generator(ints(vec![1, 2]), |v| vec![v; 2])),
            vec![1, 1, 2, 2]
        );
        assert_eq!(
            collect(flat_map_int_generators(ints(vec![2]), |v| ints(vec![v, v + 1]))),
            vec![2, 3]
        );
        assert_eq!(
            collect(flat_map_int_set_generator(ints(vec![4]), |v| HashSet::from([v]))),
            vec![4]
        );
        let step: fn(i64) -> Generator<i64> = |v| ints(vec![v * 2]);
        assert_eq!(collect(int_callable_step_generator(ints(vec![1, 2]), step)), vec![2, 4]);
        let text_step: fn(String) -> Generator<usize> = |v| once_generator(v.len());
        assert_eq!(
            collect(str_callable_step_generator(list_generator(vec![text("ab")]), text_step)),
            vec![2]
        );
        let float_step: fn(f64) -> Generator<f64> = once_generator;
        assert_eq!(
            collect(float_callable_step_generator(list_generator(vec![2.5]), float_step)),
            vec![2.5]
        );
        let bool_step: fn(bool) -> Generator<bool> = once_generator;
        assert_eq!(
            collect(bool_callable_step_generator(list_generator(vec![false]), bool_step)),
            vec![false]
        );
        let item_step: fn((i64, i64)) -> Generator<i64> = |(a, b)| ints(vec![a + b]);
        assert_eq!(
            collect(item_callable_step_generator(list_generator(vec![(1, 2)]), item_step)),
            vec![3]
        );
        let map: fn(String) -> usize = |v| v.len();
        assert_eq!(
            collect(map_text_callable_generator(list_generator(vec![text("abc")]), map)),
            vec![3]
        );
        let expand: fn(String) -> Vec<char> = |v| v.chars().collect();
        assert_eq!(
            collect(flat_map_text_callable_generator(
                list_generator(vec![text("ab")]),
                expand
            )),
            vec!['a', 'b']
        );
    }

    #[test]
    fn consumers_drain_in_order() {
        assert_eq!(count_generator(ints(vec![1, 2, 3])), 3);
        assert_eq!(sum_int_generator(ints(vec![1, 2, 3])), 6);
        assert_eq!(fold_int_generator(ints(vec![1, 2, 3]), 10, |a, b| a - b), 4);
        assert_eq!(reduce_int_generator(ints(vec![1, 2, 3]), 0, |a, b| a + b), 6);
        assert!(any_int_generator(ints(vec![1, 2]), |v| v == 2));
        assert!(!all_int_generator(ints(vec![1, 2]), |v| v == 2));
        assert_eq!(find_int_generator(ints(vec![1, 2, 3]), |v| v > 1), Some(2));
        assert_eq!(find_int_generator(ints(vec![1]), |v| v > 1), None);
        let check: fn(String) -> bool = |v| v == "b";
        assert!(any_text_callable_generator(
            list_generator(vec![text("a"), text("b")]),
            check
        ));
        for_each_int_generator(ints(vec![9100, 9101]), |slot| write_static_int(TEST_UNIT, slot, slot));
        assert_eq!(read_static_int(TEST_UNIT, 9101, 0), 9101);
    }
}

mod async_entries {
    use super::*;

    #[test]
    fn future_entries_run_on_the_canonical_executor() {
        run_future_entry(async {});
        assert_eq!(run_result_future_entry::<String, _>(async { Ok(()) }), Ok(()));
        assert_eq!(
            run_result_future_entry(async { Err(text("failed")) }),
            Err(text("failed"))
        );
        fn entry() {
            write_static_int(TEST_UNIT, 9200, 1);
        }
        run_async_entry(entry);
        assert_eq!(read_static_int(TEST_UNIT, 9200, 0), 1);
    }

    #[test]
    fn races_return_the_first_arm_in_source_order() {
        let winner = run_result_future_entry::<String, _>(async {
            let first: fn(i64) -> i64 = |v| v + 1;
            let second: fn(i64) -> i64 = |v| v + 100;
            let result = race_pair(async { 1_i64 }, first, async { 2_i64 }, second).await;
            write_static_int(TEST_UNIT, 9201, result);
            Ok(())
        });
        assert_eq!(winner, Ok(()));
        assert_eq!(read_static_int(TEST_UNIT, 9201, 0), 2);
    }
}
