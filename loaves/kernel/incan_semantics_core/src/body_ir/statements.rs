//! Helper operations, blocks, unsafe acknowledgments, statements and their rendering.

use super::*;

/// Compiler-owned runtime/helper operation: a source operator whose meaning is a call into the runtime rather than
/// a machine instruction. Represented here as an explicit Body IR operation rather than inferred later from
/// native call shapes.
///
/// Every membership operation states a runtime requirement Body IR needs and the runtime must satisfy. Each variant's
/// [`Self::as_str`] name is that requirement's name; it is not a promise that a Rust function of exactly that signature
/// already exists.
///
/// Membership and concatenation are named per operand type rather than shared across collections. One `Contains`
/// helper would oblige a consumer to re-derive string-versus-list-versus-set-versus-dict from operand types, which
/// is the inference this data model exists to replace; and one `Concat` would conflate a string join with a list
/// concatenation. Negation likewise gets its own variant per collection rather than a wrapper, so one source
/// operator stays one Body IR operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelperOp {
    StrConcat,
    StrEq,
    StrNe,
    StrLt,
    StrLe,
    StrGt,
    StrGe,
    /// `str.upper()` over a checked runtime string receiver.
    StrUpper,
    /// `str.lower()` over a checked runtime string receiver.
    StrLower,
    /// `str.strip()` over a checked runtime string receiver.
    StrStrip,
    /// `str.len()` over a checked runtime string receiver, counting Unicode scalar values.
    StrLen,
    /// `str.replace(from, to)` over a checked runtime string receiver.
    StrReplace,
    /// `separator.join(items)` over a checked runtime string receiver.
    StrJoin,
    /// `str.split(separator)` over a checked runtime string receiver.
    StrSplit,
    /// `needle in haystack` on two strings: substring containment, not element membership.
    ///
    /// Membership is the one operator whose meaning genuinely changes with its operand types — substring for `str`,
    /// element lookup for a collection — so it cannot be a [`BinOp`] without picking one of those meanings and
    /// silently applying it to the other. Naming the string policy as its own helper is what makes the substring
    /// choice a represented fact.
    ///
    /// **Argument order is the helper's own: `(haystack, needle)`.** Membership is the one string operator whose
    /// surface order is the reverse of its signature — `needle in haystack` reads needle-first, while `str_contains`
    /// takes the haystack first, as every `contains` in Rust does. Lowering swaps the operands so the call matches
    /// the function it names. Preserving source order instead would leave one helper out of nine disagreeing with
    /// its own signature, and a backend binding these positionally has no way to know which one.
    StrContains,
    /// `needle not in haystack` on two strings, with the same `(haystack, needle)` argument order as
    /// [`Self::StrContains`].
    ///
    /// A separate helper rather than a negated [`Self::StrContains`], following the [`Self::StrEq`]/[`Self::StrNe`]
    /// pair: one source operator stays one Body IR operation, so a consumer never has to recognize a negation
    /// wrapper to know which operator was written.
    StrNotContains,
    /// `xs + ys` on two lists: concatenation producing a new list, not a machine addition.
    ///
    /// Lists are the one non-string builtin whose `+` the typechecker accepts through a dedicated branch rather than
    /// through a `__add__` hook. Because that branch resolves no operator dispatch, nothing downstream would mark the
    /// operation as a call, and a plain [`BinOp::Add`] would have contradicted the Rust-emission backend outright:
    /// `determine_binop_plan` routes list `+` to `incan_std_primitives::collections::list_concat`, so no addition is
    /// emitted for it at all. Naming the concatenation as its own helper is what keeps the two backends stating the
    /// same thing.
    ///
    /// This is specifically *not* an argument that a collection operand cannot sit under a [`BinOp`]. That same
    /// backend emits comparisons as an infix operator, so `==` on two lists is faithfully a primitive, and refusing
    /// it would invent a divergence rather than close one.
    ///
    /// Arguments are in source order, `(lhs, rhs)`: concatenation is not commutative, and unlike the membership
    /// helpers there is no receiver convention pulling the operands apart.
    ListConcat,
    /// `list.repeat(value, count)`: a new list of `count` clones of `value`, raising `ValueError` for a negative
    /// count.
    ///
    /// `list` here is the builtin collection's type surface, not a list value, so the typechecker resolves the call
    /// to a compiler-owned collection helper rather than a method on any receiver. Arguments are in the helper's
    /// declared order, `(value, count)`, whatever order the call site wrote them in.
    ListRepeat,
    /// `v in xs` on a list: element containment, not substring containment.
    ///
    /// Membership is the one operator whose meaning genuinely changes with its operand types, so each collection
    /// names its own helper rather than sharing one `Contains`. A single variant would leave the backend to re-derive
    /// list-versus-set-versus-dict from operand types — exactly the inference this data model exists to replace with
    /// a represented fact.
    ///
    /// **Argument order is the helper's own: `(haystack, needle)`**, matching [`Self::StrContains`] and every
    /// `contains` in Rust, while the source spelling reads needle-first. Lowering swaps the operands so all
    /// membership helpers agree with each other and with their own signatures.
    ListContains,
    /// `v not in xs` on a list, with the same `(haystack, needle)` argument order as [`Self::ListContains`].
    ///
    /// A separate helper rather than a negated [`Self::ListContains`], following the
    /// [`Self::StrContains`]/[`Self::StrNotContains`] pair: one source operator stays one Body IR operation, so a
    /// consumer never has to recognize a negation wrapper to know which operator was written.
    ListNotContains,
    /// `v in xs` on a set: element containment, with the same `(haystack, needle)` argument order as
    /// [`Self::ListContains`].
    ///
    /// Distinct from [`Self::ListContains`] despite the identical source spelling, because the containers are
    /// different types with different lookup costs and different Rust receivers. The operation records which one the
    /// source actually held.
    SetContains,
    /// `v not in xs` on a set, with the same `(haystack, needle)` argument order as [`Self::SetContains`].
    SetNotContains,
    /// `k in d` on a dict: **key** containment, with the same `(haystack, needle)` argument order as
    /// [`Self::ListContains`].
    ///
    /// Named `ContainsKey` rather than `Contains` because dict membership tests keys while the sibling collections
    /// test elements. Leaving that to be inferred from the receiver's type would make key-versus-value a backend
    /// convention; naming it here makes it a represented fact, and matches the `contains_key` the operation lowers
    /// toward.
    DictContainsKey,
    /// `k not in d` on a dict, with the same `(haystack, needle)` argument order as [`Self::DictContainsKey`].
    DictNotContainsKey,
    /// Validate normalized builtin range bounds and step using the runtime constructor's error contract.
    RangeValidate,
    /// Fail-fast validation of a constrained newtype's saved input: checked predicates, target label, constraint label.
    NewtypeConstraintValidate,
    /// Extract a checker-selected source hook's successful wrapper or raise its legacy validation failure.
    NewtypeHookExtract,
    /// Extract a sole supplied local model field's hook result, retaining its canonical target and field labels.
    ///
    /// Arguments are the checked Result, target label, and field label. Multi-field collection and constrained or
    /// chained ingress require separate retained computations and cannot use this operation.
    NewtypeFieldHookExtract,
    /// Construct the dependency-owned builtin validation error from its checked message.
    ValidationErrorNew,
    /// Construct the dependency-owned builtin validation error from its checked message and code.
    ValidationErrorWithCode,
    /// Add two retained numeric-newtype payloads with the frozen release summation bridge's wrapping arithmetic.
    NewtypeSumAdd,
    /// Extract the one post-exhaustion newtype hook result, retaining the frozen sum failure message.
    NewtypeSumExtract,
    /// Read the source name of one checked callable value. Type reflection and ordinary fields remain separate.
    /// Consumers validate its concrete function-pointer signature and name-resolution profile before admission.
    CallableName,
    /// Invoke the canonical core ValueError primitive without a result; consumers must terminate the normal edge.
    RaiseValueError,
    /// Test a builtin codec's encoding label before evaluating later policy or receiver expressions.
    TextCodecEncodingValid,
    /// Test a builtin decoder's error policy after its encoding guard and before its receiver.
    TextCodecErrorsValid,
    /// Register a checked native function pointer as the standard panic hook; the native boundary boxes it.
    InstallPanicHook,
    /// Fill a list comprehension's scalar literal over a checked zero-based, unit-step range.
    /// Its arguments retain the literal and exact nonnegative cardinality; the source span remains the comprehension's.
    RangeListFill,
}

impl HelperOp {
    /// Return the canonical Body-IR operation admitted for one checked string-method identity.
    ///
    /// The `StringMethodId` originates in the typechecker, so this projection deliberately accepts an identity
    /// rather than a source spelling. `None` keeps every string method outside the selected #1256 subset from
    /// acquiring a helper operation merely because a later stage recognizes its text.
    pub const fn for_selected_string_method(method: StringMethodId) -> Option<Self> {
        match method {
            StringMethodId::Upper => Some(Self::StrUpper),
            StringMethodId::Lower => Some(Self::StrLower),
            StringMethodId::Strip => Some(Self::StrStrip),
            StringMethodId::Len => Some(Self::StrLen),
            StringMethodId::Replace => Some(Self::StrReplace),
            StringMethodId::Join => Some(Self::StrJoin),
            StringMethodId::Split => Some(Self::StrSplit),
            StringMethodId::Contains => Some(Self::StrContains),
            StringMethodId::ToString
            | StringMethodId::SplitWhitespace
            | StringMethodId::StartsWith
            | StringMethodId::EndsWith
            | StringMethodId::IsEmpty
            | StringMethodId::Encode
            | StringMethodId::Slice => None,
        }
    }

    /// Compact snapshot spelling for this helper operation, also used as the [`AbiV0RuntimeRequirement::RuntimeHelper`]
    /// name so callers building runtime-requirement facts stay on the same helper naming as the snapshot renderer.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RangeValidate => "range_validate",
            Self::NewtypeConstraintValidate => "newtype_constraint_validate",
            Self::NewtypeHookExtract => "newtype_hook_extract",
            Self::NewtypeFieldHookExtract => "newtype_field_hook_extract",
            Self::ValidationErrorNew => "validation_error_new",
            Self::ValidationErrorWithCode => "validation_error_with_code",
            Self::NewtypeSumAdd => "newtype_sum_add",
            Self::NewtypeSumExtract => "newtype_sum_extract",
            Self::CallableName => "callable_name",
            Self::RaiseValueError => "raise_value_error",
            Self::TextCodecEncodingValid => "text_codec_encoding_valid",
            Self::TextCodecErrorsValid => "text_codec_errors_valid",
            Self::InstallPanicHook => "install_panic_hook",
            Self::StrConcat => "str_concat",
            Self::StrEq => "str_eq",
            Self::StrNe => "str_ne",
            Self::StrLt => "str_lt",
            Self::StrLe => "str_le",
            Self::StrGt => "str_gt",
            Self::StrGe => "str_ge",
            Self::StrUpper => "str_upper",
            Self::StrLower => "str_lower",
            Self::StrStrip => "str_strip",
            Self::StrLen => "str_len",
            Self::StrReplace => "str_replace",
            Self::StrJoin => "str_join",
            Self::StrSplit => "str_split",
            Self::StrContains => "str_contains",
            Self::StrNotContains => "str_not_contains",
            Self::ListConcat => "list_concat",
            Self::ListRepeat => "list_repeat",
            Self::RangeListFill => "range_list_fill",
            Self::ListContains => "list_contains",
            Self::ListNotContains => "list_not_contains",
            Self::SetContains => "set_contains",
            Self::SetNotContains => "set_not_contains",
            Self::DictContainsKey => "dict_contains_key",
            Self::DictNotContainsKey => "dict_not_contains_key",
        }
    }
}

// ============================================================================
// Statements and blocks
// ============================================================================

/// A normalized block of statements within a body, scoped to one [`ScopeId`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub scope: ScopeId,
    pub stmts: Vec<Statement>,
}

impl Block {
    /// Check each direct acknowledgment against this block's existing scope before a consumer lowers its operations.
    /// Nested control-flow blocks are checked when consumed, and nested acknowledgment regions recurse here.
    ///
    /// `module_path` is the path of the source module whose body holds this block, which minted its receipts; for a
    /// body imported from another module it is not the unit's entry module.
    pub fn unsafe_acknowledgments_valid(&self, module_path: &str) -> bool {
        self.stmts.iter().all(|statement| match &statement.kind {
            StatementKind::UnsafeRegion { acknowledgment, body } => {
                acknowledgment.matches_region(module_path, &statement.span, self.scope)
                    && body.scope == self.scope
                    && body.unsafe_acknowledgments_valid(module_path)
            }
            _ => true,
        })
    }
}

/// Source acknowledgment of one `unsafe:` region, binding its receipt to the module, full span, and enclosing scope.
///
/// This records source syntax, not a capability grant or an authority decision. The enclosing
/// [`StatementKind::UnsafeRegion`] retains the exact operations being acknowledged; a consumer must validate this
/// receipt before admitting them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnsafeAcknowledgment {
    /// Compiler-owned statement identity minted from the source module and the region's range and expansion context.
    pub receipt: CompilerNodeId,
    /// Original region span; generated operations retain their individual spans inside this boundary.
    pub span: HirSourceSpan,
    /// Existing Incan scope containing the region; `unsafe:` introduces no separate binding scope.
    pub scope: ScopeId,
}

impl UnsafeAcknowledgment {
    /// Validate the source receipt against its owning module, enclosing scope, and statement span before admission.
    ///
    /// `module_path` is the path of the source module that minted the receipt. A matching receipt does not authorize
    /// a provider operation or admit an otherwise unsupported nested statement.
    pub fn matches_region(&self, module_path: &str, span: &HirSourceSpan, scope: ScopeId) -> bool {
        self.span == *span && self.scope == scope && self.receipt == CompilerNodeId::statement_at(module_path, *span)
    }
}

/// Render a block's statements at the given indentation depth (in two-space units).
pub(super) fn render_block(out: &mut String, block: &Block, depth: usize) {
    let indent = "  ".repeat(depth);
    for stmt in &block.stmts {
        render_statement(out, stmt, &indent, depth);
    }
}

/// Render one statement, recursing into nested blocks for control-flow statements.
pub(super) fn render_statement(out: &mut String, stmt: &Statement, indent: &str, depth: usize) {
    match &stmt.kind {
        StatementKind::UnsafeRegion { acknowledgment, body } => {
            let _ = writeln!(
                out,
                "{indent}unsafe acknowledgment={} scope={}",
                acknowledgment.receipt, acknowledgment.scope.0
            );
            render_block(out, body, depth + 1);
        }
        StatementKind::Assign { place, rvalue } => {
            let _ = writeln!(
                out,
                "{indent}{} = {}",
                place.render_snapshot(),
                rvalue.render_snapshot()
            );
        }
        StatementKind::Call {
            destination,
            callee,
            args,
            may_panic,
        } => render_call(out, destination.as_ref(), callee, args, *may_panic, indent),
        StatementKind::Await { destination, awaited } => {
            let dest = destination
                .as_ref()
                .map(|place| format!("{} = ", place.render_snapshot()))
                .unwrap_or_default();
            let _ = writeln!(out, "{indent}{dest}await {}", awaited.render_snapshot());
        }
        StatementKind::Race { destination, arms } => {
            render_race(out, destination.as_ref(), arms, indent, depth);
        }
        StatementKind::Drop { local } => {
            let _ = writeln!(out, "{indent}drop _{}", local.0);
        }
        StatementKind::If {
            cond,
            then_block,
            else_block,
        } => {
            let _ = writeln!(out, "{indent}if {}:", cond.render_snapshot());
            render_block(out, then_block, depth + 1);
            if let Some(else_block) = else_block {
                let _ = writeln!(out, "{indent}else:");
                render_block(out, else_block, depth + 1);
            }
        }
        StatementKind::Loop { body } => {
            let _ = writeln!(out, "{indent}loop:");
            render_block(out, body, depth + 1);
        }
        StatementKind::Break { value } => {
            let value_str = value.as_ref().map(Operand::render_snapshot).unwrap_or_default();
            let _ = writeln!(out, "{indent}break {value_str}");
        }
        StatementKind::Continue => {
            let _ = writeln!(out, "{indent}continue");
        }
        StatementKind::Return { value } => {
            let value_str = value.as_ref().map(Operand::render_snapshot).unwrap_or_default();
            let _ = writeln!(out, "{indent}return {value_str}");
        }
        StatementKind::Yield { value } => {
            let _ = writeln!(out, "{indent}yield {}", value.render_snapshot());
        }
        StatementKind::Assert {
            kind,
            message,
            may_panic,
        } => {
            let msg = message
                .as_ref()
                .map(|m| format!(", {}", m.render_snapshot()))
                .unwrap_or_default();
            let panic_marker = if *may_panic { " may_panic" } else { "" };
            let _ = writeln!(out, "{indent}assert {}{msg}{panic_marker}", kind.render_snapshot());
        }
        StatementKind::Expr { value } => {
            let _ = writeln!(out, "{indent}expr {}", value.render_snapshot());
        }
        StatementKind::TryPropagate {
            destination,
            operand,
            error_routing,
        } => {
            let _ = writeln!(
                out,
                "{indent}{} = try?({}, {})",
                destination.render_snapshot(),
                operand.render_snapshot(),
                error_routing.render_snapshot()
            );
        }
        StatementKind::IterNext {
            destination,
            iterator,
            protocol,
        } => {
            let _ = writeln!(
                out,
                "{indent}{} = iter_next({}, {})",
                destination.render_snapshot(),
                iterator.render_snapshot(),
                protocol.render_snapshot()
            );
        }
        StatementKind::Unsupported { description } => {
            let _ = writeln!(out, "{indent}unsupported({description})");
        }
    }
}

/// Render a call's optional destination, ordered arguments and retained panic marker.
pub(super) fn render_call(
    out: &mut String,
    destination: Option<&Place>,
    callee: &Callee,
    args: &[ArgumentElement],
    may_panic: bool,
    indent: &str,
) {
    let dest = destination
        .map(|place| format!("{} = ", place.render_snapshot()))
        .unwrap_or_default();
    let args_str: Vec<String> = args.iter().map(ArgumentElement::render_snapshot).collect();
    let panic_marker = if may_panic { " may_panic" } else { "" };
    let _ = writeln!(
        out,
        "{indent}{dest}call {}({}){panic_marker}",
        callee.render_snapshot(),
        args_str.join(", ")
    );
}

/// Render race arms at their existing nested depth, including each arm's result after its body.
pub(super) fn render_race(out: &mut String, destination: Option<&Place>, arms: &[RaceArm], indent: &str, depth: usize) {
    let dest = destination
        .map(|place| format!("{} = ", place.render_snapshot()))
        .unwrap_or_default();
    let _ = writeln!(out, "{indent}{dest}race:");
    for arm in arms {
        let _ = writeln!(out, "{indent}  {}", arm.render_header());
        render_block(out, &arm.body, depth + 2);
        let _ = writeln!(out, "{indent}  -> {}", arm.result.render_snapshot());
    }
}

/// How [`StatementKind::IterNext`] should poll one iteration, as selected by checked protocol facts: a builtin
/// collection needs no named method dispatch at all, while a user-defined iterable resolves concrete
/// `__iter__`/`__next__`-shaped method names through the typechecker's iteration-protocol resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IterProtocol {
    /// Iterate a builtin collection (`List`/`Dict`/`String`) or a range with no explicit method dispatch. How to
    /// concretely advance such an iterator is left to the consuming backend.
    Builtin,
    /// Iterate through a resolved user-defined iterator-protocol dispatch (`__iter__`/`__next__`-shaped magic
    /// methods).
    UserDefined {
        /// Method resolved on the iterator object to poll the next item (`iterator.__next__()`).
        next_method: String,
        /// Whether `next_method` returns a fallible `Result[Option[T], E]` rather than a plain `Option[T]` (`for item
        /// in iterable?:`, RFC 115). When `true`, [`StatementKind::IterNext`]'s implicit poll also carries an implicit
        /// early-return-with-conversion on the failure variant, mirroring [`StatementKind::TryPropagate`]'s own
        /// `From`/`Into` semantics, before the ordinary exhausted-vs-produced-a-value branch applies to the success
        /// payload.
        fallible: bool,
    },
}

impl IterProtocol {
    /// Compact snapshot spelling for this iteration protocol.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Builtin => "builtin".to_string(),
            Self::UserDefined { next_method, fallible } => {
                if *fallible {
                    format!("user_defined({next_method}, fallible)")
                } else {
                    format!("user_defined({next_method})")
                }
            }
        }
    }
}

/// One statement in a normalized Body IR block, carrying its own source span for diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: HirSourceSpan,
}

impl Statement {
    /// Copy a call with a selected callee and argument vector while preserving its destination, source span and
    /// panic fact. Non-call statements return None. This record projection lets Incan normalization use ordinary
    /// native calls without reconstructing a Rust enum's named fields through the frozen bootstrap (#2256).
    pub fn with_call(&self, callee: Callee, args: Vec<ArgumentElement>) -> Option<Self> {
        let StatementKind::Call {
            destination, may_panic, ..
        } = &self.kind
        else {
            return None;
        };
        Some(Self {
            kind: StatementKind::Call {
                destination: destination.clone(),
                callee,
                args,
                may_panic: *may_panic,
            },
            span: self.span,
        })
    }
}

/// Canonical statement vocabulary for Body IR v0.
///
/// `while`/`for` source statements desugar into `Loop` + a conditional `Break` during lowering, rather than being
/// represented as distinct statement kinds, so the canonical vocabulary has a single loop shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StatementKind {
    /// An explicit source `unsafe:` acknowledgment, with its receipt and nested operations in the enclosing scope.
    /// Consumers must validate the receipt and deliberately admit the region; ordinary statement flattening must not
    /// erase it.
    UnsafeRegion {
        /// Source receipt identifying exactly this region, without granting unrelated operation capabilities.
        acknowledgment: UnsafeAcknowledgment,
        /// Operations covered by the acknowledgment; its scope is the enclosing block's scope.
        body: Block,
    },
    /// Assign an rvalue into a place.
    Assign { place: Place, rvalue: Rvalue },
    /// Call a function, method, locally stored callable value, or runtime helper, optionally storing its result.
    Call {
        destination: Option<Place>,
        callee: Callee,
        args: Vec<ArgumentElement>,
        /// Whether this call is known to be able to panic (helper operations that check preconditions).
        may_panic: bool,
    },
    /// Explicit drop of a local whose value was never moved out of its declaring scope.
    Drop { local: LocalId },
    /// `if cond: then_block [else: else_block]`.
    If {
        cond: Operand,
        then_block: Block,
        else_block: Option<Block>,
    },
    /// Normalized loop body. `while`/`for` desugar into this plus a leading conditional `Break`.
    Loop { body: Block },
    /// Exit the innermost enclosing `Loop`, optionally producing a value (loop-expression support is deferred; v0
    /// only lowers `break` as a statement, so this is always `None` from lowering today but is modeled for forward
    /// compatibility with loop-expression support).
    Break { value: Option<Operand> },
    /// Skip to the next iteration of the innermost enclosing `Loop`.
    Continue,
    /// Return from the body, optionally with a value.
    Return { value: Option<Operand> },
    /// `await operand` — a suspension point in an async body.
    ///
    /// Deliberately **not** [`Self::Yield`]. A generator `yield` produces a value outward and has no destination;
    /// an `await` consumes an awaitable and writes the resumed value into `destination`. Sharing one node would make
    /// two different contracts indistinguishable, and a task runtime needs exactly the fact that separates them:
    /// where the body suspends and what it resumes with.
    ///
    /// Effect ordering across this point is source-observable — statements before it run before suspension,
    /// statements after it run after resumption — and is preserved by this statement's position in its block. The
    /// awaited operand carries its own [`OwnershipFact`] and last-use marker like any other read.
    ///
    /// This node says a suspension happens here. It says nothing about *how*: task/frame state, wake/resume routing,
    /// and scheduling are the executing backend's, tracked by #1155.
    Await {
        /// Where the resumed value is written.
        ///
        /// Frontend lowering always supplies a destination today, including for a discarded `await` in statement
        /// position: the resumed value gets a temporary that the surrounding statement then ignores. The option
        /// exists for a future producer that can prove the value is unobservable, not because one exists now.
        destination: Option<Place>,
        /// The awaitable being suspended on.
        awaited: Operand,
    },
    /// `race for value:` — concurrent await over several arms, resuming on the first to complete.
    ///
    /// The contract this node fixes, which arm ordering alone could not express:
    ///
    /// - every arm's awaitable is evaluated **before** any selection happens, in source order;
    /// - when two or more awaitables become ready in the same scheduler turn, the earliest source-order arm wins;
    /// - exactly one arm's `body` runs, the one whose awaitable completed first after applying that ready-tie rule;
    /// - every non-winning arm's awaitable is cancelled at this statement's boundary.
    ///
    /// As with [`Self::Await`], this records what the race *means*, not how it is driven: concurrent polling,
    /// first-completion detection, and the cancellation mechanism belong to #1155.
    Race {
        /// Where the winning arm's result is written. Always supplied by frontend lowering today, for the same
        /// reason as [`StatementKind::Await::destination`].
        destination: Option<Place>,
        /// The arms, in source order. Order resolves only same-turn ready ties — see this variant's docs.
        arms: Vec<RaceArm>,
    },
    /// `yield value` in statement position, suspending a generator body to produce one value.
    ///
    /// Only the statement-position form with a value is modeled -- see the module docs and
    /// [`Body::is_generator`]. A bare `yield` (no value) and expression-position `yield` (the two-way send/receive
    /// protocol, e.g. `x = yield val`) are out of scope for v0. A generator function's body needs no separate top-level
    /// state-machine node: it lowers through this same statement vocabulary, while the target runtime owns the concrete
    /// suspension mechanism. Generator expressions instead carry their own deferred [`Rvalue::Generator`] body. Neither
    /// representation asks a consumer to infer a suspension point from a target-language closure shape.
    Yield { value: Operand },
    /// One RFC 018 assertion in any of its three source forms, with the optional failure message and panic marker
    /// every form shares.
    ///
    /// The form-specific payload lives in [`AssertionKind`] rather than in three sibling statement kinds, because
    /// `message` and `may_panic` are invariant across all three forms and every assertion records the same
    /// [`PanicReason::AssertFailure`] fact and [`crate::AbiV0RuntimeRequirement::PanicStrategy`] requirement. Three
    /// sibling kinds would repeat those shared fields three times, let them drift apart, and force every exhaustive
    /// walk over this vocabulary to grow three arms for one concept; a consumer that only needs "is this an
    /// assertion" still matches one variant here, and one that cares which form matches the payload.
    Assert {
        /// Which assertion form this is, plus the operands and facts only that form carries.
        kind: AssertionKind,
        /// Optional failure message, applicable to all three forms.
        message: Option<Operand>,
        may_panic: bool,
    },
    /// An expression evaluated for its side effects only; its value is discarded.
    Expr { value: Operand },
    /// `operand?` (try/propagate). Evaluates `operand` (a `Result`-typed value; the current typechecker only
    /// allows `?` on `Result`, not `Option` -- see `validate_try_result_type` in
    /// `loaves/compiler/incan_frontend/src/typechecker/check_expr/control_flow.rs`). On the failure variant (`Err`),
    /// returns early from the enclosing function with the failure value, converting it via `From`/`Into` when the
    /// enclosing function's error type differs from `operand`'s, mirroring Rust's built-in `?` desugaring.
    /// Otherwise stores the unwrapped success value (`Ok(v)`'s `v`) into `destination` and falls through to the
    /// next statement.
    ///
    /// Modeled as a single compiler-owned primitive rather than decomposed into explicit `is_err`/`unwrap`-style
    /// calls, matching the same #653-criterion-3 rationale as [`Callee::Helper`]: this operation is a
    /// compiler-owned semantic, not something to be inferred later from native call shapes, and full
    /// call-target resolution for a manual decomposition is out of scope for v0 (see the module docs). Unlike
    /// [`Callee::Helper`] this needs no runtime-helper requirement: the conversion is Rust's own `From`/`Into`
    /// machinery, not a compiler-provided function call.
    TryPropagate {
        destination: Place,
        operand: Operand,
        /// Checked routing fact for the failure payload. A direct runtime supports only the exact same error type;
        /// a conversion remains represented so it can refuse without guessing an `Into`/`From` implementation.
        error_routing: TryErrorRouting,
    },
    /// Poll one iteration of a general (non-range) `for` loop or comprehension `for` clause, standing in for
    /// `iterator.next_method()` (or a builtin collection's implicit advance) plus the branch on its result -- the
    /// same #653-criterion-3 "compiler-owned semantic gets its own explicit node" treatment as
    /// [`Self::TryPropagate`], applied to `Option`-shaped loop polling instead of `Result`-shaped early return.
    ///
    /// On exhaustion (the poll conceptually returns `None`, or `Ok(None)` under [`IterProtocol::UserDefined`]'s
    /// `fallible` flag), breaks out of the innermost enclosing [`Self::Loop`] -- mirroring how a range-based `for`
    /// already injects a leading conditional `Break` for its own exhaustion check. On a produced value (`Some(v)`,
    /// or `Ok(Some(v))` when fallible), stores `v` into `destination` and falls through to the next statement. When
    /// `protocol` is [`IterProtocol::UserDefined`] with `fallible: true`, a failure result (`Err(e)`) additionally
    /// short-circuits with an early return from the enclosing function, converting `e` via `From`/`Into` exactly
    /// like [`Self::TryPropagate`] -- so this single statement can carry up to three implicit outcomes when
    /// fallible, matching what `for item in iterable?:` (RFC 115) means as one syntactic form at the source level
    /// rather than decomposing it into a raw match a downstream consumer would have to re-derive.
    IterNext {
        /// Where the produced item is written when the iterator was not exhausted.
        destination: Place,
        /// The iterator being polled (already materialized by an earlier `Assign`/`Call` -- see
        /// `lower_general_iteration` in `loaves/compiler/incan_frontend/src/body_ir.rs`).
        iterator: Operand,
        /// Which iteration protocol drives this poll.
        protocol: IterProtocol,
    },
    /// A source construct v0 lowering does not yet model. Keeps the model total over real programs instead of
    /// panicking or silently dropping the construct.
    Unsupported { description: String },
}

/// The form-specific payload of one [`StatementKind::Assert`], covering all three RFC 018 assertion spellings.
///
/// See [`StatementKind::Assert`] for why the three forms are one variant with this payload rather than three
/// sibling statement kinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssertionKind {
    /// `assert cond`: panics when `cond` is false.
    Condition { cond: Operand },
    /// `assert value is P`: panics when `value` does not match `P`, and on the matching path binds `P`'s names for
    /// the remainder of the enclosing block.
    ///
    /// The bindings are ordinary declared locals carried inside `pattern`'s [`PatternBinding`]s, exactly as a
    /// `match` arm's are, so a consumer reading one of those names later finds a local this body declared rather
    /// than an unresolved name. `scrutinee` is read as [`OwnershipFact::Borrow`] for the same reason
    /// [`Rvalue::Match::scrutinee`] is: the overall read must not risk an unconditional move while individual
    /// bindings compute their own, more precise facts against places projected out of it.
    Pattern { scrutinee: Operand, pattern: Box<Pattern> },
    /// `assert call() raises E`: evaluates `call`, expecting it to raise a runtime error of type `E`, and panics
    /// when it does not.
    ///
    /// `expected_error` is the resolved builtin-exception identity, not the source spelling after `raises`, so a
    /// consumer never has to re-resolve a name against the exception registry to know which error is expected.
    Raises { call: Operand, expected_error: ErrorKind },
}

impl AssertionKind {
    /// Render the form-specific part of an assertion's snapshot line, without the shared message/panic suffix.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Condition { cond } => cond.render_snapshot(),
            Self::Pattern { scrutinee, pattern } => {
                format!("{} is {}", scrutinee.render_snapshot(), pattern.render_snapshot())
            }
            Self::Raises { call, expected_error } => {
                format!("{} raises {}", call.render_snapshot(), errors::as_str(*expected_error))
            }
        }
    }
}

/// Typechecker-proven error routing selected for one `?` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TryErrorRouting {
    /// The operand and enclosing Result have the same exact error type, so direct propagation needs no conversion.
    SameType { error_type: IncanType },
    /// Source semantics selected a conversion between distinct error types. The direct runtime intentionally does
    /// not reconstruct that conversion until Body IR retains the conversion authority itself.
    ConversionRequired {
        source_error_type: IncanType,
        destination_error_type: IncanType,
    },
    /// Lowering did not retain enough checked Result type information to describe error routing honestly.
    Unresolved,
}

impl TryErrorRouting {
    /// Stable maintainer-facing rendering paired with the `try?` snapshot line.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::SameType { error_type } => format!("same_error_type={error_type}"),
            Self::ConversionRequired {
                source_error_type,
                destination_error_type,
            } => format!("error_conversion={source_error_type}->{destination_error_type}"),
            Self::Unresolved => "error_routing=unresolved".to_string(),
        }
    }
}

// ============================================================================
// Panic facts
// ============================================================================

/// One panic-interaction fact recorded for a body, without committing to a stable public panic strategy. This only
/// exposes *that* a statement may panic and *why* — strategy decisions (unwind vs. abort, drop-on-unwind ordering)
/// are left to later, target-specific work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanicFact {
    pub span: HirSourceSpan,
    pub reason: PanicReason,
}

impl PanicFact {
    /// Render a deterministic maintainer-facing snapshot line for this panic fact.
    pub(super) fn render_snapshot(&self) -> String {
        format!("{} span={}..{}", self.reason.as_str(), self.span.start, self.span.end)
    }
}

/// Why a statement may panic at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PanicReason {
    AssertFailure,
    DivisionOrModulo,
    HelperMayPanic(HelperOp),
}

impl PanicReason {
    /// Compact snapshot spelling for this panic reason.
    pub(super) fn as_str(&self) -> String {
        match self {
            Self::AssertFailure => "assert_failure".to_string(),
            Self::DivisionOrModulo => "division_or_modulo".to_string(),
            Self::HelperMayPanic(op) => format!("helper_may_panic({})", op.as_str()),
        }
    }
}
