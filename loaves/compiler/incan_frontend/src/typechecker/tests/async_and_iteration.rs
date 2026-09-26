//! `async` / `await` (unawaited-call warnings, awaitable bounds, `race_for`, join handles, semaphores), RFC 088
//! iterator adapters and terminals, builtin `zip` (#950), RFC 006 generators, and the fallible iteration protocol.

use super::*;

#[test]
fn test_local_async_function_named_sleep_shadows_no_builtin() {
    let source = r#"
import std.async

async def sleep(seconds: float) -> None:
  pass

async def foo() -> None:
  await sleep(1.0)
"#;
    assert_check_ok(source);
}

#[test]
fn test_await_outside_async_function() {
    let source = r#"
from std.async.time import sleep

def foo() -> None:
  await sleep(1.0)
"#;
    let errs = check_str_err(source, "await in sync function should fail");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("await") && e.message.contains("async")),
        "expected await-outside-async diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_await_outside_async_method() {
    let source = r#"
from std.async.time import sleep

model Widget:
  id: int

  def work(self) -> None:
    await sleep(1.0)
"#;
    let errs = check_str_err(source, "await in sync method should fail");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("await") && e.message.contains("async")),
        "expected await-outside-async diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_unawaited_async_function_call_warns() {
    let source = r#"
import std.async

async def fetch() -> int:
  return 1

async def main() -> None:
  fetch()
"#;
    let warnings = check_str_warnings(source, "unawaited async function call should warn");
    assert!(
        warnings
            .iter()
            .any(|warning| warning.message.contains("Async call `fetch` is not awaited")),
        "expected missing-await warning, got: {warnings:?}"
    );
}

#[test]
fn test_awaited_async_function_call_does_not_warn() {
    let source = r#"
import std.async

async def fetch() -> int:
  return 1

async def main() -> None:
  value = await fetch()
"#;
    let warnings = check_str_warnings(source, "awaited async function call should not warn");
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.message.contains("Async call `fetch` is not awaited")),
        "did not expect missing-await warning, got: {warnings:?}"
    );
}

#[test]
fn test_awaited_async_try_call_does_not_warn() {
    let source = r#"
import std.async

async def fetch() -> Result[int, str]:
  return Ok(1)

async def main() -> Result[None, str]:
  value = await fetch()?
  return Ok(None)
"#;
    let warnings = check_str_warnings(source, "awaited async try call should not warn");
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.message.contains("Async call `fetch` is not awaited")),
        "did not expect missing-await warning, got: {warnings:?}"
    );
}

#[test]
fn test_unawaited_imported_async_function_call_warns() {
    let source = r#"
from std.async.time import sleep

async def main() -> None:
  sleep(1.0)
"#;
    let warnings = check_str_warnings(source, "unawaited imported async function call should warn");
    assert!(
        warnings
            .iter()
            .any(|warning| warning.message.contains("Async call `sleep` is not awaited")),
        "expected missing-await warning, got: {warnings:?}"
    );
}

#[test]
fn test_unawaited_async_method_call_warns() {
    let source = r#"
import std.async

model Worker:
  id: int

  async def run(self) -> int:
    return self.id

async def main(worker: Worker) -> None:
  worker.run()
"#;
    let warnings = check_str_warnings(source, "unawaited async method call should warn");
    assert!(
        warnings
            .iter()
            .any(|warning| warning.message.contains("Async call `run` is not awaited")),
        "expected missing-await warning, got: {warnings:?}"
    );
}

#[test]
fn test_await_join_handle_returns_result_task_join_error() {
    let source = r#"
from std.async.task import JoinHandle, TaskJoinError

async def wait_for(handle: JoinHandle[int]) -> Result[int, TaskJoinError]:
  return await handle
"#;
    assert_check_ok(source);
}

#[test]
fn test_await_rejects_non_awaitable_operand() {
    let source = r#"
import std.async

async def main() -> None:
  _ = await 1
"#;
    let errors = check_str_err(source, "awaiting int should fail");
    assert!(
        errors.iter().any(|error| error.message.contains("Awaitable")),
        "expected Awaitable diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_await_generic_awaitable_bound_returns_output_type() {
    let source = r#"
import std.async

async def wait_for[T, F with Awaitable[T]](task: F) -> T:
  return await task
"#;
    assert_check_ok(source);
}

/// The documented wrapper (`stdlib_traits/awaitable.md` before #1711) is refused at the adoption: `Awaitable[T]` is
/// only a bound, and a model always derives `Clone`, which the `JoinHandle[T]` it holds cannot be.
#[test]
fn test_awaitable_wrapper_adoption_is_refused_on_a_model_issue1711() {
    let source = r#"
import std.async
from std.async.task import JoinHandle, TaskJoinError

model TaskBox[T] with Awaitable[Result[T, TaskJoinError]]:
  handle: JoinHandle[T]

async def wait_for(box: TaskBox[int]) -> Result[int, TaskJoinError]:
  return await box
"#;
    let errors = check_str_err(source, "Awaitable wrapper adoption should fail");
    assert!(
        errors.iter().any(|error| {
            error
                .message
                .contains("Type 'TaskBox' cannot adopt Awaitable[Result[T, TaskJoinError]]")
                && error.hints.iter().any(|hint| hint.contains("F with Awaitable[T]"))
        }),
        "expected the awaitable adoption refusal with its field-await hint, got: {errors:?}"
    );
}

/// A class is refused the same way.
#[test]
fn test_awaitable_wrapper_adoption_is_refused_on_a_class_issue1711() {
    let source = r#"
import std.async
from std.async.task import JoinHandle, TaskJoinError

class TaskBox with Awaitable[Result[int, TaskJoinError]]:
  handle: JoinHandle[int]
"#;
    let errors = check_str_err(source, "Awaitable wrapper adoption on a class should fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Type 'TaskBox' cannot adopt Awaitable[Result[int, TaskJoinError]]")),
        "expected the awaitable adoption refusal, got: {errors:?}"
    );
}

/// A newtype is refused too: only a `rusttype` adoption took the `awaitable_future_bridge_blocked` path, so a plain
/// newtype's adoption passed the checker and lowered to a trait impl rustc refuses.
#[test]
fn test_awaitable_wrapper_adoption_is_refused_on_a_newtype_issue1711() {
    let source = r#"
import std.async

type Ticket = newtype int with Awaitable[int]
"#;
    let errors = check_str_err(source, "Awaitable adoption on a newtype should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Type 'Ticket' cannot adopt Awaitable[int]")),
        "expected the awaitable adoption refusal, got: {errors:?}"
    );
}

#[test]
fn test_awaitable_adoption_rejects_wrapper_without_awaitable_field() {
    let source = r#"
import std.async

model Bad with Awaitable[int]:
  value: int
"#;
    let errors = check_str_err(source, "Awaitable wrapper without awaitable field should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Type 'Bad' cannot adopt Awaitable[int]")),
        "expected the awaitable adoption refusal, got: {errors:?}"
    );
}

#[test]
fn test_awaitable_adoption_rejects_wrong_wrapper_output_type() {
    let source = r#"
import std.async
from std.async.task import JoinHandle

model Bad with Awaitable[int]:
  handle: JoinHandle[int]
"#;
    let errors = check_str_err(source, "Awaitable wrapper with wrong output type should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Type 'Bad' cannot adopt Awaitable[int]")),
        "expected the awaitable adoption refusal, got: {errors:?}"
    );
}

#[test]
fn test_race_for_homogeneous_result_typechecks() {
    let source = r#"
import std.async

async def fast() -> int:
  return 1

async def slow() -> int:
  return 2

async def main() -> int:
  return race for value:
    await fast() => value
    await slow() => value
"#;
    assert_check_ok(source);
}

#[test]
fn test_race_for_union_result_typechecks() {
    let source = r#"
import std.async

async def fetch_text() -> str:
  return "ready"

async def fetch_count() -> int:
  return 1

async def main() -> str | int:
  return race for value:
    await fetch_text() => value
    await fetch_count() => value
"#;
    assert_check_ok(source);
}

#[test]
fn test_race_for_rejects_non_awaitable_arm() {
    let source = r#"
import std.async

async def main() -> int:
  return race for value:
    await 1 => value
"#;
    let errors = check_str_err(source, "race arm awaiting int should fail");
    assert!(
        errors.iter().any(|error| error.message.contains("Awaitable")),
        "expected Awaitable diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_race_for_rejects_non_async_context() {
    let source = r#"
import std.async

async def fast() -> int:
  return 1

def main() -> int:
  return race for value:
    await fast() => value
"#;
    let errors = check_str_err(source, "race outside async should fail");
    assert!(
        errors.iter().any(|error| error.message.contains("outside of an async")),
        "expected async-context diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_join_handle_satisfies_awaitable_result_bound() {
    let source = r#"
from std.async.task import JoinHandle, TaskJoinError

def accept[T, F with Awaitable[T]](task: F) -> F:
  return task

def main(handle: JoinHandle[int]) -> None:
  _ = accept[Result[int, TaskJoinError], JoinHandle[int]](handle)
"#;
    assert_check_ok(source);
}

#[test]
fn test_join_handle_rejects_wrong_awaitable_output_bound() {
    let source = r#"
from std.async.task import JoinHandle

def accept[T, F with Awaitable[T]](task: F) -> F:
  return task

def main(handle: JoinHandle[int]) -> None:
  _ = accept[int, JoinHandle[int]](handle)
"#;
    let errors = check_str_err(source, "JoinHandle[int] should not satisfy Awaitable[int]");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("violates generic bound") && error.message.contains("Awaitable[int]")),
        "expected Awaitable[int] generic bound diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_semaphore_acquire_returns_result_semaphore_acquire_error() {
    let source = r#"
from std.async.sync import Semaphore, SemaphoreAcquireError

async def take(sem: Semaphore) -> Result[int, SemaphoreAcquireError]:
  result = await sem.acquire()
  permit = result?
  return Ok(1)
"#;
    assert_check_ok(source);
}

#[test]
fn test_sleep_requires_float() {
    let source = r#"
from std.async.time import sleep

async def foo():
  await sleep(1)
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_rfc088_iterator_adapter_chain_types_collect_as_list() {
    let source = r#"
def keep(n: int) -> bool:
  return n > 0

def label(n: int) -> str:
  return str(n)

def pairs(items: Iterator[int], labels: Iterator[str]) -> list[tuple[int, str]]:
  return items.filter(keep).take(10).skip(1).zip(labels).collect()

def indexed(items: Iterator[int]) -> list[tuple[int, int]]:
  return items.enumerate().collect()

def labels(items: Iterator[int]) -> list[str]:
  return items.map(label).collect()

def leading_positive(items: Iterator[int]) -> list[int]:
  return items.take_while(keep).collect()

def after_positive_prefix(items: Iterator[int]) -> list[int]:
  return items.skip_while(keep).collect()
"#;
    assert_check_ok(source);
}

#[test]
fn test_rfc088_iterator_zip_preserves_tuple_item_types_in_for_loop_issue950() {
    let source = r#"
def score_pairs(left: list[float], right: list[float]) -> float:
  mut score = 0.0
  for pair in left.iter().zip(right.iter()):
    score += pair[0] * pair[1]
  return score
"#;
    assert_check_ok(source);
}

#[test]
fn test_builtin_zip_is_a_canonical_lazy_iterator_issue950() {
    let source = r#"
def collected_pairs(left: list[int], right: list[str]) -> list[tuple[int, str]]:
  return zip(left, right).collect()

def explicit_builtin_pairs(left: list[int], right: list[str]) -> list[tuple[int, str]]:
  return std.builtins.zip(left, right).collect()

def iterator_pairs(left: Iterator[int], right: Iterator[str]) -> list[tuple[int, str]]:
  return zip(left, right).collect()

def frozen_pairs(left: FrozenList[int], right: FrozenList[str]) -> list[tuple[int, str]]:
  return zip(left, right).collect()
"#;
    assert_check_ok(source);
}

#[test]
fn test_builtin_zip_rejects_unsupported_operands_issue950() {
    let bare = check_str_err(
        "def main() -> None:\n  for pair in zip(1, [\"one\"]):\n    println(pair[0])\n",
        "bare zip should reject a scalar operand",
    );
    assert!(
        bare.iter()
            .any(|error| error.message == "zip() argument 1 must be a list, FrozenList, or Iterator, got int"),
        "unexpected errors: {:?}",
        bare.iter().map(|error| &error.message).collect::<Vec<_>>()
    );

    let explicit = check_str_err(
        "def main() -> None:\n  for pair in std.builtins.zip([1], true):\n    println(pair[0])\n",
        "explicit builtin zip should reject a scalar operand",
    );
    assert!(
        explicit
            .iter()
            .any(|error| error.message == "zip() argument 2 must be a list, FrozenList, or Iterator, got bool"),
        "unexpected errors: {:?}",
        explicit.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_builtin_zip_requires_exactly_two_operands_issue950() {
    for (source, expected) in [
        (
            "def main() -> None:\n  zip([1])\n",
            "zip() expects 2 argument(s), got 1",
        ),
        (
            "def main() -> None:\n  std.builtins.zip([1], [2], [3])\n",
            "zip() expects 2 argument(s), got 3",
        ),
    ] {
        let errors = check_str_err(source, "zip arity mismatch should fail");
        assert!(
            errors.iter().any(|error| error.message == expected),
            "expected {expected:?}, got {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_rfc088_direct_iterator_loop_consumes_binding_issue950() {
    let source = r#"
def consume_twice(items: Iterator[int]) -> int:
  mut total = 0
  for item in items:
    total += item
  return total + items.count()
"#;
    let errs = check_str_err(source, "direct iterator loop must consume its source binding");
    assert!(
        errs.iter()
            .any(|error| error.message.contains("iterator binding `items` was consumed")),
        "unexpected errors: {:?}",
        errs.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rfc088_flat_map_accepts_list_callback_result() {
    let source = r#"
def words_for(_n: int) -> list[str]:
  return ["hello"]

def flatten(items: Iterator[int]) -> list[str]:
  return items.flat_map(words_for).collect()
"#;
    assert_check_ok(source);
}

#[test]
fn test_rfc088_iterator_terminal_methods_have_frontend_types() {
    let source = r#"
def keep(n: int) -> bool:
  return n > 0

def add(acc: int, n: int) -> int:
  return acc + n

def visit(_n: int) -> None:
  pass

def count_items(items: Iterator[int]) -> int:
  return items.count()

def any_item(items: Iterator[int]) -> bool:
  return items.any(keep)

def all_items(items: Iterator[int]) -> bool:
  return items.all(keep)

def find_item(items: Iterator[int]) -> Option[int]:
  return items.find(keep)

def reduce_items(items: Iterator[int]) -> int:
  return items.reduce(0, add)

def fold_items(items: Iterator[int]) -> int:
  return items.fold(0, add)

def visit_items(items: Iterator[int]) -> None:
  return items.for_each(visit)

def sum_items(items: Iterator[int]) -> int:
  return items.sum()
"#;
    assert_check_ok(source);
}

#[test]
fn test_rfc088_iterator_sum_accepts_numeric_items_only() {
    let source = r#"
def sum_ints(items: Iterator[int]) -> int:
  return items.sum()

def sum_floats(items: Iterator[float]) -> float:
  return items.sum()

type Money = newtype int

def sum_money(items: Iterator[Money]) -> Money:
  return items.sum()
"#;
    assert_check_ok(source);

    let bad_source = r#"
def sum_strings(items: Iterator[str]) -> str:
  return items.sum()
"#;
    let errs = check_str_err(bad_source, "sum over string iterator should be rejected");
    assert!(
        errs.iter().any(|e| e
            .message
            .contains("Iterator.sum() requires int, float, or a newtype over a summable type; found str")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rfc088_builtin_list_iter_enters_iterator_surface() {
    let source = r#"
from std.derives.collection import Iterable

def keep(n: int) -> bool:
  return n > 0

def collect_positive(items: list[int]) -> list[int]:
  return items.iter().filter(keep).batch(2).flat_map(identity_batch).collect()

def identity_batch(batch: list[int]) -> list[int]:
  return batch
"#;
    assert_check_ok(source);
}

#[test]
fn test_rfc088_filter_callback_return_mismatch_is_rejected() {
    let source = r#"
def bad(_n: int) -> str:
  return "no"

def collect_bad(items: Iterator[int]) -> list[int]:
  return items.filter(bad).collect()
"#;
    let errs = check_str_err(source, "filter callback returning str should be rejected");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("expected '(int) -> bool', found '(int) -> str'")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rfc088_batch_rejects_static_non_positive_size() {
    let source = r#"
def collect_bad(items: Iterator[int]) -> list[list[int]]:
  return items.batch(0).collect()
"#;
    let errs = check_str_err(source, "batch(0) should be rejected");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Iterator.batch() size must be greater than zero")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rfc088_terminal_consumption_rejects_obvious_same_binding_reuse() {
    let source = r#"
def consume_twice(items: Iterator[int]) -> int:
  first = items.count()
  return first + items.count()
"#;
    let errs = check_str_err(source, "same iterator binding reused after terminal method");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("iterator binding `items` was consumed")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_async_function() {
    let source = r#"
import std.async

async def foo() -> int:
  return 42
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_fallible_iteration_protocol_propagates_next_errors() -> Result<(), Vec<CompileError>> {
    let source = r#"
model ChunkStream:
  def __iter__(self) -> ChunkStream:
    return self

  def __next__(self) -> Result[Option[int], str]:
    return Ok(None)

def main() -> Result[None, str]:
  for chunk in ChunkStream()?:
    seen = chunk
  return Ok(None)
"#;

    assert_check_ok(source);
    Ok(())
}

#[test]
fn test_fallible_iteration_protocol_accepts_trait_typed_receiver() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait FallibleStream[T, E]:
  def __iter__(self) -> Self:
    return self

  def __next__(self) -> Result[Option[T], E]: ...

model ChunkStream with FallibleStream[int, str]:
  def __next__(self) -> Result[Option[int], str]:
    return Ok(None)

def chunks() -> FallibleStream[int, str]:
  return ChunkStream()

def main() -> Result[None, str]:
  for chunk in chunks()?:
    seen = chunk
  return Ok(None)
"#;

    assert_check_ok(source);
    Ok(())
}

#[test]
fn test_fallible_iteration_rejects_combined_setup_and_polling_result() {
    let source = r#"
model ChunkStream:
  def __iter__(self) -> ChunkStream:
    return self

  def __next__(self) -> Result[Option[int], str]:
    return Ok(None)

def open_stream() -> Result[ChunkStream, str]:
  return Ok(ChunkStream())

def main() -> Result[None, str]:
  for chunk in open_stream()?:
    seen = chunk
  return Ok(None)
"#;

    let errors = check_str_err(
        source,
        "combined setup and polling failures should require an explicit local",
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("cannot unwrap a Result containing a fallible iterator in the same header")),
        "expected setup-versus-polling diagnostic, got {errors:?}"
    );
}

#[test]
fn test_fallible_loop_header_preserves_result_of_ordinary_iterable() -> Result<(), Vec<CompileError>> {
    let source = r#"
def load_values() -> Result[list[int], str]:
  return Ok([1, 2])

def main() -> Result[None, str]:
  for value in load_values()?:
    seen = value
  return Ok(None)
"#;

    assert_check_ok(source);
    Ok(())
}

#[test]
fn test_fallible_iteration_requires_loop_header_marker() {
    let source = r#"
model ChunkStream:
  def __iter__(self) -> ChunkStream:
    return self

  def __next__(self) -> Result[Option[int], str]:
    return Ok(None)

def main() -> None:
  for chunk in ChunkStream():
    seen = chunk
"#;

    let errors = check_str_err(
        source,
        "fallible iteration without the header marker should be rejected",
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("expected 'Option[_]', found 'Result[Option[int], str]'")),
        "expected ordinary-loop protocol mismatch, got {errors:?}"
    );
}

#[test]
fn test_fallible_loop_header_marker_rejects_ordinary_iterable() {
    let source = r#"
def main() -> Result[None, str]:
  for value in [1, 2]?:
    seen = value
  return Ok(None)
"#;

    let errors = check_str_err(source, "the fallible header marker should reject an ordinary iterable");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("expected 'fallible iterator with __iter__() and __next__() -> Result[Option[_], _]'")),
        "expected fallible-loop protocol diagnostic, got {errors:?}"
    );
}

#[test]
fn test_rfc006_generator_function_yields_iterates_and_collects() -> Result<(), Vec<CompileError>> {
    let source = r#"
def double(value: int) -> int:
  return value * 2

def keep(value: int) -> bool:
  return value > 0

def numbers() -> Generator[int]:
  yield 1
  yield 2
  return

def main() -> List[int]:
  mut total = 0
  for item in numbers():
    total = total + item
  return numbers().map(double).filter(keep).take(2).collect()
"#;

    check_str(source)
}

#[test]
fn test_rfc006_generator_satisfies_iterable_and_iterator_traits() -> Result<(), Vec<CompileError>> {
    let source = r#"
def numbers() -> Generator[int]:
  yield 1

def accept_iterable(values: Iterable[int]) -> None:
  pass

def accept_iterator(values: Iterator[int]) -> None:
  pass

def main() -> None:
  accept_iterable(numbers())
  accept_iterator(numbers())
"#;

    check_str(source)
}

#[test]
fn test_rfc006_generator_yield_must_match_element_type() {
    let source = r#"
def broken() -> Generator[int]:
  yield "nope"
"#;

    let errs = check_str_err(source, "expected generator yield type mismatch");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("expected 'int', found 'str'")),
        "expected generator yield type mismatch, got: {errs:?}"
    );
}

#[test]
fn test_rfc006_generator_requires_reachable_yield() {
    let source = r#"
def broken() -> Generator[int]:
  return
"#;

    let errs = check_str_err(source, "expected missing generator yield diagnostic");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("must contain at least one `yield value`")),
        "expected missing generator yield diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_rfc006_yield_outside_generator_is_rejected() {
    let source = r#"
def broken() -> int:
  yield 1
  return 1
"#;

    let errs = check_str_err(source, "expected ordinary yield rejection");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("`yield` is only valid in generator functions or fixtures")),
        "expected yield context diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_rfc006_generator_return_value_is_rejected() {
    let source = r#"
def broken() -> Generator[int]:
  yield 1
  return 2
"#;

    let errs = check_str_err(source, "expected generator return-value rejection");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Generator functions cannot use `return value`")),
        "expected generator return-value diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_rfc006_generator_helpers_validate_arguments() {
    let source = r#"
def stringify(value: int) -> str:
  return f"{value}"

def keep_str(value: str) -> bool:
  return true

def numbers() -> Generator[int]:
  yield 1

def main() -> None:
  mapped = numbers().map(1)
  filtered = numbers().filter(stringify)
  wrong_input = numbers().filter(keep_str)
  limited = numbers().take("2")
"#;

    let errs = check_str_err(source, "expected generator helper argument diagnostics");
    for expected in [
        "(int) -> _",
        "expected 'bool', found 'str'",
        "expected 'str', found 'int'",
        "expected 'int', found 'str'",
    ] {
        assert!(
            errs.iter().any(|err| err.message.contains(expected)),
            "expected diagnostic containing {expected:?}, got: {errs:?}"
        );
    }
}

#[test]
fn test_rfc006_generator_expression_infers_element_type() -> Result<(), Vec<CompileError>> {
    let source = r#"
def positives(xs: List[int], ys: List[int]) -> Generator[int]:
  return (x * y for x in xs if x > 0 for y in ys if y > x)
"#;

    check_str(source)
}

#[test]
fn test_rfc006_generator_expression_filter_must_be_bool() {
    let source = r#"
def broken(xs: List[int]) -> Generator[int]:
  return (x for x in xs if x)
"#;

    let errs = check_str_err(source, "expected generator expression filter diagnostic");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("expected 'bool', found 'int'")),
        "expected generator expression filter diagnostic, got: {errs:?}"
    );
}
