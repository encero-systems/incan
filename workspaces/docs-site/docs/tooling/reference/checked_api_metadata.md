# Checked API Metadata

This page specifies the JSON that `incan tools metadata api` prints: the checked public API of an Incan package or module. The command's options are in the [CLI reference](cli_reference.md#incan-tools-metadata-api).

## Command

```bash
incan tools metadata api [PATH] --format json
incan tools metadata api [PATH] --format markdown
```

- `PATH` (default `.`) is a source file, or a directory whose `src/lib.incn`, else `src/main.incn`, is the entry. A directory with neither, or a missing path, is refused.
- The entry and the local modules it imports are type-checked first. A type error, or a docstring that contradicts the checked source (see [Docstrings](#docstrings)), produces diagnostics and no output.
- `--format json` (the default) prints the metadata package; `--format markdown` prints a Markdown API reference generated from the same metadata.
- The command does not build the project, generate Rust, or read a `.incnlib` artifact. The metadata describes the public declarations of the checked source and the contract models the checked program materializes. Model bundles are specified in [Checked contract metadata](contract_metadata.md).

## Example

For a project named `catalog` at version `0.1.0` with this `src/lib.incn`:

```incan
pub const DEFAULT_LABEL = "catalog"

@rust.allow("dead_code")
pub def label() -> str:
    """Return the catalog label."""
    return DEFAULT_LABEL
```

`incan tools metadata api . --format json` prints:

```json
{
  "schema_version": 2,
  "package": {
    "name": "catalog",
    "version": "0.1.0"
  },
  "modules": [
    {
      "schema_version": 2,
      "module_path": [
        "lib"
      ],
      "declarations": [
        {
          "kind": "const",
          "name": "DEFAULT_LABEL",
          "anchor": {
            "id": "lib::DEFAULT_LABEL",
            "span": {
              "start": 0,
              "end": 35
            }
          },
          "ty": {
            "Named": {
              "name": "FrozenStr"
            }
          },
          "value": {
            "kind": "string",
            "value": "catalog"
          }
        },
        {
          "kind": "function",
          "name": "label",
          "anchor": {
            "id": "lib::label",
            "span": {
              "start": 37,
              "end": 147
            }
          },
          "docstring": "Return the catalog label.",
          "docstring_sections": {
            "summary": "Return the catalog label.",
            "params": [],
            "returns": null,
            "fields": [],
            "aliases": [],
            "decorators": []
          },
          "decorators": [
            {
              "path": [
                "rust",
                "allow"
              ],
              "source_name": "rust.allow",
              "anchor": {
                "start": 37,
                "end": 61
              },
              "args": [
                {
                  "kind": "positional",
                  "value": {
                    "kind": "literal",
                    "value": {
                      "kind": "string",
                      "value": "dead_code"
                    }
                  }
                }
              ],
              "decorated_callable": {
                "name": "label",
                "anchor": {
                  "id": "lib::label",
                  "span": {
                    "start": 37,
                    "end": 147
                  }
                },
                "type_params": [],
                "params": [],
                "return_type": {
                  "Named": {
                    "name": "str"
                  }
                },
                "is_async": false
              }
            }
          ],
          "type_params": [],
          "params": [],
          "return_type": {
            "Named": {
              "name": "str"
            }
          },
          "is_async": false
        }
      ]
    }
  ]
}
```

## Package

| Field | Type | Contents |
| --- | --- | --- |
| `schema_version` | number | `2`. |
| `package` | object | `name` from `loaf.toml`, and `version` when declared. Omitted without a manifest that declares a project name. |
| `modules` | array | One module document for the entry module and for each local module it imports. |
| `public_namespaces` | array | The package's public namespaces. Omitted when empty. |

Each `public_namespaces` entry, sorted by `path`:

| Field | Type | Contents |
| --- | --- | --- |
| `path` | array of strings | The namespace path. |
| `members` | array | Each public declaration of the module at `path`, and of its direct child modules: its `name` and `source_path` (the declaring module path followed by the name), sorted. Omitted when empty. |
| `child_modules` | array of strings | The names of the namespace's direct child modules, sorted. Omitted when empty. |

Every prefix of a module's path is a namespace, except for a module whose path is `["lib"]` or `["main"]`. A public import alias is a member only when it is public in source.

## Modules

| Field | Type | Contents |
| --- | --- | --- |
| `schema_version` | number | `2`. |
| `module_path` | array of strings | The entry module's file stem, such as `["lib"]`, or an imported module's logical path. |
| `derivable_traits` | array of strings | The module's `__derives__` list: the members a module derive such as `@derive(toml)` expands to, each resolved among the module's declarations. Omitted when empty. |
| `declarations` | array | The module's public declarations. |

The `__derives__` const is not listed in `declarations`. A derivable trait's declaration lists its explicit Rust derive decorator in its `decorators`.

## Declarations

Every declaration has a `kind`, its `name`, and an `anchor`: an `id` (the module path and the name joined by `::`) and a `span` (`start` and `end` byte offsets of the declaration). The other fields depend on `kind`:

| `kind` | Fields |
| --- | --- |
| `function` | `docstring`, `docstring_sections`, `decorators`, `type_params`, `params`, `return_type`, `is_async`. |
| `model` | `docstring`, `docstring_sections`, `decorators`, `type_params`, `traits`, `trait_adoptions`, `derives`, `fields`, `properties`, `methods`. |
| `class` | The `model` fields, and `extends`: the base class name or `null`. |
| `trait` | `docstring`, `docstring_sections`, `decorators`, `type_params`, `supertraits` (type bounds), `requires` (required fields), `methods`. |
| `enum` | `docstring`, `docstring_sections`, `decorators`, `type_params`, `traits`, `trait_adoptions`, `value_type` (`"str"`, `"int"` or `null`), `variants`, `variant_aliases` (each a `name` and its `target` variant), `methods`, `derives`. |
| `newtype` | `docstring`, `docstring_sections`, `decorators`, `type_params`, `traits`, `trait_adoptions`, `derives`, `is_rusttype`, `underlying` (type), `checked_constructor`, `constraints`, `implicit_coercion_enabled`, `methods`. |
| `type_alias` | `type_alias`: the alias's `name`, `type_params` and `target` type. |
| `const` | `ty`, and `value`: a [safe value](#safe-values), or `null` when the value is not one. |
| `static` | `ty`. |
| `alias` | `target_path` (resolved target path segments), `is_public`, `projected_function`, `projected_type`. |
| `partial` | `target_path`, `target_kind`, `presets`, `type_params`, `params`, `return_type`, `is_async`. |

- `docstring` is the raw docstring text, or `null`; `docstring_sections` is its parsed form, or `null` without a docstring.
- `traits` lists adopted trait names; `trait_adoptions` lists them as type bounds, with their type arguments. On an `enum` or `newtype`, `traits` is omitted when empty; `trait_adoptions` is omitted when empty.
- `derives` lists the `@derive(...)` names. On a `newtype` it is omitted when empty.
- `properties`, and the `methods` of an `enum`, are omitted when empty.
- A `newtype`'s `checked_constructor` names its checked constructor and is omitted when there is none. `constraints` lists each primitive constraint as a `key` (`ge`, `gt`, `le` or `lt`), an integer `value` and its source `repr`, and is omitted when empty. `implicit_coercion_enabled` states whether the newtype admits implicit coercion.
- An `alias`'s `projected_function` is present when the target is a public function or a callable-valued decorated binding: the target's `source_path`, its `callable` metadata under the alias name, and its `decorators`. `projected_type` is present when a public type is re-exported: the checked nominal type. Both are omitted otherwise.
- A `partial`'s `target_kind` is `function`, `model_constructor`, `class_constructor`, `newtype_constructor`, `partial` or `unknown`. Each preset is also a parameter in `params` with `has_default: true`; `presets` lists each preset's `name`, checked type `ty`, and `value`.

## Members

A method:

| Field | Contents |
| --- | --- |
| `name`, `anchor` | The method name, and its anchor, whose `id` ends in `Owner.method`. |
| `canonical` | The method's canonical identity; omitted when not recorded. Same-name overloads have distinct identities. |
| `alias_of` | The method this entry aliases on the same type; omitted for a method with its own body. |
| `docstring`, `docstring_sections`, `decorators` | As for a declaration. |
| `type_params`, `params`, `return_type`, `is_async` | The checked signature. |
| `receiver` | `"Immutable"` for `self`, `"Mutable"` for `mut self`, or `null` without a receiver. |
| `has_body` | Whether the declaration has a body. |

A property has a `name`, a `canonical` identity (omitted when not recorded) and its `return_type`.

A field of a model or class, or a trait's required field:

| Field | Contents |
| --- | --- |
| `name`, `ty` | The field name and its type. |
| `canonical` | The field's canonical identity; omitted when not recorded. |
| `surface_type_name` | The type as written in source; omitted when not recorded. |
| `visibility` | `"private"` for a field declared without `pub`; omitted for a public field. |
| `has_default`, `default` | Whether the field declares a default, and that default as a [default value](#safe-values); `default` is omitted when absent. |
| `alias`, `description` | The field's alias and description metadata, or `null`. |

An enum variant has a `name`, a `canonical` identity (omitted when not recorded), its payload `fields` (types), and its `value`: the raw string or integer of a value enum's variant, or `null`.

A canonical identity has `namespace` (`ordinary_lexical`, `member` or `module_path`), `origin` (`kind` `package` with `library` and `module_path`, `rust_crate` with `path`, or `builtin`), `declaration_name`, `kind`, and `declaration_span` (`start` and `end`).

## Parameters and type parameters

A parameter:

| Field | Contents |
| --- | --- |
| `name`, `ty` | The parameter name and its type. |
| `kind` | `normal`, `rest_positional` (`*args`) or `rest_keyword` (`**kwargs`). |
| `has_default` | Whether the parameter has a default or a preset. |
| `is_mut` | `true` for a parameter the function's type marks `mut`; omitted when unmarked. |
| `default` | The default as a [default value](#safe-values); omitted when absent. |

A type parameter has a `name` and its `bounds`. A type bound has:

| Field | Contents |
| --- | --- |
| `name`, `type_args` | The trait name and its type arguments. |
| `source_name`, `module_path` | The spelling in source and the trait's module path; each omitted when not recorded. |
| `implementation_type_params` | The generic header that this one implementation requires: each parameter's `name` and `bounds`, each bound a `trait_path`, `type_args` and `associated_types` (each omitted when empty) and an `origin` of `standard`, `rust_capability` or `source_callable`. Omitted when empty. |
| `inferred` | `true` for an `Eq` or `Hash` bound inferred from the callable's body; omitted when the bound is declared. |

## Types

A type is one JSON object keyed by its form, or a string for a form without fields:

| Form | Encoding |
| --- | --- |
| Named type | `{"Named": {"name": "str"}}`, with an `origin` for a nominal type of another package: the declaring artifact's `provider` identity and the type's `canonical` identity. |
| Generic application | `{"Applied": {"name": "List", "args": [...]}}`, with an `origin` as for `Named`. A union is `Applied` with `"name": "Union"` unless it is a `NativeUnion`. |
| Function type | `{"Function": {"params": [...], "return_type": ...}}` |
| `mut` parameter of a function type | `{"MutParam": {"inner": ...}}`, only as an element of a `Function`'s `params`. |
| Type token | `{"TypeToken": {"inner": ...}}` |
| Tuple | `{"Tuple": {"elements": [...]}}` |
| Type parameter | `{"TypeParam": {"name": "T"}}` |
| Receiver type | `"SelfType"` |
| Reference | `{"Ref": {"inner": ...}}` |
| Rust path | `{"RustPath": {"path": "..."}}` |
| Unknown | `"Unknown"` |
| Emitted union | `{"NativeUnion": {...}}` |

`NativeUnion` appears in the metadata of a built library (`incan build --lib`) for a union whose native wrapper the build emitted:

| Field | Type | Contents |
| --- | --- | --- |
| `owner` | `"ContainingArtifact"` or `{"SelectedArtifact": ProviderIdentity}` | The artifact that defines the wrapper. A containing owner binds to the exact artifact selected on import; a forwarded union keeps its defining provider's name, version, digest and feature projection. |
| `rust_name` | string | The wrapper's name in the defining artifact. |
| `members` | array of types | The payload types in order: element zero is variant `V0`, element one `V1`, and so on. |
| `local_nominals` | map of type spelling to canonical identity | Owner-local payload identities; every entry names a public nominal declaration of the owner. Omitted when empty. |

The `.incnlib` field `contract_metadata.native_unions` holds the defining artifact's union entries, each with owner `"ContainingArtifact"`. An imported union matches an entry of its owner's table exactly, payload order included. A consumer's import aliases and Rust dependency paths do not change the wrapper or its variant indices.

## Decorators

A decorator entry:

| Field | Contents |
| --- | --- |
| `path` | The resolved decorator path segments. |
| `source_name` | The decorator as written. |
| `anchor` | The decorator's `start` and `end` byte offsets. |
| `type_args` | Explicit type arguments; omitted when empty. |
| `args` | Each argument: `kind` `positional` with a `value`, or `named` with a `name` and a `value`. |
| `decorated_callable` | On a function or method decorator: the decorated declaration's `name`, `anchor`, `type_params`, `receiver` (omitted without one), `params`, `return_type` and `is_async`. |

A decorator argument value is an object with a `kind`:

| `kind` | Fields |
| --- | --- |
| `literal` | `value`: a [safe value](#safe-values). |
| `const_ref` | `name`, and `value`: the const's safe value, or `null`. |
| `symbol_ref` | `path`: the referenced symbol's path segments. |
| `list` | `items`: argument values. |
| `dict` | `entries`: each a `key` and a `value` argument value. |
| `call` | `callee` path segments, `type_args`, and `args`, each `positional`, `named`, `positional_unpack` or `keyword_unpack` with a `value`. |
| `type` | `ty`: a type. |
| `unsupported` | `reason`. |

An argument expression that is none of the other kinds is `unsupported`; it is not evaluated.

## Safe values

A const value, a decorator literal or const value, a preset value and a default value are objects with a `kind` and a `value`:

| `kind` | `value` | Const and decorator values | Preset and default values |
| --- | --- | --- | --- |
| `int` | Integer | Yes | Yes |
| `float` | Number; the literal's spelling as a string in preset and default values | Yes | Yes |
| `bool` | Boolean | Yes | Yes |
| `string` | String | Yes | Yes |
| `bytes` | Array of integers | Yes | Yes |
| `none` | Absent | Yes | Yes |
| `list` | Array of values | No | Yes |
| `dict` | Array of `key` and `value` entries | No | Yes |
| `const_ref` | The const's path segments | No | Yes |
| `model_literal` | The model's `name` and its `fields`, each a `name` and a `value` | No | Presets only |
| `call` | `path`, `args` (each an optional `name` and a `value`) and an optional `signature` | No | Defaults only |
| `unsupported` | Absent | No | Yes |

Metadata carries no value that would require executing user code.

## Docstrings

`docstring_sections` holds the parsed docstring:

| Field | Contents |
| --- | --- |
| `summary` | The text before the first recognized heading, or `null`. |
| `params` | The `Args:` or `Parameters:` entries. |
| `returns` | The `Returns:` section: its `ty` (the type spelling of a `type: description` line, or `null`) and `description`; `null` without the section. |
| `fields`, `aliases`, `decorators` | The `Fields:`, `Aliases:` and `Decorators:` entries. |

An entry is a `name: description` line; a following line without a colon continues the description.

```incan
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.

    Args:
        values: Input values.

    Returns:
        float: Mean value.
    """
    return 0.0
```

A docstring that contradicts the checked source produces diagnostics and no output:

- an `Args:`, `Parameters:` or `Fields:` section that omits a checked parameter or field;
- an entry in any section that names no checked parameter, field, alias or decorator, or names one twice;
- a `Returns:` type spelling that differs from the checked return type.

## See also

- [Read compiler reports from another tool](../how-to/ci_and_automation.md#read-compiler-reports-from-another-tool)
- [LSP protocol support](lsp_protocol_support.md), for the hover previews built from this metadata
