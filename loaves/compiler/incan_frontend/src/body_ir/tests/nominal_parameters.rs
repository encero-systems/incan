//! Declaration-bound model placeholders and checked method frames for native specialization.

use super::{IncanPrimitiveType, IncanType, bir, body_named, build, named_targets};
use crate::{lexer, parser, symbols::ResolvedType, typechecker::TypeChecker};

/// An owned Result payload clones a generic field from its model, like an ordinary by-value argument.
#[test]
fn generic_field_result_retains_owned_payload() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Box[T]:\n    pub value: T\n\ndef unwrap[T](boxed: Box[T]) -> Result[T, str]:\n    return Ok(boxed.value)\n",
        &["m", "generic_field_result"],
    )?;
    let body = body_named(&module, "unwrap")?;
    let payload = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::ResultVariant(variant),
                ..
            } => Some(&variant.payload),
            _ => None,
        })
        .ok_or("missing intrinsic Result payload")?;
    assert!(
        matches!(payload, bir::Operand::Place(read) if read.fact == bir::OwnershipFact::Clone),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// Executable constructor/call facts supplement the contextual checker view without rewriting legacy inference.
#[test]
fn executable_call_facts_preserve_legacy_context() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/generic_caller_of_a_model_method_formatting_its_payload.incn"
    );
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".to_string(), "legacy_context".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let facts = checker.type_info();
    assert_eq!(facts.calls.body_ir_call_type_args.len(), 4);
    for (span, arguments) in &facts.calls.body_ir_call_type_args {
        assert!(
            arguments
                .iter()
                .all(|ty| matches!(ty, ResolvedType::Int | ResolvedType::Str))
        );
        let legacy = facts
            .calls
            .call_site_monomorph_type_args
            .get(span)
            .ok_or("missing legacy call binding")?;
        assert!(
            legacy.iter().any(|ty| matches!(ty, ResolvedType::TypeVar(_))),
            "{legacy:?}"
        );
    }
    assert_eq!(facts.calls.body_ir_constructor_types.len(), 4);
    for (span, carrier) in &facts.calls.body_ir_constructor_types {
        assert!(
            matches!(carrier, ResolvedType::Generic(_, arguments) if arguments.iter().all(|ty| matches!(ty, ResolvedType::Int | ResolvedType::Str)))
        );
        let contextual = facts
            .expr_type(crate::ast::Span::new(span.0, span.1))
            .ok_or("missing contextual constructor type")?;
        assert!(
            matches!(contextual, ResolvedType::Generic(_, arguments) if arguments.iter().any(|ty| matches!(ty, ResolvedType::TypeVar(_))))
        );
    }
    Ok(())
}

/// Constructors nested in generic calls keep the concrete checked carrier rather than the callee's open context.
#[test]
fn nested_model_constructors_retain_concrete_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/generic_caller_of_a_model_method_formatting_its_payload.incn"
    );
    let module = build(source, &["m", "nested_model_arguments"])?;
    let main = body_named(&module, "main")?;
    let mut constructors = 0;
    for local in &main.locals {
        if let IncanType::Generic { base, args } = &local.ty
            && matches!(base.as_str(), "Stream" | "Holder")
        {
            constructors += 1;
            assert!(
                args.iter().all(|ty| matches!(ty, IncanType::Primitive(_))),
                "{}",
                main.render_snapshot()
            );
        }
    }
    assert_eq!(constructors, 4, "{}", main.render_snapshot());
    Ok(())
}

/// A generic method call's retained Result carrier uses its checked concrete payload at a concrete caller.
#[test]
fn generic_method_results_retain_concrete_payloads() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/method_type_param_clone_bound.incn"
    );
    let module = build(source, &["m", "generic_method_results"])?;
    let main = body_named(&module, "main")?;
    let mut results = 0;
    for local in &main.locals {
        assert!(!matches!(local.ty, IncanType::TypeVar(_)), "{}", main.render_snapshot());
        if let IncanType::Generic { base, args } = &local.ty
            && base == "Result"
        {
            results += 1;
            assert!(
                args.iter()
                    .all(|ty| matches!(ty, IncanType::Primitive(_) | IncanType::Named(_))),
                "{}",
                main.render_snapshot()
            );
        }
    }
    assert_eq!(results, 6, "{}", main.render_snapshot());
    Ok(())
}

/// Associated methods retain owner parameters even without a receiver local, and resolve Self to the owner.
#[test]
fn retains_classmethod_owner_parameters_and_return_type() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "class Box[T]:\n  pub value: T\n  @classmethod\n  def make(cls, value: T) -> Self:\n    return cls(value=value)\n",
        &["classmethods"],
    )?;
    let body = body_named(&module, "make")?;
    assert_eq!(body.type_parameters, ["T"]);
    assert!(matches!(&body.return_type, IncanType::Generic { base, .. } if base == "Box"));
    Ok(())
}

/// A trait default declares the owner and method binders its portable body needs, including nested callable types.
#[test]
fn trait_default_declared_parameters_cover_callable_types() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"trait Source[T]:
    def transform[U](self, f: (T) -> U, value: T) -> U:
        return f(value)
"#,
        &["m", "trait_parameter_publication"],
    )?;
    let body = body_named(&module, "transform")?;
    assert_eq!(body.type_parameters, ["T", "U"]);
    let callable = body
        .params
        .iter()
        .find(|parameter| parameter.name == "f")
        .ok_or("missing callable")?;
    let IncanType::Function { params, return_type } = &callable.ty else {
        return Err("missing callable type".into());
    };
    assert_eq!(params[0].ty, IncanType::TypeVar("T".into()));
    assert_eq!(**return_type, IncanType::TypeVar("U".into()));
    Ok(())
}

/// Inferred owner arguments on a constructor retain the concrete checked carrier inside a generic call.
#[test]
fn inferred_model_constructor_carriers_are_closed() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"model Boxed[T]:
    pub value: T

def get_value[T](boxed: Boxed[T]) -> T:
    return boxed.value

def main() -> None:
    println(get_value(Boxed(value=41)))
    println(get_value(Boxed(value="forty-one")))
"#,
        &["m", "inferred_model_carriers"],
    )?;
    let main = body_named(&module, "main")?;
    let int = IncanType::Primitive(IncanPrimitiveType::Int);
    let text = IncanType::Primitive(IncanPrimitiveType::Str);
    let carriers = main
        .locals
        .iter()
        .filter(|local| local.ty != IncanType::Primitive(IncanPrimitiveType::Unit))
        .map(|local| local.ty.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        carriers,
        [
            IncanType::Generic {
                base: "Boxed".into(),
                args: vec![int.clone()]
            },
            int.clone(),
            IncanType::Generic {
                base: "Boxed".into(),
                args: vec![text.clone()]
            },
            text.clone(),
        ]
    );
    let targets = named_targets(&module, "main");
    let instantiations = targets
        .iter()
        .filter(|target| target.name == "get_value")
        .map(|target| target.type_args.clone())
        .collect::<Vec<_>>();
    assert_eq!(instantiations, [vec![int], vec![text]]);
    Ok(())
}

/// A generic class's checked alias fills a generic trait slot under the target body's identity.
#[test]
fn generic_class_trait_alias_retains_the_checked_slot() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"trait Read[T with Clone]:
    def read(self) -> T: ...

    def get(self) -> T:
        return self.read()

class Cell[U with Clone] with Read[int]:
    pub value: int
    pub witness: list[U]
    read = fetch

    def fetch(self) -> int:
        return self.value
"#,
        &["m", "generic_trait_alias"],
    )?;
    let [owner] = module.nominal_declarations.as_slice() else {
        return Err("the generic class layout must be retained".into());
    };
    assert_eq!(owner.type_parameters, ["U"]);
    let read = module
        .trait_implementations
        .iter()
        .find(|record| record.method.declaration_name == "read")
        .ok_or("the alias must fill the required trait slot")?;
    assert_eq!(read.implementation.declaration_name, "fetch");
    assert_eq!(read.owner, owner.canonical);
    assert!(module.is_well_formed_trait_implementation(read));
    let get = module
        .trait_implementations
        .iter()
        .find(|record| record.method.declaration_name == "get")
        .ok_or("the default must remain bound at the adoption's checked arguments")?;
    assert_eq!(get.type_arguments.len(), 1);
    assert_eq!(
        get.type_arguments[0].argument,
        IncanType::Primitive(IncanPrimitiveType::Int)
    );
    assert!(module.is_well_formed_trait_implementation(get));
    Ok(())
}

/// Formatting a declared type parameter keeps its source-level display contract after owner specialization.
#[test]
fn type_parameter_display_is_retained_before_specialization() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "class Box[T with Display]:\n    pub value: T\n    def text(self) -> str:\n        return f\"{self.value}\"\n",
        &["m", "type_parameter_display"],
    )?;
    let snapshot = body_named(&module, "text")?.render_snapshot();
    assert!(snapshot.contains(":type_parameter_display"), "{snapshot}");
    Ok(())
}

/// Model binders remain ordered placeholders in layouts, receiver methods, and deferred default frames.
#[test]
fn model_owner_parameter_frames_are_retained() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"model Envelope[T]:
    pub value: T
    pub label: str = "label"

    def get(self) -> T:
        return self.value

    def pick[U](self, value: U) -> U:
        return value

def main() -> None:
    envelope = Envelope[int](value=42)
    envelope.get()
    envelope.pick[str]("picked")
"#,
        &["m", "model_parameter_frames"],
    )?;
    let [declaration] = module.nominal_declarations.as_slice() else {
        return Err("expected one retained model layout".into());
    };
    assert_eq!(declaration.type_parameters, ["T"]);
    assert_eq!(declaration.type_parameter_count, 1);
    assert_eq!(declaration.field_types[0], IncanType::TypeVar("T".into()));
    assert!(module.is_well_formed_nominal_declaration(declaration));
    let defaults = declaration
        .field_default_body
        .as_ref()
        .ok_or("missing model default frame")?;
    assert_eq!(defaults.locals[0].ty, IncanType::TypeVar("T".into()));

    let get = body_named(&module, "get")?;
    assert_eq!(get.return_type, IncanType::TypeVar("T".into()));
    assert!(get.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Return { value: Some(bir::Operand::Place(read)) }
            if read.fact == bir::OwnershipFact::Clone
    )));
    let pick = body_named(&module, "pick")?;
    assert_eq!(pick.type_parameters, ["U"]);
    assert_eq!(pick.return_type, IncanType::TypeVar("U".into()));
    Ok(())
}
