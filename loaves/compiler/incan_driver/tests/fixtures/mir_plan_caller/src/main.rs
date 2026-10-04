//! A native Rust caller reads the hand-filled Incan plan through its source-named types.
mod plan;
use incan_mir_plan::caller::incan::scalar_example;
use plan::TerminatorKind;

/// Exercise both ordinary and dangling fixture construction without mirroring any Incan type.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for dangling in [false, true] {
        let plan = scalar_example("scalar.incn".into(), "native_output::caller::incan::print_int".into(), false, dangling);
        let target = match &plan.functions[1].blocks[0].terminator.kind {
            TerminatorKind::Call(_, _, _, target, _) => *target,
            _ => return Err("fixture entry is not a call".into()),
        };
        println!("plan: functions={} target={target} external={}", plan.functions.len(), plan.externals[0].path);
    }
    Ok(())
}
