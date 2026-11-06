//! Published owner bounds must retain the declaring trait's provenance rather than the caller's module.

use std::collections::HashMap;

use crate::ast::{CallArg, Expr, Literal, Span, Spanned};
use crate::symbols::{ResolvedType, TypeInfo};
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};

/// A manifest's unqualified OrdinalKey bound closes only through the authenticated stdlib owner's declaration.
#[test]
fn published_factory_bound_uses_declaring_module() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Checked stdlib owner ----
    let tokens = lexer::lex("from std.collections import OrdinalMap\n").map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let Some(TypeInfo::Class(owner)) = checker.lookup_semantic_type_info("OrdinalMap") else {
        return Err("missing checked OrdinalMap class".into());
    };
    let method = owner.methods.get("from_keys").ok_or("missing static factory")?.clone();
    let identity = method.identity.clone().ok_or("missing factory identity")?;

    // ---- Native SDK bounds without module/source-name fields ----
    for bounds in checker.nominal_type_param_bounds.values_mut() {
        for (_, requirements) in bounds {
            for bound in requirements {
                bound.module_path = None;
                bound.source_name = None;
            }
        }
    }

    // ---- Checked factory constraints ----
    let argument_span = Span::new(1, 2);
    let call_span = Span::new(1, 3);
    checker.record_expr_type(
        argument_span,
        ResolvedType::Generic("List".into(), vec![ResolvedType::Str]),
    );
    checker.type_info.record_resolved_identity(call_span, identity);
    let arguments = [CallArg::Positional(Spanned::new(
        Expr::Literal(Literal::None),
        argument_span,
    ))];
    let expected = ResolvedType::Generic(
        "Result".into(),
        vec![
            ResolvedType::Generic("OrdinalMap".into(), vec![ResolvedType::Str]),
            ResolvedType::Unknown,
        ],
    );
    checker.retain_body_ir_factory_owner(
        &method,
        &ResolvedType::Named("OrdinalMap".into()),
        &arguments,
        &HashMap::new(),
        Some(&expected),
        call_span,
    );
    assert_eq!(
        checker.type_info.calls.body_ir_call_type_arguments.get(&(1, 3)),
        Some(&vec![ResolvedType::Str])
    );
    let result = checker
        .type_info
        .calls
        .body_ir_call_result_types
        .get(&(1, 3))
        .ok_or("missing native factory result")?;
    assert_eq!(
        result,
        &ResolvedType::Generic(
            "Result".into(),
            vec![
                ResolvedType::Generic("OrdinalMap".into(), vec![ResolvedType::Str]),
                ResolvedType::Named("OrdinalMapError".into()),
            ]
        )
    );
    Ok(())
}
