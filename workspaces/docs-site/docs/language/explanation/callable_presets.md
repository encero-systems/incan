# Callable presets explained

Callable presets let a module name a common call shape without writing a wrapper whose only job is to pass the same keyword argument again.

```incan
pub def route(method: str, path: str, content_type: str = "text") -> str:
    return method

pub get = partial route(method="GET")
```

`get` is a callable derived from `route`. It still has the same result type, and callers still see `method` as a normal defaulted parameter:

```incan
def status() -> str:
    return get(path="/health")

def submit() -> str:
    return get(method="POST", path="/submit")
```

The second call is valid because a preset is a default, not a permanent freeze.

!!! tip "Coming from Python?"
    Think `functools.partial`, but with compile-time shape.

    Python's `partial(route, method="GET")` creates a runtime object. Incan's top-level `partial route(method="GET")` is a declaration the compiler can typecheck, export, and show in tooling. Local partial expressions are the runtime form when a preset needs local values.

## Why presets exist

The common alternative is a wrapper:

```incan
def get(path: str, content_type: str = "text") -> str:
    return route(method="GET", path=path, content_type=content_type)
```

That is useful when the wrapper adds behavior. It is noise when the wrapper only specializes one or two named arguments.

Callable presets keep the specialization visible while preserving the target callable's type information:

```incan
pub get = partial route(method="GET")
pub post = partial route(method="POST")
```

Use a preset when the new callable is the same operation with named defaults. Use a wrapper when the new callable validates input, branches, logs, retries, transforms values, or changes the public story enough to deserve its own body.

## Top-level presets

Top-level presets are for public or private module API surfaces:

```incan
pub BronzeReader = partial Reader(layer="bronze", format="delta")
```

The important rule is that top-level presets do not run during module initialization. Their preset values must be declaration-safe, such as literals, safe collections, and safe model literals:

```incan
pub model Profile:
    name: str

pub configured = partial configure(
    headers={"accept": "json"},
    codes=[200],
    profile=Profile(name="ops"),
)
```

If a preset value needs runtime state, use a local partial expression or a wrapper function.

## Local presets

Local presets are expressions. They evaluate where they appear and may capture local values:

```incan
def reader_for(layer: str) -> Callable[[str], Reader]:
    return partial Reader(layer=layer)
```

That makes local presets the right form when the preset value depends on a function argument, a local binding, or another runtime expression.

## Method presets

Inside a model, class, trait, or newtype, a preset can define a same-type method:

```incan
model Cell:
    alive: bool

    def set_state(mut self, state: bool) -> None:
        self.alive = state

    set_alive = partial set_state(state=true)
```

The generated method has the same receiver behavior as the target. Trait method presets follow the same path as ordinary trait default methods, so adopters receive a generated forwarding method.

## Presets, aliases, and wrappers

These three forms mean different things:

```incan
pub get = partial route(method="GET")
pub also_route = route

def checked_get(path: str) -> str:
    assert path.startswith("/")
    return route(method="GET", path=path)
```

- `get` is a new callable surface with one named default preset.
- `also_route` is only another name for `route`.
- `checked_get` is a real wrapper with behavior.

Use the smallest form that tells the truth about the API.
