# Callable presets

This page is the reference for callable presets created with `partial`.

A callable preset is a callable derived from a target callable by presetting named arguments. Each preset parameter becomes a defaulted parameter whose default is the preset value, and the preset has the target's return type.

For the mental model and examples, see [Callable presets explained](../explanation/callable_presets.md).

## Forms

| Form                    | Written as                                           | Where                                       |
| ----------------------- | ---------------------------------------------------- | ------------------------------------------- |
| Top-level declaration   | `[pub] name = partial target(keyword=value, ...)`    | At module level                             |
| Method partial          | `name = partial method(keyword=value, ...)`          | In the body of a model, class, trait or newtype |
| Local partial expression | `partial callable_expression(keyword=value, ...)`   | Anywhere an expression is allowed           |

```incan
pub def route(method: str, path: str) -> str:
    return f"{method} {path}"

pub get = partial route(method="GET")                # accepted

model Cell:
    alive: bool

    def set_state(mut self, state: bool) -> None:
        self.alive = state

    set_alive = partial set_state(state=true)         # accepted

def status_line(method: str) -> str:
    fetch = partial route(method=method)              # accepted
    return fetch("/health")
```

Every preset is a named argument; a positional preset is a syntax error, `INCAN-P0001`. A partial presets at least one argument.

## Targets

| Form                     | Target                                                                                                                                                                                                                                |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Top-level declaration    | A function, declared or imported (`partial math.sqrt(x=4.0)`), a model, class or newtype constructor, or a symbol alias or top-level partial that resolves to one of these. A newtype constructor's parameter is named `value`. |
| Method partial           | A method of the same type, named by its unqualified name or through a same-type method alias (`display = label`).                                                                                                                    |
| Local partial expression | A callable named by its name or path: a function, a generic function, a model, class or newtype constructor, a symbol alias, a top-level partial, or a method of a value (`partial user.label(prefix="x")`).                              |

A top-level target that is a const, a static, a module, an enum variant, a call expression, a local variable, a closure, a field or an unbound method is error `INCAN-T0001`. A local partial expression whose target is not callable, or is a variable or parameter holding a callable, is error `INCAN-T0001`.

## Signature

A partial has the target's signature with these changes:

- unfilled required parameters remain required;
- unfilled defaulted parameters remain defaulted and take the default the target declares; a call through the partial may leave one unbound exactly when a direct call to the target may (see [Ordinary call binding](functions.md#ordinary-call-binding));
- preset parameters become defaulted parameters whose defaults are the preset values, and a call may override a preset by naming it;
- the return type is the target's return type;
- the partial is async exactly when the target is;
- a method partial has the target method's receiver;
- a partial of a generic callable stays generic over the type parameters its presets leave free.

A local partial expression's value has a function type whose parameters are the target's parameters that are not preset, and whose return type is the target's return type.

```incan
model TableReader:
    layer: str
    format: str
    path: str

def reader_for(layer: str) -> (str) -> TableReader:
    return partial TableReader(layer=layer, format="delta")   # accepted
```

Positional arguments bind as follows:

| Form                                     | Positional arguments bind                                                                        |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------ |
| Top-level declaration and method partial | The target's parameters in declaration order, preset parameters included.                         |
| Local partial expression                 | The parameters that are not preset, in declaration order. A preset is overridden only by name. |

```incan
def submit() -> str:
    a = get(path="/submit")                    # accepted
    b = get(method="POST", path="/submit")     # accepted
    c = get("POST", "/submit")                 # accepted
    return get("/submit")                      # refused: INCAN-T0001, path is unbound

def local_call() -> str:
    fetch = partial route(method="GET")
    a = fetch("/health")                       # accepted
    return fetch("POST", "/health")            # refused: INCAN-T0001, one positional parameter
```

## Preset values

| Form                     | Preset values                                                                                                                                                  |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Top-level declaration    | The values listed below. The declaration does not call its target when its module initializes.                                                                 |
| Method partial           | Any expression of the parameter's type, evaluated like a parameter default in the module that declares the type: each time a call leaves the preset unbound. |
| Local partial expression | Any expression of the parameter's type, evaluated once, left to right, when the partial expression is evaluated.                                                |

Each preset value of a top-level declaration is one of:

- a scalar literal, including a negative number literal such as `-2`, a string or bytes literal, or `None`;
- a const, named by its identifier or by a qualified path, whose value is itself such a value;
- an enum variant path whose variant takes no payload, such as `Mode.Fast`;
- a list, set, tuple or dict literal whose elements, keys and values are such values;
- a model literal of a known model whose field values are such values.

Any other preset value of a top-level declaration, such as a function or constructor call, a closure, a comprehension or a spread entry, is error `INCAN-T0001`:

```incan
from std.hash import DEFAULT_CHUNK_SIZE

def scale(k: int, n: int) -> int:
    return k * n

twice = partial scale(k=2)                     # accepted
negative = partial scale(k=-2)                 # accepted
chunked = partial scale(k=DEFAULT_CHUNK_SIZE)  # accepted
computed = partial scale(k=len("ab"))          # refused: INCAN-T0001, a call
```

A `pub` top-level partial targets a public callable, and its preset values name only public items.

## Method and trait partials

A method partial is a method of the type that declares it. It calls the target method with the preset values applied:

```incan
model User:
    name: str

    def label(self, prefix: str) -> str:
        return prefix

    display = label
    short = partial display(prefix="name")

def main() -> None:
    println(User(name="Ada").short())                # accepted
    println(User(name="Ada").short(prefix="other"))  # accepted
```

A method partial declared in a trait is a default method of the trait: every type that adopts the trait has it.

## Imports

A `pub` top-level partial is exported from its module and imported like a function. A call of an imported partial binds its arguments against the partial's signature, preset parameters included. In a package other than the partial's, a leftover defaulted parameter follows the target's cross-package default rule, and every preset of a method partial is a required argument (see [Ordinary call binding](functions.md#ordinary-call-binding), rule 8).

## Refusals

A positional preset is syntax error `INCAN-P0001`. Each of the following is error `INCAN-T0001`:

- a partial that presets no argument;
- a preset name used twice, or one the target does not declare;
- a preset value whose type does not match the parameter;
- a target that is unknown or not one the form allows (see [Targets](#targets));
- a partial whose name is already declared in its module or on its type;
- a target with a rest parameter;
- a partial whose target resolves back to itself, directly or through symbol aliases;
- a `pub` partial whose target is private, or whose preset value names a private item;
- a top-level preset value that is not one of the values listed under [Preset values](#preset-values).
