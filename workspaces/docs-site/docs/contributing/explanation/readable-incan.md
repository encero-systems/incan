# Readable Incan

Incan code in this repository reads top-down, like a description of what it does. The compiler's own Incan, Oven, the contract runner and the standard library follow the same style. `workspaces/contracts/src/formatting.incn` is the reference example.

## The shape of a module

- The module docstring says in one sentence what the module does.
- Imports come first, then constants, then the public entry points, then the helpers they use, in the order the entry points use them.
- A public function's docstring states its contract. When the function performs a sequence of steps, the docstring numbers them.
- The body reads as those steps. A short comment opens each step, numbered like the docstring, so a reader can match the code to the contract.

## Helpers

- Each distinct job is its own small function, with a docstring that says what it returns or decides.
- A decision gets a name. A condition that encodes a rule becomes a helper or a constant named after the rule, not an inline expression.
- Repeated plumbing is written once: a `read` and `write` pair, not `.map_err((error) => error.message())` at every call.
- Functions split by step, not by line count, and each stays short enough to read at a glance.

## Lines and names

- Lines stay within 120 characters.
- Names describe the domain (`check_code`, `expected`), not the mechanism.
- Constructs use the language reference's terms: closure expressions, not lambdas.

## Example

The public entry point states its contract, and its body reads as the numbered steps:

```incan
pub def compare_format(incan: str, project: Path, header: Header, source: str) -> Result[list[str], str]:
    """
    Run a format case's four checks through `incan fmt` and return one message per broken check.

    1. `--check` on the original exits with the expected code and writes nothing.
    2. Formatting writes exactly the expected text.
    3. `--check` accepts the formatted text.
    4. Formatting the formatted text again changes nothing.
    """
    original = without_header(source)
    expected = "\n".join(header.items("expect-formatted")) + "\n"
    path = project.joinpath("src").joinpath("main.incn")
    write(path, original)?
    mut failures: list[str] = []

    # 1. Check mode reports whether formatting would change the input, and never writes.
    check_code = initial_check_code(header, original, expected)
    before = invoke(incan, ["fmt", "--check", "src/main.incn"], project)?
    if before.code != Some(check_code):
        failures.append(f"initial --check: expected {check_code}, got {before.exit_text()}: {before.stderr}")
    if read(path)? != original:
        failures.append("--check changed the input")
    ...
    return Ok(failures)
```

The rule the first step depends on has its own name, below the entry point:

```incan
def initial_check_code(header: Header, original: str, expected: str) -> int:
    """The initial check's exit code: `expect-exit:` when stated, otherwise 1 when formatting changes the input."""
    if header.declares("expect-exit"):
        return header.expected_exit()
    if original != expected:
        return 1
    return 0
```
