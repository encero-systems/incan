//! Borrowed real Body IR across an Oven-built Incan caller boundary.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use incan_test_support as support;

/// Preserve source-only fixture trees while leaving generated targets outside the test input.
fn copy_sources(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_sources(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Bake explicitly and retain full diagnostics if the declared closure cannot be published.
fn bake(project: &Path, home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = support::repo_command();
    support::configure_explicit_oven_bake_command(&mut command)?;
    let output = command
        .args(["oven", "bake", "--project"])
        .arg(project)
        .env("INCAN_HOME", home)
        .output()?;
    assert_success(&output);
    Ok(())
}

/// Include both output streams so a boundary failure remains reproducible.
fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Keep a caller-selected diagnostic fixture outside wrapper scratch; ordinary runs remain temporary.
fn fixture_root() -> Result<(std::path::PathBuf, Option<tempfile::TempDir>), Box<dyn std::error::Error>> {
    if let Some(parent) = std::env::var_os("INCAN_BODY_IR_CALLER_EVIDENCE") {
        fs::create_dir_all(&parent)?;
        let directory = tempfile::tempdir_in(parent)?;
        let path = directory.keep();
        eprintln!("retained Body IR caller fixture: {}", path.display());
        Ok((path, None))
    } else {
        let directory = tempfile::tempdir()?;
        Ok((directory.path().to_path_buf(), Some(directory)))
    }
}

/// The caller and provider borrow one real semantics-core module and match its statement enum.
#[test]
fn oven_caller_borrows_real_body_ir() -> Result<(), Box<dyn std::error::Error>> {
    let (root, _temporary) = fixture_root()?;
    let repo = support::repo_root();
    let semantics = repo.join("loaves/kernel/incan_semantics_core");
    let plan = root.join("plan");
    let lowering = root.join("lowering");
    let caller = root.join("caller");
    fs::create_dir_all(&plan)?;
    fs::create_dir_all(&lowering)?;
    fs::create_dir_all(caller.join("src"))?;
    fs::copy(
        repo.join("loaves/compiler/incan_mir_plan/loaf.toml"),
        plan.join("loaf.toml"),
    )?;
    copy_sources(&repo.join("loaves/compiler/incan_mir_plan/src"), &plan.join("src"))?;
    let manifest = fs::read_to_string(repo.join("loaves/compiler/incan_mir_lowering/loaf.toml"))?
        .replace("../incan_mir_plan", "../plan")
        .replace(
            "../../kernel/incan_semantics_core",
            semantics.to_str().ok_or("semantics path is not UTF-8")?,
        );
    fs::write(lowering.join("loaf.toml"), manifest)?;
    copy_sources(
        &repo.join("loaves/compiler/incan_mir_lowering/src"),
        &lowering.join("src"),
    )?;
    fs::write(
        caller.join("loaf.toml"),
        format!(
            "[project]\nname = 'body-ir-caller'\n[dependencies]\nincan_mir_lowering = {{ loaf = 'incan_mir_lowering', path = '../lowering' }}\n[rust-dependencies]\nincan_semantics_core = {{ path = '{}' }}\n[[rust.bin]]\nname = 'body-ir-caller'\npath = 'src/main.rs'\n",
            semantics.display()
        ),
    )?;
    fs::write(
        caller.join("src/main.rs"),
        r#"
use incan_mir_lowering::caller::incan::{validate_module, is_continue, count_continues, lower_module, Plan};
use incan_semantics_core::{CompilerNodeId, CanonicalSymbolId, HirSourceSpan, IncanType, IncanPrimitiveType, SemanticSourceTargetKind,
    body_ir::{BodyIrModule, Body, Block, ScopeId, Statement, StatementKind}};
/// Synthetic canonical identity records exercise the validator; this is not an executable-plan fixture.
fn synthetic_body() -> Body {
    let span = HirSourceSpan { start: 0, end: 10 };
    let id = CompilerNodeId::declaration_span("fixture", span.start, span.end);
    Body {
        decl_id: id.clone(), direct_call_id: id,
        canonical: Some(CanonicalSymbolId::module_declaration(vec!["fixture".into()], "sample", SemanticSourceTargetKind::Function, span)),
        name: "sample".into(), span, return_type: IncanType::Primitive(IncanPrimitiveType::Unit),
        named_type_identities: Default::default(), locals: vec![], params: vec![], param_locals: vec![], scopes: vec![],
        block: Block { scope: ScopeId(0), stmts: vec![Statement { kind: StatementKind::Return { value: None }, span }] },
        runtime_requirements: vec![], panic_facts: vec![], is_async: false, extern_delegation: None,
    }
}
/// Real Rust-backed types cross the shared caller boundary and malformed identities are refused.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut module = BodyIrModule {
        module_id: CompilerNodeId::module("fixture"), stdlib_delegations: vec![], nominal_declarations: vec![],
        fieldless_enum_declarations: vec![], value_enum_declarations: vec![], bodies: vec![synthetic_body()],
    };
    assert_eq!(validate_module(&module), Ok(1));
    assert_eq!(count_continues(&module), Ok(0));
    let plan: Plan = lower_module(&module, "def sample() -> None: return".into(), "fixture.incn".into())?;
    assert_eq!(plan.functions.len(), 1);
    assert_eq!(plan.functions[0].name, "sample");
    module.bodies[0].direct_call_id = CompilerNodeId::declaration_span("fixture", 1, 10);
    assert!(validate_module(&module).is_err());
    assert!(count_continues(&module).is_err());
    assert!(is_continue(&StatementKind::Continue));
    assert!(!is_continue(&StatementKind::Return { value: None }));
    Ok(())
}
"#,
    )?;
    let home = root.join("home");
    bake(&plan, &home)?;
    bake(&lowering, &home)?;
    bake(&caller, &home)?;
    for profile in ["debug", "release"] {
        assert_success(&Command::new(caller.join("target/rust").join(profile).join("body-ir-caller")).output()?);
    }
    Ok(())
}
