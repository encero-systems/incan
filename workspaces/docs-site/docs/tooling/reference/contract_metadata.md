# Checked contract metadata

## Project input

`[tool.incan.metadata] model-bundles` accepts an array of model-bundle JSON paths. Relative paths are resolved against the project root.

## Bundle schema

A single model has `schema_version: 1`, `stable_model_id`, `logical_type_name`, `publishable` and an ordered `fields` array. A package object has `schema_version: 1` and a `model_bundles` array of model objects.

| Field | Type | Contract |
| --- | --- | --- |
| `stable_model_id` | String or null | Required identity for a publishable bundle. |
| `logical_type_name` | String | Incan model name. |
| `publishable` | Boolean | Whether the model may be included in artifact metadata. |
| `fields` | Array | Ordered field definitions. |
| Field `name` | String | Incan field name. |
| Field `type` | String | Incan type spelling. |
| Field `nullable` | Boolean | Nullable fields have `Option[T]` unless already optional. |
| Field `alias` | String or null | Wire name. |
| Field `description` | String or null | Reflection description. |
| Field `metadata` | Object | Producer metadata outside type identity. |

Duplicate logical names, stable IDs, field names or aliases are refused. Empty aliases, descriptions or metadata keys and unknown or opaque type spellings are refused.

## Materialized declarations

Declared bundles contribute public model declarations before source checking. Constructors, field access, JSON aliases and field reflection use the declared model fields. A model name colliding with another visible declaration or bundle is refused.

`incan describe-unit PATH` reports the declared bundle paths as `contract_models`.

## LSP command

`incan-lsp` serves the `workspace/executeCommand` command `incan.metadata.model.emit`. Its arguments are one object, or the positional array `[URI_OR_PATH, MODEL, FORMAT]` with `FORMAT` optional.

| Argument | Contract |
| --- | --- |
| `uri` or `path` | A `file:` URI or path naming a project directory, a source file in a project, a bundle JSON file or a `.incnlib` artifact. |
| `model` | A bundle's `logical_type_name` or `stable_model_id`. |
| `format` | `incan` (default) or `json`. |

The result is an object with `format`, `model`, `stableModelId` and either `source` for `incan` or `bundle` for `json`. Invalid arguments, an unknown format or model, and an `.incnlib` without model bundles return an object with an `error` string.
