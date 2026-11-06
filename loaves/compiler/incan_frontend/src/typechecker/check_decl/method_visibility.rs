//! Declaration-level method visibility rules (#2570).

use super::super::collect::method_visibility::{HookOwner, hook_owner, is_language_hook_method};
use super::*;

impl TypeChecker {
    /// Refuse `pub` on a method whose visibility its declaration does not decide.
    ///
    /// A method that implements a method of an adopted trait takes the trait's visibility, as a method declared in the
    /// trait does. It implements a trait method when it names a trait target (`for Display`) or shares its name with a
    /// method of an adopted trait or one of its supertraits. A language hook, which the language or generated code
    /// calls by its name, is public by rule.
    pub(super) fn refuse_pub_on_trait_implementations(
        &mut self,
        type_name: &str,
        traits: &[Spanned<TraitBound>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let mut marked = methods
            .iter()
            .filter(|method| matches!(method.node.visibility, Visibility::Public))
            .peekable();
        if marked.peek().is_none() {
            return;
        }
        let owner = self.lookup_type_info(type_name).map(hook_owner).unwrap_or(HookOwner {
            is_newtype: false,
            derives_validate: false,
        });
        // A subclass also implements the traits its ancestors adopt.
        let extends = match self.lookup_type_info(type_name) {
            Some(TypeInfo::Class(info)) => info.extends.clone(),
            _ => None,
        };
        let mut trait_names = traits
            .iter()
            .map(|trait_ref| trait_ref.node.name.clone())
            .collect::<Vec<_>>();
        trait_names.extend(self.ancestor_trait_names(extends.as_deref()));
        let origins = self.trait_method_origins(trait_names.iter().map(String::as_str));
        for method in marked {
            let name = method.node.name.as_str();
            if is_language_hook_method(name, owner) {
                self.errors.push(errors::pub_modifier_not_allowed_on_language_hook(
                    type_name,
                    name,
                    method.span,
                ));
                continue;
            }
            let trait_name = match &method.node.trait_target {
                Some(target) => Some(target.node.name.as_str()),
                None => origins.get(name).map(String::as_str),
            };
            if let Some(trait_name) = trait_name {
                self.errors
                    .push(errors::pub_modifier_not_allowed_on_trait_implementation(
                        type_name,
                        trait_name,
                        name,
                        method.span,
                    ));
            }
        }
    }
}
