//! Imported integer constants keep source identity until the driver verifies their published value.

use super::*;

/// Direct and renamed references share one canonical source identity; spelling alone does not grant a value.
#[test]
fn imported_integer_constants_retain_publication_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std import hash\nfrom std.hash import DEFAULT_CHUNK_SIZE as CHUNK\n\ndef selected() -> int:\n  return CHUNK + hash.DEFAULT_CHUNK_SIZE\n",
        &["app", "integer_import"],
    )?;
    let body = body_named(&module, "selected")?.render_snapshot();
    assert_eq!(
        body.matches("@const:std::hash::_core::DEFAULT_CHUNK_SIZE").count(),
        2,
        "{body}"
    );
    assert_eq!(
        module.constant_declarations.len(),
        0,
        "the linked provider must authorize the imported value"
    );
    Ok(())
}

/// A local or source-owned constant with the imported spelling retains its own value.
#[test]
fn imported_integer_spelling_does_not_replace_local_values() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std import hash\nconst DEFAULT_CHUNK_SIZE: int = 7\n\ndef local(DEFAULT_CHUNK_SIZE: int) -> int:\n  return DEFAULT_CHUNK_SIZE\n\ndef declared() -> int:\n  return DEFAULT_CHUNK_SIZE\n",
        &["app", "integer_shadow"],
    )?;
    let local = body_named(&module, "local")?.render_snapshot();
    assert!(!local.contains("const(65536)"), "{local}");
    let declared = body_named(&module, "declared")?.render_snapshot();
    assert!(declared.contains("const(7)"), "{declared}");
    Ok(())
}

/// A stale declaration span cannot grant an imported literal's value to a different source declaration.
#[test]
fn imported_integer_literals_require_the_exact_declaration() -> Result<(), Box<dyn std::error::Error>> {
    let tokens =
        lexer::lex("from std.hash import DEFAULT_CHUNK_SIZE\n\ndef selected() -> int:\n  return DEFAULT_CHUNK_SIZE\n")
            .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut info = checker.type_info().clone();
    let mut changed = false;
    for identity in info.references.resolved_identities.values_mut() {
        if identity.kind == SemanticSourceTargetKind::Const && identity.declaration_name == "DEFAULT_CHUNK_SIZE" {
            identity.declaration_span.start += 1;
            changed = true;
        }
    }
    assert!(
        changed,
        "the negative control must corrupt a retained constant reference"
    );
    let module = build_body_ir_module_v0(&program, &path, &info);
    let body = body_named(&module, "selected")?.render_snapshot();
    assert!(!body.contains("const(65536)"), "{body}");
    Ok(())
}
