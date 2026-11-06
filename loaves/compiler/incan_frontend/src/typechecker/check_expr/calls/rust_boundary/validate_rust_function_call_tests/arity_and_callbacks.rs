//! Rust call arity, callable bounds, and keyword arguments.

use super::*;

/// A parameterless Rust function refuses an extra argument with an arity diagnostic.
#[test]
fn zero_parameter_rust_sig_rejects_extra_arguments() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, 1);
    let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(1))), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::crate::nop", &sig, &args, span);

    assert!(
        checker.errors.iter().any(|e| {
            e.message.contains("rust::crate::nop()")
                && e.message.contains("expects 0 argument")
                && e.message.contains("got 1")
        }),
        "expected builtin_arity for 0-param Rust call with 1 arg, errors={:?}",
        checker.errors
    );
}

/// A callback taking owned values cannot satisfy a Rust FnMut bound requiring borrowed parameters.
#[test]
fn rust_callable_bound_rejects_by_value_function_for_borrowed_fnmut() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 22);
    let data = ResolvedType::RustPath("demo::Data".to_string());
    let info = ResolvedType::RustPath("demo::OutputCallbackInfo".to_string());
    checker.symbols.define(Symbol {
        name: "write_output".to_string(),
        kind: SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Function(
                vec![CallableParam::positional(data), CallableParam::positional(info)],
                Box::new(ResolvedType::Unit),
            ),
            is_mutable: false,
            is_used: false,
        }),
        span,
        scope: 0,
    });
    let arg_expr = Spanned::new(Expr::Ident("write_output".to_string()), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("callback".to_string()),
            type_display: "impl FnMut(&mut demo::Data, &demo::OutputCallbackInfo)".to_string(),
        }],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::demo::use_callback", &sig, &args, span);

    assert!(
        checker.errors.iter().any(|error| {
            error.message.contains("FnMut")
                || (error.message.contains("&mut demo::Data") && error.message.contains("demo::OutputCallbackInfo"))
        }),
        "expected a borrowed callback diagnostic before codegen, got {:?}",
        checker.errors
    );
}

/// A callback with exactly the inspected borrowed parameters satisfies its Rust FnMut bound.
#[test]
fn rust_callable_bound_accepts_exact_borrowed_fnmut_function() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 22);
    let data = ResolvedType::RustPath("demo::Data".to_string());
    let info = ResolvedType::RustPath("demo::OutputCallbackInfo".to_string());
    checker.symbols.define(Symbol {
        name: "write_output".to_string(),
        kind: SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Function(
                vec![
                    CallableParam::positional(ResolvedType::RefMut(Box::new(data))),
                    CallableParam::positional(ResolvedType::Ref(Box::new(info))),
                ],
                Box::new(ResolvedType::Unit),
            ),
            is_mutable: false,
            is_used: false,
        }),
        span,
        scope: 0,
    });
    let arg_expr = Spanned::new(Expr::Ident("write_output".to_string()), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("callback".to_string()),
            type_display: "impl FnMut(&mut demo::Data, &demo::OutputCallbackInfo)".to_string(),
        }],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::demo::use_callback", &sig, &args, span);

    assert!(
        checker.errors.is_empty(),
        "exact borrowed Rust callback should typecheck, got {:?}",
        checker.errors
    );
    verify_structured_generic_callback_binding();
}

/// Keep nested source identities through generic callback substitution and named argument ordering, while
/// rejecting a callback whose input loses the native predicate's exclusive borrow.
fn verify_structured_generic_callback_binding() {
    let mut checker = TypeChecker::new();
    let span = Span::new(100, 180);
    let native_map = ResolvedType::Generic(
        "std::collections::HashMap".into(),
        vec![ResolvedType::Str, ResolvedType::Named("SourceEntry".into())],
    );
    let projected_map = ResolvedType::Generic(
        "Dict".into(),
        vec![ResolvedType::Str, ResolvedType::Named("SourceEntry".into())],
    );
    assert_eq!(checker.rust_inference_type(&native_map, &[]), projected_map);
    let callback = ResolvedType::Function(
        vec![CallableParam::positional(ResolvedType::RefMut(Box::new(
            native_map.clone(),
        )))],
        Box::new(ResolvedType::Bool),
    );
    for (name, ty) in [("entries", native_map.clone()), ("predicate", callback)] {
        checker.symbols.define(Symbol {
            name: name.into(),
            kind: SymbolKind::Variable(VariableInfo {
                ty,
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
    }
    let signature = RustFunctionSig {
        receiver_contract: None,
        type_params: vec!["T".into(), "F".into()],
        bounds: vec![RustTraitBound {
            self_type: "F".into(),
            path: "std::ops::FnMut".into(),
            type_arguments: vec!["(&mut T,)".into()],
            associated_type_bindings: vec![RustAssociatedTypeBinding {
                name: "Output".into(),
                value_path: "bool".into(),
            }],
        }],
        params: vec![
            RustParam {
                name: Some("entries".into()),
                type_display: "T".into(),
            },
            RustParam {
                name: Some("predicate".into()),
                type_display: "F".into(),
            },
        ],
        return_type: "bool".into(),
        is_async: false,
        is_unsafe: false,
    };
    let args = [
        CallArg::Named(
            Spanned::new("predicate".into(), span),
            Spanned::new(Expr::Ident("predicate".into()), span),
        ),
        CallArg::Named(
            Spanned::new("entries".into(), span),
            Spanned::new(Expr::Ident("entries".into()), span),
        ),
    ];
    let _ = checker.validate_rust_function_call("rust::demo::visit", &signature, &args, span);
    assert!(
        checker.errors.is_empty(),
        "nested source callback must stay structural: {:?}",
        checker.errors
    );
    let wrong_input = ResolvedType::Function(
        vec![CallableParam::positional(native_map.clone())],
        Box::new(ResolvedType::Bool),
    );
    let bindings = HashMap::from([("T".into(), native_map)]);
    assert!(checker.validate_rust_bound_callable_argument(
        &signature,
        "F",
        &bindings,
        &Spanned::new(Expr::Ident("predicate".into()), span),
        &wrong_input,
    ));
    assert!(
        !checker.errors.is_empty(),
        "an owned callback cannot satisfy an exclusive input"
    );
}

/// A generic callback placeholder remains admissible while Rust supplies its concrete callable implementation.
#[test]
fn rust_callable_bound_accepts_generic_placeholder_callback_arg() {
    let mut checker = TypeChecker::new();
    let span = Span::new(10, 22);
    checker.symbols.enter_scope(ScopeKind::Function);
    checker.symbols.define(Symbol {
        name: "TaskFn".to_string(),
        kind: SymbolKind::Type(TypeInfo::Builtin),
        span,
        scope: 1,
    });
    checker.symbols.define(Symbol {
        name: "task".to_string(),
        kind: SymbolKind::Variable(VariableInfo {
            ty: ResolvedType::Named("TaskFn".to_string()),
            is_mutable: false,
            is_used: false,
        }),
        span,
        scope: 1,
    });
    let arg_expr = Spanned::new(Expr::Ident("task".to_string()), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![RustParam {
            name: Some("task".to_string()),
            type_display: "impl FnOnce() -> T".to_string(),
        }],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::demo::spawn_blocking", &sig, &args, span);

    assert!(
        checker.errors.is_empty(),
        "generic placeholder callbacks should keep satisfying generic Rust callable bounds, got {:?}",
        checker.errors
    );
}

/// A parameterless Rust function accepts an empty argument list.
#[test]
fn zero_parameter_rust_sig_allows_no_arguments() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, 1);
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::crate::nop", &sig, &[], span);

    assert!(
        checker.errors.is_empty(),
        "expected no errors for arity match, got {:?}",
        checker.errors
    );
}

/// Missing Rust arguments produce an arity error before parameter-wise validation can truncate the list.
#[test]
fn too_few_arguments_reports_arity_before_param_zip() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, 1);
    let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(1))), span);
    let args = [CallArg::Positional(arg_expr)];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![
            RustParam {
                name: Some("a".to_string()),
                type_display: "i64".to_string(),
            },
            RustParam {
                name: Some("b".to_string()),
                type_display: "i64".to_string(),
            },
        ],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::crate::f", &sig, &args, span);

    assert!(
        checker
            .errors
            .iter()
            .any(|e| e.message.contains("expects 2 argument") && e.message.contains("got 1")),
        "expected arity error when call has fewer args than Rust params, errors={:?}",
        checker.errors
    );
}

/// Keyword arguments bind by inspected Rust parameter names even when written in another order.
#[test]
fn rust_function_call_binds_keyword_args_by_inspected_param_name() -> Result<(), String> {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, 60);
    let text_span = Span::new(10, 20);
    let count_span = Span::new(30, 31);
    let args = [
        CallArg::Named(
            Spanned::new("text".to_string(), text_span),
            Spanned::new(Expr::Literal(Literal::String("demo".to_string())), text_span),
        ),
        CallArg::Named(
            Spanned::new("count".to_string(), count_span),
            Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(3))), count_span),
        ),
    ];
    let sig = RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: vec![
            RustParam {
                name: Some("count".to_string()),
                type_display: "i64".to_string(),
            },
            RustParam {
                name: Some("text".to_string()),
                type_display: "&str".to_string(),
            },
        ],
        return_type: "()".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    let _ = checker.validate_rust_function_call("rust::crate::f", &sig, &args, span);

    assert!(
        checker.errors.is_empty(),
        "expected keyword Rust call args to bind by parameter name, errors={:?}",
        checker.errors
    );
    let recorded = checker
        .type_info
        .calls
        .call_site_callable_params
        .get(&span)
        .ok_or("keyword Rust calls should record exact call-site params for lowering")?;
    let names: Vec<_> = recorded.iter().filter_map(|param| param.name.as_deref()).collect();
    assert_eq!(names, vec!["count", "text"]);
    Ok(())
}
