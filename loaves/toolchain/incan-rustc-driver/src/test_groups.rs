//! A file group: one native entry that runs a test file's cases in one process when they use module- or
//! session-scoped fixtures, so each such fixture is set up once for the file and torn down after its last case, as
//! 0.5.1's per-file harness did.
//!
//! The group runs each case the environment selects inside the native panic boundary, between one marker line before
//! it and one after it that carries its outcome and its own duration. A shared plain fixture is set up on its first
//! use and kept in a static; a shared `yield` fixture is entered, lazily, just before the first selected case that
//! needs it, as a stage around the rest of the file's cases, so its teardown runs once after the last case. The
//! runner launches the group once per file and reads each case's outcome, output and duration from its markers.
//! Session scope is once per file process, which RFC 019 allows for a worker.

use std::collections::HashSet;

use crate::test_entries::{EntryWriter, PREFIX, Stage, constructor_spellings};
use incan_driver::testing::types::{FixtureInfo, FixtureScope};
use incan_lang::lang::conventions::NONE_TYPE_NAME;

/// The group's native entry.
pub(crate) const GROUP: &str = "incan_case_group";

/// The environment variable the runner names a group's selected case entries in, separated by commas.
const SELECTED_CASES: &str = "INCAN_TEST_CASES";

/// One case a group runs: the function that runs it, the test's return type, and the shared `yield` fixtures it
/// needs, each after those it depends on.
pub(crate) struct Member {
    pub(crate) entry: String,
    pub(crate) returns: String,
    pub(crate) yields: Vec<String>,
}

/// The module- and session-scoped fixtures `reached` names, which a file group shares.
pub(crate) fn shared_fixtures(writer: &EntryWriter<'_>, reached: &[&str]) -> HashSet<String> {
    reached
        .iter()
        .filter_map(|name| writer.fixtures.get(*name).copied())
        .filter(|fixture| fixture.scope != FixtureScope::Function)
        .map(|fixture| fixture.name.clone())
        .collect()
}

/// Every fixture `roots` reach, directly or through another fixture's parameters.
pub(crate) fn reached<'a>(writer: &EntryWriter<'a>, roots: &[&'a str]) -> Vec<&'a str> {
    let mut seen: Vec<&'a str> = Vec::new();
    let mut pending: Vec<&'a str> = roots.to_vec();
    while let Some(name) = pending.pop() {
        if seen.contains(&name) {
            continue;
        }
        let Some(fixture) = writer.fixtures.get(name).copied() else {
            continue;
        };
        seen.push(name);
        pending.extend(fixture.dependencies.iter().map(String::as_str));
    }
    seen
}

/// The shared `yield` fixtures a case with these roots needs, each after the shared fixtures it depends on.
pub(crate) fn shared_yields(writer: &EntryWriter<'_>, roots: &[&str]) -> Result<Vec<String>, String> {
    Ok(shared_order(writer, roots)?
        .into_iter()
        .filter(|fixture| fixture.has_teardown)
        .map(|fixture| fixture.name.clone())
        .collect())
}

/// Write the group: each shared fixture's static and accessor, a lazily entered stage per shared `yield` fixture, the
/// entry, and the function that runs each selected case from a given position on.
pub(crate) fn write_group(writer: &EntryWriter<'_>, members: &[Member], requested: &[&str]) -> Result<String, String> {
    let order = shared_order(writer, requested)?;
    writer.uses_group();
    let mut written = String::new();
    for fixture in &order {
        written.push_str(&shared_storage(writer, fixture)?);
    }
    for fixture in order.iter().filter(|fixture| fixture.has_teardown) {
        written.push_str(&yield_entry(writer, fixture)?);
    }
    written.push_str(&function(GROUP, &[], NONE_TYPE_NAME, &[format!("{GROUP}_run(0)")]));
    written.push_str(&run_function(members));
    written.push_str(&format!(
        "\n\ndef {PREFIX}_selected(entry: str) -> bool:\n    selected = {PREFIX}_get_or(\"{SELECTED_CASES}\", \"\")\n    return selected == \"\" or entry in selected.split(\",\")\n"
    ));
    Ok(written)
}

/// A shared fixture's static and, for a plain one, the accessor that sets it up on first use.
fn shared_storage(writer: &EntryWriter<'_>, fixture: &FixtureInfo) -> Result<String, String> {
    let ty = return_type(writer, &fixture.name)?;
    let name = &fixture.name;
    let mut written = format!("\n\nstatic {PREFIX}_scope_{name}: list[{ty}] = []");
    if fixture.has_teardown {
        return Ok(written);
    }
    let mut setup = Vec::new();
    let call = writer.fixture_call(name, GROUP, &[], &mut setup, 0)?;
    let mut body = vec![format!("if len({PREFIX}_scope_{name}) == 0:")];
    body.extend(setup.iter().map(|line| format!("    {line}")));
    body.push(format!("    {PREFIX}_scope_{name}.append({call})"));
    body.push(format!("return {PREFIX}_scope_{name}[0]"));
    written.push_str(&function(&format!("{PREFIX}_shared_{name}"), &[], &ty, &body));
    Ok(written)
}

/// A shared `yield` fixture's stage, entered with the position of the first case that needs it: its setup, the
/// group's run from that position on with its value stored, then its teardown.
fn yield_entry(writer: &EntryWriter<'_>, fixture: &FixtureInfo) -> Result<String, String> {
    let name = &fixture.name;
    let ty = return_type(writer, name)?;
    let start = format!("{PREFIX}_start");
    let stage = Stage {
        name: format!("{GROUP}_enter_{name}"),
        next: format!("{GROUP}_resume_{name}"),
        returns: NONE_TYPE_NAME.to_string(),
        returns_value: false,
        asynchronous: false,
    };
    let mut written = writer.yield_stage(&stage, fixture, &[start.as_str()], &[format!("{start}: int")])?;
    written.push_str(&function(
        &format!("{GROUP}_resume_{name}"),
        &[format!("{start}: int"), format!("{name}: {ty}")],
        NONE_TYPE_NAME,
        &[
            format!("{PREFIX}_scope_{name}.append({name})"),
            format!("{GROUP}_run({start})"),
        ],
    ));
    Ok(written)
}

/// The function that runs each selected case from position `incan_case_start` on, entering any shared `yield` fixture
/// the case needs that is not set up yet, and its cases' `Err` raisers.
fn run_function(members: &[Member]) -> String {
    let start = format!("{PREFIX}_start");
    let mut run = Vec::new();
    let mut written = String::new();
    for (position, member) in members.iter().enumerate() {
        let entry = &member.entry;
        run.push(format!("if {start} <= {position} and {PREFIX}_selected(\"{entry}\"):"));
        for name in &member.yields {
            run.push(format!("    if len({PREFIX}_scope_{name}) == 0:"));
            run.push(format!("        {GROUP}_enter_{name}({position})"));
            run.push("        return".to_string());
        }
        run.extend(member_block(member, position).iter().map(|line| format!("    {line}")));
        if member.returns != NONE_TYPE_NAME {
            written.push_str(&raise_helper(position, &member.returns));
        }
    }
    written.push_str(&function(
        &format!("{GROUP}_run"),
        &[format!("{start}: int")],
        NONE_TYPE_NAME,
        &run,
    ));
    written
}

/// The lines that run one case between its markers, its failure caught and its own duration measured.
fn member_block(member: &Member, position: usize) -> Vec<String> {
    let (_, _, some, none) = constructor_spellings();
    let entry = &member.entry;
    vec![
        format!("println(\"incan-case-start {entry}\")"),
        format!("{PREFIX}_started = {PREFIX}_clock.now()"),
        format!(
            "{PREFIX}_panic = {PREFIX}_panic_text(() => {})",
            case_call(member, position)
        ),
        format!("{PREFIX}_elapsed = {PREFIX}_started.elapsed().as_millis()"),
        format!("match {PREFIX}_panic:"),
        format!(
            "    {some}({PREFIX}_message) => println(f\"incan-case-end {entry} failed {{{PREFIX}_elapsed}} {{{PREFIX}_message}}\")"
        ),
        format!("    {none} => println(f\"incan-case-end {entry} passed {{{PREFIX}_elapsed}}\")"),
    ]
}

/// The call that runs a case, turning a returned `Err` into the failure the group catches.
fn case_call(member: &Member, position: usize) -> String {
    if member.returns == NONE_TYPE_NAME {
        format!("{}()", member.entry)
    } else {
        format!("{GROUP}_raise_{position}({}())", member.entry)
    }
}

/// A helper that turns a case's `Err` into the failure the group catches.
fn raise_helper(position: usize, returns: &str) -> String {
    let (ok, err, _, _) = constructor_spellings();
    format!(
        "\n\ndef {GROUP}_raise_{position}(outcome: {returns}) -> {NONE_TYPE_NAME}:\n    match outcome:\n        {ok}(_) => pass\n        {err}(error) => {PREFIX}_fail(f\"Error: {{error:?}}\")\n"
    )
}

/// The shared fixtures in the order the cases first request them, each after the shared fixtures it depends on.
fn shared_order<'a>(writer: &EntryWriter<'a>, requested: &[&str]) -> Result<Vec<&'a FixtureInfo>, String> {
    let mut ordered: Vec<&'a FixtureInfo> = Vec::new();
    for name in requested {
        visit(writer, name, &mut ordered, 0)?;
    }
    Ok(ordered)
}

/// Add `name` after the shared fixtures it depends on, looking through fixtures that are not shared.
fn visit<'a>(
    writer: &EntryWriter<'a>,
    name: &str,
    ordered: &mut Vec<&'a FixtureInfo>,
    depth: usize,
) -> Result<(), String> {
    let Some(fixture) = writer.fixtures.get(name).copied() else {
        return Ok(());
    };
    if ordered.iter().any(|done| done.name == name) {
        return Ok(());
    }
    if depth > writer.fixtures.len() {
        return Err(format!("fixture dependency cycle through `{name}`"));
    }
    for dependency in &fixture.dependencies {
        visit(writer, dependency, ordered, depth + 1)?;
    }
    if writer.shared.contains(name) && !ordered.iter().any(|done| done.name == name) {
        ordered.push(fixture);
    }
    Ok(())
}

/// A function with the given parameters, return type and body lines.
fn function(name: &str, parameters: &[String], returns: &str, body: &[String]) -> String {
    let lines: Vec<String> = if body.is_empty() {
        vec!["    pass".to_string()]
    } else {
        body.iter().map(|line| format!("    {line}")).collect()
    };
    format!(
        "\n\ndef {name}({}) -> {returns}:\n{}\n",
        parameters.join(", "),
        lines.join("\n")
    )
}

/// A shared fixture's written return type, the type of the static that holds its value.
fn return_type(writer: &EntryWriter<'_>, name: &str) -> Result<String, String> {
    let function = writer.function(name)?;
    let written = writer
        .text(function.return_type.span.start, function.return_type.span.end)
        .trim();
    if written.is_empty() {
        return Ok(NONE_TYPE_NAME.to_string());
    }
    Ok(written.to_string())
}
