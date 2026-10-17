//! Publication retains the checked generic collection default rather than rebuilding a trait body.

use std::collections::BTreeSet;

use crate::body_ir::build_body_ir_module_v0;
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_semantics_core::SymbolOrigin;
use incan_semantics_core::executable_representation::{SurfaceReader, build_surface};

/// A published take default retains its constructor carrier before a consumer closes Self and trait arguments.
#[test]
fn standard_take_default_retains_concrete_return_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("../../../../../stdlib/core/src/derives/collection.incn");
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["derives".to_string(), "collection".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_standard_library_source(true);
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("incan_stdlib_core".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let body = module
        .bodies
        .iter()
        .find(|body| {
            body.name == "take" && body.locals.iter().any(|local| {
            matches!(&local.ty, super::IncanType::Generic { base, .. } if base == "TakeFallibleIterator")
        })
        })
        .ok_or("missing checked fallible take default")?;
    assert!(
        matches!(&body.return_type, super::IncanType::Generic { base, args } if base == "TakeFallibleIterator" && args.len() == 3),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// The same nested Result/Option matches and list mutation as the standard collection default remain checked facts.
#[test]
fn generic_fallible_collect_default_is_publishable() -> Result<(), Box<dyn std::error::Error>> {
    checked_collect_is_publishable(false)
}

/// Standard-library checking must retain executable pattern types while preserving legacy's open Self call view.
#[test]
fn standard_source_collect_default_is_publishable() -> Result<(), Box<dyn std::error::Error>> {
    checked_collect_is_publishable(true)
}

/// Build the identical collecting body under ordinary or standard source authority, using checked slot identities.
fn checked_collect_is_publishable(standard_source: bool) -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"pub trait Stream[T with Clone, E]:
    def __next__(mut self) -> Result[Option[T], E]:
        ...

    def collect(mut self) -> Result[list[T], E]:
        mut items: list[T] = []
        while true:
            match self.__next__():
                Ok(Some(item)) => items.append(item)
                Ok(None) => return Ok(items)
                Err(error) => return Err(error)
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_standard_library_source(standard_source);
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("streams".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut public: BTreeSet<_> = checker
        .type_info()
        .traits
        .method_identities
        .values()
        .filter(|identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library == "streams"))
        .cloned()
        .collect();
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    public.extend(module.trait_declarations.iter().cloned());
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "collect")
        .ok_or("missing checked collect")?;
    let root = body.canonical.clone().ok_or("missing collect identity")?;
    let snapshot = body.render_snapshot();
    let bytes = build_surface(&[module], "streams", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        reader.covers(&root),
        "{:?}\n{snapshot}",
        reader.index().coverage(&root)?
    );
    Ok(())
}
