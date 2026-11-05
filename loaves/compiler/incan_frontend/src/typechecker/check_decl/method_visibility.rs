//! Declaration-level method visibility rules (#2570).

use super::*;

impl TypeChecker {
    /// Refuse `pub` on a method that implements a method of an adopted trait. Such a method takes the trait's
    /// visibility, as a method declared in the trait does. A method implements a trait method when it names a trait
    /// target (`for Display`) or shares its name with a method of an adopted trait or one of its supertraits.
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
        let origins = self.trait_method_origins(traits.iter().map(|trait_ref| trait_ref.node.name.as_str()));
        for method in marked {
            let trait_name = match &method.node.trait_target {
                Some(target) => Some(target.node.name.as_str()),
                None => origins.get(&method.node.name).map(String::as_str),
            };
            if let Some(trait_name) = trait_name {
                self.errors
                    .push(errors::pub_modifier_not_allowed_on_trait_implementation(
                        type_name,
                        trait_name,
                        &method.node.name,
                        method.span,
                    ));
            }
        }
    }
}
