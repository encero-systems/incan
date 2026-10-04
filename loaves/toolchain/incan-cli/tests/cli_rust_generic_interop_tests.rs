#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Rust interop regressions driven through the CLI: receiver borrowing, generic scenarios, and metadata-free inference.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_rust_interop_tests_root.rs");

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
