//! Declaration-owned constructor defaults that initialize fresh storage without accessing a caller's places.

use std::collections::{HashMap, HashSet};

use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

use super::TypeChecker;
use crate::ast::{CallArg, Declaration, Expr, FieldDecl, Literal, Program, Span, Spanned, Type};
use crate::symbols::{ResolvedType, SymbolKind, resolve_type};

/// A zero-argument nominal can be admitted only after proving all the defaults in its declaring module. Holding
/// its canonical identity until use allows forward references without importing private names into the consumer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(in crate::typechecker) enum DefaultValue {
    Literal,
    Constructor(CanonicalSymbolId),
}

#[derive(Default)]
pub(in crate::typechecker) struct ConstructorDefaults(
    pub(in crate::typechecker) HashMap<CanonicalSymbolId, Vec<Option<DefaultValue>>>,
);

impl ConstructorDefaults {
    pub(in crate::typechecker) fn clear(&mut self) {
        self.0.clear();
    }

    pub(in crate::typechecker) fn proves(&self, owner: &CanonicalSymbolId, count: usize, omitted: &[usize]) -> bool {
        let Some(fields) = self.0.get(owner).filter(|fields| fields.len() == count) else {
            return false;
        };
        let mut visiting = HashSet::new();
        let mut proven = HashSet::new();
        omitted.iter().all(|&slot| {
            fields
                .get(slot)
                .and_then(Option::as_ref)
                .is_some_and(|value| self.proves_value(value, &mut visiting, &mut proven))
        })
    }

    fn proves_value<'a>(
        &'a self,
        value: &'a DefaultValue,
        visiting: &mut HashSet<&'a CanonicalSymbolId>,
        proven: &mut HashSet<&'a CanonicalSymbolId>,
    ) -> bool {
        let DefaultValue::Constructor(owner) = value else {
            return true;
        };
        if proven.contains(owner) {
            return true;
        }
        if !visiting.insert(owner) {
            return false;
        }
        let closed = self.0.get(owner).is_some_and(|fields| {
            fields.iter().all(|field| {
                field
                    .as_ref()
                    .is_some_and(|value| self.proves_value(value, visiting, proven))
            })
        });
        visiting.remove(owner);
        if closed {
            proven.insert(owner);
        }
        closed
    }
}

/// Keep source defaults with their actual declaring model/class. Inheritance, validation and decorated construction
/// require their own effect proof. Their caller cannot acquire one from a matching field count or source spelling.
fn fields(declaration: &Declaration) -> Option<(&str, SemanticSourceTargetKind, &[Spanned<FieldDecl>])> {
    match declaration {
        Declaration::Model(model) if model.decorators.is_empty() && model.traits.is_empty() => {
            Some((&model.name, SemanticSourceTargetKind::Model, &model.fields))
        }
        Declaration::Class(class)
            if class.extends.is_none() && class.decorators.is_empty() && class.traits.is_empty() =>
        {
            Some((&class.name, SemanticSourceTargetKind::Class, &class.fields))
        }
        _ => None,
    }
}

impl TypeChecker {
    pub(in crate::typechecker) fn collect_constructor_defaults(&mut self, program: &Program) {
        let origin = self
            .symbols
            .module_declaration_identity("", SemanticSourceTargetKind::Model, Span::default())
            .origin;
        self.constructor_defaults.0.retain(|id, _| id.origin != origin);
        let mut owners = HashMap::new();
        for declaration in &program.declarations {
            if let Some((name, kind, _)) = fields(&declaration.node) {
                owners.insert(
                    name,
                    self.symbols.module_declaration_identity(name, kind, declaration.span),
                );
            }
        }
        for declaration in &program.declarations {
            let Some((name, _, fields)) = fields(&declaration.node) else {
                continue;
            };
            let values: Vec<_> = fields
                .iter()
                .filter(|field| self.member_binding_is_active(field.span))
                .map(|field| self.closed_default_value(field.node.default.as_ref()?, &field.node.ty.node, &owners))
                .collect();
            if values.is_empty() || values.iter().any(Option::is_some) {
                self.constructor_defaults.0.insert(owners[name].clone(), values);
            }
        }
    }

    fn closed_default_value(
        &self,
        value: &Spanned<Expr>,
        expected: &Type,
        local: &HashMap<&str, CanonicalSymbolId>,
    ) -> Option<DefaultValue> {
        if let Expr::Paren(inner) = &value.node {
            return self.closed_default_value(inner, expected, local);
        }
        let closed_literal = match (&value.node, resolve_type(expected, &self.symbols)) {
            (Expr::Literal(Literal::String(_)), ResolvedType::Str)
            | (Expr::Literal(Literal::Bytes(_)), ResolvedType::Bytes)
            | (Expr::Literal(Literal::Bool(_)), ResolvedType::Bool)
            | (Expr::Literal(Literal::Int(_)), ResolvedType::Int)
            | (Expr::Literal(Literal::Float(_)), ResolvedType::Float) => true,
            (Expr::Literal(Literal::None), ty) => ty.is_option() || ty == ResolvedType::Unit,
            (Expr::List(values), ResolvedType::Generic(name, _)) => {
                values.is_empty() && collections::from_str(&name) == Some(CollectionTypeId::List)
            }
            (Expr::Dict(values), ResolvedType::Generic(name, _)) => {
                values.is_empty() && collections::from_str(&name) == Some(CollectionTypeId::Dict)
            }
            _ => false,
        };
        if closed_literal {
            return Some(DefaultValue::Literal);
        }
        match (&value.node, expected) {
            (Expr::Call(callee, types, args), Type::Simple(field_type)) if types.is_empty() && args.is_empty() => {
                let Expr::Ident(name) = &callee.node else { return None };
                let owner = |name: &str| {
                    local.get(name).cloned().or_else(|| {
                        let id = self.symbols.lookup(name)?;
                        self.symbols
                            .identity_of(id)
                            .filter(|identity| {
                                matches!(
                                    identity.kind,
                                    SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
                                )
                            })
                            .cloned()
                    })
                };
                let target = owner(name)?;
                (owner(field_type)? == target).then_some(DefaultValue::Constructor(target))
            }
            (Expr::MethodCall(base, method, types, args), Type::Generic(field_type, params))
                if method == "new" && types.is_empty() =>
            {
                let (Expr::Ident(name), [CallArg::Positional(value)], [inner]) =
                    (&base.node, args.as_slice(), params.as_slice())
                else {
                    return None;
                };
                // These exact native standard constructors allocate fresh storage around the supplied value. No
                // user method, trait dispatch, callback, stored place or named allocator is part of either operation.
                let native_owner = |name: &str| {
                    let symbol = self.lookup_symbol(name)?;
                    let SymbolKind::RustItem(item) = &symbol.kind else {
                        return None;
                    };
                    Some(incan_lang::interop::metadata::ancestral_rust_path(&item.path))
                };
                let target = native_owner(name)?;
                if native_owner(field_type)? != target
                    || !matches!(target.as_str(), "alloc::rc::Rc" | "core::cell::RefCell")
                {
                    return None;
                }
                self.closed_default_value(value, &inner.node, local)
            }
            _ => None,
        }
    }
}
