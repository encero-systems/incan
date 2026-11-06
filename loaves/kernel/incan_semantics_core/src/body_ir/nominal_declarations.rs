//! Trait implementations and adoptions, field metadata, nominal declarations, native carriers and held Rust items.

use super::*;

/// One non-generic concrete adopter's implementation of a source-local trait method.
///
/// The adopter's traits include every source-local supertrait its adoptions reach, so a supertrait slot is retained
/// with the body that fills it for this adopter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitImplementation {
    /// Concrete nominal owner, matched against the retained layout registry.
    pub owner: CanonicalSymbolId,
    /// Trait slot identity minted by the checker.
    pub method: CanonicalSymbolId,
    /// Implementing method identity: the adopter's own method, the slot's default, or the default a directly adopted
    /// subtrait declares for the slot.
    pub implementation: CanonicalSymbolId,
    /// Checked arguments of the selected slot's declaring trait, independent of a default body's parameter scope.
    /// Distinct adoptions of one trait retain distinct dispatch records even when their slot identities agree.
    #[serde(default)]
    pub slot_arguments: Vec<IncanType>,
    /// Checked type arguments of the trait whose default `implementation` is, one per type parameter of that trait.
    ///
    /// Empty for the adopter's own method and for a default of a trait without type parameters.
    #[serde(default)]
    pub type_arguments: Vec<TraitTypeArgument>,
}

/// One trait type parameter bound to the type argument the checker recorded for an adoption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitTypeArgument {
    /// The type parameter as the trait declares it.
    pub parameter: String,
    /// The checked type the adoption binds it to.
    pub argument: IncanType,
}

/// One checked trait adoption, including marker protocols that have no executable method slots.
///
/// The declaring trait identity and ordered semantic arguments survive aliases and derive-bundle expansion.
/// This fact records source conformance only; a native consumer must independently select an admitted trait ABI
/// and validate every implementing body before emitting a native implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraitAdoption {
    /// Canonical checked trait declaration, never the spelling of the adoption or import alias.
    pub trait_identity: CanonicalSymbolId,
    /// Checked arguments in the declaring trait's parameter order; marker protocols retain an empty vector.
    pub arguments: Vec<IncanType>,
    /// Complete checked method-slot identities, or `None` when metadata cannot prove every slot.
    /// An authenticated marker protocol has `Some([])`; missing metadata cannot masquerade as an empty protocol.
    #[serde(default)]
    pub methods: Option<Vec<CanonicalSymbolId>>,
    /// Checked required fields bound to this adopter's exact stored fields, transported in the nominal trailer.
    #[serde(skip)]
    pub fields: Vec<TraitFieldImplementation>,
    /// Checked source-model adapter for a compiler-provided primitive family. Ordinary source adoptions carry None;
    /// this record changes only capability dispatch, never the primitive's storage or general type compatibility.
    #[serde(default)]
    pub primitive_adapter: Option<PrimitiveTraitAdapter>,
}

/// A source-owned implementation of a temporary primitive capability, authenticated by its declaring compilation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrimitiveTraitAdapter {
    /// Primitive type whose checked capability calls may use this adoption.
    pub receiver_type: IncanType,
    /// Exact sole payload field through which a receiver is wrapped without changing its meaning.
    pub payload_field: CanonicalSymbolId,
    /// Exact selected trait slot whose result the associated primitive decoder implements.
    pub decode_slot: CanonicalSymbolId,
    /// Checked associated decoder returning the primitive value rather than the adapter model.
    pub decoded_value: CanonicalSymbolId,
}

/// One required field of a trait bound to the exact stored field that satisfied conformance checking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitFieldImplementation {
    /// Checker-minted required field identity under the declaring trait.
    pub requirement: CanonicalSymbolId,
    /// Canonical stored field identity in the adopting model or class, including an inherited field.
    pub implementation: CanonicalSymbolId,
}

/// Immutable presentation values of one checked reflection-visible field, in declaration order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldMetadata {
    /// Canonical written field name.
    pub name: String,
    /// Explicit field alias, including an explicitly empty string.
    pub alias: Option<String>,
    /// Explicit source description, absent when none is declared.
    pub description: Option<String>,
    /// Canonical Incan surface type name, independent of the native storage spelling.
    pub type_name: String,
    /// Whether the source declaration supplies a default for this field.
    pub has_default: bool,
}

/// The exact local declaration and canonical field layout for one direct-executable model, class, or raw newtype.
///
/// The record belongs to its declaring module and excludes enums. Nominal owners retain ordered type parameters for
/// specialization from checked uses, including a newtype's underlying tuple slot. Inherited classes retain ancestor
/// field ownership and nearest method dispatch separately from their physical layout. Nominal
/// methods and adopted trait defaults remain separate canonical bodies with receiver origins. A consumer may load
/// this canonical context from a package artifact. Its field order is the checked constructor-slot order; a direct
/// runtime must compare it with [`ConstructorTarget::canonical_field_layout`] before applying
/// [`ConstructorTarget::binding`], rather than treating constructor argument spelling as layout evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NominalDeclaration {
    /// Checked explicit and derive-expanded adoptions; an absent older fact grants no native trait authority.
    #[serde(default)]
    pub trait_adoptions: Vec<TraitAdoption>,
    /// The compiled item that owns this layout: a linked Loaf's item (DD-0007) or a foreign runtime item the
    /// direct-route bridge selects. Empty for a layout this compilation declares; consumers validate a nonempty
    /// item against its retained canonical owner before linking it.
    #[serde(default)]
    pub native_item: String,
    /// Checked local class parent and nearest source method implementations; absent for noninherited carriers.
    #[serde(default)]
    pub class_inheritance: Option<ClassInheritance>,
    /// Fixed runtime arguments for a nongeneric source surface, such as Html's String specialization. Generic
    /// surfaces retain their checked instance arguments instead; ordinary source/SDK layouts carry none.
    #[serde(default)]
    pub native_type_arguments: Vec<IncanType>,
    /// Exact source-local declaration identity, derived from the declaration source span.
    pub direct_declaration_id: CompilerNodeId,
    /// RFC 120 identity minted by the checker for this declaration.
    pub canonical: CanonicalSymbolId,
    /// Canonical source declaration name, checked again by consumers as a defense against malformed Body IR.
    pub name: String,
    /// Canonical field names in checked constructor order, with inherited fields before local fields. A plain newtype
    /// retains its sole tuple slot as `0`.
    pub fields: Vec<String>,
    /// RFC 120 identities of [`Self::fields`] in the same constructor order. A newtype's sole generated tuple slot
    /// retains its owner's identity, because that slot has no independent source declaration.
    ///
    /// The parallel layout is intentional: constructor binding and stored runtime values retain the compact field
    /// names, while a source projection must match its checked identity at the same slot before the name is used to
    /// select storage.
    pub field_identities: Vec<CanonicalSymbolId>,
    /// Checked field types in the same canonical order, including private layout dependencies.
    pub field_types: Vec<IncanType>,
    /// Derived closed field slots that substituted a declared generic value with an environment-capable callable.
    /// Publications retain the original templates instead; specializing a loaded template recreates these facts.
    /// Explicit function fields remain function pointers; construction and projection use this same owning ABI.
    #[serde(skip)]
    pub owned_callable_fields: Vec<usize>,
    /// Checker-resolved public visibility, in the same order as the fields.
    pub field_public: Vec<bool>,
    /// Checked RFC 021 field aliases in the same order, `None` where a field declares none. A serde form names an
    /// aliased field by its alias, as legacy's `#[serde(rename)]` does. Older artifacts carry no aliases.
    #[serde(default)]
    pub field_aliases: Vec<Option<String>>,
    /// Checked reflection-visible field slots in declaration order. None cannot authorize reflection; ordinary
    /// private fields of a private model remain visible, while type-private fields are excluded.
    #[serde(default)]
    pub reflection_fields: Option<Vec<usize>>,
    /// Complete immutable records of reflection-visible source fields. None grants no metadata hook; presentation
    /// values do not change field types or access authority.
    #[serde(default)]
    pub reflection_metadata: Option<Vec<FieldMetadata>>,
    /// Canonical source body selected for a model, class or newtype's standard Eq native equality slot. Carrier
    /// admission may rely on this witness, but execution still requires that exact retained body and its validated
    /// borrowed input ABI.
    #[serde(default)]
    pub comparison_equality: Option<CanonicalSymbolId>,
    /// Whether the source nominal declaration is public.
    pub public: bool,
    /// Whether any declared field has a default; missing executable default facts must refuse omitted fields.
    pub has_field_defaults: bool,
    /// Declaration-owned deferred field computations, with one parameter slot per canonical field and a private
    /// local frame. The empty block is not a callable implementation; consumers evaluate only defaults selected by
    /// a checked constructor binding. Older artifacts without this frame must refuse omitted fields.
    #[serde(default)]
    pub field_default_body: Option<Box<Body>>,
    /// Declaration-owned frames of the owner's method partials, one per partial binding, each selected at a call only
    /// together with its exact target method identity.
    #[serde(default)]
    pub method_partial_defaults: Vec<MethodPartialDefaults>,
    /// Checker-selected source method that renders this nominal: an explicit string method or an Error message
    /// fallback. None retains the checked derived-Display path; consumers must prove a matching receiver and result
    /// signature before calling this identity. Older publications cannot authorize a custom display call.
    #[serde(default)]
    pub display_method: Option<CanonicalSymbolId>,
    /// Checked source reconstruction policy, distinguishing exact-hook validation from ordinary tuple Deserialize.
    /// Consumers close the existing nominal field and selected Body facts using exact checked owner arguments;
    /// this reference never authorizes a guessed hook, an unchecked payload, or ordinary derive reconstruction.
    #[serde(default)]
    pub newtype_deserialize: NewtypeDeserialize,
    /// Direct-only producer attestation for raw closed automatic conversion candidates. The existing canonical
    /// owner/layout remains authoritative; invocation activation and complete chain validation are separate.
    #[serde(default)]
    pub newtype_string_conversion: NewtypeStringConversion,
    /// Checked builtin and implicit nominal derives, retained in legacy normalization order. A selected `Default`
    /// over source field defaults requires a custom implementation rather than a Rust derive macro; consumers that
    /// cannot implement it must refuse that combination.
    pub derives: Vec<String>,
    /// Checked nominal bindings needed by those field types; serialized publications retain only referenced entries.
    /// Project carrier keys name a complete canonical identity here, independently of the diagnostic source name.
    pub named_type_identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
    /// Ordered checker-retained declaration parameters used to close nominal layouts at checked uses.
    #[serde(default)]
    pub type_parameters: Vec<String>,
    /// Number of declared type parameters, agreeing with [`Self::type_parameters`].
    pub type_parameter_count: usize,
}

/// Foreign runtime ownership for a compiler-known nominal with a checked builtin or stdlib source identity.
/// This direct-route bridge does not alter source resolution or legacy emission. A user declaration, including one
/// with the same name, never selects a foreign item; ordinary SDK layouts keep their compiled source item.
pub fn native_nominal_item(identity: &CanonicalSymbolId) -> Option<String> {
    if let crate::SymbolOrigin::RustCrate(path) = &identity.origin {
        if identity.namespace != crate::SymbolNamespace::OrdinaryLexical
            || identity.kind != crate::SemanticSourceTargetKind::RustItem
            || identity.scope_discriminant.is_some()
            || identity.declaration_span != crate::HirSourceSpan::new(0, 0)
            || path.len() < 2
            || path.last() != Some(&identity.declaration_name)
        {
            return None;
        }
        // A Rust item is stored as itself.
        return Some(path.join("::"));
    }
    if let Some(builtin) = crate::RuntimeBuiltinType::from_identity(identity) {
        return Some(builtin.carrier_path().to_owned());
    }
    let crate::SymbolOrigin::Package { library, module_path } = &identity.origin else {
        return None;
    };
    (matches!(library.as_str(), "incan_stdlib_core" | "incan_stdlib_primitives")
        && module_path == &["reflection".to_owned()]
        && identity.declaration_name == "FieldInfo"
        && identity.kind == crate::SemanticSourceTargetKind::Class)
        .then(|| "incan_std_primitives::reflection::FieldInfo".to_owned())
}

/// Physical direct-route declaration scope of a retained runtime carrier, or the existing source/package scope.
/// This is a layout transport coordinate, not a replacement semantic identity or a change to legacy resolution.
pub fn native_nominal_module(identity: &CanonicalSymbolId) -> Option<String> {
    if identity.origin == crate::SymbolOrigin::Builtin && native_nominal_item(identity).is_some() {
        return Some("builtin::native::ValidationError".to_owned());
    }
    if let crate::SymbolOrigin::RustCrate(path) = &identity.origin
        && native_nominal_item(identity).is_some()
    {
        return Some(format!("native::rust::{}", path.join("::")));
    }
    crate::canonical_module_identity(identity)
}

/// Exact direct-route ABI of a compiler-known runtime carrier. ValidationError stays opaque, with no field,
/// constructor or display operation; those require explicit Body IR runtime operations. Every Rust item, the web
/// extractors and responses included, is a Rust value laid out from its inspected facts instead.
pub fn native_carrier_nominal(identity: &CanonicalSymbolId) -> Option<NominalDeclaration> {
    if identity.origin != crate::SymbolOrigin::Builtin {
        return None;
    }
    let native_item = native_nominal_item(identity)?;
    let module = native_nominal_module(identity)?;
    let derives = ["Debug", "Clone", "PartialEq", "Eq"].map(str::to_owned).to_vec();
    Some(NominalDeclaration {
        trait_adoptions: Vec::new(),
        native_item,
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span(&module, 0, 0),
        canonical: identity.clone(),
        name: identity.declaration_name.clone(),
        fields: Vec::new(),
        field_identities: Vec::new(),
        field_types: Vec::new(),
        owned_callable_fields: Vec::new(),
        field_public: Vec::new(),
        field_aliases: Vec::new(),
        reflection_fields: None,
        reflection_metadata: None,
        comparison_equality: None,
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        newtype_deserialize: NewtypeDeserialize::Unselected,
        newtype_string_conversion: NewtypeStringConversion::Unselected,
        derives,
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    })
}

/// The name a Rust item's opaque nominal is laid out under, and the name a held value's type carries: the item's
/// path (`std::fs::File`). No Incan name can spell it, so a Rust item never shares a name with an Incan declaration.
pub fn native_opaque_name(identity: &CanonicalSymbolId) -> Option<String> {
    let crate::SymbolOrigin::RustCrate(path) = &identity.origin else {
        return None;
    };
    Some(path.join("::"))
}

/// Opaque storage of a Rust item a program holds as a value (DD-0006): the item itself, with `arity` written type
/// arguments and no fields or derives, so a value is moved, borrowed and passed but never projected into. It is named
/// [`native_opaque_name`].
pub fn native_opaque_nominal(identity: &CanonicalSymbolId, arity: usize) -> Option<NominalDeclaration> {
    let crate::SymbolOrigin::RustCrate(_) = &identity.origin else {
        return None;
    };
    let native_item = native_nominal_item(identity)?;
    let module = native_nominal_module(identity)?;
    let type_parameters = (0..arity).map(|index| format!("T{index}")).collect::<Vec<_>>();
    Some(NominalDeclaration {
        trait_adoptions: Vec::new(),
        native_item,
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span(&module, 0, 0),
        canonical: identity.clone(),
        name: native_opaque_name(identity)?,
        fields: Vec::new(),
        field_identities: Vec::new(),
        field_types: Vec::new(),
        owned_callable_fields: Vec::new(),
        field_public: Vec::new(),
        field_aliases: Vec::new(),
        reflection_fields: None,
        reflection_metadata: None,
        comparison_equality: None,
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        newtype_deserialize: NewtypeDeserialize::Unselected,
        newtype_string_conversion: NewtypeStringConversion::Unselected,
        derives: Vec::new(),
        named_type_identities: Default::default(),
        type_parameters,
        type_parameter_count: arity,
    })
}

/// The opaque nominals required by storage and checked external signatures (DD-0006), including their native type
/// arguments. Each item retains its checked argument count; a callee's item path alone requires no value layout.
/// An item held with two different argument counts has no single layout and is left out; its uses refuse.
pub fn held_rust_items(modules: &[BodyIrModule]) -> Vec<NominalDeclaration> {
    let mut arities = std::collections::BTreeMap::<String, Option<usize>>::new();
    let mut record = |ty: &IncanType| record_held_rust_items(ty, &mut arities);
    for module in modules {
        for callable in &module.loaf_callables {
            callable.parameters.iter().for_each(&mut record);
            callable.native_type_arguments.iter().for_each(&mut record);
            record(&callable.return_type);
            if let Some(frame) = &callable.declaration_frame {
                record_held_rust_frame(frame, &mut record);
            }
        }
        for body in &module.bodies {
            record_held_rust_frame(body, &mut record);
        }
        for declaration in &module.nominal_declarations {
            declaration.field_types.iter().for_each(&mut record);
        }
        for declaration in &module.enum_declarations {
            for variant in &declaration.variants {
                variant.fields.iter().for_each(&mut record);
            }
        }
        for storage in &module.static_declarations {
            record(&storage.ty);
            if let StaticInitializer::Computed(body) = &storage.initial {
                record_held_rust_frame(body, &mut record);
            }
        }
    }
    arities
        .into_iter()
        .filter_map(|(path, arity)| native_opaque_nominal(&rust_item_identity(&path)?, arity?))
        .collect()
}

/// Retain every declared storage type in an executable or imported default frame without granting its statements
/// execution.
pub(super) fn record_held_rust_frame(body: &Body, record: &mut impl FnMut(&IncanType)) {
    record(&body.return_type);
    body.locals.iter().for_each(|local| record(&local.ty));
    body.params.iter().for_each(|param| record(&param.ty));
}

/// Record the Rust items `ty` names anywhere inside it: a type a Rust value carries is named by its item's path.
pub(super) fn record_held_rust_items(ty: &IncanType, arities: &mut std::collections::BTreeMap<String, Option<usize>>) {
    match ty {
        IncanType::Named(name) => note_held_rust_item(name, 0, arities),
        IncanType::Generic { base, args } => {
            note_held_rust_item(base, args.len(), arities);
            args.iter().for_each(|arg| record_held_rust_items(arg, arities));
        }
        IncanType::Tuple(items) => items.iter().for_each(|item| record_held_rust_items(item, arities)),
        IncanType::Function { params, return_type } => {
            params
                .iter()
                .for_each(|param| record_held_rust_items(&param.ty, arities));
            record_held_rust_items(return_type, arities);
        }
        IncanType::TypeToken(inner)
        | IncanType::Ref(inner)
        | IncanType::RefMut(inner)
        | IncanType::NativeArray { element: inner, .. } => {
            record_held_rust_items(inner, arities);
        }
        _ => {}
    }
}

/// Note one Rust item a type names with its written argument count; a second, different count unsets it.
pub(super) fn note_held_rust_item(
    path: &str,
    arity: usize,
    arities: &mut std::collections::BTreeMap<String, Option<usize>>,
) {
    if rust_item_identity(path).is_none() {
        return;
    }
    arities
        .entry(path.to_owned())
        .and_modify(|known| {
            if *known != Some(arity) {
                *known = None;
            }
        })
        .or_insert(Some(arity));
}

/// The canonical identity of a metadata-selected Rust item (`std::sync::Arc`). Native slice methods use the internal
/// `[]::method` owner because slices have no nominal item path; no other nonidentifier segment is admitted.
pub fn rust_item_identity(path: &str) -> Option<CanonicalSymbolId> {
    let segments = path.split("::").map(str::to_owned).collect::<Vec<_>>();
    let name = segments.last()?.clone();
    let slice_method = segments.len() == 2 && segments[0] == "[]";
    if segments.len() < 2
        || segments
            .iter()
            .enumerate()
            .any(|(index, segment)| !(slice_method && index == 0) && !is_rust_identifier(segment))
    {
        return None;
    }
    Some(CanonicalSymbolId {
        namespace: crate::SymbolNamespace::OrdinaryLexical,
        origin: crate::SymbolOrigin::RustCrate(segments),
        declaration_name: name,
        kind: crate::SemanticSourceTargetKind::RustItem,
        scope_discriminant: None,
        declaration_span: crate::HirSourceSpan::new(0, 0),
    })
}

/// Whether `segment` is one Rust identifier (ASCII letters, digits and `_`, not starting with a digit).
pub(super) fn is_rust_identifier(segment: &str) -> bool {
    segment
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && segment.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// A checked local class inheritance edge and the nearest implementations of its ancestor source method slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassInheritance {
    /// The actual parent declaration identity; aliases cannot mint another class owner.
    pub parent: CanonicalSymbolId,
    /// Ancestor slots mapped to the implementation selected by checking, in canonical slot order.
    pub methods: Vec<ClassMethodImplementation>,
}

/// The prefix every [`nominal_carrier_key`] starts with; a name without it is no carrier key.
pub const NOMINAL_CARRIER_KEY_PREFIX: &str = "__incan_carrier_";

/// Encode one checked source nominal's complete owning module and declaration into an identifier-safe carrier key.
///
/// Only Body IR producers publish these keys, beside the original canonical identity in their type maps. Ordinary
/// standard and foreign storage keep their native names. A standard source declaration backed by a registry-owned
/// native carrier uses the same identity key as other source declarations, so its source and native owners cannot
/// collide with an equal-spelled local type. A key grants no lookup or layout without its retained identity.
pub fn nominal_carrier_key(identity: &CanonicalSymbolId) -> Option<String> {
    if identity.namespace != SymbolNamespace::OrdinaryLexical || identity.scope_discriminant.is_some() {
        return None;
    }
    let kind = match identity.kind {
        SemanticSourceTargetKind::Model => "model",
        SemanticSourceTargetKind::Class => "class",
        SemanticSourceTargetKind::Newtype => "newtype",
        SemanticSourceTargetKind::Enum => "enum",
        _ => return None,
    };
    match &identity.origin {
        SymbolOrigin::Package { library, .. }
            if library.starts_with("incan_stdlib_") && native_nominal_item(identity).is_none() =>
        {
            return None;
        }
        SymbolOrigin::Module(path) if path.first().is_some_and(|root| root == "std" || root == "__incan_std") => {
            return None;
        }
        SymbolOrigin::Module(_) | SymbolOrigin::Package { .. } => {}
        _ => return None,
    }
    let module = crate::canonical_module_identity(identity)?;
    // Carrier keys are retained in checked scopes and native plans. Reserve their exact spelling rather than a
    // worst-case suffix for every ordinary declaration; spare capacity otherwise survives in every owned key.
    let mut capacity = NOMINAL_CARRIER_KEY_PREFIX.len()
        + kind.len()
        + 6
        + 2 * (module.len() + identity.declaration_name.len())
        + carrier_decimal_digits(module.len())
        + carrier_decimal_digits(identity.declaration_name.len())
        + carrier_decimal_digits(identity.declaration_span.start)
        + carrier_decimal_digits(identity.declaration_span.end);
    if identity.declaration_span.expansion.is_some() {
        capacity += "_expansion_".len() + 64;
    }
    let mut key = String::with_capacity(capacity);
    write!(key, "{NOMINAL_CARRIER_KEY_PREFIX}{kind}_{}_", module.len()).ok()?;
    append_carrier_hex(&mut key, module.as_bytes());
    write!(key, "_{}_", identity.declaration_name.len()).ok()?;
    append_carrier_hex(&mut key, identity.declaration_name.as_bytes());
    write!(
        key,
        "_{}_{}",
        identity.declaration_span.start, identity.declaration_span.end
    )
    .ok()?;
    if let Some(expansion) = identity.declaration_span.expansion {
        key.push_str("_expansion_");
        append_carrier_hex(&mut key, &expansion);
    }
    Some(key)
}

/// Count the decimal digits of an unsigned carrier component without allocating a temporary string.
pub(super) fn carrier_decimal_digits(mut value: usize) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

/// Append identifier-safe lowercase hex without allocating a formatting buffer for each UTF-8 byte.
pub(super) fn append_carrier_hex(output: &mut String, bytes: &[u8]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
}

impl NominalDeclaration {
    /// Read a closed field's retained callable storage through the bootstrap's signed slot boundary. Invalid slots
    /// grant no owning representation.
    pub fn owns_callable_field(&self, slot: i64) -> bool {
        let Ok(slot) = usize::try_from(slot) else { return false };
        self.owned_callable_fields.contains(&slot)
    }

    /// Preserve the generic value category when closing a nominal's field types. A callable substituted for a
    /// declared value binder owns its captured environment, even though its checked signature is a function type.
    /// This boundary does not authorize a borrowed or consuming callable, or alter explicitly written function fields.
    pub fn retain_owned_callable_fields(&mut self, template: &Self) {
        self.owned_callable_fields.clear();
        if matches!(template.canonical.origin, crate::SymbolOrigin::RustCrate(_)) {
            return;
        }
        self.owned_callable_fields = template
            .field_types
            .iter()
            .zip(&self.field_types)
            .enumerate()
            .filter_map(|(slot, (original, closed))| {
                let IncanType::TypeVar(name) = original else {
                    return None;
                };
                (template.type_parameters.contains(name) && matches!(closed, IncanType::Function { .. }))
                    .then_some(slot)
            })
            .collect();
        // A generic template's automatic Debug is conditional on its actual field types. Owning an erased
        // callable preserves invocation and cloning, but grants no Debug implementation for the function value.
        if !self.owned_callable_fields.is_empty() {
            self.derives.retain(|derive| derive != "Debug");
        }
    }

    /// Read the declaration's generic base independently of a concrete instance's generated layout name. A checked
    /// carrier binding supplies source ownership; opaque native declarations retain their public Rust type path.
    pub fn template_carrier_name(&self) -> String {
        self.retained_carrier_key()
            .or_else(|| native_opaque_name(&self.canonical))
            .unwrap_or_else(|| self.canonical.declaration_name.clone())
    }

    /// Require the checker's exact binding before exposing an encoded source carrier key.
    pub(super) fn retained_carrier_key(&self) -> Option<String> {
        nominal_carrier_key(&self.canonical).filter(|key| self.named_type_identities.get(key) == Some(&self.canonical))
    }

    /// Read the producer-retained carrier key only when its complete identity agrees with this nongeneric layout.
    ///
    /// Older publications keep their source name and are still refused if project assembly finds an ambiguity.
    /// Generic templates retain their existing specialization keys; a missing or altered carrier map grants nothing.
    pub fn carrier_name(&self) -> String {
        self.retained_carrier_key()
            .filter(|_| self.type_parameter_count == 0)
            .unwrap_or_else(|| self.name.clone())
    }
}

impl EnumDeclaration {
    /// Read the source enum's identity-bound carrier independently of its diagnostic name. Closed instances retain
    /// their specialization symbol; native Rust enums keep their metadata-selected name. The validated layout's own
    /// canonical identity attests its owner; payload type bindings need not redundantly retain that identity.
    pub fn carrier_name(&self) -> String {
        if !self.native_type_arguments.is_empty() {
            return self.name.clone();
        }
        nominal_carrier_key(&self.canonical).unwrap_or_else(|| self.name.clone())
    }
}
