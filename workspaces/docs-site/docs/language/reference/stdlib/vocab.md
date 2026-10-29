# `std.vocab`

`std.vocab` provides vocabulary registration values and the versioned structured syntax exchanged with a native desugarer. Entry exports and execution policy are specified in [Vocabulary entries](../../../tooling/reference/vocabulary.md).

```incan
from std.vocab import VocabRegistration, DslSurface, DeclarationSurface, ClauseSurface
from std.vocab import DesugarRequest, DesugarResponse, DesugarOutput, DesugarError
```

## Versions and value conventions

| Constant | Type | Value |
| --- | --- | --- |
| `VOCAB_METADATA_VERSION` | `int` | `1` |
| `VOCAB_SYNTAX_VERSION` | `int` | `3` |

The field tables below give every model's public fields and Incan constructor defaults. A field without a default is required. Wire decoding also validates the response shape and can refuse omitted fields. Builder methods return the model named by their section; `Self` denotes that type. List-appending builders preserve argument order. Factory spellings must contain at least one whitespace-separated token.

Registration models expose `to_json(self) -> JsonValue`, except `VocabRegistration`, which exposes `metadata(self) -> JsonValue`. `KeywordActivation`, `KeywordPlacement`, `ClausePlacement`, `ScopedSurfaceSyntax` and `TypeRef` also expose `to_json(self) -> JsonValue`. String-valued descriptor enums expose `value(self) -> str`; `EmbeddedFragmentSubmode` also exposes the syntax serialization methods below. The syntax models and syntax enums expose `to_json(self) -> str` and `from_json(source: str) -> Result[Self, str]`; decoding failure returns an error string. `Span` is a syntax model. `ScopedSurfaceReceiver` exposes these syntax serialization methods and a separate `descriptor_json(self) -> JsonValue`.

## Registration

### `VocabRegistration`

| Field | Type | Default |
| --- | --- | --- |
| `metadata_version` | `int` | `VOCAB_METADATA_VERSION` |
| `keyword_registrations` | `list[KeywordRegistration]` | `[]` |
| `dsl_surfaces` | `list[DslSurface]` | `[]` |
| `library_manifest` | `LibraryManifest` | `LibraryManifest()` |
| `custom_desugaring` | `bool` | `false` |

| Method | Return | Behavior |
| --- | --- | --- |
| `new()` | `Self` | Empty registration. |
| `with_surface(self, surface: DslSurface)` | `Self` | Append one surface. |
| `with_surfaces(self, surfaces: list[DslSurface])` | `Self` | Append surfaces. |
| `with_keyword_registration(self, registration: KeywordRegistration)` | `Self` | Append one low-level keyword group. |
| `with_keyword_registrations(self, registrations: list[KeywordRegistration])` | `Self` | Append keyword groups. |
| `with_library_manifest(self, manifest: LibraryManifest)` | `Self` | Replace additional library metadata. |
| `with_desugarer(self)` | `Self` | Require the entry's public typed `desugar` export. |
| `metadata(self)` | `JsonValue` | Encode metadata, deriving keyword groups from the same surface descriptors. |

### `DslSurface`

Fields: `activation: KeywordActivation = KeywordActivation.Always`, `declarations: list[DeclarationSurface] = []`, `embedded_fragments: list[EmbeddedFragmentDescriptor] = []`, `scoped_surfaces: list[ScopedSurfaceDescriptor] = []`.

| Method | Return | Behavior |
| --- | --- | --- |
| `new(activation: KeywordActivation)` | `Self` | Empty surface with explicit activation. |
| `on_import(namespace: str)` | `Self` | Activate at an import of `namespace` or of a namespace that encloses it. |
| `always_on()` | `Self` | Active from the first line of every module of a consumer that loads the descriptors, without an import. |
| `with_declaration(self, declaration: DeclarationSurface)` | `Self` | Append one declaration. |
| `with_declarations(self, declarations: list[DeclarationSurface])` | `Self` | Append declarations. |
| `with_embedded_fragment(self, fragment: EmbeddedFragmentDescriptor)` | `Self` | Append an owned fragment descriptor. |
| `with_scoped_surface(self, surface: ScopedSurfaceDescriptor)` | `Self` | Append one scoped descriptor. |
| `with_scoped_surfaces(self, surfaces: list[ScopedSurfaceDescriptor])` | `Self` | Append scoped descriptors. |
| `keyword_registration(self)` | `KeywordRegistration` | Derive the surface's keyword group. |

### `DeclarationSurface`

| Field | Type | Default |
| --- | --- | --- |
| `keyword` | `str` | Required |
| `compound_tokens` | `list[str]` | `[]` |
| `placement` | `KeywordPlacement` | `KeywordPlacement.TopLevel` |
| `head_kind` | `DeclarationHeadKind` | `DeclarationHeadKind.NoHead` |
| `body_kind` | `DeclarationBodyKind` | `DeclarationBodyKind.Mixed` |
| `desugars_to` | `DesugarTarget` | `DesugarTarget.Statements` |
| `clauses` | `list[ClauseSurface]` | `[]` |
| `declarations` | `list[DeclarationSurface]` | `[]` |

| Method | Return | Behavior |
| --- | --- | --- |
| `named(spelling: str)` | `Self` | Split the first token and compound tail. |
| `new(keyword: str)` | `Self` | Use one keyword without splitting. |
| `with_header_args(self)` | `Self` | Select header arguments. |
| `with_signature_head(self)` | `Self` | Select a signature head. |
| `with_head_kind(self, kind: DeclarationHeadKind)` | `Self` | Set head kind. |
| `with_clause_body(self)` | `Self` | Select clauses. |
| `with_statement_body(self)` | `Self` | Select statements. |
| `with_mixed_body(self)` | `Self` | Select mixed bodies. |
| `with_body_kind(self, kind: DeclarationBodyKind)` | `Self` | Set body kind. |
| `desugars_to_expression(self)` | `Self` | Select expression output. |
| `desugars_to_statements(self)` | `Self` | Select statement output. |
| `desugars_to_declarations(self)` | `Self` | Select compilation-unit declaration output. |
| `with_target(self, target: DesugarTarget)` | `Self` | Set output position. |
| `in_block(self, parent: str)` | `Self` | Restrict placement to one parent. |
| `with_placement(self, placement: KeywordPlacement)` | `Self` | Set placement. |
| `with_compound_tokens(self, tokens: list[str])` | `Self` | Replace compound tail. |
| `with_clause(self, clause: ClauseSurface)` | `Self` | Append one clause. |
| `with_clauses(self, clauses: list[ClauseSurface])` | `Self` | Append clauses. |
| `with_declaration(self, declaration: DeclarationSurface)` | `Self` | Append a declaration restricted to this parent. |
| `with_declarations(self, declarations: list[DeclarationSurface])` | `Self` | Append declarations restricted to this parent. |
| `keywords(self)` | `list[KeywordSpec]` | Derive declaration, clause and nested declaration keywords. |

A native request for a declaration with a signature head or decorators is refused. In native requests, `head.parameters` is empty, `head.return_type` is `None`, and clause bodies are `Empty`, `Expression`, `ExpressionList` or `Items`.

### `ClauseSurface`

Fields: required `keyword: str`; `compound_tokens: list[str] = []`; `body_kind: ClauseBodyKind = ClauseBodyKind.Expression`; `expression_item_modifiers: list[ExpressionItemModifierSurface] = []`; `cardinality: ClauseCardinality = ClauseCardinality.Optional`; `placement: ClausePlacement = ClausePlacement.Anywhere`.

| Method | Return | Behavior |
| --- | --- | --- |
| `new(keyword: str, body_kind: ClauseBodyKind)` | `Self` | Explicit keyword and payload; an `ExpressionList` payload starts with the `as` alias modifier. |
| `from_spelling(spelling: str, body_kind: ClauseBodyKind)` | `Self` | Split first token and compound tail. |
| `expr(spelling: str)` | `Self` | Expression payload. |
| `expr_list(spelling: str)` | `Self` | Expression-list payload with the `as` alias modifier. |
| `fields(spelling: str)` | `Self` | Field-set payload. |
| `type_ref(spelling: str)` | `Self` | Type payload. |
| `nested_items(spelling: str)` | `Self` | Nested-item payload. |
| `required(self)` | `Self` | Required occurrence. |
| `optional(self)` | `Self` | Optional occurrence. |
| `repeating(self)` | `Self` | Repeated occurrences. |
| `before(self, other: str)` | `Self` | Set relative ordering. |
| `after(self, other: str)` | `Self` | Set relative ordering. |
| `with_compound_tokens(self, tokens: list[str])` | `Self` | Replace compound tail. |
| `with_expression_item_modifier(self, modifier: ExpressionItemModifierSurface)` | `Self` | Append one item modifier unless one with the same keyword is present. |
| `with_expression_item_modifiers(self, modifiers: list[ExpressionItemModifierSurface])` | `Self` | Append item modifiers, keeping the first for each keyword. |

`ExpressionItemModifierSurface` has `keyword: str` and `kind: ExpressionItemModifierKind = ExpressionItemModifierKind.Alias`. Its factories `alias(keyword: str) -> Self` and `expr(keyword: str) -> Self` select an identifier alias or expression payload.

### Descriptor enums

| Type | Alternatives |
| --- | --- |
| `KeywordActivation` | `Always`, `OnImport(str)`; `on_import(namespace: str) -> Self`. |
| `KeywordPlacement` | `TopLevel`, `InBlock(list[str])`; `in_block(parents: list[str]) -> Self`. |
| `KeywordSurfaceKind` | `FunctionDecl`, `TypeDecl`, `ConditionalChain`, `ForLoop`, `WhileLoop`, `MatchBlock`, `TryBlock`, `ImportStatement`, `ControlFlow`, `BindingDecl`, `BlockDeclaration`, `BlockContextKeyword`, `SubBlock`. |
| `DeclarationHeadKind` | `NoHead` (serialized as `None`), `HeaderArguments`, `Signature`, `Mixed`. |
| `DeclarationBodyKind` | `Clauses`, `Statements`, `Mixed`. |
| `ClauseBodyKind` | `Expression`, `ExpressionList`, `Type`, `FieldSet`, `NestedItems`. |
| `ClauseCardinality` | `Required`, `Optional`, `Repeating`. |
| `ClausePlacement` | `Anywhere`, `Before(str)`, `After(str)`. |
| `DesugarTarget` | `Declarations`, `Statements`, `Expression`. |
| `ExpressionItemModifierKind` | `Alias`, `Expression`. |
| `EmbeddedFragmentSubmode` | `Markup`, `Style`, `RawText`, `RegexTemplate`, `SelectorDeclarationValue`, `TypePosition`. |

`KeywordSurfaceKind`, the head/body kinds, cardinality, targets, modifier kinds and fragment submodes are string-valued enums; their string values equal their alternative names except for `DeclarationHeadKind.NoHead`.

## Scoped and embedded descriptors

### `ScopedSurfaceDescriptor`

Fields: required `key: str`, `syntax: ScopedSurfaceSyntax`; `family: ScopedSurfaceFamily = ScopedSurfaceFamily.OperatorLike`; `eligible_in: list[ScopedSurfaceEligibility] = []`; `receiver: Option[ScopedSurfaceReceiver] = None`; `pairwise: bool = false`.

| Method | Return | Behavior |
| --- | --- | --- |
| `operator(key: str, glyph: str)` | `Self` | Operator-like glyph. |
| `binding(key: str, glyph: str)` | `Self` | Binding-like glyph. |
| `leading_dot_path(key: str)` | `Self` | Expression form, at least one segment, no maximum. |
| `in_declaration_body(self, declaration: str)` | `Self` | Append declaration-body eligibility. |
| `with_eligibilities(self, eligible_in: list[ScopedSurfaceEligibility])` | `Self` | Append positive positions. |
| `with_receiver(self, receiver: ScopedSurfaceReceiver)` | `Self` | Replace receiver context. |
| `pairwise_chain(self)` | `Self` | Select pairwise-chain formatting. |

`ScopedSurfaceFamily` is a string-valued enum with `OperatorLike`, `BindingLike` and `ExpressionForm`. `ScopedSurfaceSyntax` has `Glyph(str)` and `LeadingDotPath(int, Option[int])`; the latter specifies minimum and optional maximum segment counts, each fitting an unsigned 16-bit integer. A scoped surface is recognized only in its `eligible_in` positions. Elsewhere, its glyph is ordinary Incan syntax and produces no vocabulary diagnostic.

`ScopedSurfaceReceiver` has `OwningDeclaration`, `Clause(str)` and `Custom(str)`. `custom(key: str) -> Self` constructs the custom alternative. A receiver names context for the owning desugarer; it does not evaluate that context.

### `ScopedSurfaceEligibility`

Fields: required `declaration: str`; `clause: Option[str] = None`; `call: Option[str] = None`; `position: str = "DeclarationBody"`.

`call_argument(declaration: str, call: str) -> Self` selects `CallArgument` position and sets the call. Descriptor positions are `DeclarationBody`, `ClauseBody` and `CallArgument`.

### `EmbeddedFragmentDescriptor`

Fields: required `key: str`, `submode: EmbeddedFragmentSubmode`, `artifact_key: str`; `eligible_in: list[ScopedSurfaceEligibility] = []`; `layout_sensitive: bool = false`.

| Method | Return | Behavior |
| --- | --- | --- |
| `new(key: str, submode: EmbeddedFragmentSubmode, artifact_key: str)` | `Self` | Explicit fragment identity and grammar. |
| `in_declaration_body(self, declaration: str)` | `Self` | Append declaration-body position. |
| `in_clause_body(self, declaration: str, clause: str)` | `Self` | Append clause-body position. |
| `in_call_argument(self, declaration: str, call: str)` | `Self` | Append call-argument position. |
| `with_eligibility(self, eligibility: ScopedSurfaceEligibility)` | `Self` | Append an explicit position. |

An embedded block body occupies statement position. When the providing registration calls `with_desugarer()`, its desugarer receives the fragment through the declaration. Otherwise the fragment and its expression holes are checked, and executable compilation refuses the fragment.

## Syntax and source ranges

All syntax alternatives below have derived JSON serialization. Unit enum alternatives serialize as strings, payload alternatives as single-key objects, and tuple payloads as arrays. Model fields serialize under their declared names. Optional fields serialize as `null` when absent.

`Span` has required `start: int` and `end: int`. They are half-open byte offsets in the requesting module, with `0 <= start <= end`. Every output range must lie inside the requesting vocabulary node. A `null` range is refused. Distinct output nodes may share a source range, including nested calls with different result types. Diagnostics retain the original-source coordinates. Providers supply only `start` and `end`; additional span fields are refused.

### Expressions

`IncanExpr` has required `span: Span` and `kind: IncanExprKind`.

| Factory | Return | Source range |
| --- | --- | --- |
| `path(name: str, span: Span)` | `Self` | Explicit range; splits dotted names into path segments. |
| `call(callee: IncanExpr, arguments: list[IncanExpr])` | `Self` | Inherits the callee's range; retains each argument's range. |
| `list(items: list[IncanExpr], span: Span)` | `Self` | Explicit enclosing range, including for an empty list. |

```incan
pub enum IncanExprKind:
    NoneLiteral
    Bool(bool)
    Int(int)
    Float(float)
    Str(str)
    Path(list[str])
    List(list[IncanExpr])
    Tuple(list[IncanExpr])
    Dict(list[Tuple[IncanExpr, IncanExpr]])
    Call(list[IncanExpr])
    Attribute(list[IncanExpr], str)
    Index(list[IncanExpr])
    Binary(str, list[IncanExpr])
    Unary(str, list[IncanExpr])
    Embedded(VocabFragment)
    Scoped(VocabScopedSurface)
```

`Path` requires at least one segment. `Call` contains the callee followed by positional arguments. `Attribute` has one receiver; `Index` and `Binary` have two operands; `Unary` has one. Invalid operand counts are refused. Integer payloads must fit a signed 64-bit value; floating payloads must be finite JSON numbers.

Binary operator spellings are `+`, `-`, `*`, `/`, `//`, `%`, `**`, `@`, `|>`, `<|`, `&`, `|`, `^`, `<<`, `>>`, `==`, `!=`, `<`, `>`, `<=`, `>=`, `and`, `or`, `in`, `not in`, `is` and `is not`. Unary spellings are `-`, `not` and `~`. Unknown operators are refused. Ordinary type and effect checks apply to the resulting expression.

`Embedded` and `Scoped` are input payloads. A custom expansion must replace them with ordinary output syntax. Passing either through in a response is refused. Input expressions admit the ordinary alternatives above, parentheses, descriptor-owned glyphs and leading-dot paths, and calls without type arguments or non-positional arguments. Spreads and other unrepresented expression forms are refused.

| Scoped syntax type | Fields or alternatives |
| --- | --- |
| `VocabScopedSurface` | Required `dependency_key: str`, `descriptor_key: str`, `owner: VocabSurfaceOwner`, `kind: VocabScopedSurfaceKind`. |
| `VocabSurfaceOwner` | Required `declaration: str`; `clause: Option[str] = None`; `call: Option[str] = None`. |
| `VocabScopedSurfaceKind` | `LeadingDotPath(list[str], ScopedSurfaceReceiver)`, `Glyph(str, list[IncanExpr])`. A glyph has left and right operands. |

### Statements

`IncanStatement` has required `span: Span` and `kind: IncanStatementKind`.

```incan
pub enum IncanStatementKind:
    Pass
    Expr(IncanExpr)
    Return(Option[IncanExpr])
    Assign(str, IncanExpr)
    Let(str, bool, IncanExpr)
    TypedLet(str, bool, VocabTypeExpr, IncanExpr)
    If(IncanExpr, list[IncanStatement], list[IncanStatement])
    While(IncanExpr, list[IncanStatement])
    For(str, IncanExpr, list[IncanStatement])
    Unsafe(list[IncanStatement])
```

`Assign` writes an existing binding. `Let` declares a binding; its boolean selects mutability. `If` contains condition, then-body and else-body. `For` binds its named variable. `TypedLet` is serializable as a model value but is refused in native worker output. Native requests carry `Pass`, `Expr`, `Return`, `Assign` and `Let` body statements; a request whose body holds any other statement, including an annotated assignment, is refused.

### Compilation-unit output

`IncanDeclaration` has required `span: Span`, `name: str`, `kind: IncanDeclarationKind`, and `is_public: bool = false`.

```incan
pub enum IncanDeclarationKind:
    Function(list[VocabParameter], VocabTypeExpr, list[IncanStatement])
    Model(list[VocabFieldSpec])
    Const(Option[VocabTypeExpr], IncanExpr)
```

`Function` contains fixed immutable parameters, an explicit return type and ordinary body statements. `Model` contains public fields with required type annotations and optional defaults. `Const` contains an optional annotation and its initializer. Ordinary declaration checks apply. Declaration output requires a `Declarations` descriptor and compilation-unit placement; output inside a function is refused. Declaration ranges and child ranges are retained independently.

Names must start with an alphabetic character or underscore and continue with alphanumeric characters or underscores. Output declarations do not represent methods, decorators, type parameters, async modifiers or trait adoption. `VocabTypeExpr` represents simple or dotted names and their ordered type arguments; `None` without arguments denotes the unit type.

| Type | Fields |
| --- | --- |
| `VocabTypeExpr` | Required `span: Span`, `name: str`; `arguments: list[VocabTypeExpr] = []`. |
| `VocabParameter` | Required `span: Span`, `name: str`; `param_type: Option[VocabTypeExpr] = None`, `default_value: Option[IncanExpr] = None`. An absent annotation requests inference and remains subject to ordinary declaration checks. |
| `VocabFieldSpec` | Required `span: Span`, `name: str`; `field_type: Option[VocabTypeExpr] = None`, `default_value: Option[IncanExpr] = None`. Model output requires `field_type`. |

### Parsed vocabulary nodes

| Type | Fields |
| --- | --- |
| `VocabDeclarationHead` | Required `span: Span`; `name: Option[str] = None`, `header_args: list[IncanExpr] = []`, `parameters: list[VocabParameter] = []`, `return_type: Option[VocabTypeExpr] = None`. |
| `VocabDeclaration` | Required `span: Span`, `keyword: str`, `head: VocabDeclarationHead`; `compound_tokens: list[str] = []`, `body: list[VocabBodyItem] = []`. |
| `VocabClause` | Required `span: Span`, `keyword: str`, `body: VocabClauseBody`; `compound_tokens: list[str] = []`, `head: list[IncanExpr] = []`. |
| `VocabExpressionItem` | Required `span: Span`, `expr: IncanExpr`; `alias: Option[str] = None`, `modifiers: list[VocabExpressionItemModifier] = []`. |
| `VocabExpressionItemModifier` | Required `span: Span`, `keyword: str`, `value: IncanExpr`. |

```incan
pub enum VocabSyntaxNode:
    Declaration(VocabDeclaration)
    Clause(VocabClause)
    Statement(IncanStatement)
    Expression(IncanExpr)

pub enum VocabBodyItem:
    Clause(VocabClause)
    Declaration(VocabDeclaration)
    Statement(IncanStatement)

pub enum VocabClauseBody:
    Empty
    Expression(IncanExpr)
    ExpressionList(list[VocabExpressionItem])
    Type(VocabTypeExpr)
    FieldSet(list[VocabFieldSpec])
    Items(list[VocabBodyItem])
```

Native block requests use `VocabSyntaxNode.Declaration`. An initial identifier header argument becomes `head.name`; the remaining arguments become `head.header_args`. Non-identifier first arguments remain in `header_args`. Compound keyword tails remain separate from the first keyword.

| Method | Return | Behavior and errors |
| --- | --- | --- |
| `VocabSyntaxNode.span(self)` | `Span` | The contained node's original range. |
| `VocabDeclaration.clause(self, keyword: str)` | `Result[VocabClause, DesugarError]` | First directly owned clause with that exact first keyword; absence is a declaration-local error. |
| `VocabDeclaration.fragment(self)` | `Result[VocabFragment, DesugarError]` | First directly owned embedded expression statement; absence is a declaration-local error. |
| `VocabClause.expression(self)` | `Result[IncanExpr, DesugarError]` | Value of an `Expression` body; other bodies produce a clause-local error. |
| `VocabClause.head_list(self)` | `list[IncanExpr]` | Expression-list values, one expression-body value, or the clause's head values. Aliases and modifiers remain on the original expression-list items. |

### Embedded fragments

| Type | Required fields |
| --- | --- |
| `VocabFragment` | `span: Span`, `dependency_key: str`, `descriptor_key: str`, `submode: EmbeddedFragmentSubmode`, `nodes: list[VocabFragmentNode]`, `source_text: str`, `layout_sensitive: bool`. |
| `VocabFragmentNode` | `span: Span`, `kind: VocabFragmentNodeKind`. |
| `VocabFragmentElement` | `span: Span`, `name: str`, `attributes: list[VocabFragmentAttribute]`, `children: list[VocabFragmentNode]`, `self_closing: bool`. |
| `VocabFragmentAttribute` | `span: Span`, `name: str`, `value: list[VocabFragmentNode]`. |
| `VocabFragmentType` | `span: Span`, `kind: VocabFragmentTypeKind`. |

```incan
pub enum VocabFragmentNodeKind:
    Text(str)
    Hole(IncanExpr)
    Element(VocabFragmentElement)
    EntityRef(str)
    Comment(str)
    StyleRule(list[VocabFragmentNode], list[VocabFragmentNode])
    Declaration(str, list[VocabFragmentNode])
    Value(VocabFragmentValue)
    Regex(str, str)
    TypeShape(VocabFragmentType)

pub enum VocabFragmentValue:
    Dimension(str, str)
    Color(str)
    CustomPropertyRef(str)
    Ident(str)
    StringLit(str)
    Number(str)
    Selector(str)

pub enum VocabFragmentTypeKind:
    Name(list[str])
    Generic(list[VocabFragmentType])
    Nullable(list[VocabFragmentType])
    Array(list[VocabFragmentType])
    Union(list[VocabFragmentType])
```

`StyleRule` contains selectors and declarations. `Regex` contains pattern and flags. `CustomPropertyRef` contains the bare `--name` without `var(...)` delimiters. `Generic` contains its constructor as its first child; `Nullable` and `Array` contain one child. Expression holes retain ordinary Incan expressions and their original ranges. `source_text` is the fragment's original source text.

## Requests, responses and worker service

| Type | Fields or alternatives |
| --- | --- |
| `DesugarRequest` | Required `node: VocabSyntaxNode`, `module_path: str`; `schema_version: int = VOCAB_SYNTAX_VERSION`. |
| `DesugarResponse` | Required `output: DesugarOutput`; `schema_version: int = VOCAB_SYNTAX_VERSION`. |
| `DesugarOutput` | `Statements(list[IncanStatement])`, `Expression(IncanExpr)`, `Declarations(list[IncanDeclaration])`. |
| `DesugarError` | Required `span: Span`, `message: str`; `at(span: Span, message: str) -> Self`. The span must lie inside the requesting node, or the reply is refused. |
| `DesugarBatchRequest` | Required `requests: list[DesugarRequest]`. |
| `DesugarReply` | `Ok(DesugarResponse)`, `Err(DesugarError)`. |
| `DesugarBatchResponse` | Required `responses: list[DesugarReply]`. |

```incan
pub def run_desugar_worker(
    desugar: (DesugarRequest) -> Result[DesugarResponse, DesugarError]
) -> Result[None, str]
```

The service reads one JSON batch per stdin line, invokes the callback independently for each request, and flushes one response line in the same order. End of input returns `Ok(None)`. Input decoding, I/O and output flushing failures return an error string. The generated worker entry calls this service; a vocabulary author's callback returns one typed response or diagnostic per node.

## Low-level keyword registration

`KeywordSpec` has required `name: str`; `surface_kind: KeywordSurfaceKind = KeywordSurfaceKind.BlockDeclaration`; `compound_tokens: list[str] = []`; `placement: KeywordPlacement = KeywordPlacement.TopLevel`.

Factories: `new(name: str, surface_kind: KeywordSurfaceKind) -> Self`, `block(name: str) -> Self`, `block_context(name: str, parent: str) -> Self`, `sub_block(name: str, parent: str) -> Self`.

Builders: `in_block(self, parent: str) -> Self`, `in_blocks(self, parents: list[str]) -> Self`, `with_placement(self, placement: KeywordPlacement) -> Self`, `with_compound_tokens(self, tokens: list[str]) -> Self`.

`KeywordRegistration` has `activation: KeywordActivation = KeywordActivation.Always`, `keywords: list[KeywordSpec] = []`, `valid_decorators: list[str] = []`.

Factories: `new(activation: KeywordActivation) -> Self`, `on_import(namespace: str) -> Self`, `always_on() -> Self`.

Builders: `with_keyword(self, keyword: KeywordSpec) -> Self`, `with_keywords(self, keywords: list[KeywordSpec]) -> Self`, `with_valid_decorator(self, decorator: str) -> Self`, `with_valid_decorators(self, decorators: list[str]) -> Self`. These append their arguments.

## Additional library metadata

These registration values encode additional published library metadata. They do not execute dependency resolution or replace `loaf.toml` dependencies. Their models and `TypeRef` expose `to_json(self) -> JsonValue`; `TypeExportKind` exposes `value(self) -> str`.

| Type | Fields or alternatives |
| --- | --- |
| `LibraryManifest` | `modules: list[ModuleExport] = []`; `new() -> Self`. Its encoded `format_version` is `V1`. |
| `TypeRef` | `Named(str)`, `Generic(str, list[TypeRef])`, `Optional(list[TypeRef])`, `Union(list[TypeRef])`; `named(name: str) -> Self`. `Optional` requires a nonempty list and serializes only its first element as its payload. |
| `FunctionExport` | Required `name: str`; `params: list[Tuple[str, TypeRef]] = []`, `return_type: Option[TypeRef] = None`, `is_async: bool = false`. |
| `TypeExportKind` | String-valued `Model`, `Class`, `Enum`, `Trait`, `Newtype`. |
| `FieldExport` | Required `name: str`, `field_type: TypeRef`; `has_default: bool = false`. |
| `TypeExport` | Required `name: str`; `kind: TypeExportKind = TypeExportKind.Model`, `type_params: list[str] = []`, `fields: list[FieldExport] = []`, `methods: list[FunctionExport] = []`. |
| `ModuleExport` | Required `path: str`; `functions: list[FunctionExport] = []`, `types: list[TypeExport] = []`. |

## See also

- [Vocabulary entries](../../../tooling/reference/vocabulary.md)
- [Author a vocabulary in Incan](../../../contributing/how-to/authoring_vocabularies.md)
- [Embedded language fragments](../../explanation/dsl_embeddings.md)
