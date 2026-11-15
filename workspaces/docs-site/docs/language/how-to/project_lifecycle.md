# Create and maintain a project

## Create a project

Create the directories yourself:

```bash
mkdir -p greeter/src greeter/tests
cd greeter
```

Write `loaf.toml` with the package identity:

```toml
[project]
name = "greeter"
version = "0.1.0"
```

Add the entry source and tests as shown in [Getting started](../../tooling/tutorials/getting_started.md). Dev.8 does not provide `new` or `init`.

## Choose a source layout

Put application modules under `src/`. For another layout, set `[build] source-root` to a project-relative directory. Keep tests in separate source files and import public application functions from that source root.

## Run the project

```bash
incan run src/main.incn
incan test tests/test_main.incn
incan build src/main.incn
```

Pass each test file explicitly. A manifest script declaration does not supply an omitted CLI path.

## Change metadata

Edit `[project] version` directly when releasing your package. Run checks after a manifest change. Dev.8 has no version-bump or named-environment command.

For several projects in one repository, see [Work with several projects](workspaces.md).
