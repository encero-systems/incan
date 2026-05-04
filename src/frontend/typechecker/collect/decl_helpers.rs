//! Collection helpers for the first pass (fields/methods + derive-driven injections).

use std::collections::HashMap;

use crate::frontend::ast::*;
use crate::frontend::symbols::{CallableParam, FieldInfo, MethodInfo, ResolvedType, TypeBoundInfo};
use crate::frontend::typechecker::TypeChecker;
use incan_core::lang::derives::{self, DeriveId};

/// Build the resolved surface type for a declaration owner while that owner is still being collected.
///
/// During first-pass collection the owner type symbol is not yet registered, so simple self-references like `->
/// Session` would otherwise fall back to `TypeVar("Session")`.
fn owner_resolved_type(owner_name: &str, owner_type_params: &[TypeParam]) -> ResolvedType {
    if owner_type_params.is_empty() {
        return ResolvedType::Named(owner_name.to_string());
    }
    ResolvedType::Generic(
        owner_name.to_string(),
        owner_type_params
            .iter()
            .map(|tp| ResolvedType::TypeVar(tp.name.clone()))
            .collect(),
    )
}

/// Replace unresolved simple self-references with the concrete owner type during declaration collection.
///
/// This is intentionally narrow: only `TypeVar(owner_name)` is rewritten. Other unknown names must keep flowing as
/// unresolved placeholders so forward references do not silently become owner-self references.
fn resolve_owner_self_reference(
    ty: ResolvedType,
    owner_name: Option<&str>,
    owner_self_ty: Option<&ResolvedType>,
) -> ResolvedType {
    let Some(owner_name) = owner_name else {
        return ty;
    };
    let Some(owner_self_ty) = owner_self_ty else {
        return ty;
    };

    match ty {
        ResolvedType::TypeVar(name) if name == owner_name => owner_self_ty.clone(),
        ResolvedType::Generic(name, args) => ResolvedType::Generic(
            name,
            args.into_iter()
                .map(|arg| resolve_owner_self_reference(arg, Some(owner_name), Some(owner_self_ty)))
                .collect(),
        ),
        ResolvedType::Function(params, ret) => ResolvedType::Function(
            params
                .into_iter()
                .map(|param| CallableParam {
                    name: param.name,
                    ty: resolve_owner_self_reference(param.ty, Some(owner_name), Some(owner_self_ty)),
                    kind: param.kind,
                    has_default: param.has_default,
                })
                .collect(),
            Box::new(resolve_owner_self_reference(
                *ret,
                Some(owner_name),
                Some(owner_self_ty),
            )),
        ),
        ResolvedType::Tuple(items) => ResolvedType::Tuple(
            items
                .into_iter()
                .map(|item| resolve_owner_self_reference(item, Some(owner_name), Some(owner_self_ty)))
                .collect(),
        ),
        ResolvedType::FrozenList(inner) => ResolvedType::FrozenList(Box::new(resolve_owner_self_reference(
            *inner,
            Some(owner_name),
            Some(owner_self_ty),
        ))),
        ResolvedType::FrozenSet(inner) => ResolvedType::FrozenSet(Box::new(resolve_owner_self_reference(
            *inner,
            Some(owner_name),
            Some(owner_self_ty),
        ))),
        ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
            Box::new(resolve_owner_self_reference(
                *key,
                Some(owner_name),
                Some(owner_self_ty),
            )),
            Box::new(resolve_owner_self_reference(
                *value,
                Some(owner_name),
                Some(owner_self_ty),
            )),
        ),
        ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(resolve_owner_self_reference(
            *inner,
            Some(owner_name),
            Some(owner_self_ty),
        ))),
        ResolvedType::RefMut(inner) => ResolvedType::RefMut(Box::new(resolve_owner_self_reference(
            *inner,
            Some(owner_name),
            Some(owner_self_ty),
        ))),
        other => other,
    }
}

/// Build method symbol metadata from one declared method while resolving owner `Self` references.
fn method_info_from_decl(
    method: &Spanned<MethodDecl>,
    checker: &mut TypeChecker,
    owner_name: Option<&str>,
    owner_self_ty: Option<&ResolvedType>,
) -> MethodInfo {
    let type_params: Vec<String> = method.node.type_params.iter().map(|tp| tp.name.clone()).collect();
    let type_param_bounds: HashMap<String, Vec<String>> = method
        .node
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                tp.bounds.iter().map(|bound| bound.name.clone()).collect(),
            )
        })
        .collect();
    let type_param_bound_details: HashMap<String, Vec<TypeBoundInfo>> = method
        .node
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                tp.bounds
                    .iter()
                    .map(|bound| TypeBoundInfo {
                        name: bound.name.clone(),
                        type_args: bound
                            .type_args
                            .iter()
                            .map(|type_arg| {
                                resolve_owner_self_reference(
                                    checker.resolve_type_checked(type_arg),
                                    owner_name,
                                    owner_self_ty,
                                )
                            })
                            .collect(),
                    })
                    .collect(),
            )
        })
        .collect();
    let params = method
        .node
        .params
        .iter()
        .map(|p| {
            CallableParam::named_with_default(
                p.node.name.clone(),
                resolve_owner_self_reference(checker.resolve_type_checked(&p.node.ty), owner_name, owner_self_ty),
                p.node.kind,
                p.node.default.is_some(),
            )
        })
        .collect();
    let return_type = resolve_owner_self_reference(
        checker.resolve_type_checked(&method.node.return_type),
        owner_name,
        owner_self_ty,
    );
    MethodInfo {
        type_params,
        type_param_bounds,
        type_param_bound_details,
        receiver: method.node.receiver,
        params,
        return_type,
        is_async: method.node.is_async(),
        has_body: method.node.body.is_some(),
        alias_of: None,
    }
}

/// Collect methods from method declarations into grouped overloads, preserving same-name declarations.
pub(super) fn collect_method_overloads(
    methods: &[Spanned<MethodDecl>],
    checker: &mut TypeChecker,
    owner_name: Option<&str>,
    owner_type_params: &[TypeParam],
) -> HashMap<String, Vec<MethodInfo>> {
    let owner_self_ty = owner_name.map(|name| owner_resolved_type(name, owner_type_params));
    let mut overloads: HashMap<String, Vec<MethodInfo>> = HashMap::new();
    for method in methods {
        overloads
            .entry(method.node.name.clone())
            .or_default()
            .push(method_info_from_decl(
                method,
                checker,
                owner_name,
                owner_self_ty.as_ref(),
            ));
    }
    overloads
}

/// Collapse grouped methods to the legacy single-method map, keeping the last declaration as before.
pub(super) fn collect_methods_from_overloads(
    overloads: &HashMap<String, Vec<MethodInfo>>,
) -> HashMap<String, MethodInfo> {
    overloads
        .iter()
        .filter_map(|(name, methods)| methods.last().cloned().map(|method| (name.clone(), method)))
        .collect()
}

/// Project same-type method aliases onto the target method metadata.
fn apply_method_aliases(
    aliases: &[Spanned<MethodAliasDecl>],
    methods: &mut HashMap<String, MethodInfo>,
    overloads: &mut HashMap<String, Vec<MethodInfo>>,
) -> HashMap<String, String> {
    let mut method_aliases = HashMap::new();
    for alias in aliases {
        let target = alias.node.target.clone();
        method_aliases.insert(alias.node.name.clone(), target.clone());

        if let Some(target_overloads) = overloads.get(&target).cloned() {
            let alias_overloads: Vec<_> = target_overloads
                .into_iter()
                .map(|mut info| {
                    info.alias_of = Some(target.clone());
                    info
                })
                .collect();
            if let Some(last) = alias_overloads.last().cloned() {
                methods.insert(alias.node.name.clone(), last);
            }
            overloads.insert(alias.node.name.clone(), alias_overloads);
        } else if let Some(mut info) = methods.get(&target).cloned() {
            info.alias_of = Some(target.clone());
            methods.insert(alias.node.name.clone(), info.clone());
            overloads.insert(alias.node.name.clone(), vec![info]);
        }
    }
    method_aliases
}

/// Collect same-type method alias metadata into method maps and alias maps.
pub(super) fn collect_method_aliases(
    aliases: &[Spanned<MethodAliasDecl>],
    methods: &mut HashMap<String, MethodInfo>,
    overloads: &mut HashMap<String, Vec<MethodInfo>>,
) -> HashMap<String, String> {
    apply_method_aliases(aliases, methods, overloads)
}

/// Insert a compiler-injected method into both the legacy method map and overload groups.
fn insert_injected_method(
    methods: &mut HashMap<String, MethodInfo>,
    overloads: &mut HashMap<String, Vec<MethodInfo>>,
    name: impl Into<String>,
    info: MethodInfo,
) {
    let name = name.into();
    methods.insert(name.clone(), info.clone());
    overloads.insert(name, vec![info]);
}

/// Collect fields from field declarations into a `HashMap`.
pub(super) fn collect_fields(
    fields: &[Spanned<FieldDecl>],
    checker: &mut TypeChecker,
    owner: &str,
) -> HashMap<String, FieldInfo> {
    fields
        .iter()
        .map(|f| {
            (
                f.node.name.clone(),
                FieldInfo {
                    ty: checker.resolve_type_checked(&f.node.ty),
                    visibility: f.node.visibility,
                    owner: Some(owner.to_string()),
                    has_default: f.node.default.is_some(),
                    alias: f.node.metadata.alias.clone(),
                    description: f.node.metadata.description.clone(),
                },
            )
        })
        .collect()
}

/// Inject to_json/from_json methods based on Serialize/Deserialize derives.
pub(super) fn inject_json_methods(
    methods: &mut HashMap<String, MethodInfo>,
    overloads: &mut HashMap<String, Vec<MethodInfo>>,
    type_name: &str,
    derives: &[String],
) {
    if derives
        .iter()
        .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Serialize))
    {
        insert_injected_method(
            methods,
            overloads,
            "to_json",
            MethodInfo {
                type_params: Vec::new(),
                type_param_bounds: HashMap::new(),
                type_param_bound_details: HashMap::new(),
                receiver: Some(Receiver::Immutable),
                params: vec![],
                return_type: ResolvedType::Str,
                is_async: false,
                has_body: true,
                alias_of: None,
            },
        );
    }
    if derives
        .iter()
        .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Deserialize))
    {
        insert_injected_method(
            methods,
            overloads,
            "from_json",
            MethodInfo {
                type_params: Vec::new(),
                type_param_bounds: HashMap::new(),
                type_param_bound_details: HashMap::new(),
                receiver: None, // Static method
                params: vec![CallableParam::named("json_str", ResolvedType::Str, ParamKind::Normal)],
                return_type: ResolvedType::Generic(
                    "Result".to_string(),
                    vec![ResolvedType::Named(type_name.to_string()), ResolvedType::Str],
                ),
                is_async: false,
                has_body: true,
                alias_of: None,
            },
        );
    }
}

/// Inject a `TypeName.new(...) -> Result[TypeName, E]` constructor for `@derive(Validate)` models.
///
/// This is a *typechecker-only* method injection to allow `User.new(...)` calls to typecheck even though the backend
/// generates the actual Rust implementation.
pub(super) fn inject_validate_methods(
    methods: &mut HashMap<String, MethodInfo>,
    overloads: &mut HashMap<String, Vec<MethodInfo>>,
    _type_name: &str,
    fields: &HashMap<String, FieldInfo>,
    field_order: &[Ident],
    derives: &[String],
) {
    let has_validate = derives
        .iter()
        .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate));
    if !has_validate {
        return;
    }

    // Only inject if the user didn't already define it.
    if methods.contains_key("new") {
        return;
    }

    // Use the return type of validate() if present; otherwise use Unknown (second pass will report a better error).
    let return_type = methods
        .get("validate")
        .map(|m| m.return_type.clone())
        .unwrap_or(ResolvedType::Unknown);

    // Prefer required fields only (no defaults). This keeps the signature stable and avoids needing default args.
    let mut params: Vec<CallableParam> = Vec::new();
    for field_name in field_order {
        if let Some(info) = fields.get(field_name)
            && !info.has_default
        {
            params.push(CallableParam::named(
                field_name.clone(),
                info.ty.clone(),
                ParamKind::Normal,
            ));
        }
    }

    insert_injected_method(
        methods,
        overloads,
        "new",
        MethodInfo {
            type_params: Vec::new(),
            type_param_bounds: HashMap::new(),
            type_param_bound_details: HashMap::new(),
            receiver: None, // associated function via `TypeName.new(...)`
            params,
            return_type,
            is_async: false,
            has_body: true,
            alias_of: None,
        },
    );
}
