# `third_party`

Vendored patches. Today: `crates/third_party/ra_ap_proc_macro_api` (postcard default features off, issue #260).

Its removal condition is already met upstream (`ra_ap_proc_macro_api` 0.0.350 ships postcard without defaults at MSRV 1.98), so this directory is expected to be empty once `compiler/incan_inspect` moves to rust-analyzer 0.0.350 and the project-JSON loader.
