//! Local stage-zero SDK closure measurement through the direct-rustc executor.

/// Compile the explicitly supplied seed and print reproducible JSON measurements.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 5 {
        return Err("usage: sdk_seed LOCK BLOBS OUTPUT RUSTC INDEX".into());
    }
    let report = oven_rustc::sdk_closure::compile_sdk_seed(
        std::path::Path::new(&args[0]),
        std::path::Path::new(&args[1]),
        std::path::Path::new(&args[2]),
        std::path::Path::new(&args[3]),
        std::path::Path::new(&args[4]),
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
