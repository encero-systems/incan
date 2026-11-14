# Move a project from 0.5 to 0.6

Use this guide to bring a project that builds on Incan 0.5 onto 0.6. It lists each change that stops a 0.5 project from building, the message 0.6 prints when you meet it, and what to write instead.

Work through the sections in order: the manifest first, because nothing else is read until it is accepted.

## 1. Rename and convert the manifest

0.6 reads `loaf.toml`. It does not read `incan.toml`. A directory that holds `incan.toml` and no `loaf.toml` is refused:

```text
incan.toml: `incan.toml` is not a project manifest
```

Rename the file, then convert the tables that changed:

| 0.5 `incan.toml` | 0.6 `loaf.toml` |
| --- | --- |
| `[project]`, `[project.scripts]`, `[project.features]` | Unchanged. |
| `[build]` with `source-root` | Unchanged. |
| `[dependencies]` with a `path` | Unchanged. |
| `[rust-dependencies]` | `[dependencies]`, with each crate named as a Loaf. See below. |
| `[rust-dev-dependencies]` | `[envs.test.dependencies]`, in the same Loaf form. `incan test` selects them. |
| `[sdk]` | Remove it. Every standard package is available without selection. |
| `[oven.interop]` | `[interop.c]`. |
| `[vocab]` with `crate` | `[vocab]` with `entry`. See [Vocabulary entries](../reference/vocabulary.md). |

A Rust crate is now a registry Loaf under the `crates-io` scope, and the dependency key is the name your code imports:

```toml
# 0.5
[rust-dependencies]
regex = "1"
serde = { version = "1.0", features = ["derive"] }

# 0.6
[dependencies]
regex = { loaf = "crates-io/regex", version = "1" }
serde = { loaf = "crates-io/serde", version = "1.0", features = ["derive"] }
```

When a retired table is still present, the refusal names the table. For `[rust-dependencies]` it also prints the first entry in its new form:

```text
loaf.toml: ["rust-dependencies"]: unsupported: use [dependencies] regex = { loaf = "crates-io/regex", version = "1" }
loaf.toml: ["sdk"]: unsupported: there is no SDK selection: standard packages are ambient, and a unit reaches every one it uses
loaf.toml: ["vocab", "crate"]: unsupported: [vocab] crate is refused; declare [vocab] entry
```

The lock file is `oven.lock`. Delete `incan.lock` and write the new lock with `oven lock`.

[Project configuration](../reference/project_configuration.md) specifies every table and field.

## 2. Mark the methods other code calls as `pub`

In 0.5 every method could be called from anywhere. In 0.6 a method is private to its type unless it is marked `pub`, as fields already were. A call from outside the type is refused:

```text
type error: Method 'describe' on 'Animal' is private
```

Mark the method:

```incan
# 0.5
class Animal:
    name: str

    def describe(self) -> str:
        return self.name

# 0.6
class Animal:
    name: str

    pub def describe(self) -> str:
        return self.name
```

Methods that only the type itself calls need no change.

## 3. Construct through `pub` fields or a constructor method

A caller outside a type can no longer set that type's private fields when constructing it:

```text
error[INCAN-T0135]: Private field 'breed' cannot be set while constructing class 'Dog' here
  = hint: Call a `pub` constructor method on 'Dog', or make fields intended for direct construction `pub`
```

Choose one of the two forms the hint names:

```incan
# Fields meant to be set by callers
class Dog:
    pub name: str
    pub breed: str

# Or a constructor method, with the fields kept private
class Dog:
    name: str
    breed: str

    @staticmethod
    pub def new(name: str, breed: str) -> Dog:
        return Dog(name=name, breed=breed)
```

## 4. Annotate constant collections with their frozen type

A `const` is deeply immutable. 0.5 accepted a mutable collection annotation on one; 0.6 refuses it, because a frozen collection cannot be used where the mutable one is expected:

```text
type error: const 'NUMS' is annotated 'List[int]', but a const is deeply immutable and is represented as 'FrozenList[Int]'
  = hint: Annotate the const as 'FrozenList[Int]', or omit the annotation to infer it
```

```incan
# 0.5
const NUMS: List[int] = [1, 2, 3]
const HEADERS: Dict[str, str] = {"User-Agent": "incan"}

# 0.6
const NUMS: FrozenList[int] = [1, 2, 3]
const HEADERS: FrozenDict[str, str] = {"User-Agent": "incan"}
```

`str` and `bytes` constants need no change. [Frozen collections](../../language/reference/frozen_collections.md) specifies the frozen types.

## 5. Give a Rust crate a version where a file imports it without a project

A source file that runs on its own, outside a project, has no manifest to name its crates. Its `rust::` import states the version, and any features, itself:

```text
unknown Rust crate `uuid`: no version specified; add @ "VERSION" to its import
```

```incan
# 0.5
from rust::uuid import Uuid

# 0.6
from rust::uuid @ "1.0" with ["v4"] import Uuid
```

Inside a project, declare the crate under `[dependencies]` as in section 1 and leave the import as it is.

## 6. Delete `@rust.allow`

0.6 compiles a program directly, so there is no Rust lint for the decorator to suppress. It is refused:

```text
type error: `@rust.allow(...)` has been removed because native compilation generates no Rust source to carry it
  = hint: Delete the decorator; there is no replacement Rust lint suppression
```

Delete the decorator and leave the declaration as it is.

## 7. Use the new command spellings

| 0.5 | 0.6 |
| --- | --- |
| `incan lock` | `oven lock` |
| `incan run --release`, `incan build --release` | `--profile release` |
| `incan cache inspect`, `incan cache prune` | `oven bakery status`, `oven bakery gc` |

`oven` is the project command and `incan` the language command; both run `run`, `check`, `test` and `build`. The [CLI reference](../reference/cli_reference.md) specifies each command.

## Check the result

From the project directory:

```sh
oven lock
incan check src/main.incn
incan test tests
incan run src/main.incn
```

Each change above is reported one at a time, so repeat `incan check` until it passes before moving on to the tests.
