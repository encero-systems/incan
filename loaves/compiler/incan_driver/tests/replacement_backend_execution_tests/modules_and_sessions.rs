//! Cross-module execution: the replacement execution graph resolves a canonical target to its owning module
//! and refuses what one module cannot see, and the CLI-driven builds call into sibling modules,
//! module-qualified paths and facade chains, project their JSON reports and persist session feature and
//! module provenance.

use super::*;

/// Lower one named module against a shared checker so two modules can be produced for cross-module tests.
///
/// `lower_typed_body_ir` fixes the module path, which is right for the single-module #988 profile but cannot express
/// a call that crosses a module edge. #1260 needs two Body-IR modules whose canonical identities were minted under
/// their own module paths, which is what this produces.
fn lower_named_body_ir(source: &str, module_path: &[&str]) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|part| (*part).to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Characterize the module boundary #1260 replaces: a cross-module call cannot resolve through one module.
///
/// `prepare_free_function_execution` and `execute_free_function` take one `&BodyIrModule`, so a body whose reachable
/// call graph leaves that module has no way to reach its callee: an imported callable deliberately carries no
/// same-module `direct_call_id`, and #1260 must resolve it from the compiler-owned canonical target rather than
/// manufacture one. This pins the current boundary so the `ReplacementExecutionGraph` that replaces it has a failing
/// test to satisfy, and so the refusal stays a refusal rather than silently executing the wrong body.
///
/// This passes today, so it is a characterization test rather than a failing one. Its value is the invariant it
/// pins: a cross-module call must never resolve against the caller's own module. When #1260 makes such a call
/// execute, this test inverts to assert the value it produces, and until then it stops a partial implementation from
/// silently resolving the wrong body.
#[test]
fn a_cross_module_call_is_refused_by_the_single_module_executor() -> Result<(), Box<dyn std::error::Error>> {
    let provider = lower_named_body_ir("pub def provide() -> int:\n  return 7\n", &["cross_module_provider"])?;
    let consumer = lower_named_body_ir(
        "from cross_module_provider import provide\n\ndef main() -> int:\n  return provide()\n",
        &["cross_module_consumer"],
    )?;

    // The provider's body exists, but it exists in the *other* module.
    assert!(
        provider.bodies.iter().any(|body| body.name == "provide"),
        "the provider module must retain its own callable body"
    );
    assert!(
        !consumer.bodies.iter().any(|body| body.name == "provide"),
        "the consumer module must not contain the imported callee's body; a cross-module call has to be resolved \
         through the module graph rather than found locally"
    );

    let executed = execute_free_function(&consumer, "main", &[]);
    assert!(
        executed.is_err(),
        "executing a cross-module call against only the caller's module must refuse until #1260 supplies the \
         execution graph, got: {executed:?}"
    );
    Ok(())
}

/// A canonical target owned by another module resolves through the execution graph, not through the caller.
///
/// This is the mechanism #1260 dispatches on. `direct_call_id` is a span identity and exists only for a declaration
/// physically present in the calling module, which is why an imported callable has none. The canonical identity
/// answers the different question of *which declaration was selected*, and survives the module boundary, so the
/// graph resolves it by asking each module in turn rather than by matching a name.
///
/// The negative half matters as much as the positive: asking the consumer's own module for the same target must
/// still return nothing. A resolution that succeeded locally would mean the executor could dispatch to a same-named
/// declaration in the wrong module, which is exactly the failure the canonical identity exists to prevent.
#[test]
fn a_canonical_target_resolves_to_its_owning_module_through_the_graph() -> Result<(), Box<dyn std::error::Error>> {
    let provider = lower_named_body_ir(
        "pub def provide() -> int:\n  return 7\n",
        &["graph_resolution_provider"],
    )?;
    let consumer = lower_named_body_ir("def main() -> int:\n  return 1\n", &["graph_resolution_consumer"])?;

    let provided = provider
        .bodies
        .iter()
        .find(|body| body.name == "provide")
        .ok_or("the provider module must retain its callable body")?;
    let target = provided
        .canonical
        .as_ref()
        .ok_or("the provider body must carry a canonical identity")?;

    // The consumer alone cannot resolve it: the identity is owned by another module.
    assert!(
        consumer.body_for_canonical_target(target).is_none(),
        "a canonical target owned by another module must not resolve against the caller's own module"
    );

    let graph = ReplacementExecutionGraph::new(&consumer, std::iter::once(&provider))?;
    let (owner, body) = graph
        .body_for_canonical_target(target)
        .ok_or("the graph must resolve a canonical target owned by a reachable module")?;
    assert_eq!(
        owner.module_id, provider.module_id,
        "resolution must report the module that owns the declaration"
    );
    assert_eq!(body.name, "provide", "resolution must return the selected declaration");
    Ok(())
}

/// A graph refuses to be built from two modules claiming one identity.
///
/// Assembling a graph from disagreeing analyses is a caller fault, and accepting it would make dispatch depend on
/// assembly order. Refusing at construction keeps that fault at the boundary that produced it rather than surfacing
/// later as a call resolving to whichever module happened to be first.
#[test]
fn a_graph_refuses_duplicate_module_identities() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower_named_body_ir("def main() -> int:\n  return 1\n", &["duplicate_identity"])?;
    let twin = lower_named_body_ir("def main() -> int:\n  return 2\n", &["duplicate_identity"])?;
    assert!(
        ReplacementExecutionGraph::new(&module, std::iter::once(&twin)).is_err(),
        "two modules claiming one identity must refuse rather than resolve by assembly order"
    );
    Ok(())
}

/// Two independently checked modules cannot express a resolved cross-module call.
///
/// This records why #1260's executable proof lives in a session-backed integration fixture rather than here. Each
/// module below is checked by its own `TypeChecker`, so the consumer's checker never sees the provider and the call
/// stays unresolved: `direct_call_id`, `binding`, and `canonical` are all absent. There is nothing for the execution
/// graph to resolve, and a test that lowered two modules separately and expected a cross-module call to execute
/// would be asserting against a call the frontend never resolved.
///
/// `CompilationSession::analyze_modules` is the only checker authority for a source graph, which is what makes a
/// call across a module edge carry the canonical identity dispatch resolves on.
#[test]
fn independently_checked_modules_do_not_resolve_a_cross_module_call() -> Result<(), Box<dyn std::error::Error>> {
    let consumer = lower_named_body_ir(
        "from unseen_provider import provide\n\ndef main() -> int:\n  return provide()\n",
        &["unresolved_consumer"],
    )?;
    let unresolved =
        consumer
            .bodies
            .iter()
            .flat_map(|body| body.block.stmts.iter())
            .filter_map(|statement| match &statement.kind {
                StatementKind::Call {
                    callee:
                        incan_semantics_core::body_ir::Callee::Function(
                            incan_semantics_core::body_ir::CallableTarget::Named(target),
                        ),
                    ..
                } => Some(target),
                _ => None,
            })
            .find(|target| target.name == "provide");
    if let Some(target) = unresolved {
        assert!(
            target.direct_call_id.is_none() && target.canonical.is_none(),
            "a call the frontend never resolved must carry neither identity, got: {target:?}"
        );
    }
    Ok(())
}

/// Run the explicit replacement CLI path without allowing a legacy fallback.
#[test]
fn replacement_cli_executes_typed_body_ir_and_persists_a_replacement_receipt() -> Result<(), Box<dyn std::error::Error>>
{
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def main() -> int:
  left = 40
  right = 2
  if left > right:
    return left + right
  return right
"#,
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .args([
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "replacement build must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "non-printing programs must leave stdout empty"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct replacement execution must not create a legacy generated-project directory"
    );

    let receipt_path = temporary.path().join(".incan/backend/receipt.json");
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(receipt_path)?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["selection"]["selected_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], serde_json::json!("not_needed"));
    assert!(
        receipt["replacement_execution"].is_null(),
        "persisted #986 receipts must remain receipt-only; execution evidence belongs to the report payload"
    );
    assert!(
        receipt["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:"))
    );
    Ok(())
}

/// Execute a call that leaves the entry module through the replacement route, with a #986 receipt.
///
/// This is #1260's local-module positive fixture. Everything before it executed inside one module, where a callee
/// carries a same-module `direct_call_id`. Here `main` calls a function declared in a sibling module, so the frontend
/// resolves the callee to a canonical identity instead and the execution graph has to find the module that declares
/// it. The result is asserted from the report rather than the receipt, because the receipt stays receipt-only.
#[test]
fn replacement_cli_executes_a_call_into_a_sibling_module_with_a_replacement_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("helper.incn"),
        "pub def bump(value: int) -> int:\n  return value + 1\n",
    )?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        "from helper import bump\n\ndef main() -> int:\n  return bump(41)\n",
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .args([
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("cross-module-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "a call into a sibling module must execute through the replacement route. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("cross-module-report.json"))?)?;
    assert_eq!(
        report["replacement_execution"]["result"], "42",
        "the sibling module's declaration must produce the source-observable result"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct replacement execution must not create a legacy generated-project directory"
    );

    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["selection"]["selected_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], serde_json::json!("not_needed"));
    Ok(())
}

/// Execute a module-qualified call, written and aliased, through the replacement route.
///
/// `helper.bump(41)` reaches the AST as a method call, but `helper` is a namespace, not a value. Lowering it as a
/// receiver refused at a name that never named a place: "resolved reference `helper` has no Body IR place
/// representation". The qualifier says where to look; the typechecker has already resolved what was found, and this
/// executes that declaration.
///
/// Both spellings are pinned because they differ only in the qualifier's own binding, and an alias is the case where
/// a name-derived target would go wrong: `provider.bump` and `helper.bump` are one declaration reached two ways.
#[test]
fn replacement_cli_executes_a_module_qualified_call() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "plain",
            "import helper\n\ndef main() -> int:\n  return helper.bump(41)\n",
        ),
        (
            "aliased",
            "import helper as provider\n\ndef main() -> int:\n  return provider.bump(41)\n",
        ),
    ];
    for (name, entry_source) in cases {
        let temporary = tempfile::tempdir()?;
        fs::write(
            temporary.path().join("helper.incn"),
            "pub def bump(value: int) -> int:\n  return value + 1\n",
        )?;
        let entrypoint = temporary.path().join("main.incn");
        fs::write(&entrypoint, entry_source)?;

        let report_path = temporary.path().join("module-qualified-report.json");
        let output = support::repo_command()
            .args([
                "build",
                entrypoint.to_string_lossy().as_ref(),
                "--backend",
                "replacement",
                "--backend-fallback",
                "refuse",
            ])
            .args([
                "--report",
                "json",
                "--report-output",
                report_path.to_string_lossy().as_ref(),
            ])
            .output()?;
        assert!(
            output.status.success(),
            "the {name} module-qualified call must execute through the replacement route. stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let report: serde_json::Value = serde_json::from_slice(&fs::read(&report_path)?)?;
        assert_eq!(
            report["replacement_execution"]["result"], "42",
            "the {name} module-qualified call must produce the qualified declaration's result"
        );

        let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
            temporary.path().join(".incan/backend/receipt.json"),
        )?)?;
        assert_eq!(receipt["executed_backend"], "replacement");
        assert_eq!(receipt["fallback_outcome"], serde_json::json!("not_needed"));
    }
    Ok(())
}

/// A receiver that merely shares a module's name is still an ordinary method call.
///
/// The module-qualified path keys off the receiver's resolved identity rather than its spelling, so a local bound to
/// a value keeps its own meaning even when an imported module has the same name. Getting this wrong would silently
/// redirect a method call to a free function in another module, which is the exact failure the identity-over-spelling
/// rule exists to prevent -- so it is pinned rather than assumed.
#[test]
fn a_local_sharing_a_module_name_is_not_treated_as_a_module_qualifier() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("helper.incn"),
        "pub def bump(value: int) -> int:\n  return value + 1\n",
    )?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        "import helper\n\ndef main() -> int:\n  helper = 41\n  return helper.bump()\n",
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "an int has no `bump` method, so this must not execute: {combined}"
    );
    // The program's own output would be the bare line `42`; a substring check over the whole transcript also matches
    // the digits inside a temporary-directory name, and the compiler suite's scratch path once did.
    assert!(
        !String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == "42"),
        "the shadowing local must not be redirected to the module's declaration: {combined}"
    );
    Ok(())
}

/// A typed-numeric carrier crosses a module boundary, and the callee's own operations are still validated.
///
/// The typed-numeric walk previously followed only same-module calls, because it selected the callee by its
/// same-module span identity. A cross-module callee was therefore refused outright. Admitting one is only sound if
/// its operations are validated too, so both directions are pinned here: the admitted call executes, and an
/// operation the profile does not admit is still refused when it lives in the *callee's* module rather than the
/// entrypoint's.
#[test]
fn replacement_cli_follows_a_typed_numeric_call_into_the_module_that_declares_it()
-> Result<(), Box<dyn std::error::Error>> {
    let admitted = tempfile::tempdir()?;
    fs::write(
        admitted.path().join("helper.incn"),
        "pub def widen(value: u8) -> u8:\n  return value\n",
    )?;
    let entrypoint = admitted.path().join("main.incn");
    fs::write(
        &entrypoint,
        "from helper import widen\n\ndef main() -> u8:\n  start: u8 = 41\n  return widen(start)\n",
    )?;
    let report_path = admitted.path().join("typed-numeric-report.json");
    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .args([
            "--report",
            "json",
            "--report-output",
            report_path.to_string_lossy().as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "an admitted typed-numeric call must cross the module boundary. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&report_path)?)?;
    assert_eq!(report["replacement_execution"]["result"], "41");

    // The same shape, with an operation the carrier profile does not admit moved into the callee's module. A refusal
    // here is the evidence that the walk actually entered `helper.incn` rather than trusting the call.
    let refused = tempfile::tempdir()?;
    fs::write(
        refused.path().join("helper.incn"),
        "pub def widen(value: u8) -> u8:\n  values: List[u8] = [value]\n  return values[0]\n",
    )?;
    let refused_entrypoint = refused.path().join("main.incn");
    fs::write(
        &refused_entrypoint,
        "from helper import widen\n\ndef main() -> u8:\n  start: u8 = 41\n  return widen(start)\n",
    )?;
    let refusal = support::repo_command()
        .args([
            "build",
            refused_entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    assert!(
        !refusal.status.success(),
        "an unadmitted typed-numeric operation in the callee's module must still be refused"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&refusal.stdout),
        String::from_utf8_lossy(&refusal.stderr)
    );
    assert!(
        combined.contains("INCAN-R988-UNSUPPORTED") && combined.contains("aggregate construction"),
        "the callee module's unadmitted operation must be named: {combined}"
    );
    assert!(
        combined.contains("helper.incn:") && !combined.contains("main.incn:"),
        "a refusal measured in the callee's module must report that module's source: {combined}"
    );
    Ok(())
}

/// A facade chain executes, and its direct, re-exported and aliased bindings name one canonical declaration.
///
/// This is #1261's facade fixture. `provider` declares `compute`; `facade` re-exports it and aliases it as `renamed`;
/// the entrypoint reaches it all three ways in a single expression. The arithmetic is chosen so the total only
/// balances when every binding actually reaches `compute` -- 2 + 11 + 29 -- rather than asserting three separate
/// calls that could each be satisfied differently.
///
/// Execution alone would not show that the three bindings are the *same* declaration, so identity is asserted
/// separately through `incan inspect codegraph`. That is a public projection: it reports declaration ids and call
/// targets without exposing compiler-session state, Body IR, or generated Rust names, which is the boundary #1261
/// requires its metadata evidence to respect.
#[test]
fn replacement_cli_executes_a_facade_chain_whose_bindings_share_one_declaration()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let source_dir = temporary.path().join("src");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        temporary.path().join("loaf.toml"),
        "[project]\nname = \"facade_chain\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        source_dir.join("provider.incn"),
        "pub def compute(value: int) -> int:\n  return value + 1\n",
    )?;
    fs::write(
        source_dir.join("facade.incn"),
        "pub from provider import compute\n\npub renamed = alias compute\n",
    )?;
    let entrypoint = source_dir.join("main.incn");
    fs::write(
        &entrypoint,
        "from provider import compute\nfrom facade import compute as reexported, renamed\n\ndef main() -> int:\n  return compute(1) + reexported(10) + renamed(28)\n",
    )?;

    let report_path = temporary.path().join("facade-report.json");
    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .args([
            "--report",
            "json",
            "--report-output",
            report_path.to_string_lossy().as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "the facade chain must execute through the replacement route. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&report_path)?)?;
    assert_eq!(
        report["replacement_execution"]["result"], "42",
        "the total only balances when the direct, re-exported and aliased bindings all reach `compute`"
    );
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], serde_json::json!("not_needed"));

    // The same chain, inspected: every call must name the one declaration in `provider`.
    let codegraph = support::repo_command()
        .args(["inspect", "codegraph", "src", "--format", "jsonl"])
        .current_dir(temporary.path())
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .output()?;
    assert!(
        codegraph.status.success(),
        "codegraph inspection must succeed. stderr:\n{}",
        String::from_utf8_lossy(&codegraph.stderr)
    );
    let mut call_targets: Vec<(String, String)> = Vec::new();
    for line in String::from_utf8_lossy(&codegraph.stdout).lines() {
        let record: serde_json::Value = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(_) => continue,
        };
        if record["record"] != serde_json::json!("call") {
            continue;
        }
        let (Some(callee), Some(target)) = (record["callee"].as_str(), record["target_id"].as_str()) else {
            continue;
        };
        call_targets.push((callee.to_string(), target.to_string()));
    }
    call_targets.sort();
    let observed: Vec<&str> = call_targets.iter().map(|(callee, _)| callee.as_str()).collect();
    assert_eq!(
        observed,
        ["compute", "reexported", "renamed"],
        "the fixture must record one call through each binding, got {call_targets:?}"
    );
    let distinct: BTreeSet<&str> = call_targets.iter().map(|(_, target)| target.as_str()).collect();
    assert_eq!(
        distinct.len(),
        1,
        "a direct import, a re-export and an alias must name one declaration, got {distinct:?}"
    );
    let target = distinct.iter().next().ok_or("expected one resolved declaration")?;
    assert!(
        target.contains("provider.incn") && target.ends_with(":compute"),
        "the shared declaration must be `compute` in the provider module, got {target}"
    );
    Ok(())
}

/// A sibling module is held to the same source-only profile as the entrypoint.
///
/// Allowing a local import means an unsupported declaration becomes reachable from a file the entrypoint never names.
/// The refusal must still happen, so the boundary is a property of the executed graph rather than of whichever file
/// happened to be the entrypoint.
#[test]
fn replacement_cli_refuses_an_unsupported_declaration_in_a_sibling_module() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("helper.incn"),
        "pub class Holder:\n  value: int\n\npub def bump(value: int) -> int:\n  return value + 1\n",
    )?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        "from helper import bump\n\ndef main() -> int:\n  return bump(41)\n",
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    assert!(
        !output.status.success(),
        "an unsupported declaration in a reachable module must be refused"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("INCAN-R988-UNSUPPORTED") && combined.contains("non-function top-level declaration"),
        "missing typed refusal for the sibling module's declaration: {combined}"
    );
    // The span was measured in `helper.incn`, so the reported location has to be `helper.incn`. Reporting the
    // entrypoint alongside another file's offsets points a reader at unrelated source.
    assert!(
        combined.contains("helper.incn:") && !combined.contains("main.incn:"),
        "a refusal raised in a sibling module must report that module's source: {combined}"
    );
    assert!(
        !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a refused profile must not publish a replacement receipt"
    );
    Ok(())
}

/// Execute a typed empty scalar-pair list directly and publish only replacement receipt evidence.
#[test]
fn replacement_cli_executes_typed_empty_scalar_tuple_list_with_a_replacement_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def main() -> int:
  values: list[tuple[int, int]] = []
  for a, b in values:
    return a + b
  return 42
"#,
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .args([
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "typed empty scalar-pair list must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "non-printing programs must leave stdout empty"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct replacement execution must not create a legacy generated-project directory"
    );

    let receipt_path = temporary.path().join(".incan/backend/receipt.json");
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(receipt_path)?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], serde_json::json!("not_needed"));
    Ok(())
}

/// Emit stable direct-execution evidence in the distinct, non-Oven replacement JSON report.
#[test]
fn replacement_cli_json_report_projects_canonical_execution_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        "def main() -> int:\n  println(\"answer follows\")\n  return 42\n",
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "replacement JSON report must succeed. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["schema_version"], "incan.replacement_execution.v1");
    assert_eq!(report["status"], "success");
    assert_eq!(report["mode"], "executable");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert_eq!(report["replacement_execution"]["result_type"], "int");
    assert_eq!(
        report["replacement_execution"]["emitted_output"],
        serde_json::json!(["answer follows"])
    );
    assert_eq!(output.stdout, b"answer follows\n");
    assert_eq!(
        report["replacement_execution"]["stdout_bytes"],
        serde_json::json!(output.stdout)
    );
    assert_eq!(report["replacement_execution"]["stderr_bytes"], serde_json::json!([]));
    assert!(
        output.stderr.is_empty(),
        "report metadata must not displace program output"
    );
    assert!(
        report["replacement_execution"]["output_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "direct report must project the receipt-bound output identity: {report}"
    );
    assert!(
        report["replacement_execution"]["ownership_reads"].is_array()
            && report["replacement_execution"]["runtime_requirements"].is_array(),
        "direct report must project canonical ownership and runtime evidence: {report}"
    );
    assert!(
        report["replacement_execution"]["task_lifecycle"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "a direct scalar execution that never constructs a task must publish an empty task lifecycle: {report}"
    );
    assert!(
        report.get("generated").is_none() && report.get("oven").is_none(),
        "direct replacement report must not invent generated-Rust or Oven evidence: {report}"
    );
    Ok(())
}

/// Execute only the session-projected `beta` entry and bind its semantic module to both report and receipt evidence.
#[test]
fn replacement_cli_uses_session_feature_projection_and_persists_semantic_module_provenance()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let source_root = temporary.path().join("src");
    fs::create_dir_all(&source_root)?;
    let entrypoint = source_root.join("main.incn");
    fs::write(
        temporary.path().join("loaf.toml"),
        "[project]\nname = \"replacement_session_projection\"\n\n[project.features]\nbeta = []\n",
    )?;
    fs::write(
        &entrypoint,
        "when feature(\"beta\"):\n  def main() -> int:\n    return 42\n",
    )?;

    let inactive = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    assert!(
        !inactive.status.success(),
        "an inactive session projection must not execute its gated entrypoint"
    );
    assert!(
        !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a refused inactive entrypoint must not publish a replacement receipt"
    );

    let enabled = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--features",
            "beta",
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        enabled.status.success(),
        "the selected session feature must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&enabled.stdout),
        String::from_utf8_lossy(&enabled.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    let provenance = &report["semantic_module"];
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert_eq!(provenance["module_id"], "module:main");
    assert_eq!(provenance["module_path"], "main");
    assert!(
        provenance["source_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:"))
            && provenance["semantic_snapshot_identity"]
                .as_str()
                .is_some_and(|identity| identity.starts_with("sha256:")),
        "the report must name stable source and semantic-snapshot identities: {report}"
    );
    assert!(
        report["replacement_execution"]["body_snapshot"]
            .as_str()
            .is_some_and(|snapshot| snapshot.contains("span="))
            && report["replacement_execution"]["ownership_reads"].is_array()
            && report["replacement_execution"]["runtime_requirements"].is_array(),
        "the execution evidence must stay attached to the session-owned semantic module: {report}"
    );

    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["semantic_module"], *provenance);
    assert_eq!(receipt["selection"]["source_identity"], provenance["source_identity"]);
    assert!(
        !temporary.path().join("target/incan").exists(),
        "a session-projected replacement execution must not create legacy generated output"
    );
    Ok(())
}
