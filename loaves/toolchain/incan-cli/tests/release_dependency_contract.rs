use std::fs;

#[test]
fn release_foundation_builds_lzma_as_a_retained_static_archive() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let features = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get("xz2"))
        .and_then(|xz2| xz2.get("features"))
        .and_then(toml::Value::as_array)
        .ok_or("xz2 must declare its release-foundation features")?;

    assert!(
        features.iter().any(|feature| feature.as_str() == Some("static")),
        "xz2 must select lzma-sys/static so Oven captures liblzma from the build-script output instead of a host \
         pkg-config search path"
    );
    Ok(())
}
