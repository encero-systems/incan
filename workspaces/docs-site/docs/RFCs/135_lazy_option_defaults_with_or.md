# RFC 135: Lazy Option defaults with `or`

- **Status:** Planned
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (Boolean operators and Option)
    - RFC 029 (union types and narrowing)
    - RFC 070 (explicit Result recovery)
    - RFC 129 (binding and borrowing semantics)
    - RFC 131 (typed error declarations)
    - RFC 134 (pattern bindings with exiting fallbacks)
- **Issue:** [#2464](https://github.com/encero-systems/incan/issues/2464)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Extend `or` to select a present Option value or lazily evaluate a fallback. An `Option[T]` followed by a plain `T` fallback produces `T`; an `Option[T]` followed by another `Option[T]` preserves optionality and selects the first present value. This supports readable defaults and ordered optional sources without callback wrappers. Presence, rather than payload truth, controls selection: `Some(0)`, `Some(false)`, and `Some("")` all win. Explicit one-level `Option.flatten()` lets authors merge nested absence before choosing a default. Boolean `or` retains its existing behavior, and Result failures remain explicit.

## Motivation

Choosing a default is a common operation in configuration, cached lookups, and optional presentation data. An exiting pattern binding can express it, but adds an extraction and an early return when the operation is simply value selection. RFC 134 is most useful when a successful extraction establishes a prerequisite for subsequent work.

`unwrap_or` already supplies a default, but normal argument evaluation computes a fallback call before the method executes. A lazy callback method could avoid that evaluation, at the cost of a more elaborate spelling for a simple relationship.

```incan
def resolve_port(cached: Option[int]) -> int:
    return cached or discover_port()
```

Here `discover_port` is an application helper returning an integer. The expression states both the choice and the intended evaluation order: discovery runs only when `cached` is absent. Several optional sources can express priority just as directly, ending with a plain fallback value.

## Goals

- Express defaults for optional values through the existing `or` spelling.
- Evaluate fallback expressions only when the left operand is absent.
- Support ordered selection among compatible optional sources.
- Preserve present falsy values and distinguish absence from failure.
- Define result types, precedence, nested Options, and ownership obligations explicitly.
- Provide explicit one-level flattening when nested absence should have the same fallback behavior.
- Complement RFC 134 and existing Result combinators.

## Non-Goals

- Introducing general Python-style truthiness for strings, numbers, collections, or custom objects.
- Treating an `Err` as absence or automatically discarding its error.
- Implicitly unwrapping a Result on either side of the operator.
- Adding a general suite of Option combinators such as `map`, `and_then`, or `transpose`.
- Changing `and`, adding optional attribute access, or defining optional assignment operators.
- Introducing a user-overridable absence or truth-testing protocol.

## Guide-level explanation

The Option forms below are proposed behavior. Application helpers retain the signatures described beside each example.

### Supply a default

```incan
def display_name(name: Option[str]) -> str:
    return name or "Anonymous"
```

`Some("Danny")` produces `"Danny"`. `None` produces `"Anonymous"`. `Some("")` produces the empty string because it is present.

The same rule applies to other payloads:

```incan
def choose_port(candidate: Option[int]) -> int:
    return candidate or 8080

def choose_debug(candidate: Option[bool]) -> bool:
    return candidate or true
```

A present port of zero remains zero. A present debug value of `false` remains false.

### Evaluate a fallback only when needed

Suppose `discover_port()` returns an integer and may perform work:

```incan
def resolve_port(cached: Option[int]) -> int:
    return cached or discover_port()
```

Discovery runs once for `None` and never runs for `Some(port)`. This differs from an ordinary eager argument such as `cached.unwrap_or(discover_port())`.

### Select among optional sources

```incan
def resolve_port(
    command_line: Option[int],
    environment: Option[int],
    configuration: Option[int],
) -> int:
    return command_line or environment or configuration or 8080
```

The first present source wins. If every source is absent, the expression produces `8080`. A present zero also wins.

An optional fallback preserves optionality:

```incan
def preferred_port(primary: Option[int], secondary: Option[int]) -> Option[int]:
    return primary or secondary
```

If both arguments are absent, the result is `None`. Otherwise the result is the first `Some(port)`. Choosing a plain default is a separate step:

```incan
selected = preferred_port(command_line, environment)
port = selected or 8080
```

Calls returning compatible Options work the same way. In a chain such as `command_line_port() or environment_port() or configuration_port() or 8080`, each later lookup runs only while all earlier sources are absent.

### Annotate when inference cannot determine the type

The compiler uses available operand types and surrounding context on a best-effort basis:

```incan
port = None or 8080  # inferred as int from the plain fallback

empty: Option[int] = None
selected = empty or None  # Option[int]

unknown = None or None  # rejected: no payload type is established
```

If the payload type or operator form remains ambiguous, the diagnostic must ask for an explicit annotation and identify the operand or binding that needs it. The compiler must not guess a type or add Option layers merely to make the expression fit. An annotation resolves the ambiguity only when it establishes one valid interpretation under the ordinary selection rules.

### Group a deliberate switch to Boolean operations

An Option-selection chain that continues with Boolean or after producing a plain bool is a code smell and requires explicit grouping:

```incan
Some(false) or false or true    # rejected: ungrouped semantic switch
(Some(false) or false) or true # true: Boolean operation after selection
Some(false) or (false or true) # false: Boolean expression is the fallback
```

The diagnostic explains that a present false can be overridden by a later Boolean operation and asks the author to group the intended expression. It must not silently choose or rewrite one interpretation. A chain of optional Boolean sources followed by one plain default remains valid, as does an ordinary Boolean-only chain.

### Merge nested absence explicitly

A dictionary lookup returns `Option[V]`. If the stored value type is already `Option[int]`, looking it up produces `Option[Option[int]]`:

```incan
timeouts: dict[str, Option[int]] = {
    "disabled": None,
    "short": Some(30),
}

timeouts.get("missing")   # None: no key
timeouts.get("disabled")  # Some(None): key with an absent value
timeouts.get("short")     # Some(Some(30)): key with a value
```

When missing keys and stored `None` should use the same default, explicitly merge those cases with the proposed Option method `.flatten()`:

```incan
def timeout_or_default(
    settings: dict[str, Option[int]],
    key: str,
    fallback: int,
) -> int:
    return settings.get(key).flatten() or fallback
```

Here dictionary `.get()` is existing behavior; Option `.flatten()` and Option-aware `or` are proposed by this RFC. Flattening turns both `None` and `Some(None)` into `None`, and `Some(Some(30))` into `Some(30)`. The example therefore uses `fallback` for both missing and disabled entries. That deliberately discards the distinction between them; omit flattening when the distinction matters. Each call removes exactly one Option layer, and `or` never flattens implicitly.

### Keep malformed configuration separate from absence

The existing TOML optional lookup API returns `Result[Option[T], TomlError]`. A missing key is a successful absence, while a malformed present value is an error:

```incan
port = document.get_optional[int]("server.port")? or 8080
debug = document.get_optional[bool]("server.debug")? or false
```

The `?` propagates lookup or decoding failures before `or` selects a default. An absent port produces `8080`; a valid port produces its integer; an invalid string where an integer is required remains an error. A present `false` debug setting remains false.

### Permit an explicitly fallible fallback

Suppose `read_configured_port()` returns `Result[int, ConfigError]`:

```incan
def resolve_port(cached: Option[int]) -> Result[int, ConfigError]:
    port = cached or read_configured_port()?
    return Ok(port)
```

The configuration is read only when `cached` is absent. If that read fails, the explicit `?` propagates its error. This does not make a Result itself an optional source:

```incan
port = cached or read_configured_port()
# rejected for cached: Option[int]; the fallback is Result[int, ConfigError]
```

Recovering from a Result failure remains an explicit operation such as RFC 070's `or_else`.

### Use an exiting binding for a prerequisite

With RFC 134, authentication can remain a prerequisite while optional presentation data receives a default:

```incan
def submit(request: Request) -> Result[Receipt, SubmitError]:
    let Some(user) = request.user()
    else:
        return Err(SubmitError.Unauthenticated)

    label = user.display_name() or "Anonymous"
    permissions = load_permissions(user)?
    return store_submission(request, user, label, permissions)
```

Here the application's `display_name` method returns `Option[str]`, and `SubmitError.Unauthenticated` is a unit case under RFC 131. Missing authentication exits; a missing display name selects a value.

## Reference-level explanation

### Operator forms and result types

The operator must use the existing infix `or` token. Boolean operands must retain their existing Boolean behavior. For an Option left operand, the following forms must be supported:

| Left type   | Right type  | Result type | Selection                                             |
| ----------- | ----------- | ----------- | ----------------------------------------------------- |
| `Option[T]` | `Option[T]` | `Option[T]` | First present Option, otherwise the right Option.     |
| `Option[T]` | `T`         | `T`         | Left payload when present, otherwise the right value. |

Aliases must resolve before selecting an operator form. An already typed `Option[T]` right operand must select the optional-source form before any ordinary implicit Option wrapping is considered. Otherwise the right expression must be checked against the left payload type `T`, using existing assignment conversions. The operator must not infer a new union type merely to accommodate an incompatible fallback or widen two independently typed Options with different payload types.

When an Option operand's type is not established, inference must make a best effort using checked operand types and the surrounding binding, parameter, or return context. A uniquely determined interpretation under the declared operator-selection and ordinary inference rules must be accepted. If the payload type or selected form remains ambiguous or insufficiently constrained, the compiler must issue a source diagnostic asking for an explicit type annotation and identify where it is needed. It must not choose an arbitrary type, manufacture an additional Option layer, or use implicit wrapping to conceal ambiguity.

Operator selection must not depend on which branch executes at runtime. The unselected operand must still be typechecked, even though it is not evaluated. An expected result type must not silently change the selected form by adding or removing an Option layer.

A Result must not act as an absence-testing left operand. `Option[int] or Result[int, E]` must be rejected unless the author explicitly obtains an integer from the Result, for example through `?`. An Option whose payload is itself a Result may select between Result values under the ordinary payload rule; selection must preserve the Result rather than recover from or unwrap it.

### Evaluation

- The left expression must be evaluated exactly once.
- When it produces `Some(value)`, the right expression must not be evaluated. The payload form must produce `value`; the optional-source form must preserve a present Option containing that payload.
- When it produces `None`, the right expression must be evaluated exactly once and supply the result under the selected form.
- Presence must be determined solely by the Option variant. The operator must not test the payload's Boolean value, length, equality, or custom methods.
- Side effects, failure propagation, and temporary construction on the right must occur only when that expression is selected.
- The operator must not introduce an implicit panic or error conversion.

### Precedence and chains

`or` must retain its existing precedence and left associativity. Postfix `?` must bind more tightly than `or`, so `candidate or load()?` must mean `candidate or (load()?)`; propagation on the right must remain lazy.

An ordered-source chain must be typed as the sequence of its binary operations. With `a`, `b`, and `c` all `Option[T]`, `a or b or c or fallback` must retain `Option[T]` through the optional-source operations and produce `T` at the final plain fallback. Every later source must be skipped once an earlier present source has been selected.

Once an operation produces a plain Boolean, a subsequent Boolean `or` uses Boolean semantics, but an unparenthesized chain crossing that boundary must be rejected with a source diagnostic requesting explicit grouping. For example, `(Some(false) or false) or true` is explicitly grouped and produces true: the inner expression selects a present false, then the outer Boolean operation produces true. The ungrouped `Some(false) or false or true` must receive the code-smell diagnostic rather than silently accept that semantic switch. The grouping must identify the intended Option-selection boundary or the intended Boolean fallback expression; parentheses around an unrelated operand do not suffice. Authors selecting among optional Boolean settings can keep intermediate sources optional and put a plain default last. Such ordinary source chains remain valid. Pure Boolean chains retain their existing behavior, and the compiler must not reinterpret an ordinary chain as one special multi-operand defaulting operation. The formatter must preserve grouping required to make the semantic boundary explicit, even when removing those parentheses would preserve ordinary left-associated evaluation.

### Nested Options

The operator must test only its left operand's outer Option layer. It must not recursively search for a non-None payload or flatten nested Options.

```incan
def preserve_outer(
    primary: Option[Option[int]],
    secondary: Option[Option[int]],
) -> Option[Option[int]]:
    return primary or secondary

def choose_inner(primary: Option[Option[int]], fallback: Option[int]) -> Option[int]:
    return primary or fallback
```

In `preserve_outer`, a primary value of `Some(None)` wins and remains `Some(None)`. In `choose_inner`, that same primary value wins and produces its payload, `None`, without evaluating the fallback. The right operand's static shape determines whether selection preserves or removes one outer Option layer.

A contextual `None` right operand should take the optional-source form when the left operand already determines `T`; thus `primary or None` preserves the left operand's Option type. When the left operand's Option type is not yet known, best-effort contextual inference applies; any remaining ambiguity must produce a diagnostic requesting an explicit type annotation.

### Explicit one-level flattening

An `Option[Option[T]]` must provide `.flatten() -> Option[T]` with the following behavior:

| Receiver          | Result    |
| ----------------- | --------- |
| `None`            | `None`    |
| `Some(None)`      | `None`    |
| `Some(Some(v))`   | `Some(v)` |

Flattening must remove exactly one Option layer. For example, flattening `Option[Option[Option[int]]]` once must produce `Option[Option[int]]`. It must not recursively unwrap values, test payload truth, or unwrap a Result. A receiver whose payload is not an Option must receive a type diagnostic explaining the required nested shape; insufficiently constrained receiver types must request an annotation under the ordinary inference rules.

The receiver must be evaluated exactly once. Ownership, borrowing, and payload access must follow equivalent explicit matching without an implicit clone, panic, or additional mutation permission. Flattening must be an explicit method call; neither `or` nor contextual result typing may insert it automatically.

### Ownership and mutation

The operator must preserve the ownership and borrowing contract of equivalent explicit matching and selection. It must not clone a payload or fallback implicitly to reconcile incompatible lifetimes or permit conflicting access. Only the selected branch supplies the result; the unselected fallback must not acquire a runtime borrow or consume a value merely because it was typechecked.

Borrowed results must remain valid for every possible selected branch. Temporary lifetimes, branch joins, and mutation permissions must follow the ordinary language rules, including RFC 129 where applicable. An expression returning a read-only borrow must not gain write permission through `or`.

### Diagnostics

An ungrouped Option-selection-to-Boolean chain must receive a source diagnostic identifying the operator where selection becomes Boolean logic. It must explain the code smell, including that a present false may be overridden, and request grouping of the intended selection boundary or Boolean fallback expression. The diagnostic should show the relevant parenthesized alternatives without treating them as equivalent or automatically rewriting the source. Explicitly grouped expressions retain ordinary Boolean semantics and normal Boolean diagnostics.

Diagnostics must identify incompatible payload or fallback types and explain whether the expression requires a plain payload or a compatible optional source. Ambiguous or underconstrained inference must ask for an explicit type annotation rather than silently choose a payload type or add Option layers. A Result used as an absence-testing operand must receive a diagnostic that distinguishes failure from absence. The compiler should suggest explicit propagation or recovery when that is applicable, without automatically inserting either operation.

## Design details

### One spelling, two explicit selection criteria

Boolean `or` tests truth; Option `or` tests presence. The distinction is determined by the left operand's static type. Numbers, strings, and containers do not acquire general truthiness. In particular, `Some(false) or true` produces false while ordinary `false or true` produces true.

### Fallbacks are expressions rather than callbacks

Short-circuit evaluation provides laziness directly. A fallback can be a literal, an ordinary call, an explicitly propagated fallible call, or another compatible optional source. It does not require a zero-argument closure solely to defer evaluation.

### Relationship to other proposals

RFC 134 handles a required shape whose unmatched path exits. This RFC handles value selection that continues with a result. Neither replaces the other. RFC 131 remains responsible for typed errors and explicit translation, while RFC 070 remains responsible for Result recovery. One-level Option flattening belongs here because it makes nested absence explicit before default selection; a general suite of Option combinators remains separate library design work.

## Alternatives considered

- **Continue using `unwrap_or`.** Concise for an existing fallback value, but eager argument evaluation does not defer a fallback call.
- **Add `unwrap_or_else`.** Expresses lazy defaults without extending an operator, but requires a callback where an ordinary short-circuit expression can state the same relationship. It also does not by itself provide an ordered chain of optional sources.
- **Use `let … else` everywhere.** Appropriate for prerequisites, but adds statement-level exits to simple value selection.
- **Use a dedicated `??` operator.** Separates Boolean and absence selection visually, but introduces another token and spelling. `or` reads naturally for the proposed selection and can use static typing to distinguish its forms.
- **Use Python-style truthiness.** Would replace valid zero, false, empty text, and empty collections with defaults. This RFC tests Option presence only.
- **Treat Result failure as absence.** Would make an invalid configuration indistinguishable from missing configuration and discard typed errors without explicit recovery.
- **Silently accept an ungrouped switch from Option selection to Boolean logic.** Rejected as a code smell: a later Boolean operation can override a present false. Explicit grouping and a diagnostic make the intended boundary visible.
- **Make the whole chain select the first present value regardless of intermediate types.** Would depart from ordinary binary operator composition and existing left associativity, especially when a plain Boolean is produced. Intermediate optional sources instead remain Options until the final plain fallback.
- **Flatten nested Options implicitly during selection.** Would silently discard the distinction between a missing entry and a present entry containing `None`. An explicit `.flatten()` makes that choice visible and removes exactly one layer.

## Drawbacks

- Readers must distinguish Boolean truth selection from Option presence selection.
- The right operand's static shape determines whether the result remains optional or exposes a payload.
- Nested Options and chains mixing optional and plain Booleans need careful explanation.
- Short-circuit branch joins add ownership and lifetime obligations that a spelling change alone cannot solve.
- Code using a default can still conceal missing data when the application should reject it; authors must choose the intended policy explicitly.

## Layers affected

- **Language surface registry / documentation:** Option-aware `or` forms, result types, and examples alongside Boolean `or`.
- **Typechecker:** operand classification, contextual inference, fallback compatibility, and diagnostics.
- **Ownership analysis / IR lowering:** lazy selection, selected payload access, branch joins, and checked result lifetimes.
- **Emission:** short-circuit execution without eager fallback calls or implicit clones.
- **Formatter / LSP:** formatting that preserves grouping, inferred result types, and operand diagnostics.
- **Option API / stdlib guidance:** explicit one-level `.flatten()`, configuration examples, and optional lookup examples that preserve the distinction between missing and malformed data.

## Design Decisions

- Contextual inference is best effort using available operand types and surrounding context. Unique interpretations follow the declared operator-selection and ordinary inference rules. Ambiguous or underconstrained payload types or operator forms produce a source diagnostic requesting an explicit annotation; the compiler does not guess or insert Option layers to conceal uncertainty.

- An unparenthesized or chain that switches from Option selection to subsequent Boolean or is rejected with a code-smell diagnostic. The diagnostic explains the semantic switch and asks for grouping, showing distinct parenthesized alternatives. Ordinary optional-source chains ending in a single plain default and pure Boolean chains remain valid. Explicit grouping retains normal binary semantics.

- Nested Options retain outer-layer selection semantics. Explicit `.flatten()` removes one layer when outer absence and an absent payload should share a fallback. It deliberately discards that distinction and is never inserted implicitly.
