//! Checked SDK factory selection and source-owned omitted argument defaults.

use super::{build, named_targets};
use crate::body_ir::{build_body_ir_module_v0, library_call_delegations};
use crate::{ast, lexer, parser, typechecker::TypeChecker};
use incan_semantics_core::body_ir as bir;
use incan_semantics_core::{IncanPrimitiveType, IncanType, SemanticSourceTargetKind, SymbolOrigin};

/// Omitted constructor arguments keep the selected factory and its declaring SDK's literal defaults.
#[test]
fn omitted_sdk_factory_arguments_retain_declared_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let call = "Regex(\"x+\")";
    let source = format!(
        "from std.regex import Regex, RegexError\n\ndef build() -> Result[Regex, RegexError]:\n  return {call}\n"
    );
    let tokens = lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["omitted_factory".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let start = source.find(call).ok_or("factory call absent")?;
    let span = ast::Span {
        start,
        end: start + call.len(),
    };
    let expected = checker
        .type_info()
        .resolved_identity(span)
        .ok_or("factory selection absent")?;
    assert_eq!(expected.kind, SemanticSourceTargetKind::Method);
    assert_eq!(expected.declaration_name, "__incan_new");
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let targets = named_targets(&module, "build");
    let [target] = targets.as_slice() else {
        return Err("expected one factory call".into());
    };
    assert_eq!(target.canonical.as_ref(), Some(expected));
    let bir::ArgumentBinding::ResolvedDeclarationOrder {
        arguments,
        defaulted_slots,
    } = &target.binding
    else {
        return Err("checked factory binding policy absent".into());
    };
    assert_eq!(arguments.len(), 1);
    assert_eq!(arguments[0].slot, 0);
    assert_eq!(arguments[0].written_position, 0);
    assert_eq!(*defaulted_slots, vec![1, 2, 3, 4]);
    // Source checking names the same declaration before SDK publication rebases its std root. The collector checks
    // this exact package-owned view against the selected source catalog, as library_calls::tests does.
    let SymbolOrigin::Module(declaring) = &expected.origin else {
        return Err("expected a checked source factory identity".into());
    };
    let [root, module_path @ ..] = declaring.as_slice() else {
        return Err("factory has no declaring source module".into());
    };
    assert_eq!(root, "std");
    let mut published = expected.clone();
    published.origin = SymbolOrigin::Package {
        library: "incan_stdlib_data".to_owned(),
        module_path: module_path.to_vec(),
    };
    let mut sdk_facts = checker.type_info().clone();
    sdk_facts
        .references
        .resolved_identities
        .insert((span.start, span.end), published.clone());
    let delegations = library_call_delegations(&sdk_facts);
    let selected = delegations
        .iter()
        .find(|record| record.canonical == published)
        .ok_or("SDK factory record absent")?;
    assert_eq!(
        selected.rust_path().as_deref(),
        Some("incan_stdlib_data::regex::_core::Regex::__incan_new")
    );
    assert_eq!(
        selected.parameters,
        vec![
            IncanType::Primitive(IncanPrimitiveType::Str),
            IncanType::Primitive(IncanPrimitiveType::Bool),
            IncanType::Primitive(IncanPrimitiveType::Bool),
            IncanType::Primitive(IncanPrimitiveType::Bool),
            IncanType::Primitive(IncanPrimitiveType::Bool),
        ]
    );
    assert_eq!(
        selected.return_type,
        IncanType::Generic {
            base: "Result".to_owned(),
            args: vec![
                IncanType::Named("Regex".to_owned()),
                IncanType::Named("RegexError".to_owned())
            ],
        }
    );
    let [required, flags @ ..] = selected.parameter_defaults.as_slice() else {
        return Err("factory default frame absent".into());
    };
    assert!(matches!(required, bir::CallableParamDefault::Required));
    assert_eq!(flags.len(), 4);
    for flag in flags {
        let bir::CallableParamDefault::Source(computation) = flag else {
            return Err("declared flag default is not a retained computation".into());
        };
        assert!(computation.stmts.is_empty());
        assert!(matches!(
            computation.result,
            bir::Operand::Constant(bir::Constant::Bool(false))
        ));
    }
    Ok(())
}

/// Nominal identity and callable slots cannot replace an absent checked full-call factory selection.
#[test]
fn omitted_sdk_factory_requires_full_call_selection() -> Result<(), Box<dyn std::error::Error>> {
    let call = "Regex(\"x+\")";
    let source = format!(
        "from std.regex import Regex, RegexError\n\ndef build() -> Result[Regex, RegexError]:\n  return {call}\n"
    );
    let tokens = lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["unselected_factory".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let start = source.find(call).ok_or("factory call absent")?;
    let span = ast::Span {
        start,
        end: start + call.len(),
    };
    let mut checked = checker.type_info().clone();
    let removed = checked
        .references
        .resolved_identities
        .remove(&(span.start, span.end))
        .ok_or("factory selection absent")?;
    assert_eq!(removed.kind, SemanticSourceTargetKind::Method);
    assert!(checked.call_site_callable_params(span).is_some());
    assert!(checked.resolved_identity(ast::Span { start, end: start + 5 }).is_some());
    let module = build_body_ir_module_v0(&program, &path, &checked);
    let targets = named_targets(&module, "build");
    let [target] = targets.as_slice() else {
        return Err("expected one unselected nominal call".into());
    };
    assert_ne!(target.canonical.as_ref(), Some(&removed));
    assert!(target.direct_call_id.is_none());
    Ok(())
}

/// A source function named Regex keeps its local function identity and never acquires SDK factory defaults.
#[test]
fn omitted_sdk_factory_preserves_source_shadowing() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def Regex(value: int) -> int:\n  return value + 1\n\ndef main() -> int:\n  return Regex(3)\n",
        &["shadow_factory"],
    )?;
    let targets = named_targets(&module, "main");
    let [target] = targets.as_slice() else {
        return Err("expected one local function call".into());
    };
    let identity = target.canonical.as_ref().ok_or("local function identity absent")?;
    assert_eq!(identity.kind, SemanticSourceTargetKind::Function);
    assert_eq!(identity.declaration_name, "Regex");
    assert!(target.direct_call_id.is_some());
    Ok(())
}
