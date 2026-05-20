use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incan::library_manifest::LibraryManifest;

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

fn run_incan(current_dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(incan_binary())
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .output()?)
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

fn assert_contains_fragments(path: &Path, fixture: &str) -> Result<(), Box<dyn std::error::Error>> {
    let actual = fs::read_to_string(path)?;
    let fragments = read_fixture(fixture)?;
    for fragment in fragments.split("\n---\n") {
        let fragment = fragment.trim_matches('\n');
        if fragment.trim().is_empty() {
            continue;
        }
        assert!(
            actual.contains(fragment),
            "expected `{}` to contain fragment:\n{}\n\nactual:\n{}",
            path.display(),
            fragment,
            actual
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
        "[project]\nname = \"artifact_app_baseline\"\nversion = \"0.1.0\"\n",
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
    assert_success(&output, "incan build application artifact");

    assert_required_files(&out_dir, "app_required_files.txt")?;
    assert_contains_fragments(&out_dir.join("src").join("main.rs"), "app_main_rs.fragments")?;

    let cargo_toml = read_cargo_toml(&out_dir.join("Cargo.toml"))?;
    let package = toml_table_at(&cargo_toml, "package")?;
    assert_eq!(toml_string_at(package, "name")?, "artifact_app_baseline");
    assert_eq!(toml_string_at(package, "edition")?, "2021");
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
fn generated_library_artifact_matches_baseline() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path().join("artifact_widgets_project");
    let src_dir = project_root.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_root.join("incan.toml"),
        "[project]\nname = \"artifact_widgets_core\"\nversion = \"0.1.0\"\n",
    )?;
    write_fixture(&src_dir.join("widgets.incn"), "library_widgets.incn")?;
    write_fixture(&src_dir.join("lib.incn"), "library_lib.incn")?;

    let output = run_incan(&project_root, &["build", "--lib"])?;
    assert_success(&output, "incan build --lib artifact");

    let artifact_root = project_root.join("target").join("lib");
    assert_required_files(&artifact_root, "library_required_files.txt")?;
    assert_contains_fragments(&artifact_root.join("src").join("lib.rs"), "library_lib_rs.fragments")?;
    assert_contains_fragments(
        &artifact_root.join("src").join("widgets.rs"),
        "library_widgets_rs.fragments",
    )?;

    let manifest = LibraryManifest::read_from_path(&artifact_root.join("artifact_widgets_core.incnlib"))?;
    assert_eq!(manifest.name, "artifact_widgets_core");
    assert_eq!(manifest.version, "0.1.0");
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
    assert_eq!(
        toml_string_at(toml_table_at(&cargo_toml, "package")?, "name")?,
        "artifact_widgets_core"
    );
    assert_eq!(
        toml_string_at(toml_table_at(&cargo_toml, "lib")?, "path")?,
        "src/lib.rs"
    );

    Ok(())
}

#[test]
fn generated_pub_dependency_consumer_artifact_matches_baseline() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("artifact_widgets_project");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        "[project]\nname = \"artifact_widgets_core\"\nversion = \"0.1.0\"\n",
    )?;
    write_fixture(&producer_src.join("widgets.incn"), "library_widgets.incn")?;
    write_fixture(&producer_src.join("lib.incn"), "library_lib.incn")?;

    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(&producer_build, "incan build --lib producer artifact");

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
