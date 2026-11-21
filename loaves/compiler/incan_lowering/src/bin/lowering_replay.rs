//! Replay the golden lowering corpus through this lowering and report byte identity with the reference.
//!
//! Usage: `lowering-replay CORPUS [--case PREFIX] [--show N] [--describe] [--dump DIRECTORY] [--reuse]
//! [--base PREFIX]`. Every case's layout request and lowering calls run in order; each answer is compared with the
//! captured one: refusal text, runtime requests, census lines, published unions and the whole plan's bytes. Where both
//! sides have a plan, every function is compared by name as well. The exit status is 0 only when every compared answer
//! is identical, and a records case's final plan, published as the driver publishes it, leaves the captured publication
//! in its report, file for file. With `INCAN_ENGINE_TRACE` naming a directory, `--reuse` prints the keyed frames each
//! call restored, lowered and kept, or lowered without a product, and `--base` names the case whose bodies an edit is
//! compared with.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use incan_lowering::corpus::{Case, Corpus, Outcome, Requirement, Step};
use incan_lowering::reuse_proof::{FrameOutcomes, FrameTrace, changed_bodies, check_records_keys};
use incan_lowering::{
    Plan, RefusalPlanning, RuntimeRequirement, SourceInputs, UnitContinuation, UnitPlanning,
    layout_runtime_requirements, program_refusals, publish_plan_records, resume_unit_plan, unit_plan,
};
use sha2::{Digest, Sha256};

/// What a replay prints beyond identity: the keyed frames each call reused or lowered, and a base capture to compare
/// the edited bodies with.
struct Evidence {
    report: bool,
    base: Option<Case>,
}

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
    publications: usize,
    publications_identical: usize,
    publication_files: usize,
    publication_files_identical: usize,
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

/// A case's private product and report directories. As in the driver's request, every call of a case shares them: a
/// call's products are restored over the directory as it read them, and the report is emptied once, for the case.
struct StepDirectories {
    products: PathBuf,
    report: PathBuf,
}

fn case_directories(scratch: &Path) -> Result<StepDirectories, String> {
    let root = scratch.join("case");
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
            corpus.reset(&step.products_before, &directories.products)?;
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
    tally: &mut Tally,
    evidence: &Evidence,
) -> Result<(), String> {
    tally.cases += 1;
    let mut trace = if evidence.report { FrameTrace::open() } else { None };
    if evidence.report && trace.is_none() {
        return Err("--reuse needs INCAN_ENGINE_TRACE naming a directory".into());
    }
    if !case.faults.is_empty() {
        tally.case_faults += 1;
        tally
            .mismatches
            .push(format!("{name}: capture faults {:?}", case.faults));
    }
    if !case.layout_modules.is_empty() {
        tally.layouts += 1;
        let modules = corpus.modules(&case.layout_modules)?;
        let actual: Vec<Requirement> = layout_runtime_requirements(&modules, case.layout_file.clone())
            .iter()
            .map(requirement)
            .collect();
        if actual == case.layout {
            tally.layouts_identical += 1;
        } else {
            tally.mismatches.push(format!("{name}: layout requests differ"));
        }
    }
    let mut continuation = None;
    let directories = case_directories(scratch)?;
    let mut published: Option<(Vec<u8>, Vec<u8>)> = None;
    for (index, step) in case.steps.iter().enumerate() {
        tally.steps += 1;
        let (actual, unions) = replay_step(corpus, step, &directories, &mut continuation)?;
        if let Some(cursor) = trace.as_mut() {
            report_reuse(corpus, name, index, step, &cursor.take(), evidence.base.as_ref())?;
        }
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
            let union_note = if unions == step.unions {
                String::new()
            } else {
                let first = unions.iter().zip(&step.unions).position(|(left, right)| left != right);
                format!(
                    "; unions expected {} ({} distinct) got {}, first difference at {first:?}: expected {:?} got {:?}",
                    step.unions.len(),
                    step.unions
                        .iter()
                        .map(|union| format!("{:?}", union.ty))
                        .collect::<std::collections::BTreeSet<_>>()
                        .len(),
                    unions.len(),
                    step.unions
                        .iter()
                        .take(4)
                        .map(|union| format!("{:?}/{}", union.ty, union.rust_item))
                        .collect::<Vec<_>>(),
                    unions
                        .iter()
                        .take(4)
                        .map(|union| format!("{:?}/{}", union.ty, union.rust_item))
                        .collect::<Vec<_>>()
                )
            };
            tally.mismatches.push(format!(
                "{name} step {index} ({}): expected {}; got {}{union_note}",
                step.call,
                short(&step.outcome),
                short(&actual)
            ));
        }
        if let Outcome::Complete(Some(expected)) = &step.outcome {
            tally.plans += 1;
            match &actual {
                Outcome::Complete(Some(produced)) => {
                    if step.record_report.is_some() {
                        published = Some((expected.clone(), produced.clone()));
                    }
                    if produced == expected {
                        tally.plans_identical += 1;
                    } else {
                        write_difference(name, index, expected, produced);
                    }
                    compare_functions(expected, produced, tally);
                }
                _ => compare_functions(expected, &[], tally),
            }
        }
    }
    if let Some((expected, produced)) = published {
        compare_publication(corpus, name, &expected, &produced, &directories, tally)?;
    }
    Ok(())
}

/// Publish a records case's final plan as the driver does and compare the report with the captured publication, file
/// by file: the same names and the same bytes, and nothing else.
fn compare_publication(
    corpus: &Corpus,
    name: &str,
    expected: &[u8],
    produced: &[u8],
    directories: &StepDirectories,
    tally: &mut Tally,
) -> Result<(), String> {
    let Some(captured) = corpus.publication(&hex::encode(Sha256::digest(expected)))? else {
        return Ok(());
    };
    tally.publications += 1;
    let plan: Plan = postcard::from_bytes(produced).map_err(|error| error.to_string())?;
    if let Err(refusal) = publish_plan_records(
        &plan,
        directories.products.to_string_lossy().into_owned(),
        directories.report.to_string_lossy().into_owned(),
    ) {
        tally.mismatches.push(format!("{name}: publication refused: {refusal}"));
        return Ok(());
    }
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in std::fs::read_dir(&directories.report).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_file() {
            let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
            files.push((
                path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                bytes,
            ));
        }
    }
    files.sort();
    tally.publication_files += captured.len();
    tally.publication_files_identical += captured.iter().filter(|file| files.contains(file)).count();
    if files == captured {
        tally.publications_identical += 1;
    } else {
        let names = |list: &[(String, Vec<u8>)]| list.iter().map(|(file, _)| file.clone()).collect::<Vec<_>>();
        let differing: Vec<String> = captured
            .iter()
            .filter(|file| !files.contains(file))
            .map(|(file, _)| file.clone())
            .collect();
        tally.mismatches.push(format!(
            "{name}: publication differs: captured {:?}, published {:?}, differing {differing:?}",
            names(&captured),
            names(&files)
        ));
    }
    Ok(())
}

/// Key every ordinary body of each records call as the lowering keys it and check the products the reference left.
fn check_case_records_keys(corpus: &Corpus, name: &str, case: &Case, scratch: &Path) -> Result<(), String> {
    for (index, step) in case.steps.iter().enumerate() {
        if step.record_report.is_none() || step.call != "unit" {
            continue;
        }
        let directory = scratch.join(format!("records-keys-{index}"));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).map_err(|error| error.to_string())?;
        }
        corpus.restore(&step.products_after, &directory)?;
        let modules = corpus.modules(&step.modules)?;
        let directories = StepDirectories {
            products: directory.clone(),
            report: directory.join("report"),
        };
        let inputs = inputs(corpus, step, &directories)?;
        corpus.restore(&step.products_after, &directory)?;
        let products = directory.to_string_lossy().into_owned();
        let report = check_records_keys(&modules, &inputs, step.entry, &step.kind, &products)?;
        println!(
            "RECORDS-KEYS {name} step {index}: keyed {}, stored {}, round trips {}, failures {}",
            report.keyed,
            report.stored,
            report.round_trips,
            report.failures.len()
        );
        for (label, file) in &report.stored_products {
            println!("  stored {label} {file}");
        }
        for failure in &report.failures {
            println!("  {failure}");
        }
    }
    Ok(())
}

/// Print which keyed frames one step restored or lowered and, against a base capture of the same unit, which bodies
/// changed: the evidence that an edit lowers only the functions whose Body IR changed.
fn report_reuse(
    corpus: &Corpus,
    name: &str,
    index: usize,
    step: &Step,
    outcomes: &FrameOutcomes,
    base: Option<&Case>,
) -> Result<(), String> {
    println!(
        "REUSE {name} step {index} ({}): {} keyed frames reused, {} lowered and kept, {} lowered without a product, {} restored from module pages",
        step.call,
        outcomes.reused.len(),
        outcomes.lowered.len(),
        outcomes.unkept.len(),
        outcomes.admitted.len()
    );
    for lowered in &outcomes.lowered {
        println!("  lowered {lowered}");
    }
    let Some(before) = base
        .and_then(|case| case.steps.get(index))
        .filter(|before| before.call == step.call)
    else {
        return Ok(());
    };
    let changes = changed_bodies(
        &corpus.modules(&before.modules)?,
        &before.scalar_bodies,
        &corpus.modules(&step.modules)?,
        &step.scalar_bodies,
    );
    let shifted = changes
        .iter()
        .filter(|change| change.base_start.is_some_and(|start| start != change.edited_start))
        .count();
    let witness_only = changes.iter().filter(|change| !change.body_differs).count();
    let meaning = changes.iter().filter(|change| change.meaning_differs).count();
    println!(
        "CHANGED {name} step {index}: {} bodies differ from the base ({meaning} in meaning, {shifted} moved, {witness_only} witness only)",
        changes.len()
    );
    for change in &changes {
        println!(
            "  changed {} in {} at {:?} -> {}{}",
            change.function,
            change.module,
            change.base_start,
            change.edited_start,
            if !change.body_differs {
                " (witness only)"
            } else if change.meaning_differs {
                " (meaning)"
            } else {
                " (positions only)"
            }
        );
    }
    let changed: BTreeSet<&str> = changes.iter().map(|change| change.function.as_str()).collect();
    let ordinary = |entry: &&String| entry.strip_prefix("ordinary ").map(str::to_owned);
    let kept: BTreeSet<String> = outcomes.lowered.iter().filter_map(|entry| ordinary(&entry)).collect();
    let reused: BTreeSet<String> = outcomes
        .reused
        .iter()
        .chain(&outcomes.admitted)
        .filter_map(|entry| ordinary(&entry))
        .collect();
    let kept_unchanged: Vec<&String> = kept.iter().filter(|label| !changed.contains(label.as_str())).collect();
    let changed_reused: Vec<&&str> = changed.iter().filter(|label| reused.contains(**label)).collect();
    println!(
        "KEPT-UNCHANGED {name} step {index}: {} ordinary frames lowered into a new product whose body did not change",
        kept_unchanged.len()
    );
    for label in kept_unchanged {
        println!("  {label}");
    }
    println!(
        "CHANGED-REUSED {name} step {index}: {} changed bodies restored from an earlier product",
        changed_reused.len()
    );
    for label in changed_reused {
        println!("  {label}");
    }
    Ok(())
}

/// With `LOWERING_REPLAY_DIFF` naming a directory, write a differing plan pair there as JSON for reading by hand.
fn write_difference(name: &str, index: usize, expected: &[u8], produced: &[u8]) {
    let Some(directory) = std::env::var_os("LOWERING_REPLAY_DIFF") else {
        return;
    };
    let directory = Path::new(&directory);
    for (label, bytes) in [("expected", expected), ("produced", produced)] {
        let text = postcard::from_bytes::<Plan>(bytes)
            .map_err(|error| error.to_string())
            .and_then(|plan| serde_json::to_string_pretty(&plan).map_err(|error| error.to_string()))
            .unwrap_or_else(|error| error);
        let file = directory.join(format!("{}-step-{index}-{label}.json", &name[..12.min(name.len())]));
        if let Err(error) = std::fs::write(&file, text) {
            eprintln!("{}: {error}", file.display());
        }
    }
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

/// The replay's command-line options after the corpus root.
struct Options {
    prefix: String,
    show: usize,
    describe: bool,
    dump: Option<PathBuf>,
    report: bool,
    records_keys: bool,
    base_prefix: Option<String>,
}

/// Read the options; an unknown option ends the process with status 2.
fn parse_options(arguments: &[String]) -> Options {
    let mut prefix = String::new();
    let mut show = 20usize;
    let mut describe = false;
    let mut dump: Option<PathBuf> = None;
    let mut report = false;
    let mut records_keys = false;
    let mut base_prefix: Option<String> = None;
    let mut options = arguments.iter();
    while let Some(option) = options.next() {
        if option == "--describe" {
            describe = true;
            continue;
        }
        if option == "--reuse" {
            report = true;
            continue;
        }
        if option == "--records-keys" {
            records_keys = true;
            continue;
        }
        match (option.as_str(), options.next()) {
            ("--case", Some(value)) => prefix = value.clone(),
            ("--show", Some(value)) => show = value.parse().unwrap_or(20),
            ("--dump", Some(value)) => dump = Some(PathBuf::from(value)),
            ("--base", Some(value)) => base_prefix = Some(value.clone()),
            _ => {
                eprintln!("unknown option {option}");
                std::process::exit(2);
            }
        }
    }
    Options {
        prefix,
        show,
        describe,
        dump,
        report,
        records_keys,
        base_prefix,
    }
}

/// Print the mismatches, the per-call counts and the totals.
fn print_tally(tally: &Tally, show: usize) {
    for mismatch in tally.mismatches.iter().take(show) {
        println!("MISMATCH {mismatch}");
    }
    for (call, (count, identical)) in &tally.by_call {
        println!("call {call}: {identical} of {count} identical");
    }
    println!(
        "cases {} (capture faults {}); layouts {}/{}; steps {}/{}; plans {}/{}; functions {}/{}; products {}/{}; report files {}/{}; publications {}/{}, files {}/{}",
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
        tally.reports,
        tally.publications_identical,
        tally.publications,
        tally.publication_files_identical,
        tally.publication_files
    );
}

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let Some(root) = arguments.get(1) else {
        eprintln!(
            "usage: lowering-replay CORPUS [--case PREFIX] [--show N] [--describe] [--dump DIRECTORY] [--reuse [--base PREFIX]]"
        );
        std::process::exit(2);
    };
    let Options {
        prefix,
        show,
        describe,
        dump,
        report,
        records_keys,
        base_prefix,
    } = parse_options(&arguments[2..]);
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
    let base = base_prefix.and_then(|wanted| {
        cases
            .iter()
            .find(|path| {
                path.file_stem()
                    .is_some_and(|stem| stem.to_string_lossy().starts_with(&wanted))
            })
            .and_then(|path| corpus.case(path).ok())
    });
    let evidence = Evidence { report, base };
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
        if records_keys {
            if let Err(error) = corpus
                .case(&path)
                .and_then(|case| check_case_records_keys(&corpus, &name, &case, &scratch))
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
            .and_then(|case| replay_case(&corpus, &name, &case, &scratch, &mut tally, &evidence));
        if let Err(error) = result {
            tally.mismatches.push(format!("{name}: replay failed: {error}"));
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    print_tally(&tally, show);
    let complete = tally.mismatches.is_empty()
        && tally.products_identical == tally.products
        && tally.reports_identical == tally.reports
        && tally.publications_identical == tally.publications;
    std::process::exit(if complete { 0 } else { 1 });
}
