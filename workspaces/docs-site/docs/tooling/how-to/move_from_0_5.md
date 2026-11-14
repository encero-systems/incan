# Move a project from 0.5 to 0.6

This guide takes a 0.5 project to 0.6: the manifest first, then the source changes, then the commands. Each section shows the error 0.6 reports for the 0.5 spelling, so you can search this page for the message you see. Paths in the messages are shortened to the file's name.

## Rename the manifest

0.6 reads `loaf.toml`. A project that still has only `incan.toml` is refused:

```text
incan check: /path/to/app/incan.toml: `incan.toml` is not a project manifest; rename it to loaf.toml once its tables satisfy the Loaf schema
```

Rename the file, then convert the tables below. A `loaf.toml` whose `[project]` table has `name`, `version` and `scripts` works as it did; the rest of `[project]` (`description`, `authors`, `maintainers`, `license`, `license-files`, `readme`, `homepage`, `repository`, `documentation`, `issues`, `keywords`, `classifiers`, `private`, `features`) and `[build]` are accepted unchanged.

## Convert the manifest table by table

### `[rust-dependencies]` becomes `[dependencies]`

A Rust crate is a registry Loaf under `[dependencies]`, named by `loaf = "crates-io/NAME"`.

```toml
[rust-dependencies]
serde = { version = "1", features = ["derive"] }
```

```text
incan check: /path/to/app/loaf.toml: ["rust-dependencies"]: unsupported: use [dependencies] serde = { loaf = "crates-io/serde", version = "1", features = ["derive"] }
```

```toml
[dependencies]
serde = { loaf = "crates-io/serde", version = "1", features = ["derive"] }
```

A dependency written as a bare version string is refused the same way:

```text
incan check: /path/to/app/loaf.toml: ["dependencies", "helper"]: invalid: dependency must name its Loaf; use helper = { loaf = "crates-io/helper", version = "1.0" }
```

A Rust crate must be in the installed toolchain's compiled registry closure. A crate that is not there is refused when the project builds, for example `crates-io/hound is not published in the compiled registry closure`.

### A local Cargo crate becomes a Rust-facet Loaf

A `[rust-dependencies]` entry with a `path` names a directory that gets its own `loaf.toml` with a `[rust]` table.

```toml
[rust-dependencies]
native_helper = { path = "native" }
```

```text
incan check: /path/to/app/loaf.toml: ["rust-dependencies"]: unsupported: use [dependencies] native_helper = { loaf = "native_helper", path = "native" }, with a native/loaf.toml whose [rust] table declares the crate
```

```toml
[dependencies]
native_helper = { loaf = "native_helper", path = "native" }
```

and `native/loaf.toml`, beside the crate's `src/lib.rs`:

```toml
[project]
name = "native_helper"
version = "0.1.0"

[rust]
type = "lib"
```

The crate's own Cargo dependencies move into `native/loaf.toml`'s `[dependencies]` in the same Loaf form.

### Git crates have no 0.6 source

0.6 takes Rust crates from the registry or a local path. A `git` entry is refused with the registry form, which works only if the crate is published:

```text
incan check: /path/to/app/loaf.toml: ["rust-dependencies"]: unsupported: use [dependencies] fancy = { loaf = "crates-io/fancy", version = "*" }
```

### Test-only crates move to the `test` environment

`[rust-dev-dependencies]`, and `[rust.dev-dependencies]`, become `[envs.test.dependencies]`:

```toml
[rust-dev-dependencies]
serde_json = "1"
```

```text
incan check: /path/to/app/loaf.toml: ["rust-dev-dependencies"]: unsupported: use [envs.test.dependencies] serde_json = { loaf = "crates-io/serde_json", version = "1" }
```

```toml
[envs.test.dependencies]
serde_json = { loaf = "crates-io/serde_json", version = "1" }
```

`[rust.dependencies]` becomes `[dependencies]`:

```text
incan check: /path/to/app/loaf.toml: ["rust", "dependencies"]: unsupported: use [dependencies] serde = { loaf = "crates-io/serde", version = "1" }
```

### Environments move to `[envs.NAME]`

```toml
[tool.incan.envs.ci]
env-vars = { CI = "1" }
```

```text
incan check: /path/to/app/loaf.toml: ["tool", "incan", "envs"]: unsupported: [tool.incan.envs] is refused; declare each environment as [envs.NAME]
```

```toml
[envs.ci]
env-vars = { CI = "1" }
```

0.5's `default` environment is `dev` in 0.6. Run an environment's scripts with `oven env`, as the [commands](#change-the-commands) section shows.

### `[tool.incan]` keys 0.6 does not read are ignored

`[tool.incan.metadata]` and `[tool.incan.codegraph]` are read as before. Any other key is accepted with a warning:

```text
incan check: warning: /path/to/app/loaf.toml: [tool.incan] key `newer` is not read by this toolchain and is ignored
```

### There is no SDK selection

Standard packages are always available, so `[sdk]` and a feature's `requires-sdk-components` are refused. Delete them:

```text
incan check: /path/to/app/loaf.toml: ["sdk"]: unsupported: there is no SDK selection: standard packages are ambient, and a unit reaches every one it uses
```

```text
incan check: /path/to/app/loaf.toml: ["project", "features", "gpu", "requires-sdk-components"]: unsupported: there is no SDK selection: standard packages are ambient, and a unit reaches every one it uses
```

### C interop moves to `[interop.c]`

```text
incan check: /path/to/app/loaf.toml: ["oven", "interop"]: unsupported: [oven.interop] is refused; declare C interop under [interop.c]
```

### A vocabulary names its entry module

```toml
[vocab]
crate = "app_vocab"
```

```text
incan check: /path/to/app/loaf.toml: ["vocab", "crate"]: unsupported: [vocab] crate is refused; declare [vocab] entry
```

Replace `crate` with `entry`, as the refusal says. See [`[vocab]`](../reference/project_configuration.md#vocab) in the configuration reference.

### A script naming the library is ignored

A `[project.scripts]` entry that points at the conventional library, such as `library = "src/lib.incn"`, is accepted and ignored: the library is built as a library, never as an executable. Delete the line to silence the warning:

```text
incan check: warning: /path/to/app/loaf.toml:6: [project.scripts] library = "src/lib.incn" names the conventional library, which is built as a library, not as an executable; the entry is ignored
```

### `requires-incan` on a 0.6 dev toolchain

A dev toolchain satisfies `requires-incan` by its release version, so `0.6.0-dev.8` meets `>=0.5` but not `<0.6`. `check` reads the requirement without enforcing it; `run`, `build` and `test` refuse:

```text
incan run: run refused: active Incan toolchain 0.6.0-dev.8 does not satisfy requires-incan >=0.5,<0.6
```

Widen the requirement, for example to `requires-incan = ">=0.5"`.

### Workspaces

`[workspace]` keeps `members`, `default-members` and `exclude`, `[workspace.dependencies]`, `[workspace.envs.NAME]`, `[workspace.policy]` and `[workspace.sources]`, and members inherit with `NAME = { workspace = true }` as before. The Rust tables move:

```text
incan check: /path/to/app/loaf.toml: ["workspace", "rust-dependencies"]: unsupported: use [workspace.dependencies] serde = { loaf = "crates-io/serde", version = "1" }
```

```text
incan check: /path/to/app/loaf.toml: ["workspace", "rust-dev-dependencies"]: unsupported: use [workspace.dependencies] serde_json = { loaf = "crates-io/serde_json", version = "1" }, and serde_json = { workspace = true } in each member's [envs.test.dependencies]
```

An inheriting member takes its path and identity from the root, so a path of its own is refused:

```text
incan check: /path/to/app/loaf.toml: ["dependencies", "helper", "path"]: invalid: `helper` inherits from the workspace: `path` and its other identity fields belong to [workspace.dependencies]
```

A member reaches a root's environment fragment as `extends = ["workspace:NAME"]`. See [Workspaces](../../language/how-to/workspaces.md).

## Change the source

### Methods are private unless marked `pub`

A method called from outside its type must be `pub`.

```incan
class Dog:
    pub name: str

    def fetch(self) -> str:
        return f"{self.name} fetches the ball!"


def main() -> None:
    dog = Dog(name="Buddy")
    print(dog.fetch())
```

```text
type error: Method 'fetch' on 'Dog' is private
  --> main.incn:4:5
    |
  4 |     def fetch(self) -> str:
    |     ----------------------- declaration of `fetch`
  --> main.incn:10:11
     |
  10 |     print(dog.fetch())
     |           ^^^^^^^^^^^
  = hint: Call this method from a method on the declaring type, or mark the method `pub`
```

```incan
    pub def fetch(self) -> str:
        return f"{self.name} fetches the ball!"
```

### A caller's constructor cannot set a private field

0.5 let a caller construct a class whose fields are not marked; 0.6 refuses each such field with `INCAN-T0135`. Mark the fields `pub`, or construct the value through a `pub` method of the type.

```incan
class Dog:
    name: str
    breed: str

    def describe(self) -> str:
        return f"{self.name} is a {self.breed}"


def main() -> None:
    dog = Dog(name="Buddy", breed="Golden Retriever")
    print(dog.describe())
```

```text
error[INCAN-T0135]: Private field 'name' cannot be set while constructing class 'Dog' here
  --> main.incn:10:15
     |
  10 |     dog = Dog(name="Buddy", breed="Golden Retriever")
     |               ^^^^ private constructor field named here
  = note: Only methods on the declaring class may name its private constructor fields
  = hint: Call a `pub` constructor method on 'Dog', or make fields intended for direct construction `pub`
```

The same error follows for `breed`, and the method needs `pub` as above:

```incan
class Dog:
    pub name: str
    pub breed: str

    pub def describe(self) -> str:
        return f"{self.name} is a {self.breed}"
```

### A `const` collection is frozen

A `const` is deeply immutable, so its collection annotation names the frozen type.

```incan
const NUMS: List[int] = [1, 2, 3]
```

```text
type error: const 'NUMS' is annotated 'List[int]', but a const is deeply immutable and is represented as 'FrozenList[Int]'
  --> main.incn:1:13
    |
  1 | const NUMS: List[int] = [1, 2, 3]
    |             ^^^^^^^^^
  = note: A frozen collection does not read where the mutable container is expected, so the written annotation could not be honored at any use site
  = hint: Annotate the const as 'FrozenList[Int]', or omit the annotation to infer it
```

```incan
const NUMS: FrozenList[int] = [1, 2, 3]
```

`Dict` and `Set` annotations become `FrozenDict` and `FrozenSet` the same way.

### A Rust crate import names its version

An import from a crate outside the standard library names the crate's version, and the features it needs, in the import itself.

```incan
from rust::uuid import Uuid
```

```text
incan run: unknown Rust crate `uuid`: no version specified; add @ "VERSION" to its import
```

```incan
from rust::uuid @ "1.0" with ["v4"] import Uuid
```

### A `rusttype` conversion method cannot be named `parse`

A conversion method named `parse` on a `rusttype` over `String` is refused:

```incan
from rust::std::string import String as RustString

type Email = rusttype RustString:
  def parse(raw: str) -> Email:
    ...

  interop:
    from str via Email.parse
```

```text
type error: Rust method `rust::std::string::String::parse` has no admitted callable signature
  --> main.incn:8:5
    |
  8 |     from str via Email.parse
    |     ^^^^^^^^^^^^^^^^^^^^^^^^
```

Give the conversion another name, for example `from`, and call it by that name:

```incan
type Email = rusttype RustString:
  def from(raw: str) -> Email:
    ...

  interop:
    from str via Email.from
```

### Integer arithmetic takes one integer type

Integer arithmetic over two different integer types is refused. Indexing `bytes` yields a `u8`, so a byte combined with an `int` needs a conversion:

```incan
def padding_set(payload: bytes, padding_bits: int) -> bool:
    padding_mask = (1 << padding_bits) - 1
    return payload[len(payload) - 1] & padding_mask != 0
```

```text
type error: Mixed-width integer arithmetic: 'u8 & Int' has operands of two integer types
  --> main.incn:3:12
    |
  3 |     return payload[len(payload) - 1] & padding_mask != 0
    |            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  = note: Integer arithmetic takes two operands of one integer type and yields that type
  = hint: Convert one operand to the other operand's type first, with 'resize()' where the conversion is lossless or 'try_resize()' where the value may not fit
```

Convert the byte with `int(...)`. In this position `resize()` has no target type to resize to:

```incan
    return int(payload[len(payload) - 1]) & padding_mask != 0
```

## Change the commands

Project commands that 0.6 runs through Oven take the `oven` name, and the release profile is named:

| 0.5 | 0.6 | What 0.6 prints for the 0.5 spelling |
| --- | --- | --- |
| `incan lock` | `oven lock` | ``incan: unknown command `lock`: run `oven lock` `` |
| `incan env list` | `oven env list` | ``incan: `incan env` is an Oven command: run `oven env` `` |
| `incan build src/main.incn --release` | `incan build src/main.incn --profile release` | ``incan build: unknown option `--release`: select the release profile with `--profile release` `` |

`incan run`, `check` and `build` without a path select the Loaf at or above the current directory. `incan test` without a path tests the current directory. See the [CLI reference](../reference/cli_reference.md).
