---
name: test
description: Run and write tests for the Incan compiler. Use when the user asks to test changes, add tests, run the test suite, verify a feature, or says /test. Guides test selection, provides correct patterns, and runs the right commands.
---

# Test — Incan Compiler

## Step 1: Determine what changed

Identify which compiler stages are affected by the current changes:

| Change area | Stages affected |
| --- | --- |
| New syntax or keyword | Parser, Typechecker, Lowering, Emission |
| New semantic rule or validation | Typechecker |
| New or changed codegen output | Lowering, Emission |
| Stdlib addition | Typechecker (loader), Emission, possibly Runtime |
| CLI behavior | Integration tests |
| Formatter change | Property tests, integration tests |

## Step 2: Pick the right tests to write

### Decision tree

```text
Did you change the parser?
  → Add a test in crates/incan_syntax/src/parser/tests.rs

Did you change the typechecker?
  → Add a test in src/frontend/typechecker/tests.rs

Did you change lowering or emission (codegen output)?
  → Add a .incn file in tests/codegen_snapshots/
  → Add a test function in tests/codegen_snapshot_tests.rs
  → Run: INSTA_UPDATE=1 cargo test --test codegen_snapshot_tests

Did you change end-to-end behavior (CLI, build, multi-file)?
  → Add a test in tests/integration_tests.rs

Did you change the formatter?
  → Property tests in tests/property_tests.rs verify idempotency
  → Also add a codegen snapshot if formatting affects output

Did you add a diagnostic?
  → Add a fixture in tests/fixtures/invalid/ that triggers it
  → Add an integration test that asserts the diagnostic message
```

### What to always do

For any pipeline feature (parser through emission), write **both**:

1. A **unit test** at the stage you changed (parser or typechecker)
2. A **codegen snapshot test** that exercises the feature in expressions, not just declarations

## Step 3: Write the tests

### Parser test pattern

File: `crates/incan_syntax/src/parser/tests.rs`

```rust
#[test]
fn test_parse_my_feature() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Example:
    field: str
"#;
    let program = parse_str(source)?;
    assert_eq!(program.declarations.len(), 1);
    Ok(())
}
```

Helpers available: `parse_str(source)`, `parse_str_with_module_path(source, path)`.

### Typechecker test pattern

File: `src/frontend/typechecker/tests.rs`

```rust
#[test]
fn test_my_feature_valid() {
    assert_check_ok(r#"
def example() -> str:
    return "hello"
"#);
}

#[test]
fn test_my_feature_invalid() {
    let result = check_str(r#"
def example() -> int:
    return "hello"
"#);
    assert!(result.is_err());
    let errs = result.unwrap_err();
    assert!(errs.iter().any(|e| e.message.contains("expected")));
}
```

Helpers available: `check_str(source)`, `assert_check_ok(source)`, `check_str_with_library_index(source, index)`.

**Important**: test functions that do fallible work must return `Result` and use `?`. Never use `.unwrap()` or `.expect()`.

### Codegen snapshot test pattern

1. Create `tests/codegen_snapshots/my_feature.incn`:

```incan
def example() -> str:
    return "hello"
```

2. Add to `tests/codegen_snapshot_tests.rs`:

```rust
#[test]
fn test_my_feature_codegen() {
    let source = load_test_file("my_feature");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("my_feature", rust_code);
}
```

3. Generate the snapshot:

```bash
INSTA_UPDATE=1 cargo test --test codegen_snapshot_tests -- test_my_feature_codegen
```

4. Review: `cargo insta review` or check `tests/snapshots/codegen_snapshot_tests__my_feature.snap`.

Helpers available: `load_test_file(name)` (loads from `tests/codegen_snapshots/<name>.incn`), `generate_rust(source)`, `generate_rust_with_widgets_manifest(source)` (for library import tests).

### Integration test pattern

File: `tests/integration_tests.rs`

```rust
#[test]
fn test_my_feature_compiles() -> Result<(), Vec<String>> {
    compile_source(r#"
def main():
    let x: str = "hello"
    print(x)
"#)
}
```

Helpers available: `compile_source(source)`, `compile_file(path)`.

### Invalid fixture pattern

1. Create `tests/fixtures/invalid/my_error_case.incn` with code that should fail.
2. Add an integration test that asserts the expected diagnostic.

## Step 4: Run the tests

### During development (targeted)

```bash
# Run a specific test
cargo test --test codegen_snapshot_tests -- test_my_feature

# Run all typechecker tests
cargo test -p incan --lib typechecker::tests

# Run all parser tests
cargo test -p incan_syntax --lib parser::tests

# Run integration tests
cargo test --test integration_tests
```

### Before finishing (full suite)

```bash
# Full test suite
make test

# Full pre-commit gate (format + tests + clippy)
make pre-commit-full

# Update all snapshots if codegen changed
INSTA_UPDATE=1 cargo test --test codegen_snapshot_tests
```

## Step 5: Verify

After tests pass, check:

- [ ] No `.unwrap()` or `.expect()` in test code (clippy will reject them)
- [ ] Test functions that do fallible work return `Result`
- [ ] Codegen snapshots are updated (no pending `cargo insta review`)
- [ ] Both valid and invalid cases are covered for new diagnostics
