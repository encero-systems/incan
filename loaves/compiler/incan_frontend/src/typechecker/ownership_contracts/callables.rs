//! Callable-value inputs retain their invocation access independently of their enclosing value's ownership.

use crate::ownership_metadata::{AccessMode, CheckedOwnershipContract, TypeComponent};
use crate::symbols::{CallableParam, ResolvedType};
use crate::typechecker::TypeCheckInfo;
use incan_lang::lang::types::ownership::{self, OwnershipMode};
use incan_semantics_core::IncanType;

impl TypeCheckInfo {
    /// Project only outer callable input annotations. Nominal qualification precedes mode selection so a user type
    /// named like a builtin never becomes an ownership request. The original callable type remains unchanged.
    pub(in crate::typechecker) fn callable_value_parameters(
        &self,
        parameters: &[CallableParam],
    ) -> Result<(Vec<CallableParam>, Option<CheckedOwnershipContract>), &'static str> {
        let mut values = parameters.to_vec();
        let mut modes = Vec::new();
        for (slot, parameter) in parameters.iter().enumerate() {
            let IncanType::Generic { base, args } = self.semantic_type(&parameter.ty) else {
                continue;
            };
            let Some(mode) = ownership::from_str(&base) else {
                continue;
            };
            let ResolvedType::Generic(_, underlying) = &parameter.ty else {
                return Err("callable ownership input has no checked generic type");
            };
            if args.len() != 1 || underlying.len() != 1 || mode == OwnershipMode::Reference {
                return Err("callable input has an unsupported ownership contract");
            }
            values[slot].ty = underlying[0].clone();
            modes.push(AccessMode {
                mode,
                component: TypeComponent {
                    parameter: Some(slot),
                    children: Vec::new(),
                },
                underlying: underlying[0].clone(),
            });
        }
        let contract = (!modes.is_empty()).then_some(CheckedOwnershipContract {
            schema_version: 1,
            modes,
            result_source: None,
        });
        Ok((values, contract))
    }
}
