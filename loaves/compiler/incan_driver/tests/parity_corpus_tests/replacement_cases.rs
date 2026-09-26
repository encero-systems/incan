//! The #988 replacement-execution rows (`replacement-body-v0-*`): each stable receipt-bound source case with
//! the arguments its entrypoint takes and the value direct execution must produce.

use super::*;

pub(super) const REPLACEMENT_BODY_V0_001_SRC: &str = r#"
def add(x: int, y: int) -> int:
    return x + y
"#;

pub(super) const REPLACEMENT_BODY_V0_002_SRC: &str = r#"
def greet(name: str) -> str:
    return "hello, " + name
"#;

pub(super) const REPLACEMENT_BODY_V0_003_SRC: &str = r#"
def return_owned() -> str:
    value = "owned"
    return value
"#;

pub(super) const REPLACEMENT_BODY_V0_004_SRC: &str = r#"
def control_flow() -> int:
    for value in range(1, 5):
        if value % 2 == 0:
            continue
    while false:
        return 0
    return 10
"#;

pub(super) const REPLACEMENT_BODY_V0_005_SRC: &str = r#"
def guarded_floor_div(a: int, b: int) -> int:
    assert b != 0
    return a // b
"#;

pub(super) const REPLACEMENT_BODY_V0_006_SRC: &str = r#"
def select_second_pair() -> int:
    pairs = [(1, 2), (4, 5)]
    for a, b in pairs:
        if a == 4:
            return a * 10 + b
    return 0
"#;

pub(super) const REPLACEMENT_BODY_V0_007_SRC: &str = r#"
def collect_lazy_values() -> int:
    values = (value * 10 for value in range(1, 5) if value > 2).collect()
    return values[0] + values[1]
"#;

pub(super) const REPLACEMENT_BODY_V0_008_SRC: &str = r#"
def stored_closure() -> int:
    offset = 2
    add: (int) -> int = (value) => value + offset
    return add(40)
"#;

pub(super) const REPLACEMENT_BODY_V0_009_SRC: &str = r#"
def route(method: int, path: int, content_type: int = 3) -> int:
    return method * 100 + path * 10 + content_type

def partial_defaults() -> int:
    method = 1
    get = partial route(method=method)
    normal = get(4)
    overridden = get(method=7, path=2, content_type=5)
    return normal + overridden
"#;

pub(super) const REPLACEMENT_BODY_V0_010_SRC: &str = r#"
def counter() -> Generator[int]:
    for value in range(1, 3):
        yield value
    yield 3

def generator_function() -> int:
    values = counter().collect()
    return values[0] * 100 + values[1] * 10 + values[2]
"#;

pub(super) const REPLACEMENT_BODY_V0_011_SRC: &str = r#"
def generator_adapters() -> int:
    offset = 1
    increment: (int) -> int = (value) => value + offset
    accepted: (int) -> bool = (value) => value > 2
    values = (value for value in range(1, 5)).map(increment).filter(accepted).collect()
    return values[0] * 10 + values[1]
"#;

pub(super) const REPLACEMENT_BODY_V0_012_SRC: &str = r#"
def score(mut values: list[int]) -> int:
    values[0] = 40
    pair = (values[0], 2)
    return pair.0 + pair.1

def structural_values() -> int:
    values = [1, 2]
    return score(values)
"#;

pub(super) const REPLACEMENT_BODY_V0_013_SRC: &str = r#"
model Pair:
    left: int
    right: int

def score(pair: Pair) -> int:
    return pair.left + pair.right

def nominal_values() -> int:
    pair = Pair(right=2, left=40)
    return score(pair)
"#;

pub(super) const REPLACEMENT_BODY_V0_014_SRC: &str = r#"
enum HttpStatus(int):
    Ok = 200
    NotFound = 404

def status_code(status: HttpStatus) -> int:
    return status.value()

def value_enum_values() -> int:
    return status_code(HttpStatus.NotFound)
"#;

pub(super) const REPLACEMENT_BODY_V0_015_SRC: &str = r#"
enum Signal:
    Ready
    Stop

def score(left: Signal, right: Signal) -> int:
    if left == Signal.Ready and right != Signal.Ready:
        return 42
    return 0

def fieldless_enum_values() -> int:
    return score(Signal.Ready, Signal.Stop)
"#;

pub(super) const REPLACEMENT_BODY_V0_016_SRC: &str = r#"
model Pair:
    left: int
    right: int

enum Signal:
    Ready
    Stop

def classify(pair: Pair, signal: Signal) -> int:
    match pair:
        case Pair(left=40, right=2):
            match signal:
                case Signal.Ready:
                    return 42
                case Signal.Stop:
                    return 0
        case _:
            return 0
    return 0

def direct_patterns() -> int:
    return classify(Pair(left=40, right=2), Signal.Ready)
"#;

pub(super) const REPLACEMENT_BODY_V0_017_SRC: &str = r#"
enum Failure:
    Odd

def half(value: int) -> Result[int, Failure]:
    if value % 2 != 0:
        return Err(Failure.Odd)
    return Ok(value // 2)

def quarter(value: int) -> Result[int, Failure]:
    half_value = half(value)?
    return half(half_value)

def direct_result_routing() -> int:
    match quarter(8):
        case Ok(value):
            return value
        case Err(_):
            return 0
    return 0
"#;

pub(super) const REPLACEMENT_BODY_V0_018_SRC: &str = r#"
import std.async

async def answer() -> int:
    return 42

async def direct_async_await() -> int:
    return await answer()
"#;

pub(super) const REPLACEMENT_BODY_V0_019_SRC: &str = r#"
import std.async

async def first() -> int:
    return 1

async def second() -> int:
    return 2

async def source_order_race() -> int:
    winner = race for value:
        await first() => value
        await second() => value
    return winner
"#;

// This stays a typed `str` result so the scalar conversion proof is independent of selected string-method work.
// The printed line is source-observable comparison evidence: it proves normal conversion output reaches both route
// receipts rather than treating a matching return value as a substitute for program-stream parity.
pub(super) const REPLACEMENT_BODY_V0_022_SRC: &str = r#"
def scalar_conversions() -> str:
    parsed_int = int("42")
    parsed_float = float("3.14")
    widened_float = float(10)
    println(f"converted: {parsed_int} {parsed_float} {widened_float}")
    return f"{str(parsed_int)} {parsed_float} {widened_float}"
"#;

pub(super) const REPLACEMENT_BODY_V0_023_SRC: &str = include_str!("../fixtures/replacement/enumerate_zip.incn");

pub(super) const REPLACEMENT_BODY_V0_029_SRC: &str = r#"
def typed_numeric_profile() -> f32:
    unsigned_min: u8 = 0
    unsigned_max: u8 = 255
    signed_min: i128 = -170141183460469231731687303715884105728
    wide_max: u128 = 340282366920938463463374607431768211455
    rounded: f32 = 1.23456789
    money: decimal[6, 2] = 19.90d
    println(f"{unsigned_min} {unsigned_max} {signed_min} {wide_max} {money}")
    return rounded
"#;

pub(super) fn replacement_body_v0_001_arguments() -> Vec<ReplacementValue> {
    vec![ReplacementValue::Int(40), ReplacementValue::Int(2)]
}

pub(super) fn replacement_body_v0_001_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

pub(super) fn replacement_body_v0_002_arguments() -> Vec<ReplacementValue> {
    vec![ReplacementValue::Str("Ada".to_string())]
}

pub(super) fn replacement_body_v0_002_expected() -> ReplacementValue {
    ReplacementValue::Str("hello, Ada".to_string())
}

pub(super) fn replacement_body_v0_003_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_003_expected() -> ReplacementValue {
    ReplacementValue::Str("owned".to_string())
}

pub(super) fn replacement_body_v0_004_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_004_expected() -> ReplacementValue {
    ReplacementValue::Int(10)
}

pub(super) fn replacement_body_v0_005_arguments() -> Vec<ReplacementValue> {
    vec![ReplacementValue::Int(84), ReplacementValue::Int(2)]
}

pub(super) fn replacement_body_v0_005_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

pub(super) fn replacement_body_v0_006_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_006_expected() -> ReplacementValue {
    ReplacementValue::Int(45)
}

pub(super) fn replacement_body_v0_007_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_007_expected() -> ReplacementValue {
    ReplacementValue::Int(70)
}

pub(super) fn replacement_body_v0_008_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_008_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

pub(super) fn replacement_body_v0_009_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_009_expected() -> ReplacementValue {
    ReplacementValue::Int(868)
}

pub(super) fn replacement_body_v0_010_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_010_expected() -> ReplacementValue {
    ReplacementValue::Int(123)
}

pub(super) fn replacement_body_v0_011_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_011_expected() -> ReplacementValue {
    ReplacementValue::Int(34)
}

pub(super) fn replacement_body_v0_012_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_012_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

pub(super) fn replacement_body_v0_013_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_013_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

pub(super) fn replacement_body_v0_014_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_014_expected() -> ReplacementValue {
    ReplacementValue::Int(404)
}

pub(super) fn replacement_body_v0_015_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_015_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

pub(super) fn replacement_body_v0_016_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_016_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

pub(super) fn replacement_body_v0_017_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_017_expected() -> ReplacementValue {
    ReplacementValue::Int(2)
}

pub(super) fn replacement_body_v0_018_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_018_expected() -> ReplacementValue {
    ReplacementValue::Int(42)
}

pub(super) fn replacement_body_v0_019_arguments() -> Vec<ReplacementValue> {
    vec![]
}

pub(super) fn replacement_body_v0_019_expected() -> ReplacementValue {
    ReplacementValue::Int(1)
}

pub(super) fn replacement_body_v0_025_expected() -> ReplacementValue {
    ReplacementValue::Str(JSON_STRINGIFY_SCALARS_EXPECTED.to_string())
}

pub(super) fn replacement_body_v0_022_arguments() -> Vec<ReplacementValue> {
    vec![]
}

/// `float(10)` renders as `10.0`: a `float` spells itself the way Python does on both routes (#1372).
pub(super) fn replacement_body_v0_022_expected() -> ReplacementValue {
    ReplacementValue::Str("42 3.14 10.0".to_string())
}

/// The selected list-iteration fixture has no entry arguments.
pub(super) fn replacement_body_v0_023_arguments() -> Vec<ReplacementValue> {
    vec![]
}

/// Stored enumeration contributes ten and Zip contributes thirty-nine.
pub(super) fn replacement_body_v0_023_expected() -> ReplacementValue {
    ReplacementValue::Int(49)
}

pub(super) fn replacement_body_v0_029_expected() -> ReplacementValue {
    ReplacementValue::Numeric(ReplacementNumericValue::F32(1.234_567_9_f32))
}
