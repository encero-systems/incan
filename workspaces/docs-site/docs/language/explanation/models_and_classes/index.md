# Models and Classes in Incan

Incan provides two ways to define types with fields: `model` and `class`.
Understanding when to use each is key to writing idiomatic Incan code.

- `model`: data-first (DTOs, configs, payloads, wire formats)
- `class`: behavior-first (services, stateful objects, inheritance/overrides)

## Quick start

If you're unsure which one to pick:

- Start with a `model` when you're defining **data you store or exchange** (payloads, configs, schema-shaped data).

  ```incan hl_lines="2"
  @derive(Serialize, Deserialize)
  model User:
      id: int
      email: str
  ```

- Start with a `class` when you're defining an **object with behavior** (methods, mutable state, inheritance).

  ```incan hl_lines="1"
  class UserService:
      repo: UserRepo

      def find(self, id: int) -> Option[User]:
          return self.repo.find(id)
  ```

Next: [Models](models.md) and [Classes](classes.md).

## Quick Comparison

| Aspect            | `model`                        | `class`                         |
| ----------------- | ------------------------------ | ------------------------------- |
| **Purpose**       | Data containers                | Objects with behavior           |
| **Focus**         | Fields first                   | Methods first                   |
| **Inheritance**   | No (cannot inherit)            | Yes (single inheritance)        |
| **Traits**        | Yes (behavior via traits)      | Yes (behavior via traits)       |
| **Schema mapping**| Yes (field metadata/aliases)   | No (canonical field names only) |
| **Use when**      | "This is data"                 | "This is an object/service"     |

!!! tip "Glossary"
    - **DTO (Data Transfer Object)**: a type whose job is to carry data between layers or systems
    - **wire format**: how data looks when sent over the network (e.g., JSON keys)
    - **schema**: the expected shape/structure of data (field names, types)
    - **serialization**: converting an object to a wire format (e.g., JSON)

## Fundamental differences

A `model` and a `class` are not interchangeable. They compile to Rust structs, but they represent **different language concepts**:

- **Models define data shapes**. They are the canonical representation for schema-like data and are the target for
  schema-focused features like field metadata and aliases.
- **Classes define behavior**. They can inherit, override methods, and model stateful objects and services.
- **Feature surface is intentionally different**: field aliases/metadata are **model-only**, while inheritance is **class-only**.

## When to Use Which

| Use Case             | Choose  | Why                                |
| -------------------- | ------- | ---------------------------------- |
| Config, settings     | `model` | Pure data, no behavior needed      |
| DTO, API payload     | `model` | Data transfer, serialization focus |
| Database record      | `model` | Represents stored data             |
| Service with methods | `class` | Has operations/behavior            |
| Stateful controller  | `class` | Methods that modify state          |
| Needs inheritance    | `class` | Only `class` supports `extends`    |


??? info "Coming from Rust?"
    Both `model` and `class` compile to Rust `struct`s + `impl`s. The difference is semantic:

    - `model` = "this is data" (like a plain struct)
    - `class` = "this is an object with behavior" (can add inheritance + trait composition)

    The compiler resolves inheritance at compile time by generating fields/methods/trait impls.
    In practice, this is still "zero-cost": choose `model` vs `class` for clarity and API design, not performance.

    **Why no `struct` keyword?**  
    Incan uses `model` and `class` instead because:

    1. **Python familiarity** — Python developers know `class`, not `struct`
    2. **Clearer semantics** — `model` says "this is data", not "this is a memory layout"
    3. **Tooling conventions** — ORMs, validators, serializers all use "model" terminology

??? info "Coming from Python?"
    A rough mapping:

    - Python `@dataclass` / pydantic `BaseModel` / "plain data" → `model`
    - Python "class with behavior" → `class`
    
    Construction is field-based (no `__init__`). Mutation requires `mut self`.

??? info "Coming from TypeScript / JavaScript?"
    A rough mapping:

    - TypeScript `interface` / `type` (for data shapes) → `model`
    - TypeScript `class` (for services/controllers) → `class`
    
    Use `model` for API request/response payloads (DTOs). Use `class` for services with methods.

## Where to go next

- [Models](models.md) — how models work (construction, defaults, aliases/metadata, validation, serialization)
- [Classes](classes.md) — how classes work (inheritance, `mut self`, overrides, trait composition, patterns)
