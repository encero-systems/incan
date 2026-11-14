//! A file group: one native entry that runs a test file's cases in one process when they use module- or
//! session-scoped fixtures, so each such fixture is set up once for the file and torn down after its last case, as
//! 0.5.1's per-file harness did.
//!
//! The group sets every shared fixture up once, a `yield` one as a stage around the rest, stores each value in a
//! static the cases read, then runs each case the environment selects inside the native panic boundary and prints
//! one marker line before and after it. The runner launches the group once per file and reads each case's outcome
//! and output from those markers. Session scope is once per file process, which RFC 019 allows for a worker.

use std::collections::HashSet;

use crate::test_entries::{EntryWriter, PREFIX, Stage, constructor_spellings};
use incan_driver::testing::types::{FixtureInfo, FixtureScope};
use incan_lang::lang::conventions::NONE_TYPE_NAME;

/// The group's native entry.
pub(crate) const GROUP: &str = "incan_case_group";

/// The environment variable the runner names a group's selected case entries in, separated by commas.
const SELECTED_CASES: &str = "INCAN_TEST_CASES";

/// One case a group runs: the function that runs it, and the test's return type.
pub(crate) struct Member {
    pub(crate) entry: String,
    pub(crate) returns: String,
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

/// Write the group: the shared fixtures' statics, an accessor per plain one that sets it up on first use, the entry,
/// a stage per shared `yield` fixture in the order the cases first request them, and the function that stores their
/// values and runs each selected case between its markers.
pub(crate) fn write_group(writer: &EntryWriter<'_>, members: &[Member], requested: &[&str]) -> Result<String, String> {
    let order = shared_order(writer, requested)?;
    for fixture in &order {
        if fixture.is_async {
            return Err(format!(
                "unsupported async {} fixture `{}` on the native harness",
                scope_name(fixture.scope),
                fixture.name
            ));
        }
    }
    writer.uses_group();
    let yields: Vec<&FixtureInfo> = order.iter().copied().filter(|fixture| fixture.has_teardown).collect();

    // ---- Statics, one per shared fixture, and an accessor per plain one ----
    let mut written = String::new();
    for fixture in &order {
        let ty = return_type(writer, &fixture.name)?;
        let name = &fixture.name;
        written.push_str(&format!("\n\nstatic {PREFIX}_scope_{name}: list[{ty}] = []"));
        if !fixture.has_teardown {
            let mut setup = Vec::new();
            let call = writer.fixture_call(name, GROUP, &[], &mut setup, 0)?;
            let mut body = vec![format!("if len({PREFIX}_scope_{name}) == 0:")];
            body.extend(setup.iter().map(|line| format!("    {line}")));
            body.push(format!("    {PREFIX}_scope_{name}.append({call})"));
            body.push(format!("return {PREFIX}_scope_{name}[0]"));
            written.push_str(&function(&format!("{PREFIX}_shared_{name}"), &[], &ty, &body));
        }
    }

    // ---- The entry and a stage per shared `yield` fixture ----
    let stage_name = |position: usize| {
        if position == yields.len() {
            format!("{GROUP}_run")
        } else {
            format!("{GROUP}_stage_{position}")
        }
    };
    written.push_str(&function(GROUP, &[], NONE_TYPE_NAME, &[format!("{}()", stage_name(0))]));
    let mut staged: Vec<&str> = Vec::new();
    let mut bound: Vec<String> = Vec::new();
    for (position, fixture) in yields.iter().enumerate() {
        let stage = Stage {
            name: stage_name(position),
            next: stage_name(position + 1),
            returns: NONE_TYPE_NAME.to_string(),
            returns_value: false,
            asynchronous: false,
        };
        written.push_str(&writer.yield_stage(&stage, fixture, &staged, &bound)?);
        staged.push(fixture.name.as_str());
        bound.push(format!("{}: {}", fixture.name, return_type(writer, &fixture.name)?));
    }

    // ---- The run: store the staged values, run each selected case ----
    let mut run: Vec<String> = staged
        .iter()
        .map(|name| format!("{PREFIX}_scope_{name}.append({name})"))
        .collect();
    for (position, member) in members.iter().enumerate() {
        run.extend(member_block(member, position));
        if member.returns != NONE_TYPE_NAME {
            written.push_str(&raise_helper(position, &member.returns));
        }
    }
    written.push_str(&function(&stage_name(yields.len()), &bound, NONE_TYPE_NAME, &run));
    written.push_str(&format!(
        "\n\ndef {PREFIX}_selected(entry: str) -> bool:\n    selected = {PREFIX}_get_or(\"{SELECTED_CASES}\", \"\")\n    return selected == \"\" or entry in selected.split(\",\")\n"
    ));
    Ok(written)
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

/// The lines that run one selected case between its markers, its failure caught and reported in the end marker.
fn member_block(member: &Member, position: usize) -> Vec<String> {
    let (_, _, some, none) = constructor_spellings();
    let entry = &member.entry;
    let call = if member.returns == NONE_TYPE_NAME {
        format!("{entry}()")
    } else {
        format!("{GROUP}_raise_{position}({entry}())")
    };
    vec![
        format!("if {PREFIX}_selected(\"{entry}\"):"),
        format!("    println(\"incan-case-start {entry}\")"),
        format!("    {PREFIX}_panic = {PREFIX}_panic_text(() => {call})"),
        format!("    match {PREFIX}_panic:"),
        format!("        {some}({PREFIX}_message) => println(f\"incan-case-end {entry} failed {{{PREFIX}_message}}\")"),
        format!("        {none} => println(\"incan-case-end {entry} passed\")"),
    ]
}

/// A helper that turns a case's `Err` into the failure the group catches.
fn raise_helper(position: usize, returns: &str) -> String {
    let (ok, err, _, _) = constructor_spellings();
    format!(
        "\n\ndef {GROUP}_raise_{position}(outcome: {returns}) -> {NONE_TYPE_NAME}:\n    match outcome:\n        {ok}(_) => pass\n        {err}(error) => {PREFIX}_fail(f\"Error: {{error:?}}\")\n"
    )
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

/// `module` or `session`.
fn scope_name(scope: FixtureScope) -> &'static str {
    match scope {
        FixtureScope::Module => "module",
        FixtureScope::Session => "session",
        FixtureScope::Function => "function",
    }
}
