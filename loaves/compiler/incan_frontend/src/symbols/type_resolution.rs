//! Resolution of source type annotations and rendering of checked types as Rust arguments.

use super::*;

/// Resolve `a::b::c` in type position when `a` is a `rust::` import binding (module or item).
pub(super) fn resolve_qualified_rust_type_path(segments: &[String], symbols: &SymbolTable) -> ResolvedType {
    if segments.len() < 2 {
        return ResolvedType::Unknown;
    }
    let Some(root) = segments.first() else {
        return ResolvedType::Unknown;
    };
    let Some(id) = symbols.lookup(root) else {
        return ResolvedType::Unknown;
    };
    let Some(sym) = symbols.get(id) else {
        return ResolvedType::Unknown;
    };
    let SymbolKind::RustItem(info) = &sym.kind else {
        return ResolvedType::Unknown;
    };
    let mut path = info.path.clone();
    for seg in segments.iter().skip(1) {
        path.push_str("::");
        path.push_str(seg);
    }
    ResolvedType::RustPath(path)
}

/// Resolve an AST type annotation into the canonical semantic type representation.
///
/// A module-qualified spelling (`mod.Type`) resolves through registries this table does not hold, so it stays
/// `Unknown` here; the typechecker supplies that resolution through [`resolve_type_with_rust_arg_renderer`].
pub fn resolve_type(ty: &Type, symbols: &SymbolTable) -> ResolvedType {
    resolve_type_with_rust_arg_renderer(ty, symbols, &render_resolved_type_as_rust_arg, &|_| {}, &|_| None)
}

/// Resolve an AST type while allowing the typechecker to preserve provider identity inside opaque Rust applications.
///
/// `resolve_qualified_type` answers a module-qualified spelling (`mod.Type`, or the constructor of `mod.Box[T]`) with
/// the nominal type the typechecker proved for it, or `None` when it proved nothing; the symbol table alone cannot
/// see into a module's members, so the resolver never derives a type from the written segments itself.
pub fn resolve_type_with_rust_arg_renderer<F, G, Q>(
    ty: &Type,
    symbols: &SymbolTable,
    render_rust_arg: &F,
    qualify_structured_rust_arg: &G,
    resolve_qualified_type: &Q,
) -> ResolvedType
where
    F: Fn(&ResolvedType) -> String,
    G: Fn(&mut ResolvedType),
    Q: Fn(&[String]) -> Option<ResolvedType>,
{
    let resolve = |ty: &Type| {
        resolve_type_with_rust_arg_renderer(
            ty,
            symbols,
            render_rust_arg,
            qualify_structured_rust_arg,
            resolve_qualified_type,
        )
    };
    match ty {
        Type::Qualified(segments) => resolve_qualified_rust_type_path(segments, symbols),
        Type::Dotted(segments) => resolve_qualified_type(segments).unwrap_or(ResolvedType::Unknown),
        Type::Simple(name) => resolve_simple_type(name, symbols),
        Type::ConstrainedPrimitive(name, _) => {
            let base = Type::Simple(name.clone());
            resolve(&base)
        }
        Type::Generic(name, args) => {
            let resolved_args = args.iter().map(|arg| resolve(&arg.node)).collect();
            resolve_generic_type(
                name,
                resolved_args,
                symbols,
                render_rust_arg,
                qualify_structured_rust_arg,
            )
        }
        Type::DottedGeneric(segments, args) => {
            let resolved_args = args.iter().map(|arg| resolve(&arg.node)).collect();
            // A proven constructor applies its declaration's name; an unproven one keeps the dotted spelling it
            // always had, so nothing downstream sees a new shape for a spelling the checker did not prove.
            match resolve_qualified_type(segments) {
                Some(ResolvedType::Named(name)) => ResolvedType::Generic(name, resolved_args),
                _ => ResolvedType::Generic(segments.join("."), resolved_args),
            }
        }
        Type::IntLiteral(value) => ResolvedType::TypeVar(value.repr.clone()),
        Type::Function(params, ret) => {
            // A `mut`-marked parameter keeps its type and carries the marker on the callable parameter (#1790).
            let resolved_params: Vec<_> = params
                .iter()
                .map(|param| {
                    let (param_ty, is_mut) = match &param.node {
                        Type::MutParam(inner) => (&inner.node, true),
                        other => (other, false),
                    };
                    CallableParam::positional(resolve(param_ty)).with_mut(is_mut)
                })
                .collect();
            let resolved_ret = resolve(&ret.node);
            ResolvedType::Function(resolved_params, Box::new(resolved_ret))
        }
        Type::Ref(inner) => ResolvedType::Ref(Box::new(resolve(&inner.node))),
        // The parser produces the marker only inside a callable type's parameters, handled above; anywhere else it
        // names its type.
        Type::MutParam(inner) => resolve(&inner.node),
        Type::RefMut(inner) => ResolvedType::RefMut(Box::new(resolve(&inner.node))),
        Type::Unit => ResolvedType::Unit,
        Type::Tuple(elems) => {
            let resolved_elems: Vec<_> = elems.iter().map(|element| resolve(&element.node)).collect();
            ResolvedType::Tuple(resolved_elems)
        }
        Type::SelfType => ResolvedType::SelfType,
        Type::Infer => ResolvedType::CallSiteInfer,
    }
}

/// Apply a simple generic constructor to already resolved arguments, preserving source facade and Rust identities.
///
/// Structured native applications retain nominal arguments needed by Body IR. Opaque native applications use the
/// caller's Rust argument renderer, while builtin collection and union constructors keep their canonical mapping.
pub(super) fn resolve_generic_type<F, G>(
    name: &str,
    mut resolved_args: Vec<ResolvedType>,
    symbols: &SymbolTable,
    render_rust_arg: &F,
    qualify_structured_rust_arg: &G,
) -> ResolvedType
where
    F: Fn(&ResolvedType) -> String,
    G: Fn(&mut ResolvedType),
{
    if let Some(id) = symbols.lookup(name)
        && let Some(symbol) = symbols.get(id)
        && let SymbolKind::RustItem(info) = &symbol.kind
        && !matches!(info.binding, RustImportBindingKind::CrateRoot)
    {
        if rust_generic_preserves_nominal_ir(info)
            || resolved_args.iter().any(|arg| source_nominal_argument(arg, symbols))
        {
            // Source facades keep their wrapped-field contract; other native containers keep their canonical path.
            let source_surface =
                surface_types::from_str(name).is_some_and(surface_types::field_access_reads_wrapped_value);
            if source_surface {
                return ResolvedType::Generic(name.to_owned(), resolved_args);
            }
            for arg in &mut resolved_args {
                qualify_structured_rust_arg(arg);
                if !rust_generic_preserves_nominal_ir(info) {
                    *arg = crate::resolved_type_subst::map_resolved_type_leaves(arg, &|leaf| {
                        let ResolvedType::Named(name) = leaf else {
                            return None;
                        };
                        symbols
                            .module_type_identity(name)
                            .and_then(incan_semantics_core::body_ir::nominal_carrier_key)
                            .map(ResolvedType::Named)
                    });
                }
            }
            let base = if rust_generic_preserves_nominal_ir(info) {
                name.to_owned()
            } else {
                info.path.clone()
            };
            return ResolvedType::Generic(base, resolved_args);
        }
        let rendered_args = resolved_args.iter().map(render_rust_arg).collect::<Vec<_>>().join(", ");
        return ResolvedType::RustPath(format!("{}<{rendered_args}>", info.path));
    }
    if name == "Type" {
        return ResolvedType::TypeToken(Box::new(
            resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown),
        ));
    }
    if let Some(id) = collections::from_str(name) {
        return ResolvedType::builtin_generic(id, resolved_args);
    }
    if name == UNION_TYPE_NAME {
        return union_ty(resolved_args);
    }
    ResolvedType::Generic(name.to_owned(), resolved_args)
}

/// Resolve builtin and native import names before ordinary source nominal or still-open parameter spellings.
///
/// A source nominal is a type or trait binding; any other binding of the spelling stays open (#2594).
pub(super) fn resolve_simple_type(name: &str, symbols: &SymbolTable) -> ResolvedType {
    if let Some(builtin) = ResolvedType::builtin_simple(name) {
        return builtin;
    }
    if let Some(id) = symbols.lookup(name)
        && let Some(symbol) = symbols.get(id)
        && let SymbolKind::RustItem(info) = &symbol.kind
    {
        return match info.binding {
            RustImportBindingKind::CrateRoot => ResolvedType::Unknown,
            RustImportBindingKind::RootedPath | RustImportBindingKind::FromImport => {
                ResolvedType::RustPath(info.path.clone())
            }
        };
    }
    // Only a type or trait binding names a type. A bare variant, function, constant, local, module or capability
    // holding the spelling leaves it open, so an annotation spelling one is refused like an undeclared name (#2594).
    // A variant never displaces a declaration of its spelling, so a same-named model still resolves here.
    if let Some(id) = symbols.lookup(name)
        && symbols
            .get(id)
            .is_none_or(|symbol| matches!(symbol.kind, SymbolKind::Type(_) | SymbolKind::Trait(_)))
    {
        return ResolvedType::Named(name.to_owned());
    }
    ResolvedType::TypeVar(name.to_owned())
}

/// Return whether one compiler-owned Rust generic keeps its structured semantic IR representation.
///
/// `Box[T]` participates in receiver/method semantics that consume `ResolvedType::Generic`. Other Rust applications,
/// without source-owned arguments retain their complete canonical display. Source-owned arguments instead stay
/// structural so their declaring identity crosses native generic boundaries.
pub(super) fn rust_generic_preserves_nominal_ir(info: &RustItemInfo) -> bool {
    matches!(
        info.path.as_str(),
        "std::boxed::Box" | "alloc::boxed::Box" | "rust::std::boxed::Box" | "rust::alloc::boxed::Box"
    )
}

/// Keep source-owned generic arguments structural while their declaring symbols still establish their identities.
pub(super) fn source_nominal_argument(ty: &ResolvedType, symbols: &SymbolTable) -> bool {
    match ty {
        ResolvedType::Named(name) => symbols
            .module_type_identity(name)
            .and_then(incan_semantics_core::body_ir::nominal_carrier_key)
            .is_some(),
        ResolvedType::Generic(name, arguments) => {
            symbols
                .module_type_identity(name)
                .and_then(incan_semantics_core::body_ir::nominal_carrier_key)
                .is_some()
                || arguments.iter().any(|ty| source_nominal_argument(ty, symbols))
        }
        ResolvedType::Tuple(elements) => elements.iter().any(|ty| source_nominal_argument(ty, symbols)),
        ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner)
        | ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner) => source_nominal_argument(inner, symbols),
        ResolvedType::FrozenDict(key, value) => {
            source_nominal_argument(key, symbols) || source_nominal_argument(value, symbols)
        }
        _ => false,
    }
}

/// Render one checked type as a Rust generic argument without retaining source-only collection spellings.
pub fn render_resolved_type_as_rust_arg(ty: &ResolvedType) -> String {
    match ty {
        ResolvedType::Never => "!".to_string(),
        ResolvedType::Int => "i64".to_string(),
        ResolvedType::Float => "f64".to_string(),
        ResolvedType::Numeric(id) => numerics::rust_name(*id).to_string(),
        ResolvedType::Bool => "bool".to_string(),
        ResolvedType::Str => "String".to_string(),
        ResolvedType::Bytes => "Vec<u8>".to_string(),
        ResolvedType::FrozenStr => "incan_std_core::frozen::FrozenStr".to_string(),
        ResolvedType::FrozenBytes => "incan_std_core::frozen::FrozenBytes".to_string(),
        ResolvedType::FrozenList(inner) => format!(
            "incan_std_core::frozen::FrozenList<{}>",
            render_resolved_type_as_rust_arg(inner)
        ),
        ResolvedType::FrozenDict(key, value) => format!(
            "incan_std_core::frozen::FrozenDict<{}, {}>",
            render_resolved_type_as_rust_arg(key),
            render_resolved_type_as_rust_arg(value)
        ),
        ResolvedType::FrozenSet(inner) => format!(
            "incan_std_core::frozen::FrozenSet<{}>",
            render_resolved_type_as_rust_arg(inner)
        ),
        ResolvedType::Unit => "()".to_string(),
        ResolvedType::Named(name) => name
            .strip_prefix("pub::")
            .map_or_else(|| name.clone(), ToString::to_string),
        ResolvedType::TypeVar(name) => name.clone(),
        ResolvedType::Generic(name, args) => {
            if collections::from_str(name.as_str()) == Some(CollectionTypeId::Tuple) {
                return render_rust_tuple(args);
            }
            if args.is_empty() {
                return name.clone();
            }
            if name == "[]"
                && let [element] = args.as_slice()
            {
                return format!("[{}]", render_resolved_type_as_rust_arg(element));
            }
            let base = match collections::from_str(name.as_str()) {
                Some(CollectionTypeId::List) => "Vec",
                Some(CollectionTypeId::Dict) => "std::collections::HashMap",
                Some(CollectionTypeId::Set) => "std::collections::HashSet",
                Some(CollectionTypeId::Option) => "Option",
                Some(CollectionTypeId::Result) => "Result",
                Some(CollectionTypeId::FrozenList) => "incan_std_core::frozen::FrozenList",
                Some(CollectionTypeId::FrozenDict) => "incan_std_core::frozen::FrozenDict",
                Some(CollectionTypeId::FrozenSet) => "incan_std_core::frozen::FrozenSet",
                Some(CollectionTypeId::Generator) => "incan_std_core::iter::Generator",
                Some(CollectionTypeId::Tuple) | None => name,
            };
            let rendered_args = args
                .iter()
                .map(render_resolved_type_as_rust_arg)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{base}<{rendered_args}>")
        }
        ResolvedType::Function(params, ret) => {
            let params = params
                .iter()
                .map(|param| render_resolved_type_as_rust_arg(&param.ty))
                .collect::<Vec<_>>()
                .join(", ");
            format!("fn({params}) -> {}", render_resolved_type_as_rust_arg(ret))
        }
        ResolvedType::TypeToken(inner) => {
            format!(
                "incan_std_core::reflection::TypeToken<{}>",
                render_resolved_type_as_rust_arg(inner)
            )
        }
        ResolvedType::Tuple(items) => render_rust_tuple(items),
        ResolvedType::SelfType => "Self".to_string(),
        ResolvedType::Ref(inner) => format!("&{}", render_resolved_type_as_rust_arg(inner)),
        ResolvedType::RefMut(inner) => format!("&mut {}", render_resolved_type_as_rust_arg(inner)),
        ResolvedType::RustPath(path) => path.clone(),
        ResolvedType::CallSiteInfer | ResolvedType::Unknown => "_".to_string(),
    }
}

/// Render one checked tuple using Rust's single-element trailing-comma rule.
pub(super) fn render_rust_tuple(items: &[ResolvedType]) -> String {
    let rendered = items.iter().map(render_resolved_type_as_rust_arg).collect::<Vec<_>>();
    match rendered.as_slice() {
        [only] => format!("({only},)"),
        _ => format!("({})", rendered.join(", ")),
    }
}
