# Compile time and runtime

This page is written for Python users learning Incan's compiled design model. The goal is not to make you think about build steps; it is to show how compile-time facts, runtime behavior, types, traits, models, `const`, and `static` change the way you structure code.

This page explains one of the most important mental shifts when coming to Incan from Python:

> Some Incan code defines facts the compiler must understand before the program runs. Other Incan code performs work when the program runs.

That difference is not mainly about waiting for a build. Incan tooling should make compiler feedback fast. The important point is semantic: **compile-time facts and runtime behavior are different kinds of code**.

Despite the names, this page is less about wall-clock time and more about program design. The question is not "does this happen earlier or later?" The question is "can the compiler know and use this fact without running my program?"

## The short version

In Python, most code is runtime code. A module can define functions, create objects, read environment variables, open files, register handlers, and start work as the file is imported or executed.

In Incan, the top level of a module is mostly for declarations the compiler can analyze:

- imports
- `const` bindings
- type aliases and declarations
- models, classes, enums, traits, and functions
- derives and other compiler-recognized metadata

Runtime behavior normally starts from a function such as `main`.

```incan
const DEFAULT_DISCOUNT_PERCENT: int = 10

def discount_cents(subtotal_cents: int) -> int:
    return subtotal_cents * DEFAULT_DISCOUNT_PERCENT // 100

def main() -> None:
    discount = discount_cents(1200)
    print(discount)
```

`DEFAULT_DISCOUNT_PERCENT` is a compile-time fact. The compiler can validate it and bake it into the output program. Calling `discount_cents` and printing the result are runtime behavior. They start doing application work only when `main` runs.

## Why Python intuition can mislead

Python has a top-level execution model. The Python docs describe `__main__` as the environment where "top-level code is run".[^python-main] That is why this feels natural in Python:

```python
DEFAULT_DISCOUNT_PERCENT = 10

subtotal_cents = 1200
discount = subtotal_cents * DEFAULT_DISCOUNT_PERCENT // 100

print(discount)
```

This is ordinary Python. The file is executable code.

In Incan, the equivalent design should separate fixed program facts from live runtime inputs:

```incan
const DEFAULT_DISCOUNT_PERCENT: int = 10

def discount_cents(subtotal_cents: int) -> int:
    return subtotal_cents * DEFAULT_DISCOUNT_PERCENT // 100

def main() -> None:
    subtotal_cents = 1200
    discount = discount_cents(subtotal_cents)
    print(discount)
```

This is not ceremony for its own sake. It lets the compiler understand the program without running the program. The compiler can resolve names, check types, validate declarations, and prepare generated code without opening files, reading environment variables, calling APIs, or executing setup logic.

## The design question

When writing Incan, ask:

| Question | Compile-time design | Runtime design |
| --- | --- | --- |
| Is this known from the source code itself? | yes | usually no |
| Does it depend on the current run? | no | yes |
| Can the compiler use it to reject bad code? | yes | only through explicit runtime checks |
| Can it be baked into the output program? | often | no |
| Examples | types, consts, model fields, traits, derives | env vars, files, clock time, network, database rows |

Rule of thumb:

> If changing the value requires changing source code, it may be a compile-time fact. If changing it only requires changing the environment, input data, filesystem, network response, or command-line arguments, it is runtime behavior.

## Example: `const`

`const` is the most direct example.

```incan
const MAX_RETRIES: int = 3
const SERVICE_NAME = "billing-api"
```

These are compile-time facts. Incan's `const` page defines `const` as a compile-time constant that is validated during compilation and can be baked into the output program. See [Const bindings](consts.md).

This is different:

```incan
def default_max_retries() -> int:
    return 3

const MAX_RETRIES = default_max_retries()
```

That is invalid as a `const`, because an ordinary function call is runtime behavior. The compiler is not allowed to run arbitrary program functions just to produce a `const` value.

Write it as runtime code instead:

```incan
def default_max_retries() -> int:
    return 3

def main() -> None:
    max_retries = default_max_retries()
    print(max_retries)
```

The design benefit is clarity. A `const` initializer must be known from compile-time expressions. A function call belongs to runtime code unless the language explicitly defines it as const-evaluable.

## Example: `const` vs `static`

Python module globals can hide several different design choices behind the same syntax:

```python
API_VERSION = "v1"
request_count = 0
cache = {}
```

In Incan, those choices are separated. Fixed data belongs in `const`. Module-owned runtime storage belongs in `static`. Short-lived values usually belong inside functions.

```incan
const API_VERSION: str = "v1"
static request_count: int = 0

def record_request() -> int:
    request_count += 1
    return request_count

def main() -> None:
    count = record_request()
    print(API_VERSION)
    print(count)
```

`API_VERSION` is a compile-time fact. It is fixed data. `request_count` is runtime storage. It has one module-owned storage cell, and each call to `record_request` updates the same live value.

That is why `static` is not a "mutable const". It is a different phase choice: persistent runtime state owned by a module. See [Module static storage](static_storage.md).

## Example: top-level code and `main`

Python lets top-level code perform work:

```python
print("starting")
server = make_server()
server.run()
```

Incan top-level code is not a script body. Put executable behavior in a function:

```incan
def main() -> None:
    print("starting")
    server = make_server()
    server.run()
```

This is one of the places where Incan shows its Rust influence. Rust executable crates use a `main` function as the program entry point.[^rust-main] Incan follows that broad shape because it keeps module analysis separate from program execution.

The practical benefit is that importing or checking a module does not mean running the application. The compiler can analyze the module as structure first.

That separation is why arbitrary top-level execution is not allowed. Top-level source should answer questions like:

- What names does this module define?
- What types and models exist?
- What functions can be called later?
- What constants can be evaluated from source alone?
- What traits or derives does a type commit to?

Runtime functions answer different questions:

- What arguments did the user pass?
- What files exist today?
- What is in the environment?
- What did the database return?
- What network call failed?

Keeping those questions separate lets the compiler check modules without accidentally performing application work.

## Example: type annotations are contracts

In Python, type annotations are often checked by separate tools. The Python runtime "does not enforce" function and variable type annotations.[^python-typing]

In Incan, annotations are part of the compiler-checked program:

```incan
def discount_cents(total_cents: int, percent: int) -> int:
    return total_cents * percent // 100

def main() -> None:
    discount = discount_cents("1200", 10)
```

The problem is not that the call might fail someday. The call is wrong by type before the program runs. The compiler can reject it because the function signature is a compile-time contract.

This changes how you design APIs. In Python, you may write runtime validation to defend a function from every caller. In Incan, you should put stable shape and capability requirements in the type signature where the compiler can enforce them. Runtime validation still matters for untrusted input, but it should not replace compile-time structure.

## Example: duck typing vs declared capabilities

Python is comfortable with duck typing: if an object has the behavior you use at runtime, the code works.

```python
def greet(item):
    print(f"hello {item.name()}")
```

That style is flexible. The tradeoff is that the capability is discovered by use. If `item` does not have a compatible `name` method, the program fails when this code runs.

Python can make the expected shape more visible with a `Protocol`, but that belongs to Python's optional static-typing layer, not to runtime enforcement by the Python interpreter:[^python-protocol]

```python
from typing import Protocol

class Named(Protocol):
    def name(self) -> str: ...

def greet(item: Named) -> None:
    print(f"hello {item.name()}")
```

Or Python code can check at runtime:

```python
def greet(item):
    name = getattr(item, "name", None)
    if not callable(name):
        raise TypeError("expected an object with name()")
    print(f"hello {name()}")
```

That guard is real runtime validation, but it is still a check you wrote and executed. It does not make the capability part of the function's compile-time contract, and it still does not prove that `name()` returns a `str`. `hasattr` and `getattr` are useful tools for dynamic code, but they check object attributes at runtime.[^python-getattr]

Incan keeps the flexibility, but asks you to make the capability visible to the compiler:

```incan
trait Named:
    def name(self) -> str

def greet[T with Named](item: T) -> None:
    print(f"hello {item.name()}")
```

Here `T` is an arbitrary type parameter: read it as "any type", with `with Named` adding the requirement that the type must adopt the `Named` trait. The function does not require one concrete class. The difference is that this flexibility is now a compile-time fact. The compiler can check that callers pass something with the declared capability.

That is the main shift from Python duck typing to Incan traits:

| Python duck typing | Incan traits |
| --- | --- |
| Capability is discovered when code runs | Capability is declared in source |
| A missing method becomes a runtime error | A missing trait obligation becomes a compile-time error |
| Very flexible by default | Flexible when the capability is named |
| Good for exploratory code | Good for APIs that should stay correct as they grow |

The goal is not to force every function to accept one concrete type. The goal is to let the compiler understand the shape of the flexibility.

## Example: models describe shape

Models are another compile-time design surface.

```incan
model Customer:
    id: int
    email: str
    active: bool
```

That declaration is not just a convenient class-shaped container. It gives the compiler a named shape: `Customer` has `id`, `email`, and `active` fields with known types.

That means code can be checked against the model:

```incan
def send_welcome(customer: Customer) -> None:
    send_email(customer.email)
```

If you mistype the field name, the compiler has enough information to reject the code:

```incan
def send_welcome(customer: Customer) -> None:
    send_email(customer.emali)
```

In plain Python, an attribute typo like this is not rejected by the Python compiler. Attribute lookup is runtime behavior; Python raises `AttributeError` when an attribute reference fails.[^python-attribute-error] If the same data were represented as a dictionary and you wrote the wrong key, Python would raise `KeyError` when that mapping key is not found.[^python-key-error] Static type checkers and IDEs can catch some of these cases earlier, but that is extra tooling layered on Python, not the Python runtime contract.

Dataclasses and Pydantic are important Python answers to this problem, but they still live on the Python side of the line. A dataclass uses annotated fields to generate methods such as `__init__`, while the dataclasses docs explicitly note that, with narrow exceptions, the decorator does not examine the annotated type itself.[^python-dataclasses] Pydantic goes further: it validates and coerces input when creating a model instance, and its docs state that after validation the resulting fields conform to the model's declared field types.[^pydantic-models] Those are useful runtime and tooling patterns. They can make Python code much safer, especially at data boundaries. They still do not make `customer.emali` a Python compiler error in ordinary Python execution.

In Incan, the model declaration makes the field set a compile-time fact.

The runtime side is the actual data:

```incan
def main() -> None:
    customer = load_customer_from_database()
    send_welcome(customer)
```

The compiler knows the shape of `Customer`. It does not know which row the database will return.

## Example: derives choose behavior from declarations

Derives are selected from source declarations.

```incan
from std.derives.string import Debug

model Customer with Debug:
    id: int
    email: str
```

The `with Debug` clause is a compile-time request: generate or provide the debug formatting behavior for this model. Calling that behavior is runtime:

```incan
def main() -> None:
    customer = Customer(id=1, email="a@example.com")
    print(f"{customer:?}")
```

This distinction matters when designing reusable types. The decision that `Customer` supports debug formatting belongs with the type declaration. The act of formatting a particular customer belongs in runtime code.

## Example: adopting a trait

Traits are compile-time structure. If a type adopts a trait, the compiler can check that the required methods exist with compatible signatures.

```incan
trait Named:
    def name(self) -> str

class User with Named:
    username: str

    def name(self) -> str:
        return self.username
```

The obligation is compile-time: `User` must satisfy `Named`. Method execution is runtime:

```incan
def greet[T with Named](item: T) -> None:
    print(f"hello {item.name()}")
```

The trait makes the capability explicit enough for the compiler to reason about it.

## What still belongs at runtime

Compile-time checking is not a replacement for runtime validation. It answers different questions.

Compile time is good for:

- names that exist in source code
- types and signatures
- constant expressions
- model fields and declared shapes
- trait obligations
- derive requests
- import resolution and feature activation

Runtime is still where you handle:

- files that may or may not exist
- environment variables and secrets
- command-line arguments
- user input
- database contents
- network failures
- current time
- permissions
- data quality problems in external data

For example, if a function is declared to return `Customer`, the compiler can check callers against that shape:

```incan
def load_customer_from_database(id: int) -> Customer:
    # runtime database code lives here
    ...
```

The compiler can check code that uses the returned `Customer`. It cannot know whether the database is online or whether today's data violates a business rule.

## Common mistakes

### Treating `const` as "a variable I promise not to change"

Incan `const` is not Python's all-caps naming convention. It is a compile-time declaration. Use regular bindings or `static` for runtime state.

### Putting setup work at module top level

If code opens a connection, reads config, starts a server, or calls application logic, it belongs in `main` or another runtime function.

### Treating annotations as documentation

In Incan, annotations are not only hints for humans or external tools. They are part of the program contract.

### Treating models like dictionaries

A model declaration is a compiler-visible shape. If you need dynamic keys from external data, model the boundary explicitly instead of pretending the dynamic structure is a compile-time field set.

## Why learning this pays off

The phase distinction can feel strict at first, especially if you are used to Python's "just run the file" workflow. But it buys you a different style of confidence:

- refactors fail early when names, fields, or signatures stop lining up
- fixed data can be baked into the output program
- imports and modules can be checked without running setup code
- generated behavior comes from visible declarations
- runtime code can focus on real runtime uncertainty

The goal is not to make you think about the compiler all day. The goal is to make the compiler understand enough of your program that many structural mistakes never become runtime surprises.

## See also

- [How Incan works](how_incan_works.md)
- [Const bindings](consts.md)
- [Module static storage](static_storage.md)
- [Imports and modules](imports_and_modules.md)
- [Models and classes](models_and_classes/index.md)
- [Derives and traits](derives_and_traits.md)

[^python-main]: Python's `__main__` docs: [`__main__` - Top-level code environment](https://docs.python.org/3/library/__main__.html).
[^python-typing]: Python's `typing` docs: [`typing` - Support for type hints](https://docs.python.org/3/library/typing.html).
[^python-protocol]: Python's `typing.Protocol` docs: [`Protocol`](https://docs.python.org/3/library/typing.html#typing.Protocol).
[^python-getattr]: Python's built-in function docs: [`getattr`](https://docs.python.org/3/library/functions.html#getattr) and [`hasattr`](https://docs.python.org/3/library/functions.html#hasattr).
[^python-attribute-error]: Python's built-in exception docs: [`AttributeError`](https://docs.python.org/3/library/exceptions.html#AttributeError).
[^python-key-error]: Python's built-in exception docs: [`KeyError`](https://docs.python.org/3/library/exceptions.html#KeyError).
[^python-dataclasses]: Python's `dataclasses` docs: [`dataclasses` - Data Classes](https://docs.python.org/3/library/dataclasses.html).
[^pydantic-models]: Pydantic docs: [Models](https://docs.pydantic.dev/latest/concepts/models/).
[^rust-main]: Rust Reference: [Main functions](https://doc.rust-lang.org/reference/crates-and-source-files.html#main-functions).
