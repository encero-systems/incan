# New to typed programming

Use this route if you can read small programs but types, explicit failures, or native builds are still unfamiliar. You do not need to understand Rust before starting, and you do not need to read the reference in order.

## The short route

1. **Complete [Getting Started](../tooling/tutorials/getting_started.md).** It owns the supported installation choices and first-project commands. The direct installer can provision the pinned Rust 1.98.0 backend; the other installation channels tell you when Rust must be managed separately.
2. **Set up [editor feedback](../tooling/how-to/editor_setup.md).** Diagnostics beside the source are easier to learn from than a long terminal error after several edits.
3. **Read the first six chapters of [The Incan Book](../language/tutorials/book/index.md).** Stop after the errors chapter and complete each exercise before continuing.
4. **Build [your first real project](../tooling/tutorials/your_first_project.md).** This is where modules, tests, failures, and a release build become one coherent workflow.
5. **Return to the Book for models, traits, and tests.** Use the [glossary](../language/reference/glossary.md) only when a word blocks you.

## Four ideas to recognize

| Idea | What it means while learning |
| --- | --- |
| Type | A checked description of the values a name may hold. |
| Function | A named operation with explicit inputs and an explicit result. |
| `Result` | A value that records either success or a typed failure. |
| Test | Executable evidence that a small piece of behavior still works. |

You do not need to memorize the generated Rust or ownership rules. Read the Incan source, make failures explicit, and let the compiler explain the boundary when it needs more information.

## You are ready to choose your own route when

- you can create and run a project without copying an unexplained command;
- you can read a function signature and identify its inputs and result;
- you can tell the difference between `Ok(...)` and `Err(...)`;
- you have changed a test, watched it fail, and repaired the program; and
- you know where to look next: a tutorial to learn, a how-to to complete a task, or the reference to look up exact behavior.

If the first project fails before you reach the language material, use [Troubleshooting](../tooling/how-to/troubleshooting.md). RFCs and contributor architecture are design records, not prerequisites for this route.
