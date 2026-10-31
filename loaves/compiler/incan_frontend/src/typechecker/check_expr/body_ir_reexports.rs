//! Resolve published calls through retained source facade links and preserve their selected declaration facts.

use std::collections::{HashMap, HashSet};

use super::TypeChecker;
use crate::ast::{
    CallArg, Declaration, Expr, ImportKind, ImportPath, ParamKind, Program, Span, Spanned, Type, Visibility,
};
use crate::diagnostics::CompileError;
use crate::symbols::{ResolvedType, SymbolKind};
use crate::typechecker::type_info::BodyIrReexportCallInfo;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

/// One published callable and the checked identities of its complete exported overload set.
struct PublishedReexport {
    library: String,
    member: String,
    kind: SymbolKind,
    identities: Vec<Option<CanonicalSymbolId>>,
}

impl TypeChecker {
    /// Check an overloaded function reached through a source-module bridge with the ordinary overload selector.
    /// Every candidate must retain a distinct published identity before selection; only a uniquely checked call
    /// contributes executable facts. Source shadows and unresolved/private endpoints never grant a callable.
    #[allow(clippy::too_many_arguments)] // The call and its contextual result remain independent checker inputs.
    pub(in crate::typechecker::check_expr) fn check_source_reexport_overload_call(
        &mut self,
        callee: &Spanned<Expr>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        expected: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let Expr::Ident(name) = &callee.node else { return None };
        let target = self.source_reexport_target(name)?;
        if !matches!(target.kind, SymbolKind::FunctionOverloads(_)) {
            return None;
        }
        let Some(SymbolKind::FunctionOverloads(overloads)) =
            self.lookup_pub_library_symbol_member(&target.library, &target.member)
        else {
            self.errors.push(CompileError::type_error(
                format!("Published overload set '{name}' has no checked provider signatures"),
                callee.span,
            ));
            return Some(ResolvedType::Unknown);
        };
        let mut seen = HashSet::new();
        let complete = overloads.len() == target.identities.len()
            && overloads.iter().zip(&target.identities).all(|(overload, identity)| {
                identity.as_ref().is_some_and(|identity| {
                    identity.kind == SemanticSourceTargetKind::Function
                        && matches!(identity.origin, SymbolOrigin::Package { .. })
                        && overload.identity.as_ref() == Some(identity)
                        && seen.insert(identity.clone())
                })
            });
        if !complete {
            self.errors.push(CompileError::type_error(
                format!("Published overload set '{name}' has incomplete or conflicting declaration identities"),
                callee.span,
            ));
            return Some(ResolvedType::Unknown);
        }

        // ---- Ordinary unique-candidate selection ----
        let errors_before = self.errors.len();
        let result = self.validate_function_overload_call_with_callee_span(
            name,
            &overloads,
            type_args,
            args,
            span,
            Some(callee.span),
            expected,
        );
        if self.errors.len() != errors_before {
            return Some(result);
        }
        let selected = self.type_info.resolved_identity(callee.span).cloned();
        let Some((canonical, overload)) = selected.as_ref().and_then(|identity| {
            overloads
                .iter()
                .find(|overload| overload.identity.as_ref() == Some(identity))
                .map(|overload| (identity.clone(), overload))
        }) else {
            self.errors.push(CompileError::type_error(
                format!("Published call '{name}' did not retain its selected overload identity"),
                callee.span,
            ));
            return Some(ResolvedType::Unknown);
        };
        self.type_info.calls.body_ir_reexport_calls.insert(
            span,
            BodyIrReexportCallInfo {
                canonical,
                parameters: overload.info.params.clone(),
            },
        );
        self.retain_body_ir_async_call_target(overload.info.is_async, name, span, callee.span, errors_before);
        Some(result)
    }

    /// Follow a source import only while its active binding is the unresolved module selected by collection.
    fn source_reexport_target(&self, name: &str) -> Option<PublishedReexport> {
        let SymbolKind::Module(module) = &self.lookup_symbol(name)?.kind else {
            return None;
        };
        let path = self.import_binding_path(name).filter(|path| *path == module.path)?;
        if module.is_python {
            return None;
        }
        let (member, segments) = path.split_last()?;
        let import = ImportPath::simple(segments.to_vec());
        let base = self.current_module_path.as_deref().unwrap_or_default();
        self.body_ir_reexport_target(base, &import, member, 0)
    }

    /// Retain public-library endpoints once during dependency-member collection. These explicit links supplement
    /// the frozen source-only re-export cache without changing symbols, import selection or legacy metadata.
    pub(in crate::typechecker) fn retain_body_ir_dependency_pub_reexports(
        &mut self,
        module_name: &str,
        program: &Program,
        public_only: bool,
    ) {
        let mut links = HashMap::new();
        for declaration in &program.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            if public_only && import.visibility != Visibility::Public {
                continue;
            }
            let ImportKind::PubFrom { library, path, items } = &import.kind else {
                continue;
            };
            let mut segments = vec![crate::typechecker::PUBLIC_LIBRARY_NAMESPACE.to_owned(), library.clone()];
            segments.extend(path.iter().cloned());
            let target = ImportPath::simple(segments);
            for item in items {
                links.insert(
                    item.alias.as_ref().unwrap_or(&item.name).clone(),
                    (target.clone(), item.name.clone()),
                );
            }
        }
        self.body_ir_dependency_pub_reexports
            .insert(module_name.to_owned(), links);
    }

    /// Validate an unresolved imported placeholder against one closed manifest signature, retaining only Body IR
    /// target, parameter and result facts. Invalid calls, shadows, defaults, spreads and generic or async signatures
    /// remain unproven; ordinary symbols, expression snapshots and diagnostics are never changed.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_reexport_call(
        &mut self,
        expression: &Spanned<Expr>,
        errors_before: usize,
        returned: &ResolvedType,
    ) {
        if self.errors.len() != errors_before || *returned != ResolvedType::Unknown {
            return;
        }
        // ---- Selected imported binding ----
        let Expr::Call(callee, type_args, arguments) = &expression.node else {
            return;
        };
        let Expr::Ident(name) = &callee.node else {
            return;
        };
        if !type_args.is_empty() || self.type_info.resolved_identity(callee.span).is_some() {
            return;
        }
        let Some(target) = self.source_reexport_target(name) else {
            return;
        };
        let SymbolKind::Function(signature) = target.kind else {
            return;
        };
        let [Some(canonical)] = target.identities.as_slice() else {
            return;
        };
        if canonical.kind != SemanticSourceTargetKind::Function
            || !matches!(canonical.origin, SymbolOrigin::Package { .. })
        {
            return;
        }

        // ---- Validated fixed arguments ----
        if signature.is_async
            || !signature.type_params.is_empty()
            || !fixed_scalar(&signature.return_type)
            || arguments.len() != signature.params.len()
        {
            return;
        }
        for (argument, parameter) in arguments.iter().zip(&signature.params) {
            let CallArg::Positional(value) = argument else {
                return;
            };
            if parameter.kind != ParamKind::Normal
                || parameter.is_mut
                || parameter.has_default
                || parameter.is_partial_preset
                || !fixed_scalar(&parameter.ty)
                || self.body_ir_checked_value_type(value.span) != Some(&parameter.ty)
            {
                return;
            }
        }
        let key = expression.span;
        self.type_info.calls.body_ir_reexport_calls.insert(
            key,
            BodyIrReexportCallInfo {
                canonical: canonical.clone(),
                parameters: signature.params,
            },
        );
        self.type_info
            .calls
            .body_ir_call_result_types
            .insert(key, signature.return_type);
    }

    /// Follow existing explicit re-export links in source candidate order, stopping at direct declarations and
    /// retaining the facade's resolution base. Only a loaded manifest with published callable facts ends this walk;
    /// cycles, ambiguous or absent targets, and source-owned same-name declarations grant no fallback contract.
    fn body_ir_reexport_target(
        &self,
        base: &[String],
        module: &ImportPath,
        member: &str,
        depth: usize,
    ) -> Option<PublishedReexport> {
        if depth >= Self::MAX_REEXPORT_DEPTH {
            return None;
        }
        // ---- Published declaration endpoint ----
        if depth > 0
            && module.parent_levels == 0
            && !module.is_absolute
            && let [root, library] = module.segments.as_slice()
            && root == crate::typechecker::PUBLIC_LIBRARY_NAMESPACE
        {
            let manifest = self.loaves.manifest(library)?;
            let kind = self.pub_library_function_symbol(manifest, member)?;
            let identities = manifest
                .contract_metadata
                .identity_graph
                .function_identities_for_public_name(member);
            return Some(PublishedReexport {
                library: library.clone(),
                member: member.to_owned(),
                kind,
                identities,
            });
        }
        // ---- Source facade candidate order ----
        for owner in self.dependency_source_import_candidates(base, module) {
            let key = owner.join("_");
            if self
                .dependency_direct_member_symbols
                .get(&key)
                .is_some_and(|members| members.contains_key(member))
            {
                return None;
            }
            if let Some((target_module, target_name)) = self
                .dependency_member_reexports
                .get(&key)
                .and_then(|links| links.get(member))
                .or_else(|| {
                    self.body_ir_dependency_pub_reexports
                        .get(&key)
                        .and_then(|links| links.get(member))
                })
            {
                return self.body_ir_reexport_target(&owner, target_module, target_name, depth + 1);
            }
            if self
                .dependency_member_symbols
                .get(&key)
                .is_some_and(|members| members.contains_key(member))
            {
                return None;
            }
        }
        None
    }
}

/// The bounded contract carries only primitive scalar values without nominal remapping or type inference.
fn fixed_scalar(ty: &ResolvedType) -> bool {
    matches!(
        ty,
        ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool | ResolvedType::Str | ResolvedType::Unit
    )
}
