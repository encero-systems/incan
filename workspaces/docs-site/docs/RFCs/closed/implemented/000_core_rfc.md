# RFC 000: Incan Core Language RFC (Phase 1)

- **Status:** Implemented
- **Created:** 2024-11-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** —
- **Issue:** —
- **RFC PR:** [#50](https://github.com/encero-systems/incan/pull/50)
- **Written against:** v0.1
- **Shipped in:** v0.1

This RFC consolidates the core semantics decisions for Incan's first implementation phase.

## 1. Core Semantics

### 1.1 Typing Model

- **Required types**: All bindings, parameters, and return types must have explicit types; no gradual or optional typing.
- **Compile-time enforcement**: Type mismatches fail at compile time, not runtime.
- **Inference**: Local type inference is allowed where unambiguous (e.g., `x=42` infers `int`), but function signatures require explicit types.

### 1.2 Bindings and Mutability

- **Immutable by default**: Bindings are immutable unless marked `mut`.
- **Syntax**: `x = value` for immutable; `mut x = value` for mutable.
- **Optional `let`**: `let x = value` is equivalent to `x = value`; `let` is optional for Pythonic feel.

### 1.3 Receivers (`self` / `mut self`)

- **`self` is a keyword**, not a convention; it must appear explicitly in method signatures.
- **Immutable receiver**: `def method(self, ...)` — read-only access to fields.
- **Mutable receiver**: `def method(mut self, ...)` — can mutate fields.
- **Static dispatch**: Method calls resolve statically unless trait objects are used.
- **No full borrow checker**: Simplified mutability model (`self` vs `mut self`) without Rust's full ownership/borrowing.

### 1.4 Error Handling (`Result` / `?`)

- **`Result[T, E]`**: Primary error type; represents `Ok(value)` or `Err(error)`.
- **`Option[T]`**: Represents `Some(value)` or `None`; for optional values.
- **`?` operator**: If expression is `Err(e)`, early-return with that error; if `Ok(v)`, unwrap to `v`.
- **Compile-time enforcement**: Functions using `?` must return a compatible `Result`.
- **Panics are exceptional**: Reserved for unrecoverable errors; normal errors use `Result`.
- `unwrap()`/`expect()` are for unrecoverable paths only; normal errors use `Result` + `?`.
- **Optional sugar**: `Result[T, E1 | E2]` may be provided as syntax sugar, desugaring to a generated enum so `?` still operates on a single concrete error type. Prefer one error type per function; use `map_err` / `From` conversions as needed.

### 1.5 Traits

- **Behavior-only**: Traits define methods only; no storage/fields. Traits must not declare fields; if a trait assumes fields exist, they must be provided by the adopter. This is enforced at compile time.
- **Default methods**: Traits can provide default implementations that may assume fields exist on the adopter.
- **Deterministic composition**: Multiple traits compose left-to-right; order is explicit and compile-time resolved.
- **No Python MRO**: No dynamic method resolution order; conflicts must be resolved explicitly.
- **No runtime patching**: Traits cannot be added at runtime; no monkey-patching.
- **Conflict resolution**: When multiple traits provide the same method, the adopting class must explicitly resolve (e.g., `TraitA.method(self, ...)`).
- **Optional `@requires`**: Annotation to declare expected fields/methods on adopters.

```incan
trait Loggable:
  def log(self, msg: str):
    print(f"[{self.name}] {msg}")   # assumes self.name exists

# With explicit requires (optional ergonomic)
@requires(name: str)
trait Loggable:
  def log(self, msg: str):
    print(f"[{self.name}] {msg}")
```

### 1.6 Models

- **Data-first containers**: Declarative fields with optional validation.
- **Own storage**: Models own their fields.
- **Validation**: Via `@derive(Validate)` or manual `validate(self) -> Result[Self, E]` methods.
- **Cannot inherit from classes**: Models are not classes; they are data containers.
- **Can implement traits**: Models can adopt storage-free traits.

```incan
@derive(Validate)
model User:
  id: UserId
  email: Email
  is_active: bool = true

  def validate(self) -> Result[User, ValidationError]:
    if not self.is_active and not self.email.as_str().endswith("@example.com"):
      return Err(ValidationError.InactiveExternal)
    return Ok(self)
```

### 1.7 Classes

- **General-purpose types**: Fields, methods, single inheritance, trait composition.
- **Own storage**: Classes own their fields.
- **Single inheritance**: `class Child extends Parent`.
- **Trait composition**: `class Service with TraitA, TraitB`.

```incan
class Service with Loggable:
  name: str
  repo: UserRepo

  def onboard(self, user: User) -> Result[User, ValidationError]:
    self.log(f"creating {user.email.as_str()}")
    return self.repo.save(user)
```

### 1.8 Newtypes

- **Zero-cost wrappers**: Enforce invariants at construction.
- **Constructors return `Result`**: Validation failures are explicit. Newtypes must be constructed via their validated constructors; direct field construction is disallowed. Codegen enforces the constructor path.
- **Type-safe**: Newtypes are distinct from their underlying type.

```incan
type Email = newtype str:
  @validate new
  def from_str(v: str) -> Result[Email, ValidationError]:
    if "@" not in v:
      return Err(ValidationError.EmailMissingAt)
    return Ok(Email(v.lower()))

type UserId = newtype uuid
```

### 1.9 Enums

- **Algebraic data types**: Variants can carry data.
- **Exhaustive matching**: `match` must cover all variants.

```incan
enum ValidationError:
  EmailMissingAt
  InactiveExternal
  RepoError(str)

enum Option[T]:
  Some(T)
  None

enum Result[T, E]:
  Ok(T)
  Err(E)
```

### 1.10 Functions

- **`def` keyword**: `def name(params) -> ReturnType:`.
- **`async def`**: For async functions.
- **Explicit return types**: Required for all functions.
- **No implicit returns**: Functions must use `return`. (Future: implicit last-expression returns could be considered, but are not allowed now.)

```incan
def load_config(path: Path) -> Result[Config, IoError]:
  raw = read_file(path)?
  return Config.parse(raw)?

async def fetch_all(urls: List[str]) -> List[Bytes]:
  tasks = [fetch(u) for u in urls]
  return await gather(tasks)
```

---

## 2. Builtin Types and Traits

### 2.1 Core Types

| Type            | Description                                               |
| --------------- | --------------------------------------------------------- |
| `int`           | Signed integer (platform-sized or explicit: `i32`, `i64`) |
| `float`         | Floating point (`f32`, `f64`)                             |
| `bool`          | Boolean (`true`, `false`)                                 |
| `str`           | UTF-8 string                                              |
| `bytes`         | Byte sequence                                             |
| `List[T]`       | Growable list                                             |
| `Dict[K, V]`    | Hash map                                                  |
| `Set[T]`        | Hash set                                                  |
| `Tuple[T, ...]` | Fixed-size heterogeneous tuple                            |
| `Option[T]`     | `Some(T)` or `None`                                       |
| `Result[T, E]`  | `Ok(T)` or `Err(E)`                                       |
| `Unit`          | Void/unit type (empty tuple)                              |

### 2.2 Core Traits

| Trait          | Description                  | Python Analog              |
| -------------- | ---------------------------- | -------------------------- |
| `Debug`        | Debug formatting             | `__repr__`                 |
| `Display`      | User-facing formatting       | `__str__`                  |
| `Eq`           | Equality comparison          | `__eq__`                   |
| `PartialEq`    | Partial equality             | `__eq__`                   |
| `Ord`          | Total ordering               | `__lt__`, `__le__`, etc.   |
| `PartialOrd`   | Partial ordering             | `__lt__`, `__le__`, etc.   |
| `Hash`         | Hashable                     | `__hash__`                 |
| `Clone`        | Explicit duplication         | `copy.copy`                |
| `Default`      | Default construction         | `__init__` with defaults   |
| `From[T]`      | Conversion from `T`          | `@classmethod` constructor |
| `Into[T]`      | Conversion into `T`          | —                          |
| `TryFrom[T]`   | Fallible conversion from `T` | —                          |
| `TryInto[T]`   | Fallible conversion into `T` | —                          |
| `Iterator`     | Iteration protocol           | `__iter__`, `__next__`     |
| `IntoIterator` | Convert to iterator          | `__iter__`                 |
| `Error`        | Error behavior               | `Exception`                |

### 2.3 Derives

Derives auto-generate trait implementations for models and newtypes:

| Derive              | Generated             |
| ------------------- | --------------------- |
| `@derive(Debug)`    | Debug formatting      |
| `@derive(Display)`  | Display formatting    |
| `@derive(Eq)`       | Equality + PartialEq  |
| `@derive(Ord)`      | Ordering + PartialOrd |
| `@derive(Hash)`     | Hash implementation   |
| `@derive(Clone)`    | Clone implementation  |
| `@derive(Default)`  | Default construction  |
| `@derive(Validate)` | Validation hooks      |

Multiple derives: `@derive(Debug, Eq, Hash, Clone)`.

---

## 3. Grammar (EBNF-like)

```ebnf
(* Top-level declarations *)
program        = { declaration } ;
declaration    = model_decl | class_decl | trait_decl | newtype_decl
               | enum_decl | function_decl | import_decl ;

(* Imports *)
import_decl    = import_stmt | from_import_stmt ;

import_stmt    = "import" ( python_import | rust_import | module_path ) [ "as" IDENT ] ;
from_import_stmt
              = "from" rust_import "import" import_item { "," import_item }
              | "from" module_path "import" import_item { "," import_item } ;

import_item    = IDENT [ "as" IDENT ] ;

python_import  = "python" STRING ;
rust_import    = "rust" "::" IDENT { "::" IDENT } ;

(* Module paths *)
module_path    = parent_prefix IDENT { ( "::" | "." ) IDENT } ;
parent_prefix  = { ".." }
              | { "super" ( "::" | "." ) } ;
(*
  Note: Absolute (project-root) module paths via `crate::...` are specified by RFC 005.
*)

(* Model *)
model_decl     = { decorator } "model" IDENT [ type_params ] ":"
                 INDENT { field_decl | method_decl } DEDENT ;
field_decl     = IDENT ":" type [ "=" expr ] ;

(* Class *)
class_decl     = { decorator } "class" IDENT [ type_params ]
                 [ "extends" IDENT ] [ "with" trait_list ] ":"
                 INDENT { field_decl | method_decl } DEDENT ;
trait_list     = IDENT { "," IDENT } ;

(* Trait *)
trait_decl     = { decorator } "trait" IDENT [ type_params ] ":"
                 INDENT { method_decl } DEDENT ;

(* Newtype *)
newtype_decl   = "type" IDENT "=" "newtype" type [ ":" INDENT { method_decl } DEDENT ] ;

(* Enum *)
enum_decl      = "enum" IDENT [ type_params ] ":"
                 INDENT { variant_decl } DEDENT ;
variant_decl   = IDENT [ "(" type_list ")" ] ;

(* Function / Method *)
function_decl  = { decorator } [ "async" ] "def" IDENT "(" params ")" "->" type ":"
                 INDENT { statement } DEDENT ;
method_decl    = { decorator } [ "async" ] "def" IDENT "(" receiver [ "," params ] ")" "->" type ":"
                 INDENT { statement } DEDENT
               | { decorator } [ "async" ] "def" IDENT "(" params ")" "->" type ":" "..." ;

receiver       = "self" | "mut" "self" ;
params         = [ param { "," param } ] ;
param          = IDENT ":" type [ "=" expr ] ;

(* Decorators *)
decorator      = "@" IDENT [ "(" decorator_args ")" ] ;
decorator_args = expr { "," expr } | IDENT ":" type { "," IDENT ":" type } ;

(Note: Core supports both `import foo.bar` and `from foo import bar`. Decorator syntax is reserved; semantics are defined per decorator in later phases/stdlib.)

(* Types *)
type           = simple_type | generic_type | function_type ;
simple_type    = IDENT | "Unit" ;
generic_type   = IDENT "[" type_list "]" ;
type_list      = type { "," type } ;
type_params    = "[" IDENT { "," IDENT } "]" ;
function_type  = "(" type_list ")" "->" type ;

(* Expressions *)
expr           = primary | binary_expr | unary_expr | call_expr | index_expr
               | field_expr | method_expr | await_expr | try_expr | match_expr
               | if_expr | list_comp | dict_comp | closure_expr ;
               (* Note: Python-style lambda intentionally omitted in favor of arrow closures *)

closure_expr   = "(" [ IDENT { "," IDENT } ] ")" "=>" expr ;

primary        = IDENT | literal | "(" expr ")" | "self" ;
literal        = INT | FLOAT | STRING | "true" | "false" | "None"
               | list_literal | dict_literal | tuple_literal ;
list_literal   = "[" [ expr { "," expr } ] "]" ;
dict_literal   = "{" [ dict_entry { "," dict_entry } ] "}" ;
dict_entry     = expr ":" expr ;
tuple_literal  = "(" expr "," [ expr { "," expr } ] ")" ;

binary_expr    = expr binary_op expr ;
binary_op      = "+" | "-" | "*" | "/" | "%" | "==" | "!=" | "<" | ">" | "<=" | ">="
               | "and" | "or" | "in" | "not in" ;
unary_expr     = unary_op expr ;
unary_op       = "-" | "not" ;

call_expr      = expr "(" [ arg_list ] ")" ;
arg_list       = arg { "," arg } ;
arg            = expr | IDENT "=" expr ;

index_expr     = expr "[" expr "]" ;
field_expr     = expr "." IDENT ;
method_expr    = expr "." IDENT "(" [ arg_list ] ")" ;
await_expr     = "await" expr ;
try_expr       = expr "?" ;

match_expr     = "match" expr ":" INDENT { match_arm } DEDENT ;
match_arm      = "case" pattern ":" INDENT { statement } DEDENT
               | pattern "=>" expr
               | pattern "=>" INDENT { statement } DEDENT ;
pattern        = IDENT | literal | IDENT "(" pattern_list ")" | "_" ;
pattern_list   = pattern { "," pattern } ;

if_expr        = "if" expr ":" INDENT { statement } DEDENT
                 { "elif" expr ":" INDENT { statement } DEDENT }
                 [ "else" ":" INDENT { statement } DEDENT ] ;

list_comp      = "[" expr "for" IDENT "in" expr [ "if" expr ] "]" ;
dict_comp      = "{" expr ":" expr "for" IDENT "in" expr [ "if" expr ] "}" ;

(* Statements *)
statement      = assignment | return_stmt | if_stmt | while_stmt | for_stmt
               | expr_stmt | pass_stmt ;
assignment     = [ "let" | "mut" ] IDENT [ ":" type ] "=" expr ;
return_stmt    = "return" [ expr ] ;
if_stmt        = if_expr ;
while_stmt     = "while" expr ":" INDENT { statement } DEDENT ;
for_stmt       = "for" IDENT "in" expr ":" INDENT { statement } DEDENT ;
expr_stmt      = expr ;
pass_stmt      = "pass" | "..." ;
```

---

## 4. Type Rules

### 4.1 Assignment

- LHS type must match RHS type exactly (or be inferred).
- `mut` bindings can be reassigned; non-`mut` bindings cannot.

### 4.2 Function Signatures

- All parameters must have explicit types.
- Return type must be explicit.
- Functions using `?` must return `Result[T, E]` where `E` is compatible with the propagated error.

### 4.3 Method Resolution

1. Check the type's own methods.
2. Check composed traits in left-to-right order.
3. If conflict, require explicit resolution or compile error.

### 4.4 Trait Adoption

- `class X with TraitA, TraitB`: `X` must satisfy all trait requirements.
- Missing required methods → compile error.
- Conflicting methods → must resolve explicitly.

### 4.5 Error Propagation (`?`)

- `expr?` on `Result[T, E]`:
    - If `Err(e)`: return `Err(e)` from enclosing function.
    - If `Ok(v)`: evaluate to `v`.
- Enclosing function must return `Result[_, E]` (or compatible).

### 4.6 Newtype Invariants

- Newtype constructors returning `Result` enforce invariants.
- Cannot construct newtype directly; must use constructor.

---

## 5. Invariants

1. **No implicit mutation**: All mutations require `mut`.
2. **No silent errors**: Errors surface via `Result`; `?` propagates explicitly.
3. **No runtime trait patching**: Trait composition is static.
4. **Deterministic dispatch**: Method resolution is predictable (no MRO).
5. **Type safety**: All types known at compile time; no dynamic typing.
6. **Explicit receiver**: `self` is required in method signatures.

---

## 6. Examples

### Complete Example

```incan
import polars::prelude as pl

# Structured error type
enum AppError:
  ValidationError(str)
  IoError(str)
  NotFound

# Newtype with invariant
type Email = newtype str:
  @validate new
  def from_str(v: str) -> Result[Email, AppError]:
    if "@" not in v:
      return Err(AppError.ValidationError("missing @"))
    return Ok(Email(v.lower()))

  def as_str(self) -> str:
    return self.0   # access underlying value

# Model with validation
@derive(Debug, Eq, Clone)
model User:
  id: int
  email: Email
  is_active: bool = true

  def validate(self) -> Result[User, AppError]:
    if not self.is_active:
      return Err(AppError.ValidationError("inactive"))
    return Ok(self)

# Trait
trait Loggable:
  def log(self, msg: str):
    print(f"[{self.name}] {msg}")

# Class with trait
class UserService with Loggable:
  name: str
  users: Dict[int, User]

  def create(mut self, email_str: str) -> Result[User, AppError]:
    email = Email.from_str(email_str)?
    user = User(id=len(self.users) + 1, email=email).validate()?
    self.users[user.id] = user
    self.log(f"created user {user.id}")
    return Ok(user)

  def get(self, id: int) -> Result[User, AppError]:
    match self.users.get(id):
      case Some(user): Ok(user)
      case None: Err(AppError.NotFound)

# Usage
def main() -> Result[Unit, AppError]:
  mut svc = UserService(name="user-svc", users={})
  user = svc.create("ada@example.com")?
  print(f"Created: {user}")
  return Ok(())
```

---

## 7. Checklist

### Core Semantics

- [x] Required typing
- [x] Immutable-by-default bindings
- [x] `self` / `mut self` receivers
- [x] `Result` / `Option` / `?` error handling
- [x] Behavior-only traits with deterministic composition
- [x] Models vs Classes distinction
- [x] Newtypes with validation
- [x] Enums (algebraic data types)

### Builtin Types

- [x] Primitives: `int`, `float`, `bool`, `str`, `bytes`
- [x] Collections: `List`, `Dict`, `Set`, `Tuple`
- [x] Error types: `Result`, `Option`
- [x] `Unit` type

### Builtin Traits

- [x] `Debug`, `Display`
- [x] `Eq`, `PartialEq`, `Ord`, `PartialOrd`
- [x] `Hash`
- [x] `Clone`
- [x] `Default`
- [x] `From`, `Into`, `TryFrom`, `TryInto`
- [x] `Iterator`, `IntoIterator`
- [x] `Error`

### Derives

- [x] `@derive(Debug, Display, Eq, Ord, Hash, Clone, Default, Validate)`

### Grammar

- [x] Declarations: model, class, trait, newtype, enum, function
- [x] Expressions: calls, `?`, match, if, comprehensions
- [x] Statements: assignment, return, control flow
- [x] Receivers: `self`, `mut self`

### Type Rules

- [x] Assignment typing
- [x] Function signature requirements
- [x] Method resolution order
- [x] Trait adoption rules
- [x] Error propagation (`?`)
- [x] Newtype invariants
