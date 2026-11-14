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

Read `compare_format` in `workspaces/contracts/src/formatting.incn`. Its public docstring numbers four checks: the original check, formatting, the formatted check and repeated formatting. The body follows those steps with one numbered comment per step.

The decision about the original check's exit code belongs to `initial_check_code`, below the entry point. File reading and writing are wrapped once, while `without_header` names the case-source transformation. The example's project entry is created in contract scratch; it is not a source file at the repository root.
