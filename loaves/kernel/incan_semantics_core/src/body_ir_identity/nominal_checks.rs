//! Well-formedness of nominal, class, newtype and enum declarations.

use super::*;

impl BodyIrModule {
    /// Whether a retained layout agrees with its canonical identities and physical declaration scope. Native layouts
    /// either match a fixed carrier or extend an opaque skeleton with public metadata-selected fields, whose exact
    /// native types rustc verifies. A linked Loaf's item must encode its canonical declaration. A selected native
    /// equality witness must be a member of its model; lowering separately validates that body's executable ABI.
    pub fn is_well_formed_nominal_declaration(&self, declaration: &NominalDeclaration) -> bool {
        if !Self::nominal_trait_adoptions_are_well_formed(declaration) {
            return false;
        }
        if !Self::nominal_reflection_metadata_matches_fields(declaration) {
            return false;
        }
        if !declaration.native_item.is_empty()
            && crate::body_ir::native_nominal_item(&declaration.canonical)
                .or_else(|| crate::body_ir::loaf_item_path(&declaration.canonical))
                .as_deref()
                != Some(declaration.native_item.as_str())
        {
            return false;
        }
        if matches!(
            declaration.canonical.origin,
            crate::SymbolOrigin::Builtin | crate::SymbolOrigin::RustCrate(_)
        ) {
            let exact = crate::body_ir::native_carrier_nominal(&declaration.canonical).or_else(|| {
                crate::body_ir::native_opaque_nominal(&declaration.canonical, declaration.type_parameter_count)
            });
            return (exact.as_ref() == Some(declaration) || self.is_well_formed_native_struct_layout(declaration))
                && crate::body_ir::native_nominal_module(&declaration.canonical).as_deref()
                    == Some(self.module_id.path());
        }
        if !declaration.native_type_arguments.is_empty() {
            return false;
        }
        if declaration.display_method.as_ref().is_some_and(|method| {
            method.kind != SemanticSourceTargetKind::Method || method.namespace != SymbolNamespace::Member
        }) {
            return false;
        }
        if declaration.comparison_equality.as_ref().is_some_and(|method| {
            !matches!(
                declaration.canonical.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class | SemanticSourceTargetKind::Newtype
            ) || method.kind != SemanticSourceTargetKind::Method
                || method.namespace != SymbolNamespace::Member
                || method.origin != declaration.canonical.origin
                || !declares_member(&declaration.canonical, method)
        }) {
            return false;
        }
        if declaration.type_parameters.len() != declaration.type_parameter_count
            || declaration.type_parameters.iter().any(String::is_empty)
            || declaration.type_parameters.iter().collect::<BTreeSet<_>>().len() != declaration.type_parameters.len()
        {
            return false;
        }
        if declaration.canonical.kind == SemanticSourceTargetKind::Newtype {
            return self.declaration_id_for_canonical(
                &declaration.canonical,
                SymbolNamespace::OrdinaryLexical,
                SemanticSourceTargetKind::Newtype,
            ) == Some(declaration.direct_declaration_id.clone())
                && declaration.canonical.declaration_name == declaration.name
                && declaration.fields == ["0"]
                && declaration.field_identities == [declaration.canonical.clone()]
                && declaration.field_types.len() == 1
                && declaration.field_public == [true]
                && declaration.field_aliases.iter().all(Option::is_none)
                && !declaration.has_field_defaults
                && declaration.class_inheritance.is_none()
                && declaration.method_partial_defaults.is_empty()
                && Self::has_well_formed_newtype_string_conversion(declaration)
                && self.has_well_formed_newtype_deserialize(declaration);
        }
        if declaration.newtype_deserialize != crate::body_ir::NewtypeDeserialize::Unselected
            || declaration.newtype_string_conversion != crate::body_ir::NewtypeStringConversion::Unselected
        {
            return false;
        }
        if !self.are_well_formed_method_partial_defaults(declaration) {
            return false;
        }
        if !self.class_inheritance_is_well_formed(declaration) {
            return false;
        }

        matches!(
            declaration.canonical.kind,
            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
        ) && self.declaration_id_for_canonical(
            &declaration.canonical,
            SymbolNamespace::OrdinaryLexical,
            declaration.canonical.kind.clone(),
        ) == Some(declaration.direct_declaration_id.clone())
            && declaration.canonical.declaration_name == declaration.name
            && declaration.fields.len() == declaration.field_identities.len()
            && declaration.fields.len() == declaration.field_types.len()
            && declaration.fields.len() == declaration.field_public.len()
            && (declaration.field_aliases.is_empty() || declaration.fields.len() == declaration.field_aliases.len())
            && declaration.reflection_fields.as_ref().is_none_or(|slots| {
                slots.iter().all(|slot| *slot < declaration.fields.len())
                    && slots.windows(2).all(|pair| pair[0] < pair[1])
            })
            && declaration.fields.iter().collect::<BTreeSet<_>>().len() == declaration.fields.len()
            && declaration.field_identities.iter().collect::<BTreeSet<_>>().len() == declaration.field_identities.len()
            && declaration
                .fields
                .iter()
                .zip(&declaration.field_identities)
                .enumerate()
                .all(|(index, (name, identity))| {
                    identity.namespace == SymbolNamespace::Member
                        && identity.kind == SemanticSourceTargetKind::Field
                        && identity.scope_discriminant.is_none()
                        && identity.origin == declaration.canonical.origin
                        && (declares_member(&declaration.canonical, identity)
                            || self.inherited_field_matches(declaration, index))
                        && identity.declaration_name == *name
                })
    }

    /// Require each retained reflection record to name its selected physical field in the same order.
    pub(super) fn nominal_reflection_metadata_matches_fields(declaration: &NominalDeclaration) -> bool {
        let Some(metadata) = &declaration.reflection_metadata else {
            return true;
        };
        let Some(slots) = &declaration.reflection_fields else {
            return false;
        };
        slots.len() == metadata.len()
            && slots
                .iter()
                .zip(metadata)
                .all(|(slot, record)| declaration.fields.get(*slot) == Some(&record.name))
    }

    /// Validate adoption identities, selected methods and primitive adapters without accepting duplicate records.
    pub(super) fn nominal_trait_adoptions_are_well_formed(declaration: &NominalDeclaration) -> bool {
        declaration.trait_adoptions.iter().enumerate().all(|(index, adoption)| {
            adoption.trait_identity.kind == SemanticSourceTargetKind::Trait
                && adoption.trait_identity.namespace == SymbolNamespace::OrdinaryLexical
                && !adoption.trait_identity.declaration_name.is_empty()
                && adoption.trait_identity.scope_discriminant.is_none()
                && adoption.methods.as_ref().is_none_or(|methods| {
                    methods.iter().enumerate().all(|(index, method)| {
                        method.kind == SemanticSourceTargetKind::Method
                            && method.namespace == SymbolNamespace::Member
                            && method.scope_discriminant.is_none()
                            && !methods[..index].contains(method)
                    })
                })
                && adoption
                    .primitive_adapter
                    .as_ref()
                    .is_none_or(|adapter| adapter.is_well_formed_for(declaration, adoption))
                && !declaration.trait_adoptions[..index].contains(adoption)
        })
    }

    /// Validate a retained native struct layout, including inaccessible private storage slots. Ordered fields, their
    /// checked visibility, native binders and Copy capability extend the opaque declaration; rustc verifies these
    /// facts.
    pub(super) fn is_well_formed_native_struct_layout(&self, declaration: &NominalDeclaration) -> bool {
        if !matches!(declaration.canonical.origin, crate::SymbolOrigin::RustCrate(_))
            || (declaration.fields.is_empty() && declaration.derives != ["Copy"])
            || declaration.fields.len() != declaration.field_types.len()
            || declaration.fields.len() != declaration.field_identities.len()
            || declaration.fields.len() != declaration.field_public.len()
            || declaration.fields.iter().any(String::is_empty)
            || declaration.fields.iter().collect::<BTreeSet<_>>().len() != declaration.fields.len()
            || declaration.type_parameters.len() != declaration.type_parameter_count
            || declaration.type_parameters.iter().any(String::is_empty)
            || declaration.type_parameters.iter().collect::<BTreeSet<_>>().len() != declaration.type_parameters.len()
        {
            return false;
        }
        for (name, field) in declaration.fields.iter().zip(&declaration.field_identities) {
            let mut expected = declaration.canonical.clone();
            expected.namespace = SymbolNamespace::Member;
            expected.kind = SemanticSourceTargetKind::Field;
            expected.declaration_name = name.clone();
            if *field != expected {
                return false;
            }
        }
        let Some(mut opaque) =
            crate::body_ir::native_opaque_nominal(&declaration.canonical, declaration.type_parameter_count)
        else {
            return false;
        };
        opaque.fields = declaration.fields.clone();
        opaque.field_types = declaration.field_types.clone();
        opaque.field_identities = declaration.field_identities.clone();
        opaque.field_public = declaration.field_public.clone();
        opaque.type_parameters = declaration.type_parameters.clone();
        if declaration.derives == ["Copy"] {
            opaque.derives = declaration.derives.clone();
        }
        opaque == *declaration
    }

    /// Validate a bounded local class edge and complete ancestor-slot mapping without recursively validating bodies.
    /// Original bodies are validated separately; nearest dispatch is checked against their retained owner spans.
    pub(super) fn class_inheritance_is_well_formed(&self, declaration: &NominalDeclaration) -> bool {
        let Some(inheritance) = &declaration.class_inheritance else {
            return true;
        };
        if declaration.canonical.kind != SemanticSourceTargetKind::Class
            || declaration.type_parameter_count != 0
            || declaration.has_field_defaults
            || !declaration.method_partial_defaults.is_empty()
        {
            return false;
        }
        let Some(chain) = self.class_owner_chain(declaration) else {
            return false;
        };
        let parent = chain[1];
        if declaration.fields.len() < parent.fields.len()
            || !parent.fields.iter().enumerate().all(|(index, name)| {
                declaration.fields.get(index) == Some(name)
                    && declaration.field_identities.get(index) == parent.field_identities.get(index)
                    && declaration.field_types.get(index) == parent.field_types.get(index)
                    && declaration.field_public.get(index) == parent.field_public.get(index)
            })
        {
            return false;
        }
        let ancestors = &chain[1..];
        let slots: BTreeSet<_> = self
            .bodies
            .iter()
            .filter_map(|body| body.canonical.as_ref())
            .filter(|method| {
                method.kind == SemanticSourceTargetKind::Method
                    && ancestors.iter().any(|owner| declares_member(&owner.canonical, method))
            })
            .collect();
        if inheritance.methods.len() != slots.len() {
            return false;
        }
        let mut recorded = BTreeSet::new();
        inheritance.methods.iter().all(|record| {
            slots.contains(&record.method)
                && recorded.insert(&record.method)
                && self.nearest_class_body_identity(&chain, &record.method) == Some(&record.implementation)
                && !self.decorated_bindings.iter().any(|binding| {
                    binding.original_body.canonical.as_ref() == Some(&record.method)
                        || binding.original_body.canonical.as_ref() == Some(&record.implementation)
                })
        })
    }

    /// Resolve a local class's concrete-to-oldest owner chain, rejecting cycles, unretained parents and open layouts.
    pub(super) fn class_owner_chain<'a>(
        &'a self,
        declaration: &'a NominalDeclaration,
    ) -> Option<Vec<&'a NominalDeclaration>> {
        let mut chain = vec![declaration];
        let mut seen = BTreeSet::from([&declaration.canonical]);
        let mut current = declaration;
        while let Some(inheritance) = &current.class_inheritance {
            let parent = self
                .nominal_declarations
                .iter()
                .find(|owner| owner.canonical == inheritance.parent)?;
            if !seen.insert(&parent.canonical)
                || parent.canonical.kind != SemanticSourceTargetKind::Class
                || parent.type_parameter_count != 0
                || parent.canonical.origin != declaration.canonical.origin
                || self.declaration_id_for_canonical(
                    &parent.canonical,
                    SymbolNamespace::OrdinaryLexical,
                    SemanticSourceTargetKind::Class,
                ) != Some(parent.direct_declaration_id.clone())
            {
                return None;
            }
            chain.push(parent);
            current = parent;
        }
        Some(chain)
    }

    /// Check an inherited field at its exact parent-first position, preserving identity, type and visibility.
    pub(super) fn inherited_field_matches(&self, declaration: &NominalDeclaration, index: usize) -> bool {
        let Some(chain) = self.class_owner_chain(declaration) else {
            return false;
        };
        declaration.class_inheritance.is_some()
            && chain[1..].iter().any(|parent| {
                parent.field_identities.get(index) == declaration.field_identities.get(index)
                    && parent.fields.get(index) == declaration.fields.get(index)
                    && parent.field_types.get(index) == declaration.field_types.get(index)
                    && parent.field_public.get(index) == declaration.field_public.get(index)
            })
    }

    /// Select the nearest retained original method body for one ancestor slot, without minting or rewriting identities.
    pub(super) fn nearest_class_body_identity<'a>(
        &'a self,
        chain: &[&NominalDeclaration],
        slot: &CanonicalSymbolId,
    ) -> Option<&'a CanonicalSymbolId> {
        for owner in chain {
            for body in &self.bodies {
                let Some(method) = &body.canonical else {
                    continue;
                };
                if method.kind == SemanticSourceTargetKind::Method
                    && method.declaration_name == slot.declaration_name
                    && method.origin == owner.canonical.origin
                    && declares_member(&owner.canonical, method)
                {
                    return Some(method);
                }
            }
        }
        None
    }

    /// Validate reconstruction authority against the exact attached owner and its unique checked source Body.
    /// Missing selection never authorizes Deserialize. Checked hooks retain no duplicate signature or binder facts;
    /// their existing frame must carry one owned underlying parameter and the exact owner/ValidationError Result.
    ///
    /// Nongeneric project owners use their identity-checked carrier key, matching the hook frame after projection.
    pub(super) fn has_well_formed_newtype_deserialize(&self, declaration: &NominalDeclaration) -> bool {
        use crate::body_ir::{CallableParamDefault, NewtypeDeserialize};
        let deserialize = declaration.derives.iter().any(|derive| derive == "serde::Deserialize");
        let hook = match &declaration.newtype_deserialize {
            NewtypeDeserialize::Unselected => return !deserialize,
            NewtypeDeserialize::Ordinary => return deserialize,
            NewtypeDeserialize::Checked(hook) if deserialize => hook,
            NewtypeDeserialize::Checked(_) => return false,
        };
        if hook.kind != SemanticSourceTargetKind::Method
            || hook.namespace != SymbolNamespace::Member
            || hook.origin != declaration.canonical.origin
            || !declares_member(&declaration.canonical, hook)
        {
            return false;
        }
        let Some(body) = self.body_for_canonical_target(hook) else {
            return false;
        };
        let [parameter] = body.params.as_slice() else {
            return false;
        };
        let [underlying] = declaration.field_types.as_slice() else {
            return false;
        };
        // This check is nonrecursive: body_has_canonical_direct_call_id validates the owner's layout in turn.
        if self.declaration_id_for_canonical(hook, SymbolNamespace::Member, SemanticSourceTargetKind::Method)
            != Some(body.direct_call_id.clone())
            || body.direct_call_id != CompilerNodeId::declaration_at(self.module_id.path(), body.span)
            || body.name != hook.declaration_name
            || body.is_async
            || body.is_generator()
            || body.type_parameters != declaration.type_parameters
            || parameter.mutable
            || !matches!(parameter.default, CallableParamDefault::Required)
            || &parameter.ty != underlying
        {
            return false;
        }
        let owner = if declaration.type_parameters.is_empty() {
            if underlying != &IncanType::Primitive(IncanPrimitiveType::Str) {
                return false;
            }
            IncanType::Named(declaration.carrier_name())
        } else {
            let [name] = declaration.type_parameters.as_slice() else {
                return false;
            };
            if underlying != &IncanType::TypeVar(name.clone()) {
                return false;
            }
            IncanType::Generic {
                base: declaration.template_carrier_name(),
                args: vec![underlying.clone()],
            }
        };
        let expected = [
            owner,
            IncanType::NativeBuiltin(crate::RuntimeBuiltinType::ValidationError),
        ];
        body.return_type.collection_arguments(CollectionTypeId::Result) == Some(expected.as_slice())
    }

    /// Check retained storage consistency without inferring support or activation from a raw tuple layout.
    /// The producer's automatic selection attests source-only exclusions that physical layout cannot recover.
    /// Qualified carrier keys must encode the exact retained inner identity; a matching source name alone cannot
    /// substitute another owner.
    pub(super) fn has_well_formed_newtype_string_conversion(declaration: &NominalDeclaration) -> bool {
        use crate::body_ir::NewtypeStringConversion;
        if declaration.newtype_string_conversion == NewtypeStringConversion::Unselected {
            return true;
        }
        declaration.type_parameters.is_empty()
            && declaration.field_default_body.is_none()
            && !matches!(
                declaration.newtype_deserialize,
                crate::body_ir::NewtypeDeserialize::Checked(_)
            )
            && match declaration.field_types.as_slice() {
                [IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Str)] => true,
                [IncanType::Named(name)] => declaration.named_type_identities.get(name).is_some_and(|identity| {
                    identity.kind == SemanticSourceTargetKind::Newtype
                        && (identity.declaration_name == *name
                            || crate::body_ir::nominal_carrier_key(identity).as_ref() == Some(name))
                }),
                _ => false,
            }
    }

    /// Whether each method-partial frame of a model or class is a uniquely named binding inside its owner whose frame
    /// is the target method's own surface: the checked target is a method of this owner, and the frame carries that
    /// method's identity, name, span and direct call id, with the owner's receiver first.
    pub(super) fn are_well_formed_method_partial_defaults(&self, declaration: &NominalDeclaration) -> bool {
        let partials = &declaration.method_partial_defaults;
        partials
            .iter()
            .map(|binding| &binding.name)
            .collect::<BTreeSet<_>>()
            .len()
            == partials.len()
            && partials.iter().all(|binding| {
                !binding.name.is_empty()
                    && binding.span.start < binding.span.end
                    && binding.span.start >= declaration.canonical.declaration_span.start
                    && binding.span.end <= declaration.canonical.declaration_span.end
                    && binding.target.kind == SemanticSourceTargetKind::Method
                    && declares_member(&declaration.canonical, &binding.target)
                    && binding.frame.canonical.as_ref() == Some(&binding.target)
                    && binding.frame.name == binding.target.declaration_name
                    && self.declaration_id_for_canonical(
                        &binding.target,
                        SymbolNamespace::Member,
                        SemanticSourceTargetKind::Method,
                    ) == Some(binding.frame.direct_call_id.clone())
                    && binding
                        .frame
                        .params
                        .first()
                        .is_some_and(|receiver| receiver.ty == nominal_receiver_type(declaration))
            })
    }

    /// Whether a retained fieldless enum is bound to this module, owner and members alike.
    pub fn is_well_formed_fieldless_enum_declaration(&self, declaration: &FieldlessEnumDeclaration) -> bool {
        self.is_well_formed_enum_declaration(
            &declaration.direct_declaration_id,
            &declaration.canonical,
            &declaration.name,
            &declaration.variants,
            |variant| {
                (
                    &variant.direct_declaration_id,
                    &variant.canonical,
                    variant.name.as_str(),
                )
            },
        )
    }

    /// Whether a retained value enum is bound to this module, owner and members alike.
    pub fn is_well_formed_value_enum_declaration(&self, declaration: &ValueEnumDeclaration) -> bool {
        self.is_well_formed_enum_declaration(
            &declaration.direct_declaration_id,
            &declaration.canonical,
            &declaration.name,
            &declaration.variants,
            |variant| {
                (
                    &variant.direct_declaration_id,
                    &variant.canonical,
                    variant.name.as_str(),
                )
            },
        )
    }

    /// One retained enum owner and member registry, validated without recovering any identity from a spelling.
    pub(super) fn is_well_formed_enum_declaration<T>(
        &self,
        direct_declaration_id: &CompilerNodeId,
        canonical: &CanonicalSymbolId,
        name: &str,
        variants: &[T],
        variant_facts: impl Fn(&T) -> (&CompilerNodeId, &CanonicalSymbolId, &str),
    ) -> bool {
        self.declaration_id_for_canonical(
            canonical,
            SymbolNamespace::OrdinaryLexical,
            SemanticSourceTargetKind::Enum,
        ) == Some(direct_declaration_id.clone())
            && canonical.declaration_name == name
            && variants
                .iter()
                .map(|variant| variant_facts(variant).0)
                .collect::<BTreeSet<_>>()
                .len()
                == variants.len()
            && variants
                .iter()
                .map(|variant| variant_facts(variant).2)
                .collect::<BTreeSet<_>>()
                .len()
                == variants.len()
            && variants.iter().all(|variant| {
                let (direct_variant_id, variant_canonical, variant_name) = variant_facts(variant);
                self.declaration_id_for_canonical(
                    variant_canonical,
                    SymbolNamespace::Member,
                    SemanticSourceTargetKind::Variant,
                ) == Some(direct_variant_id.clone())
                    && variant_canonical.origin == canonical.origin
                    && declares_member(canonical, variant_canonical)
                    && variant_canonical.declaration_name == variant_name
            })
    }
}
