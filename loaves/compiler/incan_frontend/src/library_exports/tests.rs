use super::*;
use crate::ast::{Span, Spanned};

/// Exported free-function bounds retain their resolved Rust identity after the provider's alias leaves scope.
#[test]
fn checked_foreign_generic_bound_retains_import_identity() {
    use crate::symbols::{RustImportBindingKind, RustItemInfo, Symbol, SymbolKind};

    let mut checker = TypeChecker::new();
    checker.symbols.define(Symbol {
        name: "Owned".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "serde".to_string(),
            path: "serde::de::DeserializeOwned".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: None,
        }),
        span: Span::default(),
        scope: 0,
    });
    let bound = checked_trait_bound(
        &TraitBound {
            name: "Owned".to_string(),
            type_args: vec![],
        },
        &checker,
    );
    assert_eq!(bound.name, "::serde::de::DeserializeOwned");
}

fn spanned(expr: Expr) -> Spanned<Expr> {
    Spanned::new(expr, Span::default())
}

#[test]
fn checked_preset_value_rejects_non_symbolic_field_paths() {
    let value = Expr::Field(
        Box::new(spanned(Expr::Call(
            Box::new(spanned(Expr::Ident("defaults".to_string()))),
            Vec::new(),
            Vec::new(),
        ))),
        "method".to_string(),
    );

    let checker = TypeChecker::new();
    assert_eq!(
        checked_preset_value(&value, DefaultPathContext::for_checker(&checker)),
        CheckedPresetValue::Unsupported
    );
}

#[test]
fn compiler_registry_materializers_cross_compiled_provider_boundaries() {
    assert!(is_exported_method_name("_checked_current_unit"));
    assert!(is_exported_method_name("_checked_package"));
    assert!(!is_exported_method_name("_private_helper"));
}

/// Public callable aliases retain whether provider parameters are optional even when the alias cannot carry the
/// provider's materializable default expressions itself.
#[test]
fn projected_callable_alias_retains_provider_default_flags() -> Result<(), String> {
    let info = FunctionInfo {
        params: vec![CallableParam::named_with_default(
            "level",
            ResolvedType::Int,
            crate::ast::ParamKind::Normal,
            true,
        )],
        return_type: ResolvedType::Unit,
        is_async: false,
        type_params: Vec::new(),
        type_param_bounds: HashMap::new(),
        type_param_bound_details: HashMap::new(),
        emitted_name: None,
    };
    let projected = checked_projected_function_export("basic_config", &SymbolKind::Function(info))
        .ok_or_else(|| "expected projected function".to_string())?;
    let manifest = crate::library_manifest::function_export_from_checked("demo", &projected);
    assert_eq!(manifest.params.len(), 1);
    assert!(manifest.params[0].has_default);
    Ok(())
}

/// One checked export with the given name and kind, with an unresolved canonical identity unless one is given.
fn named_export(name: &str, kind: CheckedExportKind, canonical: Option<CanonicalSymbolId>) -> CheckedNamedExport {
    let mut identity = CheckedExportIdentity::direct(vec!["lib".to_string(), name.to_string()]);
    identity.canonical = canonical;
    CheckedNamedExport {
        name: name.to_string(),
        identity,
        kind,
    }
}

/// A canonical identity for a module-level declaration in `lib`.
fn canonical(name: &str) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(
        vec!["lib".to_string()],
        name,
        SemanticSourceTargetKind::Function,
        incan_semantics_core::HirSourceSpan::new(0, 1),
    )
}

/// Renaming rewrites the public name on the export and on its kind, and an alias's projected callable follows
/// the alias while its emitted name stays with the declaration.
#[test]
fn a_renamed_export_keeps_its_kind_and_renames_the_projected_callable_issue1298() -> Result<(), String> {
    let function = CheckedFunctionExport {
        ownership: None,
        name: "compute".to_string(),
        emitted_name: Some("compute".to_string()),
        type_params: Vec::new(),
        params: Vec::new(),
        param_defaults: Vec::new(),
        return_type: ResolvedType::Unit,
        is_async: false,
    };
    let alias = named_export(
        "compute",
        CheckedExportKind::Alias(CheckedAliasExport {
            name: "compute".to_string(),
            target_path: vec!["math".to_string(), "compute".to_string()],
            projected_function: Some(function.clone()),
            projected_type: None,
        }),
        None,
    );
    let renamed = alias.renamed("calc");
    assert_eq!(renamed.name, "calc");
    let CheckedExportKind::Alias(kind) = &renamed.kind else {
        return Err("renaming must preserve the alias kind".to_string());
    };
    assert_eq!(kind.name, "calc");
    assert_eq!(kind.projected_function.as_ref().map(|f| f.name.as_str()), Some("calc"));
    assert_eq!(
        kind.projected_function.as_ref().and_then(|f| f.emitted_name.as_deref()),
        Some("compute")
    );

    let plain = named_export("compute", CheckedExportKind::Function(function), None).renamed("calc");
    let CheckedExportKind::Function(kind) = &plain.kind else {
        return Err("renaming must preserve the function kind".to_string());
    };
    assert_eq!(
        (plain.name.as_str(), kind.name.as_str(), kind.emitted_name.as_deref()),
        ("calc", "calc", Some("compute"))
    );
    Ok(())
}

/// A re-exported provider export adopts the entrypoint identity whose canonical id matches it; a lone candidate
/// is adopted without a match; with no candidate the renamed provider identity stands.
#[test]
fn a_reexport_adopts_the_matching_entrypoint_identity_issue1298() {
    let constant = |name: &str, canonical: Option<CanonicalSymbolId>| {
        named_export(
            name,
            CheckedExportKind::Const(CheckedConstExport {
                name: name.to_string(),
                ty: ResolvedType::Int,
            }),
            canonical,
        )
    };
    let provider = constant("limit", Some(canonical("limit")));
    let mut matching = constant("cap", Some(canonical("limit")));
    matching.identity.source_path = vec!["entry".to_string(), "cap".to_string()];
    let other = constant("cap", Some(canonical("other")));
    let entrypoint = HashMap::from([("cap".to_string(), vec![other.clone(), matching.clone()])]);

    let projected = provider.projected_through_reexport("cap", Some(&entrypoint));
    assert_eq!(projected.name, "cap");
    assert_eq!(projected.identity.source_path, matching.identity.source_path);

    let lone = HashMap::from([("cap".to_string(), vec![other.clone()])]);
    let projected = provider.projected_through_reexport("cap", Some(&lone));
    assert_eq!(projected.identity.canonical, other.identity.canonical);

    let projected = provider.projected_through_reexport("cap", None);
    assert_eq!(projected.identity.source_path, provider.identity.source_path);
    assert_eq!(projected.identity.canonical, provider.identity.canonical);
}

#[test]
fn library_export_registry_owns_duplicate_detection_and_diagnostic() -> Result<(), String> {
    let mut registry = LibraryExportBindingRegistry::default();
    let first = Span::new(4, 10);
    let duplicate = Span::new(20, 26);

    assert!(registry.register("render", first).is_ok());
    let error = match registry.register("render", duplicate) {
        Ok(()) => return Err("the second projected name did not collide".to_string()),
        Err(error) => error,
    };

    assert_eq!(error.message, "Duplicate library export `render` in `src/lib.incn`");
    assert_eq!(error.span, duplicate);
    assert_eq!(error.related_spans().first().map(|related| related.span), Some(first));
    Ok(())
}
