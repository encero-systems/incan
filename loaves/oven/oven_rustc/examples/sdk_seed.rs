//! Local stage-zero SDK closure measurement through the direct-rustc executor.

/// Compile the explicitly supplied seed and print reproducible JSON measurements.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !(5..=7).contains(&args.len()) {
        return Err("usage: sdk_seed LOCK BLOBS OUTPUT RUSTC INDEX [INSPECTION_PROJECT [LOCAL_SDK_ROOT]]".into());
    }
    let mut closure = oven_rustc::sdk_closure::prepare_sdk_seed(
        std::path::Path::new(&args[0]),
        std::path::Path::new(&args[1]),
        std::path::Path::new(&args[2]),
        std::path::Path::new(&args[3]),
        std::path::Path::new(&args[4]),
    )?;
    let mut local_failures = Vec::new();
    if let Some(root) = args.get(6) {
        for (relative, domain, features) in [
            ("loaves/kernel/incan_lang", "target", Vec::new()),
            ("loaves/stdlib/derive/incan_derive", "host", Vec::new()),
            ("loaves/stdlib/derive/incan_web_macros", "host", Vec::new()),
            ("loaves/kernel/incan_vocab", "target", vec!["serde".to_string()]),
            ("loaves/stdlib/core", "target", Vec::new()),
            ("loaves/stdlib/async", "target", Vec::new()),
            ("loaves/stdlib/data", "target", Vec::new()),
            ("loaves/stdlib/web", "target", Vec::new()),
            ("loaves/stdlib/testing", "target", Vec::new()),
        ] {
            if let Err(error) = oven_rustc::sdk_closure::compile_local_sdk_facet(
                &mut closure,
                &std::path::Path::new(root).join(relative),
                &features,
                domain,
                std::path::Path::new(&args[2]),
                std::path::Path::new(&args[3]),
            ) {
                local_failures.push(format!("{relative}: {error}"));
            }
        }
    }
    if let Some(path) = args.get(5) {
        std::fs::write(path, serde_json::to_vec_pretty(&closure.inspection_project())?)?;
    }
    let mut report = serde_json::to_value(closure.report())?;
    report["local_failures"] = serde_json::to_value(&local_failures)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if local_failures.is_empty() {
        Ok(())
    } else {
        Err("one or more local SDK facets failed; see local_failures in the report".into())
    }
}
