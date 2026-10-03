#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use oven_model::manifest::{
    RustFactArgument, RustFactArtifact, RustFactArtifactKind, RustFactEnvironment, RustFactExecutable, RustFactOutput,
    RustFactTool,
};
use oven_store::digest_bytes;
use oven_store::publisher_execution::{
    OvenPublisherExecutionMode, OvenPublisherToolError, OvenPublisherToolOwner, OvenPublisherToolRequest,
    execute_publisher_tool,
};
use oven_store::publisher_owner::publisher_owner_identity;

const HOST: &str = "aarch64-apple-darwin";
const TARGET: &str = "aarch64-apple-darwin";
const CONSUMER: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn write_executable(root: &Path, body: &str) -> Result<RustFactExecutable, Box<dyn std::error::Error>> {
    let path = root.join("bin/isle-fixture");
    fs::create_dir_all(path.parent().ok_or("fixture executable has no parent")?)?;
    fs::write(&path, body)?;
    let mut permissions = fs::metadata(&path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions)?;
    Ok(RustFactExecutable {
        name: "isle-fixture".to_string(),
        owner: publisher_owner_identity(root, ["bin/isle-fixture"])?,
        path: "bin/isle-fixture".to_string(),
        digest: digest_bytes(body.as_bytes()),
    })
}

fn generator_body(suffix: &str) -> String {
    format!(
        "#!/bin/sh\ninput=\"$1\"\noutput=\"$2\"\nprintf '%s\\n' '// generated {suffix}' > \"$output\"\nwhile IFS= read -r line; do printf '%s\\n' \"$line\" >> \"$output\"; done < \"$input\"\n"
    )
}

fn request<'a>(
    tool: &'a RustFactTool,
    fact_root: &'a Path,
    executable_root: &'a Path,
    product_root: &'a Path,
    mode: OvenPublisherExecutionMode,
) -> OvenPublisherToolRequest<'a> {
    OvenPublisherToolRequest {
        mode,
        tool,
        fact_owner: OvenPublisherToolOwner {
            identity: digest_bytes(b"fixture fact owner"),
            root: fact_root,
        },
        executable_owner: OvenPublisherToolOwner {
            identity: tool.executable.owner.clone(),
            root: executable_root,
        },
        host: HOST,
        target: TARGET,
        consuming_units: &[CONSUMER],
        product_root,
    }
}

fn fixture_tool(executable: RustFactExecutable, input_path: &str, input_bytes: &[u8]) -> RustFactTool {
    RustFactTool {
        name: "isle-meta".to_string(),
        target: TARGET.to_string(),
        executable,
        arguments: vec![
            RustFactArgument::Input {
                input: "definitions".to_string(),
            },
            RustFactArgument::Output {
                output: "generated-rust".to_string(),
            },
        ],
        environment: Vec::new(),
        inputs: vec![RustFactArtifact {
            name: "definitions".to_string(),
            kind: RustFactArtifactKind::File,
            path: input_path.to_string(),
            digest: digest_bytes(input_bytes),
            members: Vec::new(),
        }],
        outputs: vec![RustFactOutput {
            name: "generated-rust".to_string(),
            kind: RustFactArtifactKind::File,
            path: "generated/isle.rs".to_string(),
        }],
    }
}

fn prepare_fixture(
    input_name: &str,
    input_bytes: &[u8],
    tool_body: &str,
) -> Result<(tempfile::TempDir, tempfile::TempDir, RustFactTool), Box<dyn std::error::Error>> {
    let fact_owner = tempfile::tempdir()?;
    let tool_owner = tempfile::tempdir()?;
    let input_path = format!("isle/{input_name}");
    let physical_input = fact_owner.path().join(&input_path);
    fs::create_dir_all(physical_input.parent().ok_or("fixture input has no parent")?)?;
    fs::write(&physical_input, input_bytes)?;
    let executable = write_executable(tool_owner.path(), tool_body)?;
    Ok((
        fact_owner,
        tool_owner,
        fixture_tool(executable, &input_path, input_bytes),
    ))
}

fn confinement_was_denied(error: &OvenPublisherToolError) -> bool {
    matches!(
        error,
        OvenPublisherToolError::Execution { message, .. }
            if message.contains("sandbox_apply: Operation not permitted")
    )
}

#[test]
fn publisher_tool_isle_fixtures_are_repeatable_and_identity_bound() -> Result<(), Box<dyn std::error::Error>> {
    for (package, input_name, input_bytes) in [
        (
            "cranelift-codegen",
            "cranelift-codegen.isle",
            include_bytes!("fixtures/isle/cranelift-codegen.isle").as_slice(),
        ),
        (
            "cranelift-assembler-x64",
            "cranelift-assembler-x64.meta",
            include_bytes!("fixtures/isle/cranelift-assembler-x64.meta").as_slice(),
        ),
    ] {
        let body = generator_body(package);
        let (fact_owner, tool_owner, mut tool) = prepare_fixture(input_name, input_bytes, &body)?;
        let first_products = tempfile::tempdir()?;
        let second_products = tempfile::tempdir()?;
        let first = match execute_publisher_tool(&request(
            &tool,
            fact_owner.path(),
            tool_owner.path(),
            first_products.path(),
            OvenPublisherExecutionMode::Publisher,
        )) {
            Ok(receipt) => receipt,
            Err(error) if confinement_was_denied(&error) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let second = execute_publisher_tool(&request(
            &tool,
            fact_owner.path(),
            tool_owner.path(),
            second_products.path(),
            OvenPublisherExecutionMode::Publisher,
        ))?;
        assert_eq!(first.identity, second.identity);
        assert_eq!(first.outputs, second.outputs);
        assert_eq!(
            fs::read(first_products.path().join("generated/isle.rs"))?,
            fs::read(second_products.path().join("generated/isle.rs"))?
        );

        tool.inputs[0].digest = digest_bytes(b"changed input identity");
        assert!(
            execute_publisher_tool(&request(
                &tool,
                fact_owner.path(),
                tool_owner.path(),
                tempfile::tempdir()?.path(),
                OvenPublisherExecutionMode::Publisher,
            ))
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn publisher_tool_changed_tool_identity_and_missing_tool_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let input = include_bytes!("fixtures/isle/cranelift-codegen.isle");
    let body = generator_body("codegen");
    let (fact_owner, tool_owner, mut tool) = prepare_fixture("codegen.isle", input, &body)?;
    tool.executable.digest = digest_bytes(b"substituted executable");
    let products = tempfile::tempdir()?;
    assert!(
        execute_publisher_tool(&request(
            &tool,
            fact_owner.path(),
            tool_owner.path(),
            products.path(),
            OvenPublisherExecutionMode::Publisher,
        ))
        .is_err()
    );

    tool.executable.digest = digest_bytes(body.as_bytes());
    fs::remove_file(tool_owner.path().join(&tool.executable.path))?;
    let path_substitute = tempfile::tempdir()?;
    let marker = path_substitute.path().join("substituted");
    let substitute = path_substitute.path().join("isle-fixture");
    fs::write(
        &substitute,
        format!("#!/bin/sh\nprintf '%s\\n' substituted > '{}'\n", marker.display()),
    )?;
    let mut permissions = fs::metadata(&substitute)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&substitute, permissions)?;
    tool.environment = vec![RustFactEnvironment {
        name: "PATH".to_string(),
        literal: Some(path_substitute.path().to_string_lossy().into_owned()),
        input: None,
    }];
    assert!(
        execute_publisher_tool(&request(
            &tool,
            fact_owner.path(),
            tool_owner.path(),
            products.path(),
            OvenPublisherExecutionMode::Publisher,
        ))
        .is_err()
    );
    assert!(!marker.exists());
    Ok(())
}

#[test]
fn publisher_tool_refuses_output_escape_extra_output_and_undeclared_read() -> Result<(), Box<dyn std::error::Error>> {
    let input = include_bytes!("fixtures/isle/cranelift-codegen.isle");
    let escape_body = "#!/bin/sh\nprintf '%s\\n' escaped > \"$1\"\n";
    let (fact_owner, tool_owner, mut tool) = prepare_fixture("codegen.isle", input, escape_body)?;
    tool.arguments = vec![RustFactArgument::Output {
        output: "generated-rust".to_string(),
    }];
    tool.outputs[0].path = "../escape.rs".to_string();
    let products = tempfile::tempdir()?;
    assert!(
        execute_publisher_tool(&request(
            &tool,
            fact_owner.path(),
            tool_owner.path(),
            products.path(),
            OvenPublisherExecutionMode::Publisher,
        ))
        .is_err()
    );
    assert!(
        !products
            .path()
            .parent()
            .ok_or("products root has no parent")?
            .join("escape.rs")
            .exists()
    );

    let probe_body = generator_body("sandbox-probe");
    let (probe_fact, probe_tool_owner, probe_tool) = prepare_fixture("codegen.isle", input, &probe_body)?;
    let probe_products = tempfile::tempdir()?;
    match execute_publisher_tool(&request(
        &probe_tool,
        probe_fact.path(),
        probe_tool_owner.path(),
        probe_products.path(),
        OvenPublisherExecutionMode::Publisher,
    )) {
        Ok(_) => {}
        Err(error) if confinement_was_denied(&error) => return Ok(()),
        Err(error) => return Err(error.into()),
    }

    let extra_body = "#!/bin/sh\nprintf '%s\\n' declared > \"$2\"\nprintf '%s\\n' extra > extra.rs\n";
    let (fact_owner, tool_owner, tool) = prepare_fixture("codegen.isle", input, extra_body)?;
    let products = tempfile::tempdir()?;
    let extra_error = execute_publisher_tool(&request(
        &tool,
        fact_owner.path(),
        tool_owner.path(),
        products.path(),
        OvenPublisherExecutionMode::Publisher,
    ))
    .err()
    .ok_or("extra output was accepted")?;
    assert!(extra_error.to_string().contains("undeclared product"));

    let missing_body = "#!/bin/sh\nexit 0\n";
    let (fact_owner, tool_owner, tool) = prepare_fixture("codegen.isle", input, missing_body)?;
    let products = tempfile::tempdir()?;
    let missing_error = execute_publisher_tool(&request(
        &tool,
        fact_owner.path(),
        tool_owner.path(),
        products.path(),
        OvenPublisherExecutionMode::Publisher,
    ))
    .err()
    .ok_or("missing output was accepted")?;
    assert!(missing_error.to_string().contains("generated/isle.rs"));

    let read_body = "#!/bin/sh\nset -eu\nIFS= read -r stolen < /etc/hosts\nprintf '%s\\n' \"$stolen\" > \"$2\"\n";
    let (fact_owner, tool_owner, tool) = prepare_fixture("codegen.isle", input, read_body)?;
    let products = tempfile::tempdir()?;
    let read_error = execute_publisher_tool(&request(
        &tool,
        fact_owner.path(),
        tool_owner.path(),
        products.path(),
        OvenPublisherExecutionMode::Publisher,
    ))
    .err()
    .ok_or("undeclared read was accepted")?;
    assert!(read_error.to_string().contains("Operation not permitted"));
    Ok(())
}

#[test]
fn publisher_tool_consumer_never_executes() -> Result<(), Box<dyn std::error::Error>> {
    let input = include_bytes!("fixtures/isle/cranelift-codegen.isle");
    let marker_root = tempfile::tempdir()?;
    let marker = marker_root.path().join("ran");
    let body = format!("#!/bin/sh\nprintf '%s\\n' ran > '{}'\n", marker.display());
    let (fact_owner, tool_owner, tool) = prepare_fixture("codegen.isle", input, &body)?;
    let products = tempfile::tempdir()?;
    assert!(
        execute_publisher_tool(&request(
            &tool,
            fact_owner.path(),
            tool_owner.path(),
            products.path(),
            OvenPublisherExecutionMode::Consumer,
        ))
        .is_err()
    );
    assert!(!marker.exists());
    Ok(())
}

#[test]
fn publisher_tool_receipt_changes_with_declared_input_and_executable_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let first_input = b"(decl lower (Value) Value)\n";
    let second_input = b"(decl lower (Value) Inst)\n";
    let first_body = generator_body("first");
    let second_body = generator_body("second");
    let (first_fact, first_tool_owner, first_tool) = prepare_fixture("input.isle", first_input, &first_body)?;
    let (second_fact, second_tool_owner, second_tool) = prepare_fixture("input.isle", second_input, &second_body)?;
    let first_products = tempfile::tempdir()?;
    let second_products = tempfile::tempdir()?;
    let first = match execute_publisher_tool(&request(
        &first_tool,
        first_fact.path(),
        first_tool_owner.path(),
        first_products.path(),
        OvenPublisherExecutionMode::Publisher,
    )) {
        Ok(receipt) => receipt,
        Err(error) if confinement_was_denied(&error) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let second = execute_publisher_tool(&request(
        &second_tool,
        second_fact.path(),
        second_tool_owner.path(),
        second_products.path(),
        OvenPublisherExecutionMode::Publisher,
    ))?;
    assert_ne!(first.identity, second.identity);
    assert_ne!(first.executable.digest, second.executable.digest);
    assert_ne!(first.inputs, second.inputs);
    Ok(())
}
