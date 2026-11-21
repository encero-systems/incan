//! Per-instantiation decorator bindings and their cached frames.

use incan_semantics_core::body_ir::{BodyIrModule, DecoratedBinding, DecoratedInitialization};

use crate::classes::body_module_identity;

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

/// The source role of a frame: the normal callable, a decorated binding's original body, or its initializer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecoratorFrame {
    Normal,
    Original,
    Initializer,
}

/// Give authenticated hidden frames distinct symbols while retaining the normal closed instance's key.
pub fn frame_name(base: String, role: DecoratorFrame) -> String {
    match role {
        DecoratorFrame::Normal => base,
        DecoratorFrame::Original => format!("{base}__incan_original"),
        DecoratorFrame::Initializer => format!("{base}__incan_initializer"),
    }
}

/// Bound dynamic name lookup to its registration module independently of signature and cache lifetime policy.
pub fn callable_name_profile(module: &BodyIrModule, owner: &str) -> Result<bool, String> {
    for binding in &module.decorated_bindings {
        if binding.initializer.is_some() && body_module_identity(&binding.original_body)? != owner {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Name the frame that publishes a closed instance once and returns its decorated callable.
pub fn binding_accessor_name(base: &str) -> String {
    format!("{base}__incan_binding")
}

/// The pointer signature of the frame being built, read from its parameter slots and then its return slot.
pub fn builder_signature(builder: &crate::frame::Builder) -> Vec<incan_mir_plan_native::PlanType> {
    let mut signature: Vec<_> = (1..=builder.parameter_count as usize)
        .map(|index| builder.locals[index].ty.clone())
        .collect();
    signature.push(builder.locals[0].ty.clone());
    signature
}
