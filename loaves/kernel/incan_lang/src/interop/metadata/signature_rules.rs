//! Compiler-owned and metadata-free Rust function signature rules.

use super::*;

/// One parameter in a registry-backed Rust free-function signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RustFunctionParamRule {
    pub name: Option<&'static str>,
    pub type_display: &'static str,
}

/// Complete callable contract for one registry-backed Rust free-function surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RustFunctionSignatureRule {
    pub path: &'static str,
    pub params: &'static [RustFunctionParamRule],
    pub return_type: &'static str,
    pub is_async: bool,
    pub is_unsafe: bool,
}

/// Authoritative signatures for compiler-owned runtime helpers whose source typing must be cache-independent.
pub const COMPILER_OWNED_FUNCTION_SIGNATURE_RULES: &[RustFunctionSignatureRule] = &[
    RustFunctionSignatureRule {
        path: "incan_std_core::strings::str_slice_byte_range",
        params: &[
            RustFunctionParamRule {
                name: Some("s"),
                type_display: "&str",
            },
            RustFunctionParamRule {
                name: Some("start"),
                type_display: "i64",
            },
            RustFunctionParamRule {
                name: Some("end"),
                type_display: "i64",
            },
        ],
        return_type: "String",
        is_async: false,
        is_unsafe: false,
    },
    RustFunctionSignatureRule {
        path: "incan_std_core::strings::str_slice_from_byte_offset",
        params: &[
            RustFunctionParamRule {
                name: Some("s"),
                type_display: "&str",
            },
            RustFunctionParamRule {
                name: Some("start"),
                type_display: "i64",
            },
        ],
        return_type: "String",
        is_async: false,
        is_unsafe: false,
    },
];

/// Metadata-free Rust free-function signatures for stable APIs absent from the selected fact cache.
///
/// A partial cache may omit individual sysroot functions. Keep this table to stable APIs whose signatures are part
/// of Rust's public contract and whose result shapes matter for Incan source typing.
pub const METADATA_FREE_FUNCTION_SIGNATURE_RULES: &[RustFunctionSignatureRule] = &[
    RustFunctionSignatureRule {
        path: "std::io::stdin",
        params: &[],
        return_type: "std::io::Stdin",
        is_async: false,
        is_unsafe: false,
    },
    RustFunctionSignatureRule {
        path: "std::io::stdout",
        params: &[],
        return_type: "std::io::Stdout",
        is_async: false,
        is_unsafe: false,
    },
    RustFunctionSignatureRule {
        path: "std::fs::metadata",
        params: &[RustFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<std::fs::Metadata>",
        is_async: false,
        is_unsafe: false,
    },
    RustFunctionSignatureRule {
        path: "std::fs::symlink_metadata",
        params: &[RustFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<std::fs::Metadata>",
        is_async: false,
        is_unsafe: false,
    },
    RustFunctionSignatureRule {
        path: "std::fs::read",
        params: &[RustFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<Vec<u8>>",
        is_async: false,
        is_unsafe: false,
    },
    RustFunctionSignatureRule {
        path: "std::fs::read_dir",
        params: &[RustFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<std::fs::ReadDir>",
        is_async: false,
        is_unsafe: false,
    },
    RustFunctionSignatureRule {
        path: "std::fs::read_to_string",
        params: &[RustFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<String>",
        is_async: false,
        is_unsafe: false,
    },
    RustFunctionSignatureRule {
        path: "std::fs::write",
        params: &[
            RustFunctionParamRule {
                name: Some("path"),
                type_display: "impl AsRef<std::path::Path>",
            },
            RustFunctionParamRule {
                name: Some("contents"),
                type_display: "impl AsRef<[u8]>",
            },
        ],
        return_type: "std::io::Result<()>",
        is_async: false,
        is_unsafe: false,
    },
];

/// Return conservative callable metadata for Rust free functions whose signatures are stable but unavailable through
/// the selected Rust fact cache.
#[must_use]
pub fn metadata_free_function_signature(rust_path: &str) -> Option<RustFunctionSig> {
    let rule = METADATA_FREE_FUNCTION_SIGNATURE_RULES
        .iter()
        .find(|rule| rule.path == rust_path)?;
    Some(function_signature_from_rule(rule))
}

/// Return the authoritative callable contract for a compiler-owned Rust runtime helper.
///
/// These helpers are part of the compiler/runtime ABI, so their source typing must not vary with the availability or
/// freshness of extracted Rust metadata. Keep this set deliberately narrow: ordinary Rust dependencies remain
/// owned by their extracted metadata.
#[must_use]
pub fn compiler_owned_function_signature(rust_path: &str) -> Option<RustFunctionSig> {
    let rule = COMPILER_OWNED_FUNCTION_SIGNATURE_RULES
        .iter()
        .find(|rule| rule.path == rust_path)?;
    Some(function_signature_from_rule(rule))
}

/// Materialize a portable callable signature from one registry rule.
pub(super) fn function_signature_from_rule(rule: &RustFunctionSignatureRule) -> RustFunctionSig {
    RustFunctionSig {
        receiver_contract: None,
        bounds: Vec::new(),
        type_params: Vec::new(),
        params: rule
            .params
            .iter()
            .map(|param| RustParam {
                name: param.name.map(str::to_string),
                type_display: param.type_display.to_string(),
            })
            .collect(),
        return_type: rule.return_type.to_string(),
        is_async: rule.is_async,
        is_unsafe: rule.is_unsafe,
    }
}
