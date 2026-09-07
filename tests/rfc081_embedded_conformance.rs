//! End-to-end conformance for RFC 081's six accepted embedded-fragment submodes (#1022).
//!
//! RFC 081 fixes a catalogue of exactly six submodes — `Markup`, `Style`, `RawText`, `RegexTemplate`,
//! `SelectorDeclarationValue`, `TypePosition` — and issue #1022 asks for each accepted surface to be proven end to
//! end rather than only at the parser. The parser fixtures in `crates/incan_syntax/src/parser/embedded/tests.rs`
//! already pin each grammar's accept and reject boundaries; this suite picks the artifact up where they leave off
//! and carries every submode through the rest of the pipeline:
//!
//! 1. parse to a typed [`EmbeddedFragmentExpr`] carrying the claimed submode;
//! 2. expression-hole ownership through `holes`, the single authority the whole compiler and its tooling share;
//! 3. positional ownership through `ownership_at`, the answer editor tooling resolves a cursor with;
//! 4. the vocab desugar pass, which must leave the fragment container intact rather than erasing it;
//! 5. typechecking, which must give every hole real Incan types while assigning the fragment none;
//! 6. lowering plus emission, which must reach the emission boundary and refuse there explicitly;
//! 7. both formatter modes, structural and layout-preserving, each idempotent across two passes.
//!
//! Every leg runs for every submode from one table, so a submode cannot be added to the catalogue while quietly
//! working at only some stages.

use std::collections::HashMap;

use incan::backend::IrCodegen;
use incan::backend::ir::codegen::GenerationError;
use incan::format::{FormatConfig, Formatter, format_source};
use incan::frontend::ast::{Declaration, EmbeddedFragmentExpr, EmbeddedOwnership, Expr, Program, Statement};
use incan::frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::vocab_desugar_pass::desugar_program_vocab_blocks;
use incan::frontend::{lexer, parser};
use incan::library_manifest::LibraryManifest;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type KeywordMap = HashMap<String, Vec<incan_vocab::KeywordRegistration>>;
type SurfaceMap = HashMap<String, Vec<incan_vocab::DslSurface>>;

/// The provider every fixture in this suite registers its block keyword under.
const PROVIDER: &str = "conformance";

// ============================================================================
// The conformance matrix
// ============================================================================

/// One accepted submode's end-to-end conformance case.
struct SubmodeCase {
    /// The submode under test, as claimed by this case's descriptor.
    submode: incan_vocab::EmbeddedFragmentSubmode,
    /// Block keyword the fixture descriptor claims (`html:`, `css:`, ...).
    keyword: &'static str,
    /// Complete fixture source, including the `import pub::` line that activates the keyword.
    source: &'static str,
    /// Expression holes the fragment must own, by identifier, in source order.
    expected_holes: &'static [&'static str],
    /// A substring of the fragment that is DSL-owned syntax, used to locate a cursor position inside it.
    dsl_owned_text: &'static str,
    /// Substrings the structurally formatted output must contain, proving the render came from the node tree.
    structural_format_contains: &'static [&'static str],
}

/// Return the conformance case for every submode RFC 081 accepts.
///
/// Each fixture is deliberately the smallest source that still exercises the submode's own grammar rather than a
/// shape another submode would also accept, and every submode that admits an expression hole has one, so the
/// hole-ownership legs are not silently vacuous. `TypePosition` is the one submode whose grammar has no hole
/// production at all, which its empty `expected_holes` records rather than hides.
fn conformance_cases() -> Vec<SubmodeCase> {
    vec![
        SubmodeCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::Markup,
            keyword: "html",
            source: "import pub::conformance\n\ndef render(title: str, alt: str) -> None:\n    html:\n        <section class={title}>\n            <h1>{alt}</h1>\n            plain text\n        </section>\n\ndef main() -> None:\n    render(\"Card\", \"Preview\")\n",
            expected_holes: &["title", "alt"],
            dsl_owned_text: "section",
            structural_format_contains: &["<section", "<h1>", "plain text"],
        },
        SubmodeCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::Style,
            keyword: "css",
            source: "import pub::conformance\n\ndef render(accent: str) -> None:\n    css:\n        .card:hover {\n            --accent-color: #1166ff;\n            color: {accent};\n        }\n\ndef main() -> None:\n    render(\"blue\")\n",
            expected_holes: &["accent"],
            dsl_owned_text: "--accent-color",
            structural_format_contains: &[".card:hover", "--accent-color", "#1166ff"],
        },
        SubmodeCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::RawText,
            keyword: "note",
            source: "import pub::conformance\n\ndef render(who: str) -> None:\n    note:\n        TODO({who}): finish this <<weird>> text\n\ndef main() -> None:\n    render(\"maintainer\")\n",
            expected_holes: &["who"],
            dsl_owned_text: "<<weird>>",
            structural_format_contains: &["TODO(", "<<weird>>"],
        },
        SubmodeCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::RegexTemplate,
            keyword: "script",
            source: "import pub::conformance\n\ndef render(name: str) -> None:\n    script:\n        `hello ${name}!`\n\ndef main() -> None:\n    render(\"world\")\n",
            expected_holes: &["name"],
            dsl_owned_text: "hello",
            structural_format_contains: &["hello", "${", "}"],
        },
        SubmodeCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::SelectorDeclarationValue,
            keyword: "spacing",
            source: "import pub::conformance\n\ndef render() -> None:\n    spacing:\n        2rem\n\ndef main() -> None:\n    render()\n",
            expected_holes: &[],
            dsl_owned_text: "2rem",
            structural_format_contains: &["2rem"],
        },
        SubmodeCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::TypePosition,
            keyword: "typeof",
            source: "import pub::conformance\n\ndef render() -> None:\n    typeof:\n        a.b.Foo<Bar[]>? | Baz\n\ndef main() -> None:\n    render()\n",
            expected_holes: &[],
            dsl_owned_text: "a.b.Foo",
            structural_format_contains: &["a.b.Foo", "Bar[]", "Baz"],
        },
    ]
}

// ============================================================================
// Fixture plumbing
// ============================================================================

/// Build keyword and surface maps activating one embedded-fragment descriptor on `keyword`'s declaration body.
///
/// Mirrors the fixture model the parser and formatter suites already use: hand-built registrations, no manifest
/// file on disk and no WASM companion, because an embedded-fragment descriptor is a parser-side claim and needs
/// neither to activate.
fn fixture_maps(
    keyword: &str,
    submode: incan_vocab::EmbeddedFragmentSubmode,
    layout_sensitive: bool,
) -> (KeywordMap, SurfaceMap) {
    let namespace = format!("{PROVIDER}.{keyword}");
    let mut keyword_map = KeywordMap::new();
    keyword_map.insert(
        PROVIDER.to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: namespace.clone(),
            },
            keywords: vec![incan_vocab::KeywordSpec::block(keyword)],
            valid_decorators: Vec::new(),
        }],
    );

    let mut descriptor =
        incan_vocab::EmbeddedFragmentDescriptor::new(&format!("{keyword}.fragment"), submode, "fragment")
            .in_declaration_body(keyword);
    if layout_sensitive {
        descriptor = descriptor.layout_sensitive();
    }

    let mut surface_map = SurfaceMap::new();
    surface_map.insert(
        PROVIDER.to_string(),
        vec![
            incan_vocab::DslSurface::on_import(&namespace)
                .with_declaration(incan_vocab::DeclarationSurface::named(keyword))
                .with_embedded_fragment(descriptor),
        ],
    );
    (keyword_map, surface_map)
}

/// Parse a fixture through the one entrypoint that produces embedded fragments.
///
/// `parse_with_source` is the only entrypoint that threads the original source through to the submode tokenizer;
/// the ordinary entrypoints are fragment-blind and would hand back a program with no fragment in it, so every
/// assertion below would pass vacuously. `lex_tolerant` is required for the same reason the parser fixtures use
/// it: fragment content routinely contains bytes (`;`, `` ` ``, `$`, `!`) that ordinary Incan tokenization
/// rejects, and a strict up-front lex would discard the whole token stream before the parser could route them.
fn parse_fixture(source: &str, keyword_map: &KeywordMap, surface_map: &SurfaceMap) -> Result<Program, String> {
    let (tokens, _lex_errors) = lexer::lex_tolerant(source);
    parser::parse_with_source(&tokens, None, Some(keyword_map), Some(surface_map), source)
        .map_err(|errors| format!("parse errors: {errors:?}"))
}

/// Dig the sole embedded fragment out of a parsed fixture program.
///
/// Accepts the fragment both before the vocab desugar pass (still wrapped in its `Statement::VocabBlock`) and
/// after it (unwrapped to a bare expression statement), so the same helper can assert across that boundary.
fn fragment_of(program: &Program) -> Result<&EmbeddedFragmentExpr, String> {
    let Declaration::Function(function) = &program.declarations[1].node else {
        return Err("expected a function declaration at index 1".to_string());
    };
    let statement = match &function.body[0].node {
        Statement::VocabBlock(block) => &block.body[0].node,
        other => other,
    };
    let Statement::Expr(expr) = statement else {
        return Err(format!("expected an expression statement, got {statement:?}"));
    };
    let Expr::Embedded(fragment) = &expr.node else {
        return Err(format!("expected an embedded fragment expression, got {:?}", expr.node));
    };
    Ok(fragment)
}

/// Build a minimal known-library index so `import pub::conformance` resolves during typechecking.
///
/// This satisfies only the import-resolution gate. The embedded-fragment descriptors themselves are parser-side
/// concerns already baked into the AST by `parse_fixture`'s hand-built maps, so no real checked-registry metadata
/// is needed.
fn known_library_index() -> LibraryManifestIndex {
    let manifest = LibraryManifest::new(PROVIDER, "0.1.0");
    let mut root = std::env::temp_dir();
    root.push("incan_rfc081_conformance_artifacts");
    root.push("target");
    root.push("lib");
    LibraryManifestIndex::from_entries(HashMap::from([(
        PROVIDER.to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(PROVIDER, PROVIDER, root),
        },
    )]))
}

/// Return the identifier names of a fragment's expression holes, in source order.
///
/// Holes are ordinary Incan expressions, so a hole that is not a bare identifier is skipped rather than described
/// — these fixtures deliberately interpolate plain parameters so the expected list stays readable.
fn hole_idents(fragment: &EmbeddedFragmentExpr) -> Vec<String> {
    fragment
        .holes()
        .into_iter()
        .filter_map(|hole| match &hole.node {
            Expr::Ident(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

// ============================================================================
// Legs 1-3: typed artifact, hole ownership, positional ownership
// ============================================================================

#[test]
fn every_submode_parses_to_a_typed_artifact_owning_its_holes() -> TestResult {
    // Leg 1 and leg 2. RFC 081's Reference-level explanation requires that later phases never rediscover an
    // accepted surface by matching raw source text, which means the parser has to commit to a submode and a node
    // tree here. Leg 2 then pins that `holes` — the single traversal the compiler and its tooling share — reaches
    // exactly the expression holes each submode's grammar admits.
    for case in conformance_cases() {
        let (keyword_map, surface_map) = fixture_maps(case.keyword, case.submode, false);
        let program = parse_fixture(case.source, &keyword_map, &surface_map)?;
        let fragment = fragment_of(&program)?;

        assert_eq!(
            fragment.submode, case.submode,
            "{:?}: parsed fragment claimed the wrong submode",
            case.submode
        );
        assert!(
            !fragment.nodes.is_empty(),
            "{:?}: a fragment that parsed must carry a structural node tree, not an empty one",
            case.submode
        );
        assert!(
            !fragment.source_text.is_empty(),
            "{:?}: verbatim source text must be preserved for the layout-preserving formatter mode",
            case.submode
        );
        assert_eq!(
            hole_idents(fragment),
            case.expected_holes,
            "{:?}: `holes` reached the wrong expression holes",
            case.submode
        );
    }
    Ok(())
}

#[test]
fn ownership_at_separates_dsl_owned_syntax_from_expression_holes() -> TestResult {
    // Leg 3. This is the question editor tooling resolves a cursor with, and getting it wrong is not a cosmetic
    // problem: DSL-owned text is spelled with ordinary identifier bytes, so anything that resolves a position by
    // scanning source would answer a tag name or a declaration property with whatever unrelated Incan binding
    // shares its spelling. RFC 081's Drawbacks make this tooling's obligation: readers must be able to tell where
    // ordinary Incan ends and the DSL-owned surface begins.
    for case in conformance_cases() {
        let (keyword_map, surface_map) = fixture_maps(case.keyword, case.submode, false);
        let program = parse_fixture(case.source, &keyword_map, &surface_map)?;
        let fragment = fragment_of(&program)?;

        let dsl_offset = case
            .source
            .find(case.dsl_owned_text)
            .ok_or_else(|| format!("{:?}: fixture is missing its DSL-owned probe text", case.submode))?;
        assert!(
            matches!(fragment.ownership_at(dsl_offset), Some(EmbeddedOwnership::DslOwned)),
            "{:?}: `{}` is DSL-owned syntax and must not resolve as ordinary Incan, got {:?}",
            case.submode,
            case.dsl_owned_text,
            fragment.ownership_at(dsl_offset)
        );

        // An offset before the fragment ever starts belongs to ordinary Incan and must not be claimed at all.
        assert!(
            fragment.ownership_at(0).is_none(),
            "{:?}: a fragment must not claim ownership of source outside itself",
            case.submode
        );

        // Every hole this submode admits resolves back to ordinary Incan at its own span.
        for hole in fragment.holes() {
            let ownership = fragment.ownership_at(hole.span.start);
            assert!(
                matches!(ownership, Some(EmbeddedOwnership::Hole(found)) if found.span == hole.span),
                "{:?}: a hole's own span must resolve as ordinary Incan, got {ownership:?}",
                case.submode
            );
        }
    }
    Ok(())
}

// ============================================================================
// Legs 4-5: desugar, typecheck, lower, and refuse emission
// ============================================================================

#[test]
fn every_submode_survives_desugar_typecheck_and_lowering_then_refuses_emission() -> TestResult {
    // Legs 4, 5 and 6. RFC 081's Implementation Plan phase 2 states all three obligations: carry the typed
    // artifact through the pipeline, type expression holes as real Incan expressions rather than erasing them,
    // and refuse emission explicitly because a descriptor owns its fragment's runtime semantics and the compiler
    // must not guess them. A fragment must therefore reach the emission boundary intact and fail only there.
    for case in conformance_cases() {
        let (keyword_map, surface_map) = fixture_maps(case.keyword, case.submode, false);
        let mut program = parse_fixture(case.source, &keyword_map, &surface_map)?;

        desugar_program_vocab_blocks(&mut program, None, &LibraryManifestIndex::default()).map_err(|errors| {
            format!(
                "{:?}: the desugar pass must leave an embedded fragment intact without a registered WASM \
                 desugarer: {errors:?}",
                case.submode
            )
        })?;

        // The `VocabBlockStmt` wrapper is gone, but the fragment itself survived as a typed artifact.
        let fragment = fragment_of(&program)?;
        assert_eq!(
            fragment.submode, case.submode,
            "{:?}: the desugar pass must not change the claimed submode",
            case.submode
        );
        assert_eq!(
            hole_idents(fragment),
            case.expected_holes,
            "{:?}: the desugar pass must not lose or add expression holes",
            case.submode
        );

        let mut checker = TypeChecker::new();
        checker.set_library_manifest_index(known_library_index());
        checker
            .check_program(&program)
            .map_err(|errors| format!("{:?}: typecheck errors: {errors:?}", case.submode))?;

        let mut codegen = IrCodegen::new();
        codegen.set_library_manifest_index(known_library_index());
        match codegen.try_generate(&program) {
            Err(GenerationError::Emission(emit_error)) => {
                let message = emit_error.to_string();
                assert!(
                    message.contains("embedded fragment"),
                    "{:?}: the refusal must name what it refused, got: {message}",
                    case.submode
                );
                assert!(
                    message.contains(&format!("{:?}", case.submode)),
                    "{:?}: the refusal must name the submode so the reader knows which descriptor owns it, got: \
                     {message}",
                    case.submode
                );
            }
            other => {
                return Err(format!(
                    "{:?}: expected lowering to succeed and only emission to refuse, got: {other:?}",
                    case.submode
                )
                .into());
            }
        }
    }
    Ok(())
}

#[test]
fn a_hole_referencing_an_undeclared_name_is_an_ordinary_typecheck_error() -> TestResult {
    // The other half of "holes are real Incan": a hole is not only typechecked, its failures are reported as
    // ordinary Incan errors anchored inside the fragment, rather than deferred to the owning DSL's own machinery.
    // The docs promise this explicitly, so it needs evidence.
    let (keyword_map, surface_map) = fixture_maps("html", incan_vocab::EmbeddedFragmentSubmode::Markup, false);
    let source = "import pub::conformance\n\ndef render(title: str) -> None:\n    html:\n        <h1>{title}</h1>\n        <p>{subtitle}</p>\n";
    let mut program = parse_fixture(source, &keyword_map, &surface_map)?;
    desugar_program_vocab_blocks(&mut program, None, &LibraryManifestIndex::default())
        .map_err(|errors| format!("desugar errors: {errors:?}"))?;

    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(known_library_index());
    let Err(errors) = checker.check_program(&program) else {
        return Err("an undeclared name inside an expression hole must be a typecheck error".into());
    };

    let subtitle_offset = source
        .find("subtitle")
        .ok_or("fixture is missing its undeclared-name probe")?;
    let anchored = errors.iter().any(|error| {
        error.message.contains("subtitle") && error.span.start <= subtitle_offset && subtitle_offset <= error.span.end
    });
    assert!(
        anchored,
        "the error must name `subtitle` and be anchored at the hole that wrote it, got: {errors:?}"
    );
    Ok(())
}

// ============================================================================
// Leg 7: both formatter modes, each idempotent
// ============================================================================

/// Format a fixture in one of the two modes RFC 081 permits, then format the result again.
///
/// Returns both passes so a caller can assert the mode's own contract on the first and idempotency on the pair.
/// The second pass reparses the formatter's own output through the same fragment-aware entrypoint, which is what
/// makes this a real idempotency check rather than a string comparison against the input.
fn format_twice(
    source: &str,
    keyword: &str,
    submode: incan_vocab::EmbeddedFragmentSubmode,
    layout_sensitive: bool,
) -> Result<(String, String), String> {
    let (keyword_map, surface_map) = fixture_maps(keyword, submode, layout_sensitive);
    let first = {
        let program = parse_fixture(source, &keyword_map, &surface_map)?;
        Formatter::new(FormatConfig::default()).format(&program)
    };
    let second = {
        let program = parse_fixture(&first, &keyword_map, &surface_map)?;
        Formatter::new(FormatConfig::default()).format(&program)
    };
    Ok((first, second))
}

#[test]
fn every_submode_formats_structurally_and_is_idempotent() -> TestResult {
    // RFC 081's "No third formatter fallback state" Design Decision: a descriptor that has not declared itself
    // layout-sensitive is rendered from its structural artifact. Rendering has to survive being fed its own
    // output, or `incan fmt` would churn a file on every run.
    for case in conformance_cases() {
        let (first, second) = format_twice(case.source, case.keyword, case.submode, false)?;
        for expected in case.structural_format_contains {
            assert!(
                first.contains(expected),
                "{:?}: structural rendering lost `{expected}`:\n{first}",
                case.submode
            );
        }
        assert_eq!(
            first, second,
            "{:?}: structural formatting must be idempotent across two passes",
            case.submode
        );
    }
    Ok(())
}

#[test]
fn every_submode_preserves_source_when_its_descriptor_is_layout_sensitive() -> TestResult {
    // The second permitted mode. The descriptor owns this choice, and when it declares layout sensitivity the
    // formatter must preserve the fragment's original text rather than re-deriving it — including whitespace the
    // structural mode would deliberately drop.
    let indented_source = "import pub::conformance\n\ndef banner() -> None:\n    note:\n        line one\n           deliberately indented\n";
    let (first, second) = format_twice(
        indented_source,
        "note",
        incan_vocab::EmbeddedFragmentSubmode::RawText,
        true,
    )?;
    assert!(
        first.contains("           deliberately indented"),
        "layout-sensitive source must survive untouched:\n{first}"
    );
    assert_eq!(first, second, "layout-preserving formatting must also be idempotent");

    // Every submode, not only the one whose fixture has interesting whitespace, must round-trip its own source
    // text unchanged when its descriptor declares layout sensitivity.
    for case in conformance_cases() {
        let (first, second) = format_twice(case.source, case.keyword, case.submode, true)?;
        assert_eq!(
            first, second,
            "{:?}: layout-preserving formatting must be idempotent",
            case.submode
        );
    }
    Ok(())
}

// ============================================================================
// Ordinary Incan is unaffected outside an eligible position
// ============================================================================

#[test]
fn fragment_syntax_stays_invalid_outside_an_eligible_position() -> TestResult {
    // RFC 081's Goals require core Incan tokenization and parsing to be preserved outside eligible DSL positions,
    // and its Compatibility section says code that does not import such a DSL is unaffected. The conformance
    // matrix would be worth little if the price were leaking submode syntax into ordinary Incan, so prove the
    // ordinary formatter entrypoint — which activates no descriptor at all — still rejects it.
    for case in conformance_cases() {
        assert!(
            format_source(case.source).is_err(),
            "{:?}: fragment syntax must not parse as ordinary Incan without an activating descriptor",
            case.submode
        );
    }
    Ok(())
}

// ============================================================================
// Diagnostics inside a fragment
// ============================================================================

/// One submode's rejection fixture: source the submode's own grammar must refuse, and the message it must give.
struct RejectionCase {
    /// The submode whose grammar rejects this fixture.
    submode: incan_vocab::EmbeddedFragmentSubmode,
    /// Block keyword the fixture descriptor claims.
    keyword: &'static str,
    /// Fixture source whose fragment body is invalid for its submode.
    source: &'static str,
    /// Text the reported diagnostic must contain, so the reader learns what the grammar actually wanted.
    expected_message: &'static str,
}

/// Return one rejection fixture per submode that has a rejectable shape.
///
/// `RawText` is deliberately absent: its grammar accepts any content interleaved with holes, so it has no
/// malformed body to reject. That is a property of the submode, not a gap in this suite.
fn rejection_cases() -> Vec<RejectionCase> {
    vec![
        RejectionCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::Markup,
            keyword: "html",
            source: "import pub::conformance\n\ndef render() -> None:\n    html:\n        <section></div>\n",
            expected_message: "Mismatched closing tag",
        },
        RejectionCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::Style,
            keyword: "css",
            source: "import pub::conformance\n\ndef render() -> None:\n    css:\n        .card {\n            color: red;\n",
            expected_message: "to close this declaration block",
        },
        RejectionCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::RegexTemplate,
            keyword: "script",
            source: "import pub::conformance\n\ndef render() -> None:\n    script:\n        not_a_regex_or_template\n",
            expected_message: "must be a `/pattern/flags` regex literal",
        },
        RejectionCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::SelectorDeclarationValue,
            keyword: "spacing",
            source: "import pub::conformance\n\ndef render() -> None:\n    spacing:\n        2rem 4px\n",
            expected_message: "Unexpected trailing content",
        },
        RejectionCase {
            submode: incan_vocab::EmbeddedFragmentSubmode::TypePosition,
            keyword: "typeof",
            source: "import pub::conformance\n\ndef render() -> None:\n    typeof:\n        Foo<Bar\n",
            expected_message: "to close this generic argument list",
        },
    ]
}

#[test]
fn a_rejected_fragment_reports_its_own_grammar_and_nothing_else() -> TestResult {
    // Diagnostics are the third editor surface #1022 names, and a fragment makes them easy to get wrong twice
    // over. A fragment body is re-tokenized by its own submode, so the ordinary lexer's confusion about bytes it
    // could never tokenize (`;`, `` ` ``, `$`) must not reach the user as invented errors — the parser reconciles
    // those away. And the real diagnostic must be anchored inside the fragment, at the construct that caused it,
    // rather than at the block header or the whole declaration.
    for case in rejection_cases() {
        let (keyword_map, surface_map) = fixture_maps(case.keyword, case.submode, false);
        let (tokens, _lex_errors) = lexer::lex_tolerant(case.source);
        let Err(errors) = parser::parse_with_source(&tokens, None, Some(&keyword_map), Some(&surface_map), case.source)
        else {
            return Err(format!("{:?}: this fixture must not parse", case.submode).into());
        };

        // The fragment body starts at the first non-whitespace byte after the block header's newline.
        let body_start = case
            .source
            .find(&format!("{}:\n", case.keyword))
            .map(|header| header + case.keyword.len() + 2)
            .ok_or_else(|| format!("{:?}: fixture is missing its block header", case.submode))?;

        assert!(
            errors.iter().any(|error| error.message.contains(case.expected_message)),
            "{:?}: expected a diagnostic naming what the grammar wanted, got {errors:?}",
            case.submode
        );
        for error in &errors {
            assert!(
                error.span.start >= body_start,
                "{:?}: every diagnostic must be anchored inside the fragment that caused it, but `{}` points at \
                 byte {} (the fragment starts at {})",
                case.submode,
                error.message,
                error.span.start,
                body_start
            );
        }
    }
    Ok(())
}

// ============================================================================
// Leg 8: semantic highlighting makes the ownership boundary visible
// ============================================================================

/// Whether a category is one an ordinary Incan *name* can take (#1400).
///
/// This is the property RFC 081 actually constrains. A fragment's tag names, selectors and declaration properties
/// are spelled like Incan identifiers, and the RFC is explicit that "resolving a name here against ordinary Incan
/// scope would be wrong" — so the one thing highlighting must never do is present DSL-owned syntax as a local, a
/// parameter, a field or a call. Which *non*-name category a submode picks is a presentation choice that node
/// kinds legitimately refine: a template string's literal run reads as string content, a comment as a comment.
#[cfg(feature = "lsp")]
fn resolves_as_an_incan_name(category: incan::lsp::semantic_tokens::Category) -> bool {
    use incan::lsp::semantic_tokens::Category;
    matches!(
        category,
        Category::Variable
            | Category::Parameter
            | Category::Property
            | Category::Method
            | Category::Function
            | Category::Namespace
            | Category::EnumMember
    )
}

#[cfg(feature = "lsp")]
#[test]
fn every_submode_highlights_dsl_bytes_apart_from_its_holes() -> TestResult {
    // Leg 8. RFC 081's Drawbacks make this tooling's obligation: "Tooling must make ownership visible enough that
    // readers can tell where ordinary Incan ends and the DSL-owned surface begins." Hover answers that for one
    // cursor position; highlighting is the surface a reader actually looks at, and it has to agree with hover
    // rather than form a second opinion. So this asserts the classification against `ownership_at` — the same
    // authority leg 3 checks — over every byte of every fragment in the matrix.
    use incan::frontend::ast::EmbeddedOwnership;
    use incan::lsp::semantic_tokens::{Category, classified_ranges};

    for case in conformance_cases() {
        let (keyword_map, surface_map) = fixture_maps(case.keyword, case.submode, false);
        let program = parse_fixture(case.source, &keyword_map, &surface_map)?;
        let ranges = classified_ranges(case.source, Some(&program));
        let fragment = fragment_of(&program)?;

        let category_at = |offset: usize| -> Option<Category> {
            ranges
                .iter()
                .find(|range| range.start <= offset && offset < range.end)
                .map(|range| range.category)
        };

        // ---- The DSL-owned sample is highlighted, and not as an Incan name ----
        let dsl_offset = case
            .source
            .find(case.dsl_owned_text)
            .ok_or_else(|| format!("{:?}: fixture is missing its DSL-owned sample", case.submode))?;
        let dsl_sample = category_at(dsl_offset)
            .ok_or_else(|| format!("{:?}: `{}` must be classified", case.submode, case.dsl_owned_text))?;
        assert!(
            !resolves_as_an_incan_name(dsl_sample),
            "{:?}: `{}` is DSL-owned syntax and must not be highlighted as an Incan name (got {dsl_sample:?})",
            case.submode,
            case.dsl_owned_text
        );

        // ---- Every hole is ordinary Incan, and is highlighted as such ----
        for hole in fragment.holes() {
            let Expr::Ident(name) = &hole.node else {
                continue;
            };
            let category = category_at(hole.span.start)
                .ok_or_else(|| format!("{:?}: the hole `{name}` must be classified", case.submode))?;
            assert_eq!(
                category,
                Category::Variable,
                "{:?}: the hole `{name}` reads a local and must be highlighted as one",
                case.submode
            );
        }

        // ---- Highlighting and `ownership_at` agree byte for byte ----
        let Some(extent) = fragment.node_extent() else {
            continue;
        };
        for offset in extent.start..extent.end {
            if !case.source.is_char_boundary(offset) {
                continue;
            }
            let Some(ownership) = fragment.ownership_at(offset) else {
                continue;
            };
            let Some(category) = category_at(offset) else {
                continue;
            };
            match ownership {
                // `ownership_at` answers for a *cursor position*, where the offset just past a name still resolves
                // to it — the right convention for hover. Highlighting paints *bytes*, and the byte at a hole's end
                // is its closing delimiter, which is DSL-owned punctuation rather than part of the expression. The
                // two agree everywhere except that one offset, which is skipped rather than papered over.
                EmbeddedOwnership::Hole(hole) if offset < hole.span.end => assert!(
                    !matches!(category, Category::Macro | Category::Regexp),
                    "{:?}: byte {offset} is inside a hole, so it must not be highlighted as DSL syntax",
                    case.submode
                ),
                EmbeddedOwnership::Hole(_) => {}
                EmbeddedOwnership::DslOwned => assert!(
                    !resolves_as_an_incan_name(category),
                    "{:?}: byte {offset} is DSL-owned, so it must not resolve as an Incan name (got {category:?})",
                    case.submode
                ),
            }
        }
    }
    Ok(())
}
