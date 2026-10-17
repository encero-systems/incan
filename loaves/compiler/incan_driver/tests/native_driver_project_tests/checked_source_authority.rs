//! Selected dependency values and defaults survive ordinary publication without ambient source rediscovery.

use super::{Command, corpus, driver_fixture, fs, success, support};

/// Publish a real dependency and an intermediate library through the ordinary preparation path, then execute that
/// intermediate library's retained Body IR. Values and nominal defaults must come from its selected dependency.
#[test]
fn ordinary_library_publication_retains_selected_values_and_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("checked-source-authority")?;
    let numbers = root.join("deps/numbers");
    let bridge = root.join("deps/bridge");
    for package in [&numbers, &bridge] {
        fs::create_dir_all(package.join("src"))?;
    }
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        numbers.join("loaf.toml"),
        "[project]\nname = 'numbers'\nversion = '0.1.0'\n",
    )?;
    fs::write(
        numbers.join("src/lib.incn"),
        "pub const VALUE: float = 3.25\npub model Box:\n    pub value: float = VALUE\npub type Choice = Box | str\npub def adjusted(offset: float = VALUE) -> float:\n    return offset\n",
    )?;
    fs::write(
        bridge.join("loaf.toml"),
        "[project]\nname = 'bridge'\nversion = '0.1.0'\n[dependencies]\nnumbers = { path = '../numbers' }\n",
    )?;
    fs::write(
        bridge.join("src/lib.incn"),
        "from pub::numbers import VALUE, adjusted, Box\npub from alias_only import alias_value\npub def value() -> float:\n    return VALUE\npub def defaulted() -> float:\n    return adjusted()\npub def boxed() -> float:\n    return Box().value\n",
    )?;
    fs::write(
        bridge.join("src/alias_only.incn"),
        "from pub::numbers import Choice\npub def alias_value(value: Choice) -> Choice:\n    return value\n",
    )?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = 'checked_source_consumer'\nversion = '0.1.0'\n[dependencies]\nbridge = { path = 'deps/bridge' }\n",
    )?;
    let source = root.join("src/main.incn");
    fs::write(
        &source,
        "from pub::bridge import value, defaulted, boxed, alias_value\ndef main() -> None:\n    println(value())\n    println(defaulted())\n    println(boxed())\n    chosen = alias_value(\"selected\")\n    if isinstance(chosen, str):\n        println(chosen)\n",
    )?;
    for package in [&numbers, &bridge] {
        let mut publish = support::cli_project::configured_incan_command(package, &["oven", "bake", "--project", "."]);
        support::configure_explicit_oven_bake_command(&mut publish)?;
        success(&publish.output()?, "checked dependency publication");
    }
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .current_dir(&root)
        .output()?,
        "checked dependency direct compilation",
    );
    let actual = Command::new(native).output()?;
    success(&actual, "checked dependency direct execution");
    assert_eq!(actual.stdout, b"3.25\n3.25\n3.25\nselected\n");
    assert!(actual.stderr.is_empty());
    Ok(())
}
