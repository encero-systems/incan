# Anti-pattern inventory

Every anti-pattern the repository's own rules name, with the rule's source, one search that finds its violations, and the count of what it finds. The sources hold the rules and their reasons; this file holds only the searches and the counts, so a rule is changed at its source and never here.

## How to read and rerun it

- Run every search from the repository root with ripgrep 14. A search whose count needs structure that a regular expression cannot see (fenced code, docstrings, scopes) names an awk program from [Structural searches](#structural-searches).
- Incan scope is every `*.incn` file outside a `fixtures` directory, and Rust scope is every `*.rs` file outside one. Contract and Loaf fixtures keep the shape their cases assert, so they are not counted.
- **Before** is the count at `dd3ee72d8` (the `queue/square-1` head this inventory was built from). **Now** is the count at the commit that last changed this file. A clearing commit updates its own rule's **Now**. This branch merged `queue/square-1` at `d2920bffc`, whose 61 commits added hits to some rules (IN1 rose from 2589 to 2628, for example); **Now** counts them.
- **Real** separates the hits that violate the rule from those the search over-approximates (comments, user-facing advice text, generated pages), as the notes under each rule list them. A dash marks a review set: a search whose hits each need a judgment that has not been made for all of them.
- Some hits sit in paths other lanes were rewriting when this was built: `workspaces/oven/src/compiler_body_publication.incn`, `loaves/compiler/internal_models/src/compiled_body_frames.incn`, `loaves/platform/src/store/index.incn` and `loaves/toolchain/incan-lsp/`. They are counted, and listed as lane hits, but not cleared here.

## Summary

| ID | Rule | Before | Real | Now |
| --- | --- | ---: | ---: | ---: |
| RS1 | `.unwrap()` / `.expect("…")` in Rust | 6 | 5 | 1 |
| RS2 | `.expect()` spelled `unwrap_or_else(\|…\| panic!(…))` | 11 | 11 | 9 |
| RS3 | `panic!` control flow (tests and production) | 284 | 284 | 281 |
| RS4 | Wildcard `use …::*;` | 644 | 644 | 647 |
| RS5 | `&String`, `&Vec<T>`, `&Box<T>` parameters | 6 | 2 | 6 |
| RS6 | Numeric `as` casts | 290 | 290 | 290 |
| RS7 | `pub fn … -> Result<_, String>` | 32 | 32 | 32 |
| RS8 | `Rc<RefCell<_>>` | 0 | 0 | 0 |
| RS9 | `macro_rules!` outside `incan_derive` | 22 | 22 | 22 |
| RS10 | `collect::<Vec<_>>()` only to iterate again | 0 | 0 | 0 |
| RS11 | Builtin trait and derive names as string literals in compiler layers | 13 | 4 | 9 |
| RS12 | `Err(_) =>` arms that drop the error (Rust) | 47 | — | 47 |
| RS13 | `map_err(\|e\| e.to_string())` that drops the operation | 106 | — | 112 |
| IN1 | Inline `.map_err((error) => error.message())` plumbing | 2589 | 2589 | 2628 |
| IN2 | String-literal `match` arms where an `Enum(str)` belongs | 172 | — | 172 |
| IN3 | Repeated `kind == "a" or kind == "b"` chains | 46 | 46 | 46 |
| IN4 | Imports after the first declaration | 1 | 1 | 0 |
| IN5 | Incan lines over 120 characters | 1252 | 1252 | 1263 |
| IN6 | `Err(e) => return Err(e)` where `?` reads directly | 32 | — | 33 |
| IN7 | `str("literal")` wrappers | 8 | 8 | 0 |
| IN8 | `== true` / `== false` comparisons | 0 | 0 | 0 |
| IN9 | Ceremony helpers `empty_*` / `clone_*` | 26 | — | 26 |
| IN10 | `Err(_) =>` arms that drop the error (Incan) | 407 | — | 416 |
| IN11 | Stale Python script names in Incan programs | 13 | 13 | 0 |
| IN12 | Declarations without a docstring | 529 | 529 | 506 |
| IN13 | Helpers nested inside function bodies | 0 | 0 | 0 |
| IN14 | Positional magic numbers (`parts[3]`) | 257 | — | 259 |
| IN15 | The toolchain version spelled again where its constant exists | 11 | 7 | 5 |
| IN16 | Failure messages that name no subject | 341 | — | 335 |
| MD1 | Hard-wrapped Markdown prose | 34 | 30 | 4 |
| MD2 | RFC references in user-facing docs | 12 | 12 | 0 |
| MD3 | Workstation paths in committed text | 8 | 0 | 8 |
| PR1 | Production Python outside the allowed uses | 0 | 0 | 0 |
| PR2 | Oven tests missing from `workspaces/contracts/oven-tests.txt` | 0 | 0 | 0 |

IN15 counts candidates. A message that names its subject in the surrounding error type passes IN16.

## Rust rules

### RS1 `.unwrap()` / `.expect("…")`

- Source: `AGENTS.md`, "CRITICAL — NO `.unwrap()` / `.expect()` ANYWHERE"; `.agents/skills/review/SKILL.md`, Checklist 1.
- Search: `rg -n '^\s*[^/\s].*(\.unwrap\(\)|\.expect\(")' -g '*.rs' -g '!**/fixtures/**'`
- Real: the five `.expect("…")` calls in the tests of `loaves/compiler/incan_frontend/src/rust_abi_cache/strings.rs` (lines 707–724). The sixth hit is user-facing advice text in `loaves/kernel/incan_syntax/src/diagnostics/catalog/errors/types/mismatches_and_functions.rs:147` about Incan's own `.unwrap()`. The parser's `self.expect(&TokenKind…)` is its own `Result`-returning method and is not matched.

### RS2 `.expect()` spelled as `unwrap_or_else(|…| panic!(…))`

- Source: `AGENTS.md`, "CRITICAL — NO `.unwrap()` / `.expect()`"; `readable-maintainable-rust.md`, "Panics on recoverable paths".
- Search: `rg -n -P 'unwrap_or_else\(\|[^|]*\|\s*panic!' -g '*.rs' -g '!**/fixtures/**'`
- Hits: `incan_lang` (3), `incan_syntax/src/lexer/mod.rs` (2), `incan_frontend/src/typechecker/diagnostic_values.rs` (2), `loaves/stdlib/web/rust/src/lib.rs` (4).
- Left: the nine production sites. Each states a total function whose failure is a programming error (the `incan_lang` registry lookups and the checked-diagnostic capture of `diagnostic_values.rs`) or is a public entry point (`App.run` in `std.web`, whose TODO asks for a `Result`). Clearing them changes their signatures and every caller, so they wait for a toolchain that can build and test those callers.

### RS3 `panic!` control flow

- Source: `.agents/skills/review/SKILL.md`, closing checklist ("panic helpers, panic-style control flow"); `AGENTS.md`, "Error handling in tests".
- Search: `rg -n 'panic!\(' -g '*.rs' -g '!**/fixtures/**'`
- Split: 268 in test context (a file under a `tests` directory, a `tests.rs` file, or a line after `#[cfg(test)]`), 16 in production. Four of the production hits are `loaves/stdlib/primitives/rust/src/errors.rs`, which raises Incan's runtime errors by design.

### RS4 Wildcard imports

- Source: `AGENTS.md`, "Rust Anti-Patterns to Avoid"; `readable-maintainable-rust.md`, quick reference.
- Search: `rg -n '^\s*(pub(\([^)]*\))? )?use [^;]*::\*;' -g '*.rs' -g '!**/fixtures/**'`
- Split: 442 are `use super::*;`, mostly in split submodules and test modules. Lane hits: 37 in `loaves/toolchain/incan-lsp/`.

### RS5 Borrowed owning containers as parameters

- Source: `AGENTS.md`, "Rust Anti-Patterns to Avoid".
- Search: `rg -n -P '\w+: &(String\b|Vec<|Box<)' -g '*.rs' -g '!**/fixtures/**'`
- `&mut String` and `&mut Vec<T>` are not matched: a callee that pushes needs them. Real: `decode_scalar` and `decode_text` in `loaves/stdlib/primitives/rust/src/crumb.rs`, which Incan calls across its Rust interop boundary with its byte list, so a slice parameter changes that boundary. Not real: three closure parameters whose type is their iterator's item, and the `type_param` of `open_type_params.rs`, from which `std::slice::from_ref` builds the `&[String]` its callee takes.

### RS6 Numeric `as` casts

- Source: `AGENTS.md`, "Rust Anti-Patterns to Avoid".
- Search: `rg -n -P '^\s*[^/\s].*\bas (u8|u16|u32|u64|u128|usize|i8|i16|i32|i64|i128|isize|f32|f64)\b' -g '*.rs' -g '!**/fixtures/**'`
- Lane hits: 18.

### RS7 `Result<_, String>` in public functions

- Source: `AGENTS.md`, "Rust Anti-Patterns to Avoid"; `.agents/skills/review/SKILL.md`, Checklist 4.
- Search: `rg -n -P '\bpub fn [^{;]*-> Result<.*, ?String>' -g '*.rs' -g '!**/fixtures/**'`

### RS8 `Rc<RefCell<_>>`

- Source: `AGENTS.md`, "Rust Anti-Patterns to Avoid".
- Search: `rg -n 'Rc<RefCell<' -g '*.rs' -g '!**/fixtures/**'`

### RS9 Declarative macros outside the derive crate

- Source: `CONTRIBUTING.md`, "Macro Discipline"; `readable-maintainable-rust.md`, "Macro Discipline".
- Search: `rg -n 'macro_rules!' -g '*.rs' -g '!**/fixtures/**' -g '!loaves/stdlib/derive/incan_derive/**'`

### RS10 Collecting only to iterate again

- Source: `AGENTS.md`, "Rust Anti-Patterns to Avoid".
- Search: `rg -n -P 'collect::<Vec<_>>\(\)\.(iter|into_iter)\(' -g '*.rs' -g '!**/fixtures/**'`

### RS11 Builtin trait and derive names as literals in compiler layers

- Source: `.agents/learnings.md`, "RFC 041 (first-class Rust interop) implementation notes".
- Search: `rg -n '"(Clone|Copy|Debug|Default|Eq|PartialEq|Ord|PartialOrd|Hash|Display)"' -g '*.rs' loaves/compiler loaves/toolchain`
- Real: the four `"Copy"` derive names, pushed in `loaves/compiler/incan_frontend/src/body_ir/rust_values.rs` and compared in `loaves/toolchain/incan-rustc-driver/src/adapter/library_verification.rs` and `loaves/toolchain/incan-rustc-driver/src/validation/nominal_layouts.rs`. Not real: the eight named constants of `ast_build.rs` and `library_exports.rs`, which spell rustc's own standard-library symbols in one place each, and a rustdoc example.

### RS12 `Err(_)` arms that drop the error

- Source: `readable-maintainable-rust.md`, "Clear error handling" ("don't lose context when mapping"); `.agents/skills/review-incan-source-quality/SKILL.md`, "safety over silence".
- Search: `rg -n 'Err\(_\)\s*=>' -g '*.rs' -g '!**/fixtures/**'`
- Each hit either swallows a failure or replaces its cause. `unwrap_or` and `unwrap_or_else` are not counted: `.agents/learnings.md` allows them.

### RS13 Errors stringified without their operation

- Source: `readable-maintainable-rust.md`, "Clear error handling" and "Error taxonomy".
- Search: `rg -n -P 'map_err\(\|\w*\|\s*\w+\.to_string\(\)\)' -g '*.rs' -g '!**/fixtures/**'`
- `std::io::Error` text carries no path, so each hit loses the file and the operation. Lane hits: 5. Reviewed: the ten in `loaves/stdlib/primitives/rust/src/file.rs` pass, because `std.fs` wraps each in an `IoError` that names the operation and the path (`_io_error`, `_fs_error` in `loaves/stdlib/system/src/fs/path.incn`); adding the path natively would name it twice. 53 hits are in tests.

## Incan rules

### IN1 Inline error plumbing

- Source: `readable-incan.md`, "Helpers" ("Repeated plumbing is written once … not `.map_err((error) => error.message())` at every call"), which `AGENTS.md` makes binding for all Incan code.
- Search: `rg -n -P 'map_err\(\((\w+)\) => \1\.message\(\)\)' -g '*.incn' -g '!**/fixtures/**'`
- Split: 1287 in sources, 1302 in tests. Lane hits: 27. `std.fs` errors already render `"{operation} {path}: {detail}"`, so these hits repeat plumbing; they do not lose context.

### IN2 String-literal `match` arms

- Source: `readable-maintainable-rust.md`, "Stringly-typed APIs"; `.agents/skills/review-incan-source-quality/SKILL.md` ("stringly or byte-twiddling logic without named intent"); repository rule: closed sets are `Enum(str)`.
- Search: `rg -n -P '^\s+"[^"]*"(\s*\|\s*"[^"]*")*\s*=>' -g '*.incn' -g '!**/fixtures/**'`
- Lane hits: 9. A hit that parses external text into an enum at one boundary is the enum's constructor and passes on review.

### IN3 Repeated kind checks

- Source: `.agents/learnings.md`, "Name repeated kind checks".
- Search: `rg -n -P '(\w+(?:\.\w+)*) == "[^"]*" or \1 == "' -g '*.incn' -g '!**/fixtures/**'`

### IN4 Imports after the first declaration

- Source: `readable-incan.md`, "The shape of a module" ("Imports come first").
- Search: the `lateimport` findings of [`incan_structure.awk`](#incan_structureawk).
- Hit: `loaves/stdlib/primitives/src/primitives/io.incn:269`.

### IN5 Long lines

- Source: `readable-incan.md`, "Lines and names".
- Search: `rg -n '^.{121,}' -g '*.incn' -g '!**/fixtures/**'`
- Lane hits: 30.

### IN6 Rewrapping an error that `?` would propagate

- Source: `.agents/skills/review-incan-source-quality/SKILL.md` ("verbose `match` blocks that just rewrap a `Result` where `?` would read naturally"); `.agents/learnings.md`, "Async result passthrough should stay direct".
- Search: `rg -n -P 'Err\((\w+)\)\s*=>\s*(return\s+)?Err\(\1\)\s*$' -g '*.incn' -g '!**/fixtures/**'`
- Reviewed: most hits are three-way `Ok(Some(…))`, `Ok(None)`, `Err` matches over an iterator's `__next__()`, where `?` means restructuring the match as `match x?:`, and three are the bodies of `Result.map`, `and_then` and `inspect` in `loaves/stdlib/primitives/src/result.incn`, where the match is the implementation. All are standard-library source that no Linux toolchain could parse when this was built.

### IN7 String literal wrappers

- Source: `.agents/learnings.md`, "String literal wrappers are showcase noise".
- Search: `rg -n '(?:^|[^.\w])str\("' -g '*.incn' -g '!**/fixtures/**'`
- Hits: `loaves/stdlib/data/src/json.incn:642` and seven lines of `loaves/stdlib/data/src/math.incn`.

### IN8 Boolean equality

- Source: `.agents/learnings.md`, "Boolean equality checks are noise".
- Search: `rg -n -P '[!=]= (true|false)\b' -g '*.incn' -g '!**/fixtures/**'`

### IN9 Ceremony helpers

- Source: `.agents/learnings.md`, "Ceremony helpers need a job".
- Search: `rg -n 'def (empty|_?clone)_\w+\(' -g '*.incn' -g '!**/fixtures/**'`
- Each hit passes on review when it carries a type witness, a cross-stage invariant or a boundary contract; the count is the review set.

### IN10 `Err(_)` arms that drop the error

- Source: `.agents/skills/review-incan-source-quality/SKILL.md` ("safety over silence"); `readable-maintainable-rust.md`, "Clear error handling".
- Search: `rg -n 'Err\(_\)\s*=>' -g '*.incn' -g '!**/fixtures/**'`
- Lane hits: 2.

### IN11 Stale Python script names

- Source: `AGENTS.md`, "No Python in production logic" (the checks are Incan programs, so a `.py` name in their usage text is a stale fact).
- Search: `rg -n -P '\b[a-z_]+\.py\b' scripts -g '*.incn'`
- Hits: the usage text of `scripts/check_doc_paths.incn`, `scripts/check_changed_rustdocs.incn`, `scripts/check_reference_contract.incn` and `scripts/check_us_english.incn`, spelled once in `HELP` and again at the option refusal, and two retired `.py` paths in the spelling check's skip list.

### IN12 Declarations without a docstring

- Source: `.agents/skills/review-incan-source-quality/SKILL.md` ("docstrings on every authored declaration, including private/internal helpers"); `.agents/skills/review/SKILL.md`, Checklist 5.
- Search: the `nodoc` findings of [`incan_structure.awk`](#incan_structureawk).
- Split: 522 in sources, 7 in tests. By top directory at `dd3ee72d8`: 246 in `loaves/stdlib`, 209 in `examples`, 30 in `workspaces/docs-site`, 23 in `workspaces/benchmarks` (now 0), 15 in `loaves/compiler` and 1 in `workspaces/oven`.

### IN13 Helpers nested in function bodies

- Source: `readable-incan.md`, "Helpers" ("Each distinct job is its own small function").
- Search: the `nested` findings of [`incan_structure.awk`](#incan_structureawk).

### IN14 Positional magic numbers

- Source: `readable-incan.md`, "Helpers" ("A decision gets a name … a constant named after the rule, not an inline expression"); `.agents/skills/review/SKILL.md`, Checklist 3 ("magic constants").
- Search: `rg -n '\w\[([2-9]|[1-9][0-9]+)\]' -g '*.incn' -g '!**/fixtures/**'`
- Split: 217 in sources, 40 in tests. Lane hits: 14. Indexes 0 and 1 are not counted.

### IN15 One fact, one home: the toolchain version

- Source: `.agents/skills/review/SKILL.md`, Checklist 8 ("Single source of truth is preserved"); `.agents/learnings.md`, "Single-source plans prevent split brain".
- Search: `rg -n -F '0.6.0-dev.9' -g '*.incn' -g '*.rs' -g '!**/fixtures/**'`
- Homes: `TOOLCHAIN` in `workspaces/oven/src/home_paths.incn`, `TOOLCHAIN_VERSION` in `scripts/release_support.incn`, `TOOLCHAIN_NAME` in `workspaces/benchmarks/benchmark_support.incn` and `TOOLCHAIN_DIRECTORY` in `workspaces/benchmarks/dev_loop/report.incn` (the dev loop is its own program and cannot import the benchmark support module); one hit is a rustdoc example. Real at `dd3ee72d8`: the seven spellings beside a home, in `scripts/release_support.incn` (`TOOLCHAIN_DIRECTORY`), the two dev-loop programs, `workspaces/oven/src/dependency_freshness_archive.incn`, `workspaces/oven/src/test_engine_capture.incn` and twice in `workspaces/oven/src/test_source_loaf.incn`; `scripts/test_release_support.incn` added an eighth after it. Each now reads its home.

### IN16 Failure messages that name no subject

- Source: `readable-maintainable-rust.md`, "Clear error handling" ("keep error messages actionable"); repository rule: an error names its path or operation.
- Search: `rg -n 'Err\("(cannot|could not|failed to|unable to|invalid|malformed|missing|unexpected|corrupt)[^"{]*"\)' -g '*.incn' -g '!**/fixtures/**'`
- Reviewed: the seven script and benchmark failures that named an operation on a path or commit without either now name it. 280 hits are `incan_mir_lowering` invariant errors that name the malformed checked structure; whether each also needs its declaration is a judgment per site.

## Markdown and process rules

### MD1 Hard-wrapped prose

- Source: `AGENTS.md`, "Docs-site Workflow" ("Line length: no hard wrap … and other non-code markdown files"); `.agents/skills/review/SKILL.md`, Checklist 5.
- Search: [`markdown_wrap.awk`](#markdown_wrapawk) over every tracked `*.md` outside `fixtures`.
- Not real: the button rows of `workspaces/docs-site/docs/index.md` (2), the generated `workspaces/docs-site/docs/language/reference/feature_inventory.md` (1, fixed in its generator) and the generated `workspaces/benchmarks/results/results.md` (1).

### MD2 RFC references in user-facing docs

- Source: `.agents/skills/review/SKILL.md`, Checklist 5 ("User-facing docs do not lean on RFC references").
- Search: `rg -n -P '\bRFC[ -]?\d{2,3}\b|/RFCs/closed/' workspaces/docs-site/docs/language workspaces/docs-site/docs/tooling workspaces/docs-site/docs/start_here workspaces/docs-site/docs/project workspaces/docs-site/docs/comparisons -g '!**/reference/language.md' -g '!**/feature_inventory.md'`
- Links to the RFC index for contributors are not matched.

### MD3 Workstation paths

- Source: `.agents/learnings.md`, "Never expose local paths".
- Search: `git grep -n -I -E '/Users/[A-Za-z]|/home/[a-z]' -- ':!workspaces/contracts/fixtures'`
- Every hit is a placeholder (`/path/to/home`) or a synthetic test value.

### PR1 Production Python

- Source: `AGENTS.md`, "No Python in production logic".
- Search: `git ls-files '*.py'`; every file must be a docs-site hook, lexer or extraction script, a benchmark baseline, the pip shim, an agent hook or a research script.

### PR2 Unlisted Oven tests

- Source: `scripts/oven_tests.incn` (the passing ids must equal `workspaces/contracts/oven-tests.txt`).
- Search: the `test_` definitions of `workspaces/oven/src/test_*.incn` and `loaves/*/*/tests/test_*.incn`, spelled `<file>::<name>`, compared with the registry: `for f in $(git ls-files 'workspaces/oven/src/test_*.incn' 'loaves/*/*/tests/test_*.incn'); do grep -oE '^ *(pub )?(async )?def test_[A-Za-z0-9_]+' "$f" | sed -E "s/^ *(pub )?(async )?def /$(basename "$f")::/"; done | sort | diff - <(sort workspaces/contracts/oven-tests.txt)`

## Recorded for other lanes

These hits belong to T9.4 (sizes) and T8.3 (methods) and are not cleared here.

- Files over 1,300 lines: `loaves/compiler/incan_frontend/src/typechecker/body_check/records.rs` (1349), `loaves/compiler/internal_models/src/checker_records.incn` (1352), `workspaces/oven/src/compiler_body_publication.incn` (1337), `workspaces/oven/src/test_dependency_freshness.incn` (1301).
- Functions over 120 lines: 9 Incan, 58 Rust (counted from each definition to the end of its body).
- Free functions threading state: 418 Incan functions whose first parameter is `mut` (`rg -n '^(pub )?def \w+\(mut \w+:' -g '*.incn' -g '!**/fixtures/**'`), and 306 with a later `mut` parameter.

## Structural searches

Run each with `git ls-files -z '<glob>' | xargs -0 awk -f <program>`, writing the program to a scratch file first.

### incan_structure.awk

Over `*.incn` outside `fixtures`. It skips string bodies, so docstring examples are not read as code.

```awk
# Prints `<rule>\t<file>:<line>:<text>`: nested (a def inside a function body), lateimport (a top-level import after
# the first declaration) and nodoc (a def, model, class, enum, trait or newtype whose body does not open with a string).
function indent(s) { match(s, /^[ ]*/); return RLENGTH }
FNR == 1 { depth = 0; seen = 0; in_str = 0; pending = 0 }
{
  line = $0
  if (in_str) { if (gsub(/"""/, "&", line) % 2 == 1) in_str = 0; next }
  if (line ~ /^[ ]*$/ || line ~ /^[ ]*#/) next
  ind = indent(line)
  if (pending == 2) { if (line !~ /^[ ]*(r|f)?"/) print "nodoc\t" at; pending = 0 }
  if (pending == 1) {
    if (line ~ /:[ ]*(#.*)?$/) pending = 2
    if (gsub(/"""/, "&", line) % 2 == 1) in_str = 1
    next
  }
  while (depth > 0 && ind <= sind[depth]) depth--
  is_def = (line ~ /^[ ]*(pub(\([a-z]+\))? )?(async )?def [A-Za-z_]/)
  is_type = (line ~ /^[ ]*(pub(\([a-z]+\))? )?(model|class|enum|trait|newtype) [A-Za-z_]/)
  if (is_def && depth > 0 && kind[depth] == "def") print "nested\t" FILENAME ":" FNR ":" line
  if (ind == 0) {
    if (line ~ /^(from [^ ]+ import |import [^ ])/ && seen) print "lateimport\t" FILENAME ":" FNR ":" line
    if (line ~ /^(pub(\([a-z]+\))? )?(async )?(def|model|class|enum|trait|newtype|const|static|type|module) / || line ~ /^@/) seen = 1
  }
  if (is_def || is_type) {
    depth++; sind[depth] = ind; kind[depth] = (is_def ? "def" : "type")
    at = FILENAME ":" FNR ":" line
    pending = (line ~ /:[ ]*(#.*)?$/) ? 2 : 1
    next
  }
  if (line ~ /^[ ]*module [A-Za-z_]+:/) { depth++; sind[depth] = ind; kind[depth] = "module"; next }
  if (gsub(/"""/, "&", line) % 2 == 1) in_str = 1
}
```

### markdown_wrap.awk

Over `*.md` outside `fixtures`. It prints the first line of each paragraph or list item that continues on the next line, skipping front matter, fenced code, tables, headings, HTML, admonitions, snippets and link definitions.

```awk
function prose(s) {
  if (s ~ /^[ \t]*$/ || s ~ /^[ \t]/) return 0
  if (s ~ /^(#|\||>|<|!!!|\?\?\?|---|\*\*\*|___|--8<--|\[[^]]*\]:|\{|:::)/) return 0
  return 1
}
function item(s) { return s ~ /^[ ]*([-*+]|[0-9]+[.)])[ ]+[^ ]/ }
FNR == 1 { fence = 0; front = 0; prevkind = ""; reported = 0 }
FNR == 1 && /^---[ ]*$/ { front = 1; next }
front { if ($0 ~ /^---[ ]*$/) front = 0; next }
/^[ ]*(```|~~~)/ { fence = !fence; prevkind = ""; next }
fence { next }
{
  kind = ""
  if (item($0)) kind = "item"
  else if (prose($0)) kind = "prose"
  else if ((prevkind == "item" || prevkind == "cont") && $0 ~ /^[ ]{2,4}[^ *+0-9|>`<!-]/ && $0 !~ /^[ ]*([-*+]|[0-9]+[.)])[ ]/) kind = "cont"
  if ((kind == "prose" && prevkind == "prose") || (kind == "cont" && (prevkind == "item" || prevkind == "cont"))) {
    if (!reported) { print FILENAME ":" startline ":" start; reported = 1 }
  } else { reported = 0; start = $0; startline = FNR }
  if (kind == "item") { reported = 0; start = $0; startline = FNR }
  prevkind = kind
}
```
