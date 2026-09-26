# Decorators (how-to)

This guide shows how to write the common kinds of user-defined decorator and how to move a method decorator to the receiver spelling of the method it decorates.

For the exact rules, see [Decorators (reference)](../reference/language.md#decorators). For why a method decorator spells the receiver the way the method does, see [Decorated methods](../explanation/rust_shaped_confidence.md#decorated-methods).

!!! tip "Coming from Python?"
    A Python decorator can replace a function with any object. An Incan decorator receives the decorated callable and must return a callable, and the declared name has the type of what it returns. Python's `Callable[[A, B], R]` is Incan's `(A, B) -> R`; `=>` is only for closure expressions, not for callable types. Stacked decorators apply bottom-up, as in Python.

## Task: record a function and keep its signature

Registry, catalog, routing, telemetry and validation decorators usually record something about the function and return it unchanged. Make the decorator, or the factory that produces it, generic over the whole callable, `(F) -> F`, so the decorated name keeps its own signature:

```incan
def registered[F](function_ref: str) -> ((F) -> F):
    return (func) => func

@registered("incql.functions.col")
pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)
```

If `F` cannot be inferred where the decorator is applied, pass the decorated function's type on the factory call:

```incan
@registered[(str) -> ColumnExpr]("incql.functions.col")
pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)
```

## Task: record the decorated function's name

Read `func.__name__` in the decorator instead of repeating the declaration's name in a string argument:

```incan
def capture[F](func: F) -> F:
    registry_names.append(func.__name__)
    return func

def registered[F]() -> ((F) -> F):
    return (func) => capture[F](func)

@registered()
pub def sample(value: int) -> int:
    return value + 1
```

## Task: change the decorated function's signature

Spell the shape the decorator accepts and the shape it returns separately:

```incan
def as_int(func: (int) -> str) -> (int) -> int:
    return parse

def parse(value: int) -> int:
    return value

@as_int
def label(value: int) -> str:
    return "value"
```

A factory that changes the signature spells both shapes in its result, such as `((str) -> R) -> ((str, str) -> R)`.

## Task: decorate a method

1. Write the receiver in the decorator's shapes the way the method writes it: `(Box, int) -> str` for `def label(self, value: int) -> str`, and `(mut Counter, int) -> int` for `def bump(mut self, by: int) -> int`.
2. Declare a function the decorator returns in the method's place with the receiver as its first parameter: `box: Box` for a `self` method, `mut counter: Counter` for a `mut self` method.
3. For a `self` method, keep the decorator and the functions it returns private to the module that declares the method's type, write their shapes as callable types, return functions by name, and use the callable the decorator accepts only to return it. Use a function returned in the method's place only by returning it or by calling it directly in that module.

```incan
class Box:
    value: int

    @as_int
    def label(self, value: int) -> str:
        return "value"

def parse(box: Box, value: int) -> int:
    return value + 1

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
    return parse
```

If the compiler refuses the chain with `INCAN-T0116`, the message names the rule it breaks:

- an imported decorator: move the decorator into the module that declares the method's type;
- a shape written through a type alias: write the callable type out;
- a returned closure or local variable: return a function declared in the module, by name;
- the decorated callable called, stored or passed on inside the decorator: return it unchanged, or make the decorator generic over the whole callable, `(F) -> F`;
- a `pub` function in the chain: make it private and give other modules a separate function;
- a function of the chain used elsewhere, such as passed as a value: give that use its own function.

## Task: migrate a method decorator to the method's receiver spelling

Method decorators written for 0.5 spelled the receiver `&Box` or `&mut Box`. The compiler refuses that spelling with `INCAN-T0110` and names the spelling to write.

1. In each shape of a `self` method's decorator, replace `&Box` with `Box`.
2. In each shape of a `mut self` method's decorator, replace `&mut Box` with `mut Box`.
3. In each function a decorator returns in the method's place, replace `box: &Box` with `box: Box`, and `box: &mut Box` with `mut box: Box`.
4. Check the program. If `INCAN-T0116` refuses a `self` method's chain, follow [decorate a method](#task-decorate-a-method).

Before:

```incan
class Box:
    value: int

    @as_int
    def label(self, value: int) -> str:
        return "value"

    @keep
    def bump(mut self, by: int) -> int:
        self.value += by
        return self.value

def parse(box: &Box, value: int) -> int:
    return value + 1

def as_int(func: (&Box, int) -> str) -> (&Box, int) -> int:
    return parse

def keep(func: (&mut Box, int) -> int) -> (&mut Box, int) -> int:
    return func
```

After:

```incan
class Box:
    value: int

    @as_int
    def label(self, value: int) -> str:
        return "value"

    @keep
    def bump(mut self, by: int) -> int:
        self.value += by
        return self.value

def parse(box: Box, value: int) -> int:
    return value + 1

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
    return parse

def keep(func: (mut Box, int) -> int) -> (mut Box, int) -> int:
    return func
```

The migrated program passes the receiver as the former spelling did.
