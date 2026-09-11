//! Shared traversal of typed API leaves for publication and admitted-provider projection.

use std::collections::BTreeMap;

use super::model::{
    AliasExport, ClassExport, ConstExport, EnumExport, EnumVariantExport, FieldExport, FieldRequirementExport,
    FunctionExport, ImplementationAssociatedTypeExport, ImplementationTraitBoundExport, ImplementationTypeParamExport,
    LibraryContractMetadata, LibraryExports, LibraryManifest, MethodExport, ModelExport, NewtypeExport,
    NominalTypeOriginExport, ParamDefaultCallArgExport, ParamDefaultCallSignatureExport, ParamDefaultDictEntryExport,
    ParamDefaultExport, ParamExport, PartialExport, PartialPresetExport, PropertyExport, StaticExport, TraitExport,
    TypeAliasExport, TypeBoundExport, TypeParamExport, TypeRef,
};
use crate::frontend::api_metadata::{
    ApiAlias, ApiCallableMetadata, ApiClass, ApiConst, ApiDeclaration, ApiEnum, ApiEnumVariant, ApiFunction, ApiMethod,
    ApiModel, ApiNewtype, ApiPartial, ApiProjectedFunction, ApiProperty, ApiStatic, ApiTrait, ApiTypeAlias,
    CheckedApiMetadata, CheckedApiMetadataPackage, DecoratorArgMetadata, DecoratorCallArgMetadata, DecoratorDictEntry,
    DecoratorMetadata, DecoratorValue,
};

/// Traverse every semantic type position, including generic heads, bounds, defaults and callable binders.
///
/// Publication and provider admission share this traversal so a type position cannot silently lose origin metadata
/// in one projection while retaining it in another. Presentation-only strings are deliberately outside this API.
pub(crate) trait VisitTypeRefs {
    /// Visit this value and every nested semantic type position.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef));
}

impl<T: VisitTypeRefs> VisitTypeRefs for Vec<T> {
    /// Apply the shared semantic type visitor to this container and its nested values.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
        for value in self {
            value.visit_type_refs(visit);
        }
    }
}
impl<T: VisitTypeRefs> VisitTypeRefs for Option<T> {
    /// Apply the shared semantic type visitor to this container and its nested values.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
        if let Some(value) = self {
            value.visit_type_refs(visit);
        }
    }
}
impl<T: VisitTypeRefs> VisitTypeRefs for Box<T> {
    /// Apply the shared semantic type visitor to this container and its nested values.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
        (**self).visit_type_refs(visit);
    }
}
impl VisitTypeRefs for TypeRef {
    /// Apply the shared semantic type visitor to this container and its nested values.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
        visit(self);
        match self {
            Self::Applied { args, .. } => args.visit_type_refs(visit),
            Self::NativeUnion(native) => native.members.visit_type_refs(visit),
            Self::Function { params, return_type } => {
                params.visit_type_refs(visit);
                return_type.visit_type_refs(visit);
            }
            Self::TypeToken { inner } | Self::Ref { inner } => inner.visit_type_refs(visit),
            Self::Tuple { elements } => elements.visit_type_refs(visit),
            Self::Named { .. } | Self::TypeParam { .. } | Self::SelfType | Self::RustPath { .. } | Self::Unknown => {}
        }
    }
}

macro_rules! type_fields {
    ($($ty:ty => [$($field:ident),*];)*) => { $(
        impl VisitTypeRefs for $ty {
            /// Apply the shared semantic type visitor to this container and its nested values.
            fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
                $(self.$field.visit_type_refs(visit);)*
            }
        }
    )* };
}

type_fields! {
    LibraryManifest => [exports, contract_metadata];
    LibraryContractMetadata => [api];
    LibraryExports => [aliases, partials, models, classes, functions, traits, enums, type_aliases, newtypes, consts, statics];
    TypeParamExport => [bounds];
    TypeBoundExport => [type_args, implementation_type_params];
    ImplementationTypeParamExport => [bounds];
    ImplementationTraitBoundExport => [type_args, associated_types];
    ImplementationAssociatedTypeExport => [ty];
    ParamExport => [ty, default];
    ParamDefaultCallSignatureExport => [params, return_type];
    ParamDefaultCallArgExport => [value];
    ParamDefaultDictEntryExport => [key, value];
    FunctionExport => [type_params, params, return_type];
    MethodExport => [type_params, params, return_type];
    FieldExport => [ty, default];
    PropertyExport => [return_type];
    FieldRequirementExport => [ty];
    PartialPresetExport => [ty];
    PartialExport => [presets, type_params, params, return_type];
    AliasExport => [projected_function, projected_type];
    TypeAliasExport => [type_params, target];
    ModelExport => [type_params, trait_adoptions, fields, properties, methods];
    ClassExport => [type_params, trait_adoptions, fields, properties, methods];
    TraitExport => [type_params, supertraits, requires, methods];
    EnumExport => [type_params, trait_adoptions, variants, methods];
    EnumVariantExport => [fields];
    NewtypeExport => [type_params, trait_adoptions, underlying, methods];
    ConstExport => [ty];
    StaticExport => [ty];
    CheckedApiMetadataPackage => [modules];
    CheckedApiMetadata => [declarations];
    ApiFunction => [decorators, type_params, params, return_type];
    ApiMethod => [decorators, type_params, params, return_type];
    ApiCallableMetadata => [type_params, params, return_type];
    ApiProjectedFunction => [callable, decorators];
    ApiModel => [decorators, type_params, trait_adoptions, fields, properties, methods];
    ApiClass => [decorators, type_params, trait_adoptions, fields, properties, methods];
    ApiProperty => [return_type];
    ApiTrait => [decorators, type_params, supertraits, requires, methods];
    ApiEnum => [decorators, type_params, trait_adoptions, variants, methods];
    ApiEnumVariant => [fields];
    ApiNewtype => [decorators, type_params, trait_adoptions, underlying, methods];
    ApiTypeAlias => [type_alias];
    ApiConst => [ty];
    ApiStatic => [ty];
    ApiAlias => [projected_function, projected_type];
    ApiPartial => [presets, type_params, params, return_type];
    DecoratorMetadata => [type_args, args, decorated_callable];
    DecoratorDictEntry => [key, value];
}

impl VisitTypeRefs for ApiDeclaration {
    /// Apply the shared semantic type visitor to this container and its nested values.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
        match self {
            Self::Function(value) => value.visit_type_refs(visit),
            Self::Model(value) => value.visit_type_refs(visit),
            Self::Class(value) => value.visit_type_refs(visit),
            Self::Trait(value) => value.visit_type_refs(visit),
            Self::Enum(value) => value.visit_type_refs(visit),
            Self::Newtype(value) => value.visit_type_refs(visit),
            Self::TypeAlias(value) => value.visit_type_refs(visit),
            Self::Const(value) => value.visit_type_refs(visit),
            Self::Static(value) => value.visit_type_refs(visit),
            Self::Alias(value) => value.visit_type_refs(visit),
            Self::Partial(value) => value.visit_type_refs(visit),
        }
    }
}

impl VisitTypeRefs for ParamDefaultExport {
    /// Apply the shared semantic type visitor to this container and its nested values.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
        match self {
            Self::List(values) => values.visit_type_refs(visit),
            Self::Dict(values) => values.visit_type_refs(visit),
            Self::Call { args, signature, .. } => {
                args.visit_type_refs(visit);
                signature.visit_type_refs(visit);
            }
            Self::Int(_)
            | Self::Float(_)
            | Self::Bool(_)
            | Self::String(_)
            | Self::Bytes(_)
            | Self::None
            | Self::ConstRef(_)
            | Self::Unsupported => {}
        }
    }
}

/// Project an emitter-owned metadata copy through routes retained by the successful checker.
///
/// Origin-bearing names never double as physical paths. Absolute Rust paths preserve the importing crate root
/// through provider-local union normalization; an absent route produces an unsupported type, never a guessed name.
pub(crate) fn with_checked_type_routes<T: VisitTypeRefs>(
    mut value: T,
    routes: Option<&std::collections::HashMap<String, String>>,
) -> T {
    let mut native_members = Vec::new();
    value.visit_type_refs(&mut |ty| {
        if let TypeRef::NativeUnion(native) = ty {
            native_members.push((native.owner.clone(), native.rust_name.clone(), native.members.clone()));
        }
    });
    value.visit_type_refs(&mut |leaf| {
        if let TypeRef::Named { name, origin } | TypeRef::Applied { name, origin, .. } = leaf
            && let Some(identity) = origin.take()
        {
            if let Some(route) = routes
                .and_then(|routes| routes.get(&identity.binding_key()))
                .and_then(|route| route.strip_prefix("pub::"))
            {
                *name = format!("::{route}");
            } else {
                *leaf = TypeRef::Unknown;
            }
        }
    });
    // Native wire members remain semantic evidence. The admitted projection already carries its separately routed
    // member copy, so this general nominal rewrite must not replace the producer's immutable member identities.
    value.visit_type_refs(&mut |ty| {
        if let TypeRef::NativeUnion(native) = ty
            && let Some((_, _, members)) = native_members
                .iter()
                .find(|(owner, name, _)| owner == &native.owner && name == &native.rust_name)
        {
            native.members = members.clone();
        }
    });
    value
}

/// Attach origins selected by the declaring checker to their exact named or generic type positions.
///
/// The map contains accepted bindings from this declaration's module. It is never serialized or used to resolve a
/// consumer name: the resulting leaf carries the exact artifact and declaration identity through later projections.
pub(crate) fn with_checked_type_origins<T: VisitTypeRefs>(
    mut value: T,
    origins: &BTreeMap<String, NominalTypeOriginExport>,
) -> T {
    value.visit_type_refs(&mut |ty| {
        if let TypeRef::Named { name, origin } | TypeRef::Applied { name, origin, .. } = ty
            && origin.is_none()
        {
            *origin = origins.get(name).cloned();
        }
    });
    value
}

impl VisitTypeRefs for DecoratorArgMetadata {
    /// Preserve typed decorator arguments through both positional and named metadata projections.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
        match self {
            Self::Positional { value } | Self::Named { value, .. } => value.visit_type_refs(visit),
        }
    }
}
impl VisitTypeRefs for DecoratorCallArgMetadata {
    /// Traverse ordinary and unpacked symbolic call arguments without interpreting their values.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
        match self {
            Self::Positional { value }
            | Self::Named { value, .. }
            | Self::PositionalUnpack { value }
            | Self::KeywordUnpack { value } => value.visit_type_refs(visit),
        }
    }
}
impl VisitTypeRefs for DecoratorValue {
    /// Retain nominal origins inside every structured decorator value that contains a semantic type.
    fn visit_type_refs(&mut self, visit: &mut impl FnMut(&mut TypeRef)) {
        match self {
            Self::List { items } => items.visit_type_refs(visit),
            Self::Dict { entries } => entries.visit_type_refs(visit),
            Self::Call { type_args, args, .. } => {
                type_args.visit_type_refs(visit);
                args.visit_type_refs(visit);
            }
            Self::Type { ty } => ty.visit_type_refs(visit),
            Self::Literal { .. } | Self::ConstRef { .. } | Self::SymbolRef { .. } | Self::Unsupported { .. } => {}
        }
    }
}

/// Return whether any type reference inside a value is an anonymous native union.
///
/// This is the single implementation. `TypeRef::has_native_union` is the same question asked of one type
/// reference, and delegates here; nothing else should re-fold the visitor to answer it, because a second fold is
/// how the two spellings drifted apart in the first place.
pub(crate) fn contains_native_union(value: &(impl VisitTypeRefs + Clone)) -> bool {
    let mut found = false;
    value
        .clone()
        .visit_type_refs(&mut |ty| found |= matches!(ty, crate::library_manifest::TypeRef::NativeUnion(_)));
    found
}

/// Bind native carriers through the existing admitted provider graph before projecting a compiler-owned API copy.
///
/// The original producer members stay immutable. Only the non-serialized physical projection is routed for emission;
/// its owner is selected by the admitted artifact, never inferred from a generated wrapper spelling.
pub(crate) fn with_checked_native_unions<T: VisitTypeRefs>(
    mut value: T,
    library: &str,
    plan: Option<&crate::provider::ProviderPlan>,
    routes: Option<&std::collections::HashMap<String, String>>,
) -> Result<T, String> {
    let mut failure = None;
    value.visit_type_refs(&mut |ty| {
        if failure.is_some() {
            return;
        }
        let TypeRef::NativeUnion(native) = ty else {
            return;
        };
        if native.checked_projection.is_some() {
            return;
        }
        let Some(plan) = plan else {
            failure = Some(format!(
                "native union `{}` has no admitted provider plan",
                native.rust_name
            ));
            return;
        };
        let (mut bound, route) = match plan.public_native_union_projection(library, native) {
            Ok(projection) => projection,
            Err(error) => {
                failure = Some(error);
                return;
            }
        };
        let mut owner_path = vec![library.to_string()];
        for dependency in route {
            owner_path.push(crate::frontend::rust_type_display::PROVIDER_RUST_BRIDGE_MODULE.to_string());
            owner_path.push(dependency);
        }
        let rust_owner = format!("::{}", owner_path.join("::"));
        let mut members = match with_checked_native_unions(bound.members.clone(), library, Some(plan), routes) {
            Ok(members) => with_checked_type_routes(members, routes),
            Err(error) => {
                failure = Some(error);
                return;
            }
        };
        members.visit_type_refs(&mut |member| match member {
            TypeRef::Named { name, origin: None } if !name.starts_with("::") => {
                if matches!(
                    super::resolved_type_from_manifest_type_ref(&TypeRef::Named {
                        name: name.clone(),
                        origin: None
                    }),
                    crate::frontend::symbols::ResolvedType::Named(_)
                ) {
                    *name = format!("{rust_owner}::{name}");
                }
            }
            TypeRef::Applied { name, origin: None, .. }
                if !name.starts_with("::")
                    && incan_core::lang::types::collections::from_str(name).is_none()
                    && name != incan_core::lang::types::UNION_TYPE_NAME
                    && name != "decimal" =>
            {
                *name = format!("{rust_owner}::{name}");
            }
            _ => {}
        });
        bound.checked_projection = Some(Box::new(super::model::NativeUnionProjection {
            dependency_root: library.to_string(),
            rust_owner,
            members,
            nominal_origins: BTreeMap::new(),
        }));
        *native = bound;
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(value),
    }
}

/// Attach one source module's checked nominal bindings to its consumer-only native conversion projections.
pub(crate) fn with_native_nominal_origins<T: VisitTypeRefs>(
    mut value: T,
    origins: &BTreeMap<String, NominalTypeOriginExport>,
) -> T {
    /// Include nested physical carriers while preserving the ordinary semantic visitor's immutable wire walk.
    fn attach(ty: &mut TypeRef, origins: &BTreeMap<String, NominalTypeOriginExport>) {
        if let TypeRef::NativeUnion(native) = ty
            && let Some(projection) = &mut native.checked_projection
        {
            projection.nominal_origins = origins.clone();
            projection.members.visit_type_refs(&mut |ty| attach(ty, origins));
        }
    }
    value.visit_type_refs(&mut |ty| attach(ty, origins));
    value
}

#[cfg(test)]
mod tests {
    use super::{VisitTypeRefs, with_checked_type_origins};
    use crate::frontend::api_metadata::{
        DecoratorArgMetadata, DecoratorCallArgMetadata, DecoratorMetadata, DecoratorValue, SourceSpan,
    };
    use crate::library_manifest::TypeRef;

    /// Structured decorator values and nested call type arguments retain the same checked nominal origin.
    #[test]
    fn decorator_argument_types_preserve_checked_nominal_origins() {
        let origin = crate::library_manifest::NominalTypeOriginExport {
            provider: crate::provider::ProviderIdentity {
                name: "catalog".into(),
                version: "1.2.3".into(),
                digest: "a".repeat(64),
                feature_projection: Default::default(),
            },
            canonical: crate::library_manifest::CanonicalIdentityExport {
                namespace: crate::library_manifest::CanonicalIdentityNamespaceExport::OrdinaryLexical,
                origin: crate::library_manifest::CanonicalIdentityOriginExport::Package {
                    library: "catalog".into(),
                    module_path: vec!["lib".into()],
                },
                declaration_name: "Product".into(),
                kind: "model".into(),
                declaration_span: crate::library_manifest::CanonicalIdentitySpanExport { start: 0, end: 20 },
            },
        };
        let leaf = TypeRef::Named {
            name: "Product".into(),
            origin: None,
        };
        let metadata = DecoratorMetadata {
            path: vec!["marker".into()],
            source_name: "marker".into(),
            anchor: SourceSpan { start: 0, end: 1 },
            type_args: vec![],
            decorated_callable: None,
            args: vec![DecoratorArgMetadata::Named {
                name: "value".into(),
                value: DecoratorValue::List {
                    items: vec![
                        DecoratorValue::Type { ty: leaf.clone() },
                        DecoratorValue::Call {
                            callee: vec!["wrap".into()],
                            type_args: vec![leaf.clone()],
                            args: vec![DecoratorCallArgMetadata::PositionalUnpack {
                                value: DecoratorValue::Type { ty: leaf },
                            }],
                        },
                    ],
                },
            }],
        };
        let mut projected = with_checked_type_origins(
            metadata,
            &std::collections::BTreeMap::from([("Product".into(), origin.clone())]),
        );
        let mut seen = 0;
        projected.visit_type_refs(&mut |leaf| {
            if let TypeRef::Named { origin: actual, .. } = leaf {
                assert_eq!(actual.as_ref(), Some(&origin));
                seen += 1;
            }
        });
        assert_eq!(seen, 3);
    }
}
