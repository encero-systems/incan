//! Body-IR-only declaration facts for checked SDK facade imports missing legacy inference metadata.

use super::args::DeclaredSlot;
use super::{BodyBuilder, ast, bir, hir_span};
use crate::symbols::SymbolKind;
use crate::typechecker::{FunctionBindingInfo, TypeCheckInfo, stdlib_loader::StdlibAstCache};
use incan_lang::lang::stdlib;
use incan_semantics_core::{IncanType, SymbolOrigin};

/// Supplement only a private snapshot from active checked import paths and exact SDK source declarations.
///
/// The original checker snapshot is immutable: source emitters keep their existing unresolved inference and bytes.
/// Existing identities always win, and unresolved or overloaded exports remain unproved.
pub(super) fn project(program: &ast::Program, info: &TypeCheckInfo) -> TypeCheckInfo {
    let mut projected = info.clone();
    let mut cache = StdlibAstCache::new();
    for (local, path) in info.import_bindings.iter() {
        if !from_import_proves(program, local, path) {
            continue;
        }
        let Some((name, module)) = path.split_last() else {
            continue;
        };
        if !stdlib::is_any_stdlib_path(module) || info.resolved_import_identity(local).is_some() {
            continue;
        }
        let Some(identity) = cache.lookup_identity(module, name) else {
            continue;
        };
        if super::library_nominals::sdk_identity(&identity).is_none() {
            continue;
        }
        projected
            .declarations
            .resolved_import_identities
            .insert(local.to_owned(), identity.clone());
        if let Some(SymbolKind::Function(function)) = cache.lookup_function_symbol(module, name) {
            projected
                .declarations
                .function_bindings
                .entry(local.to_owned())
                .or_insert(FunctionBindingInfo {
                    is_async: function.is_async,
                    params: function.params,
                    return_type: function.return_type,
                    identity: Some(identity),
                });
        } else if cache.lookup_type(module, name).is_some() {
            projected
                .declarations
                .named_type_identities
                .entry(local.to_owned())
                .or_insert(identity);
        }
    }
    projected
}

/// Require a source member import at exactly the active checked path; a module import grants no callable identity.
fn from_import_proves(program: &ast::Program, local: &str, path: &[String]) -> bool {
    program.declarations.iter().any(|declaration| {
        let ast::Declaration::Import(import) = &declaration.node else {
            return false;
        };
        let ast::ImportKind::From { module, items } = &import.kind else {
            return false;
        };
        items.iter().any(|item| {
            item.alias.as_deref().unwrap_or(&item.name) == local
                && path
                    .split_last()
                    .is_some_and(|(name, prefix)| name == &item.name && prefix == module.segments)
        })
    })
}

impl BodyBuilder<'_, '_> {
    /// Close a checked SDK newtype's one-parameter static constructor from its already typed source argument.
    ///
    /// This bounded profile requires one owner binder, that binder as the sole parameter, and the same owner as
    /// the result. It never selects a source-owned type, overload, method binder, spread, default or receiver.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_sdk_static_constructor(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let ast::Expr::Ident(local) = &receiver.node else {
            return None;
        };
        if self.bindings.contains_key(local) {
            return None;
        }
        let identity = self.type_info.resolved_import_identity(local)?;
        if let Some(selected) = self.type_info.resolved_identity(receiver.span)
            && super::library_nominals::sdk_identity(selected) != super::library_nominals::sdk_identity(identity)
        {
            return None;
        }
        super::library_nominals::sdk_identity(identity)?;
        let SymbolOrigin::Module(module) = &identity.origin else {
            return None;
        };
        let mut cache = StdlibAstCache::new();
        let crate::symbols::TypeInfo::Newtype(owner) = cache.lookup_type(module, &identity.declaration_name)? else {
            return None;
        };
        if owner.is_rusttype || owner.type_params.len() != 1 {
            return None;
        }
        let method = owner.methods.get(name)?;
        if method.receiver.is_some() || !method.has_body || !method.type_params.is_empty() || method.params.len() != 1 {
            return None;
        }
        let selected = method.identity.as_ref()?;
        let record = super::library_calls::library_call(selected, &mut cache)?;
        let [IncanType::TypeVar(parameter)] = record.parameters.as_slice() else {
            return None;
        };
        if record.type_parameters.len() != 1 || record.type_parameters[0] != *parameter {
            return None;
        }
        let [argument] = args else { return None };
        let expression = match argument {
            ast::CallArg::Positional(value) => value,
            ast::CallArg::Named(key, value) if method.params[0].name.as_ref() == Some(&key.node) => value,
            _ => return None,
        };
        let actual = self.resolve_ty(expression.span);
        if self.leaves_type_open(&actual) {
            return None;
        }
        let result = IncanType::Generic {
            base: identity.declaration_name.clone(),
            args: vec![actual.clone()],
        };
        let IncanType::Generic { base, args: output } = &record.return_type else {
            return None;
        };
        if base != &identity.declaration_name || output != &record.parameters {
            return None;
        }
        let mut slot = DeclaredSlot::from_checked_param(&method.params[0]);
        if slot.has_default || slot.kind != incan_semantics_core::IncanCallableParamKind::Normal {
            return None;
        }
        slot.ty = actual.clone();
        let (operands, binding) = self
            .bind_declared_args("checked SDK static constructor", Some(vec![slot]), args, scope, out)
            .ok()?;
        Some(self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: Some(result.clone()),
                name: format!("{local}.{name}"),
                direct_call_id: None,
                canonical: Some(selected.clone()),
                builtin: None,
                type_args: Vec::new(),
                binding,
            })),
            operands,
            result,
            scope,
            hir_span(span),
            false,
            out,
        ))
    }
}
