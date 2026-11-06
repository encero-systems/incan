//! One-pass collection of the module-local facts lowering retains: defaults, declarations, and canonical member
//! layouts.

use super::*;
use crate::typechecker::ClassLayoutInfo;

mod callable_facts;
mod constants_and_statics;
mod nominal_declarations;
mod trait_implementations;

pub(super) use callable_facts::*;
pub(super) use constants_and_statics::*;
pub(super) use nominal_declarations::*;
pub(super) use trait_implementations::*;

/// The source-owned return of one checked nongeneric method, indexed without resolving a consumer spelling.
#[derive(Clone, PartialEq)]
pub(super) struct PublishedNewtypeReturn {
    /// The providing frame's checked return carrier, with its own canonical type binding.
    pub(super) ty: IncanType,
    /// Exact checked storage selected by that return binding; a consumer must retain this same declaration.
    pub(super) declaration: bir::NominalDeclaration,
}

/// One source-local trait an admitted adopter implements, at the instantiation the checker recorded for it.
struct AdoptedTrait<'program> {
    declaration: &'program ast::TraitDecl,
    /// Checked type arguments, one per trait type parameter.
    arguments: Vec<ResolvedType>,
    /// Supertraits this direct adoption reaches, by name; `None` for a trait reached only as a supertrait.
    direct_ancestors: Option<Vec<String>>,
}

impl AdoptedTrait<'_> {
    /// Bind the trait's type parameters to this instantiation's arguments for its default bodies.
    fn type_arguments(&self) -> Vec<bir::TraitTypeArgument> {
        self.declaration
            .type_params
            .iter()
            .zip(&self.arguments)
            .map(|(parameter, argument)| bir::TraitTypeArgument {
                parameter: parameter.name.clone(),
                argument: semantic_type_from_resolved(argument),
            })
            .collect()
    }
}
