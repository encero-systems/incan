# Modeling with enums

This guide shows practical ways to use enums to model real program structure: state machines, commands, error types, and
expression trees.

If you haven’t read it yet, start with: [Enums](../explanation/enums.md).

??? info "Coming from TS/JS?"
    If you’re used to discriminated unions (`{ kind: "A", ... } | { kind: "B", ... }`), Incan enums play the same role:
    a closed set of variants with typed payloads, and `match` for case handling.

    **Example**:

    === *Typescript*

        ```typescript
        type Event =
        | { kind: "click"; x: number; y: number }
        | { kind: "key"; key: string };

        function handle(e: Event) {
            switch (e.kind) {
                case "click": return `${e.x},${e.y}`;
                case "key": return e.key;
            }
        }
        ```

    ===*Incan*

        ```incan
        enum Event:
            Click(x: int, y: int)
            Key(key: str)

        def handle(e: Event) -> str:
            match e:
                Click(x, y) => return f"{x},{y}"
                Key(key) => return key
        ```

## Pattern 1: State machines

Use this when a value progresses through a **closed** set of states and you want transitions to be explicit and checked.

```incan
enum ConnectionState:
    Disconnected
    Connecting(str)          # URL being connected to
    Connected(Connection)
    Error(str)

def handle_state(state: ConnectionState) -> ConnectionState:
    match state:
        Disconnected =>
            return ConnectionState.Connecting("https://api.example.com")
        Connecting(url) =>
            match try_connect(url):
                Ok(conn) => return ConnectionState.Connected(conn)
                Err(e) => return ConnectionState.Error(e)
        Connected(_) =>
            # Stay connected
            return state
        Error(msg) =>
            println(f"Error: {msg}")
            return ConnectionState.Disconnected
```

Tips:

- Prefer representing transitions as `state -> state` functions (like above).
- Move small state queries into enum methods when callers ask the enum the same question repeatedly.
- Avoid “boolean soup” (`is_connected`, `is_connecting`, `last_error`, …) when the states are mutually exclusive.

For example, terminal-state checks belong well on the enum itself:

```incan
enum ConnectionState:
    Disconnected
    Connecting(str)
    Connected(Connection)
    Error(str)

    def is_terminal(self) -> bool:
        match self:
            ConnectionState.Disconnected => return true
            ConnectionState.Error(_) => return true
            _ => return false
```

## Pattern 2: Commands / actions

Use this when your program receives a **finite set** of commands and each command has its own payload.

```incan
enum Command:
    Create(str, str)         # (name, content)
    Update(int, str)         # (id, new_content)
    Delete(int)              # id
    List

def execute(cmd: Command) -> Result[str, str]:
    match cmd:
        Create(name, content) =>
            return create_item(name, content)
        Update(id, content) =>
            return update_item(id, content)
        Delete(id) =>
            return delete_item(id)
        List =>
            return Ok(list_items())
```

Tips:

- Keep the payload minimal; prefer IDs over large embedded objects if you can look them up.
- If the set of commands is open-ended (plugins), consider a trait-based approach instead.
  See: [Traits as language hooks](../explanation/traits_as_language_hooks.md) (open-ended “interfaces” via `trait`).

## Pattern 3: Error hierarchies

Use this when you want rich, typed errors but still keep exhaustiveness and structure.

```incan
enum DatabaseError:
    ConnectionFailed(str)
    QueryFailed(str, int)    # (query, error_code)
    NotFound(str)            # table/record name
    PermissionDenied

enum AppError:
    Database(DatabaseError)  # Nested enum
    Validation(str)
    NotAuthenticated

def handle_error(err: AppError) -> str:
    match err:
        Database(db_err) =>
            match db_err:
                ConnectionFailed(host) => return f"Can't reach {host}"
                QueryFailed(q, code) => return f"Query error {code}: {q}"
                NotFound(name) => return f"Not found: {name}"
                PermissionDenied => return "Access denied"
        Validation(msg) => return f"Invalid: {msg}"
        NotAuthenticated => return "Please log in"
```

Tips:

- Keep “leaf” errors close to the layer that produces them (e.g. database layer).
- Wrap/translate into an app-level enum at boundaries so the rest of the app doesn’t depend on lower-level details.

## Pattern 4: Expression trees (ASTs)

Use this when you want to represent recursive structure (expressions, queries, filters) and interpret/transform it.

```incan
enum Expr:
    Number(int)
    Add(Expr, Expr)
    Mul(Expr, Expr)
    Neg(Expr)

def eval(expr: Expr) -> int:
    match expr:
        Number(n) => return n
        Add(a, b) => return eval(a) + eval(b)
        Mul(a, b) => return eval(a) * eval(b)
        Neg(e) => return -eval(e)

# (3 + 4) * -2 = -14
expr = Expr.Mul(
    Expr.Add(Expr.Number(3), Expr.Number(4)),
    Expr.Neg(Expr.Number(2)),
)
result = eval(expr)  # -14
```

Tips:

- Prefer small, composable constructors.
- Use helper functions to build trees if you want a cleaner “builder” API.

## Pattern 5: Behavior on the enum itself

Use enum methods when the operation is part of the closed set's meaning. This keeps callers from reaching for detached helper functions that can drift away from the enum definition.

```incan
trait Describable:
    def describe(self) -> str: ...

enum JobState with Describable:
    Queued
    Running(str)      # worker id
    Failed(str, int)  # message, retry count
    Finished

    def describe(self) -> str:
        match self:
            JobState.Queued => return "queued"
            JobState.Running(worker) => return f"running on {worker}"
            JobState.Failed(message, retry_count) => return f"failed after {retry_count} retries: {message}"
            JobState.Finished => return "finished"

    def is_terminal(self) -> bool:
        match self:
            JobState.Failed(_, _) => return true
            JobState.Finished => return true
            _ => return false

    def initial() -> Self:
        return JobState.Queued
```

Callers get the behavior from the type that owns the cases:

```incan
def log_job[T with Describable](value: T) -> None:
    println(value.describe())

state = JobState.initial()
log_job(state)

if state.is_terminal():
    println("done")
```

Tips:

- Put behavior in the enum body when every variant participates in the operation.
- Use an associated function for a canonical starting value, fallback value, or parser entry point.
- Use `with TraitName` when the enum should be accepted by APIs that depend on a reusable capability.
- Keep free functions for behavior that combines several independent types rather than belonging to the enum itself.

Avoid this shape when the operation really belongs to another layer:

```incan
def render_status_for_admin_dashboard(state: JobState) -> str:
    ...
```

That kind of rendering combines enum state with UI policy, so a free function or view-layer helper is clearer than a method on `JobState`.

## See also

- [Enums (concepts)](../explanation/enums.md)
- [Error handling](../explanation/error_handling.md)
