#!/usr/bin/env python3
"""Render the test-corpus inventory page from the scanned corpus and the hand-maintained dispositions.

The page is `workspaces/docs-site/docs/contributing/reference/test_corpus_inventory.md`, a generated
control-plane reference on the pattern of the replacement compatibility inventory: a summary that reconciles to
the total number of tests, one row per test file grouped by crate, per-test rows only where a file carries
per-test overrides, and the `.incn` fixture roots. `--check` exits non-zero when the page on disk differs from
what the current tree and dispositions render to.
"""

from __future__ import annotations

import argparse
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from collect import (  # noqa: E402 -- sibling module
    BEHAVIOR_FIXTURES_ROOT,
    DIES,
    DISPOSITIONS,
    DURABLE_DISPOSITIONS,
    LANE_ORDER,
    ROOT,
    Corpus,
    ScannedFile,
    collect,
    disposition_totals,
    effective_disposition,
    effective_dies,
    effective_twin,
    load_dispositions,
    retire_fate,
    retire_totals,
)

PAGE_PATH = ROOT / "workspaces" / "docs-site" / "docs" / "contributing" / "reference" / "test_corpus_inventory.md"
HOW_TO_LINK = "https://github.com/encero-systems/incan/issues/1561#issuecomment-5750194193"

DISPOSITION_MEANING = {
    "keep": "Asserts source meaning through the parser, typechecker, Body IR, formatter, LSP or semantics core, and never touches generated Rust. Survives the slice-7 cutover untouched.",
    "re-point": "Asserts program behaviour (output, exit code, diagnostics of a run) but proves it by building or running generated Rust. The assertion stays; slice 7 changes the route.",
    "retire": "Asserts the shape of the generated Rust itself: snapshot text, `contains(\"fn ...\")` on emitted source, emitter unit tests. Dies with #654, and only after its row names a twin or records `dies` with the reason.",
    "unaffected": "Oven, store, rustc, installer, stdlib runtime, layering guards and other tests the cutover does not touch. Listed so the total reconciles.",
    "unreviewed": "Nobody has read the file yet. The mechanical proposal is recorded in the notes when there is one; the maintainer works these rows through.",
}

TWIN_MEANING = (
    ("`path::fn`", "a `keep` or `re-point` test that proves the same behaviour."),
    ("a fixture root", "a declared `.incn` fixture root, by its bare path, when running that root's programs proves the behaviour."),
    (
        "a behaviour fixture",
        f"a file or directory under `{BEHAVIOR_FIXTURES_ROOT}/<area>/`: an Incan program whose header declares its expected observables and names the tests it retires in `# retires:` lines. The gate refuses a fixture and a row that do not name each other.",
    ),
    (
        "`dies`",
        "the test has no user-observable behaviour to twin; the reason is recorded in a `dies` field beside it and the page shows it. Generated projects, `inspect rust` output and the build-report Cargo fields die with #654 (no Rust is generated at all any more); a data-structure invariant of a dying crate dies with the crate.",
    ),
)

LANE_MEANING = {
    "codegen": "calls a codegen API (`IrCodegen`, `try_generate`, `generate_rust`, `emit_program`, `read_generated_rust`) or reads generated Rust (`target/incan/<project>/src/*.rs`, `incan --emit-rust`)",
    "snapshot": "asserts an `insta` snapshot",
    "generated_text": "asserts on generated Rust text (`contains(\"fn \")`, `contains(\"impl \")` and the like)",
    "build_run": "builds or runs generated Rust (`run_explicit_oven_bake`, `compare_source_observable`, a `cargo`/`rustc` command, or a CLI invocation such as `run_incan` / `incan_command` beside a `build`, `run`, `test` or `bake` subcommand or a generated-target read; `incan fmt`, `incan check`, `--help` and `--version` alone do not count)",
    "replacement": "uses the replacement route or Body IR (`replacement::`, `shadow_support`, `body_ir`, `lower_typed_body_ir`, `execute_free_function`)",
    "checker": "typechecks or reads diagnostics (`TypeChecker`, `check_str`, `CompileError`, `CompilationSession`)",
    "parser": "lexes or parses (`parser::parse`, `parse_str`, `lexer::lex`)",
    "legacy_ir": "lowers through the Rust-source backend's own IR (`AstLowering`, `lower_program`, `IrProgram`, `IrType`), which #654 removes with the emitter",
    "formatter": "formats source (`format_source`, `incan_format`)",
    "lsp": "drives the language server (`incan_lsp::`, `tower_lsp`, `lsp_types::`, hover and completion params)",
}


def crate_of(rel_path: str) -> str:
    """The crate directory a file belongs to: `loaves/<ring>/<crate>` or `workspaces/<name>`."""
    parts = rel_path.split("/")
    if parts[0] == "loaves":
        return "/".join(parts[:3])
    return "/".join(parts[:2])


def plural(count: int, noun: str) -> str:
    """`1 file`, `2 files`."""
    return f"{count} {noun}" if count == 1 else f"{count} {noun}s"


def md_cell(text: str) -> str:
    """Escape a value for a Markdown table cell."""
    return text.replace("|", "\\|").replace("\n", " ").strip() or "-"


def code(text: str) -> str:
    """Wrap a non-empty value in backticks; an empty value renders as a dash."""
    return f"`{text}`" if text else "-"


def signals_cell(file: ScannedFile) -> str:
    """Compact lane summary for a file: `codegen 12, text 8`; a dash when no lane fires."""
    totals = file.lane_totals
    parts = [f"{lane.replace('generated_text', 'text').replace('build_run', 'run')} {totals[lane]}" for lane in LANE_ORDER if totals.get(lane)]
    return ", ".join(parts) or "-"


def twin_coverage(file: ScannedFile, entry: dict) -> tuple[int, int, int]:
    """`(twinned, dies, retire_total)` for the retire-class tests of a file."""
    fates = Counter(retire_fate(entry, key) for key in file.keys if effective_disposition(entry, key) == "retire")
    return fates["twinned"], fates[DIES], sum(fates.values())


def split_cell(entry: dict) -> str:
    """The split column: the planned target, `required` when none is planned yet, or a dash."""
    if not entry.get("split_required", False):
        return "-"
    target = entry.get("split_target", "")
    return f"planned: {target}" if target else "required"


def file_row(file: ScannedFile, entry: dict | None) -> str:
    """One table row for a test file."""
    if entry is None:
        return (
            f"| `{file.path}` | {len(file.tests)} | {file.lines} | {file.test_lines} | **unclassified** | - | - | - | - | "
            f"{signals_cell(file)} | no row in dispositions.json |"
        )
    disposition = entry.get("disposition", "unreviewed")
    overrides = entry.get("tests", {})
    disposition_cell = disposition
    if overrides:
        distinct = Counter(effective_disposition(entry, key) for key in file.keys)
        disposition_cell = f"{disposition} (" + ", ".join(f"{name} {count}" for name, count in sorted(distinct.items())) + ")"
    twinned, dies, retire_total = twin_coverage(file, entry)
    twins = f"{twinned}/{retire_total}" if retire_total else "-"
    dies_cell = str(dies) if retire_total else "-"
    owner = f"#{entry['owner']}" if entry.get("owner") else "-"
    return (
        f"| `{file.path}` | {len(file.tests)} | {file.lines} | {file.test_lines} | {disposition_cell} | {twins} | "
        f"{dies_cell} | {split_cell(entry)} | {owner} | {signals_cell(file)} | {md_cell(entry.get('notes', ''))} |"
    )


FILE_TABLE_HEADER = (
    "| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |",
    "|---|---:|---:|---:|---|---:|---:|---|---|---|---|",
)


def override_rows(file: ScannedFile, entry: dict) -> list[str]:
    """Per-test rows for a file that carries per-test overrides, in file order."""
    overrides = entry.get("tests", {})
    rows = [
        f"| Test | Disposition | Twin | Dies | Lanes | Notes |",
        f"|---|---|---|---|---|---|",
    ]
    for test in file.tests:
        override = overrides.get(test.qualified)
        if override is None:
            continue
        lanes = ", ".join(test.lanes) or "-"
        rows.append(
            f"| `{test.qualified}` | {effective_disposition(entry, test.qualified)} | "
            f"{code(effective_twin(entry, test.qualified))} | {md_cell(effective_dies(entry, test.qualified))} | "
            f"{lanes} | {md_cell(override.get('notes', ''))} |"
        )
    return rows


def render(corpus: Corpus, dispositions: dict) -> str:
    """The whole page as Markdown."""
    entries = dispositions.get("files", {})
    threshold = int(dispositions.get("split_threshold_lines", 1500))
    totals = disposition_totals(corpus, dispositions)
    total_tests = corpus.total_tests
    fixture_cases = sum(root.cases for root in corpus.fixture_roots)

    # ---- Per-disposition file counts (a file counts under its default) ----
    file_counts: Counter = Counter()
    for file in corpus.files:
        entry = entries.get(file.path)
        file_counts[entry.get("disposition", "unreviewed") if entry else "unclassified"] += 1
    fixture_counts: Counter = Counter()
    for root in corpus.fixture_roots:
        fixture_counts[root.disposition] += root.cases

    # ---- Retire fates and split flags across the corpus ----
    fates = retire_totals(corpus, dispositions)
    retire_total = sum(fates.values())
    open_files = []
    split_files = []
    split_durable = []
    unreviewed_files = []
    for file in corpus.files:
        entry = entries.get(file.path)
        if entry is None:
            continue
        twinned, dies, retire_count = twin_coverage(file, entry)
        if retire_count > twinned + dies:
            open_files.append(file.path)
        if entry.get("split_required", False):
            split_files.append(file.path)
            if entry.get("disposition") in DURABLE_DISPOSITIONS:
                split_durable.append(file.path)
        if entry.get("disposition") == "unreviewed":
            unreviewed_files.append(file.path)

    out: list[str] = []
    out.append("# Test corpus inventory")
    out.append("")
    out.append('!!! warning "Generated control-plane reference"')
    out.append("")
    out.append(
        "    Do not edit this page by hand. Regenerate it with `make test-inventory` from the tests in the tree and "
        "`scripts/test_inventory/dispositions.json`; `make test-inventory-check` fails when it is stale."
    )
    out.append("")
    out.append(
        "    Temporary. This page, the dispositions record and the `test-inventory` gate exist for the v0.6 cutover only: "
        "they retire with [#654](https://github.com/encero-systems/incan/issues/654) once no `retire` row remains and the "
        "durable corpus is frozen. Nothing here is a lasting contributor contract."
    )
    out.append("")
    out.append(
        "This is the control plane for the slice-7 cutover (issue [#1561](https://github.com/encero-systems/incan/issues/1561), "
        "test corpus). Every Rust test under `loaves/` and `workspaces/` has a disposition by what it proves and how, not by "
        "where it lives; every `.incn` fixture root is listed with its case count. A disposition is a recorded decision, "
        "the lane signals beside it are the mechanical evidence, and a `retire` row may be deleted only once it names a "
        f"twin or records `dies`. How to classify a test, record a twin, or plan a split is recorded on the owning issue: [working the test corpus inventory]({HOW_TO_LINK})."
    )
    out.append("")

    # ---- Summary ----
    out.append("## Summary")
    out.append("")
    out.append("| Disposition | Tests | Files | Fixture cases |")
    out.append("|---|---:|---:|---:|")
    for name in DISPOSITIONS:
        out.append(f"| {name} | {totals.get(name, 0)} | {file_counts.get(name, 0)} | {fixture_counts.get(name, 0)} |")
    if totals.get("unclassified"):
        out.append(f"| **unclassified** | {totals['unclassified']} | {file_counts.get('unclassified', 0)} | 0 |")
    out.append(f"| **Total** | **{total_tests}** | **{len(corpus.files)}** | **{fixture_cases}** |")
    out.append("")
    out.append(
        f"- Retire-class tests: {retire_total}, of which twinned {fates['twinned']}, dies {fates[DIES]}, "
        f"open {fates['open']} (neither yet)."
    )
    out.append(
        f"- Retire-class files with open rows: {len(open_files)} (a file whose retire tests are all twinned or "
        f"recorded `dies` is done)."
    )
    out.append(
        f"- Files whose test region exceeds the split threshold of {threshold} lines: {len(split_files)}, "
        f"of which {len(split_durable)} in the durable corpus (keep or re-point)."
    )
    out.append(f"- Unreviewed files: {len(unreviewed_files)}.")
    out.append("")

    # ---- Vocabulary ----
    out.append("## Dispositions")
    out.append("")
    out.append("| Disposition | Meaning |")
    out.append("|---|---|")
    for name in DISPOSITIONS:
        out.append(f"| `{name}` | {DISPOSITION_MEANING[name]} |")
    out.append("")
    out.append("## Twins and `dies`")
    out.append("")
    out.append(
        "A `retire` row leaves the corpus by naming what proves the behaviour after the cutover in its `twin` field, "
        "or by recording that nothing user-observable is lost. `Twins` counts the first kind against the file's "
        "retire-class tests; `Dies` counts the second."
    )
    out.append("")
    out.append("| `twin` | Meaning |")
    out.append("|---|---|")
    for spelling, meaning in TWIN_MEANING:
        out.append(f"| {spelling} | {meaning} |")
    out.append("")
    out.append("## Lane signals")
    out.append("")
    out.append(
        "The collector counts these in the text of each test function and of the file-local helpers it calls. They are "
        "evidence, not the verdict: the disposition column is what the reviewer recorded. Helpers that live in a "
        "`#[path = \"support/...\"]` module outside the file are invisible to the scanner, so a test that drives the "
        "shadow comparison through such a helper shows only the lanes its own text carries. A `#[cfg_attr(..., test)]` "
        "attribute and a one-line `#[test] fn ...` are not counted; the tree has neither."
    )
    out.append("")
    out.append("| Signal | Fires when the test |")
    out.append("|---|---|")
    for lane in LANE_ORDER:
        out.append(f"| `{lane}` | {LANE_MEANING[lane]} |")
    out.append("")

    # ---- Fixture roots ----
    out.append("## Fixture roots")
    out.append("")
    out.append(
        "`.incn` fixtures are programs; the compiler suite, the example runner and the verified documentation examples "
        "run them as programs, so they are inventoried by root rather than per file."
    )
    out.append("")
    out.append("| Root | Pattern | Cases | Disposition | Owner | Notes |")
    out.append("|---|---|---:|---|---|---|")
    for root in corpus.fixture_roots:
        owner = f"#{root.owner}" if root.owner else "-"
        out.append(
            f"| `{root.root}` | `{root.pattern}` | {root.cases} | {root.disposition} | {owner} | {md_cell(root.notes)} |"
        )
    out.append("")

    # ---- Files by crate ----
    by_crate: dict[str, list[ScannedFile]] = defaultdict(list)
    for file in corpus.files:
        by_crate[crate_of(file.path)].append(file)

    def crate_totals(files: list[ScannedFile]) -> Counter:
        counts: Counter = Counter()
        for file in files:
            entry = entries.get(file.path)
            if entry is None:
                counts["unclassified"] += len(file.tests)
                continue
            for key in file.keys:
                counts[effective_disposition(entry, key)] += 1
        return counts

    out.append("## Test files by crate")
    out.append("")
    out.append(
        "`Lines` is the file length; `Test lines` is the test region the split threshold applies to: the `#[cfg(test)]` "
        "modules when the file has any, otherwise the whole file. `Twins` is `twinned/retire-class` and `Dies` the "
        "number recorded `dies`, for files with retire-class tests. Per-test rows follow a file only when it carries "
        "per-test overrides."
    )
    out.append("")
    affected_crates = []
    unaffected_crates = []
    for crate, files in sorted(by_crate.items()):
        counts = crate_totals(files)
        if set(counts) <= {"unaffected"}:
            unaffected_crates.append((crate, files, counts))
        else:
            affected_crates.append((crate, files, counts))

    def crate_section(crate: str, files: list[ScannedFile], counts: Counter, level: str) -> list[str]:
        summary = ", ".join(f"{name} {counts[name]}" for name in DISPOSITIONS + ("unclassified",) if counts.get(name))
        section = [
            f"{level} `{crate}` ({plural(sum(len(f.tests) for f in files), 'test')} in {plural(len(files), 'file')}: {summary})",
            "",
            *FILE_TABLE_HEADER,
        ]
        ordered = sorted(files, key=lambda f: f.path)
        section.extend(file_row(file, entries.get(file.path)) for file in ordered)
        section.append("")
        for file in ordered:
            entry = entries.get(file.path)
            if not entry or not entry.get("tests"):
                continue
            section.append(f"Per-test overrides in `{file.path}`:")
            section.append("")
            section.extend(override_rows(file, entry))
            section.append("")
        return section

    for crate, files, counts in affected_crates:
        out.extend(crate_section(crate, files, counts, "###"))

    if unaffected_crates:
        unaffected_tests = sum(sum(len(f.tests) for f in files) for _, files, _ in unaffected_crates)
        unaffected_files = sum(len(files) for _, files, _ in unaffected_crates)
        out.append(f'??? note "Unaffected crates ({plural(unaffected_tests, "test")} in {plural(unaffected_files, "file")})"')
        out.append("")
        out.append(
            "    Every test in these crates is `unaffected`: the cutover does not touch them. They are listed so the "
            "summary reconciles to the whole tree."
        )
        out.append("")
        # The collapsed block is an admonition: every line inside it is indented by four spaces.
        for crate, files, counts in unaffected_crates:
            out.extend("    " + line if line else "" for line in crate_section(crate, files, counts, "####"))
    return "\n".join(out).rstrip("\n") + "\n"


def page_staleness(corpus: Corpus, dispositions: dict) -> str | None:
    """None when the page on disk matches the render, otherwise the gate failure line."""
    rendered = render(corpus, dispositions)
    if not PAGE_PATH.exists():
        return f"rendered page missing: `{PAGE_PATH.relative_to(ROOT).as_posix()}` (run `make test-inventory`)"
    if PAGE_PATH.read_text(encoding="utf-8") != rendered:
        return f"rendered page is stale: `{PAGE_PATH.relative_to(ROOT).as_posix()}` (run `make test-inventory`)"
    return None


def parse_args(argv: list[str]) -> argparse.Namespace:
    """Command-line options."""
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="exit non-zero when the page on disk is stale")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    """Entry point."""
    args = parse_args(sys.argv[1:] if argv is None else argv)
    dispositions = load_dispositions()
    corpus = collect(dispositions)
    if args.check:
        stale = page_staleness(corpus, dispositions)
        if stale:
            print(stale)
            return 1
        print(f"{PAGE_PATH.relative_to(ROOT).as_posix()} is current")
        return 0
    PAGE_PATH.write_text(render(corpus, dispositions), encoding="utf-8")
    print(f"wrote {PAGE_PATH.relative_to(ROOT).as_posix()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
