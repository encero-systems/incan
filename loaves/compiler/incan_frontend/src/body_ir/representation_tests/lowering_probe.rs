//! Lane-local probe (not committed): lower checked sources through the Rust lowering and record what it says.

use super::*;
use std::fmt::Write as _;

fn lower(source: &str) -> String {
    let Ok(tokens) = crate::lexer::lex(source) else {
        return "lex-error".into();
    };
    let Ok(program) = crate::parser::parse(&tokens) else {
        return "parse-error".into();
    };
    let mut checker = TypeChecker::new();
    if let Err(errors) = checker.check_program(&program) {
        return format!("refused {:?}", errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>());
    }
    let facts = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let module = crate::body_ir::build_body_ir_module(&program, &[], &facts, &context);
    if source.starts_with("#bir") {
        return module.bodies.iter().map(|body| format!("{body:?}\n")).collect();
    }
    let inputs = incan_lowering::SourceInputs {
        sources: vec![source.to_owned()],
        files: vec!["app.incn".to_owned()],
        ..Default::default()
    };
    let mut published = Vec::new();
    match incan_lowering::unit_plan(std::slice::from_ref(&module), &inputs, 0, "library", &mut published) {
        Ok(incan_lowering::UnitPlanning::Complete(Some(plan))) => {
            if !source.starts_with("#dump") {
                return "plan".into();
            }
            let names: Vec<String> = program
                .declarations
                .iter()
                .filter_map(|declaration| match &declaration.node {
                    crate::ast::Declaration::Function(function) => Some(function.name.clone()),
                    _ => None,
                })
                .collect();
            let mut text = String::from("plan\n");
            for function in plan.functions.iter().filter(|function| names.iter().any(|name| function.name.ends_with(name.as_str()))) {
                let _ = writeln!(text, "fn {} params {:?}\n  locals {:?}\n  blocks {:?}", function.name, function.parameters, function.locals, function.blocks);
            }
            text
        }
        Ok(incan_lowering::UnitPlanning::Complete(None)) => "no plan".into(),
        Ok(incan_lowering::UnitPlanning::NeedsRuntime(_, requirements)) => format!("needs runtime {requirements:?}"),
        Err(reason) => format!("lowering refused: {reason}"),
    }
}

#[test]
fn probe_lowering() -> Result<(), Box<dyn std::error::Error>> {
    let list = std::fs::read_to_string("/Volumes/Builds/s1-night-1118/lanes/borrowed/run2/probe/sources.txt")?;
    let mut out = String::new();
    for case in list.split("\n=====\n") {
        let case = case.trim_matches('\n');
        if case.is_empty() {
            continue;
        }
        let verdict = std::panic::catch_unwind(|| lower(case)).unwrap_or_else(|_| "panic".into());
        writeln!(out, "{case}\n--> {verdict}\n")?;
    }
    std::fs::write("/Volumes/Builds/s1-night-1118/lanes/borrowed/run2/probe/out.txt", out)?;
    Ok(())
}
