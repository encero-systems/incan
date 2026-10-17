//! Checked generic default frames retain their declaring context at package publication.

use std::collections::BTreeSet;

use crate::body_ir::build_body_ir_module_v0;
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::executable_representation::{ExecutableDeclaration, SurfaceReader, build_surface};

/// Use the declaring parser and checker instead of inventing a generic frame at the consumer.
fn checked_module() -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let source = "pub model Envelope[T]:\n    pub value: T\n    marker: Option[T] = None\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("envelopes".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// A generic default frame belongs to its actual declaring model, even though both carry the same canonical identity.
#[test]
fn generic_default_frame_retains_its_declaring_context() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module()?;
    let model = module.nominal_declarations.first().ok_or("missing checked Envelope")?;
    let frame = model
        .field_default_body
        .as_ref()
        .ok_or("missing checked default frame")?;
    assert_eq!(frame.canonical.as_ref(), Some(&model.canonical));
    assert_eq!(model.type_parameters, ["T"]);
    assert!(frame.type_parameters.is_empty());
    assert_eq!(frame.params[0].ty, IncanType::TypeVar("T".to_string()));
    Ok(())
}

/// Publication retains the complete checked generic layout and its deferred defaults without exporting private storage.
#[test]
fn generic_default_layout_is_publishable() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked_module()?;
    let model = module.nominal_declarations.first().ok_or("missing checked Envelope")?;
    let root = model.canonical.clone();
    let mut public = BTreeSet::from([root.clone()]);
    public.extend(
        model
            .field_identities
            .iter()
            .zip(&model.field_public)
            .filter(|(_, visible)| **visible)
            .map(|(identity, _)| identity.clone()),
    );
    let bytes = build_surface(&[module], "envelopes", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&root));
    let ExecutableDeclaration::Nominal(model) = reader.fragment(&root)? else {
        return Err("expected checked nominal layout".into());
    };
    assert!(
        model
            .field_default_body
            .ok_or("missing published defaults")?
            .type_parameters
            .is_empty()
    );
    assert_eq!(
        reader.index().declarations.keys().cloned().collect::<BTreeSet<_>>(),
        public
    );
    Ok(())
}

/// Declaring-owner context cannot legitimize a type variable absent from that actual model's binder list.
#[test]
fn generic_default_layout_refuses_an_undeclared_parameter() -> Result<(), Box<dyn std::error::Error>> {
    let mut module = checked_module()?;
    let model = module
        .nominal_declarations
        .first_mut()
        .ok_or("missing checked Envelope")?;
    let root = model.canonical.clone();
    let frame = model
        .field_default_body
        .as_mut()
        .ok_or("missing checked default frame")?;
    frame.params[0].ty = IncanType::TypeVar("Other".to_string());
    frame.locals[0].ty = IncanType::TypeVar("Other".to_string());
    let bytes = build_surface(
        &[module],
        "envelopes",
        "0.1.0",
        &BTreeSet::from([root.clone()]),
        &BTreeSet::new(),
    )?;
    assert!(!SurfaceReader::open(&bytes)?.covers(&root));
    Ok(())
}

/// The actual Iterator.find default publishes its generic Option payload without inventing a missing reference.
#[test]
fn iterator_find_retains_publishable_option_payload() -> Result<(), Box<dyn std::error::Error>> {
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
        .find(|body| body.name == "find")
        .ok_or("missing actual find default")?;
    let root = body.canonical.clone().ok_or("missing find identity")?;
    assert_eq!(body.type_parameters, ["T"]);
    let mut public = module.trait_declarations.iter().cloned().collect::<BTreeSet<_>>();
    public.extend(
        checker
            .type_info()
            .declarations
            .method_bindings_by_span
            .values()
            .filter_map(|binding| binding.identity.clone()),
    );
    public.insert(root.clone());
    let snapshot = body.render_snapshot();
    let bytes = build_surface(&[module], "incan_stdlib_core", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        reader.covers(&root),
        "{:?}\n{snapshot}",
        reader.index().coverage(&root)?
    );
    Ok(())
}
