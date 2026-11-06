//! Public storage fragments retain canonical initialization and the bodies that need them.

use super::{bir, build_body_ir_module_v0};
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_semantics_core::executable_representation::{DeclarationCoverage, SurfaceReader, build_surface};
use std::collections::BTreeSet;

/// Build real checked storage and its public reader in one package module.
fn storage_module() -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let source = "\"\"\"Retain a public cell and its exact reader.\"\"\"\n\npub static count: int = 10\n\npub def current_count() -> int:\n    \"\"\"Read the same public cell on each call.\"\"\"\n    return count\n\npub def bump(amount: int) -> int:\n    \"\"\"Update the same cell through its checked write identity.\"\"\"\n    count += amount\n    return count\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["storage".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("cells".to_string()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// A published mutating body retains the same checked storage requirement as a reader.
#[test]
fn public_static_writer_retains_its_storage_requirement() -> Result<(), Box<dyn std::error::Error>> {
    let module = storage_module()?;
    let storage = module.static_declarations.first().ok_or("missing storage")?;
    let writer = module
        .bodies
        .iter()
        .find(|body| body.name == "bump")
        .ok_or("missing writer")?;
    assert!(writer.first_representation_gap().is_none(), "writer gap: {writer:#?}");
    let identity = writer.canonical.as_ref().ok_or("missing writer identity")?;
    let public = BTreeSet::from([storage.canonical.clone(), identity.clone()]);
    let bytes = build_surface(
        std::slice::from_ref(&module),
        "cells",
        "0.1.0",
        &public,
        &BTreeSet::new(),
    )?;
    let archive = SurfaceReader::open(&bytes)?;
    let DeclarationCoverage::Covered { requirements, .. } = archive.index().coverage(identity)? else {
        return Err(format!(
            "writer coverage: {:?}; writer {writer:#?}",
            archive.index().coverage(identity)?
        )
        .into());
    };
    assert_eq!(requirements, std::slice::from_ref(&storage.canonical));
    assert_eq!(archive.declaration(identity)?.block, writer.block);
    Ok(())
}

/// A checked import of storage cannot acquire the declaring module's permission to rebind it.
#[test]
fn imported_package_static_keeps_projection_only_writes() -> Result<(), Box<dyn std::error::Error>> {
    let source = "pub static count: int = 10\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let provider = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let source = "from storage import count\n\npub def read() -> int:\n    return count\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let consumer = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["reader".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.set_current_package_identity(Some("cells".to_string()));
    checker.register_dependency_module_path_segments("storage", vec!["storage".to_string()]);
    checker
        .check_with_imports(&consumer, &[("storage", &provider)])
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module = build_body_ir_module_v0(&consumer, &path, checker.type_info());
    let body = module.bodies.first().ok_or("missing reader")?;
    let global = body
        .block
        .stmts
        .iter()
        .find_map(|statement| {
            if let bir::StatementKind::Return {
                value: Some(bir::Operand::Place(value)),
            } = &statement.kind
            {
                value.place.global()
            } else {
                None
            }
        })
        .ok_or("missing imported storage read")?;
    assert_eq!(global.write_policy, bir::GlobalWritePolicy::ProjectionOnly);
    Ok(())
}

/// A public reader publishes its exact storage requirement rather than discarding the initializer or global place.
#[test]
fn public_static_reader_retains_its_storage_requirement() -> Result<(), Box<dyn std::error::Error>> {
    let module = storage_module()?;
    let storage = module.static_declarations.first().ok_or("missing checked storage")?;
    let reader = module
        .bodies
        .first()
        .and_then(|body| body.canonical.as_ref())
        .ok_or("missing reader identity")?;
    let public = BTreeSet::from([storage.canonical.clone(), reader.clone()]);
    let bytes = build_surface(
        std::slice::from_ref(&module),
        "cells",
        "0.1.0",
        &public,
        &BTreeSet::new(),
    )?;
    let archive = SurfaceReader::open(&bytes)?;
    assert!(
        archive.covers(&storage.canonical),
        "public storage lacks executable context: {:?}; module {:?}; storage {storage:#?}",
        archive.index().coverage(&storage.canonical)?,
        module.module_id
    );
    let DeclarationCoverage::Covered { requirements, .. } = archive.index().coverage(reader)? else {
        return Err("public reader lacks executable coverage".into());
    };
    assert_eq!(requirements, std::slice::from_ref(&storage.canonical));
    let published = archive.declaration(reader)?;
    assert_eq!(published.block, module.bodies[0].block);
    Ok(())
}

/// An absent checked storage initializer keeps every requiring reader uncovered before execution.
#[test]
fn public_static_reader_requires_its_checked_initializer() -> Result<(), Box<dyn std::error::Error>> {
    let mut module = storage_module()?;
    let storage = module
        .static_declarations
        .first()
        .ok_or("missing checked storage")?
        .canonical
        .clone();
    let reader = module
        .bodies
        .first()
        .and_then(|body| body.canonical.clone())
        .ok_or("missing reader identity")?;
    module.static_declarations.clear();
    let public = BTreeSet::from([storage, reader.clone()]);
    let bytes = build_surface(&[module], "cells", "0.1.0", &public, &BTreeSet::new())?;
    assert!(!SurfaceReader::open(&bytes)?.covers(&reader));
    Ok(())
}
