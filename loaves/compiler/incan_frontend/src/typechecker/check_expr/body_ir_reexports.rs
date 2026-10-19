//! Close validated published calls through retained source facade links without changing legacy resolution.

use std::collections::HashMap;

use super::TypeChecker;
use crate::ast::{CallArg, Declaration, Expr, ImportKind, ImportPath, ParamKind, Program, Spanned, Visibility};
use crate::symbols::{FunctionInfo, ResolvedType, SymbolKind};
use crate::typechecker::type_info::BodyIrReexportCallInfo;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

impl TypeChecker {
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
        let Some(SymbolKind::Module(module)) = self.lookup_symbol(name).map(|symbol| &symbol.kind) else {
            return;
        };
        let Some(path) = self.import_binding_path(name).filter(|path| *path == module.path) else {
            return;
        };
        if module.is_python {
            return;
        }
        let Some((member, segments)) = path.split_last() else {
            return;
        };
        let import = ImportPath::simple(segments.to_vec());
        let base = self.current_module_path.as_deref().unwrap_or_default();
        let Some((canonical, signature)) = self.body_ir_reexport_signature(base, &import, member, 0) else {
            return;
        };

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
        let key = (expression.span.start, expression.span.end);
        self.type_info.calls.body_ir_reexport_calls.insert(
            key,
            BodyIrReexportCallInfo {
                canonical,
                parameters: signature.params,
            },
        );
        self.type_info
            .calls
            .body_ir_call_result_types
            .insert(key, signature.return_type);
    }

    /// Follow existing explicit re-export links in source candidate order, stopping at direct declarations and
    /// retaining the facade's resolution base. Only a loaded manifest with one minted function target ends this walk;
    /// cycles, ambiguous or absent targets, and source-owned same-name declarations grant no fallback contract.
    fn body_ir_reexport_signature(
        &self,
        base: &[String],
        module: &ImportPath,
        member: &str,
        depth: usize,
    ) -> Option<(CanonicalSymbolId, FunctionInfo)> {
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
            let SymbolKind::Function(signature) = self.pub_library_function_symbol(manifest, member)? else {
                return None;
            };
            let canonical = manifest
                .contract_metadata
                .identity_graph
                .canonical_for_public_name(member)?;
            if canonical.kind != SemanticSourceTargetKind::Function
                || !matches!(&canonical.origin, SymbolOrigin::Package { library: owner, .. } if owner == library)
            {
                return None;
            }
            return Some((canonical, signature));
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
                return self.body_ir_reexport_signature(&owner, target_module, target_name, depth + 1);
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
