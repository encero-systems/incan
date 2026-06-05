# Incan vs Python

Python is the default choice when ecosystem reach, hiring familiarity, notebooks, and package availability matter most. Incan should not be chosen just because a program could be written in a Python-like syntax.

Choose Incan when you want Python-shaped application code but do not want Python's runtime and deployment tradeoffs.

## Where Python wins

- Existing packages, especially for data science, notebooks, AI/ML, and web frameworks.
- Team familiarity and hiring.
- Fast one-file scripts where runtime correctness risk is low.
- Interactive exploration.
- Compatibility with the broader Python packaging ecosystem.

## Where Incan is trying to win

- New application code that benefits from static type checking before runtime.
- Tools, services, and workflows where deployment should produce a native binary.
- Codebases where errors and mutability should be explicit in review.
- Rust ecosystem access without forcing every line of application code to be Rust.
- Agent-generated code where a smaller typed surface is easier to audit than dynamic Python.

## The honest tradeoff

Python has the ecosystem. Incan has to earn every library, tool, and example. That means Incan is a bad fit if the first question is, "Can I use all my Python packages?"

The better question is, "Would this new tool or service be safer and easier to ship if it were typed, native, and still readable to a Python-minded developer?"

## Decision guide

| Use Python when... | Use Incan when... |
| --- | --- |
| You need existing Python libraries. | You are writing new application logic. |
| You are exploring data interactively. | You want a native binary. |
| Runtime flexibility matters more than static guarantees. | Reviewable contracts matter more than dynamic flexibility. |
| A script will stay small. | A script is becoming a product, service, or governed workflow. |

## Source notes

- Stack Overflow's 2025 Developer Survey says Python adoption "accelerated significantly" and ties it to AI, data science, and backend work: [Technology | 2025 Stack Overflow Developer Survey](https://survey.stackoverflow.co/2025/technology/).
- Meta's typed Python survey reports that 88% of respondents often or always use types in Python code, while also naming usability, latency, and library typing gaps as pain points: [Typed Python in 2024](https://engineering.fb.com/2024/12/09/developer-tools/typed-python-2024-survey-meta/).
