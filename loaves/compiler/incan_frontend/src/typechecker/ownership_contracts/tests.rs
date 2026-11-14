//! Tests for checked ownership contracts, source flow and projection.

use super::*;

mod body_projection;
mod contracts_and_modes;
mod explicit_calls;
mod invocations_and_accesses;
mod lazy_reader_control_flow;
mod source_flow;
mod trait_inputs;

/// Project actual checked source so CFG tests cannot bypass canonical root/type lookup with hand-built facts.
fn source_graph(source: &str) -> Result<Result<flow::Graph, super::source::Unavailable>, Box<dyn std::error::Error>> {
    let checker = checked(source)?;
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declaration = program.declarations.first().ok_or("missing function")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("expected function".into());
    };
    let identity = checker
        .type_info
        .declarations
        .function_bindings_by_span
        .get(&declaration.span)
        .and_then(|binding| binding.identity.as_ref())
        .ok_or("missing selected source declaration")?;
    let projected = super::source::build(&checker, CallableSource::Function(function), declaration.span, identity);
    let recorded = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get(identity)
            .ok_or_else(|| format!("source analysis was not retained with its checked body\n{source}\nerrors: {:?}\ncalls: {:?}\ncontracts: {:?}",
                checker.errors, checker.type_info.protocols.explicit_input_calls, checker.type_info.declarations.ownership_contracts))?;
    match (recorded, &projected) {
        (InputFlow::Checked { graph, analysis }, Ok(rebuilt)) => {
            assert_eq!(graph.nodes.len(), rebuilt.nodes.len());
            assert_eq!(analysis, &flow::analyze(rebuilt).map_err(|fault| format!("{fault:?}"))?);
        }
        (InputFlow::Unavailable(reason), Err(rebuilt)) => assert_eq!(reason, rebuilt),
        (InputFlow::Invalid { graph, fault }, _) => {
            return Err(format!("invalid body graph ({} nodes): {fault:?}", graph.nodes.len()).into());
        }
        _ => return Err(format!("scoped and rebuilt source flow differ: {recorded:?}\n{projected:?}").into()),
    }
    Ok(projected)
}

/// Check source directly so ownership diagnostics cannot be supplied by a native compiler failure.
fn checked(source: &str) -> Result<TypeChecker, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    if let Err(errors) = checker.check_program(&program) {
        checker.errors = errors;
    }
    Ok(checker)
}

/// Each body's flow outcome, for assertion messages.
fn flow_summary(checker: &TypeChecker) -> Vec<(String, String)> {
    checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .map(|(owner, flow)| {
            let outcome = match flow {
                InputFlow::Checked { analysis, .. } => format!("checked {:?}", analysis.conflicts),
                InputFlow::Unavailable(reason) => format!("unavailable {reason:?}"),
                InputFlow::Invalid { fault, .. } => format!("invalid {fault:?}"),
            };
            (owner.declaration_name.clone(), outcome)
        })
        .collect()
}
