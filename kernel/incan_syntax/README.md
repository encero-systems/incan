# `incan_syntax`

Ring: **kernel**

Lexer, parser, AST, and the diagnostics catalog.

## Moves here from

- `crates/incan_syntax/`

## May depend on

`incan_lang`, `incan_vocab`, `incan_semantics`

Drops the miette `fancy` feature and the unused `thiserror` and `insta` dependencies on the way in. Renders nothing; it builds diagnostics.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
