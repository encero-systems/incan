# Incan vs JS/TS

JavaScript and TypeScript are the right default when the browser, Node.js, npm packages, or frontend frameworks are central to the product. Incan should not be chosen just because a tool or service could be written with familiar high-level syntax.

Choose Incan when the code is new, application-shaped, and should ship as a native binary with compiler-owned contracts and inspection data.

## Where JavaScript and TypeScript win

- Browser applications and UI frameworks.
- Node.js services that depend heavily on npm packages.
- Teams already standardized on TypeScript tooling.
- Fast iteration where runtime deployment is already solved.
- Full-stack projects where sharing types across frontend and backend matters most.

## Where Incan is trying to win

- CLIs, services, workflows, and domain packages that should deploy without a JavaScript runtime.
- Application code where errors, optionality, and mutability should stay visible in source review.
- Agent-generated code where a smaller typed surface is easier to inspect than a large framework stack.
- Native artifacts with Rust ecosystem boundaries and generated Rust that can be inspected.
- Toolchains that need stable diagnostics, build reports, and compiler-owned project facts.

## The honest tradeoff

TypeScript has the ecosystem, framework reach, and hiring base. Incan does not compete with the browser or npm. It is a poor fit when the application is mostly UI, when existing JavaScript packages define the product, or when runtime flexibility is the main advantage.

Incan is a better question when a TypeScript service, script, or workflow is becoming infrastructure: something that needs static contracts, explicit failure paths, native deployment, and artifacts a compiler can explain.

## Runtime boundary

TypeScript improves JavaScript authoring, but the deployed program still runs in a JavaScript runtime. Incan uses readable source as the authoring layer and moves the deployment path toward native artifacts.

| TypeScript expectation | Incan position |
| --- | --- |
| Types help authoring and are erased before JavaScript runs. | Types are part of the compiler contract before lowering and native build output. |
| npm packages are the integration boundary. | Rust crates, Incan packages, and explicit toolchain metadata are the integration boundary. |
| Exceptions and dynamic values are common runtime paths. | Fallible paths and optional values should be visible through typed source constructs. |
| Deployment targets Node.js, browsers, or edge JavaScript runtimes. | Deployment targets native artifacts through the Incan/Rust build path. |

## Decision guide

| Use JavaScript or TypeScript when... | Use Incan when... |
| --- | --- |
| The browser is the product boundary. | The runtime should be a native binary. |
| npm packages define the application. | Rust crates or native tooling define the boundary. |
| Frontend/backend type sharing is the main value. | Compiler-owned diagnostics and artifacts are the main value. |
| Runtime flexibility matters more than static deployment shape. | Explicit contracts and failure paths matter more than dynamic flexibility. |
