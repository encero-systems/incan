//! Imported enum payloads publish only their checked canonical type context.

use super::{bir, build_body_ir_module_v0};
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_semantics_core::SymbolOrigin;
use incan_semantics_core::executable_representation::{
    DeclarationCoverage, ExecutableDeclaration, SurfaceReader, build_surface,
};
use std::collections::BTreeSet;

/// Build a package enum and its imported public payload through the ordinary checker.
fn payload_modules() -> Result<Vec<bir::BodyIrModule>, Box<dyn std::error::Error>> {
    let payload_source = "pub model Payload:\n    \"\"\"Retain the declaring payload context.\"\"\"\n\n    pub value: int\n\npub model Unused:\n    \"\"\"Keep unrelated public context out of the selected closure.\"\"\"\n\n    pub other: int\n";
    let enum_source = "from shapes import Payload as Item\n\npub enum Outcome:\n    \"\"\"Retain the imported payload's checked origin.\"\"\"\n\n    Found(Item)\n    Missing\n";
    let payload_tokens = lexer::lex(payload_source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let payload_program =
        parser::parse(&payload_tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let enum_tokens = lexer::lex(enum_source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let enum_program = parser::parse(&enum_tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut modules = Vec::new();
    for (path, program, imports) in [
        ("shapes", &payload_program, Vec::new()),
        ("results", &enum_program, vec![("shapes", &payload_program)]),
    ] {
        let path = vec![path.to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(path.clone()));
        checker.set_current_package_identity(Some("payloads".to_string()));
        checker.register_dependency_module_path_segments("shapes", vec!["shapes".to_string()]);
        checker
            .check_with_imports(program, &imports)
            .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        modules.push(build_body_ir_module_v0(program, &path, checker.type_info()));
    }
    Ok(modules)
}

/// An aliased imported payload carries the declaring module's exact identity and only that used requirement.
#[test]
fn imported_enum_payload_publishes_its_canonical_context() -> Result<(), Box<dyn std::error::Error>> {
    let modules = payload_modules()?;
    let payload = modules[0].nominal_declarations.first().ok_or("missing payload")?;
    let value = modules[1].enum_declarations.first().ok_or("missing enum")?;
    let mut public = BTreeSet::from([payload.canonical.clone(), value.canonical.clone()]);
    public.extend(payload.field_identities.iter().cloned());
    public.extend(value.variants.iter().map(|variant| variant.canonical.clone()));
    let bytes = build_surface(&modules, "payloads", "0.1.0", &public, &BTreeSet::new())?;
    let reader = SurfaceReader::open(&bytes)?;
    let DeclarationCoverage::Covered { requirements, .. } = reader.index().coverage(&value.canonical)? else {
        return Err(format!(
            "imported payload enum lacks executable coverage: {:?}; enum {value:#?}; payload {payload:#?}",
            reader.index().coverage(&value.canonical)?
        )
        .into());
    };
    assert_eq!(requirements, &[payload.canonical.clone()]);
    let ExecutableDeclaration::Enum(published) = reader.fragment(&value.canonical)? else {
        return Err("expected published enum layout".into());
    };
    assert_eq!(published.variants[0].fields, value.variants[0].fields);
    assert_eq!(
        payload.canonical.origin,
        SymbolOrigin::Package {
            library: "payloads".to_string(),
            module_path: vec!["shapes".to_string()],
        }
    );
    Ok(())
}

/// An unavailable payload keeps its public enum uncovered instead of fabricating a layout from its spelling.
#[test]
fn imported_enum_payload_requires_the_public_payload_layout() -> Result<(), Box<dyn std::error::Error>> {
    let modules = payload_modules()?;
    let value = modules[1].enum_declarations.first().ok_or("missing enum")?;
    let mut public = BTreeSet::from([value.canonical.clone()]);
    public.extend(value.variants.iter().map(|variant| variant.canonical.clone()));
    let bytes = build_surface(&modules, "payloads", "0.1.0", &public, &BTreeSet::new())?;
    assert!(!SurfaceReader::open(&bytes)?.covers(&value.canonical));
    Ok(())
}
