//! First-pass collection: register types, functions, and imports into the symbol table.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostics::CompileError;
use crate::diagnostics::errors;
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::*;
use crate::typechecker::helpers::freeze_const_type;
use incan_lang::lang::decorators::{self as core_decorators, DecoratorId};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

use super::{
    FunctionBindingInfo, PUBLIC_LIBRARY_NAMESPACE, PartialProjectionInfo, PartialProjectionPreset,
    PartialProjectionTargetKind, TypeChecker,
};

mod aliases_and_partials;
mod decl_helpers;
mod declaration_scopes;
pub(super) mod decorators;
mod forward_references;
mod functions;
mod imports;
pub(in crate::typechecker) mod loaf_imports;
mod method_visibility;
mod nominal_declarations;
mod rust_imports;
mod std_root_imports;
mod trait_bounds;
mod traits_and_capabilities;

use self::decl_helpers::{
    collect_fields, collect_method_aliases, collect_method_overloads, collect_methods_from_overloads,
    collect_properties, inject_validate_methods, owner_resolved_type, resolve_declared_type, type_param_name_set,
};

/// Read a capability's `description` clause as compile-time text.
///
/// Shared by collection and checking so the two cannot disagree about what counts as a description. Collection uses
/// it to decide what to store; checking uses it to decide whether the clause is acceptable. When the two carried
/// separate rules, `description = 42` satisfied checking and stored nothing, leaving a declaration that looked
/// documented while carrying no description at all.
///
/// The text has to exist at compile time because the audience is a person deciding whether to grant the capability.
/// There is no later moment at which that review happens, so an expression to be evaluated at runtime cannot serve.
pub(super) fn capability_description_text(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Literal(Literal::String(text)) => Some(text.as_str()),
        _ => None,
    }
}

/// Flatten a dotted reference expression into its path segments.
///
/// A capability's `requires` entries parse as ordinary expressions so they can be checked symbol references rather
/// than strings. Only a bare name or a chain of field accesses spells a reference; anything else — a call, a literal,
/// a subscript — is not a path, and returning `None` lets the caller report it against the entry's own span.
pub(super) fn dotted_path_segments(expr: &Expr) -> Option<Vec<String>> {
    match expr {
        Expr::Ident(name) => Some(vec![name.clone()]),
        Expr::Field(base, name) => {
            let mut segments = dotted_path_segments(&base.node)?;
            segments.push(name.clone());
            Some(segments)
        }
        _ => None,
    }
}

type InheritedMembers = (
    HashMap<String, FieldInfo>,
    HashMap<String, Spanned<Expr>>,
    HashMap<String, crate::library_exports::CheckedParamDefault>,
    HashMap<String, String>,
    Vec<String>,
    HashMap<String, PropertyInfo>,
    HashMap<String, MethodInfo>,
    HashMap<String, Vec<MethodInfo>>,
);

pub(in crate::typechecker) type PartialCallableSignature = (
    Vec<CallableParam>,
    ResolvedType,
    bool,
    Vec<String>,
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<TypeBoundInfo>>,
);

/// Extract constrained-primitive predicates from a newtype underlying annotation.
fn newtype_constraints(ty: &Type) -> Vec<NewtypePrimitiveConstraint> {
    let Type::ConstrainedPrimitive(_, constraints) = ty else {
        return Vec::new();
    };
    constraints
        .iter()
        .map(|constraint| NewtypePrimitiveConstraint {
            key: constraint.node.key,
            value: constraint.node.value.value,
            repr: constraint.node.value.repr.clone(),
        })
        .collect()
}

/// Return whether a newtype declaration permits RFC 017 implicit coercion.
fn newtype_allows_implicit_coercion(decorators: &[Spanned<Decorator>]) -> bool {
    !decorators.iter().any(|decorator| {
        core_decorators::from_segments(&decorator.node.path.segments) == Some(DecoratorId::NoImplicitCoercion)
    })
}

/// Convert parsed value enum backing syntax into symbol-table metadata.
fn value_enum_backing(value_type: ValueEnumType) -> ValueEnumBacking {
    match value_type {
        ValueEnumType::Str => ValueEnumBacking::Str,
        ValueEnumType::Int => ValueEnumBacking::Int,
    }
}

/// Convert parsed value enum raw literals into symbol-table metadata.
fn value_enum_value_from_literal(value: &ValueEnumLiteral) -> ValueEnumValue {
    match value {
        ValueEnumLiteral::Str(value) => ValueEnumValue::Str(value.clone()),
        ValueEnumLiteral::Int(value) => ValueEnumValue::Int(value.value),
    }
}
