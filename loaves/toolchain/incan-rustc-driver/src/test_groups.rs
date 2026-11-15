//! A file group: one native entry that runs a test file's cases in one process when they use module- or
//! session-scoped fixtures, so each such fixture is set up once for the file and torn down after its last case, as
//! 0.5.1's per-file harness did.
//!
//! The group runs each case the environment selects inside the native panic boundary, between one marker line before
//! it and one after it that carries its outcome and its own duration. A shared plain fixture is set up on its first
//! use and kept in a static; a shared `yield` fixture is entered, lazily, just before the first selected case that
//! needs it, as a stage around the rest of the file's cases, so its teardown runs once after the last case. Async
//! cases and async shared fixtures run under the async runtime on a thread of their own, so no runtime call nests in
//! another. The runner launches the group once per file and reads each case's outcome, output and duration from its
//! markers. Session scope is once per file process, which RFC 019 allows for a worker.

use std::cell::RefCell;
use std::collections::HashSet;

use crate::test_entries::{EntryWriter, PREFIX, Stage, constructor_spellings};
use incan_driver::testing::types::{FixtureInfo, FixtureScope};
use incan_lang::lang::conventions::NONE_TYPE_NAME;

/// The group's native entry.
pub(crate) const GROUP: &str = "incan_case_group";

/// The environment variable the runner names a group's selected case entries in, separated by commas.
const SELECTED_CASES: &str = "INCAN_TEST_CASES";

/// One case a group runs: the function that runs it, the test's return type, whether that function is async, and the
/// shared `yield` fixtures it needs, each after those it depends on.
pub(crate) struct Member {
    pub(crate) entry: String,
    pub(crate) returns: String,
    pub(crate) asynchronous: bool,
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

/// Helpers that run an async call to completion, one per result type, written once each.
#[derive(Default)]
struct Blocking {
    types: RefCell<Vec<String>>,
}

impl Blocking {
    /// The helper that blocks on a future whose output is `ty`.
    fn helper(&self, ty: &str) -> String {
        let mut types = self.types.borrow_mut();
        let position = match types.iter().position(|known| known == ty) {
            Some(position) => position,
            None => {
                types.push(ty.to_string());
                types.len() - 1
            }
        };
        format!("{GROUP}_block_{position}")
    }

    /// Whether any async call runs.
    fn used(&self) -> bool {
        !self.types.borrow().is_empty()
    }

    /// Every helper's text.
    fn write(&self) -> String {
        let (ok, err, _, _) = constructor_spellings();
        let mut written = String::new();
        for (position, ty) in self.types.borrow().iter().enumerate() {
            let body = if ty == NONE_TYPE_NAME {
                vec![
                    format!("match {PREFIX}_block_on(future):"),
                    format!("    {ok}(_) => pass"),
                    format!("    {err}(error) => {PREFIX}_fail(f\"{{error}}\")"),
                ]
            } else {
                vec![
                    format!("match {PREFIX}_block_on(future):"),
                    format!("    {ok}(value) => return value"),
                    format!("    {err}(error) => return {PREFIX}_fail(f\"{{error}}\")"),
                ]
            };
            written.push_str(&function(
                &format!("{GROUP}_block_{position}[F with {PREFIX}_RuntimeFuture[{ty}]]"),
                &["future: F".to_string()],
                ty,
                &body,
            ));
        }
        written
    }
}

/// Write the group: each shared fixture's static and accessor, a lazily entered stage per shared `yield` fixture, the
/// entry, and the function that runs each selected case from a given position on.
pub(crate) fn write_group(writer: &EntryWriter<'_>, members: &[Member], requested: &[&str]) -> Result<String, String> {
    let order = shared_order(writer, requested)?;
    writer.uses_group();
    let blocking = Blocking::default();
    let mut written = String::new();
    for fixture in &order {
        written.push_str(&shared_storage(writer, fixture, &blocking)?);
    }
    if order.iter().any(|fixture| fixture.has_teardown && fixture.is_async) {
        written.push_str(&format!("\n\nstatic {PREFIX}_entering: list[int] = []"));
    }
    for fixture in order.iter().filter(|fixture| fixture.has_teardown) {
        written.push_str(&yield_entry(writer, fixture, &blocking)?);
    }
    written.push_str(&function(GROUP, &[], NONE_TYPE_NAME, &[format!("{GROUP}_run(0)")]));
    written.push_str(&run_function(writer, members, &blocking)?);
    written.push_str(&format!(
        "\n\ndef {PREFIX}_selected(entry: str) -> bool:\n    selected = {PREFIX}_get_or(\"{SELECTED_CASES}\", \"\")\n    return selected == \"\" or entry in selected.split(\",\")\n"
    ));
    written.push_str(&blocking.write());
    if blocking.used() {
        writer.uses_threads();
    }
    Ok(written)
}

/// A shared fixture's static and, for a plain one, the accessor that sets it up on first use; an async plain fixture
/// is set up on a thread of its own.
fn shared_storage(writer: &EntryWriter<'_>, fixture: &FixtureInfo, blocking: &Blocking) -> Result<String, String> {
    let ty = return_type(writer, &fixture.name)?;
    let name = &fixture.name;
    let mut written = format!("\n\nstatic {PREFIX}_scope_{name}: list[{ty}] = []");
    if fixture.has_teardown {
        return Ok(written);
    }
    let mut setup = Vec::new();
    let call = writer.fixture_call(name, GROUP, &[], &mut setup, 0)?;
    let mut body = vec![format!("if len({PREFIX}_scope_{name}) == 0:")];
    if fixture.is_async {
        let (ok, err, _, _) = constructor_spellings();
        let future = call.strip_prefix("await ").unwrap_or(&call);
        setup.push(format!("return {}({future})", blocking.helper(&ty)));
        written.push_str(&function(&format!("{GROUP}_setup_{name}"), &[], &ty, &setup));
        body.push(format!(
            "    match {PREFIX}_spawn(() => {GROUP}_setup_{name}()).join():"
        ));
        body.push(format!("        {ok}(value) => {PREFIX}_scope_{name}.append(value)"));
        body.push(format!(
            "        {err}(_) => {PREFIX}_fail(\"fixture `{name}` did not set up\")"
        ));
    } else {
        body.extend(setup.iter().map(|line| format!("    {line}")));
        body.push(format!("    {PREFIX}_scope_{name}.append({call})"));
    }
    body.push(format!("return {PREFIX}_scope_{name}[0]"));
    written.push_str(&function(&format!("{PREFIX}_shared_{name}"), &[], &ty, &body));
    Ok(written)
}

/// A shared `yield` fixture's stage, entered with the position of the first case that needs it: its setup, the
/// group's run from that position on with its value stored, then its teardown. An async one is entered on a thread of
/// its own, which reads that position from a static.
fn yield_entry(writer: &EntryWriter<'_>, fixture: &FixtureInfo, blocking: &Blocking) -> Result<String, String> {
    let name = &fixture.name;
    let ty = return_type(writer, name)?;
    let start = format!("{PREFIX}_start");
    let stage = Stage {
        name: format!("{GROUP}_enter_{name}"),
        next: format!("{GROUP}_resume_{name}"),
        returns: NONE_TYPE_NAME.to_string(),
        returns_value: false,
        asynchronous: fixture.is_async,
        awaits_next: false,
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
    if fixture.is_async {
        let (ok, err, _, _) = constructor_spellings();
        let block = blocking.helper(NONE_TYPE_NAME);
        written.push_str(&function(
            &format!("{GROUP}_enter_{name}_blocking"),
            &[],
            NONE_TYPE_NAME,
            &[format!(
                "{block}({GROUP}_enter_{name}({PREFIX}_entering[len({PREFIX}_entering) - 1]))"
            )],
        ));
        written.push_str(&function(
            &format!("{GROUP}_enter_{name}_threaded"),
            &[format!("{start}: int")],
            NONE_TYPE_NAME,
            &[
                format!("{PREFIX}_entering.append({start})"),
                format!("match {PREFIX}_spawn(() => {GROUP}_enter_{name}_blocking()).join():"),
                format!("    {ok}(_) => pass"),
                format!("    {err}(_) => {PREFIX}_fail(\"fixture `{name}` did not finish\")"),
            ],
        ));
    }
    Ok(written)
}

/// The function that runs each selected case from position `incan_case_start` on, entering any shared `yield` fixture
/// the case needs that is not set up yet, and the helpers its async cases run through.
fn run_function(writer: &EntryWriter<'_>, members: &[Member], blocking: &Blocking) -> Result<String, String> {
    let start = format!("{PREFIX}_start");
    let mut run = Vec::new();
    let mut written = String::new();
    for (position, member) in members.iter().enumerate() {
        let entry = &member.entry;
        run.push(format!("if {start} <= {position} and {PREFIX}_selected(\"{entry}\"):"));
        for name in &member.yields {
            let asynchronous = writer.fixtures.get(name).is_some_and(|fixture| fixture.is_async);
            let enter = if asynchronous {
                format!("{GROUP}_enter_{name}_threaded")
            } else {
                format!("{GROUP}_enter_{name}")
            };
            run.push(format!("    if len({PREFIX}_scope_{name}) == 0:"));
            run.push(format!("        {enter}({position})"));
            run.push("        return".to_string());
        }
        run.extend(member_block(member, position).iter().map(|line| format!("    {line}")));
        written.push_str(&member_helpers(member, position, blocking));
    }
    written.push_str(&function(
        &format!("{GROUP}_run"),
        &[format!("{start}: int")],
        NONE_TYPE_NAME,
        &run,
    ));
    Ok(written)
}

/// The lines that run one case between its markers, its failure caught and its own duration measured.
fn member_block(member: &Member, position: usize) -> Vec<String> {
    let (_, _, some, none) = constructor_spellings();
    let entry = &member.entry;
    let outcome = if member.asynchronous {
        format!("{GROUP}_threaded_{position}()")
    } else {
        format!("{PREFIX}_panic_text(() => {})", case_call(member, position))
    };
    vec![
        format!("println(\"incan-case-start {entry}\")"),
        format!("{PREFIX}_started = {PREFIX}_clock.now()"),
        format!("{PREFIX}_panic = {outcome}"),
        format!("{PREFIX}_elapsed = {PREFIX}_started.elapsed().as_millis()"),
        format!("match {PREFIX}_panic:"),
        format!(
            "    {some}({PREFIX}_message) => println(f\"incan-case-end {entry} failed {{{PREFIX}_elapsed}} {{{PREFIX}_message}}\")"
        ),
        format!("    {none} => println(f\"incan-case-end {entry} passed {{{PREFIX}_elapsed}}\")"),
    ]
}

/// The call that runs a synchronous case, turning a returned `Err` into the failure the group catches.
fn case_call(member: &Member, position: usize) -> String {
    if member.returns == NONE_TYPE_NAME {
        format!("{}()", member.entry)
    } else {
        format!("{GROUP}_raise_{position}({}())", member.entry)
    }
}

/// A case's helpers: its `Err` raiser, and for an async case the functions that run it to completion on a thread of
/// its own inside the panic boundary.
fn member_helpers(member: &Member, position: usize, blocking: &Blocking) -> String {
    let (ok, err, some, _) = constructor_spellings();
    let mut written = String::new();
    if member.returns != NONE_TYPE_NAME {
        written.push_str(&raise_helper(position, &member.returns));
    }
    if !member.asynchronous {
        return written;
    }
    let block = blocking.helper(&member.returns);
    let blocking_body = if member.returns == NONE_TYPE_NAME {
        format!("{block}({}())", member.entry)
    } else {
        format!("return {block}({}())", member.entry)
    };
    written.push_str(&function(
        &format!("{GROUP}_blocking_{position}"),
        &[],
        &member.returns,
        &[blocking_body],
    ));
    let guarded = if member.returns == NONE_TYPE_NAME {
        format!("{GROUP}_blocking_{position}()")
    } else {
        format!("{GROUP}_raise_{position}({GROUP}_blocking_{position}())")
    };
    written.push_str(&function(
        &format!("{GROUP}_guarded_{position}"),
        &[],
        "Option[str]",
        &[format!("return {PREFIX}_panic_text(() => {guarded})")],
    ));
    written.push_str(&function(
        &format!("{GROUP}_threaded_{position}"),
        &[],
        "Option[str]",
        &[
            format!("match {PREFIX}_spawn(() => {GROUP}_guarded_{position}()).join():"),
            format!("    {ok}(found) => return found"),
            format!("    {err}(_) => return {some}(\"the test's thread ended without an outcome\")"),
        ],
    ));
    written
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
