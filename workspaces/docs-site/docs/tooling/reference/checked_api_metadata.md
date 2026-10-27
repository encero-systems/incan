# Checked API metadata

Compiled libraries publish checked API metadata in the `contract_metadata.api` field of their `.incnlib` manifest. The package and each module use `schema_version: 2`.

## Package and namespaces

| Field | Type | Contract |
| --- | --- | --- |
| `schema_version` | Integer | `2`. |
| `package` | Object, optional | `name: str` and optional `version: str`. |
| `modules` | Array | Module API records. |
| `public_namespaces` | Array, omitted when empty | Public namespace records with `path: list[str]`, `members` and `child_modules: list[str]`. Empty member and child lists may be omitted. |

A namespace member has `name: str` and `source_path: list[str]`, the source module path followed by the declaration name. A module record has `schema_version`, `module_path: list[str]`, `declarations`, and an optional `derivable_traits: list[str]` (empty when omitted).

## Declarations

Each declaration has a `kind`, a `name` and an `anchor`. The anchor has `id: str` and `span`, the declaration's source coordinates and expansion identity. Declaration kinds are:

| `kind` | API data |
| --- | --- |
| `function` | Type parameters, parameters, return type, async marker, decorators, documentation and optional `native_attribute` path. |
| `model` | Type parameters, traits and trait adoptions, derives, fields, properties, methods and documentation. |
| `class` | The model fields above and optional `extends` base class. |
| `trait` | Type parameters, supertraits, required fields, methods and documentation. |
| `enum` | Type parameters, traits, derives, variants, variant aliases, optional value type, methods and documentation. Each variant has a name, payload field types, optional value and optional canonical identity. |
| `newtype` | Type parameters, traits, derives, underlying type, `is_rusttype`, optional checked constructor, constraints, implicit-coercion flag, methods and documentation. |
| `type_alias` | The exported type alias. |
| `const` | `ty` and optional safe metadata `value`. |
| `static` | `ty`. |
| `alias` | `target_path`, `is_public`, optional projected function and optional projected type. |
| `partial` | Target path and kind, presets, type parameters, parameters, return type and async marker. |

Callables retain parameter names, types, default values and `is_mut`; function types retain mutable parameter markers. Methods also retain their receiver, canonical identity when available, alias target when applicable, and whether they have a body. Documentation-bearing declarations retain both `docstring` and parsed `docstring_sections`, each nullable.

## Editor preview

LSP hover on an imported public declaration previews its checked signature and documentation. Module hover identifies the imported package namespace. Completion lists public namespace members and child modules. The protocol's supported requests are listed in [LSP protocol support](lsp_protocol_support.md).

Public source declaration records, documentation digests and checked identities are specified in [Codegraph records](codegraph_inspection.md). [Agent context CLI and MCP](agent_context_cli.md) specifies declaration queries, context packs and snapshot freshness. Model-bundle data is specified in [Checked contract metadata](contract_metadata.md).
