//! Bounds a generic caller needs from the source-declared methods it calls (#1779, #1280).
//!
//! A method call on a value of a source nominal type reaches one method of one of that type's impl blocks. Rust holds
//! the call to the whole header of that impl block, whichever method put a bound there, and to the called method's own
//! bounds, instantiated with the type arguments the call supplies. A generic caller that forwards its own type
//! parameter into one of those positions, as a type argument of the receiver or through an argument, needs the same
//! bounds. This module indexes every impl method by its owner and name so an ordinary method call carries that
//! requirement, and adds the implementation header a checked trait dispatch selected, matched by the parent module.

use std::collections::{HashMap, HashSet};

use incan_ir::IrProgram;
use incan_ir::decl::{FunctionParam, IrDeclKind, IrTraitBound, IrTypeParam};
use incan_ir::expr::{IrCallArg, IrExpr, IrMethodDispatch};
use incan_ir::types::IrType;
use incan_lang::lang::stdlib;
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin, encode_incan_symbol_identity};

use super::{
    BoundCollectionContext, ImplementationBoundRequirement, PropagatedBoundRequirement,
    collect_implementation_bound_requirements, collect_type_param_mapping, type_param_name_from_ir_type,
};

/// The parts of one method call that decide what it requires of the caller.
pub(super) struct MethodCallParts<'a> {
    pub(super) receiver: &'a IrExpr,
    pub(super) method: &'a str,
    pub(super) dispatch: Option<&'a IrMethodDispatch>,
    pub(super) type_args: &'a [IrType],
    pub(super) args: &'a [IrCallArg],
}

/// One method a call on its owner type can reach, with the header and signature its requirement is read from.
#[derive(Debug, Clone)]
struct MethodCallee {
    /// The method's callable key in the bound maps (`impl:{target}:{trait or <inherent>}:{index}:{name}`).
    key: String,
    /// The keys of every method in the same impl block; they share its header.
    sibling_keys: Vec<String>,
    /// The impl block's type parameters with the bounds its header carries so far.
    owner_type_params: Vec<IrTypeParam>,
    /// The method's own type parameters with the bounds its header carries so far.
    type_params: Vec<IrTypeParam>,
    /// The method's value parameters other than the receiver, in order.
    params: Vec<FunctionParam>,
    /// Whether the method sits in an inherent impl block rather than a trait implementation.
    inherent: bool,
}

impl MethodCallee {
    /// Return the bounds a call to this method requires, keyed by the impl's and the method's type parameter names.
    ///
    /// An owner parameter needs every bound the shared header has, from its explicit bounds and from each sibling
    /// method's inferred bounds in `function_bounds`; an own parameter needs its explicit bounds and the method's own
    /// inferred bounds.
    fn required_bounds(&self, function_bounds: &HashMap<String, Vec<IrTypeParam>>) -> Vec<IrTypeParam> {
        let owner = self.owner_type_params.iter().map(|param| {
            let mut bounds = param.bounds.clone();
            for key in &self.sibling_keys {
                merge_bounds(&mut bounds, inferred_bounds(function_bounds, key, &param.name));
            }
            IrTypeParam {
                name: param.name.clone(),
                bounds,
            }
        });
        let own = self.type_params.iter().map(|param| {
            let mut bounds = param.bounds.clone();
            merge_bounds(&mut bounds, inferred_bounds(function_bounds, &self.key, &param.name));
            IrTypeParam {
                name: param.name.clone(),
                bounds,
            }
        });
        owner.chain(own).collect()
    }
}

/// Return the bounds one callable's entry in `function_bounds` records for one type parameter.
fn inferred_bounds<'a>(
    function_bounds: &'a HashMap<String, Vec<IrTypeParam>>,
    key: &str,
    param: &str,
) -> &'a [IrTraitBound] {
    function_bounds
        .get(key)
        .and_then(|params| params.iter().find(|candidate| candidate.name == param))
        .map_or(&[], |candidate| candidate.bounds.as_slice())
}

/// Append each bound that is not already present.
fn merge_bounds(target: &mut Vec<IrTraitBound>, source: &[IrTraitBound]) {
    for bound in source {
        if !target.contains(bound) {
            target.push(bound.clone());
        }
    }
}

/// Return the type after removing transparent borrow wrappers.
fn peel_refs(mut ty: &IrType) -> &IrType {
    while let IrType::Ref(inner) | IrType::RefMut(inner) = ty {
        ty = inner.as_ref();
    }
    ty
}

/// Whether two nominal spellings name the same type, comparing their last path segment.
///
/// A callee's parameter type and a caller's argument type name one declaration, possibly spelled with different
/// qualification across modules; two different declarations never pair their type arguments.
pub(super) fn nominal_names_match(left: &str, right: &str) -> bool {
    owner_segment(left) == owner_segment(right)
}

/// Return an identity with a source standard-library module rebased onto the generated `incan_std` namespace, the
/// spelling a projected method call uses for a stdlib-owned declaration.
fn rebased_stdlib_identity(identity: &CanonicalSymbolId) -> CanonicalSymbolId {
    let mut identity = identity.clone();
    if let SymbolOrigin::Module(module_path) = &mut identity.origin
        && let Some(first) = module_path.first_mut()
        && first == stdlib::STDLIB_ROOT
    {
        *first = stdlib::INCAN_STD_NAMESPACE.to_string();
    }
    identity
}

/// Return the last path segment of a nominal spelling, the part every module spells the same way.
fn owner_segment(owner: &str) -> &str {
    owner.rsplit("::").next().unwrap_or(owner)
}

/// Every impl method of the programs in scope, keyed by owner type (its last path segment, so an imported owner
/// matches however the caller's module qualifies it) and by each spelling a call may use for the method.
#[derive(Debug, Default)]
pub(super) struct InherentMethodIndex {
    callees: HashMap<(String, String), MethodCallee>,
}

impl InherentMethodIndex {
    /// Index the impl methods of `program` and then of `externals`; a method of the current program wins over an
    /// external one of the same owner and name, and an inherent method over a trait implementation's.
    pub(super) fn from_programs(program: &IrProgram, externals: &[&IrProgram]) -> Self {
        let mut index = Self::default();
        index.add_program(program, true);
        for external in externals {
            index.add_program(external, false);
        }
        index
    }

    /// Add one program's impl methods under their source names and under the projected names calls spell them with.
    fn add_program(&mut self, program: &IrProgram, current: bool) {
        let mut added = HashSet::new();
        for decl in &program.declarations {
            let IrDeclKind::Impl(impl_block) = &decl.kind else {
                continue;
            };
            let trait_segment = impl_block.trait_name.as_deref().unwrap_or("<inherent>");
            let keys: Vec<String> = impl_block
                .methods
                .iter()
                .enumerate()
                .map(|(index, method)| {
                    format!(
                        "impl:{}:{}:{}:{}",
                        impl_block.target_type, trait_segment, index, method.name
                    )
                })
                .collect();
            for (method, key) in impl_block.methods.iter().zip(&keys) {
                let slot = (owner_segment(&impl_block.target_type).to_string(), method.name.clone());
                let callee = MethodCallee {
                    key: key.clone(),
                    sibling_keys: keys.clone(),
                    owner_type_params: impl_block.type_params.clone(),
                    type_params: method.type_params.clone(),
                    params: method.params.iter().filter(|param| !param.is_self).cloned().collect(),
                    inherent: impl_block.trait_name.is_none(),
                };
                let replace = match self.callees.get(&slot) {
                    None => true,
                    Some(existing) => current && added.contains(&slot) && callee.inherent && !existing.inherent,
                };
                if replace {
                    self.callees.insert(slot.clone(), callee);
                    added.insert(slot);
                }
            }
        }
        // A method is declared under its projected identity; a call spells the same identity, or, for a
        // standard-library owner, the identity rebased onto the generated namespace. Index both, and the source name.
        for (owner, name, identity) in &program.member_projections {
            let owner = owner_segment(owner).to_string();
            let projected = encode_incan_symbol_identity(identity);
            let callee = [projected.as_str(), name.as_str()]
                .into_iter()
                .find_map(|spelling| self.callees.get(&(owner.clone(), spelling.to_string())).cloned());
            let Some(callee) = callee else {
                continue;
            };
            for spelling in [
                projected.clone(),
                encode_incan_symbol_identity(&rebased_stdlib_identity(identity)),
                name.clone(),
            ] {
                self.callees
                    .entry((owner.clone(), spelling))
                    .or_insert_with(|| callee.clone());
            }
        }
    }

    /// Record what one method call requires of the caller's type parameters.
    ///
    /// The receiver's type arguments bind the impl block's type parameters, and explicit type arguments and value
    /// arguments bind the method's own; each binding to one of the caller's type parameters carries the callee's
    /// bounds for that parameter. A Rust extension-trait call, or a call whose receiver is not a source nominal with an
    /// indexed method, adds nothing.
    pub(super) fn collect_requirements(
        &self,
        call: &MethodCallParts<'_>,
        caller_type_params: &HashSet<&str>,
        function_bounds: &HashMap<String, Vec<IrTypeParam>>,
        result: &mut Vec<PropagatedBoundRequirement>,
    ) {
        if matches!(call.dispatch, Some(IrMethodDispatch::RustExtensionTraitImport { .. })) {
            return;
        }
        let (receiver, method, type_args, args) = (call.receiver, call.method, call.type_args, call.args);
        let Some((owner, receiver_type_args)) = nominal_receiver_type(&receiver.ty) else {
            return;
        };
        let Some(callee) = self
            .callees
            .get(&(owner_segment(owner).to_string(), method.to_string()))
        else {
            return;
        };
        let mut mapping = HashMap::new();
        if callee.owner_type_params.len() == receiver_type_args.len() {
            for (param, arg) in callee.owner_type_params.iter().zip(receiver_type_args) {
                if let Some(caller) = type_param_name_from_ir_type(peel_refs(arg), caller_type_params) {
                    mapping.insert(param.name.clone(), caller);
                }
            }
        }
        for (param, arg) in callee.type_params.iter().zip(type_args) {
            if let Some(caller) = type_param_name_from_ir_type(peel_refs(arg), caller_type_params) {
                mapping.insert(param.name.clone(), caller);
            }
        }
        let mut positional = callee.params.iter();
        for arg in args {
            let param = match &arg.name {
                Some(name) => callee.params.iter().find(|param| &param.name == name),
                None => positional.next(),
            };
            if let Some(param) = param {
                collect_type_param_mapping(
                    peel_refs(&param.ty),
                    peel_refs(&arg.expr.ty),
                    caller_type_params,
                    &mut mapping,
                );
            }
        }
        if !mapping.is_empty() {
            result.push((callee.required_bounds(function_bounds), mapping));
        }
    }
}

/// Return the exact nominal receiver and its concrete generic arguments after removing borrow wrappers.
pub(super) fn nominal_receiver_type(ty: &IrType) -> Option<(&str, &[IrType])> {
    match ty {
        IrType::Ref(inner) | IrType::RefMut(inner) => nominal_receiver_type(inner),
        IrType::NamedGeneric(name, type_args) => Some((name.as_str(), type_args)),
        IrType::Struct(name) | IrType::Enum(name) => Some((name.as_str(), &[])),
        _ => None,
    }
}

/// Collect the requirements one method call puts on the caller without widening every recursive expression-scan frame.
///
/// Any call that is not a Rust extension-trait call contributes the impl method it reaches on a source nominal
/// receiver (#1779); checked trait dispatch also contributes the implementation header it selected.
pub(super) fn collect_method_implementation_bound_requirements(
    call: MethodCallParts<'_>,
    context: &BoundCollectionContext<'_, '_>,
    result: &mut Vec<PropagatedBoundRequirement>,
) {
    context
        .inherent_methods
        .collect_requirements(&call, context.type_params, context.function_bounds, result);
    let (receiver, dispatch) = (call.receiver, call.dispatch);
    let Some(IrMethodDispatch::Trait(dispatch) | IrMethodDispatch::SourceProjection(dispatch)) = dispatch else {
        return;
    };
    let trait_source_name = &dispatch.trait_source_name;
    let trait_module_path = &dispatch.trait_module_path;
    let implementation_type_params = &dispatch.implementation_type_params;
    let trait_type_args = &dispatch.type_args;

    if !implementation_type_params.is_empty()
        && let Some((target_type, _)) = nominal_receiver_type(&receiver.ty)
    {
        let checked_requirement = ImplementationBoundRequirement {
            target_type: target_type.to_string(),
            type_params: implementation_type_params.clone(),
            trait_source_name: trait_source_name.clone(),
            trait_module_path: trait_module_path.clone(),
            trait_type_args: trait_type_args.clone(),
        };
        collect_implementation_bound_requirements(
            receiver,
            trait_source_name,
            trait_module_path.as_deref(),
            trait_type_args,
            context.type_params,
            std::slice::from_ref(&checked_requirement),
            result,
        );
    }
    collect_implementation_bound_requirements(
        receiver,
        trait_source_name,
        trait_module_path.as_deref(),
        trait_type_args,
        context.type_params,
        context.implementation_requirements,
        result,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two nominal spellings pair their type arguments only when they name the same declaration (#1779).
    #[test]
    fn nominal_names_match_compares_the_declaration_not_the_arity_issue1779() {
        assert!(nominal_names_match("Stream", "Stream"));
        assert!(nominal_names_match("crate::shapes::Stream", "Stream"));
        assert!(!nominal_names_match("Stream", "Holder"));
        assert!(!nominal_names_match("crate::a::Stream", "crate::b::Other"));
    }
}
