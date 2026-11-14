//! The text of generated test entries: each case's stages, its fixtures' setup expressions, the built-in fixtures and
//! the imports the entries use (`crate::test_cases` plans them).
//!
//! Entries are written at the test file's top level, where an inline `module tests:` is hoisted for a test unit
//! (`crate::test_cases`).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use incan_driver::testing::types::{FixtureInfo, TestInfo};
use incan_frontend::ast::{Declaration, Expr, FunctionDecl, Spanned, Statement};
use incan_lang::lang::conventions::NONE_TYPE_NAME;
use incan_lang::lang::surface::constructors::{self, ConstructorId};

/// The prefix of every generated entry and local. Source names may not start with `__incan_`, which the checker
/// reserves for names it generates itself, so the source-level entries use this one.
pub(crate) const PREFIX: &str = "incan_case";

/// The runner-provided fixtures 0.5.1 injects by parameter name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Builtin {
    /// A fresh directory under the temporary directory, as the parameter's path type.
    TmpPath,
    /// A fresh directory that becomes the process's working directory.
    TmpWorkdir,
    /// `std.testing`'s environment helper, which restores what it changed when the test drops it.
    Env,
}

impl Builtin {
    /// The built-in fixture a parameter name selects.
    fn named(name: &str) -> Option<Self> {
        match name {
            "tmp_path" => Some(Self::TmpPath),
            "tmp_workdir" => Some(Self::TmpWorkdir),
            "env" => Some(Self::Env),
            _ => None,
        }
    }
}

/// What the written entries use, so the checked source imports exactly that.
#[derive(Default)]
struct Uses {
    panic_boundary: bool,
    working_directory: bool,
    /// A file group selects its cases from the environment (`crate::test_groups`).
    group: bool,
    /// One directory helper per built-in fixture and parameter type: its kind, the type's text and its name.
    directories: Vec<(Builtin, String, String)>,
}

/// One stage function of an entry: its name, the stage it hands on to, the test's return type, and whether the entry
/// awaits (an async test or an async fixture anywhere in its setup).
pub(crate) struct Stage {
    pub(crate) name: String,
    pub(crate) next: String,
    pub(crate) returns: String,
    pub(crate) returns_value: bool,
    pub(crate) asynchronous: bool,
}

impl Stage {
    /// `async def` or `def`.
    fn keyword(&self) -> &'static str {
        if self.asynchronous { "async def" } else { "def" }
    }

    /// A call, awaited in an async entry, returning the callee's outcome when the test returns one.
    fn call(&self, callee: &str, arguments: &[String], awaited: bool) -> String {
        let call = format!(
            "{}{callee}({})",
            if awaited { "await " } else { "" },
            arguments.join(", ")
        );
        if self.returns_value {
            format!("return {call}")
        } else {
            call
        }
    }
}

/// Writes entries from the checked text: fixture and test signatures, parameter values and `yield` bodies.
pub(crate) struct EntryWriter<'a> {
    pub(crate) source: &'a str,
    pub(crate) fixtures: HashMap<String, &'a FixtureInfo>,
    pub(crate) functions: HashMap<String, &'a FunctionDecl>,
    /// Module- and session-scoped fixtures a file group sets up once (`crate::test_groups`); a case reads their
    /// values.
    pub(crate) shared: HashSet<String>,
    uses: RefCell<Uses>,
    locals: Cell<usize>,
}

impl<'a> EntryWriter<'a> {
    /// A writer over the checked text and the fixtures and functions declared in it.
    pub(crate) fn new(
        source: &'a str,
        fixtures: HashMap<String, &'a FixtureInfo>,
        functions: HashMap<String, &'a FunctionDecl>,
    ) -> Self {
        Self {
            source,
            fixtures,
            functions,
            shared: HashSet::new(),
            uses: RefCell::new(Uses::default()),
            locals: Cell::new(0),
        }
    }

    /// The source text between two byte offsets.
    pub(crate) fn text(&self, start: usize, end: usize) -> &'a str {
        self.source.get(start..end).unwrap_or("")
    }

    /// A function declared where the tests are.
    pub(crate) fn function(&self, name: &str) -> Result<&'a FunctionDecl, String> {
        self.functions
            .get(name)
            .copied()
            .ok_or_else(|| format!("collected test or fixture `{name}` is not declared beside its tests"))
    }

    /// A fresh local name for a value bound before its use.
    fn local(&self) -> String {
        let next = self.locals.get();
        self.locals.set(next + 1);
        format!("{PREFIX}_value_{next}")
    }

    /// Set up `name` for one parameter of type `ty`, appending any statement it needs to `prelude`, and return the
    /// expression that passes it: a `yield` fixture's staged value, a shared fixture's value as its file group set it
    /// up, a built-in fixture, or a call that sets up the fixture's own parameters first, again for each use as 0.5.1
    /// does. A value passed to a `mut` parameter is bound to a `mut` local first.
    fn setup(
        &self,
        name: &str,
        ty: &str,
        label: &str,
        staged: &[&str],
        prelude: &mut Vec<String>,
        depth: usize,
    ) -> Result<String, String> {
        if staged.contains(&name) {
            return Ok(name.to_string());
        }
        if let Some(fixture) = self.fixtures.get(name).copied()
            && self.shared.contains(name)
        {
            // A shared `yield` fixture is set up by its group before any case; a plain one on its first use.
            return Ok(if fixture.has_teardown {
                format!("{PREFIX}_scope_{name}[0]")
            } else {
                format!("{PREFIX}_shared_{name}()")
            });
        }
        if !self.fixtures.contains_key(name) {
            return match Builtin::named(name) {
                Some(builtin) => Ok(self.builtin(builtin, ty, &format!("{label}-{name}"))),
                None => Err(format!("missing fixture `{name}`")),
            };
        }
        self.fixture_call(name, label, staged, prelude, depth)
    }

    /// A call of fixture `name` that sets up its own parameters first.
    pub(crate) fn fixture_call(
        &self,
        name: &str,
        label: &str,
        staged: &[&str],
        prelude: &mut Vec<String>,
        depth: usize,
    ) -> Result<String, String> {
        let fixture = self
            .fixtures
            .get(name)
            .copied()
            .ok_or_else(|| format!("missing fixture `{name}`"))?;
        if depth > self.fixtures.len() {
            return Err(format!("fixture dependency cycle through `{name}`"));
        }
        let mut arguments = Vec::new();
        match self.functions.get(name).copied() {
            Some(function) => {
                for parameter in &function.params {
                    let parameter_type = self.text(parameter.node.ty.span.start, parameter.node.ty.span.end);
                    let value = self.setup(&parameter.node.name, parameter_type, label, staged, prelude, depth + 1)?;
                    arguments.push(self.bound(value, parameter.node.is_mut, prelude));
                }
            }
            // A fixture another module declares is called with its collected dependencies, in parameter order.
            None => {
                for dependency in &fixture.dependencies {
                    arguments.push(self.setup(dependency, "", label, staged, prelude, depth + 1)?);
                }
            }
        }
        let awaited = if fixture.is_async { "await " } else { "" };
        Ok(format!("{awaited}{name}({})", arguments.join(", ")))
    }

    /// Pass `value` as it is, or through a `mut` local when the parameter is `mut`.
    fn bound(&self, value: String, mutable: bool, prelude: &mut Vec<String>) -> String {
        if !mutable {
            return value;
        }
        let local = self.local();
        prelude.push(format!("mut {local} = {value}"));
        local
    }

    /// The expression for a built-in fixture of parameter type `ty`, unique to `label` within the test process.
    fn builtin(&self, builtin: Builtin, ty: &str, label: &str) -> String {
        if builtin == Builtin::Env {
            return format!("{ty}.new()");
        }
        let mut uses = self.uses.borrow_mut();
        let existing = uses
            .directories
            .iter()
            .find(|(kind, written, _)| *kind == builtin && written == ty)
            .map(|(_, _, helper)| helper.clone());
        let helper = match existing {
            Some(helper) => helper,
            None => {
                let helper = format!("{PREFIX}_directory_{}", uses.directories.len());
                uses.directories.push((builtin, ty.to_string(), helper.clone()));
                helper
            }
        };
        if builtin == Builtin::TmpWorkdir {
            uses.working_directory = true;
        }
        format!("{helper}(\"{label}\")")
    }

    /// The `yield` fixtures a case uses, each after the `yield` fixtures it depends on.
    pub(crate) fn stages(&self, roots: &[&str]) -> Result<Vec<&'a FixtureInfo>, String> {
        let mut ordered: Vec<&'a FixtureInfo> = Vec::new();
        let mut pending: Vec<(&str, usize)> = roots.iter().rev().map(|root| (*root, 0)).collect();
        let mut visiting: Vec<String> = Vec::new();
        while let Some((name, depth)) = pending.pop() {
            let Some(fixture) = self.fixtures.get(name).copied() else {
                continue;
            };
            if self.shared.contains(&fixture.name) || ordered.iter().any(|staged| staged.name == fixture.name) {
                continue;
            }
            if depth > self.fixtures.len() {
                return Err(format!("fixture dependency cycle through `{name}`"));
            }
            let open = fixture
                .dependencies
                .iter()
                .filter(|dependency| {
                    self.fixtures.contains_key(dependency.as_str())
                        && !self.shared.contains(dependency.as_str())
                        && !ordered.iter().any(|staged| &staged.name == *dependency)
                })
                .collect::<Vec<_>>();
            if open.is_empty() || visiting.contains(&fixture.name) {
                visiting.retain(|visited| visited != &fixture.name);
                if fixture.has_teardown {
                    ordered.push(fixture);
                }
                continue;
            }
            visiting.push(fixture.name.clone());
            pending.push((name, depth + 1));
            for dependency in open.into_iter().rev() {
                pending.push((dependency.as_str(), depth + 1));
            }
        }
        Ok(ordered)
    }

    /// Whether a case awaits: its test is async, or a fixture it sets up, directly or through another, is async.
    pub(crate) fn awaits(&self, test: &TestInfo, roots: &[&str]) -> bool {
        if test.is_async {
            return true;
        }
        let mut pending: Vec<&str> = roots.to_vec();
        let mut seen: Vec<&str> = Vec::new();
        while let Some(name) = pending.pop() {
            if seen.contains(&name) {
                continue;
            }
            seen.push(name);
            let Some(fixture) = self.fixtures.get(name).copied() else {
                continue;
            };
            if fixture.is_async {
                return true;
            }
            pending.extend(fixture.dependencies.iter().map(String::as_str));
        }
        false
    }

    /// The last stage: set up the remaining fixtures in parameter order, run the autouse fixtures, call the test.
    pub(crate) fn call_stage(
        &self,
        stage: &Stage,
        test: &TestInfo,
        function: &FunctionDecl,
        values: &HashMap<String, String>,
        autouse: &[&str],
        staged: &[&str],
        bound: &[String],
    ) -> Result<String, String> {
        let mut body = Vec::new();
        let mut arguments = Vec::new();
        for (position, parameter) in function.params.iter().enumerate() {
            let name = parameter.node.name.as_str();
            if let Some(value) = values.get(name) {
                arguments.push(value.clone());
                continue;
            }
            let ty = self.text(parameter.node.ty.span.start, parameter.node.ty.span.end);
            let value = self.setup(name, ty, &stage.name, staged, &mut body, 0)?;
            if staged.contains(&name) && !parameter.node.is_mut {
                arguments.push(value);
                continue;
            }
            let local = format!("{PREFIX}_argument_{position}");
            let binding = if parameter.node.is_mut { "mut " } else { "" };
            body.push(format!("{binding}{local} = {value}"));
            arguments.push(local);
        }
        for fixture in autouse {
            if !staged.contains(fixture) {
                let call = self.setup(fixture, "", &stage.name, staged, &mut body, 0)?;
                body.push(call);
            }
        }
        body.push(stage.call(&test.function_name, &arguments, test.is_async));
        Ok(format!(
            "\n\n{} {}({}) -> {}:\n{}\n",
            stage.keyword(),
            stage.name,
            bound.join(", "),
            stage.returns,
            body.iter()
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }

    /// One `yield` fixture's stage, as two functions. The stage sets up the fixture's parameters that are not staged
    /// values and passes them on; its body is the fixture's statements up to `yield`, the next stage called with the
    /// staged values and the yielded one, then the fixture's teardown statements. In a synchronous entry the next
    /// stage runs inside the native panic boundary, so the teardown runs after a failure too, which is raised again
    /// with its message; a test's `Err` becomes that failure.
    pub(crate) fn yield_stage(
        &self,
        stage: &Stage,
        fixture: &FixtureInfo,
        staged: &[&str],
        bound: &[String],
    ) -> Result<String, String> {
        let function = self.function(&fixture.name)?;
        let (yielded, value) = yielded_value(function, &fixture.name)?;
        let (ok, err, some, none) = constructor_spellings();
        let (Some(first), Some(last)) = (function.body.first(), function.body.last()) else {
            return Err(format!("fixture `{}` has no body", fixture.name));
        };

        // ---- The stage: the staged values, then the fixture's other parameters, set up here ----
        let body_name = format!("{}_body", stage.name);
        let mut parameters = bound.to_vec();
        let mut arguments: Vec<String> = staged.iter().map(|name| name.to_string()).collect();
        let mut prelude = Vec::new();
        for parameter in &function.params {
            let name = parameter.node.name.as_str();
            if staged.contains(&name) {
                continue;
            }
            let ty = self.text(parameter.node.ty.span.start, parameter.node.ty.span.end);
            let binding = if parameter.node.is_mut { "mut " } else { "" };
            parameters.push(format!("{binding}{name}: {ty}"));
            let value = self.setup(name, ty, &stage.name, staged, &mut prelude, 0)?;
            arguments.push(self.bound(value, parameter.node.is_mut, &mut prelude));
        }
        prelude.push(stage.call(&body_name, &arguments, stage.asynchronous));
        let mut written = format!(
            "\n\n{} {}({}) -> {}:\n{}\n",
            stage.keyword(),
            stage.name,
            bound.join(", "),
            stage.returns,
            prelude
                .iter()
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        );

        // ---- The body: the fixture's statements, the next stage where it yields ----
        let indent = self
            .text(self.line_start(yielded.span.start), yielded.span.start)
            .to_string();
        let mut next_arguments: Vec<String> = staged.iter().map(|name| name.to_string()).collect();
        next_arguments.push(self.text(value.span.start, value.span.end).to_string());
        let awaited = if stage.asynchronous { "await " } else { "" };
        let next = format!("{awaited}{}({})", stage.next, next_arguments.join(", "));
        let guarded = !stage.asynchronous;
        let raise = format!("{body_name}_raise");
        written.push_str(&format!(
            "\n\n{} {body_name}({}) -> {}:\n",
            stage.keyword(),
            parameters.join(", "),
            stage.returns
        ));
        written.push_str(&self.text(self.line_start(first.span.start), yielded.span.start));
        if guarded {
            self.uses.borrow_mut().panic_boundary = true;
            let inner = if stage.returns_value {
                format!("{raise}({next})")
            } else {
                next
            };
            written.push_str(&format!("{PREFIX}_panic = {PREFIX}_panic_text(() => {inner})"));
        } else if stage.returns_value {
            written.push_str(&format!("{PREFIX}_outcome = {next}"));
        } else {
            written.push_str(&next);
        }
        written.push_str(&self.text(yielded.span.end, self.line_end(last.span.end)));
        if guarded {
            let reraise = if stage.returns_value { "return " } else { "" };
            written.push_str(&format!(
                "\n{indent}match {PREFIX}_panic:\n{indent}    {some}({PREFIX}_message) => {reraise}{PREFIX}_fail({PREFIX}_message)\n{indent}    {none} => pass"
            ));
            if stage.returns_value {
                written.push_str(&format!("\n{indent}return {ok}({none})"));
            }
        } else if stage.returns_value {
            written.push_str(&format!("\n{indent}return {PREFIX}_outcome"));
        }
        written.push('\n');
        if guarded && stage.returns_value {
            written.push_str(&format!(
                "\n\ndef {raise}(outcome: {}) -> {NONE_TYPE_NAME}:\n    match outcome:\n        {ok}(_) => pass\n        {err}(error) => {PREFIX}_fail(f\"Error: {{error:?}}\")\n",
                stage.returns
            ));
        }
        Ok(written)
    }

    /// The offset where the line holding `offset` starts.
    pub(crate) fn line_start(&self, offset: usize) -> usize {
        self.source
            .get(..offset)
            .and_then(|before| before.rfind('\n'))
            .map_or(0, |newline| newline + 1)
    }

    /// The offset where the line holding `offset` ends, before its newline.
    fn line_end(&self, offset: usize) -> usize {
        self.source
            .get(offset..)
            .and_then(|after| after.find('\n'))
            .map_or(self.source.len(), |newline| offset + newline)
    }

    /// The written entries, then the helpers and imports they use.
    pub(crate) fn finish(&self, entries: &str) -> String {
        let uses = self.uses.borrow();
        let mut written = String::from(entries);
        for (builtin, ty, helper) in &uses.directories {
            written.push_str(&directory_helper(*builtin, ty, helper));
        }
        let mut finished = written;
        let mut imports = Vec::new();
        if uses.panic_boundary || !uses.directories.is_empty() {
            let mut names = vec![format!("fail_t as {PREFIX}_fail")];
            if uses.panic_boundary {
                names.insert(0, format!("panic_text as {PREFIX}_panic_text"));
            }
            imports.push(format!("from rust::incan_std_testing import {}", names.join(", ")));
        }
        if !uses.directories.is_empty() {
            let mut environment = vec![format!("temp_dir as {PREFIX}_temp_dir")];
            if uses.working_directory {
                environment.push(format!("set_current_dir as {PREFIX}_set_current_dir"));
            }
            imports.push(format!("from rust::std::env import {}", environment.join(", ")));
            imports.push(format!(
                "from rust::std::fs import create_dir_all as {PREFIX}_create_dir_all"
            ));
            imports.push(format!("from rust::std::process import id as {PREFIX}_process_id"));
        }
        if uses.group {
            imports.push(format!("from rust::std::time import Instant as {PREFIX}_clock"));
            imports.push(format!("from std.environ import get_or as {PREFIX}_get_or"));
        }
        if !imports.is_empty() {
            finished.push_str("\n\n");
            finished.push_str(&imports.join("\n"));
            finished.push('\n');
        }
        finished
    }

    /// The checked text with each `yield` fixture's decorators and declaration blanked, keeping every line break.
    pub(crate) fn blank_yield_fixtures(&self, declarations: &[Spanned<Declaration>]) -> String {
        let mut regions = Vec::new();
        for declaration in declarations {
            let Declaration::Function(function) = &declaration.node else {
                continue;
            };
            let yields = self
                .fixtures
                .get(&function.name)
                .is_some_and(|fixture| fixture.has_teardown);
            if yields {
                regions.push(declaration_region(self.source, declaration, function));
            }
        }
        blank(self.source, &regions)
    }
}

/// A directory built-in fixture's helper: a fresh directory under the temporary directory, named for its process and
/// use as 0.5.1 names it, and for `tmp_workdir` the process's working directory.
fn directory_helper(builtin: Builtin, ty: &str, helper: &str) -> String {
    let (ok, err, _, _) = constructor_spellings();
    let (kind, enter) = match builtin {
        Builtin::TmpWorkdir => (
            "tmp_workdir",
            format!(
                "    match {PREFIX}_set_current_dir(path.clone()):\n        {ok}(_) => pass\n        {err}(error) => {PREFIX}_fail(f\"failed to enter tmp_workdir fixture: {{error}}\")\n"
            ),
        ),
        _ => ("tmp_path", String::new()),
    };
    format!(
        "\n\ndef {helper}(label: str) -> {ty}:\n    path = {PREFIX}_temp_dir().join(f\"incan-test-{{{PREFIX}_process_id()}}-{{label}}\")\n    match {PREFIX}_create_dir_all(path.clone()):\n        {ok}(_) => pass\n        {err}(error) => {PREFIX}_fail(f\"failed to create {kind} fixture: {{error}}\")\n{enter}    return path\n"
    )
}

/// The byte range of a declaration with its decorators.
pub(crate) fn declaration_region(
    source: &str,
    declaration: &Spanned<Declaration>,
    function: &FunctionDecl,
) -> (usize, usize) {
    let mut start = function
        .decorators
        .iter()
        .map(|decorator| decorator.span.start)
        .fold(declaration.span.start, usize::min);
    if start > 0 && source.as_bytes().get(start - 1) == Some(&b'@') {
        start -= 1;
    }
    (start, declaration.span.end)
}

/// `source` with every character inside `regions` replaced by a space, keeping every line break.
pub(crate) fn blank(source: &str, regions: &[(usize, usize)]) -> String {
    let mut blanked = String::with_capacity(source.len());
    for (offset, character) in source.char_indices() {
        let hidden = regions.iter().any(|(start, end)| offset >= *start && offset < *end);
        blanked.push(if hidden && character != '\n' { ' ' } else { character });
    }
    blanked
}

/// A `yield` fixture's top-level `yield` statement and the value it yields.
fn yielded_value<'f>(
    function: &'f FunctionDecl,
    name: &str,
) -> Result<(&'f Spanned<Statement>, &'f Spanned<Expr>), String> {
    for statement in &function.body {
        if let Statement::Expr(expr) = &statement.node
            && let Expr::Yield(yielded) = &expr.node
        {
            return match yielded {
                Some(value) => Ok((statement, &**value)),
                None => Err(format!(
                    "fixture `{name}` uses `yield` teardown without a yielded value; fixtures must yield the fixture value"
                )),
            };
        }
    }
    Err(format!("fixture `{name}` has no top-level `yield`"))
}

impl EntryWriter<'_> {
    /// Record that the written text needs the native panic boundary, and the environment when a file group reads its
    /// selected cases.
    pub(crate) fn uses_group(&self) {
        let mut uses = self.uses.borrow_mut();
        uses.panic_boundary = true;
        uses.group = true;
    }
}

/// The language's spellings of `Ok`, `Err`, `Some` and `None`, which the generated text writes.
pub(crate) fn constructor_spellings() -> (&'static str, &'static str, &'static str, &'static str) {
    (
        constructors::as_str(ConstructorId::Ok),
        constructors::as_str(ConstructorId::Err),
        constructors::as_str(ConstructorId::Some),
        constructors::as_str(ConstructorId::None),
    )
}
