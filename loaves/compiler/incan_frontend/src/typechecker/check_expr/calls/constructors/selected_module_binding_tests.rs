use super::super::PublicModuleConstructorContext;
use super::TypeChecker;
use crate::ast::{Declaration, Expr, Program, Statement};
use crate::symbols::ResolvedType;

/// Parse the provider and consumer independently so the consumer has no accidental lexical owner binding.
fn parse_module(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    Ok(crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?)
}

/// Module resolution supplies the exact declaration; constructor checking must retain its slots without a second
/// lexical lookup.
#[test]
fn qualified_generic_constructor_retains_selected_owner_and_field_slots() -> Result<(), Box<dyn std::error::Error>> {
    let provider_source =
        "pub model Holder[T]:\n    pub first: T\n    pub middle: int = 9\n    pub last: str = \"default\"\n";
    let provider = parse_module(provider_source)?;
    let mut provider_checker = TypeChecker::new();
    provider_checker.set_current_module_path(Some(vec!["shapes".to_owned()]));
    provider_checker
        .check_program(&provider)
        .map_err(|errors| format!("{errors:?}"))?;
    let declaration = provider_checker
        .lookup_type_info("Holder")
        .cloned()
        .ok_or("missing provider model")?;
    let owner = provider_checker
        .symbols
        .lookup("Holder")
        .and_then(|id| provider_checker.symbols.identity_of(id))
        .cloned()
        .ok_or("missing canonical model owner")?;

    let consumer = parse_module("def main() -> None:\n    value = shapes.Holder[int](last=\"tail\", first=7)\n")?;
    let Declaration::Function(function) = &consumer.declarations.first().ok_or("missing consumer function")?.node
    else {
        return Err("expected a consumer function".into());
    };
    let Statement::Assignment(assignment) = &function.body.first().ok_or("missing constructor assignment")?.node else {
        return Err("expected a constructor assignment".into());
    };
    let Expr::MethodCall(_, _, type_args, arguments) = &assignment.value.node else {
        return Err("expected a module-qualified constructor call".into());
    };
    let span = assignment.value.span;
    let canonical_name = "pub::shapes::Holder";
    let mut checker = TypeChecker::new();
    assert!(checker.lookup_type_info(canonical_name).is_none());
    checker.type_info.record_resolved_identity(span, owner.clone());
    let result = checker.check_public_module_type_constructor_call(
        PublicModuleConstructorContext {
            display_name: "shapes.Holder",
            canonical_name,
            type_info: &declaration,
        },
        type_args,
        arguments,
        span,
        span,
    );
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert_eq!(
        result,
        ResolvedType::Generic(canonical_name.to_owned(), vec![ResolvedType::Int])
    );
    assert_eq!(checker.type_info.resolved_identity(span), Some(&owner));
    let binding = checker
        .type_info
        .body_ir_constructor_field_binding(span)
        .ok_or("missing checked field slots")?;
    assert_eq!(binding.argument_slots, vec![2, 0]);
    assert_eq!(binding.defaulted_slots, vec![1]);
    assert_eq!(binding.field_count, 3);
    Ok(())
}
