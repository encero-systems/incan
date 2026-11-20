//! Replay the golden lowering corpus through this lowering and report byte identity with the reference.
//!
//! Usage: `lowering-replay CORPUS [--case PREFIX] [--show N] [--oracle]`; `--oracle` runs the reference lowering
//! instead, proving the corpus reproduces its own answers. Every case's layout request and lowering calls run in
//! order; each answer is compared with the captured one: refusal text, runtime requests, census lines, published
//! unions and the whole plan's bytes. Where both sides have a plan, every function is compared by name as well. The
//! exit status is 0 only when every compared answer is identical.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use incan_lowering::corpus::{Case, Corpus, Outcome, Requirement, Step};
use incan_lowering::{
    Plan, RefusalPlanning, RuntimeRequirement, SourceInputs, UnitContinuation, UnitPlanning,
    layout_runtime_requirements, program_refusals, resume_unit_plan, unit_plan,
};

#[derive(Default)]
struct Tally {
    cases: usize,
    case_faults: usize,
    layouts: usize,
    layouts_identical: usize,
    steps: usize,
    steps_identical: usize,
    plans: usize,
    plans_identical: usize,
    functions: usize,
    functions_identical: usize,
    /// Per-declaration product files the reference wrote, and how many this lowering wrote identically.
    products: usize,
    products_identical: usize,
    reports: usize,
    reports_identical: usize,
    by_call: BTreeMap<String, (usize, usize)>,
    mismatches: Vec<String>,
}

fn requirement(value: &RuntimeRequirement) -> Requirement {
    Requirement {
        canonical: value.canonical.clone(),
        path: value.path.clone(),
        span: postcard::to_allocvec(&value.span).unwrap_or_default(),
    }
}

fn outcome(result: Result<UnitPlanning, String>, continuation: &mut Option<UnitContinuation>) -> Outcome {
    match result {
        Err(message) => Outcome::Refused(message),
        Ok(UnitPlanning::NeedsRuntime(retained, requirements)) => {
            *continuation = Some(retained);
            Outcome::NeedsRuntime(requirements.iter().map(requirement).collect())
        }
        Ok(UnitPlanning::Complete(plan)) => Outcome::Complete(
            plan.as_ref()
                .map(|plan| postcard::to_allocvec(plan).unwrap_or_default()),
        ),
    }
}

fn short(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Refused(message) => format!("refused: {message}"),
        Outcome::NeedsRuntime(requirements) => format!("needs {} runtime declarations", requirements.len()),
        Outcome::Complete(Some(plan)) => format!("plan of {} bytes", plan.len()),
        Outcome::Complete(None) => "no entry".to_owned(),
        Outcome::Census(lines) => format!("census of {} lines", lines.len()),
    }
}

/// Compare two plans function by function, by name and first occurrence.
fn compare_functions(expected: &[u8], actual: &[u8], tally: &mut Tally) {
    let Ok(expected) = postcard::from_bytes::<Plan>(expected) else {
        tally.mismatches.push("reference plan does not decode".into());
        return;
    };
    let actual: Option<Plan> = postcard::from_bytes(actual).ok();
    let mut produced: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if let Some(actual) = &actual {
        for function in &actual.functions {
            produced
                .entry(function.name.clone())
                .or_insert_with(|| postcard::to_allocvec(function).unwrap_or_default());
        }
    }
    for function in &expected.functions {
        tally.functions += 1;
        let bytes = postcard::to_allocvec(function).unwrap_or_default();
        if produced.get(&function.name) == Some(&bytes) {
            tally.functions_identical += 1;
        }
    }
}

/// Count the captured files of a directory and how many exist here with identical bytes.
fn compare_directory(corpus: &Corpus, expected: &[(String, String)], directory: &Path) -> (usize, usize) {
    let mut same = 0;
    for (name, digest) in expected {
        if let (Ok(want), Ok(have)) = (corpus.blob(digest), std::fs::read(directory.join(name))) {
            if want == have {
                same += 1;
            }
        }
    }
    (expected.len(), same)
}

/// A private directory holding one step's restored products and its fresh report.
struct StepDirectories {
    products: PathBuf,
    report: PathBuf,
}

fn step_directories(scratch: &Path, index: usize) -> Result<StepDirectories, String> {
    let root = scratch.join(format!("step-{index}"));
    if root.exists() {
        std::fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
    }
    let directories = StepDirectories {
        products: root.join("products"),
        report: root.join("report"),
    };
    std::fs::create_dir_all(&directories.products).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&directories.report).map_err(|error| error.to_string())?;
    Ok(directories)
}

fn inputs(corpus: &Corpus, step: &Step, directories: &StepDirectories) -> Result<SourceInputs, String> {
    let scalar_products = match &step.scalar_products {
        Some(_) => {
            corpus.restore(&step.products_before, &directories.products)?;
            Some(directories.products.to_string_lossy().into_owned())
        }
        None => None,
    };
    Ok(SourceInputs {
        sources: corpus.texts(&step.sources)?,
        files: step.files.clone(),
        tests: step.tests.clone(),
        package_versions: step.package_versions.clone(),
        overflow_checks: step.overflow_checks,
        scalar_products,
        scalar_bodies: step.scalar_bodies.clone(),
        record_report: step
            .record_report
            .as_ref()
            .map(|_| directories.report.to_string_lossy().into_owned()),
    })
}

/// The reference lowering, run on the decoded corpus inputs to prove the corpus reproduces its own answers.
fn oracle_step(
    corpus: &Corpus,
    step: &Step,
    directories: &StepDirectories,
    continuation: &mut Option<incan_mir_lowering::UnitContinuation>,
) -> Result<(Outcome, Vec<incan_semantics_core::body_ir::NativeUnionDeclaration>), String> {
    use incan_mir_lowering as reference;
    let modules = corpus.modules(&step.modules)?;
    let ours = inputs(corpus, step, directories)?;
    let inputs = reference::SourceInputs {
        sources: ours.sources,
        files: ours.files,
        tests: ours.tests,
        package_versions: ours.package_versions,
        overflow_checks: ours.overflow_checks,
        scalar_products: ours.scalar_products,
        scalar_bodies: ours.scalar_bodies,
        record_report: ours.record_report,
    };
    let requirement = |value: &reference::RuntimeRequirement| Requirement {
        canonical: value.canonical.clone(),
        path: value.path.clone(),
        span: postcard::to_allocvec(&value.span).unwrap_or_default(),
    };
    let mut unions = Vec::new();
    let planned = |result: Result<reference::UnitPlanning, String>,
                   continuation: &mut Option<reference::UnitContinuation>| match result {
        Err(message) => Outcome::Refused(message),
        Ok(reference::UnitPlanning::NeedsRuntime(retained, requirements)) => {
            *continuation = Some(retained);
            Outcome::NeedsRuntime(requirements.iter().map(requirement).collect())
        }
        Ok(reference::UnitPlanning::Complete(plan)) => Outcome::Complete(
            plan.as_ref()
                .map(|plan| postcard::to_allocvec(plan).unwrap_or_default()),
        ),
    };
    let answer = match step.call.as_str() {
        "unit" => planned(
            reference::unit_plan(&modules, &inputs, step.entry, &step.kind, &mut unions),
            continuation,
        ),
        "resume" => match continuation.take() {
            Some(retained) => planned(
                reference::resume_unit_plan(retained, &modules, &inputs, step.entry, &step.kind, &mut unions),
                continuation,
            ),
            None => Outcome::Refused("replay: no retained continuation to resume".into()),
        },
        "census" => {
            match reference::program_refusals(
                &modules,
                &inputs,
                step.entry,
                step.progress,
                &step.selection,
                &step.kind,
            ) {
                reference::RefusalPlanning::Complete(lines) => Outcome::Census(lines),
                reference::RefusalPlanning::NeedsRuntime(requirements) => {
                    Outcome::NeedsRuntime(requirements.iter().map(requirement).collect())
                }
            }
        }
        other => return Err(format!("unknown call {other}")),
    };
    Ok((answer, unions))
}

fn replay_step(
    corpus: &Corpus,
    step: &Step,
    directories: &StepDirectories,
    continuation: &mut Option<UnitContinuation>,
) -> Result<(Outcome, Vec<incan_semantics_core::body_ir::NativeUnionDeclaration>), String> {
    let modules = corpus.modules(&step.modules)?;
    let inputs = inputs(corpus, step, directories)?;
    let mut unions = Vec::new();
    let answer = match step.call.as_str() {
        "unit" => outcome(
            unit_plan(&modules, &inputs, step.entry, &step.kind, &mut unions),
            continuation,
        ),
        "resume" => match continuation.take() {
            Some(retained) => outcome(
                resume_unit_plan(retained, &modules, &inputs, step.entry, &step.kind, &mut unions),
                continuation,
            ),
            None => Outcome::Refused("replay: no retained continuation to resume".into()),
        },
        "census" => match program_refusals(
            &modules,
            &inputs,
            step.entry,
            step.progress,
            &step.selection,
            &step.kind,
        ) {
            RefusalPlanning::Complete(lines) => Outcome::Census(lines),
            RefusalPlanning::NeedsRuntime(requirements) => {
                Outcome::NeedsRuntime(requirements.iter().map(requirement).collect())
            }
        },
        other => return Err(format!("unknown call {other}")),
    };
    Ok((answer, unions))
}

fn replay_case(
    corpus: &Corpus,
    name: &str,
    case: &Case,
    scratch: &Path,
    oracle: bool,
    tally: &mut Tally,
) -> Result<(), String> {
    tally.cases += 1;
    if !case.faults.is_empty() {
        tally.case_faults += 1;
        tally
            .mismatches
            .push(format!("{name}: capture faults {:?}", case.faults));
    }
    if !case.layout_modules.is_empty() {
        tally.layouts += 1;
        let modules = corpus.modules(&case.layout_modules)?;
        let actual: Vec<Requirement> = if oracle {
            incan_mir_lowering::layout_runtime_requirements(&modules, case.layout_file.clone())
                .iter()
                .map(|value| Requirement {
                    canonical: value.canonical.clone(),
                    path: value.path.clone(),
                    span: postcard::to_allocvec(&value.span).unwrap_or_default(),
                })
                .collect()
        } else {
            layout_runtime_requirements(&modules, case.layout_file.clone())
                .iter()
                .map(requirement)
                .collect()
        };
        if actual == case.layout {
            tally.layouts_identical += 1;
        } else {
            tally.mismatches.push(format!("{name}: layout requests differ"));
        }
    }
    let mut continuation = None;
    let mut reference_continuation = None;
    for (index, step) in case.steps.iter().enumerate() {
        tally.steps += 1;
        let directories = step_directories(scratch, index)?;
        let (actual, unions) = if oracle {
            oracle_step(corpus, step, &directories, &mut reference_continuation)?
        } else {
            replay_step(corpus, step, &directories, &mut continuation)?
        };
        let identical = actual == step.outcome && unions == step.unions;
        if step.scalar_products.is_some() {
            let (expected, same) = compare_directory(corpus, &step.products_after, &directories.products);
            tally.products += expected;
            tally.products_identical += same;
        }
        if step.record_report.is_some() {
            let (expected, same) = compare_directory(corpus, &step.report_after, &directories.report);
            tally.reports += expected;
            tally.reports_identical += same;
        }
        let entry = tally.by_call.entry(step.call.clone()).or_default();
        entry.0 += 1;
        if identical {
            entry.1 += 1;
            tally.steps_identical += 1;
        } else {
            tally.mismatches.push(format!(
                "{name} step {index} ({}): expected {}; got {}",
                step.call,
                short(&step.outcome),
                short(&actual)
            ));
        }
        if let Outcome::Complete(Some(expected)) = &step.outcome {
            tally.plans += 1;
            match &actual {
                Outcome::Complete(Some(produced)) => {
                    if produced == expected {
                        tally.plans_identical += 1;
                    }
                    compare_functions(expected, produced, tally);
                }
                _ => compare_functions(expected, &[], tally),
            }
        }
    }
    Ok(())
}

/// Print one case's calls and the shape of each answer without lowering anything.
fn describe_case(corpus: &Corpus, name: &str, case: &Case) {
    println!(
        "case {name}: layout file {}; {} layout requests; {} calls",
        case.layout_file,
        case.layout.len(),
        case.steps.len()
    );
    for (index, step) in case.steps.iter().enumerate() {
        let functions = match &step.outcome {
            Outcome::Complete(Some(bytes)) => postcard::from_bytes::<Plan>(bytes)
                .map(|plan| {
                    format!(
                        "{} functions, {} models, {} enums, {} externals",
                        plan.functions.len(),
                        plan.models.len(),
                        plan.enums.len(),
                        plan.externals.len()
                    )
                })
                .unwrap_or_else(|error| format!("undecodable plan: {error}")),
            _ => String::new(),
        };
        let bodies: usize = corpus
            .modules(&step.modules)
            .map(|modules| modules.iter().map(|module| module.bodies.len()).sum())
            .unwrap_or(0);
        println!(
            "  {index} {} kind {} entry {} modules {} bodies {} products {} before {} after; report {} files; scalar bodies {}: {} {}",
            step.call,
            step.kind,
            step.entry,
            step.modules.len(),
            bodies,
            step.products_before.len(),
            step.products_after.len(),
            step.report_after.len(),
            step.scalar_bodies.len(),
            short(&step.outcome),
            functions
        );
    }
}

/// Write one case's reference plans and input modules as JSON files into `directory`, for reading by hand.
fn dump_case(corpus: &Corpus, case: &Case, directory: &Path) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    for (index, step) in case.steps.iter().enumerate() {
        if let Outcome::Complete(Some(bytes)) = &step.outcome {
            let plan: Plan = postcard::from_bytes(bytes).map_err(|error| error.to_string())?;
            let text = serde_json::to_string_pretty(&plan).map_err(|error| error.to_string())?;
            std::fs::write(directory.join(format!("step-{index}-plan.json")), text)
                .map_err(|error| error.to_string())?;
        }
        for (position, module) in corpus.modules(&step.modules)?.iter().enumerate() {
            let text = serde_json::to_string_pretty(module).map_err(|error| error.to_string())?;
            std::fs::write(directory.join(format!("step-{index}-module-{position}.json")), text)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let Some(root) = arguments.get(1) else {
        eprintln!("usage: lowering-replay CORPUS [--case PREFIX] [--show N] [--oracle]");
        std::process::exit(2);
    };
    let mut prefix = String::new();
    let mut show = 20usize;
    let mut oracle = false;
    let mut describe = false;
    let mut dump: Option<PathBuf> = None;
    let mut options = arguments[2..].iter();
    while let Some(option) = options.next() {
        if option == "--oracle" {
            oracle = true;
            continue;
        }
        if option == "--describe" {
            describe = true;
            continue;
        }
        match (option.as_str(), options.next()) {
            ("--case", Some(value)) => prefix = value.clone(),
            ("--show", Some(value)) => show = value.parse().unwrap_or(20),
            ("--dump", Some(value)) => dump = Some(PathBuf::from(value)),
            _ => {
                eprintln!("unknown option {option}");
                std::process::exit(2);
            }
        }
    }
    let corpus = Corpus::open(Path::new(root));
    let scratch = std::env::temp_dir().join(format!("lowering-replay-{}", std::process::id()));
    let mut tally = Tally::default();
    let cases = match corpus.cases() {
        Ok(cases) => cases,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    for path in cases {
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.starts_with(&prefix) {
            continue;
        }
        if let Some(directory) = &dump {
            if let Err(error) = corpus
                .case(&path)
                .and_then(|case| dump_case(&corpus, &case, &directory.join(&name)))
            {
                eprintln!("{name}: {error}");
            }
            continue;
        }
        if describe {
            if let Ok(case) = corpus.case(&path) {
                describe_case(&corpus, &name, &case);
            }
            continue;
        }
        let result = corpus
            .case(&path)
            .and_then(|case| replay_case(&corpus, &name, &case, &scratch, oracle, &mut tally));
        if let Err(error) = result {
            tally.mismatches.push(format!("{name}: replay failed: {error}"));
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    for mismatch in tally.mismatches.iter().take(show) {
        println!("MISMATCH {mismatch}");
    }
    for (call, (count, identical)) in &tally.by_call {
        println!("call {call}: {identical} of {count} identical");
    }
    println!(
        "cases {} (capture faults {}); layouts {}/{}; steps {}/{}; plans {}/{}; functions {}/{}; products {}/{}; report files {}/{}",
        tally.cases,
        tally.case_faults,
        tally.layouts_identical,
        tally.layouts,
        tally.steps_identical,
        tally.steps,
        tally.plans_identical,
        tally.plans,
        tally.functions_identical,
        tally.functions,
        tally.products_identical,
        tally.products,
        tally.reports_identical,
        tally.reports
    );
    let complete = tally.mismatches.is_empty()
        && tally.products_identical == tally.products
        && tally.reports_identical == tally.reports;
    std::process::exit(if complete { 0 } else { 1 });
}
