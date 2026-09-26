//! A union member nominal that several modules of one crate declare is spelled by its declaring module, so each
//! module's union has its own wrapper (#1796).

use super::*;
use crate::lower::CrateNominalContext;
use std::sync::Arc;

/// One module declaring `Product` and `Answer = Product | int`.
const PRODUCT_MODULE: &str = "pub model Product:\n    pub value: int\n\n\npub type Answer = Product | int\n";

/// Parse one module, keeping lexer and parser failures as test errors.
fn parse_module(source: &str) -> Result<ast::Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))
}

/// The crate context for modules `first` and `second`, emitted at their own paths, that both declare `Product`.
fn two_module_context(first: &ast::Program, second: &ast::Program) -> CrateNominalContext {
    CrateNominalContext::from_modules([
        (vec![vec!["first".to_string()]], vec!["first".to_string()], first),
        (vec![vec!["second".to_string()]], vec!["second".to_string()], second),
    ])
}

/// Check and lower `program` as module `module` of a crate described by `context`, returning `Answer`'s IR type.
fn lowered_answer(
    module: &str,
    program: &ast::Program,
    context: Option<CrateNominalContext>,
) -> Result<IrType, String> {
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec![module.to_string()]));
    checker
        .check_program(program)
        .map_err(|errors| format!("{module} should typecheck: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    lowering.set_current_source_module_name(Some(module.to_string()));
    lowering.set_crate_nominal_context(context.map(Arc::new));
    let ir = lowering
        .lower_program(program)
        .map_err(|errors| format!("{module} lowering failed: {errors:?}"))?;
    ir.declarations
        .into_iter()
        .find_map(|decl| match decl.kind {
            IrDeclKind::TypeAlias { name, ty, .. } if name == "Answer" => Some(ty),
            _ => None,
        })
        .ok_or_else(|| format!("{module} lowered no `Answer` alias"))
}

/// #1796: two modules' `Product | int` spell their `Product` by its declaring module, so the two unions are two
/// wrappers, each carrying its own module's `Product`, where before they were one.
#[test]
fn shared_nominal_union_members_are_spelled_by_their_declaring_module_issue1796() -> Result<(), String> {
    let first = parse_module(PRODUCT_MODULE)?;
    let second = parse_module(PRODUCT_MODULE)?;
    let context = two_module_context(&first, &second);
    assert!(context.shared_spellings.contains("Product"), "{context:?}");

    let first_answer = lowered_answer("first", &first, Some(context.clone()))?;
    let second_answer = lowered_answer("second", &second, Some(context))?;
    assert_eq!(
        first_answer.union_members(),
        Some(&[IrType::Struct("crate::first::Product".to_string()), IrType::Int][..]),
        "{first_answer:?}"
    );
    assert_eq!(
        second_answer.union_members(),
        Some(&[IrType::Struct("crate::second::Product".to_string()), IrType::Int][..]),
        "{second_answer:?}"
    );
    assert_ne!(
        first_answer.union_type_name(),
        second_answer.union_type_name(),
        "each module's union needs its own wrapper"
    );
    assert_eq!(
        first_answer.union_variant_index_for_member(&IrType::Struct("Product".to_string())),
        Some(0),
        "a value of the module's own `Product` still inhabits the union"
    );
    Ok(())
}

/// #1796: a nominal only one module declares keeps its spelling, and so its union keeps the wrapper name it had.
#[test]
fn unshared_nominal_union_members_keep_their_spelling_issue1796() -> Result<(), String> {
    let first = parse_module(PRODUCT_MODULE)?;
    let second = parse_module("pub model Receipt:\n    pub total: int\n")?;
    let context = two_module_context(&first, &second);
    let answer = lowered_answer("first", &first, Some(context))?;
    let without_context = lowered_answer("first", &first, None)?;
    assert_eq!(
        answer.union_members(),
        Some(&[IrType::Struct("Product".to_string()), IrType::Int][..]),
        "{answer:?}"
    );
    assert_eq!(answer, without_context);
    Ok(())
}
