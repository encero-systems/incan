# Work with several projects

Give each project its own `loaf.toml`, source tree and package identity. Connect reusable libraries through explicit path dependencies. Dev.8 does not accept `[workspace]`, inherited workspace declarations or `--member` selection.

## Connect a consumer to a producer

```text
repository/
├── producer/
│   ├── loaf.toml
│   └── src/lib.incn
└── consumer/
    ├── loaf.toml
    └── src/main.incn
```

In the consumer manifest:

```toml
[project]
name = "consumer"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
pricing = { path = "../producer" }
```

Import the producer's public exports through `pub::pricing`. Run the consumer by its source path:

```bash
incan run consumer/src/main.incn
```

The source path selects the consumer's manifest even when invoked from the repository root. Oven prepares its path dependencies. There is no shared workspace lock or implicit fan-out.

Follow [Build and consume a library](../../tooling/tutorials/build_and_consume_library.md) for the complete source.
