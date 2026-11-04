//! Member visibility at a use site: the type-private rule fields and methods share, and method calls (#884, #2570).

use super::*;

impl TypeChecker {
    /// Whether a member private to the type named `owner` is out of reach here.
    ///
    /// Only the methods of the type that declares the member, in the module that declares it, may use it; a subclass's
    /// methods do not reach the private members it inherits. A caller's same-spelled type grants no rights to a foreign
    /// member, and a member without declaration identity cannot prove where it was declared.
    pub(in crate::typechecker::check_expr) fn type_private_member_is_inaccessible(
        &self,
        owner: &str,
        identity: Option<&CanonicalSymbolId>,
    ) -> bool {
        if self.current_method_owner.as_deref() != Some(owner) {
            return true;
        }
        match (identity, self.symbols.module_type_identity(owner)) {
            (Some(member), Some(owner)) => member.origin != owner.origin,
            _ => true,
        }
    }

    /// Refuse a call to a method the calling context cannot see.
    ///
    /// A public method is callable wherever its type is. A method private to its type follows the rule its fields do
    /// ([`Self::type_private_member_is_inaccessible`]). A method that takes the visibility of a trait that is not `pub`
    /// is callable anywhere in the trait's module.
    pub(in crate::typechecker::check_expr) fn refuse_inaccessible_method(
        &mut self,
        method: &str,
        info: &MethodInfo,
        receiver_ty: &ResolvedType,
        span: Span,
    ) {
        let accessible = match info.scope {
            MethodScope::Public => true,
            MethodScope::Module => info
                .identity
                .as_ref()
                .is_some_and(|identity| identity.origin == self.symbols.declaration_origin()),
            MethodScope::Type => info
                .owner
                .as_deref()
                .is_some_and(|owner| !self.type_private_member_is_inaccessible(owner, info.identity.as_ref())),
        };
        if accessible {
            return;
        }
        let type_name = info.owner.clone().unwrap_or_else(|| receiver_ty.to_string());
        self.errors.push(match info.scope {
            MethodScope::Module => errors::module_private_method(&type_name, method, span),
            MethodScope::Type | MethodScope::Public => errors::private_method(&type_name, method, span),
        });
    }
}
