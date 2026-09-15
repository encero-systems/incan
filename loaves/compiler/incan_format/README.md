# `incan_format`

Ring: **compiler**

Source formatter.

## Current sources

- `loaves/compiler/incan_format/src/`

## May depend on

`kernel`

The formatter lives here and depends on the syntax crate alone, which is why the frontend can format source for its contract metadata without a cycle.
