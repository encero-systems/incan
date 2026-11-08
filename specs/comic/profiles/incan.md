# Incan profile

Status: draft. Profile ID `incan/1`. This profile describes how Incan programs and the Rust and C code they use appear as COMIC records: roots, kinds, functions and the code they hold. The neutral core in [spec](../spec/) does not depend on it. Where this profile and the core disagree, the core decides.

## Roots

A path in this profile starts at one of these roots:

| Root | Means | Example |
| --- | --- | --- |
| `pub` | A published Loaf, by its registry identity | `pub.crates-io.hex.encode` |
| `std` | The Incan standard library | `std.primitives.json.JsonEncoder` |
| `rust` | Rust's own crates | `rust.alloc.vec.Vec` |
| Any other | A local Loaf, meaningful only inside its project | `shop.billing.Cents` |

Segments after `pub` may contain hyphens, which an Incan expression cannot. An optional `registry` field on a binding names a private registry. A Loaf's registry identity, its code namespace and its records are separate: `spark = { loaf = "apache/spark" }` is imported in source as `from pub::spark.sql import SparkSession`, and its record is `pub.apache.spark.sql.SparkSession`.

Builtin types (`Int`, `Float`, `Str`, `Bool`, `Bytes`, `Decimal`) have no path or prefix, because they are the same type in every Loaf. Type arguments follow a path in `[ ]`, as `shop.Box[Int]`. A repeated trait adoption is a namespace, as `shop.Reading.Convert[Int].convert`.

## Kinds

A binding's kind is named by its language, because one word can mean different things: a C enum is open and a Rust enum is closed. Each language owns its kinds, and a new language adds a namespace of its own. The kinds include:

- **Incan:** `Incan.Model`, `Incan.Class`, `Incan.Enum`, `Incan.Newtype`, `Incan.Type`, `Incan.Trait`, `Incan.Function`, `Incan.Closure`, `Incan.Module`, `Incan.Proof`.
- **Rust:** `Rust.Struct`, `Rust.Enum`, `Rust.Trait`, `Rust.Function`, `Rust.DeriveMacro`, `Rust.Macro`.
- **C:** `C.Struct`, `C.Enum`, `C.Union`, `C.Function`, `C.Macro`.

A path and a kind together are unique, so one name can hold two kinds: `pub.crates-io.serde.Serialize = Rust.Trait { … }` and `pub.crates-io.serde.Serialize = Rust.DeriveMacro { … }` are two bindings. An overload adds its parameter types, as `overload = [shop.ColumnExpr, Type[Int]]`. Each language's block holds that language's own facts, such as `Rust.Struct { lifetimes = ["a"] }` or `C.Struct { header = "zstd.h", size = 24, align = 8 }`. An optional `binding` field names the toolchain, target, profile and feature set a description holds for, because a Rust item can exist only on one target or gain a field from a feature.

`Incan.Model` and `Incan.Class` map to the core `Record`, and `Incan.Enum` to the core `Enum`, so a document's data shapes and its program records are one form. A model is described the way it is written in Incan, with its fields as `name: Type` and Incan's own facts as properties:

~~~text
shop.billing.Invoice = Incan.Model {
    visibility = Public
    id: Int
    customer: Str
    total: shop.billing.Cents
}

shop.billing.Status = Incan.Enum { Draft, Sent, Paid(on: Str) }
~~~

This is the model's schema descriptor (RFC 086) printed, not a second description of a model.

## Modules and links

A module is a binding, and an import is a link on it:

~~~text
shop.billing = Incan.Module {
    links = [
        (name="encode", target=pub.crates-io.hex.encode),
        (name="web", target=std.web),
    ]
}
~~~

A binding references the real target, never a link.

## Functions

A function is one block holding what callers see, `type`, and what it does, `body`:

~~~text
shop.billing.charge = Incan.Function {
    visibility = Public
    decorators = [
        Decorator(std.web.post, ["/invoices/charge"]),
        Decorator(shop.audit.logged, ["billing"]),
    ]
    type = {
        parameters = [
            (name="invoice", type=shop.billing.Invoice),
            (name="amount", type=shop.billing.Cents),
        ]
        returns = Result[shop.billing.Cents, Str]
        requires = [Compare(Gt, Parameter(1, "amount"), Construct(shop.billing.Cents, [0]))]
    }
    body = [
        Return(Ok(Construct(shop.billing.Cents, [
            Binary(Sub, Project(Field(Parameter(0, "invoice"), "total"), 0), Project(Parameter(1, "amount"), 0)),
        ]))),
    ]
}
~~~

- **Decorators** sit on the function, in order, outside `type`, because changing one changes behavior and not what callers compile against. A decorator that changes the callable's type changes `type`, which reports what callers actually call; a `declared` field then holds the written type.
- **Contracts and capabilities** sit in `type`, because callers rely on them.
- **A closure** is the same block with `captures` and no path, so it sits where it is written.
- **A Rust or C function** has no `body`, because its body is opaque.
- **Code nodes** are enum variants, so they print positionally, as `Compare(Gt, a, b)`. Every name in a body is a slot or an absolute path, so a record has no scope.
- **Result variants** print unqualified, as `Ok(2)` and `Err("bad")`.

## Declaration identity and change

Whether a declaration changed is answered by Incan's declaration identity and digests (RFC 106): a semantic digest over checked meaning, a closure digest over dependencies and a documentation digest. This profile prints those and defines no second declaration digest. The core's canonical, object and model projections stay for stored values: a model's hash must leave paths out, so that moving a type does not force a migration.

A proof is a value, not a binding, because a function can have one proof per obligation. It anchors to the function's declaration identity and digest; RFC 111 owns its obligation, assumptions, outcome and verifier. A proof goes stale when that digest moves.

## Values and diagnostics

- `SecretStr` and `SecretBytes` map to the core's `secret_string` and `secret_bytes`. Incan keeps secrets out of stored data: a model holding one cannot be crumbled or stored, so Incan writes `<secret>` only in printed text.
- A value with private state, such as a Rust `Duration`, prints as a marker with its own debug text, `<Duration: 1.5s>`. C pointers, handles and untagged unions print as `<ZSTD_CCtx>` or `<union Value>`. Reading or crumbling one is refused unless the type provides a conversion.

## C

C has no modules, so a C binding sits directly under its Loaf, as `pub.c.zstd.ZSTD_compress`. `char*` is `Bytes`. `long` under a given `binding` is `Int { storage = i64 }` or `Int { storage = i32 }`. An open enum prints `Mode.Read` when the value is exactly a variant and `Mode(3)` when it is not, and the first declared name wins when two share a value. A macro that is not a constant is `C.Macro { text = "…" }`, the one place a body stays a string; Rust's `macro_rules!` is `Rust.Macro` the same way.

## Vocabularies and surfaces

A vocabulary or source surface that lowers to the same tree has the same canonical hash, so translating a document is not a code change. A block vocabulary's description holds `surface`, as written and noncanonical, and `body`, what it became. `generated_by` names the provider by its full identity. A generated name comes from the author's name or the canonical hash of the input, never a counter.

## Lock files

Proposed: `oven.lock` contents as COMIC. The lock declares its own shape; the locked Loaves are one table and the root is one value. `Loaf` is this profile's reference to a Loaf, written as its path. A reference spells a version, as `pub.crates-io.hashbrown[0.14.5]`, only when the lock holds two versions of one Loaf; a locked row always has its version in its own column. Digests are shortened here:

~~~text
oven.Lock = Record {
    root: Str
    mode: oven.Mode
    registry: oven.Registry
    toolchain: Str
    target: Str
    dependencies: List[Loaf]
}

oven.Locked = Record {
    loaf: Loaf
    version: Str
    digest: Str
    trust [canonical = false]: oven.Trust
    satisfied_by [canonical = false]: oven.Satisfied
    target_features: List[Str] = []
    dependencies: List[Loaf] = []
}

oven.Mode = Enum { Lowest, Highest }

oven.Registry = Record { endpoint: Str, protocol: Str }

oven.Satisfied = Enum { Source, Asset }

oven.Trust = Enum { Verified(signer: Str), Unverified }

oven.Locked << [
    (pub.crates-io.aho-corasick, "1.1.4", "sha256:558ce675…", Verified("…"), Source, ["perf-literal", "std"], [pub.crates-io.memchr]),
    (pub.crates-io.memchr, "2.8.0", "sha256:a734fee1…", Verified("…"), Source, ["alloc", "std"]),
]

oven.Lock << (
    root="regex_tutorial",
    mode=Lowest,
    registry=(endpoint="https://loaves.pub", protocol="sparse/1"),
    toolchain="rustc 1.98.0 (88d9e12ae 2026-08-18)",
    target="aarch64-apple-darwin",
    dependencies=[pub.crates-io.regex],
)
~~~

`satisfied_by` and `trust` are noncanonical, so a newly baked asset moves the object projection and not the graph identity. The resolved graph identity binds each entry's origin, name, version and dependency edges, shaped like RFC 106's closure digest. `oven.lock` stays TOML until this proposal is adopted.

## Open

- Which tree a function body prints. The tree as written (RFC 160) leaves names unresolved, so a reference is only a word. The compiler's checked tree is resolved but private, so a stored body would break when the compiler changes. Proposed: SCIR's versioned, read-only inspection view (RFC 161), which is resolved, versioned and the form every source surface lowers into, at the cost of spelling out copies, moves and borrows.
