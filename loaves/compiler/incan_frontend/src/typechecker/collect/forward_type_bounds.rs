//! Record the type a bound names when that type is declared after the bound's function or method (#1033).
//!
//! A bound that names a type records that type's declaration while its callable is collected, so every caller compares
//! its type argument with the declaration instead of resolving the bound's spelling again. Collection registers
//! declarations in source order, so a bound on a type declared further down the module finds no type to record, and
//! its callers fell back to their own names: a caller's own type of the same name satisfied it, and the declared type
//! imported under another name did not. Declaration order must not matter within a module, so once every declaration
//! is collected this pass gives each such bound the record it would have had.
//!
//! Two overloads keyed on one type are a duplicate declaration. Collection compares each overload with the earlier
//! ones as it registers it, which misses a pair whose key was declared later; this pass compares those.

use std::collections::HashMap;

use crate::ast::{Declaration, MethodDecl, Span, Spanned, TypeParam};
use crate::diagnostics::errors;
use crate::symbols::{MethodInfo, ResolvedType, SymbolKind, TypeBoundInfo, TypeInfo};
use crate::typechecker::trait_bound_relations::type_keyed_overload_signatures_identical;

use super::TypeChecker;

/// One bound whose type collection could not yet name, with the record its declaring module gives it now.
struct LateBoundRecord {
    /// The type parameter that carries the bound.
    type_param: String,
    /// The bound's position among that parameter's bounds.
    position: usize,
    /// The name the bound spells.
    spelling: String,
    /// The recorded type, as [`TypeChecker::type_bound_declaration`] spells it.
    source_name: String,
    /// The recorded type's declaring module, absent for a builtin.
    module_path: Option<Vec<String>>,
}

/// Give each bound that still has no recorded type its record, and return whether any bound changed.
///
/// A bound is matched by its type parameter, position and spelling, so an overload that does not declare the bound is
/// left alone.
fn apply_late_bounds(details: &mut HashMap<String, Vec<TypeBoundInfo>>, records: &[LateBoundRecord]) -> bool {
    let mut changed = false;
    for record in records {
        let Some(bound) = details
            .get_mut(&record.type_param)
            .and_then(|bounds| bounds.get_mut(record.position))
        else {
            continue;
        };
        if bound.name != record.spelling
            || !bound.type_args.is_empty()
            || TypeChecker::declared_bound_type(bound).is_some()
        {
            continue;
        }
        bound.source_name = Some(record.source_name.clone());
        bound.module_path.clone_from(&record.module_path);
        changed = true;
    }
    changed
}

/// The name, type parameters and methods of a declaration that declares methods.
fn method_owner(declaration: &Declaration) -> Option<(&str, &[TypeParam], &[Spanned<MethodDecl>])> {
    match declaration {
        Declaration::Model(model) => Some((
            model.name.as_str(),
            model.type_params.as_slice(),
            model.methods.as_slice(),
        )),
        Declaration::Class(class) => Some((
            class.name.as_str(),
            class.type_params.as_slice(),
            class.methods.as_slice(),
        )),
        Declaration::Trait(tr) => Some((tr.name.as_str(), tr.type_params.as_slice(), tr.methods.as_slice())),
        Declaration::Enum(en) => Some((en.name.as_str(), en.type_params.as_slice(), en.methods.as_slice())),
        Declaration::Newtype(nt) => Some((nt.name.as_str(), nt.type_params.as_slice(), nt.methods.as_slice())),
        _ => None,
    }
}

/// Apply late bound records, by method name, to every collected signature of a nominal or trait.
fn apply_to_methods(kind: &mut SymbolKind, late: &[(String, Vec<LateBoundRecord>)]) {
    let (methods, overloads): (
        &mut HashMap<String, MethodInfo>,
        Option<&mut HashMap<String, Vec<MethodInfo>>>,
    ) = match kind {
        SymbolKind::Type(TypeInfo::Model(info)) => (&mut info.methods, Some(&mut info.method_overloads)),
        SymbolKind::Type(TypeInfo::Class(info)) => (&mut info.methods, Some(&mut info.method_overloads)),
        SymbolKind::Type(TypeInfo::Newtype(info)) => (&mut info.methods, Some(&mut info.method_overloads)),
        SymbolKind::Type(TypeInfo::Enum(info)) => (&mut info.methods, Some(&mut info.method_overloads)),
        SymbolKind::Trait(info) => (&mut info.methods, None),
        _ => return,
    };
    for (name, records) in late {
        if let Some(method) = methods.get_mut(name) {
            apply_late_bounds(&mut method.type_param_bound_details, records);
        }
    }
    let Some(overloads) = overloads else {
        return;
    };
    for (name, records) in late {
        for method in overloads.get_mut(name).into_iter().flatten() {
            apply_late_bounds(&mut method.type_param_bound_details, records);
        }
    }
}

impl TypeChecker {
    /// Record the types that bounds of this module's functions and methods name when collection met the bound before
    /// the type. `forward` maps each type-position declaration of the module to the type its name resolves to now.
    pub(super) fn record_forward_type_bounds(
        &mut self,
        declarations: &[Spanned<Declaration>],
        forward: &HashMap<String, ResolvedType>,
    ) {
        for declaration in declarations {
            if let Declaration::Function(func) = &declaration.node {
                let records = self.late_bound_records(&func.type_params, &[], forward);
                if !records.is_empty() {
                    self.record_function_late_bounds(&func.name, declaration.span, &records);
                }
                continue;
            }
            let Some((owner, owner_type_params, methods)) = method_owner(&declaration.node) else {
                continue;
            };
            let late: Vec<(String, Vec<LateBoundRecord>)> = methods
                .iter()
                .map(|method| {
                    let records = self.late_bound_records(&method.node.type_params, owner_type_params, forward);
                    (method.node.name.clone(), records)
                })
                .filter(|(_, records)| !records.is_empty())
                .collect();
            if late.is_empty() {
                continue;
            }
            let Some(symbol_id) = self.symbols.lookup(owner) else {
                continue;
            };
            if let Some(symbol) = self.symbols.get_mut(symbol_id)
                && symbol.span == declaration.span
            {
                apply_to_methods(&mut symbol.kind, &late);
            }
        }
    }

    /// The bounds among `type_params` that name a type this module declares, with the record each now resolves to.
    ///
    /// A name one of the callable's or its owner's type parameters shadows names that parameter, not the type.
    fn late_bound_records(
        &self,
        type_params: &[TypeParam],
        owner_type_params: &[TypeParam],
        forward: &HashMap<String, ResolvedType>,
    ) -> Vec<LateBoundRecord> {
        let shadowed = |name: &str| {
            type_params
                .iter()
                .chain(owner_type_params)
                .any(|param| param.name == name)
        };
        let mut records = Vec::new();
        for type_param in type_params {
            for (position, bound) in type_param.bounds.iter().enumerate() {
                if !bound.type_args.is_empty() || !forward.contains_key(&bound.name) || shadowed(&bound.name) {
                    continue;
                }
                if let Some((source_name, module_path)) = self.type_bound_declaration(&bound.name) {
                    records.push(LateBoundRecord {
                        type_param: type_param.name.clone(),
                        position,
                        spelling: bound.name.clone(),
                        source_name,
                        module_path,
                    });
                }
            }
        }
        records
    }

    /// Apply late bound records to the function declared at `span`, in its symbol and in the overload set recorded for
    /// lowering, and refuse it when it is now keyed on the same types as another overload of its name.
    fn record_function_late_bounds(&mut self, name: &str, span: Span, records: &[LateBoundRecord]) {
        let mut duplicate_span = None;
        if let Some(symbol) = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get_mut(symbol_id))
        {
            let declared_here = symbol.span == span;
            match &mut symbol.kind {
                SymbolKind::Function(info) if declared_here => {
                    apply_late_bounds(&mut info.type_param_bound_details, records);
                }
                SymbolKind::FunctionOverloads(overloads) => {
                    let position = overloads.iter().position(|overload| overload.span == span);
                    if let Some(position) = position
                        && apply_late_bounds(&mut overloads[position].info.type_param_bound_details, records)
                    {
                        let changed = &overloads[position].info;
                        duplicate_span = overloads
                            .iter()
                            .enumerate()
                            .find(|(other, overload)| {
                                *other != position && type_keyed_overload_signatures_identical(&overload.info, changed)
                            })
                            .map(|(other, _)| overloads[other.max(position)].span);
                    }
                }
                _ => {}
            }
        }
        if let Some(overloads) = self.type_info.declarations.function_overloads.get_mut(name) {
            for overload in overloads.iter_mut().filter(|overload| overload.span == span) {
                apply_late_bounds(&mut overload.info.type_param_bound_details, records);
            }
        }
        if let Some(duplicate_span) = duplicate_span {
            self.errors
                .push(errors::duplicate_type_keyed_overload(name, duplicate_span));
        }
    }
}
