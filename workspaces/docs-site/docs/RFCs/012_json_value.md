# RFC 012: JsonValue Type for Dynamic JSON

**Status:** Draft

## Summary

Add a `JsonValue` type for handling JSON with unknown or varying structure at runtime.

## Module placement and names

This RFC proposes that dynamic JSON support lives in a dedicated stdlib module:

- **Canonical module**: `std.json`
- **Python analogy**: `import json`

Rationale:

- JSON is cross-cutting (web responses, HTTP clients, config, tooling).
- A dedicated module avoids duplicating “JSON-ish” vocabulary across `std.web`, `std.http`, etc.
- It cleanly separates “dynamic JSON data” (`std.json.JsonValue`) from “web transport wrappers” like `std.web.Json[T]`.

Clarifications:

- `std.json.JsonValue` is the **dynamic JSON data** type (parseable, indexable, inspectable).
- `std.web.Json[T]` is a **web transport wrapper** for request/response bodies; it is not “the JSON module”.
  It may internally use `std.json` but has distinct semantics and typing.
- Rust backing type is expected to be `serde_json::Value` under the hood, but that Rust type is an implementation detail
  (accessible explicitly via `rust::…` interop when needed).

Recommended import style:

```incan
import std.json as json
from std.json import JsonValue
```

## Motivation

Currently, Incan requires defining models with `@derive(Serialize, Deserialize)` for JSON handling.
This works well for known, fixed schemas but falls short for:

1. **Dynamic APIs** — APIs that return varying structures depending on context
2. **Exploration** — Prototyping without defining full models
3. **Partial parsing** — Extracting specific fields from large JSON without modeling everything
4. **Mixed schemas** — JSON where some parts are typed and others are dynamic

### Current Approach (Works)

```incan
@derive(Serialize, Deserialize)
model User:
    name: str
    age: int

user = User.from_json(json_str)?
println(user.name)
```

### Proposed Addition

```incan
# Recommended: import the stdlib json module
import std.json as json

# Parse unknown JSON
data = JsonValue.parse(json_str)?

# Access dynamically
name = data["user"]["name"].as_str()
count = data["count"].as_int()
items = data["items"].as_list()

# Check types at runtime
if data["field"].is_string():
    println(data["field"].as_str())
```

## Detailed Design

### JsonValue Type

`JsonValue` is an enum representing any JSON value:

```incan
enum JsonValue:
    Null
    Bool(bool)
    Int(int)
    Float(float)
    String(str)
    Array(List[JsonValue])
    Object(Dict[str, JsonValue])
```

### Constructors

```incan
# Parse from string
value = JsonValue.parse(json_str) -> Result[JsonValue, str]

# Create values directly
null_val = JsonValue.null()
bool_val = JsonValue.bool(true)
int_val = JsonValue.int(42)
str_val = JsonValue.string("hello")
arr_val = JsonValue.array([val1, val2])
obj_val = JsonValue.object({"key": value})
```

### Access Methods

```incan
# Type checking
value.is_null() -> bool
value.is_bool() -> bool
value.is_int() -> bool
value.is_float() -> bool
value.is_string() -> bool
value.is_array() -> bool
value.is_object() -> bool

# Value extraction (returns Option)
value.as_bool() -> Option[bool]
value.as_int() -> Option[int]
value.as_float() -> Option[float]
value.as_str() -> Option[str]
value.as_array() -> Option[List[JsonValue]]
value.as_object() -> Option[Dict[str, JsonValue]]
```

### Indexing

```incan
# Object field access
value["key"] -> JsonValue  # Returns JsonValue.Null if missing

# Array index access
value[0] -> JsonValue  # Returns JsonValue.Null if out of bounds

# Chained access
value["user"]["address"]["city"].as_str()
```

### Serialization

```incan
value.to_json() -> str  # Serialize back to JSON string
```

## Rust Implementation

Maps to `serde_json::Value`:

```rust
pub type JsonValue = serde_json::Value;

impl JsonValue {
    pub fn parse(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
    
    pub fn is_null(&self) -> bool { self.is_null() }
    pub fn as_str(&self) -> Option<&str> { self.as_str() }
    // ... etc
}
```

## Hybrid Models

Mix typed and dynamic:

```incan
@derive(Serialize, Deserialize)
model ApiResponse:
    status: int
    message: str
    data: JsonValue  # Dynamic payload
```

## Alternatives Considered

### 1. Dict[str, Any]

Rust doesn't have `Any` like Python. Would require boxing and type erasure.

### 2. Generic parsing only

Keep `json_parse[T]()` and require models for everything. Rejected because:

- Too restrictive for dynamic use cases
- Poor developer experience for exploration

### 3. Automatic Dict conversion

Auto-convert JSON objects to `Dict[str, ???]`. Rejected because:

- Loses type information
- Can't handle nested structures uniformly

## Implementation Plan

1. Add `JsonValue` as a language surface type in `incan_core::lang::surface` (maps to `serde_json::Value`)
2. Implement constructors and methods in codegen
3. Add typechecker support for indexing operations
4. Add `JsonValue` field support in models
5. Document with examples

## Open Questions

1. Should `value["key"]` return `JsonValue` or `Option[JsonValue]`?
   - Returning `JsonValue` (with Null for missing) is more ergonomic for chaining
   - Returning `Option` is more explicit about missing keys

2. Should we support `value.key` syntax as sugar for `value["key"]`?

3. How to handle numeric types? JSON has only one number type, but Incan has `int` and `float`.

## References

- Rust: `serde_json::Value`
- Python: `dict` from `json.loads()`
- TypeScript: `any` or `unknown`
- Go: `interface{}` / `any`
