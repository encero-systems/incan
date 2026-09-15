# `incan_syntax`

Ring: **kernel**

Lexer, parser, AST, and the diagnostics catalog.

## Moved here from

- `crates/incan_syntax/`

## May depend on

`incan_lang`, `incan_vocab`, `incan_semantics`

Drops the miette `fancy` feature and the unused `thiserror` and `insta` dependencies on the way in. Renders nothing; it builds diagnostics.
