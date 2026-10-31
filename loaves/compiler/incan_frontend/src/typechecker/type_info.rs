//! Lowering-facing typechecker artifact snapshots.
//!
//! This module contains the reusable semantic metadata that later compiler stages consume after typechecking. It keeps
//! the cross-phase snapshot surface separate from the main [`TypeChecker`](super::TypeChecker) orchestration state.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use sha2::{Digest, Sha256};

use crate::ast::{Expr, ParamKind, Span, Spanned, Visibility};
use crate::library_exports::CheckedParamDefault;
use crate::symbols::{
    CallableParam, ClassInfo, FunctionOverloadInfo, ImplementationTypeParamInfo, MethodInfo,
    NewtypePrimitiveConstraint, ResolvedType, TypeBoundInfo,
};
use incan_lang::interop::{CoercionPolicy, RustFunctionSig};
use incan_lang::lang::builtins::BuiltinFnId;
use incan_lang::lang::c_abi::{LinkCapabilityId, ScalarTypeId, link_capability_as_str, scalar_type_as_str};
use incan_lang::lang::surface::string_methods::StringMethodId;
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeCategory};
use incan_lang::lang::types::collections::{self as collection_types, CollectionTypeId};
use incan_semantics_core::{
    CanonicalStableDeclarationContext, CanonicalSymbolId, CompilerNodeId, IncanCallableParam, IncanCallableParamKind,
    IncanPrimitiveType, IncanType, SemanticFact, SemanticFactKind, SemanticFactStore, SemanticFactValue,
    SemanticRegistryEntry, SemanticRegistrySubjectKind, SemanticRegistryValue, SemanticSourceTarget,
    SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

use super::{ConstValue, const_eval};

/// Capture reusable typechecking output for later compiler stages.
///
/// This struct is the bridge that lets backend lowering/codegen consume the typechecker’s view of the program rather
/// than re-deriving types and semantics from the AST. The bridge is intentionally grouped by consumer contract: each
/// field names a semantic artifact family instead of exposing one flat collection of unrelated side channels.
///
/// ## Notes
/// - Expression types are keyed by `span` so downstream code can look them up without holding AST node identities.
/// - Const classification is recorded to support RFC 008 “Rust-native vs Frozen” const emission.
///
/// ## Examples
/// ```ignore
/// use incan_frontend::{lexer, parser, typechecker};
///
/// let tokens = lexer::lex("def foo() -> int: return 1")?;
/// let ast = parser::parse(&tokens)?;
/// let mut tc = typechecker::TypeChecker::new();
/// tc.check_program(&ast)?;
/// let info = tc.type_info();
/// // info.expr_type(...) can now be queried by spans.
/// ```
#[derive(Debug, Default, Clone)]
pub struct TypeCheckInfo {
    /// Trait hierarchy metadata consumed by trait impl and default-method lowering.
    pub traits: TraitArtifacts,
    /// Derive expansion metadata imported from dependency modules and manifests.
    pub derivations: DerivationArtifacts,
    /// Expression-local resolution facts keyed by source spans.
    pub expressions: ExpressionArtifacts,
    /// Source-reference resolution facts keyed by source spans.
    pub references: ReferenceArtifacts,
    /// Const evaluation facts needed by runtime and emission boundaries.
    pub consts: ConstArtifacts,
    /// Rust interop decisions that must be preserved exactly across lowering.
    pub rust: RustInteropArtifacts,
    /// Declaration-level binding rewrites and visibility facts consumed by lowering.
    pub declarations: DeclarationArtifacts,
    /// Checked typed declaration-registry definitions and descriptions.
    pub registry: RegistryArtifacts,
    /// Call-site semantic decisions selected by the typechecker.
    pub calls: CallArtifacts,
    /// Custom protocol decisions that lower into explicit runtime calls.
    pub protocols: ProtocolArtifacts,
    /// Checked C ABI declaration facts consumed by verification and code generation.
    pub c_abi: CAbiInteropArtifacts,
    /// Source import and alias paths accepted by typechecking, keyed by their active local binding.
    pub import_bindings: CheckedImportBindings,
    /// Active builtin type identities used only by the Body IR bridge; no checker decision or legacy consumer reads
    /// this snapshot.
    pub body_ir_runtime_types: BTreeMap<String, CanonicalSymbolId>,
    /// Atomic accepted-body refinements consumed only by Body IR; no checker or legacy consumer reads this group.
    pub(crate) body_ir_newtypes: super::native_newtype_facts::NativeNewtypeFacts,
    /// Accepted zero-argument iterator sum item types within the direct profile, keyed by the exact checked call span.
    ///
    /// Source newtype trait adoptions and source `sum` members need explicit dispatch and stay outside this snapshot.
    /// Only Body IR reads it; checking decisions and legacy-consumed facts are unchanged.
    pub(crate) body_ir_iterator_sum_items: BTreeMap<crate::ast::Span, ResolvedType>,
}

/// Checked source-path facts for active import-derived bindings.
///
/// These paths describe how source resolution accepted a local name. They intentionally do not replace canonical
/// symbol identities: a re-exported binding can retain its facade path here while its identity names the original
/// declaring module.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CheckedImportBindings {
    paths: BTreeMap<String, Vec<String>>,
}

impl CheckedImportBindings {
    /// Return the checked source path for one active local binding.
    pub fn path(&self, local_name: &str) -> Option<&[String]> {
        self.paths.get(local_name).map(Vec::as_slice)
    }

    /// Iterate active local bindings and their checked source paths in deterministic name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.paths.iter().map(|(name, path)| (name.as_str(), path.as_slice()))
    }

    /// Build checked import bindings from compiler-resolved local names and source paths.
    pub fn from_paths(paths: impl IntoIterator<Item = (String, Vec<String>)>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
        }
    }
}

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
    /// argument names or generated Rust.
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
/// exact descriptor they consumed without reconstructing it from source spelling or generated Rust. It deliberately
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
fn hash_c_binding_symbol(hasher: &mut Sha256, symbol: &CBindingSymbol) {
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
fn hash_c_binding_type(hasher: &mut Sha256, ty: &CBindingType) {
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
fn hash_c_binding_list<T>(hasher: &mut Sha256, label: &str, values: &[T], hash_value: impl Fn(&mut Sha256, &T)) {
    hash_c_binding_text(hasher, "list", label);
    hash_c_binding_text(hasher, "length", &values.len().to_string());
    for value in values {
        hash_value(hasher, value);
    }
}

/// Hash one labeled text field with explicit byte lengths for stable descriptor identities.
fn hash_c_binding_text(hasher: &mut Sha256, label: &str, value: &str) {
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

/// Trait hierarchy metadata consumed by trait impl and default-method lowering.
#[derive(Debug, Default, Clone)]
pub struct TraitArtifacts {
    /// RFC 042: Direct supertraits per trait name, copied from
    /// [`TraitInfo::supertraits`](crate::symbols::TraitInfo::supertraits) for IR lowering.
    ///
    /// Lowering does not retain the typechecker symbol table; this snapshot supplies resolved supertrait type
    /// arguments after a successful check.
    pub direct_supertraits: HashMap<String, Vec<(String, Vec<ResolvedType>)>>,
    /// RFC 042: Trait type parameter names keyed by trait name for lowering-time generic substitution.
    ///
    /// Includes locally-declared and imported traits so backend lowering can handle cross-module trait hierarchies
    /// without relying on local AST declarations.
    pub type_params: HashMap<String, Vec<String>>,
    /// Exact source identities of visible trait methods, keyed by trait identity and method spelling.
    ///
    /// A source-visible trait uses its local binding as the trait key. A dependency-only trait uses its
    /// module-qualified source name. Imported default-method ASTs retain declaration spans from another source file,
    /// so lowering cannot look them up in the current module's span-keyed declaration table. This checked map carries
    /// the already-resolved identity across that boundary without reconstructing it from either spelling.
    pub method_identities: HashMap<(String, String), CanonicalSymbolId>,
    /// Visible trait slots, local or imported, whose checked declaration supplies a default body.
    ///
    /// A consumer still requires that body's executable representation; this fact never supplies a missing body.
    pub default_method_identities: HashSet<CanonicalSymbolId>,
    /// Visible trait slots the standard packages declare, including through aliases and facades.
    ///
    /// Their protocol contracts do not imply executable source bodies among the project's modules.
    pub standard_method_identities: HashSet<CanonicalSymbolId>,
    /// Checked trait slots selected by calls on an open `Self` receiver, keyed by the whole call span.
    ///
    /// Body IR retains these declaration identities for concrete specialization. They remain separate from ordinary
    /// resolved call identities because legacy default expansion selects each adopter's implementation later.
    pub self_method_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Declared result types of the trait slots called on an open `Self` receiver in the standard library's own trait
    /// defaults, keyed by the whole call span.
    ///
    /// The standard library's source leaves such a call's checked type open, because legacy expands those defaults by
    /// its own rules. Body IR, which publishes the defaults' executable representation, takes the slot's declared
    /// result from here instead; no legacy route reads it.
    pub self_method_result_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Declared parameters of the same slots, keyed by the whole call span, so Body IR binds the call's arguments to
    /// the slot's declaration as it binds any checked call; no legacy route reads it.
    pub self_method_params: HashMap<crate::ast::Span, Vec<CallableParam>>,
    /// Compiler-owned requirements inferred for otherwise unresolved calls on rigid declaration parameters. Only
    /// Body IR consumes these span-keyed targets; legacy call identities and explicit bound selections stay intact.
    pub inferred_method_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Checked type arguments of each source `with Trait[...]` adoption clause, keyed by the clause's span.
    ///
    /// Body IR instantiates an adopted trait's default methods for a concrete adopter from these arguments, so no
    /// later stage re-resolves the written clause.
    pub adoption_type_args: HashMap<crate::ast::Span, Vec<ResolvedType>>,
    /// Implicit arguments of bare source adoption clauses, retained only for Body IR by the clause's span. The
    /// existing semantic instantiation rule supplies owner parameter witnesses; legacy's explicit-only snapshot stays
    /// unchanged. Ambiguous or incomplete adoptions contribute no fact.
    pub body_ir_adoption_type_args: HashMap<crate::ast::Span, Vec<ResolvedType>>,
}

impl TraitArtifacts {
    /// Whether a visible trait name resolves to a standard-package protocol: the checker recorded slots for it, and
    /// every one carries such an identity.
    pub fn is_standard_trait(&self, name: &str) -> bool {
        let mut slots = self
            .method_identities
            .iter()
            .filter(|((trait_name, _), _)| trait_name == name)
            .peekable();
        slots.peek().is_some() && slots.all(|(_, identity)| self.is_standard_slot(identity))
    }

    /// Whether a trait slot belongs to a standard-package protocol.
    pub fn is_standard_slot(&self, identity: &CanonicalSymbolId) -> bool {
        self.standard_method_identities.contains(identity)
    }

    /// Retain one visible slot's identity under its trait key, and whether its checked declaration has a default body.
    pub(crate) fn record_method(&mut self, trait_name: &str, method_name: &str, method: &MethodInfo) {
        let Some(identity) = &method.identity else {
            return;
        };
        self.method_identities
            .insert((trait_name.to_owned(), method_name.to_owned()), identity.clone());
        if method.has_body {
            self.default_method_identities.insert(identity.clone());
        }
    }
}

/// Derive expansion metadata imported from dependency modules and manifests.
#[derive(Debug, Default, Clone)]
pub struct DerivationArtifacts {
    /// Exact exported derive macro paths inspected from declared native dependencies. These authorize forwarding
    /// the macro to rustc, never an assumed layout change, generated method, or trait implementation.
    pub native_macro_paths: HashSet<String>,
    /// RFC 024: Trait-level Rust derive paths keyed by module-qualified trait name, such as `yaml.Serialize`.
    ///
    /// The typechecker owns this because dependency modules are already imported and validated there. Lowering should
    /// not re-run module resolution or assume RFC 024 metadata only exists in stdlib source.
    pub trait_rust_derive_paths: HashMap<String, Vec<String>>,
}

/// Transient requirements for one exact mutable empty-list initializer; only successful checked uses can close it.
#[derive(Debug, Clone)]
pub(crate) struct BodyIrEmptyListRequirement {
    /// Source span of the syntactically empty initializer whose element is still unknown.
    pub(crate) initializer: crate::ast::Span,
    /// The declaration's only authorized write; any additional write prevents publication for this initializer.
    pub(crate) declaration_write: crate::ast::Span,
    /// Checked builtin collection name, preserved when the agreed element replaces its hole.
    pub(crate) list_name: String,
    /// Agreed fully closed append element, absent until a validated append supplies one.
    pub(crate) element: Option<ResolvedType>,
    /// A conflict, reassignment or unproven method prevents later appends from reinstating a grant.
    pub(crate) invalidated: bool,
    /// Exact receiver and length-argument reads whose checked contracts do not escape the candidate binding.
    pub(crate) permitted_reads: HashSet<crate::ast::Span>,
}

/// Expression-local resolution facts keyed by source spans.
#[derive(Debug, Default, Clone)]
pub struct ExpressionArtifacts {
    /// Unbounded source call binders awaiting constraints from their exact lexical result values.
    pub(crate) body_ir_open_calls: Vec<BodyIrOpenCall>,
    /// Symbolic results tied to one source call's binder scope, including tuple elements and method results.
    pub(crate) body_ir_open_values: HashMap<crate::ast::Span, BodyIrOpenValue>,
    /// Lexical aliases of a symbolic result; spelling never joins independent factory calls.
    pub(crate) body_ir_open_bindings: HashMap<CanonicalSymbolId, BodyIrOpenValue>,
    /// Candidate empty-list constraints collected during checking and consumed by successful final publication.
    /// Legacy never reads this state; overload transactions preserve it with the rest of TypeCheckInfo.
    pub(crate) body_ir_empty_list_requirements: HashMap<CanonicalSymbolId, BodyIrEmptyListRequirement>,
    /// Body IR-only callable-name reads selected by checking the receiver, keyed by the full field expression span.
    /// Ordinary source fields and metadata on type expressions do not enter this set; legacy member tables stay
    /// intact.
    pub(crate) body_ir_callable_name_reads: std::collections::BTreeSet<crate::ast::Span>,
    /// Executable expression types closed from checked value contracts, consumed only by Body IR. Legacy keeps its
    /// original contextual expression snapshot, including holes that Rust inference completes later.
    pub body_ir_expr_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Closed value contracts for exact checked lexical bindings; shadowed names never share an entry.
    pub body_ir_binding_types: HashMap<CanonicalSymbolId, ResolvedType>,
    /// Checked nominal pattern owners reached through an import alias when the legacy internal-name lookup has no
    /// lexical symbol. Only Body IR consumes these additive facts; missing identities remain unsupported patterns.
    pub body_ir_nominal_pattern_owners: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Agreed types required by validated function calls for exact parameter identities; `None` records a conflict.
    /// Only Body IR's implicit closure contracts consume these requirements; legacy parameter snapshots stay
    /// unchanged.
    pub body_ir_parameter_requirements: HashMap<CanonicalSymbolId, Option<ResolvedType>>,
    /// Resolved expression types keyed by original-source range and compiler-owned expansion context.
    pub expr_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Canonical named-type leaves of each checked expression type, addressed by generic/tuple child indices.
    ///
    /// The checker selects these identities while its accepted type bindings are available. Consumers retain the
    /// structural path and identity; an import alias is never a runtime type or layout key.
    pub expression_type_identities: HashMap<crate::ast::Span, BTreeMap<Vec<usize>, CanonicalSymbolId>>,
    /// Final checked type of an assignment binding, keyed by the assignment statement span.
    ///
    /// This differs from the initializer expression type when contextual numeric typing or a validated coercion
    /// selects the annotated destination type. Body IR consumes this fact instead of reconstructing annotations.
    pub assignment_binding_types: HashMap<crate::ast::Span, ResolvedType>,
    /// RFC 046 computed property reads keyed by the full field-access expression span.
    ///
    /// Lowering/emission can use this to distinguish `obj.field` storage reads from `obj.property` getter calls while
    /// still consuming the same resolved expression type map for the property return type.
    pub computed_property_accesses: HashMap<crate::ast::Span, ComputedPropertyAccessInfo>,
    /// Identifier resolution keyed by the complete checked span (value, type or module).
    ///
    /// This exists so downstream stages (IR lowering/codegen) can reliably distinguish:
    /// - `x.method(...)` where `x` is a value binding, from
    /// - `Type.method(...)` where `Type` is a type name (emits `Type::method(...)` in Rust), and
    /// - imported placeholders (e.g. `from rust::... import Foo`) which are not value bindings.
    pub ident_kinds: HashMap<crate::ast::Span, IdentKind>,
    /// Identifier spans that resolved to the compiler-provided ambient `std.logging` logger binding.
    ///
    /// The binding is typechecked like an ordinary immutable `Logger` value, but lowering must materialize it as a
    /// module-local `std.logging.get_logger(...)` call so source metadata can become the logger name.
    pub ambient_logger_bindings: HashSet<crate::ast::Span>,
    /// The type of the place a value is written to, keyed by the value span, when the value needs adapting to it: a
    /// field or index assignment, a model or class constructor field, a `return`, a `yield`, or a `match` arm or
    /// `break` value of the construct's type (#1858, RFC 009).
    ///
    /// The checker accepts a value of an `Option`'s payload type there (`box.count = 5` for an `Option[int]` field),
    /// and a numeric value of a type that losslessly widens to the place's numeric type or union member
    /// (`box.count = small` for an `i8` value and an `int` field). Lowering wraps such a value in the `Some` layers
    /// the destination adds and widens it to the numeric type the destination holds.
    pub value_destination_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Explicitly admitted numeric conversions keyed by the complete boundary expression span. Unlike ordinary
    /// lossless destination adaptation, these facts authorize a source conversion that compatibility alone rejects.
    /// Body IR materializes the selected destination through its ordinary numeric assignment machinery.
    pub numeric_conversion_destinations: HashMap<crate::ast::Span, ResolvedType>,
    /// Spans of `dict.get(key)` calls whose result is only read, so no copy of the stored value is needed.
    ///
    /// `get` answers with `Option[V]` on every dict. A lookup whose result feeds a `match` or `if let` that only reads
    /// its bindings (see `check_expr/dict_lookups.rs`) is lowered to read the entry in place; every other lookup of a
    /// dict that is not static storage is completed with a copy of the entry.
    pub read_only_dict_lookups: HashSet<crate::ast::Span>,
    /// Values of chained assignments that each target gets its own evaluation of, keyed by the value span (#1806).
    pub chained_values_written_per_target: HashSet<crate::ast::Span>,
    /// RFC 017 validated-newtype coercion decisions keyed by source expression span.
    ///
    /// Lowering consumes these decisions when an expression is used at an approved implicit-coercion site, such as a
    /// function argument, typed initializer, or model/class field initializer.
    pub validated_newtype_coercions: HashMap<crate::ast::Span, ValidatedNewtypeCoercionInfo>,
    /// Source-level codegraph targets proven during expression checking, keyed by call or reference expression span.
    ///
    /// The codegraph exporter consumes this instead of re-resolving names from syntax. Absence means the target is
    /// unsupported, ambiguous, degraded, or outside the current conservative source target set.
    pub source_targets: HashMap<crate::ast::Span, SourceTargetInfo>,
    /// The method a local partial's target names on a value (`partial user.label(prefix="x")`), keyed by the target
    /// span, by the name its declaration has (after a method alias) (RFC 084).
    ///
    /// Lowering evaluates the target's receiver once, when the partial is built, and calls this method on it.
    pub local_partial_method_targets: HashMap<crate::ast::Span, String>,
}

/// One accepted source call's independent inference scope; unresolved and conflicting groups grant no storage.
#[derive(Debug, Clone)]
pub(crate) struct BodyIrOpenCall {
    pub span: Span,
    pub parameters: Vec<String>,
    pub result: ResolvedType,
    pub bindings: HashMap<String, ResolvedType>,
    pub invalidated: bool,
}

/// A value template whose placeholders belong only to the indexed source call.
#[derive(Debug, Clone)]
pub(crate) struct BodyIrOpenValue {
    pub group: usize,
    pub ty: ResolvedType,
}

/// Source-reference resolution facts keyed by source spans.
#[derive(Debug, Default, Clone)]
pub struct ReferenceArtifacts {
    /// RFC 120 canonical identities of resolved value and type references.
    ///
    /// A local, an import, an alias, and a re-export of one declaration all record the *same* value here, so a
    /// consumer can decide "do these two references mean the same thing" structurally without comparing spellings.
    /// [`ExpressionArtifacts::source_targets`] stays the string-shaped codegraph projection; it is never the
    /// identity. Absence means resolution did not prove an identity for that reference — consumers fail closed
    /// rather than reconstruct one.
    pub resolved_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// RFC 120 identities selected for statement-owned write targets at their exact authored identifier spans.
    ///
    /// Single, tuple-unpack, chained, and compound assignments retain one target span per written identifier. Keying
    /// by that exact span plus the target spelling preserves each target independently without asking Body IR lowering
    /// to repeat lexical lookup after the typechecker has exited the binding's scope. Compiler-generated assignments
    /// use their unique synthetic target spans through the same contract.
    pub resolved_write_identities: HashMap<(crate::ast::Span, String), CanonicalSymbolId>,
    /// Checked type of each statement-owned write target, keyed identically to [`Self::resolved_write_identities`].
    pub resolved_write_types: HashMap<(crate::ast::Span, String), ResolvedType>,
}

/// Const evaluation facts needed by runtime and emission boundaries.
/// Additive Body IR values do not replace legacy's classification or scalar evaluation tables.
#[derive(Debug, Default, Clone)]
pub struct ConstArtifacts {
    /// Exact immutable sized-integer constructor values selected by the const checker, keyed by initializer span.
    pub(crate) body_ir_newtype_values: HashMap<crate::ast::Span, BodyIrNewtypeConstantValue>,
    /// Const category classification (RFC 008): const name -> kind.
    pub const_kinds: HashMap<String, const_eval::ConstKind>,
    /// Computed const values (when available), keyed by const name.
    pub const_values: HashMap<String, ConstValue>,
}

/// A validated const expression's newtype owner and full numeric value, without a backend-selected layout.
#[derive(Debug, Clone)]
pub(crate) struct BodyIrNewtypeConstantValue {
    /// Canonical lexical newtype binding selected during const validation.
    pub owner: CanonicalSymbolId,
    /// Complete checked numeric kind and value; source `int` and noninteger carriers are outside this slice.
    pub value: incan_semantics_core::body_ir::TypedNumericConstant,
}

/// Rust interop decisions that must be preserved exactly across lowering.
#[derive(Debug, Default, Clone)]
pub struct RustInteropArtifacts {
    /// Structural native storage keyed by the original checked type. Diagnostic displays are opaque RustInteropPath
    /// keys; consumers never parse or complete them.
    pub storage_types: HashMap<IncanType, IncanType>,
    /// Declaration-order native binders and defaults used while the accepting checker freezes native storage.
    pub owner_parameters: HashMap<String, Vec<(String, Option<ResolvedType>)>>,
    /// Native by-value receiver calls whose nongeneric owner has an unconditional metadata-selected Copy
    /// implementation. The adapter verifies that capability against the exact linked native layout.
    pub copy_receivers: HashMap<crate::ast::Span, String>,
    /// Public native field layouts selected during checking. Hidden fields remain absent from metadata and the
    /// adapter refuses incomplete layouts before any construction or projection is emitted.
    pub checked_layouts: HashMap<String, RustCheckedLayout>,
    /// Complete native enum payload templates selected by constructor patterns, keyed by the public owner path.
    pub checked_enums: HashMap<String, RustCheckedEnum>,
    /// Public Rust constants selected from metadata, keyed by their complete value-expression span.
    pub checked_constants: HashMap<crate::ast::Span, String>,
    /// Resolved Rust callees with closed native arguments and signatures, keyed by the complete call span.
    /// Body IR consumes these records without resolving source spellings or inferring foreign types again.
    pub checked_calls: HashMap<crate::ast::Span, RustCheckedCall>,
    /// Checked native argument adaptations, including dereference calls and owned payload storage constructors,
    /// keyed by argument expression span. Body IR emits these exact steps before the source call.
    pub argument_adjustments: HashMap<crate::ast::Span, Vec<RustCheckedCall>>,
    /// Exact infallible rusttype edges selected from authored adapters and inspected native signatures.
    /// Ordinary source boundaries select only a matching source/destination pair, never any edge on the owner.
    pub(crate) declared_conversions: Vec<RustDeclaredConversion>,
    /// Exact owned native From calls applied to each source list element at an argument boundary. The list is
    /// evaluated once; the selected conversion is executed once per element in source order.
    pub element_adjustments: HashMap<crate::ast::Span, RustCheckedCall>,
    /// Metadata-selected shared dereference steps before a native public field read, keyed by the field expression.
    /// Field identity and payload remain in their ordinary checked-field facts.
    pub field_adjustments: HashMap<crate::ast::Span, Vec<RustCheckedCall>>,
    /// Exact physical field paths selected for source extractor forwarding, including the native tuple slot.
    pub field_projection_paths: HashMap<crate::ast::Span, Vec<CanonicalSymbolId>>,
    /// Exact native values of public field expressions before source collection and scalar projection.
    pub field_values: HashMap<crate::ast::Span, RustCheckedFieldValue>,
    /// Safe closed native functions selected as values, keyed by the read span rather than an application span.
    /// Their signatures share call publication while their value role never supplies a call's result provenance.
    pub checked_function_values: HashMap<crate::ast::Span, RustCheckedCall>,
    /// Closed native iteration calls selected from IntoIterator and Iterator facts at the iterable's span.
    pub iteration_hooks: HashMap<crate::ast::Span, RustIterationHooks>,
    /// Native result item selected for each local binding, even when its source type projects to str or a collection.
    pub value_item_paths: HashMap<CanonicalSymbolId, String>,
    /// Native carriers of Rust-derived local values before source reference and collection projection.
    pub value_native_types: HashMap<CanonicalSymbolId, ResolvedType>,
    /// Validated synchronous diverging Rust import calls, keyed by full call span. Body IR alone consumes the
    /// selected foreign path; legacy reference identities and Rust emission facts remain unchanged (#1698).
    pub(crate) body_ir_diverging_import_calls: HashMap<crate::ast::Span, Vec<String>>,
    /// Source-proven receiver and returned-borrow relationships keyed by the exact checked method-call span.
    /// Unknown metadata never authorizes a shared alias; lowering must not reconstruct this from method names.
    pub receiver_contracts: HashMap<crate::ast::Span, incan_lang::interop::RustReceiverContract>,
    /// `rusttype` Incan name → canonical Rust path string (`substrait::proto::type::Binary`), when the checker
    /// resolved the underlying type to [`ResolvedType::RustPath`]. Used by lowering so `m::T` spellings emit full
    /// paths without re-running import resolution.
    pub rusttype_canonical_paths: HashMap<String, String>,
    /// Every spelling of an imported Rust item (its import path and its definition path) → the public path that names
    /// it. Rust metadata spells a returned type by its definition (`std::sync::poison::mutex::Mutex`), an annotation
    /// by its import (`std::sync::Mutex`); direct-route storage keys both on the one public path (DD-0006).
    pub item_public_paths: HashMap<String, String>,
    /// Rust-boundary coercion decisions keyed by argument expression span.
    pub arg_coercions: HashMap<crate::ast::Span, RustArgCoercionInfo>,
    /// Rust trait imports keyed by the source binding name with the trait path and method names they can place in
    /// scope.
    ///
    /// Lowering carries this into IR import items so codegen can retain extension-trait imports when Rust method
    /// lookup needs the trait in scope even though emitted call tokens do not otherwise mention the trait name.
    pub trait_imports: HashMap<String, RustTraitImportInfo>,
    /// Body-less rusttype Rust-trait adoptions proven by metadata and therefore satisfied by the backing type alias,
    /// keyed by `(rusttype name, written trait name)`.
    ///
    /// Body IR keeps such a rusttype a transparent alias (`is_direct_native_rusttype_alias`): Rust coherence treats
    /// the alias as the foreign backing type, which already implements the trait. The typechecker records only
    /// non-generic trait paths that metadata proved.
    pub rusttype_forwarded_trait_adoptions: HashSet<(String, String)>,
    /// Rust-boundary coercion decisions for method return values, keyed by the call expression span.
    ///
    /// Checker working state: recorded when metadata shows a `rusttype` method's actual Rust return type requires
    /// coercion to the Incan-declared type (e.g. `&str` → `String` for a method declared `-> str`), and read by later
    /// trait-method selection in the same check.
    pub return_coercions: HashMap<crate::ast::Span, RustArgCoercionInfo>,
    /// Borrowed Rust call results that must remain in their native Rust representation at the next call boundary.
    ///
    /// Exact generic parameters provide this evidence directly. An unresolved imported Rust call also preserves its
    /// direct Rust-return arguments rather than inventing an Incan-owned conversion before rustc sees the boundary.
    /// The marker makes either decision independent of whether result or argument checking runs first.
    pub native_return_consumers: HashSet<crate::ast::Span>,
    /// Rust closure parameter displays keyed by closure-expression span.
    ///
    /// This is populated when contextual Rust metadata proves a closure is being used as a Rust callable boundary
    /// whose parameter shape cannot be faithfully represented by ordinary Incan surface types, such as `&[T]`.
    /// Lowering/emission consumes the displays directly so generated closures keep Rust inference stable.
    pub closure_param_type_displays: HashMap<crate::ast::Span, Vec<String>>,
    /// Native closure storage admitted at a Rust callback boundary, keyed by its originating closure expression.
    pub closure_storage: HashMap<crate::ast::Span, incan_semantics_core::body_ir::NativeClosureStorage>,
    /// Native callback parameter slots paired with their exact declaration-order generic argument positions.
    pub native_callable_parameters: HashMap<String, Vec<(usize, usize)>>,
    /// Exact Rust parameter displays for source functions passed directly to a Rust callable boundary.
    ///
    /// Source annotations intentionally use Incan collection vocabulary (`&mut list[f32]`), while Rust callback
    /// bounds can require a slice (`&mut [f32]`). Lowering uses this fact only for the proven callback function so it
    /// does not globally change how Incan lists are represented.
    pub function_param_type_displays: HashMap<String, Vec<String>>,
    /// Rust async call expressions proven during call validation, keyed by the full call-expression span.
    ///
    /// Rust metadata is now loaded lazily at the call boundary rather than during import collection. `await` consumes
    /// this artifact so direct async Rust calls still realize to their output type without reintroducing import-time
    /// extraction or relying on stale symbol metadata.
    pub async_call_realizations: HashSet<crate::ast::Span>,
    /// Metadata-selected IntoFuture conversions keyed by the await operand's span; Body IR emits each once before
    /// suspension.
    pub await_conversions: HashMap<crate::ast::Span, RustCheckedCall>,
    /// Transparent rusttype aliases with their native owner template and source type parameters.
    pub type_aliases: HashMap<String, (Vec<String>, ResolvedType)>,
}

/// A metadata-selected native struct layout, generic over its declaring type parameters.
#[derive(Debug, Clone)]
pub struct RustCheckedLayout {
    /// Source visibility of each native field slot, aligned with `fields`.
    pub field_public: Vec<bool>,
    /// An unconditional native Copy implementation retained with the owner, independent of which module uses it.
    pub is_copy: bool,
    /// Declared type binders; lifetime arguments carry no layout and are absent.
    pub type_parameters: Vec<String>,
    /// Ordered native field names and exact storage templates, including inaccessible private slots.
    pub fields: Vec<CallableParam>,
    /// Canonical field targets minted with the same owner as the retained layout.
    pub field_identities: Vec<CanonicalSymbolId>,
}

/// A public field's native carrier and item identity before source type projection.
#[derive(Debug, Clone)]
pub struct RustCheckedFieldValue {
    /// Instantiated native field type, including references and type arguments.
    pub native_type: ResolvedType,
    /// Public native item selected from metadata, when the field has a nominal carrier.
    pub item_path: Option<String>,
}

/// A complete native enum layout selected from metadata; declaration binders remain open until storage is closed.
#[derive(Debug, Clone)]
pub struct RustCheckedEnum {
    /// Unconditional native Copy capability, retained before any concrete enum carrier is emitted.
    pub is_copy: bool,
    /// Type binders in native declaration order, excluding lifetimes.
    pub type_parameters: Vec<String>,
    /// Ordered variant names, exact native payload templates and named-field slots, before match ergonomics adds
    /// references.
    pub variants: Vec<(String, Vec<ResolvedType>, Vec<String>)>,
}

/// One authored rusttype edge and the exact native adapter selected for its source and destination types.
#[derive(Debug, Clone)]
pub(crate) struct RustDeclaredConversion {
    /// Exact source type accepted by this edge.
    pub(crate) source: ResolvedType,
    /// Exact destination selected by this edge.
    pub(crate) destination: ResolvedType,
    /// The authored edge's span, used to admit only declarations whose complete native edges were checked.
    pub(crate) span: Span,
    /// Inspected adapter signature and canonical callable identity.
    pub(crate) call: RustCheckedCall,
}

/// A metadata-selected Rust call, retaining the signature and receiver passing mode the checker admitted.
#[derive(Debug, Clone, PartialEq)]
pub struct RustCheckedCall {
    /// Fully qualified structural signature, published by the accepting checker before Body IR construction.
    pub storage: Option<RustCheckedSignature>,
    /// Source declaration binders active when this native call was admitted, independently of nominal spellings.
    pub source_type_parameters: Vec<String>,
    /// Flat, checked Deref/DerefMut callees evaluated on the receiver before written ordinary arguments.
    pub receiver_adjustments: Vec<RustCheckedCall>,
    /// Public item path of the selected free function or associated function.
    pub path: String,
    /// Native type arguments in the Rust declaration's order, including impl arguments for a method.
    pub type_arguments: Vec<ResolvedType>,
    /// Instantiated parameters; a method's receiver is first, including its reference mode.
    pub parameters: Vec<CallableParam>,
    /// The checked instantiated result type.
    pub return_type: ResolvedType,
    /// Native result's nominal item before source scalar or collection projection, when metadata names one.
    pub result_item_path: Option<String>,
    /// Whether the source receiver is a value passed in the first parameter slot.
    pub has_receiver: bool,
}

/// Native call storage and source nominal owners selected in the checker's declaration environment.
#[derive(Debug, Clone, PartialEq)]
pub struct RustCheckedSignature {
    /// Native argument types, including the receiver's exact reference mode.
    pub parameters: Vec<IncanType>,
    /// Native result before source boundary conversions.
    pub return_type: IncanType,
    /// Native type arguments in declaration order.
    pub type_arguments: Vec<IncanType>,
    /// Exact source owners backing identity-encoded storage leaves.
    pub identities: BTreeMap<String, CanonicalSymbolId>,
}

/// Native iteration retains both calls independently of the written expression's call facts.
#[derive(Debug, Clone, PartialEq)]
pub struct RustIterationHooks {
    /// Consuming conversion that materializes iterator storage once.
    pub iter: RustCheckedCall,
    /// Exclusive poll whose Option payload defines the loop item's carrier.
    pub next: RustCheckedCall,
}

/// Declaration-level binding rewrites and visibility facts consumed by lowering.
#[derive(Debug, Default, Clone)]
pub struct DeclarationArtifacts {
    /// Checked source-signature identities published before body checks or native representation selection.
    pub source_identities: BTreeMap<CanonicalSymbolId, incan_semantics_core::stable_identity::StableDeclarationId>,
    /// Body IR-only canonical trait adoptions, retaining marker protocols and ordered checked arguments.
    /// Legacy conformance, derive selection, and method tables never consume this snapshot.
    pub body_ir_trait_adoptions: HashMap<String, Vec<BodyIrCheckedTraitAdoption>>,
    /// Rust trait method slots a class fills with its own methods. The native compiler checks each implementation.
    pub native_trait_implementations: Vec<incan_semantics_core::body_ir::TraitImplementation>,
    /// Body IR-only checked local inheritance and ordered ancestor field identities; legacy class layouts stay intact.
    pub body_ir_class_inheritance: HashMap<String, BodyIrClassInheritance>,
    /// Body IR-only source trait slots adopted by checked derive bundles, keyed by the adopting declaration.
    pub body_ir_derived_methods: HashMap<String, Vec<BodyIrDerivedMethod>>,
    /// Checked payload types keyed by their source annotation span; direct lowering never resolves raw annotations.
    pub enum_payload_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Explicit and automatic native derives selected by checking for each source enum; a std.serde.json protocol
    /// selects serde's derive path, as for models.
    pub enum_derives: HashMap<String, Vec<String>>,
    /// Explicit builtin, canonical serde, and implicit native derives selected for source models, in legacy emission
    /// order, except that the builtins a `@rust.derive(...)` names follow the `@derive(...)` selections whichever
    /// decorator comes first (the order of a Rust derive list has no observable effect). Derived Display remains a
    /// semantic selection, not a Rust macro.
    pub model_derives: HashMap<String, Vec<String>>,
    /// Body IR-only native derive paths after checked RFC 024 module and trait expansion. Descriptor is omitted
    /// because its checked structural opt-in has no native macro; legacy keeps consuming `model_derives` unchanged.
    pub body_ir_model_derives: HashMap<String, Vec<String>>,
    /// Declaration spans of `@rust.extern` functions whose Rust item is an exported attribute macro, with its
    /// canonical path. Such a declaration names a macro that rustc applies to an item; it has no callable body to
    /// delegate to.
    pub native_attribute_externs: HashMap<crate::ast::Span, String>,
    /// Declaration spans of functions that apply exported attribute macros through their decorators, with each
    /// application in written order. The providing Loaf's facts select the macro; the written arguments are kept.
    pub native_attribute_applications: HashMap<crate::ast::Span, Vec<incan_semantics_core::body_ir::NativeAttribute>>,
    /// Canonical explicit native derives selected for newtypes; automatic derives remain construction facts.
    pub body_ir_newtype_derives: HashMap<String, Vec<String>>,
    /// The same native derive selection for source classes, whose automatic derives match a model's.
    pub class_derives: HashMap<String, Vec<String>>,
    /// Serde derives a source class's trait adoptions imply, directly or through a supertrait, keyed by class name.
    ///
    /// A class retains the plain native derives besides these; legacy emission forwards the same selection.
    pub class_adoption_derives: HashMap<String, Vec<String>>,
    /// Source types whose `@rust.derive(...)` decorators are completely covered by their retained native selections.
    /// Builtins and inspected exported derive macros retain their canonical targets; expansion predictions are absent.
    pub rust_native_derive_types: HashSet<String>,
    /// Active source and package nominal bindings and their canonical carrier keys, retained by the checker before
    /// lexical context is discarded. Body IR copies this proof without rebuilding a name table.
    pub named_type_identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
    /// Active compiler-owned native nominal bindings for Body IR only. Legacy continues to consume the ordinary
    /// nominal table; a real source binding under the same spelling cannot authorize this foreign ABI.
    pub body_ir_native_type_identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
    /// Checked local model field types keyed by exact field declaration span.
    pub model_field_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Checked field visibility for local models, keyed first by model name and then canonical field name.
    ///
    /// Field modifiers and the containing model's visibility are resolved by the frontend. Lowering consumes this
    /// snapshot instead of reinterpreting the source visibility contract.
    pub model_field_visibilities: HashMap<String, HashMap<String, Visibility>>,
    /// Local model fields whose checked privacy boundary is the declaring type rather than only the source module.
    pub model_type_private_fields: HashSet<(String, String)>,
    /// Checked, parent-first field layouts for local classes.
    ///
    /// Local subclasses can inherit from classes reconstructed from compiled dependency manifests. Those parents do
    /// not have consumer-side AST declarations, so lowering must consume this checked layout instead of rediscovering
    /// inherited fields from syntax.
    pub class_layouts: HashMap<String, ClassLayoutInfo>,
    /// Compiler-checked construction semantics for local newtypes.
    ///
    /// Lowering consumes this snapshot instead of rediscovering newtypes from tuple-struct shape or raw decorators.
    pub newtype_construction: HashMap<String, NewtypeConstructionInfo>,
    /// Module-local function declarations keyed by source name after annotation resolution.
    ///
    /// Lowering consumes this instead of re-lowering raw AST annotations so aliases such as `type Expr = Union[...]`
    /// do not produce a different callable surface from typechecked call sites.
    pub function_bindings: HashMap<String, FunctionBindingInfo>,
    /// Canonical identity proven for each imported binding, keyed by the local name the import introduced.
    ///
    /// [`SourceTargetInfo::module_path`] records the import path *as written*, which is not necessarily the module
    /// resolution selected: sibling-relative candidates are tried before bare ones, so a written path can name a
    /// different module that merely declares the same leaf name. A declaration identity must never be built from the
    /// written path, so the proven identity is recorded here and is simply absent when resolution did not prove one.
    /// A re-export resolves to the identity of the module that *declares* the member, never to the facade.
    pub resolved_import_identities: HashMap<String, CanonicalSymbolId>,
    /// Module-qualified type annotations the checker resolved, keyed by their dotted source spelling.
    ///
    /// `mod.Type` in type position reaches a declaration through a module binding rather than through a local type
    /// name, so no import binding carries it and lowering has no local symbol to consult (#1437). The spelling is a
    /// module-scope fact: its root is a module binding and its tail names a member of that module, so one spelling
    /// resolves to one declaration wherever it appears in the module. The checker resolves it once through the
    /// module member registry and records the proven identity here; lowering reads it to place the nominal type at
    /// its declaring module and must never rebuild that placement from the written segments. An absent entry means
    /// the checker did not prove a type and reported that at the annotation.
    pub qualified_type_references: HashMap<String, QualifiedTypeReferenceInfo>,
    /// The target of each non-generic type alias this module declares or imports, keyed by its local name.
    ///
    /// Lowering reads an imported alias's target here where the emitter cannot place the alias by its name, because
    /// another module of the crate declares a type of that name too (#1796).
    pub type_alias_targets: HashMap<String, ResolvedType>,
    /// RFC 120 identities of this module's own top-level declarations, keyed by declaration span.
    ///
    /// Exported from the symbol table's minting after checking as a compatibility view for span-keyed declaration
    /// consumers. Binding-aware consumers use [`Self::hir_bindings_by_span`], which also represents imports and
    /// aliases carrying a target's identity.
    pub declaration_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Canonical identities of source-owned declarations below module scope, keyed by declaration span.
    ///
    /// This is a complete checked declaration inventory, including bindings that have no reads. Stable identity
    /// projection uses it so adding the first use of an existing sibling cannot change another binding's ordinal.
    pub nested_declaration_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// RFC 120 identities of accepted source-owned member declarations, keyed by their declaration span.
    ///
    /// Fields, methods, properties, and enum variants do not occupy the module's ordinary lexical declaration map,
    /// but declaration-aware consumers still need their compiler-owned identity before any use site exists. This
    /// map is populated from the checked member registry after collision resolution; an absent entry is unproven and
    /// must never be reconstructed from an owner/name pair.
    pub member_declaration_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Checked source bindings introduced by each top-level declaration, in source binding order.
    ///
    /// This is the declaration-level HIR handoff. It preserves the local spelling separately from the canonical
    /// identity of the declaration it names, and it can represent every binding of one multi-item import without
    /// asking HIR to reinterpret import syntax. An absent identity is an explicit unproven result.
    pub hir_bindings_by_span: BTreeMap<crate::ast::Span, Vec<CheckedSourceBinding>>,
    /// Checked provider-operation declarations, keyed by their provider function's canonical identity.
    ///
    /// This is the producer-side fact that package publication persists. Body-IR lowering consumes the resulting
    /// provider-plan projection rather than re-reading a decorator or inferring an operation from a module name.
    pub provider_operations: BTreeMap<CanonicalSymbolId, ProviderOperationDeclarationInfo>,
    /// Checked RFC 104 capability declarations, keyed by their own canonical identity.
    ///
    /// The declaration is validated at its own site, so its resolved `requires` edges are already known there.
    /// Retaining them makes the authority contract inspectable as static facts: a consumer asking which capability
    /// a package declares, or what that capability itself needs, reads checked identities instead of re-resolving a
    /// dotted reference or parsing source (RFC 104, #1027).
    pub capabilities: BTreeMap<CanonicalSymbolId, CapabilityDeclarationInfo>,
    /// Module-local function declarations keyed by declaration span, preserving same-name overloads.
    pub function_bindings_by_span: HashMap<crate::ast::Span, FunctionBindingInfo>,
    /// Concrete class/model/trait method declarations keyed by declaration span (#1121).
    ///
    /// Method names are not unique the way top-level function names are: two owners can declare a method with the
    /// same name, and one owner can declare same-name overloads. Declaration span is therefore the only key that
    /// stays collision-safe without inventing a separate declaration-identity scheme, mirroring
    /// `function_bindings_by_span`. Body IR lowering (`loaves/compiler/incan_frontend/src/body_ir.rs`) consumes this
    /// instead of re-resolving raw AST parameter annotations, so aliased and generic method parameter types match
    /// the checked callable signature exactly rather than a local re-parse. Populated for every method checked
    /// through `TypeChecker::check_method_with_self_ty` (trait defaults included, keyed by the trait method's own
    /// span), so static methods (no receiver) are covered the same as instance methods. Newtype and enum methods
    /// are checked through the same function and so also populate this table, even though Body IR does not lower
    /// their bodies (#1102's own deliberate scope) — the fact simply goes unread for those owners.
    pub method_bindings_by_span: HashMap<crate::ast::Span, FunctionBindingInfo>,
    /// Body IR-only getter signatures and receiver provenance, keyed by the exact property declaration span.
    /// Legacy property metadata and callable binding tables remain unchanged.
    pub body_ir_property_bindings: HashMap<crate::ast::Span, BodyIrPropertyBindingInfo>,
    /// Overload candidates keyed by the source binding name visible in the current module.
    ///
    /// This includes declarations, imports, and aliases so call resolution, export metadata, and lowering all see one
    /// overload surface instead of rebuilding overload sets from syntax-specific paths.
    pub function_overloads: HashMap<String, Vec<FunctionOverloadInfo>>,
    /// Module-visible partial projections keyed by their source binding name.
    ///
    /// Constructor partials can be materialized in const contexts without calling the generated wrapper. Keeping the
    /// target and preset expressions here lets const-eval and lowering consume one resolved projection surface.
    pub partial_projections: HashMap<String, PartialProjectionInfo>,
    /// Same-type method aliases keyed by nominal type name (`alias -> target_method`).
    ///
    /// This includes imported type metadata so lowering can rewrite calls through aliases such as
    /// `Path.__truediv__` or `OrdinalMap.nbytes` even when the alias was declared in stdlib or a dependency module
    /// rather than the current source file.
    pub type_method_rebindings: HashMap<String, HashMap<String, String>>,
    /// RFC 036: Decorated function bindings keyed by declaration span, preserving same-name overloads.
    pub decorated_function_bindings_by_span: HashMap<crate::ast::Span, DecoratedFunctionBindingInfo>,
    /// Checked decorator applications whose sole parameter and result are the same generic type variable.
    ///
    /// This relation is captured before substitution, so native representation can follow the input callable
    /// without inferring representation identity from structurally equal result types.
    pub generic_identity_decorator_applications: HashSet<crate::ast::Span>,
    /// Validated decorator substitutions and ordered factory arguments retained exclusively for Body IR.
    /// Legacy keeps its original expression and call-site tables; this snapshot closes the factory only after its
    /// returned decorator has been checked against the decorated declaration.
    pub body_ir_decorator_applications: HashMap<crate::ast::Span, BodyIrDecoratorApplication>,
    /// Static receivers of checked two-segment decorator factories, retained separately from the selected method at
    /// the same synthetic span. Legacy reference and expression-type tables remain unchanged.
    pub body_ir_decorator_static_receivers: HashMap<crate::ast::Span, BodyIrDecoratorStaticReceiver>,
    /// RFC 036: Method names whose declaration was rebound through a user-defined decorator chain.
    pub decorated_method_bindings: HashMap<(String, String), DecoratedMethodBindingInfo>,
    /// Local function declarations that take a decorated method's receiver, keyed by declaration span (#1790).
    ///
    /// Source spells a method decorator's receiver the way the method does, `(Box, int) -> str`, and the checker
    /// proved each of these declarations takes it. Lowering passes the receiver to them the way the method's
    /// generated wrapper passes it.
    pub method_decorator_receiver_slots: HashMap<crate::ast::Span, MethodDecoratorReceiverSlot>,
    /// Whether each ordinary `mut` parameter of a function or method is marked, keyed by parameter span and name
    /// (#1790, #1773).
    ///
    /// A marked parameter's changes reach the caller; an unmarked one (an `int`, `float` or `bool`, also through an
    /// alias, or a Rust handle) is the function's own value. Lowering takes each `mut` parameter's passing mode from
    /// this fact, not from the parameter's IR type. The parameters of imported source modules a check collects are
    /// recorded too, for their trait defaults expanded into this module; the name keeps them apart from this module's
    /// parameters at the same offsets.
    pub mut_param_markers: HashMap<(crate::ast::Span, String), bool>,
}

/// A checked decorator application before substitution, with the bindings needed for its native instance.
#[derive(Debug, Clone)]
pub struct BodyIrDecoratorApplication {
    /// Returned decorator signature before applying the validated bindings.
    pub callable_type: ResolvedType,
    /// Generic variable bindings established by ordinary callable argument validation. A checked generic identity
    /// method application additionally retains its receiver's passing reference here, exclusively for Body IR.
    pub type_bindings: HashMap<String, ResolvedType>,
    /// Factory type arguments in the resolved declaration's parameter order, before application substitution.
    pub factory_type_args: Vec<ResolvedType>,
}

/// The checked static value a decorator factory method borrows, without conflating it with the selected method.
#[derive(Debug, Clone)]
pub struct BodyIrDecoratorStaticReceiver {
    /// Source binding used for this receiver; aliases keep their canonical storage identity below.
    pub binding_name: String,
    /// Canonical checked static declaration, including its provider module.
    pub identity: CanonicalSymbolId,
    /// Static carrier before the factory result overwrites the shared synthetic expression span.
    pub ty: ResolvedType,
}

/// Checked local inheritance facts for direct execution, without changing the legacy class or symbol tables.
#[derive(Debug, Clone)]
pub struct BodyIrClassInheritance {
    /// Canonical parent edge and nearest inherited source-method implementations.
    pub inheritance: incan_semantics_core::body_ir::ClassInheritance,
    /// Parent-first physical field identities, preserving each field's declaring class.
    pub field_identities: Vec<CanonicalSymbolId>,
}

/// Checked nominal conformance before Body IR projects declaration binders and final inferred clause arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrCheckedTraitAdoption {
    /// Actual trait declaration selected by the checked binding or provider member.
    pub trait_identity: CanonicalSymbolId,
    /// Checked arguments, retaining legacy's representation until Body IR applies its binder projection.
    pub arguments: Vec<ResolvedType>,
    /// Complete checked method identities; `None` cannot authorize an empty native marker.
    pub methods: Option<Vec<CanonicalSymbolId>>,
    /// Exact stored fields selected when checking this trait's required fields against the adopter.
    pub fields: Vec<incan_semantics_core::body_ir::TraitFieldImplementation>,
    /// Physical adoption or derive clause used to select the final Body IR-only inferred argument witness.
    pub span: Span,
}

/// A checked source trait slot introduced by a derive bundle; this does not supply its executable body.
#[derive(Debug, Clone)]
pub struct BodyIrDerivedMethod {
    /// Source slot identity retained from the resolved trait's method metadata.
    pub method: CanonicalSymbolId,
    /// Whether that resolved slot supplies a default body; consumers still require its retained source body.
    pub has_default: bool,
    /// Checked adoption arguments paired with the trait's declared parameters.
    pub type_arguments: Vec<(String, ResolvedType)>,
}

/// Where a local function declaration takes a decorated method's receiver, and how the method takes it (#1790).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodDecoratorReceiverSlot {
    /// Whether the decorated method takes `mut self`, so its receiver is spelled `mut Box` in the chain.
    pub mutable: bool,
    /// Which positions of the declaration's signature hold the receiver.
    pub role: MethodDecoratorReceiverRole,
}

/// The positions of a declaration's signature that hold a decorated method's receiver (#1790).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodDecoratorReceiverRole {
    /// A decorator, `def as_int(func: (Box, int) -> str) -> (Box, int) -> int`: the first parameter of the callable
    /// type it accepts first and of the callable type it returns.
    Decorator,
    /// A decorator factory, `def logged(label: str) -> (((Box, int) -> str) -> (Box, int) -> str)`: the same two
    /// positions inside the decorator shape it returns.
    Factory,
    /// A function a decorator returns in the method's place, `def parse(box: Box, value: int) -> int`: its first
    /// parameter.
    Replacement,
}

/// One active source binding exported for declaration-level HIR lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedSourceBinding {
    /// Spelling introduced in the current module.
    pub local_name: String,
    /// Canonical declaration identity proven for this binding, if any.
    pub canonical: Option<CanonicalSymbolId>,
}

/// One provider function's checked authority requirement.
///
/// The callable and capability are both resolved identities. The source decorator never contributes a stringly
/// authority name to a backend; it only tells the typechecker which two already-resolved declarations are related.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOperationDeclarationInfo {
    /// Canonical identity of the provider function this declaration annotates.
    pub operation: CanonicalSymbolId,
    /// Canonical identity of the RFC 104 capability required to invoke that operation.
    pub required_capability: CanonicalSymbolId,
    /// Compiler-owned requirements the operation's runtime implementation imposes.
    ///
    /// The first vertical currently records none. A future declaration contract may add requirements only when it
    /// has a checked source form; lowering must not infer them from a provider name or generated implementation.
    pub runtime_requirements: Vec<incan_semantics_core::AbiV0RuntimeRequirement>,
}

/// One checked RFC 104 capability declaration.
///
/// Every field is a resolved fact rather than source spelling. `requires` in particular holds the canonical
/// identities the declaration's references resolved to, not the dotted paths the author wrote: two packages can
/// spell a requirement the same way and mean different capabilities, so a consumer that compared spellings would
/// conflate them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDeclarationInfo {
    /// Canonical identity of the capability being declared.
    pub capability: CanonicalSymbolId,
    /// Prose from the `description` clause.
    pub description: Option<String>,
    /// Typed scope dimensions from the `scope:` block, in declaration order, as `(name, rendered type)`.
    pub scope: Vec<(String, String)>,
    /// Canonical identities of the capabilities this one declares it requires, in declaration order.
    ///
    /// A requirement that failed to resolve is absent: the reference already reported its own error, and recording a
    /// placeholder would let a consumer read an unproven edge as a checked one.
    pub requires: Vec<CanonicalSymbolId>,
    /// Whether the declaration is public to package consumers.
    pub is_public: bool,
}

/// Checked RFC 113 registry data that later stages consume without re-parsing decorator expressions.
#[derive(Debug, Default, Clone)]
pub struct RegistryArtifacts {
    /// Registry definitions keyed by their module-static binding name.
    pub definitions: HashMap<String, RegistryDefinitionInfo>,
    /// Canonical registry definitions imported into this module, keyed by the local import name.
    ///
    /// These are dependency facts, not duplicate local definitions. A checked `@describe` may use one only after the
    /// importer resolves the public static binding to the defining module's `Registry.define(...)` contract.
    pub imported_definitions: HashMap<String, ImportedRegistryDefinitionInfo>,
    /// Declaration descriptions in source order.
    pub descriptions: Vec<RegistryDescriptionInfo>,
    /// Explicit compilation-unit and package entries in source order.
    pub explicit_entries: Vec<RegistryExplicitEntryInfo>,
    /// The standard `Registry` class and `RegistryEntry` type, read from the standard package owning `std.registry`
    /// when this module defines, imports or enters a registry. Lowering selects the registry's reserved methods here.
    pub standard: Option<StandardRegistryTypes>,
}

/// The standard package's registry declarations a module's registry facts are checked against.
#[derive(Debug, Clone)]
pub struct StandardRegistryTypes {
    /// The `Registry` class's identity.
    pub registry: CanonicalSymbolId,
    /// The `Registry` class, with its methods.
    pub registry_class: ClassInfo,
    /// The `RegistryEntry` type's identity.
    pub entry: CanonicalSymbolId,
}

/// Type and subject contract declared by one `Registry.define(...)` static.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryDefinitionInfo {
    pub key_type: ResolvedType,
    pub descriptor_type: ResolvedType,
    pub subjects: Vec<SemanticRegistrySubjectKind>,
    /// Whether dependency consumers may inspect this registry through a published package artifact.
    pub is_public: bool,
}

/// A public registry definition imported from the source module that owns its `Registry.define(...)` declaration.
///
/// This fact carries the defining contract across an import boundary without inserting a second definition into the
/// consumer's [`RegistryArtifacts::definitions`] map. `owner_module_path` and `owner_binding` are the canonical
/// registry identity used by package metadata; `local_name` is intentionally only the consumer's spelling.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedRegistryDefinitionInfo {
    /// Contract checked from the defining static declaration.
    pub definition: RegistryDefinitionInfo,
    /// Canonical source module that owns the registry static.
    pub owner_module_path: Vec<String>,
    /// Canonical static binding in the owning source module.
    pub owner_binding: String,
}

/// Provenance of the canonical registry binding selected for one checked description.
///
/// Runtime lowering still uses the source-local `registry_name` to access the imported static. Metadata and semantic
/// facts use this reference so an imported alias cannot invent a second registry identity in the consumer module.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RegistryDescriptionRegistry {
    /// The described registry is declared by a static in this module.
    Local {
        /// Module-local static binding that owns the `Registry.define(...)` call.
        binding: String,
    },
    /// The described registry is a public static imported from its defining source module.
    Imported {
        /// Canonical source module containing the defining static.
        module_path: Vec<String>,
        /// Static binding in the canonical source module.
        binding: String,
        /// Public visibility checked on the canonical definition.
        public: bool,
    },
}

/// One checked `@describe` declaration attachment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegistryDescriptionInfo {
    /// Source-local binding that lowering reads to register the runtime descriptor.
    pub registry_name: String,
    /// Canonical definition selected for this description.
    pub registry: RegistryDescriptionRegistry,
    pub key: SemanticRegistryValue,
    pub descriptor: SemanticRegistryValue,
    pub subject_kind: SemanticRegistrySubjectKind,
    pub declaration_name: String,
    pub declaration_span: crate::ast::Span,
    /// Exact decorator attachment that produced this fact.
    ///
    /// Backend lowering uses this to require the frontend-approved artifact for the concrete syntax it materializes;
    /// it must not treat a raw `@describe` expression as semantic authority.
    pub decorator_span: crate::ast::Span,
    /// Source range of the checked key materialization expression.
    pub key_span: crate::ast::Span,
    /// Source range of the checked descriptor materialization expression.
    pub descriptor_span: crate::ast::Span,
}

/// One checked explicit compilation-unit or package registry entry.
///
/// The entry binding is a real source value. Its structural facts remain distinct from declaration decorators because
/// its subject is the defining compilation unit or package rather than the binding declaration itself.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegistryExplicitEntryInfo {
    pub registry_name: String,
    pub key: SemanticRegistryValue,
    pub descriptor: SemanticRegistryValue,
    pub subject_kind: SemanticRegistrySubjectKind,
    /// Exact source method approved for the declaration-only registry entry call.
    pub entry_method_identity: CanonicalSymbolId,
    /// Exact source constructor approved for the explicit subject expression.
    pub subject_constructor_identity: CanonicalSymbolId,
    /// Exact source-owned materializer selected by the frontend for backend substitution.
    pub checked_constructor_identity: CanonicalSymbolId,
    pub entry_name: String,
    pub declaration_span: crate::ast::Span,
    pub key_span: crate::ast::Span,
    pub subject_span: crate::ast::Span,
    pub descriptor_span: crate::ast::Span,
}

/// Typechecker-owned class layout consumed by backend struct lowering.
#[derive(Debug, Clone)]
pub struct ClassLayoutInfo {
    /// Whether the source class participates in the compiled library's public ABI.
    pub is_public: bool,
    /// Generic parameters that must remain unqualified inside Rust type displays.
    pub type_params: Vec<String>,
    /// Flattened fields in constructor ABI order: oldest ancestor first, then local fields.
    pub fields: Vec<ClassFieldLayoutInfo>,
    /// Ordered field names that violated the checked class-layout invariant.
    ///
    /// Lowering fails closed when this is non-empty instead of silently emitting a smaller Rust struct.
    pub missing_fields: Vec<String>,
    /// Defaults whose checked provider semantics cannot be materialized in a flattened consumer subclass.
    ///
    /// Typechecking diagnoses these at the class declaration. Lowering also fails closed if a caller attempts to use
    /// artifacts from an unsuccessful typecheck.
    pub unmaterializable_defaults: Vec<String>,
}

/// One checked class field retained across the frontend/backend boundary.
#[derive(Debug, Clone)]
pub struct ClassFieldLayoutInfo {
    /// Source-visible field name.
    pub name: String,
    /// Fully resolved field type selected by the typechecker.
    pub ty: ResolvedType,
    /// Source-level Incan spelling retained solely for reflection and documentation.
    pub surface_type_name: Option<String>,
    /// Checked source visibility, including visibility reconstructed from compiled manifests.
    pub visibility: Visibility,
    /// Checked initializer plan, retaining whether the expression belongs to this source unit or a compiled provider.
    pub default: Option<ClassFieldDefaultInfo>,
    /// Canonical source field alias used for construction and access.
    pub alias: Option<String>,
    /// Source-authored field description preserved for generated reflection metadata.
    pub description: Option<String>,
}

/// Checked origin and value for one class-field default.
#[derive(Debug, Clone)]
pub enum ClassFieldDefaultInfo {
    /// Source expression owned by the current package, including defaults inherited through source classes.
    Source(Spanned<Expr>),
    /// Manifest-safe expression owned by a compiled dependency.
    PublicDependency {
        /// Dependency key used to qualify constants and helper calls during consumer lowering.
        library: String,
        /// Canonical provider-owned default expression.
        value: CheckedParamDefault,
    },
}

/// One compiler-checked newtype construction plan shared by lowering and generated bridges.
#[derive(Debug, Clone, PartialEq)]
pub struct NewtypeConstructionInfo {
    /// Derives lowering adds automatically, beyond `Copy` for a `Copy` underlying type: `Debug` unless the underlying
    /// type is known to lack it, and `Clone` when the underlying type implements it (#1754).
    ///
    /// The checker's derive relation decides both, so a newtype implements what the checker says it does.
    pub automatic_derives: Vec<String>,
    /// Declared type parameters in source order.
    pub type_params: Vec<String>,
    /// Resolved wrapped value type, including references to the declared type parameters.
    pub underlying: ResolvedType,
    /// Canonical checked constructor selected by the typechecker, when present.
    pub checked_constructor: Option<String>,
    /// Exact declaration identity of `checked_constructor`, when source provenance is available.
    pub checked_constructor_identity: Option<CanonicalSymbolId>,
    /// Checked hook signature available to consumers even when its declaration span belongs to another module.
    pub checked_constructor_binding: Option<FunctionBindingInfo>,
    /// Compiler-generated constrained-primitive predicates used when no checked constructor exists.
    pub constraints: Vec<NewtypePrimitiveConstraint>,
    /// Whether ordinary implicit construction from the underlying value is allowed.
    pub implicit_coercion_enabled: bool,
    /// Whether the typechecker proved the newtype can participate in `TryFrom[str]` composition.
    pub supports_string_conversion: bool,
}

/// Source-level partial projection metadata preserved after collection.
#[derive(Debug, Clone)]
pub struct PartialProjectionInfo {
    /// Source name of the partial declaration visible in the current module.
    pub name: String,
    /// Resolved source path to the projected target.
    pub target_path: Vec<String>,
    /// Semantic kind of the projected target when collection could classify it.
    pub target_kind: PartialProjectionTargetKind,
    /// Preset keyword expressions supplied by the partial declaration.
    pub presets: Vec<PartialProjectionPreset>,
}

/// One preset keyword/value pair from a partial declaration.
#[derive(Debug, Clone)]
pub struct PartialProjectionPreset {
    pub name: String,
    pub value: Spanned<Expr>,
}

/// Target kinds that matter to downstream partial projection consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialProjectionTargetKind {
    Function,
    ModelConstructor,
    ClassConstructor,
    NewtypeConstructor,
    Unknown,
}

/// One checked public declaration reached through explicit source facade links at an otherwise unresolved call.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrReexportCallInfo {
    /// The manifest's minted declaring identity, never the facade or consumer spelling.
    pub canonical: CanonicalSymbolId,
    /// Exact fixed scalar parameters whose written arguments validated without coercion or inference.
    pub parameters: Vec<CallableParam>,
}

/// Call-site semantic decisions selected by the typechecker.
#[derive(Debug, Default, Clone)]
pub struct CallArtifacts {
    /// Body IR-only layout requirements for checked constructor field arguments, keyed by the written field span.
    /// Each pair retains the selected field and constructor owner; navigation keeps the field identity while
    /// executable dependency selection loads its checked layout instead of treating private storage as a callable.
    pub body_ir_constructor_field_requirements: HashMap<crate::ast::Span, (CanonicalSymbolId, CanonicalSymbolId)>,
    /// Validated fixed scalar calls through source re-exports ending at a checked public library declaration.
    /// Only Body IR consumes their target and slots; legacy retains its unresolved module placeholder.
    pub body_ir_reexport_calls: HashMap<crate::ast::Span, BodyIrReexportCallInfo>,
    /// Primitive string slice members selected from checked receiver and int bounds. Only Body IR consumes these
    /// identities; legacy retains its unknown method result and ordinary reference tables.
    pub body_ir_string_slice_members: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Body IR-only calls whose selected instance method declares an exact Self result, keyed by call span.
    /// The retained canonical method prevents a speculative or subsequently replaced overload from authorizing
    /// receiver-carrier propagation; legacy keeps its existing substituted expression result unchanged.
    pub body_ir_self_result_methods: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Validated async function and method calls, keyed by complete call span and selected canonical callable. Body IR
    /// uses their eventual output as a future argument; legacy expression snapshots retain the eventual output alone.
    pub body_ir_async_call_targets: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Body IR-only tuple constructor facts selected by an active std.web surface binding. Legacy keeps its existing
    /// construction table; the canonical owner is checked from the original Rust re-export at this call site.
    pub body_ir_native_constructor_bindings: HashMap<crate::ast::Span, (CanonicalSymbolId, ConstructorFieldBinding)>,
    /// Complete default-only class bindings checked through a published module's selected type declaration.
    /// The canonical owner authenticates this private fallback; legacy's constructor slot table stays unchanged.
    pub body_ir_module_constructor_bindings: HashMap<crate::ast::Span, (CanonicalSymbolId, ConstructorFieldBinding)>,
    /// Native surface payload types missing from legacy's expression snapshot. An already checked String symbol or
    /// literal authorizes Html's fixed String slot; legacy expression typing remains unchanged.
    pub body_ir_native_argument_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Checked variable identities accompanying native payload types when legacy did not visit the argument. This
    /// selects the existing parameter/local, never a same-spelled external or a new semantic binding.
    pub body_ir_native_argument_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Complete argument types for Body IR native instances, normalized from checked call relationships only.
    ///
    /// Concrete constructor carriers and ordinary checked arguments share this ordered fact. Legacy never reads this
    /// additive table: its own contextual types and monomorph decisions remain untouched.
    pub body_ir_call_type_arguments: HashMap<crate::ast::Span, Vec<ResolvedType>>,
    /// Closed call-site parameters under the same checked instantiation, preserving declaration roles and defaults.
    /// Only Body IR reads this contract; legacy parameter snapshots retain their original inferred holes.
    pub body_ir_call_parameter_types: HashMap<crate::ast::Span, Vec<CallableParam>>,
    /// Checked static factory constraints that conflict with their destination. Only Body IR reads this denial;
    /// later supplemental value propagation cannot authorize a closed result at the same call span.
    pub body_ir_factory_result_conflicts: HashSet<crate::ast::Span>,
    /// The exact nominal owner closed by a checked static factory, independent of wrappers in its return type.
    pub body_ir_factory_owner_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Executable constructor carriers recovered from original declared fields and checked values after contextual
    /// checking substituted a callee's open parameters into those fields. Only Body IR reads this additive fact.
    pub body_ir_constructor_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Executable call results under the same retained argument binding, consumed only by Body IR.
    pub body_ir_call_result_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Selected frozen codec members whose legacy registry omits an identity. This independent Body IR fact is
    /// recorded only at the existing checked builtin branch; legacy reference and method tables stay unchanged.
    pub body_ir_native_codec_members: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Concrete owner types inferred from checked nominal constructor fields before an open caller context masks them.
    ///
    /// Body IR retains these executable carriers while legacy checking preserves its contextual expression type.
    pub inferred_constructor_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Executable result types closed by a checked constructor argument at an inferred generic call.
    ///
    /// These accompany the retained call instantiation; legacy contextual expression types remain unchanged.
    pub inferred_call_result_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Method calls, by full call span, whose receiver's source type declares the method only with `mut self`.
    ///
    /// Lowering marks the binding at the root of such a call's receiver as borrowed mutably, so every later question
    /// about whether a body changes that binding, such as whether a `for` loop must reach its items in place, sees the
    /// call wherever it sits in the body (#1561). A method that trait dispatch selects carries its receiver in the
    /// dispatch fact instead.
    pub mutable_receiver_method_calls: HashSet<crate::ast::Span>,
    /// Compiler-owned builtin selected for a call, keyed by the full call span.
    ///
    /// This distinguishes an explicit `std.builtins.name(...)` or unshadowed ambient builtin from a source/import
    /// declaration with the same spelling without asking lowering to reconstruct name resolution.
    pub resolved_builtin_calls: HashMap<crate::ast::Span, BuiltinFnId>,
    /// RFC 038: unpack operands whose static shape has been proven by call binding.
    ///
    /// Lowering consumes these plans to rewrite fixed/static unpack operands into ordinary IR call arguments. This
    /// keeps backend emission from re-deriving the frontend's binding decision from raw IR shape.
    pub fixed_unpack_plans: HashMap<crate::ast::Span, FixedUnpackPlan>,
    /// Map a full call expression span to its checked type arguments in declaration order.
    ///
    /// Functions retain both inferred and explicit bindings; methods retain explicit bindings (RFC 054). Body IR
    /// consumes these decisions for native instantiation. Legacy emission consumes them only for written `[...]`,
    /// preserving Rust inference on other calls and ensuring `_` never reaches codegen as `IrType::Unknown`.
    ///
    /// ## Span stability
    ///
    /// Checker and Body IR retain the same source range and expansion context. Generated calls at one original
    /// location therefore keep independent bindings without changing diagnostic coordinates.
    pub call_site_monomorph_type_args: HashMap<crate::ast::Span, Vec<ResolvedType>>,
    /// Checked target facts for compiler-owned `isinstance(value, Target)` calls, keyed by the full call span.
    ///
    /// `Target` is source syntax for a type rather than a runtime argument. Retaining the resolved type, optional
    /// declaration identity, and target-local span here lets Body IR represent the test without reparsing a name or
    /// asking a runtime to materialize arbitrary type values.
    pub isinstance_targets: HashMap<crate::ast::Span, IsInstanceTargetInfo>,
    /// RFC 038: Rest-aware callable signatures keyed by full call expression span.
    ///
    /// Function-value calls can recover this from the callee expression type, but method calls need a snapshot because
    /// lowering does not retain the frontend method table.
    pub call_site_callable_params: HashMap<crate::ast::Span, Vec<CallableParam>>,
    /// Resolved winner-binding type for one `race for` arm, keyed by that arm's awaitable expression span (#1164).
    ///
    /// A race arm binds the shared `race for value:` name to the *awaited output* type, not to the awaitable's own
    /// type: `Awaitable[T]` binds `T`, and `JoinHandle[T]` binds `Result[T, TaskJoinError]`. Only the typechecker
    /// performs that unwrapping (`await_output_type`). The shared header span identifies the source binding, while
    /// the awaitable span distinguishes each arm's refined type, so that type is recorded here instead of re-derived.
    pub race_arm_binding_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Resolved `model`/`class` construction field binding, keyed by full call expression span (#1158).
    ///
    /// Source-level construction is named-only, so the constructor check is the stage that decides which declared
    /// field each written argument fills — including resolving a field alias to its canonical name. Recording that
    /// decision here is what lets Body IR lowering represent construction faithfully: declared field order lives in
    /// the symbol table (`ModelInfo::field_order`/`ClassInfo::field_order`), which lowering deliberately cannot
    /// reach, and re-deriving the binding from AST spelling would duplicate alias resolution in a second place.
    pub constructor_field_bindings: HashMap<crate::ast::Span, ConstructorFieldBinding>,
    /// RFC 028: User-defined operator dispatch resolved by the typechecker.
    ///
    /// Lowering consumes this map so `a + b`, `-a`, and `a[b]` can become direct dunder method calls without
    /// re-running backend-side infix/index semantics. Primitive operators are intentionally absent from this map.
    pub resolved_operator_calls: HashMap<crate::ast::Span, ResolvedOperatorCall>,
    /// Trait-backed method dispatch selected by overload resolution.
    ///
    /// Lowering consumes this for calls whose selected method lives in a trait impl rather than an inherent Rust impl.
    /// This keeps codegen from re-deriving dispatch from method names or argument shapes.
    pub resolved_method_calls: HashMap<crate::ast::Span, ResolvedMethodCall>,
    /// Compiler-generated member identities observed at checked call sites.
    ///
    /// These helpers retain owner-discriminated semantic identities for tooling, but they are not source declarations
    /// and therefore must not receive an RFC 120 recoverable source-symbol projection during lowering. Each entry
    /// retains its canonical nominal owner as the context dependency used by executable admission.
    pub compiler_generated_member_identities: HashMap<CanonicalSymbolId, CanonicalSymbolId>,
    /// Collection constructors selected from the canonical collection vocabulary.
    ///
    /// Lowering consumes this decision instead of interpreting a source spelling such as `set(...)` as an ordinary
    /// function call or independently guessing whether a same-named binding shadows the collection constructor.
    pub resolved_collection_constructors: HashMap<crate::ast::Span, CollectionTypeId>,
    /// Selected runtime-string helper methods, keyed by full call expression span.
    ///
    /// This carries the typechecker's canonical [`StringMethodId`] through Body IR so a consumer can select an
    /// admitted helper operation from a resolved identity rather than rediscovering a method target from source text.
    pub resolved_string_helper_calls: HashMap<crate::ast::Span, StringMethodId>,
    /// Function-typed parameters of this module's functions and methods that hold any callable of their type, since
    /// the body only calls them, keyed by the parameter's span (#1561).
    ///
    /// Lowering spells each such parameter `impl Fn(A) -> R`, which holds a closure that captures local values as well
    /// as a named function; any other function-typed parameter is a function pointer.
    pub closure_holding_params: HashSet<crate::ast::Span>,
    /// Functions and methods of this module whose one returned value is a closure that captures local values, keyed by
    /// the span of their declared function return type, which lowering spells `impl Fn(A) -> R` (#1561).
    pub closure_returning_callables: HashSet<crate::ast::Span>,
    /// New local bindings whose value is a closure that captures local values, a local partial or the closure a
    /// closure-returning function returns, keyed by the assignment statement span (#1561).
    ///
    /// The binding takes the value's own type, so lowering does not spell the binding's function-type annotation, and
    /// spells the parameter types of a closure bound there instead.
    pub capturing_callable_bindings: HashSet<crate::ast::Span>,
    /// Exact source string methods selected for display operands. Body IR emits these as ordinary shared-receiver
    /// calls; native Rust formatting never supplies a source method by guesswork.
    pub source_display_methods: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Types this module declares whose values display through `message()`, keyed by type name.
    ///
    /// A model, class, enum or newtype that adopts `Error` and has no `Display` of its own displays its `message()`,
    /// so it satisfies a `Display` bound. The value is the `message()` call the checker resolved on the type, which
    /// lowering makes the body of the Rust `Display` it gives the type.
    pub error_message_display_types: HashMap<String, ErrorMessageDisplay>,
}

/// The `message()` call one displayed `Error` adopter renders through (#1778).
///
/// Resolved by the checker exactly as a written `value.message()` call: through the adopter's own method, a trait
/// default it inherits, or the `Error` bound of a type parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorMessageDisplay {
    /// Canonical identity of the selected `message` declaration, when declaration provenance retained one.
    pub identity: Option<CanonicalSymbolId>,
    /// Trait dispatch the call needs, when the selected `message` is reached through a trait.
    pub dispatch: Option<ResolvedMethodDispatch>,
    /// Whether the operand is `self` inside a default method of a trait that extends `Error`.
    ///
    /// Such a display is checked once against the trait's `Self`, like a written `self.message()` there, which the
    /// checker resolves no further; lowering expands the default into each adopter.
    pub receiver_is_trait_self: bool,
    /// For a trait-`Self` display, the adopters in this module that have a `Display` of their own and so keep it
    /// where the default is expanded into them. Empty for every other display.
    pub self_displaying_adopters: BTreeSet<String>,
}

/// Typechecker-owned meaning of one `isinstance` target expression.
#[derive(Debug, Clone, PartialEq)]
pub struct IsInstanceTargetInfo {
    /// Alias-expanded semantic target type.
    pub ty: ResolvedType,
    /// Canonical declaration identity for a nominal target, when resolution proved one.
    ///
    /// Compiler-owned primitives and aliases expanded to primitives need no declaration identity. An absent identity
    /// on another target is a visible downstream refusal, never permission to dispatch from its source spelling.
    pub canonical: Option<CanonicalSymbolId>,
    /// Original source range of the target expression, including parentheses when they were written.
    pub span: Span,
}

/// Custom protocol decisions that lower into explicit runtime calls.
#[derive(Debug, Default, Clone)]
pub struct ProtocolArtifacts {
    /// Receiver value identities retained only for Body IR before synthetic truthiness dispatch reuses their spans.
    /// The legacy reference at that span still selects `__bool__`; this separate record never changes that table.
    pub(crate) body_ir_truthiness_receivers: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Independent checked hook calls for Body IR, separate from the legacy iteration record and written call facts.
    pub body_ir_iterations: HashMap<crate::ast::Span, BodyIrIterationHooksInfo>,
    /// RFC 068: Custom `for` iteration protocol choices keyed by iterable expression span.
    ///
    /// Lowering consumes this so a structural `__iter__` / `__next__` pair can become an explicit loop that calls the
    /// resolved hooks without relying on Rust's `IntoIterator`.
    pub iterations: HashMap<crate::ast::Span, ProtocolIterationInfo>,
    /// `for` loops that take the items out of the list binding they iterate, keyed by iterable expression span
    /// (#1844).
    ///
    /// The loop body hands on by value items that can be neither copied nor cloned, so lowering iterates the list by
    /// value and owns each item. The checker refuses every read of the list inside the loop or after it, until an
    /// assignment that no branch, `break` or `continue` can skip gives the binding a new list, a closure that captured
    /// the list before the loop, and a repeat of the loop that would reach the emptied list.
    pub item_taking_iterations: HashSet<crate::ast::Span>,
    /// Comprehension collection sources whose direct identifier read is consumed, keyed by iterable expression span
    /// (#1983).
    ///
    /// Both outcomes are recorded: `true` makes lowering move the collection and iterate its owned elements, while
    /// `false` keeps the collection and requires the borrowed-item iteration plan. Recording the negative outcome is
    /// essential because it prevents lowering from running a second last-use heuristic that could disagree with the
    /// checker about whether cloning an element is required.
    pub comprehension_source_consumption: HashMap<crate::ast::Span, bool>,
    /// Scrutinee spans of the `match`, `if let` and `while let` forms whose pattern binds a view into a caller-visible
    /// `mut` parameter that an arm changes (#1561).
    ///
    /// The change has to reach the parameter, so lowering matches such a scrutinee in place, through a mutable
    /// reference, and binds each pattern name to the part of the parameter it names rather than to a copy.
    pub in_place_match_scrutinees: HashSet<crate::ast::Span>,
}

/// A typechecker-resolved user-defined operator call consumed by IR lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOperatorCall {
    /// The concrete dunder method name selected by frontend method/trait dispatch.
    pub method: String,
    /// The AST operator shape this call replaces.
    pub kind: ResolvedOperatorKind,
}

/// Metadata for one imported Rust trait binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustTraitImportInfo {
    /// Canonical import path used by Incan for this trait binding.
    pub trait_path: String,
    /// Resolved Rust definition path after re-export resolution, when available.
    pub definition_path: Option<String>,
    /// Method names this trait can place in Rust method-lookup scope.
    pub methods: HashSet<String>,
    /// Whether `methods` is the trait's declared surface, from inspected metadata or the compiler's fallback trait
    /// vocabulary.
    ///
    /// An import with neither has an unknown surface: the compiler cannot say which methods it provides, or even
    /// that it is a trait. Such an import stays a candidate for any method call that no inspected surface resolves,
    /// so the generated `use` survives when Rust method lookup may need it (#1450).
    pub methods_known: bool,
    /// Method signatures this trait metadata provided, keyed by method name.
    pub method_signatures: HashMap<String, RustFunctionSig>,
}

/// A typechecker-resolved Rust extension-trait import required by a method call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustMethodTraitImportUse {
    /// Local import binding to retain in generated Rust.
    pub binding: String,
    /// Trait path selected for the call.
    pub trait_path: String,
    /// Method name observed at the call site.
    pub method: String,
    /// Trait method signature, when metadata supplied it.
    pub signature: Option<RustFunctionSig>,
}

/// A typechecker-resolved method call consumed by IR lowering.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMethodCall {
    /// The concrete source-level method name selected by frontend method/trait dispatch.
    pub method: String,
    /// How the backend should emit this method call.
    pub dispatch: ResolvedMethodDispatch,
}

/// Semantic dispatch target for a resolved method call.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedMethodDispatch {
    /// Preserve the selected trait owner and receiver contract for downstream lowering.
    Trait {
        /// Source-level trait name selected by semantic resolution.
        trait_name: String,
        /// Canonical source module that owns the trait, when the import boundary supplies one.
        module_path: Option<Vec<String>>,
        /// Concrete trait type arguments selected by overload resolution.
        type_args: Vec<ResolvedType>,
        /// Compiler-resolved generic header attached to the exact selected implementation.
        implementation_type_params: Vec<ImplementationTypeParamInfo>,
        /// Whether the selected trait method declares `mut self`.
        receiver_is_mutable: bool,
    },
}

/// The facts method resolution records for one call at the call's span: the method call it selected, the declaration it
/// reaches, the parameters it binds its arguments to and the type arguments it instantiates.
///
/// Taken with [`TypeCheckInfo::take_call_site_facts`] and put back with [`TypeCheckInfo::restore_call_site_facts`], so
/// a call the checker resolves on the program's behalf at a source expression's span, such as a `for` loop's
/// `__iter__` and `__next__` hooks at its iterable's span, leaves the facts of the call written there as they were.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CallSiteFacts {
    checked_rust_call: Option<RustCheckedCall>,
    checked_rust_function_value: Option<RustCheckedCall>,
    body_ir_reexport_call: Option<BodyIrReexportCallInfo>,
    body_ir_string_slice_member: Option<CanonicalSymbolId>,
    inferred_constructor_type: Option<ResolvedType>,
    inferred_call_result_type: Option<ResolvedType>,
    method_call: Option<ResolvedMethodCall>,
    identity: Option<CanonicalSymbolId>,
    callable_params: Option<Vec<CallableParam>>,
    monomorph_type_args: Option<Vec<ResolvedType>>,
    body_ir_constructor_type: Option<ResolvedType>,
    body_ir_call_type_arguments: Option<Vec<ResolvedType>>,
    body_ir_call_result_type: Option<ResolvedType>,
    body_ir_factory_owner_type: Option<ResolvedType>,
    body_ir_async_call_target: Option<CanonicalSymbolId>,
    body_ir_self_result_method: Option<CanonicalSymbolId>,
    body_ir_self_method_identity: Option<CanonicalSymbolId>,
    body_ir_self_method_result_type: Option<ResolvedType>,
    body_ir_self_method_params: Option<Vec<CallableParam>>,
    body_ir_inferred_method_identity: Option<CanonicalSymbolId>,
}

/// Exact checker-selected calls behind one structural or fallible iteration; only Body IR consumes this record.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrIterationHooksInfo {
    /// Call that materializes the iterator before the first poll. None transfers a checked owned source iterator
    /// directly into its polling frame; it is already its own iteration state.
    pub iter: Option<BodyIrProtocolCallInfo>,
    /// Call that advances the materialized iterator inside the loop.
    pub next: BodyIrProtocolCallInfo,
}

/// Retained signature and identity for one synthetic hook, never inferred by a backend from its spelling.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrProtocolCallInfo {
    /// Exact source or active SDK declaration; absent remains a named refusal.
    pub canonical: Option<CanonicalSymbolId>,
    /// Exact checked trait instantiation for this Body IR-only hook snapshot; the written call's facts stay intact.
    pub dispatch: Option<ResolvedMethodDispatch>,
    /// Checked ordinary parameters, excluding the receiver, including their default-presence contract.
    pub params: Option<Vec<CallableParam>>,
    /// Checked method-scoped instantiation arguments, independent of the written iterable's generic call.
    pub type_args: Vec<ResolvedType>,
    /// Checked result after receiver and trait arguments have been applied.
    pub return_type: ResolvedType,
    /// Declared instance receiver mode; an absent or ambiguous receiver remains refused.
    pub receiver_is_mutable: Option<bool>,
}

/// Typechecker-resolved custom iteration protocol consumed by IR lowering.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolIterationInfo {
    /// Method selected on the iterable expression.
    pub iter_method: String,
    /// Concrete iterator object type returned from `__iter__`.
    pub iterator_type: ResolvedType,
    /// Method selected on the iterator object.
    pub next_method: String,
    /// Element type unwrapped from `__next__() -> Option[T]`.
    pub item_type: ResolvedType,
    /// Exact semantic dispatch selected for `__iter__`, when the hook belongs to an adopted trait.
    pub iter_dispatch: Option<ResolvedMethodDispatch>,
    /// Exact semantic dispatch selected for `__next__`, when the hook belongs to an adopted trait.
    pub next_dispatch: Option<ResolvedMethodDispatch>,
    /// Error type propagated by `for item in iterable?`, when this is a fallible iteration route.
    pub fallible_error_type: Option<ResolvedType>,
}

/// Lowering metadata for one RFC 046 computed property read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputedPropertyAccessInfo {
    pub owner_type: String,
    pub property: String,
}

/// Operator expression shape for a resolved user-defined operator call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedOperatorKind {
    Binary,
    Unary,
    Index,
    /// `obj[start:end:step]` on a type that defines `__getslice__` (`Sliceable[T]`).
    Slice,
    IndexAssign,
    Truthiness,
    Len,
    Contains,
    Call,
}

/// Typechecker-proven call-unpack shape consumed by IR lowering.
#[derive(Debug, Clone, PartialEq)]
pub enum FixedUnpackPlan {
    /// `*expr` has a statically known ordered shape with one type per contributed positional item.
    Positional(Vec<ResolvedType>),
    /// `**expr` has statically known string keys in source order.
    Keyword(Vec<String>),
}

/// How an identifier expression resolved in the symbol table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentKind {
    /// A value binding (variable/field), or a callable value (function).
    Value,
    /// A module static binding.
    Static,
    /// A type name (models/classes/enums/newtypes).
    TypeName,
    /// An enum variant constructor identifier.
    Variant,
    /// A module-like namespace (e.g. imported module placeholders).
    Module,
    /// A Rust import placeholder (`import rust::...` / `from rust::... import ...`).
    RustImport,
    /// A Rust value import, such as a public Rust constant.
    RustValue,
    /// A trait name (may be used as a type-like namespace).
    Trait,
}

/// One module-qualified type annotation (`mod.Type`) resolved to the declaration it names.
///
/// Each part serves a different consumer. The identity is the RFC 120 proof of which declaration was selected, for
/// reference-site facts. The module path is the checked path the spelling walked -- the module binding's resolved
/// path plus any nested segments -- which is where lowering places the nominal reference, since this module holds no
/// import binding for the bare type name; it is the same path a call through the binding is emitted against. The
/// resolved type is what a direct import of the same declaration would have bound, so a value checked against the
/// qualified spelling is compatible with one checked against the imported name.
#[derive(Debug, Clone, PartialEq)]
pub struct QualifiedTypeReferenceInfo {
    /// Canonical identity of the declaration the qualified spelling names.
    pub identity: CanonicalSymbolId,
    /// Checked module path the spelling reached the declaration through (`["std", "toml"]`, `["pub", "widgets"]`).
    pub module_path: Vec<String>,
    /// The nominal type the annotation denotes, exactly as a direct import of the declaration resolves it.
    pub resolved: ResolvedType,
}

/// Compiler-proven source declaration target for codegraph call/reference records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTargetInfo {
    /// Import path segments **as written** at the reference site.
    ///
    /// Not necessarily the module that owns the declaration: resolution tries sibling-relative candidates before bare
    /// ones, so this can name a different module that merely declares the same leaf name. The proven owner lives in
    /// [`DeclarationArtifacts::resolved_import_identities`].
    pub module_path: Vec<String>,
    /// Source declaration name in the owning module.
    pub name: String,
    /// Source declaration kind, matching the codegraph declaration `kind` spelling.
    pub kind: String,
}

impl SourceTargetInfo {
    /// Return whether this recorded declaration target is the canonical function kind.
    pub fn is_function(&self) -> bool {
        SemanticSourceTargetKind::from_kind_str(&self.kind) == SemanticSourceTargetKind::Function
    }
}

/// Coercion category selected by the typechecker for a Rust-boundary call argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustArgCoercionKind {
    /// Builtin boundary matrix coercion (`i16 -> i64`, `str -> &str`, ...).
    Builtin(CoercionPolicy),
    /// Rusttype alias can flow to its backing Rust type without an explicit adapter call.
    RustTypeUnwrap,
    /// Rusttype alias uses a declared `interop:` adapter edge.
    RustTypeInterop,
    /// The Rust enum variant stores this payload as `Box<T>`; the argument is the semantic `T` and lowering boxes it.
    BoxPayload,
    /// Rust metadata requires a concrete reference, so preserve its borrow shape during emission.
    Borrow {
        /// Whether Rust requires an exclusive mutable borrow.
        mutable: bool,
    },
    /// Rust metadata requires a trait-object reference, so preserve the required borrow shape at emission.
    TraitObjectBorrow {
        /// Whether Rust requires an exclusive `&mut dyn Trait` borrow.
        mutable: bool,
    },
}

/// Lowering metadata for one Rust-boundary call argument.
#[derive(Debug, Clone, PartialEq)]
pub struct RustArgCoercionInfo {
    /// Normalized Rust parameter type display from metadata (e.g. `f32`, `&str`).
    pub rust_target_type: String,
    /// Resolved target type for lowering IR typing.
    pub target_type: ResolvedType,
    /// Coercion strategy to apply.
    pub kind: RustArgCoercionKind,
}

/// One typechecker-approved validated-newtype coercion chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedNewtypeCoercionInfo {
    /// Ordered underlying-to-target conversion steps.
    pub steps: Vec<ValidatedNewtypeCoercionStep>,
    /// Final target type after all steps.
    pub target_type: ResolvedType,
    /// Runtime failure strategy selected for the coercion site.
    pub mode: ValidatedNewtypeCoercionMode,
}

/// Runtime failure behavior for one validated-newtype coercion site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedNewtypeCoercionMode {
    /// Ordinary sites panic on the first validation error.
    FailFast,
    /// Model/class constructor fields collect this field's validation error before the constructor fails.
    AggregateField { field_name: String },
}

/// One conversion step in a validated-newtype coercion chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedNewtypeCoercionStep {
    /// Newtype being constructed by this step.
    pub newtype_name: String,
    /// Checker-selected declaration owning the tuple storage, including a signature-only imported newtype.
    pub owner_identity: Option<CanonicalSymbolId>,
    /// Canonical validation hook to call. `None` means direct newtype wrapping is sufficient.
    pub ctor: Option<String>,
    /// Exact source declaration selected for `ctor`, when the hook has compiler-owned identity metadata.
    ///
    /// Lowering uses this to select the RFC 120 physical projection without reconstructing declaration provenance
    /// from the conventional `from_underlying` spelling.
    pub ctor_identity: Option<CanonicalSymbolId>,
    /// Generated constrained-primitive predicates to enforce before direct wrapping.
    pub constraints: Vec<NewtypePrimitiveConstraint>,
}

/// Lowering metadata for one source function or method declaration.
///
/// Shared by [`DeclarationArtifacts::function_bindings`]/[`DeclarationArtifacts::function_bindings_by_span`] (top-
/// level `def`) and [`DeclarationArtifacts::method_bindings_by_span`] (class/model/trait methods, #1121) rather than
/// duplicating an equivalent struct per callable kind.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBindingInfo {
    /// Invoking this declaration constructs a lazy future whose output is `return_type`.
    pub is_async: bool,
    /// Typechecker-resolved source parameters, including default-presence markers.
    pub params: Vec<CallableParam>,
    /// Typechecker-resolved source return type.
    pub return_type: ResolvedType,
    /// RFC 120 canonical identity of this declaration, minted once when the binding is recorded.
    ///
    /// This is the declaration-side fact Body IR lowering consumes for `NamedCallableTarget::canonical` instead of
    /// re-deriving an identity from module path plus spelling. Span-keyed entries always carry it for local
    /// declarations (each overload keeps its own); name-keyed *imported* entries carry the declaring module's proven
    /// identity or `None` when import resolution could not prove one — absent is never permission to reconstruct.
    pub identity: Option<CanonicalSymbolId>,
}

/// Checked facts needed to retain a property as a canonical immutable getter on the direct route.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrPropertyBindingInfo {
    /// Zero-argument callable signature, excluding the implicit receiver, under the property declaration's identity.
    pub callable: FunctionBindingInfo,
    /// Declared owner frame, whose parameters close through a checked concrete receiver at a read.
    pub receiver_type: ResolvedType,
    /// Ordered binders declared by that owner, never inferred from the getter's result spelling.
    pub owner_parameters: Vec<String>,
    /// Exact implicit receiver binding created when the getter body is checked; absent for abstract requirements.
    pub receiver_identity: Option<CanonicalSymbolId>,
}

/// Typechecker-resolved binding of one `model`/`class` construction's arguments to the declared field layout, or a
/// newtype's single positional argument to tuple slot zero.
///
/// Slots index the type's declared field order. `argument_slots` is in **written source order**, so a consumer sees
/// both which field each argument fills and the order the argument expressions were written — the two facts that
/// differ whenever a caller writes fields out of declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorFieldBinding {
    /// Declared field slot filled by each written argument, in written source order.
    pub argument_slots: Vec<usize>,
    /// Declared field slots the call site omitted, ascending. Each takes its declared field default.
    pub defaulted_slots: Vec<usize>,
    /// Total declared field count, i.e. the construction's declaration-order arity.
    pub field_count: usize,
}

/// Lowering metadata for one RFC 036 decorated function binding.
#[derive(Debug, Clone, PartialEq)]
pub struct DecoratedFunctionBindingInfo {
    /// Final type of the module-visible binding after applying all user-defined decorators.
    pub ty: ResolvedType,
    /// Original callable type before decorators are applied.
    pub original_ty: ResolvedType,
    /// Source-declared type parameters preserved for explicit and inferred call-site generic arguments.
    pub type_params: Vec<String>,
    /// Explicit source-declared bounds per type parameter.
    pub type_param_bounds: HashMap<String, Vec<String>>,
    /// Resolved source-declared bounds, preserving generic type arguments.
    pub type_param_bound_details: HashMap<String, Vec<TypeBoundInfo>>,
    /// Whether the original declaration is async.
    pub is_async: bool,
}

/// Lowering metadata for one RFC 036 decorated method binding.
#[derive(Debug, Clone, PartialEq)]
pub struct DecoratedMethodBindingInfo {
    /// Final unbound callable type after applying all user-defined decorators. The receiver is the first parameter.
    pub unbound_ty: ResolvedType,
    /// Original unbound callable type before decorators are applied. The receiver is the first parameter.
    pub original_unbound_ty: ResolvedType,
}

impl DeclarationArtifacts {
    /// Return whether the `mut` parameter declared at `span` as `name` is marked, when the checker recorded it.
    pub fn mut_param_marker(&self, span: Span, name: &str) -> Option<bool> {
        self.mut_param_markers.get(&(span, name.to_string())).copied()
    }
}

impl TypeCheckInfo {
    /// Return whether the method call at `span` calls a source method whose receiver is `mut self`.
    pub fn method_call_takes_mutable_receiver(&self, span: Span) -> bool {
        self.calls.mutable_receiver_method_calls.contains(&span)
    }

    /// Return the checked source path associated with one active import-derived binding.
    pub fn import_binding_path(&self, local_name: &str) -> Option<&[String]> {
        self.import_bindings.path(local_name)
    }

    /// Export a backend-neutral fact snapshot for consumers that should not depend on typed AST or Rust IR shapes.
    ///
    /// This is the first bridge into the v0.5 semantic fact store. It deliberately reuses facts that the typechecker
    /// has already proven, keyed by source spans, and avoids introducing a separate semantic authority.
    pub fn semantic_fact_store(&self, module_path: &[String]) -> SemanticFactStore {
        self.semantic_fact_store_with_package(module_path, None)
    }

    /// Export a backend-neutral fact snapshot with an optional canonical package identity.
    ///
    /// Package names are a compilation-session concern, not something a typechecker can safely reconstruct from one
    /// module path. Command boundaries that know the manifest package must call this form so package registry subjects
    /// retain their real identity. The legacy no-package form remains for module-only consumers and deliberately uses
    /// a visibly synthetic fallback rather than pretending the module name is the package name.
    pub fn semantic_fact_store_with_package(
        &self,
        module_path: &[String],
        package_identity: Option<&str>,
    ) -> SemanticFactStore {
        let module_identity = semantic_module_identity(module_path);
        let mut facts = Vec::new();
        let declarations = self.semantic_reference_declarations();
        self.record_semantic_type_facts(&module_identity, &mut facts);
        Self::record_semantic_declaration_facts(&module_identity, &declarations, &mut facts);
        self.record_semantic_reference_facts(&module_identity, &declarations, &mut facts);
        self.record_semantic_iteration_facts(&module_identity, &declarations, &mut facts);
        self.record_semantic_registry_facts(&module_identity, package_identity, &mut facts);
        facts.sort();
        let mut store = SemanticFactStore::new();
        for fact in facts {
            store.insert(fact);
        }
        store
    }

    /// Retain the checker's expression and function types without resolving source syntax a second time.
    fn record_semantic_type_facts(&self, module_identity: &str, facts: &mut Vec<SemanticFact>) {
        for (&span, ty) in &self.expressions.expr_types {
            facts.push(SemanticFact::new(
                CompilerNodeId::expression_at(module_identity, span),
                SemanticFactKind::Type,
                SemanticFactValue::semantic_type(self.semantic_type(ty)),
            ));
        }

        for (&span, target) in &self.expressions.source_targets {
            facts.push(SemanticFact::new(
                CompilerNodeId::expression_at(module_identity, span),
                SemanticFactKind::SymbolTarget,
                SemanticFactValue::source_target(semantic_source_target_from_typecheck(target)),
            ));
        }
        for (name, binding) in &self.declarations.function_bindings {
            facts.push(SemanticFact::new(
                CompilerNodeId::declaration(module_identity, name),
                SemanticFactKind::Type,
                SemanticFactValue::semantic_type(
                    self.qualify_semantic_type(semantic_type_from_function_binding(binding)),
                ),
            ));
        }
    }

    /// Select physical checked declarations that can own references; generated members retain their nominal owner.
    fn semantic_reference_declarations(&self) -> BTreeSet<&CanonicalSymbolId> {
        self.declarations
            .declaration_identities
            .values()
            .chain(self.declarations.member_declaration_identities.values())
            .filter(|identity| !self.is_compiler_generated_member_identity(identity))
            .filter(|identity| {
                !matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Local
                        | SemanticSourceTargetKind::Parameter
                        | SemanticSourceTargetKind::Receiver
                        | SemanticSourceTargetKind::GenericBinder
                        | SemanticSourceTargetKind::Module
                        | SemanticSourceTargetKind::Builtin
                        | SemanticSourceTargetKind::RustItem
                        | SemanticSourceTargetKind::Other(_)
                )
            })
            .collect()
    }

    /// Record declaration identities and the containing owner required by a field or enum variant.
    fn record_semantic_declaration_facts(
        module_identity: &str,
        declarations: &BTreeSet<&CanonicalSymbolId>,
        facts: &mut Vec<SemanticFact>,
    ) {
        for identity in declarations {
            facts.push(SemanticFact::new(
                CompilerNodeId::declaration_at(module_identity, identity.declaration_span),
                SemanticFactKind::DeclarationIdentity,
                SemanticFactValue::canonical_identity((*identity).clone()),
            ));
            if matches!(
                identity.kind,
                SemanticSourceTargetKind::Field | SemanticSourceTargetKind::Variant
            ) && let Some(owner) = incan_semantics_core::dependencies::closest_declaring_owner(
                declarations.iter().copied().filter(|candidate| *candidate != *identity),
                identity.declaration_span,
            ) {
                facts.push(SemanticFact::new(
                    CompilerNodeId::declaration_at(module_identity, identity.declaration_span),
                    SemanticFactKind::RequiredMemberOwner,
                    SemanticFactValue::canonical_identity(owner.clone()),
                ));
            }
        }
    }

    /// Give nested bindings stable owner-relative ordinals without using process-local scope identifiers.
    fn stable_declaration_contexts(
        &self,
        declarations: &BTreeSet<&CanonicalSymbolId>,
    ) -> BTreeMap<CanonicalSymbolId, CanonicalStableDeclarationContext> {
        let mut stable_context_groups = BTreeMap::<
            (CanonicalSymbolId, SymbolNamespace, SemanticSourceTargetKind, String),
            BTreeSet<CanonicalSymbolId>,
        >::new();
        for identity in self.declarations.nested_declaration_identities.values() {
            if identity.scope_discriminant.is_none() {
                continue;
            }
            let Some(owner) = incan_semantics_core::dependencies::closest_declaring_owner(
                declarations.iter().copied(),
                identity.declaration_span,
            ) else {
                continue;
            };
            stable_context_groups
                .entry((
                    owner.clone(),
                    identity.namespace,
                    identity.kind.clone(),
                    identity.declaration_name.clone(),
                ))
                .or_default()
                .insert(identity.clone());
        }
        let mut stable_contexts = BTreeMap::new();
        for ((owner, _, _, _), identities) in stable_context_groups {
            let mut identities = identities.into_iter().collect::<Vec<_>>();
            identities.sort_by_key(|identity| identity.declaration_span);
            for (binding_ordinal, identity) in identities.into_iter().enumerate() {
                if let Ok(binding_ordinal) = u32::try_from(binding_ordinal) {
                    stable_contexts.insert(
                        identity,
                        CanonicalStableDeclarationContext {
                            owner: owner.clone(),
                            binding_ordinal,
                        },
                    );
                }
            }
        }
        stable_contexts
    }

    /// Project written references and nominal type leaves once for both dependency execution and inspection.
    fn record_semantic_reference_facts(
        &self,
        module_identity: &str,
        declarations: &BTreeSet<&CanonicalSymbolId>,
        facts: &mut Vec<SemanticFact>,
    ) {
        let stable_contexts = self.stable_declaration_contexts(declarations);
        for (&span, identity) in &self.references.resolved_identities {
            let subject = CompilerNodeId::expression_at(module_identity, span);
            Self::record_semantic_reference(facts, &subject, span, identity, declarations);
            if let Some(context) = stable_contexts.get(identity) {
                facts.push(SemanticFact::new(
                    subject.clone(),
                    SemanticFactKind::StableDeclarationContext,
                    SemanticFactValue::stable_declaration_context(context.clone()),
                ));
            }
            if let Some(owner) = self.body_ir_reference_requirement(span, identity) {
                facts.push(SemanticFact::new(
                    subject.clone(),
                    SemanticFactKind::RequiredReferenceTarget,
                    SemanticFactValue::canonical_identity(owner.clone()),
                ));
            }
        }

        for (&span, identities) in &self.expressions.expression_type_identities {
            let source_span = span;
            for (indices, identity) in identities {
                let subject = CompilerNodeId::expression_type_component_at(module_identity, span, indices);
                Self::record_semantic_reference(facts, &subject, source_span, identity, declarations);
            }
        }
    }

    /// Keep each synthetic iteration call's canonical target separate from the written iterable's call facts.
    /// Dependency loading must see the selected defaults before Body IR lowers the loop into calls.
    fn record_semantic_iteration_facts(
        &self,
        module_identity: &str,
        declarations: &BTreeSet<&CanonicalSymbolId>,
        facts: &mut Vec<SemanticFact>,
    ) {
        for (&span, hooks) in &self.protocols.body_ir_iterations {
            for (role, hook) in [("iter", hooks.iter.as_ref()), ("next", Some(&hooks.next))] {
                let Some(hook) = hook else {
                    continue;
                };
                let Some(identity) = &hook.canonical else {
                    continue;
                };
                let subject = CompilerNodeId::protocol_call_at(module_identity, span, role);
                Self::record_semantic_reference(facts, &subject, span, identity, declarations);
            }
        }
    }

    /// Attach one exact canonical target and its unique nearest owner, shared by written and synthetic references.
    fn record_semantic_reference(
        facts: &mut Vec<SemanticFact>,
        subject: &CompilerNodeId,
        span: incan_semantics_core::HirSourceSpan,
        identity: &CanonicalSymbolId,
        declarations: &BTreeSet<&CanonicalSymbolId>,
    ) {
        facts.push(SemanticFact::new(
            subject.clone(),
            SemanticFactKind::ReferenceSpan,
            SemanticFactValue::SourceSpan(span),
        ));
        facts.push(SemanticFact::new(
            subject.clone(),
            SemanticFactKind::SymbolIdentity,
            SemanticFactValue::canonical_identity(identity.clone()),
        ));
        if let Some(owner) =
            incan_semantics_core::dependencies::closest_declaring_owner(declarations.iter().copied(), span)
        {
            facts.push(SemanticFact::new(
                subject.clone(),
                SemanticFactKind::ReferenceOwner,
                SemanticFactValue::canonical_identity(owner.clone()),
            ));
        }
    }

    /// Retain registry subjects from their checked declarations and the session's explicit package identity.
    fn record_semantic_registry_facts(
        &self,
        module_identity: &str,
        package_identity: Option<&str>,
        facts: &mut Vec<SemanticFact>,
    ) {
        for description in &self.registry.descriptions {
            let registry = match &description.registry {
                RegistryDescriptionRegistry::Local { binding } => CompilerNodeId::declaration(module_identity, binding),
                RegistryDescriptionRegistry::Imported {
                    module_path, binding, ..
                } => CompilerNodeId::declaration(&module_path.join("::"), binding),
            };
            facts.push(SemanticFact::new(
                CompilerNodeId::declaration(module_identity, &description.declaration_name),
                SemanticFactKind::Registry,
                SemanticFactValue::registry_entry(SemanticRegistryEntry {
                    registry,
                    key: description.key.clone(),
                    descriptor: description.descriptor.clone(),
                    subject_kind: description.subject_kind,
                    subject_identity: format!("{module_identity}.{}", description.declaration_name),
                }),
            ));
        }

        for entry in &self.registry.explicit_entries {
            let (subject, subject_identity) = match entry.subject_kind {
                SemanticRegistrySubjectKind::CompilationUnit => {
                    (CompilerNodeId::module(module_identity), module_identity.to_owned())
                }
                SemanticRegistrySubjectKind::Package => {
                    let package_identity = package_identity
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{module_identity}::package"));
                    (CompilerNodeId::package(&package_identity), package_identity)
                }
                SemanticRegistrySubjectKind::Function | SemanticRegistrySubjectKind::Method => {
                    continue;
                }
            };
            facts.push(SemanticFact::new(
                subject,
                SemanticFactKind::Registry,
                SemanticFactValue::registry_entry(SemanticRegistryEntry {
                    registry: CompilerNodeId::declaration(module_identity, &entry.registry_name),
                    key: entry.key.clone(),
                    descriptor: entry.descriptor.clone(),
                    subject_kind: entry.subject_kind,
                    subject_identity,
                }),
            ));
        }
    }

    /// Return the resolved type recorded for the expression at `span`, if any.
    pub fn expr_type(&self, span: Span) -> Option<&ResolvedType> {
        self.expressions.expr_types.get(&span)
    }

    /// Admit registry-owned task, race and synchronization carriers without granting an equal-spelled source owner
    /// any SDK authority. Both checking and Body IR use this same provenance boundary.
    pub(crate) fn runtime_owner_admitted(&self, id: surface_types::SurfaceTypeId) -> bool {
        if !matches!(
            surface_types::category(id),
            SurfaceTypeCategory::AsyncTask | SurfaceTypeCategory::AsyncRace | SurfaceTypeCategory::AsyncSync
        ) {
            return false;
        }
        let Some(path) = surface_types::stdlib_module_path(id) else {
            return false;
        };
        let path = path.split('.').map(str::to_owned).collect::<Vec<_>>();
        let name = surface_types::as_str(id);
        let Some(identity) = self
            .declarations
            .named_type_identities
            .get(name)
            .or_else(|| self.declarations.body_ir_native_type_identities.get(name))
        else {
            return true;
        };
        match &identity.origin {
            SymbolOrigin::Module(owner) => owner == &path,
            SymbolOrigin::Package { library, module_path } => {
                library == "incan_stdlib_async" && module_path == &path[1..]
            }
            SymbolOrigin::RustCrate(owner) => surface_types::from_runtime_rust_path(&owner.join("::")) == Some(id),
            _ => false,
        }
    }

    /// Project a checked type without erasing its nominal owners. Carrier identities come only from the checker's
    /// accepted bindings; an unresolved name remains unresolved rather than borrowing another module's declaration.
    pub fn semantic_type(&self, ty: &ResolvedType) -> IncanType {
        self.qualify_semantic_type(semantic_type_from_resolved(ty))
    }

    /// Retain checker-owned nominal heads and leaves after an active declaration's binders have been classified.
    /// Body IR consumes this result directly and must not rebuild identities from normalized source names.
    pub fn qualify_semantic_type(&self, ty: IncanType) -> IncanType {
        match ty {
            IncanType::Named(name) => {
                let identity = self
                    .declarations
                    .named_type_identities
                    .get(&name)
                    .or_else(|| self.declarations.body_ir_native_type_identities.get(&name));
                let Some(identity) = identity else {
                    return IncanType::Named(name);
                };
                let Some(key) = incan_semantics_core::body_ir::nominal_carrier_key(identity) else {
                    return IncanType::Named(identity.declaration_name.clone());
                };
                if self
                    .declarations
                    .named_type_identities
                    .get(&key)
                    .is_some_and(|binding| binding != identity)
                {
                    return IncanType::Unsupported("conflicting checked nominal carrier binding".into());
                }
                IncanType::Named(key)
            }
            IncanType::Generic { base, args } => match self.qualify_semantic_type(IncanType::Named(base)) {
                IncanType::Named(base) => IncanType::Generic {
                    base,
                    args: args.into_iter().map(|ty| self.qualify_semantic_type(ty)).collect(),
                },
                refused => refused,
            },
            IncanType::Tuple(items) => {
                IncanType::Tuple(items.into_iter().map(|ty| self.qualify_semantic_type(ty)).collect())
            }
            IncanType::Ref(inner) => IncanType::Ref(Box::new(self.qualify_semantic_type(*inner))),
            IncanType::NativeArray { element, length } => IncanType::NativeArray {
                element: Box::new(self.qualify_semantic_type(*element)),
                length,
            },
            IncanType::RefMut(inner) => IncanType::RefMut(Box::new(self.qualify_semantic_type(*inner))),
            IncanType::TypeToken(inner) => IncanType::TypeToken(Box::new(self.qualify_semantic_type(*inner))),
            IncanType::Function {
                mut params,
                return_type,
            } => {
                for parameter in &mut params {
                    parameter.ty = self.qualify_semantic_type(parameter.ty.clone());
                }
                IncanType::Function {
                    params,
                    return_type: Box::new(self.qualify_semantic_type(*return_type)),
                }
            }
            ty => ty,
        }
    }

    /// Return the final compiler-selected type of a binding introduced by an assignment statement.
    pub fn assignment_binding_type(&self, span: Span) -> Option<&ResolvedType> {
        self.expressions.assignment_binding_types.get(&span)
    }

    /// Return the type of the place the value at `span` is written to, if the checker recorded one because the value
    /// needs `Some` layers or a numeric widening there (#1858, RFC 009).
    pub fn value_destination_type(&self, span: Span) -> Option<&ResolvedType> {
        self.expressions.value_destination_types.get(&span)
    }

    /// Return computed-property metadata for a field-access expression, if that access resolved to a property.
    pub fn computed_property_access(&self, span: Span) -> Option<&ComputedPropertyAccessInfo> {
        self.expressions.computed_property_accesses.get(&span)
    }

    /// Record that a field-access expression resolved to a computed property read.
    pub fn record_computed_property_access(&mut self, span: Span, owner_type: &str, property: &str) {
        self.expressions.computed_property_accesses.insert(
            span,
            ComputedPropertyAccessInfo {
                owner_type: owner_type.to_string(),
                property: property.to_string(),
            },
        );
    }

    /// Return the RFC 038 fixed/static unpack plan recorded for an unpack operand, if any.
    pub fn fixed_unpack_plan(&self, span: Span) -> Option<&FixedUnpackPlan> {
        self.calls.fixed_unpack_plans.get(&span)
    }

    /// Return how the identifier expression at `span` resolved in the symbol table.
    pub fn ident_kind(&self, span: Span) -> Option<IdentKind> {
        self.expressions.ident_kinds.get(&span).copied()
    }

    /// Return the canonical identity proven for an imported binding, if import resolution proved one.
    ///
    /// Absent means unproven, never "assume the written import path": see
    /// [`DeclarationArtifacts::resolved_import_identities`].
    pub fn resolved_import_identity(&self, local_name: &str) -> Option<&CanonicalSymbolId> {
        self.declarations.resolved_import_identities.get(local_name)
    }

    /// Select the executable identity of a checked call while preserving a module binding's navigation identity.
    /// A matching imported function refines that module binding; otherwise an existing selected target wins over a
    /// validated reexport fallback. An absent call-site identity stays absent; only checking may publish a proven
    /// import selection at a successful call site. Source shadows never inherit an import's authority.
    pub fn resolved_call_identity<'a>(
        &'a self,
        name: &str,
        span: Span,
        reexport: Option<&'a CanonicalSymbolId>,
    ) -> Option<&'a CanonicalSymbolId> {
        let selected = self.resolved_identity(span);
        let imported = self.resolved_import_identity(name);
        match (selected, imported) {
            (Some(module), Some(function))
                if module.kind == SemanticSourceTargetKind::Module
                    && function.kind == SemanticSourceTargetKind::Function
                    && module.origin == function.origin =>
            {
                Some(function)
            }
            _ => selected.or(reexport),
        }
    }

    /// Return the declaration a module-qualified type annotation resolved to, keyed by its dotted spelling.
    ///
    /// Absent means the checker did not prove a type for that spelling and reported it at the annotation; lowering
    /// must not fall back to the written segments: see [`DeclarationArtifacts::qualified_type_references`].
    pub fn qualified_type_reference(&self, spelling: &str) -> Option<&QualifiedTypeReferenceInfo> {
        self.declarations.qualified_type_references.get(spelling)
    }

    /// Return the target of a non-generic type alias this module declares or imports.
    pub fn type_alias_target(&self, name: &str) -> Option<&ResolvedType> {
        self.declarations.type_alias_targets.get(name)
    }

    /// Return a compiler-proven source target for the expression at `span`, if one was recorded.
    pub fn source_target(&self, span: Span) -> Option<&SourceTargetInfo> {
        self.expressions.source_targets.get(&span)
    }

    /// Return the RFC 120 canonical identity resolved for the reference at `span`, if resolution proved one.
    ///
    /// Absent means unproven — never permission to rebuild an identity from the reference's spelling or from
    /// [`Self::source_target`], which is a string-shaped codegraph projection rather than an identity.
    pub fn resolved_identity(&self, span: Span) -> Option<&CanonicalSymbolId> {
        self.references.resolved_identities.get(&span)
    }

    /// Record the RFC 120 canonical identity proven for one source reference.
    ///
    /// Expression and type-reference checking share this write boundary so consumers never need to know which AST
    /// category produced the reference fact. Callers must pass an identity obtained from the resolved symbol; this
    /// method deliberately does not reconstruct identities from source spellings.
    pub fn record_resolved_identity(&mut self, span: Span, identity: CanonicalSymbolId) {
        self.references.resolved_identities.insert(span, identity);
    }

    /// Return the canonical binding selected for a statement-owned write target.
    pub fn resolved_write_identity(&self, span: Span, name: &str) -> Option<&CanonicalSymbolId> {
        self.references.resolved_write_identities.get(&(span, name.to_string()))
    }

    /// Return the checked type of a statement-owned write target.
    pub fn resolved_write_type(&self, span: Span, name: &str) -> Option<&ResolvedType> {
        self.references.resolved_write_types.get(&(span, name.to_string()))
    }

    /// Record the canonical binding selected for a statement-owned write target.
    pub fn record_resolved_write_identity(
        &mut self,
        span: Span,
        name: &str,
        identity: CanonicalSymbolId,
        ty: ResolvedType,
    ) {
        let key = (span, name.to_string());
        self.references.resolved_write_identities.insert(key.clone(), identity);
        self.references.resolved_write_types.insert(key, ty);
    }

    /// Return whether the identifier at `span` resolved to the ambient `std.logging` logger binding.
    pub fn is_ambient_logger_binding(&self, span: Span) -> bool {
        self.expressions.ambient_logger_bindings.contains(&span)
    }

    /// Record that an identifier resolved to the ambient `std.logging` logger binding.
    pub fn record_ambient_logger_binding(&mut self, span: Span) {
        self.expressions.ambient_logger_bindings.insert(span);
    }

    /// Return whether the `dict.get(key)` call at `span` only has its result read (see
    /// [`ExpressionArtifacts::read_only_dict_lookups`]).
    pub fn is_read_only_dict_lookup(&self, span: Span) -> bool {
        self.expressions.read_only_dict_lookups.contains(&span)
    }

    /// Record that the `dict.get(key)` call at `span` only has its result read.
    pub fn record_read_only_dict_lookup(&mut self, span: Span) {
        self.expressions.read_only_dict_lookups.insert(span);
    }

    /// Return partial projection metadata for a visible partial binding, if collection recorded one.
    pub fn partial_projection(&self, local_name: &str) -> Option<&PartialProjectionInfo> {
        self.declarations.partial_projections.get(local_name)
    }

    /// Record partial projection metadata for a visible partial binding.
    pub fn record_partial_projection(&mut self, projection: PartialProjectionInfo) {
        self.declarations
            .partial_projections
            .insert(projection.name.clone(), projection);
    }

    /// Return overload candidates for one source binding, if any.
    pub fn function_overloads(&self, local_name: &str) -> Option<&[FunctionOverloadInfo]> {
        self.declarations.function_overloads.get(local_name).map(Vec::as_slice)
    }

    /// Record overload candidates for one source binding.
    pub fn record_function_overloads(&mut self, local_name: String, overloads: Vec<FunctionOverloadInfo>) {
        self.declarations.function_overloads.insert(local_name, overloads);
    }

    /// Return the computed const value for `name`, when const evaluation succeeded.
    pub fn const_value(&self, name: &str) -> Option<&ConstValue> {
        self.consts.const_values.get(name)
    }

    /// Return the recorded Rust-boundary argument coercion for the expression at `span`, if any.
    pub fn rust_arg_coercion(&self, span: Span) -> Option<&RustArgCoercionInfo> {
        self.rust.arg_coercions.get(&span)
    }

    /// Return the validated-newtype coercion recorded for the expression at `span`, if any.
    pub fn validated_newtype_coercion(&self, span: Span) -> Option<&ValidatedNewtypeCoercionInfo> {
        self.expressions.validated_newtype_coercions.get(&span)
    }

    /// Record a typechecker-approved validated-newtype coercion for a source expression span.
    pub fn record_validated_newtype_coercion(&mut self, span: Span, info: ValidatedNewtypeCoercionInfo) {
        self.expressions.validated_newtype_coercions.insert(span, info);
    }

    /// Return rest-aware callable metadata recorded for the full call expression span, if any.
    pub fn call_site_callable_params(&self, span: Span) -> Option<&[CallableParam]> {
        self.calls.call_site_callable_params.get(&span).map(Vec::as_slice)
    }

    /// Return closed checked call parameters for Body IR, preserving the legacy signature when no complete fact exists.
    pub fn body_ir_call_site_callable_params(&self, span: Span) -> Option<&[CallableParam]> {
        self.calls
            .body_ir_call_parameter_types
            .get(&span)
            .map(Vec::as_slice)
            .or_else(|| self.call_site_callable_params(span))
    }

    /// Return the resolved winner-binding type for the `race for` arm whose awaitable is at `span` (#1164).
    pub fn race_arm_binding_type(&self, span: Span) -> Option<&ResolvedType> {
        self.calls.race_arm_binding_types.get(&span)
    }

    /// Record the resolved winner-binding type for one `race for` arm (#1164).
    pub fn record_race_arm_binding_type(&mut self, span: Span, ty: ResolvedType) {
        self.calls.race_arm_binding_types.insert(span, ty);
    }

    /// Return the resolved `model`/`class` construction field binding recorded for `span`, if any (#1158).
    pub fn constructor_field_binding(&self, span: Span) -> Option<&ConstructorFieldBinding> {
        self.calls.constructor_field_bindings.get(&span)
    }

    /// Return checked ordinary, module-default or native tuple slots only to Body IR, leaving legacy's getter
    /// unchanged. A module fallback must still match the exact canonical owner selected at this call.
    pub fn body_ir_constructor_field_binding(&self, span: Span) -> Option<&ConstructorFieldBinding> {
        self.constructor_field_binding(span)
            .or_else(|| {
                self.calls
                    .body_ir_module_constructor_bindings
                    .get(&span)
                    .filter(|(owner, _)| self.resolved_identity(span) == Some(owner))
                    .map(|(_, binding)| binding)
            })
            .or_else(|| {
                self.calls
                    .body_ir_native_constructor_bindings
                    .get(&span)
                    .map(|(_, binding)| binding)
            })
    }

    /// The exact foreign owner checked for a native surface construction; ordinary source calls retain their
    /// existing expression identity instead.
    pub fn body_ir_native_constructor_identity(&self, span: Span) -> Option<&CanonicalSymbolId> {
        self.calls
            .body_ir_native_constructor_bindings
            .get(&span)
            .map(|(owner, _)| owner)
    }

    /// Record a validated native tuple construction in the direct-route table only, never in legacy's slot table.
    pub fn record_body_ir_native_constructor(&mut self, span: Span, owner: CanonicalSymbolId) {
        self.calls.body_ir_native_constructor_bindings.insert(
            span,
            (
                owner,
                ConstructorFieldBinding {
                    argument_slots: vec![0],
                    defaulted_slots: Vec::new(),
                    field_count: 1,
                },
            ),
        );
    }

    /// Record the resolved field binding for one nominal construction call site, including a newtype's tuple slot
    /// (#1158).
    pub fn record_constructor_field_binding(&mut self, span: Span, binding: ConstructorFieldBinding) {
        self.calls.constructor_field_bindings.insert(span, binding);
    }

    /// Return whether the function-typed parameter declared at `span` holds any callable of its type, so lowering
    /// spells it `impl Fn(A) -> R` (#1561).
    pub fn is_closure_holding_param(&self, span: Span) -> bool {
        self.calls.closure_holding_params.contains(&span)
    }

    /// Return whether the function or method whose function return type is written at `span` returns a closure that
    /// captures local values, so lowering spells the return type `impl Fn(A) -> R` (#1561).
    pub fn is_closure_returning_type(&self, span: Span) -> bool {
        self.calls.closure_returning_callables.contains(&span)
    }

    /// Return whether the assignment statement at `span` binds a new local to a capturing callable, whose own type the
    /// binding takes (#1561).
    pub fn binds_capturing_callable(&self, span: Span) -> bool {
        self.calls.capturing_callable_bindings.contains(&span)
    }

    /// Select checked executable context without changing the reference's navigation identity.
    /// Constructor requirements apply only while the exact checked field still occupies this reference site;
    /// replacing its binding cannot reuse an earlier constructor's layout authorization.
    fn body_ir_reference_requirement(
        &self,
        span: crate::ast::Span,
        identity: &CanonicalSymbolId,
    ) -> Option<&CanonicalSymbolId> {
        self.calls
            .compiler_generated_member_identities
            .get(identity)
            .or_else(|| {
                self.calls
                    .body_ir_constructor_field_requirements
                    .get(&span)
                    .filter(|(field, _)| field == identity)
                    .map(|(_, owner)| owner)
            })
    }

    /// Return whether `identity` names a compiler-generated member rather than a source declaration.
    pub fn is_compiler_generated_member_identity(&self, identity: &CanonicalSymbolId) -> bool {
        self.calls.compiler_generated_member_identities.contains_key(identity)
    }

    /// Preserve a generated member's exact declaring nominal as its executable context requirement.
    pub fn record_compiler_generated_member_identity(&mut self, identity: CanonicalSymbolId, owner: CanonicalSymbolId) {
        self.calls.compiler_generated_member_identities.insert(identity, owner);
    }

    /// Return the canonical collection constructor selected for one source call.
    pub fn resolved_collection_constructor(&self, span: Span) -> Option<CollectionTypeId> {
        self.calls.resolved_collection_constructors.get(&span).copied()
    }

    /// Return the checked target fact for one compiler-owned `isinstance` call.
    pub fn isinstance_target(&self, call_span: Span) -> Option<&IsInstanceTargetInfo> {
        self.calls.isinstance_targets.get(&call_span)
    }

    /// Return the compiler-owned builtin selected for one checked call.
    pub fn resolved_builtin_call(&self, call_span: Span) -> Option<BuiltinFnId> {
        self.calls.resolved_builtin_calls.get(&call_span).copied()
    }

    /// Return the `message()` call a declared type displays through, or `None` when it has a `Display` of its own or
    /// does not adopt `Error`.
    ///
    /// See [`CallArtifacts::error_message_display_types`].
    pub fn error_message_display_type(&self, type_name: &str) -> Option<&ErrorMessageDisplay> {
        self.calls.error_message_display_types.get(type_name)
    }

    /// Record the `message()` call a declared type displays through.
    pub fn record_error_message_display_type(&mut self, type_name: &str, display: ErrorMessageDisplay) {
        self.calls
            .error_message_display_types
            .insert(type_name.to_string(), display);
    }

    /// Record the compiler-owned builtin selected for one checked call.
    pub fn record_resolved_builtin_call(&mut self, call_span: Span, builtin: BuiltinFnId) {
        self.calls.resolved_builtin_calls.insert(call_span, builtin);
    }

    /// Record the checked target fact for one compiler-owned `isinstance` call.
    pub fn record_isinstance_target(&mut self, call_span: Span, target: IsInstanceTargetInfo) {
        self.calls.isinstance_targets.insert(call_span, target);
    }

    /// Record the canonical collection constructor selected for one source call.
    pub fn record_resolved_collection_constructor(&mut self, span: Span, constructor: CollectionTypeId) {
        self.calls.resolved_collection_constructors.insert(span, constructor);
    }

    /// Return the selected runtime-string helper identity for one source call, if it is in the admitted subset.
    pub fn resolved_string_helper_call(&self, span: Span) -> Option<StringMethodId> {
        self.calls.resolved_string_helper_calls.get(&span).copied()
    }

    /// Record a selected runtime-string helper identity for later Body-IR lowering.
    pub fn record_resolved_string_helper_call(&mut self, span: Span, method: StringMethodId) {
        self.calls.resolved_string_helper_calls.insert(span, method);
    }

    /// Record callable metadata needed by lowering when the callee expression alone cannot carry it.
    pub fn record_call_site_callable_params(&mut self, span: Span, params: &[CallableParam]) {
        if params
            .iter()
            .any(|param| param.kind != ParamKind::Normal || callable_param_needs_boundary_snapshot(&param.ty))
        {
            self.calls.call_site_callable_params.insert(span, params.to_vec());
        }
    }

    /// Record exact callable metadata when overload/source-method resolution selected a concrete callable.
    pub fn record_call_site_callable_params_exact(&mut self, span: Span, params: &[CallableParam]) {
        self.calls.call_site_callable_params.insert(span, params.to_vec());
    }

    /// Record callable metadata required by an explicit lowered dispatch path.
    pub fn record_call_site_callable_params_for_dispatch(&mut self, span: Span, params: &[CallableParam]) {
        self.calls.call_site_callable_params.insert(span, params.to_vec());
    }

    /// Return a typechecker-resolved user-defined operator call for `span`, if any.
    pub fn resolved_operator_call(&self, span: Span) -> Option<&ResolvedOperatorCall> {
        self.calls.resolved_operator_calls.get(&span)
    }

    /// Return a typechecker-resolved method call for `span`, if any.
    pub fn resolved_method_call(&self, span: Span) -> Option<&ResolvedMethodCall> {
        self.calls.resolved_method_calls.get(&span)
    }

    /// Return custom iteration protocol metadata for `span`, if any.
    pub fn protocol_iteration(&self, span: Span) -> Option<&ProtocolIterationInfo> {
        self.protocols.iterations.get(&span)
    }

    /// Record a user-defined operator call that lowering should emit as a direct dunder method call.
    pub fn record_resolved_operator_call(&mut self, span: Span, method: impl Into<String>, kind: ResolvedOperatorKind) {
        self.calls.resolved_operator_calls.insert(
            span,
            ResolvedOperatorCall {
                method: method.into(),
                kind,
            },
        );
    }

    /// Record a resolved method dispatch that lowering should preserve explicitly.
    pub fn record_resolved_method_call(
        &mut self,
        span: Span,
        method: impl Into<String>,
        dispatch: ResolvedMethodDispatch,
    ) {
        self.calls.resolved_method_calls.insert(
            span,
            ResolvedMethodCall {
                method: method.into(),
                dispatch,
            },
        );
    }

    /// Remove and return the facts method resolution recorded for the call at `span` (see [`CallSiteFacts`]).
    pub(crate) fn take_call_site_facts(&mut self, span: Span) -> CallSiteFacts {
        let key = span;
        CallSiteFacts {
            checked_rust_call: self.rust.checked_calls.remove(&key),
            checked_rust_function_value: self.rust.checked_function_values.remove(&key),
            body_ir_reexport_call: self.calls.body_ir_reexport_calls.remove(&key),
            body_ir_string_slice_member: self.calls.body_ir_string_slice_members.remove(&key),
            inferred_constructor_type: self.calls.inferred_constructor_types.remove(&key),
            inferred_call_result_type: self.calls.inferred_call_result_types.remove(&key),
            method_call: self.calls.resolved_method_calls.remove(&key),
            identity: self.references.resolved_identities.remove(&key),
            callable_params: self.calls.call_site_callable_params.remove(&key),
            monomorph_type_args: self.calls.call_site_monomorph_type_args.remove(&key),
            body_ir_constructor_type: self.calls.body_ir_constructor_types.remove(&key),
            body_ir_call_type_arguments: self.calls.body_ir_call_type_arguments.remove(&key),
            body_ir_call_result_type: self.calls.body_ir_call_result_types.remove(&key),
            body_ir_factory_owner_type: self.calls.body_ir_factory_owner_types.remove(&key),
            body_ir_async_call_target: self.calls.body_ir_async_call_targets.remove(&key),
            body_ir_self_result_method: self.calls.body_ir_self_result_methods.remove(&key),
            body_ir_self_method_identity: self.traits.self_method_identities.remove(&key),
            body_ir_self_method_result_type: self.traits.self_method_result_types.remove(&key),
            body_ir_self_method_params: self.traits.self_method_params.remove(&key),
            body_ir_inferred_method_identity: self.traits.inferred_method_identities.remove(&key),
        }
    }

    /// Clear only direct-route hook facts between two synthetic resolutions, leaving legacy's probe state unchanged.
    pub(crate) fn clear_body_ir_call_site_facts(&mut self, span: Span) {
        let key = span;
        self.rust.checked_calls.remove(&key);
        self.rust.checked_function_values.remove(&key);
        self.calls.body_ir_reexport_calls.remove(&key);
        self.calls.body_ir_string_slice_members.remove(&key);
        self.calls.body_ir_constructor_types.remove(&key);
        self.calls.body_ir_call_type_arguments.remove(&key);
        self.calls.body_ir_call_result_types.remove(&key);
        self.calls.body_ir_factory_owner_types.remove(&key);
        self.calls.body_ir_async_call_targets.remove(&key);
        self.calls.body_ir_self_result_methods.remove(&key);
        self.traits.self_method_identities.remove(&key);
        self.traits.self_method_result_types.remove(&key);
        self.traits.self_method_params.remove(&key);
        self.traits.inferred_method_identities.remove(&key);
    }

    /// Make `facts`, as [`Self::take_call_site_facts`] returned them, the facts of the call at `span`, dropping any a
    /// later resolution recorded there.
    pub(crate) fn restore_call_site_facts(&mut self, span: Span, facts: CallSiteFacts) {
        /// Record `value` under `key`, or remove the entry when there is none.
        fn put<T>(map: &mut HashMap<crate::ast::Span, T>, key: crate::ast::Span, value: Option<T>) {
            match value {
                Some(value) => {
                    map.insert(key, value);
                }
                None => {
                    map.remove(&key);
                }
            }
        }
        let key = span;
        put(&mut self.rust.checked_calls, key, facts.checked_rust_call);
        put(
            &mut self.rust.checked_function_values,
            key,
            facts.checked_rust_function_value,
        );
        put(&mut self.calls.body_ir_reexport_calls, key, facts.body_ir_reexport_call);
        put(
            &mut self.calls.body_ir_string_slice_members,
            key,
            facts.body_ir_string_slice_member,
        );
        put(
            &mut self.calls.body_ir_self_result_methods,
            key,
            facts.body_ir_self_result_method,
        );
        put(
            &mut self.calls.inferred_constructor_types,
            key,
            facts.inferred_constructor_type,
        );
        put(
            &mut self.calls.inferred_call_result_types,
            key,
            facts.inferred_call_result_type,
        );
        put(&mut self.calls.resolved_method_calls, key, facts.method_call);
        put(&mut self.references.resolved_identities, key, facts.identity);
        put(&mut self.calls.call_site_callable_params, key, facts.callable_params);
        put(
            &mut self.calls.call_site_monomorph_type_args,
            key,
            facts.monomorph_type_args,
        );
        put(
            &mut self.calls.body_ir_constructor_types,
            key,
            facts.body_ir_constructor_type,
        );
        put(
            &mut self.calls.body_ir_call_type_arguments,
            key,
            facts.body_ir_call_type_arguments,
        );
        put(
            &mut self.calls.body_ir_call_result_types,
            key,
            facts.body_ir_call_result_type,
        );
        put(
            &mut self.calls.body_ir_factory_owner_types,
            key,
            facts.body_ir_factory_owner_type,
        );
        put(
            &mut self.calls.body_ir_async_call_targets,
            key,
            facts.body_ir_async_call_target,
        );
        put(
            &mut self.traits.self_method_identities,
            key,
            facts.body_ir_self_method_identity,
        );
        put(
            &mut self.traits.self_method_result_types,
            key,
            facts.body_ir_self_method_result_type,
        );
        put(
            &mut self.traits.self_method_params,
            key,
            facts.body_ir_self_method_params,
        );
        put(
            &mut self.traits.inferred_method_identities,
            key,
            facts.body_ir_inferred_method_identity,
        );
    }

    /// Preserve a resolved trait's defining module across an adapter chain that returns the same trait.
    ///
    /// Source types record the exact trait they adopted, but a method return such as `Stream[U, E]` intentionally keeps
    /// only its source spelling in [`ResolvedType`]. When the next call resolves that same trait, inherit the prior
    /// call's canonical module instead of degrading backend dispatch to an unqualified Rust trait method.
    pub fn inherit_same_trait_method_module(&mut self, receiver_span: Span, call_span: Span) {
        let Some(ResolvedMethodCall {
            dispatch:
                ResolvedMethodDispatch::Trait {
                    trait_name: receiver_trait,
                    module_path: Some(receiver_module),
                    ..
                },
            ..
        }) = self.resolved_method_call(receiver_span).cloned()
        else {
            return;
        };
        let Some(ResolvedMethodCall {
            dispatch:
                ResolvedMethodDispatch::Trait {
                    trait_name,
                    module_path,
                    ..
                },
            ..
        }) = self.calls.resolved_method_calls.get_mut(&call_span)
        else {
            return;
        };
        if module_path.is_none() && *trait_name == receiver_trait {
            *module_path = Some(receiver_module);
        }
    }

    /// Record a custom `for` iteration protocol route.
    pub fn record_protocol_iteration(&mut self, span: Span, info: ProtocolIterationInfo) {
        self.protocols.iterations.insert(span, info);
    }

    /// Record that the `for` loop over the list at `iter_span` takes the list's items (#1844).
    pub fn record_for_loop_takes_items(&mut self, iter_span: Span) {
        self.protocols.item_taking_iterations.insert(iter_span);
    }

    /// Return whether the `for` loop over the list at `iter_span` takes the list's items (#1844).
    pub fn for_loop_takes_items(&self, iter_span: Span) -> bool {
        self.protocols.item_taking_iterations.contains(&iter_span)
    }

    /// Record whether a checked comprehension consumes its direct collection source (#1983).
    pub fn record_comprehension_source_consumption(&mut self, iter_span: Span, consumed: bool) {
        self.protocols
            .comprehension_source_consumption
            .insert(iter_span, consumed);
    }

    /// Return the checker's collection-source consumption decision for a comprehension, when one applies (#1983).
    pub fn comprehension_source_is_consumed(&self, iter_span: Span) -> Option<bool> {
        self.protocols.comprehension_source_consumption.get(&iter_span).copied()
    }

    /// Record that an arm of the `match`, `if let` or `while let` over the scrutinee at `span` changes a caller-visible
    /// `mut` parameter through a name its pattern binds (#1561).
    pub fn record_match_scrutinee_changed_in_place(&mut self, span: Span) {
        self.protocols.in_place_match_scrutinees.insert(span);
    }

    /// Return whether the scrutinee at `span` is matched in place: an arm changes a caller-visible `mut` parameter
    /// through a name the pattern binds, so each name binds the part of the parameter it names (#1561).
    pub fn match_scrutinee_is_changed_in_place(&self, span: Span) -> bool {
        self.protocols.in_place_match_scrutinees.contains(&span)
    }
}

/// Return whether a callable parameter type carries borrow shape that lowering cannot recover from the callee alone.
fn callable_param_needs_boundary_snapshot(ty: &ResolvedType) -> bool {
    if ty.is_union() {
        return true;
    }
    match ty {
        ResolvedType::Ref(_) | ResolvedType::RefMut(_) | ResolvedType::FrozenStr | ResolvedType::FrozenBytes => true,
        ResolvedType::Function(params, ret) => {
            params
                .iter()
                .any(|param| callable_param_needs_boundary_snapshot(&param.ty))
                || callable_param_needs_boundary_snapshot(ret)
        }
        ResolvedType::Generic(_, args) => args.iter().any(callable_param_needs_boundary_snapshot),
        ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => {
            callable_param_needs_boundary_snapshot(inner)
        }
        ResolvedType::FrozenDict(key, value) => {
            callable_param_needs_boundary_snapshot(key) || callable_param_needs_boundary_snapshot(value)
        }
        ResolvedType::Tuple(items) => items.iter().any(callable_param_needs_boundary_snapshot),
        _ => false,
    }
}

/// Render the compiler-owned module identity used by semantic fact subjects.
fn semantic_module_identity(module_path: &[String]) -> String {
    incan_semantics_core::module_identity_for_path(module_path)
}

/// Convert a typechecker source-target artifact into the backend-neutral fact payload.
fn semantic_source_target_from_typecheck(target: &SourceTargetInfo) -> SemanticSourceTarget {
    SemanticSourceTarget::from_kind_str(target.module_path.clone(), target.name.clone(), target.kind.as_str())
}

/// Convert a resolved function binding into the semantic callable type stored on declaration facts.
fn semantic_type_from_function_binding(binding: &FunctionBindingInfo) -> IncanType {
    IncanType::Function {
        params: binding
            .params
            .iter()
            .map(semantic_callable_param_from_resolved)
            .collect(),
        return_type: Box::new(semantic_type_from_resolved(&binding.return_type)),
    }
}

/// Convert the current typechecker type universe into the backend-neutral Incan semantic type model.
///
/// This converts structural shape and preserves source spellings, not nominal authority. Checked fact and Body IR
/// consumers must use [`TypeCheckInfo::semantic_type`] so the accepting checker's nominal owners survive the
/// boundary. Builtin tuple annotations and inferred tuple literals share one structural semantic shape.
pub fn semantic_type_from_resolved(ty: &ResolvedType) -> IncanType {
    match ty {
        ResolvedType::Never => IncanType::Never,
        ResolvedType::Int => IncanType::Primitive(IncanPrimitiveType::Int),
        ResolvedType::Float => IncanType::Primitive(IncanPrimitiveType::Float),
        ResolvedType::Numeric(id) => IncanType::Primitive(IncanPrimitiveType::Numeric(*id)),
        ResolvedType::Bool => IncanType::Primitive(IncanPrimitiveType::Bool),
        ResolvedType::Str => IncanType::Primitive(IncanPrimitiveType::Str),
        ResolvedType::Bytes => IncanType::Primitive(IncanPrimitiveType::Bytes),
        ResolvedType::FrozenStr => IncanType::Primitive(IncanPrimitiveType::FrozenStr),
        ResolvedType::FrozenBytes => IncanType::Primitive(IncanPrimitiveType::FrozenBytes),
        ResolvedType::FrozenList(elem) => IncanType::Generic {
            base: collection_types::as_str(CollectionTypeId::FrozenList).to_string(),
            args: vec![semantic_type_from_resolved(elem)],
        },
        ResolvedType::FrozenDict(key, value) => IncanType::Generic {
            base: collection_types::as_str(CollectionTypeId::FrozenDict).to_string(),
            args: vec![semantic_type_from_resolved(key), semantic_type_from_resolved(value)],
        },
        ResolvedType::FrozenSet(elem) => IncanType::Generic {
            base: collection_types::as_str(CollectionTypeId::FrozenSet).to_string(),
            args: vec![semantic_type_from_resolved(elem)],
        },
        ResolvedType::Unit => IncanType::Primitive(IncanPrimitiveType::Unit),
        ResolvedType::Named(name) => IncanType::Named(name.clone()),
        ResolvedType::Generic(base, args) if collection_types::from_str(base) == Some(CollectionTypeId::Tuple) => {
            IncanType::Tuple(args.iter().map(semantic_type_from_resolved).collect())
        }
        ResolvedType::Generic(base, args)
            if incan_lang::lang::types::numerics::decimal_constructor_from_str(base).is_some() =>
        {
            match args.as_slice() {
                [ResolvedType::TypeVar(precision), ResolvedType::TypeVar(scale)] => {
                    match (precision.parse(), scale.parse()) {
                        (Ok(precision), Ok(scale)) => IncanType::Decimal { precision, scale },
                        _ => IncanType::Unknown,
                    }
                }
                _ => IncanType::Unknown,
            }
        }
        ResolvedType::Generic(base, args) => IncanType::Generic {
            base: base.clone(),
            args: args.iter().map(semantic_type_from_resolved).collect(),
        },
        ResolvedType::Function(params, return_type) => IncanType::Function {
            params: params.iter().map(semantic_callable_param_from_resolved).collect(),
            return_type: Box::new(semantic_type_from_resolved(return_type)),
        },
        ResolvedType::TypeToken(inner) => IncanType::TypeToken(Box::new(semantic_type_from_resolved(inner))),
        ResolvedType::Tuple(items) => IncanType::Tuple(items.iter().map(semantic_type_from_resolved).collect()),
        ResolvedType::TypeVar(name) => IncanType::TypeVar(name.clone()),
        ResolvedType::SelfType => IncanType::SelfType,
        ResolvedType::Ref(inner) => IncanType::Ref(Box::new(semantic_type_from_resolved(inner))),
        ResolvedType::RefMut(inner) => IncanType::RefMut(Box::new(semantic_type_from_resolved(inner))),
        // A builtin's own Rust carrier is that builtin, whichever spelling names it.
        ResolvedType::RustPath(path) => incan_semantics_core::RuntimeBuiltinType::from_carrier_path(path)
            .map_or_else(|| IncanType::RustInteropPath(path.clone()), IncanType::NativeBuiltin),
        ResolvedType::CallSiteInfer => IncanType::Infer,
        ResolvedType::Unknown => IncanType::Unknown,
    }
}

/// Convert typechecker callable parameter metadata into semantic callable parameter metadata.
pub(super) fn semantic_callable_param_from_resolved(param: &CallableParam) -> IncanCallableParam {
    IncanCallableParam {
        name: param.name.clone(),
        ty: semantic_type_from_resolved(&param.ty),
        kind: match param.kind {
            ParamKind::Normal => IncanCallableParamKind::Normal,
            ParamKind::RestPositional => IncanCallableParamKind::RestPositional,
            ParamKind::RestKeyword => IncanCallableParamKind::RestKeyword,
        },
        has_default: param.has_default,
        is_partial_preset: param.is_partial_preset,
        is_mut: param.is_mut,
        native_owned: false,
    }
}
