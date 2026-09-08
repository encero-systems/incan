//! Preserve receiver lifetime evidence before Rust display normalization erases lifetime labels.

use incan_core::interop::RustReceiverContract;
use ra_ap_syntax::{AstNode, ast};

/// Recover only ordinary shared receivers and output references tied to that receiver by source lifetime rules.
/// Typed/arbitrary receivers, async functions, mutable receivers, and foreign output lifetimes fail closed.
pub(crate) fn receiver_contract(function: &ast::Fn) -> Option<RustReceiverContract> {
    let receiver = function.param_list()?.self_param()?;
    if receiver.colon_token().is_some() || function.async_token().is_some() || function.unsafe_token().is_some() {
        return None;
    }
    let shared = receiver.amp_token().is_some() && receiver.mut_token().is_none();
    let receiver_lifetime = receiver.lifetime().map(|lifetime| lifetime.to_string());
    let another_input_shares_lifetime = receiver_lifetime.as_ref().is_some_and(|lifetime| {
        function
            .param_list()
            .into_iter()
            .flat_map(|params| params.params())
            .any(|param| {
                param
                    .syntax()
                    .descendants()
                    .filter_map(ast::Lifetime::cast)
                    .any(|other| other.to_string() == *lifetime)
            })
    });
    let output_refs = function
        .ret_type()
        .and_then(|ret| ret.ty())
        .map(|ty| {
            ty.syntax()
                .descendants()
                .filter_map(ast::RefType::cast)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let returns_receiver_borrow = shared
        && receiver_lifetime.as_deref() != Some("'static")
        && !another_input_shares_lifetime
        && !output_refs.is_empty()
        && output_refs.iter().all(|reference| {
            if reference.mut_token().is_some() {
                return false;
            }
            match reference.lifetime().map(|lifetime| lifetime.to_string()) {
                None => true,
                Some(lifetime) if lifetime == "'_" => true,
                Some(lifetime) => receiver_lifetime.as_ref() == Some(&lifetime) && lifetime != "'static",
            }
        });
    Some(RustReceiverContract {
        shared,
        returns_receiver_borrow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract source facts without loading a workspace or consulting normalized type displays.
    fn contract(source: &str) -> Result<RustReceiverContract, String> {
        let parsed = ra_ap_syntax::SourceFile::parse(source, ra_ap_syntax::Edition::CURRENT);
        let function = parsed
            .syntax_node()
            .descendants()
            .find_map(ast::Fn::cast)
            .ok_or_else(|| "missing fixture function".to_string())?;
        receiver_contract(&function).ok_or_else(|| "missing receiver contract".to_string())
    }

    #[test]
    fn shared_child_uses_receiver_elision_even_with_a_borrowed_key() -> Result<(), String> {
        assert_eq!(
            contract("fn get(&self, key: &str) -> Option<&Item> {}")?,
            RustReceiverContract {
                shared: true,
                returns_receiver_borrow: true
            }
        );
        Ok(())
    }

    #[test]
    fn output_from_another_argument_is_not_a_receiver_descendant() -> Result<(), String> {
        assert!(!contract("fn get<'a>(&self, other: &'a Item) -> Option<&'a Item> {}")?.returns_receiver_borrow);
        assert!(contract("fn get<'a>(&'a self, key: &str) -> Option<&'a Item> {}")?.returns_receiver_borrow);
        assert!(!contract("fn get(&self) -> &'static Item {}")?.returns_receiver_borrow);
        assert!(!contract("fn get<'a>(&'a self, other: &'a Item) -> &'a Item {}")?.returns_receiver_borrow);
        Ok(())
    }

    #[test]
    fn consuming_and_mutable_receivers_do_not_prove_shared_access() -> Result<(), String> {
        assert!(!contract("fn into_item(self) -> Item {}")?.shared);
        assert!(!contract("fn get_mut(&mut self) -> Option<&mut Item> {}")?.shared);
        Ok(())
    }
}
