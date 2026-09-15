//! The embedded-fragment vocab pass, through lowering to the emission refusal.

use incan_frontend::ast;
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::vocab_desugar_pass::desugar_program_vocab_blocks;
use incan_syntax::lexer;

/// Build a minimal "known library" index entry so `import pub::webkit` resolves during typecheck.
///
/// This satisfies only the import-resolution gate (`collect_pub_library_import` in
/// `typechecker/collect/stdlib_imports.rs`) -- the embedded-fragment descriptors themselves are parser-only
/// concerns already baked into the AST by `parse_with_source`'s hand-built keyword/surface maps, so no real
/// checked-registry metadata is needed here.
fn known_library_index(name: &str) -> LibraryManifestIndex {
    let manifest = LibraryManifest::new(name, "0.1.0");
    let mut root = std::env::temp_dir();
    root.push(format!("incan_embedded_fragment_test_{name}_artifacts"));
    root.push("target");
    root.push("lib");
    LibraryManifestIndex::from_entries(std::collections::HashMap::from([(
        name.to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(name, name, root),
        },
    )]))
}

/// Full parser -> desugar -> typecheck pipeline for one `html:`-block fixture using a hand-built descriptor map (RFC
/// 081, `#1023`), proving the `vocab_block_body_is_embedded_fragment` bypass this file adds: an embedded fragment's
/// `VocabBlockStmt` wrapper must reach typecheck as its unwrapped `Expr::Embedded` directly, never through
/// `runtime.desugar_node` (which would require a registered WASM desugarer that embedded-fragment descriptors never
/// register).
#[test]
fn embedded_fragment_vocab_block_bypasses_wasm_desugar_and_typechecks() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::webkit\n\ndef render(title: str) -> None:\n    html:\n        <h1>{title}</h1>\n\ndef main() -> None:\n    render(\"Hello\")\n";
    let (tokens, _lex_errors) = lexer::lex_tolerant(source);

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "webkit".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "webkit.html".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec::block("html")],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "webkit".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("webkit.html")
                .with_declaration(incan_vocab::DeclarationSurface::named("html"))
                .with_embedded_fragment(
                    incan_vocab::EmbeddedFragmentDescriptor::new(
                        "html.fragment",
                        incan_vocab::EmbeddedFragmentSubmode::Markup,
                        "fragment",
                    )
                    .in_declaration_body("html"),
                ),
        ],
    );

    let mut program =
        incan_syntax::parser::parse_with_source(&tokens, None, Some(&keyword_map), Some(&surface_map), source)
            .map_err(|errs| format!("parse errors: {errs:?}"))?;

    // Before this pass: the fragment is still wrapped in an ordinary `Statement::VocabBlock`.
    let ast::Declaration::Function(function) = &program.declarations[1].node else {
        return Err("expected a function declaration at index 1".into());
    };
    assert!(matches!(&function.body[0].node, ast::Statement::VocabBlock(_)));

    desugar_program_vocab_blocks(&mut program, None, &LibraryManifestIndex::default())
        .map_err(|errs| format!("desugar pass errors (should not need a registered WASM desugarer): {errs:?}"))?;

    // After this pass: the `VocabBlockStmt` wrapper is gone, unwrapped directly to its embedded expression.
    let ast::Declaration::Function(function) = &program.declarations[1].node else {
        return Err("expected a function declaration at index 1 after desugaring".into());
    };
    let ast::Statement::Expr(expr) = &function.body[0].node else {
        return Err(format!(
            "expected the VocabBlock wrapper to be unwrapped to an expression statement, got {:?}",
            function.body[0].node
        )
        .into());
    };
    let ast::Expr::Embedded(fragment) = &expr.node else {
        return Err(format!("expected an embedded fragment expression, got {:?}", expr.node).into());
    };
    assert_eq!(fragment.submode, incan_vocab::EmbeddedFragmentSubmode::Markup);

    // The container survives typecheck as itself, and its hole (`{title}`, referencing the real `title`
    // parameter) gets genuine Incan expression typing -- not pre-typecheck erasure.
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(known_library_index("webkit"));
    checker
        .check_program(&program)
        .map_err(|errs| format!("typecheck errors: {errs:?}"))?;

    // Lowering + emission, through the actual `IrCodegen` pipeline `#1023` extended (`src/backend/ir/lower`,
    // `IrExprKind::EmbeddedFragment`): the fragment must reach lowering successfully (it is not a
    // `VocabBlock`/`Surface`-style contract violation) and only fail at the emission boundary, with the exact
    // honest `EmitError::Unsupported` message that node's rustdoc documents -- never a panic, and never a
    // silent guess at DSL-owned runtime semantics.
    let mut codegen = crate::IrCodegen::new();
    codegen.set_library_manifest_index(known_library_index("webkit"));
    let result = codegen.try_generate(&program);
    let Err(crate::codegen::GenerationError::Emission(emit_error)) = result else {
        return Err(format!(
            "expected lowering to succeed and only emission to refuse the embedded fragment, got: {result:?}"
        )
        .into());
    };
    let message = emit_error.to_string();
    assert!(
        message.contains("descriptor-gated embedded fragment") && message.contains("Markup"),
        "expected the documented embedded-fragment emission refusal, got: {message}"
    );

    Ok(())
}
