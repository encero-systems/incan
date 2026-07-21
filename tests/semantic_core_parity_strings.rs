//! Parity checks for string semantics between the shared semantic core and runtime stdlib,
//! plus const-eval acceptance of string operations.

use incan::frontend::typechecker::{ConstValue, TypeCheckInfo};
use incan::frontend::{lexer, parser, typechecker};
use incan_core::errors::IncanError;
use incan_core::strings::{
    StringAccessError, str_char_at, str_concat, str_contains, str_slice, str_slice_byte_range,
    str_slice_from_byte_offset,
};
use incan_stdlib::strings::{
    str_concat as rt_str_concat, str_index as rt_str_index, str_slice as rt_str_slice,
    str_slice_byte_range as rt_str_slice_byte_range, str_slice_from_byte_offset as rt_str_slice_from_byte_offset,
};

fn run_const_eval_with_info(src: &str) -> Result<TypeCheckInfo, Vec<String>> {
    let tokens = lexer::lex(src).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>())?;
    let ast = parser::parse(&tokens).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>())?;
    let mut checker = typechecker::TypeChecker::new();
    match checker.check_program(&ast) {
        Ok(_) => Ok(checker.type_info().clone()),
        Err(errs) => Err(errs.into_iter().map(|e| e.message).collect()),
    }
}

#[test]
fn semantics_vs_runtime_concat_and_contains() {
    assert!(str_contains("héllo", "éll"));
    assert!(!str_contains("héllo", "xyz"));
    assert_eq!(rt_str_concat("foo", "bar"), str_concat("foo", "bar"));

    // Comparisons
    assert!(incan_stdlib::strings::str_eq("abc", "abc"));
    assert!(incan_stdlib::strings::str_lt("abc", "abd"));
    assert!(incan_stdlib::strings::str_gt("abd", "abc"));
}

#[test]
fn compiler_owned_byte_slice_helpers_keep_the_registered_runtime_abi() {
    let _: fn(&str, i64, i64) -> String = rt_str_slice_byte_range;
    let _: fn(&str, i64) -> String = rt_str_slice_from_byte_offset;
}

#[test]
fn semantics_vs_runtime_index_and_slice() -> Result<(), StringAccessError> {
    let s = "héllo";
    // Index
    assert_eq!(rt_str_index(s, 1), str_char_at(s, 1)?);
    assert_eq!(rt_str_index(s, -1), str_char_at(s, -1)?);

    // Slice forward
    assert_eq!(
        rt_str_slice(s, Some(1), Some(4), Some(1)),
        str_slice(s, Some(1), Some(4), Some(1))?
    );
    // Slice with step
    assert_eq!(
        rt_str_slice(s, Some(0), Some(5), Some(2)),
        str_slice(s, Some(0), Some(5), Some(2))?
    );
    // Slice backwards
    assert_eq!(
        rt_str_slice(s, Some(4), Some(0), Some(-2)),
        str_slice(s, Some(4), Some(0), Some(-2))?
    );
    // Slice backwards with default end (Python-like `[::-1]` behavior)
    assert_eq!(
        rt_str_slice(s, Some(-1), None, Some(-1)),
        str_slice(s, Some(-1), None, Some(-1))?
    );
    // Negative start
    assert_eq!(
        rt_str_slice(s, Some(-2), None, None),
        str_slice(s, Some(-2), None, None)?
    );

    // Host byte-span reconstruction remains separate from ordinary Unicode-scalar slices.
    assert_eq!(rt_str_slice_byte_range(s, 1, 3), str_slice_byte_range(s, 1, 3)?);
    assert_eq!(rt_str_slice_from_byte_offset(s, 3), str_slice_from_byte_offset(s, 3)?);

    // Methods parity
    assert_eq!(incan_stdlib::strings::str_upper("héllo"), "HÉLLO");
    assert_eq!(incan_stdlib::strings::str_lower("HÉLLO"), "héllo");
    assert_eq!(incan_stdlib::strings::str_strip("  hi  "), "hi");
    assert_eq!(incan_stdlib::strings::str_replace("abcabc", "ab", "xy"), "xycxyc");
    assert_eq!(
        incan_stdlib::strings::str_split("a,b,c", Some(",")),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    assert_eq!(
        incan_stdlib::strings::str_join("-", &["a".to_string(), "b".to_string(), "c".to_string()]),
        "a-b-c"
    );
    assert!(incan_stdlib::strings::str_starts_with("hello", "he"));
    assert!(incan_stdlib::strings::str_ends_with("hello", "lo"));
    Ok(())
}

#[test]
#[should_panic(expected = "IndexError: string index out of range")]
fn runtime_index_panics_on_oob() {
    let _ = rt_str_index("abc", 99);
}

#[test]
#[should_panic(expected = "ValueError: slice step cannot be zero")]
fn runtime_slice_panics_on_zero_step() {
    let _ = rt_str_slice("abc", None, None, Some(0));
}

#[test]
#[should_panic(expected = "IndexError: string index out of range")]
fn runtime_byte_range_panics_on_mid_codepoint() {
    let _ = rt_str_slice_byte_range("é", 0, 1);
}

#[test]
#[should_panic(expected = "IndexError: string index out of range")]
fn runtime_byte_suffix_panics_on_mid_codepoint() {
    let _ = rt_str_slice_from_byte_offset("é", 1);
}

#[test]
fn const_eval_computes_string_values() -> Result<(), String> {
    let src = r#"
pub const S: str = "héllo"
pub const A: str = S[1]
pub const B: str = S[-2:]
pub const C: str = S[0:5:2]
pub const D: bool = "é" in S
pub const E: bool = "z" not in S
pub const F: str = "foo" + "bar"
"#;
    let info = run_const_eval_with_info(src).map_err(|errs| errs.join("; "))?;

    let s = "héllo";
    let expected_a = str_char_at(s, 1).map_err(|e| e.to_string())?;
    let expected_b = str_slice(s, Some(-2), None, None).map_err(|e| e.to_string())?;
    let expected_c = str_slice(s, Some(0), Some(5), Some(2)).map_err(|e| e.to_string())?;
    let expected_d = str_contains(s, "é");
    let expected_e = !str_contains(s, "z");
    let expected_f = str_concat("foo", "bar");

    assert_eq!(info.const_value("A"), Some(&ConstValue::FrozenStr(expected_a.clone())));
    assert_eq!(info.const_value("B"), Some(&ConstValue::FrozenStr(expected_b)));
    assert_eq!(info.const_value("C"), Some(&ConstValue::FrozenStr(expected_c)));
    assert_eq!(info.const_value("F"), Some(&ConstValue::FrozenStr(expected_f.clone())));
    match info.const_value("D") {
        Some(ConstValue::Bool(b)) => assert_eq!(*b, expected_d),
        other => panic!("expected bool const value for D, got {:?}", other),
    }
    match info.const_value("E") {
        Some(ConstValue::Bool(b)) => assert_eq!(*b, expected_e),
        other => panic!("expected bool const value for E, got {:?}", other),
    }
    Ok(())
}

#[test]
fn const_eval_reports_string_access_errors() {
    let idx_src = r#"
pub const S: str = "abc"
pub const BAD: str = S[99]
"#;
    let errs = match run_const_eval_with_info(idx_src) {
        Err(e) => e,
        Ok(_) => panic!("expected const-eval to fail for out-of-bounds index"),
    };
    let expected = IncanError::string_index_out_of_range().to_string();
    assert!(
        errs.iter().any(|e| e.contains(&expected)),
        "expected index error, got {errs:?}"
    );

    let step_src = r#"
pub const S: str = "abc"
pub const BAD: str = S[0:3:0]
"#;
    let errs = match run_const_eval_with_info(step_src) {
        Err(e) => e,
        Ok(_) => panic!("expected const-eval to fail for zero slice step"),
    };
    let expected = IncanError::slice_step_zero().to_string();
    assert!(
        errs.iter().any(|e| e.contains(&expected)),
        "expected slice step error, got {errs:?}"
    );
}

#[test]
fn fstring_shared_helper() {
    let parts = ["Hello ", "!"];
    let args = vec![format!("{}", "world")];
    assert_eq!(
        incan_core::strings::fstring(&parts, &args),
        incan_stdlib::strings::fstring(&parts, &args)
    );

    let parts2 = ["{", "}", ""];
    let args2 = vec![format!("{}", 42), format!("{}", 7)];
    assert_eq!(incan_stdlib::strings::fstring(&parts2, &args2), "{42}7");
}
