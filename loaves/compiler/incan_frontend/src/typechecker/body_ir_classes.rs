//! Read-only checked class snapshots for Body IR; legacy inheritance and layout tables remain untouched.

use std::collections::HashSet;

use super::TypeChecker;
use super::type_info::BodyIrClassInheritance;
use crate::ast::{Declaration, ParamKind, Program};
use crate::symbols::{ClassInfo, ResolvedType, TypeInfo};
use incan_semantics_core::body_ir::{ClassInheritance, ClassMethodImplementation};

impl TypeChecker {
    /// Retain only proven local instance inheritance, using checked parent/member identities and constructor order.
    pub(super) fn record_body_ir_class_inheritance(&mut self, program: &Program) {
        let local: HashSet<_> = program
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                Declaration::Class(class) => Some(class.name.as_str()),
                _ => None,
            })
            .collect();
        let mut retained = Vec::new();
        for name in &local {
            if let Some(facts) = self.body_ir_class_inheritance(name, &local) {
                retained.push(((*name).to_string(), facts));
            }
        }
        self.type_info.declarations.body_ir_class_inheritance.extend(retained);
    }

    /// Snapshot a local acyclic parent chain and nearest overrides; missing identities or open shapes stay absent.
    fn body_ir_class_inheritance(&self, name: &str, local: &HashSet<&str>) -> Option<BodyIrClassInheritance> {
        let TypeInfo::Class(info) = self.lookup_type_info(name)? else {
            return None;
        };
        let parent_name = info.extends.as_deref()?;
        let parent = self.symbols.module_type_identity(parent_name)?.clone();
        if !plain_class(info)
            || self
                .type_info
                .declarations
                .decorated_method_bindings
                .keys()
                .any(|(owner, _)| owner == name)
        {
            return None;
        }
        let field_identities = info
            .field_order
            .iter()
            .map(|name| info.fields.get(name)?.identity.clone())
            .collect::<Option<Vec<_>>>()?;
        let mut methods = Vec::new();
        let mut seen = HashSet::from([name.to_string()]);
        let mut current = Some(parent_name);
        while let Some(ancestor_name) = current {
            if !local.contains(ancestor_name) || !seen.insert(ancestor_name.to_string()) {
                return None;
            }
            let TypeInfo::Class(ancestor) = self.lookup_type_info(ancestor_name)? else {
                return None;
            };
            if !plain_class(ancestor)
                || !ancestor.field_order.iter().enumerate().all(|(index, field)| {
                    info.field_order.get(index) == Some(field)
                        && info.fields.get(field).and_then(|field| field.identity.as_ref())
                            == ancestor.fields.get(field).and_then(|field| field.identity.as_ref())
                })
            {
                return None;
            }
            for (slot, method) in &ancestor.methods {
                let method = method.identity.as_ref()?;
                let implementation = info.methods.get(slot)?.identity.as_ref()?;
                if self
                    .type_info
                    .declarations
                    .decorated_method_bindings
                    .contains_key(&(ancestor_name.to_string(), slot.clone()))
                {
                    return None;
                }
                if methods
                    .iter()
                    .any(|retained: &ClassMethodImplementation| &retained.method == method)
                {
                    continue;
                }
                methods.push(ClassMethodImplementation {
                    method: method.clone(),
                    implementation: implementation.clone(),
                });
            }
            current = ancestor.extends.as_deref();
        }
        methods.sort_by(|left, right| left.method.cmp(&right.method));
        Some(BodyIrClassInheritance {
            inheritance: ClassInheritance { parent, methods },
            field_identities,
        })
    }
}

/// Bound inheritance to concrete ordinary instance methods and fields whose executable frames are proven here.
/// Checked trait adoptions remain independent of the parent layout and nearest implementation identities.
fn plain_class(info: &ClassInfo) -> bool {
    info.type_params.is_empty()
        && info.properties.is_empty()
        && info.method_aliases.is_empty()
        && info.field_defaults.is_empty()
        && info.field_default_metadata.is_empty()
        && info
            .derives
            .iter()
            .all(|derive| matches!(derive.as_str(), "Debug" | "Clone"))
        && info
            .fields
            .values()
            .all(|field| field.alias.is_none() && field.identity.is_some())
        && info.method_overloads.values().all(|methods| methods.len() == 1)
        && info.methods.values().all(|method| {
            method.has_body
                && !method.is_async
                && method.receiver.is_some()
                && method.type_params.is_empty()
                && method.alias_of.is_none()
                && scalar_type(&method.return_type)
                && method.params.iter().all(|param| {
                    param.name.is_some()
                        && param.kind == ParamKind::Normal
                        && !param.has_default
                        && scalar_type(&param.ty)
                })
        })
}

/// Primitive ordinary inputs and results need no inherited nominal substitution beyond the explicit receiver.
fn scalar_type(ty: &ResolvedType) -> bool {
    matches!(
        ty,
        ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool | ResolvedType::Str | ResolvedType::Unit
    )
}
