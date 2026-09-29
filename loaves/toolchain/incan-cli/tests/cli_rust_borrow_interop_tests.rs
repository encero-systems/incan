//! Rust interop regressions driven through the CLI: receiver borrowing, generic scenarios, and metadata-free inference.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_rust_interop_tests_root.rs");

#[test]
fn build_typed_web_extractors_and_scalar_captures_issue867() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "typed_web_extractors", "")?;
    fs::write(
        &main_path,
        r#"import api::routes
from std.web import App

def main() -> None:
  App.run(host="127.0.0.1", port=0)
"#,
    )?;
    let api_dir = tmp.path().join("src/api");
    fs::create_dir_all(&api_dir)?;
    fs::write(
        api_dir.join("routes.incn"),
        r#"from std.web import route, Json, Query, Path, GET, POST
from std.serde import json
import std.async

@derive(json)
model Search:
  q: str

@derive(json)
model Update:
  name: str

@derive(json)
model Reply:
  value: str

@route("/search", methods=[GET])
async def search(query: Query[Search]) -> Json[Reply]:
  return Json(Reply(value=query.q))

@route("/json", methods=[POST])
async def create(body: Json[Update]) -> Json[Reply]:
  return Json(Reply(value=body.name))

@route("/typed/{id}", methods=[GET])
async def typed_path(_: Path[int]) -> Json[Reply]:
  return Json(Reply(value="typed"))

@route("/scalar/{id}", methods=[GET])
async def scalar_path(id: int) -> Json[Reply]:
  return Json(Reply(value=f"{id}"))

@route("/multi/{year}/{month}", methods=[GET])
async def multiple_paths(year: int, month: int) -> Json[Reply]:
  return Json(Reply(value=f"{year}-{month}"))

@route("/mixed/{id}", methods=[POST])
async def mixed(id: int, _query: Query[Search], _body: Json[Update]) -> Json[Reply]:
  return Json(Reply(value=f"{id}"))

@route("/methods", methods=[GET, POST])
async def multiple_methods() -> Json[Reply]:
  return Json(Reply(value="methods"))
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["build", main_path.to_string_lossy().as_ref(), "--offline"],
    )?;
    assert_success(&output, "typed web extractor build");

    let generated_root = tmp.path().join("target/incan/typed_web_extractors/src");
    let generated_main = fs::read_to_string(generated_root.join("main.rs"))?;
    let generated_routes = fs::read_to_string(generated_root.join("api/routes.rs"))?;
    let generated = format!("{generated_main}\n{generated_routes}");
    let compact_generated = generated
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(
        generated.contains("\"/typed/{id}\""),
        "generated route must retain Axum 0.8 capture syntax"
    );
    assert!(
        compact_generated.contains("Query<Search>") && compact_generated.contains("Json<Update>"),
        "generated typed request extractors must retain their concrete types"
    );
    assert!(
        generated.contains("\"/multi/{year}/{month}\"")
            && !compact_generated.contains("Query<_>")
            && !compact_generated.contains("Json<_>"),
        "generated multiple captures must retain Axum 0.8 syntax without inferred item signatures"
    );
    Ok(())
}

#[test]
fn rust_std_io_trait_interop_borrows_receivers_and_propagates_results_issues878_888()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "rust_std_io_trait_interop",
        r#"

[rust-dependencies]
console_interop = { path = "rust/console_interop" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::std::io import Error as IoError, Read, stdin, stdout
from rust::console_interop import EnterAlternateScreen, ExecutableCommand

def enter_alternate_screen() -> Result[None, IoError]:
    mut output = stdout()
    _ = output.execute(EnterAlternateScreen)?
    return Ok(None)

def inspect_alternate_screen_result() -> None:
    mut output = stdout()
    match output.execute(EnterAlternateScreen):
        Ok(_) => pass
        Err(_) => pass

def main() -> None:
    mut input = stdin()
    _ = Read.by_ref(input)
    inspect_alternate_screen_result()
    match enter_alternate_screen():
        Ok(_) => pass
        Err(_) => pass
"#,
    )?;
    let helper_src = tmp.path().join("rust/console_interop/src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("console interop source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "console_interop"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"use std::io::{self, Write};

pub struct EnterAlternateScreen;

pub trait Command {}

impl Command for EnterAlternateScreen {}

pub trait ExecutableCommand {
    fn execute(&mut self, command: impl Command) -> io::Result<&mut Self>;
}

impl<W: Write + ?Sized> ExecutableCommand for W {
    fn execute(&mut self, _command: impl Command) -> io::Result<&mut Self> {
        Ok(self)
    }
}
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("non-utf8 main path")?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for direct std-I/O trait interop");
    let build_output = run_incan(tmp.path(), &["build", main_arg, "--locked"])?;
    assert_success(&build_output, "incan build for direct std-I/O trait interop");

    let generated = fs::read_to_string(tmp.path().join("target/incan/rust_std_io_trait_interop/src/main.rs"))?;
    assert!(
        generated.contains("Read::by_ref(&mut input)"),
        "owned Rust trait receiver must be borrowed without dereference:\n{generated}"
    );
    assert!(
        !generated.contains("Read::by_ref(&mut *input)"),
        "owned Rust trait receiver must not use the guard reborrow shape:\n{generated}"
    );
    let compact = generated.split_whitespace().collect::<String>();
    assert!(
        compact.contains("output.execute(EnterAlternateScreen)?"),
        "generated Rust must preserve the fallible extension-trait call:\n{generated}"
    );
    assert!(
        compact.contains("matchoutput.execute(EnterAlternateScreen)"),
        "generated Rust must preserve direct Result inspection for the extension-trait call:\n{generated}"
    );
    Ok(())
}

#[test]
fn rust_trait_object_method_arguments_borrow_by_metadata_issue832() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "rust_trait_object_borrow_arguments",
        r#"

[rust-dependencies]
duck_adapter = { path = "rust/duck_adapter" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::duck_adapter import InterleavedOwned, Processor

def main() -> None:
  mut processor = Processor.new()
  input_frames: usize = 3
  empty_frames: usize = 0
  input = InterleavedOwned.new(input_frames)
  mut output = InterleavedOwned.new(empty_frames)
  println(processor.process_into_buffer(input, output))
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("duck_adapter").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("duck adapter source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "duck_adapter"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub trait Adapter {
    fn frames(&self) -> usize;
}

pub trait AdapterMut: Adapter {
    fn set_frames(&mut self, frames: usize);
}

pub struct InterleavedOwned {
    frames: usize,
}

impl InterleavedOwned {
    pub fn new(frames: usize) -> Self {
        Self { frames }
    }
}

impl Adapter for InterleavedOwned {
    fn frames(&self) -> usize {
        self.frames
    }
}

impl AdapterMut for InterleavedOwned {
    fn set_frames(&mut self, frames: usize) {
        self.frames = frames;
    }
}

pub struct Processor;

impl Processor {
    pub fn new() -> Self {
        Self
    }

    pub fn process_into_buffer(&mut self, input: &dyn Adapter, output: &mut dyn AdapterMut) -> usize {
        output.set_frames(input.frames());
        output.frames()
    }
}
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for Rust trait-object method argument borrowing",
    );
    let output = run_incan(tmp.path(), &["run"])?;
    assert_success(&output, "Rust trait-object method argument borrowing");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3");

    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/rust_trait_object_borrow_arguments/src/main.rs"),
    )?;
    let compact_generated: String = generated.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(
        compact_generated.contains("process_into_buffer(&input,&mutoutput)"),
        "trait-object argument borrows must survive generated Rust:\n{generated}"
    );
    Ok(())
}

#[test]
fn rust_concrete_reference_arguments_borrow_by_metadata_issue861() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "rust_concrete_reference_arguments",
        r#"

[rust-dependencies]
mut_ref_probe = { path = "rust/mut_ref_probe" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::mut_ref_probe import Header, Writer

def main() -> None:
  mut writer = Writer.new()
  mut header = Header.new()
  println(writer.mutate(header, 1))
  println(writer.view_value(header))
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("mut_ref_probe").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("mutable-reference probe source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "mut_ref_probe"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub struct Header {
    value: usize,
}

impl Header {
    pub fn new() -> Self {
        Self { value: 0 }
    }
}

pub mod writer {
    use super::Header;

    pub struct Writer;

    impl Writer {
        pub fn new() -> Self {
            Self
        }

        pub fn mutate<T>(&mut self, header: &mut Header, _value: T) -> usize {
            header.value += 1;
            header.value
        }

        pub fn view_value(&self, header: &Header) -> usize {
            header.value
        }
    }
}

pub use writer::Writer;
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for Rust concrete-reference argument borrowing",
    );
    let output = run_incan(tmp.path(), &["run"])?;
    assert_success(&output, "Rust concrete-reference argument borrowing");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n1\n");

    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/rust_concrete_reference_arguments/src/main.rs"),
    )?;
    let compact_generated: String = generated.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(
        compact_generated.contains("writer.mutate(&mutheader,1)"),
        "generic method concrete mutable-reference argument must preserve its generated Rust borrow:\n{generated}"
    );
    assert!(
        compact_generated.contains("writer.view_value(&header)"),
        "concrete shared-reference argument must preserve its generated Rust borrow:\n{generated}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_std_result_and_contextual_f32_interop_compile_together_issues801_802() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "result_interop_probe"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    let source_path = src_dir.join("main.incn");
    fs::write(
        &source_path,
        r#"from rust::std::fs import metadata
from rust::std::io import Error as IoError
from rust::std::path import Path as RustPath

pub def file_len(path: str) -> Result[int, IoError]:
  meta = metadata(RustPath.new(path))?
  return Ok(int(meta.len()))

def accepts_f32(value: f32) -> None:
  print("ok")

def main() -> None:
  result = file_len("loaf.toml")
  zero: f32 = 0.0
  accepts_f32(1.5)
  print("checked")
"#,
    )?;
    let source_arg = source_path.to_str().ok_or("source path was not valid UTF-8")?;

    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "explicit Oven bake for std Result and contextual f32 interop");
    let build = run_incan(tmp.path(), &["build", source_arg, "--offline"])?;
    assert_success(
        &build,
        "incan build should emit Rust for std::fs::metadata try operator and contextual f32 literals",
    );
    let generated = fs::read_to_string(tmp.path().join("target/incan/result_interop_probe/src/main.rs"))?;
    assert!(
        !generated.contains("0f64") && !generated.contains("1.5f64"),
        "contextual float literals should not be hard-suffixed as f64:\n{generated}"
    );

    Ok(())
}

#[test]
fn cold_library_build_preserves_rust_string_compound_assignment_issue896() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let stdlib_crate = support::repo_root().join(oven_model::toolchain_layout::development_support_crate_dir(
        "incan_std_core",
    ));
    let stdlib_path = stdlib_crate.to_string_lossy().replace('\\', "\\\\");
    let _main_path = write_minimal_project(
        tmp.path(),
        "cold_rust_string_compound_assignment",
        &format!(
            r#"
[sdk]
profile = "minimal"

[rust-dependencies.incan_std_core]
path = "{stdlib_path}"
"#,
        ),
    )?;
    fs::write(
        tmp.path().join("src/lib.incn"),
        r#"from rust::incan_std_core::strings import str_slice_byte_range


pub def append_range(text: str, start: int, end: int) -> str:
  mut out = ""
  out += str_slice_byte_range(text, start, end)
  return out


pub def join_ranges(text: str, start: int, middle: int, end: int) -> str:
  return str_slice_byte_range(text, start, middle) + str_slice_byte_range(text, middle, end)
"#,
    )?;

    assert!(
        !tmp.path().join("target").exists(),
        "the regression must begin without a project-local Rust metadata cache"
    );
    let build = run_incan_with_env(tmp.path(), &["build", "--lib"], &[("INCAN_RUST_INSPECT_PREWARM", "0")])?;
    assert_success(
        &build,
        "cold library build with a direct Rust String compound assignment",
    );

    let generated = fs::read_to_string(tmp.path().join("target/lib/src/lib.rs"))?;
    let compact_generated = generated.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact_generated.contains("out=incan_std_core::strings::str_concat(")
            && compact_generated.contains("&str_slice_byte_range(&text,start,end),"),
        "cold Rust metadata must select string-aware compound-assignment lowering:\n{generated}"
    );
    assert!(
        !compact_generated.contains("out=out+str_slice_byte_range"),
        "a direct Rust String result must not reach generated Rust's owned `String + String` path:\n{generated}"
    );
    assert!(
        compact_generated.contains("incan_std_core::strings::str_concat(&str_slice_byte_range(&text,start,middle),&str_slice_byte_range(&text,middle,end),)"),
        "binary concatenation of direct Rust String results must use the string helper:\n{generated}"
    );
    Ok(())
}
