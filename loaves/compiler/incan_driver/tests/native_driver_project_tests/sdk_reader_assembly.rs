//! Published protocol implementation facts must survive the actual driver's SDK context assembly.

use super::*;

/// Compare an authenticated SDK closure with actual session output before native lowering or receiver specialization.
#[test]
fn reader_protocol_implementation_survives_session_assembly() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("reader-protocol-assembly")?;
    fs::write(root.join("main.incn"), super::sdk_reader_callback::FACTORY)?;
    let binary = super::composed_conversion::bake_session_assembly_caller(
        fixture,
        &root,
        include_str!("sdk_reader_assembly_caller.rs"),
    )?;
    let output = driver_command(&binary).arg(root.join("main.incn")).output()?;
    success(&output, "reader checked protocol implementation assembly");
    assert_eq!(
        output.stdout,
        b"reader protocol assembly: exact relation, body, owner frame and provenance retained\n"
    );
    Ok(())
}
