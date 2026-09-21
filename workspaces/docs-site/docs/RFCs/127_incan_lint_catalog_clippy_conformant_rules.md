# RFC 127: The Incan lint catalog: clippy-conformant rules, `loaf.toml` configuration, `incan architect` as engine and fixer, `incan fmt` as canonicalizer

- **Status:** Planned
- **Created:** 2026-09-20
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (stdlib namespacing; the reserved `std` root that import ordering sections by)
    - RFC 031 (library system; the `pub::` root and compile-time import resolution)
    - RFC 053 (formatter vertical spacing buckets)
    - RFC 057 (`@rust.allow(...)` targeted Rust lint suppression)
    - RFC 105 (`incan architect` rule engine; the engine and finding contract this catalog is evaluated by)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (Oven-native Rust build facets; where #1698's `[rust.lints]` will be recorded)
    - #159 (trivia-aware formatter)
    - #1698 (`[rust.lints]` in `loaf.toml`)
- **Issue:** [#1703](https://github.com/encero-systems/incan/issues/1703)
- **RFC PR:** [#1702](https://github.com/encero-systems/incan/pull/1702)
- **Written against:** v0.7
- **Shipped in:** —

## Summary

This RFC defines one catalog of Incan lint rules and the contract that configures it. The catalog follows clippy's: where an Incan construct translates a Rust construct, the rule keeps clippy's name, group, and default level; where the construct exists but clippy's name would mislead, the rule is renamed and the correspondence is recorded; where the Rust construct has no Incan counterpart (ownership, borrowing, lifetimes, raw pointers, `unsafe`, `as` casts), the catalog says so instead of inventing an approximation. Levels (`allow`, `warn`, `deny`, `forbid`), groups (`correctness`, `suspicious`, `style`, `complexity`, `perf`, `pedantic`, `restriction`), priorities, and defaults are clippy's, and the configuration is an `[incan.lints]` table in `loaf.toml` beside the `[rust.lints]` table that governs a project's Rust facets, so one manifest carries both sides of a mixed project and they cannot drift apart. RFC 105's `incan architect` engine evaluates the catalog over compiler-backed facts, with `allow` and `warn` as advice and `deny` and `forbid` as enforcement, and `incan architect --fix` applies the safe structural rewrites the catalog marks as fixes, under the same levels and under the project's fix policy, which may keep a finding and decline its rewrite with `fix = false`. `incan fmt` is not a lint fixer: it canonicalizes representation only, applying the catalog's `format` class, which has no level and no configuration, so its output is identical for every project and `[incan.lints]` never reaches it; representation is trivia, layout, literal spelling, redundant parentheses, and the order of independent declarations, and a `format` entry never adds, removes, merges, or re-nests a node. Where this contract touches RFC 105, that RFC is amended in the same change rather than contradicted: findings are advisory by default and enforcement is a level, fixes are a mode of the same command with their own contract, and the open suppression question is answered. Every seed lint entry lands in preview and is promoted only after a calibration run over the toolchain's own corpus, so the catalog fails nothing on the day it lands. RFC 053's spacing rules and the style guide's layout rules are `format`-class entries with names, so #159's trivia-aware formatter is testable rule by rule, and RFC 057's `@rust.allow(...)` gains an Incan-side twin, `@allow(...)`, for targeted suppression. Ruff is the second reference, for the Python-shaped half of the surface: its fix-applicability vocabulary names the fix modes (`format` and `fix` are safe by construction, `suggest` is display-only, and there is no unsafe tier), its per-rule documentation template is what `incan explain` renders, its `per-file-ignores` becomes a per-path allow table, its `preview` gate becomes a flag on lint entries, and its isort rule family becomes a `format`-class normalization over Incan's namespace roots, where the section a module belongs to is decided by its root rather than by configuration.

## Core model

1. **One catalog.** Every configurable advisory rule and every formatter normalization is a catalog entry, in one compiler-owned catalog, with one name, one stable diagnostic code, one declared fact tier, one declared evaluator, one declared fix mode, and either one group with one default level or membership in the `format` class. Diagnostics fall into three kinds: **errors and contract diagnostics** are the compiler's and their RFCs', are not configurable, and are outside the catalog; **catalog findings** are configurable policy and advice, with a level; the **`format` class** is canonical representation, applied unconditionally, with no level. The four checker warnings that stay outside the catalog are contract diagnostics.
2. **Clippy is the reference.** A rule that reports the same defect on the translated construct carries clippy's name and sits in clippy's group at clippy's default level. Renames and non-translations are recorded in the catalog, not left to folklore, and a rule that clippy has no notion of is named for the Incan construct it reports on.
3. **Three tools, three jobs.** `incan check` reports compiler facts and language errors produced during parsing, resolution, typing, and flow analysis. `incan architect` evaluates the catalog and reports findings, and under `--fix` applies the catalog's safe structural fixes, formats the result, and evaluates it again. `incan fmt` canonicalizes representation. None of the three duplicates another's analysis: the engine is RFC 105's, the formatter applies the `format` class and calls no lint rule, and `--fix` output is itself formatted before it is written.
4. **Levels are clippy's.** `allow`, `warn`, `deny`, and `forbid` mean what they mean in rustc and clippy; groups carry a priority so a single rule can override its group; `all` is a selector for the default-on groups, not a group. Advice is `allow` and `warn`; enforcement is `deny` and `forbid`.
5. **One manifest.** Levels and rule parameters live in `loaf.toml` under `[incan.lints]`, and workspace policy under `[workspace.incan.lints]`, beside the `[rust.lints]` table of the same manifest. There is no `clippy.toml`, no `.incanfmt`, and no other side file, and `[incan.lints]` is never formatter configuration.
6. **Suppression is targeted.** `@allow("rule", reason="…")` on a declaration narrows a rule at the smallest scope, exactly as RFC 057's `@rust.allow(...)` does for the generated Rust of that declaration.
7. **Rewrites are two.** `incan fmt` canonicalizes representation: it applies the `format` class and nothing else, so its output is the same for every project. Representation is trivia, layout, the spelling of a literal, redundant parentheses, and the order of independent declarations; a `format` entry never adds, removes, merges, or re-nests a statement or expression node. `incan architect --fix` applies safe structural fixes: a `fix`-mode entry is decidable on the syntax tree alone, meaning-preserving for every well-typed program in every configuration, idempotent, and structure-preserving, and it is applied under the lint levels, so `allow` means not applied, and under the project's fix policy, so `fix = false` on the entry means reported and not applied. Detecting a finding and deciding to rewrite it are separate decisions, as RFC 105 holds. No other command rewrites source. In Ruff's vocabulary both are safe fixes and every suggestion is display-only; the catalog has no unsafe tier, because the line is drawn by construction, not by judgment per rule.
8. **Every rule explains itself.** Each entry carries the documentation `incan explain` renders, in one template: what it does, why it matters, an example with its fix, and its options. A rule without that documentation is not a catalog entry.
9. **Every rule earns its default.** A lint entry enters the catalog in preview with positive fixtures, negative fixtures for the counterexamples its risks name, and a tested example pair; it is promoted to its group's default level only after a calibration run over the toolchain's own corpus with every finding triaged. A default level is a measured claim, not a guess, and the seed catalog of this RFC fails nothing on the day it lands.

## Motivation

Incan today decides "style" in three places, and none of them is a catalog. The formatter normalizes layout by rules stated in RFC 053 and the style guide, but reports every deviation as "file would be reformatted". The checker emits one lint-shaped diagnostic, unreachable code after `return` (`INCAN-T0101`), and four contract diagnostics about interop and async (an async call that is not awaited, a `rust.module()` directive with no effect, a public function that calls a checked C symbol, dot-notation in a `rust` import), and only the first has any business having a level. RFC 105 proposes an engine for design findings with its own categories and leaves suppression, configuration, and the relationship to formatting unresolved. A project that also carries Rust facets configures clippy in a fourth place. A Rust developer who reaches for `needless_bool` or `unwrap_used` finds nothing by that name; a Python developer who expects Ruff's "format and lint in one tool" finds a formatter that does not lint.

Clippy is the right reference for three reasons. First, expectations: Incan's audience includes Rust developers, and a rule that has the same name and the same meaning as the clippy lint they already know costs nothing to learn. Second, the compiler itself: the toolchain's own Rust facets are linted with clippy, and #1698 moves that configuration into `loaf.toml` as `[rust.lints]`; if Incan's rules used other names, levels, or defaults, one repository would carry two vocabularies for the same intent. Third, calibration: clippy's group boundaries (`correctness` denied; `suspicious`, `style`, `complexity`, and `perf` warned; `pedantic` and `restriction` opt-in) encode years of judgment about what a default-on lint may cost, so adopting them means Incan starts from a defensible default set rather than inventing one. Ruff, which grew up on Flake8's letter-prefixed codes, is converging on the same grouping: its preview rule categories are `correctness`, `suspicious`, `complexity`, `performance`, `style`, `security`, `formatting`, `pedantic`, and `restriction`, with the first five as the default set and a stated plan to retire the per-tool linter groups. Two tools with different histories arriving at one taxonomy is a second, independent vote for it.

Clippy is not a template to copy blindly. A large share of its catalog exists because Rust exposes ownership, borrowing, lifetimes, raw pointers, `unsafe`, and `as` casts. Incan exposes none of those: the compiler's duckborrowing planner decides how generated Rust moves, borrows, or clones, and source code never spells a borrow. An honest catalog lists those lints as untranslatable by construction rather than inventing lookalikes, and it adds the rules a Python-shaped language needs that clippy has no notion of: f-strings, comprehensions and generator expressions, docstrings, decorators, `mut self`, `pass`, and `...` bodies. Ruff is the reference for that half, and the prior-art section says which of it this RFC takes: not its hundreds of pycodestyle layout codes, which a canonical formatter makes unnecessary, and not its `noqa` culture, but its fix-safety vocabulary, its rule documentation template, its import-sorting family, and its per-file configuration shape.

## Goals

- Define the rule model: identity, naming, groups, levels, priority, defaults, declared facts, declared evaluator, and declared fix mode.
- Publish the correspondence between clippy's catalog and Incan's, group by group, in three dispositions: translates, translates under another name, does not translate.
- Name the entries without a clippy counterpart: the `format` class from RFC 053 and the style guide, the rustc lints that translate, and the Python-shaped idiom rules.
- Define `[incan.lints]` and `[workspace.incan.lints]` in `loaf.toml`, their precedence as a lattice, the per-path allow table, and the manifest refusals.
- Define `@allow(...)` as the Incan-side twin of `@rust.allow(...)`.
- Define how `incan architect` evaluates the catalog and applies its `fix`-mode entries under `--fix`, and how `incan fmt` applies the `format` class, including `--check` semantics, machine-readable output, and exit codes.
- Define the documentation every entry carries, in one template, and how `incan explain` renders it by code or by name.
- Define fix applicability in Ruff's terms, so a reader who knows safe, unsafe, and display-only fixes knows what `format`, `fix`, `suggest`, and `none` mean, and define fix policy, `fix = false`, as the project's say over whether a safe fix is applied.
- Define the `format` class as representation only, by a test a reader can apply to any candidate normalization, so that the formatter's scope is a definition rather than a list.
- Define the builtin-name guard, the tree walk that lets a `syntax`-tier entry recognize a builtin by name without name resolution, and say once when a method name on an unknown receiver is a `semantic` fact.
- Define preview gating for lint entries that are not yet stable, as eligibility distinct from level, and import ordering as a `format`-class normalization over Incan's namespace roots.
- Define the lifecycle of `incan architect --fix`: what is evaluated, what is applied, what is re-evaluated, and what the report and the exit status are computed from.
- Define what the documentation test of an entry proves, per declared evaluator, so that a lint example, a `format`-class example, and a `fix` example are each held to the contract their evaluator makes.
- Give #159's trivia-aware formatter its rule surface: every formatter normalization has a name, a code, and a reason.
- Define the quality gate every entry passes: fixtures, the tested example pair, preview on entry, and a calibration run before promotion.
- Define the narrow extension points this RFC adds to RFC 105 and amend that RFC in the same change so the two do not contradict.

## Non-Goals

- Changing language syntax or semantics. Every rule reports on programs the compiler already accepts.
- Replacing `incan check`. Type errors, resolution errors, and exhaustiveness are the checker's and stay errors, not lints.
- Redefining RFC 105's engine, Oven-selected project scope, provenance, evidence model, typed-fact contract, profiles, priorities, or confidence. RFC 105 defines the engine and base finding envelope; this RFC defines the catalog and policy that extend it. The ownership boundary is recorded under "Relationship to RFC 105"; RFC 105's default profile and baselines stay open there.
- Rewriting anything that cannot be decided from the syntax tree. `incan architect --fix` applies `fix`-mode entries and nothing else, and every `fix`-mode entry is safe by construction. There is no unsafe fix tier and no `--unsafe-fixes` flag: a later RFC may define type-dependent or judgment-dependent fixes, and it must do so by adding a fix mode, not by loosening `fix`.
- Making the formatter a linter. `incan fmt` applies the `format` class, reads no level table, and never restructures code.
- Linting generated Rust. `[rust.lints]` (#1698, to be recorded in RFC 119) and `@rust.allow(...)` (RFC 057) own the Rust side.
- A `# fmt: off` region, an `# isort: skip` marker, or any other formatter opt-out. None exists today and this RFC adds none.
- Statement-level suppression. `@allow(...)` attaches to declarations and modules, and the per-path table covers directories; a statement-scope directive is not added, as the design decisions record. A blanket line directive that names no rule (Ruff's bare `# noqa`) is rejected outright, because a suppression that names nothing suppresses everything.
- A tool that inserts suppressions (Ruff's `--add-noqa`). Suppressions are written by hand, with a reason.
- Porting all of clippy. The catalog grows by correspondence entries; this RFC fixes the model and seeds it with the entries in the tables below.
- Nursery and cargo lints. Nursery lints are not stable in clippy; cargo lints check `Cargo.toml`, whose Incan counterpart is validated by RFC 117 itself.

## Guide-level explanation

### One catalog, three tools

A rule in the catalog is something like `needless_bool`. It reports an `if` whose two branches only return `true` and `false`:

```incan
def is_adult(age: int) -> bool:
    if age >= 18:
        return true
    else:
        return false
```

`needless_bool` is a clippy lint. It translates: the construct is the same, the defect is the same, and the fix is the same, `return age >= 18`. So the Incan rule has the same name, sits in the same group (`complexity`), and has the same default level (`warn`).

One command acts on the rule, in two modes. `incan architect` evaluates the catalog and reports the finding in RFC 105's shape:

```text
[P3] complexity.needless_bool (warn): an `if` whose branches only return `true` and `false`
Suggestions:
  - Return the condition itself: `return age >= 18`
Evidence:
  - src/users.incn:12:5 in is_adult (fix: `incan architect --fix`)
```

`incan architect --fix` rewrites it, because the catalog gives `needless_bool` the fix mode `fix`: the rewrite needs no type information, it means the same thing in every configuration the file can be compiled under, it is idempotent, and it preserves the structure of everything around it. The rewritten file is run through the formatter before it is written, so the result is canonical, and then evaluated again, so the report says what the file looks like now: the `needless_bool` finding is listed as applied, and whatever the final evaluation finds is listed as remaining. `--fix` is applied under the same levels as the report: a project that sets `needless_bool = "allow"` neither sees the finding nor gets the rewrite. A project that wants the finding and not the rewrite says so on the entry: `needless_bool = { level = "warn", fix = false }` reports it and never applies it, because detecting a finding and deciding to fix it are separate decisions, which is RFC 105's principle kept.

`incan fmt` is not involved. The formatter canonicalizes representation and nothing else: blank lines, indentation, quotes, parentheses, digit grouping, import order. Its output is the same for every project, whatever `[incan.lints]` says, because nothing in that table reaches it. `incan fmt --check` fails only on the `format` class, the way it already fails on a missing trailing newline:

```text
src/users.incn:3:1 fmt_import_order: `import std.time` sorts before `from models import User`
src/users.incn:20:1 fmt_blank_lines: three blank lines between top-level declarations; at most two
1 file would be reformatted
```

The third command, `incan check`, is not involved either. It reports what the type system knows, and `is_adult` typechecks.

The same catalog holds rules nothing rewrites. `too_many_arguments` needs a threshold and a human decision; `unwrap_used` is a policy, not a rewrite; `float_cmp` needs to know that both operands are floats. `incan architect` reports those, and the level table decides whether they are advice (`warn`) or enforcement (`deny`). The defaults are clippy's, and clippy denies `correctness`: once `eq_op` and `never_loop` are promoted, a project that writes no `[incan.lints]` at all and contains one fails `incan architect` outright. That is the reference's calibration, adopted as it stands, and it is the one way a default project can fail the command with no configuration. It holds for promoted entries only. Every seed lint entry of this RFC enters the catalog in preview, where its default level is `allow`, and a `correctness` entry is promoted only after a calibration run over the toolchain's own corpus reports zero untriaged findings, so the catalog fails nothing on the day it lands and a default `deny` is a claim the corpus has been made to support.

### Reading a rule

Every entry documents itself in one template, and `incan explain` renders it for a code or a bare name:

```text
$ incan explain needless_bool
needless_bool (INCAN-L0042)  complexity  warn by default  clippy: needless_bool
facts: syntax  fix: fix (applied by `incan architect --fix`)  options: none

What it does
  Reports an `if` whose two branches only return `true` and `false`.

Why it matters
  The condition is already the boolean; the branches restate it.

Example
    if age >= 18:
        return true
    else:
        return false

Use instead
    return age >= 18

Fix
  `incan architect --fix` rewrites the `if`/`else` to `return <condition>`. The
  condition is a `bool` by its position, so the rewrite needs no type information.
```

The template is Ruff's ("What it does", "Why is this bad?", "Example", "Use instead", "Fix safety", "Options"), and it is the same text the generated catalog reference page carries, so the documentation site, the terminal, and an editor hover all say one thing.

### Configuring levels in `loaf.toml`

A project that carries Incan and Rust sources configures both in one manifest, with the same vocabulary:

```toml
[project]
name = "weather_service"
version = "0.6.0"

[incan.source]
root = "sources/incan"

[rust.source]
root = "sources/rust"

# Rust facets: rustc and clippy lints with Cargo's [lints] semantics (#1698).
[rust.lints]
unsafe_code = "forbid"
clippy.all = { level = "warn", priority = -1 }
clippy.pedantic = { level = "warn", priority = -1 }
clippy.unwrap_used = "deny"

# Incan sources: the same names, the same levels, the same priority rules.
[incan.lints]
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"
too_many_arguments = { level = "warn", threshold = 6 }
print_stdout = "warn"
fstring_over_concat = "allow"
needless_else = { level = "warn", fix = false }

# Paths where a rule is allowed: Ruff's per-file-ignores, with `allow` as its only verb.
[incan.lints.per-file-allow]
"tests/**" = ["unwrap_used", "print_stdout"]
```

The `[rust.lints]` shape is #1698's and appears here only to put the two tables side by side. `[incan.lints]` sits where the manifest puts Incan's other facet tables, beside `[incan.source]`, and reads the way a Rust developer expects: a group at priority `-1` so the single rules below it win, a rule raised to `deny`, a rule with a parameter, an Incan-only rule switched off, and a rule kept as a finding whose rewrite `incan architect --fix` must not apply. A project that writes no `[incan.lints]` gets clippy's defaults. The per-path table reads the way a Python developer expects: a glob, and the rules that are allowed under it. It is the manifest-side counterpart of a module-level `@allow(...)`, for the case where every file under `tests/` would otherwise carry the same decorator. Nothing in either table is formatter configuration: `incan fmt` never reads them, and two projects with different tables format identically.

Ruff's `select` and `ignore` lists are not a second spelling of this table. `select = ["pedantic"]` is `pedantic = "warn"`, `ignore = ["print_stdout"]` is `print_stdout = "allow"`, and Ruff's rule that a narrower selector beats a broader one is what Cargo's `priority` says out loud. One table, one spelling.

### Suppressing at the smallest scope

When a rule is right in general and wrong for one declaration, the declaration says so:

```incan
@allow("too_many_arguments", reason="mirrors the wire format field by field")
def decode_header(version: int, flags: int, length: int, stream: int, offset: int, crc: int, padding: int) -> Header:
    ...
```

`@allow(...)` is the Incan twin of `@rust.allow(...)`: a compiler-owned decorator, string arguments that name catalog rules or groups, an optional `reason`, and an effect limited to the declaration it decorates. A `@rust.allow("clippy::unwrap_used")` on the same declaration still means the generated Rust; the two decorators never name each other's rules. The twin is not a copy: RFC 057 rejects keyword arguments, group names, and the module position, and the reference-level explanation says where and why `@allow(...)` differs.

When the reason goes away, so should the decorator: `unused_allow` reports an `@allow(...)` whose rules would not have fired, the way Ruff's `unused-noqa` retires a stale `# noqa`. There is no `# noqa` line directive. A decorator is a declaration's metadata: the checker validates its names, the codegraph records it, `unused_allow` retires it, and a reviewer finds it where the declaration is. A trailing comment is none of those, and a tool that writes them for you is how a codebase ends up with hundreds.

### What does not translate, and why

Clippy's `needless_borrow`, `ptr_arg`, `needless_lifetimes`, and `needless_pass_by_value` have no Incan rule because Incan source has no borrow, no lifetime, and no pass-by-value spelling to get wrong: parameters are values, `self` and `mut self` say whether a method mutates, and the compiler plans the Rust shape. Clippy's `needless_return` has no Incan rule because `return` is the only way to yield a value: there are no tail expressions to prefer. Clippy's `single_match` translates only for `Option`, because `if x is not None:` is a real narrowing form and there is no `if let` for an enum payload. Each of those facts is a row in the correspondence tables, so a Rust developer who types a clippy name into `[incan.lints]` is told exactly why it is not there.

### The formatter's own rules

RFC 053's three blank-line buckets, the trailing newline, docstring interior spacing, comment placement, the style guide's horizontal spacing, parenthesization, digit grouping, and import order are catalog entries in the `format` class, named `fmt_*`. A `format`-class entry has no level: it is always applied by `incan fmt`, it cannot be configured or suppressed, and `--check` reports it by name. Its value is identity: a formatter rewrite is no longer "the file changed" but `fmt_blank_lines` at a line, with a reason, which is what #159's trivia-aware formatter needs to be testable rule by rule. The test for whether a normalization belongs in the class is whether it is representation: the same program, spelled the canonical way. Representation is trivia, layout, the spelling of a literal (its quotes, its digit grouping, the `f` prefix of a literal with no placeholder), redundant parentheses, and the order of independent declarations, which imports are. A `format` entry never adds, removes, merges, or re-nests a statement or an expression node. Removing `((x))`'s extra parentheses, regrouping `1_00_000`, sorting imports, and dropping the `f` from a placeholder-free f-string are representation. Turning an `if`/`else` into `return c` is a structural change, and so, however small each looks, are removing a redundant `pass`, dropping a trailing bare `return`, splicing a nested f-string into its outer string, and merging two `from` statements for one module: each adds, removes, or re-nests a node, so each is a `fix`-mode entry that `incan architect --fix` applies under the levels, not a normalization the formatter applies to every file.

Ruff draws a similar line between `ruff format` and `ruff check --fix`, and pays for a seam: its formatter does not sort imports, so a project runs `ruff check --select I --fix` and then `ruff format`, in that order, and its documentation lists the lint rules that conflict with the formatter and must be switched off. Incan has the line without the seam. Import ordering is in the `format` class, so there is no lint step to run first; `incan architect --fix` runs its output through the formatter before writing it, so a fixed file is canonical without a second command; and no catalog entry may report a construct the `format` class normalizes, so the two never disagree and there is nothing to switch off.

## Reference-level explanation

### Rule identity and naming

Every catalog entry must have:

- a **name**: a `lower_snake_case` identifier, unique across the catalog, used in `[incan.lints]`, in `@allow(...)`, and in human output;
- a **stable diagnostic code**, never reused and never renumbered, resolved by `incan explain`. New lint entries receive one in the `INCAN-L` family (`INCAN-L0001`, `INCAN-L0002`, …); an entry promoted from an existing diagnostic (`unreachable_code`, `INCAN-T0101`) retains its code. `INCAN-L` is the allocation family for new entries, not the definition of an entry;
- either a **group** (exactly one) and a **default level**, or membership in the **`format` class**, which has neither;
- an **origin**: `clippy` (same name and meaning as the clippy lint), `clippy-renamed` (an Incan name with the clippy counterpart recorded), `rustc` (a rustc lint that translates), or `incan` (no counterpart);
- a **fact tier**, an **evaluator**, and a **fix mode** as defined below;
- zero or more **parameters**, each with a name, a type, and a default; a `format`-class entry has none;
- for a lint entry, a **preview** flag, `false` for a stable entry, as defined below; a `format`-class entry has no preview flag, because a formatter change ships as a formatter change (see "Compatibility and migration");
- **documentation** in the template defined under "Rule documentation" below.

The entry is catalog metadata: name, code, group or class, default level, fact tier, evaluator, fix mode, and, for a lint entry, the preview flag. A diagnostic instance is what an evaluation produces at a location, and it carries what the entry cannot: the effective `level` at that location and the presentation `severity` derived from it, by the table under "Machine-readable output" (a `format`-class finding under `incan fmt --check` is a `warning`). Severity is therefore never an entry field.

The `INCAN-L` family is a family of the compiler's diagnostic catalog in `incan_syntax::diagnostics`, beside `INCAN-P` (parser), `INCAN-T` (typecheck), `INCAN-I` (import), `INCAN-C` (tooling), and `INCAN-U` (unknown). Each catalog entry is projected into that catalog's entry shape (code, title, phase, summary, explanation, examples, common causes, fixes, documentation URL), which is the shape `incan explain` renders today, so a lint code needs no second lookup path and `incan explain INCAN-T0101` and `incan explain INCAN-L0042` go through one table. That shape's `severity` field is not filled from the entry: for a catalog entry it is the instance's, derived from the effective level at each finding. Ruff's letter prefix names the tool a rule came from (`F` for Pyflakes, `E` for pycodestyle, `I` for isort); the `L` says only "lint". The group is a catalog field, not a digit of the code, so an entry can change group without renumbering, which is what Ruff's own move from per-tool prefixes to categories would have needed.

Naming rules:

- A rule whose origin is `clippy` must use clippy's name verbatim and must report the defect clippy's lint reports, on the translated construct. A name may be reused from clippy only when the rule is the same; the catalog must not carry a clippy name with a different meaning.
- A rule whose origin is `clippy-renamed` must not collide with any current clippy lint name, and its catalog entry must record the clippy lint it corresponds to and the reason for the different name.
- A rule whose origin is `incan` must not collide with any current clippy lint name. `format`-class entries use the `fmt_` prefix, and no entry outside the class may; other Incan-only rules are named for the Incan construct they report on (`fstring_`, `comprehension_`, `docstring_`, `ellipsis_`, and so on).
- A `format`-class entry that corresponds to a clippy lint (`fmt_parentheses` to `precedence` and `double_parens`, `fmt_digit_grouping` to `unreadable_literal` and `inconsistent_digit_grouping`) records the correspondence with origin `clippy-renamed`, so a manifest key naming the clippy lint is answered with the `fmt_*` entry and the fact that it has no level.
- The catalog records the clippy release it was last reconciled against. A conformance check must verify every `clippy` and `clippy-renamed` entry against that release's lint list.
- Every finding carries `qualified_rule`, the group-qualified rule name (`complexity.needless_bool`; `format.fmt_blank_lines` for the class). The qualifier is derived from the entry's group or class and is never authored by users. Catalog findings additionally carry `rule`, the bare name used by configuration and suppression, and `code`, the stable diagnostic code.

### Groups, levels, priority, and defaults

The groups and their default levels mirror clippy's:

| Group | Default level | In `all` | Contents |
| --- | --- | --- | --- |
| `correctness` | `deny` | yes | code that is outright wrong or useless |
| `suspicious` | `warn` | yes | code that is most likely wrong or useless |
| `style` | `warn` | yes | code that should be written in a more idiomatic way |
| `complexity` | `warn` | yes | code that does something simple in a complex way |
| `perf` | `warn` | yes | code that can be written to run faster |
| `pedantic` | `allow` | no | rules that are strict or have occasional false positives |
| `restriction` | `allow` | no | rules that restrict a construct a project may want to forbid; enabled one rule at a time |
| RFC 105's `arch`, `safety`, `idiom`, `maintainability`, `risk` | as RFC 105 defines | no | evidence-backed design findings; RFC 105's `experimental` is a profile, not a category, and decides eligibility, not level |

**The `format` class.** Beside the groups the catalog has one class, `format`, and this paragraph is the one place its status is stated. A `format`-class entry (`fmt_*`) is a normalization `incan fmt` applies unconditionally. It has no level, is in no group, is not covered by `all`, takes no parameters, and has no entry in any level table, per-path table, or `@allow(...)`: it is never configurable and never suppressible, because it is representation, not policy. Every rule below that refuses a `format`-class name does so for that one reason, that there is nothing to set; the class is not a group with a special level.

Level semantics must match rustc's and clippy's:

- `allow`: the rule is not reported and `incan architect --fix` does not apply its fix. An implementation may skip an allowed rule entirely, except when suppression provenance is inspected, which evaluates it in probe mode (see `unused_allow` below).
- `warn`: the rule is reported; `incan architect`'s exit code is unaffected; `incan architect --fix` applies its fix when the entry's fix mode is `fix` and the fix policy below does not decline it.
- `deny`: the rule is reported; `incan architect` exits non-zero; `incan architect --fix` applies its fix when the entry's fix mode is `fix` and the fix policy does not decline it, after which the finding no longer exists.
- `forbid`: as `deny`, and no lower layer (a member manifest, a per-path table, or `@allow`) may relax it.

Priority semantics must match Cargo's `[lints]` table: every entry has an integer `priority`, `0` by default; entries apply in ascending priority order, so a higher priority wins; a group is expected to be given a negative priority so that single rules override it. A tie is a manifest refusal, not a warning (clippy's `lint_groups_priority` becomes a refusal because RFC 117 validates manifests strictly). Two entries tie when they are in the same table, at the same priority, both cover one rule, and disagree on its level: a group and one of its member rules, or two overlapping selectors such as `all` and `style`. Overlapping entries at the same priority and the same level are accepted. The refusal never reaches across layers: a workspace entry and a member entry at the same priority are not a tie, because priority resolution happens inside each manifest layer independently and the two results are combined by the lattice under "Workspace policy and precedence".

`all` is not a group. It is a selector for the default-on groups, `correctness`, `suspicious`, `style`, `complexity`, and `perf`, accepted as a level-table key where it is an alias for five group entries at one level and one priority. It never covers `pedantic`, `restriction`, the `format` class, or a preview entry. A rule belongs to exactly one group, and its default level is its group's default level.

**Preview entries: eligibility is not level.** Two predicates decide whether a rule produces findings. `eligible(rule, invocation)` says whether the invocation asks for the rule at all: a stable rule is eligible under the default invocation and under any `--profile` that covers its group; a preview rule is eligible only under RFC 105's `experimental` profile. `enabled(rule, configuration)` is the rule's effective level, resolved from the lattice below. A finding is produced iff `eligible(rule, invocation)` and the effective level is not `allow`.

A new lint entry enters the catalog with `preview = true` and stays in preview for at least one release. Every seed lint entry of this RFC enters that way, with one exception stated under "Diagnostics the checker emits today": an entry promoted from a diagnostic that already ships keeps shipping. While in preview it has a group, so its documentation and `--list-rules` show where it will sit, but its default level is `allow` whatever the group, and group membership never raises it: `pedantic = "warn"` does not turn on a preview `pedantic` entry, and neither does `all`. Naming the rule in a level table does, and `--profile experimental` makes it eligible; both are needed. Promotion clears the flag, gives the entry its group's default level, makes it eligible under the default invocation, and is a release-notes item, as it is in Ruff's versioning policy. Promotion is earned, not scheduled: it requires the calibration run defined under "Rule documentation", over the toolchain's own corpus, with every finding triaged, and for a `correctness` entry with zero untriaged findings, because a `correctness` entry is `deny` by default and a default `deny` that the corpus has not been made to pass is a claim the catalog cannot make. This is Ruff's `explicit-preview-rules` made the only mode: there is no `preview = true` manifest switch that enables every preview entry at once, because the manifest names rules and a switch would be a second way to enable them. An RFC 105 rule in the `experimental` profile is a preview entry of this catalog: the profile decides its eligibility, the level table decides its level.

Preview is a property of lint entries only. A `format`-class entry has no preview flag and no preview period: it has no level to hold at `allow` and no invocation to be ineligible under, because `incan fmt` applies the class unconditionally. A new `format`-class entry ships as a formatter change, listed in the release notes, and "Compatibility and migration" says what a project sees, which is a one-time reformat.

### Declared facts, evaluator, and fix mode

Every entry declares:

- **Fact tier**: `syntax` when the rule is decidable on the trivia-aware syntax tree of one file, or `semantic` when it needs resolved names, types, or other codegraph facts. A `syntax` rule must run on any file that parses; a `semantic` rule must be skipped, with a note in the output, for a file the checker rejects. The two tiers are availability classes, saying when a rule can run; they are not the engine's fact model. RFC 105's registry declares the facts each rule needs (resolved names, types, flow, codegraph, project graph, other rules' results), and this RFC does not restate that model: a `semantic` entry's registry declaration says which of those facts it consumes.
    - **The builtin-name guard.** A `syntax`-tier entry that recognizes a builtin function by its name (`str`, `int`, `float`, `bool`, `len`, `min`, `max`, `range`, `enumerate`, `sum`, `any`, `all`, `sorted`, `reversed`, `zip`, `abs`, `repr`) applies only where no enclosing lexical scope in the tree binds that name: a parameter, an assignment target, a `for`, `with`, `match`, or comprehension binding, an import, or a declaration. Where one does, the entry is silent at that site. The guard is a tree walk, not name resolution: every Incan binding is lexical and the parser refuses wildcard imports, so the tree shows every rebinding, and the walk needs neither the checker nor the codegraph. `print` needs no guard, because it is the only builtin protected from rebinding. The same walk covers a `std` module an entry recognizes through the import that binds its name (`invalid_regex`, `regex_creation_in_loops`): the import is on the tree, and so is any narrower rebinding. A row in the tables below marked `syntax, builtin-guarded` is an entry under this guard; a builtin that appears only in an entry's suggestion needs no guard, because a suggestion is display-only and the risk text names the rebinding.
    - **A method name on an unknown receiver is a `semantic` fact.** An entry that recognizes a method by its name (`.replace`, `.skip`, `.iter`, `.count`, `.for_each`, `.append`, `.lower`) on a receiver whose type the tree does not fix is a `semantic` entry, because a user-defined method may carry the same name, unless the entry's row says the shape is unambiguous by construction: the receiver is a literal (`xs = []` binds a `list`; `Some(1)` is an `Option`), or the entry is a `restriction` on the spelling itself and says so. A row that once assumed a receiver's type is re-tiered `semantic` below, and each such row says which method made it so.
    - **Operator equivalence requires resolved primitive operands.** RFC 028 permits user-defined operator methods, including equality, ordering, arithmetic, in-place arithmetic, and bitwise operations. An entry that claims an operator expression is constant, impossible, redundant, or equivalent to another spelling is therefore `semantic` unless the operands are literals whose primitive meaning the tree fixes. Its fact declaration must require resolved primitive operand types and, where relevant, exclude floating-point cases such as NaN. Layout-only rules may inspect operator spelling at the `syntax` tier because they make no semantic claim.
- **Evaluator**: `architect` (RFC 105's engine), `check` (the checker emits it as a by-product of resolution or flow analysis), or `fmt` (the formatter applies it as part of formatting; every `format`-class entry and only those).
- **Fix mode**: `format` (a `format`-class entry; `incan fmt` applies it unconditionally), `fix` (a safe structural rewrite; `incan architect --fix` applies it under the lint levels), `suggest` (the finding carries a suggestion and nothing rewrites source), or `none`.

The fix modes are Ruff's fix applicability levels with the safe tier split in two by who applies it and the unsafe tier removed:

| Fix mode | Ruff applicability | Who applies it | What the reader may assume |
| --- | --- | --- | --- |
| `format` | safe | `incan fmt`, always; no level | representation only: the same program, canonically spelled; comments preserved; idempotent |
| `fix` | safe | `incan architect --fix`, at any effective level other than `allow`, unless the project's fix policy declines it | meaning preserved for every well-typed program in every configuration, comments preserved, idempotent, structure preserved except at the reported construct |
| `suggest` | display-only | nobody; `incan architect` prints the replacement text | the suggestion may be wrong for the reasons the finding's risks name |
| `none` | no fix | nobody | the finding is a report |
| (not defined) | unsafe | nobody | Ruff applies these behind `--unsafe-fixes`; no Incan command does |

Ruff needs the unsafe level because its safe/unsafe line is drawn per rule by judgment and moved per project by `extend-safe-fixes` and `extend-unsafe-fixes`; its common reason for marking a fix unsafe is "comments may be dropped", which is a property of a formatter and a linter that hold two different trees. This catalog draws the line by construction: a `fix` is one the constraints below admit, a `format` entry is one that changes representation and nothing else, and every other rewrite is a suggestion. Fix safety is therefore a catalog property and not a project setting: there is no manifest key that promotes a suggestion to a fix or demotes a fix to a suggestion. Fix policy is a project setting, and it is the one RFC 105 asks for when it separates detecting a finding from deciding to fix it: a rule entry in `[incan.lints]` or `[workspace.incan.lints]` may carry `fix = false`, meaning the finding is reported at its level and `incan architect --fix` never applies its rewrite. `fix = true` is the default for a `fix`-mode entry, and `fix` is refused on an entry whose fix mode is not `fix`, because a suggestion cannot be promoted. Ruff's `unfixable` is this key said per entry; Ruff's `fixable` is its default. The name `unsafe` is reserved for a later RFC that defines type-dependent or judgment-dependent fixes, so that such fixes arrive as a new mode with its own flag rather than as a loosening of `fix`.

Constraints:

- A `format` entry and a `fix` entry both require the `syntax` tier: neither the formatter nor `--fix` may need the checker to decide a rewrite.
- A `fix` must be meaning-preserving for every well-typed program in every configuration the file can be compiled under, regardless of the types involved (a `syntax`-tier rule sees every conditional-compilation branch, not only the checked one), idempotent (`fix(fix(x)) == fix(x)`), and structure-preserving (the syntax tree after the rewrite differs from the tree before it only at the reported construct and its trivia). These are #159's invariants, stated per entry. Meaning-preserving is read strictly: a rewrite whose result differs from the original on any input is not a `fix`, even where the original fails on that input. `range_plus_one` and `range_minus_one` are the worked example: `range(a..(b + 1))` and `range(a..=b)` differ where `b + 1` overflows, and since integer overflow is not a documented language contract ([numeric semantics](../language/reference/numeric_semantics.md), "Current limitations"), the edge is an observable change and the two entries are suggestions.
- A `format` entry must satisfy the same invariants and one more: it may change how the program is written and nothing about its structure. Representation is trivia, layout, the spelling of a literal (quotes, digit grouping, the `f` prefix of a literal with no placeholder), redundant parentheses, and the order of independent declarations, which imports are; a `format` entry never adds, removes, merges, or re-nests a statement or expression node, and a rewrite that does is a `fix` however small it looks. Because a `format` entry has no level, it is applied to every file, so the bar is higher than `fix`'s, not lower. Where the admissibility of a normalization depends on the text it meets, the entry states the condition and the formatter proves it per construct, leaving the construct as written where the proof fails; `fmt_quote_style` is the worked example, re-quoting a literal only where the decoded value is unchanged, which the formatter establishes by lexing both spellings. A normalization the formatter cannot prove on the spot is not a `format` entry.
- Where a `fix` entry's rewrite is admissible only under a condition the tree can show, the entry states the condition as the fix's guard, applies the rewrite where the guard holds, and reports the finding without a fix where it does not. `nested_fstring` (a lex-and-compare of the spliced and the two-level spellings) and `str_in_fstring` (the builtin-name guard) are the worked examples; `list_init_then_append` reports without a fix where an appended expression mentions the list.
- The catalog is the only source of both sets. No command applies a rewrite for an entry whose fix mode is `suggest` or `none`; `incan fmt` applies only `format` entries; `incan architect --fix` applies only `fix` entries.
- No entry outside the `format` class may report a construct the class normalizes: a `fix` and a `format` entry must never both want the same construct, so the two never conflict and no ordering between the commands exists.

### The `[incan.lints]` table

`[incan.lints]` is a facet table in `loaf.toml` (RFC 117), namespaced the way the manifest namespaces every facet: `[incan.source]`, `[rust.source]`, `[interop.c]`, and so `[incan.lints]` beside `[rust.lints]`. It borrows Cargo's level and priority semantics, not Cargo's table hierarchy. Each key is a rule name, a group name, or `all`; each value is either a level string or an inline table:

```toml
[incan.lints]
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"
too_many_arguments = { level = "warn", threshold = 6 }
disallowed_names = { level = "deny", names = ["foo", "bar", "baz", "tmp"] }
```

Rules:

- The value must be one of `"allow"`, `"warn"`, `"deny"`, `"forbid"`, or an inline table with a required `level` of the same domain, an optional integer `priority`, an optional boolean `fix`, and, for a rule entry, that rule's declared parameters.
- A group entry must not carry parameters. It may carry `fix = false`, which applies to the group's `fix`-mode members.
- `fix` is fix policy, not fix safety. On a rule entry whose fix mode is `fix`, `fix = false` means the finding is reported at its level and `incan architect --fix` never applies its rewrite; `fix = true` is the default and may be written. On an entry whose fix mode is not `fix`, any `fix` value is refused: a suggestion cannot be promoted. Inside one table a rule entry and a group entry that disagree on `fix` resolve by priority as levels do; across layers `false` wins, as "Workspace policy and precedence" states.
- A rule entry may set a parameter only when the catalog declares it for that rule; the value must have the declared type. An unset parameter takes the catalog default.
- A key must name a configurable catalog entry, a group, or `all`. A `format`-class entry has no level, so a key naming one (or naming `format`) falls under the unknown-key refusal below, with a diagnostic that says the entry is canonical and has nothing to set.
- An entry for the `restriction` group as a whole must be refused: restriction rules are enabled individually, as clippy's `blanket_clippy_restriction_lints` insists.
- `[incan.lints]` never carries a Rust lint, and `[rust.lints]` never carries an Incan rule.
- There is no `select`, `ignore`, `extend-select`, `fixable`, `unfixable`, or `preview` key. Each is either the level table said another way (`select` and `ignore`), the per-entry `fix` key said as a list (`fixable` and `unfixable`), or a switch this RFC deliberately does not provide (`preview`). A key with one of those names is refused with a diagnostic that names the spelling to use.
- Nothing in the table is read by `incan fmt`.

### The per-path allow table

`[incan.lints.per-file-allow]` is a subtable of `[incan.lints]` whose keys are path globs and whose values are arrays of rule or group names. Its name is hyphenated so it can never collide with a rule name, which is always `lower_snake_case`.

```toml
[incan.lints.per-file-allow]
"tests/**" = ["unwrap_used", "print_stdout", "missing_docstrings"]
"src/generated/**" = ["too_many_lines"]
```

Rules:

- A glob is resolved against the directory of the manifest that declares it, matches `.incn` files, and uses `*`, `**`, and `?` with the meaning Cargo gives them in `[workspace] members` and Ruff gives them in `per-file-ignores`. A glob that matches no file is not a refusal, because the table describes policy, not inventory.
- Each value must name a configurable catalog rule or a group other than `all` and `restriction`, with the same rejections as `@allow(...)`: `all` is a blanket, and `restriction` is enabled one rule at a time. A `format`-class name is refused for the reason stated once above.
- The only effect is `allow`. The table cannot raise a level, set a parameter, carry a priority, or change fix policy; Ruff's `per-file-ignores` has the same single verb, and a per-path level table would be a second manifest inside the first.
- It may appear in `[workspace.incan.lints]` as well, where its globs resolve against the workspace root. A member's per-path table adds to the workspace's; neither can relax a `forbid`.
- `[incan.lints.per-file-allow]` is exactly a module-position `@allow(...)` applied to every file the glob matches. A file that carries both is allowed the union.

### Workspace policy and precedence

A workspace root (RFC 117) may declare `[workspace.incan.lints]` with the same shape. Lint policy is workspace policy: it applies to every selected member without an opt-in, because RFC 117 makes the root the authority for policy and a member may narrow but not silently replace it. The workspace table is the package-policy floor, subject to explicit scoped exemptions; member composition is monotonic strengthening; path and declaration suppression is scoped exemption.

Levels are ordered `allow < warn < deny < forbid`, and the effective level of a rule at a source location is a lattice join over the workspace's explicit assignments followed by one scoped exemption:

```text
member_level(rule)           = member(rule)              if the member's [incan.lints] assigns the rule
                             = catalog_default(rule)     otherwise

project_level(rule)          = max(workspace(rule), member_level(rule))   if [workspace.incan.lints] assigns the rule
                             = member_level(rule)                          otherwise

location_level(rule, loc)    = allow                     if targeted_allow(rule, loc) and project_level(rule) != forbid
                             = project_level(rule)       otherwise
```

where:

- `catalog_default(rule)` is the entry's default level (`allow` for a preview entry);
- `member(rule)` is the level the project's own `[incan.lints]` assigns the rule after priority resolution inside that one table, directly, by group, or by `all`; a project that is not a workspace member has only this layer, and a workspace member's table is this layer;
- `workspace(rule)` is the level `[workspace.incan.lints]` assigns the rule after priority resolution inside that one table, directly, by group, or by `all`; a rule the workspace table does not mention has no workspace policy. Priority resolution happens inside each manifest layer independently and never across layers;
- `targeted_allow(rule, loc)` holds when a per-path allow table of the workspace or the member matches the file containing `loc` and names the rule or its group, or when an `@allow(...)` on a declaration enclosing `loc` names the rule or its group.

The floor is the workspace's explicit assignments only. A rule the workspace names is joined with `max`, so a member can only raise it, and a member entry that assigns a level below the workspace's is refused as a manifest error rather than silently absorbed (see "Manifest refusals"). A rule the workspace does not name has no workspace policy, and the member decides it exactly as a standalone project would, including setting a default-on rule to `allow`. The consequence, stated plainly: a workspace that wants a floor on a rule must name it. Two lines show both halves:

```toml
[workspace.incan.lints]
unwrap_used = "deny"          # named: a member's `unwrap_used = "warn"` is refused; the floor holds
# fstring_over_concat is not named: a member's `fstring_over_concat = "allow"` holds
```

Fix policy composes on the same lattice with `false` on top: a rule's effective `fix` is `false` if the workspace table or the member table sets `fix = false` for it, directly or by group, and `true` otherwise, so a workspace `fix = false` is a floor a member cannot lift, and a per-path table or an `@allow(...)` cannot re-enable a declined rewrite, because an exemption only takes a location to `allow`. The exemption is what makes suppression targeted: a per-path table or an `@allow(...)` takes a location to `allow` and nothing else, and `forbid` is outside its reach, which is `forbid`'s meaning in rustc. Ruff resolves the same question by directory walk: the nearest `ruff.toml` wins, and `extend` inherits from a parent. That is not adopted, because RFC 117 already says who is the authority over whom, and a second, path-shaped precedence would let a nested file override the workspace's floor. This layering diverges from Cargo's `[lints] workspace = true`, which is all-or-nothing: a Cargo member either inherits the whole workspace table or writes its own, and cannot merge the two. The divergence is deliberate and is recorded in the design decisions. There are no command-line level flags: the manifest is the single source of truth, and a flag would be a second one.

### Manifest refusals

Oven must refuse a manifest, with the diagnostic naming the offending entry, when:

- a key names neither a configurable catalog rule, nor a group, nor `all`; when the key is a clippy lint the catalog records as renamed, the diagnostic must name the Incan rule; when it is a clippy lint the catalog records as untranslatable, the diagnostic must quote the recorded reason; when it names a `format`-class entry, or `format`, the diagnostic must say the entry is canonical and has no level;
- a level is not one of the four level strings;
- two entries tie as defined above: a group and one of its member rules, or two overlapping selectors, at the same priority with different levels;
- a workspace member's manifest assigns a rule a level below the one `[workspace.incan.lints]` assigns it; a rule the workspace table does not name has no floor and is not refused;
- an entry targets the `restriction` group as a whole;
- a parameter is unknown for the rule, has the wrong type, or appears on a group entry;
- `fix` appears on an entry whose fix mode is not `fix`, or its value is not a boolean; a workspace member's manifest sets `fix = true` on a rule that `[workspace.incan.lints]` sets `fix = false`, directly or by group;
- a key is one of the Ruff spellings this table does not carry (`select`, `ignore`, `extend-select`, `fixable`, `unfixable`, `preview`); the diagnostic names the spelling to use instead;
- the manifest carries a top-level `lints` table, Cargo's hierarchy with the language under the table rather than the table under the facet; the diagnostic names `[incan.lints]` or `[rust.lints]` as the spelling to use;
- a per-path allow entry names an unknown rule, `all`, `restriction`, or a `format`-class entry, or would relax a `forbid`, or its value is not an array of strings.

### `@allow(...)`

`@allow(...)` is a compiler-owned decorator, defined by analogy with RFC 057. The name `allow` is reserved: a user-defined decorator (RFC 036) must not take it. RFC 057's argument rules are the starting point, and this RFC departs from them in three places, each stated below with its reason:

- It must take one or more string literal arguments, each naming a catalog rule or group, and may take a `reason` keyword argument whose value is a string literal. An empty argument list, a non-string argument, a duplicate name, and an unknown name must be rejected. RFC 057 rejects every keyword argument; `@allow(...)` accepts exactly `reason`, because a suppression with a recorded reason is what `allow_without_reason` exists to ask for and what the finding's provenance reports.
- A group name is accepted, except `all`, which must be rejected: allowing every default-on rule for a declaration is a blanket, and the manifest is the place for blankets. RFC 057 rejects every group because a Rust lint group on generated code would hide Rust diagnostics the author cannot see; an Incan group on an Incan declaration hides nothing the author did not write.
- It may appear on the supported declarations, which are: a function, an `async def`, a method, a class, a model, a trait, an enum, a newtype, a type alias, a `const` or `static` binding, a `module tests:` block, and, in the module-decorator position, the module itself. It must be rejected on statements, expressions, imports, fields, variants, and any declaration not in that list. RFC 057 is item-only and forbids the module position, because a module-level Rust suppression would widen into crate-wide Rust warning policy; a module-level `@allow(...)` narrows an Incan rule for one file, which is the smallest scope a file-wide rule has.
- Its effect is the `targeted_allow` of the lattice above: the named rules are `allow` for the decorated declaration and everything lexically inside it. Naming a group allows every rule in the group. It never touches fix policy: a rewrite declined by `fix = false` stays declined, and an allowed rule has nothing to apply.
- Naming a rule whose project level is `forbid` must produce a diagnostic at the decorator, as rustc does for an `allow` under a `forbid`; the lattice already leaves such a location at `forbid`.
- Naming a `format`-class entry must be rejected, for the reason stated once under "The `format` class".
- `@allow(...)` never names a Rust lint; `@rust.allow(...)` never names an Incan rule. Each must reject the other's names.
- The restriction rule `allow_without_reason` reports an `@allow(...)` with no `reason`.
- The style rule `unused_allow` reports an `@allow(...)` naming a rule that would not have fired anywhere in the decorated scope, as Ruff's `unused-noqa` does for a stale `# noqa`. It is evaluated after every other rule, so its fact tier is `semantic` and its evaluator is `architect`; the finding names the unused rule and suggests removing the name or, when nothing is left, the decorator. Ruff keeps `unused-noqa` opt-in because a `# noqa` may be addressed to another tool (its `external` setting exists for that); `@allow(...)` names only catalog rules, so the entry is on by default.
- **Probe mode.** `allow` suppresses reporting, and an implementation may skip an allowed rule. When suppression provenance is inspected, which is what `unused_allow` does, the engine evaluates each suppressed rule over the suppressed scope in probe mode: the rule runs, its would-be findings are recorded against the decorator that suppressed them, and probe results never become findings. `unused_allow` reasons over the checked configuration, the same one the `semantic` tier evaluates. An `@allow(...)` whose rule would fire only in code under another conditional-compilation configuration is reported as unused until that configuration is the one being checked, and is stated to be so in the finding's note; it is not reported as used on the strength of a branch the checker did not compile.

### `incan architect` evaluates the catalog

`incan architect` (RFC 105) is the evaluator for every entry whose evaluator is `architect`:

- It must resolve the effective level of every rule from the lattice above and must not report a rule whose level at the location is `allow`; it may skip such a rule except in probe mode.
- It must include entries the checker emits (evaluator `check`) by projecting the codegraph's diagnostic records into findings rather than re-evaluating them, so a checker diagnostic is never reported twice.
- Findings must carry RFC 105's record (`qualified_rule`, category, priority, confidence, evidence, suggestions, risks) and, for catalog entries, `rule`, stable diagnostic `code`, the group, the effective level, and the manifest layer that set it.
- `--profile` decides eligibility; the level table decides the level; a finding needs both, as "Preview entries" defines. A `syntax`-tier rule must run over a file the checker rejects, and a `semantic`-tier rule must be reported as skipped for that file.
- `incan architect --list-rules` is a new flag on RFC 105's command surface, added by this RFC: it must print every catalog entry with its group or class and its fix mode, and, for a lint entry, its preview flag, its effective level, the layer that set it, whether it is eligible under the current invocation, and, for a `fix`-mode entry, whether its rewrite is declined by `fix = false` and by which layer; a `format`-class entry has none of those, and its row says so. With `--format json` it must print the same table as a record per entry. It is the one place that answers both of Ruff's questions, `ruff rule --all` (what exists) and `ruff check --show-settings` (what is on here).
- A preview entry is eligible only under `--profile experimental` and is enabled only when a level table names it; naming its group never enables it.

### `incan architect --fix` applies the safe fixes

`--fix` is a new flag on RFC 105's command surface, added by this RFC, and it is Ruff's `check --fix`: the same evaluation as `incan architect`, followed by the application of every finding whose entry has fix mode `fix`, and then by a second evaluation of what the application produced. The lifecycle is normative, and it is one pass per file:

1. **Evaluate the original tree.** Every eligible rule runs over the file as written, producing the original findings.
2. **Select and apply.** From the original findings, those whose entry has fix mode `fix`, whose effective level at the location is not `allow`, and whose effective fix policy is not `false` are selected, and their rewrites are applied to the tree, except where the entry's guard does not hold at that site, in which case the finding is reported without a fix. A finding at `allow` is neither reported nor fixed; a finding at `warn`, `deny`, or `forbid` is fixed unless `fix = false` declines it, in which case it is reported and left. An entry with fix mode `suggest` or `none` is reported as under `incan architect` and never applied.
3. **Format.** The `format` class runs over the resulting tree, so the file `--fix` leaves behind is canonical. The two rewrites cannot conflict because no `fix` entry may report a construct the `format` class normalizes; there is no order to document between them.
4. **Re-evaluate.** Every eligible rule runs again over the resulting tree, producing the final findings. This is what the file looks like now: a finding the rewrite created, a finding whose span moved because text above it changed, and a finding the rewrite did not remove are all here, at their final spans.
5. **Report.** The report is RFC 105's finding envelope and nothing beside it: no separate array of applied fixes. It holds every original finding whose fix was applied, marked `applied: true` and at its original span, and every finding of the final evaluation, marked `applied: false` and at its final span. Record identity in the final set is (`qualified_rule`, final span), so a rule that still fires at the same construct after its fix was attempted is one record of the final evaluation, not two; a `suggest` or `none` finding appears once, in the final set, because nothing was applied to it.
6. **Exit.** The exit status is computed from the final evaluation exclusively: `deny` or `forbid` among the final findings fails the command, and an applied finding, however it was leveled, does not.

Rules that hold across the lifecycle:

- The output must not depend on the order in which `fix` entries are applied; a pair of entries whose rewrites do not commute is a catalog defect, not a documented ordering. Applying the set must be idempotent as a whole: `incan architect --fix` twice must leave the file as once did, so the final evaluation of one run holds no `fix`-mode finding a second run would apply, and a rewrite that creates one is the same kind of catalog defect.
- `--fix --diff` must print the rewrite each fix would make and write nothing, and must exit non-zero when any fix is pending, so a CI job can ask whether a tree is fully fixed without changing it. Under `--diff` the lifecycle runs in memory through the re-evaluation: the report lists the original findings whose fix would be applied, marked `applied: false` at their original spans because nothing was applied, beside the final findings as under `--fix`; a pending fix is a finding step 2 would select, so a rewrite declined by `fix = false` or by the entry's guard is not pending, and the exit status follows the exit-code table, non-zero when any fix is pending or any final finding is at `deny` or `forbid`.
- A file with a syntax error is refused for fixing, as `incan fmt` refuses it; a file the checker rejects is fixed for `syntax`-tier entries only, which every `fix` entry is, and its `semantic`-tier rules are reported as skipped in both evaluations.

### `incan fmt` applies the `format` class

`incan fmt` keeps its command surface (`incan fmt [PATH]`, `--check`, `--diff`, `--workspace`, `--member`) and gains catalog identities:

- Formatting applies every `format`-class entry and nothing else. The formatter reads no level table, no per-path table, and no `@allow(...)`, and its output is the same for every project. It applies no `fix`-mode entry: the structural rewrites are `incan architect --fix`'s.
- `--check` must report each construct that formatting would change by rule name, location, and a one-line reason, and must exit non-zero when any file would change. It fails only on the `format` class; a pending `fix`-mode rewrite is not the formatter's to report.
- `--diff` shows the rewrite as today.
- `incan fmt` must not report entries it does not apply. Reporting is `incan architect`'s job.
- A file with a syntax error is refused, as today.
- Applying the class must be idempotent as a whole: running `incan fmt` twice must produce the same file as running it once.

### Machine-readable output

`incan fmt --check --format json` must emit the `incan check --format json` envelope, schema 2: `schema_version`, `ok`, and `diagnostics`, whose entries are diagnostic records in schema 2's shape: a stable `code`, `severity`, `phase`, `origin`, a `primary_span` with the file and `start` and `end` positions in line, column, and offset form, `message`, `notes`, `hints`, related spans, and the `incan explain` hook. Each catalog record must additionally carry `qualified_rule`, `rule`, `group` (the group name, or `format` for the class), `fix` (the entry's fix mode), and, for an entry with a level, `level` (the effective level at the location) and `level_source` (the layer that set it). `qualified_rule` is the group-qualified identity, `rule` is the bare catalog name used in configuration, and `code` is the stable diagnostic identity. `severity` is presentation derived from `level`: `warn` renders as `warning`, and `deny` and `forbid` render as `error`; a `format`-class record, which has no level, renders as `warning`, because a pending representation change is not an error in the program. `ok` is `true` iff no file would change. The `phase` of a formatter record is `format`. A consumer that reads `incan check --format json` today must read this report unchanged: the shared fields keep their names and shapes, and the added fields are ignorable.

```json
{
  "schema_version": 2,
  "ok": false,
  "diagnostics": [
    {
      "code": "INCAN-L0007",
      "qualified_rule": "format.fmt_blank_lines",
      "rule": "fmt_blank_lines",
      "group": "format",
      "fix": "format",
      "severity": "warning",
      "phase": "format",
      "origin": "fmt",
      "primary_span": {
        "file": "src/users.incn",
        "start": { "line": 20, "column": 1, "offset": 402 },
        "end": { "line": 22, "column": 1, "offset": 404 }
      },
      "message": "three blank lines between top-level declarations; at most two",
      "hints": ["remove one blank line"],
      "explain": "incan explain INCAN-L0007"
    }
  ]
}
```

Schema 2's `origin` is the producing tool (`fmt`, `check`, `architect`), as it is today; the catalog entry's origin (`clippy`, `clippy-renamed`, `rustc`, `incan`) is documentation, rendered by `incan explain`, and is not a report field.

`incan architect --format json` keeps RFC 105's finding record and adds the same catalog fields: `rule`, `code`, `group`, `fix`, `level`, and `level_source`; `qualified_rule` remains the RFC 105 identity. `severity` is derived from `level` by the same table, and `ok` reports whether any finding is at `deny` or `forbid`. Under `--fix` the envelope is the same one, with no separate array of applied fixes, and `applied` is a field of each record: an original finding whose fix was applied carries `applied: true` at its original span, and every finding of the final evaluation carries `applied: false` at its final span, including a finding the rewrite created, a finding whose span moved, and a finding whose rule still fires at the construct its fix was attempted on, which is one record, not two, because a final record is identified by (`qualified_rule`, final span). A record whose entry is `suggest` or `none` carries `applied: false`, and so does a pending fix under `--fix --diff`, and so does a finding whose rewrite `fix = false` declines or whose guard does not hold: no `fix_policy` field is added, because the record's `fix` mode and `applied: false` together say that a safe rewrite exists and was not applied, and `incan architect --list-rules` says why for the policy case. `ok` under `--fix` is computed over the final findings. `fix` is what Ruff's `applicability` field carries: `format` and `fix` where Ruff says `safe`, `suggest` where Ruff says `display-only`. A `suggest` record carries the replacement text in `hints`, so a consumer that wants to show a fix it will not apply has it, as Ruff's JSON output always carries a fix whether or not `--fix` would apply it.

### Rule documentation

Every entry carries its documentation in the catalog, in one template, and `incan explain` renders it. The template is Ruff's per-rule page with the headings renamed where the catalog's vocabulary differs:

| Section | Ruff heading | Content | Diagnostic catalog field |
| --- | --- | --- | --- |
| header | (badges) | name, code, group and default level (or the `format` class), origin and clippy counterpart, fact tier, fix mode, and, for a lint entry, the preview flag | `code`, `title`, `phase` |
| What it does | What it does | one sentence naming the construct the entry reports | `summary` |
| Why it matters | Why is this bad? | the reason; renamed because a `restriction` or `pedantic` entry reports a policy, not a defect | `explanation` |
| Example / Use instead | Example / Use instead | a before-and-after pair in Incan | `examples` |
| Fix | Fix safety | for `format`, what `incan fmt` normalizes; for `fix`, what `incan architect --fix` rewrites; for `suggest`, the suggestion's shape and its risks; for `none`, "none" | `fixes` |
| Options | Options | each parameter with its type, default, and `[incan.lints]` spelling | parameters (a lint-only field) |
| Known problems | Known problems | optional: the counterexamples RFC 105's finding names as risks | `common_causes` |

Rules:

- Every entry ships positive fixtures, in which the finding fires, and negative fixtures, in which the counterexamples its risks name do not fire, in addition to the tested example pair below. This is RFC 105's authoring contract ("each rule should have positive and negative fixtures; negative fixtures are required for common counterexamples named in the rule's risk text") kept as a gate: an entry without both sets is not a catalog entry, whatever its documentation says.
- Every seed lint entry of this RFC enters the catalog in preview. Promotion out of preview requires a calibration run over the toolchain's own corpus (the stdlib, `examples/`, and the test fixtures) with every finding triaged as a true positive, a fixture gap, or a rule defect, and for a `correctness` entry, which is `deny` by default, zero untriaged findings. The triage is recorded with the promotion, so a default level is a measured claim. `format`-class entries have no preview and no calibration gate, because they have no level; their gate is the idempotency and parse-preservation property tests.
- Both halves of the example must parse, and a documentation test holds the pair to the contract the entry's declared evaluator makes, so a rule's documentation cannot drift from its implementation. What the test proves depends on the evaluator:
    - for a lint entry (evaluator `architect` or `check`), the "before" example produces the entry's finding and the "after" example does not, both through the engine;
    - for a `format`-class entry (evaluator `fmt`), formatting the "before" example produces the "after" example, and formatting the "after" example leaves it unchanged: canonicalization and idempotency, in one pair;
    - for a `fix` entry, the lint contract above holds and, in addition, `incan architect --fix` on the "before" example equals the formatted "after" example, so the documented rewrite is the rewrite the command makes.
- `incan explain` accepts the bare rule name as well as the code (`incan explain needless_bool`, `incan explain INCAN-L0042`); the name resolves through the catalog to the code. `--format json` prints the entry as a record with the template's fields.
- The generated catalog reference page is rendered from the same entries, so the terminal, the site, and an editor hover agree.

### Exit codes

| Command | 0 | 1 |
| --- | --- | --- |
| `incan fmt` | files formatted (or nothing to do) | a file could not be parsed or written |
| `incan fmt --check` | no file would change | at least one file would change under the `format` class, or an operational error |
| `incan architect` | no finding at `deny` or `forbid` | at least one finding at `deny` or `forbid`, or an operational error |
| `incan architect --fix` | no finding at `deny` or `forbid` in the final evaluation | at least one such finding in the final evaluation, a file could not be written, or an operational error |
| `incan architect --fix --diff` | no fix is pending and no finding is at `deny` or `forbid` | a fix is pending, a finding is at `deny` or `forbid`, or an operational error |

A manifest refusal is an operational error for every command that reads the manifest; `incan fmt` reads none of the lint tables and cannot fail on them.

Ruff's contract has a third value: `0` clean, `1` violations, `2` abnormal termination (an invalid configuration, a bad flag, an internal error), so a CI job can tell "the code has findings" from "the tool did not run". The distinction is adopted; the third exit code is not. The toolchain's convention is that success is `0` and everything else is `1` (`incan check`, `incan test`, and `incan fmt --check` today), and one pair of commands with a three-valued contract would be a second convention; a `2` is a toolchain-wide change that RFC 118 decides, not this RFC. Under `--format json` the two cases are told apart by the report: an operational error is a diagnostic in the `tooling` phase or RFC 117's manifest family, carries no `rule` field, and sets `ok` to `false` with no catalog record beside it. Ruff's `--exit-zero` and `--exit-non-zero-on-fix` are not adopted: a project that wants a finding not to fail CI sets its level to `warn`, `incan fmt --check` is the CI form of the formatter, and `incan architect --fix --diff` is the CI form of the fixer.

### Diagnostics the checker emits today

The checker emits one lint-shaped diagnostic today: unreachable code after `return`, stable code `INCAN-T0101`, a warning produced by flow analysis. It becomes a catalog entry with evaluator `check`, keeps its code, its text, and its position in output, and gains a level:

| Today | Catalog entry | Origin | Group | Default |
| --- | --- | --- | --- | --- |
| unreachable code after `return` (`INCAN-T0101`) | `unreachable_code` | `rustc` | `suspicious` | `warn` |

Nothing else the checker emits is a lint. An unused binding, an unused import, and a wildcard `_` arm are not reported today: the exhaustiveness check treats `_` as covering the remaining cases and says nothing. The three entries that cover them are new, not renamed. `unused_variables` and `unused_imports` are by-products of name resolution, which the checker already performs, so their evaluator is `check` and they join `unreachable_code` as the checker's catalog entries. `wildcard_enum_match_arm` needs the scrutinee's enum type and clippy's restriction-group judgment, so its evaluator is `architect`:

| Entry | Origin | Group | Default | Facts | Evaluator |
| --- | --- | --- | --- | --- | --- |
| `unused_variables` | `rustc` | `style` | `warn` | semantic | `check` |
| `unused_imports` | `rustc` | `style` | `warn` | semantic | `check` |
| `wildcard_enum_match_arm` | `clippy` | `restriction` | `allow` (clippy's) | semantic | `architect` |

The last row follows clippy, and the design decisions record why: the checker enforces exhaustiveness, so a wildcard arm an author writes is deliberate, and clippy is the calibration reference. `incan check` must read `[incan.lints]` for the level of its three `check`-evaluated entries and no others; it must not evaluate any `architect` or `fmt` entry. The preview gate applies to the checker's entries as to every other lint entry, with one exception. `unreachable_code` is promoted from a diagnostic that already ships as a warning, so it enters the catalog promoted: its calibration is the releases it has shipped in, and holding it in preview would silence a warning projects see today. `unused_variables` and `unused_imports` are new and enter in preview: the checker records them in the codegraph regardless, `incan check` reports neither until promotion, because a preview entry is never eligible under the default invocation, and `incan architect --profile experimental` reports them when a level table names them.

The checker's four other warnings are contract diagnostics and stay outside the catalog: an async call that is not awaited, a `rust.module()` directive with no `@rust.extern` item, a public function that directly calls a checked C symbol (RFC 116), and dot-notation in a `rust` import (RFC 005). Each reports a contract its own RFC owns, at a severity that RFC fixed, and none is a style choice a project would set a level for; in the taxonomy of the core model they are the compiler's, not the catalog's. A later revision of this catalog may take one in once a project has a reason to configure it.

### Relationship to RFC 105

RFC 105 owns the Architect engine and its analysis contract. This RFC extends that engine with a catalog and policy surface; it does not replace RFC 105's bilingual project model.

| RFC 105 owns | RFC 127 owns |
| --- | --- |
| Oven-selected Incan, Rust, and mixed-Loaf scope | catalog entries and Clippy correspondence |
| frontend and codegraph semantic authority | groups, levels, preview, and manifest precedence |
| language, source-role, facet, generation, and project provenance | `@allow(...)` and per-path suppression |
| typed fact views, capabilities, rule execution, and de-duplication | fix applicability, fix policy, and the `--fix` lifecycle |
| categories, profiles, priority, confidence, evidence, risks, and the base finding envelope | `INCAN-L` codes, qualified catalog names, and `--list-rules` |
| rule authoring, fixtures, calibration expectations, and baselines | the `format` class and the formatter/fixer boundary |

RFC 127 extends RFC 105's finding envelope with `rule`, stable diagnostic `code`, `group`, `level`, `level_source`, `fix`, and `applied`. It also makes findings advisory by default but enforceable at `deny` or `forbid`, resolves local suppression syntax, and defines safe rewrites as the separately requested `incan architect --fix` mode with independent `fix = false` policy. Those are extension points over the same selected graph, provenance, evidence, and typed facts; they do not authorize a second project resolver or language-private rule engine. RFC 105's default profile and baseline storage remain open there.

## Design details

### Correspondence catalog

The tables below are the seed catalog. Each row is one clippy lint and fills exactly one of the three Incan columns. Parenthesized markers give the fact tier (`syntax` or `semantic`) and the fix mode where it is not `suggest`: `format` marks a `format`-class entry, which is always renamed `fmt_*` and has no group or level whatever table its clippy row sits in; `fix` marks a structural rewrite `incan architect --fix` applies; every other translated entry has fix mode `suggest` unless its row says `none`. `syntax, builtin-guarded` marks a `syntax`-tier entry that recognizes a builtin by name under the builtin-name guard defined under "Declared facts"; a row re-tiered `semantic` because it recognizes a method by name on a receiver the tree does not fix says which method made it so. Group membership and default levels follow clippy's, except where a row states the Incan group and the reason (`str_in_fstring`, `nested_fstring`); the `restriction` and `pedantic` rows are `allow` by default. Lints in clippy's `nursery`, `cargo`, and `deprecated` groups are out of scope. The clippy names and groups in this draft were reconciled against the clippy `master` lint list published on 2026-09-20; the catalog itself records, as its reconciliation baseline, the clippy release that ships with the Rust release the toolchain pins.

#### Correctness (default `deny`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `absurd_extreme_comparisons` | a comparison against a bound the operand's numeric type cannot cross, such as `n < 0` for a `u8` (semantic) | | |
| `almost_swapped` | `a = b` immediately followed by `b = a` (syntax) | | |
| `approx_constant` | a float literal that approximates a `std.math` constant, such as `3.14159` for `math.PI` (syntax) | | |
| `bad_bit_mask` | `x & mask == value` that can never hold for the literal mask and value, with resolved primitive integer operands (semantic) | | |
| `char_indices_as_byte_indices` | | | `len` and indexing on `str` count Unicode scalars; there is no byte-index API to confuse them with |
| `derive_ord_xor_partial_ord` | | | `@derive(Ord)` implies `PartialOrd`, and `@derive(Ord)` beside a hand-written `__lt__` is a checker error, not a lint |
| `derived_hash_with_manual_eq` | `@derive(Hash)` beside a hand-written `__eq__` (semantic) | | |
| `eq_op` | the same side-effect-free place or literal on both sides of `==`, `!=`, `<`, `-`, `//`, `and`, or `or`, with resolved primitive operands and floating-point exclusions where NaN changes the result (semantic) | | |
| `erasing_op` | `x * 0`, `0 * x`, `0 // x`, `x & 0` with resolved primitive numeric operands (semantic) | | |
| `ifs_same_cond` | an `if`/`elif` chain that repeats a condition (syntax) | | |
| `impossible_comparisons` | `x < 5 and x > 10` and other constant double comparisons that cannot hold for the resolved primitive operand type (semantic) | | |
| `ineffective_bit_mask` | a constant mask applied with `^` or bitwise or, followed by a comparison the mask cannot change for resolved primitive integer operands, such as `x ^ 1 < 4` (semantic) | | |
| `inherent_to_string_shadow_display` | a `def to_string(self) -> str` method; every `model`, `class`, `enum`, and `newtype` carries Display, so the method shadows `__str__` (syntax) | | |
| `invalid_regex` | a `std.regex` pattern literal that does not compile; the module is recognized through the import that binds its name (syntax, builtin-guarded) | | |
| `invisible_characters` | zero-width and other invisible Unicode in source text (syntax) | | |
| `iter_skip_zero` | `.skip(0)` on an iterator; `skip` is a method name on a receiver the tree does not fix, so the receiver's being an iterator is a type fact (semantic) | | |
| `iterator_step_by_zero` | | `range_step_zero`: `range(a, b, 0)` raises `ValueError` at run time; the construct is the `range` builtin, not an adapter (syntax, builtin-guarded) | |
| `lint_groups_priority` | | | not a lint: a group and one of its rules at the same priority is a manifest refusal |
| `match_str_case_mismatch` | `match s.lower():` or `match s.upper():` with a string arm the case-folded value can never equal; the reasoning holds only for the string methods, and `lower` and `upper` are method names on a receiver the tree does not fix (semantic) | | |
| `min_max` | `min(max(x, hi), lo)` with the bounds reversed so the result is constant for resolved primitive comparable operands (semantic) | | |
| `modulo_one` | `x % 1` with a resolved primitive integer operand (semantic) | | |
| `never_loop` | a `for`, `while`, or `loop` whose body always leaves on the first iteration (syntax) | | |
| `not_unsafe_ptr_arg_deref` | | | no raw pointers and no `unsafe` in Incan source |
| `out_of_bounds_indexing` | a constant index into a list literal of known length (syntax) | | |
| `panicking_unwrap` | `.unwrap()` on a value the enclosing branch has already narrowed to `None` or `Err` (semantic) | | |
| `possible_missing_comma` | a bracketed literal in which a line starts with a binary operator, so two elements silently merge (syntax) | | |
| `recursive_format_impl` | `f"{self}"` or `str(self)` inside `__str__`, which calls `__str__` again; the `str(self)` half is under the guard, the `f"{self}"` half needs none (syntax, builtin-guarded) | | |
| `redundant_comparisons` | `x > 5 and x > 3` where resolved primitive operand types make the second comparison redundant (semantic) | | |
| `reversed_empty_ranges` | `range(10, 0)`, `range(10..0)`, or a slice with constant reversed bounds (syntax, builtin-guarded) | | |
| `self_assignment` | `x = x`, `self.a = self.a` (syntax) | | |
| `serde_api_misuse` | | | derives are compiler-owned; there is no serde surface to misuse |
| `unit_cmp` | | | there is no unit value in expression position; `x == None` is the `comparison_to_none` entry |
| `uninit_assumed_init` | | | no uninitialized memory in Incan |
| `while_immutable_condition` | a `while` whose condition reads only bindings the body never assigns (semantic) | | |
| `wrong_transmute` | | | no `transmute` in Incan |

#### Suspicious (default `warn`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `await_holding_lock` | | | `Mutex` and `RwLock` are the async runtime's locks; holding one across `await` is their intended use |
| `blanket_clippy_restriction_lints` | | | not a lint: enabling the `restriction` group as a whole is a manifest refusal |
| `duplicated_attributes` | | `duplicated_decorators`: the same decorator applied twice to one declaration, such as two `@derive(Clone)` lines; the construct is a decorator (syntax) | |
| `empty_docs` | | `empty_docstring`: a docstring with no text (syntax) | |
| `empty_line_after_outer_attr` | | `fmt_decorator_spacing`: a blank line between a decorator and the declaration it decorates is removed; representation, so a `format`-class entry (syntax; format) | |
| `empty_loop` | `loop:` or `while true:` whose body is only `pass` (syntax) | | |
| `manual_unwrap_or_default` | | | `Option` and `Result` have no `unwrap_or_default` on the Incan surface; `unwrap_or` with an explicit default is the spelling |
| `misrefactored_assign_op` | `a += a + b` and `a -= a - b` (syntax) | | |
| `mut_range_bound` | `for i in range(n):` whose body assigns `n`, which cannot change the iteration (semantic) | | |
| `mutable_key_type` | | | no interior mutability; a `dict` key is a value |
| `no_effect_replace` | `s.replace("a", "a")`; `replace` is a method name on a receiver the tree does not fix, and the no-effect reasoning is the string method's (semantic) | | |
| `possible_missing_else` | | | indentation is the block structure; a dangling block cannot occur |
| `print_in_format_impl` | `print` inside a `__str__` implementation; `print` is protected from rebinding, so no guard (syntax) | | |
| `suspicious_assignment_formatting` | `x =- 1` where `x -= 1` was likely meant; `=!` and `=*` have no Incan counterpart (syntax) | | |
| `suspicious_else_formatting` | | | `else` is positioned by indentation; there is no brace layout to get wrong |
| `suspicious_unary_op_formatting` | `a -b`, a space before a unary operator and none after it in a binary position; fix mode `none`, because the `format`-class `fmt_horizontal_spacing` normalizes the spacing and the construct does not survive formatting, so the entry owns no rewrite of its own (syntax) | | |
| `test_attr_in_doctest` | | | docstrings are not executed as tests |
| `unconditional_recursion` | a method that calls itself on the same receiver on every path, such as `__eq__` written as `return self == other` (semantic) | | |

#### Style (default `warn`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `assertions_on_constants` | `assert true` and `assert false` (syntax) | | |
| `assign_op_pattern` | `x = x + 1` where `x += 1` is equivalent; RFC 105's compound-assignment candidate is this entry, and resolved operator facts must establish that the in-place and ordinary operations agree (semantic) | | |
| `blocks_in_conditions` | | | conditions are expressions; there is no block expression to put in one |
| `bool_assert_comparison` | `assert x == true` where `assert x` is meant and `x` resolves to primitive `bool` (semantic) | | |
| `collapsible_if` | a nested `if` with no `else` whose conditions can join with `and` (syntax) | | |
| `comparison_to_empty` | `s == ""` where `s.is_empty()` is meant; `str` and the frozen collections only, since `list` and `dict` have no `is_empty`, `not xs` on a list is a type error, and `xs == []` has no shorter spelling (semantic) | | |
| `disallowed_names` | binding names from a configured list, `foo`, `bar`, and `baz` by default (syntax; parameter `names`) | | |
| `disallowed_methods` | calls to functions or methods from a configured list of paths (semantic; parameter `paths`) | | |
| `duplicate_underscore_argument` | parameters `x` and `_x` on one signature (syntax) | | |
| `enum_variant_names` | variants that repeat the enum's name as prefix or suffix, such as a `ColorRed` variant declared inside `enum Color:` (syntax) | | |
| `excessive_precision` | a float literal with more digits than its type can represent (syntax) | | |
| `if_same_then_else` | `if` and `else` bodies that are identical (syntax) | | |
| `inconsistent_digit_grouping` | | `fmt_digit_grouping`: `1_00_000` is regrouped as `100_000`; shared with `unreadable_literal`, since one normalization decides both (syntax; format) | |
| `inherent_to_string` | | | every type carries Display, so a `to_string` method always shadows it: `inherent_to_string_shadow_display` is the rule |
| `len_zero` | `len(s) == 0` where `s.is_empty()` is meant; `str` and the frozen collections only, since `list` and `dict` have no `is_empty` and `len(items) > 0` is their documented idiom (semantic) | | |
| `let_and_return` | a binding returned by the very next statement (syntax) | | |
| `let_unit_value` | | `bind_none_value`: binding the `None` result of a `-> None` call, `x = print(...)` (semantic) | |
| `main_recursion` | `main` calling `main` (syntax) | | |
| `manual_map` | | `manual_result_map`: `match r:` with `Ok(v) => Ok(f(v))` and `Err(e) => Err(e)` where `r.map(f)` is meant; clippy's lint is `Option`-shaped and Incan's `Option` has `copied`, `unwrap_or`, and `unwrap` but no `map`, so the name says `Result`. RFC 105's `idiom.result_combinator_candidate` yields the `map` shape to this entry and keeps the `map_err`, `and_then`, `or_else`, and `inspect` shapes; resolved variant and receiver facts establish `Result` (semantic) | |
| `manual_ok_or` | | | `Option` has no `ok_or` on the Incan surface |
| `match_like_matches_macro` | | | no `matches!`; a unit variant compares with `==`, and a payload variant has no boolean pattern form |
| `match_ref_pats` | | | patterns never mention references; the compiler decides how generated Rust binds them |
| `needless_borrow` | | | no borrow syntax in Incan source |
| `needless_else` | `else: pass` (syntax; fix) | | |
| `needless_range_loop` | `for i in range(len(xs)):` whose body only reads `xs[i]`, where `for x in xs:` or `enumerate(xs)` is meant; `range` and `len` are under the guard (syntax, builtin-guarded) | | |
| `needless_return` | | | `return` is the only way to yield a value; there are no tail expressions. A trailing bare `return` in a `-> None` body is the Incan-only `needless_bare_return` |
| `neg_multiply` | `x * -1` where `-x` is equivalent for the resolved primitive numeric type (semantic) | | |
| `partialeq_to_none` | | `comparison_to_none`: `x == None` and `x != None` where `x is None` and `x is not None` are meant; resolved equality facts exclude a user-defined `__eq__` contract (semantic) | |
| `print_literal` | | | `print` takes values, not a format string; `useless_fstring` covers the f-string side |
| `print_with_newline` | `print("...\n")`: `print` already ends the line, so the output gains a blank line (syntax) | | |
| `println_empty_string` | | | `print("")` is the documented spelling of an empty line; `print` has no documented zero-argument form to prefer |
| `ptr_arg` | | | parameter types are value types; there is no `&String` or `&Vec` to take |
| `question_mark` | a `match` on a resolved `Result` that returns the `Err` arm unchanged, where `?` is meant (semantic) | | |
| `redundant_closure` | an arrow closure `(x) => f(x)` where `f` itself is meant (syntax) | | |
| `redundant_field_names` | | | keyword construction `User(name=name)` has no shorthand to prefer |
| `redundant_pattern_matching` | `match x:` with resolved `Option` variants `None => true` and `_ => false`, where `x is None` is meant; `Option` only, since `Result` has no `is_ok` on the surface (semantic) | | |
| `redundant_static_lifetimes` | | | no lifetimes |
| `result_unit_err` | `-> Result[T, None]` (syntax) | | |
| `same_item_push` | `for _ in range(n): xs.append(v)` with a loop-invariant `v`, where `list.repeat(v, n)` is meant; `range` would be under the guard, but `append` is a method name on a receiver the tree does not fix and the suggestion is a `list` method (semantic) | | |
| `single_component_path_imports` | | | `import foo` is the ordinary module import; there is nothing to simplify |
| `single_match` | a one-arm `match` over a resolved `Option` with `_ => pass`, where `if x is not None:` narrowing is meant; a payload enum arm has no `if let` form and is not reported (semantic) | | |
| `tabs_in_doc_comments` | | `tabs_in_docstrings`: a tab inside a docstring (syntax) | |
| `trim_split_whitespace` | `s.strip().split_whitespace()` where `s.split_whitespace()` is meant (semantic) | | |
| `unused_unit` | | | `-> None` is the required spelling of a value-less return type; there is no `()` to drop |
| `unwrap_or_default` | | | no `unwrap_or_default`; `unwrap_or([])` is the spelling |
| `upper_case_acronyms` | `class HTTPClient` where `HttpClient` is meant; the style guide's `UpperCamelCase` rule does not yet say how an acronym is cased, so this entry needs that ruling before it ships (syntax) | | |
| `while_let_on_iterator` | | | no `while let`; `for` is the iteration form |
| `wrong_self_convention` | `is_`, `to_`, and `as_` prefixes checked against `self` versus `mut self`; `into_` has no consuming receiver and is not checked (syntax) | | |

#### Complexity (default `warn`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `bool_comparison` | `x == true` and `x != false` where `x` resolves to primitive `bool` and `x` and `not x` are equivalent (semantic) | | |
| `borrowed_box` | | | no `Box` and no borrow syntax |
| `bytes_count_to_len` | | | `len(s)` counts scalars and `len(s.encode())` counts bytes; they are different values, so neither replaces the other |
| `double_comparisons` | side-effect-free primitive operands in `x == y or x > y` where `x >= y` is equivalent, excluding floating-point cases where NaN changes the result (semantic) | | |
| `double_parens` | | `fmt_parentheses`: `((x))` and `f((x))` lose the redundant pair; shared with `precedence`, since parenthesization is representation and one normalization owns it (syntax; format) | |
| `excessive_nesting` | blocks nested past a threshold; clippy's default is off until a threshold is configured, and so is this entry's: `threshold` defaults to `0`, meaning never (syntax; parameter `threshold`) | | |
| `explicit_auto_deref` | | | no dereference operator |
| `explicit_counter_loop` | a counter initialized before a `for` and incremented once per iteration, where `enumerate` is meant; `enumerate` appears only in the suggestion, so no guard (syntax) | | |
| `identity_op` | `x + 0`, `x * 1`, `x // 1`, and a bitwise or with `0` for resolved primitive operands (semantic) | | |
| `int_plus_one` | `x >= y + 1` where `x > y` is meant for resolved primitive integers; the suggestion retains the overflow risk described by the catalog (semantic) | | |
| `iter_count` | `xs.iter().count()` where `len(xs)` is meant; `iter` and `count` are method names on a receiver the tree does not fix (semantic) | | |
| `manual_filter_map` | | | no `filter_map` on the iterator surface |
| `manual_strip` | | | no `strip_prefix` or `strip_suffix` on the string surface |
| `manual_swap` | `tmp = xs[i]` then `xs[i] = xs[j]` then `xs[j] = tmp`, where resolved receiver facts establish a collection with `swap(i, j)` (semantic) | | |
| `match_single_binding` | a `match` with one irrefutable arm (syntax) | | |
| `needless_bool` | `if c: return true` with `else: return false`, where `return c` is meant; the condition position guarantees `c` is a `bool` (syntax; fix) | | |
| `needless_bool_assign` | `if c: x = true` with `else: x = false`, where `x = c` is meant (syntax; fix) | | |
| `needless_ifs` | `if c: pass` with no `else` (syntax) | | |
| `needless_lifetimes` | | | no lifetimes |
| `needless_question_mark` | `return Ok(f()?)` where `return f()` is meant, when the error types agree (semantic) | | |
| `no_effect` | an expression statement proven to have no effect, such as primitive arithmetic with an unused result (semantic) | | |
| `precedence` | | `fmt_parentheses`: `1 << 2 + 3` and `a & b == c`, bit and arithmetic or comparison operators mixed without parentheses, gain them; shared with `double_parens` (syntax; format) | |
| `single_element_loop` | `for x in [item]:` (syntax) | | |
| `too_many_arguments` | more parameters than the threshold, 7 by default (syntax; parameter `threshold`) | | |
| `type_complexity` | a type expression nested past the threshold, such as `Dict[str, list[Result[Option[int], E]]]` (syntax; parameter `threshold`) | | |
| `unnecessary_cast` | a `resize()`, `try_resize()`, `wrapping_resize()`, or `saturating_resize()` whose target type is the operand's own type; the builtin conversions `int`, `float`, `str`, and `bool` belong to `useless_conversion` (semantic) | | |
| `unnecessary_literal_unwrap` | `Some(1).unwrap()` and `Ok(v).unwrap()` (syntax) | | |
| `unnecessary_unwrap` | `.unwrap()` on a binding the enclosing `is not None` narrowing already unwrapped (semantic) | | |
| `useless_conversion` | `int(x)` where `x` is already an `int`, `str(s)` on a `str`, `float(f)` on a `float` (semantic) | | |
| `useless_format` | | `useless_fstring`: `f"{x}"` where `x` is already a `str` (semantic). The placeholder-free form, `f"literal"`, is representation and is the `format`-class `fmt_fstring_prefix`: the `f` is dropped and doubled braces are unescaped, so the value is unchanged (syntax; format) | |
| `while_let_loop` | | | no `while let` |
| `zero_divided_by_zero` | `0.0 / 0.0` (syntax) | | |

#### Perf (default `warn`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `box_collection` | | | no `Box` |
| `cmp_owned` | | | the compiler plans the conversions generated Rust needs for a comparison; source never spells one |
| `collapsible_str_replace` | | | `replace` takes one pattern; there is no multi-pattern form to collapse into |
| `expect_fun_call` | | | no `expect` on the surface |
| `format_in_format_args` | | `nested_fstring`: an f-string placeholder whose expression is itself an f-string with at least one placeholder is spliced into the outer string (a placeholder-free inner literal is `fmt_fstring_prefix`'s). The lexer scans a placeholder by brace depth, so the form is one the language accepts; the splice removes a node and re-nests the inner parts, so it is a `fix`, not representation. The rewrite is admissible only where re-lexing the spliced text yields exactly the same sequence of literal and placeholder parts as the two-level form: each placeholder's expression text and format spec unchanged, literal braces `{{` and `}}` preserved, and escapes decoding to the same text. That lex-and-compare is the fix's guard: where it holds the rewrite is applied, and where it fails the finding is reported without a fix. In `style` rather than clippy's `perf`, because the two spellings render one text and the nesting is spelling, not cost (syntax; fix) | |
| `iter_overeager_cloned` | | | the iterator surface has no `cloned` adapter; the compiler decides where generated Rust clones |
| `large_enum_variant` | | | the source language has no layout model; variant sizes are a backend fact |
| `manual_retain` | | | a filtering comprehension is the idiom; `retain` is not on the list surface |
| `map_entry` | | | no entry API; `contains_key` followed by `insert` is the spelling |
| `regex_creation_in_loops` | a `std.regex` pattern compiled from a literal inside a loop body; the module is recognized through the import that binds its name (syntax, builtin-guarded) | | |
| `to_string_in_format_args` | | `str_in_fstring`: `f"{str(x)}"` where `f"{x}"` is meant; both go through the Display protocol, so the value is the same. Whether `str` at the call site is the builtin is decided by the builtin-name guard: the entry is silent where an enclosing scope binds `str`, and where none does the rewrite drops the call and keeps the placeholder, which is decidable on the tree. Rebinding stays allowed by design; the guard is what makes the rewrite a `fix` without reserving the name. A placeholder that carries a format spec is reported without a fix. In `style` rather than clippy's `perf`, because the two spellings go through one protocol and the redundant call is spelling, not cost (syntax, builtin-guarded; fix) | |
| `unnecessary_to_owned` | | | no `to_owned` on the surface; the compiler decides how generated Rust owns a value |
| `useless_vec` | | | `list` is the only sequence type; there is no array or slice to prefer |
| `vec_init_then_push` | | `list_init_then_append`: `xs = []` followed only by `xs.append(...)` statements, where a list literal is meant; the annotation, if any, is kept, and a run in which an appended expression mentions `xs` is reported without a fix. `append` is a method name, but the receiver's type is fixed by construction: `xs = []` binds a `list`, or the checker rejects the file, so the entry stays `syntax` and keeps its fix (syntax; fix) | |

#### Pedantic (default `allow`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `bool_to_int_with_if` | `if c: x = 1` with `else: x = 0`, or the same pair of `return` statements, where `int(c)` is meant; `int` appears only in the suggested spelling, so no guard applies, and the risk text names a rebound `int` (syntax) | | |
| `cast_possible_truncation` | | | `resize()` is accepted only for lossless conversions and `wrapping_resize()` truncates by declaration; `as_conversions` is the rule for the lossy forms |
| `doc_markdown` | | | docstring rendering conventions are RFC 082's; until it fixes the format there is nothing to check against |
| `explicit_iter_loop` | `for x in xs.iter():` where `for x in xs:` is meant; `iter` is a method name on a receiver the tree does not fix, and the rewrite needs `xs` itself to be iterable (semantic) | | |
| `float_cmp` | `==` or `!=` between floats (semantic) | | |
| `fn_params_excessive_bools` | more `bool` parameters than the threshold (syntax; parameter `max`) | | |
| `if_not_else` | `if not c:` with an `else`, where swapping the branches is meant; the branch bodies' comments move with them, so this entry waits for #159's trivia-aware tree (syntax; fix) | | |
| `inconsistent_struct_constructor` | | `inconsistent_model_constructor`: keyword arguments in a different order from the field declaration; evaluation order changes, so the fix is a suggestion (semantic) | |
| `manual_assert` | | | no panic-family call to fold into `assert` |
| `manual_let_else` | | | no `let ... else`; `match` and `?` are the forms |
| `many_single_char_names` | more single-character bindings in one scope than the threshold (syntax; parameter `threshold`) | | |
| `map_unwrap_or` | | | no `map_or`; `r.map(f).unwrap_or(d)` is the spelling |
| `match_bool` | `match flag:` with `true =>` and `false =>` arms, where `if` is meant (syntax) | | |
| `match_same_arms` | arms with identical bodies (syntax) | | |
| `match_wildcard_for_single_variants` | a `_` arm that covers exactly one remaining variant (semantic) | | |
| `missing_errors_doc` | | `missing_errors_docstring`: a `pub def` returning `Result` whose docstring has no errors section; the recognized docstring sections are `Args`, `Parameters`, `Returns`, `Fields`, `Aliases`, and `Decorators`, so this entry needs an errors-section convention from RFC 082 before it ships (syntax) | |
| `needless_continue` | `continue` as the last statement of a loop body, or an `if ... continue` that an `else` avoids (syntax) | | |
| `needless_for_each` | `.for_each(f)` where a `for` loop reads better; `for_each` is a method name on a receiver the tree does not fix (semantic) | | |
| `needless_pass_by_value` | | | parameters are values in source, and the compiler decides the Rust shape |
| `option_option` | `Option[Option[T]]` in a signature (syntax) | | |
| `range_minus_one` | `range(a..=(b - 1))` where `range(a..b)` is meant; the two differ where `b - 1` underflows, which fails the original at run time, and that edge is an observable change, so the rewrite does not meet the `fix` contract and is a suggestion (syntax, builtin-guarded) | | |
| `range_plus_one` | `range(a..(b + 1))` where `range(a..=b)` is meant; the two differ where `b + 1` overflows, which fails the original at run time, and that edge is an observable change, so the rewrite does not meet the `fix` contract and is a suggestion (syntax, builtin-guarded) | | |
| `redundant_else` | an `else` after a branch that always returns, breaks, or continues; the `else` body is un-nested, and its comments move with it, which is why this entry waits for #159's trivia-aware tree (syntax; fix) | | |
| `semicolon_if_nothing_returned` | | | no semicolons |
| `similar_names` | bindings in one scope that differ by one character (syntax) | | |
| `struct_excessive_bools` | | `model_excessive_bools`: more `bool` fields on a `model` or `class` than the threshold (syntax; parameter `max`) | |
| `struct_field_names` | | `model_field_names`: fields that repeat the type's name as prefix or suffix (syntax) | |
| `too_many_lines` | a function body longer than the threshold (syntax; parameter `threshold`) | | |
| `trivially_copy_pass_by_ref` | | | no by-reference parameter spelling |
| `unicode_not_nfc` | a string literal not in NFC (syntax) | | |
| `uninlined_format_args` | | | f-strings inline their arguments by construction |
| `unnecessary_wraps` | a non-`pub` function that only ever returns `Ok(...)` (semantic) | | |
| `unreadable_literal` | | `fmt_digit_grouping`: `1000000` is written `1_000_000`; decimal literals of six or more digits are grouped by three from the right, and hexadecimal and binary literals by four; shared with `inconsistent_digit_grouping` (syntax; format) | |
| `unused_async` | `async def` with no `await` (syntax) | | |
| `unused_self` | a method that never reads `self`, where `@staticmethod` or a free function is meant (semantic) | | |
| `used_underscore_binding` | reading a binding named with a leading underscore (semantic) | | |
| `wildcard_imports` | | | the parser refuses every `from module import *`; there is no wildcard import to report |

#### Restriction (default `allow`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `allow_attributes_without_reason` | | `allow_without_reason`: `@allow("...")` with no `reason` (syntax) | |
| `arbitrary_source_item_ordering` | declarations out of a configured order (syntax; parameter `order`) | | |
| `as_conversions` | `wrapping_resize()` and `saturating_resize()`, the lossy integer resizes; the two are method names on a receiver the tree does not fix, and the entry reports the integer methods, not a user method of the same name (semantic) | | |
| `dbg_macro` | | | no `dbg`; `print_stdout` covers debug output |
| `else_if_without_else` | an `if`/`elif` chain with no final `else` (syntax) | | |
| `expect_used` | | | no `expect`; `unwrap_used` is the rule |
| `float_arithmetic` | arithmetic on floats (semantic) | | |
| `implicit_return` | | | returns are explicit by construction |
| `indexing_slicing` | `xs[i]`, `s[i]`, and slices, which raise `IndexError` (syntax) | | |
| `infinite_loop` | `loop:` with no `break` or `return` on any path (syntax) | | |
| `integer_division` | `//` on integers (syntax) | | |
| `min_ident_chars` | identifiers shorter than the threshold (syntax; parameter `threshold`) | | |
| `missing_assert_message` | `assert c` without a message (syntax) | | |
| `missing_docs_in_private_items` | | `missing_docstrings`: a declaration without a docstring; `pub` declarations follow RFC 082's checked-documentation conventions (syntax; parameter `scope`) | |
| `modulo_arithmetic` | `%` where an operand may be negative; Incan's `%` takes the sign of the divisor, which differs from Rust's (semantic) | | |
| `non_ascii_literal` | non-ASCII characters in string literals (syntax) | | |
| `panic` | | | no `panic` call on the surface; `assert` and `raise` are the failure forms |
| `pattern_type_mismatch` | | | patterns never mention references |
| `print_stdout` | any `print` call (syntax) | | |
| `question_mark_used` | any use of `?` (syntax) | | |
| `redundant_type_annotations` | `x: int = 1` where the literal already fixes the type (semantic) | | |
| `renamed_function_params` | a trait method implementation whose parameter names differ from the trait's, which changes keyword call sites (semantic) | | |
| `single_call_fn` | a function called from exactly one place (semantic); RFC 105's `maintainability.single_use_trivial_helper` is the evidence-backed neighbor and stays a separate entry | | |
| `string_slice` | | | string indexing and slicing count scalars and cannot split a code point |
| `tests_outside_test_module` | a `test_*` function declared outside a `module tests:` block and outside a test file (syntax) | | |
| `todo` | | `ellipsis_body`: a function or method whose body is only `...`, which the grammar reads as `pass` (syntax) | |
| `undocumented_unsafe_blocks` | | | no `unsafe` |
| `unimplemented` | | `ellipsis_body`, shared with `todo`: the two clippy lints report one Incan construct | |
| `unreachable` | | | no `unreachable` call on the surface; a `match` the checker proves exhaustive needs none |
| `unwrap_used` | `.unwrap()` on an `Option` or `Result`; on the surface `unwrap` is a method of those two types only, and this is a restriction on the fail-fast spelling, so the receiver's type is not consulted and a user method of that name is reported as well (syntax) | | |
| `wildcard_enum_match_arm` | a `_` arm in a `match` over an enum (semantic) | | |

### Entries without a clippy counterpart

Rules clippy has no notion of: rustc lints that translate, rules for Python-shaped constructs, and rules for the surface Incan has where Rust has ownership syntax. The `format`-class entries without a clippy counterpart (`fmt_import_order` and the layout entries) are listed under "Formatter entries" instead, because they have no group:

| Rule | Group | Origin | Facts | Fix | Reports |
| --- | --- | --- | --- | --- | --- |
| `fstring_over_concat` | `style` | `incan` | semantic | suggest | a `+` chain whose operands are strings and `str(...)` calls, where one f-string is meant |
| `comprehension_over_loop` | `style` | `incan` | syntax | suggest | `out = []` followed by a `for` whose body only appends to `out`, where a list comprehension is meant; this entry owns RFC 105's append-only builder shape |
| `unnecessary_comprehension` | `style` | `incan` | syntax | suggest | `[x for x in xs]`, where the source list itself, or `xs.clone()` when a copy is meant, is the spelling |
| `generator_over_list` | `perf` | `incan` | syntax, builtin-guarded | suggest | a list comprehension consumed once by `sum`, `min`, `max`, a `for`, or the `any` and `all` iterator methods, where a generator expression is meant; the three builtins are under the guard, and `any` and `all` are called on the comprehension itself, whose type is fixed by construction |
| `while_true` | `style` | `rustc` | syntax | fix | `while true:` where `loop:` is meant |
| `redundant_pass` | `style` | `incan` | syntax | fix | a `pass` in a block that has other statements; the fix removes it. Removing a statement node is structure, not representation, which is why this is a `fix` and not a `format` entry |
| `needless_bare_return` | `style` | `incan` | syntax | fix | a bare `return` as the last statement of a `-> None` function body; the fix removes it, and a body that was only `return` becomes `pass`. Removing a statement node is structure, not representation |
| `merge_imports` | `style` | `incan` | syntax | fix | two `from` statements for one module in one import run; the fix merges them into one statement that binds exactly the names the two bound, in `fmt_import_order`'s name order. A run in which a name is bound twice is the checker's (`ambiguous_import_binding`, `INCAN-I0001`) and is neither reported nor merged. Merging removes a statement node, which is why this is a `fix` and not part of `fmt_import_order` |
| `unused_mut` | `style` | `rustc` | semantic | suggest | a `mut` binding, or a `mut self` receiver, that is never mutated |
| `unused_variables` | `style` | `rustc` | semantic | suggest | an unused local binding; new, evaluator `check` |
| `unused_imports` | `style` | `rustc` | semantic | suggest | an unused import; new, evaluator `check` |
| `unreachable_code` | `suspicious` | `rustc` | semantic | suggest | statements after a `return` in the same block; evaluator `check`; keeps `INCAN-T0101` |
| `unused_allow` | `style` | `incan` | semantic | suggest | an `@allow(...)` naming a rule that would not have fired in the decorated scope; Ruff's `unused-noqa` (`RUF100`) for the decorator form; evaluated after every other rule, in probe mode |

### Import ordering (`fmt_import_order`)

Import ordering is a `format`-class normalization, not a lint, because reordering imports cannot change what an Incan program means. An Incan module is declarations only: the parser refuses a statement at module level, a `static` initializer is a storage cell rather than code that runs at import ([Static storage](../language/reference/static_storage.md)), and an import is a compile-time name binding that the checker resolves under the reserved roots (`std`, `pub::`, `rust::`, and the project) as RFC 022 ([stdlib namespacing](closed/implemented/022_stdlib_namespacing_and_compiler_handoff.md)) and RFC 031 ([`pub` imports](closed/implemented/031_library_system_phase1.md)) define and the [imports and modules reference](../language/reference/imports_and_modules.md) documents. There is no import-time execution, so sectioning and sorting imports cannot change program meaning; the only observable is resolution, and the one case where reordering could touch resolution, a run in which one local name is bound twice, is already a checker error in the configuration compiled (`ambiguous_import_binding`, `INCAN-I0001`) and is excluded from the normalization. `fmt_import_order` leaves such a run as written and emits no formatting finding, because it would make no change; the checker independently reports the binding error. The entry sections, sorts, and spaces the sections, and nothing else: merging two `from` statements for one module removes a statement node, so it is the `fix` entry `merge_imports` under "Entries without a clippy counterpart", and de-duplication does not exist at all.

Ruff's isort rule (`I001`) de-duplicates, merges, groups, and sorts imports by isort's rules; this entry takes the grouping and the sorting, `merge_imports` takes the merging, and the de-duplication is dropped. isort's rules are mostly configuration: which modules are `standard-library`, `third-party`, `first-party`, or `local-folder` is decided by `known-first-party`, `known-third-party`, and `known-local-folder`, because a Python import path does not say where it comes from. An Incan import path does. `std` and `rust` are reserved root namespaces, `pub::` is the published-library root, and everything else is the project, so the section of every import is decided by its first path segment: no `known-*` configuration exists, and none could, because Incan's namespacing already answers the question. The section order below is this RFC's ruling; the style guide records it, and does not decide it:

| Section | Root | Forms |
| --- | --- | --- |
| 1. standard library | `std` | `import std.async`, `from std.io import File`, `from std import toml` |
| 2. published libraries | `pub::` | `from pub::hees_ai import hyperquant`, `import pub::hees_ai.hyperquant as hq` |
| 3. Rust crates | `rust::` | `from rust::polars import (...)`, `import rust::serde_json` |
| 4. project | `crate.`, `crate::`, a sibling or child module | `from crate.config import Settings`, `from db.models import User`, `import models::User` |
| 5. parent-relative | `..`, `super::` | `from ..common import Logger`, `import super::utils::format_date` |

Rules:

- Sections are separated by exactly one blank line and contain none. RFC 053's bucket C already permits zero or one blank line inside an import run and leaves the choice open; this entry narrows it for import runs, which RFC 053 allows a later RFC to do.
- Within a section, statements sort by module path, compared segment by segment, case-insensitively, with `import module` and `from module import ...` for the same module adjacent and the `import` form first (isort's `from-first = false`). The two spellings are never rewritten into each other: which spelling a project prefers is the style guide's, not this entry's.
- Within one `from` statement, names sort case-insensitively; a name with an alias sorts by its original name. isort's `order-by-type` (constants, then classes, then functions) is not adopted: one sort key is enough, and the casing conventions that would drive it are the style guide's business.
- Two `from` statements for one module are not merged by this entry: merging removes a statement node, so it is `merge_imports`, a `fix` that `incan architect --fix` applies under the levels, and the merged statement binds exactly the names the two bound. A name with an alias is distinct from the same name without one. Nothing de-duplicates a name listed twice: a name bound twice in one run is the checker's `ambiguous_import_binding`, not a formatting matter.
- A run in which one local name is bound twice is left as written: nothing in it is sorted, `merge_imports` does not report it, and the formatter emits no finding because it would make no change. The binding itself is the checker's error (`ambiguous_import_binding`, `INCAN-I0001`), not a lint or formatting condition. Reordering is meaning-preserving because it cannot change resolution, and that run is the one case where it might.
- `merge_imports` and this entry never want the same construct: sorting keeps both statements where they are relative to the rest of the run, and merging keeps the order. `incan architect --fix` formats its output, so a merged run is sorted before it is written.
- `pub from module import Item` re-exports form their own run, sorted by the same key, and are never merged into the plain import run. `import this` stays where it is.
- Comments attached to an import move with it, which is why this entry waits for #159's trivia-aware tree.

Not adopted from isort, with the reason: `force-single-line` and `combine-as-imports` (line shape is `fmt_line_length` and `fmt_trailing_comma`'s job); `force-sort-within-sections`, `force-to-top`, `no-lines-before`, `sections`, and `section-order` (configurability that makes two projects' import blocks disagree for no gain); `lines-after-imports` and `lines-between-types` (RFC 053 owns vertical spacing); `required-imports` (a Python `from __future__` need); `relative-imports-order` (one order, above); `detect-same-package` (the root segment answers it); and the `# isort: skip`, `# isort: off`, and `# isort: split` action comments (no formatter opt-out). The isort profile Ruff targets, `profile = "black"`, is the one that agrees with a canonical formatter, and that is the only profile this entry has.

### Formatter entries (the `format` class)

Every normalization the formatter performs is a catalog entry with fact tier `syntax`, evaluator `fmt`, and fix mode `format`. Their names give `--check` output and #159's regression tests an identity per rule:

| Rule | Source | Normalizes |
| --- | --- | --- |
| `fmt_indentation` | style guide | four spaces per level; tabs and inconsistent indentation |
| `fmt_blank_lines` | RFC 053 | the three buckets: exactly two, exactly one, at most one blank line at each transition; never more than two; at most one inside a docstring |
| `fmt_decorator_spacing` | this RFC (clippy `empty_line_after_outer_attr`) | no blank line between a decorator and the declaration it decorates |
| `fmt_trailing_newline` | RFC 053 | exactly one line terminator at end of file |
| `fmt_comment_placement` | RFC 053 | same-scope, structure-aware attachment of stand-alone comments |
| `fmt_docstring_layout` | style guide | single-line docstrings stay on one line; multi-line docstrings put the quotes on their own lines |
| `fmt_horizontal_spacing` | style guide | spaces around binary operators, after commas, and after annotation colons; none around `=` in keyword arguments; none before `(` |
| `fmt_parentheses` | this RFC (clippy `double_parens`, `precedence`) | redundant parentheses `((x))` and `f((x))` removed; bit, arithmetic, and comparison operators mixed in one expression parenthesized |
| `fmt_digit_grouping` | this RFC (clippy `unreadable_literal`, `inconsistent_digit_grouping`) | decimal literals of six or more digits grouped by three from the right, hexadecimal and binary literals by four; any other grouping regrouped |
| `fmt_quote_style` | style guide | double quotes where the decoded value is unchanged |
| `fmt_fstring_prefix` | this RFC (clippy `useless_format`, placeholder-free form) | the `f` prefix dropped from a string literal with no placeholder, doubled braces unescaped |
| `fmt_import_order` | this RFC | imports sectioned by root and sorted as "Import ordering" defines, with one blank line between sections and none inside; nothing merged and nothing de-duplicated; a run that binds one name twice is left as written and reported independently by the checker |
| `fmt_trailing_comma` | style guide | trailing commas in multi-line constructs |
| `fmt_line_length` | style guide | the 120-character target: overflowing calls, constructors, `with (...)` lists, logical chains, and fluent chains are re-wrapped; a line no rewrite can shorten is not a finding, and in particular a line that only a comment, a docstring, or a string literal makes long is not one, because the formatter never splits those |
| `fmt_match_arm_layout` | style guide | short single-statement arms inline; multi-statement arms as a block body |

Every row above is representation by the definition under "Declared facts": trivia, layout, the spelling of a literal, redundant parentheses, or the order of independent declarations, and no row adds, removes, merges, or re-nests a node. That is the test that moved three earlier entries out of this table: removing a redundant `pass` and a trailing bare `return` deletes a statement, and splicing a nested f-string removes a node and re-nests its parts, so they are the `fix` entries `redundant_pass`, `needless_bare_return`, and `nested_fstring`, and merging two `from` statements left `fmt_import_order` for the same reason and became `merge_imports`. Formatter invariants apply to the class as a whole: idempotency, parse preservation, and opacity of string literal bodies (RFC 053's string-and-comment safety rule). A file that does not parse is not formatted. The same invariants bind every `fix`-mode entry `incan architect --fix` applies, which is why its output can be handed to the formatter without a second parse. The bar for a `format` entry is higher than for a `fix`, because the class runs on every file with no level to turn it off, and `fmt_quote_style` shows what meeting it means: the entry does not assume a re-quoting is an identity, it establishes per literal that the decoded value is unchanged by lexing both spellings, and it leaves any literal the check does not cover as written. The opacity rule is why that check is a lex-and-compare and not a reading of the string: the formatter never interprets a literal's value, so equality of the lexed parts is the only equality it may claim. `nested_fstring` carries the same kind of proof as its fix's guard, on the fixer's side of the line.

### Boundary with RFC 105

RFC 105 defines the engine and base finding contract; this RFC defines the catalog and policy that engine evaluates. The ownership table under "Relationship to RFC 105" is normative. Concretely:

- RFC 105's categories `arch`, `safety`, `idiom`, `maintainability`, and `risk` are groups of this catalog. Their default levels are RFC 105's to set, and this RFC does not decide them. RFC 105's `experimental` is a profile, not a category: a rule in that profile keeps its category's group here, is a preview entry of this catalog, is never part of `all`, is eligible only under `--profile experimental`, and RFC 105's rule that experimental findings may not fail CI by default holds because a preview entry defaults to `allow` and eligibility never raises a level.
- An RFC 105 candidate shape that coincides with a clippy lint takes the clippy name and group: `assign_op_pattern` owns the compound-assignment shape and `comprehension_over_loop` owns the append-only builder shape. `manual_result_map` owns the `map` form of RFC 105's `idiom.result_combinator_candidate`; that RFC 105 rule keeps the `map_err`, `and_then`, `or_else`, `inspect`, and `inspect_err` forms that no single clippy lint expresses. `question_mark` has no RFC 105 counterpart: `?` propagation is not a combinator. RFC 105's own names remain for evidence-backed, project-scope rules that a clippy lint does not express: `safety.fail_fast_boundary_call` is not `unwrap_used` (it reasons about reachability from a public boundary), and `maintainability.single_use_trivial_helper` is not `single_call_fn` (it reasons about triviality and domain meaning). Both members of each pair are catalog entries.
- Priority, confidence, evidence, suggestions, and risks are RFC 105's finding fields. Level, group, and fix mode are this RFC's. A finding at `deny` fails the command regardless of its priority; a `P1` finding at `warn` does not.
- `@allow(...)` answers RFC 105's open suppression question for catalog entries, and `incan architect --list-rules` and `incan architect --fix` are flags this RFC adds to RFC 105's command surface. Baselines remain RFC 105's, and the facts a rule consumes are declared in RFC 105's registry, not restated here.
- RFC 105's authoring contract is this RFC's quality gate: positive and negative fixtures per entry, and a calibration run over real source before a rule is on by default, which is what promotion out of preview requires here.

### Interaction with existing features

- **async/await**: `unused_async` translates. The lock-holding lints do not, because Incan's `Mutex` and `RwLock` are the async runtime's own locks.
- **Traits and derives**: the dunder protocol is what makes `derived_hash_with_manual_eq`, `inherent_to_string_shadow_display`, `recursive_format_impl`, `print_in_format_impl`, `unconditional_recursion`, and `renamed_function_params` translate: `__eq__`, `__str__`, `__hash__`, and `__lt__` are the manual implementations clippy's lints reason about, and every `model`, `class`, `enum`, and `newtype` carries Display.
- **Imports and modules**: `wildcard_imports` does not translate because the parser refuses the form, which is also what makes the builtin-name guard a tree walk; `unused_imports` is an Incan-side lint entry, a duplicate binding stays the checker's error (`ambiguous_import_binding`, `INCAN-I0001`), `fmt_import_order` is a `format`-class normalization that reads the section of an import off its root segment, as "Import ordering" defines, on the guarantee that imports are compile-time bindings with no import-time execution, and `merge_imports` is the `fix` that joins two `from` statements for one module. `pub from module import Item` re-exports are ordinary declarations to every rule, and a separate run to `fmt_import_order`.
- **Result, Option, and `?`**: `question_mark`, `manual_result_map`, `unwrap_used`, `panicking_unwrap`, `unnecessary_unwrap`, and `redundant_pattern_matching` translate on the methods the surface has (`map`, `map_err`, `and_then`, `or_else`, `inspect`, `inspect_err`, `unwrap`, and `unwrap_or` on `Result`; `copied`, `unwrap_or`, and `unwrap` on `Option`) and on `is None` narrowing. Methods the surface lacks (`Option.map`, `map_or`, `ok_or`, `expect`, `unwrap_or_default`, `is_ok`) are recorded as untranslatable or narrow the entry to `Result`; when a later RFC adds such a method, the clippy entry moves from "does not translate" to "translates" without a rename.
- **Rust interop**: `rust::` imports and `@rust.*` decorators are the Rust side. `[rust.lints]` and `@rust.allow(...)` own them; `@allow(...)` rejects a Rust lint name and `@rust.allow(...)` rejects an Incan rule name.
- **Expression vocab blocks**: a brace-form vocab block is opaque to every rule except the `format` class's preservation of its surface. A vocab may register its own catalog entries under a later RFC.
- **Conditional compilation**: `semantic`-tier rules evaluate the checked configuration; `syntax`-tier rules and the `format` class see every branch, and a `fix` entry must be meaning-preserving in every branch.

### Compatibility and migration

- A project with no `[incan.lints]` gets the catalog defaults. Nothing needs to be written to keep today's behavior, with one consequence stated in the guide-level explanation: `correctness` is `deny`, so a default project with a finding from a promoted `correctness` entry fails `incan architect`. On the day the catalog lands no lint entry other than `unreachable_code` is promoted, so a default project's `incan architect` output does not change; each promotion is a calibrated, release-noted step after that.
- A preview entry reports nothing until a level table names it and `--profile experimental` makes it eligible, so a rule added to the catalog never changes a project's output on upgrade; its promotion does, and is a release-notes item that records the calibration triage.
- Formatter output on already-formatted code does not change until a `format`-class entry lands or changes. Each such entry changes formatter output for code that matches it; each is listed in the release notes and projects see it as a one-time reformat. There is no preview period for the class, because a `format`-class entry has no level to hold at `allow`: the release note is the notice. The five new `format`-class entries this RFC names beyond RFC 053 and the style guide (`fmt_decorator_spacing`, `fmt_parentheses`, `fmt_digit_grouping`, `fmt_fstring_prefix`, `fmt_import_order`) are that reformat for this RFC; the redundant `pass`, the trailing bare `return`, the nested f-string, and the mergeable `from` pair are `fix` entries and change no file the formatter touches.
- A `fix`-mode entry never changes a file on its own: `incan architect --fix` is opt-in per invocation, and a project that runs it sees the rewrites the levels admit and the fix policy does not decline.
- `INCAN-T0101` keeps its code, its text, and its position in output; its level becomes configurable, and the entry enters promoted because it already ships. `unused_variables` and `unused_imports` are new checker by-products and enter in preview, so a project sees nothing from them on upgrade; their promotion, after calibration, is the release in which a project sees them as it would any new rustc lint. `wildcard_enum_match_arm` is new at clippy's `allow` default and reports nothing unless a project enables it.
- `[rust.lints]` and `@rust.allow(...)` are untouched.
- `incan.toml` is not a manifest after RFC 117; there is no legacy lint configuration to migrate.

## Prior art

- **clippy**: lint groups and default levels, `renamed_and_removed_lints`, `lint_groups_priority`, `blanket_clippy_restriction_lints`, suggestion applicability. The correspondence tables are drawn against clippy's published lint list.
- **rustc**: the four lint levels, `forbid` being unrelaxable, `allow(..., reason = "...")`, and the `unused` family this catalog adopts as `rustc`-origin entries.
- **Cargo's `[lints]` table**: the `"level"` and `{ level, priority }` entry forms and the priority ordering, adopted verbatim so `[incan.lints]` and `[rust.lints]` read the same; its table hierarchy (the language under `[lints]`) and its all-or-nothing `workspace = true` inheritance are not adopted, as the design decisions record.
- **Ruff**: the reference for the Python-shaped half of the surface; what is adopted, adapted, and rejected is tabled in the next section. Its `ruff format` / `ruff check --fix` split is the `incan fmt` / `incan architect --fix` split, with the seam between them closed.
- **Black**: the canonical, non-configurable formatter that this RFC keeps the `format` class faithful to.
- **The mypy, pylint, Ruff triad**: the analogy behind "three tools, three jobs": types, design advice and its safe fixes, and representation, respectively `incan check`, `incan architect`, and `incan fmt`.

### Prior art: Ruff

Ruff is worth a close reading because Incan is styled like Python, and Ruff is what the Python world converged on once one fast tool could hold what Flake8, its sixty-odd plugins, isort, pyupgrade, and Black used to hold separately. What it got right is mostly shape, not rules. A rule has a code and a name, and every rule page reads the same way: what it does, why it is bad, an example and what to use instead, how safe the fix is, which options apply. A fix is classified before it is applied: safe fixes run under `--fix`, unsafe ones only under `--unsafe-fixes`, display-only ones are shown and never applied, and the JSON output says which is which. New rules enter behind `preview` and stay there at least one minor release. Configuration is one `[lint]` table plus one `[format]` table, with `per-file-ignores` for the directory that needs a different policy. The formatter is Black, opinionated by design, with a handful of settings, and the linter's job is everything the formatter does not decide. Most striking, Ruff's preview rule categories are `correctness`, `suspicious`, `complexity`, `performance`, `style`, `security`, `formatting`, `pedantic`, and `restriction`, the first five on by default, with the per-tool linter groups slated for removal: a Python tool has arrived at clippy's taxonomy on its own.

What Ruff got right for Python is not all right for Incan, because much of Ruff exists to work around what Python lacks. It has hundreds of pycodestyle `E` and `W` codes because Python has no canonical formatter in the standard toolchain and a linter had to police whitespace; it needs `known-first-party` because a Python import path does not say where a module comes from; it needs an unsafe fix tier because its formatter and its linter hold two different trees and a lint fix may drop a comment; it needs `# noqa`, `--add-noqa`, `# ruff: noqa`, `# ruff: disable`/`enable`, and `RUF100` to police them, because a comment is the only metadata surface a Python statement has. The table records each decision and its reason.

| Ruff | Decision | Where it lands | Reason |
| --- | --- | --- | --- |
| Rule code plus rule name; names accepted in suppressions under preview | adopt, name primary | Rule identity | the name is what a Rust developer knows; the `INCAN-L` code is the diagnostic catalog's stable handle, and Ruff's own move to names in `ruff: ignore[...]` shows which of the two people reach for |
| Letter prefix encodes the source tool (`E`, `F`, `I`) | reject | Rule identity | the group is a catalog field, not a digit of the code, so an entry can move groups without renumbering, which Ruff's per-tool prefixes could not survive |
| Rule categories `correctness` … `restriction`, first five default-on | already adopted from clippy | Groups | independent confirmation; `security` has no entries here and RFC 105's `safety` is its neighbor; Ruff's `formatting` ("generally redundant with a code formatter") is this RFC's `format` class, made the formatter's outright and taken out of the level table |
| `select` / `ignore` / `extend-select` with breadth-based precedence | reject as a spelling | `[incan.lints]` | it is the Cargo level table said another way, and Cargo's explicit `priority` with a tie refusal is stricter than "narrower wins"; one table, one spelling |
| `per-file-ignores` | adapt as `[incan.lints.per-file-allow]` | `[incan.lints]` | the manifest-side form of a module-level `@allow(...)`; `allow` is its only verb, `forbid` stays unrelaxable, and in the lattice it is a `targeted_allow` |
| Nearest-config-wins discovery, `extend` | reject | Precedence | RFC 117 already says who is the authority over whom; a path-shaped precedence would let a nested file undercut the workspace floor |
| Fix applicability: safe, unsafe, display-only; `applicability` in JSON | adopt two of three, split safe by applier | Fix mode | `format` and `fix` are safe (the formatter's and `incan architect --fix`'s), `suggest` is display-only, `fix` carries it in JSON; no unsafe tier, because the line is drawn by construction and the trivia-aware tree removes Ruff's reason for one |
| `fixable` / `unfixable`, `extend-safe-fixes` / `extend-unsafe-fixes` | adapt the first pair as `fix = false` on the entry; reject the second | Fix mode | fix safety stays a catalog property, so no key moves an entry across the safe line; fix policy is the project's, and `fix = false` on a rule or group entry keeps the finding at its level and declines the rewrite, which is `unfixable` said where the rule is configured rather than in a second list |
| Per-rule documentation template; `ruff rule <code>` | adopt | Rule documentation | the same headings, rendered by `incan explain` by code or by name, generated from the catalog, with the example pair tested |
| `# noqa: CODE`, `# ruff: noqa`, `# ruff: disable`/`enable`, `--add-noqa` | reject | `@allow(...)` | declarations are Incan's metadata surface; a decorator is validated, recorded, and retired; `--add-noqa` institutionalizes sprawl. Statement scope is not added, and if a later RFC adds it the shape is Ruff's leading `ruff: ignore[rule]` on the line above, not a trailing `# noqa` |
| `unused-noqa` (`RUF100`) | adapt as `unused_allow` | `@allow(...)` | a suppression should not outlive what it suppressed; on by default because `@allow(...)` names only catalog rules |
| `preview` rules, `explicit-preview-rules`, one minor release before promotion | adapt as a flag on lint entries, with a calibration gate | Groups | preview entries default to `allow`, are outside `all` and their group for level purposes, are eligible only under `--profile experimental`, and are enabled by name; no manifest switch, because the manifest names rules; promotion requires a calibration run over the toolchain's corpus, not only elapsed time; no preview for the `format` class, because a formatter change has no level to hold back and ships as a release-noted reformat |
| isort sections and sort order (`unsorted-imports`, `I001`) | adapt the sectioning and sorting as `fmt_import_order`, a `format`-class entry; adapt the merging as `merge_imports`, a `fix`; drop the de-duplication | Import ordering | sections are decided by the root segment (`std`, `pub::`, `rust::`, project, parent), so `known-*` settings do not exist; imports are compile-time bindings with no import-time execution, so ordering is representation and belongs to the formatter; merging removes a statement node, so it is a fix; a name bound twice is the checker's error, so nothing de-duplicates; the rest of isort's settings are rejected row by row in that section |
| `ruff format` beside `ruff check --fix`, run in order; documented conflicting rules; `# fmt: off` | adopt the split, reject the seam | `incan fmt`, `incan architect --fix` | the formatter canonicalizes representation and the fixer restructures; `--fix` output is formatted before it is written, import ordering is the formatter's, and no lint may report what the `format` class normalizes, so there is no order to run in, no conflict list, and no opt-out |
| Formatter settings (`line-length`, `quote-style`, `indent-style`) | reject | `format` class | Black's stance, kept stricter than Ruff keeps it: the `format` class takes no project values and `[incan.lints]` never reaches the formatter |
| Exit codes `0` / `1` / `2`, `--exit-zero`, `--exit-non-zero-on-fix` | adopt the distinction, reject the third code and the flags | Exit codes | the toolchain is `0`/`1`; the JSON report tells findings from operational errors; levels are the CI policy |
| `ruff rule --all`, `--show-settings`, `--statistics`, twelve output formats | fold into `--list-rules`; reject the rest | `incan architect` | `--list-rules` answers "what exists" and "what is on here"; JSON is the one integration surface |
| pycodestyle `E`/`W` layout codes, `dummy-variable-rgx`, rules that exist because Python has no static types | reject | catalog | the `format` class makes layout codes impossible, the underscore convention is fixed, and `incan check` owns what parsing, resolution, typing, and flow analysis decide |

## Alternatives considered

### A separate `incan lint` command

Ruff ships `ruff check` beside `ruff format`. An `incan lint` would be a third analyzer beside `incan check` and `incan architect`, consuming the same facts as RFC 105's engine and reporting in a third shape, and an `incan lint --fix` would be a third rewriter beside `incan fmt` and `incan architect --fix`. Rejected: the engine is already specified by RFC 105 for exactly this evaluation, so the catalog's findings reach users through it, and `--fix` is a mode of that command, not a fourth command; the formatter stays a canonicalizer and `incan architect` owns every structural rewrite.

### Rule codes as the primary identity

Ruff identifies rules by code (`E501`) with a name as an alias. Rejected as primary: the name is what a Rust developer knows, and a code alone says nothing about correspondence. Codes exist (`INCAN-L`) because `incan explain` and tooling need a stable, never-renamed handle. Ruff itself now accepts rule names in its `ruff: ignore[...]` comments under preview, which is the direction this RFC starts from.

### Ruff-style `select` and `ignore` lists as a second spelling

`select = ["pedantic"]` and `ignore = ["print_stdout"]` could be accepted beside the level table, for a Python developer who knows them. Rejected: they say exactly what `pedantic = "warn"` and `print_stdout = "allow"` say, so one manifest could state one policy two ways and a reader would have to merge them; and Ruff's precedence ("the narrower selector wins") is what Cargo's `priority` makes explicit and refuses to leave ambiguous. The refusal for those keys names the spelling to use.

### An unsafe fix tier behind a flag

Ruff applies fixes that may change behavior or drop comments under `--unsafe-fixes`, and lets a project move a fix across the line with `extend-safe-fixes` and `extend-unsafe-fixes`. Rejected: the `fix` tier is defined by construction (syntax-only, meaning-preserving in every configuration, idempotent, structure-preserving), so there is no judgment call to expose as a flag, and the trivia-aware tree removes the comment-dropping reason for the tier. A later RFC that defines type-dependent or judgment-dependent fixes adds a mode named `unsafe` with its own flag; it does not loosen `fix`.

### Declining a fix by setting the rule to `allow`

An earlier draft of this RFC answered "I want the finding but not the rewrite" with "set the entry to `allow`", and on that basis refused Ruff's `fixable` and `unfixable`. Rejected, on review: `allow` silences the finding as well as the rewrite, so a project that wants `needless_else` reported in CI but hand-applied could not say so, and RFC 105's principle that detecting a finding and deciding to fix it are separate decisions was being met by collapsing the two. The answer is `fix = false` on the entry, which keeps the finding at its level and declines the rewrite; fix safety stays a catalog property, and no key moves an entry across that line.

### A `preview = true` manifest switch

Ruff turns every preview rule on at once with `preview = true`. Rejected: the manifest names rules, and a preview entry is enabled by naming it, which is Ruff's `explicit-preview-rules` made the only mode. A switch would be a second way to enable rules, one that changes meaning on every upgrade, and it would conflate eligibility with level.

### A Cargo-shaped `lints` table with the language beneath it

Cargo spells its lint tables with the language under `[lints]`: the Rust and clippy subtables of one `lints` table, which is the shape a Rust developer recognizes and the shape an earlier draft of this RFC chose for Incan's table. Rejected: the manifest namespaces by facet. RFC 117 writes `[incan.source]`, `[rust.source]`, and `[interop.c]`, #1698 writes `[rust.lints]`, and a `loaf.toml` never sits beside a `Cargo.toml`, so there is no file whose spelling it would match and one it would contradict. This RFC borrows Cargo's level and priority semantics, which is what a Rust developer needs to carry over, and not Cargo's table hierarchy. The facet-shaped `[incan.lints]` is the decision, recorded below, and a manifest that carries the Cargo shape is refused with a diagnostic naming the facet-shaped table.

### A side file for rule parameters

Clippy keeps thresholds and lists in `clippy.toml`. Rejected: #1698 settles the Rust side on one manifest and no `clippy.toml`, and the Incan side follows by putting parameters on the rule's own entry.

### Comment directives as the primary suppression

Ruff uses `# noqa: CODE` on a line, `# ruff: noqa` for a file, `# ruff: disable[...]`/`enable[...]` for a range, `# ruff: ignore[...]` above a logical line, and `--add-noqa` to write them for you; pylint uses `# pylint: disable=`. Rejected as primary: declarations are Incan's metadata surface (RFC 036 decorators, RFC 057 `@rust.allow`, RFC 096 metadata blocks), and a decorator is visible to the checker, the codegraph, and reflection in a way a comment is not. The file form is the module-position `@allow(...)` and the many-files form is the per-path allow table, so neither needs a comment. Statement-scope suppression is not added; Ruff's experience says that if a later RFC adds it, it is a leading directive above the statement, because a trailing `# noqa` on a multi-line construct forced Ruff to special-case strings and import blocks.

### Approximating ownership lints

`needless_borrow` could be approximated by "a `clone()` the planner would have elided", `needless_pass_by_value` by "a parameter never mutated". Rejected: every such approximation puts borrow vocabulary into user-facing text, which the language deliberately keeps out, and reports a decision the compiler makes rather than one the author made.

### The formatter as the lint fixer

An earlier draft of this RFC had `incan fmt` apply the type-independent lint fixes beside the `format` class, so that one command people already run would fix `needless_bool` as it fixes blank lines. Rejected, on review: a formatter whose output depends on `[incan.lints]` is no longer a canonical formatter, because a project that sets `needless_bool = "allow"` and one that does not would format the same file differently, and a formatter that restructures an `if`/`else` is no longer a formatter. `incan fmt` canonicalizes representation and reads no level table; the structural fixes are `incan architect --fix`'s, applied under the levels, with the output formatted before it is written so nothing is lost by the split.

### Type-dependent fixes under `--fix`

`incan architect --fix` could apply `comparison_to_empty` or `useless_conversion`, which need types, since the engine has them. Rejected for this RFC: a `fix` entry must be applicable to any file that parses, in every configuration, so that `--fix` never depends on the checked configuration and never rewrites one branch on facts from another; the fixable set is by construction the type-independent set. Type-dependent fixes are the `unsafe` mode a later RFC may add. A rewrite that hinges on a builtin's identity is not on that side: `str_in_fstring` needs to know that `str` at the call site is the builtin, and the builtin-name guard answers that on the tree, because every binding is lexical and the parser refuses wildcard imports; a rewrite that hinges on a method's receiver type (`iter_count`, `explicit_iter_loop`) is, and stays a `semantic` suggestion.

### Reserving the conversion builtins so `str_in_fstring` can be a fix

An earlier draft of this RFC made `f"{str(x)}"` to `f"{x}"` a `format`-class normalization guarded by "no local binding named `str` in the file", a second draft moved it to a `semantic` suggestion on the reasoning that only `print` is protected from rebinding and `str` may arrive as a parameter, a local, or an import, and the option in between was to reserve `str`, `int`, `float`, and `bool` the way `print` is reserved so that the guard would be a tree fact. Rejected, and unnecessary: rebinding a builtin is allowed by design and stays so, and the language does not change to make a rewrite syntax-decidable; but the guard was a tree fact all along. Every Incan binding is lexical and the parser refuses wildcard imports, so a parameter, a local, an import, or a declaration that rebinds `str` is on the tree of the file, and the builtin-name guard under "Declared facts" walks the enclosing scopes for one. With that guard `str_in_fstring` is a `syntax`-tier `fix`, silent where the name is rebound, and no name is reserved.

### Splicing nested f-strings in the formatter

An earlier draft made the nested f-string splice a `format`-class entry, `fmt_nested_fstring`, on the reasoning that a splice whose lexed parts are identical is representation. Rejected, on review: the splice removes the inner f-string node and re-nests its parts under the outer string, and a `format` entry never adds, removes, merges, or re-nests a node, whatever the lexer says about the result. The entry is the `fix` `nested_fstring`, applied by `incan architect --fix` under the levels, and the lex-and-compare stays as the fix's guard: the entry does not assume the splice is an identity, it proves it per construct by lexing the spliced text and the two-level form and comparing their part sequences, and it reports without a fix every construct where the sequences differ. A looser guard, splicing any inner f-string whose placeholder has no format spec on the reasoning that the value is unchanged, is rejected for the same reason as before: the reasoning is about the value, and neither the formatter nor a `syntax`-tier fix reads a literal's value (RFC 053's opacity rule); what can be compared is what the lexer produces.

### Removing `pass` and bare `return` in the formatter

An earlier draft gave the formatter `fmt_redundant_pass` and `fmt_bare_return`, and had `fmt_import_order` merge and de-duplicate `from` statements, on the reasoning that each leaves the program's meaning untouched. Rejected, on review, by the definition of representation: each removes a statement node, and merging two statements removes one and rewrites another, so each is structure however small it looks, and the formatter, which runs on every file with no level, must not restructure. They are the `fix` entries `redundant_pass`, `needless_bare_return`, and `merge_imports`, under the levels and the fix policy. De-duplication is dropped rather than moved: a name bound twice is already the checker's `ambiguous_import_binding`, and a rewrite that hides a checker error is not a fix.

### Configurable levels for formatter rules

`fmt_blank_lines = "allow"` would let a project keep three blank lines. Rejected: RFC 053 and the style guide are non-configurable by design, and a formatter whose output depends on a level table is no longer a canonical formatter. This is why `format` is a class with no level rather than a group with a fixed one.

## Drawbacks

- The catalog drifts from clippy unless someone reconciles it: clippy renames, deprecates, and regroups lints between releases. The pinned reconciliation baseline and the conformance check make drift visible, not impossible.
- Two identities per rule (name and code) cost documentation and a lookup table, in exchange for names people know and codes that never move.
- `incan check` reads `[incan.lints]` for its three `check`-evaluated entries. The checker already reads `loaf.toml` for compilation, so this adds a table to a dependency that exists, not a new dependency.
- Every `format`-class entry added later changes formatter output, and a project that pins nothing sees a reformat on upgrade. That is the price of a class with no level, and it is why the class is held to representation only.
- Two rewriters instead of one: a project that wants both canonical representation and the safe structural fixes runs `incan fmt` and `incan architect --fix`. The second formats its own output, so the order does not matter, but it is a second command to know about.
- `syntax`-tier heuristics can over-report where types would have said otherwise; the fact tier is declared per entry so the trade is explicit, and the `semantic` tier is available when the heuristic is not good enough.
- The seed catalog is large. The size is the point, since a partial correspondence table would leave a Rust developer guessing, but it is a review burden.
- Without an unsafe tier, a mechanical rewrite that might move a comment or change an exception path (`unnecessary_comprehension`, `comprehension_over_loop`) stays a suggestion the author applies by hand, where Ruff would apply it under `--unsafe-fixes`. That is the cost of a fixable set with no judgment calls in it.
- Every entry must ship its documentation, a tested example pair, and positive and negative fixtures before it can land, and every lint entry must pass a calibration run over the toolchain's corpus before it is on by default. Ruff's per-rule pages show that the discipline pays for itself, but it is a real cost per entry, and the seed catalog has more than two hundred of them.
- Because every seed lint entry lands in preview, the catalog reports nothing new by default on the day it lands; its value arrives promotion by promotion. That is the price of a default `deny` that means something.
- The builtin-name guard makes a `syntax`-tier entry silent in a scope that rebinds the builtin it recognizes, which is a missed finding where the rebinding is itself a mistake; the `semantic` tier is the answer where that matters, and the guard's tree walk is what keeps `--fix` off the checker.

## Implementation architecture

This section is non-normative. It describes a recommended shape, not a task list.

- **The catalog is a registry.** Like the language's other registries that generate reference tables, the catalog is a compiler-owned table of entries (name, code, group or class, default level, origin, clippy counterpart, fact tier, evaluator, fix mode, the preview flag of a lint entry, parameters, and the documentation template) from which the docs-site reference page, the `INCAN-L` records of the diagnostic catalog in `incan_syntax::diagnostics` that `incan explain` reads, and the example-pair fixtures are generated. A conformance test compares every `clippy` and `clippy-renamed` entry with the pinned clippy lint list, and a documentation test holds every entry's "before" and "after" example to the contract its evaluator declares: through the engine for a lint entry, through the formatter for a `format`-class entry, and through `incan architect --fix` against the formatted "after" for a `fix` entry. Beside the example pair, every entry has a positive and a negative fixture directory, and a calibration harness runs the engine over the stdlib, `examples/`, and the test fixtures and emits a triage sheet per entry, which is the artifact a promotion records.
- **One rule implementation per entry, one engine.** `syntax`-tier rules are functions over the trivia-aware syntax tree; `semantic`-tier rules are functions over RFC 105's typed fact views. `incan architect` runs both tiers and, under `--fix`, applies the rewrites of `fix`-mode findings, hands the result to the formatter, and runs both tiers again over what comes back, which is the lifecycle the reference-level explanation fixes. `incan fmt` runs the `format` class and no lint rule; the formatter holds no rule logic beyond that class. The `fix` rewrites live beside their rules in the engine, not in the formatter crate, and the formatter is a library the engine calls on the fixed tree.
- **Relationship to #159.** Rules are defined against the trivia-aware tree that #159 introduces, so comment and docstring trivia are inputs to a rule rather than repair work after it. Two sets follow from the classification in the design decisions. The `format` class gains five entries beyond RFC 053 and the style guide; the current AST-based formatter can host the three whose normalization moves no comment (`fmt_parentheses`, `fmt_digit_grouping`, `fmt_fstring_prefix`), and the two that move trivia or reorder declarations (`fmt_decorator_spacing`, `fmt_import_order`) wait for #159. The `fix` set is twelve entries; `incan architect --fix` can apply the nine whose rewrite moves no comment (`needless_bool`, `needless_bool_assign`, `needless_else`, `list_init_then_append`, `while_true`, `redundant_pass`, `needless_bare_return`, `nested_fstring`, `str_in_fstring`) on the current tree, and the three that move a branch body's comments with the body or reorder statements that carry comments (`if_not_else`, `redundant_else`, `merge_imports`) wait for #159. The builtin-name guard is one scope walk shared by every `syntax, builtin-guarded` entry, not a per-rule check.

## Layers affected

- **Parser / AST**: `@allow(...)` as a compiler-owned decorator on the declarations listed above and in the module-decorator position; no grammar change. `allow` becomes a reserved, compiler-owned decorator name that a user-defined decorator may not take. The trivia-aware tree is #159's, and every `syntax`-tier rule is defined against it.
- **Typechecker**: `unreachable_code` keeps its emitter and reads its level from the manifest; `unused_variables` and `unused_imports` are two new by-products of name resolution, emitted through the same channel. `@allow(...)` validation (unknown name, `forbid` conflict, Rust lint name, `format`-class name) is reported as an ordinary diagnostic.
- **Lowering / emission**: unaffected.
- **Stdlib / runtime**: unaffected.
- **Manifest / Oven (RFC 117)**: `[incan.lints]` and `[workspace.incan.lints]` parsing as facet tables, the `per-file-allow` subtable and its globs, the `fix` key and its policy join, the precedence lattice, and the refusals, including the refusal that names the spelling for a Ruff key or for a Cargo-shaped `lints` table; the manifest ownership map gains an `[incan.lints]` line owned by this RFC.
- **CLI / tooling**: `incan architect` resolves levels from the manifest through the lattice, distinguishes eligibility from level, carries the catalog fields in its findings, and gains `--fix` (with `--diff`), which applies `fix`-mode findings, formats the result, re-evaluates it, and reports and exits from the final evaluation; `incan fmt --check` reports each representation change by rule and gains `--format json`; `incan explain` resolves `INCAN-L` codes and bare rule names and renders the documentation template; `incan architect --list-rules` is a new flag on RFC 105's surface and lists the catalog with group or class and fix mode, and, for a lint entry, preview flag, eligibility, and effective level.
- **LSP / formatter**: the formatter hosts the `format` class and nothing else; it reads no manifest, no workspace table, and no `@allow(...)`, and the formatter crate becomes a library the engine calls on the tree `--fix` leaves. The language server surfaces catalog findings by calling the engine; `incan check` does not run the `syntax` tier.
- **Documentation**: a generated catalog reference page; the formatting how-to and the style guide gain their rule names, and the style guide records the import section order this RFC rules; the CLI reference documents `[incan.lints]`, `@allow(...)`, `incan architect --fix`, the `--check` output, and the exit codes.

## Inspectability and tooling surface

- **Artifact or metadata:** the catalog registry and its generated reference page; the effective level table per project, which `incan architect --list-rules --format json` prints with each rule's level, the manifest layer that set it, and its eligibility under the invocation.
- **Inspection command:** `incan architect --format json` for findings and, under `--fix`, for what was applied; `incan fmt --check --format json` for the `format` class; `incan explain INCAN-L####` or `incan explain <rule>` for any entry's documentation; `incan architect --list-rules` for the effective configuration, including which entries are in preview.
- **Diagnostics:** manifest refusals in RFC 117's diagnostic family, naming the entry and, for a clippy name, the recorded rename or reason; `@allow(...)` diagnostics at the decorator; a per-file note when `semantic`-tier rules were skipped because the file does not typecheck.
- **Provenance:** every finding carries its source span, its owning declaration, its rule name and code, its effective level, the layer that set the level (`default`, `workspace`, `member`, or the `@allow` decorator's span), and, for a suppressed rule, the `reason`; an applied fix carries `applied: true`.
- **Not implicit:** no rule runs that is not a catalog entry; no level comes from anywhere but the defaults, the two manifest tables, and `@allow(...)`; no rewrite is declined except by `fix = false` in the two manifest tables or by the entry's own guard; no command rewrites source except `incan fmt`, which applies only the `format` class, and `incan architect --fix`, which applies only `fix`-mode entries; nothing a project configures changes what `incan fmt` produces; no entry is on by default without a recorded calibration.

## Acceptance criteria

This RFC is done when:

- the catalog registry holds every entry in the tables above with name, code, group and default level or `format`-class membership, origin, fact tier, evaluator, fix mode, the preview flag of each lint entry and no preview flag on a `format`-class entry, parameters, and documentation in the template, a conformance test checks every `clippy` and `clippy-renamed` entry against the pinned clippy lint list, and a documentation test holds every entry's example pair to its evaluator's contract: a lint entry's "before" fires and its "after" does not, through the engine; formatting a `format`-class entry's "before" yields its "after" and formatting the "after" leaves it unchanged; `incan architect --fix` on a `fix` entry's "before" equals the formatted "after";
- every entry ships positive fixtures in which its finding fires and negative fixtures in which the counterexamples its risks name do not fire, and the fixture test fails an entry that lacks either set; every seed lint entry other than `unreachable_code` lands with `preview = true`; a calibration harness runs the engine over the stdlib, `examples/`, and the test fixtures and records a triage per entry, promotion is refused without one, and for a `correctness` entry with any untriaged finding; and `incan architect` over that corpus, with no configuration, reports no finding at `deny` or `forbid` on the day the catalog lands;
- RFC 105 is amended in the same change at the extension points listed under "Relationship to RFC 105", while its Oven-selected bilingual scope, provenance, evidence, typed-fact, de-duplication, rule-authoring, default-profile, and baseline contracts remain its own;
- `[incan.lints]`, `[workspace.incan.lints]`, and their `per-file-allow` subtables parse with every refusal listed above, and the lattice is covered by tests for the join over a workspace-named rule, for a member lowering a catalog default the workspace does not name and being unable to lower one it does, for the member-below-workspace refusal, for the per-path and `@allow(...)` exemptions, for `forbid` being outside their reach, for the same-priority conflict inside one table and its absence across tables, for a preview entry being enabled by name and eligible only under `--profile experimental`, for `fix = false` on a rule and on a group declining the rewrite while the finding is still reported, for a workspace `fix = false` that a member cannot lift, for a member's explicit `fix = true` against it being refused, for `fix` on a `suggest` or `none` entry being refused, and for a per-path table and an `@allow(...)` leaving a declined rewrite declined;
- `@allow(...)` is accepted on the listed declarations and in the module-decorator position, rejected elsewhere, and diagnosed under `forbid`, for `all`, for a Rust lint name, and for a `format`-class name;
- `incan architect --format json` findings carry qualified rule, bare rule, stable diagnostic code, group, effective level, level source, and severity derived from level, the catalog entry itself carries no severity, and the checker's entries appear once;
- `incan architect --fix` follows the lifecycle: it evaluates the original tree, applies every `fix`-mode finding at an effective level other than `allow` whose fix policy is not `false` and whose guard holds, and none at `allow`, none declined, and none whose guard fails, formats the result, re-evaluates it, reports the applied originals as `applied: true` and every final finding as `applied: false` in RFC 105's envelope with no separate applied-fixes array, no `fix_policy` field, and (`qualified_rule`, final span) as the identity of a final record, takes its exit status from the final evaluation alone, honors `--diff` with a declined or guard-failed rewrite not counted as pending, and passes an idempotency property test per `fix` entry and for the set as a whole, with the twelve `fix` entries being `needless_bool`, `needless_bool_assign`, `needless_else`, `redundant_else`, `if_not_else`, `list_init_then_append`, `while_true`, `redundant_pass`, `needless_bare_return`, `nested_fstring`, `merge_imports`, and `str_in_fstring`; `nested_fstring` is covered by a test in which the lexed part sequences differ and the finding is reported without a fix, and `merge_imports` by a test in which the two statements share a name and nothing is reported;
- `incan fmt --check` reports each representation change by rule name, fails on the `format` class and on nothing else, reads no lint table, emits no finding for an unchanged duplicate-binding import run, and emits the schema 2 report under `--format json`; `incan fmt` applies the fifteen entries of the class and no other, never removes a `pass` or a `return`, never merges or de-duplicates an import, and the idempotency property test covers each `format`-class entry;
- `str_in_fstring` is a `syntax`-tier entry under the builtin-name guard with fix mode `fix`, evaluated by `incan architect`, applied by `incan architect --fix`, and silent where an enclosing scope in the file binds `str`; the guard is covered by one test per binding form (parameter, assignment, `for`, `with`, `match`, comprehension, import, declaration) for one guarded entry, and every entry marked `syntax, builtin-guarded` shares the walk;
- `incan explain` resolves every `INCAN-L` code and every bare rule name to the documentation template, `unused_allow` reports a stale `@allow(...)` from probe results over the checked configuration, and `incan architect --list-rules` prints the effective table;
- the generated catalog reference page exists, and the formatting how-to, the style guide, and the CLI reference name the rules and the table.

## Design Decisions

- **`[incan.lints]`, facet-shaped.** Incan lint policy is `[incan.lints]`, workspace policy is `[workspace.incan.lints]`, and the per-path table is `[incan.lints.per-file-allow]`, beside #1698's `[rust.lints]`. The manifest namespaces by facet (`[incan.source]`, `[rust.source]`, `[interop.c]`); this RFC borrows Cargo's level and priority semantics, not its table hierarchy; and a `loaf.toml` never sits beside a `Cargo.toml`, so there is no file whose spelling the Cargo shape would match. The Cargo shape is refused with a diagnostic naming the facet-shaped table.
- **`incan fmt` canonicalizes; `incan architect --fix` rewrites.** The formatter applies the `format` class and nothing else, reads no level table, and produces the same output for every project. Safe structural fixes are `incan architect --fix`'s, applied under the lint levels and formatted before they are written. No other command rewrites source.
- **`format` is representation only.** Representation is trivia, layout, the spelling of a literal (quotes, digit grouping, the `f` prefix of a literal with no placeholder), redundant parentheses, and the order of independent declarations, which imports are. A `format` entry never adds, removes, merges, or re-nests a statement or expression node; a rewrite that does is a `fix` however small it looks, because the formatter runs on every file with no level and must not restructure. This is the test that decides membership in the class, and it is stated once under "Declared facts".
- **The twenty, re-sorted by kind.** The twenty type-independent rewrites an earlier draft gave the formatter are classified as representation, as structure, or as neither, by the definition above. Representation, and therefore the `format` class with no level, five entries: `precedence` and `double_parens` (as `fmt_parentheses`), `unreadable_literal` and `inconsistent_digit_grouping` (as `fmt_digit_grouping`), `empty_line_after_outer_attr` (as `fmt_decorator_spacing`), `useless_format`'s placeholder-free form (as `fmt_fstring_prefix`), and import sectioning and sorting (as `fmt_import_order`). Structure, and therefore fix mode `fix` under `incan architect --fix`, twelve entries: `needless_bool`, `needless_bool_assign`, `needless_else`, `redundant_else`, `if_not_else`, `list_init_then_append`, `while_true`, `redundant_pass`, `needless_bare_return`, `nested_fstring` (under the lex-and-compare guard), `merge_imports` (the merging half of import ordering, split out), and `str_in_fstring` (under the builtin-name guard). An earlier round placed `redundant_pass`, `needless_bare_return`, and `nested_fstring` in the class under `fmt_` names and had `fmt_import_order` merge and de-duplicate; that placement is superseded, because each removes or re-nests a node, and the alternatives record it. Neither, and therefore `suggest`: `range_plus_one` and `range_minus_one`, whose rewrite is not an identity at an overflowing bound. De-duplication of imports is dropped rather than sorted, because a name bound twice is the checker's error. Everything type-dependent stays `suggest`.
- **`range_plus_one` and `range_minus_one` are suggestions.** `range(a..(b + 1))` and `range(a..=b)` differ where `b + 1` overflows; the original fails there at run time, but integer overflow is not a documented language contract, so the difference is an observable change, and the `fix` contract (meaning preserved for every well-typed program in every configuration) is read strictly rather than loosened for the case. The `fix` set is the twelve entries above.
- **The builtin-name guard is a tree walk.** A `syntax`-tier entry that recognizes a builtin function by name applies only where no enclosing lexical scope in the file binds that name (parameter, assignment target, `for`/`with`/`match`/comprehension binding, import, declaration), and is silent where one does. It is not name resolution: every Incan binding is lexical and the parser refuses wildcard imports, so the tree shows every rebinding. `print` needs no guard, being the only protected builtin. Rebinding a builtin stays allowed by design; the guard is what makes a rewrite that hinges on a builtin's identity decidable on the tree without reserving the name.
- **A method name on an unknown receiver is a `semantic` fact.** An entry that recognizes a method by name on a receiver whose type the tree does not fix is `semantic`, unless its row says the shape is unambiguous by construction (a literal receiver such as `xs = []` or `Some(1)`, or a restriction on the spelling itself). The audit re-tiered `iter_skip_zero`, `no_effect_replace`, `match_str_case_mismatch`, `iter_count`, `needless_for_each`, `explicit_iter_loop`, `same_item_push`, and `as_conversions` to `semantic` on that ground; `list_init_then_append` keeps `syntax` and its fix because `xs = []` fixes the receiver's type.
- **Operator equivalence is semantic.** RFC 028 permits user-defined equality, ordering, arithmetic, in-place arithmetic, and bitwise methods, so syntax alone cannot establish that an operator expression is constant, impossible, redundant, or equivalent to another spelling. The affected entries require resolved primitive operands, side-effect-free shapes where evaluation count matters, and floating-point exclusions where NaN changes the claim. Layout-only entries such as `fmt_parentheses` remain syntax-tier because they do not interpret the operator.
- **`str_in_fstring` is a builtin-guarded fix.** Whether `str(x)` in a placeholder is the builtin is answered by the builtin-name guard on the tree, so the entry is `syntax`-tier with fix mode `fix`, in `style` at `warn`, with the clippy correspondence (`to_string_in_format_args`) recorded; it is silent where an enclosing scope binds `str`, and a placeholder with a format spec is reported without a fix. An earlier round made it a `semantic` suggestion on the reasoning that only `print` is protected; that reasoning conflated rebinding, which is allowed, with resolution, which the guard does not need.
- **`nested_fstring` proves each splice as its fix's guard.** The splice removes the inner f-string node and re-nests its parts, so it is a `fix`, not a `format` entry. It is admissible only where re-lexing the spliced text yields exactly the same sequence of literal and placeholder parts as the two-level form (expression text and format spec of each placeholder unchanged, `{{` and `}}` preserved, escapes decoding to the same text); the fix lexes both and compares the part sequences, applies where they agree, and reports without a fix otherwise. An inner literal with no placeholder is `fmt_fstring_prefix`'s, so the two never want one construct.
- **Fix policy is a project setting.** A rule or group entry in `[incan.lints]` or `[workspace.incan.lints]` may carry `fix = false`: the finding is reported at its level and `incan architect --fix` never applies its rewrite. `fix = true` is the default for a `fix`-mode entry and is refused on any other, because a suggestion cannot be promoted. Policy joins with `false` on top across layers, so a workspace `fix = false` is a floor, and neither a per-path table nor `@allow(...)` re-enables a declined rewrite. This keeps RFC 105's principle that detecting a finding and deciding to fix it are separate decisions, and it is what Ruff's `unfixable` becomes.
- **`incan architect --fix` has one lifecycle.** Evaluate the original tree; select and apply the eligible `fix`-mode findings whose effective level is not `allow`, whose fix policy is not `false`, and whose guard holds; run the `format` class over the result; re-evaluate the result; report every applied original as `applied: true` and every final finding as `applied: false` in RFC 105's envelope, with no separate applied-fixes array, no `fix_policy` field, and (`qualified_rule`, final span) as the identity of a final record, so a rule that still fires at the construct its fix was attempted on is one record; take the exit status from the final evaluation exclusively.
- **The documentation test proves the evaluator's contract, and fixtures prove the rule.** A lint entry's "before" produces the finding and its "after" does not, through the engine; formatting a `format`-class entry's "before" produces its "after" and formatting the "after" leaves it unchanged; `incan architect --fix` on a `fix` entry's "before" equals the formatted "after". One template, three proofs, chosen by the declared evaluator. Beside the pair, every entry ships positive fixtures and negative fixtures for the counterexamples its risks name, which is RFC 105's authoring contract kept as a gate.
- **Preview is a property of lint entries, and promotion is calibrated.** A `format`-class entry has no preview flag in the entry shape, in `--list-rules`, or in the rule documentation header: it has no level to hold at `allow`, so a new normalization ships as a formatter change, listed in the release notes, and a project sees a one-time reformat. Every seed lint entry lands in preview, except `unreachable_code`, which already ships. Promotion requires a calibration run over the toolchain's own corpus (the stdlib, `examples/`, the test fixtures) with every finding triaged, and for a `correctness` entry zero untriaged findings, so "a default project can fail `incan architect` with no configuration" is true of promoted entries only and the seed catalog fails nothing on the day it lands.
- **A duplicate import binding is the checker's, and nothing de-duplicates.** No lint or formatter entry reports it: the checker already refuses an ambiguous binding (`ambiguous_import_binding`, `INCAN-I0001`). `fmt_import_order` leaves such a run as written and emits no finding because it would make no change; `merge_imports` does not report it. A rewrite that removed the duplicate would hide a checker error, so there is none.
- **RFC 127 extends RFC 105; it does not replace it.** RFC 105 keeps the engine, Oven-selected bilingual scope, provenance, typed facts, evidence, and base finding contract. This RFC owns catalog identity and policy: levels, suppression, fix applicability, fix policy, the `--fix` lifecycle, and formatter boundaries. RFC 105 stays Draft on its remaining questions, and its default profile and baselines stay open there.
- **Entry metadata and instance fields are distinct.** The catalog entry carries name, code, group or class, default level, fact tier, evaluator, fix mode, and, for a lint entry, the preview flag. A diagnostic instance carries the effective `level` and the presentation `severity` derived from it; a `format`-class finding under `--check` is a `warning`. Severity is never an entry field, and the diagnostic catalog's projection leaves it to the instance.
- **`format` is a class, not a group.** A `format`-class entry has no level, is not covered by `all`, and cannot be named in a level table, a per-path table, or `@allow(...)`; it is representation, and there is nothing to set. `all` is a selector for the five default-on groups, not a group.
- **Precedence is a lattice.** `member_level` is the member's assignment or else the catalog default; `project_level = max(workspace, member_level)` over `allow < warn < deny < forbid` when the workspace assigns the rule, and `member_level` when it does not; priority is resolved inside each manifest layer independently; `location_level = allow` under a per-path or `@allow(...)` exemption unless `project_level` is `forbid`. The floor is the workspace's explicit assignments only: a workspace that wants a floor on a rule must name it (workspace `unwrap_used = "deny"`, and a member cannot lower it; workspace silent on `fstring_over_concat`, and a member's `allow` holds). A rule the workspace does not name is the member's to decide, exactly as in a standalone project. The workspace is the package-policy floor, subject to explicit scoped exemptions; a member composes monotonically; a path or a declaration exempts. This diverges from Cargo's all-or-nothing `[lints] workspace = true`, which cannot merge a member table with the workspace's, and it is what RFC 117's "narrow but not replace" means for lint policy.
- **Eligibility is not level.** A finding is produced iff the rule is eligible under the invocation (`--profile`, or the default invocation for a stable rule; `--profile experimental` for a preview rule) and its effective level is not `allow`. Group membership never raises a preview rule's level; naming it in a level table does.
- **`wildcard_enum_match_arm` stays at `allow`.** Clippy is the calibration reference, and the checker enforces exhaustiveness, so a wildcard arm an author writes on an enum is deliberate; a project that wants it reported enables the `restriction` entry by name.
- **Import ordering is representation; merging is a fix.** An Incan module is declarations only, a `static` initializer is a storage cell, and an import is a compile-time binding resolved under the reserved roots (RFC 022, RFC 031), so there is no import-time execution and reordering cannot change program meaning. `fmt_import_order` sections, sorts, and spaces the sections, and nothing else; merging two `from` statements for one module removes a statement node, so it is the `fix` `merge_imports`; a run that binds one name twice is left as written without a formatter finding, and the checker reports the binding error. The section order (`std`, `pub::`, `rust::`, project, parent-relative) is this RFC's ruling. No `known-*` configuration exists, because the root segment decides the section.
- **Tiers are availability, not the fact model.** `syntax` and `semantic` say when a rule can run; RFC 105's registry says which facts it consumes.
- **`unused_allow` reasons in probe mode over the checked configuration.** An allowed rule may be skipped, except that inspecting suppression provenance evaluates it in probe mode, whose results never become findings. A suppression for code under another configuration is reported only when that configuration is checked.
- **Codes are stable; `INCAN-L` is the allocation family.** Every entry has a stable diagnostic code; new lint entries receive `INCAN-L####`, and an entry promoted from an existing diagnostic keeps its code.
- **Formatter rules are not configurable.** `fmt_line_length`, `fmt_indentation`, `fmt_quote_style`, and `fmt_trailing_comma` take no project values, even though the library formatter can accept them. RFC 053 places any formatter configurability in a separate RFC and the style guide is non-configurable by design; a formatter whose output depends on a level table is no longer a canonical formatter.
- **Reconciliation follows the toolchain's Rust release.** The catalog pins the clippy lint list that ships with the Rust release the toolchain pins, and reconciliation runs when that release changes, as #1698 does for `[rust.lints]`. There is no separate schedule: a clippy release the toolchain does not build with is not one users meet.
- **`@allow("all")` is rejected.** A group name is accepted on a declaration; `all` is not, because allowing every default-on rule for a declaration is a blanket, and blankets belong in the manifest.
- **RFC 105's group defaults are RFC 105's.** The default levels of `arch`, `safety`, `idiom`, `maintainability`, and `risk` are decided in RFC 105, whose default profile is open there. This RFC gives those groups a place in the level table and nothing more.
- **Fix safety is a catalog property; fix policy is a project setting.** `format` and `fix` are Ruff's safe tier, split by who applies them, and `suggest` is its display-only tier, decided by the entry's constraints and not by a project setting; there is no unsafe tier, no `--unsafe-fixes`, and no `extend-safe-fixes`/`extend-unsafe-fixes` key, and no key moves an entry across the safe line. Fix policy is `fix = false` on a rule or group entry, which keeps the finding and declines the rewrite; Ruff's `fixable`/`unfixable` lists are refused as keys and answered by that spelling. A later RFC that wants type-dependent fixes adds a mode named `unsafe` with its own flag.
- **One selection spelling.** The Cargo level table is the only way to turn a rule on or off; Ruff's `select`/`ignore`/`extend-select` are refused with a diagnostic that names the equivalent entry.
- **Preview entries are named, not switched on.** A preview entry defaults to `allow`, is outside `all` and its group for level purposes, and is enabled by naming it; there is no `preview = true` manifest switch.
- **Every entry documents itself.** The documentation template is part of the entry, generated into the diagnostic catalog and the reference page, and its example pair is tested under its evaluator's contract; an entry without it cannot land.
- **`incan fmt` reports only the `format` class.** The formatter prints nothing it does not apply, so a `syntax`-tier lint finding never appears in `incan fmt` output; `incan architect` is the one reporting surface for findings, and a second one would make two commands disagree about the same file.
- **Suppression scopes are declaration, module, and path.** `@allow(...)` on a declaration or in the module position, and `[incan.lints.per-file-allow]` for a directory, are the three scopes; no statement-scope directive is added, because every finding sits inside a declaration and the decorator reaches it there, and a narrower form would be a second suppression syntax for a case the three already cover.
- **`[incan.lints.per-file-allow]` stays.** A `tests/` directory would otherwise carry one identical module-position `@allow(...)` per file; the table is that decorator written once, keeps Ruff's single verb, and cannot relax a `forbid`.
- **Exit codes stay `0`/`1`.** The toolchain's convention is one success code and one failure code, the JSON report tells findings from operational errors, and a third code is a toolchain-wide change that RFC 118 decides, not this RFC.
- **The language server calls the engine.** Catalog findings reach the editor through `incan architect`'s engine, not through `incan check` running the `syntax` tier, so the checker keeps reporting compiler facts and language errors only and one evaluator produces every finding.
