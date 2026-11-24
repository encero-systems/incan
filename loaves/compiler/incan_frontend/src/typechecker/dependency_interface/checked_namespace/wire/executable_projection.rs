// Generated from the Incan executable shapes by scripts/generate_executable_projection.incn.

sum! { AbiV0RuntimeRequirement {
    RuntimeHelper = 0x0f63ea09b0e0 (arg0: 0xcc1d0de5a773),
    HostedStd = 0x248379e3de07,
    Allocator = 0xca24e09fc69e,
    PanicStrategy = 0x0c706f51242d,
    AsyncRuntime = 0x76622a9af8b9,
} }

sum! { AggregateKind {
    OptionSome = 0xbea050a267a6,
    Tuple = 0xf136ee78c33b,
    List = 0x6f202f54a7b2,
    Set = 0xb6f6f3ad07b3,
    Range = 0x5de74a81b192,
    Constructor = 0x7a3933df8d02 (arg0: 0xcc1d0de5a773),
    EnumVariant = 0x9d4bc157f252 (arg0: 0xcc1d0de5a773),
} }

sum! { ArgumentBinding {
    UnresolvedPositional = 0x34275abcabbf,
    Resolved = 0x5be3c2c8354e { arguments: 0xb192017da47f, defaulted_slots: 0xcca3dfdda803 },
    ResolvedDeclarationOrder = 0x3dc029d522aa { arguments: 0xb192017da47f, defaulted_slots: 0xcca3dfdda803 },
} }

sum! { ArgumentElement {
    One = 0x8b12507783d5 (arg0: 0xcc1d0de5a773),
    Named = 0x8605605ae026 { name: 0x82a3537ff0db, operand: 0x0b081d24f7f5 },
    Spread = 0xb11fb1f593a8 (arg0: 0xcc1d0de5a773),
} }

sum! { AssertionKind {
    Condition = 0x39b36d38d6eb { cond: 0x0ab2dd2f64c9 },
    Pattern = 0x4288ade73ff9 { scrutinee: 0x84fa17e0c3ba, pattern: 0x1fd38d5cbd2a },
    Raises = 0xe97e76330ffc { call: 0x7edb360f06ac, expected_error: 0x0c5121042d00 },
} }

sum! { BinOp {
    Add = 0x9fd728c66c9a,
    Sub = 0x21c1eb891c68,
    Mul = 0x082acfbd6d9b,
    Div = 0xd24b5d15c0b1,
    FloorDiv = 0x3523c74e791a,
    Mod = 0x2fe83d132e3d,
    Pow = 0x2f8df4920880,
    BitAnd = 0xc4a4f3d569c3,
    BitOr = 0xfa1b8342e3fc,
    BitXor = 0x79ad2f63690f,
    Shl = 0x4749a999793d,
    Shr = 0xb8e7288d3867,
    Eq = 0xd4035e75fdf8,
    Ne = 0x38cebf4836a9,
    Lt = 0xc28e01d09adf,
    Le = 0xa2fbc754b9ee,
    Gt = 0x12b5d558bcfd,
    Ge = 0x954edbbc5f5c,
    Is = 0xf5985de1fd3a,
    IsNot = 0xebd944c7a6e6,
    And = 0x0f67514b901d,
    Or = 0x0568bd03b8eb,
} }

model! { Block {
    scope: 0x5f161c914988,
    stmts: 0xbabf02d55ffe,
} }

model_with_omissions! { Body {
    type_parameters: 0xa1c4e8ba64bd,
    decl_id: 0xf4d27facff97,
    direct_call_id: 0x7e8f06e505b0,
    canonical: 0x0deeb8fa1dbb,
    name: 0x82a3537ff0db,
    span: 0x37a0025aa1f2,
    return_type: 0x407ddf95ec74,
    callable_representation: 0x341a2ec3997c,
    checked_input_profile: 0x0ad41a53eae7,
    explicit_input_bindings: 0x760cb1d7a442,
    returned_view: 0x95fe1cfb1660,
    named_type_identities: 0x1e61b087a864,
    locals: 0xe9e8d4b193cf,
    params: 0xa20b52fae57c,
    param_locals: 0x796605618958,
    scopes: 0x56b3669c3a19,
    block: 0x496aca80e4d8,
    runtime_requirements: 0x679751f43980,
    panic_facts: 0xd807d228f129,
    is_async: 0x4e13cb86ec33,
} default [nominal_instantiation, trait_default_instantiation, retired_rust_facade_wire, native_attributes] }

model_with_omissions! { BodyIrModule {
    decorated_bindings: 0x68a753a71d25,
    entrypoint_effects: 0x515c29b485ab,
    string_conversion_contract: 0x50568e7e9b8d,
    constant_declarations: 0xaddd2957ee60,
    type_alias_declarations: 0xf7f87b004a47,
    enum_declarations: 0x7611a34dd065,
    loaf_callables: 0x50af4d6d0e2d,
    native_union_declarations: 0x1aa2c68f9640,
    static_declarations: 0x9695e49e53d4,
    module_id: 0xbf1dfac8e8f0,
    nominal_declarations: 0x3a2f9fd2cf05,
    fieldless_enum_declarations: 0x1483250a9cca,
    value_enum_declarations: 0x255b9de843c8,
    trait_declarations: 0x8a47a875cd2e,
    trait_implementations: 0xfaa7a4a577c9,
    bodies: 0x29a71163aa38,
} default [runtime_callable_bindings, runtime_binding_scopes, program_entrypoints, published_callables] }

model! { BoundArgument {
    slot: 0x6558838331b7,
    written_position: 0x4e4e06b077a7,
} }

sum! { BuiltinFnId {
    Print = 0xdf0fe79898ef,
    Len = 0x58fa4ba6b2e3,
    Sum = 0xa21d7df04c33,
    Min = 0xdea79332147f,
    Max = 0xa1a5936d3b0f,
    Str = 0x8084a51b3c64,
    Int = 0x0b5f608070c6,
    Float = 0x1a693c00c40c,
    Bool = 0x49f411f0a1a7,
    Abs = 0xef68818cc22e,
    Range = 0x5de74a81b192,
    Enumerate = 0x6d41343c8b28,
    Zip = 0x7bc88ebb6d01,
    Sorted = 0x049b4c0df7ac,
    ReadFile = 0xbfe9459af485,
    WriteFile = 0x5b9305a0e439,
    JsonStringify = 0x28c7c6c3f69c,
    IsInstance = 0x9185d38b964b,
} }

sum! { BuiltinMethodId {
    Str = 0x8084a51b3c64 (arg0: 0xcc1d0de5a773),
    FrozenStr = 0x4fabe157e8f9 (arg0: 0xcc1d0de5a773),
    Bytes = 0x4d8000301fcc (arg0: 0xcc1d0de5a773),
    FrozenBytes = 0xa51a55d13bee (arg0: 0xcc1d0de5a773),
    Iterator = 0xb0b0485265ee (arg0: 0xcc1d0de5a773),
    Generator = 0x1ade60abe93f (arg0: 0xcc1d0de5a773),
    Option = 0x45aaacba7ea1 (arg0: 0xcc1d0de5a773),
    Result = 0x6e7d50e84f47 (arg0: 0xcc1d0de5a773),
    Clone = 0x5779f32fab00 (arg0: 0xcc1d0de5a773),
    Float = 0x1a693c00c40c (arg0: 0xcc1d0de5a773, arg1: 0x84ff873bc51d),
    IntegerHelper = 0xd96d76eb9277 (arg0: 0xcc1d0de5a773, arg1: 0x84ff873bc51d, arg2: 0x2092f891c0cc),
    NumericResize = 0xe2d7bd697d2f (arg0: 0xcc1d0de5a773, arg1: 0x84ff873bc51d),
    List = 0x6f202f54a7b2 (arg0: 0xcc1d0de5a773),
    ListIter = 0x1dbc1f3e0d5d,
    Dict = 0x9611d3d800fc (arg0: 0xcc1d0de5a773),
    Set = 0xb6f6f3ad07b3 (arg0: 0xcc1d0de5a773),
    FrozenList = 0x2a9735e0e131 (arg0: 0xcc1d0de5a773),
    FrozenDict = 0xe577020378dd (arg0: 0xcc1d0de5a773),
    FrozenSet = 0x7e4bc6638560 (arg0: 0xcc1d0de5a773),
} }

sum! { BytesMethodId {
    Decode = 0x1ae3fcff1a58,
    AsSlice = 0xc64a953e4b13,
} }

model! { CallableParam {
    local: 0x25bf8e1a2393,
    name: 0x82a3537ff0db,
    ty: 0x4f9db7f8e506,
    kind: 0x0b27c158feb3,
    span: 0x37a0025aa1f2,
    default: 0x37a8eec1ce19,
    mutable: 0x1da805f3d03b,
    native_owned: 0xa6cbca317c1a,
    input_access: 0x2b237b58d072,
} }

sum! { CallableParamDefault {
    Required = 0x4850b174b713,
    Source = 0x0e570ca6fabe (arg0: 0xcc1d0de5a773),
    PartialPreset = 0x09825ef683d3 { capture: 0x460ee6aa3a80 },
    Unsupported = 0x54324658e2eb { span: 0x37a0025aa1f2, description: 0xc9046f7a37ad },
} }

model! { CallableRepresentation {
    function_pointer_locals: 0x363a14e82a77,
    closure_holding_locals: 0xcd2fc30d93e4,
    closure_holding_parameters: 0x6febe5519866,
    closure_holding_return: 0xc10aac4db07f,
} }

sum! { CallableTarget {
    Named = 0x8605605ae026 (arg0: 0xcc1d0de5a773),
    Local = 0x8c31e6e72230 (arg0: 0xcc1d0de5a773),
} }

sum! { Callee {
    Function = 0xc803710302d5 (arg0: 0xcc1d0de5a773),
    Method = 0x52a0f9b65b27 (arg0: 0xcc1d0de5a773),
    Helper = 0xce793a72d4ab (arg0: 0xcc1d0de5a773),
    ProviderOperation = 0xcd7b72a3a429 (arg0: 0xcc1d0de5a773),
    TypeMetadata = 0xad91a4bc7a49 (arg0: 0xcc1d0de5a773),
    CheckedForeign = 0xae983dd659ff (arg0: 0xcc1d0de5a773),
    CheckedBridge = 0x78b3537d777f (arg0: 0xcc1d0de5a773),
} }

model! { CheckedBridgeCall {
    authority: 0x8f76fd501bb6,
    operation: 0x9bf5a24e4aa7,
    return_type: 0x407ddf95ec74,
} }

sum! { CheckedBridgeOperation {
    MakeCString = 0xfe750bd0d5e0,
    CStringPointer = 0xe62d8697d504,
    CopyUtf8 = 0x697234bdde31,
    NewOutputSlot = 0xc211236df2ea,
    NewInOutSlot = 0x898c43527430,
    TakeOutputSlot = 0xb3b6aa8d6fd6,
    MakeSpan = 0x891dff0fb70a,
    SpanConstPointer = 0x9ce3a7fb183e,
    SpanMutPointer = 0xaae45bfe8109,
    SpanCount = 0x80059afa49ff,
    FinishSpan = 0x88f75b8f6a93,
} }

model! { CheckedForeignCall {
    binding: 0x80f70afeef3c,
    member: 0xe31ab643c44f,
    symbol: 0xb76a7ca153c2,
    library: 0xb718f1354f72,
    clove: 0x9e8c9731465c,
    configuration: 0xb7d64a922100,
    framework: 0x06a61df042ad,
    header: 0x1e0584a25d9f,
    parameters: 0xf528212a496d,
    return_type: 0x407ddf95ec74,
} }

model! { CheckedInputParameter {
    slot: 0x6558838331b7,
    access: 0xa0561fd649cd,
} }

model! { CheckedInputProfile {
    result_requirements: 0x4df5dd21701f,
    version: 0x5ca4f3850ccc,
    owner: 0x4c1029697ee3,
    parameters: 0xf528212a496d,
    owned_initializers: 0x7e91b02b6384,
} }

model! { CheckedOwnedInitializer {
    source: 0x41cf6794ba42,
    destination: 0xb5c755aaab10,
    assignment: 0xce7a7c10b0df,
} }

model! { ClassInheritance {
    parent: 0xe47125968b3b,
    methods: 0x3f3867313a9d,
} }

model! { ClassMethodImplementation {
    method: 0x5b7e6bf2dc4a,
    implementation: 0xda31012c4033,
} }

sum! { CloneMethodId {
    Clone = 0x5779f32fab00,
} }

model! { ClosureBody {
    native_storage: 0xd3ecee8f80d3,
    original_callable: 0x9bf1178c347b,
    function_item: 0x60da7e7c84cd,
    consumes_environment: 0x4c8202729198,
    capture_locals: 0xb60aecd27639,
    stmts: 0xbabf02d55ffe,
    result: 0xf6a214f7a5fc,
} }

sum! { CompilerNodeKind {
    Package = 0x59de121db1b8,
    Module = 0xf148224ae44f,
    Declaration = 0x7609342dc466,
    Statement = 0x6171b2a46120,
    Expression = 0xc67415bcff32,
    Local = 0x8c31e6e72230,
    Type = 0xbaaddf70fb5d,
} }

sum! { Constant {
    TypeToken = 0xeb900d7764d0 (arg0: 0xcc1d0de5a773),
    FrozenAggregate = 0x573dce058c8b (arg0: 0xcc1d0de5a773, arg1: 0x84ff873bc51d),
    Int = 0x0b5f608070c6 (arg0: 0xcc1d0de5a773),
    Float = 0x1a693c00c40c (arg0: 0xcc1d0de5a773),
    TypedNumeric = 0x4312798fd5b1 (arg0: 0xcc1d0de5a773),
    Bool = 0x49f411f0a1a7 (arg0: 0xcc1d0de5a773),
    Str = 0x8084a51b3c64 (arg0: 0xcc1d0de5a773),
    FrozenStr = 0x4fabe157e8f9 (arg0: 0xcc1d0de5a773),
    Bytes = 0x4d8000301fcc (arg0: 0xcc1d0de5a773),
    Unit = 0x4e545960f1bf,
    None = 0xdc937b598926,
    NumericNewtype = 0x74905445d493 (arg0: 0xcc1d0de5a773),
    RustItem = 0xd2e475b00bf0 (arg0: 0xcc1d0de5a773, arg1: 0x84ff873bc51d),
} }

model! { ConstantDeclaration {
    canonical: 0x0deeb8fa1dbb,
    value: 0xcd42404d52ad,
} }

model! { ConstructorTarget {
    name: 0x82a3537ff0db,
    canonical: 0x0deeb8fa1dbb,
    direct_declaration_id: 0xa6b05d0bcaad,
    canonical_field_layout: 0xe8d470c4444a,
    binding: 0x80f70afeef3c,
} }

model! { DecoratedBinding {
    original_body: 0xff7755c9334b,
    initializer: 0xf9d7a792ac66,
    initialization: 0x05fd548cb946,
} }

sum! { DecoratedInitialization {
    ModuleStorage = 0x3b5d8515afe5,
    PerInstantiation = 0xdbe7375de591,
    PerInvocation = 0x31bdb5284fc5,
} }

model! { DefaultComputation {
    span: 0x37a0025aa1f2,
    stmts: 0xbabf02d55ffe,
    result: 0xf6a214f7a5fc,
} }

sum! { DictEntry {
    Pair = 0x989da04b0aaa (arg0: 0xcc1d0de5a773, arg1: 0x84ff873bc51d),
    Spread = 0xb11fb1f593a8 (arg0: 0xcc1d0de5a773),
} }

sum! { DictMethodId {
    Keys = 0xf0d66a79c138,
    Values = 0x53b09e104f60,
    Get = 0x5fb9c3aba7ad,
    Insert = 0x8aa318eb7eaa,
    ContainsKey = 0x2074015bf301,
} }

sum! { EntrypointEffect {
    PrintText = 0x83914590a586 { text: 0x982d9e3eb996, span: 0x37a0025aa1f2 },
} }

model_with_omissions! { EnumDeclaration {
    record_package_identity: 0x64508989d4cc,
    native_item: 0xf12fd2a70b90,
    direct_declaration_id: 0xa6b05d0bcaad,
    canonical: 0x0deeb8fa1dbb,
    name: 0x82a3537ff0db,
    public: 0xefa1f375d761,
    type_parameters: 0xa1c4e8ba64bd,
    native_type_arguments: 0xd2eb5c55af3e,
    variants: 0xdad569e101ad,
    derives: 0xa88fe6ec6115,
    named_type_identities: 0x1e61b087a864,
} default [linked_layout_witness] }

model! { EnumVariantDeclaration {
    direct_declaration_id: 0xa6b05d0bcaad,
    canonical: 0x0deeb8fa1dbb,
    name: 0x82a3537ff0db,
    fields: 0xbfe5d697162d,
} }

model! { EnumVariantTarget {
    enum_name: 0xda34c190c141,
    variant_name: 0xee46be2f00b8,
    enum_canonical: 0xf0003214c967,
    variant_canonical: 0x147e39df0083,
    binding: 0x80f70afeef3c,
} }

sum! { ErrorKind {
    AssertionError = 0x16b0fd395786,
    ValueError = 0xb6276ba72bbc,
    TypeError = 0x7af339841d33,
    ZeroDivisionError = 0x7bb3f1819e84,
    IndexError = 0xa20dcf583f56,
    KeyError = 0xf0f0edf867a4,
    JsonDecodeError = 0xbd57d6ebd3f9,
} }

model! { ExplicitInputBinding {
    declaration: 0xabcd0ee1fc2c,
    identity: 0x689f6a627384,
    access: 0xa0561fd649cd,
} }

model! { FieldMetadata {
    name: 0x82a3537ff0db,
    record_identity: 0x45b066b79a4d,
    record_canonical: 0xbdeb90f55061,
    renamed_from: 0xcd457f2e581d,
    renamed_from_lifecycle: 0xcf5a34b43709,
    position: 0x5a452459503b,
    default_is_empty: 0xdad08f7938e2,
    alias: 0x1a0a6a36ca0a,
    description: 0xc9046f7a37ad,
    type_name: 0x2501a7dde456,
    has_default: 0x682a777c2328,
} }

model! { FieldlessEnumDeclaration {
    direct_declaration_id: 0xa6b05d0bcaad,
    canonical: 0x0deeb8fa1dbb,
    name: 0x82a3537ff0db,
    variants: 0xdad569e101ad,
} }

model! { FieldlessEnumVariantDeclaration {
    direct_declaration_id: 0xa6b05d0bcaad,
    canonical: 0x0deeb8fa1dbb,
    name: 0x82a3537ff0db,
} }

model! { FieldlessEnumVariantTarget {
    enum_declaration_id: 0xd44af7ffba27,
    enum_canonical: 0xf0003214c967,
    variant_declaration_id: 0xe0e343d940a0,
    variant_canonical: 0x147e39df0083,
    enum_name: 0xda34c190c141,
    variant_name: 0xee46be2f00b8,
} }

sum! { FloatMethodId {
    Sqrt = 0x7ac7d1f3de31,
    Abs = 0xef68818cc22e,
    Floor = 0xd268c9c44920,
    Ceil = 0x8e6b04e04615,
    Round = 0xcd9558acfc38,
    Sin = 0x0250af7e4079,
    Cos = 0x09ae01caf77c,
    Tan = 0x1eaa40752329,
    Exp = 0xda8a9b93366b,
    Ln = 0x6fd2a6981e2e,
    Log2 = 0x8ed7999269e3,
    Log10 = 0xb311188588d3,
    IsNan = 0xd7d586413cbc,
    IsInfinite = 0x29fc7ecae36c,
    IsFinite = 0xc32f3603e35e,
    Powi = 0x2b2c9bee87a7,
    Powf = 0xa8c2efe2bdc3,
} }

sum! { FormatPart {
    Literal = 0xa6230c2a81ab (arg0: 0xcc1d0de5a773),
    Expr = 0x08103d8108b8 { operand: 0x0b081d24f7f5, style: 0xcb86eb2d914d },
} }

sum! { FormatStyle {
    Display = 0x34e108c0896d,
    TypeParameterDisplay = 0xd8e97cd1abf1,
    Debug = 0x1a03bd2fd107,
} }

sum! { FrozenBytesMethodId {
    Len = 0x58fa4ba6b2e3,
    IsEmpty = 0x93230514d1ce,
    Decode = 0x1ae3fcff1a58,
} }

sum! { FrozenDictMethodId {
    Len = 0x58fa4ba6b2e3,
    IsEmpty = 0x93230514d1ce,
    ContainsKey = 0x2074015bf301,
} }

sum! { FrozenListMethodId {
    Len = 0x58fa4ba6b2e3,
    IsEmpty = 0x93230514d1ce,
} }

sum! { FrozenSetMethodId {
    Len = 0x58fa4ba6b2e3,
    IsEmpty = 0x93230514d1ce,
    Contains = 0x2eaecb3d0cf1,
} }

model! { GeneratorBody {
    source_local: 0xe483ab695fec,
    capture_locals: 0xb60aecd27639,
    stmts: 0xbabf02d55ffe,
} }

model! { GlobalPlace {
    identity: 0x689f6a627384,
    ty: 0x4f9db7f8e506,
    write_policy: 0x01e3322c615f,
} }

sum! { GlobalWritePolicy {
    ReadOnly = 0x9d696bc71c27,
    ProjectionOnly = 0x4e9db1534b2e,
    Rebindable = 0x68e343e0eff9,
} }

sum! { HelperOp {
    StrConcat = 0xd60b2e091174,
    StrEq = 0xf82a19af8784,
    StrNe = 0x7e84d50fa9b2,
    StrLt = 0xc535db49371c,
    StrLe = 0x64dc26957158,
    StrGt = 0x1f0c45e77be3,
    StrGe = 0x433b6242abbf,
    StrUpper = 0xd4b942e4e9a0,
    StrLower = 0xfcb9a6e62169,
    StrStrip = 0xcd1df4cfe073,
    StrLen = 0xfb49cf5bc977,
    StrReplace = 0xaac9aade3d59,
    StrJoin = 0x4bee2a9ea252,
    StrSplit = 0x8b6eb15b9442,
    StrContains = 0xc1633d591cd2,
    StrNotContains = 0x069111b03aec,
    ListConcat = 0x3d3143c6db44,
    ListRepeat = 0x8cc1ef0b8c7e,
    ListContains = 0x4f7b4e844edc,
    ListNotContains = 0x5334371029fe,
    SetContains = 0xbd8d950fe946,
    SetNotContains = 0x3088e8ed2e9c,
    DictContainsKey = 0x7f4ffac9aeb8,
    DictNotContainsKey = 0x49cd46dc1da4,
    RangeValidate = 0xd6d00319d7f0,
    NewtypeConstraintValidate = 0xa46124f8b645,
    NewtypeHookExtract = 0x5416b59a3b3c,
    NewtypeFieldHookExtract = 0x1906fb50634d,
    ValidationErrorNew = 0x1c8daac3cc3c,
    ValidationErrorWithCode = 0xa1d0e3d39f1b,
    NewtypeSumAdd = 0xcfef0332a350,
    NewtypeSumExtract = 0x88619c6e6023,
    CallableName = 0xef2a85512645,
    RaiseValueError = 0x9625db3f8ea0,
    TextCodecEncodingValid = 0xf2110dafbf90,
    TextCodecErrorsValid = 0x249203949f1f,
    InstallPanicHook = 0xce3f1b1f2d41,
    RangeListFill = 0xf9670dffc3c6,
} }

sum! { IntegerHelperFamily {
    Checked = 0x0efd92a33590,
    Wrapping = 0xbb1999072b6e,
    Saturating = 0xbb576d32d66f,
} }

sum! { IntegerHelperOperation {
    Add = 0x9fd728c66c9a,
    Sub = 0x21c1eb891c68,
    Mul = 0x082acfbd6d9b,
    Pow = 0x2f8df4920880,
} }

model! { IsInstanceRvalue {
    value: 0xcd42404d52ad,
    value_ty: 0xd794097dc090,
    target: 0x34a04005bcaf,
} }

model! { IsInstanceTarget {
    ty: 0x4f9db7f8e506,
    canonical: 0x0deeb8fa1dbb,
    span: 0x37a0025aa1f2,
} }

sum! { IterProtocol {
    Builtin = 0xa6a56071a42b,
    UserDefined = 0x4caa20a50bfb { next_method: 0x5cc32ed6e8b7, fallible: 0xd2007e94cd13 },
} }

sum! { IteratorMethodId {
    Iter = 0x084d1b796e70,
    Map = 0xbe176b0015c4,
    Filter = 0x638e249f4a15,
    Enumerate = 0x6d41343c8b28,
    Zip = 0x7bc88ebb6d01,
    Take = 0x8b862ba45dce,
    Skip = 0x28d03596d24e,
    TakeWhile = 0x1a32de3ad43d,
    SkipWhile = 0x85e134597988,
    Chain = 0xdae0896cbc2c,
    FlatMap = 0x3feecdf92400,
    Batch = 0x320b5d80f8cf,
    Collect = 0x08a0c33f913a,
    Count = 0x37bac4952bec,
    Reduce = 0x42412678202e,
    Fold = 0x92c122b5b1e2,
    Any = 0x2b505597daa7,
    All = 0xa52ace420f21,
    Find = 0x822b2ae4542b,
    ForEach = 0x560c9e47b124,
    Sum = 0xa21d7df04c33,
} }

model! { LinkedStaticAccessors {
    load: 0x0cf67fc72b3c,
    store: 0x824d80d71985,
    initialize_module: 0x67dd510071ed,
    guarded: 0xe7da2135b6d3,
} }

model! { LinkedStaticGuardAccessors {
    force: 0x32500d7500ce,
    with_ref: 0x880707d8081a,
    with_mut: 0x8548f8cb1f36,
} }

sum! { ListMethodId {
    Append = 0xfc15cc0a8046,
    Extend = 0x496a37e3a10b,
    Clone = 0x5779f32fab00,
    Pop = 0x4d616d53988e,
    Contains = 0x2eaecb3d0cf1,
    Swap = 0x6ec282d40a8a,
    Reserve = 0x5ae6f30c297b,
    ReserveExact = 0x742f6bf5b6d8,
    Remove = 0xc3812fc4acb8,
    Count = 0x37bac4952bec,
    Index = 0x39c3be1198a2,
} }

model_with_omissions! { LoafCallable {
    canonical: 0x0deeb8fa1dbb,
    item: 0x4a33eacd5fa6,
    parameters: 0xf528212a496d,
    return_type: 0x407ddf95ec74,
    type_parameters: 0xa1c4e8ba64bd,
    native_type_arguments: 0xd2eb5c55af3e,
    native_callable_parameters: 0xf8e16aa81d8e,
    parameter_defaults: 0x50426f5eb2fd,
} default [declaration_frame] }

model! { LoafTraitCall {
    trait_identity: 0x24c0715f724c,
    native_item: 0xf12fd2a70b90,
    type_arguments: 0x09ba95abc806,
    receiver_type: 0x58e4421134ca,
} }

model! { LocalCallableTarget {
    operand: 0x0b081d24f7f5,
    binding: 0x80f70afeef3c,
} }

model! { LocalDecl {
    id: 0xa56145270ce6,
    name: 0x82a3537ff0db,
    identity: 0x689f6a627384,
    ty: 0x4f9db7f8e506,
    origin: 0x181fdd46fc4a,
    scope: 0x5f161c914988,
    span: 0x37a0025aa1f2,
} }

tuple_record!(LocalId, 0xcc1d0de5a773);

sum! { LocalOrigin {
    Parameter = 0xdd066ec1bf1d,
    UserBinding = 0x679a915f553a,
    Temporary = 0xa29c13b95838,
    External = 0x68c114ea9c8c,
    Receiver = 0x86cec081a528 { mutable: 0x1da805f3d03b },
    Captured = 0x8a03fa9ad749,
} }

model! { MatchArm {
    pattern: 0x1fd38d5cbd2a,
    guard_stmts: 0x206b75b05177,
    guard: 0xa9d4034da07d,
    body_stmts: 0x655aac5de8d9,
    result: 0xf6a214f7a5fc,
} }

model! { MethodPartialDefaults {
    name: 0x82a3537ff0db,
    span: 0x37a0025aa1f2,
    target: 0x34a04005bcaf,
    frame: 0x9dff50df08c6,
} }

model! { MethodTarget {
    name: 0x82a3537ff0db,
    intrinsic_carrier: 0x59d50b58b211,
    operation: 0x9bf5a24e4aa7,
    loaf_trait: 0xa5298ba24f06,
    trait_arguments: 0xad8f795794aa,
    canonical: 0x0deeb8fa1dbb,
    type_args: 0xb06bd0a0bba8,
    native_trait_delegation: 0xde3b32b0ed9b,
    binding: 0x80f70afeef3c,
} }

model! { NamedCallableTarget {
    name: 0x82a3537ff0db,
    direct_call_id: 0x7e8f06e505b0,
    builtin: 0x2f0177192e8b,
    type_args: 0xb06bd0a0bba8,
    binding: 0x80f70afeef3c,
    canonical: 0x0deeb8fa1dbb,
    receiver_type: 0x58e4421134ca,
} }

model! { NativeCallableParameter {
    parameter_slot: 0x58e80a8ec504,
    type_argument_slot: 0xbfad8291b6ba,
} }

model! { NativeClosureStorage {
    move_capture: 0xba600ae61e3d,
    send: 0x27ce1d1bf427,
    sync: 0x75c75efe327a,
} }

model! { NativeUnionDeclaration {
    ty: 0x4f9db7f8e506,
    rust_item: 0xfa8609f12af5,
    members: 0x17373ca1c763,
    closure_members: 0xddf4b4b7f47f,
    closure_items: 0xb51b90659a78,
} }

sum! { NewtypeDeserialize {
    Unselected = 0x26118cfc7e5b,
    Ordinary = 0xf46f36f72dc6,
    Checked = 0x0efd92a33590 (arg0: 0xcc1d0de5a773),
} }

sum! { NewtypeStringConversion {
    Unselected = 0x26118cfc7e5b,
    Automatic = 0xd461a493a375,
} }

model_with_omissions! { NominalDeclaration {
    record_package_identity: 0x64508989d4cc,
    trait_adoptions: 0x812fff9c3cf4,
    native_item: 0xf12fd2a70b90,
    class_inheritance: 0xa979b0141676,
    native_type_arguments: 0xd2eb5c55af3e,
    direct_declaration_id: 0xa6b05d0bcaad,
    canonical: 0x0deeb8fa1dbb,
    name: 0x82a3537ff0db,
    fields: 0xbfe5d697162d,
    field_identities: 0xbb82664ede2c,
    field_types: 0x1a335cea15f9,
    field_public: 0x0bc433f3aa0b,
    field_aliases: 0x9a4fedcf67e4,
    reflection_fields: 0x089c33151881,
    reflection_metadata: 0x1b4fa396b0a9,
    comparison_equality: 0x3090d6c408b6,
    public: 0xefa1f375d761,
    has_field_defaults: 0x04cc6b2d1eea,
    field_default_body: 0x1362e32f1f0c,
    validated_constructor: 0xc1283f7df89f,
    method_partial_defaults: 0x5be2c1333222,
    display_method: 0xfd6f8ea7fde2,
    newtype_deserialize: 0x720330582318,
    newtype_string_conversion: 0x7fb111cf2a56,
    derives: 0xa88fe6ec6115,
    named_type_identities: 0x1e61b087a864,
    type_parameters: 0xa1c4e8ba64bd,
    type_parameter_count: 0xe794f9dcef77,
} default [linked_layout_witness, owned_callable_fields] }

model! { NominalPatternTarget {
    direct_declaration_id: 0xa6b05d0bcaad,
    canonical: 0x0deeb8fa1dbb,
    name: 0x82a3537ff0db,
} }

model! { NumericNewtypeConstant {
    constructor: 0xe3c1703abf8a,
    value: 0xcd42404d52ad,
} }

sum! { NumericResizeMethodId {
    Lossless = 0x4b86dab6fdea,
    Try = 0x85d6c0718cfe,
    Wrapping = 0xbb1999072b6e,
    Saturating = 0xbb576d32d66f,
} }

sum! { Operand {
    Place = 0xe9463dccf054 (arg0: 0xcc1d0de5a773),
    Constant = 0x8ab253e4aab6 (arg0: 0xcc1d0de5a773),
} }

sum! { OptionMethodId {
    AsMut = 0xb7790fad7d64,
    Copied = 0x8d525e5f158b,
    UnwrapOr = 0x9c8a0a948406,
    Unwrap = 0xe6876edcd9ef,
    IsSome = 0x5fb3176be435,
    IsNone = 0xdac89e101e21,
    Clone = 0x5779f32fab00,
} }

sum! { OwnershipFact {
    Copy = 0xe21f935f11d7,
    Move = 0x6ecc3df6bffd,
    Clone = 0x5779f32fab00,
    Borrow = 0x2edfb168b38b,
    MutBorrow = 0xa83721325fde,
    Unknown = 0xb764cdc0eab7,
} }

model! { PanicFact {
    span: 0x37a0025aa1f2,
    reason: 0x8fcfccf675b1,
} }

sum! { PanicReason {
    AssertFailure = 0x0a649b75e41c,
    DivisionOrModulo = 0x2e15c1e0e453,
    HelperMayPanic = 0x723f9ac4ffe2 (arg0: 0xcc1d0de5a773),
} }

sum! { Pattern {
    Wildcard = 0xd073b0d3cb4c,
    Var = 0x60529803e0f9 (arg0: 0xcc1d0de5a773),
    Literal = 0xa6230c2a81ab (arg0: 0xcc1d0de5a773),
    Tuple = 0xf136ee78c33b (arg0: 0xcc1d0de5a773),
    Struct = 0x5e95797a7202 { canonical: 0x0deeb8fa1dbb, name: 0x82a3537ff0db, fields: 0xbfe5d697162d },
    Nominal = 0x1b9ffa438066 { target: 0x34a04005bcaf, fields: 0xbfe5d697162d },
    FieldlessEnumVariant = 0xec6eb705c5e0 (arg0: 0xcc1d0de5a773),
    Result = 0x6e7d50e84f47 { variant: 0xcb7bf562420b, fields: 0xbfe5d697162d },
    Enum = 0x54108dafaac5 { canonical: 0x0deeb8fa1dbb, name: 0x82a3537ff0db, variant: 0xcb7bf562420b, fields: 0xbfe5d697162d },
    Or = 0x0568bd03b8eb (arg0: 0xcc1d0de5a773),
    UnionMember = 0x502ad682f179 { ty: 0x4f9db7f8e506, fields: 0xbfe5d697162d },
} }

model! { PatternBinding {
    local: 0x25bf8e1a2393,
    fact: 0x1e7dc6d6c165,
    last_use: 0x19f5528fa6eb,
} }

model! { Place {
    root: 0x4813494d137e,
    projection: 0x1b250ea199be,
} }

sum! { PlaceElem {
    Field = 0xf45fc1dfdc96 { name: 0x82a3537ff0db, canonical: 0x0deeb8fa1dbb, structural: 0xadf0d3f5f1ad },
    Index = 0x39c3be1198a2 (arg0: 0xcc1d0de5a773),
    Slice = 0x87bd78650264 { start: 0xcced28c6dc3f, end: 0x361e48d0308f, step: 0x3e64cc41cf8e },
    UnionMember = 0x502ad682f179 { ty: 0x4f9db7f8e506 },
    OptionPayload = 0xcba2720d44bf { option_type: 0x7ac951f9a7be, payload_type: 0xd5b5006cfc1e },
    Deref = 0x122f9bd768a6,
} }

model! { PlaceOperand {
    place: 0x81df63580cc9,
    fact: 0x1e7dc6d6c165,
    last_use: 0x19f5528fa6eb,
} }

sum! { PlaceRoot {
    Local = 0x8c31e6e72230 (arg0: 0xcc1d0de5a773),
    Global = 0xa258b30f88c3 (arg0: 0xcc1d0de5a773),
} }

model! { PrimitiveTraitAdapter {
    receiver_type: 0x58e4421134ca,
    payload_field: 0x911e80a2d76f,
    decode_slot: 0xe4e882d27a32,
    decoded_value: 0xd56e2b5c6324,
} }

model! { ProviderActivation {
    provider_key: 0x02b5508dea12,
    module_path: 0x815194995bb8,
    state: 0x4ba69735ca53,
} }

sum! { ProviderActivationState {
    Active = 0x92340695899b,
    Disabled = 0x75081b593d15,
    Unavailable = 0xca1844969742,
} }

model! { ProviderOperationInput {
    slot: 0x6558838331b7,
    written_position: 0x4e4e06b077a7,
    ty: 0x4f9db7f8e506,
    span: 0x37a0025aa1f2,
} }

model! { ProviderOperationPlan {
    operation: 0x9bf5a24e4aa7,
    provider: 0x5c4c1964340a,
    required_capability: 0x55fb6d5218fe,
    runtime_requirements: 0x679751f43980,
    inputs: 0x32381523e631,
    call_span: 0xe9acdc0e85b0,
} }

model! { RaceArm {
    awaitable: 0xeb77607252d4,
    binding: 0x80f70afeef3c,
    body: 0x230d8358dc8e,
    result: 0xf6a214f7a5fc,
} }

model! { RecordPackageIdentity {
    registry_id: 0x81b7027670fb,
    scoped_name: 0x72a97b1c2336,
} }

sum! { ReflectionOperation {
    Metadata = 0x9eddf573cb50,
    TypedValue = 0x81622539b15e,
    TypedItems = 0x07d7505b7071,
    StringValue = 0x24b828daee6b,
    StringItems = 0x8c0604546476,
    GenericMetadata = 0x596861c28979 (arg0: 0xcc1d0de5a773),
    Builtin = 0xa6a56071a42b (arg0: 0xcc1d0de5a773),
} }

sum! { ResultMethodId {
    Map = 0xbe176b0015c4,
    MapErr = 0xc43bc584f637,
    AndThen = 0x59f577e957c4,
    OrElse = 0xc92bcb778c36,
    Inspect = 0xe0723a86a5b9,
    InspectErr = 0x1eb70223dfd2,
    Unwrap = 0xe6876edcd9ef,
    UnwrapOr = 0x9c8a0a948406,
    IsOk = 0xbc4c05c97fde,
    IsErr = 0xe0ffc2314a27,
} }

model! { ResultVariant {
    kind: 0x0b27c158feb3,
    payload: 0x239f59ed55e7,
    ok_type: 0x4656d5933fa4,
    error_type: 0xe5bdba7c7bee,
    canonical_types: 0xf0cdd579ff40,
} }

sum! { ResultVariantKind {
    Ok = 0x843ac01149cc,
    Err = 0xf265d4d8f5e4,
} }

model! { ReturnedView {
    slot: 0x6558838331b7,
    access: 0xa0561fd649cd,
    component: 0x6985ca1f4daa,
    carried: 0x309074dc4898,
} }

sum! { Rvalue {
    Use = 0xc36d819e7bc6 (arg0: 0xcc1d0de5a773),
    UnaryOp = 0xe71613916b9e (arg0: 0xcc1d0de5a773, arg1: 0x84ff873bc51d),
    BinaryOp = 0xfb9707415731 (arg0: 0xcc1d0de5a773, arg1: 0x84ff873bc51d, arg2: 0x2092f891c0cc),
    IsInstance = 0x9185d38b964b (arg0: 0xcc1d0de5a773),
    Aggregate = 0x3c9193d0b2a4 (arg0: 0xcc1d0de5a773, arg1: 0x84ff873bc51d),
    Dict = 0x9611d3d800fc (arg0: 0xcc1d0de5a773),
    ValueEnumVariant = 0x088691c8a605 (arg0: 0xcc1d0de5a773),
    FieldlessEnumVariant = 0xec6eb705c5e0 (arg0: 0xcc1d0de5a773),
    ResultVariant = 0x125e93d27fed (arg0: 0xcc1d0de5a773),
    Format = 0x2f343666aaa8 (arg0: 0xcc1d0de5a773),
    Closure = 0xe90d617d2b55 { params: 0xa20b52fae57c, captured_operands: 0xde6e9b044079, body: 0x230d8358dc8e },
    Generator = 0x1ade60abe93f { source: 0x41cf6794ba42, captured_operands: 0xde6e9b044079, body: 0x230d8358dc8e },
    Match = 0x03c0e806becb { scrutinee: 0x84fa17e0c3ba, arms: 0xd86982d2329a },
} }

tuple_record!(ScopeId, 0xcc1d0de5a773);

model! { ScopeInfo {
    id: 0xa56145270ce6,
    parent: 0xe47125968b3b,
    span: 0x37a0025aa1f2,
} }

sum! { SetMethodId {
    Add = 0x9fd728c66c9a,
    Contains = 0x2eaecb3d0cf1,
} }

model! { SpreadElement {
    source: 0x41cf6794ba42,
    kind: 0x0b27c158feb3,
} }

sum! { SpreadKind {
    Sequence = 0x0740f4bade2a,
    Mapping = 0x1ff9a5f8b4bd,
} }

model! { Statement {
    kind: 0x0b27c158feb3,
    span: 0x37a0025aa1f2,
} }

sum! { StatementKind {
    UnsafeRegion = 0x7aff0a0af161 { acknowledgment: 0x29f2793914d5, body: 0x230d8358dc8e },
    Assign = 0x8ece895c9b32 { place: 0x81df63580cc9, rvalue: 0x74da0d307b24 },
    Call = 0xd6e645b7d2b2 { destination: 0xb5c755aaab10, callee: 0x92ca4c0ced62, args: 0x090772cf4068, may_panic: 0x47b982b90ce1 },
    Drop = 0xc8af9ae37a48 { local: 0x25bf8e1a2393 },
    If = 0x1e3abf61a37e { cond: 0x0ab2dd2f64c9, then_block: 0xfd767693d3d6, else_block: 0x0fabfcfc7fe8 },
    Loop = 0xf2f6a0188995 { body: 0x230d8358dc8e },
    Break = 0xf71081fb0af4 { value: 0xcd42404d52ad },
    Continue = 0x31fbef162594,
    Return = 0x6b0143d03864 { value: 0xcd42404d52ad },
    Await = 0xaa0e84884b9f { destination: 0xb5c755aaab10, awaited: 0x54f93ded12d9 },
    Race = 0x73134ce12829 { destination: 0xb5c755aaab10, arms: 0xd86982d2329a },
    Yield = 0x347de0eefc8c { value: 0xcd42404d52ad },
    Assert = 0x661a49aed53e { kind: 0x0b27c158feb3, message: 0xab530a13e459, may_panic: 0x47b982b90ce1 },
    Expr = 0x08103d8108b8 { value: 0xcd42404d52ad },
    TryPropagate = 0x8ccaf4b8fe59 { destination: 0xb5c755aaab10, operand: 0x0b081d24f7f5, error_routing: 0xdb0947172832 },
    IterNext = 0xaab9a2088d91 { destination: 0xb5c755aaab10, iterator: 0xef9baedff2f4, protocol: 0x2ea88c7a3035 },
    Unsupported = 0x54324658e2eb { description: 0xc9046f7a37ad },
} }

model! { StaticDeclaration {
    canonical: 0x0deeb8fa1dbb,
    ty: 0x4f9db7f8e506,
    initial: 0xac1b5c0961a7,
} }

sum! { StaticInitializer {
    Literal = 0xa6230c2a81ab (arg0: 0xcc1d0de5a773),
    List = 0x6f202f54a7b2 (arg0: 0xcc1d0de5a773),
    Dict = 0x9611d3d800fc (arg0: 0xcc1d0de5a773),
    Set = 0xb6f6f3ad07b3 (arg0: 0xcc1d0de5a773),
    Computed = 0x7a15102667bd (arg0: 0xcc1d0de5a773),
    Linked = 0xbfda026e6c59 (arg0: 0xcc1d0de5a773),
} }

sum! { StringConversionContract {
    Unselected = 0x26118cfc7e5b,
    Inactive = 0xac7c949f1211,
    Selected = 0x57fd7a0cf33f (arg0: 0xcc1d0de5a773),
} }

model! { StringConversionSelection {
    trait_identity: 0x24c0715f724c,
    method_identity: 0xc6c1cf3f70ac,
} }

sum! { StringMethodId {
    Upper = 0xada6d7a662f0,
    Lower = 0x4e8bbf3a2bea,
    Strip = 0x58f0cabedaa2,
    Replace = 0x95e154398a4b,
    Join = 0xfd30fe681b4b,
    ToString = 0x2d487ef37f84,
    SplitWhitespace = 0x28b042e1a3bd,
    Split = 0x32afaa784333,
    Contains = 0x2eaecb3d0cf1,
    StartsWith = 0x590e1b9e8927,
    EndsWith = 0x1513505285ef,
    Len = 0x58fa4ba6b2e3,
    IsEmpty = 0x93230514d1ce,
    Encode = 0x23389a1f5476,
    Slice = 0x87bd78650264,
} }

model! { TraitAdoption {
    trait_identity: 0x24c0715f724c,
    arguments: 0xb192017da47f,
    methods: 0x3f3867313a9d,
    fields: 0xbfe5d697162d,
    primitive_adapter: 0x0fd97be20ef1,
} }

model! { TraitFieldImplementation {
    requirement: 0xf8dcb7a13bf4,
    implementation: 0xda31012c4033,
} }

model! { TraitImplementation {
    owner: 0x4c1029697ee3,
    method: 0x5b7e6bf2dc4a,
    implementation: 0xda31012c4033,
    slot_arguments: 0xac76d7a307b9,
    type_arguments: 0x09ba95abc806,
} }

model! { TraitTypeArgument {
    parameter: 0x6390361dfdff,
    argument: 0xb74f83c9353b,
} }

sum! { TryErrorRouting {
    SameType = 0x0c3b7e50d15d { error_type: 0xe5bdba7c7bee },
    ConversionRequired = 0x8577fb80e8d0 { source_error_type: 0xe635edd7d1e1, destination_error_type: 0x71755dcb334f },
    Unresolved = 0x36afca802c4d,
} }

model! { TypeAliasDeclaration {
    canonical: 0x0deeb8fa1dbb,
    ty: 0x4f9db7f8e506,
    named_type_identities: 0x1e61b087a864,
} }

sum! { TypeMetadataOperation {
    ClassName = 0x44f64edac626,
    FieldCount = 0xa78acb8eebb4,
    Fields = 0x616f48c1db19,
} }

model! { TypeMetadataTarget {
    member: 0xe31ab643c44f,
    owner: 0x4c1029697ee3,
    receiver_type: 0x58e4421134ca,
    operation: 0x9bf5a24e4aa7,
} }

sum! { TypedNumericConstant {
    Signed = 0x08251562b340 { kind: 0x0b27c158feb3, value: 0xcd42404d52ad },
    Unsigned = 0x9daf8e05c684 { kind: 0x0b27c158feb3, value: 0xcd42404d52ad },
    F32 = 0x82255aee6ccc { bits: 0xe5e59d10f196 },
    F64 = 0xa44d7f74c829 { bits: 0xe5e59d10f196 },
    Decimal = 0x18517d332d1b { precision: 0x68c2f9ee3147, scale: 0xf469802a3144, coefficient: 0x7ef50ace94f8, literal_scale: 0xe43e9686fd8e },
} }

sum! { UnOp {
    Neg = 0xee02b7e6deb6,
    Not = 0xa3cb7fba638e,
    Invert = 0x7e04f2ffb933,
} }

model! { UnsafeAcknowledgment {
    receipt: 0x6f32860910ca,
    span: 0x37a0025aa1f2,
    scope: 0x5f161c914988,
} }

model! { ValidatedConstructor {
    constructor: 0xe3c1703abf8a,
    validator: 0xf82af32160bc,
    required_fields: 0xb2451d4d544c,
} }

sum! { ValueEnumBacking {
    Int = 0x0b5f608070c6,
    Str = 0x8084a51b3c64,
} }

model! { ValueEnumDeclaration {
    direct_declaration_id: 0xa6b05d0bcaad,
    canonical: 0x0deeb8fa1dbb,
    name: 0x82a3537ff0db,
    backing: 0xbc3347de8e8c,
    variants: 0xdad569e101ad,
} }

model! { ValueEnumVariantDeclaration {
    direct_declaration_id: 0xa6b05d0bcaad,
    canonical: 0x0deeb8fa1dbb,
    name: 0x82a3537ff0db,
    raw_value: 0xa1ae8a706465,
} }

model! { ValueEnumVariantTarget {
    enum_declaration_id: 0xd44af7ffba27,
    enum_canonical: 0xf0003214c967,
    variant_declaration_id: 0xe0e343d940a0,
    variant_canonical: 0x147e39df0083,
    enum_name: 0xda34c190c141,
    variant_name: 0xee46be2f00b8,
} }
