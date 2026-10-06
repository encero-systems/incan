//! Local stage-zero SDK closure measurement through the direct-rustc executor.

/// Compile the explicitly supplied seed and print reproducible JSON measurements.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !(5..=6).contains(&args.len()) {
        return Err("usage: sdk_seed LOCK BLOBS OUTPUT RUSTC INDEX [INSPECTION_PROJECT]".into());
    }
    let closure = oven_rustc::sdk_closure::prepare_sdk_seed(
        std::path::Path::new(&args[0]),
        std::path::Path::new(&args[1]),
        std::path::Path::new(&args[2]),
        std::path::Path::new(&args[3]),
        std::path::Path::new(&args[4]),
    )?;
    if let Some(path) = args.get(5) {
        std::fs::write(path, serde_json::to_vec_pretty(&closure.inspection_project())?)?;
    }
    println!("{}", serde_json::to_string_pretty(closure.report())?);
    Ok(())
}
