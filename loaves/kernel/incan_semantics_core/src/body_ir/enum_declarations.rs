//! Class inheritance, newtype conversions, method partial defaults, and enum and value-enum declarations.

use super::*;

/// One inherited source method slot and its nearest checked implementation for the concrete subclass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassMethodImplementation {
    /// Canonical ancestor method identity carried by checked calls of that slot.
    pub method: CanonicalSymbolId,
    /// Canonical source body to instantiate for this subclass; original bodies keep their own identities.
    pub implementation: CanonicalSymbolId,
}

/// Exact reconstruction policy for a source newtype selecting serde Deserialize.
/// A missing older record is unselected, so it cannot silently authorize raw reconstruction of a checked owner.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum NewtypeDeserialize {
    /// No admitted reconstruction selection; Deserialize consumers must refuse source newtypes with this state.
    #[default]
    Unselected,
    /// The source declaration is unchecked, so serde's ordinary tuple reconstruction is the complete contract.
    Ordinary,
    /// Validation requires the exact selected source hook; its Body owns parameter, result and binder facts.
    Checked(CanonicalSymbolId),
}

/// Whether source checking authorized this raw owner as a possible automatic string conversion step.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum NewtypeStringConversion {
    /// Missing support, binders, hooks, constraints, source adoptions or unproved storage grant no automatic step.
    #[default]
    Unselected,
    /// The producer checked support and excluded every source override in the admitted raw nongeneric profile.
    /// Consumers must still validate the exact underlying chain and the consumer invocation's selected contract.
    Automatic,
}

/// The deferred defaults of one same-owner method partial (`short = partial label(prefix="x")`), without a second
/// semantic method identity.
///
/// Checking gives a call of the partial the target method's identity, with each preset the call omits as a defaulted
/// slot. The frame is the target method's parameter surface, receiver first, whose defaults are the partial's presets
/// and the target's own remaining defaults. A consumer evaluates only the selected default computations, never the
/// frame's empty block, and must match both [`Self::name`] and [`Self::target`] before using it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodPartialDefaults {
    /// Source binding name of the partial within its nominal owner.
    pub name: String,
    /// Source span of the partial binding, inside its owner's declaration.
    pub span: HirSourceSpan,
    /// Exact canonical method the checker selected as the partial's target, through any method alias.
    pub target: CanonicalSymbolId,
    /// The target-shaped default frame: the target's identity, name, span and parameters, with the presets as
    /// defaults.
    pub frame: Box<Body>,
}

/// One source-local fieldless normal enum retained for direct identity comparison.
///
/// The record exists only for undecorated, non-generic declarations with no traits, methods, aliases, value backing,
/// or payload variants. A runtime validates these records before materializing a carrier and never treats the enum
/// spelling itself as an execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldlessEnumDeclaration {
    /// Exact source-local enum declaration identity, derived from the declaration source span.
    pub direct_declaration_id: CompilerNodeId,
    /// Checker-minted canonical identity of this enum declaration.
    pub canonical: CanonicalSymbolId,
    /// Canonical source declaration name.
    pub name: String,
    /// Canonical zero-payload variants in source declaration order.
    pub variants: Vec<FieldlessEnumVariantDeclaration>,
}

/// Canonical payload layout of a checked source, SDK or native enum; generic templates retain their ordered binders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumDeclaration {
    /// The compiled item a linked Loaf declares this enum as (DD-0007); empty for an enum this compilation declares.
    #[serde(default)]
    pub native_item: String,
    /// Exact source-local declaration identity derived from the enum's span.
    pub direct_declaration_id: CompilerNodeId,
    /// Exact checker identity of the enum owner.
    pub canonical: CanonicalSymbolId,
    /// Source name, cross-checked against constructor diagnostics.
    pub name: String,
    /// Checked source visibility.
    pub public: bool,
    /// Ordered declaration binders selected by checking; payload placeholders close only at checked concrete uses.
    /// Older publications retain an empty list and cannot authorize a generic payload layout.
    #[serde(default)]
    pub type_parameters: Vec<String>,
    /// Concrete arguments of a specialized enum layout, in declaration order. Published generic source enums and SDK
    /// enums alias their provider-owned templates; native metadata verification checks the instantiated variants and
    /// payloads.
    #[serde(default)]
    pub native_type_arguments: Vec<IncanType>,
    /// Declaration-ordered variants; each index selects rustc's actual discriminant value for native matching.
    pub variants: Vec<EnumVariantDeclaration>,
    /// Explicit and automatic derives selected by the checker.
    pub derives: Vec<String>,
    /// Checker-retained origins for named payload types; publication keeps only bindings the payloads require.
    #[serde(default)]
    pub named_type_identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
}

/// One canonical variant and its checked positional payload layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumVariantDeclaration {
    /// Exact source-local declaration identity derived from the variant's span.
    pub direct_declaration_id: CompilerNodeId,
    /// Checker-selected variant identity; construction and patterns dispatch by this fact.
    pub canonical: CanonicalSymbolId,
    /// Source variant name, used only after identity resolution.
    pub name: String,
    /// Checked payload types in declaration order.
    pub fields: Vec<IncanType>,
}

/// Canonical member identity and physical declaration coordinate for a selected native enum variant. Native items
/// have no source span, so the owner path and variant name supply the unique declaration scope instead.
pub fn native_enum_variant(path: &str, name: &str) -> Option<(CanonicalSymbolId, CompilerNodeId)> {
    let owner = rust_item_identity(path)?;
    if !is_rust_identifier(name) {
        return None;
    }
    let module = native_nominal_module(&owner)?;
    let mut identity = owner;
    identity.namespace = crate::SymbolNamespace::Member;
    identity.kind = crate::SemanticSourceTargetKind::Variant;
    identity.declaration_name = name.to_owned();
    Some((
        identity,
        CompilerNodeId::declaration_span(&format!("{module}::variant::{name}"), 0, 0),
    ))
}

/// Retain a complete metadata-selected Rust enum through the ordinary enum vocabulary. The native item and exact
/// variant identities authorize an alias only after the adapter verifies the instantiated foreign payload layout.
pub fn native_enum_declaration(
    path: &str,
    type_parameters: Vec<String>,
    variants: Vec<(String, Vec<IncanType>)>,
) -> Option<EnumDeclaration> {
    let canonical = rust_item_identity(path)?;
    let module = native_nominal_module(&canonical)?;
    let variants = variants
        .into_iter()
        .map(|(name, fields)| {
            let (canonical, direct_declaration_id) = native_enum_variant(path, &name)?;
            Some(EnumVariantDeclaration {
                direct_declaration_id,
                canonical,
                name,
                fields,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(EnumDeclaration {
        native_item: path.to_owned(),
        direct_declaration_id: CompilerNodeId::declaration_span(&module, 0, 0),
        canonical,
        name: path.to_owned(),
        public: true,
        type_parameters,
        native_type_arguments: Vec::new(),
        variants,
        derives: Vec::new(),
        named_type_identities: Default::default(),
    })
}

/// One canonical zero-payload member of a retained fieldless normal enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldlessEnumVariantDeclaration {
    /// Exact source-local variant declaration identity, derived from the declaration source span.
    pub direct_declaration_id: CompilerNodeId,
    /// Checker-minted canonical identity of this member declaration.
    pub canonical: CanonicalSymbolId,
    /// Canonical member name; aliases are intentionally not retained.
    pub name: String,
}

impl FieldlessEnumVariantDeclaration {
    /// Render the member identity for a deterministic module snapshot.
    pub(super) fn render_snapshot(&self) -> String {
        format!(
            "variant {} id={} canonical={}",
            self.name,
            self.direct_declaration_id,
            self.canonical.render_compact()
        )
    }
}

/// The scalar backing category a retained RFC 032 value enum exposes through `.value()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueEnumBacking {
    /// An integer-backed value enum.
    Int,
    /// A string-backed value enum.
    Str,
}

impl ValueEnumBacking {
    /// Render the canonical source-level scalar type name for snapshots and diagnostics.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Str => "str",
        }
    }
}

/// One source-local RFC 032 value enum retained for direct scalar extraction.
///
/// The record exists only for undecorated, non-generic declarations with no traits, methods, aliases, or payload
/// variants. Each member has its own source-span identity, so a direct executor verifies the actual selected member
/// without treating a qualified `Enum.Member` spelling as an identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueEnumDeclaration {
    /// Exact source-local enum declaration identity, derived from its declaration span.
    pub direct_declaration_id: CompilerNodeId,
    /// Checker-minted canonical identity of this enum declaration.
    pub canonical: CanonicalSymbolId,
    /// Canonical source declaration name.
    pub name: String,
    /// The only scalar carrier exposed by the admitted generated `.value()` operation.
    pub backing: ValueEnumBacking,
    /// Canonical value-enum members in source declaration order.
    pub variants: Vec<ValueEnumVariantDeclaration>,
}

/// One canonical scalar member of a retained source-local value enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueEnumVariantDeclaration {
    /// Exact source-local variant declaration identity, derived from its declaration span.
    pub direct_declaration_id: CompilerNodeId,
    /// Checker-minted canonical identity of this member declaration.
    pub canonical: CanonicalSymbolId,
    /// Canonical member name; aliases are intentionally not retained.
    pub name: String,
    /// The checked source literal exposed only through identity-validated `.value()` extraction.
    pub raw_value: Constant,
}

impl ValueEnumVariantDeclaration {
    /// Render the member's identity and raw scalar fact for a deterministic module snapshot.
    pub(super) fn render_snapshot(&self) -> String {
        format!(
            "variant {} id={} canonical={} raw={}",
            self.name,
            self.direct_declaration_id,
            self.canonical.render_compact(),
            self.raw_value.render_snapshot()
        )
    }
}

/// Checked callable representation choices for a declaration's signature.
///
/// The function type describes the call surface, but does not distinguish a function pointer from a parameter or
/// return that must hold a capturing closure. These checker facts preserve that distinction without asking a
/// consumer to rediscover it from the declaration's syntax or its body's uses.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallableRepresentation {
    /// Explicitly annotated local bindings that require function-pointer coercion rather than inferred item storage.
    #[serde(default)]
    pub function_pointer_locals: Vec<LocalId>,
    /// Local bindings whose checked callable value holds an environment, including bindings annotated with a
    /// function type. They infer the closure's own type rather than require function-pointer coercion.
    #[serde(default)]
    pub closure_holding_locals: Vec<LocalId>,
    /// Parameter locals whose checked function type accepts closures through an `Fn` bound.
    pub closure_holding_parameters: Vec<LocalId>,
    /// The checked return type holds a capturing closure through an opaque `Fn` implementation.
    pub closure_holding_return: bool,
}
