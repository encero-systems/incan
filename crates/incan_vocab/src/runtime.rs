//! WASM runtime export helpers for companion crate desugarers.
//!
//! Companion crates can use [`export_wasm_desugarer!`] to expose the expected WASM entrypoint and memory globals
//! consumed by the Incan compiler desugar runtime.
//!
//! The ABI intentionally uses explicit buffers in guest linear memory rather than direct host calls:
//! - the compiler writes one JSON request into the guest input buffer
//! - the desugarer writes either JSON output or an error message into guest-owned buffers
//! - exported `__incan_*` symbols let the host discover where those buffers live
//!
//! `__incan_init_desugarer` is only a required exported function name. It does not imply any `__init__` source file
//! or Python-style module convention; it simply initializes the guest-side bookkeeping cells before the compiler reads
//! or writes buffer state.

/// Default linear-memory buffer capacity used by generated desugarer exports.
pub const WASM_DESUGAR_BUFFER_CAPACITY: usize = 64 * 1024;

/// Export the standard WASM desugarer ABI for one desugarer type.
///
/// The provided type must implement `VocabDesugarer + Default`. The macro emits:
/// - `desugar_block() -> i32` entrypoint
/// - `__incan_*` globals whose values are addresses of 4-byte runtime cells
/// - `__incan_init_desugarer()` initializer that fills those cells with buffer pointers/lengths
///
/// This is the canonical export surface consumed by `incan build --lib` and the compiler desugar pass.
#[macro_export]
macro_rules! export_wasm_desugarer {
    ($desugarer_ty:ty) => {
        #[cfg(target_arch = "wasm32")]
        const _: () = {
            // Exported globals identify 4-byte bookkeeping cells in guest memory.
            #[unsafe(no_mangle)]
            pub static mut __incan_input_ptr: i32 = 0;
            #[unsafe(no_mangle)]
            pub static mut __incan_input_capacity: i32 = 0;
            #[unsafe(no_mangle)]
            pub static mut __incan_input_len: i32 = 0;

            #[unsafe(no_mangle)]
            pub static mut __incan_output_ptr: i32 = 0;
            #[unsafe(no_mangle)]
            pub static mut __incan_output_len: i32 = 0;

            #[unsafe(no_mangle)]
            pub static mut __incan_error_ptr: i32 = 0;
            #[unsafe(no_mangle)]
            pub static mut __incan_error_len: i32 = 0;

            static mut __INCAN_INPUT_BUFFER: [u8; $crate::runtime::WASM_DESUGAR_BUFFER_CAPACITY] =
                [0; $crate::runtime::WASM_DESUGAR_BUFFER_CAPACITY];
            static mut __INCAN_OUTPUT_BUFFER: [u8; $crate::runtime::WASM_DESUGAR_BUFFER_CAPACITY] =
                [0; $crate::runtime::WASM_DESUGAR_BUFFER_CAPACITY];
            static mut __INCAN_ERROR_BUFFER: [u8; $crate::runtime::WASM_DESUGAR_BUFFER_CAPACITY] =
                [0; $crate::runtime::WASM_DESUGAR_BUFFER_CAPACITY];

            #[unsafe(no_mangle)]
            pub extern "C" fn __incan_init_desugarer() {
                unsafe {
                    // Point each bookkeeping cell at the guest-owned request/response buffers.
                    __incan_input_ptr = __INCAN_INPUT_BUFFER.as_ptr() as usize as i32;
                    __incan_input_capacity = __INCAN_INPUT_BUFFER.len() as i32;
                    __incan_output_ptr = __INCAN_OUTPUT_BUFFER.as_ptr() as usize as i32;
                    __incan_output_len = 0;
                    __incan_error_ptr = __INCAN_ERROR_BUFFER.as_ptr() as usize as i32;
                    __incan_error_len = 0;
                }
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn desugar_block() -> i32 {
                unsafe {
                    // The host writes the request bytes first, then records the used prefix length here.
                    let input_len = if __incan_input_len < 0 {
                        0
                    } else {
                        __incan_input_len as usize
                    };
                    if input_len > __INCAN_INPUT_BUFFER.len() {
                        write_error("input length exceeds desugarer buffer capacity");
                        return $crate::WASM_DESUGAR_FAILURE_STATUS;
                    }

                    let request_bytes = &__INCAN_INPUT_BUFFER[..input_len];
                    match $crate::desugar::execute_desugar_request::<$desugarer_ty>(request_bytes) {
                        Ok(output_bytes) => {
                            // Successful desugaring returns JSON in the output buffer.
                            if output_bytes.len() > __INCAN_OUTPUT_BUFFER.len() {
                                write_error("desugar response exceeds output buffer capacity");
                                return $crate::WASM_DESUGAR_FAILURE_STATUS;
                            }
                            __INCAN_OUTPUT_BUFFER[..output_bytes.len()].copy_from_slice(&output_bytes);
                            __incan_output_len = output_bytes.len() as i32;
                            __incan_error_len = 0;
                            $crate::WASM_DESUGAR_SUCCESS_STATUS
                        }
                        Err(message) => {
                            // Failures still round-trip through the guest-owned error buffer.
                            write_error(&message);
                            $crate::WASM_DESUGAR_FAILURE_STATUS
                        }
                    }
                }
            }

            fn write_error(message: &str) {
                unsafe {
                    let bytes = message.as_bytes();
                    let max_len = __INCAN_ERROR_BUFFER.len();
                    let len = std::cmp::min(bytes.len(), max_len);
                    __INCAN_ERROR_BUFFER[..len].copy_from_slice(&bytes[..len]);
                    __incan_error_len = len as i32;
                    __incan_output_len = 0;
                }
            }
        };
    };
}
