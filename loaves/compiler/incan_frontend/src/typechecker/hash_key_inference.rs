//! Which type parameters a generic function or method hashes, read from its declaration when it is collected (#1758).
//!
//! The generated program bounds a type parameter by `Eq` and `Hash` when the callable's body stores its values in a
//! hashed collection: a set literal element or dict literal key of that type, a dict comprehension key of that type, a
//! `set(...)` call over a collection of it, or a call that passes it on to a callable that hashes it. A call that binds
//! such a parameter to a type without `Eq` and `Hash` then fails the build. The requirement is part of the callable's
//! signature, so it is inferred at collection: for this module's declarations before any body is checked, and for a
//! source module's public declarations when they are imported, so calls through an import see it too. A compiled
//! library carries it in its exported bounds ([`TypeChecker::export_inferred_hash_key_bounds`]).
//!
//! The inference types the body only as far as its declaration says: parameters, `for` targets and plain assignments
//! over them, and comprehension targets. A use it cannot type is not a requirement, so it never refuses a call the
//! build would accept.

use std::collections::{BTreeSet, HashMap};

use super::TypeChecker;
use crate::ast::{
    CallArg, Declaration, DictEntry, Expr, ListEntry, MethodDecl, ParamKind, Pattern, Spanned, Statement, TypeParam,
};
use crate::ast_walk::any_expr_in_body;
use crate::symbols::{ResolvedType, SymbolKind, TypeBoundInfo, TypeInfo, resolve_type};
use crate::typechecker::helpers::collection_type_id;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_lang::lang::types::collections::CollectionTypeId;

/// A generic callable of one module, named the way the module's own calls reach it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in crate::typechecker) enum HashKeyCallable {
    /// A module-level function.
    Function(String),
    /// A method, by its owner type and name.
    Method {
        /// The declaring type.
        owner: String,
        /// The method name.
        method: String,
    },
}

/// One generic callable's declaration, prepared for the inference.
struct ScanTarget<'a> {
    callable: HashKeyCallable,
    type_params: Vec<String>,
    params: Vec<(String, ParamKind, ResolvedType)>,
    body: &'a [Spanned<Statement>],
}

impl TypeChecker {
    /// Infer the hashed type parameters of every generic function and method in `declarations` and record them by
    /// declaration identity in `hash_key_type_params`.
    ///
    /// `local` marks the module being checked: its requirements are also kept by name so they can be written into the
    /// exported signatures once checking is done. Runs to a fixed point, because a callable that passes its parameter
    /// to another one hashes it too.
    pub(in crate::typechecker) fn infer_hash_key_type_params(
        &mut self,
        declarations: &[Spanned<Declaration>],
        local: bool,
    ) {
        let targets = self.hash_key_scan_targets(declarations);
        if targets.is_empty() {
            return;
        }
        let mut requirements: HashMap<HashKeyCallable, BTreeSet<String>> = HashMap::new();
        loop {
            let mut changed = false;
            for target in &targets {
                for type_param in self.hashed_type_params(target, &targets, &requirements) {
                    changed |= requirements
                        .entry(target.callable.clone())
                        .or_default()
                        .insert(type_param);
                }
            }
            if !changed {
                break;
            }
        }
        let mut ordered = requirements.into_iter().collect::<Vec<_>>();
        ordered.sort();
        for (callable, type_params) in ordered {
            if let Some(identity) = self.hash_key_callable_identity(&callable) {
                self.hash_key_type_params
                    .entry(identity)
                    .or_default()
                    .extend(type_params.iter().cloned());
            }
            if local {
                self.local_hash_key_requirements.push((callable, type_params));
            }
        }
    }

    /// Collect the generic functions and methods of `declarations` that have a body.
    fn hash_key_scan_targets<'a>(&self, declarations: &'a [Spanned<Declaration>]) -> Vec<ScanTarget<'a>> {
        let mut targets = Vec::new();
        for declaration in declarations {
            match &declaration.node {
                Declaration::Function(function) if !function.type_params.is_empty() => {
                    targets.push(ScanTarget {
                        callable: HashKeyCallable::Function(function.name.clone()),
                        type_params: self.hash_key_candidate_type_params(&function.type_params),
                        params: self.hash_key_scan_params(&function.params),
                        body: &function.body,
                    });
                }
                Declaration::Model(model) => self.push_method_targets(&model.name, &model.methods, &mut targets),
                Declaration::Class(class) => self.push_method_targets(&class.name, &class.methods, &mut targets),
                Declaration::Enum(en) => self.push_method_targets(&en.name, &en.methods, &mut targets),
                Declaration::Newtype(newtype) => {
                    self.push_method_targets(&newtype.name, &newtype.methods, &mut targets);
                }
                _ => {}
            }
        }
        targets
    }

    /// Add the generic methods with a body of one type to the inference targets.
    fn push_method_targets<'a>(
        &self,
        owner: &str,
        methods: &'a [Spanned<MethodDecl>],
        targets: &mut Vec<ScanTarget<'a>>,
    ) {
        for method in methods {
            let (false, Some(body)) = (method.node.type_params.is_empty(), method.node.body.as_ref()) else {
                continue;
            };
            targets.push(ScanTarget {
                callable: HashKeyCallable::Method {
                    owner: owner.to_string(),
                    method: method.node.name.clone(),
                },
                type_params: self.hash_key_candidate_type_params(&method.node.type_params),
                params: self.hash_key_scan_params(&method.node.params),
                body,
            });
        }
    }

    /// Return the type parameters whose hashing is inferred: those not already bounded by `Eq` and `Hash`.
    ///
    /// A declared bound is checked at every call by the ordinary bound check, so inferring it again would refuse the
    /// same call twice.
    fn hash_key_candidate_type_params(&self, type_params: &[TypeParam]) -> Vec<String> {
        type_params
            .iter()
            .filter(|param| {
                let declared = param
                    .bounds
                    .iter()
                    .filter_map(|bound| self.builtin_derive_bound(&bound.name))
                    .flat_map(|derive| std::iter::once(derive).chain(derives::implied_derives(derive).iter().copied()))
                    .collect::<Vec<_>>();
                !(declared.contains(&DeriveId::Eq) && declared.contains(&DeriveId::Hash))
            })
            .map(|param| param.name.clone())
            .collect()
    }

    /// Resolve declared parameter types without reporting anything: the declaration is checked on its own later.
    fn hash_key_scan_params(&self, params: &[Spanned<crate::ast::Param>]) -> Vec<(String, ParamKind, ResolvedType)> {
        params
            .iter()
            .map(|param| {
                (
                    param.node.name.clone(),
                    param.node.kind,
                    resolve_type(&param.node.ty.node, &self.symbols),
                )
            })
            .collect()
    }

    /// Return the declaration identity a module's own calls resolve a scanned callable to.
    fn hash_key_callable_identity(
        &self,
        callable: &HashKeyCallable,
    ) -> Option<incan_semantics_core::CanonicalSymbolId> {
        match callable {
            HashKeyCallable::Function(name) => {
                let symbol_id = self.symbols.lookup(name)?;
                match &self.symbols.get(symbol_id)?.kind {
                    SymbolKind::Function(_) => self.symbols.identity_of(symbol_id).cloned(),
                    _ => None,
                }
            }
            HashKeyCallable::Method { owner, method } => {
                let methods = match self.lookup_type_info(owner)? {
                    TypeInfo::Model(info) => &info.methods,
                    TypeInfo::Class(info) => &info.methods,
                    TypeInfo::Enum(info) => &info.methods,
                    TypeInfo::Newtype(info) => &info.methods,
                    TypeInfo::Builtin | TypeInfo::TypeAlias => return None,
                };
                methods.get(method)?.identity.clone()
            }
        }
    }

    /// Return the type parameters of `target` its body hashes, given what is known of the other callables so far.
    fn hashed_type_params(
        &self,
        target: &ScanTarget<'_>,
        targets: &[ScanTarget<'_>],
        requirements: &HashMap<HashKeyCallable, BTreeSet<String>>,
    ) -> Vec<String> {
        let mut env = target
            .params
            .iter()
            .map(|(name, _, ty)| (name.clone(), ty.clone()))
            .collect::<HashMap<_, _>>();
        Self::bind_statement_locals(target.body, &mut env);
        let mut hashed = Vec::new();
        let owner = match &target.callable {
            HashKeyCallable::Method { owner, .. } => Some(owner.as_str()),
            HashKeyCallable::Function(_) => None,
        };
        any_expr_in_body(target.body, |expr| {
            match expr {
                Expr::Call(callee, _, args) => {
                    if let (Expr::Ident(name), [CallArg::Positional(source)]) = (&callee.node, args.as_slice())
                        && collection_type_id(name) == Some(CollectionTypeId::Set)
                        && let Some(item) = Self::scan_type(source, &env).as_ref().and_then(Self::scan_item_type)
                    {
                        Self::collect_named_type_params(&item, &target.type_params, &mut hashed);
                    }
                    if let Expr::Ident(name) = &callee.node {
                        self.passed_hashed_type_params(
                            &HashKeyCallable::Function(name.clone()),
                            args,
                            &env,
                            target,
                            targets,
                            requirements,
                            &mut hashed,
                        );
                    }
                }
                Expr::MethodCall(receiver, method, _, args) => {
                    if let (Expr::SelfExpr, Some(owner)) = (&receiver.node, owner) {
                        let callable = HashKeyCallable::Method {
                            owner: owner.to_string(),
                            method: method.clone(),
                        };
                        self.passed_hashed_type_params(
                            &callable,
                            args,
                            &env,
                            target,
                            targets,
                            requirements,
                            &mut hashed,
                        );
                    }
                }
                Expr::Set(elements) => {
                    for element in elements {
                        Self::collect_direct_type_param(
                            Self::scan_type(element, &env),
                            &target.type_params,
                            &mut hashed,
                        );
                    }
                }
                Expr::Dict(entries) => {
                    for entry in entries {
                        if let DictEntry::Pair(key, _) = entry {
                            Self::collect_direct_type_param(
                                Self::scan_type(key, &env),
                                &target.type_params,
                                &mut hashed,
                            );
                        }
                    }
                }
                Expr::DictComp(comp) => {
                    let mut comp_env = env.clone();
                    if let (Pattern::Binding(name), Some(item)) = (
                        &comp.pattern.node,
                        Self::scan_type(&comp.iter, &env)
                            .as_ref()
                            .and_then(Self::scan_item_type),
                    ) {
                        comp_env.insert(name.clone(), item);
                    }
                    Self::collect_direct_type_param(
                        Self::scan_type(&comp.key, &comp_env),
                        &target.type_params,
                        &mut hashed,
                    );
                }
                _ => {}
            }
            false
        });
        hashed
    }

    /// Add the type parameters `target` passes to a callable at the positions that callable hashes.
    ///
    /// A callable of the same module is found by name among the scan targets; one declared elsewhere is found through
    /// its identity in the requirements recorded when its module was collected.
    #[allow(clippy::too_many_arguments)]
    fn passed_hashed_type_params(
        &self,
        callee: &HashKeyCallable,
        args: &[CallArg],
        env: &HashMap<String, ResolvedType>,
        target: &ScanTarget<'_>,
        targets: &[ScanTarget<'_>],
        requirements: &HashMap<HashKeyCallable, BTreeSet<String>>,
        hashed: &mut Vec<String>,
    ) {
        let (required, params) = if let Some(scanned) = targets.iter().find(|candidate| &candidate.callable == callee) {
            let Some(required) = requirements.get(callee) else {
                return;
            };
            let params = scanned
                .params
                .iter()
                .map(|(name, kind, ty)| (name.clone(), *kind, ty.clone()))
                .collect::<Vec<_>>();
            (required.clone(), params)
        } else {
            let HashKeyCallable::Function(name) = callee else {
                return;
            };
            let Some(symbol_id) = self.symbols.lookup(name) else {
                return;
            };
            let Some(identity) = self.symbols.identity_of(symbol_id) else {
                return;
            };
            let (Some(required), Some(SymbolKind::Function(info))) = (
                self.hash_key_type_params.get(identity),
                self.symbols.get(symbol_id).map(|symbol| &symbol.kind),
            ) else {
                return;
            };
            let params = info
                .params
                .iter()
                .map(|param| (param.name.clone().unwrap_or_default(), param.kind, param.ty.clone()))
                .collect::<Vec<_>>();
            (required.clone(), params)
        };
        let mut positional = params.iter().filter(|(_, kind, _)| *kind == ParamKind::Normal);
        for arg in args {
            let (param, expr) = match arg {
                CallArg::Positional(expr) => (positional.next(), expr),
                CallArg::Named(name, expr) => (params.iter().find(|(param, _, _)| *param == name.node), expr),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => return,
            };
            let (Some((_, _, param_ty)), Some(arg_ty)) = (param, Self::scan_type(expr, env)) else {
                continue;
            };
            for required_param in &required {
                if let Some(bound) = Self::scan_binding(param_ty, &arg_ty, required_param) {
                    Self::collect_named_type_params(&bound, &target.type_params, hashed);
                }
            }
        }
    }

    /// Bind `for` targets and plain assignments whose value the scan can type, in source order, into `env`.
    fn bind_statement_locals(body: &[Spanned<Statement>], env: &mut HashMap<String, ResolvedType>) {
        for statement in body {
            match &statement.node {
                Statement::Assignment(assignment) => {
                    let declared = assignment
                        .ty
                        .is_none()
                        .then(|| Self::scan_type(&assignment.value, env))
                        .flatten();
                    if let Some(ty) = declared {
                        env.entry(assignment.name.clone()).or_insert(ty);
                    }
                }
                Statement::For(for_stmt) => {
                    if let (Pattern::Binding(name), Some(item)) = (
                        &for_stmt.pattern.node,
                        Self::scan_type(&for_stmt.iter, env)
                            .as_ref()
                            .and_then(Self::scan_item_type),
                    ) {
                        env.entry(name.clone()).or_insert(item);
                    }
                    Self::bind_statement_locals(&for_stmt.body, env);
                }
                Statement::If(if_stmt) => {
                    Self::bind_statement_locals(&if_stmt.then_body, env);
                    for (_, branch) in &if_stmt.elif_branches {
                        Self::bind_statement_locals(branch, env);
                    }
                    if let Some(else_body) = &if_stmt.else_body {
                        Self::bind_statement_locals(else_body, env);
                    }
                }
                Statement::While(while_stmt) => Self::bind_statement_locals(&while_stmt.body, env),
                Statement::Loop(loop_stmt) => Self::bind_statement_locals(&loop_stmt.body, env),
                Statement::Unsafe(unsafe_stmt) => Self::bind_statement_locals(&unsafe_stmt.body, env),
                _ => {}
            }
        }
    }

    /// Type an expression as far as the declaration says: a known name, an element of one, or a list built from one.
    fn scan_type(expr: &Spanned<Expr>, env: &HashMap<String, ResolvedType>) -> Option<ResolvedType> {
        match &expr.node {
            Expr::Ident(name) => env.get(name).cloned(),
            Expr::Paren(inner) => Self::scan_type(inner, env),
            Expr::Index(base, _) => {
                let base_ty = Self::scan_type(base, env)?;
                match &base_ty {
                    ResolvedType::Generic(name, args) if collection_type_id(name) == Some(CollectionTypeId::Dict) => {
                        args.get(1).cloned()
                    }
                    _ => Self::scan_item_type(&base_ty),
                }
            }
            Expr::List(entries) => match entries.first() {
                Some(ListEntry::Element(first)) => Some(ResolvedType::Generic(
                    "List".to_string(),
                    vec![Self::scan_type(first, env)?],
                )),
                _ => None,
            },
            Expr::Call(callee, _, args) => {
                let (Expr::Ident(name), [CallArg::Positional(source)]) = (&callee.node, args.as_slice()) else {
                    return None;
                };
                let item = Self::scan_type(source, env).as_ref().and_then(Self::scan_item_type)?;
                match collection_type_id(name) {
                    Some(CollectionTypeId::List) => Some(ResolvedType::Generic("List".to_string(), vec![item])),
                    Some(CollectionTypeId::Set) => Some(ResolvedType::Generic("Set".to_string(), vec![item])),
                    _ if name == "sorted" => Some(ResolvedType::Generic("List".to_string(), vec![item])),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Return the element type iterating a collection type yields: an element, or a dict's key.
    fn scan_item_type(ty: &ResolvedType) -> Option<ResolvedType> {
        match ty {
            ResolvedType::Generic(name, args) => {
                let iterable = matches!(
                    collection_type_id(name),
                    Some(
                        CollectionTypeId::List
                            | CollectionTypeId::Set
                            | CollectionTypeId::Dict
                            | CollectionTypeId::FrozenList
                            | CollectionTypeId::FrozenSet
                            | CollectionTypeId::FrozenDict
                    )
                ) || surface_types::from_str(name) == Some(SurfaceTypeId::Vec);
                iterable.then(|| args.first().cloned()).flatten()
            }
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => Some(inner.as_ref().clone()),
            ResolvedType::FrozenDict(key, _) => Some(key.as_ref().clone()),
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => Self::scan_item_type(inner),
            _ => None,
        }
    }

    /// Find what `arg_ty` binds type parameter `name` to, by matching it against `param_ty`.
    fn scan_binding(param_ty: &ResolvedType, arg_ty: &ResolvedType, name: &str) -> Option<ResolvedType> {
        match (param_ty, arg_ty) {
            (ResolvedType::TypeVar(param) | ResolvedType::Named(param), _) if param == name => Some(arg_ty.clone()),
            (ResolvedType::Generic(param_name, params), ResolvedType::Generic(arg_name, args))
                if (param_name == arg_name
                    || collection_type_id(param_name).is_some_and(|id| collection_type_id(arg_name) == Some(id)))
                    && params.len() == args.len() =>
            {
                params
                    .iter()
                    .zip(args)
                    .find_map(|(param, arg)| Self::scan_binding(param, arg, name))
            }
            (ResolvedType::Tuple(params), ResolvedType::Tuple(args)) if params.len() == args.len() => params
                .iter()
                .zip(args)
                .find_map(|(param, arg)| Self::scan_binding(param, arg, name)),
            (ResolvedType::FrozenList(param), ResolvedType::FrozenList(arg))
            | (ResolvedType::FrozenSet(param), ResolvedType::FrozenSet(arg)) => Self::scan_binding(param, arg, name),
            _ => None,
        }
    }

    /// Add a type parameter the expression's type is directly, as a literal element or key hashes it.
    fn collect_direct_type_param(ty: Option<ResolvedType>, type_params: &[String], hashed: &mut Vec<String>) {
        if let Some(ResolvedType::TypeVar(name) | ResolvedType::Named(name)) = ty
            && type_params.contains(&name)
            && !hashed.contains(&name)
        {
            hashed.push(name);
        }
    }

    /// Write this module's inferred requirements into its callables' signatures as `Eq` and `Hash` bounds.
    ///
    /// Runs once the module is checked, so the bounds do not change how the module itself is checked; they reach the
    /// exported signature a compiled library publishes, where a consumer's ordinary bound check applies them. A bound
    /// is written only while its bare name reaches the builtin trait: a module that imports another `Eq` or `Hash`
    /// would otherwise publish a requirement on that unrelated trait.
    pub(in crate::typechecker) fn export_inferred_hash_key_bounds(&mut self) {
        let requirements = std::mem::take(&mut self.local_hash_key_requirements);
        if requirements.is_empty() {
            return;
        }
        let mut bounds = Vec::new();
        for derive in [DeriveId::Eq, DeriveId::Hash] {
            let name = derives::as_str(derive);
            if self.builtin_derive_bound(name) != Some(derive) {
                continue;
            }
            bounds.push(TypeBoundInfo {
                name: self.resolve_generic_bound_name(name, crate::ast::Span::default()),
                source_name: self.trait_bound_source_name(name),
                type_args: Vec::new(),
                module_path: self.trait_bound_module_path(name),
                implementation_type_params: Vec::new(),
                inferred: true,
            });
        }
        if bounds.is_empty() {
            return;
        }
        for (callable, type_params) in requirements {
            match &callable {
                HashKeyCallable::Function(name) => {
                    let Some(symbol_id) = self.symbols.lookup(name) else {
                        continue;
                    };
                    if let Some(symbol) = self.symbols.get_mut(symbol_id)
                        && let SymbolKind::Function(info) = &mut symbol.kind
                    {
                        let added = Self::add_inferred_bounds(
                            &mut info.type_param_bounds,
                            &mut info.type_param_bound_details,
                            &type_params,
                            &bounds,
                        );
                        self.inferred_function_bounds.insert(name.clone(), added);
                    }
                }
                HashKeyCallable::Method { owner, method } => {
                    let Some(symbol_id) = self.symbols.lookup(owner) else {
                        continue;
                    };
                    let Some(symbol) = self.symbols.get_mut(symbol_id) else {
                        continue;
                    };
                    let (methods, method_overloads) = match &mut symbol.kind {
                        SymbolKind::Type(TypeInfo::Model(info)) => (&mut info.methods, &mut info.method_overloads),
                        SymbolKind::Type(TypeInfo::Class(info)) => (&mut info.methods, &mut info.method_overloads),
                        SymbolKind::Type(TypeInfo::Enum(info)) => (&mut info.methods, &mut info.method_overloads),
                        SymbolKind::Type(TypeInfo::Newtype(info)) => (&mut info.methods, &mut info.method_overloads),
                        _ => continue,
                    };
                    // A method's export reads its checked signature, which is its entry in the overload record; a
                    // name with several overloads is left alone, since the inference cannot tell them apart.
                    let checked = method_overloads
                        .get_mut(method)
                        .filter(|overloads| overloads.len() == 1)
                        .into_iter()
                        .flatten()
                        .chain(methods.get_mut(method));
                    for info in checked {
                        let _ = Self::add_inferred_bounds(
                            &mut info.type_param_bounds,
                            &mut info.type_param_bound_details,
                            &type_params,
                            &bounds,
                        );
                    }
                }
            }
        }
    }

    /// Add each bound a type parameter does not already carry, in both bound records, and return the added bounds by
    /// type parameter.
    fn add_inferred_bounds(
        names: &mut HashMap<String, Vec<String>>,
        details: &mut HashMap<String, Vec<TypeBoundInfo>>,
        type_params: &BTreeSet<String>,
        bounds: &[TypeBoundInfo],
    ) -> HashMap<String, Vec<TypeBoundInfo>> {
        let mut added = HashMap::new();
        for type_param in type_params {
            let param_details = details.entry(type_param.clone()).or_default();
            let param_names = names.entry(type_param.clone()).or_default();
            for bound in bounds {
                if !param_details.iter().any(|existing| existing.name == bound.name) {
                    param_details.push(bound.clone());
                    added
                        .entry(type_param.clone())
                        .or_insert_with(Vec::new)
                        .push(bound.clone());
                }
                if !param_names.contains(&bound.name) {
                    param_names.push(bound.name.clone());
                }
            }
        }
        added
    }

    /// Return the `Eq` and `Hash` bounds inferred for a type parameter of a module-level function of the checked
    /// module, which its declaration does not spell (#1758).
    ///
    /// A function's library export reads its declared bounds from source, so it adds these to publish the whole
    /// requirement; a method's export already reads them from its checked signature.
    pub(crate) fn inferred_type_param_bounds(&self, function: &str, type_param: &str) -> &[TypeBoundInfo] {
        self.inferred_function_bounds
            .get(function)
            .and_then(|by_param| by_param.get(type_param))
            .map_or(&[], Vec::as_slice)
    }
}
