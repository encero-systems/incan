//! Rust interop regressions driven through the CLI: receiver borrowing, generic scenarios, and metadata-free inference.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

use std::fs;

use incan_test_support as support;

use incan_test_support::cli_project;

use cli_project::*;

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

#[test]
fn rust_generic_interop_scenarios_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_generic_rust_param_scenarios",
        r#"

[rust-dependencies]
arc_callback = { path = "rust/arc_callback" }
generic_helpers = { path = "rust/generic_helpers" }
prost = { path = "rust/prost" }
prost-types = { path = "rust/prost-types" }
reexport_identity = { path = "rust/reexport_identity" }
stream_host = { path = "rust/stream_host" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from arc_callback import arc_callback_case, match_arm_callback_case
from borrowed_generic import borrowed_generic_case
from by_value_decode import by_value_decode_case
from cross_crate_decode import cross_crate_decode_case
from method_arity import method_arity_case
from reexport_identity import reexport_identity_case
from trait_by_value_decode import trait_by_value_decode_case

def main() -> None:
  println(arc_callback_case())
  println(match_arm_callback_case())
  println(borrowed_generic_case())
  println(by_value_decode_case())
  println(trait_by_value_decode_case())
  println(cross_crate_decode_case())
  println(reexport_identity_case())
  method_arity_case()
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("arc_callback.incn"),
        r#"from rust::arc_callback import CallbackError, ColumnarValue, DataType, ScalarFunctionImplementation, ScalarUDF, SliceCallback, Volatility, create_simple_udf, create_udf, create_udf_full
from rust::std::sync import Arc

def callback(args: list[ColumnarValue]) -> Result[ColumnarValue, CallbackError]:
  return Ok(args[0].clone())

def inline_arc_callback_value() -> int:
  match create_simple_udf(callback=Arc.from((args) => callback(args.to_vec())), name="inline"):
    Ok(value) => return value.value()
    Err(_) => return -1

def inline_datafusion_shaped_callback_value() -> int:
  match create_udf_full(
    name="sha1",
    input_types=[DataType.Utf8],
    return_type=DataType.Utf8,
    volatility=Volatility.Immutable,
    fun=Arc.from((args) => callback(args.to_vec())),
  ):
    Ok(value) => return value.value()
    Err(_) => return -1

pub def arc_callback_case() -> str:
  implementation: SliceCallback = Arc.from((args) => callback(args.to_vec()))
  match create_simple_udf(callback=implementation, name="assigned"):
    Ok(value) => return f"arc_callback:{value.value()}:{inline_arc_callback_value()}:{inline_datafusion_shaped_callback_value()}"
    Err(_) => return "arc_callback:err"

@derive(Clone)
enum ReproFunction(str):
  First = "first"
  Second = "second"

def make_udf(function: ReproFunction) -> ScalarUDF:
  match function:
    ReproFunction.First =>
      return create_udf(
        name=function.value(),
        input_types=[DataType.Utf8],
        return_type=DataType.Utf8,
        volatility=Volatility.Immutable,
        fun=Arc.from((args) => callback(args.to_vec())),
      )
    ReproFunction.Second =>
      return create_udf(
        name=function.value(),
        input_types=[DataType.Utf8],
        return_type=DataType.Utf8,
        volatility=Volatility.Immutable,
        fun=Arc.from((args) => callback(args.to_vec())),
      )

pub def match_arm_callback_case() -> str:
  first = make_udf(ReproFunction.First)
  second = make_udf(ReproFunction.Second)
  return f"match-callback:{first.value()}:{second.value()}"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("borrowed_generic.incn"),
        r#"from rust::generic_helpers::borrow import takes_ref

model Payload:
  name: str

pub def borrowed_generic_case() -> str:
  payload = Payload(name="demo")
  return f"borrowed:{takes_ref(payload)}"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("by_value_decode.incn"),
        r#"from rust::generic_helpers::inherent_decode import FileDescriptorSet
from rust::std::io import Cursor

pub def by_value_decode_case() -> str:
  mut cursor = Cursor.new(b"abc")
  match FileDescriptorSet.decode(cursor):
    Ok(_) => return "by_value:ok"
    Err(_) => return "by_value:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("trait_by_value_decode.incn"),
        r#"from rust::generic_helpers::trait_decode import FileDescriptorSet, Message

pub def trait_by_value_decode_case() -> str:
  encoded = b"abc"
  match FileDescriptorSet.decode(encoded.as_slice()):
    Ok(_) => return "trait_by_value:ok"
    Err(_) => return "trait_by_value:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("cross_crate_decode.incn"),
        r#"from rust::prost import Message
from rust::prost_types import FileDescriptorSet, ProducerPlan

pub def cross_crate_decode_case() -> str:
  producer = ProducerPlan.new()
  encoded = producer.encode_to_vec()
  match FileDescriptorSet.decode(encoded):
    Ok(_) => return "cross_crate:ok"
    Err(_) => return "cross_crate:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("reexport_identity.incn"),
        r#"from rust::reexport_identity import Expr as RustExpr, ScalarFunction as RustScalarFunction, registry

pub def reexport_identity_case() -> str:
  state = registry()
  udf = state.udf()
  args: list[RustExpr] = []
  _ = RustExpr.ScalarFunction(RustScalarFunction.new_udf(udf, args))
  return "reexport_identity:ok"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("method_arity.incn"),
        r#"from rust::stream_host import DeviceTrait, OutputCallbackInfo, device

def consume(_value: f32) -> None:
  pass

def write_silence(_data: &mut list[f32], _info: &OutputCallbackInfo) -> None:
  pass

def report_error(_error: str) -> None:
  pass

pub def method_arity_case() -> None:
  stream = device()
  stream.build_output_stream[f32, _, _](1.0, consume, consume)
  println("stream-built")
  stream.run[f32, _, _](write_silence, report_error)
  stream.run[f32, _, _]((_data, _info) => println(len(_data)), report_error)
  println("callbacks-built")
"#,
    )?;
    // Keep this fixture DataFusion-shaped but crate-light. The real DataFusion crate is far too expensive for a
    // compiler regression test; the behavior under test is the Rust metadata shape:
    // `ScalarFunctionImplementation -> SliceCallback -> Arc<dyn Fn(...)>`. The same fixture exercises both
    // assigned/inline callback coercion and #733's match-arm closure context.
    let helper_src = tmp.path().join("rust").join("arc_callback").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("arc_callback src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "arc_callback"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"use std::sync::Arc;

#[derive(Clone)]
pub struct ColumnarValue {
    value: i64,
}

impl ColumnarValue {
    pub fn new(value: i64) -> Self {
        Self { value }
    }

    pub fn value(&self) -> i64 {
        self.value
    }
}

pub struct CallbackError;

pub type SliceCallback = Arc<dyn Fn(&[ColumnarValue]) -> Result<ColumnarValue, CallbackError> + Send + Sync>;
pub type ScalarFunctionImplementation = crate::SliceCallback;

#[derive(Clone)]
pub struct ScalarUDF {
    value: i64,
}

impl ScalarUDF {
    pub fn value(&self) -> i64 {
        self.value
    }
}

#[derive(Clone)]
pub enum DataType {
    Utf8,
}

#[derive(Clone)]
pub enum Volatility {
    Immutable,
}

pub fn invoke(callback: SliceCallback) -> Result<ColumnarValue, CallbackError> {
    let args = vec![ColumnarValue::new(7)];
    callback(&args)
}

pub fn create_simple_udf(name: &str, callback: crate::SliceCallback) -> Result<ColumnarValue, CallbackError> {
    let _ = name;
    let args = vec![ColumnarValue::new(11)];
    callback(&args)
}

pub fn create_udf_full(
    name: &str,
    input_types: Vec<DataType>,
    return_type: DataType,
    volatility: Volatility,
    fun: crate::ScalarFunctionImplementation,
) -> Result<ColumnarValue, CallbackError> {
    let _ = name;
    let _ = input_types;
    let _ = return_type;
    let _ = volatility;
    let args = vec![ColumnarValue::new(13)];
    fun(&args)
}

pub fn create_udf(
    name: &str,
    input_types: Vec<DataType>,
    return_type: DataType,
    volatility: Volatility,
    fun: crate::ScalarFunctionImplementation,
) -> ScalarUDF {
    let _ = name;
    let _ = input_types;
    let _ = return_type;
    let _ = volatility;
    let args = vec![ColumnarValue::new(13)];
    let value = match fun(&args) {
        Ok(value) => value.value(),
        Err(_) => -1,
    };
    ScalarUDF { value }
}
"#,
    )?;
    // These three isolated helper crates used to force separate package and metadata walks in an already
    // single-project regression. Their import routes remain distinct Rust modules, while one fixture crate now owns
    // the shared package boundary.
    let helper_src = tmp.path().join("rust").join("generic_helpers").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "generic_helpers"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub mod borrow {
    pub fn takes_ref<TValue>(_value: &TValue) -> i64 {
        1
    }
}

pub mod inherent_decode {
    pub trait DecodeBuf {}

    impl DecodeBuf for std::io::Cursor<Vec<u8>> {}

    pub struct DecodeError;

    pub struct FileDescriptorSet;

    impl FileDescriptorSet {
        pub fn decode<T: DecodeBuf>(_buf: T) -> Result<Self, DecodeError> {
            Ok(Self)
        }
    }
}

pub mod trait_decode {
    pub trait DecodeBuf {}

    impl DecodeBuf for &[u8] {}

    pub struct DecodeError;

    pub struct FileDescriptorSet;

    pub trait Message: Sized {
        fn decode(_buf: impl DecodeBuf) -> Result<Self, DecodeError>;
    }

    impl Message for FileDescriptorSet {
        fn decode(_buf: impl DecodeBuf) -> Result<Self, DecodeError> {
            Ok(Self)
        }
    }
}
"#,
    )?;
    let prost_src = tmp.path().join("rust").join("prost").join("src");
    fs::create_dir_all(&prost_src)?;
    fs::write(
        prost_src.parent().ok_or("prost src has no parent")?.join("Cargo.toml"),
        r#"[package]
name = "prost"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        prost_src.join("lib.rs"),
        r#"pub trait Buf {}

impl Buf for &[u8] {}

pub struct DecodeError;

pub trait Message: Sized {
    fn decode(_buf: impl Buf) -> Result<Self, DecodeError>;
}
"#,
    )?;
    let prost_types_src = tmp.path().join("rust").join("prost-types").join("src");
    fs::create_dir_all(&prost_types_src)?;
    fs::write(
        prost_types_src
            .parent()
            .ok_or("prost-types src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "prost-types"
version = "0.1.0"
edition = "2021"

[dependencies]
prost = { path = "../prost" }
"#,
    )?;
    fs::write(
        prost_types_src.join("lib.rs"),
        r#"pub struct ProducerPlan;

impl ProducerPlan {
    pub fn new() -> Self {
        Self
    }

    pub fn encode_to_vec(&self) -> Vec<u8> {
        b"abc".to_vec()
    }
}

pub struct FileDescriptorSet;

impl prost::Message for FileDescriptorSet {
    fn decode(_buf: impl prost::Buf) -> Result<Self, prost::DecodeError> {
        Ok(Self)
    }
}
"#,
    )?;
    let reexport_identity_src = tmp.path().join("rust").join("reexport_identity").join("src");
    fs::create_dir_all(&reexport_identity_src)?;
    fs::write(
        reexport_identity_src
            .parent()
            .ok_or("reexport_identity src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "reexport_identity"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        reexport_identity_src.join("lib.rs"),
        r#"use std::sync::Arc;

pub mod udf {
    pub struct ScalarUDF;
}

pub use udf::ScalarUDF;

pub struct FunctionRegistry;

pub fn registry() -> FunctionRegistry {
    FunctionRegistry
}

impl FunctionRegistry {
    pub fn udf(&self) -> Arc<udf::ScalarUDF> {
        Arc::new(udf::ScalarUDF)
    }
}

pub struct Expr;
pub struct ScalarFunction;

impl ScalarFunction {
    pub fn new_udf(_udf: Arc<ScalarUDF>, _args: Vec<Expr>) -> Self {
        Self
    }
}

impl Expr {
    #[allow(non_snake_case)]
    pub fn ScalarFunction(_function: ScalarFunction) -> Self {
        Self
    }
}
"#,
    )?;

    let stream_host_src = tmp.path().join("rust").join("stream_host").join("src");
    fs::create_dir_all(&stream_host_src)?;
    fs::write(
        stream_host_src
            .parent()
            .ok_or("stream host source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "stream_host"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        stream_host_src.join("lib.rs"),
        r#"pub struct Device;
pub struct OutputCallbackInfo;

pub fn device() -> Device {
    Device
}

pub trait DeviceTrait {
    fn build_output_stream<T, D, E>(&self, value: T, data_callback: D, error_callback: E)
    where
        T: Copy,
        D: FnMut(T),
        E: FnMut(T);

    fn run<T, D, E>(&self, data_callback: D, error_callback: E)
    where
        T: Copy + Default,
        D: FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static,
        E: FnMut(String);
}

impl DeviceTrait for Device {
    fn build_output_stream<T, D, E>(&self, value: T, mut data_callback: D, mut error_callback: E)
    where
        T: Copy,
        D: FnMut(T),
        E: FnMut(T),
    {
        data_callback(value);
        error_callback(value);
    }

    fn run<T, D, E>(&self, mut data_callback: D, mut error_callback: E)
    where
        T: Copy + Default,
        D: FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static,
        E: FnMut(String),
    {
        let mut data = [T::default(); 2];
        let info = OutputCallbackInfo;
        data_callback(&mut data, &info);
        error_callback("synthetic callback error".to_string());
    }
}
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for grouped generic Rust interop scenarios",
    );
    let output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&output, "incan run with grouped generic Rust interop scenarios");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "arc_callback:11:11:13\nmatch-callback:13:13\nborrowed:1\nby_value:ok\ntrait_by_value:ok\ncross_crate:ok\nreexport_identity:ok\nstream-built\n2\ncallbacks-built",
        "expected grouped generic Rust interop output, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn rust_method_into_bound_keeps_string_argument_inferable_issue804() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_rust_method_into_bound",
        r#"

[rust-dependencies.into_method_helper]
path = "rust/into_method_helper"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::into_method_helper import Tokenizer

def main() -> None:
  tokenizer = Tokenizer.new()
  println(tokenizer.encode("hello world", false))
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("into_method_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("into_method_helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "into_method_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub struct Tokenizer;

impl Tokenizer {
    pub fn new() -> Self {
        Self
    }

    pub fn encode<E: Into<String>>(&self, input: E, uppercase: bool) -> String {
        let text = input.into();
        if uppercase {
            text.to_uppercase()
        } else {
            text
        }
    }
}
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for a Rust Into-bound method argument");
    let output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&output, "incan run with a Rust Into-bound method argument");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "hello world",
        "Rust Into-bound method should receive the original string type"
    );

    let generated = fs::read_to_string(tmp.path().join("target/incan/cli_rust_method_into_bound/src/main.rs"))?;
    assert!(
        generated.contains("tokenizer.encode(\"hello world\", false)"),
        "unresolved Rust method generic should preserve the string literal shape, got:\n{generated}"
    );
    assert!(
        !generated.contains("tokenizer.encode(\"hello world\".into(), false)"),
        "unresolved Rust method generic must not emit an ambiguous `.into()`, got:\n{generated}"
    );
    Ok(())
}

#[test]
fn build_combined_rust_and_source_imports_preserves_never_return_issue381() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "combined_rust_and_source_imports_preserve_never_return",
        r#"

[rust-dependencies]
polyglot_probe = { path = "rust/polyglot_probe" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::polyglot_probe import DialectType
from prism import PrismCursor


def main() -> None:
    pass
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("prism.incn"),
        r#"from rust::incan_std_core::errors import raise_value_error
from rust::std::primitive import i32 as RustI32


pub model PrismCursor:
    pub offset: int


def fail_to_lower() -> RustI32:
    return raise_value_error("cannot lower cursor")
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("polyglot_probe").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("polyglot probe source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "polyglot_probe"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub enum DialectType {
    PostgreSql,
}
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for combined Rust and source imports");
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "generated Rust for combined imports with a diverging Rust helper",
    );
    Ok(())
}

/// Ensures f-string values compile through both borrowed and owned Rust interop boundaries.
#[test]
fn build_inline_fstring_rust_interop_variants_issue716() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tiny_error");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tiny_error\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub enum TinyError {
    Execution(String),
}

pub fn consume(err: TinyError) -> i64 {
    match err {
        TinyError::Execution(message) => message.len() as i64,
    }
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "inline_fstring_rust_interop_variants_issue716",
        r#"
[rust-dependencies]
tiny_error = { path = "rust/tiny_error" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::incan_std_core::errors import raise_value_error
from rust::tiny_error import TinyError, consume


def fail_inline(value: str) -> int:
    return raise_value_error(f"bad value `{value}`")


def fail_local(value: str) -> int:
    message = f"bad value `{value}`"
    return raise_value_error(message)


def make_error(value: str) -> int:
    return consume(TinyError.Execution(f"bad value `{value}`"))


def main() -> None:
    println(str(make_error("x")))
    fail_inline("x")
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for inline f-string Rust interop variants",
    );
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for inline f-string Rust &str and String enum variants issue716",
    );
    Ok(())
}

#[test]
fn build_static_str_const_rust_string_struct_field() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tiny_option");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tiny_option\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub struct FunctionOption {
    pub name: String,
    pub enabled: bool,
}

pub fn option_name(option: FunctionOption) -> String {
    option.name
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "static_str_const_rust_string_struct_field",
        r#"
[rust-dependencies]
tiny_option = { path = "rust/tiny_option" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tiny_option import FunctionOption, option_name


pub const OPTION_NAME: str = "sketch_family"


def main() -> None:
    option = FunctionOption(name=OPTION_NAME, enabled=True)
    println(option_name(option))
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for a static str const in a Rust String field",
    );
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for static str const into Rust String struct field",
    );
    Ok(())
}

#[test]
fn build_metadata_free_into_bound_tokenizer_encode_issue804() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tokenizers");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tokenizers\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub struct Tokenizer;

pub struct EncodeInput<'a>(&'a str);

impl<'a> From<&'a str> for EncodeInput<'a> {
    fn from(value: &'a str) -> Self {
        Self(value)
    }
}

impl Tokenizer {
    pub fn new() -> Self {
        Self
    }

    pub fn encode<'a, E>(&self, value: E, _add_special_tokens: bool) -> Result<(), ()>
    where
        E: Into<EncodeInput<'a>>,
    {
        let _ = value.into();
        Ok(())
    }
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "metadata_free_into_bound_tokenizer_encode_issue804",
        r#"
[rust-dependencies]
tokenizers = { path = "rust/tokenizers" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tokenizers import Tokenizer

def main() -> None:
    tokenizer = Tokenizer.new()
    literal = tokenizer.encode("literal", False)
    text = "variable"
    variable = tokenizer.encode(text, False)
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for metadata-free Into-bound tokenizer encode",
    );
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for metadata-free Into-bound tokenizer encode issue804",
    );
    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/metadata_free_into_bound_tokenizer_encode_issue804/src/main.rs"),
    )?;
    assert!(
        generated.contains("tokenizer.encode(\"literal\", false)"),
        "literal must preserve its direct &str shape, got:\n{generated}"
    );
    assert!(
        generated.contains("tokenizer.encode((text).as_str(), false)"),
        "owned Incan strings must become &str for the Into-bound method, got:\n{generated}"
    );
    Ok(())
}

/// A comprehension over a by-value Rust iterator (`std::env::Args` has no `.iter()`) consumes it exactly as the
/// `for` statement over the same value does, and `list(args())` collects it the same way (#1490, #1464).
#[test]
fn comprehension_over_rust_iterator_consumes_it_by_value_issue1490() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "comprehension_rust_iterator_issue1490", "")?;
    fs::write(
        &main_path,
        r#"from rust::std::env import args


def lengths() -> list[int]:
    return [len(argument) for argument in args()]


def present_arguments() -> list[str]:
    return [argument for argument in args() if len(argument) > 0]


pub def main() -> None:
    arguments: list[str] = [argument for argument in args()]
    println(f"{len(arguments)}")
    println(len(lengths()))
    println(len(present_arguments()))
    collected = list(args())
    println(len(collected))
    for argument in args():
        println(len(argument) > 0)
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for the #1490 comprehension program");
    let run_output = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run_output, "incan run for the #1490 comprehension program");
    assert_eq!(
        String::from_utf8(run_output.stdout)?.lines().collect::<Vec<_>>(),
        vec!["1", "1", "1", "1", "true"],
        "every argument-vector read must see exactly the program name"
    );

    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/comprehension_rust_iterator_issue1490/src/main.rs"),
    )?;
    assert!(
        !generated.contains("(args()).iter()") && !generated.contains("(args()).clone()"),
        "a by-value Rust iterator must be neither borrowed with .iter() nor cloned:\n{generated}"
    );
    assert!(
        generated.contains("((args()).into_iter())"),
        "the comprehension must consume the iterator through IntoIterator:\n{generated}"
    );
    Ok(())
}
