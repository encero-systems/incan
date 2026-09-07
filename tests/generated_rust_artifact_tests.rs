use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incan::library_manifest::LibraryManifest;

mod support;

#[path = "support/canonical_projection.rs"]
mod canonical_projection;

const FIXTURE_ROOT: &str = "tests/fixtures/generated_rust_artifacts";

fn incan_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_incan") {
        return PathBuf::from(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = PathBuf::from(target_dir).join("debug").join("incan");
        if path.exists() {
            return path;
        }
    }

    manifest_dir.join("target").join("debug").join("incan")
}

fn configured_incan_command(current_dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(incan_binary());
    command
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1");
    if !support::oven_compiler_suite_is_active() {
        command
            .env(
                "INCAN_GENERATED_CARGO_TARGET_DIR",
                support::generated_cargo_target_dir(),
            )
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", support::sdk_provider_store());
    }
    command
}

/// Normal commands must not inherit the suite's narrowly granted baker proxy.
fn run_incan(current_dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = configured_incan_command(current_dir, args);
    command.env_remove("CARGO");
    Ok(command.output()?)
}

/// Publish a provider closure only through Oven's explicit project bake boundary.
fn run_explicit_oven_bake(current_dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    let mut bake_args = vec!["oven", "bake", "--project", "."];
    bake_args.extend_from_slice(args);
    Ok(configured_incan_command(current_dir, &bake_args).output()?)
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_ROOT).join(name)
}

fn read_fixture(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(fs::read_to_string(fixture_path(name))?)
}

fn write_fixture(destination: &Path, fixture: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(destination, read_fixture(fixture)?)?;
    Ok(())
}

fn assert_required_files(root: &Path, fixture: &str) -> Result<(), Box<dyn std::error::Error>> {
    let expected_files = read_fixture(fixture)?;
    for relative in expected_files.lines().map(str::trim) {
        if relative.is_empty() || relative.starts_with('#') {
            continue;
        }
        let path = root.join(relative);
        assert!(path.is_file(), "expected generated artifact file `{}`", path.display());
    }
    Ok(())
}

/// Normal Oven output is a direct-rustc artifact, not a generated Cargo workspace.
fn assert_no_cargo_lock(root: &Path) {
    let lock = root.join("Cargo.lock");
    assert!(
        !lock.exists(),
        "normal Oven output must not reconstruct Cargo state at `{}`",
        lock.display()
    );
}

/// Assert that a generated artifact still contains the source-shaped declarations its fixture pins.
///
/// The fixtures describe declarations as the Incan source spells them, so the artifact is compared after RFC 120
/// projections are decoded. Pinning the physical projections instead would make these fixtures churn on any unrelated
/// source-line move, because a canonical identity encodes its declaration span; `emitted_symbol_projection_tests`
/// owns the exact-projection assertions this gate deliberately leaves alone.
///
/// A fragment may match either the decoded artifact or its re-formatted form. Both are faithful views of the same
/// artifact: decoding preserves comments such as the generated header, while re-formatting restores the one-line
/// signatures that the longer encoded spellings had forced `prettyplease` to wrap.
fn assert_contains_fragments(path: &Path, fixture: &str) -> Result<(), Box<dyn std::error::Error>> {
    let decoded = canonical_projection::decoded_source_spellings(&fs::read_to_string(path)?);
    let reformatted = canonical_projection::reformatted_after_decode(&decoded);
    let fragments = read_fixture(fixture)?;
    for fragment in fragments.split("\n---\n") {
        let fragment = fragment.trim_matches('\n');
        if fragment.trim().is_empty() {
            continue;
        }
        let present = decoded.contains(fragment) || reformatted.as_deref().is_some_and(|code| code.contains(fragment));
        assert!(
            present,
            "expected `{}` to contain fragment:\n{}\n\nactual (RFC 120 projections decoded to source spellings):\n{}",
            path.display(),
            fragment,
            decoded
        );
    }
    Ok(())
}

fn toml_at<'a>(table: &'a toml::Table, key: &str) -> Result<&'a toml::Value, Box<dyn std::error::Error>> {
    table
        .get(key)
        .ok_or_else(|| format!("generated Cargo.toml missing `{key}`").into())
}

fn toml_table_at<'a>(table: &'a toml::Table, key: &str) -> Result<&'a toml::Table, Box<dyn std::error::Error>> {
    toml_at(table, key)?
        .as_table()
        .ok_or_else(|| format!("generated Cargo.toml `{key}` was not a table").into())
}

fn toml_string_at<'a>(table: &'a toml::Table, key: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    toml_at(table, key)?
        .as_str()
        .ok_or_else(|| format!("generated Cargo.toml `{key}` was not a string").into())
}

fn read_cargo_toml(path: &Path) -> Result<toml::Table, Box<dyn std::error::Error>> {
    let cargo_toml = fs::read_to_string(path)?;
    Ok(toml::from_str(&cargo_toml)?)
}

#[test]
fn generated_application_artifact_matches_baseline() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path().join("artifact_app_project");
    let src_dir = project_root.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_root.join("incan.toml"),
        r#"[project]
name = "artifact_app_baseline"
version = "2.3.4"
license = "Apache-2.0"
license-files = ["LICENSE"]
"#,
    )?;
    fs::write(project_root.join("LICENSE"), "Apache License 2.0\n")?;
    write_fixture(&src_dir.join("main.incn"), "app_main.incn")?;

    let out_dir = project_root.join("out");
    let main_arg = src_dir
        .join("main.incn")
        .to_str()
        .ok_or("application source path was not valid UTF-8")?
        .to_string();
    let out_arg = out_dir
        .to_str()
        .ok_or("application output path was not valid UTF-8")?
        .to_string();
    let output = run_incan(&project_root, &["build", &main_arg, &out_arg])?;
    assert_success(&output, "incan build application artifact");

    assert_required_files(&out_dir, "app_required_files.txt")?;
    assert_no_cargo_lock(&out_dir);
    assert_contains_fragments(&out_dir.join("src").join("main.rs"), "app_main_rs.fragments")?;

    let cargo_toml = read_cargo_toml(&out_dir.join("Cargo.toml"))?;
    let package = toml_table_at(&cargo_toml, "package")?;
    assert_eq!(toml_string_at(package, "name")?, "artifact_app_baseline");
    assert_eq!(toml_string_at(package, "version")?, "2.3.4");
    assert_eq!(toml_string_at(package, "edition")?, "2024");
    assert_eq!(toml_string_at(package, "license")?, "Apache-2.0");
    assert!(
        package.get("license-file").is_none(),
        "Incan's plural license-files metadata must not become Cargo's singular license-file field"
    );
    let dependencies = toml_table_at(&cargo_toml, "dependencies")?;
    assert!(
        toml_at(dependencies, "incan_stdlib").is_ok(),
        "generated application Cargo.toml should include incan_stdlib"
    );
    assert!(
        toml_at(dependencies, "incan_derive").is_ok(),
        "generated application Cargo.toml should include incan_derive"
    );

    Ok(())
}

#[test]
fn generated_application_without_package_metadata_uses_compiler_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path().join("artifact_default_metadata_project");
    let src_dir = project_root.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_root.join("incan.toml"),
        "[project]\nname = \"artifact_default_metadata\"\n",
    )?;
    write_fixture(&src_dir.join("main.incn"), "app_main.incn")?;

    let out_dir = project_root.join("out");
    let main_arg = src_dir
        .join("main.incn")
        .to_str()
        .ok_or("application source path was not valid UTF-8")?
        .to_string();
    let out_arg = out_dir
        .to_str()
        .ok_or("application output path was not valid UTF-8")?
        .to_string();
    let output = run_incan(&project_root, &["build", &main_arg, &out_arg])?;
    assert_success(&output, "incan build application artifact with default metadata");

    let cargo_toml = read_cargo_toml(&out_dir.join("Cargo.toml"))?;
    let package = toml_table_at(&cargo_toml, "package")?;
    assert_eq!(toml_string_at(package, "version")?, env!("CARGO_PKG_VERSION"));
    assert!(package.get("license").is_none());

    Ok(())
}

#[test]
fn generated_library_and_pub_dependency_consumer_artifacts_match_baseline() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path().join("artifact_widgets_project");
    let src_dir = project_root.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_root.join("incan.toml"),
        r#"[project]
name = "artifact_widgets_core"
version = "4.5.6"
license = "MIT OR Apache-2.0"
license-files = ["LICENSE-MIT", "LICENSE-APACHE"]
"#,
    )?;
    fs::write(project_root.join("LICENSE-MIT"), "MIT License\n")?;
    fs::write(project_root.join("LICENSE-APACHE"), "Apache License 2.0\n")?;
    write_fixture(&src_dir.join("widgets.incn"), "library_widgets.incn")?;
    write_fixture(&src_dir.join("lib.incn"), "library_lib.incn")?;

    let output = run_explicit_oven_bake(&project_root, &[])?;
    assert_success(&output, "explicit Oven bake for the public library artifact");

    let artifact_root = project_root.join("target").join("lib");
    assert_required_files(&artifact_root, "library_required_files.txt")?;
    assert_no_cargo_lock(&artifact_root);
    assert_contains_fragments(&artifact_root.join("src").join("lib.rs"), "library_lib_rs.fragments")?;
    assert_contains_fragments(
        &artifact_root.join("src").join("widgets.rs"),
        "library_widgets_rs.fragments",
    )?;

    let manifest = LibraryManifest::read_from_path(&artifact_root.join("artifact_widgets_core.incnlib"))?;
    assert_eq!(manifest.name, "artifact_widgets_core");
    assert_eq!(manifest.version, "4.5.6");
    let semantic_source_digest = manifest
        .contract_metadata
        .provider
        .semantic_source_digest
        .as_deref()
        .ok_or("generated provider manifest omitted its authored semantic source digest")?;
    assert!(
        semantic_source_digest.starts_with("sha256:") && semantic_source_digest.len() == 71,
        "generated provider manifest carried an invalid authored semantic source digest: {semantic_source_digest}"
    );
    assert!(
        manifest.exports.models.iter().any(|model| model.name == "Widget"),
        "generated .incnlib should export Widget, got {:#?}",
        manifest.exports.models
    );
    assert!(
        manifest
            .exports
            .functions
            .iter()
            .any(|function| function.name == "make_widget"),
        "generated .incnlib should export make_widget, got {:#?}",
        manifest.exports.functions
    );

    let cargo_toml = read_cargo_toml(&artifact_root.join("Cargo.toml"))?;
    let package = toml_table_at(&cargo_toml, "package")?;
    assert_eq!(toml_string_at(package, "name")?, "artifact_widgets_core");
    assert_eq!(toml_string_at(package, "version")?, manifest.version);
    assert_eq!(toml_string_at(package, "license")?, "MIT OR Apache-2.0");
    assert!(
        package.get("license-file").is_none(),
        "Incan's plural license-files metadata must not become Cargo's singular license-file field"
    );
    assert_eq!(
        toml_string_at(toml_table_at(&cargo_toml, "lib")?, "path")?,
        "src/lib.rs"
    );

    let consumer_root = tmp.path().join("artifact_consumer_project");
    let consumer_src = consumer_root.join("src");
    fs::create_dir_all(&consumer_src)?;
    fs::write(
        consumer_root.join("incan.toml"),
        "[project]\nname = \"artifact_consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nwidgets = { path = \"../artifact_widgets_project\" }\n",
    )?;
    write_fixture(&consumer_src.join("main.incn"), "consumer_main.incn")?;

    let out_dir = consumer_root.join("out");
    let main_arg = consumer_src
        .join("main.incn")
        .to_str()
        .ok_or("consumer source path was not valid UTF-8")?
        .to_string();
    let out_arg = out_dir
        .to_str()
        .ok_or("consumer output path was not valid UTF-8")?
        .to_string();
    let consumer_build = run_incan(&consumer_root, &["build", &main_arg, &out_arg])?;
    assert_success(&consumer_build, "incan build pub dependency consumer artifact");

    assert_required_files(&out_dir, "consumer_required_files.txt")?;
    assert_no_cargo_lock(&out_dir);
    assert_contains_fragments(&out_dir.join("src").join("main.rs"), "consumer_main_rs.fragments")?;

    let generated_toml = fs::read_to_string(out_dir.join("Cargo.toml"))?;
    assert!(
        generated_toml.contains("[dependencies.widgets]"),
        "expected dependency alias table, got:\n{generated_toml}"
    );
    assert!(
        generated_toml.contains("package = \"artifact_widgets_core\""),
        "expected dependency package mapping, got:\n{generated_toml}"
    );
    assert!(
        generated_toml.contains("path = "),
        "expected path dependency to generated library artifact, got:\n{generated_toml}"
    );

    let generated_main_rs = fs::read_to_string(out_dir.join("src").join("main.rs"))?;
    assert!(
        !generated_main_rs.contains("pub use widgets::Widget as PublicWidget;"),
        "private pub:: alias import should not become a public Rust reexport, got:\n{generated_main_rs}"
    );
    assert!(
        !generated_main_rs.contains("pub use widgets::make_widget;"),
        "private pub:: item import should not become a public Rust reexport, got:\n{generated_main_rs}"
    );

    Ok(())
}

#[test]
fn path_dependency_artifact_rebuilds_for_a_b_a_feature_projections() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let library_root = tmp.path().join("feature_library");
    let library_src = library_root.join("src");
    fs::create_dir_all(&library_src)?;
    fs::write(
        library_root.join("incan.toml"),
        r#"[project]
name = "feature_library"
version = "0.1.0"

[project.features]
alpha = []
beta = []
"#,
    )?;
    fs::write(
        library_src.join("lib.incn"),
        r#"when feature("alpha"):
    pub def alpha_value() -> str:
        return "alpha"

when feature("beta"):
    pub def beta_value() -> str:
        return "beta"
"#,
    )?;

    let consumer_root = tmp.path().join("feature_consumer");
    let consumer_src = consumer_root.join("src");
    fs::create_dir_all(&consumer_src)?;
    fs::write(
        consumer_root.join("incan.toml"),
        r#"[project]
name = "feature_consumer"
version = "0.1.0"

[project.features]
alpha = ["feature_library/alpha"]
beta = ["feature_library/beta"]

[dependencies]
feature_library = { path = "../feature_library", default-features = false }
"#,
    )?;
    let main_path = consumer_src.join("main.incn");
    fs::write(
        &main_path,
        r#"from pub::feature_library import alpha_value

def main() -> None:
    println(alpha_value())
"#,
    )?;
    let main_arg = main_path
        .to_str()
        .ok_or("consumer source path was not valid UTF-8")?
        .to_string();
    let manifest_path = library_root.join("target/lib/feature_library.incnlib");
    let generated_library_path = library_root.join("target/lib/src/lib.rs");

    let default_bake = run_explicit_oven_bake(&library_root, &[])?;
    assert_success(
        &default_bake,
        "explicit Oven bake for the default provider feature projection",
    );

    let disabled_output_dir = consumer_root.join("out_disabled");
    let disabled_output_arg = disabled_output_dir
        .to_str()
        .ok_or("disabled-feature output path was not valid UTF-8")?
        .to_string();
    let disabled = run_incan(
        &consumer_root,
        &["build", "--no-default-features", &main_arg, &disabled_output_arg],
    )?;
    assert!(
        !disabled.status.success(),
        "disabled dependency feature should reject its gated export"
    );
    let disabled_stderr = String::from_utf8_lossy(&disabled.stderr);
    assert!(
        disabled_stderr.contains("`alpha_value` from `pub::feature_library` requires disabled package feature(s)")
            && disabled_stderr.contains("features = [\"alpha\"]"),
        "expected package-feature remedy rather than a generic missing export:\n{disabled_stderr}"
    );

    for (feature, expected, unexpected) in [
        ("alpha", "\"alpha\".to_string()", "\"beta\".to_string()"),
        ("beta", "\"beta\".to_string()", "\"alpha\".to_string()"),
        ("alpha", "\"alpha\".to_string()", "\"beta\".to_string()"),
    ] {
        let provider_bake = run_explicit_oven_bake(&library_root, &["--no-default-features", "--features", feature])?;
        assert_success(
            &provider_bake,
            &format!("explicit Oven bake for provider feature {feature}"),
        );
        fs::write(
            &main_path,
            format!(
                "from pub::feature_library import {feature}_value\n\ndef main() -> None:\n    println({feature}_value())\n"
            ),
        )?;
        let output_dir = consumer_root.join(format!("out_{feature}"));
        let output_arg = output_dir
            .to_str()
            .ok_or("consumer output path was not valid UTF-8")?
            .to_string();
        let output = run_incan(
            &consumer_root,
            &[
                "build",
                "--no-default-features",
                "--features",
                feature,
                &main_arg,
                &output_arg,
            ],
        )?;
        assert_success(
            &output,
            &format!("incan build consumer with dependency feature {feature}"),
        );

        let artifact = LibraryManifest::read_from_path(&manifest_path)?;
        assert_eq!(
            artifact.contract_metadata.provider.active_features,
            std::collections::BTreeSet::from([feature.to_string()]),
            "dependency artifact must carry the exact active feature projection"
        );
        for conditioned_feature in ["alpha", "beta"] {
            assert!(
                artifact
                    .contract_metadata
                    .provider
                    .fact_requirements
                    .iter()
                    .any(|requirement| {
                        requirement.identity == format!("main::{conditioned_feature}_value")
                            && requirement.required_features
                                == std::collections::BTreeSet::from([conditioned_feature.to_string()])
                    }),
                "artifact projection `{feature}` must preserve the inactive `{conditioned_feature}` fact for inspection"
            );
        }
        let generated_library = fs::read_to_string(&generated_library_path)?;
        assert!(
            generated_library.contains(expected),
            "feature `{feature}` dependency artifact should contain `{expected}`:\n{generated_library}"
        );
        assert!(
            !generated_library.contains(unexpected),
            "feature `{feature}` dependency artifact retained stale projection `{unexpected}`:\n{generated_library}"
        );
    }

    Ok(())
}

/// A package that calls between its own modules must project the callee's emitted name.
///
/// Regression for #1174. A package origin says which library *declares* a method; it does not say the method is
/// foreign to this build. Reading it as the latter made a package's own build treat its own declarations as imported
/// and emit a raw `c.bumped()` where the wrapper it was itself emitting is named `__incan_v1_…`. Building
/// `stdlib-core` then failed with 67 lowering errors, because the checked registry lowering requires the projected
/// name.
///
/// Deliberately a generated-Rust assertion rather than a build-to-completion one: `incan build --lib` emits the
/// generated project *before* the dependency check stops it, so this costs about a second and needs no Oven bake.
/// That matters, because the defect is otherwise reachable only through a cold SDK component build -- minutes of
/// work that neither a local suite nor a gated development pull request performs.
#[test]
fn a_package_projects_a_call_into_its_own_sibling_module() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("pkgprobe");
    let src = root.join("src");
    fs::create_dir_all(&src)?;
    fs::write(
        root.join("incan.toml"),
        "[project]\nname = \"pkgprobe\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        src.join("types.incn"),
        "pub model Counter:\n    pub value: int\n\n    def bumped(self) -> int:\n        return self.value + 1\n",
    )?;
    fs::write(
        src.join("lib.incn"),
        "from types import Counter\n\npub def probe() -> int:\n  c = Counter(value=41)\n  return c.bumped()\n",
    )?;

    // The command stops at the dependency check, which is expected and not what this pins. The generated project is
    // already written by then, and it is the artifact under test.
    let _ = run_incan(&root, &["build", "--lib", "src/lib.incn"])?;

    let generated = fs::read_to_string(root.join("target/lib/src/lib.rs"))?;
    assert!(
        !generated.contains("c.bumped()"),
        "a sibling module's method must not be called by its source spelling:\n{generated}"
    );
    assert!(
        generated.contains("__incan_v1_"),
        "the call must name the projected wrapper this build emits:\n{generated}"
    );
    Ok(())
}
