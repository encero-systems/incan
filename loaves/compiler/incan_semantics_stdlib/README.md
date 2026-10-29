# `incan_semantics_stdlib`

Ring: **compiler**

The standard surface-semantics pack the frontend consults for assertions, async constructs and decorators. Every compiler consumer includes the complete pack; source imports and provider catalogs control the constructs available to a program. This is compiler implementation per `layering.md`, which is why this crate lives in `loaves/compiler/` rather than the kernel ring.

## Current location

- `loaves/compiler/incan_semantics_stdlib/`

## May depend on

`kernel`
