//! Resolve generated typed-value calls through the primitive trait's checked public signature.
//!
//! The frontend selects ownership and generic call facts only. Incan lowering generates the body;
//! no codec or stored-format interpretation enters this boundary (RFC 173, #2519).

use super::*;
use crate::typechecker::typed_value_protocol::ValueProtocol;

impl TypeChecker {
    /// Resolve an automatic hook only when no authored hook replaces the stored representation. The
    /// published primitive trait owns its generic signature, parameter borrowing and return carrier.
    pub(super) fn check_typed_value_member_call(
        &mut self,
        call: &MethodCallSite<'_>,
        receiver: &ResolvedType,
        arguments: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let (trait_name, protocol) = match call.method {
            "__shape__" | "__walk__" => ("Walk", ValueProtocol::Walk),
            "__reconstruct__" => ("Reconstruct", ValueProtocol::Reconstruct),
            _ => return None,
        };
        let receiver = self.expand_type_aliases(receiver.clone());
        if self.active_type_param_name(&receiver).is_some() {
            // An open parameter uses its explicit ordinary trait bound and retains the selected slot.
            return None;
        }
        if let ResolvedType::Named(name) | ResolvedType::Generic(name, _) = &receiver {
            let methods = match self.lookup_semantic_type_info(name) {
                Some(TypeInfo::Model(info)) => Some(&info.methods),
                Some(TypeInfo::Enum(info)) => Some(&info.methods),
                Some(TypeInfo::Newtype(info)) if !info.is_rusttype => Some(&info.methods),
                // Builtin carriers have no authored methods to replace the protocol. Their exact
                // constructor and payload admission is checked below, as for a bounded call.
                Some(TypeInfo::Builtin) | None => None,
                _ => return None,
            };
            if methods.is_some_and(|methods| {
                ["__shape__", "__walk__", "__reconstruct__"]
                    .iter()
                    .any(|name| methods.contains_key(*name))
            }) {
                return None;
            }
        }
        let module = vec!["std".to_owned(), "primitives".to_owned(), "reflection".to_owned()];
        let identity = self.loaf_member_identity(&module, trait_name)?;
        let SymbolOrigin::Package { library, module_path } = &identity.origin else {
            return None;
        };
        if library != "incan_stdlib_primitives" || module_path.as_slice() != ["primitives", "reflection"] {
            return None;
        }
        let (library, path) = self.loaf_module(&module)?;
        let (SymbolKind::Trait(info), _) = self.lookup_pub_library_module_symbol_member(&library, &path, trait_name)?
        else {
            return None;
        };
        let method = info.methods.get(call.method)?.clone();
        let bound = TypeBoundInfo {
            name: trait_name.to_owned(),
            source_name: Some(trait_name.to_owned()),
            type_args: Vec::new(),
            module_path: Some(module),
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        if !self.typed_value_protocol_support(&receiver, &bound, protocol, &mut Vec::new()) {
            self.errors
                .push(errors::type_mismatch(trait_name, &receiver.to_string(), call.span));
            return Some(ResolvedType::Unknown);
        }
        // Keeping the authenticated trait slot lets direct and bounded calls use the same native dispatch.
        Some(self.check_resolved_generic_method_call(
            call.method,
            method,
            call.type_args,
            call.args,
            arguments,
            call.span,
            &receiver,
            call.expected_return_ty,
        ))
    }
}
