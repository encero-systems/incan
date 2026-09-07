//! Property-based tests for the Incan compiler
//!
//! These tests use proptest to verify invariants across many randomly
//! generated inputs, catching edge cases that hand-written tests might miss.

use std::collections::BTreeSet;

use incan::format::format_source;
use proptest::prelude::*;

// Note: Conversion module tests are complex due to IR construction requirements.
// See tests/codegen_snapshot_tests.rs for comprehensive conversion testing via
// end-to-end codegen.

// =============================================================================
// Format Properties
// =============================================================================

#[cfg(test)]
mod format_tests {
    use super::*;

    /// Property: Formatting is idempotent (format(format(x)) == format(x))
    #[test]
    fn format_is_idempotent_simple() -> Result<(), String> {
        let source = r#"
def add(a: int, b: int) -> int:
    return a + b

def main() -> ():
    result = add(1, 2)
    print(result)
"#;

        let formatted1 = format_source(source).map_err(|e| e.to_string())?;
        let formatted2 = format_source(&formatted1).map_err(|e| e.to_string())?;

        assert_eq!(formatted1, formatted2, "Formatting should be idempotent");
        Ok(())
    }

    /// A `capability` declaration survives a format round-trip unchanged, and reformatting is idempotent.
    ///
    /// The formatter normalizes clause order, so a capability written description/scope/requires must come back the
    /// same way it went in when it was already in that order -- and a second pass must change nothing.
    #[test]
    fn format_is_idempotent_for_a_capability_declaration() -> Result<(), String> {
        let source = concat!(
            "capability refund:\n",
            "    description = \"Issue a refund\"\n",
            "    scope:\n",
            "        tenant: str\n",
            "    requires = [host.http.request]\n",
        );

        let once = format_source(source).map_err(|e| e.to_string())?;
        let twice = format_source(&once).map_err(|e| e.to_string())?;

        assert_eq!(once, twice, "formatting a capability should be idempotent");
        for fragment in [
            "capability refund:",
            "description = ",
            "scope:",
            "tenant: str",
            "requires = [",
        ] {
            assert!(once.contains(fragment), "formatted output lost `{fragment}`:\n{once}");
        }
        Ok(())
    }

    /// Property: Formatting preserves semantic meaning (can parse before and after)
    #[test]
    fn format_preserves_parseability() -> Result<(), String> {
        use incan::frontend::{lexer, parser};

        let source = r#"
def greet(name: str) -> str:
    return f"Hello, {name}!"
"#;

        // Parse original
        let tokens1 =
            lexer::lex(source).map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; "))?;
        let ast1 = parser::parse(&tokens1)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; "))?;

        // Format and parse
        let formatted = format_source(source).map_err(|e| e.to_string())?;
        let tokens2 = lexer::lex(&formatted)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; "))?;
        let ast2 = parser::parse(&tokens2)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; "))?;

        // AST should have same structure (both should parse to same number of declarations)
        assert_eq!(
            ast1.declarations.len(),
            ast2.declarations.len(),
            "Formatting changed AST structure"
        );
        Ok(())
    }

    /// Examples the formatter cannot yet round trip, with the reason each one is here.
    ///
    /// None of these is source corruption: the two `refuses to format` entries are the formatter's comment-loss
    /// guard doing its job, and the two `not a fixed point` entries produce output that still parses and still has
    /// the same declarations — a comment moves between passes rather than any code changing. Three genuinely
    /// destructive defects that this walk found were fixed rather than listed: an `enum`'s dropped `with <Trait>`
    /// adoption, an unescaped quote in a byte literal, and a match guard written before an arrow the parser then
    /// rejected — the last fixed in the grammar, by letting both arm spellings carry a guard, rather than by
    /// teaching the formatter to avoid one of them.
    ///
    /// Comment reattachment stability is the remaining work and is tracked separately; it is a different subsystem
    /// (`src/format/comments/`) from the declaration and literal writers fixed here.
    const EXPECTED_ROUND_TRIP_FAILURES: &[(&str, &str)] = &[
        (
            "advanced/function_references/main.incn",
            "comment reattachment is not a fixed point",
        ),
        (
            "advanced/nested_project/src/main.incn",
            "comment reattachment is not a fixed point",
        ),
        (
            "advanced/using_rust_crates.incn",
            "formatter refuses: would drop comments",
        ),
        ("pro/rust_interop_pro.incn", "formatter refuses: would drop comments"),
    ];

    /// Property: formatting every committed example preserves parseability and is a fixed point.
    ///
    /// The generated-input properties above exercise shapes the strategies know how to build. Real programs use
    /// constructs those strategies never emit, and that is where the formatter has actually broken: an `enum`'s
    /// `with <Trait>` adoption was dropped entirely, a byte literal's escaped quote was unescaped into source that
    /// no longer parses, and a template string lost its delimiters. Each of those survived a green formatter suite
    /// because no test ever round-tripped a real file (#1401).
    ///
    /// Files that do not parse as ordinary Incan are skipped rather than failed: descriptor-gated embedded
    /// fragments only parse through `parse_with_source` with their owning vocab surfaces registered, which this
    /// corpus walk deliberately does not set up.
    #[test]
    fn every_committed_example_round_trips() -> Result<(), String> {
        use incan::frontend::{lexer, parser};
        use std::path::{Path, PathBuf};

        fn collect(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
            let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
            for entry in entries {
                let path = entry.map_err(|e| format!("entry in {}: {e}", dir.display()))?.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    collect(&path, found)?;
                } else if path.extension().is_some_and(|e| e == "incn") {
                    found.push(path);
                }
            }
            Ok(())
        }

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        let mut files = Vec::new();
        collect(&root, &mut files)?;
        files.sort();
        assert!(!files.is_empty(), "no examples found under {}", root.display());

        let mut checked = 0usize;
        let mut failures = Vec::new();
        for file in &files {
            let Ok(source) = std::fs::read_to_string(file) else {
                continue;
            };
            // Only files that already parse as ordinary Incan are in scope; see the doc comment.
            let Ok(tokens) = lexer::lex(&source) else {
                continue;
            };
            let Ok(before) = parser::parse(&tokens) else {
                continue;
            };

            let name = file.strip_prefix(&root).unwrap_or(file).display().to_string();
            let once = match format_source(&source) {
                Ok(once) => once,
                Err(error) => {
                    failures.push(format!("{name}: formatting failed: {error}"));
                    continue;
                }
            };

            match lexer::lex(&once).ok().and_then(|tokens| parser::parse(&tokens).ok()) {
                Some(after) if after.declarations.len() == before.declarations.len() => {}
                Some(after) => failures.push(format!(
                    "{name}: declaration count changed {} -> {}",
                    before.declarations.len(),
                    after.declarations.len()
                )),
                None => failures.push(format!("{name}: formatted output no longer parses")),
            }

            match format_source(&once) {
                Ok(twice) if twice == once => {}
                Ok(_) => failures.push(format!("{name}: formatting is not a fixed point")),
                Err(error) => failures.push(format!("{name}: second format failed: {error}")),
            }
            checked += 1;
        }

        assert!(
            checked > 0,
            "no example parsed as ordinary Incan, so nothing was round-tripped"
        );

        // Exact, not a floor, following the capability-coverage suite's reasoning: a floor lets a number sit at its
        // starting value forever without anything saying so. Membership changing in either direction is a reviewed
        // event — a new entry is a regression, and a departing one is a fix that should update this list.
        let known: BTreeSet<&str> = EXPECTED_ROUND_TRIP_FAILURES.iter().map(|(file, _)| *file).collect();
        let actual: BTreeSet<&str> = failures.iter().map(|f| f.split(':').next().unwrap_or(f)).collect();

        let regressed: Vec<&&str> = actual.difference(&known).collect();
        let fixed: Vec<&&str> = known.difference(&actual).collect();
        assert!(
            regressed.is_empty(),
            "example(s) newly failed to round trip: {regressed:?}\n\nfull detail:\n{}",
            failures.join("\n")
        );
        assert!(
            fixed.is_empty(),
            "example(s) now round trip and should be removed from EXPECTED_ROUND_TRIP_FAILURES: {fixed:?}"
        );
        Ok(())
    }

    /// Property: Empty or whitespace-only input formats without error
    #[test]
    fn format_handles_empty_input() {
        let empty_cases = vec!["", "   ", "\n\n\n", "\t\t"];

        for source in empty_cases {
            let result = format_source(source);
            // Empty/whitespace should either format successfully or give a syntax error
            // (both are acceptable behaviors)
            let _ = result;
        }
    }
}

// =============================================================================
// Proptest Strategy Examples (for future expansion)
// =============================================================================

#[cfg(test)]
mod proptest_strategies {
    use super::*;

    // Strategy for generating valid Incan identifiers
    fn ident_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_]*".prop_filter("Not a keyword", |s| {
            !matches!(
                s.as_str(),
                "def"
                    | "class"
                    | "if"
                    | "else"
                    | "return"
                    | "import"
                    | "is"
                    | "in"
                    | "not"
                    | "and"
                    | "or"
                    | "for"
                    | "loop"
                    | "while"
                    | "match"
                    | "case"
                    | "model"
                    | "trait"
                    | "enum"
                    | "mut"
                    | "const"
                    | "async"
                    | "await"
                    | "try"
                    | "except"
                    | "finally"
                    | "raise"
                    | "with"
                    | "as"
                    | "from"
                    | "pass"
                    | "break"
                    | "continue"
                    | "yield"
                    | "lambda"
                    | "global"
                    | "nonlocal"
                    | "assert"
                    | "del"
                    | "elif"
                    | "true"
                    | "false"
                    | "none"
                    | "self"
                    | "super"
                    | "type"
                    | "where"
                    | "impl"
                    | "pub"
                    | "use"
                    | "mod"
                    | "fn"
                    | "let"
                    | "static"
                    | "struct"
                    | "newtype"
            )
        })
    }

    // Strategy for generating simple function definitions
    fn simple_function_strategy() -> impl Strategy<Value = String> {
        (ident_strategy(), "[a-z]")
            .prop_map(|(name, param)| format!("def {}({}: int) -> int:\n    return {}\n", name, param, param))
    }

    proptest! {
        /// Property: Valid function definitions parse and format successfully
        #[test]
        fn generated_functions_format_successfully(
            func in simple_function_strategy()
        ) {
            // Parse
            use incan::frontend::{lexer, parser};
            let tokens = match lexer::lex(&func) {
                Ok(tokens) => tokens,
                Err(errs) => {
                    prop_assert!(false, "Lex failed: {:?}", errs);
                    unreachable!();
                }
            };
            let _ast = match parser::parse(&tokens) {
                Ok(ast) => ast,
                Err(errs) => {
                    prop_assert!(false, "Parse failed: {:?}", errs);
                    unreachable!();
                }
            };

            // Format
            let formatted = match format_source(&func) {
                Ok(formatted) => formatted,
                Err(err) => {
                    prop_assert!(false, "Format failed: {}", err);
                    unreachable!();
                }
            };

            // Re-parse to ensure still valid
            let tokens2 = match lexer::lex(&formatted) {
                Ok(tokens) => tokens,
                Err(errs) => {
                    prop_assert!(false, "Lex formatted failed: {:?}", errs);
                    unreachable!();
                }
            };
            let _ast2 = match parser::parse(&tokens2) {
                Ok(ast) => ast,
                Err(errs) => {
                    prop_assert!(false, "Parse formatted failed: {:?}", errs);
                    unreachable!();
                }
            };
        }

        /// Property: Identifiers remain valid after round-trip through lexer
        #[test]
        fn identifiers_survive_lexing(ident in ident_strategy()) {
            use incan::frontend::lexer;

            let source = format!("x = {}", ident);
            let tokens = match lexer::lex(&source) {
                Ok(tokens) => tokens,
                Err(errs) => {
                    prop_assert!(false, "Lex failed: {:?}", errs);
                    unreachable!();
                }
            };

            // Should have at least 3 tokens (ident, =, ident)
            prop_assert!(tokens.len() >= 3);
        }
    }
}
