# 8. Collections and iteration

<div class="inc-book-progress" aria-label="Chapter 8 of 13"><div class="inc-book-progress__meta"><strong>Chapter 8 of 13</strong><span>Collections and iteration</span></div><div class="inc-book-progress__bar" aria-hidden="true"><span style="--inc-progress: 61.5%"></span></div></div>

Incan has Python-like collections and a familiar `for` loop.

!!! tip "Coming from Rust?"
    Incan’s built-in collections compile to the standard Rust collections:

    - `list[T]` / `List[T]` → `Vec<T>`
        - `Vec[T]` is accepted as an alias for `List[T]` in type annotations
    - `dict[K, V]` / `Dict[K, V]` → `HashMap<K, V>`
    - `set[T]` / `Set[T]` → `HashSet<T>`

    (See the prelude type mapping in the [imports/modules reference](../../reference/imports_and_modules.md) for details.)

## Lists

```incan
def main() -> None:
    xs = [1, 2, 3]
    println(f"first={xs[0]}")
```

Use `append` to add an element at the end, and `pop()` to remove and return the last element. If you call `pop()` when the list is empty, the program panics with `IndexError: pop from empty list` and does not return a default value for the element type (the compiler does not require `Default` on the element type).

!!! tip "Coming from Python?"
    Incan lists are close to Python’s: out-of-range indexing and empty `list.pop()` both surface as [`IndexError`](../../reference/language.md#indexerror) panics with the same canonical messages as CPython where applicable (including `pop from empty list`).

Use `clone()` when you need to keep the original list and work with a copy. The element type must satisfy `Clone`, so models and classes that are not plain scalar values should derive or implement that capability.

```incan
@derive(Clone)
model Node:
    id: int

nodes = [Node(id=1), Node(id=2)]
copy = nodes.clone()
```

Use `list.repeat(value, count)` for fixed-length initialization when every element should start from the same clone-derived value:

```incan
ids: list[int] = list.repeat(-1, 8)
labels: list[str] = list.repeat("pending", 3)
```

The helper is available without importing `std.collections`. `count` must be an `int`; negative counts raise [`ValueError`](../../reference/language.md#valueerror) with the provided count in the message.

## Dicts

```incan
def main() -> None:
    scores = dict([("alice", 10), ("bob", 7)])
    println(f"alice={scores['alice']}")
```

## Sets

Use a set to deduplicate values:

```incan
def main() -> None:
    names = ["Alice", "Bob", "Alice"]
    unique = set(names)
    println(f"unique_count={len(unique)}")
```

## Iteration with `for`

```incan
def main() -> None:
    names = ["Alice", "Bob", "Cara"]
    for name in names:
        println(name)
```

When you iterate a list stored in a variable, the compiler picks a Rust iteration strategy that matches Incan’s element type. Scalars such as `int` are stepped by value. For **enums**, the loop variable is the enum type itself (the compiler uses clone-backed iteration under the hood), so you can compare it to another value of the same enum with `==` without extra cloning in your source.

## Iterator adapter chains

Use iterator adapters when you want to describe a lazy pipeline and only build the final list at the end:

```incan
def is_ready(job: Job) -> bool:
    return job.ready

def job_name(job: Job) -> str:
    return job.name

ready_names: list[str] = jobs.iter()
    .filter(is_ready)
    .map(job_name)
    .collect()
```

The adapter calls above do not build intermediate lists. `.filter(...)` and `.map(...)` return new iterators; `.collect()` is the terminal step that consumes the iterator and returns a `list[T]`.

Short-circuiting consumers are useful when you need a summary instead of a list:

```incan
has_blocked_job: bool = jobs.iter().any(is_blocked)
all_ready: bool = jobs.iter().all(is_ready)
first_failed: Option[Job] = jobs.iter().find(is_failed)
```

Terminal methods consume the iterator they are called on. If you need to keep the iterator for a later pass, clone it before the terminal method:

```incan
job_iter = jobs.iter()
ready_count = job_iter.clone().filter(is_ready).count()

for job in job_iter:
    println(job.name)
```

Some adapters combine or reshape streams:

```incan
pairs: list[tuple[str, int]] = names.iter()
    .zip(scores.iter())
    .collect()

chunks: list[list[Job]] = jobs.iter()
    .batch(100)
    .collect()
```

Use `.flat_map(...)` when each input item expands into another iterable value:

```incan
words: list[str] = documents.iter()
    .flat_map(document_words)
    .filter(is_searchable)
    .collect()
```

Here `document_words` can return any `Iterable[str]`, including a list or another iterator. `.collect()` still returns only `list[T]`; use an explicit conversion after collection if another container type is needed.

## Comprehensions (quick transforms)

Use comprehensions to build a new list/dict from an existing collection:

```incan
def main() -> None:
    names = [" Alice ", "Bob", " Cara "]

    normalized = [name.strip().lower() for name in names]
    counts = {name: 1 for name in normalized}

    println(f"normalized={normalized:?}")
    println(f"counts={counts:?}")
```

Use a generator when the pipeline should stay lazy until it is consumed:

```incan
def main() -> None:
    values = (x * 2 for x in [1, 2, 3, 4] if x > 1).take(2).collect()
    println(values[0])
    println(values[1])
```

Generator functions can also yield values one at a time:

```incan
def numbers() -> Generator[int]:
    yield 1
    yield 2
```

## Try it

1. Read a list of names, normalize (`strip().lower()`), and print them.
2. Create a `dict[str, int]` of counts for how often each name appears.
3. Use a `set` to print only unique names.

??? example "One possible solution"

    ```incan
    --8<-- "_snippets/language/examples/verified_collections_solution.incn"
    ```

`dict.get(...)` returns a borrowed value. Calling `.copied()` turns `Option[&int]` into `Option[int]` before `unwrap_or(0)` supplies the owned fallback.

## Where to learn more

- Specialized collections: [Choosing collection types](../../how-to/choosing_collections.md)
- Strings and slicing: [Strings](../../reference/strings.md)
- Generators: [Use generators for lazy pipelines](../../how-to/generators.md)
- Control flow overview: [Control flow](../../explanation/control_flow.md)

<nav class="inc-prev-next" aria-label="Book chapter navigation"><a href="../07_strings_and_formatting/"><small>Previous chapter</small><strong>← 7. Strings and formatting</strong></a><a href="../09_enums/"><small>Next chapter</small><strong>9. Enums and match →</strong></a></nav>
