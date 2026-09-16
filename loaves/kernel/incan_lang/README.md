# `incan_lang`

Ring: **kernel**

Language vocabulary, keyword and builtin tables, the stdlib registry, and shared semantic helpers.

## Current location

- `loaves/kernel/incan_lang/`

## May depend on

none

The crate was `incan_core` until layout step 5 renamed it, so that `core` names the mandatory standard-library component and nothing else. It is the most-imported crate in the repository; the rename is a substring rewrite over every spelling in the tree, with no change to the crate's contents or API.
