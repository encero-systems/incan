//! Native crate, callable, nominal, export, and entry declarations shared by lowering and the plan codec.

use serde::{Deserialize, Serialize};

use crate::body::{BasicBlock, Callee, FieldMetadata, Local, Parameter};
use crate::record_types::CheckedType;
use crate::types::{PlanType, SourceSpan, TupleElement};

/// A literal an attribute macro receives as a Rust literal; a list keeps its elements in written order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AttributeValue {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<AttributeValue>),
}

/// One attribute argument; an empty name marks a positional one.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributeArgument {
    pub name: String,
    pub value: AttributeValue,
}

/// One exported attribute macro applied to a declaration: its canonical Rust path and written arguments.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAttribute {
    pub path: String,
    pub arguments: Vec<AttributeArgument>,
}

/// One source type parameter's occurrence in a concrete signature, preserving its nested native loans.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericLoan {
    pub slot: i64,
    pub path: Vec<i64>,
    pub parameter: i64,
    pub ty: PlanType,
}

/// A declaration and its body, with locals zero through parameter count reserved.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub return_type: PlanType,
    pub locals: Vec<Local>,
    pub blocks: Vec<BasicBlock>,
    pub span: SourceSpan,
    /// A coroutine's constructor is native; its executable body still comes entirely from this plan.
    pub is_async: bool,
    /// Only the explicit boxing constructor retains native MIR for this callee.
    pub future_callee: Option<Callee>,
    /// Attribute macros rustc expands on the declaration; the macro's output replaces it.
    pub attributes: Vec<NativeAttribute>,
    /// Reference preorder for each parameter and the result; equal nonempty names retain one source generic loan.
    /// Empty names keep ordinary elision, including the independent outer borrow of a mutable collection.
    pub signature_lifetimes: Vec<Vec<String>>,
    /// Type-parameter provenance retains hidden native lifetime arguments independently of explicit references.
    pub generic_loans: Vec<GenericLoan>,
    /// Parameter slot whose loan is retained inside an owned native result; -1 means no retained result source.
    pub retained_result_source: i64,
}

impl Function {
    /// A synchronous function with no attributes, lifetimes, loans or retained result source.
    pub fn new(
        name: String,
        parameters: Vec<Parameter>,
        return_type: PlanType,
        locals: Vec<Local>,
        blocks: Vec<BasicBlock>,
        span: SourceSpan,
    ) -> Self {
        Self {
            name,
            parameters,
            return_type,
            locals,
            blocks,
            span,
            is_async: false,
            future_callee: None,
            attributes: Vec::new(),
            signature_lifetimes: Vec::new(),
            generic_loans: Vec::new(),
            retained_result_source: -1,
        }
    }
}

/// A native crate with scalar functions, plain nominal layouts and normal enums; external functions are admitted by
/// canonical path.
///
/// External declarations give validation a signature and explicit type arguments before rustc runs. The adapter
/// verifies those signatures against the loaded dependency metadata.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub enums: Vec<EnumDeclaration>,
    pub models: Vec<ModelDeclaration>,
    pub functions: Vec<Function>,
    pub externals: Vec<ExternalFunction>,
    pub span: SourceSpan,
    /// Computed static storage the adapter declares, each initialized by its planned initializer function.
    pub statics: Vec<StaticCell>,
    /// A library plan's public names, each re-exported at the crate root; an executable plan has none.
    pub exports: Vec<PlanExport>,
    /// Collected test entry functions; the adapter registers these with rustc's native test harness.
    pub tests: Vec<String>,
    /// A governed runtime's checked zip result owns each consumer-local carrier; absent entries use the linked ABI.
    pub zip_carriers: Vec<ZipCarrier>,
    /// Canonically lowered program entries; rustc applies Termination to the one selected for this process.
    pub programs: Vec<String>,
    /// A library publishes each generic ADT once; concrete layouts alias its instantiations.
    pub generic_models: Vec<GenericModelDeclaration>,
    pub generic_enums: Vec<GenericEnumDeclaration>,
}

impl Plan {
    /// A plan with no statics, exports, tests, zip carriers, programs or generic templates.
    pub fn new(
        enums: Vec<EnumDeclaration>,
        models: Vec<ModelDeclaration>,
        functions: Vec<Function>,
        externals: Vec<ExternalFunction>,
        span: SourceSpan,
    ) -> Self {
        Self {
            enums,
            models,
            functions,
            externals,
            span,
            statics: Vec::new(),
            exports: Vec::new(),
            tests: Vec::new(),
            zip_carriers: Vec::new(),
            programs: Vec::new(),
            generic_models: Vec::new(),
            generic_enums: Vec::new(),
        }
    }
}

/// A generic ADT field's checked type; function-pointer signatures retain inputs followed by one result entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GenericLayoutType {
    Parameter(String),
    Concrete(PlanType),
    Applied(String, Vec<GenericLayoutType>),
    Tuple(Vec<GenericLayoutType>),
    /// Declaration-order native inputs followed by the result; never an empty signature.
    FunctionPointer(Vec<GenericLayoutType>),
}

/// One source field in a generic ADT, in declaration order and with its checked visibility.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericLayoutField {
    pub name: String,
    pub ty: GenericLayoutType,
    pub public: bool,
}

/// A provider-owned generic ADT whose parameters, fields, and derive authority come from its checked declaration.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericModelDeclaration {
    pub name: String,
    pub source_name: String,
    pub parameters: Vec<String>,
    pub fields: Vec<GenericLayoutField>,
    pub derives: Vec<String>,
    pub span: SourceSpan,
}

/// One authored variant with generic payload types in declaration order.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericEnumVariant {
    pub name: String,
    pub fields: Vec<GenericLayoutType>,
}

/// One provider-owned generic enum template; closed plans alias it with exact type arguments.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericEnumDeclaration {
    pub name: String,
    pub parameters: Vec<String>,
    pub variants: Vec<GenericEnumVariant>,
    pub derives: Vec<String>,
    pub span: SourceSpan,
}

/// Associate a builtin zip's checked element layouts with the published runtime result's closed source carrier.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ZipCarrier {
    pub left: TupleElement,
    pub right: TupleElement,
    pub target: PlanType,
}

/// One public name a library plan publishes, emitted as `pub use <path> as <name>;` at the crate root.
///
/// `path` is either a planned item of this crate (`crate::<item>`) or a Rust item the library re-exports (`::<path>`).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanExport {
    pub name: String,
    pub path: String,
}

/// One computed module static's native storage: a lazily initialized cell holding a value of `ty`.
///
/// The adapter declares the cell and its load, store and force accessors; `initializer` names the planned function with
/// no parameters that computes the first value, run at most once, when the cell is first forced or read.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticCell {
    pub slot: i64,
    pub ty: PlanType,
    pub initializer: String,
    pub span: SourceSpan,
    /// Only the first computed cell of a module owns its initialization completion flag.
    pub module_completion: bool,
}

/// Spell a native operation attached to the unit's storage slot. Only producer-local plans use slot names.
pub fn static_accessor_name(slot: i64, operation: &str) -> String {
    format!("__incan_static_{operation}_{slot}")
}

/// Checked source poll and item carrier authorizing an ordinary native SDK iterator implementation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct IteratorMethods {
    pub item: PlanType,
    pub next: String,
}

/// Complete standard default selection and the source poll body authorizing one concrete native implementation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FallibleIteratorMethods {
    pub item: PlanType,
    pub error: PlanType,
    pub next: String,
}

/// A Rust trait method filled by a planned source method; rustc checks the implementation's signature.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTraitMethod {
    pub name: String,
    pub function: String,
}

/// A class's request to implement a concrete Rust trait; rustc proves the implementation, not this record.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTraitAdoption {
    pub path: String,
    pub methods: Vec<NativeTraitMethod>,
}

/// Checked field layout and implicit derives for one concrete source nominal layout.
///
/// A newtype has exactly one public field named `0`, emitted as a tuple slot; named model fields retain their names.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelDeclaration {
    pub name: String,
    pub public: bool,
    pub fields: Vec<Parameter>,
    pub field_public: Vec<bool>,
    pub derives: Vec<String>,
    pub span: SourceSpan,
    /// Source-owned closed generic carriers retain one native lifetime parameter for each reference in their field
    /// layout. Zero keeps the ordinary owned-layout declaration.
    pub view_lifetime_count: i64,
    /// A specialized carrier keeps its source name for derived Debug and declaration metadata.
    pub source_name: String,
    /// A stdlib layout names the compiled SDK crate item that owns its storage; the plan never declares it. With no
    /// fields the layout is opaque: its values move and drop, but no field is read or written.
    pub library_item: String,
    /// A closed SDK alias keeps the checked arguments of its declaring model's type parameters.
    pub library_type_arguments: Vec<PlanType>,
    /// Per field, the name a serde form uses where a checked alias renames it; an empty entry keeps the field name,
    /// and no entries means no field is renamed.
    pub field_serde_names: Vec<String>,
    /// A checked Deserialize forwards std String ingress to this exact planned root; empty keeps ordinary derives.
    pub checked_deserialize: String,
    /// None grants no generated hook; an empty list is an authorized empty metadata surface.
    pub reflection_metadata: Option<Vec<FieldMetadata>>,
    /// The checked primitive declaration owns generated field records. Equal-spelled user records do not gain this
    /// role.
    pub field_metadata_carrier: bool,
    /// Exact source comparison symbols: equality alone, or equality followed by lt/le/gt/ge. Empty grants no bridge.
    pub comparison_methods: Vec<String>,
    /// A source adoption grants native authority only after every standard slot and the poll ABI are checked.
    pub fallible_iterator: Option<FallibleIteratorMethods>,
    /// Ordinary native Iterator needs only the selected poll; its sum default remains SDK-owned.
    pub iterator: Option<IteratorMethods>,
    /// A checked standard Callable1 member and its ordered owned input/output carriers; empty grants no bridge.
    pub callable_method: String,
    pub callable_signature: Vec<PlanType>,
    /// The selected source SDK TryFrom[str] identity authenticates this planned Incan conversion root. Empty paired
    /// fields grant no conversion, independently of serde's native Rust TryFrom contract.
    pub string_conversion_root: String,
    pub string_conversion_trait: String,
    /// Rust traits a source class implements with its own methods; empty implements none.
    pub native_trait_adoptions: Vec<NativeTraitAdoption>,
    /// Native field slots the plan cannot carry. Each keeps a unit placeholder type that never authorizes a read, a
    /// write or a construction of the layout.
    pub unavailable_native_fields: Vec<i64>,
}

impl ModelDeclaration {
    /// A model layout with every optional native authority absent.
    pub fn new(
        name: String,
        public: bool,
        fields: Vec<Parameter>,
        field_public: Vec<bool>,
        derives: Vec<String>,
        span: SourceSpan,
    ) -> Self {
        Self {
            name,
            public,
            fields,
            field_public,
            derives,
            span,
            view_lifetime_count: 0,
            source_name: String::new(),
            library_item: String::new(),
            library_type_arguments: Vec::new(),
            field_serde_names: Vec::new(),
            checked_deserialize: String::new(),
            reflection_metadata: None,
            field_metadata_carrier: false,
            comparison_methods: Vec::new(),
            fallible_iterator: None,
            iterator: None,
            callable_method: String::new(),
            callable_signature: Vec::new(),
            string_conversion_root: String::new(),
            string_conversion_trait: String::new(),
            native_trait_adoptions: Vec::new(),
            unavailable_native_fields: Vec::new(),
        }
    }
}

/// The signature of a canonical dependency function used by this plan.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalFunction {
    pub path: String,
    pub type_arguments: Vec<PlanType>,
    pub parameters: Vec<PlanType>,
    pub return_type: PlanType,
    pub span: SourceSpan,
}

/// Compare a retained carrier's exact checked type without reconstructing identity from diagnostic text.
pub fn semantic_type_matches(declaration: &EnumDeclaration, ty: &CheckedType) -> bool {
    declaration.semantic_key.as_ref() == Some(ty)
}

/// Which standard enum a retained layout instantiates. `Option` lays out `None` then `Some`, and `Result` lays out
/// `Ok` then `Err`; any other enum is `Declared`, with its variants in source declaration order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Carrier {
    Declared,
    Option,
    Result,
}

/// Checked normal enum layout; variant indices follow source declaration order.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumDeclaration {
    pub name: String,
    pub public: bool,
    pub variants: Vec<EnumVariant>,
    pub derives: Vec<String>,
    pub span: SourceSpan,
    /// Source-owned enum payloads retain one native lifetime parameter for each reference in source variant order.
    /// Zero keeps the ordinary owned-layout declaration.
    pub view_lifetime_count: i64,
    /// Standard carriers use their concrete type arguments from the variant fields.
    pub carrier: Carrier,
    /// The exact checked carrier witness is used for identity; `source_type` remains diagnostic presentation only.
    pub semantic_key: Option<CheckedType>,
    pub source_type: String,
    /// A union's Debug prints the member it holds, as legacy's hand-written impl does; it is never derived.
    pub member_debug: bool,
    /// A stdlib enum names the compiled SDK crate item that owns it; the plan never declares it.
    pub library_item: String,
    /// Closed SDK alias arguments retained from the checked enum instance, in declaration order.
    pub library_type_arguments: Vec<PlanType>,
    /// A value enum's backing ("str" or "int") and its raw values in variant order, which its serde form writes and
    /// reads as legacy's hand-written impls do; empty for any other enum.
    pub raw_kind: String,
    pub raw_values: Vec<String>,
    /// Checked source equality and ordering bodies; the adapter validates their exact enum input ABI before bridging.
    pub comparison_methods: Vec<String>,
    /// Exact callable object aliases for environment-holding union variants, in their physical order.
    pub closure_items: Vec<String>,
}

impl EnumDeclaration {
    /// A declared enum layout with every optional native authority absent.
    pub fn new(name: String, public: bool, variants: Vec<EnumVariant>, derives: Vec<String>, span: SourceSpan) -> Self {
        Self {
            name,
            public,
            variants,
            derives,
            span,
            view_lifetime_count: 0,
            carrier: Carrier::Declared,
            semantic_key: None,
            source_type: String::new(),
            member_debug: false,
            library_item: String::new(),
            library_type_arguments: Vec::new(),
            raw_kind: String::new(),
            raw_values: Vec::new(),
            comparison_methods: Vec::new(),
            closure_items: Vec::new(),
        }
    }
}

/// One unit or positional payload variant; its field types are checker facts.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumVariant {
    pub name: String,
    pub fields: Vec<PlanType>,
}

crate::structural_debug!(AttributeArgument, "AttributeArgument", name, value);
crate::structural_debug!(NativeAttribute, "NativeAttribute", path, arguments);
crate::structural_debug!(GenericLoan, "GenericLoan", slot, path, parameter, ty);
crate::structural_debug!(
    Function,
    "Function",
    name,
    parameters,
    return_type,
    locals,
    blocks,
    span,
    is_async,
    future_callee,
    attributes,
    signature_lifetimes,
    generic_loans,
    retained_result_source,
);
crate::structural_debug!(
    Plan,
    "Plan",
    enums,
    models,
    functions,
    externals,
    span,
    statics,
    exports,
    tests,
    zip_carriers,
    programs,
    generic_models,
    generic_enums,
);
crate::structural_debug!(GenericLayoutField, "GenericLayoutField", name, ty, public);
crate::structural_debug!(
    GenericModelDeclaration,
    "GenericModelDeclaration",
    name,
    source_name,
    parameters,
    fields,
    derives,
    span,
);
crate::structural_debug!(GenericEnumVariant, "GenericEnumVariant", name, fields);
crate::structural_debug!(
    GenericEnumDeclaration,
    "GenericEnumDeclaration",
    name,
    parameters,
    variants,
    derives,
    span
);
crate::structural_debug!(ZipCarrier, "ZipCarrier", left, right, target);
crate::structural_debug!(PlanExport, "PlanExport", name, path);
crate::structural_debug!(StaticCell, "StaticCell", slot, ty, initializer, span, module_completion);
crate::structural_debug!(IteratorMethods, "IteratorMethods", item, next);
crate::structural_debug!(FallibleIteratorMethods, "FallibleIteratorMethods", item, error, next);
crate::structural_debug!(NativeTraitMethod, "NativeTraitMethod", name, function);
crate::structural_debug!(NativeTraitAdoption, "NativeTraitAdoption", path, methods);
crate::structural_debug!(
    ModelDeclaration,
    "ModelDeclaration",
    name,
    public,
    fields,
    field_public,
    derives,
    span,
    view_lifetime_count,
    source_name,
    library_item,
    library_type_arguments,
    field_serde_names,
    checked_deserialize,
    reflection_metadata,
    field_metadata_carrier,
    comparison_methods,
    fallible_iterator,
    iterator,
    callable_method,
    callable_signature,
    string_conversion_root,
    string_conversion_trait,
    native_trait_adoptions,
    unavailable_native_fields,
);
crate::structural_debug!(
    ExternalFunction,
    "ExternalFunction",
    path,
    type_arguments,
    parameters,
    return_type,
    span
);
crate::structural_debug!(
    EnumDeclaration,
    "EnumDeclaration",
    name,
    public,
    variants,
    derives,
    span,
    view_lifetime_count,
    carrier,
    semantic_key,
    source_type,
    member_debug,
    library_item,
    library_type_arguments,
    raw_kind,
    raw_values,
    comparison_methods,
    closure_items,
);
crate::structural_debug!(EnumVariant, "EnumVariant", name, fields);
