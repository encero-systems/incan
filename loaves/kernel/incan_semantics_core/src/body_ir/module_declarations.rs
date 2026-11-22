//! Module-level declarations: runtime callables, entrypoint effects, constants, statics, decorated bindings and Loaf
//! callables.

use super::*;

/// One authenticated public runtime facade and the declaration selected by its Loaf identity graph. A Loaf's installed
/// compiler records publish its bindings as a record of their own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeCallableBinding {
    /// Public runtime ABI path introduced by lowering.
    pub path: String,
    /// Exact declaring callable, including its provider and module identity.
    pub canonical: CanonicalSymbolId,
}

/// Observable setup retained by the checked frontend for the selected entry module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EntrypointEffect {
    /// Print the retained text with a trailing newline before evaluating the entrypoint body.
    PrintText {
        /// Authoritative text retained at compilation, independent of the consumer's working directory.
        text: String,
        /// Import declaration that requires this setup output.
        span: HirSourceSpan,
    },
}

/// Compilation-scoped authority for the frozen source-owned `TryFrom[str]` bridge.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum StringConversionContract {
    /// Older metadata or a nonentry fragment supplies no consumer invocation authority.
    #[default]
    Unselected,
    /// The consumer checked its complete source import union and found no activation.
    Inactive,
    /// The consumer activated the contract and authenticated the exact linked trait and callable slot.
    Selected(Box<StringConversionSelection>),
}

/// Exact published identities authenticated against the selected source API and analysis cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StringConversionSelection {
    /// SDK-owned trait used by compiled consumers, independent of source import aliases.
    pub trait_identity: CanonicalSymbolId,
    /// Published method identity selected by checked manifest-backed generic associated calls.
    pub method_identity: CanonicalSymbolId,
}

/// Canonical immutable value evaluated by the declaring compilation, with no runtime storage or initialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstantDeclaration {
    /// Exact checker-selected declaration; aliases never mint another value identity.
    pub canonical: CanonicalSymbolId,
    /// The checked scalar, text, or effect-free frozen aggregate retained by the declaring frontend.
    pub value: Constant,
}

/// Declaring compilation's checked meaning of an erased type alias, including canonical named member types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeAliasDeclaration {
    /// Canonical declaration selected through an alias or facade binding.
    pub canonical: CanonicalSymbolId,
    /// Alias-expanded semantic type, without a consumer-side name lookup.
    pub ty: IncanType,
    /// Checker-retained named type identities; publication removes unused entries and audits the required closure.
    #[serde(with = "shared_identities")]
    pub named_type_identities: SharedIdentities,
}

/// Canonical persistent storage with a checker-typed initializer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticDeclaration {
    /// Exact checker-selected storage identity, independent of aliases at its uses.
    pub canonical: CanonicalSymbolId,
    /// Checked carrier type; initializer validation must preserve its exact element types.
    pub ty: IncanType,
    /// Initialization value; repeated reads must not reinitialize an assigned cell.
    pub initial: StaticInitializer,
}

/// One checked decorator replacement and the source callable it receives during initialization.
///
/// The original and the visible wrapper share a source declaration, but are different runtime items. Only the
/// replacement evaluation's explicit original-callable read may select this body; ordinary calls select the visible
/// wrapper. Nongeneric replacements initialize in module declaration order. Every generic replacement retains its
/// checked initializer template for one initialization per closed instantiation, including signatures using binders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecoratedBinding {
    /// Executable source body and original signature, before applying user-defined decorators.
    pub original_body: Body,
    /// Source-owned template required by PerInstantiation; module storage retains None. The template has the
    /// original's exact binders and declaration identity; consumers must close it before use.
    #[serde(default)]
    pub initializer: Option<Box<Body>>,
    /// Checked evaluation policy; consumers must not infer generic cache behavior from a storage lookup.
    pub initialization: DecoratedInitialization,
}

/// Checked lifetime of a decorator replacement, with once-only initialization for each declaration instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecoratedInitialization {
    /// A callable without type parameters initializes its replacement once in module declaration order.
    ModuleStorage,
    /// A closed generic replacement initializes once for each checked instantiation, using its retained template.
    PerInstantiation,
    /// Retired serialized discriminant. Validation refuses the former per-call initialization policy.
    PerInvocation,
}

/// Checked storage initialization, independent of its eventual native representation.
///
/// Literal initializers are effect-free, so when they are evaluated is not observable. A computed initializer is
/// evaluated as legacy evaluates its static (RFC 052): once, when its module's statics are initialized in declaration
/// order on entry to the first function of that module, or earlier when a read reaches it first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StaticInitializer {
    /// One primitive literal, interpreted in the storage declaration's checked carrier.
    Literal(Constant),
    /// An ordered list of primitive literals, each checked against the declaration's element type.
    List(Vec<Constant>),
    /// Ordered primitive key and value literals; a later duplicate key replaces an earlier value, as in source.
    Dict(Vec<(Constant, Constant)>),
    /// Primitive member literals of a set, including the empty `Set()` constructor; duplicates collapse on insert.
    Set(Vec<Constant>),
    /// Any other checked initializer, as a body with no parameters that returns the storage carrier.
    Computed(Box<Body>),
    /// A published static that must use the declaring Loaf's native storage.
    Linked(Box<LinkedStaticAccessors>),
}

/// Producer-selected crate-relative native names for one Loaf static.
///
/// These bindings do not grant Incan visibility. The consumer resolves them in the crate that the declaration's
/// canonical identity names and cannot initialize its own storage from this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedStaticAccessors {
    /// Read a value from the declaring Loaf's storage.
    pub load: String,
    /// Replace a value in the declaring Loaf's storage. Constants publish no store path.
    pub store: Option<String>,
    /// Run the producer's module initializer in declaration order when this static has computed initialization.
    pub initialize_module: Option<String>,
    /// Operate under the declaring Loaf's guard for native cell carriers.
    pub guarded: Option<LinkedStaticGuardAccessors>,
}

/// Native operations that keep a storage guard in the declaring Loaf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedStaticGuardAccessors {
    /// Force lazy initialization without copying the value.
    pub force: String,
    /// Run a callback while the producer holds a shared guard.
    pub with_ref: String,
    /// Run a callback while the producer holds an exclusive guard. Constants publish no mutable path.
    pub with_mut: Option<String>,
}

/// A namespace reserved for the hidden native exports of Loaf static bindings.
pub const LINKED_STATIC_ACCESSOR_PREFIX: &str = "__incan_linked_static_";

impl LinkedStaticAccessors {
    /// Choose native names in the declaring Loaf. Ordinary statics use their module and name. Decorated replacements
    /// retain the full callable identity to distinguish overloads. Neither depends on the native slot.
    pub fn for_declaration(declaration: &StaticDeclaration) -> Option<Self> {
        let identity = &declaration.canonical;
        if !linked_storage_owner(identity) || matches!(declaration.initial, StaticInitializer::Linked(_)) {
            return None;
        }
        let writable = identity.kind == crate::SemanticSourceTargetKind::Static;
        let replacement = matches!(
            identity.kind,
            crate::SemanticSourceTargetKind::Function | crate::SemanticSourceTargetKind::Method
        );
        let computed = matches!(declaration.initial, StaticInitializer::Computed(_));
        if replacement && !computed {
            return None;
        }
        let mut stable = identity.clone();
        if !replacement {
            stable.declaration_span = crate::HirSourceSpan::new(0, 0);
        }
        let name = crate::encode_incan_symbol_identity(&stable);
        let path = |operation: &str| format!("{LINKED_STATIC_ACCESSOR_PREFIX}{operation}_{name}");

        let initialize_module = computed.then(|| {
            stable.declaration_name.clear();
            stable.kind = crate::SemanticSourceTargetKind::Static;
            stable.namespace = crate::SymbolNamespace::OrdinaryLexical;
            stable.declaration_span = crate::HirSourceSpan::new(0, 0);
            let module = crate::encode_incan_symbol_identity(&stable);
            format!("{LINKED_STATIC_ACCESSOR_PREFIX}initialize_{module}")
        });
        let guarded = (computed
            || matches!(
                declaration.initial,
                StaticInitializer::List(_) | StaticInitializer::Dict(_)
            ))
        .then(|| LinkedStaticGuardAccessors {
            force: path("force"),
            with_ref: path("with_ref"),
            with_mut: writable.then(|| path("with_mut")),
        });
        Some(Self {
            load: path("load"),
            store: writable.then(|| path("store")),
            initialize_module,
            guarded,
        })
    }

    /// Check that every claimed operation is a hidden native identifier in the declaring crate. A read-only static
    /// cannot carry a write operation, and module initialization must have a native guard to force.
    pub(crate) fn valid_for(&self, identity: &CanonicalSymbolId) -> bool {
        let writable = identity.kind == crate::SemanticSourceTargetKind::Static;
        let replacement = matches!(
            identity.kind,
            crate::SemanticSourceTargetKind::Function | crate::SemanticSourceTargetKind::Method
        );
        linked_storage_owner(identity)
            && (!replacement || self.initialize_module.is_some())
            && self.store.is_some() == writable
            && self
                .initialize_module
                .as_deref()
                .is_none_or(|path| self.guarded.is_some() && linked_static_identifier(path, "initialize_"))
            && linked_static_identifier(&self.load, "load_")
            && self
                .store
                .as_deref()
                .is_none_or(|path| linked_static_identifier(path, "store_"))
            && self.guarded.as_ref().is_none_or(|guarded| {
                linked_static_identifier(&guarded.force, "force_")
                    && linked_static_identifier(&guarded.with_ref, "with_ref_")
                    && guarded.with_mut.is_some() == writable
                    && guarded
                        .with_mut
                        .as_deref()
                        .is_none_or(|path| linked_static_identifier(path, "with_mut_"))
            })
    }
}

/// Ordinary package storage and a decorated callable's replacement can have producer-owned native cells.
fn linked_storage_owner(identity: &CanonicalSymbolId) -> bool {
    use crate::{SemanticSourceTargetKind as Kind, SymbolNamespace as Namespace};
    matches!(identity.origin, crate::SymbolOrigin::Package { .. })
        && identity.scope_discriminant.is_none()
        && matches!(
            (&identity.namespace, &identity.kind),
            (Namespace::OrdinaryLexical, Kind::Static | Kind::Const | Kind::Function)
                | (Namespace::Member, Kind::Method)
        )
}

/// Validate an operation path as one crate-relative identifier in the compiler-owned namespace.
fn linked_static_identifier(path: &str, operation: &str) -> bool {
    path.strip_prefix(LINKED_STATIC_ACCESSOR_PREFIX)
        .and_then(|identifier| identifier.strip_prefix(operation))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

/// One anonymous union whose payload order and callable ownership were retained before native lowering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeUnionDeclaration {
    /// Checked semantic union whose nominal leaves retain canonical declaration identities.
    pub ty: IncanType,
    /// A source-local native symbol, or the exact qualified item admitted from a selected producer.
    pub rust_item: String,
    /// Payload types in retained variant order; lowering preserves this order even if local names differ.
    pub members: Vec<IncanType>,
    /// Callable members own reusable captured environments instead of function-pointer storage.
    #[serde(default)]
    pub closure_members: Vec<IncanType>,
    /// Published callable object aliases, in the same order as `closure_members`.
    #[serde(default)]
    pub closure_items: Vec<String>,
}

/// One checked callback parameter and the native type argument that retains its environment representation.
/// The named fields preserve the checker's slot mapping across the Body IR boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeCallableParameter {
    /// Declaration-order parameter slot whose checked value is a callback.
    pub parameter_slot: usize,
    /// Position in the native instantiation's type arguments that carries this callback's environment.
    pub type_argument_slot: usize,
}

/// An external callable: a linked Loaf declaration (DD-0007) or a checker-selected Rust instantiation (DD-0006).
/// The driver verifies its recorded signature against the exact compiled item's metadata.
///
/// Published fields use positional records. The declaration frame is a transient view projected from a loaded body,
/// and is not an additional publication or a second copy of the executable representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoafCallable {
    /// Exact declaration identity selected by checking, independent of the caller's spelling.
    pub canonical: CanonicalSymbolId,
    /// The compiled item: a Loaf's encoded identity ([`loaf_item_path`]) or the checked public Rust item path.
    pub item: String,
    /// Declaration-order parameter types; argument binding still belongs to the call site.
    pub parameters: Vec<IncanType>,
    /// The declared result type, verified against native metadata before execution.
    pub return_type: IncanType,
    /// The callee's own type parameters in declaration order, which `parameters` and `return_type` name. A call
    /// instantiates them with its checked type arguments, in the same order; a nongeneric callee has none.
    #[serde(default)]
    pub type_parameters: Vec<String>,
    /// Native instantiation for an explicit trait implementation, or a checker-selected Rust call. Trait arguments
    /// start with Self and close owner placeholders through `type_parameters`. A Rust call has an already-instantiated
    /// signature and retains the impl's native arguments before the method's arguments. Consumers must not reconstruct
    /// these arguments from names.
    #[serde(default)]
    pub native_type_arguments: Vec<IncanType>,
    /// Checked callback parameter slots and their exact native generic argument positions, before signature erasure.
    #[serde(default)]
    pub native_callable_parameters: Vec<NativeCallableParameter>,
    /// What each parameter takes when a call omits it, in `parameters` order, a method's receiver first.
    ///
    /// A parameter without a default is [`CallableParamDefault::Required`]. A default is a
    /// [`CallableParamDefault::Source`] computation, evaluated in [`Self::declaration_frame`] when it has
    /// statements, or a directly usable constant otherwise. An unavailable computation is explicitly Unsupported.
    #[serde(default)]
    pub parameter_defaults: Vec<CallableParamDefault>,
    /// Checked parameter passing, callable representation and default storage from the authenticated loaded body.
    /// Its ordinary statements are cleared; consumers compile no copy of the function's implementation.
    #[serde(skip)]
    pub declaration_frame: Option<Box<Body>>,
}

impl LoafCallable {
    /// Borrow the authenticated declaration frame without exposing its optional Box storage to consumers. The
    /// frame remains declaration-only and grants no implementation body or additional execution authority.
    pub fn declaration_frame(&self) -> Option<&Body> {
        self.declaration_frame.as_deref()
    }
}

/// The single native-name projection shared by publication, retained bodies and Oven.
pub use incan_loaf_names::native_crate_name as loaf_crate_name;

/// The compiled item a linked Loaf declares `canonical` as (DD-0007): `<crate>::<encoded identity>`, the item the Loaf
/// exports at its crate root under the encoding of the declaration's canonical identity. `None` for an identity no
/// Loaf declares.
pub fn loaf_item_path(canonical: &CanonicalSymbolId) -> Option<String> {
    let crate::SymbolOrigin::Package { library, .. } = &canonical.origin else {
        return None;
    };
    Some(format!(
        "{}::{}",
        loaf_crate_name(library),
        crate::encode_incan_symbol_identity(canonical)
    ))
}

impl LoafCallable {
    /// The declaration record of a Loaf's callable body: its parameters, result, type parameters and defaults, and the
    /// compiled item its package identity names. Async items expose a future constructor rather than the resumed
    /// body output, so consumers use the same retained boxed-future ABI as calls within the declaring package.
    /// `None` for a body no Loaf declares or a governed template that must first close its named type parameters or
    /// open `Self` receiver at the checked adopter.
    pub fn from_body(body: &Body) -> Option<Self> {
        if !body.type_parameters.is_empty() || body.locals.first().is_some_and(|local| local.ty == IncanType::SelfType)
        {
            return None;
        }
        let canonical = body.canonical.clone()?;
        let item = loaf_item_path(&canonical)?;
        let mut frame = body.clone();
        frame.block.stmts.clear();
        Some(Self {
            item,
            parameters: body
                .params
                .iter()
                .map(
                    |param| match body.locals.get(param.local.index()).map(|local| &local.origin) {
                        Some(LocalOrigin::Receiver { mutable: false }) => IncanType::Ref(Box::new(param.ty.clone())),
                        Some(LocalOrigin::Receiver { mutable: true }) => IncanType::RefMut(Box::new(param.ty.clone())),
                        _ => param.ty.clone(),
                    },
                )
                .collect(),
            return_type: if body.is_async {
                IncanType::Generic {
                    base: "Awaitable".into(),
                    args: vec![body.return_type.clone()],
                }
            } else {
                body.return_type.clone()
            },
            type_parameters: body.type_parameters.clone(),
            native_type_arguments: Vec::new(),
            native_callable_parameters: Vec::new(),
            parameter_defaults: body.params.iter().map(|param| param.default.clone()).collect(),
            declaration_frame: Some(Box::new(frame)),
            canonical,
        })
    }

    /// The compiled item a call of this callable names.
    pub fn rust_path(&self) -> Option<String> {
        Some(self.item.clone())
    }
}
