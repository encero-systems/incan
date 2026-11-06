//! Predicates selecting the declarations Body IR lowers directly in place of the legacy emitter.

use super::*;

/// Admit serde derives only after checked derive metadata resolves all selections to canonical supported paths, Copy
/// only after checking accepted it for every field, and `@rust.derive(...)` only when checking classified every derive
/// it names as a Rust builtin retained among the model's derives. A field alias crosses the boundary in the retained
/// declaration, so a serde form can rename the field as legacy does.
pub fn is_direct_replacement_checked_model(model: &ast::ModelDecl, type_info: &TypeCheckInfo) -> bool {
    if model
        .decorators
        .iter()
        .all(|decorator| is_direct_replacement_model_derive(&decorator.node))
        && has_direct_replacement_model_shape(model, Some(type_info))
    {
        return true;
    }
    has_checked_native_derives(
        &model.decorators,
        type_info.declarations.body_ir_model_derives.get(&model.name),
        type_info.declarations.rust_native_derive_types.contains(&model.name),
        type_info,
    ) && has_direct_replacement_model_shape(model, Some(type_info))
}

/// Admit a source nominal whose derive decorators are completely described by its checked native selection.
///
/// Checking has already resolved module-qualified JSON protocol derives to their canonical serde paths, so the
/// selection, not the decorator spelling, decides admission. A declaration without a checked selection stays refused.
/// `rust_derives` also admits `@rust.derive(...)` when checking recorded that its complete native selection is
/// retained, including supported serde derives.
pub(super) fn has_checked_native_derives(
    decorators: &[ast::Spanned<ast::Decorator>],
    selection: Option<&Vec<String>>,
    rust_derives: bool,
    type_info: &TypeCheckInfo,
) -> bool {
    let Some(names) = selection else {
        return false;
    };
    decorators.iter().all(|decorator| {
        let derive = &decorator.node;
        derive.name == "derive"
            && derive.type_args.is_empty()
            && (derive.path.segments == ["derive"] || (rust_derives && derive.path.segments == ["rust", "derive"]))
    }) && names.iter().all(|name| {
        type_info.derivations.native_macro_paths.contains(name)
            || matches!(
                derives::from_str(name),
                Some(
                    DeriveId::Debug
                        | DeriveId::Clone
                        | DeriveId::Copy
                        | DeriveId::Eq
                        | DeriveId::PartialEq
                        | DeriveId::Hash
                        | DeriveId::Ord
                        | DeriveId::PartialOrd
                        | DeriveId::Default
                        | DeriveId::Display
                )
            )
            || [
                derives::FIELD_INFO_DERIVE_NAME,
                derives::INCAN_CLASS_DERIVE_NAME,
                "serde::Serialize",
                "serde::Deserialize",
            ]
            .contains(&name.as_str())
    })
}

/// Admit builtin static and class method markers.
///
/// Checking already reflects both markers in the method's facts: a static method has no receiver, and a
/// classmethod's `cls` is a checked type token whose constructions resolve to the owner. Every other method decorator
/// rebinds or wraps the method and stays refused.
pub fn is_direct_replacement_method_decorator(decorator: &ast::Decorator) -> bool {
    decorator.args.is_empty()
        && decorator.type_args.is_empty()
        && matches!(
            incan_lang::lang::decorators::from_segments(&decorator.path.segments),
            Some(
                incan_lang::lang::decorators::DecoratorId::StaticMethod
                    | incan_lang::lang::decorators::DecoratorId::ClassMethod
            )
        )
}

/// Determine whether a nominal declaration uses at most one of type parameters, a generic trait adoption and method
/// aliases.
///
/// Each has parity evidence alone; any two together stay refused until that combination does.
pub(super) fn has_at_most_one_open_shape_feature(type_parameters: bool, generic_adoption: bool, aliases: bool) -> bool {
    [type_parameters, generic_adoption, aliases]
        .iter()
        .filter(|present| **present)
        .count()
        <= 1
}

/// Determine whether every decorator on one method is a builtin marker [`is_direct_replacement_method_decorator`]
/// admits.
pub(super) fn has_direct_replacement_method_decorators(method: &ast::MethodDecl) -> bool {
    method
        .decorators
        .iter()
        .all(|decorator| is_direct_replacement_method_decorator(&decorator.node))
}

/// Resolve the method a same-owner method partial targets: the method its target names, or the one a method alias of
/// that name rebinds. `None` unless exactly one method of the owner answers.
pub(crate) fn method_partial_target<'a>(
    methods: &'a [ast::Spanned<ast::MethodDecl>],
    aliases: &[ast::Spanned<ast::MethodAliasDecl>],
    partial: &ast::MethodPartialDecl,
) -> Option<&'a ast::Spanned<ast::MethodDecl>> {
    let target = aliases
        .iter()
        .find(|alias| alias.node.name == partial.target)
        .map_or(partial.target.as_str(), |alias| alias.node.target.as_str());
    let mut candidates = methods.iter().filter(|method| method.node.name == target);
    match (candidates.next(), candidates.next()) {
        (Some(method), None) => Some(method),
        _ => None,
    }
}

/// Determine whether every method partial of a nominal owner has a retained default frame: each partial targets,
/// directly or through a method alias, exactly one non-generic receiver method whose parameters its presets name.
/// Owner parameters close with the owner's retained layout and preset frame at each checked instantiation.
///
/// A call of such a partial is a call of its target method whose omitted presets are defaulted slots, so the frame
/// carrying the presets as defaults is the whole executable fact (see [`bir::MethodPartialDefaults`]).
pub fn has_direct_replacement_method_partials(
    _owner_type_params: &[ast::TypeParam],
    methods: &[ast::Spanned<ast::MethodDecl>],
    aliases: &[ast::Spanned<ast::MethodAliasDecl>],
    partials: &[ast::Spanned<ast::MethodPartialDecl>],
) -> bool {
    partials.iter().all(|partial| {
        method_partial_target(methods, aliases, &partial.node).is_some_and(|method| {
            method.node.receiver.is_some()
                && method.node.type_params.is_empty()
                && method.node.body.is_some()
                && partial.node.args.iter().all(|preset| {
                    method
                        .node
                        .params
                        .iter()
                        .any(|parameter| parameter.node.name == preset.name)
                })
        })
    })
}

/// Admit source-local trait partials whose non-generic receiver targets have bodies and whose presets are literals.
/// A literal carries no lexical dependency or observable evaluation effect; other forms need declaration-owned frames.
pub fn has_direct_replacement_literal_trait_partials(source: &ast::TraitDecl) -> bool {
    has_direct_replacement_method_partials(
        &source.type_params,
        &source.methods,
        &source.method_aliases,
        &source.method_partials,
    ) && source.method_partials.iter().all(|partial| {
        partial
            .node
            .args
            .iter()
            .all(|preset| matches!(preset.value.node, ast::Expr::Literal(_)))
    })
}

/// Determine whether every computed property has a checked getter body; abstract properties remain refused.
/// Owner parameters stay in the immutable receiver frame, and property reads call the getter without adding layout
/// fields.
pub fn has_direct_replacement_properties(
    _owner_type_params: &[ast::TypeParam],
    properties: &[ast::Spanned<ast::PropertyDecl>],
) -> bool {
    properties.iter().all(|property| property.node.body.is_some())
}

/// Keep the structural admission boundary shared by builtin and checked serde derive selections.
/// Same-type method aliases keep the checked target identity and use its retained method body; they add no native
/// layout or wrapper declaration. Method partials retain deferred preset frames
/// ([`has_direct_replacement_method_partials`]) and computed properties getter bodies
/// ([`has_direct_replacement_properties`]). Without checked facts, methods may carry only builtin static and class
/// method markers. Checked nongeneric models also admit proven method replacement chains, retaining their original
/// callables and initialized bindings. Checked method descriptions with complete literal materialization retain their
/// once-only registrations inside a sole source-local registry static's initializer; every other registration profile
/// remains refused.
pub(super) fn has_direct_replacement_model_shape(model: &ast::ModelDecl, type_info: Option<&TypeCheckInfo>) -> bool {
    has_direct_replacement_method_partials(
        &model.type_params,
        &model.methods,
        &model.method_aliases,
        &model.method_partials,
    ) && has_direct_replacement_properties(&model.type_params, &model.properties)
        && model.methods.iter().all(|method| {
            has_direct_replacement_method_decorators(&method.node)
                || model.type_params.is_empty()
                    && type_info.is_some_and(|info| {
                        decorated_bindings::supports_method(&method.node, &model.name, info)
                            || registry_descriptions::supports_method(&method.node, &model.name, info)
                    })
        })
}

/// Admit only the bare builtin derive decorator whose complete native trait selection Body IR retains.
/// Qualified, bundled, foreign, and semantic derives need their own checked expansion facts and remain refused. Display
/// retains its checked selection and uses the implied Debug rendering.
pub fn is_direct_replacement_model_derive(decorator: &ast::Decorator) -> bool {
    decorator.name == "derive"
        && decorator.path.segments == ["derive"]
        && decorator.type_args.is_empty()
        && decorator.args.iter().all(|argument| {
            matches!(argument, ast::DecoratorArg::Positional(value) if matches!(&value.node,
                ast::Expr::Ident(name) if matches!(derives::from_str(name), Some(DeriveId::Debug | DeriveId::Clone | DeriveId::Eq | DeriveId::PartialEq | DeriveId::Hash | DeriveId::Ord | DeriveId::PartialOrd | DeriveId::Default | DeriveId::Display))))
        })
}

/// Admit a transparent rusttype only when checking retained its underlying storage and it authors no executable
/// members or trait implementations. A trait adoption the checker proved the backing type already implements
/// (`rusttype_forwarded_trait_adoptions`) is satisfied by that type and authors nothing; any other adoption
/// disqualifies the alias. Native declaration stubs and infallible edges require exact checked native signatures for
/// every edge. It introduces no wrapper layout or constructor.
pub fn is_direct_native_rusttype_alias(newtype: &ast::NewtypeDecl, type_info: &TypeCheckInfo) -> bool {
    let alias = if newtype.type_params.is_empty() {
        IncanType::Named(newtype.name.clone())
    } else {
        IncanType::Generic {
            base: newtype.name.clone(),
            args: newtype
                .type_params
                .iter()
                .map(|parameter| IncanType::TypeVar(parameter.name.clone()))
                .collect(),
        }
    };
    newtype.is_rusttype
        && newtype.decorators.is_empty()
        && newtype.traits.iter().all(|adoption| {
            type_info
                .rust
                .rusttype_forwarded_trait_adoptions
                .contains(&(newtype.name.clone(), adoption.node.name.clone()))
        })
        && newtype.methods.iter().all(|method| {
            method
                .node
                .body
                .as_ref()
                .is_none_or(|body| crate::typechecker::is_trivial_body(body))
                && newtype.interop_edges.iter().any(
                    |edge| matches!(&edge.node.adapter.node, ast::Expr::Field(_, name) if name == &method.node.name),
                )
        })
        && newtype.rebindings.is_empty()
        && newtype.method_aliases.is_empty()
        && newtype.method_partials.is_empty()
        && newtype.associated_types.is_empty()
        && newtype
            .interop_edges
            .iter()
            .all(|edge| type_info.rust.validated_rusttype_edges.contains(&edge.span))
        && type_info.rust.storage_types.get(&alias).is_some_and(|storage| {
            !matches!(
                storage,
                IncanType::Unknown | IncanType::Unsupported(_) | IncanType::RustInteropPath(_)
            )
        })
}

/// Admit raw tuple wrappers with checked native derives, owner and method parameters, and trait adoptions.
///
/// A trait adopted with type arguments is instantiated from the checker-recorded arguments. Integer primitive
/// constraints retain their checked predicates at construction sites; source-local nongeneric hooks retain canonical
/// calls followed by checked-result extraction. Selected owned-string serde hooks retain direct-only reconstruction
/// facts, including an unbounded identity binder closed at exact checked uses; other validation profiles remain
/// refused. Same-type method aliases, and short rebindings naming one
/// of the newtype's own methods, keep the checked target identity, as aliases on models do, unless combined with a
/// generic trait adoption or owner parameters; lint suppressions change nothing at run time. Associated type targets
/// are resolved by checking into the retained method frames and trait implementation identities. Interop edges and
/// method partials require additional representation facts and remain outside this profile. Bare and module-qualified
/// JSON protocol derives use the checker's canonical serde selections, as on models.
pub fn is_direct_replacement_checked_newtype(newtype: &ast::NewtypeDecl, type_info: &TypeCheckInfo) -> bool {
    let selections = type_info.declarations.body_ir_newtype_derives.get(&newtype.name);
    let Some(facts) = type_info.declarations.newtype_construction.get(&newtype.name) else {
        return false;
    };
    if (!facts.constraints.is_empty() || facts.checked_constructor.is_some())
        && selections
            .into_iter()
            .flatten()
            .chain(facts.automatic_derives.iter())
            .any(|selection| selection == "serde::Deserialize")
        && checked_json::checked_deserialize_selection(newtype, type_info).is_none()
    {
        return false;
    }
    !newtype.is_rusttype
        && has_checked_native_derives(
            &newtype.decorators,
            selections,
            type_info.declarations.rust_native_derive_types.contains(&newtype.name),
            type_info,
        )
        && newtype.rebindings.iter().all(|rebinding| {
            matches!(&rebinding.node.target.node, ast::Expr::Ident(target)
                if newtype.methods.iter().any(|method| method.node.name == *target))
        })
        && has_at_most_one_open_shape_feature(
            !newtype.type_params.is_empty(),
            newtype
                .traits
                .iter()
                .any(|adoption| !adoption.node.type_args.is_empty()),
            !newtype.method_aliases.is_empty() || !newtype.rebindings.is_empty(),
        )
        && newtype.method_partials.is_empty()
        && newtype.interop_edges.is_empty()
        && newtype
            .methods
            .iter()
            .all(|method| has_direct_replacement_method_decorators(&method.node))
}

/// Admit source classes whose fields and method bodies have complete direct-route facts.
///
/// Ordered owner parameters and method parameters are retained for checked instance scheduling, and trait
/// adoptions, including ones with checked type arguments, are retained in the implementation registry. Type
/// parameters, generic trait adoptions and same-type method aliases keep their independent checked identities and
/// can occur together. Derives use the checked selection shared with models, field defaults the declaration-owned
/// deferred computations models use, and
/// builtin `@staticmethod` and `@classmethod` markers the checker's method facts, and method partials deferred preset
/// frames and computed properties getter bodies, as on models; lint suppressions change nothing at run time.
/// Local concrete instance inheritance uses checked parent, field and nearest-dispatch facts; other inheritance stays
/// refused. Checked nongeneric method replacement chains retain originals and initialized bindings.
pub fn is_direct_replacement_checked_class(class: &ast::ClassDecl, type_info: &TypeCheckInfo) -> bool {
    has_checked_native_derives(
        &class.decorators,
        type_info.declarations.class_derives.get(&class.name),
        type_info.declarations.rust_native_derive_types.contains(&class.name),
        type_info,
    ) && (class.extends.is_none()
        || type_info
            .declarations
            .body_ir_class_inheritance
            .contains_key(&class.name))
        && has_direct_replacement_method_partials(
            &class.type_params,
            &class.methods,
            &class.method_aliases,
            &class.method_partials,
        )
        && has_direct_replacement_properties(&class.type_params, &class.properties)
        && class.declarative_members.is_empty()
        && class.fields.iter().all(|field| field.node.metadata.alias.is_none())
        && class.methods.iter().all(|method| {
            has_direct_replacement_method_decorators(&method.node)
                || class.type_params.is_empty()
                    && (decorated_bindings::supports_method(&method.node, &class.name, type_info)
                        || registry_descriptions::supports_method(&method.node, &class.name, type_info))
        })
}

/// Determine whether an enum carries the narrow source-local fieldless normal-enum declaration fact.
///
/// This excludes every declaration form whose behavior needs additional semantic representation: scalar value enums,
/// payload construction, aliases, trait dispatch, custom methods, decorators, and generic substitution. The direct
/// runtime can therefore materialize only a canonical unit carrier and compare its retained identity.
pub fn is_direct_replacement_fieldless_enum(enum_decl: &ast::EnumDecl) -> bool {
    enum_decl.decorators.is_empty()
        && enum_decl.type_params.is_empty()
        && enum_decl.value_type.is_none()
        && enum_decl.traits.is_empty()
        && enum_decl.variant_aliases.is_empty()
        && enum_decl.methods.is_empty()
        && enum_decl
            .variants
            .iter()
            .all(|variant| variant.node.fields.is_empty() && variant.node.value.is_none())
}

/// Admit checked value-enum layouts with supported ordinary/static bodies, and normal enum layouts with undecorated
/// methods and trait adoptions but without generic substitution in methods. An enum with type parameters is admitted
/// only without user methods or trait adoptions; its ordered binders and payload placeholders close through checked
/// layout instantiation. Variant aliases keep the selected canonical variant identity rather than adding a second
/// payload slot.
///
/// A normal enum's methods are ordinary bodies over its retained layout, and the trait slots its adopted local traits
/// fill are retained in the module implementation registry, as for a model. A value enum also retains its separate
/// canonical raw-value registry; its unit layout alone never authorizes scalar extraction. Lint suppressions change
/// nothing at run time.
pub fn is_direct_native_enum(value: &ast::EnumDecl) -> bool {
    is_direct_replacement_value_enum(value)
        || ((value.type_params.is_empty() || (value.methods.is_empty() && value.traits.is_empty()))
            && value.value_type.is_none()
            && value.methods.iter().all(|method| method.node.decorators.is_empty())
            && value.decorators.iter().all(|decorator| is_bare_derive(&decorator.node)))
}

/// Whether checking selected only derives the native route realizes for a source enum: Rust's builtin derives and
/// serde's, which a value enum writes through its raw values as legacy does and any other enum derives.
/// A normal enum's checked Display selection uses Incan's borrowed Debug formatting path, never a Rust derive.
///
/// The enum predicates admit any bare `@derive(...)`; this checked selection decides which of those reach a plan.
pub fn has_direct_native_enum_derives(value: &ast::EnumDecl, type_info: &TypeCheckInfo) -> bool {
    type_info
        .declarations
        .enum_derives
        .get(&value.name)
        .is_some_and(|names| {
            names.iter().all(|name| {
                (derives::from_str(name) == Some(DeriveId::Display) && value.value_type.is_none())
                    || type_info.derivations.native_macro_paths.contains(name)
                    || matches!(
                        derives::from_str(name),
                        Some(
                            DeriveId::Debug
                                | DeriveId::Clone
                                | DeriveId::Copy
                                | DeriveId::PartialEq
                                | DeriveId::Eq
                                | DeriveId::PartialOrd
                                | DeriveId::Ord
                                | DeriveId::Hash
                        )
                    )
                    || matches!(name.as_str(), "serde::Serialize" | "serde::Deserialize")
            })
        })
}

/// The plain `@derive(...)` decorator, not a qualified or Rust derive spelled with the same final name.
pub(super) fn is_bare_derive(decorator: &ast::Decorator) -> bool {
    decorator.name == "derive" && decorator.path.segments == ["derive"] && decorator.type_args.is_empty()
}

/// Determine whether an enum carries the narrow source-local RFC 032 scalar declaration fact.
///
/// The checked native enum layout and its derives accompany this scalar registry. Immutable ordinary and static
/// methods contribute independent checked bodies and do not change the variants or raw values. Variant aliases retain
/// their checker's physical variant identity. Trait dispatch, generic substitution, payloads, and other method shapes
/// remain excluded rather than inferred from a layout.
pub fn is_direct_replacement_value_enum(enum_decl: &ast::EnumDecl) -> bool {
    enum_decl
        .decorators
        .iter()
        .all(|decorator| is_bare_derive(&decorator.node))
        && enum_decl.type_params.is_empty()
        && enum_decl.value_type.is_some()
        && enum_decl.traits.is_empty()
        && enum_decl
            .methods
            .iter()
            .all(|method| has_direct_value_enum_method_shape(&method.node))
        && enum_decl.variants.iter().all(|variant| {
            variant.node.fields.is_empty()
                && matches!(
                    variant.node.value.as_ref().map(|value| &value.node),
                    Some(ast::ValueEnumLiteral::Int(_) | ast::ValueEnumLiteral::Str(_))
                )
        })
}

/// Admit only concrete immutable or static enum bodies whose receiver contract already crosses the ordinary method
/// boundary.
///
/// Class receivers, mutable receivers, async bodies, and method binders need separate proof; their presence must not
/// silently broaden the scalar registry's source admission. Decorators are checked by the shared builtin registry.
pub(super) fn has_direct_value_enum_method_shape(method: &ast::MethodDecl) -> bool {
    method.body.is_some()
        && method.type_params.is_empty()
        && !method.is_async()
        && method.receiver != Some(ast::Receiver::Mutable)
        && has_direct_replacement_method_decorators(method)
        && !method.decorators.iter().any(|decorator| {
            incan_lang::lang::decorators::from_segments(&decorator.node.path.segments)
                == Some(incan_lang::lang::decorators::DecoratorId::ClassMethod)
        })
}
