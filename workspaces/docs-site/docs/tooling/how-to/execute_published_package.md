# Execute a published package without native linking

Use the replacement backend when you want to execute supported public calls from an already built library without compiling or linking the consumer's generated Rust.

You need an Incan 0.6 development compiler, a consumer project with declared package dependencies, and complete built artifacts for those dependencies. The producer must publish executable coverage for the calls and types you use. For initial project setup, follow [Build and consume an Incan library](../tutorials/build_and_consume_library.md); its native example does not establish replacement coverage for every value shape.

## Run the consumer

From the consumer project directory, select the source entrypoint and backend:

```bash
incan build src/main.incn --backend replacement
```

Despite the command name, this backend executes the program directly. Program output appears on its ordinary streams; it does not produce a native executable. The dependency's original Incan source files need not be present, provided its selected artifact and dependency closure remain available.

Keep each built library artifact together when moving it. Copying only its `.incnlib` manifest loses the semantic content that this route requires. See the [artifact layout](../reference/package_executable_representation.md#artifact-layout).

## Record an execution report

`--report-output` is required here, not merely advisable. Replacement execution keeps stdout and stderr for the program, so the report needs a path of its own; omitting it is an error rather than merged output:

```bash
incan build src/main.incn --backend replacement --report json --report-output execution.json
```

After success, inspect `execution.json` for the result, program output and package-loading counters. The command also writes `.incan/backend/receipt.json`; inspect it with:

```bash
incan inspect backend-selection --receipt .incan/backend/receipt.json
```

## Handle an unavailable package declaration

Use the diagnostic's package, version and requirement to identify the dependency that needs attention. If its executable content is absent or incompatible, rebuild that dependency with a compatible compiler and make the complete resulting artifact available to the consumer. If the declaration is uncovered, its implementation or the replacement execution profile must support the required operations before this route can use it.

To execute through the native route instead, run the program without selecting the replacement profile:

```bash
incan run src/main.incn
```

`incan run` always uses the native route; the replacement profile is reached only by asking for it with `incan build --backend replacement`. A package whose executable content is absent or uncovered still links natively, so the native route remains available whenever the representation cannot serve a requirement. The [representation reference](../reference/package_executable_representation.md#refusals) defines the coverage and refusal conditions.
