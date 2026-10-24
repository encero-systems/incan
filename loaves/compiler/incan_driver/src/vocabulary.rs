//! Refuse vocabulary import cycles before module collection attempts to load the producer's own artifacts.

use std::path::Path;

use incan_frontend::ast::{Declaration, ImportKind, Program, Span, Visibility};
use incan_frontend::diagnostics::CompileError;
use incan_lang::lang::standard_packages::standard_package_namespace_policy;

use crate::unit::UnitDescription;

/// Validate the designated registration export and refuse self-activation in every module of its import closure.
/// Standard namespaces use the compiler's ownership registry rather than a second list of special package names.
pub(crate) fn validate_source(
    program: &Program,
    unit: &UnitDescription,
    source: &Path,
) -> Result<(), Vec<CompileError>> {
    validate_entry_export(program, unit, source)?;
    if unit.vocab_desugarer && is_entry_source(unit, source) {
        validate_desugar_export(program)?;
    }
    for declaration in &program.declarations {
        let Declaration::Import(import) = &declaration.node else {
            continue;
        };
        let activates = match &import.kind {
            ImportKind::PubLibrary { library, .. } | ImportKind::PubFrom { library, .. } => library == &unit.package,
            ImportKind::Module(path) | ImportKind::From { module: path, .. } => {
                let policy = standard_package_namespace_policy(&unit.package);
                path.segments.first().is_some_and(|root| root == "std")
                    && path.segments.get(1).is_some_and(|name| {
                        policy.is_some_and(|policy| policy.namespace_roots.contains(&name.as_str()))
                    })
            }
            _ => false,
        };
        if activates {
            return Err(vec![CompileError::syntax(
                format!(
                    "vocabulary activation cycle: {} entry imports its own surfaces",
                    unit.package
                ),
                declaration.span,
            )]);
        }
    }
    Ok(())
}

/// Require the designated entry to export a synchronous function with no parameters or type parameters.
/// The generated adapter's explicit `VocabRegistration` annotation delegates return-type identity to the checker.
fn validate_entry_export(program: &Program, unit: &UnitDescription, source: &Path) -> Result<(), Vec<CompileError>> {
    if !is_entry_source(unit, source) {
        return Ok(());
    }
    for declaration in &program.declarations {
        let Declaration::Function(function) = &declaration.node else {
            continue;
        };
        if function.name != "library_vocab" {
            continue;
        }
        if function.visibility == Visibility::Public
            && function.params.is_empty()
            && function.type_params.is_empty()
            && !function.is_async()
        {
            return Ok(());
        }
        return Err(export_refusal(declaration.span));
    }
    Err(export_refusal(Span::default()))
}

/// Match only an existing authored entry file; missing paths never become equal by both failing canonicalization.
fn is_entry_source(unit: &UnitDescription, source: &Path) -> bool {
    let Some(entry) = &unit.vocab_entry else {
        return false;
    };
    match (unit.project_root.join(entry).canonicalize(), source.canonicalize()) {
        (Ok(entry), Ok(source)) => entry == source,
        _ => false,
    }
}

/// Require the worker's export to be public, synchronous, non-generic and to accept one immutable request.
/// The generated adapter delegates the request and result type identities to ordinary callable checking.
fn validate_desugar_export(program: &Program) -> Result<(), Vec<CompileError>> {
    let mut span = Span::default();
    for declaration in &program.declarations {
        let Declaration::Function(function) = &declaration.node else {
            continue;
        };
        if function.name != "desugar" {
            continue;
        }
        span = declaration.span;
        if function.visibility == Visibility::Public
            && function.type_params.is_empty()
            && !function.is_async()
            && function.params.len() == 1
            && !function.params[0].node.is_mut
        {
            return Ok(());
        }
        break;
    }
    Err(vec![CompileError::syntax(
        "vocabulary entry must export pub def desugar(request: DesugarRequest) -> Result[DesugarResponse, DesugarError]".into(), span,
    )])
}

/// Anchor the registration export contract at the invalid declaration, or the entry when it is missing.
fn export_refusal(span: Span) -> Vec<CompileError> {
    vec![CompileError::syntax(
        "vocabulary entry must export pub def library_vocab() -> VocabRegistration".to_string(),
        span,
    )]
}
