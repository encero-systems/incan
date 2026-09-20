# RFC 127: The Incan lint catalogue: clippy-conformant rules, `loaf.toml` configuration, `incan architect` as engine, `incan fmt` as fixer

- **Status:** Draft
- **Created:** 2026-09-20
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 053 (formatter vertical spacing buckets)
    - RFC 057 (`@rust.allow(...)` targeted Rust lint suppression)
    - RFC 105 (`incan architect` rule engine; the engine and finding contract this catalogue is evaluated by)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (Oven-native Rust build facets; the home of `[rust.lints]`)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6.0-dev.6
- **Shipped in:** —

## Summary

This RFC defines one catalogue of Incan lint rules and the contract that configures it. The catalogue follows clippy's: where an Incan construct translates a Rust construct, the rule keeps clippy's name, group, and default level; where the construct exists but clippy's name would mislead, the rule is renamed and the correspondence is recorded; where the Rust construct has no Incan counterpart (ownership, borrowing, lifetimes, raw pointers, `unsafe`, `as` casts), the catalogue says so instead of inventing an approximation. Levels (`allow`, `warn`, `deny`, `forbid`), groups (`correctness`, `suspicious`, `style`, `complexity`, `perf`, `pedantic`, `restriction`), priorities, and defaults are clippy's, and the configuration is a `[lints.incan]` table in `loaf.toml` beside the `[rust.lints]` table that governs a project's Rust facets, so one manifest carries both sides of a mixed project and they cannot drift apart. RFC 105's `incan architect` engine evaluates the catalogue over compiler-backed facts, with `allow` and `warn` as advice and `deny` and `forbid` as enforcement, and `incan fmt` applies the auto-fixable style subset while it formats and fails `--check` on it, so style is enforced by the tool people already run without the formatter growing a semantic analyser. RFC 053's spacing rules become catalogue entries the formatter fixes, and RFC 057's `@rust.allow(...)` gains an Incan-side twin, `@allow(...)`, for targeted suppression.

## Core model

1. **One catalogue.** Every advisory rule Incan tooling can report, whether a style rule, a correctness heuristic, a formatter normalization, or one of RFC 105's design findings, is an entry in one compiler-owned catalogue with one name, one stable diagnostic code, one group, one default level, one declared fact tier, one declared evaluator, and one declared fix mode.
2. **Clippy is the reference.** A rule that reports the same defect on the translated construct carries clippy's name and sits in clippy's group at clippy's default level. Renames and non-translations are recorded in the catalogue, not left to folklore, and a rule that clippy has no notion of is named for the Incan construct it reports on.
3. **Three tools, three jobs.** `incan check` reports what the type system knows. `incan architect` evaluates the catalogue and reports findings. `incan fmt` rewrites source. None of the three duplicates another's analysis: the engine is RFC 105's, and the formatter calls the engine's syntactic rules rather than growing its own.
4. **Levels are clippy's.** `allow`, `warn`, `deny`, and `forbid` mean what they mean in rustc and clippy; groups carry a priority so a single rule can override its group; `all` is the default-on set. Advice is `allow` and `warn`; enforcement is `deny` and `forbid`.
5. **One manifest.** Levels and rule parameters live in `loaf.toml` under `[lints.incan]`, and workspace policy under `[workspace.lints.incan]`. There is no `clippy.toml`, no `.incanfmt`, and no other side file.
6. **Suppression is targeted.** `@allow("rule", reason="…")` on a declaration narrows a rule at the smallest scope, exactly as RFC 057's `@rust.allow(...)` does for the generated Rust of that declaration.
7. **Fixes are the formatter's.** A catalogue entry marked fixable by the formatter is decidable on the syntax tree alone, idempotent, and structure-preserving. `incan fmt` applies it, `incan fmt --check` fails on it, and no other command rewrites source.

## Motivation

Incan today decides "style" in three places, and none of them is a catalogue. The formatter normalizes layout by rules stated in RFC 053 and the style guide, but reports every deviation as "file would be reformatted". The checker emits a handful of advisories (an unused binding, an unused import, a wildcard match arm, unreachable code after `return`) with no way to set their level. RFC 105 proposes an engine for design findings with its own categories and leaves suppression, configuration, and the relationship to formatting unresolved. A project that also carries Rust facets configures clippy in a fourth place. A Rust developer who reaches for `needless_bool` or `unwrap_used` finds nothing by that name; a Python developer who expects Ruff's "format and lint in one tool" finds a formatter that does not lint.

Clippy is the right reference for three reasons. First, expectations: Incan's audience includes Rust developers, and a rule that has the same name and the same meaning as the clippy lint they already know costs nothing to learn. Second, the compiler itself: the toolchain's own Rust facets are linted with clippy, and #1698 moves that configuration into `loaf.toml` as `[rust.lints]`; if Incan's rules used other names, levels, or defaults, one repository would carry two vocabularies for the same intent. Third, calibration: clippy's group boundaries (`correctness` denied; `suspicious`, `style`, `complexity`, and `perf` warned; `pedantic` and `restriction` opt-in) encode years of judgement about what a default-on lint may cost, so adopting them means Incan starts from a defensible default set rather than inventing one.

Clippy is not a template to copy blindly. A large share of its catalogue exists because Rust exposes ownership, borrowing, lifetimes, raw pointers, `unsafe`, and `as` casts. Incan exposes none of those: the compiler's duckborrowing planner decides how generated Rust moves, borrows, or clones, and source code never spells a borrow. An honest catalogue lists those lints as untranslatable by construction rather than inventing lookalikes, and it adds the rules a Python-shaped language needs that clippy has no notion of: f-strings, comprehensions and generator expressions, docstrings, decorators, `mut self`, `pass`, and `...` bodies.

## Goals

- Define the rule model: identity, naming, groups, levels, priority, defaults, declared facts, declared evaluator, and declared fix mode.
- Publish the correspondence between clippy's catalogue and Incan's, group by group, in three dispositions: translates, translates under another name, does not translate.
- Name the entries without a clippy counterpart: the formatter's normalizations from RFC 053 and the style guide, the rustc lints that translate, and the Python-shaped idiom rules.
- Define `[lints.incan]` and `[workspace.lints.incan]` in `loaf.toml`, their precedence, and the manifest refusals.
- Define `@allow(...)` as the Incan-side twin of `@rust.allow(...)`.
- Define how `incan architect` evaluates the catalogue and how `incan fmt` fixes its fixable subset, including `--check` semantics, machine-readable output, and exit codes.
- Give #159's trivia-aware formatter its rule surface: every formatter rewrite has a name, a code, and a reason.

## Non-Goals

- Changing language syntax or semantics. Every rule reports on programs the compiler already accepts.
- Replacing `incan check`. Type errors, resolution errors, and exhaustiveness are the checker's and stay errors, not lints.
- Redefining RFC 105's engine, finding record, evidence model, profiles, priorities, or confidence. RFC 105 defines the engine and the finding contract; this RFC defines the rule set and the configuration that engine evaluates.
- Auto-fixing anything the formatter cannot decide from the syntax tree. `incan architect` reports; it does not rewrite. A later RFC may define engine-applied fixes.
- Linting generated Rust. `[rust.lints]` (#1698, RFC 119) and `@rust.allow(...)` (RFC 057) own the Rust side.
- A `# fmt: off` region or any other formatter opt-out. None exists today and this RFC adds none.
- Statement-level suppression. `@allow(...)` attaches to declarations; a narrower form is an unresolved question.
- Porting all of clippy. The catalogue grows by correspondence entries; this RFC fixes the model and seeds it with the entries in the tables below.
- Nursery and cargo lints. Nursery lints are not stable in clippy; cargo lints check `Cargo.toml`, whose Incan counterpart is validated by RFC 117 itself.

## Guide-level explanation

### One catalogue, three tools

A rule in the catalogue is something like `needless_bool`. It reports an `if` whose two branches only return `true` and `false`:

```incan
def is_adult(age: int) -> bool:
    if age >= 18:
        return true
    else:
        return false
```

`needless_bool` is a clippy lint. It translates: the construct is the same, the defect is the same, and the fix is the same, `return age >= 18`. So the Incan rule has the same name, sits in the same group (`complexity`), and has the same default level (`warn`).

Two commands act on the rule. `incan architect` evaluates the catalogue and reports the finding in RFC 105's shape:

```text
[P3] complexity.needless_bool (warn): an `if` whose branches only return `true` and `false`
Suggestions:
  - Return the condition itself: `return age >= 18`
Evidence:
  - src/users.incn:12:5 in is_adult (fixed by `incan fmt`)
```

`incan fmt` rewrites it, because the catalogue marks `needless_bool` as fixable by the formatter: the rewrite needs no type information, it is idempotent, and it preserves the structure of everything around it. After `incan fmt` the finding is gone. `incan fmt --check` fails on it, the way it already fails on a missing trailing newline:

```text
src/users.incn:12:5 needless_bool: would rewrite the `if`/`else` to `return age >= 18`
src/users.incn:20:1 fmt_blank_lines: three blank lines between top-level declarations; at most two
1 file would be reformatted
```

The third command, `incan check`, is not involved. It reports what the type system knows, and `is_adult` typechecks.

The same catalogue holds rules the formatter cannot fix. `too_many_arguments` needs a threshold and a human decision; `unwrap_used` is a policy, not a rewrite; `float_cmp` needs to know that both operands are floats. `incan architect` reports those, and the level table decides whether they are advice (`warn`) or enforcement (`deny`).

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

# Rust facets: rustc and clippy lints with Cargo's [lints] semantics (#1698, RFC 119).
[rust.lints]
unsafe_code = "forbid"
clippy.all = { level = "warn", priority = -1 }
clippy.pedantic = { level = "warn", priority = -1 }
clippy.unwrap_used = "deny"

# Incan sources: the same names, the same levels, the same priority rules.
[lints.incan]
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"
too_many_arguments = { level = "warn", threshold = 6 }
print_stdout = "warn"
fstring_over_concat = "allow"
```

The `[rust.lints]` shape is #1698's and appears here only to put the two tables side by side. The `[lints.incan]` table reads the way a Rust developer expects: a group at priority `-1` so the single rules below it win, a rule raised to `deny`, a rule with a parameter, an Incan-only rule switched off. A project that writes no `[lints.incan]` gets clippy's defaults.

### Suppressing at the smallest scope

When a rule is right in general and wrong for one declaration, the declaration says so:

```incan
@allow("too_many_arguments", reason="mirrors the wire format field by field")
def decode_header(version: int, flags: int, length: int, stream: int, offset: int, crc: int, padding: int) -> Header:
    ...
```

`@allow(...)` is the Incan twin of `@rust.allow(...)`: a compiler-owned decorator, string arguments that name catalogue rules or groups, an optional `reason`, and an effect limited to the declaration it decorates. A `@rust.allow("clippy::unwrap_used")` on the same declaration still means the generated Rust; the two decorators never name each other's rules.

### What does not translate, and why

Clippy's `needless_borrow`, `ptr_arg`, `needless_lifetimes`, and `needless_pass_by_value` have no Incan rule because Incan source has no borrow, no lifetime, and no pass-by-value spelling to get wrong: parameters are values, `self` and `mut self` say whether a method mutates, and the compiler plans the Rust shape. Clippy's `needless_return` has no Incan rule because `return` is the only way to yield a value: there are no tail expressions to prefer. Clippy's `single_match` translates only for `Option`, because `if x is not None:` is a real narrowing form and there is no `if let` for an enum payload. Each of those facts is a row in the correspondence tables, so a Rust developer who types a clippy name into `[lints.incan]` is told exactly why it is not there.

### The formatter's own rules

RFC 053's three blank-line buckets, the trailing newline, docstring interior spacing, comment placement, and the style guide's horizontal spacing are catalogue entries in a `format` group named `fmt_*`. They are not configurable, they are always applied by `incan fmt`, and they are reported by name under `--check`. Their value is identity: a formatter rewrite is no longer "the file changed" but `fmt_blank_lines` at a line, with a reason, which is what #159's trivia-aware formatter needs to be testable rule by rule.

## Reference-level explanation

### Rule identity and naming

Every catalogue entry must have:

- a **name**: a `lower_snake_case` identifier, unique across the catalogue, used in `[lints.incan]`, in `@allow(...)`, and in human output;
- a **stable code** in the `INCAN-L` family (`INCAN-L0001`, `INCAN-L0002`, …), never reused and never renumbered, resolved by `incan explain`; an entry that already carries a stable code today (for example `INCAN-T0101` for unreachable code) keeps it;
- a **group** (exactly one);
- a **default level**;
- an **origin**: `clippy` (same name and meaning as the clippy lint), `clippy-renamed` (an Incan name with the clippy counterpart recorded), `rustc` (a rustc lint that translates), or `incan` (no counterpart);
- a **fact tier**, an **evaluator**, and a **fix mode** as defined below;
- zero or more **parameters**, each with a name, a type, and a default.

Naming rules:

- A rule whose origin is `clippy` must use clippy's name verbatim and must report the defect clippy's lint reports, on the translated construct. A name may be reused from clippy only when the rule is the same; the catalogue must not carry a clippy name with a different meaning.
- A rule whose origin is `clippy-renamed` must not collide with any current clippy lint name, and its catalogue entry must record the clippy lint it corresponds to and the reason for the different name.
- A rule whose origin is `incan` must not collide with any current clippy lint name. Formatter-owned rules use the `fmt_` prefix; other Incan-only rules are named for the Incan construct they report on (`fstring_`, `comprehension_`, `docstring_`, `ellipsis_`, and so on).
- The catalogue records the clippy release it was last reconciled against. A conformance check must verify every `clippy` and `clippy-renamed` entry against that release's lint list.
- In RFC 105's finding record, the rule code is the group-qualified name (`complexity.needless_bool`). The qualifier is derived from the entry's group and is never authored by users; configuration and suppression use the bare name.

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
| `format` | fixed | n/a | formatter normalizations; always applied, not configurable |
| RFC 105's `arch`, `safety`, `idiom`, `maintainability`, `risk`, `experimental` | as RFC 105 defines | `experimental` never | evidence-backed design findings |

Level semantics must match rustc's and clippy's:

- `allow`: the rule is not evaluated for reporting and the formatter does not apply its fix.
- `warn`: the rule is reported; the exit code is unaffected; the formatter applies its fix when the entry is fixable.
- `deny`: the rule is reported; `incan architect` exits non-zero; the formatter applies its fix when the entry is fixable, after which the finding no longer exists.
- `forbid`: as `deny`, and no lower layer (a member manifest or `@allow`) may relax it.

Priority semantics must match Cargo's `[lints]` table: every entry has an integer `priority`, `0` by default; entries apply in ascending priority order, so a higher priority wins; a group is expected to be given a negative priority so that single rules override it. A group and one of its member rules at the same priority is a manifest refusal, not a warning (clippy's `lint_groups_priority` becomes a refusal because RFC 117 validates manifests strictly).

`all` names the default-on set: `correctness`, `suspicious`, `style`, `complexity`, and `perf`. It never includes `pedantic`, `restriction`, `format`, or `experimental`. A rule belongs to exactly one group, and its default level is its group's default level; clippy has no exceptions to that and neither does this catalogue.

### Declared facts, evaluator, and fix mode

Every entry declares:

- **Fact tier**: `syntax` when the rule is decidable on the trivia-aware syntax tree of one file, or `semantic` when it needs resolved names, types, or other codegraph facts. A `syntax` rule must run on any file that parses; a `semantic` rule must be skipped, with a note in the output, for a file the checker rejects.
- **Evaluator**: `architect` (RFC 105's engine), `check` (the checker emits it as a by-product of resolution or flow analysis), or `fmt` (the formatter applies it as part of formatting).
- **Fix mode**: `fmt` (the formatter applies the rewrite), `suggest` (the finding carries a suggestion and nothing rewrites source), or `none`.

Constraints:

- A `fmt` fix requires the `syntax` tier: the formatter must never need the checker to decide a rewrite.
- A `fmt` fix must be meaning-preserving for every well-typed program regardless of the types involved, idempotent (`fmt(fmt(x)) == fmt(x)`), and structure-preserving (the syntax tree after the rewrite differs from the tree before it only at the reported construct and its trivia). These are #159's invariants, stated per entry.
- The catalogue is the only source of the fixable set. No command applies a rewrite for an entry whose fix mode is not `fmt`.
- The syntactic rule implementations are shared: `incan fmt` invokes the engine's `syntax`-tier rules over the tree it already holds. There is no second matcher.

### The `[lints.incan]` table

`[lints.incan]` is a table in `loaf.toml` (RFC 117). Each key is a rule name or a group name; each value is either a level string or an inline table:

```toml
[lints.incan]
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"
too_many_arguments = { level = "warn", threshold = 6 }
disallowed_names = { level = "deny", names = ["foo", "bar", "baz", "tmp"] }
```

Rules:

- The value must be one of `"allow"`, `"warn"`, `"deny"`, `"forbid"`, or an inline table with a required `level` of the same domain, an optional integer `priority`, and, for a rule entry, that rule's declared parameters.
- A group entry must not carry parameters.
- A rule entry may set a parameter only when the catalogue declares it for that rule; the value must have the declared type. An unset parameter takes the catalogue default.
- Entries for the `format` group or for any `fmt_*` rule must be refused: formatter rules are not configurable through the level table.
- An entry for the `restriction` group as a whole must be refused: restriction rules are enabled individually, as clippy's `blanket_clippy_restriction_lints` insists.
- `[lints.incan]` never carries a Rust lint, and `[rust.lints]` never carries an Incan rule.

### Workspace policy and precedence

A workspace root (RFC 117) may declare `[workspace.lints.incan]` with the same shape. Lint policy is workspace policy: it applies to every selected member without an opt-in, because RFC 117 makes the root the authority for policy and a member may narrow but not silently replace it.

The effective level of a rule at a source location is resolved in this order, each layer overriding the previous one entry by entry:

1. the catalogue default for the rule;
2. `[workspace.lints.incan]` at the workspace root, in priority order;
3. `[lints.incan]` in the member's own `loaf.toml`, in priority order;
4. `@allow(...)` on the enclosing declarations, innermost last.

A member manifest may raise or lower a level the workspace set, except that it must not relax a workspace `forbid`. `@allow(...)` may lower any level except `forbid`. There are no command-line level flags: the manifest is the single source of truth, and a flag would be a second one.

### Manifest refusals

Oven must refuse a manifest, with the diagnostic naming the offending entry, when:

- a key names neither a catalogue rule nor a group; when the key is a clippy lint the catalogue records as renamed, the diagnostic must name the Incan rule; when it is a clippy lint the catalogue records as untranslatable, the diagnostic must quote the recorded reason;
- a level is not one of the four level strings;
- a group and one of its member rules are given the same priority;
- a member manifest lowers a level the workspace root set to `forbid`;
- an entry targets the `format` group, an `fmt_*` rule, or the `restriction` group as a whole;
- a parameter is unknown for the rule, has the wrong type, or appears on a group entry.

### `@allow(...)`

`@allow(...)` is a compiler-owned decorator, defined by analogy with RFC 057:

- It must take one or more string literal arguments, each naming a catalogue rule or group, and may take a `reason` keyword argument whose value is a string literal. An empty argument list, a non-string argument, a duplicate name, and an unknown name must be rejected.
- It may appear on any declaration that owns a scope: a function, an `async def`, a method, a class, a model, a trait, an enum, a newtype, a type alias, a `const` or `static` binding, and, in the module-decorator position, the module itself. It must be rejected on statements, expressions, imports, and fields.
- Its effect is to set the named rules to `allow` for the decorated declaration and everything lexically inside it. Naming a group allows every rule in the group.
- Naming a rule whose effective level is `forbid` must produce a diagnostic at the decorator, as rustc does for an `allow` under a `forbid`.
- Naming an `fmt_*` rule must be rejected: this RFC provides no formatter opt-out.
- `@allow(...)` never names a Rust lint; `@rust.allow(...)` never names an Incan rule. Each must reject the other's names.
- The restriction rule `allow_without_reason` reports an `@allow(...)` with no `reason`.

### `incan architect` evaluates the catalogue

`incan architect` (RFC 105) is the evaluator for every entry whose evaluator is `architect`:

- It must resolve the effective level of every rule from the precedence chain above and must not evaluate rules at `allow`.
- It must include entries the checker emits (evaluator `check`) by projecting the codegraph's diagnostic records into findings rather than re-evaluating them, so a checker advisory is never reported twice.
- Findings must carry RFC 105's record (rule code, category, priority, confidence, evidence, suggestions, risks) and, for catalogue entries, the rule name, the stable code, the group, the effective level, and the manifest layer that set it.
- `--profile` selects groups; the level table decides what is on. A `syntax`-tier rule must run over a file the checker rejects, and a `semantic`-tier rule must be reported as skipped for that file.

### `incan fmt` fixes the fixable subset

`incan fmt` keeps its command surface (`incan fmt [PATH]`, `--check`, `--diff`, `--workspace`, `--member`) and gains catalogue identities:

- Formatting applies every `format`-group entry and every entry whose fix mode is `fmt` and whose effective level is not `allow`.
- `--check` must report each construct that formatting would change by rule name, location, and a one-line reason, and must exit non-zero when any file would change.
- `--diff` shows the rewrite as today.
- `incan fmt` must not report entries it does not fix. Reporting is `incan architect`'s job; the two commands share the syntactic rule implementations, not the output.
- A file with a syntax error is refused, as today.
- Applying the fixable set must be idempotent as a whole: running `incan fmt` twice must produce the same file as running it once.

### Machine-readable output

`incan fmt --check --format json` must emit a report whose envelope is `schema_version`, `ok`, and `diagnostics`, and whose `diagnostics` entries are diagnostic records in the shape `incan check --format json` (schema 2) uses: a stable `code`, `severity`, `phase`, `origin`, the primary source span in line and column form, `message`, `notes`, `hints`, related spans, and the `incan explain` hook. Each record must additionally carry `rule`, `group`, `level`, `level_source`, and `fix`. The `phase` of a formatter record is `format`.

```json
{
  "schema_version": 1,
  "ok": false,
  "diagnostics": [
    {
      "code": "INCAN-L0042",
      "rule": "needless_bool",
      "group": "complexity",
      "level": "warn",
      "level_source": "default",
      "fix": "fmt",
      "severity": "warning",
      "phase": "format",
      "origin": "fmt",
      "span": { "file": "src/users.incn", "line": 12, "column": 5 },
      "message": "an `if` whose branches only return `true` and `false`",
      "hints": ["return the condition itself: `return age >= 18`"],
      "explain": "incan explain INCAN-L0042"
    }
  ]
}
```

`incan architect --format json` keeps RFC 105's finding record and adds the same five fields. A consumer that already reads `incan check --format json` must be able to read `incan fmt --check --format json` by ignoring the added fields.

`incan explain INCAN-L0042` must resolve every catalogue entry to its name, group, default level, origin, clippy counterpart when there is one, fact tier, fix mode, parameters, and a description with a before-and-after example.

### Exit codes

| Command | 0 | 1 |
| --- | --- | --- |
| `incan fmt` | files formatted (or nothing to do) | a file could not be parsed or written |
| `incan fmt --check` | no file would change | at least one file would change, or an operational error |
| `incan architect` | no finding at `deny` or `forbid` | at least one finding at `deny` or `forbid`, or an operational error |

A manifest refusal is an operational error for every command that reads the manifest.

### Advisories the checker emits today

The checker emits four advisories as by-products of resolution and flow analysis: an unused local binding, an unused import, a wildcard `_` arm in a `match`, and code after a `return`. They become catalogue entries with evaluator `check`, keep their text, and gain levels:

| Today | Catalogue entry | Origin | Group | Default |
| --- | --- | --- | --- | --- |
| unused variable hint | `unused_variables` | `rustc` | `style` | `warn` |
| unused import hint | `unused_imports` | `rustc` | `style` | `warn` |
| unreachable code after `return` (`INCAN-T0101`) | `unreachable_code` | `rustc` | `suspicious` | `warn` |
| wildcard match hint | `wildcard_enum_match_arm` | `clippy` | `restriction` | `allow` (clippy's) |

The last row is a behaviour change: clippy classifies a wildcard arm on an enum as a restriction lint, off by default, and this RFC follows clippy unless the unresolved question below is answered the other way. `incan check` must read `[lints.incan]` for the level of these four entries and no others; it must not evaluate any `architect` or `fmt` entry.

## Design details

### Correspondence catalogue

The tables below are the seed catalogue. Each row is one clippy lint and fills exactly one of the three Incan columns. Parenthesised markers give the fact tier (`syntax` or `semantic`) and, where the fix mode is `fmt`, say so; every other translated entry has fix mode `suggest`. Group membership and default levels follow clippy's; the `restriction` and `pedantic` rows are `allow` by default. Lints in clippy's `nursery`, `cargo`, and `deprecated` groups are out of scope. The clippy names and groups were reconciled against the clippy `master` lint list published on 2026-09-20; the catalogue records that as its reconciliation baseline.

#### Correctness (default `deny`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `absurd_extreme_comparisons` | a comparison against a bound the operand's numeric type cannot cross, such as `n < 0` for a `u8` (semantic) | | |
| `almost_swapped` | `a = b` immediately followed by `b = a` (syntax) | | |
| `approx_constant` | a float literal that approximates a `std.math` constant, such as `3.14159` for `math.pi` (syntax) | | |
| `bad_bit_mask` | `x & mask == value` that can never hold for the literal mask and value (syntax) | | |
| `char_indices_as_byte_indices` | | | `len` and indexing on `str` count Unicode scalars; there is no byte-index API to confuse them with |
| `derived_hash_with_manual_eq` | `@derive(Hash)` beside a hand-written `__eq__` (semantic) | | |
| `eq_op` | identical operands on both sides of `==`, `!=`, `<`, `-`, `//`, `and`, or `or` (syntax) | | |
| `erasing_op` | `x * 0`, `0 * x`, `0 // x`, `x & 0` (syntax) | | |
| `ifs_same_cond` | an `if`/`elif` chain that repeats a condition (syntax) | | |
| `impossible_comparisons` | `x < 5 and x > 10` and other constant double comparisons that cannot hold (syntax) | | |
| `invalid_regex` | a `std.regex` pattern literal that does not compile (syntax) | | |
| `invisible_characters` | zero-width and other invisible Unicode in source text (syntax) | | |
| `iter_skip_zero` | `.skip(0)` on an iterator (syntax) | | |
| `iterator_step_by_zero` | | `range_step_zero`: `range(a, b, 0)` raises `ValueError` at run time; the construct is the `range` builtin, not an adapter | |
| `lint_groups_priority` | | | not a lint: a group and one of its rules at the same priority is a manifest refusal |
| `min_max` | `min(max(x, hi), lo)` with the bounds reversed so the result is constant (syntax) | | |
| `modulo_one` | `x % 1` (syntax) | | |
| `never_loop` | a `for`, `while`, or `loop` whose body always leaves on the first iteration (syntax) | | |
| `not_unsafe_ptr_arg_deref` | | | no raw pointers and no `unsafe` in Incan source |
| `out_of_bounds_indexing` | a constant index into a list literal of known length (syntax) | | |
| `panicking_unwrap` | `.unwrap()` on a value the enclosing branch has already narrowed to `None` or `Err` (semantic) | | |
| `possible_missing_comma` | a bracketed literal in which a line starts with a binary operator, so two elements silently merge (syntax) | | |
| `redundant_comparisons` | `x > 5 and x > 3` (syntax) | | |
| `reversed_empty_ranges` | `range(10, 0)`, `10..0`, or a slice with constant reversed bounds (syntax) | | |
| `self_assignment` | `x = x`, `self.a = self.a` (syntax) | | |
| `serde_api_misuse` | | | derives are compiler-owned; there is no serde surface to misuse |
| `unit_cmp` | | | there is no unit value in expression position; a comparison to `None` is `Option` narrowing, which `incan check` owns |
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
| `empty_line_after_outer_attr` | | `empty_line_after_decorator`: a blank line between a decorator and the declaration it decorates (syntax; fmt) | |
| `empty_loop` | `loop:` or `while true:` whose body is only `pass` (syntax) | | |
| `manual_unwrap_or_default` | | | `Option` and `Result` have no `unwrap_or_default` on the Incan surface; `unwrap_or` with an explicit default is the spelling |
| `misrefactored_assign_op` | `a += a + b` and `a -= a - b` (syntax) | | |
| `mut_range_bound` | `for i in range(n):` whose body assigns `n`, which cannot change the iteration (semantic) | | |
| `mutable_key_type` | | | no interior mutability; a `dict` key is a value |
| `no_effect_replace` | `s.replace("a", "a")` (syntax) | | |
| `possible_missing_else` | | | indentation is the block structure; a dangling block cannot occur |
| `print_in_format_impl` | `print` inside a `__str__` implementation (syntax) | | |
| `suspicious_assignment_formatting` | `x =- 1` where `x -= 1` was likely meant; `=!` and `=*` have no Incan counterpart (syntax) | | |
| `suspicious_else_formatting` | | | `else` is positioned by indentation; there is no brace layout to get wrong |
| `suspicious_unary_op_formatting` | `a -b`, a space before a unary operator and none after it in a binary position; the `format` group's `fmt_horizontal_spacing` rewrites it (syntax; fmt) | | |
| `test_attr_in_doctest` | | | docstrings are not executed as tests |
| `unconditional_recursion` | a method that calls itself on the same receiver on every path, such as `__eq__` written as `return self == other` (semantic) | | |

#### Style (default `warn`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `assertions_on_constants` | `assert true` and `assert false` (syntax) | | |
| `assign_op_pattern` | `x = x + 1` where `x += 1` is meant; RFC 105's compound-assignment candidate is this entry (syntax) | | |
| `blocks_in_conditions` | | | conditions are expressions; there is no block expression to put in one |
| `bool_assert_comparison` | `assert x == true` where `assert x` is meant (syntax) | | |
| `collapsible_if` | a nested `if` with no `else` whose conditions can join with `and` (syntax) | | |
| `comparison_to_empty` | `s == ""` where `s.is_empty()` is meant; `xs == []` where `not xs` is meant (semantic) | | |
| `disallowed_names` | binding names from a configured list, `foo`, `bar`, and `baz` by default (syntax; parameter `names`) | | |
| `disallowed_methods` | calls to functions or methods from a configured list of paths (semantic; parameter `paths`) | | |
| `duplicate_underscore_argument` | parameters `x` and `_x` on one signature (syntax) | | |
| `enum_variant_names` | variants that repeat the enum's name as prefix or suffix, such as `enum Color: ColorRed` (syntax) | | |
| `excessive_precision` | a float literal with more digits than its type can represent (syntax) | | |
| `if_same_then_else` | `if` and `else` bodies that are identical (syntax) | | |
| `inconsistent_digit_grouping` | `1_00_000` (syntax; fmt) | | |
| `inherent_to_string` | a `def to_string(self) -> str` method where `__str__` is the Display protocol (syntax) | | |
| `len_zero` | `len(xs) == 0` where `not xs` is meant; `len(s) == 0` where `s.is_empty()` is meant (semantic) | | |
| `let_and_return` | a binding returned by the very next statement (syntax) | | |
| `let_unit_value` | | `bind_none_value`: binding the `None` result of a `-> None` call, `x = print(...)` (semantic) | |
| `main_recursion` | `main` calling `main` (syntax) | | |
| `manual_map` | `match r:` with `Ok(v) => Ok(f(v))` and `Err(e) => Err(e)` where `r.map(f)` is meant; `Result` only, since `Option` has no `map` on the surface (syntax) | | |
| `manual_ok_or` | | | `Option` has no `ok_or` on the Incan surface |
| `match_like_matches_macro` | | | no `matches!`; a unit variant compares with `==`, and a payload variant has no boolean pattern form |
| `match_ref_pats` | | | patterns never mention references; duckborrowing owns that |
| `needless_borrow` | | | no borrow syntax in Incan source |
| `needless_else` | `else: pass` (syntax; fmt) | | |
| `needless_range_loop` | `for i in range(len(xs)):` whose body only reads `xs[i]`, where `for x in xs:` or `enumerate(xs)` is meant (syntax) | | |
| `needless_return` | | | `return` is the only way to yield a value; there are no tail expressions |
| `neg_multiply` | `x * -1` where `-x` is meant (syntax) | | |
| `partialeq_to_none` | | `comparison_to_none`: `x == None` and `x != None` where `x is None` and `x is not None` are meant (syntax) | |
| `print_literal` | | | `print` takes values, not a format string; `useless_fstring` covers the f-string side |
| `print_with_newline` | `print("...\n")`: `print` already ends the line, so the output gains a blank line (syntax) | | |
| `println_empty_string` | `print("")` where `print()` is meant (syntax; fmt) | | |
| `ptr_arg` | | | parameter types are value types; there is no `&String` or `&Vec` to take |
| `question_mark` | a `match` on a `Result` that returns the `Err` arm unchanged, where `?` is meant; RFC 105's Result-match candidate is this entry (syntax) | | |
| `redundant_closure` | | | no anonymous function syntax; function values are named |
| `redundant_field_names` | | | keyword construction `User(name=name)` has no shorthand to prefer |
| `redundant_pattern_matching` | `match x:` with `None => true` and `_ => false` where `x is None` is meant; `Option` only, since `Result` has no `is_ok` on the surface (syntax) | | |
| `redundant_static_lifetimes` | | | no lifetimes |
| `result_unit_err` | `-> Result[T, None]` (syntax) | | |
| `same_item_push` | `for _ in range(n): xs.append(v)` with a loop-invariant `v`, where `list.repeat(v, n)` is meant (syntax) | | |
| `single_component_path_imports` | | | `import foo` is the ordinary module import; there is nothing to simplify |
| `single_match` | a one-arm `match` over an `Option` with `_ => pass`, where `if x is not None:` narrowing is meant; a payload enum arm has no `if let` form and is not reported (syntax) | | |
| `tabs_in_doc_comments` | | `tabs_in_docstrings`: a tab inside a docstring (syntax) | |
| `trim_split_whitespace` | `s.strip().split_whitespace()` where `s.split_whitespace()` is meant (semantic) | | |
| `unused_unit` | | | `-> None` is the required spelling of a value-less return type; there is no `()` to drop |
| `unwrap_or_default` | | | no `unwrap_or_default`; `unwrap_or([])` is the spelling |
| `upper_case_acronyms` | `class HTTPClient` where the style guide's `UpperCamelCase` gives `HttpClient` (syntax) | | |
| `while_let_on_iterator` | | | no `while let`; `for` is the iteration form |
| `wrong_self_convention` | `is_`, `to_`, and `as_` prefixes checked against `self` versus `mut self`; `into_` has no consuming receiver and is not checked (syntax) | | |

#### Complexity (default `warn`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `bool_comparison` | `x == true` and `x != false` where `x` and `not x` are meant (syntax) | | |
| `borrowed_box` | | | no `Box` and no borrow syntax |
| `bytes_count_to_len` | | | `len(s)` counts scalars and `len(s.encode())` counts bytes; they are different values, so neither replaces the other |
| `double_comparisons` | `x == y or x > y` where `x >= y` is meant (syntax) | | |
| `double_parens` | `((x))` and `f((x))` (syntax; fmt) | | |
| `excessive_nesting` | blocks nested past a threshold (syntax; parameter `threshold`) | | |
| `explicit_auto_deref` | | | no dereference operator |
| `explicit_counter_loop` | a counter initialised before a `for` and incremented once per iteration, where `enumerate` is meant (syntax) | | |
| `identity_op` | `x + 0`, `x * 1`, `x // 1`, and a bitwise or with `0` (syntax) | | |
| `int_plus_one` | `x >= y + 1` where `x > y` is meant (syntax) | | |
| `iter_count` | `xs.iter().count()` where `len(xs)` is meant (syntax) | | |
| `manual_filter_map` | | | no `filter_map` on the iterator surface |
| `manual_strip` | | | no `strip_prefix` or `strip_suffix` on the string surface |
| `manual_swap` | `tmp = xs[i]` then `xs[i] = xs[j]` then `xs[j] = tmp`, where `xs.swap(i, j)` is meant (syntax) | | |
| `match_single_binding` | a `match` with one irrefutable arm (syntax) | | |
| `needless_bool` | `if c: return true` with `else: return false`, where `return c` is meant; the condition position guarantees `c` is a `bool` (syntax; fmt) | | |
| `needless_bool_assign` | `if c: x = true` with `else: x = false`, where `x = c` is meant (syntax; fmt) | | |
| `needless_ifs` | `if c: pass` with no `else` (syntax) | | |
| `needless_lifetimes` | | | no lifetimes |
| `needless_question_mark` | `return Ok(f()?)` where `return f()` is meant, when the error types agree (semantic) | | |
| `no_effect` | an expression statement with no effect, such as a bare `x + 1` (syntax) | | |
| `precedence` | `1 << 2 + 3` and `a & b == c`, bit and arithmetic or comparison operators mixed without parentheses; the fix adds them (syntax; fmt) | | |
| `single_element_loop` | `for x in [item]:` (syntax) | | |
| `too_many_arguments` | more parameters than the threshold, 7 by default (syntax; parameter `threshold`) | | |
| `type_complexity` | a type expression nested past the threshold, such as `Dict[str, list[Result[Option[int], E]]]` (syntax; parameter `threshold`) | | |
| `unnecessary_cast` | | | no `as`; conversions are the checked `int`, `float`, `str`, and `bool` builtins, covered by `useless_conversion` |
| `unnecessary_literal_unwrap` | `Some(1).unwrap()` and `Ok(v).unwrap()` (syntax) | | |
| `unnecessary_unwrap` | `.unwrap()` on a binding the enclosing `is not None` narrowing already unwrapped (semantic) | | |
| `useless_conversion` | `int(x)` where `x` is already an `int`, `str(s)` on a `str`, `float(f)` on a `float` (semantic) | | |
| `useless_format` | | `useless_fstring`: `f"literal"` with no placeholder (syntax; fmt), and `f"{x}"` where `x` is already a `str` (semantic) | |
| `while_let_loop` | | | no `while let` |
| `zero_divided_by_zero` | `0.0 / 0.0` (syntax) | | |

#### Perf (default `warn`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `box_collection` | | | no `Box` |
| `cmp_owned` | | | duckborrowing plans conversions; source never spells an owned or borrowed comparison |
| `collapsible_str_replace` | | | `replace` takes one pattern; there is no multi-pattern form to collapse into |
| `expect_fun_call` | | | no `expect` on the surface |
| `format_in_format_args` | | `nested_fstring`: an f-string placeholder that is itself an f-string (syntax; fmt) | |
| `iter_overeager_cloned` | | | duckborrowing |
| `large_enum_variant` | | | the source language has no layout model; variant sizes are a backend fact |
| `manual_retain` | | | a filtering comprehension is the idiom; `retain` is not on the list surface |
| `map_entry` | | | no entry API; `contains_key` followed by `insert` is the spelling |
| `regex_creation_in_loops` | a `std.regex` pattern compiled from a literal inside a loop body (syntax) | | |
| `to_string_in_format_args` | | `str_in_fstring`: `f"{str(x)}"` where `f"{x}"` is meant; both go through the Display protocol (syntax; fmt) | |
| `unnecessary_to_owned` | | | duckborrowing |
| `useless_vec` | | | `list` is the only sequence type; there is no array or slice to prefer |
| `vec_init_then_push` | | `list_init_then_append`: `xs = []` followed only by `xs.append(...)` statements, where a list literal is meant (syntax; fmt) | |

#### Pedantic (default `allow`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `bool_to_int_with_if` | `1 if c else 0` where `int(c)` is meant (syntax) | | |
| `cast_possible_truncation` | | | no `as`; `int(x)` is a checked conversion with documented numeric semantics |
| `doc_markdown` | | | docstring rendering conventions are RFC 082's; until it fixes the format there is nothing to check against |
| `explicit_iter_loop` | `for x in xs.iter():` where `for x in xs:` is meant (syntax) | | |
| `float_cmp` | `==` or `!=` between floats (semantic) | | |
| `fn_params_excessive_bools` | more `bool` parameters than the threshold (syntax; parameter `max`) | | |
| `if_not_else` | `if not c:` with an `else`, where swapping the branches is meant (syntax; fmt) | | |
| `inconsistent_struct_constructor` | | `inconsistent_model_constructor`: keyword arguments in a different order from the field declaration; evaluation order changes, so the fix is a suggestion (semantic) | |
| `manual_assert` | | | no panic-family call to fold into `assert` |
| `manual_let_else` | | | no `let ... else`; `match` and `?` are the forms |
| `many_single_char_names` | more single-character bindings in one scope than the threshold (syntax; parameter `threshold`) | | |
| `map_unwrap_or` | | | no `map_or`; `r.map(f).unwrap_or(d)` is the spelling |
| `match_bool` | `match flag:` with `true =>` and `false =>` arms, where `if` is meant (syntax) | | |
| `match_same_arms` | arms with identical bodies (syntax) | | |
| `match_wildcard_for_single_variants` | a `_` arm that covers exactly one remaining variant (semantic) | | |
| `missing_errors_doc` | | `missing_errors_docstring`: a `pub def` returning `Result` whose docstring has no errors section; the section convention follows RFC 082 (syntax) | |
| `needless_continue` | `continue` as the last statement of a loop body, or an `if ... continue` that an `else` avoids (syntax) | | |
| `needless_for_each` | `.for_each(f)` where a `for` loop reads better (syntax) | | |
| `needless_pass_by_value` | | | duckborrowing: parameters are values in source, and the planner decides the Rust shape |
| `option_option` | `Option[Option[T]]` in a signature (syntax) | | |
| `range_minus_one` | `a..=(b - 1)` where `a..b` is meant (syntax; fmt) | | |
| `range_plus_one` | `a..(b + 1)` where `a..=b` is meant (syntax; fmt) | | |
| `redundant_else` | an `else` after a branch that always returns, breaks, or continues (syntax; fmt) | | |
| `semicolon_if_nothing_returned` | | | no semicolons |
| `similar_names` | bindings in one scope that differ by one character (syntax) | | |
| `struct_excessive_bools` | | `model_excessive_bools`: more `bool` fields on a `model` or `class` than the threshold (syntax; parameter `max`) | |
| `struct_field_names` | | `model_field_names`: fields that repeat the type's name as prefix or suffix (syntax) | |
| `too_many_lines` | a function body longer than the threshold (syntax; parameter `threshold`) | | |
| `trivially_copy_pass_by_ref` | | | duckborrowing |
| `unicode_not_nfc` | a string literal not in NFC (syntax) | | |
| `uninlined_format_args` | | | f-strings inline their arguments by construction |
| `unnecessary_wraps` | a non-`pub` function that only ever returns `Ok(...)` (semantic) | | |
| `unreadable_literal` | `1000000` where `1_000_000` is meant (syntax; fmt) | | |
| `unused_async` | `async def` with no `await` (syntax) | | |
| `unused_self` | a method that never reads `self`, where `@staticmethod` or a free function is meant (semantic) | | |
| `used_underscore_binding` | reading a binding named with a leading underscore (semantic) | | |
| `wildcard_imports` | `from module import *`; the parser already refuses it for user modules and a stdlib prelude is exempt as in clippy, so the rule reports only under `warn_on_all_wildcard_imports` (syntax; parameter) | | |

#### Restriction (default `allow`)

| clippy lint | translates | translates renamed | does not translate |
| --- | --- | --- | --- |
| `allow_attributes_without_reason` | | `allow_without_reason`: `@allow("...")` with no `reason` (syntax) | |
| `arbitrary_source_item_ordering` | declarations out of a configured order (syntax; parameter `order`) | | |
| `as_conversions` | | | no `as` |
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
| `panic`, `todo`, `unimplemented`, `unreachable` | | `ellipsis_body`: a function or method whose body is only `...` (syntax) | |
| `pattern_type_mismatch` | | | duckborrowing |
| `print_stdout` | any `print` call (syntax) | | |
| `question_mark_used` | any use of `?` (syntax) | | |
| `redundant_type_annotations` | `x: int = 1` where the literal already fixes the type (semantic) | | |
| `renamed_function_params` | a trait method implementation whose parameter names differ from the trait's, which changes keyword call sites (semantic) | | |
| `single_call_fn` | a function called from exactly one place (semantic); RFC 105's `maintainability.single_use_trivial_helper` is the evidence-backed neighbour and stays a separate entry | | |
| `string_slice` | | | string indexing and slicing count scalars and cannot split a code point |
| `tests_outside_test_module` | | | tests are `test_*` functions discovered by `std.testing`; there is no test module |
| `undocumented_unsafe_blocks` | | | no `unsafe` |
| `unwrap_used` | `.unwrap()` on an `Option` or `Result` (syntax) | | |
| `wildcard_enum_match_arm` | a `_` arm in a `match` over an enum; the advisory `incan check` emits today (semantic; evaluator `check`) | | |

### Entries without a clippy counterpart

Rules clippy has no notion of: rustc lints that translate, rules for Python-shaped constructs, and rules for the surface Incan has where Rust has ownership syntax:

| Rule | Group | Origin | Facts | Fix | Reports |
| --- | --- | --- | --- | --- | --- |
| `fstring_over_concat` | `style` | `incan` | semantic | suggest | a `+` chain whose operands are strings and `str(...)` calls, where one f-string is meant |
| `comprehension_over_loop` | `style` | `incan` | syntax | suggest | `out = []` followed by a `for` whose body only appends to `out`, where a list comprehension is meant; RFC 105's append-only builder candidate is this entry |
| `unnecessary_comprehension` | `style` | `incan` | syntax | suggest | `[x for x in xs]`, where the source list itself, or `xs.clone()` when a copy is meant, is the spelling |
| `generator_over_list` | `perf` | `incan` | syntax | suggest | a list comprehension consumed once by `sum`, `min`, `max`, `any`, `all`, or a `for`, where a generator expression is meant |
| `needless_bare_return` | `style` | `incan` | syntax | fmt | a bare `return` as the last statement of a `-> None` body |
| `redundant_pass` | `style` | `incan` | syntax | fmt | `pass` in a block that has other statements |
| `while_true` | `style` | `rustc` | syntax | fmt | `while true:` where `loop:` is meant |
| `unused_mut` | `style` | `rustc` | semantic | suggest | a `mut` binding, or a `mut self` receiver, that is never mutated |
| `unsorted_imports` | `pedantic` | `incan` | syntax | fmt | import statements out of the canonical order (`std` modules, then dependencies, then local modules, alphabetical within each block); the order needs a style-guide ruling before this entry ships |
| `unused_variables` | `style` | `rustc` | semantic | suggest | an unused local binding; evaluator `check` |
| `unused_imports` | `style` | `rustc` | semantic | suggest | an unused import; evaluator `check` |
| `unreachable_code` | `suspicious` | `rustc` | semantic | suggest | statements after a `return` in the same block; evaluator `check`; keeps `INCAN-T0101` |

### Formatter entries (the `format` group)

Every normalization the formatter performs is a catalogue entry with fact tier `syntax`, evaluator `fmt`, and fix mode `fmt`. They are never configurable and never suppressible; their names give `--check` output and #159's regression tests an identity per rule:

| Rule | Source | Normalizes |
| --- | --- | --- |
| `fmt_indentation` | style guide | four spaces per level; tabs and inconsistent indentation |
| `fmt_blank_lines` | RFC 053 | the three buckets: exactly two, exactly one, at most one blank line at each transition; never more than two; at most one inside a docstring |
| `fmt_trailing_newline` | RFC 053 | exactly one line terminator at end of file |
| `fmt_comment_placement` | RFC 053 | same-scope, structure-aware attachment of stand-alone comments |
| `fmt_docstring_layout` | style guide | single-line docstrings stay on one line; multi-line docstrings put the quotes on their own lines |
| `fmt_horizontal_spacing` | style guide | spaces around binary operators, after commas, and after annotation colons; none around `=` in keyword arguments; none before `(` |
| `fmt_quote_style` | style guide | double quotes where the decoded value is unchanged |
| `fmt_trailing_comma` | style guide | trailing commas in multi-line constructs |
| `fmt_line_length` | style guide | the 120-character target: overflowing calls, constructors, `with (...)` lists, logical chains, and fluent chains are re-wrapped; a line no rewrite can shorten is not a finding |
| `fmt_match_arm_layout` | style guide | short single-statement arms inline; multi-statement arms as a block body |

Formatter invariants apply to the group as a whole and to every `fmt`-fixable entry: idempotency, parse preservation, and opacity of string literal bodies (RFC 053's string-and-comment safety rule). A file that does not parse is not formatted.

### Boundary with RFC 105

RFC 105 defines the engine and the finding contract; this RFC defines the rule set and the configuration that engine evaluates. Concretely:

- RFC 105's categories `arch`, `safety`, `idiom`, `maintainability`, `risk`, and `experimental` are groups of this catalogue. Their default levels are RFC 105's to set; `experimental` is never in `all`, and RFC 105's rule that it may not fail CI by default holds because its level is never `deny` by default.
- An RFC 105 candidate that coincides with a clippy lint takes the clippy name and the clippy group: the compound-assignment candidate is `assign_op_pattern`, the Result-match candidate is `question_mark`, the append-only builder is `comprehension_over_loop`. RFC 105's own names remain for evidence-backed, project-scope rules that a clippy lint does not express: `safety.fail_fast_boundary_call` is not `unwrap_used` (it reasons about reachability from a public boundary), and `maintainability.single_use_trivial_helper` is not `single_call_fn` (it reasons about triviality and domain meaning). Both members of each pair are catalogue entries.
- Priority, confidence, evidence, suggestions, and risks are RFC 105's finding fields. Level, group, and fix mode are this RFC's. A finding at `deny` fails the command regardless of its priority; a `P1` finding at `warn` does not.
- `@allow(...)` answers RFC 105's open suppression question for catalogue entries. Baselines remain RFC 105's.

### Interaction with existing features

- **async/await**: `unused_async` translates. The lock-holding lints do not, because Incan's `Mutex` and `RwLock` are the async runtime's own locks.
- **Traits and derives**: the dunder protocol is what makes `derived_hash_with_manual_eq`, `inherent_to_string`, `print_in_format_impl`, `unconditional_recursion`, and `renamed_function_params` translate: `__eq__`, `__str__`, `__hash__`, and `__lt__` are the manual implementations clippy's lints reason about.
- **Imports and modules**: `wildcard_imports` translates with clippy's prelude exemption; `unsorted_imports` and `unused_imports` are Incan-side. `pub from module import Item` re-exports are ordinary declarations to every rule.
- **Result, Option, and `?`**: `question_mark`, `manual_map`, `unwrap_used`, `panicking_unwrap`, `unnecessary_unwrap`, and `redundant_pattern_matching` translate on the methods the surface has (`map`, `map_err`, `and_then`, `or_else`, `inspect`, `inspect_err`, `unwrap`, `unwrap_or`) and on `is None` narrowing. Methods the surface lacks (`map_or`, `ok_or`, `expect`, `unwrap_or_default`, `is_ok`) are recorded as untranslatable; when a later RFC adds such a method, the clippy entry moves from "does not translate" to "translates" without a rename.
- **Rust interop**: `rust::` imports and `@rust.*` decorators are the Rust side. `[rust.lints]` and `@rust.allow(...)` own them; `@allow(...)` rejects a Rust lint name and `@rust.allow(...)` rejects an Incan rule name.
- **Expression vocab blocks**: a brace-form vocab block is opaque to every rule except the `format` group's preservation of its surface. A vocab may register its own catalogue entries under a later RFC.
- **Conditional compilation**: `semantic`-tier rules evaluate the checked configuration; `syntax`-tier rules and the `format` group see every branch.

### Compatibility and migration

- A project with no `[lints.incan]` gets the catalogue defaults. Nothing needs to be written to keep today's behaviour.
- Formatter output on already-formatted code does not change until an `fmt`-fixable entry lands. Each such entry changes formatter output for code that matches it; each is listed in the release notes and projects see it as a one-time reformat.
- The four checker advisories keep their text and their positions in output. Their levels become configurable. The wildcard-arm hint follows clippy's `allow` default unless the unresolved question below decides otherwise.
- `[rust.lints]` and `@rust.allow(...)` are untouched.
- `incan.toml` is not a manifest after RFC 117; there is no legacy lint configuration to migrate.

## Alternatives considered

### A separate `incan lint` command

Ruff ships `ruff check` beside `ruff format`. An `incan lint` would be a third analyser beside `incan check` and `incan architect`, consuming the same facts as RFC 105's engine and reporting in a third shape. Rejected: RFC 105's engine already exists for exactly this evaluation, and `incan fmt` already runs in every CI pipeline, so the fixable subset reaches users through a command they run today.

### Rule codes as the primary identity

Ruff identifies rules by code (`E501`) with a name as an alias. Rejected as primary: the name is what a Rust developer knows, and a code alone says nothing about correspondence. Codes exist (`INCAN-L`) because `incan explain` and tooling need a stable, never-renamed handle.

### A facet-shaped `[incan.lints]` table

RFC 117 namespaces facet tables as `[incan.source]` and `[rust.source]`, and #1698 puts Rust lint policy under `[rust.lints]`. By that symmetry, Incan lint policy would be `[incan.lints]`. This RFC chooses `[lints.incan]` because Cargo's `[lints.rust]` and `[lints.clippy]` is the shape a Rust developer recognises and the level and priority semantics are Cargo's. The resulting asymmetry with `[rust.lints]` is an unresolved question rather than a settled choice.

### A side file for rule parameters

Clippy keeps thresholds and lists in `clippy.toml`. Rejected: #1698 settles the Rust side on one manifest and no `clippy.toml`, and the Incan side follows by putting parameters on the rule's own entry.

### Comment directives as the primary suppression

Ruff uses `# noqa: CODE`; pylint uses `# pylint: disable=`. Rejected as primary: declarations are Incan's metadata surface (RFC 036 decorators, RFC 057 `@rust.allow`, RFC 096 metadata blocks), and a decorator is visible to the checker, the codegraph, and reflection in a way a comment is not. Statement-scope suppression is left open.

### Approximating ownership lints

`needless_borrow` could be approximated by "a `clone()` the planner would have elided", `needless_pass_by_value` by "a parameter never mutated". Rejected: every such approximation puts borrow vocabulary into user-facing text, which the language deliberately keeps out, and reports a decision the compiler makes rather than one the author made.

### Formatter fixes for semantic rules

The formatter could typecheck before formatting and then fix `comparison_to_empty` or `useless_conversion`. Rejected: the formatter must work on any file that parses, must stay fast, and must not grow a second semantic analyser; the fixable set is by construction the type-independent set.

### Configurable levels for formatter rules

`fmt_blank_lines = "allow"` would let a project keep three blank lines. Rejected: RFC 053 and the style guide are non-configurable by design, and a formatter whose output depends on a level table is no longer a canonical formatter.

## Prior art

- **clippy**: lint groups and default levels, `renamed_and_removed_lints`, `lint_groups_priority`, `blanket_clippy_restriction_lints`, suggestion applicability. The correspondence tables are drawn against clippy's published lint list.
- **rustc**: the four lint levels, `forbid` being unrelaxable, `allow(..., reason = "...")`, and the `unused` family this catalogue adopts as `rustc`-origin entries.
- **Cargo's `[lints]` table**: the `"level"` and `{ level, priority }` entry forms and the priority ordering, adopted verbatim so `[lints.incan]` and `[rust.lints]` read the same.
- **Ruff**: one tool for formatting and lint rules, fix safety classes, rule code plus rule name, per-file configuration in the project manifest, and isort-style import ordering.
- **Black**: the canonical, non-configurable formatter that this RFC keeps the `format` group faithful to.
- **The mypy, pylint, Ruff triad**: the analogy behind "three tools, three jobs": types, design advice, and style, respectively `incan check`, `incan architect`, and `incan fmt`.

## Drawbacks

- The catalogue drifts from clippy unless someone reconciles it: clippy renames, deprecates, and regroups lints between releases. The pinned reconciliation baseline and the conformance check make drift visible, not impossible.
- Two identities per rule (name and code) cost documentation and a lookup table, in exchange for names people know and codes that never move.
- `incan check` reads `[lints.incan]` for four entries, which couples the checker to the manifest for the first time.
- Every `fmt`-fixable entry added later changes formatter output, and a project that pins nothing sees a reformat on upgrade.
- `syntax`-tier heuristics can over-report where types would have said otherwise; the fact tier is declared per entry so the trade is explicit, and the `semantic` tier is available when the heuristic is not good enough.
- The seed catalogue is large. The size is the point, since a partial correspondence table would leave a Rust developer guessing, but it is a review burden.

## Implementation architecture

This section is non-normative. It describes a recommended shape, not a task list.

- **The catalogue is a registry.** Like the language's other registries that generate reference tables, the catalogue is a compiler-owned table of entries (name, code, group, default level, origin, clippy counterpart, fact tier, evaluator, fix mode, parameters, description, example) from which the docs-site reference page and `incan explain` are generated. A conformance test compares every `clippy` and `clippy-renamed` entry with the pinned clippy lint list.
- **One rule implementation per entry, one engine.** `syntax`-tier rules are functions over the trivia-aware syntax tree; `semantic`-tier rules are functions over RFC 105's typed fact views. `incan architect` runs both tiers; `incan fmt` runs the `syntax` tier for entries with fix mode `fmt` and applies their rewrites. The formatter holds no rule logic of its own beyond the `format` group.
- **Relationship to #159.** Rules are defined against the trivia-aware tree that #159 introduces, so comment and docstring trivia are inputs to a rule rather than repair work after it. The current AST-based formatter can host the `format` group and those `fmt`-fixable entries whose rewrite does not depend on trivia (`needless_else`, `double_parens`, `useless_fstring`, `nested_fstring`, `list_init_then_append`, `range_plus_one`, `range_minus_one`, `unreadable_literal`, `inconsistent_digit_grouping`, `while_true`, `redundant_pass`, `needless_bare_return`); entries that move comments (`redundant_else`, `if_not_else`, `empty_line_after_decorator`, `unsorted_imports`) wait for #159.
- **Phasing.** A first phase lands the registry, `[lints.incan]` and `[workspace.lints.incan]` parsing with the refusals, `@allow(...)`, `incan explain` for `INCAN-L` codes, and the four checker advisories as catalogue entries. A second phase lands the `correctness`, `suspicious`, and `style` entries in the engine's `syntax` tier, `incan fmt --check` output by rule, and the first `fmt`-fixable entries. A third phase lands `complexity`, `perf`, `pedantic`, and `restriction`, the `semantic` tier, parameters, workspace precedence, and editor surfacing. Reconciliation against clippy becomes a maintenance rule, run when the toolchain's pinned Rust release changes.

## Layers affected

- **Parser / AST**: `@allow(...)` as a compiler-owned decorator on the declarations listed above and in the module-decorator position; no grammar change. The trivia-aware tree is #159's, and every `syntax`-tier rule is defined against it.
- **Typechecker**: no new checks. The four advisories it emits become catalogue entries with levels read from the manifest. `@allow(...)` validation (unknown name, `forbid` conflict, Rust lint name, `fmt_*` name) is reported as an ordinary diagnostic.
- **Lowering / emission**: unaffected.
- **Stdlib / runtime**: unaffected.
- **Manifest / Oven (RFC 117)**: `[lints.incan]` and `[workspace.lints.incan]` parsing, precedence, and the refusals; the manifest ownership map gains a `[lints.incan]` line owned by this RFC.
- **CLI / tooling**: `incan architect` resolves levels from the manifest and carries the five catalogue fields in its findings; `incan fmt --check` reports by rule and gains `--format json`; `incan explain` resolves `INCAN-L` codes; `incan architect --list-rules` lists the catalogue with effective levels.
- **LSP / formatter**: the formatter hosts the `format` group and the `fmt`-fixable entries; the language server surfaces catalogue findings through the engine, which is an unresolved question below.
- **Documentation**: a generated catalogue reference page; the formatting how-to and the style guide gain their rule names; the CLI reference documents `[lints.incan]`, `@allow(...)`, the `--check` output, and the exit codes.

## Inspectability and tooling surface

- **Artifact or metadata:** the catalogue registry and its generated reference page; the effective level table per project, which `incan architect --list-rules --format json` prints with each rule's level and the manifest layer that set it.
- **Inspection command:** `incan architect --format json` for findings, `incan fmt --check --format json` for the fixable subset, `incan explain INCAN-L####` for any entry, `incan architect --list-rules` for the effective configuration.
- **Diagnostics:** manifest refusals in RFC 117's diagnostic family, naming the entry and, for a clippy name, the recorded rename or reason; `@allow(...)` diagnostics at the decorator; a per-file note when `semantic`-tier rules were skipped because the file does not typecheck.
- **Provenance:** every finding carries its source span, its owning declaration, its rule name and code, its effective level, the layer that set the level (`default`, `workspace`, `member`, or the `@allow` decorator's span), and, for a suppressed rule, the `reason`.
- **Not implicit:** no rule runs that is not a catalogue entry; no level comes from anywhere but the defaults, the two manifest tables, and `@allow(...)`; no command rewrites source except `incan fmt`, and it rewrites only `format`-group and `fmt`-fixable entries.

## Acceptance criteria

This RFC is done when:

- the catalogue registry holds every entry in the tables above with name, code, group, default level, origin, fact tier, evaluator, fix mode, and parameters, and a conformance test checks every `clippy` and `clippy-renamed` entry against the pinned clippy lint list;
- `[lints.incan]` and `[workspace.lints.incan]` parse with every refusal listed above, and precedence is covered by tests for each layer, for `forbid` stickiness, and for the same-priority conflict;
- `@allow(...)` is accepted on the listed declarations and in the module-decorator position, rejected elsewhere, and diagnosed under `forbid`, for a Rust lint name, and for an `fmt_*` name;
- `incan architect --format json` findings carry rule, code, group, effective level, and level source, and checker advisories appear once;
- `incan fmt --check` reports by rule name and fails on the `format` group and every `fmt`-fixable entry at a level other than `allow`; `incan fmt` applies them, and the idempotency property test covers each fixable entry;
- `incan explain` resolves every `INCAN-L` code, and `incan architect --list-rules` prints the effective table;
- the generated catalogue reference page exists, and the formatting how-to, the style guide, and the CLI reference name the rules and the table.

## Unresolved questions

- Which entries are `fmt`-fixable and which stay `suggest`? This draft marks the type-independent rewrites (`needless_bool`, `needless_bool_assign`, `needless_else`, `double_parens`, `precedence`, `useless_fstring` for the placeholder-free form, `nested_fstring`, `str_in_fstring`, `list_init_then_append`, `if_not_else`, `redundant_else`, `range_plus_one`, `range_minus_one`, `unreadable_literal`, `inconsistent_digit_grouping`, `empty_line_after_decorator`, `needless_bare_return`, `redundant_pass`, `while_true`, `unsorted_imports`) and leaves everything type-dependent as a suggestion. The marker is the entry's fix mode; confirm the set or move entries across.
- One invocation or two: should `incan fmt` also print the `syntax`-tier findings it does not fix, or is `incan architect` the only reporting surface? This draft says `incan fmt` reports only what it fixes.
- `[lints.incan]` beside `[rust.lints]` names the same kind of table with two roots. Settle one spelling for both: `[lints.incan]` with `[lints.rust]`, or `[incan.lints]` with `[rust.lints]`.
- Should the wildcard-arm advisory keep firing by default, diverging from clippy's `allow` for `wildcard_enum_match_arm`, or follow clippy?
- Statement-scope suppression: is a trailing `# allow: rule` directive wanted, or is declaration scope enough?
- Should `fmt_line_length`, `fmt_indentation`, `fmt_quote_style`, and `fmt_trailing_comma` accept project values, as the library formatter already can, or does the style guide stay non-configurable?
- Default levels for RFC 105's groups: RFC 105 leaves its default profile open. Are they decided there, or here once the catalogue lands?
- Editor surfacing: should `incan check` run the `syntax` tier so style findings reach the editor through the existing diagnostics channel, or does the language server call the engine?
- `unsorted_imports` needs a style-guide ruling on canonical import order before it can be `fmt`-fixable.
- Reconciliation cadence: which clippy release the catalogue pins, and whether reconciliation follows the toolchain's pinned Rust release or a fixed schedule.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
