//! Typed vocabulary for the first checked C ABI binding slice.
//!
//! The parser and vocabulary desugarer stay generic. This module is the single language-level source for the C
//! spellings that the typechecker recognizes after vocabulary lowering. Resource ownership, output slots, native
//! artifact classes, and shim policy deliberately belong to later RFC 116 slices.

/// Stable fields accepted by an `@c.binding` descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingArgumentId {
    /// Literal C header spelling.
    Header,
    /// Logical native link capability.
    Link,
}

/// Stable declarative member kinds accepted by the checked C foundation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingMemberId {
    /// A nominal opaque C resource and its associated release operation.
    Resource,
    /// A non-executable raw C function declaration.
    Symbol,
    /// A target-verified C enum carrier.
    Enum,
    /// A target-verified by-value C structure.
    Struct,
}

/// Stable fields accepted by a plain C structure declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlainStructArgumentId {
    /// Exact native C struct tag or typedef spelling.
    Native,
}

/// Stable fields accepted by a C symbol declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolArgumentId {
    /// Exact native C symbol spelling.
    Native,
    /// Explicit pointer-to-length associations for checked byte spans.
    Bounds,
}

/// Stable fields accepted by an opaque C resource declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceArgumentId {
    /// Exact native opaque type spelling.
    Native,
    /// Binding-local symbol that releases one owned resource.
    Release,
}

/// Nested declarative member keyword for one raw C symbol result outcome.
pub const SYMBOL_OUTCOME_KEYWORD: &str = "outcome";

/// Prefix of compiler-internal nominal identities used for C output-slot handles.
///
/// This is not source vocabulary. It lets the frontend retain a binding-qualified slot identity without depending on
/// backend Rust name generation; lowering maps each slot to a private generated carrier later.
pub const OUTPUT_SLOT_TYPE_PREFIX: &str = "__incan_c_output_slot";

/// Compiler-internal nominal identity for a terminator-checked temporary C string.
///
/// This is not source vocabulary. `c.cstr(value)` returns it only after validating that `value` has no interior NUL;
/// `as_const_ptr()` may then expose its checked pointer inside an `unsafe:` region.
pub const C_STRING_TYPE_ID: &str = "__incan_c_cstring";

/// Compiler-internal nominal identity for an immutable checked byte span.
///
/// This is not source vocabulary. `c.bytes_span(value)` moves one owned Incan `bytes` value into a private carrier
/// whose pointer and length can only be extracted as a declared pair inside an `unsafe:` bridge.
pub const C_BYTES_SPAN_TYPE_ID: &str = "__incan_c_bytes_span";

/// Compiler-internal nominal identity for a mutable checked caller-owned byte buffer.
///
/// The carrier holds initialized owned bytes, so foreign code may write at most the checked buffer length without
/// exposing uninitialized memory or a general mutable-pointer API.
pub const C_MUTABLE_BYTES_SPAN_TYPE_ID: &str = "__incan_c_mutable_bytes_span";

/// Compiler-internal nominal identity for an immutable checked `f32` span.
///
/// This moves one owned Incan `list[f32]` allocation into the closed C bridge, where its pointer and element count
/// can only be extracted as one declared pair inside an `unsafe:` acknowledgement.
pub const C_F32_SPAN_TYPE_ID: &str = "__incan_c_f32_span";

/// Compiler-internal nominal identity for a mutable checked caller-owned `f32` span.
pub const C_MUTABLE_F32_SPAN_TYPE_ID: &str = "__incan_c_mutable_f32_span";

/// Compiler-internal nominal identity for a returned `const char *` view.
///
/// A scoped view is intentionally distinct from a checked C-string input pointer. It may only become Incan-owned
/// text through the bounded `copy_utf8(max_bytes=...)` operation inside the originating unsafe region.
pub const SCOPED_C_STRING_VIEW_TYPE_ID: &str = "__incan_c_scoped_string_view";

/// Generated helper that validates Incan text before it becomes a C string temporary.
///
/// The helper remains compiler-private. Its spelling is shared only so typed lowering and Rust emission cannot drift.
pub const C_STRING_CONSTRUCTOR_RUST_NAME: &str = "__incan_checked_c_cstr";

/// Generated helper that copies a returned C string view only after an explicit bounded terminator scan and UTF-8
/// validation.
pub const SCOPED_C_STRING_COPY_UTF8_RUST_NAME: &str = "__incan_checked_c_copy_utf8";

/// Generated helper that validates a foreign written element count before returning a caller-owned typed allocation.
pub const MUTABLE_SPAN_FINISH_RUST_NAME: &str = "__incan_checked_c_finish_span";

/// Prefix of compiler-internal nominal identities for checked C pointers.
///
/// Pointer identities carry mutability and the complete pointed-to C contract. They never represent ordinary integer
/// addresses and are only produced by compiler-authorized bridge views.
pub const POINTER_TYPE_PREFIX: &str = "__incan_c_pointer";

/// Return the compiler-internal nominal identity for one checked C pointer contract.
pub fn pointer_type_identity(mutable: bool, pointee: &str) -> String {
    let mut identity = String::from(POINTER_TYPE_PREFIX);
    identity.push_str(if mutable { "::mut::" } else { "::const::" });
    identity.push_str(pointee);
    identity
}

/// Return the compiler-internal nominal identity for one checked C output slot.
///
/// The source offsets distinguish otherwise-identical slots in nested scopes. They are not an exposed ABI detail:
/// lowering maps every instance of one parameter contract to its private generated Rust carrier.
pub fn output_slot_type_identity(
    binding: &str,
    symbol: &str,
    parameter: &str,
    constructor_start: usize,
    constructor_end: usize,
) -> String {
    format!("{OUTPUT_SLOT_TYPE_PREFIX}::{binding}::{symbol}::{parameter}::{constructor_start}_{constructor_end}")
}

/// Split one compiler-internal C output-slot identity into its checked source components.
pub fn parse_output_slot_type_identity(value: &str) -> Option<(&str, &str, &str)> {
    let mut parts = value.split("::");
    if parts.next()? != OUTPUT_SLOT_TYPE_PREFIX {
        return None;
    }
    let binding = parts.next()?;
    let symbol = parts.next()?;
    let parameter = parts.next()?;
    let instance = parts.next()?;
    (!instance.is_empty() && parts.next().is_none()).then_some((binding, symbol, parameter))
}

/// Stable data fields accepted by a C symbol outcome declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolOutcomeArgumentId {
    /// Output slots initialized by this raw result.
    Initializes,
    /// In/out slots updated by this raw result.
    Updates,
    /// In/out slots invalidated by this raw result.
    Invalidates,
}

/// Stable resource and compiler-managed output wrappers accepted by C binding signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceTypeConstructorId {
    /// A non-copyable resource whose declared release operation is owed by the caller.
    Owned,
    /// A resource borrowed for the duration of one raw call.
    Borrowed,
    /// A resource borrowed mutably for the duration of one raw call.
    BorrowedMut,
    /// A foreign output position whose initialization depends on a declared outcome.
    Out,
    /// An initialized foreign position that may be updated by a declared outcome.
    InOut,
}

/// Stable exact scalar representations accepted by the checked C foundation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarTypeId {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    /// A target-verified Clang `__int128` extension; never a portable-C claim.
    I128,
    /// A target-verified Clang `unsigned __int128` extension; never a portable-C claim.
    U128,
    F32,
    F64,
    Size,
    CChar,
    CInt,
}

/// Resolve an exact C scalar spelling to its stable identifier.
pub fn scalar_type_from_str(name: &str) -> Option<ScalarTypeId> {
    match name {
        "c.i8" => Some(ScalarTypeId::I8),
        "c.u8" => Some(ScalarTypeId::U8),
        "c.i16" => Some(ScalarTypeId::I16),
        "c.u16" => Some(ScalarTypeId::U16),
        "c.i32" => Some(ScalarTypeId::I32),
        "c.u32" => Some(ScalarTypeId::U32),
        "c.i64" => Some(ScalarTypeId::I64),
        "c.u64" => Some(ScalarTypeId::U64),
        "c.i128" => Some(ScalarTypeId::I128),
        "c.u128" => Some(ScalarTypeId::U128),
        "c.f32" => Some(ScalarTypeId::F32),
        "c.f64" => Some(ScalarTypeId::F64),
        "c.Size" => Some(ScalarTypeId::Size),
        "c.c_char" => Some(ScalarTypeId::CChar),
        "c.c_int" => Some(ScalarTypeId::CInt),
        _ => None,
    }
}

/// Return the canonical source spelling for an exact C scalar representation.
pub const fn scalar_type_as_str(id: ScalarTypeId) -> &'static str {
    match id {
        ScalarTypeId::I8 => "c.i8",
        ScalarTypeId::U8 => "c.u8",
        ScalarTypeId::I16 => "c.i16",
        ScalarTypeId::U16 => "c.u16",
        ScalarTypeId::I32 => "c.i32",
        ScalarTypeId::U32 => "c.u32",
        ScalarTypeId::I64 => "c.i64",
        ScalarTypeId::U64 => "c.u64",
        ScalarTypeId::I128 => "c.i128",
        ScalarTypeId::U128 => "c.u128",
        ScalarTypeId::F32 => "c.f32",
        ScalarTypeId::F64 => "c.f64",
        ScalarTypeId::Size => "c.Size",
        ScalarTypeId::CChar => "c.c_char",
        ScalarTypeId::CInt => "c.c_int",
    }
}

/// Return the exact ordinary Incan numeric representation for a portable or target-verified fixed C scalar.
///
/// `c.c_char` and `c.c_int` intentionally return `None`: their width/signedness is target-defined and cannot be
/// represented as a compile-time Incan numeric identity before the selected target receipt verifies it.
pub const fn scalar_numeric_type(id: ScalarTypeId) -> Option<crate::lang::types::numerics::NumericTypeId> {
    use crate::lang::types::numerics::NumericTypeId;

    match id {
        ScalarTypeId::I8 => Some(NumericTypeId::I8),
        ScalarTypeId::U8 => Some(NumericTypeId::U8),
        ScalarTypeId::I16 => Some(NumericTypeId::I16),
        ScalarTypeId::U16 => Some(NumericTypeId::U16),
        ScalarTypeId::I32 => Some(NumericTypeId::I32),
        ScalarTypeId::U32 => Some(NumericTypeId::U32),
        ScalarTypeId::I64 => Some(NumericTypeId::I64),
        ScalarTypeId::U64 => Some(NumericTypeId::U64),
        ScalarTypeId::I128 => Some(NumericTypeId::I128),
        ScalarTypeId::U128 => Some(NumericTypeId::U128),
        ScalarTypeId::F32 => Some(NumericTypeId::F32),
        ScalarTypeId::F64 => Some(NumericTypeId::F64),
        ScalarTypeId::Size => Some(NumericTypeId::USize),
        ScalarTypeId::CChar | ScalarTypeId::CInt => None,
    }
}

/// Stable pointer constructors accepted by the checked C foundation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerConstructorId {
    /// A read-only C pointer.
    ConstPtr,
    /// A mutable C pointer.
    MutPtr,
}

/// Return the canonical source spelling for an admitted pointer constructor.
pub const fn pointer_constructor_as_str(id: PointerConstructorId) -> &'static str {
    match id {
        PointerConstructorId::ConstPtr => "c.ConstPtr",
        PointerConstructorId::MutPtr => "c.MutPtr",
    }
}

/// Source path that must be imported for C binding vocabulary and descriptor use.
pub const INTEROP_NAMESPACE_PATH: &[&str] = &["std", "interop", "c"];

/// Ordinary marker base that establishes a class as a binding declaration.
pub const BINDING_DECLARATION_BASE: &str = "BindingDeclaration";

/// Canonical Incan spelling for a C `void` return.
pub const VOID_TYPE_SPELLING: &str = "None";

/// Return whether an Incan type spelling represents a C `void` return.
pub fn is_void_type_spelling(name: &str) -> bool {
    name == VOID_TYPE_SPELLING
}

/// Resolve a C binding descriptor field to its stable identifier.
pub fn binding_argument_from_str(name: &str) -> Option<BindingArgumentId> {
    match name {
        "header" => Some(BindingArgumentId::Header),
        "link" => Some(BindingArgumentId::Link),
        _ => None,
    }
}

/// Resolve a C binding member spelling to its stable identifier.
pub fn binding_member_from_str(name: &str) -> Option<BindingMemberId> {
    match name {
        "resource" => Some(BindingMemberId::Resource),
        "symbol" => Some(BindingMemberId::Symbol),
        "enum" => Some(BindingMemberId::Enum),
        "struct" => Some(BindingMemberId::Struct),
        _ => None,
    }
}

/// Resolve an opaque C resource field to its stable identifier.
pub fn resource_argument_from_str(name: &str) -> Option<ResourceArgumentId> {
    match name {
        "native" => Some(ResourceArgumentId::Native),
        "release" => Some(ResourceArgumentId::Release),
        _ => None,
    }
}

/// Resolve a C symbol outcome field to its stable identifier.
pub fn symbol_outcome_argument_from_str(name: &str) -> Option<SymbolOutcomeArgumentId> {
    match name {
        "initializes" => Some(SymbolOutcomeArgumentId::Initializes),
        "updates" => Some(SymbolOutcomeArgumentId::Updates),
        "invalidates" => Some(SymbolOutcomeArgumentId::Invalidates),
        _ => None,
    }
}

/// Return the canonical spelling for a C resource or output wrapper.
pub const fn resource_type_constructor_as_str(id: ResourceTypeConstructorId) -> &'static str {
    match id {
        ResourceTypeConstructorId::Owned => "c.Owned",
        ResourceTypeConstructorId::Borrowed => "c.Borrowed",
        ResourceTypeConstructorId::BorrowedMut => "c.BorrowedMut",
        ResourceTypeConstructorId::Out => "c.Out",
        ResourceTypeConstructorId::InOut => "c.InOut",
    }
}

/// Resolve a plain C structure field to its stable identifier.
pub fn plain_struct_argument_from_str(name: &str) -> Option<PlainStructArgumentId> {
    (name == "native").then_some(PlainStructArgumentId::Native)
}

/// Resolve a raw C symbol field to its stable identifier.
pub fn symbol_argument_from_str(name: &str) -> Option<SymbolArgumentId> {
    match name {
        "native" => Some(SymbolArgumentId::Native),
        "bounds" => Some(SymbolArgumentId::Bounds),
        _ => None,
    }
}

/// Stable C link capability names admitted by the descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkCapabilityId {
    /// A named system capability that a later target resolver must select explicitly.
    SystemLibrary,
    /// An Apple-style framework selected by the target-native Oven interop plan.
    Framework,
}

/// Return the stable source-vocabulary spelling for one checked native-link capability.
pub fn link_capability_as_str(capability: LinkCapabilityId) -> &'static str {
    match capability {
        LinkCapabilityId::SystemLibrary => "system_library",
        LinkCapabilityId::Framework => "framework",
    }
}

/// Resolve a C namespace member name to a supported link capability.
pub fn link_capability_from_str(name: &str) -> Option<LinkCapabilityId> {
    match name {
        "system_library" => Some(LinkCapabilityId::SystemLibrary),
        "framework" => Some(LinkCapabilityId::Framework),
        _ => None,
    }
}

/// Return whether path segments name the imported C interop namespace.
pub fn is_interop_namespace_path<'a>(segments: impl IntoIterator<Item = &'a str>) -> bool {
    segments.into_iter().eq(INTEROP_NAMESPACE_PATH.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::{
        BINDING_DECLARATION_BASE, BindingArgumentId, BindingMemberId, C_BYTES_SPAN_TYPE_ID,
        C_MUTABLE_BYTES_SPAN_TYPE_ID, C_STRING_TYPE_ID, LinkCapabilityId, POINTER_TYPE_PREFIX, PlainStructArgumentId,
        PointerConstructorId, ResourceArgumentId, ResourceTypeConstructorId, SYMBOL_OUTCOME_KEYWORD, ScalarTypeId,
        SymbolArgumentId, SymbolOutcomeArgumentId, binding_argument_from_str, binding_member_from_str,
        is_interop_namespace_path, is_void_type_spelling, link_capability_as_str, link_capability_from_str,
        plain_struct_argument_from_str, pointer_constructor_as_str, pointer_type_identity, resource_argument_from_str,
        resource_type_constructor_as_str, scalar_numeric_type, scalar_type_as_str, scalar_type_from_str,
        symbol_argument_from_str, symbol_outcome_argument_from_str,
    };
    use crate::lang::types::numerics::NumericTypeId;

    #[test]
    fn checked_c_foundation_vocabulary_is_canonical() {
        assert_eq!(binding_argument_from_str("header"), Some(BindingArgumentId::Header));
        assert_eq!(binding_member_from_str("resource"), Some(BindingMemberId::Resource));
        assert_eq!(binding_member_from_str("symbol"), Some(BindingMemberId::Symbol));
        assert_eq!(binding_member_from_str("struct"), Some(BindingMemberId::Struct));
        assert_eq!(
            plain_struct_argument_from_str("native"),
            Some(PlainStructArgumentId::Native)
        );
        assert_eq!(symbol_argument_from_str("native"), Some(SymbolArgumentId::Native));
        assert_eq!(symbol_argument_from_str("bounds"), Some(SymbolArgumentId::Bounds));
        assert_eq!(resource_argument_from_str("native"), Some(ResourceArgumentId::Native));
        assert_eq!(resource_argument_from_str("release"), Some(ResourceArgumentId::Release));
        assert_eq!(SYMBOL_OUTCOME_KEYWORD, "outcome");
        assert_eq!(
            symbol_outcome_argument_from_str("initializes"),
            Some(SymbolOutcomeArgumentId::Initializes)
        );
        assert_eq!(
            resource_type_constructor_as_str(ResourceTypeConstructorId::Owned),
            "c.Owned"
        );
        assert_eq!(
            resource_type_constructor_as_str(ResourceTypeConstructorId::InOut),
            "c.InOut"
        );
        assert_eq!(scalar_type_from_str("c.i32"), Some(ScalarTypeId::I32));
        assert_eq!(scalar_type_from_str("c.i128"), Some(ScalarTypeId::I128));
        assert_eq!(scalar_type_from_str("c.f64"), Some(ScalarTypeId::F64));
        assert_eq!(scalar_type_as_str(ScalarTypeId::CInt), "c.c_int");
        assert_eq!(scalar_type_as_str(ScalarTypeId::U128), "c.u128");
        assert_eq!(scalar_numeric_type(ScalarTypeId::I16), Some(NumericTypeId::I16));
        assert_eq!(scalar_numeric_type(ScalarTypeId::Size), Some(NumericTypeId::USize));
        assert_eq!(scalar_numeric_type(ScalarTypeId::F64), Some(NumericTypeId::F64));
        assert_eq!(scalar_numeric_type(ScalarTypeId::CInt), None);
        assert_eq!(pointer_constructor_as_str(PointerConstructorId::ConstPtr), "c.ConstPtr");
        assert_eq!(C_STRING_TYPE_ID, "__incan_c_cstring");
        assert_eq!(C_BYTES_SPAN_TYPE_ID, "__incan_c_bytes_span");
        assert_eq!(C_MUTABLE_BYTES_SPAN_TYPE_ID, "__incan_c_mutable_bytes_span");
        assert_eq!(POINTER_TYPE_PREFIX, "__incan_c_pointer");
        assert_eq!(
            pointer_type_identity(false, "c.c_char"),
            "__incan_c_pointer::const::c.c_char"
        );
        assert_eq!(
            link_capability_from_str("system_library"),
            Some(LinkCapabilityId::SystemLibrary)
        );
        assert_eq!(link_capability_from_str("framework"), Some(LinkCapabilityId::Framework));
        assert_eq!(
            link_capability_as_str(LinkCapabilityId::SystemLibrary),
            "system_library"
        );
        assert_eq!(link_capability_as_str(LinkCapabilityId::Framework), "framework");
        assert!(is_interop_namespace_path(["std", "interop", "c"]));
        assert_eq!(BINDING_DECLARATION_BASE, "BindingDeclaration");
        assert!(is_void_type_spelling("None"));
    }
}
