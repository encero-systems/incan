//! Canonical WASM ABI names shared by vocab desugarers and compiler tooling.
//!
//! Companion crates export this ABI via [`crate::export_wasm_desugarer!`], while compiler tooling validates and
//! consumes the same names during producer extraction and consumer-time desugaring.
//!
//! The important nuance is that the exported `__incan_*` globals are ABI anchors, not the payload itself. They point
//! the host at 4-byte bookkeeping cells in guest memory, and those cells in turn store buffer offsets/lengths for
//! the request, output, and error channels.

/// Exported linear-memory name required by vocab desugarer modules.
pub const WASM_DESUGAR_MEMORY_EXPORT: &str = "memory";

/// Default exported entrypoint symbol for vocab desugarer execution.
pub const WASM_DESUGAR_ENTRYPOINT: &str = "desugar_block";

/// Required exported initializer symbol for vocab desugarer instances.
pub const WASM_DESUGAR_INIT_ENTRYPOINT: &str = "__incan_init_desugarer";

/// Status code returned by a successful desugarer entrypoint call.
pub const WASM_DESUGAR_SUCCESS_STATUS: i32 = 0;

/// Status code returned by a failed desugarer entrypoint call.
pub const WASM_DESUGAR_FAILURE_STATUS: i32 = 1;

/// Exported `i32` cell global for request-buffer pointer storage.
pub const WASM_DESUGAR_INPUT_PTR_GLOBAL: &str = "__incan_input_ptr";

/// Exported `i32` cell global for request-buffer capacity storage.
pub const WASM_DESUGAR_INPUT_CAPACITY_GLOBAL: &str = "__incan_input_capacity";

/// Exported `i32` cell global for request-buffer length storage.
pub const WASM_DESUGAR_INPUT_LEN_GLOBAL: &str = "__incan_input_len";

/// Exported `i32` cell global for response-buffer pointer storage.
pub const WASM_DESUGAR_OUTPUT_PTR_GLOBAL: &str = "__incan_output_ptr";

/// Exported `i32` cell global for response-buffer length storage.
pub const WASM_DESUGAR_OUTPUT_LEN_GLOBAL: &str = "__incan_output_len";

/// Exported `i32` cell global for error-buffer pointer storage.
pub const WASM_DESUGAR_ERROR_PTR_GLOBAL: &str = "__incan_error_ptr";

/// Exported `i32` cell global for error-buffer length storage.
pub const WASM_DESUGAR_ERROR_LEN_GLOBAL: &str = "__incan_error_len";

/// Ordered list of required exported `i32` globals used to locate runtime bookkeeping cells.
pub const WASM_DESUGAR_REQUIRED_I32_GLOBAL_EXPORTS: &[&str] = &[
    WASM_DESUGAR_INPUT_PTR_GLOBAL,
    WASM_DESUGAR_INPUT_CAPACITY_GLOBAL,
    WASM_DESUGAR_INPUT_LEN_GLOBAL,
    WASM_DESUGAR_OUTPUT_PTR_GLOBAL,
    WASM_DESUGAR_OUTPUT_LEN_GLOBAL,
    WASM_DESUGAR_ERROR_PTR_GLOBAL,
    WASM_DESUGAR_ERROR_LEN_GLOBAL,
];
