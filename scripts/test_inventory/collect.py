#!/usr/bin/env python3
"""Enumerate every Rust test and `.incn` fixture root in the tree and check the test-corpus inventory against it.

The inventory is the control plane for the slice-7 cutover (issue #1561, "Test corpus"): every test carries a
disposition -- `keep`, `re-point`, `retire`, `unaffected` or `unreviewed` -- recorded by hand in
`scripts/test_inventory/dispositions.json` and rendered by `render.py` into the contributor reference page. This
script is the mechanical half: it walks `loaves/` and `workspaces/`, finds each `#[test]` / `#[tokio::test]`
function, measures the lane signals in its body (codegen calls, snapshot assertions, generated-text assertions,
build-and-run helpers, replacement-route APIs, checker, parser, legacy IR lowering, formatter, LSP), counts the `.incn` cases
under each declared fixture root, and compares all of that with the hand-maintained dispositions.

Modes:

- no flags: print a summary of the corpus and the disposition totals;
- `--propose`: write `proposals.json` beside the dispositions with a mechanical disposition per file and per test,
  for a reviewer to fold into `dispositions.json` by hand;
- `--check`: exit non-zero when a test has no disposition, a disposition names a test that no longer exists, a twin
  does not resolve, a recorded split flag disagrees with the measured test region, or the rendered page is stale;
- `--dispositions <path>`: read another dispositions file, for scratch probes that must not edit the tracked record.

The scanner is deliberately shallow: it masks strings and comments, counts braces, and reads `fn` names. It never
parses Rust. Signals are evidence for a reviewer, not a verdict; the disposition in `dispositions.json` is the
verdict, and the page says which one the reviewer recorded. Known limits: a `#[cfg_attr(..., test)]` attribute and a
one-line `#[test] fn ...` are not counted (the tree has neither), and helpers that live in a `#[path = "support/..."]
module outside the file are invisible to the signals.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INVENTORY_DIR = Path(__file__).resolve().parent
DISPOSITIONS_PATH = INVENTORY_DIR / "dispositions.json"
PROPOSALS_PATH = INVENTORY_DIR / "proposals.json"
SCAN_ROOTS = ("loaves", "workspaces")
SKIPPED_DIR_NAMES = {"target", ".lane", "node_modules"}

DISPOSITIONS = ("keep", "re-point", "retire", "unaffected", "unreviewed")
DURABLE_DISPOSITIONS = ("keep", "re-point")
TWIN_DISPOSITIONS = ("keep", "re-point")

TEST_ATTR_RE = re.compile(r"^\s*#\[\s*(?:test|tokio::test(?:\([^)]*\))?)\s*\]\s*$")
ATTR_RE = re.compile(r"^\s*#\s*\[")
FN_RE = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
FN_HEAD_RE = re.compile(
    r"(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+)?fn\s+[A-Za-z_][A-Za-z0-9_]*"
)
MOD_OPEN_RE = re.compile(r"\bmod\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{")
CFG_TEST_RE = re.compile(r"^\s*#\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*$")
IDENT_CALL_RE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(")

# Lane signals, as regexes over the original text of a test function (signature and body, strings included).
# Each list is evidence for one lane; the proposal rules below read them in a fixed order of authority. The
# names in `build_run` come from `loaves/compiler/incan_test_support/src/cli_project.rs` and `lib.rs` and from
# the copies the driver and CLI test files carry; the codegen names come from
# `loaves/compiler/incan_emit/src/test_support.rs` and the emit crate's tests. Every pattern matches something in
# the tree; a guessed name that matches nothing is noise in the table, not evidence.
SIGNALS: dict[str, tuple[str, ...]] = {
    "codegen": (
        r"\bIrCodegen\b",
        r"\bIrEmitter\b",
        r"\btry_generate\s*\(",
        r"\bgenerate_rust\s*\(",
        r"\bgenerate_projected_rust\s*\(",
        r"\bgenerate_registry_rust\s*\(",
        r"\bgenerate_projected_registry_rust\s*\(",
        r"\bemit_program\s*\(",
        r"\bread_generated_rust\s*\(",
        r"\bincan_emit::",
        r"\bGeneratedRust\b",
        r"\bgenerated_rust\b",
        r"\bgenerate\s*\(",
        r"\bprettyplease\b",
        # A read of the generated project's Rust under `target/incan/<project>/src/*.rs`, or the CLI's own
        # generated-Rust emission (`incan --emit-rust`).
        r"target/incan/[^\s\"']*\.rs\b",
        r"\"--emit-rust\"",
    ),
    "snapshot": (
        r"\bassert_snapshot!",
        r"\bassert_debug_snapshot!",
        r"\bassert_display_snapshot!",
        r"\binsta::",
        r"\.snap\"",
    ),
    "generated_text": (
        r"contains\(\s*&?(?:r#*)?\"(?:pub(?:\([^)]*\))? fn |fn |impl |let |use |mod |async fn|unsafe |#\[derive|#\[allow|#!\[|::new\(|\.clone\(\)|\.to_string\(\)|\.into\(\)|\.as_str\(\)|&mut |&str|String::|Vec<|Vec::|Box<|Box::|Option<|Rc<|Arc<|HashMap<|HashSet<|BTreeMap<|Result<)",
        r"contains\(\s*&?format!\(\s*\"(?:pub fn |fn |impl |let |use |mod )",
        r"\bassert_no_generated_unused_lint_allows\s*\(",
    ),
    # Evidence that generated Rust is built or run. A bare CLI invocation is not on this list: `incan fmt`,
    # `incan check`, `--help` and `--version` never reach the backend, so those helpers count only through
    # `CLI_INVOCATION_RE` below, beside a compiling subcommand or a generated-target read.
    "build_run": (
        r"\brun_explicit_oven_bake\w*\s*\(",
        r"\bconfigure_explicit_oven_bake_command\s*\(",
        r"Command::new\(\s*\"cargo\"",
        r"Command::new\(\s*\"rustc\"",
        r"\bcompile_and_run\w*\s*\(",
        r"\bProjectGenerator\b",
        r"\bbake\s*\(",
        r"\bLegacyOvenCapability\b",
        r"\bcompare_source_observable\s*\(",
        r"\bwrite_minimal_project\s*\(",
        r"\bunique_test_project_name\s*\(",
        r"\.args?\(\s*\[?\s*\"(?:run|build|test|bake)\"",
    ),
    "replacement": (
        r"\breplacement::",
        r"\breplacement_compatibility\b",
        r"\bReplacement(?:Value|Set|Execution\w*|NumericValue|Dict|Program\w*|Backend\w*|Route\w*|Compatibility\w*)\b",
        r"\bshadow_support::",
        r"\bshadow::",
        r"\bShadowComparison\w*\b",
        r"\bbody_ir::",
        r"\bbuild_body_ir_module\w*\s*\(",
        r"\blower_typed_body_ir\s*\(",
        r"\blower_named_body_ir\s*\(",
        r"\bBodyIr\w*\b",
        r"\bdirect_execution\b",
        r"\bexecute_free_function\w*\s*\(",
        r"\bBackendKind::",
        r"\bFallbackPolicy\b",
        r"\bFallbackOutcome\b",
        r"\bLegacyOvenCapability\b",
        r"\bcompare_source_observable\s*\(",
    ),
    "checker": (
        r"\bTypeChecker\b",
        r"\btypechecker::",
        r"\btypecheck\w*\s*\(",
        r"\bcheck_str\w*\s*\(",
        r"\bcheck_program\w*\s*\(",
        r"\bcheck_source\w*\s*\(",
        r"\bcheck_module\w*\s*\(",
        r"\bcheck_err\w*\s*\(",
        r"\bCompileError\b",
        r"\bincan_syntax::diagnostics\b",
        r"\bdiagnostics::(?:catalog|Diagnostic\w*|render\w*)\b",
        r"\bTypeError\b",
        r"\bTypeDiagnostic\w*\b",
        r"\bcheck_program_with\w*\s*\(",
        r"\bcompile_source\s*\(",
        r"\bcompile_file\s*\(",
        r"\bCompilationSession\b",
        r"\bSymbolTable\b",
        r"\bapi_metadata::",
        r"\bLibraryManifest(?:Index)?::",
    ),
    "parser": (
        r"\bparser::parse\s*\(",
        r"\bparse_program\w*\s*\(",
        r"\bparse_str\w*\s*\(",
        r"\bparse_source\w*\s*\(",
        r"\blexer::lex\s*\(",
        r"\bLexer::new\s*\(",
        r"\bParser::new\s*\(",
        r"\btokenize\s*\(",
        r"\bincan_syntax::",
        r"\bparse_expr\w*\s*\(",
        r"\bparse_stmt\w*\s*\(",
        r"\bparse_decl\w*\s*\(",
    ),
    "legacy_ir": (
        r"\bAstLowering\b",
        r"\blower_program\s*\(",
        r"\blower_source\s*\(",
        r"\bincan_ir::",
        r"\bIr(?:Program|Module|Type|Decl|DeclKind|Expr|ExprKind|Stmt|StmtKind|Function|CallArg|CallArgKind|ImportItem|GenerationMetadata|GenerationOptions|CheckedCType|RustTraitImport)\b",
    ),
    "formatter": (
        r"\bformat_source\w*\s*\(",
        r"\bincan_format::",
        r"\bFormatter\b",
        r"\bformat_program\w*\s*\(",
        r"\bassert_format\w*\s*\(",
    ),
    "lsp": (
        r"\btower_lsp\b",
        r"\bincan_lsp::",
        r"\blsp_types::",
        r"\bLanguageServer\b",
        r"\bHoverParams\b",
        r"\bCompletionParams\b",
        r"\bGotoDefinitionParams\b",
    ),
}
# One alternation per lane: a single scan of each function's text per lane keeps the gate a few seconds long.
COMPILED_SIGNALS = {
    lane: re.compile("|".join(f"(?:{pattern})" for pattern in patterns)) for lane, patterns in SIGNALS.items()
}
LANE_ORDER = tuple(SIGNALS)

# A CLI invocation is `build_run` evidence only when the same function names a subcommand that compiles the
# program (`build`, `run`, `test`, `bake`) or reads the generated project; `incan fmt`, `incan check`,
# `--help` and `--version` stop in the frontend and the cutover does not change their route.
CLI_INVOCATION_RE = re.compile(
    "|".join(
        f"(?:{pattern})"
        for pattern in (
            r"\brun_incan\w*\s*\(",
            r"\bincan_command\s*\(",
            r"\bconfigured_incan_command\s*\(",
            r"\bincan_binary\s*\(",
            r"\bincan_debug_binary\s*\(",
            r"\bcargo_bin\s*\(",
            r"Command::new\(\s*&?incan",
        )
    )
)
COMPILING_CONTEXT_RE = re.compile(r"\"(?:build|run|test|bake)\"|\bread_generated_rust\s*\(|target/incan/")

# Crate roots whose tests do not touch the compiler pipeline at all. A test under one of these is proposed
# `unaffected` unless its body says otherwise.
UNAFFECTED_PREFIXES = (
    "loaves/oven/",
    "loaves/stdlib/",
    "loaves/third_party/",
    "loaves/toolchain/oven-cli/",
    "loaves/compiler/incan_provider/",
    "loaves/compiler/rust_inspect/",
    "loaves/compiler/incan_oven_facet/",
    "loaves/kernel/incan_vocab/",
    "loaves/kernel/incan_codegraph/",
)
# Crate roots that assert source meaning: the frontend, syntax, semantics core, formatter and LSP. The
# Rust-source backend's own IR (`loaves/compiler/incan_ir/`) is not one of them: its tests carry the `legacy_ir`
# signal and the record classifies them retire under the condition in their notes.
KEEP_PREFIXES = (
    "loaves/kernel/incan_syntax/",
    "loaves/kernel/incan_semantics_core/",
    "loaves/kernel/incan_lang/",
    "loaves/compiler/incan_frontend/",
    "loaves/compiler/incan_format/",
    "loaves/toolchain/incan-lsp/",
)
# The emitter proper: everything under it asserts generated Rust shape.
RETIRE_PREFIXES = ("loaves/compiler/incan_emit/src/emit/",)


@dataclass
class TestFn:
    """One test function: where it is, what it is called, and which lane signals its text carries."""

    name: str
    qualified: str
    line: int
    signals: Counter = field(default_factory=Counter)
    via_helpers: tuple[str, ...] = ()

    @property
    def lanes(self) -> tuple[str, ...]:
        """Lanes with at least one hit, in the fixed lane order."""
        return tuple(lane for lane in LANE_ORDER if self.signals.get(lane, 0) > 0)


@dataclass
class ScannedFile:
    """One Rust file that carries tests, with the measurements the inventory records about it."""

    path: str
    lines: int
    test_lines: int
    tests: list[TestFn]
    anomalies: list[str] = field(default_factory=list)

    @property
    def keys(self) -> list[str]:
        """Test keys in file order: the bare `fn` name, or the module-qualified name when the bare one repeats."""
        return [test.qualified for test in self.tests]

    @property
    def lane_totals(self) -> Counter:
        """How many tests in the file carry each lane."""
        totals: Counter = Counter()
        for test in self.tests:
            for lane in test.lanes:
                totals[lane] += 1
        return totals


@dataclass
class FixtureRoot:
    """One declared `.incn` fixture root with the cases the collector counted under it."""

    root: str
    pattern: str
    cases: int
    disposition: str
    owner: int
    notes: str


@dataclass
class Corpus:
    """Everything the collector measured: scanned files plus fixture roots."""

    files: list[ScannedFile]
    fixture_roots: list[FixtureRoot]

    @property
    def total_tests(self) -> int:
        """Number of test functions across all scanned files."""
        return sum(len(file.tests) for file in self.files)

    def by_path(self) -> dict[str, ScannedFile]:
        """Scanned files keyed by repository-relative path."""
        return {file.path: file for file in self.files}


# ============================================================
# Masking: strings, chars and comments become spaces
# ============================================================


def mask_rust(text: str) -> str:
    """Return `text` with every string literal, char literal and comment replaced by spaces, newlines kept.

    Brace counting and `fn` detection run over the masked text so a `contains("fn main() {")` assertion or a
    Rust program embedded in a raw string cannot unbalance the scanner. Lifetimes are distinguished from char
    literals by looking for the closing quote; nested block comments are honoured.
    """
    out = list(text)
    n = len(text)
    i = 0

    def blank(start: int, end: int) -> None:
        for k in range(start, min(end, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        ch = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        # ---- Line comment (including doc comments) ----
        if ch == "/" and nxt == "/":
            end = text.find("\n", i)
            end = n if end == -1 else end
            blank(i, end)
            i = end
            continue
        # ---- Block comment, nested ----
        if ch == "/" and nxt == "*":
            depth = 1
            j = i + 2
            while j < n and depth > 0:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        # ---- Raw string: r"...", r#"..."#, br"...", cr"..." ----
        if ch in "rbc" and not _ident_before(text, i):
            j = i
            if ch in "bc" and nxt == "r":
                j += 1
            if text[j] == "r" and j + 1 < n and text[j + 1] in '#"':
                k = j + 1
                hashes = 0
                while k < n and text[k] == "#":
                    hashes += 1
                    k += 1
                if k < n and text[k] == '"':
                    closer = '"' + "#" * hashes
                    end = text.find(closer, k + 1)
                    end = n if end == -1 else end + len(closer)
                    blank(i, end)
                    i = end
                    continue
        # ---- Ordinary or byte string ----
        if ch == '"' or (ch in "bc" and nxt == '"' and not _ident_before(text, i)):
            j = i + 1 if ch == '"' else i + 2
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    j += 1
                    break
                j += 1
            blank(i, j)
            i = j
            continue
        # ---- Char literal vs lifetime ----
        if ch == "'" or (ch == "b" and nxt == "'" and not _ident_before(text, i)):
            q = i if ch == "'" else i + 1
            end = _char_literal_end(text, q)
            if end is not None:
                blank(i, end)
                i = end
                continue
            i = q + 1
            continue
        i += 1
    return "".join(out)


def _ident_before(text: str, i: int) -> bool:
    """True when the character before `i` continues an identifier, so `i` cannot start a literal prefix."""
    return i > 0 and (text[i - 1].isalnum() or text[i - 1] == "_")


def _char_literal_end(text: str, q: int) -> int | None:
    """Return the index just past the char literal opening at `q`, or None when `q` opens a lifetime or label."""
    n = len(text)
    if q + 1 >= n:
        return None
    if text[q + 1] == "\\":
        j = q + 2
        if j < n and text[j] == "u":
            close = text.find("}", j)
            j = n if close == -1 else close + 1
        elif j < n and text[j] == "x":
            j += 3
        else:
            j += 1
        if j < n and text[j] == "'":
            return j + 1
        return None
    if q + 2 < n and text[q + 2] == "'":
        return q + 3
    return None


# ============================================================
# Brace structure and function bodies
# ============================================================


def matching_brace(masked: str, open_index: int) -> int:
    """Return the index of the `}` matching the `{` at `open_index` in masked text, or the text end when unbalanced."""
    depth = 0
    for k in range(open_index, len(masked)):
        ch = masked[k]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return k
    return len(masked) - 1


def line_of(text: str, index: int, line_starts: list[int]) -> int:
    """1-based line number of character `index`, given the sorted line start offsets of `text`."""
    lo, hi = 0, len(line_starts) - 1
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if line_starts[mid] <= index:
            lo = mid
        else:
            hi = mid - 1
    return lo + 1


def body_open_index(masked: str, start: int) -> int | None:
    """Index of the `{` that opens the body of the `fn` whose name ends at `start`; None when a `;` ends it first.

    Only a `{` or `;` outside the signature's own brackets counts, so `fn x() -> [u8; 4] {` has a body and a
    trait method `fn x() -> [u8; 4];` does not. `->` is an arrow, not a closing angle bracket.
    """
    depth = 0
    for index in range(start, len(masked)):
        ch = masked[index]
        if ch in "([<":
            depth += 1
        elif ch in ")]":
            depth -= 1
        elif ch == ">":
            if masked[index - 1] != "-":
                depth -= 1
        elif depth == 0:
            if ch == "{":
                return index
            if ch == ";":
                return None
    return None


def function_spans(masked: str) -> list[tuple[str, int, int, int]]:
    """Every `fn` in masked text as `(name, fn_index, body_open, body_close)`; a bodiless `fn` is skipped."""
    spans = []
    for match in FN_RE.finditer(masked):
        open_index = body_open_index(masked, match.end())
        if open_index is None:
            continue
        close_index = matching_brace(masked, open_index)
        spans.append((match.group(1), match.start(), open_index, close_index))
    return spans


def next_fn_after(masked: str, pos: int) -> int | None:
    """Index of the `fn` keyword that follows position `pos` in masked text, skipping further attributes.

    Attributes may span lines (`#[cfg_attr(not(any(...)), ignore)]`), so they are skipped by bracket depth rather
    than by line. None when the next item is not a function.
    """
    n = len(masked)
    i = pos
    while i < n:
        while i < n and masked[i].isspace():
            i += 1
        if i >= n:
            return None
        if masked[i] == "#":
            open_index = masked.find("[", i)
            if open_index == -1:
                return None
            depth = 0
            k = open_index
            while k < n:
                if masked[k] == "[":
                    depth += 1
                elif masked[k] == "]":
                    depth -= 1
                    if depth == 0:
                        break
                k += 1
            i = k + 1
            continue
        head = FN_HEAD_RE.match(masked, i)
        if head is None:
            return None
        keyword = FN_RE.search(masked, i, head.end())
        return None if keyword is None else keyword.start()
    return None


def module_paths(masked: str) -> list[tuple[int, int, tuple[str, ...]]]:
    """Regions `(start, end, path)` of every `mod name { ... }` block, innermost last, for qualifying test names."""
    regions = []
    for match in MOD_OPEN_RE.finditer(masked):
        open_index = match.end() - 1
        close_index = matching_brace(masked, open_index)
        regions.append((open_index, close_index, match.group(1)))
    regions.sort()
    result = []
    for open_index, close_index, name in regions:
        path = tuple(
            outer_name
            for outer_open, outer_close, outer_name in regions
            if outer_open < open_index and outer_close > close_index
        ) + (name,)
        result.append((open_index, close_index, path))
    return result


def module_path_at(index: int, regions: list[tuple[int, int, tuple[str, ...]]]) -> tuple[str, ...]:
    """The innermost module path enclosing character `index`."""
    best: tuple[str, ...] = ()
    for open_index, close_index, path in regions:
        if open_index < index < close_index and len(path) > len(best):
            best = path
    return best


# ============================================================
# Signals
# ============================================================


def lane_signals(text: str) -> Counter:
    """Count lane-signal hits in one function's text (signature and body, strings included).

    CLI invocations (`run_incan`, `incan_command` and their kin) are `build_run` evidence only beside a compiling
    subcommand or a generated-target read in the same text; on their own they prove nothing about the route.
    """
    counts: Counter = Counter()
    for lane, pattern in COMPILED_SIGNALS.items():
        hits = sum(1 for _ in pattern.finditer(text))
        if hits:
            counts[lane] = hits
    invocations = sum(1 for _ in CLI_INVOCATION_RE.finditer(text))
    if invocations and COMPILING_CONTEXT_RE.search(text):
        counts["build_run"] += invocations
    return counts


def cfg_test_region_lines(lines: list[str], masked_lines: list[str], masked: str, line_starts: list[int]) -> int:
    """Total lines inside `#[cfg(test)] mod name { ... }` blocks; zero when the file has none.

    This is the test region the split threshold applies to whenever a file has one, whatever the file is called:
    a source module named `*_test.rs` that carries a runner beside its `#[cfg(test)]` module is measured by the
    module, not the file. A file with no such block is test code throughout and is measured whole.
    """
    total = 0
    for index, line in enumerate(masked_lines):
        if not CFG_TEST_RE.match(line):
            continue
        j = index + 1
        while j < len(masked_lines) and (ATTR_RE.match(masked_lines[j]) or not masked_lines[j].strip()):
            j += 1
        if j >= len(masked_lines):
            continue
        mod_match = re.search(r"\bmod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{", masked_lines[j])
        if mod_match is None:
            continue
        open_index = line_starts[j] + mod_match.end() - 1
        close_index = matching_brace(masked, open_index)
        close_line = line_of(masked, close_index, line_starts)
        total += close_line - index
    return total


# ============================================================
# Scanning one file
# ============================================================


def scan_file(rel_path: str, text: str) -> ScannedFile | None:
    """Scan one Rust file for test functions; None when it has no test attribute at all."""
    lines = text.split("\n")
    if "#[" not in text or "test" not in text:
        # Cheap substring pre-filter; the masked pass below is authoritative (a `#[test] // note` line has a
        # trailing comment on the raw line and none once masked, so the raw line must not be the judge).
        return None
    masked = mask_rust(text)
    masked_lines = masked.split("\n")
    line_starts = [0]
    for line in lines[:-1]:
        line_starts.append(line_starts[-1] + len(line) + 1)
    spans = function_spans(masked)
    regions = module_paths(masked)

    # ---- Every function's own signals, for the helper closure ----
    fn_by_start = {fn_index: (name, open_index, close_index) for name, fn_index, open_index, close_index in spans}
    own_signals: dict[int, Counter] = {}
    calls: dict[int, set[str]] = {}
    indexes_by_name: dict[str, list[int]] = {}
    for name, fn_index, open_index, close_index in spans:
        own_signals[fn_index] = lane_signals(text[fn_index : close_index + 1])
        calls[fn_index] = set(IDENT_CALL_RE.findall(masked[open_index : close_index + 1]))
        indexes_by_name.setdefault(name, []).append(fn_index)

    # ---- Test functions: attribute, then the next `fn` ----
    tests: list[TestFn] = []
    anomalies: list[str] = []
    for index, line in enumerate(masked_lines):
        if not TEST_ATTR_RE.match(line):
            continue
        fn_index = next_fn_after(masked, line_starts[index] + len(line))
        if fn_index is None:
            anomalies.append(f"line {index + 1}: test attribute not followed by a `fn`")
            continue
        span = fn_by_start.get(fn_index)
        if span is None:
            anomalies.append(f"line {index + 1}: the `fn` after the test attribute has no body")
            continue
        name, open_index, close_index = span
        j = line_of(masked, fn_index, line_starts) - 1
        signals = Counter(own_signals[fn_index])
        via: list[str] = []
        seen: set[int] = {fn_index}
        frontier = [
            index for callee in calls[fn_index] for index in indexes_by_name.get(callee, []) if index != fn_index
        ]
        while frontier:
            callee_index = frontier.pop()
            if callee_index in seen:
                continue
            seen.add(callee_index)
            callee_signals = own_signals[callee_index]
            if callee_signals:
                via.append(fn_by_start[callee_index][0])
                signals.update(callee_signals)
            frontier.extend(
                index
                for nested in calls[callee_index]
                for index in indexes_by_name.get(nested, [])
                if index not in seen
            )
        tests.append(
            TestFn(
                name=name,
                qualified="::".join(module_path_at(fn_index, regions) + (name,)),
                line=j + 1,
                signals=signals,
                via_helpers=tuple(sorted(set(via))),
            )
        )

    # ---- Keys: bare names unless a name repeats within the file ----
    name_counts = Counter(test.name for test in tests)
    for test in tests:
        if name_counts[test.name] == 1:
            test.qualified = test.name
    qualified_counts = Counter(test.qualified for test in tests)
    for key, count in qualified_counts.items():
        if count > 1:
            anomalies.append(f"duplicate test key `{key}` ({count} functions share the same module path and name)")

    # ---- Test region: the `#[cfg(test)]` modules when there are any, else the whole file ----
    test_lines = cfg_test_region_lines(lines, masked_lines, masked, line_starts) or len(lines)
    return ScannedFile(path=rel_path, lines=len(lines), test_lines=test_lines, tests=tests, anomalies=anomalies)


def rust_files() -> list[Path]:
    """Every `.rs` file under the scan roots, skipping build output and lane scratch directories."""
    found: list[Path] = []
    for root_name in SCAN_ROOTS:
        root = ROOT / root_name
        if not root.is_dir():
            continue
        for path in sorted(root.rglob("*.rs")):
            rel = path.relative_to(ROOT)
            if any(part in SKIPPED_DIR_NAMES for part in rel.parts):
                continue
            found.append(path)
    return found


def scan_tree() -> list[ScannedFile]:
    """Scan every Rust file under the scan roots and keep those that carry tests."""
    scanned = []
    for path in rust_files():
        text = path.read_text(encoding="utf-8")
        result = scan_file(path.relative_to(ROOT).as_posix(), text)
        if result is not None and result.tests:
            scanned.append(result)
    return scanned


# ============================================================
# Dispositions and fixture roots
# ============================================================


def load_dispositions(path: Path = DISPOSITIONS_PATH) -> dict:
    """Read the hand-maintained dispositions file, or an empty skeleton when it does not exist yet."""
    if not path.exists():
        return {"schema": 1, "split_threshold_lines": 1500, "fixture_roots": {}, "files": {}}
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def count_fixture_roots(dispositions: dict) -> list[FixtureRoot]:
    """Count the `.incn` cases under each declared fixture root."""
    roots = []
    for root, entry in sorted(dispositions.get("fixture_roots", {}).items()):
        pattern = entry.get("pattern", "**/*.incn")
        base = ROOT / root
        cases = 0
        if base.is_dir():
            cases = sum(
                1
                for path in base.glob(pattern)
                if path.is_file() and not any(part in SKIPPED_DIR_NAMES for part in path.relative_to(ROOT).parts)
            )
        roots.append(
            FixtureRoot(
                root=root,
                pattern=pattern,
                cases=cases,
                disposition=entry.get("disposition", "unreviewed"),
                owner=int(entry.get("owner", 0) or 0),
                notes=entry.get("notes", ""),
            )
        )
    return roots


def collect(dispositions: dict | None = None) -> Corpus:
    """Scan the tree and count fixture roots; the single entry point the renderer and the gate share."""
    dispositions = load_dispositions() if dispositions is None else dispositions
    return Corpus(files=scan_tree(), fixture_roots=count_fixture_roots(dispositions))


def effective_disposition(entry: dict, key: str) -> str:
    """The disposition of one test: its override when present, otherwise the file default."""
    override = entry.get("tests", {}).get(key)
    if override and override.get("disposition"):
        return override["disposition"]
    return entry.get("disposition", "unreviewed")


def effective_twin(entry: dict, key: str) -> str:
    """The twin of one test: its override when present, otherwise the file-level twin."""
    override = entry.get("tests", {}).get(key)
    if override and override.get("twin"):
        return override["twin"]
    return entry.get("twin", "")


def disposition_totals(corpus: Corpus, dispositions: dict) -> Counter:
    """Test counts per disposition; a file with no entry counts as `unclassified`."""
    totals: Counter = Counter()
    files = dispositions.get("files", {})
    for file in corpus.files:
        entry = files.get(file.path)
        if entry is None:
            totals["unclassified"] += len(file.tests)
            continue
        for key in file.keys:
            totals[effective_disposition(entry, key)] += 1
    return totals


# ============================================================
# Proposals
# ============================================================


def propose_test(rel_path: str, test: TestFn) -> str:
    """Mechanical disposition for one test from its lanes and its crate. A reviewer overrides it by hand."""
    lanes = set(test.lanes)
    if rel_path.startswith(RETIRE_PREFIXES):
        return "retire"
    if "snapshot" in lanes or "generated_text" in lanes:
        return "retire"
    if "codegen" in lanes and "build_run" not in lanes:
        return "retire"
    if "build_run" in lanes:
        return "re-point"
    if "legacy_ir" in lanes and not (lanes & {"replacement", "build_run"}):
        return "retire"
    if lanes & {"replacement", "checker", "parser", "formatter", "lsp"}:
        return "keep"
    if rel_path.startswith(KEEP_PREFIXES):
        return "keep"
    if rel_path.startswith(UNAFFECTED_PREFIXES):
        return "unaffected"
    return "unreviewed"


def propose(corpus: Corpus, dispositions: dict) -> dict:
    """Build the proposals sidecar: a file default (the majority proposal) plus per-test proposals that differ."""
    threshold = int(dispositions.get("split_threshold_lines", 1500))
    files = {}
    for file in corpus.files:
        per_test = {test.qualified: propose_test(file.path, test) for test in file.tests}
        majority = Counter(per_test.values()).most_common(1)[0][0]
        overrides = {key: value for key, value in per_test.items() if value != majority}
        files[file.path] = {
            "disposition": majority,
            "tests_count": len(file.tests),
            "lines": file.lines,
            "test_lines": file.test_lines,
            "split_required": file.test_lines > threshold,
            "lanes": dict(sorted(file.lane_totals.items())),
            "mixed": bool(overrides),
            "tests": overrides,
        }
    return {"files": files}


# ============================================================
# Gate
# ============================================================


def resolve_twin(twin: str, corpus: Corpus, dispositions: dict) -> str | None:
    """Return None when `twin` names an existing keep/re-point test or a declared fixture root, else the reason."""
    if not twin:
        return "twin is empty"
    files = corpus.by_path()
    entries = dispositions.get("files", {})
    if "::" in twin:
        path, key = twin.split("::", 1)
        file = files.get(path)
        if file is None:
            return f"twin file `{path}` carries no tests"
        if key not in file.keys:
            return f"twin `{key}` is not a test in `{path}`"
        entry = entries.get(path)
        if entry is None:
            return f"twin file `{path}` has no disposition row"
        disposition = effective_disposition(entry, key)
        if disposition not in TWIN_DISPOSITIONS:
            return f"twin `{twin}` is `{disposition}`, not keep or re-point"
        return None
    roots = {root.root: root for root in corpus.fixture_roots}
    root = roots.get(twin)
    if root is None:
        return f"twin `{twin}` is neither `path::fn` nor a declared fixture root"
    if root.disposition not in TWIN_DISPOSITIONS:
        return f"twin fixture root `{twin}` is `{root.disposition}`, not keep or re-point"
    return None


def check(corpus: Corpus, dispositions: dict, rendered_page_stale: str | None) -> list[str]:
    """Every gate failure as one line; an empty list means the inventory is green."""
    failures: list[str] = []
    threshold = int(dispositions.get("split_threshold_lines", 1500))
    entries = dispositions.get("files", {})
    scanned = corpus.by_path()

    # ---- Every scanned test has a disposition ----
    for file in corpus.files:
        for anomaly in file.anomalies:
            failures.append(f"scanner anomaly in `{file.path}`: {anomaly}")
        entry = entries.get(file.path)
        if entry is None:
            names = ", ".join(file.keys[:5]) + (", ..." if len(file.tests) > 5 else "")
            failures.append(
                f"unclassified: `{file.path}` carries {len(file.tests)} test(s) with no row in dispositions.json ({names})"
            )
            continue
        disposition = entry.get("disposition")
        if disposition not in DISPOSITIONS:
            failures.append(f"`{file.path}`: disposition `{disposition}` is not one of {', '.join(DISPOSITIONS)}")
        recorded_split = bool(entry.get("split_required", False))
        measured_split = file.test_lines > threshold
        if recorded_split != measured_split:
            failures.append(
                f"`{file.path}`: split_required is {str(recorded_split).lower()} but the test region is "
                f"{file.test_lines} lines against a threshold of {threshold}; record {str(measured_split).lower()}"
            )
        keys = set(file.keys)
        for key, override in entry.get("tests", {}).items():
            if key not in keys:
                failures.append(f"`{file.path}`: override names `{key}`, which is not a test in the file")
                continue
            override_disposition = override.get("disposition")
            if override_disposition is not None and override_disposition not in DISPOSITIONS:
                failures.append(f"`{file.path}::{key}`: disposition `{override_disposition}` is not valid")
        # ---- Every named twin resolves, whatever the row's disposition; one line per distinct twin ----
        twins: dict[str, str] = {}
        for key in file.keys:
            twin = effective_twin(entry, key)
            if twin:
                twins.setdefault(twin, key)
        for twin, key in twins.items():
            reason = resolve_twin(twin, corpus, dispositions)
            if reason is not None:
                failures.append(f"`{file.path}::{key}`: {reason}")

    # ---- Every disposition row names a file that still carries tests ----
    for path in entries:
        if path not in scanned:
            failures.append(f"stale row: `{path}` has a disposition but carries no tests in the tree")

    # ---- Fixture roots exist and are non-empty ----
    for root in corpus.fixture_roots:
        if root.disposition not in DISPOSITIONS:
            failures.append(f"fixture root `{root.root}`: disposition `{root.disposition}` is not valid")
        if not (ROOT / root.root).is_dir():
            failures.append(f"fixture root `{root.root}` does not exist")
        elif root.cases == 0:
            failures.append(f"fixture root `{root.root}` matches no `{root.pattern}` case")

    if rendered_page_stale:
        failures.append(rendered_page_stale)
    return failures


# ============================================================
# CLI
# ============================================================


def print_summary(corpus: Corpus, dispositions: dict) -> None:
    """Human summary: corpus size, disposition totals, split candidates and files without a row."""
    totals = disposition_totals(corpus, dispositions)
    threshold = int(dispositions.get("split_threshold_lines", 1500))
    print(f"files with tests: {len(corpus.files)}")
    print(f"test functions:   {corpus.total_tests}")
    print(f"fixture roots:    {len(corpus.fixture_roots)} ({sum(r.cases for r in corpus.fixture_roots)} cases)")
    print("dispositions:")
    for name in DISPOSITIONS + ("unclassified",):
        if totals.get(name):
            print(f"  {name:<13}{totals[name]:>6}")
    split = [file for file in corpus.files if file.test_lines > threshold]
    print(f"split candidates (test region > {threshold} lines): {len(split)}")
    for file in sorted(split, key=lambda f: -f.test_lines):
        print(f"  {file.test_lines:>6}  {file.path}")
    anomalies = [(file.path, a) for file in corpus.files for a in file.anomalies]
    if anomalies:
        print("scanner anomalies:")
        for path, anomaly in anomalies:
            print(f"  {path}: {anomaly}")


def parse_args(argv: list[str]) -> argparse.Namespace:
    """Command-line options."""
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="run the gate and exit non-zero on any failure")
    parser.add_argument("--propose", action="store_true", help="write proposals.json with mechanical dispositions")
    parser.add_argument("--json", action="store_true", help="print the scanned corpus as JSON instead of a summary")
    parser.add_argument(
        "--dispositions",
        type=Path,
        default=DISPOSITIONS_PATH,
        metavar="PATH",
        help="dispositions file to read instead of the tracked record, for scratch probes",
    )
    return parser.parse_args(argv)


def corpus_as_json(corpus: Corpus) -> dict:
    """The scanned corpus in a plain structure, for `--json` and for the unit tests."""
    return {
        "files": [
            {
                "path": file.path,
                "lines": file.lines,
                "test_lines": file.test_lines,
                "tests": [
                    {
                        "key": test.qualified,
                        "name": test.name,
                        "line": test.line,
                        "lanes": list(test.lanes),
                        "signals": dict(test.signals),
                        "via_helpers": list(test.via_helpers),
                    }
                    for test in file.tests
                ],
                "anomalies": file.anomalies,
            }
            for file in corpus.files
        ],
        "fixture_roots": [root.__dict__ for root in corpus.fixture_roots],
    }


def main(argv: list[str] | None = None) -> int:
    """Entry point."""
    args = parse_args(sys.argv[1:] if argv is None else argv)
    dispositions = load_dispositions(args.dispositions)
    corpus = collect(dispositions)

    if args.json:
        json.dump(corpus_as_json(corpus), sys.stdout, indent=2)
        sys.stdout.write("\n")
        return 0

    if args.propose:
        proposals = propose(corpus, dispositions)
        with PROPOSALS_PATH.open("w", encoding="utf-8") as handle:
            json.dump(proposals, handle, indent=2, sort_keys=True)
            handle.write("\n")
        print(f"wrote {PROPOSALS_PATH.relative_to(ROOT)} for {len(proposals['files'])} files")
        return 0

    if args.check:
        from render import page_staleness  # noqa: PLC0415 -- sibling module, imported lazily to avoid a cycle

        failures = check(corpus, dispositions, page_staleness(corpus, dispositions))
        if failures:
            print("Test corpus inventory is not green. Classify the test in scripts/test_inventory/dispositions.json,")
            print("then run `make test-inventory` to regenerate the page. Failures:\n")
            for failure in failures:
                print(f"- {failure}")
            print(f"\n{len(failures)} failure(s).")
            return 1
        totals = disposition_totals(corpus, dispositions)
        print(
            f"test corpus inventory green: {corpus.total_tests} tests in {len(corpus.files)} files, "
            f"{sum(r.cases for r in corpus.fixture_roots)} fixture cases in {len(corpus.fixture_roots)} roots "
            f"({', '.join(f'{name} {totals[name]}' for name in DISPOSITIONS if totals.get(name))})"
        )
        return 0

    print_summary(corpus, dispositions)
    return 0


if __name__ == "__main__":
    sys.exit(main())
