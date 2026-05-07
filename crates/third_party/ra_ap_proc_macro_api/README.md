# Patched `ra_ap_proc_macro_api`

This crate is a local copy of `ra_ap_proc_macro_api` 0.0.325 with one manifest-only patch: its `postcard` dependency disables default features while keeping `alloc`.

The upstream manifest requests `postcard` with `alloc` but leaves `default-features` enabled. In `postcard` 1.1.3 the default feature is `heapless-cas`, which enables `heapless/cas` and pulls the unmaintained `atomic-polyfill` crate into Incan's dependency graph.

Remove this patch when upstream `ra_ap_proc_macro_api` no longer enables postcard defaults for this dependency.
