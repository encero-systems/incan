//! Fallback Rust extension-trait method vocabulary used when rust-inspect metadata is unavailable.

/// Return fallback trait method names for Rust traits when structured trait metadata is unavailable.
#[must_use]
pub fn fallback_rust_trait_methods(path: &str) -> &'static [&'static str] {
    match path {
        "std::io::Read" => &[
            "read",
            "read_to_end",
            "read_to_string",
            "read_exact",
            "read_buf",
            "read_buf_exact",
            "bytes",
            "chain",
            "take",
        ],
        "std::io::Write" => &["write", "write_all", "write_fmt", "flush"],
        "std::io::Seek" => &["seek", "rewind", "stream_position", "seek_relative"],
        "byteorder::ReadBytesExt" => &[
            "read_u8",
            "read_i8",
            "read_u16",
            "read_i16",
            "read_u32",
            "read_i32",
            "read_u64",
            "read_i64",
            "read_u128",
            "read_i128",
            "read_f32",
            "read_f64",
        ],
        "byteorder::WriteBytesExt" => &[
            "write_u8",
            "write_i8",
            "write_u16",
            "write_i16",
            "write_u32",
            "write_i32",
            "write_u64",
            "write_i64",
            "write_u128",
            "write_i128",
            "write_f32",
            "write_f64",
        ],
        "sha2::Digest" | "sha3::Digest" | "blake2::Digest" | "md5::Digest" | "sha1::Digest" => &[
            "new",
            "new_with_prefix",
            "update",
            "chain_update",
            "finalize",
            "finalize_into",
            "finalize_reset",
            "reset",
            "output_size",
            "digest",
        ],
        "blake2::digest::XofReader" | "sha3::digest::XofReader" => &["read"],
        "std::os::unix::fs::MetadataExt" => &[
            "dev", "ino", "mode", "nlink", "uid", "gid", "rdev", "size", "atime", "mtime", "ctime", "blksize", "blocks",
        ],
        _ => &[],
    }
}
