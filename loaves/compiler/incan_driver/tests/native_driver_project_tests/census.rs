//! Bounded direct-route measurement using the route-independent fixture contract.

use super::*;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::{Duration, Instant};
use support::behavior_fixtures::{self as fixtures, BehaviorFixture, Expectation, FixtureLayout};

/// A fixture's exclusive census class and its independently retained observed result.
#[derive(serde::Serialize)]
struct Record {
    area: String,
    name: String,
    class: String,
    observed: String,
    detail: String,
    pending_reason: Option<String>,
    formerly_multi_module: bool,
    elapsed_ms: u128,
}

/// Run a child with file-backed streams so pipe capacity cannot defeat the deadline.
fn bounded(
    command: &mut Command,
    scratch: &Path,
    deadline: Instant,
) -> Result<Option<Output>, Box<dyn std::error::Error>> {
    let stdout = tempfile::tempfile_in(scratch)?;
    let stderr = tempfile::tempfile_in(scratch)?;
    let mut child = command
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn()?;
    loop {
        if let Some(status) = child.try_wait()? {
            use std::io::{Read, Seek, SeekFrom};
            let mut stdout = stdout;
            let mut stderr = stderr;
            stdout.seek(SeekFrom::Start(0))?;
            stderr.seek(SeekFrom::Start(0))?;
            let mut out = Vec::new();
            let mut err = Vec::new();
            stdout.read_to_end(&mut out)?;
            stderr.read_to_end(&mut err)?;
            return Ok(Some(Output {
                status,
                stdout: out,
                stderr: err,
            }));
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Normalize named unsupported constructs while discarding fixture-specific source coordinates.
fn compile_failure(output: &Output) -> (String, String) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stderr.lines().chain(stdout.lines());
    for line in lines.clone() {
        if let Some(index) = line.find("unsupported ") {
            let detail = line[index..]
                .trim_end_matches(['"', '\\'])
                .split(" at ")
                .next()
                .unwrap_or(line);
            let detail = detail.split(['(', '{']).next().unwrap_or(detail).trim();
            return ("refused".into(), detail.to_string());
        }
    }
    let line = lines
        .clone()
        .find(|line| line.contains("Error:") || line.contains("error:") || line.contains("error["))
        .or_else(|| lines.find(|line| !line.trim().is_empty()))
        .unwrap_or("compile failed without diagnostics");
    ("driver-error".into(), line.to_string())
}

/// Measure one original fixture without changing its source or its expected observables.
fn measure(
    fixture: &BehaviorFixture,
    scratch: &Path,
    driver: &Path,
    sysroot: &Path,
    closure: &corpus::NativeClosure,
    timeout: Duration,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    if let Expectation::Refused { diagnostics } = &fixture.header.expectation {
        fixtures::materialize(fixture, scratch)?;
        let mut check =
            support::cli_project::configured_incan_command(scratch, &["check", "src/main.incn", "--format", "json"]);
        let detail = match bounded(&mut check, scratch, deadline)? {
            Some(output) => fixtures::compare_refusal(diagnostics, &output)
                .err()
                .unwrap_or_else(|| "declared diagnostics verified".into()),
            None => "check timeout".into(),
        };
        return Ok(("check-only".into(), detail));
    }
    let source = if fixture.layout == FixtureLayout::SingleFile {
        fixture.path.clone()
    } else {
        fixtures::materialize(fixture, scratch)?;
        let projects = std::iter::once(scratch.to_path_buf())
            .chain(fixture.providers.iter().map(|provider| scratch.join(&provider.path)));
        for project in projects {
            let manifest = oven_model::manifest::ProjectManifest::load(&project.join("loaf.toml"))?;
            if let Some((name, dependency)) = manifest.rust_dependencies().iter().min_by_key(|(name, _)| *name) {
                let source = match dependency.source {
                    oven_model::manifest::DependencySource::Registry => "registry",
                    oven_model::manifest::DependencySource::Path { .. } => "native path",
                    oven_model::manifest::DependencySource::Git { .. } => "native Git",
                };
                return Ok((
                    "refused".into(),
                    format!("unsupported source {source} dependency `{name}` on the native route"),
                ));
            }
        }
        for provider in &fixture.providers {
            let mut bake = support::cli_project::configured_incan_command(
                &scratch.join(&provider.path),
                &["oven", "bake", "--project", "."],
            );
            support::configure_explicit_oven_bake_command(&mut bake)?;
            let Some(output) = bounded(&mut bake, scratch, deadline)? else {
                return Ok((
                    "driver-error".into(),
                    format!("local library {} preparation timeout", provider.name),
                ));
            };
            if !output.status.success() {
                return Ok(compile_failure(&output));
            }
        }
        scratch.join("src/main.incn")
    };
    let binary = scratch.join("native");
    let mut command = corpus::source_command(driver, &source, &binary, sysroot, closure);
    command.current_dir(scratch);
    let Some(output) = bounded(&mut command, scratch, deadline)? else {
        return Ok(("driver-error".into(), "compile timeout".into()));
    };
    if !output.status.success() {
        return Ok(compile_failure(&output));
    }
    let Some(output) = bounded(Command::new(binary).current_dir(scratch), scratch, deadline)? else {
        return Ok(("wrong".into(), "binary timeout".into()));
    };
    let Expectation::Run {
        stdout,
        stderr_contains,
        exit_code,
    } = &fixture.header.expectation
    else {
        return Err("run fixture lost its expectation".into());
    };
    Ok(
        match fixtures::compare_run(stdout, stderr_contains, *exit_code, &output) {
            Ok(()) => ("pass".into(), String::new()),
            Err(detail) => ("wrong".into(), detail),
        },
    )
}

/// Discover every area with the shared strict parser, retaining stable area/name ordering.
fn all_fixtures() -> Result<Vec<(String, BehaviorFixture)>, Box<dyn std::error::Error>> {
    let mut all = Vec::new();
    for area in fs::read_dir(support::fixtures_dir().join("behavior"))? {
        let area = area?;
        if area.file_type()?.is_dir() {
            let name = area.file_name().to_string_lossy().into_owned();
            for fixture in fixtures::discover(&area.path())? {
                all.push((name.clone(), fixture));
            }
        }
    }
    all.sort_by(|a, b| (&a.0, &a.1.name).cmp(&(&b.0, &b.1.name)));
    Ok(all)
}

/// Render counts, ranked refusals, and every unexpected outcome as reviewable Markdown.
fn markdown(records: &[Record]) -> String {
    let mut classes = BTreeMap::new();
    let mut areas: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    let mut refusals = BTreeMap::new();
    for record in records {
        *classes.entry(record.class.as_str()).or_insert(0usize) += 1;
        *areas.entry(&record.area).or_default().entry(&record.class).or_default() += 1;
        if record.observed == "refused" {
            *refusals.entry(record.detail.as_str()).or_insert(0usize) += 1;
        }
    }
    let mut text = format!(
        "# Direct-route fixture census\n\n{} fixtures. Pending fixtures retain their observed result in JSON; refusal ranks include pending observations. Check-only details report verification failures without attributing them to the direct route.\n\n| Class | Count |\n| --- | ---: |\n",
        records.len()
    );
    for class in [
        "pass",
        "refused",
        "wrong",
        "driver-error",
        "check-only",
        "multi-module",
        "pending",
    ] {
        text.push_str(&format!("| {class} | {} |\n", classes.get(class).copied().unwrap_or(0)));
    }
    text.push_str("\n## Areas\n\n| Area | pass | refused | wrong | driver-error | check-only | multi-module | pending |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for (area, counts) in areas {
        text.push_str(&format!("| {area} |"));
        for class in [
            "pass",
            "refused",
            "wrong",
            "driver-error",
            "check-only",
            "multi-module",
            "pending",
        ] {
            text.push_str(&format!(" {} |", counts.get(class).copied().unwrap_or(0)));
        }
        text.push('\n');
    }
    let mut ranked: Vec<_> = refusals.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    text.push_str("\n## Refusal constructs\n\n| Construct | Count |\n| --- | ---: |\n");
    for (detail, count) in ranked {
        text.push_str(&format!("| {} | {count} |\n", detail.replace('|', "\\|")));
    }
    text.push_str("\n## Former directory-fixture exclusions\n\n| Fixture | Class | Detail |\n| --- | --- | --- |\n");
    for record in records.iter().filter(|record| record.formerly_multi_module) {
        text.push_str(&format!(
            "| {}/{} | {} | {} |\n",
            record.area,
            record.name,
            record.class,
            record.detail.replace('|', "\\|")
        ));
    }
    text.push_str("\n## Wrong and driver errors\n");
    for record in records
        .iter()
        .filter(|record| matches!(record.observed.as_str(), "wrong" | "driver-error"))
    {
        text.push_str(&format!(
            "\n### {}/{} ({}, observed {})\n\n```text\n{}\n```\n",
            record.area, record.name, record.class, record.observed, record.detail
        ));
    }
    text
}

/// Use the shared baked graph, then distribute independent fixture measurements over at most the host core count.
pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    let root = graph.scratch("census")?;
    let output = std::env::var_os("INCAN_CENSUS_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("report"));
    fs::create_dir_all(&output)?;
    let closure = corpus::runtime_closure(&graph.formatting)?;
    let driver = graph.driver_binary("debug");
    let sysroot = &graph.sysroot;
    let all = all_fixtures()?;
    let workers = std::thread::available_parallelism()?.get();
    let timeout = Duration::from_secs(
        std::env::var("INCAN_CENSUS_TIMEOUT_SECONDS")
            .ok()
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or(60),
    );
    let next = std::sync::atomic::AtomicUsize::new(0);
    let records = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            handles.push(scope.spawn(|| -> Result<Vec<Record>, String> {
                let mut records = Vec::new();
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((area, fixture)) = all.get(index) else {
                        break;
                    };
                    let start = Instant::now();
                    let scratch = tempfile::tempdir_in(&root).map_err(|error| error.to_string())?;
                    let (observed, detail) = measure(fixture, scratch.path(), &driver, sysroot, &closure, timeout)
                        .unwrap_or_else(|error| ("driver-error".into(), error.to_string()));
                    let class = if fixture.header.pending.is_some() {
                        "pending".into()
                    } else {
                        observed.clone()
                    };
                    records.push(Record {
                        area: area.clone(),
                        name: fixture.name.clone(),
                        class,
                        observed,
                        detail,
                        pending_reason: fixture.header.pending.clone(),
                        formerly_multi_module: fixture.layout != FixtureLayout::SingleFile
                            && fixture.header.pending.is_none()
                            && matches!(fixture.header.expectation, Expectation::Run { .. }),
                        elapsed_ms: start.elapsed().as_millis(),
                    });
                }
                Ok(records)
            }));
        }
        let mut records = Vec::new();
        for handle in handles {
            records.extend(handle.join().map_err(|_| "census worker panicked")??);
        }
        Ok::<_, String>(records)
    })?;
    let mut records = records;
    records.sort_by(|a, b| (&a.area, &a.name).cmp(&(&b.area, &b.name)));
    fs::write(output.join("census.json"), serde_json::to_vec_pretty(&records)?)?;
    fs::write(output.join("census.md"), markdown(&records))?;
    println!("measured {} fixtures: {}", records.len(), output.display());
    Ok(())
}

/// Different source payloads for one refused type belong to the same ranked construct.
#[cfg(unix)]
#[test]
fn refusal_groups_discard_fixture_payloads() {
    use std::os::unix::process::ExitStatusExt;
    let output = |message: &str| Output {
        status: std::process::ExitStatus::from_raw(1 << 8),
        stdout: Vec::new(),
        stderr: message.as_bytes().to_vec(),
    };
    assert_eq!(
        compile_failure(&output("Error: \"unsupported Body IR type Struct { name: one }\"")),
        compile_failure(&output("Error: \"unsupported Body IR type Struct { name: two }\""))
    );
    assert_eq!(
        compile_failure(&output("Error: checking failed: missing name")).0,
        "driver-error"
    );
}

/// File-backed capture drains large output and terminates an over-deadline executable.
#[cfg(unix)]
#[test]
fn bounded_capture_enforces_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let scratch = tempfile::tempdir()?;
    let output = bounded(
        Command::new("sh").args(["-c", "i=0; while [ $i -lt 5000 ]; do echo payload; i=$((i+1)); done"]),
        scratch.path(),
        Instant::now() + Duration::from_secs(5),
    )?
    .ok_or("capture timed out")?;
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 40000);
    assert!(
        bounded(
            Command::new("sleep").arg("5"),
            scratch.path(),
            Instant::now() + Duration::from_millis(25)
        )?
        .is_none()
    );
    Ok(())
}
