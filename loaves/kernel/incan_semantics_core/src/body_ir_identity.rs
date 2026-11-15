//! Identity well-formedness of a [`BodyIrModule`]: the checks a consumer runs before it trusts a module's span-derived
//! declaration identities, retained declaration records and direct call targets.
//!
//! Lowering derives every same-module identity from a declaration's span and carries the checker's canonical identity
//! beside it. Body IR travels as data (in a published executable representation, for instance), so a consumer cannot
//! take those facts on trust: a malformed module could copy a valid identity onto an unrelated body or carry a
//! coherent-looking foreign record. These checks never mint or complete an identity; they only confirm that the
//! retained ones agree with each other and belong to this module. They answer the same question for every consumer.

use incan_lang::lang::types::collections::CollectionTypeId;
use std::collections::BTreeSet;

use crate::body_ir::{
    Body, BodyIrModule, CallableParamDefault, Constant, FieldlessEnumDeclaration, LocalOrigin, NominalDeclaration,
    StaticInitializer, ValueEnumDeclaration,
};
use crate::{
    CanonicalSymbolId, CompilerNodeId, CompilerNodeKind, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
    SymbolNamespace, SymbolOrigin, canonical_module_identity,
};

mod declaration_checks;
mod nominal_checks;

/// Match an initialized method's explicit passing reference against its receiver frame's nominal type and mode.
/// Ordinary inputs must agree exactly; this does not authorize another owner or a different receiver mode.
fn decorated_parameter_matches(
    body: &Body,
    left: &crate::IncanCallableParam,
    right: &crate::body_ir::CallableParam,
) -> bool {
    if left.kind != right.kind || left.native_owned != right.native_owned || left.input_access != right.input_access {
        return false;
    }
    match body.locals.get(right.local.index()).map(|local| &local.origin) {
        Some(LocalOrigin::Receiver { mutable }) => match (&left.ty, mutable) {
            (IncanType::Ref(inner), false) | (IncanType::RefMut(inner), true) => **inner == right.ty && !left.is_mut,
            _ => false,
        },
        _ => decorated_input_type_matches(left, &right.ty) && left.is_mut == right.mutable,
    }
}

/// Require each retained rest frame to contain exactly the source signature's checked element type.
fn decorated_input_type_matches(parameter: &crate::IncanCallableParam, packed: &IncanType) -> bool {
    match parameter.kind {
        crate::IncanCallableParamKind::Normal => parameter.ty == *packed,
        crate::IncanCallableParamKind::RestPositional => matches!(packed,
            IncanType::Generic { base, args } if base == "List" && args.as_slice() == [parameter.ty.clone()]),
        crate::IncanCallableParamKind::RestKeyword => matches!(packed,
            IncanType::Generic { base, args } if base == "Dict"
                && args.as_slice() == [IncanType::Primitive(IncanPrimitiveType::Str), parameter.ty.clone()]),
    }
}

/// Return the declaration's open receiver type in its retained parameter order, never a concrete inferred use.
fn nominal_receiver_type(owner: &NominalDeclaration) -> crate::IncanType {
    if owner.type_parameters.is_empty() {
        crate::IncanType::Named(owner.carrier_name())
    } else {
        crate::IncanType::Generic {
            base: owner.template_carrier_name(),
            args: owner
                .type_parameters
                .iter()
                .cloned()
                .map(crate::IncanType::TypeVar)
                .collect(),
        }
    }
}

/// Whether a static initializer's literals match its storage carrier: a primitive, or a list, dictionary or set of
/// primitives whose hashed keys and members exclude floating point, as legacy hashed collections do. A computed
/// initializer is a closed body with no parameters that returns exactly the storage carrier.
fn static_initializer_matches_parts(ty: &IncanType, initial: &StaticInitializer) -> bool {
    match initial {
        StaticInitializer::Computed(body) => {
            body.params.is_empty()
                && body.param_locals.is_empty()
                && body.type_parameters.is_empty()
                && !body.is_async
                && body.return_type == *ty
        }
        StaticInitializer::Literal(value) => literal_static_carrier_matches(ty, value),
        StaticInitializer::List(values) => match ty.collection_arguments(CollectionTypeId::List) {
            Some([element]) => {
                element.static_literal_carrier()
                    && values
                        .iter()
                        .all(|value| literal_static_carrier_matches(element, value))
            }
            _ => false,
        },
        StaticInitializer::Dict(entries) => match ty.collection_arguments(CollectionTypeId::Dict) {
            Some([key, value]) => {
                key.hashed_static_literal_carrier()
                    && value.static_literal_carrier()
                    && entries.iter().all(|(entry_key, entry_value)| {
                        literal_static_carrier_matches(key, entry_key)
                            && literal_static_carrier_matches(value, entry_value)
                    })
            }
            _ => false,
        },
        StaticInitializer::Set(members) => match ty.collection_arguments(CollectionTypeId::Set) {
            Some([element]) => {
                element.hashed_static_literal_carrier()
                    && members
                        .iter()
                        .all(|member| literal_static_carrier_matches(element, member))
            }
            _ => false,
        },
    }
}

/// Validate a literal payload against its exact primitive storage or collection element carrier.
fn literal_static_carrier_matches(ty: &IncanType, value: &Constant) -> bool {
    matches!(
        (ty, value),
        (IncanType::Primitive(IncanPrimitiveType::Int), Constant::Int(_))
            | (IncanType::Primitive(IncanPrimitiveType::Float), Constant::Float(_))
            | (IncanType::Primitive(IncanPrimitiveType::Bool), Constant::Bool(_))
            | (IncanType::Primitive(IncanPrimitiveType::Str), Constant::Str(_))
    )
}

impl crate::body_ir::PrimitiveTraitAdapter {
    /// Return the exact source carrier admitted for an adapter family. Checker retention and publication validation
    /// share this projection so a registered family cannot receive inconsistent payload-field authority.
    pub fn registered_receiver_type(
        family: incan_lang::lang::trait_capabilities::TraitCapabilityType,
    ) -> Option<IncanType> {
        use crate::IncanPrimitiveType;
        use incan_lang::lang::trait_capabilities::TraitCapabilityType;
        match family {
            TraitCapabilityType::Str => Some(IncanType::Primitive(IncanPrimitiveType::Str)),
            TraitCapabilityType::Int => Some(IncanType::Primitive(IncanPrimitiveType::Int)),
            TraitCapabilityType::Bool => Some(IncanType::Primitive(IncanPrimitiveType::Bool)),
            TraitCapabilityType::Numeric(numeric) => Some(IncanType::Primitive(IncanPrimitiveType::Numeric(numeric))),
            TraitCapabilityType::Decimal => Some(IncanType::Decimal {
                precision: 38,
                scale: 0,
            }),
            _ => None,
        }
    }

    /// Validate the registered source owner, its sole payload field, exact trait and owned decoder identity.
    /// Executable consumers must additionally validate the retained method bodies and decoder result signature.
    pub fn is_well_formed_for(&self, owner: &NominalDeclaration, adoption: &crate::body_ir::TraitAdoption) -> bool {
        let SymbolOrigin::Package { library, module_path } = &owner.canonical.origin else {
            return false;
        };
        let SymbolOrigin::Package {
            library: trait_library,
            module_path: trait_module,
        } = &adoption.trait_identity.origin
        else {
            return false;
        };
        let registered = incan_lang::lang::trait_capabilities::PRIMITIVE_SOURCE_ADAPTERS
            .iter()
            .any(|specification| {
                specification.matches_owner(library, module_path, &owner.canonical.declaration_name)
                    && specification.matches_trait(
                        trait_library,
                        trait_module,
                        &adoption.trait_identity.declaration_name,
                    )
                    && Self::registered_receiver_type(specification.receiver).as_ref() == Some(&self.receiver_type)
                    && self.payload_field.declaration_name == specification.payload_field
                    && self.decode_slot.declaration_name == specification.decode_slot
                    && self.decoded_value.declaration_name == specification.decoded_value
            });
        registered
            && owner.canonical.kind == SemanticSourceTargetKind::Model
            && owner.type_parameters.is_empty()
            && owner.field_types.as_slice() == [self.receiver_type.clone()]
            && owner.field_identities.as_slice() == [self.payload_field.clone()]
            && owner.field_public.as_slice() == [true]
            && adoption.arguments.is_empty()
            && adoption.methods.is_some()
            && adoption
                .methods
                .as_ref()
                .is_some_and(|methods| methods.contains(&self.decode_slot))
            && self.decode_slot.kind == SemanticSourceTargetKind::Method
            && self.decode_slot.namespace == SymbolNamespace::Member
            && self.decode_slot.origin == adoption.trait_identity.origin
            && declares_member(&adoption.trait_identity, &self.decode_slot)
            && self.decoded_value.kind == SemanticSourceTargetKind::Method
            && self.decoded_value.namespace == SymbolNamespace::Member
            && self.decoded_value.scope_discriminant.is_none()
            && self.decoded_value.origin == owner.canonical.origin
            && declares_member(&owner.canonical, &self.decoded_value)
    }
}

/// Whether `member` is declared inside `owner`: its declaration site lies within the owner's.
///
/// A canonical identity's origin names the module, not the declaration, so two declarations of one module with a
/// same-named member (`A.value` and `B.value`) give member identities that agree on everything else. The declaration
/// site is the one fact that tells them apart.
fn declares_member(owner: &CanonicalSymbolId, member: &CanonicalSymbolId) -> bool {
    owner.declaration_span.start <= member.declaration_span.start
        && member.declaration_span.end <= owner.declaration_span.end
}

/// Validate the implicit immutable getter receiver independently of the member's owning nominal layout.
/// This rejects forged static, mutable, generic-method or ordinary-parameter property bodies before scheduling.
fn is_getter_frame(body: &Body, canonical: &CanonicalSymbolId) -> bool {
    let Some(receiver) = body.locals.first() else {
        return false;
    };
    let [parameter] = body.params.as_slice() else {
        return false;
    };
    let Some(identity) = receiver.identity.as_ref() else {
        return false;
    };
    !body.is_async
        && body.type_parameters.is_empty()
        && body.param_locals.as_slice() == [receiver.id]
        && matches!(receiver.origin, LocalOrigin::Receiver { mutable: false })
        && identity.namespace == SymbolNamespace::OrdinaryLexical
        && identity.kind == SemanticSourceTargetKind::Receiver
        && identity.origin == canonical.origin
        && identity.declaration_name == "self"
        && identity.declaration_span == body.span
        && parameter.local == receiver.id
        && parameter.ty == receiver.ty
        && parameter.span == body.span
        && receiver.span == body.span
        && parameter.name == "self"
        && !parameter.mutable
        && matches!(parameter.default, CallableParamDefault::Required)
}

/// Whether a checked type names no open type: no type variable, `Self`, or uninferred placeholder at any depth.
///
/// A trait default instantiated for a concrete adopter must receive closed type arguments; an open one would leave
/// the default generic after specialization.
fn is_closed_type(ty: &IncanType) -> bool {
    is_owner_bound_type(ty, &[])
}

/// Validate one argument tree in a retained owner's declared parameter scope, without resolving names or inferring
/// missing arguments. An empty scope preserves the closed-type contract used outside nominal templates.
fn is_owner_bound_type(ty: &IncanType, parameters: &[String]) -> bool {
    match ty {
        IncanType::TypeVar(name) => parameters.iter().any(|parameter| parameter == name),
        IncanType::SelfType | IncanType::Infer | IncanType::Unknown | IncanType::Unsupported(_) => false,
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => {
            args.iter().all(|argument| is_owner_bound_type(argument, parameters))
        }
        IncanType::Function { params, return_type } => {
            params.iter().all(|param| is_owner_bound_type(&param.ty, parameters))
                && is_owner_bound_type(return_type, parameters)
        }
        IncanType::TypeToken(inner)
        | IncanType::Ref(inner)
        | IncanType::RefMut(inner)
        | IncanType::NativeArray { element: inner, .. } => is_owner_bound_type(inner, parameters),
        IncanType::Never
        | IncanType::Primitive(_)
        | IncanType::Named(_)
        | IncanType::Trait(_)
        | IncanType::Decimal { .. }
        | IncanType::RustInteropPath(_)
        | IncanType::NativeTraitObject { .. }
        | IncanType::NativeBuiltin(_)
        | IncanType::NativeCChar => true,
        IncanType::NativePointer { pointee, .. } => is_owner_bound_type(pointee, parameters),
    }
}
