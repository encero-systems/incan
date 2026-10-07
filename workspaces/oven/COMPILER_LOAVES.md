# Compiler Loaf adoption and native preparation

`incan oven convert-cargo --workspace <workspace> --project <crate>` explicitly adopts a workspace Rust library closure. It reads the workspace manifests and Cargo lock once, preserves declared version requirements and feature forwarding, and writes Loaf declarations with Rust facets. Conversion validates every result before writing. Build dependencies are retained as inert adoption inventory; conversion does not run build scripts.

`src/loaf_local_main.incn` reads these Loaf declarations and the retained target cfg. Its arguments are a project path, cfg file, and root features. Root defaults are disabled. It uses the existing feature-activation policy and emits local facet selections plus active registry requests. It reads no Cargo manifests.

The native SDK publisher accepts `INCAN_SDK_NATIVE_COMPILER_GRAPH`, a JSON selection document containing `index_commit`, `registry_lock`, and `facets`. Lock and facet paths are relative to the selection document unless absolute. Each facet has `project`, `features`, and `domain` (`target` or `host`). The registry lock must retain the SDK closure as well as compiler requirements. Native compilation consumes the explicit resolved graph, orders local facets by dependencies, and rejects conflicting bindings. Selection, lock, and local source bytes participate in publication identity. Compiler path admission verifies source snapshot identity before reusing a compiled local binding.

Registry source archives and retained build facts come from incan.pub. The local store and configured mirrors cache verified products; they do not replace registry authority. Missing registry versions or build facts refuse compilation. Bakes must select the debug profile for the compiler-fixture lane.
