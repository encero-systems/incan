//! Native projection of the compiler declarations in the checked editable namespace.
//! Exhaustive patterns make an added field or alternative a compiler error until its Crumb contract is assigned.

use super::super::{CheckedNamespaceInterface, NamespaceMemberIdentities};
use super::*;
use crate::api_metadata::*;
use crate::library_manifest::*;
use crate::loaf_index::LoafIdentity;
use crate::ownership_metadata::{AccessMode, OwnershipContract, ResultDependency, SourceSlot, TypeComponent};
use crate::typechecker::ownership_contracts::defaults::DefaultValue;
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::body_ir::LifecycleMetadata;
use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, IncanCallableParam, IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType,
    IncanType, NumericTypeId, RuntimeBuiltinType, ScopeDiscriminant, SemanticSourceTargetKind, SymbolNamespace,
    SymbolOrigin,
};

model! { AccessMode<TypeRef> {
    mode: 0xe642b12901a6,
    component: 0x6985ca1f4daa,
    underlying: 0x342c8c61e679,
} }

model! { ApiAlias {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    target_path: 0xab801be656e0,
    is_public: 0x2a56e0445875,
    projected_function: 0x995c290cb59e,
    projected_type: 0xf7ea57604f28,
} }

model! { ApiCallableMetadata {
    ownership: 0x01b8016fdce4,
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    type_params: 0x75e7e9dead7e,
    receiver: 0x81bae876b705,
    params: 0xa20b52fae57c,
    return_type: 0x407ddf95ec74,
    is_async: 0x4e13cb86ec33,
} }

model! { ApiClass {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    docstring: 0x247499209ab0,
    docstring_sections: 0xc92ef4d8e45b,
    decorators: 0x4cc3599f825d,
    type_params: 0x75e7e9dead7e,
    extends: 0x8e7ce76b88cc,
    traits: 0x9e27940861d7,
    trait_adoptions: 0x812fff9c3cf4,
    derives: 0xa88fe6ec6115,
    fields: 0xbfe5d697162d,
    properties: 0x96adb9245286,
    methods: 0x3f3867313a9d,
} }

model! { ApiConst {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    ty: 0x4f9db7f8e506,
    value: 0xcd42404d52ad,
} }

sum! { ApiDeclaration {
    Function = 0xc803710302d5 (arg0: 0xcc1d0de5a773),
    Model = 0x5e2c614c23f0 (arg0: 0xcc1d0de5a773),
    Class = 0x4f3a9bd00397 (arg0: 0xcc1d0de5a773),
    Trait = 0x28e3e479e087 (arg0: 0xcc1d0de5a773),
    Enum = 0x54108dafaac5 (arg0: 0xcc1d0de5a773),
    Newtype = 0x2e7e824e5950 (arg0: 0xcc1d0de5a773),
    TypeAlias = 0x6b9d65546ff3 (arg0: 0xcc1d0de5a773),
    Const = 0x1071e7ae66c8 (arg0: 0xcc1d0de5a773),
    Static = 0x1d2c73dc01f2 (arg0: 0xcc1d0de5a773),
    Alias = 0xb19e02e9502b (arg0: 0xcc1d0de5a773),
    Partial = 0xa4d50fb85403 (arg0: 0xcc1d0de5a773),
} }

model! { ApiDocstring {
    summary: 0x761b7ad8ad43,
    params: 0xa20b52fae57c,
    returns: 0x153b2a38cb4a,
    fields: 0xbfe5d697162d,
    aliases: 0x19459a61f7d5,
    decorators: 0x4cc3599f825d,
} }

model! { ApiDocstringEntry {
    name: 0x82a3537ff0db,
    description: 0xc9046f7a37ad,
} }

model! { ApiDocstringReturn {
    ty: 0x4f9db7f8e506,
    description: 0xc9046f7a37ad,
} }

model! { ApiEnum {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    docstring: 0x247499209ab0,
    docstring_sections: 0xc92ef4d8e45b,
    decorators: 0x4cc3599f825d,
    type_params: 0x75e7e9dead7e,
    traits: 0x9e27940861d7,
    trait_adoptions: 0x812fff9c3cf4,
    value_type: 0x21f50267a8ad,
    variants: 0xdad569e101ad,
    variant_aliases: 0x39ce615db680,
    methods: 0x3f3867313a9d,
    derives: 0xa88fe6ec6115,
} }

model! { ApiEnumVariant {
    name: 0x82a3537ff0db,
    canonical: 0x0deeb8fa1dbb,
    fields: 0xbfe5d697162d,
    value: 0xcd42404d52ad,
} }

model! { ApiEnumVariantAlias {
    name: 0x82a3537ff0db,
    target: 0x34a04005bcaf,
} }

model! { ApiFunction {
    ownership: 0x01b8016fdce4,
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    docstring: 0x247499209ab0,
    docstring_sections: 0xc92ef4d8e45b,
    decorators: 0x4cc3599f825d,
    type_params: 0x75e7e9dead7e,
    params: 0xa20b52fae57c,
    return_type: 0x407ddf95ec74,
    is_async: 0x4e13cb86ec33,
} }

model! { ApiMethod {
    ownership: 0x01b8016fdce4,
    name: 0x82a3537ff0db,
    canonical: 0x0deeb8fa1dbb,
    visibility: 0x7d2fb3da93a0,
    alias_of: 0x2a7e64035b19,
    anchor: 0x79bfb0e2ba76,
    docstring: 0x247499209ab0,
    docstring_sections: 0xc92ef4d8e45b,
    decorators: 0x4cc3599f825d,
    type_params: 0x75e7e9dead7e,
    receiver: 0x81bae876b705,
    params: 0xa20b52fae57c,
    return_type: 0x407ddf95ec74,
    is_async: 0x4e13cb86ec33,
    has_body: 0x44aa6cc35675,
    made_private: 0x745f562ac40d,
} }

model! { ApiModel {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    docstring: 0x247499209ab0,
    docstring_sections: 0xc92ef4d8e45b,
    decorators: 0x4cc3599f825d,
    type_params: 0x75e7e9dead7e,
    traits: 0x9e27940861d7,
    trait_adoptions: 0x812fff9c3cf4,
    derives: 0xa88fe6ec6115,
    fields: 0xbfe5d697162d,
    properties: 0x96adb9245286,
    methods: 0x3f3867313a9d,
} }

model! { ApiNewtype {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    docstring: 0x247499209ab0,
    docstring_sections: 0xc92ef4d8e45b,
    decorators: 0x4cc3599f825d,
    type_params: 0x75e7e9dead7e,
    traits: 0x9e27940861d7,
    trait_adoptions: 0x812fff9c3cf4,
    derives: 0xa88fe6ec6115,
    is_rusttype: 0x46c9039d0e71,
    underlying: 0x342c8c61e679,
    checked_constructor: 0x9843d55167fc,
    constraints: 0xe225063b944e,
    implicit_coercion_enabled: 0x49b173280631,
    methods: 0x3f3867313a9d,
} }

model! { ApiPartial {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    target_path: 0xab801be656e0,
    target_kind: 0xc663f7f60c47,
    presets: 0x4700153d9be6,
    type_params: 0x75e7e9dead7e,
    params: 0xa20b52fae57c,
    return_type: 0x407ddf95ec74,
    is_async: 0x4e13cb86ec33,
} }

model! { ApiProjectedFunction {
    source_path: 0x857dc6ebc7d2,
    callable: 0xc15849ab11dd,
    decorators: 0x4cc3599f825d,
} }

model! { ApiProperty {
    name: 0x82a3537ff0db,
    canonical: 0x0deeb8fa1dbb,
    return_type: 0x407ddf95ec74,
} }

model! { ApiStatic {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    ty: 0x4f9db7f8e506,
} }

model! { ApiTrait {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    docstring: 0x247499209ab0,
    docstring_sections: 0xc92ef4d8e45b,
    decorators: 0x4cc3599f825d,
    type_params: 0x75e7e9dead7e,
    supertraits: 0x4b4a86416d45,
    requires: 0xb9014eaab626,
    methods: 0x3f3867313a9d,
} }

model! { ApiTypeAlias {
    name: 0x82a3537ff0db,
    anchor: 0x79bfb0e2ba76,
    type_alias: 0x28fcea907b58,
} }

model! { CanonicalIdentityExport {
    namespace: 0x9359f3bb2efa,
    origin: 0x181fdd46fc4a,
    declaration_name: 0xd63bdabe07a6,
    kind: 0x0b27c158feb3,
    declaration_span: 0xbbf073bf62dc,
} }

sum! { CanonicalIdentityNamespaceExport {
    OrdinaryLexical = 0x2f9fca1ff4f3,
    Member = 0x7c968fb71f50,
    ModulePath = 0xa8fc20e8c826,
} }

sum! { CanonicalIdentityOriginExport {
    Package = 0x59de121db1b8 { library: 0xb718f1354f72, module_path: 0x815194995bb8 },
    RustCrate = 0x6ea06689dd52 { path: 0xa0af9f865bf6 },
    Builtin = 0xa6a56071a42b,
} }

model! { CanonicalIdentitySpanExport {
    expansion: 0x167f448f11f3,
    start: 0xcced28c6dc3f,
    end: 0x361e48d0308f,
} }

model! { CanonicalSymbolId {
    namespace: 0x9359f3bb2efa,
    origin: 0x181fdd46fc4a,
    declaration_name: 0xd63bdabe07a6,
    kind: 0x0b27c158feb3,
    scope_discriminant: 0xe471ce42545e,
    declaration_span: 0xbbf073bf62dc,
} }

model! { CheckedApiMetadata {
    schema_version: 0x8c08b3c1c990,
    derivable_traits: 0xaf911067297c,
    module_path: 0x815194995bb8,
    declarations: 0x35e2c3de6980,
} }

model! { CheckedNamespaceInterface {
    schema: 0xdf0ad6e43880,
    module_name: 0x2c9684b961b6,
    module_path: 0x815194995bb8,
    api: 0x14c2529eb449,
    identities: 0x13c7351360f0,
    members: 0x17373ca1c763,
    constructor_defaults: 0x6d7c7890aef6,
    nominal_scope: 0x043a98799675,
} }

sum! { DecoratorArgMetadata {
    Positional = 0x8a26b9661ab4 { value: 0xcd42404d52ad },
    Named = 0x8605605ae026 { name: 0x82a3537ff0db, value: 0xcd42404d52ad },
} }

sum! { DecoratorCallArgMetadata {
    Positional = 0x8a26b9661ab4 { value: 0xcd42404d52ad },
    Named = 0x8605605ae026 { name: 0x82a3537ff0db, value: 0xcd42404d52ad },
    PositionalUnpack = 0xd0a164adad9a { value: 0xcd42404d52ad },
    KeywordUnpack = 0x9f92910a5ece { value: 0xcd42404d52ad },
} }

model! { DecoratorDictEntry {
    key: 0x2c70e12b7a06,
    value: 0xcd42404d52ad,
} }

model! { DecoratorMetadata {
    path: 0xa0af9f865bf6,
    source_name: 0x2a12f50168c9,
    anchor: 0x79bfb0e2ba76,
    type_args: 0xb06bd0a0bba8,
    args: 0x090772cf4068,
    decorated_callable: 0x80b18c029fb2,
} }

sum! { DecoratorValue {
    Literal = 0xa6230c2a81ab { value: 0xcd42404d52ad },
    ConstRef = 0x4f0411ebaad2 { name: 0x82a3537ff0db, value: 0xcd42404d52ad },
    SymbolRef = 0xbf4948a3df1f { path: 0xa0af9f865bf6 },
    List = 0x6f202f54a7b2 { items: 0x5f3c4f8580d3 },
    Dict = 0x9611d3d800fc { entries: 0x87d05cd08817 },
    Call = 0xd6e645b7d2b2 { callee: 0x92ca4c0ced62, type_args: 0xb06bd0a0bba8, args: 0x090772cf4068 },
    Type = 0xbaaddf70fb5d { ty: 0x4f9db7f8e506 },
    Unsupported = 0x54324658e2eb { reason: 0x8fcfccf675b1 },
} }

sum! { DefaultValue {
    Literal = 0xa6230c2a81ab,
    Constructor = 0x7a3933df8d02 (arg0: 0xcc1d0de5a773),
} }

sum! { EnumValueExport {
    Str = 0x8084a51b3c64 (arg0: 0xcc1d0de5a773),
    Int = 0x0b5f608070c6 (arg0: 0xcc1d0de5a773),
} }

sum! { EnumValueTypeExport {
    Str = 0x8084a51b3c64,
    Int = 0x0b5f608070c6,
} }

model! { FieldExport {
    name: 0x82a3537ff0db,
    canonical: 0x0deeb8fa1dbb,
    record_identity: 0x45b066b79a4d,
    record_canonical: 0xbdeb90f55061,
    renamed_from: 0xcd457f2e581d,
    renamed_from_lifecycle: 0xcf5a34b43709,
    default_is_empty: 0xdad08f7938e2,
    ty: 0x4f9db7f8e506,
    surface_type_name: 0xb068007f7f6b,
    visibility: 0x7d2fb3da93a0,
    has_default: 0x682a777c2328,
    default: 0x37a8eec1ce19,
    alias: 0x1a0a6a36ca0a,
    description: 0xc9046f7a37ad,
    made_private: 0x745f562ac40d,
} }

model! { HirSourceSpan {
    start: 0xcced28c6dc3f,
    end: 0x361e48d0308f,
    expansion: 0x167f448f11f3,
} }

model! { ImplementationAssociatedTypeExport {
    name: 0x82a3537ff0db,
    ty: 0x4f9db7f8e506,
} }

model! { ImplementationTraitBoundExport {
    trait_path: 0xaf70eef65607,
    type_args: 0xb06bd0a0bba8,
    associated_types: 0xeeaa8f656b2b,
    origin: 0x181fdd46fc4a,
} }

sum! { ImplementationTraitBoundOriginExport {
    Standard = 0xef6691545d2c,
    RustCapability = 0xed9849103930,
    SourceCallable = 0xee5bff56226d,
} }

model! { ImplementationTypeParamExport {
    name: 0x82a3537ff0db,
    bounds: 0x6ba9234898ef,
} }

model! { IncanCallableParam {
    name: 0x82a3537ff0db,
    ty: 0x4f9db7f8e506,
    kind: 0x0b27c158feb3,
    has_default: 0x682a777c2328,
    is_partial_preset: 0xc2ea353856a6,
    is_mut: 0xc57d21c0f95b,
    native_owned: 0xa6cbca317c1a,
    input_access: 0x2b237b58d072,
} }

sum! { IncanCallableParamKind {
    Normal = 0xa7248eeb45eb,
    RestPositional = 0xede421e168fb,
    RestKeyword = 0xffad05374421,
} }

sum! { IncanInputAccess {
    Inferred = 0xd461cbb49066,
    Shared = 0xe3c4b39d6d50,
    Exclusive = 0x4883b7dfffb0,
    Owned = 0x17b760c41cd2,
    InferredShared = 0x31caf164bb26,
} }

sum! { IncanPrimitiveType {
    Int = 0x0b5f608070c6,
    Float = 0x1a693c00c40c,
    Numeric = 0x36752d6cd147 (arg0: 0xcc1d0de5a773),
    Bool = 0x49f411f0a1a7,
    Str = 0x8084a51b3c64,
    Bytes = 0x4d8000301fcc,
    FrozenStr = 0x4fabe157e8f9,
    FrozenBytes = 0xa51a55d13bee,
    Unit = 0x4e545960f1bf,
} }

sum! { IncanType {
    Never = 0x6300ef800bb8,
    Primitive = 0x0956173ca415 (arg0: 0xcc1d0de5a773),
    Named = 0x8605605ae026 (arg0: 0xcc1d0de5a773),
    Generic = 0x0228c6d48ecf { base: 0xcae662172fd4, args: 0x090772cf4068 },
    Decimal = 0x18517d332d1b { precision: 0x68c2f9ee3147, scale: 0xf469802a3144 },
    Function = 0xc803710302d5 { params: 0xa20b52fae57c, return_type: 0x407ddf95ec74 },
    TypeToken = 0xeb900d7764d0 (arg0: 0xcc1d0de5a773),
    Tuple = 0xf136ee78c33b (arg0: 0xcc1d0de5a773),
    TypeVar = 0xc7a4a1414be4 (arg0: 0xcc1d0de5a773),
    SelfType = 0x80302493f2d4,
    Ref = 0x9b6e88e8943a (arg0: 0xcc1d0de5a773),
    RefMut = 0xffc490a6a7b3 (arg0: 0xcc1d0de5a773),
    RustInteropPath = 0xe7757f0ddf56 (arg0: 0xcc1d0de5a773),
    NativeBuiltin = 0x6fc742f9dcbd (arg0: 0xcc1d0de5a773),
    Unsupported = 0x54324658e2eb (arg0: 0xcc1d0de5a773),
    Infer = 0x27400ff4b809,
    Unknown = 0xb764cdc0eab7,
    NativeArray = 0x303fb129ace5 { element: 0x445929267209, length: 0x0f82aca66af9 },
    NativeTraitObject = 0x5c7a25e29c52 { path: 0xa0af9f865bf6, send: 0x27ce1d1bf427, sync: 0x75c75efe327a },
    NativePointer = 0xa69d400835c9 { mutable: 0x1da805f3d03b, pointee: 0x8ab1b3810888 },
    NativeCChar = 0xed5c31434646,
    Trait = 0x28e3e479e087 (arg0: 0xcc1d0de5a773),
} }

model! { LifecycleMetadata {
    since: 0x6b5a3ded016a,
    retire_in: 0xf3433ec556e6,
    message: 0xab530a13e459,
    by: 0xa7e2d26e8d15,
    issue: 0x4a502846d070,
} }

model! { LoafIdentity {
    name: 0x82a3537ff0db,
    version: 0x5ca4f3850ccc,
    digest: 0x0bf474896363,
    feature_projection: 0x548ca5897f6a,
} }

model! { MadePrivateExport {
    since: 0x6b5a3ded016a,
    retire_in: 0xf3433ec556e6,
    message: 0xab530a13e459,
    by: 0xa7e2d26e8d15,
    issue: 0x4a502846d070,
    replacement: 0x95713e9cbdd1,
} }

sum! { MemberVisibilityExport {
    Private = 0xc63eb6720c6e,
    Public = 0x591935b15b1c,
} }

model! { NamespaceMemberIdentities {
    variants: 0xdad569e101ad,
    fields: 0xbfe5d697162d,
    properties: 0x96adb9245286,
    methods: 0x3f3867313a9d,
} }

model! { NativeUnionExport {
    owner: 0x4c1029697ee3,
    rust_name: 0x8bc177c08361,
    members: 0x17373ca1c763,
    closure_members: 0xddf4b4b7f47f,
    closure_items: 0xb51b90659a78,
    semantic_type: 0x791297bd1a04,
    local_nominals: 0x2e2e4fe50a59,
    checked_projection: 0x7bc5aec271a4,
} }

sum! { NativeUnionOwnerExport {
    ContainingArtifact = 0xfc091e4a297a,
    SelectedArtifact = 0xdb3868c3ed48 (arg0: 0xcc1d0de5a773),
} }

model! { NativeUnionProjection {
    dependency_root: 0x61596ffbf7ea,
    rust_owner: 0xf921f806f9fb,
    members: 0x17373ca1c763,
    nominal_origins: 0xc543b337e462,
} }

model! { NewtypeConstraintExport {
    key: 0x2c70e12b7a06,
    value: 0xcd42404d52ad,
    repr: 0xa5fdb30d22d6,
} }

sum! { NewtypeConstraintKeyExport {
    Ge = 0x954edbbc5f5c,
    Gt = 0x12b5d558bcfd,
    Le = 0xa2fbc754b9ee,
    Lt = 0xc28e01d09adf,
} }

model! { NominalTypeOriginExport {
    provider: 0x5c4c1964340a,
    canonical: 0x0deeb8fa1dbb,
} }

sum! { NumericTypeId {
    I8 = 0x3146628c118c,
    I16 = 0xb9ff3f6e71da,
    I32 = 0xbe2d2cd0317d,
    I64 = 0xc1c7e02dd6cb,
    I128 = 0x47ec6bda4da3,
    U8 = 0x3473d38a1241,
    U16 = 0xef09bc80cd62,
    U32 = 0x80072486f2ea,
    U64 = 0x84b89e224554,
    U128 = 0x0c800dc4f949,
    F32 = 0x82255aee6ccc,
    F64 = 0xa44d7f74c829,
    ISize = 0xec7c4491f686,
    USize = 0xba4cad75a65a,
    Bool = 0x49f411f0a1a7,
} }

model! { OwnershipContract<TypeRef, CanonicalIdentityExport> {
    schema_version: 0x8c08b3c1c990,
    modes: 0x00c25d6301a7,
    result_source: 0x30affa51784a,
} }

sum! { OwnershipExport {
    Checked = 0x0efd92a33590 (arg0: 0xcc1d0de5a773),
    Unsupported = 0x54324658e2eb,
} }

sum! { OwnershipMode {
    Borrowed = 0xc88d22413095,
    BorrowedMut = 0xce704ee8660e,
    Owned = 0x17b760c41cd2,
    Reference = 0x71bf90935fc2,
} }

model! { ParamDefaultCallArgExport {
    name: 0x82a3537ff0db,
    value: 0xcd42404d52ad,
} }

model! { ParamDefaultCallSignatureExport {
    params: 0xa20b52fae57c,
    return_type: 0x407ddf95ec74,
} }

model! { ParamDefaultDictEntryExport {
    key: 0x2c70e12b7a06,
    value: 0xcd42404d52ad,
} }

sum! { ParamDefaultExport {
    Int = 0x0b5f608070c6 (arg0: 0xcc1d0de5a773),
    Float = 0x1a693c00c40c (arg0: 0xcc1d0de5a773),
    Bool = 0x49f411f0a1a7 (arg0: 0xcc1d0de5a773),
    String = 0xb2ef230e7f4f (arg0: 0xcc1d0de5a773),
    Bytes = 0x4d8000301fcc (arg0: 0xcc1d0de5a773),
    None = 0xdc937b598926,
    List = 0x6f202f54a7b2 (arg0: 0xcc1d0de5a773),
    Dict = 0x9611d3d800fc (arg0: 0xcc1d0de5a773),
    ConstRef = 0x4f0411ebaad2 (arg0: 0xcc1d0de5a773),
    Call = 0xd6e645b7d2b2 { path: 0xa0af9f865bf6, args: 0x090772cf4068, signature: 0x1a2fc26dc7ea },
    MethodCall = 0x5b3660d0209a { receiver_path: 0xee8823d9d4b2, method: 0x5b7e6bf2dc4a, args: 0x090772cf4068 },
    Unsupported = 0x54324658e2eb,
} }

model! { ParamExport {
    name: 0x82a3537ff0db,
    ty: 0x4f9db7f8e506,
    surface_type_name: 0xb068007f7f6b,
    kind: 0x0b27c158feb3,
    has_default: 0x682a777c2328,
    is_mut: 0xc57d21c0f95b,
    default: 0x37a8eec1ce19,
} }

sum! { ParamKindExport {
    Normal = 0xa7248eeb45eb,
    RestPositional = 0xede421e168fb,
    RestKeyword = 0xffad05374421,
} }

model! { PartialPresetExport {
    name: 0x82a3537ff0db,
    ty: 0x4f9db7f8e506,
    value: 0xcd42404d52ad,
} }

sum! { PartialTargetKindExport {
    Function = 0xc803710302d5,
    ModelConstructor = 0x7d5e928d2ee0,
    ClassConstructor = 0xfd1984cc16fb,
    NewtypeConstructor = 0x724dfd19e10b,
    Partial = 0xa4d50fb85403,
    Unknown = 0xb764cdc0eab7,
} }

model! { PresetDictEntryExport {
    key: 0x2c70e12b7a06,
    value: 0xcd42404d52ad,
} }

model! { PresetModelFieldExport {
    name: 0x82a3537ff0db,
    value: 0xcd42404d52ad,
} }

sum! { PresetValueExport {
    Int = 0x0b5f608070c6 (arg0: 0xcc1d0de5a773),
    Float = 0x1a693c00c40c (arg0: 0xcc1d0de5a773),
    Bool = 0x49f411f0a1a7 (arg0: 0xcc1d0de5a773),
    String = 0xb2ef230e7f4f (arg0: 0xcc1d0de5a773),
    Bytes = 0x4d8000301fcc (arg0: 0xcc1d0de5a773),
    None = 0xdc937b598926,
    List = 0x6f202f54a7b2 (arg0: 0xcc1d0de5a773),
    Set = 0xb6f6f3ad07b3 (arg0: 0xcc1d0de5a773),
    Tuple = 0xf136ee78c33b (arg0: 0xcc1d0de5a773),
    Dict = 0x9611d3d800fc (arg0: 0xcc1d0de5a773),
    ConstRef = 0x4f0411ebaad2 (arg0: 0xcc1d0de5a773),
    ModelLiteral = 0xa5d9178bbe92 { name: 0x82a3537ff0db, fields: 0xbfe5d697162d },
    Unsupported = 0x54324658e2eb,
} }

sum! { ReceiverExport {
    Immutable = 0x1e408dc27422,
    Mutable = 0xa0ba6d8a374a,
} }

model! { ResultDependency<CanonicalIdentityExport> {
    slot: 0x6558838331b7,
    fields: 0xbfe5d697162d,
    aggregate: 0xee3c65b5c7f4,
} }

sum! { RuntimeBuiltinType {
    ValidationError = 0xbdfaef14be91,
} }

sum! { SafeMetadataValue {
    Int = 0x0b5f608070c6 (arg0: 0xcc1d0de5a773),
    Float = 0x1a693c00c40c (arg0: 0xcc1d0de5a773),
    Bool = 0x49f411f0a1a7 (arg0: 0xcc1d0de5a773),
    String = 0xb2ef230e7f4f (arg0: 0xcc1d0de5a773),
    Bytes = 0x4d8000301fcc (arg0: 0xcc1d0de5a773),
    None = 0xdc937b598926,
} }

impl Project for ScopeDiscriminant {
    fn project(&self) -> Cell {
        Cell::Record(vec![(0xcd42404d52ad, self.0.project())])
    }
    fn restore(value: Cell) -> Result<Self, String> {
        let Cell::Record(fields) = value else {
            return Err("expected namespace scope".into());
        };
        let [value] = read_fields(fields, [0xcd42404d52ad])?;
        Ok(Self(usize::restore(value)?))
    }
}
sum! { SemanticSourceTargetKind {
    Function = 0xc803710302d5,
    Model = 0x5e2c614c23f0,
    Class = 0x4f3a9bd00397,
    Newtype = 0x2e7e824e5950,
    Rusttype = 0x92e438499e72,
    Enum = 0x54108dafaac5,
    TypeAlias = 0x6b9d65546ff3,
    Partial = 0xa4d50fb85403,
    Variant = 0x3f19fe84a2de,
    Trait = 0x28e3e479e087,
    Capability = 0x5faf58a69db5,
    Field = 0xf45fc1dfdc96,
    Method = 0x52a0f9b65b27,
    Property = 0x40040cb181f7,
    Const = 0x1071e7ae66c8,
    Static = 0x1d2c73dc01f2,
    Local = 0x8c31e6e72230,
    Parameter = 0xdd066ec1bf1d,
    Receiver = 0x86cec081a528,
    GenericBinder = 0xd6699c8e0e17,
    Module = 0xf148224ae44f,
    RustItem = 0xd2e475b00bf0,
    Builtin = 0xa6a56071a42b,
    Other = 0xf97e9da0e3b8 (arg0: 0xcc1d0de5a773),
} }

model! { SourceAnchor {
    id: 0xa56145270ce6,
    span: 0x37a0025aa1f2,
} }

sum! { SourceSlot {
    Parameter = 0xdd066ec1bf1d (arg0: 0xcc1d0de5a773),
    Receiver = 0x86cec081a528,
} }

sum! { SymbolNamespace {
    OrdinaryLexical = 0x2f9fca1ff4f3,
    Member = 0x7c968fb71f50,
    ModulePath = 0xa8fc20e8c826,
} }

sum! { SymbolOrigin {
    Module = 0xf148224ae44f (arg0: 0xcc1d0de5a773),
    Package = 0x59de121db1b8 { library: 0xb718f1354f72, module_path: 0x815194995bb8 },
    RustCrate = 0x6ea06689dd52 (arg0: 0xcc1d0de5a773),
    Builtin = 0xa6a56071a42b,
} }

model! { TypeAliasExport {
    name: 0x82a3537ff0db,
    type_params: 0x75e7e9dead7e,
    target: 0x34a04005bcaf,
} }

model! { TypeBoundExport {
    name: 0x82a3537ff0db,
    source_name: 0x2a12f50168c9,
    module_path: 0x815194995bb8,
    type_args: 0xb06bd0a0bba8,
    implementation_type_params: 0xb55b6a25c2a7,
    inferred: 0x204ed55e1af9,
} }

model! { TypeComponent {
    parameter: 0x6390361dfdff,
    children: 0x10910087de24,
} }

model! { TypeParamExport {
    name: 0x82a3537ff0db,
    bounds: 0x6ba9234898ef,
} }

sum! { TypeRef {
    Named = 0x8605605ae026 { name: 0x82a3537ff0db, origin: 0x181fdd46fc4a },
    Applied = 0x0c79a9c22284 { name: 0x82a3537ff0db, args: 0x090772cf4068, origin: 0x181fdd46fc4a },
    Function = 0xc803710302d5 { params: 0xa20b52fae57c, return_type: 0x407ddf95ec74 },
    TypeToken = 0xeb900d7764d0 { inner: 0x33bf6fbd7cd8 },
    Tuple = 0xf136ee78c33b { elements: 0xb0b17893a513 },
    TypeParam = 0x807561a2904e { name: 0x82a3537ff0db },
    SelfType = 0x80302493f2d4,
    Ref = 0x9b6e88e8943a { inner: 0x33bf6fbd7cd8 },
    RustPath = 0x442de14cd678 { path: 0xa0af9f865bf6 },
    Unknown = 0xb764cdc0eab7,
    NativeUnion = 0xda426b7ca634 (arg0: 0xcc1d0de5a773),
    MutParam = 0x4319f9302d74 { inner: 0x33bf6fbd7cd8 },
    Ownership = 0xc7d3acc826a8 { mode: 0xe642b12901a6, inner: 0x33bf6fbd7cd8 },
    Trait = 0x28e3e479e087 { identity: 0x689f6a627384 },
} }
