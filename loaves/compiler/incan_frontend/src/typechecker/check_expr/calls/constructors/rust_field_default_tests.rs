//! A Rust record's published field defaults let an Incan caller leave out exactly those fields.

use super::TypeChecker;
use crate::ast::{CallArg, Declaration, Expr, Program, Span, Statement};
use crate::symbols::ResolvedType;
use incan_lang::interop::{
    RustFieldInfo, RustFunctionSig, RustImplementedTrait, RustMethodSig, RustTypeInfo, RustTypeShape,
};

const OWNER: &str = "demo::Request";

/// Parse one consumer statement and return its constructor call's arguments and span.
fn constructor_call(source: &str) -> Result<(Program, Span), Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let span = match &program.declarations.first().ok_or("missing consumer function")?.node {
        Declaration::Function(function) => match &function.body.first().ok_or("missing constructor")?.node {
            Statement::Assignment(assignment) => assignment.value.span,
            _ => return Err("expected a constructor assignment".into()),
        },
        _ => return Err("expected a consumer function".into()),
    };
    Ok((program, span))
}

/// The written arguments of the parsed constructor call.
fn arguments(program: &Program) -> Result<&[CallArg], Box<dyn std::error::Error>> {
    let Declaration::Function(function) = &program.declarations.first().ok_or("missing consumer function")?.node else {
        return Err("expected a consumer function".into());
    };
    let Statement::Assignment(assignment) = &function.body.first().ok_or("missing constructor")?.node else {
        return Err("expected a constructor assignment".into());
    };
    let Expr::Call(_, _, arguments) = &assignment.value.node else {
        return Err("expected a constructor call".into());
    };
    Ok(arguments)
}

fn field(name: &str, display: &str, shape: RustTypeShape) -> RustFieldInfo {
    RustFieldInfo {
        name: name.into(),
        type_display: display.into(),
        type_shape: shape,
    }
}

/// An associated function with no receiver, no arguments and no type parameters.
fn associated(name: &str, returns: &str) -> RustMethodSig {
    RustMethodSig {
        name: name.into(),
        signature: RustFunctionSig {
            receiver_contract: None,
            type_params: Vec::new(),
            bounds: Vec::new(),
            params: Vec::new(),
            return_type: returns.into(),
            is_async: false,
            is_unsafe: false,
        },
        impl_type_params: Vec::new(),
        impl_self_type: Some(OWNER.into()),
    }
}

/// `name` is required; `limit` and `retries` publish their defaults; `label` publishes one of the wrong type.
fn request(methods: Vec<RustMethodSig>, implemented_traits: Vec<RustImplementedTrait>) -> RustTypeInfo {
    RustTypeInfo {
        fields: vec![
            field("name", "i64", RustTypeShape::Int),
            field("limit", "i64", RustTypeShape::Int),
            field("retries", "i64", RustTypeShape::Int),
            field("label", "bool", RustTypeShape::Bool),
        ],
        methods,
        implemented_traits,
        ..RustTypeInfo::default()
    }
}

fn published() -> Vec<RustMethodSig> {
    vec![
        associated("__incan_default_limit", "i64"),
        associated("__incan_default_retries", "i64"),
        associated("__incan_default_label", "i64"),
        associated("default_name", "i64"),
    ]
}

fn check(source: &str, info: &RustTypeInfo) -> Result<(TypeChecker, Span), Box<dyn std::error::Error>> {
    let (program, span) = constructor_call(source)?;
    let mut checker = TypeChecker::new();
    let result = checker.check_rust_named_field_constructor_call(OWNER, info, arguments(&program)?, span);
    if checker.errors.is_empty() {
        assert_eq!(result, ResolvedType::RustPath(OWNER.into()));
    }
    Ok((checker, span))
}

/// Omitted fields with a published default keep the construction, with those slots recorded as defaulted; the
/// written arguments keep their written order.
#[test]
fn rust_construction_omitting_published_defaults_retains_defaulted_slots() -> Result<(), Box<dyn std::error::Error>> {
    let info = request(published(), Vec::new());
    let (checker, span) = check("def main() -> None:\n    value = Request(label=true, name=7)\n", &info)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let binding = checker
        .type_info
        .body_ir_constructor_field_binding(span)
        .ok_or("missing retained native construction")?;
    assert_eq!(binding.argument_slots, vec![3, 0]);
    assert_eq!(binding.defaulted_slots, vec![1, 2]);
    assert_eq!(binding.field_count, 4);
    assert!(checker.type_info.body_ir_native_constructor_identity(span).is_some());
    Ok(())
}

/// A field without a published default stays required, even when the record also implements `Default`, and a
/// published function whose result is not the field's type publishes nothing.
#[test]
fn rust_construction_refuses_fields_without_a_published_default() -> Result<(), Box<dyn std::error::Error>> {
    let default_trait = RustImplementedTrait {
        path: "core::default::Default".into(),
        mutable_reference: false,
        self_type: OWNER.into(),
        type_params: Vec::new(),
        type_arguments: Vec::new(),
        bounds: Vec::new(),
        associated_type_bindings: Vec::new(),
    };
    let info = request(published(), vec![default_trait]);
    for source in [
        "def main() -> None:\n    value = Request(label=true)\n",
        "def main() -> None:\n    value = Request(name=7)\n",
    ] {
        let (checker, span) = check(source, &info)?;
        assert_eq!(checker.errors.len(), 1, "{source}: {:?}", checker.errors);
        assert!(checker.type_info.body_ir_constructor_field_binding(span).is_none());
    }
    Ok(())
}
