//! Per-instantiation decorator bindings and their cached frames.

use incan_semantics_core::body_ir::{DecoratedBinding, DecoratedInitialization};

/// Admit a synchronous per-instantiation binding with its initializer; its frames check the closed signature.
pub fn validate_instance_binding(binding: &DecoratedBinding) -> Result<(), String> {
    if binding.initialization != DecoratedInitialization::PerInstantiation {
        return Err("unsupported Body IR cached decorator outside per-instantiation policy".into());
    }
    let body = &binding.original_body;
    if body.is_async || body.is_generator() {
        return Err("unsupported Body IR generic decorator on an async or generator callable".into());
    }
    if binding.initializer.is_none() {
        return Err("unsupported Body IR generic decorator without a per-instantiation initializer".into());
    }
    Ok(())
}

/// Honor the kernel-validated lifetime and complete initializer field without inferring a replacement from storage.
pub fn validate_binding_policy(binding: &DecoratedBinding) -> Result<(), String> {
    match binding.initialization {
        DecoratedInitialization::PerInstantiation => validate_instance_binding(binding),
        DecoratedInitialization::PerInvocation => {
            Err("unsupported Body IR retired per-invocation decorator lifetime".into())
        }
        DecoratedInitialization::ModuleStorage => {
            if !binding.original_body.type_parameters.is_empty() || binding.initializer.is_some() {
                return Err("unsupported Body IR module decorator lifetime".into());
            }
            Ok(())
        }
    }
}
