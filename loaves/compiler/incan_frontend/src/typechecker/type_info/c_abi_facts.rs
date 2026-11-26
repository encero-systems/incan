//! Checked C ABI interop artifacts, C binding descriptors, and their descriptor identity.

use super::*;

/// Checked source-level C ABI binding contracts.
#[derive(Debug, Default, Clone)]
pub struct CAbiInteropArtifacts {
    /// Descriptors accepted by the selected target's Clang verifier before executable projection.
    pub verified_bindings: BTreeSet<String>,
    /// Span-specific operations accepted by the C bridge's constructor, lifetime and output-state checks.
    pub bridge_calls: Vec<CAbiBridgeCall>,
    /// Binding descriptor keyed by the ordinary lowered class name.
    pub bindings: HashMap<String, CBindingDescriptor>,
    /// Direct binding calls admitted through an explicit `unsafe:` acknowledgment.
    pub raw_calls: Vec<CBindingRawCall>,
    /// Compiler-proven ordinary function calls made by a named callable while typechecking.
    ///
    /// These are retained only for the C-ABI bridge/facade projection; general codegraph targets remain in
    /// [`ExpressionArtifacts::source_targets`].
    pub function_calls: Vec<CBindingFunctionCall>,
    /// Public facade to private raw-bridge edges derived from checked function targets and raw-call owners.
    pub facades: Vec<CBindingFacade>,
    /// Compiler-managed source slots bound to checked `Out` or `InOut` call parameters.
    pub output_slots: Vec<CAbiOutputSlot>,
    /// Source accesses to target-verified C enum constants.
    pub enum_accesses: Vec<CBindingEnumAccess>,
    /// Folded C enum values keyed by `(binding, enum, variant)` after Clang verification.
    pub enum_values: HashMap<(String, String, String), i64>,
    /// For each verified `c.loaf_library` binding, the link name of the Clove library its unit describes.
    pub clove_libraries: HashMap<String, String>,
}

/// One checked C bridge call, retained before its source method spelling is erased.
#[derive(Debug, Clone)]
pub struct CAbiBridgeCall {
    /// Exact compiler-managed slot selected by state checking, when this operation consumes one.
    pub slot_identity: Option<String>,
    /// Full source call expression range.
    pub span: Span,
    /// The bridge operation selected by checking.
    pub operation: incan_semantics_core::body_ir::CheckedBridgeOperation,
}

/// Exact scalar representation and mutability of one compiler-owned checked span carrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CAbiSpanKind {
    /// The exact checked C scalar representation of every element in the carrier.
    pub element: ScalarTypeId,
    /// Whether this is an immutable input view or a caller-owned mutable output buffer.
    pub mutable: bool,
}

/// Compiler-known callable body that owns one direct native call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingRawCallOwner {
    /// Source-visible callable name.
    pub name: String,
    /// Source visibility of the owning callable.
    pub visibility: Visibility,
    /// Full source range of the owning declaration.
    pub declaration_span: Span,
}

/// One direct source call to a checked C binding symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingRawCall {
    /// Source range for the full call expression.
    pub span: Span,
    /// Compiler-known callable body that owns this direct native call, when the call occurs in a named function.
    pub owner: Option<CBindingRawCallOwner>,
    /// Lowered binding class that owns the native symbol.
    pub binding: String,
    /// Binding-local symbol selected by the call.
    pub symbol: String,
}

/// One ordinary function call whose target was resolved by the typechecker while a callable body was active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingFunctionCall {
    /// Full source range of the ordinary function call.
    pub span: Span,
    /// Callable containing the call.
    pub caller: CBindingRawCallOwner,
    /// Compiler-proven declaration target selected by the call.
    pub target: SourceTargetInfo,
}

/// One explicit public-facade to private-raw-bridge relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingFacade {
    /// Public callable that invokes the bridge through a checked ordinary function target.
    pub facade: CBindingRawCallOwner,
    /// Private callable that owns one or more direct checked raw calls.
    pub bridge: CBindingRawCallOwner,
    /// Source range of the ordinary facade-to-bridge call.
    pub call_span: Span,
}

/// One compiler-managed local storage slot used by a checked C output parameter.
///
/// The source handle is created with ordinary `c.out[...]()` or `c.inout(...)` syntax. This artifact is recorded
/// only after a checked raw call proves which binding symbol and exact C carrier own the storage contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CAbiOutputSlot {
    /// Closure frame that owns the compiler-managed allocation.
    pub closure_depth: usize,
    /// Full source range of the slot constructor call.
    pub constructor_span: Span,
    /// Compiler-internal nominal identity for this particular source slot instance.
    pub identity: String,
    /// Local source binding that owns the slot handle.
    pub local_name: String,
    /// C binding class selected by the raw call.
    pub binding: String,
    /// Binding-local native symbol that receives this slot.
    pub symbol: String,
    /// Native parameter name represented by this slot.
    pub parameter: String,
    /// Whether the slot is fresh output storage or caller-initialized storage.
    pub mode: COutputMode,
    /// Exact C value contract carried by the slot, without its outer output wrapper.
    pub value: CBindingType,
}

/// One source access to a target-verified C enum constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingEnumAccess {
    /// Source range for the complete `Binding.Enum.Variant` expression.
    pub span: Span,
    /// Lowered binding class that owns the enum declaration.
    pub binding: String,
    /// Binding-local enum declaration.
    pub enumeration: String,
    /// Source variant name.
    pub variant: String,
}

/// One checked C binding declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingDescriptor {
    /// Source location of the binding declaration, used for target-verifier diagnostics.
    pub span: Span,
    /// Ordinary class name visible to Incan source.
    pub class_name: String,
    /// Header path supplied by the binding declaration.
    pub header: String,
    /// Logical system-library capability selected by the declaration.
    pub system_library: String,
    /// Exact native link shape selected by the checked declaration.
    pub link_capability: LinkCapabilityId,
    /// For `c.loaf_library`, the Clove configuration the binding links (`system_library` names the Clove's Loaf);
    /// empty for every other link shape.
    pub configuration: String,
    /// Nominal opaque resources and their binding-local release associations.
    pub resources: Vec<CBindingResource>,
    /// Raw C functions declared by the binding.
    pub symbols: Vec<CBindingSymbol>,
    /// C enum carriers whose values are target-verified constants.
    pub enums: Vec<CBindingEnum>,
    /// Plain C structures whose layout is target-verified.
    pub structs: Vec<CBindingStruct>,
}

/// One raw C function declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingSymbol {
    /// Source member name.
    pub name: String,
    /// Native linker symbol.
    pub native: String,
    /// Explicit parameter contracts in source order.
    pub parameters: Vec<CBindingParameter>,
    /// Explicit return contract.
    pub return_type: CBindingType,
    /// Explicit pointer-to-length contracts admitted for checked typed spans.
    ///
    /// Each record names a raw checked scalar-pointer parameter and the exact `c.Size` parameter that bounds it. This
    /// lives with the descriptor so all later stages consume one checked association rather than recovering one from
    /// argument names.
    pub buffers: Vec<CBindingBuffer>,
    /// Raw outcomes that establish output-slot state after this call.
    pub outcomes: Vec<CBindingOutcome>,
}

/// One declared typed-pointer and C-size association on a raw C symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingBuffer {
    /// Raw typed-pointer parameter name.
    pub pointer_parameter: String,
    /// Exact `c.Size` parameter that bounds this pointer for the call.
    pub length_parameter: String,
    /// Exact scalar representation validated for the pointer parameter.
    pub element: ScalarTypeId,
}

/// One raw C function parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingParameter {
    /// Parameter name.
    pub name: String,
    /// Exact checked C type.
    pub ty: CBindingType,
}

/// Initial checked C type vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CBindingType {
    /// A fixed scalar representation.
    Scalar(ScalarTypeId),
    /// A pointer to an admitted C type.
    Pointer { mutable: bool, pointee: Box<CBindingType> },
    /// A plain by-value C structure named by its binding member.
    Struct(String),
    /// A nominal resource with one call-site ownership mode.
    Resource {
        /// Whether the call borrows or consumes the resource.
        access: CResourceAccess,
        /// Binding-local resource declaration name.
        resource: String,
    },
    /// Compiler-managed foreign output storage.
    Output {
        /// Whether the storage is uninitialized output or initialized in/out state.
        mode: COutputMode,
        /// Checked native value held by the slot.
        value: Box<CBindingType>,
    },
    /// A nullable owned resource factory result.
    Nullable(Box<CBindingType>),
    /// `None`/unit for a C `void` return.
    Void,
}

/// Ownership mode declared for one opaque-resource C parameter or result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CResourceAccess {
    /// Transfer the release obligation to or from the call.
    Owned,
    /// Shared access confined to the raw call.
    Borrowed,
    /// Exclusive mutable access confined to the raw call.
    BorrowedMut,
}

/// Initialization contract for one compiler-managed C output position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum COutputMode {
    /// Foreign code initializes the position on a declared raw outcome.
    Out,
    /// Foreign code receives initialized storage and may update it.
    InOut,
}

/// One declared opaque C resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingResource {
    /// Source span of the resource declaration.
    pub span: Span,
    /// Incan-local nominal resource name.
    pub name: String,
    /// Exact C opaque type spelling.
    pub native: String,
    /// Binding-local symbol that releases one owned resource.
    pub release: String,
}

/// One raw result value that changes declared output-slot state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingOutcome {
    /// Binding-local enum and variant spelling, such as `ResultCode.OK`.
    pub result: String,
    /// `c.Out[...]` parameters made readable on this path.
    pub initializes: Vec<String>,
    /// `c.InOut[...]` parameters updated on this path.
    pub updates: Vec<String>,
    /// `c.InOut[...]` parameters invalidated on this path.
    pub invalidates: Vec<String>,
}

/// One C enum declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingEnum {
    /// Source enum name.
    pub name: String,
    /// Shared scalar carrier representation.
    pub carrier: ScalarTypeId,
    /// Native constant facts in source order.
    pub variants: Vec<CBindingEnumVariant>,
}

/// One target-verified C enum constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingEnumVariant {
    /// Source variant name.
    pub name: String,
    /// Native constant spelling such as `SQLITE_OK`.
    pub native: String,
}

/// One plain C structure declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingStruct {
    /// Source structure name.
    pub name: String,
    /// Native C tag or typedef spelling.
    pub native: String,
    /// Fields in declared C layout order.
    pub fields: Vec<CBindingStructField>,
}

/// One plain C structure field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CBindingStructField {
    /// Source and native field name (the first slice does not rename fields).
    pub name: String,
    /// Exact checked C field type.
    pub ty: CBindingType,
}

/// Return a relocation-stable identity for one checked C binding declaration.
///
/// The compiler owns this identity so inspection, codegraph, language-server, lock, and Oven tooling can join the
/// exact descriptor they consumed without reconstructing it from source spelling. It deliberately
/// includes the checked declaration contract and logical module path, but excludes source spans and source-file
/// locations. The declared header spelling remains part of the ABI contract, so authors who require relocation-stable
/// identities must use a portable header spelling rather than a machine-local absolute path.
pub fn c_binding_descriptor_identity(module_path: &[String], descriptor: &CBindingDescriptor) -> String {
    let mut hasher = Sha256::new();
    hash_c_binding_text(&mut hasher, "schema", "incan-checked-c-binding-v1");
    hash_c_binding_list(&mut hasher, "module", module_path, |hasher, segment| {
        hash_c_binding_text(hasher, "segment", segment);
    });
    hash_c_binding_text(&mut hasher, "class", &descriptor.class_name);
    hash_c_binding_text(&mut hasher, "header", &descriptor.header);
    hash_c_binding_text(&mut hasher, "system_library", &descriptor.system_library);
    hash_c_binding_text(
        &mut hasher,
        "link_capability",
        link_capability_as_str(descriptor.link_capability),
    );
    if !descriptor.configuration.is_empty() {
        hash_c_binding_text(&mut hasher, "configuration", &descriptor.configuration);
    }
    hash_c_binding_list(&mut hasher, "resources", &descriptor.resources, |hasher, resource| {
        hash_c_binding_text(hasher, "name", &resource.name);
        hash_c_binding_text(hasher, "native", &resource.native);
        hash_c_binding_text(hasher, "release", &resource.release);
    });
    hash_c_binding_list(&mut hasher, "symbols", &descriptor.symbols, hash_c_binding_symbol);
    hash_c_binding_list(&mut hasher, "enums", &descriptor.enums, |hasher, enumeration| {
        hash_c_binding_text(hasher, "name", &enumeration.name);
        hash_c_binding_text(hasher, "carrier", scalar_type_as_str(enumeration.carrier));
        hash_c_binding_list(hasher, "variants", &enumeration.variants, |hasher, variant| {
            hash_c_binding_text(hasher, "name", &variant.name);
            hash_c_binding_text(hasher, "native", &variant.native);
        });
    });
    hash_c_binding_list(&mut hasher, "structs", &descriptor.structs, |hasher, structure| {
        hash_c_binding_text(hasher, "name", &structure.name);
        hash_c_binding_text(hasher, "native", &structure.native);
        hash_c_binding_list(hasher, "fields", &structure.fields, |hasher, field| {
            hash_c_binding_text(hasher, "name", &field.name);
            hash_c_binding_type(hasher, &field.ty);
        });
    });
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Add one symbol's ordered checked ABI contract to a descriptor identity.
pub(super) fn hash_c_binding_symbol(hasher: &mut Sha256, symbol: &CBindingSymbol) {
    hash_c_binding_text(hasher, "name", &symbol.name);
    hash_c_binding_text(hasher, "native", &symbol.native);
    hash_c_binding_list(hasher, "parameters", &symbol.parameters, |hasher, parameter| {
        hash_c_binding_text(hasher, "name", &parameter.name);
        hash_c_binding_type(hasher, &parameter.ty);
    });
    hash_c_binding_type(hasher, &symbol.return_type);
    hash_c_binding_list(hasher, "buffers", &symbol.buffers, |hasher, buffer| {
        hash_c_binding_text(hasher, "pointer_parameter", &buffer.pointer_parameter);
        hash_c_binding_text(hasher, "length_parameter", &buffer.length_parameter);
        hash_c_binding_text(hasher, "element", scalar_type_as_str(buffer.element));
    });
    hash_c_binding_list(hasher, "outcomes", &symbol.outcomes, |hasher, outcome| {
        hash_c_binding_text(hasher, "result", &outcome.result);
        hash_c_binding_list(hasher, "initializes", &outcome.initializes, |hasher, value| {
            hash_c_binding_text(hasher, "value", value);
        });
        hash_c_binding_list(hasher, "updates", &outcome.updates, |hasher, value| {
            hash_c_binding_text(hasher, "value", value);
        });
        hash_c_binding_list(hasher, "invalidates", &outcome.invalidates, |hasher, value| {
            hash_c_binding_text(hasher, "value", value);
        });
    });
}

/// Add a structural checked-C type encoding to a descriptor identity.
pub(super) fn hash_c_binding_type(hasher: &mut Sha256, ty: &CBindingType) {
    match ty {
        CBindingType::Scalar(scalar) => {
            hash_c_binding_text(hasher, "type", "scalar");
            hash_c_binding_text(hasher, "scalar", scalar_type_as_str(*scalar));
        }
        CBindingType::Pointer { mutable, pointee } => {
            hash_c_binding_text(hasher, "type", "pointer");
            hash_c_binding_text(hasher, "mutable", if *mutable { "true" } else { "false" });
            hash_c_binding_type(hasher, pointee);
        }
        CBindingType::Struct(name) => {
            hash_c_binding_text(hasher, "type", "struct");
            hash_c_binding_text(hasher, "name", name);
        }
        CBindingType::Resource { access, resource } => {
            hash_c_binding_text(hasher, "type", "resource");
            hash_c_binding_text(
                hasher,
                "access",
                match access {
                    CResourceAccess::Owned => "owned",
                    CResourceAccess::Borrowed => "borrowed",
                    CResourceAccess::BorrowedMut => "borrowed_mut",
                },
            );
            hash_c_binding_text(hasher, "resource", resource);
        }
        CBindingType::Output { mode, value } => {
            hash_c_binding_text(hasher, "type", "output");
            hash_c_binding_text(
                hasher,
                "mode",
                match mode {
                    COutputMode::Out => "out",
                    COutputMode::InOut => "in_out",
                },
            );
            hash_c_binding_type(hasher, value);
        }
        CBindingType::Nullable(value) => {
            hash_c_binding_text(hasher, "type", "nullable");
            hash_c_binding_type(hasher, value);
        }
        CBindingType::Void => hash_c_binding_text(hasher, "type", "void"),
    }
}

/// Delimit and hash an ordered descriptor list without ambiguous concatenation.
pub(super) fn hash_c_binding_list<T>(
    hasher: &mut Sha256,
    label: &str,
    values: &[T],
    hash_value: impl Fn(&mut Sha256, &T),
) {
    hash_c_binding_text(hasher, "list", label);
    hash_c_binding_text(hasher, "length", &values.len().to_string());
    for value in values {
        hash_value(hasher, value);
    }
}

/// Hash one labeled text field with explicit byte lengths for stable descriptor identities.
pub(super) fn hash_c_binding_text(hasher: &mut Sha256, label: &str, value: &str) {
    hasher.update(label.len().to_be_bytes());
    hasher.update(label.as_bytes());
    hasher.update(value.len().to_be_bytes());
    hasher.update(value.as_bytes());
}

impl CAbiInteropArtifacts {
    /// Return the verified C enum value selected by one recorded source access.
    pub fn enum_value_for_access(&self, span: Span) -> Option<i64> {
        let access = self.enum_accesses.iter().find(|access| access.span == span)?;
        self.enum_values
            .get(&(
                access.binding.clone(),
                access.enumeration.clone(),
                access.variant.clone(),
            ))
            .copied()
    }

    /// Derive facade edges only from typechecker-retained ordinary targets and direct raw-call owners.
    ///
    /// A public callable is a facade only when it directly calls a private callable in the same checked module that
    /// owns a checked raw call. The relation is deliberately absent for imported name matches, syntax-only calls,
    /// methods, and incomplete input.
    pub fn resolve_checked_facades(&mut self, module_path: Option<&[String]>) {
        self.facades.clear();
        let Some(module_path) = module_path else {
            return;
        };
        for function_call in &self.function_calls {
            if function_call.caller.visibility != Visibility::Public
                || !function_call.target.is_function()
                || function_call.target.module_path != module_path
            {
                continue;
            }
            for raw_call in &self.raw_calls {
                let Some(bridge) = raw_call.owner.as_ref() else {
                    continue;
                };
                if bridge.visibility != Visibility::Private || bridge.name != function_call.target.name {
                    continue;
                }
                let facade = CBindingFacade {
                    facade: function_call.caller.clone(),
                    bridge: bridge.clone(),
                    call_span: function_call.span,
                };
                if !self.facades.contains(&facade) {
                    self.facades.push(facade);
                }
            }
        }
        self.facades.sort_by(|left, right| {
            left.facade
                .name
                .cmp(&right.facade.name)
                .then_with(|| left.bridge.name.cmp(&right.bridge.name))
                .then_with(|| left.call_span.start.cmp(&right.call_span.start))
                .then_with(|| left.call_span.end.cmp(&right.call_span.end))
        });
    }
}
