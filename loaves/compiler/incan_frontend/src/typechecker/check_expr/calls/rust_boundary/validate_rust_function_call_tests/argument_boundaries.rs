//! Argument boundary matching and the parameter shapes recorded for Rust method and function calls.

use super::*;

#[test]
fn rust_arg_boundary_accepts_structural_list_to_vec() {
    let checker = TypeChecker::new();
    let arg_ty = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Str]);
    assert!(checker.rust_arg_matches_boundary(&arg_ty, "Vec<String>"));
}

#[test]
fn rust_arg_boundary_accepts_structural_option_int_to_option_i64() {
    let checker = TypeChecker::new();
    let arg_ty = ResolvedType::Generic("Option".to_string(), vec![ResolvedType::Int]);
    assert!(checker.rust_arg_matches_boundary(&arg_ty, "Option<i64>"));
}

#[test]
fn rust_arg_boundary_rejects_structural_float_to_f32() {
    let checker = TypeChecker::new();
    let arg_ty = ResolvedType::FrozenDict(Box::new(ResolvedType::Str), Box::new(ResolvedType::Float));
    assert!(!checker.rust_arg_matches_boundary(&arg_ty, "std::collections::HashMap<&str, f32>"));
}

#[test]
fn rust_arg_boundary_accepts_lossless_exact_width_numeric_widening() {
    let checker = TypeChecker::new();
    let arg_ty = ResolvedType::Numeric(NumericTypeId::I16);
    assert!(checker.rust_arg_matches_boundary(&arg_ty, "i64"));
}

#[test]
fn rust_arg_boundary_accepts_pointer_sized_numeric_identity() {
    let checker = TypeChecker::new();
    assert!(checker.rust_arg_matches_boundary(&ResolvedType::Numeric(NumericTypeId::ISize), "isize"));
    assert!(checker.rust_arg_matches_boundary(&ResolvedType::Numeric(NumericTypeId::USize), "usize"));
}

#[test]
fn rust_arg_boundary_rejects_exact_width_numeric_narrowing() {
    let checker = TypeChecker::new();
    let arg_ty = ResolvedType::Numeric(NumericTypeId::I16);
    assert!(!checker.rust_arg_matches_boundary(&arg_ty, "i8"));
}

#[test]
fn rust_arg_boundary_rejects_structural_list_str_to_vec_i64() {
    let checker = TypeChecker::new();
    let arg_ty = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Str]);
    assert!(!checker.rust_arg_matches_boundary(&arg_ty, "Vec<i64>"));
}

/// Fixed-array relation binds type and const parameters only from exact concrete lengths, including nested arrays.
#[test]
fn rust_array_inference_retains_only_exact_literal_lengths() {
    let mut checker = TypeChecker::new();
    let type_parameters = vec!["T".to_string()];
    let const_parameters = vec!["N".to_string(), "M".to_string()];
    let mut bindings = HashMap::new();

    checker.infer_rust_array_display_bindings(
        "[[T; M]; N]",
        "rust::[[u8; 8]; 4]",
        "",
        &type_parameters,
        &const_parameters,
        &mut bindings,
    );
    assert_eq!(bindings.get("T"), Some(&ResolvedType::Numeric(NumericTypeId::U8)));
    assert_eq!(bindings.get("M"), Some(&ResolvedType::RustPath("8".to_string())));
    assert_eq!(bindings.get("N"), Some(&ResolvedType::RustPath("4".to_string())));

    checker.infer_rust_array_display_bindings("[u8; N]", "[u8; 32]", "", &[], &const_parameters, &mut bindings);
    assert_eq!(bindings.get("N"), Some(&ResolvedType::Unknown));

    let mut unconstrained = HashMap::new();
    checker.infer_rust_array_display_bindings("[u8; N]", "[u8; M]", "", &[], &const_parameters, &mut unconstrained);
    assert!(!unconstrained.contains_key("N"));
    assert!(TypeChecker::rust_call_arg_wants_owner_expectation(
        &ResolvedType::RustPath("[u8;N]".to_string()),
        &ResolvedType::RustPath("[u8;32]".to_string())
    ));

    let signature = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: vec!["T".to_string()],
        params: Vec::new(),
        return_type: "[T; N]".to_string(),
        is_async: false,
        is_unsafe: false,
    };
    let parameters = TypeChecker::rust_signature_inference_parameters(&signature);
    let inferred = checker.infer_rust_signature_bindings(
        &signature,
        "std::array::from_fn",
        &parameters,
        &[],
        &[],
        Some(&ResolvedType::RustPath("[u8; 32]".to_string())),
    );
    assert_eq!(inferred.get("T"), Some(&ResolvedType::Numeric(NumericTypeId::U8)));
    assert_eq!(inferred.get("N"), Some(&ResolvedType::RustPath("32".to_string())));

    let nested = checker.infer_rust_signature_bindings(
        &signature,
        "std::array::from_fn",
        &parameters,
        &[],
        &[],
        Some(&ResolvedType::RustPath("[[u8; 8]; 4]".to_string())),
    );
    assert_eq!(nested.get("T"), Some(&ResolvedType::RustPath("[u8;8]".to_string())));
    assert_eq!(nested.get("N"), Some(&ResolvedType::RustPath("4".to_string())));

    let callback_signature = RustFunctionSig {
        receiver_contract: None,
        bounds: vec![RustTraitBound {
            self_type: "F".to_string(),
            path: "std::ops::FnMut".to_string(),
            type_arguments: vec!["(usize,)".to_string()],
            associated_type_bindings: vec![RustAssociatedTypeBinding {
                name: "Output".to_string(),
                value_path: "T".to_string(),
            }],
        }],
        type_params: vec!["F".to_string(), "T".to_string()],
        params: vec![RustParam {
            name: Some("callback".to_string()),
            type_display: "F".to_string(),
        }],
        return_type: "[T; N]".to_string(),
        is_async: false,
        is_unsafe: false,
    };
    let callback_parameters = TypeChecker::rust_signature_inference_parameters(&callback_signature);
    let callback_args = [CallArg::Positional(Spanned::new(
        Expr::Ident("callback".to_string()),
        Span::new(4, 5),
    ))];
    let provisional_callback = ResolvedType::Function(
        vec![CallableParam::positional(ResolvedType::Numeric(NumericTypeId::USize))],
        Box::new(ResolvedType::RustPath("[T; N]".to_string())),
    );
    let callback_inferred = checker.infer_rust_signature_bindings(
        &callback_signature,
        "std::array::from_fn",
        &callback_parameters,
        &callback_args,
        &[provisional_callback],
        Some(&ResolvedType::RustPath("[[u8; 8]; 4]".to_string())),
    );
    assert_eq!(
        callback_inferred.get("T"),
        Some(&ResolvedType::RustPath("[u8;8]".to_string()))
    );
    assert_eq!(
        callback_inferred.get("N"),
        Some(&ResolvedType::RustPath("4".to_string()))
    );
    let retained_span = Span::new(2, 3);
    checker.retain_checked_rust_function("std::array::from_fn", &signature, &inferred, retained_span);
    assert_eq!(
        checker
            .type_info
            .rust
            .checked_calls
            .get(&retained_span)
            .map(|call| &call.return_type),
        Some(&ResolvedType::RustPath("[u8;32]".to_string()))
    );

    let mut provisional = TypeChecker::new();
    provisional.call_argument_depth = 1;
    provisional.record_inferred_rust_arguments("std::array::from_fn", &signature, &HashMap::new(), Span::new(0, 1));
    assert!(provisional.errors.is_empty());
    assert!(!TypeChecker::rust_signature_const_parameters_are_bound(
        &signature,
        &HashMap::new()
    ));

    provisional.call_argument_depth = 0;
    provisional.record_inferred_rust_arguments("std::array::from_fn", &signature, &HashMap::new(), Span::new(0, 1));
    assert!(
        provisional
            .errors
            .iter()
            .any(|error| error.message.contains("const parameter `N` is not fixed"))
    );
}

/// A Rust receiver does not satisfy a missing required value parameter.
#[test]
fn rust_method_call_rejects_missing_required_arguments_after_receiver() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, 1);
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![
            RustParam {
                name: Some("self".to_string()),
                type_display: "&Self".to_string(),
            },
            RustParam {
                name: Some("pattern".to_string()),
                type_display: "&str".to_string(),
            },
        ],
        return_type: "bool".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_method_call("rust::regex::Regex.is_match", &sig, &[], &[], false, span);

    assert!(
        checker
            .errors
            .iter()
            .any(|e| e.message.contains("rust::regex::Regex.is_match()") && e.message.contains("expects 1 argument")),
        "expected arity diagnostic for missing method arg, errors={:?}",
        checker.errors
    );
}

/// A method's first authored argument is checked after the receiver and retains structured mismatch facts.
#[test]
fn rust_method_call_rejects_type_mismatch_after_receiver() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, 1);
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![
            RustParam {
                name: Some("self".to_string()),
                type_display: "&Self".to_string(),
            },
            RustParam {
                name: Some("pattern".to_string()),
                type_display: "&str".to_string(),
            },
        ],
        return_type: "bool".to_string(),
        is_async: false,
        is_unsafe: false,
    };
    let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(123))), span);
    let args = [CallArg::Positional(arg_expr)];
    let arg_types = [ResolvedType::Int];

    let _ = checker.validate_rust_method_call("rust::regex::Regex.is_match", &sig, &args, &arg_types, false, span);

    assert!(
        checker.errors.iter().any(|error| {
            error.span == span
                && error.expected() == Some("&str")
                && error.actual() == Some(numerics::descriptor(NumericTypeId::I64))
        }),
        "expected type mismatch diagnostic for method arg, errors={:?}",
        checker.errors
    );
}

/// A lookup-preserving method accepts a string probe for an inspected String reference without coercing it.
#[test]
fn rust_lookup_preserving_method_accepts_string_probe_for_ref_string_param() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, 1);
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![
            RustParam {
                name: Some("self".to_string()),
                type_display: "&Self".to_string(),
            },
            RustParam {
                name: Some("key".to_string()),
                type_display: "&String".to_string(),
            },
        ],
        return_type: "Option<&i64>".to_string(),
        is_async: false,
        is_unsafe: false,
    };
    let arg_expr = Spanned::new(Expr::Literal(Literal::String("the".to_string())), span);
    let args = [CallArg::Positional(arg_expr)];
    let arg_types = [ResolvedType::Str];

    let _ = checker.validate_rust_method_call(
        "rust::std::collections::HashMap.get",
        &sig,
        &args,
        &arg_types,
        true,
        span,
    );

    assert!(
        checker.errors.is_empty(),
        "expected lookup-preserving rust method call to stay permissive for string probes, errors={:?}",
        checker.errors
    );
    assert!(
        checker.type_info.rust.arg_coercions.is_empty(),
        "expected lookup-preserving rust method call to preserve arg shape without coercion, got {:?}",
        checker.type_info.rust.arg_coercions
    );
}

/// A lookup-preserving generic reference parameter keeps a string probe's original argument shape, and native
/// retention preserves validated written method arguments independently of the receiver's impl binders.
#[test]
fn rust_lookup_preserving_method_accepts_string_probe_for_generic_ref_param() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, 1);
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![
            RustParam {
                name: Some("self".to_string()),
                type_display: "&Self".to_string(),
            },
            RustParam {
                name: Some("key".to_string()),
                type_display: "&Q".to_string(),
            },
        ],
        return_type: "Option<&i64>".to_string(),
        is_async: false,
        is_unsafe: false,
    };
    let arg_expr = Spanned::new(Expr::Ident("word".to_string()), span);
    let args = [CallArg::Positional(arg_expr)];
    let arg_types = [ResolvedType::Str];

    let _ = checker.validate_rust_method_call(
        "rust::std::collections::HashMap.get",
        &sig,
        &args,
        &arg_types,
        true,
        span,
    );

    assert!(
        checker.errors.is_empty(),
        "expected lookup-preserving generic probe to stay permissive for string probes, errors={:?}",
        checker.errors
    );
    assert!(
        checker.type_info.rust.arg_coercions.is_empty(),
        "expected lookup-preserving generic probe to preserve arg shape without coercion, got {:?}",
        checker.type_info.rust.arg_coercions
    );
    TypeChecker::verify_written_native_method_arguments();
}

#[test]
fn rust_arg_boundary_accepts_rusttype_from_interop_target() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, 1);
    checker.symbols.define(Symbol {
        name: "Email".to_string(),
        kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
            type_params: Vec::new(),
            is_rusttype: true,
            has_interop: true,
            underlying: ResolvedType::Named("RustString".to_string()),
            constraints: Vec::new(),
            implicit_coercion_enabled: true,
            method_rebindings: HashMap::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            method_aliases: HashMap::new(),
            methods: HashMap::new(),
            method_overloads: HashMap::new(),
        })),
        span,
        scope: 0,
    });

    assert!(
        checker.rust_arg_matches_boundary(&ResolvedType::Str, "Email"),
        "expected `str` to be admitted for rusttype target boundary via `from` interop edge hint"
    );
}

/// A string value is accepted at a Rust function's borrowed String boundary.
#[test]
fn rust_function_call_accepts_string_for_borrowed_string_param() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 20);
    let arg_expr = Spanned::new(Expr::Literal(Literal::String("{}".to_string())), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("value".to_string()),
            type_display: "&String".to_string(),
        }],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::demo::takes_borrowed_string", &sig, &args, span);

    assert!(
        checker.errors.is_empty(),
        "expected borrowed String boundary to admit Incan str, errors={:?}",
        checker.errors
    );
    assert!(
        checker.type_info.rust.arg_coercions.contains_key(&span),
        "expected rust arg coercion metadata for borrowed String boundary"
    );
    let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath("String".to_string())));
    assert_eq!(
        checker
            .type_info
            .rust
            .arg_coercions
            .get(&span)
            .map(|coercion| &coercion.target_type),
        Some(&expected),
        "borrowed owned Rust params must preserve owned target shape in lowering metadata"
    );
}

/// A string value is accepted at a Rust function's borrowed str boundary.
#[test]
fn rust_function_call_accepts_string_for_borrowed_str_param() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 20);
    let arg_expr = Spanned::new(Expr::Literal(Literal::String("{}".to_string())), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("value".to_string()),
            type_display: "&str".to_string(),
        }],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::demo::takes_borrowed_str", &sig, &args, span);

    assert!(
        checker.errors.is_empty(),
        "expected borrowed str boundary to admit Incan str, errors={:?}",
        checker.errors
    );
    let expected = ResolvedType::Ref(Box::new(ResolvedType::Str));
    assert_eq!(
        checker
            .type_info
            .rust
            .arg_coercions
            .get(&span)
            .map(|coercion| &coercion.target_type),
        Some(&expected),
        "borrowed str params must stay distinct from borrowed owned String params"
    );
}

/// Bytes satisfy an inspected borrowed byte-vector parameter.
#[test]
fn rust_function_call_accepts_bytes_for_borrowed_vec_param() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 20);
    let arg_expr = Spanned::new(Expr::Literal(Literal::Bytes(b"abc".to_vec())), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("value".to_string()),
            type_display: "&Vec<u8>".to_string(),
        }],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::demo::takes_borrowed_vec", &sig, &args, span);

    assert!(
        checker.errors.is_empty(),
        "expected borrowed Vec<u8> boundary to admit Incan bytes, errors={:?}",
        checker.errors
    );
    let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath("Vec<u8>".to_string())));
    assert_eq!(
        checker
            .type_info
            .rust
            .arg_coercions
            .get(&span)
            .map(|coercion| &coercion.target_type),
        Some(&expected),
        "borrowed owned Rust byte-vector params must preserve owned target shape in lowering metadata"
    );
}

/// A concrete borrowed Rust path keeps its nominal type instead of becoming an unknown-reference diagnostic.
#[test]
fn rust_function_call_accepts_concrete_borrowed_rust_path_param_without_ref_unknown_diagnostic() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 20);
    checker.symbols.define(Symbol {
        name: "plan".to_string(),
        kind: SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::RustPath("demo::Plan".to_string()),
            is_mutable: false,
            is_used: false,
        }),
        span,
        scope: 0,
    });

    let arg_expr = Spanned::new(Expr::Ident("plan".to_string()), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("value".to_string()),
            type_display: "&demo::Plan".to_string(),
        }],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::demo::consume_plan", &sig, &args, span);

    assert!(
        checker.errors.is_empty(),
        "expected concrete borrowed Rust path boundary to typecheck, errors={:?}",
        checker.errors
    );
}

/// An accepted rusttype function argument records the interop conversion required by lowering.
#[test]
fn validate_rust_function_call_records_interop_coercion_for_rusttype_target() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 20);
    checker.symbols.define(Symbol {
        name: "Email".to_string(),
        kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
            type_params: Vec::new(),
            is_rusttype: true,
            has_interop: true,
            underlying: ResolvedType::Named("RustString".to_string()),
            constraints: Vec::new(),
            implicit_coercion_enabled: true,
            method_rebindings: HashMap::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            method_aliases: HashMap::new(),
            methods: HashMap::new(),
            method_overloads: HashMap::new(),
        })),
        span,
        scope: 0,
    });

    let arg_expr = Spanned::new(Expr::Literal(Literal::String("alice@example.com".to_string())), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("value".to_string()),
            type_display: "Email".to_string(),
        }],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::demo::takes_email", &sig, &args, span);

    assert!(
        checker.errors.is_empty(),
        "expected rusttype interop boundary to avoid type mismatch, errors={:?}",
        checker.errors
    );
    assert!(
        checker.type_info.rust.arg_coercions.contains_key(&span),
        "expected rust arg coercion metadata for rusttype target boundary"
    );
}

#[test]
fn borrowed_generic_rust_function_param_accepts_owned_incan_value() {
    let checker = TypeChecker::new();

    assert!(checker.rust_arg_matches_boundary(&ResolvedType::Named("Payload".to_string()), "T",));
    assert!(checker.rust_arg_matches_boundary(&ResolvedType::Bytes, "impl Buf"));
    assert!(checker.rust_arg_matches_boundary(&ResolvedType::Bytes, "implBuf"));
    assert!(checker.rust_arg_matches_boundary(&ResolvedType::Named("Payload".to_string()), "&T",));
    assert!(checker.rust_arg_matches_boundary(&ResolvedType::Named("Payload".to_string()), "&TValue",));
}

/// A by-value generic Rust method parameter retains its checked concrete argument shape for lowering.
#[test]
fn validate_rust_method_call_records_by_value_generic_param_shape() {
    let mut checker = TypeChecker::new();
    let span = Span::new(30, 40);
    let arg_expr = Spanned::new(Expr::Ident("cursor".to_string()), span);
    let args = [CallArg::Positional(arg_expr)];
    let arg_types = [ResolvedType::RustPath("std::io::Cursor<Vec<u8>>".to_string())];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("buf".to_string()),
            type_display: "T".to_string(),
        }],
        return_type: "demo::FileDescriptorSet".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_method_call(
        "rust::demo::FileDescriptorSet.decode",
        &sig,
        &args,
        &arg_types,
        false,
        span,
    );

    assert!(
        checker.errors.is_empty(),
        "expected by-value Rust generic param to accept the owned argument, got {:?}",
        checker.errors
    );
    assert!(
        checker.type_info.rust.arg_coercions.is_empty(),
        "an owned generic argument must not acquire a borrow coercion: {:?}",
        checker.type_info.rust.arg_coercions
    );
    assert!(
        checker
            .type_info
            .calls
            .call_site_callable_params
            .get(&span)
            .is_some_and(|params| params.len() == 1 && params[0].ty == arg_types[0]),
        "expected the concrete owned Rust generic method parameter, got {:?}",
        checker.type_info.calls.call_site_callable_params
    );
}

/// An inspected owned String method parameter remains by value in the recorded callable signature.
#[test]
fn validate_rust_method_call_records_owned_string_param_shape() {
    let mut checker = TypeChecker::new();
    let span = Span::new(30, 40);
    let arg_expr = Spanned::new(Expr::Ident("value".to_string()), span);
    let args = [CallArg::Positional(arg_expr)];
    let arg_types = [ResolvedType::Str];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("value".to_string()),
            type_display: "String".to_string(),
        }],
        return_type: "demo::Value".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_method_call("rust::demo::Value.string", &sig, &args, &arg_types, false, span);

    assert!(
        checker.errors.is_empty(),
        "unexpected Rust boundary errors: {:?}",
        checker.errors
    );
    assert!(
        checker
            .type_info
            .calls
            .call_site_callable_params
            .get(&span)
            .is_some_and(|params| params.len() == 1 && params[0].ty == ResolvedType::Str),
        "an inspected by-value String parameter must reach lowering exactly"
    );
}

/// A by-value impl Trait method parameter retains its inferred byte-vector storage without a borrow coercion.
#[test]
fn validate_rust_method_call_records_by_value_impl_trait_param_shape() {
    let mut checker = TypeChecker::new();
    let span = Span::new(30, 40);
    let arg_expr = Spanned::new(Expr::Ident("encoded".to_string()), span);
    let args = [CallArg::Positional(arg_expr)];
    let arg_types = [ResolvedType::Bytes];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("buf".to_string()),
            type_display: "implBuf".to_string(),
        }],
        return_type: "demo::FileDescriptorSet".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_method_call(
        "rust::demo::FileDescriptorSet.decode",
        &sig,
        &args,
        &arg_types,
        false,
        span,
    );

    assert!(
        checker.errors.is_empty(),
        "expected by-value impl Trait Rust param to accept bytes without borrow coercion, got {:?}",
        checker.errors
    );
    assert!(
        checker.type_info.rust.arg_coercions.is_empty(),
        "expected by-value impl Trait Rust param to avoid borrow coercion, got {:?}",
        checker.type_info.rust.arg_coercions
    );
    assert!(
        checker
            .type_info
            .calls
            .call_site_callable_params
            .get(&span)
            .is_some_and(|params| params.len() == 1
                && params[0].ty
                    == ResolvedType::Generic(
                        collections::as_str(CollectionTypeId::List).to_owned(),
                        vec![ResolvedType::Numeric(NumericTypeId::U8)],
                    )),
        "expected the concrete owned byte-vector method parameter, got {:?}",
        checker.type_info.calls.call_site_callable_params
    );
}

/// A borrowed-slice callback preserves the exact inspected Rust parameter displays for lowering.
#[test]
fn validate_rust_method_call_records_borrowed_slice_callback_displays_issue835() {
    let mut checker = TypeChecker::new();
    let span = Span::new(30, 40);
    let callback_span = Span::new(12, 25);
    let callback_expr = Spanned::new(Expr::Ident("write_silence".to_string()), callback_span);
    let args = [CallArg::Positional(callback_expr)];
    let callback_ty = ResolvedType::Function(
        vec![
            CallableParam::positional(ResolvedType::RefMut(Box::new(ResolvedType::Generic(
                "List".to_string(),
                vec![ResolvedType::Numeric(NumericTypeId::F32)],
            )))),
            CallableParam::positional(ResolvedType::Ref(Box::new(ResolvedType::RustPath(
                "demo::OutputCallbackInfo".to_string(),
            )))),
        ],
        Box::new(ResolvedType::Unit),
    );
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![
            RustParam {
                name: Some("self".to_string()),
                type_display: "&self".to_string(),
            },
            RustParam {
                name: Some("callback".to_string()),
                type_display: "impl FnMut(&mut [f32], &demo::OutputCallbackInfo)".to_string(),
            },
        ],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_method_call("rust::demo::Device.run", &sig, &args, &[callback_ty], false, span);

    assert!(
        checker.errors.is_empty(),
        "expected the borrowed-slice callback to satisfy the Rust method bound, got {:?}",
        checker.errors
    );
    assert_eq!(
        checker.type_info.rust.function_param_type_displays.get("write_silence"),
        Some(&vec!["&mut [f32]".to_string(), "&demo::OutputCallbackInfo".to_string(),]),
        "expected method validation to preserve exact callback parameter displays"
    );
}

/// An accepted rusttype method argument records the same interop conversion as a function argument.
#[test]
fn validate_rust_method_call_records_interop_coercion_for_rusttype_target() {
    let mut checker = TypeChecker::new();
    let span = Span::new(30, 40);
    checker.symbols.define(Symbol {
        name: "Email".to_string(),
        kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
            type_params: Vec::new(),
            is_rusttype: true,
            has_interop: true,
            underlying: ResolvedType::Named("RustString".to_string()),
            constraints: Vec::new(),
            implicit_coercion_enabled: true,
            method_rebindings: HashMap::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            method_aliases: HashMap::new(),
            methods: HashMap::new(),
            method_overloads: HashMap::new(),
        })),
        span,
        scope: 0,
    });

    let arg_expr = Spanned::new(Expr::Literal(Literal::String("alice@example.com".to_string())), span);
    let args = [CallArg::Positional(arg_expr)];
    let arg_types = [ResolvedType::Str];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![
            RustParam {
                name: Some("self".to_string()),
                type_display: "&Self".to_string(),
            },
            RustParam {
                name: Some("value".to_string()),
                type_display: "Email".to_string(),
            },
        ],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_method_call(
        "rust::demo::EmailService.set_email",
        &sig,
        &args,
        &arg_types,
        false,
        span,
    );

    assert!(
        checker.errors.is_empty(),
        "expected rusttype interop boundary to avoid type mismatch, errors={:?}",
        checker.errors
    );
    assert!(
        checker.type_info.rust.arg_coercions.contains_key(&span),
        "expected rust arg coercion metadata for rust method boundary"
    );
}
