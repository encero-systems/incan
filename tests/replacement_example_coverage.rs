//! Tracked coverage of the real example corpus under the replacement backend.
//!
//! Every focused suite for the replacement backend passes against fixtures written to exercise one construct each.
//! That says nothing about whether the backend can run a program somebody would actually write, and the gap between
//! those two questions went unmeasured long enough that "no example executes at all" was discovered by accident
//! rather than reported. This suite exists to make that a number.
//!
//! Two numbers, because they fail for different reasons and have different owners:
//!
//! - **Represented**: the example lowers without a Body-IR `Unsupported` placeholder. A shortfall here is
//!   source-representation work (#1101).
//! - **Executed**: the example's `main` runs to a value. A shortfall here is executor work (#988 and its slice).
//!
//! Both are recorded as exact baselines rather than floors. A floor would have let the executed count sit at zero
//! forever without anything saying so — which is precisely how this went unnoticed — and a floor of zero is not an
//! assertion at all. An exact baseline makes movement in *either* direction a deliberate, reviewed event: improve
//! the backend and the suite tells you to record the new number in the same change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use incan::backend::replacement::{ProgramIo, ReplacementExecutionError, execute_free_function_with_io};
use incan::frontend::body_ir::{apply_body_ir_input_contract, build_body_ir_module_v0};
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};

/// Examples that lower to fully supported Body IR today. Update this in the same change that moves it.
///
/// The prior 61 count treated two modules containing explicit `Unsupported` placeholders as represented. They are
/// intentionally excluded: a placeholder is the compiler's proof that the source was *not* represented for a
/// consumer, not a successful lowering result. RFC 120's checked-identity integration then exposed ten more examples
/// whose resolved callable or type-member references never had a Body-IR value/place representation; the old 59
/// baseline counted those silent omissions as represented. They remain source-representation work under #1101.
const REPRESENTED_BASELINE: usize = 49;

/// Number of committed example sources included in this fixed corpus.
///
/// Keeping the denominator explicit makes additions/removals reviewed coverage events rather than silently changing
/// the percentage while the representation and execution baselines happen to stay the same.
const EXAMPLE_SOURCE_BASELINE: usize = 68;

/// Examples whose `main` executes today. Update this in the same change that moves it.
///
/// It moved from zero to four when `print` gained a represented builtin identity and an executed implementation:
/// 25 of the 68 examples had been stopping at their first call.
/// Hashed set membership then admitted `examples/advanced/membership_ops.incn`, raising execution to five.
/// Canonical string helpers then admitted `examples/simple/strings.incn`, raising execution to six.
/// Checked scalar conversions admit `examples/advanced/type_conversions.incn`, raising execution to seven.
///
/// Remaining model/default profiles are tracked under #1250; #989 owns imports and multi-module execution. The
/// selected #1256 string helpers are no longer a blocker for the committed strings example. Repeated-binding
/// limitations remain dependent on RFC 120's Slice 5 identity-keyed resolution; the frontend's #1072 reassignment
/// repair does not replace Body IR's interim flat name-to-local map.
///
/// #1252 owns the other half of the problem, and it is the one that decides what this number is worth: the corpus
/// covers roughly a third of the capability surface the v0.5 catalogue documents, so reaching 68 here would still
/// leave `if let`, generators, iterator adapters, value enums and most of the standard library unexecuted. Both
/// sit under Slice 1 (#1137), because execution evidence has to be trustworthy before anything is cut over to it.
const EXECUTED_BASELINE: usize = 7;

/// How far one example got through the replacement pipeline.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    /// Ran to a value.
    Executed,
    /// Lowered to fully supported Body IR but refused during direct execution, with a typed refusal category.
    RepresentedNotExecuted(String),
    /// Did not reach Body IR. Covers imports and other multi-module shapes a single file cannot resolve.
    NotRepresented(String),
}

/// Collect every committed example source, skipping build output.
fn example_sources() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                walk(&path, found)?;
            } else if path.extension().is_some_and(|ext| ext == "incn") {
                found.push(path);
            }
        }
        Ok(())
    }

    let mut found = Vec::new();
    walk(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").as_path(),
        &mut found,
    )?;
    found.sort();
    Ok(found)
}

/// Run one example as far through the replacement pipeline as it will go.
fn classify(source_path: &Path) -> Result<Outcome, Box<dyn std::error::Error>> {
    let source = std::fs::read_to_string(source_path)?;
    let Ok(tokens) = lexer::lex(&source) else {
        return Ok(Outcome::NotRepresented("lex".to_string()));
    };
    let Ok(program) = parser::parse(&tokens) else {
        return Ok(Outcome::NotRepresented("parse".to_string()));
    };
    let Ok(program) = apply_body_ir_input_contract(program, source_path) else {
        return Ok(Outcome::NotRepresented("input contract".to_string()));
    };

    let module_path = vec!["example_coverage".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    if checker.check_program(&program).is_err() {
        return Ok(Outcome::NotRepresented("typecheck".to_string()));
    }

    let module = build_body_ir_module_v0(&program, &module_path, checker.type_info());
    if module.render_snapshot().contains("unsupported(") {
        return Ok(Outcome::NotRepresented("Body IR refusal".to_string()));
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    match execute_free_function_with_io(&module, "main", &[], &mut io) {
        Ok(_) => Ok(Outcome::Executed),
        Err(error) => Ok(Outcome::RepresentedNotExecuted(refusal_bucket(&error))),
    }
}

/// Reduce a typed refusal to a stable category without parsing `Debug` output or source-span text.
fn refusal_bucket(error: &ReplacementExecutionError) -> String {
    match error {
        // A library module has no entrypoint to run. Keep it in the denominator, but do not call its absence an
        // executor defect.
        ReplacementExecutionError::MissingFunction { .. } => "no `main` (library module, not a defect)",
        ReplacementExecutionError::ArgumentCount { .. } => "entrypoint argument contract",
        ReplacementExecutionError::Unsupported { .. } => "unsupported direct replacement profile",
        ReplacementExecutionError::RuntimeFailure { .. } => "direct replacement runtime failure",
        ReplacementExecutionError::ProgramIo { .. } => "program stream failure",
        ReplacementExecutionError::ProviderAuthorityDenied { .. } => "provider authority denied",
        ReplacementExecutionError::ProviderOperationFailed { .. } => "provider operation failed",
    }
    .to_string()
}

#[test]
fn replacement_example_corpus_coverage_does_not_regress() -> Result<(), Box<dyn std::error::Error>> {
    let sources = example_sources()?;
    assert!(!sources.is_empty(), "the example corpus must not be empty");
    assert_eq!(
        sources.len(),
        EXAMPLE_SOURCE_BASELINE,
        "the committed example denominator moved from {EXAMPLE_SOURCE_BASELINE}; record that intentional corpus change"
    );

    let mut executed = 0usize;
    let mut represented = 0usize;
    let mut buckets: BTreeMap<String, usize> = BTreeMap::new();

    for source_path in &sources {
        match classify(source_path)? {
            Outcome::Executed => {
                executed += 1;
                represented += 1;
            }
            Outcome::RepresentedNotExecuted(bucket) => {
                represented += 1;
                *buckets.entry(bucket).or_default() += 1;
            }
            Outcome::NotRepresented(stage) => {
                *buckets.entry(format!("not represented ({stage})")).or_default() += 1;
            }
        }
    }

    println!("replacement backend coverage over {} committed examples", sources.len());
    println!("  represented (lowers without a Body-IR refusal): {represented}");
    println!("  executed (main runs to a value): {executed}");
    for (bucket, count) in &buckets {
        println!("  blocked by {bucket}: {count}");
    }

    assert_eq!(
        represented, REPRESENTED_BASELINE,
        "representation coverage moved to {represented} from the recorded {REPRESENTED_BASELINE}. If it went up, \
         record the new number here in the same change. If it went down, say why before doing so."
    );
    assert_eq!(
        executed, EXECUTED_BASELINE,
        "execution coverage moved to {executed} from the recorded {EXECUTED_BASELINE}. If it went up, record the \
         new number here in the same change — that is the number this suite exists to move."
    );
    Ok(())
}
