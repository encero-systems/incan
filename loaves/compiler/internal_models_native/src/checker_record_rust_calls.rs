//! Validate checked Rust-call record primitives without reconstructing native compiler objects.

use crate::checker_record_models::{CanonicalField, CodecFault};
use crate::checker_record_types::{canonical_decimal, checked_type_end, parameters_end};
use CodecFault::Malformed;

/// Return the complete admitted Rust artifact length or refuse an unknown selector.
pub fn rust_artifact_end(field: &CanonicalField) -> Result<usize, CodecFault> {
    if field.node.is_empty() {
        return Err(Malformed);
    }
    match field.selector.as_str() {
        "checked_constants" => {
            if field.values.len() != 1 || field.values[0].is_empty() {
                return Err(Malformed);
            }
            Ok(1)
        }
        "checked_calls" => RustCallFields::validate(&field.values, 1),
        "iteration_hooks" => RustCallFields::validate(&field.values, 2),
        _ => RustCallFields::value(field),
    }
}

/// Advance through bounded structural Rust-call fields while retaining one shared cursor.
struct RustCallFields<'a> {
    values: &'a [String],
    offset: usize,
}

const COERCIONS: [&str; 11] = [
    "exact",
    "lossless",
    "builtin-borrow",
    "lossy",
    "rusttype-unwrap",
    "rusttype-interop",
    "box-payload",
    "borrow",
    "borrow-mut",
    "trait-borrow",
    "trait-borrow-mut",
];

impl<'a> RustCallFields<'a> {
    /// Validate one checked field carrier, bound item path or argument conversion.
    fn value(field: &'a CanonicalField) -> Result<usize, CodecFault> {
        let mut reader = RustCallFields {
            values: &field.values,
            offset: 0,
        };
        match field.selector.as_str() {
            "value_item_paths" => {
                reader.handle()?;
                reader.handle()?;
            }
            "field_values" => {
                reader.checked_type()?;
                if reader.tag(&["none", "some"])? == "some" {
                    reader.handle()?;
                }
            }
            "arg_coercions" => {
                reader.handle()?;
                reader.tag(&COERCIONS)?;
                reader.checked_type()?;
            }
            selector => return Err(CodecFault::Unsupported(selector.to_owned())),
        }
        Ok(reader.offset)
    }

    /// Validate a fixed number of calls and return the consumed field count.
    fn validate(values: &'a [String], calls: usize) -> Result<usize, CodecFault> {
        let mut reader = RustCallFields { values, offset: 0 };
        for _ in 0..calls {
            reader.call(0)?;
        }
        Ok(reader.offset)
    }

    /// Read one checked Rust call and its recursively adjusted receivers.
    fn call(&mut self, depth: i64) -> Result<(), CodecFault> {
        if depth > 16 {
            return Err(CodecFault::Limit);
        }
        if self.tag(&["none", "some"])? == "some" {
            self.signature()?;
        }
        self.strings()?;
        for _ in 0..self.count()? {
            self.strings()?;
        }
        for _ in 0..self.count()? {
            self.call(depth + 1)?;
        }
        self.handle()?;
        for _ in 0..self.count()? {
            self.checked_type()?;
        }
        self.parameters()?;
        self.checked_type()?;
        if self.tag(&["none", "some"])? == "some" {
            self.handle()?;
        }
        self.boolean()
    }

    /// Read native parameters, result, arguments and canonical identity bindings.
    fn signature(&mut self) -> Result<(), CodecFault> {
        for _ in 0..self.count()? {
            self.native_type(0)?;
        }
        self.native_type(0)?;
        for _ in 0..self.count()? {
            self.native_type(0)?;
        }
        let mut previous = "";
        for _ in 0..self.count()? {
            let name = self.handle()?;
            if name <= previous {
                return Err(Malformed);
            }
            previous = name;
            self.handle()?;
        }
        Ok(())
    }

    /// Read one closed native storage type using the frontend codec's structural tags.
    fn native_type(&mut self, depth: i64) -> Result<(), CodecFault> {
        if depth > 32 {
            return Err(CodecFault::Limit);
        }
        let tag = self.token()?;
        match tag {
            "never" | "self-type" | "infer" | "unknown" | "native-c-char" => Ok(()),
            "primitive" => {
                let primitive = self.tag(&[
                    "int",
                    "float",
                    "numeric",
                    "bool",
                    "str",
                    "bytes",
                    "frozen-str",
                    "frozen-bytes",
                    "unit",
                ])?;
                if primitive == "numeric" {
                    self.handle()?;
                }
                Ok(())
            }
            "named" | "type-var" | "rust-path" | "unsupported" | "trait" => self.handle().map(|_| ()),
            "generic" => {
                self.handle()?;
                for _ in 0..self.count()? {
                    self.native_type(depth + 1)?;
                }
                Ok(())
            }
            "decimal" => {
                self.number()?;
                self.number()?;
                Ok(())
            }
            "function" => {
                for _ in 0..self.count()? {
                    self.native_parameter(depth + 1)?;
                }
                self.native_type(depth + 1)
            }
            "type-token" | "ref" | "ref-mut" => self.native_type(depth + 1),
            "tuple" => {
                for _ in 0..self.count()? {
                    self.native_type(depth + 1)?;
                }
                Ok(())
            }
            "native-builtin" => self.tag(&["validation-error"]).map(|_| ()),
            "native-array" => {
                self.number()?;
                self.native_type(depth + 1)
            }
            "native-trait-object" => {
                self.handle()?;
                self.boolean()?;
                self.boolean()
            }
            "native-pointer" => {
                self.boolean()?;
                self.native_type(depth + 1)
            }
            _ => Err(CodecFault::Unsupported(format!("native-type:{tag}"))),
        }
    }

    /// Read one native callable parameter and its storage type.
    fn native_parameter(&mut self, depth: i64) -> Result<(), CodecFault> {
        if self.tag(&["none", "some"])? == "some" {
            self.handle()?;
        }
        self.tag(&["normal", "rest", "keywords"])?;
        for _ in 0..4 {
            self.boolean()?;
        }
        self.tag(&["inferred", "shared", "exclusive", "owned", "inferred-shared"])?;
        self.native_type(depth)
    }

    /// Advance through one ordinary source type with the shared canonical grammar.
    fn checked_type(&mut self) -> Result<(), CodecFault> {
        self.offset = checked_type_end(self.values, self.offset, 0)?;
        Ok(())
    }

    /// Advance through ordinary callable parameters with the shared canonical grammar.
    fn parameters(&mut self) -> Result<(), CodecFault> {
        self.offset = parameters_end(self.values, self.offset, 0)?;
        Ok(())
    }

    /// Read one count-prefixed string list.
    fn strings(&mut self) -> Result<(), CodecFault> {
        for _ in 0..self.count()? {
            self.handle()?;
        }
        Ok(())
    }

    /// Read one canonical Boolean token.
    fn boolean(&mut self) -> Result<(), CodecFault> {
        self.tag(&["false", "true"]).map(|_| ())
    }

    /// Read one token admitted by a closed set.
    fn tag(&mut self, allowed: &[&str]) -> Result<&'a str, CodecFault> {
        let value = self.token()?;
        if allowed.contains(&value) {
            Ok(value)
        } else {
            Err(Malformed)
        }
    }

    /// Read one nonempty opaque handle.
    fn handle(&mut self) -> Result<&'a str, CodecFault> {
        let value = self.token()?;
        if value.is_empty() { Err(Malformed) } else { Ok(value) }
    }

    /// Read one canonical decimal field.
    fn number(&mut self) -> Result<i64, CodecFault> {
        canonical_decimal(self.token()?)
    }

    /// Read one collection count bounded by the remaining field inventory.
    fn count(&mut self) -> Result<i64, CodecFault> {
        let value = self.number()?;
        if value > (self.values.len() - self.offset) as i64 {
            return Err(Malformed);
        }
        Ok(value)
    }

    /// Read the next field without permitting an out-of-bounds cursor.
    fn token(&mut self) -> Result<&'a str, CodecFault> {
        let value = self.values.get(self.offset).ok_or(Malformed)?;
        self.offset += 1;
        Ok(value)
    }
}
