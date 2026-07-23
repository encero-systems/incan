# Fallible and infallible paths

This tutorial shows how to choose between plain return values, `Option`, `Result`, and panics.

## Coming from Python

Python code often uses `None`, `raise`, `try`/`except`, and `assert` to describe paths that do not produce the ordinary value. Incan keeps those paths visible in signatures and at call sites.

| Python habit                                               | Incan shape                         |
| ---------------------------------------------------------- | ----------------------------------- |
| `None` means "not found" or "not present"                  | `Option[T]`                         |
| `ValueError`, `OSError`, or a custom recoverable exception | `Result[T, E]`                      |
| `raise SomeError(...)`                                     | `return Err(SomeError(...))`        |
| `try`/`except` handles a failure locally                   | `match` on `Result`                 |
| Let an exception bubble to the caller                      | `?` propagates an `Err(...)`        |
| `assert` for an impossible state or programmer mistake     | `panic()` or a narrowly proven path |

In Python, an exception can skip the rest of the current block and search outward for a handler. In Incan, fallible paths are ordinary return values. Propagation is visible where it happens:

```python title="Python"
def load_username(path: str) -> str:
    try:
        text = read_text(path)
    except OSError:
        return "guest"

    if text.strip() == "":
        raise ValueError("name must not be empty")

    return text.strip()
```

```incan title="Incan"
from std.fs import IoError, Path

enum NameError:
    Io(IoError)
    Empty

def read_username(path: Path) -> Result[str, NameError]:
    text = path.read_text("utf-8", "strict").map_err(NameError.Io)?
    name = text.strip()
    if len(name) == 0:
        return Err(NameError.Empty)
    return Ok(name)

def load_username(path: Path) -> str:
    match read_username(path):
        Ok(name) => return name
        Err(NameError.Io(_)) => return "guest"
        Err(NameError.Empty) => return "guest"
```

The important difference is not syntax. It is control flow: Incan makes recovery (`match`) and propagation (`?`) explicit instead of hiding them behind exception unwinding.

The short rule:

- Return a plain value when the operation is expected to succeed for valid inputs.
- Return `Option[T]` when "not found" or "not present" is an ordinary outcome.
- Return `Result[T, E]` when the caller may need to recover, retry, report, or attach context.
- Use `panic()` or `unwrap()` only for bugs and broken invariants, not normal user, file, network, or config failures.

## Start with an infallible path

An infallible function returns the value directly. The caller does not need error handling.

```incan
model User:
    id: int
    name: str

def display_name(user: User) -> str:
    return user.name.strip()
```

This function can still be wrong if the program built a bad `User`, but there is no expected runtime failure for the caller to handle. The signature says: pass a `User`, get a `str`.

## Use `Option` for absence

Use `Option` when the operation may not find a value and absence is not itself an error.

```incan
def find_user(id: int, users: list[User]) -> Option[User]:
    for user in users:
        if user.id == id:
            return Some(user)
    return None

def print_user_if_present(id: int, users: list[User]) -> None:
    match find_user(id, users):
        Some(user) => println(display_name(user))
        None => println("no user found")
```

The caller decides whether `None` is acceptable. This is better than inventing an error when the outcome is simply "not there".

## Use `Result` for recoverable failure

Use `Result` when failure carries meaning that the caller may need to act on.

```incan
enum SignupError:
    EmptyName
    DuplicateUser(int)

def register_user(id: int, raw_name: str, users: list[User]) -> Result[User, SignupError]:
    name = raw_name.strip()
    if len(name) == 0:
        return Err(SignupError.EmptyName)

    match find_user(id, users):
        Some(_) => return Err(SignupError.DuplicateUser(id))
        None =>
            user = User(id=id, name=name)
            return Ok(user)
```

This path is fallible because the caller may respond differently to different errors:

```incan
def explain_signup(id: int, raw_name: str, users: list[User]) -> str:
    match register_user(id, raw_name, users):
        Ok(user) => return f"created {display_name(user)}"
        Err(SignupError.EmptyName) => return "name is required"
        Err(SignupError.DuplicateUser(existing_id)) => return f"user {existing_id} already exists"
```

## Propagate when the caller owns the decision

Use `?` when the current function cannot make the right recovery decision. The containing function must also return a compatible `Result`.

```incan
from std.fs import IoError, Path

enum ImportError:
    Io(IoError)
    Signup(SignupError)

def import_user(path: Path, users: list[User]) -> Result[User, ImportError]:
    data = path.read_bytes().map_err(ImportError.Io)?
    name = parse_user_name(data).map_err(ImportError.Signup)?
    user = register_user(42, name, users).map_err(ImportError.Signup)?
    return Ok(user)
```

The `?` is visible in the body, and the fallibility is visible in the return type. There is no hidden exception path.

## Propagate failures while iterating

Some operations can fail each time a loop requests another item. `FallibleIterator[T, E]` keeps that polling error separate from ordinary exhaustion. Put `?` on the loop header to propagate the first polling error:

```incan
from std.fs import Path
from std.io import BinaryReader, IoError

@derive(Clone)
enum ImportError:
    Read(IoError)


def count_bytes[R with BinaryReader](reader: R) -> Result[int, ImportError]:
    mut total = 0

    for chunk in reader.chunks(8192).map_err(ImportError.Read)?:
        total += len(chunk)

    return Ok(total)
```

The loop sees only non-empty chunks. `Ok(None)` ends it normally; `Err(IoError)` is converted by `map_err` and propagated by the trailing `?`. The adapter is lazy, so it maps errors produced by future polls rather than an error that already exists.

Opening a stream can fail once, while reading from it can fail on every poll. Keep those phases visible by unwrapping setup before the loop:

```incan
def count_path(path: Path) -> Result[int, ImportError]:
    reader = path.open("rb", -1, None, None, None).map_err(ImportError.Read)?
    return count_bytes(reader)
```

In `count_path`, the first `?` handles setup. The `?` inside `count_bytes` handles polling. A single `for item in open_stream()?:` header is rejected when the setup result contains a fallible iterator because it would hide those two different failure boundaries.

Fallible streams also support lazy `map`, `filter`, `flat_map`, `take`, `inspect`, `map_err`, and `inspect_err` adapters. Use `collect` or `fold` when you want one final `Result` instead of a loop. Retry is not automatic: the source must own retry because only it knows whether a failed poll advanced state or can safely be repeated.

## Compose `Result` with combinators

Use `Result` combinators when the code is transforming one branch of a result rather than making a multi-branch decision. The six standard methods are Rust-shaped and keep the same names in Incan: `map`, `map_err`, `and_then`, `or_else`, `inspect`, and `inspect_err`.

| Method | Callable argument | Use it when |
| ------ | ----------------- | ----------- |
| `map` | `Callable[T, U]` | transform an `Ok(T)` value into `Ok(U)` |
| `map_err` | `Callable[E, F]` | transform an `Err(E)` value into `Err(F)` |
| `and_then` | `Callable[T, Result[U, E]]` | chain another fallible operation after success |
| `or_else` | `Callable[E, Result[T, F]]` | recover from or remap a failure with another fallible operation |
| `inspect` | `Callable[T, None]` | observe or log a success value without changing the result |
| `inspect_err` | `Callable[E, None]` | observe or log an error value without changing the result |

The method form is usually the clearest spelling because the value flows left to right. The same operations are also available from `std.result` when a function-shaped API reads better or when you need to pass a combinator as a named helper:

```incan
from std.result import map as result_map

def double(value: int) -> int:
    return value * 2

def double_if_present(result: Result[int, str]) -> Result[int, str]:
    return result_map(result, double)
```

`map` and `map_err` are good at API boundaries because they keep the success path and error path separated:

```incan
from std.fs import IoError, Path

enum LoadError:
    Io(IoError)
    Parse(str)

def parse_profile(data: bytes) -> Result[Profile, str]:
    # Imagine a real parser here.
    return Ok(Profile.from_bytes(data))

def parse_profile_for_load(data: bytes) -> Result[Profile, LoadError]:
    return parse_profile(data).map_err(LoadError.Parse)

def load_profile(path: Path) -> Result[Profile, LoadError]:
    return path.read_bytes().map_err(LoadError.Io).and_then(parse_profile_for_load)
```

`and_then` avoids nested `match` when the next step is also fallible. `or_else` keeps recovery explicit:

```incan
def read_default_profile(_err: LoadError) -> Result[Profile, LoadError]:
    return Ok(Profile.guest())

def load_profile_or_guest(path: Path) -> Result[Profile, LoadError]:
    return load_profile(path).or_else(read_default_profile)
```

Use `inspect` and `inspect_err` for side effects such as logging, metrics, or debugging taps. They return the original `Result` unchanged, so they should not replace code that needs to transform the value or recover. The compiler passes the observed payload to the callable through an implicit borrow when preserving the branch requires it, which lets the original `Ok` or `Err` value continue through the pipeline without forcing a clone just because it was observed.

```incan
def log_profile(profile: Profile) -> None:
    println(f"loaded profile {profile.name}")

def log_load_error(err: LoadError) -> None:
    println(f"profile load failed: {err}")

def load_profile_with_logging(path: Path) -> Result[Profile, LoadError]:
    return load_profile(path).inspect(log_profile).inspect_err(log_load_error)
```

Prefer `match` when both branches matter, when you need multiple statements with local branching, or when the reader benefits from seeing every case explicitly. Prefer combinators when the intent is a single branch transformation, chain, recovery, or inspection tap.

## Convert at boundaries

Low-level functions should usually return low-level errors. Boundary functions should convert those errors into the language of the caller.

That keeps public APIs stable and caller-oriented. A CLI should not leak parser, filesystem, or backend implementation types when the useful caller-facing question is whether config can be read or validated.

```incan
from std.fs import IoError, Path

enum CliError:
    CouldNotReadConfig(str)
    InvalidConfig(str)

def config_read_error(err: IoError) -> CliError:
    return CliError.CouldNotReadConfig(err.message())

def config_signup_message(err: SignupError) -> str:
    match err:
        SignupError.EmptyName => return "name is required"
        SignupError.DuplicateUser(existing_id) => return f"user {existing_id} already exists"

def config_signup_error(err: SignupError) -> CliError:
    return CliError.InvalidConfig(config_signup_message(err))

def load_cli_user(path: Path, users: list[User]) -> Result[User, CliError]:
    data = path.read_bytes().map_err(config_read_error)?
    name = parse_config_user_name(data).map_err(config_signup_error)?
    user = register_user(42, name, users).map_err(config_signup_error)?
    return Ok(user)
```

Only the boundary needs to know whether config came from files, environment variables, network calls, or generated defaults.

## Keep panics for invariants

`unwrap()` says "this cannot fail here." If it can fail because of user input, config, files, network state, or ordinary runtime data, use `Result` or `Option` instead.

```incan
def first_registered_user(users: list[User]) -> User:
    # Valid only if the caller has already proven the list is non-empty.
    return users[0]
```

If the list might be empty in normal use, make that fact explicit:

```incan
def maybe_first_registered_user(users: list[User]) -> Option[User]:
    if len(users) == 0:
        return None
    return Some(users[0])
```

## Decision checklist

Ask these in order:

1. Can valid input still produce an expected non-value? Use `Option[T]`.
2. Can valid input still fail in a way the caller may recover from or report? Use `Result[T, E]`.
3. Is failure a bug in the caller or an impossible internal state? A panic may be acceptable.
4. Is the current function the right place to decide? If yes, `match`; if no, `?`.
5. Is this a public boundary? Convert low-level errors into the caller-facing error type.
6. Is the code only transforming, chaining, recovering, or observing one `Result` branch? Use a Result combinator.
7. Can fetching each next item fail? Use `FallibleIterator` and make polling propagation visible with `for item in stream?:`.

## Try it

1. Write `def parse_age(raw: str) -> Result[int, str]` that rejects an empty string.
2. Write `def find_age(name: str, ages: dict[str, int]) -> Option[int]`.
3. Write `def describe_age(name: str, ages: dict[str, int]) -> str` that handles `None` locally.
4. Write `def require_age(name: str, ages: dict[str, int]) -> Result[int, str]` that converts `None` into `Err(...)`.
5. Rewrite a nested `match` over `Result[int, str]` using `map`, `map_err`, or `and_then`.
6. Rewrite a manual `read_bytes` / empty-chunk loop using `reader.chunks(size)` and map its `IoError` into an application error.

## What to learn next

- Core concepts: [Error handling](../explanation/error_handling.md)
- Recipes: [Error handling recipes](../how-to/error_handling_recipes.md)
- File APIs with `Result`: [File I/O](../how-to/file_io.md)
- Fallible iterator hooks, adapters, and terminals: [Collection protocols](../reference/stdlib_traits/collection_protocols.md#fallibleiterator-fallible-iteration)
