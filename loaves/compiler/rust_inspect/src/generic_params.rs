//! Canonical projection of Rust source generic declarations into interop metadata.

use incan_core::interop::RustMutableReferenceTypeParam;
use ra_ap_syntax::ast::{self, HasName};

/// Generic facts retained for an associated-function receiver.
#[derive(Debug, Default)]
pub(crate) struct SourceOwnerGenerics {
    /// Type parameters in declaration order, with lifetimes elided.
    pub(crate) type_params: Vec<String>,
    /// Written default type arguments aligned with `type_params`.
    pub(crate) type_param_defaults: Vec<Option<String>>,
    /// Type parameters whose ownership lowering requires complete HIR inspection.
    ///
    /// Syntax-only metadata intentionally leaves this empty. It cannot establish whether a bound trait has a
    /// generic mutable-reference implementation in another module or crate.
    pub(crate) mutable_reference_type_params: Vec<RustMutableReferenceTypeParam>,
    /// Whether the declaration contains a const parameter that Incan cannot specialize yet.
    pub(crate) has_const_params: bool,
}

/// Project one Rust generic declaration into the receiver-specialization contract.
///
/// Rust lifetimes are deliberately elided because they do not occupy explicit turbofish positions. Const parameters do
/// occupy positions, so callers must retain their presence and reject receiver specialization rather than shifting a
/// later type argument into the wrong Rust slot.
pub(crate) fn source_owner_generics(params: Option<ast::GenericParamList>) -> SourceOwnerGenerics {
    let mut projected = SourceOwnerGenerics::default();
    for param in params.into_iter().flat_map(|params| params.generic_params()) {
        match param {
            ast::GenericParam::TypeParam(param) => {
                if let Some(name) = param.name() {
                    projected.type_params.push(name.text().to_string());
                    projected
                        .type_param_defaults
                        .push(param.default_type().map(|ty| ty.to_string()));
                }
            }
            ast::GenericParam::ConstParam(_) => projected.has_const_params = true,
            ast::GenericParam::LifetimeParam(_) => {}
        }
    }
    projected
}
