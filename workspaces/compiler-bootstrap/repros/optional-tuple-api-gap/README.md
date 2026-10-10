# Optional borrowed tuple at the public output-selection boundary

Tracked by the open Incan authoring bridge matrix [#872](https://github.com/encero-systems/incan/issues/872), alongside the compiler-loop acceptance in [#1698](https://github.com/encero-systems/incan/issues/1698).

The reduced `src/main.incn` calls the existing public `matching_baked_project_outputs_with_source_authority` function with `None` for its final `Option<(&str, &str)>` argument. Native baking panics during emission: `"(&str,&str)" is not a valid Ident`, at `incan_emit::emit::Codegen::rust_ident`. The function is never executed. The reduced probe and compiler-bootstrap call both fail with exit 101 under the same retained stage-zero compiler and admitted SDK. Complete diagnostics are retained in the recovery evidence.

The existing matching function already selects portable receipt-bound output owners under leases; replacing that selection in a new Rust implementation is unnecessary. The minimal removable workaround is an exported argument-shape adapter which forwards the caller's store, project, entrypoint, target, profile and source authority unchanged, supplying `None` internally. Exact compiler bytes, dependency authority, lock fingerprint, target/toolchain acceptance and verified projection remain authored in Incan. Remove the adapter when the tuple metadata boundary can be called directly.

This does not establish a general inability to call Rust APIs. It records this concrete borrowed-tuple parameter failure and does not authorize a new Rust cache policy.
