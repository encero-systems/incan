//! Lowering an admitted provider-service call into a checked [`bir::ProviderOperationPlan`] (#1213).
//!
//! Body IR does not decide what a provider operation *is*. It is told, by canonical identity, through a
//! private [`ProviderOperationCatalog`] projected from the selected [`ProviderPlan`]. That direction is the point of
//! this module rather than an implementation convenience: a lowering pass that recognized provider operations by
//! module name, callee spelling, or emitted Rust name would be a second source-resolution mechanism competing with
//! the one that already resolved the call, and it would make ordinary package-defined providers impossible to
//! express.
//!
//! What lowering does own is admission. An operation reaches [`bir::Callee::ProviderOperation`] only when its
//! provider is active in this compilation, its required capability identity really names a capability declaration,
//! and every declared input is present for evaluation at the call site. Anything else refuses at the original source
//! span through the ordinary [`bir::StatementKind::Unsupported`] node, so an operation that cannot be executed produces
//! no plan — and therefore nothing that could emit an execution receipt.
//!
//! Authority is deliberately absent here. Importing a provider module is ordinary source resolution and grants
//! nothing; the authority question is asked only when an admitted operation is invoked, and asking it is the
//! executing consumer's job through [`bir::ProviderOperationPlan::authority_request`] (#1156). This module produces
//! the plan that makes that question answerable without re-reading source.

use std::collections::BTreeMap;

use super::args::*;
use super::refusals::*;
use super::*;
use crate::provider::ProviderPlan;

/// One checked provider operation the compiler will admit for execution, keyed elsewhere by canonical identity.
///
/// Every field is metadata this stage *consumes*. The provider and its activation come from the checked provider
/// catalog, and `required_capability` is an RFC 104 `capability` declaration's own canonical identity — not a grant
/// spelling and not a provider-local authority model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProviderOperationRecord {
    /// The checked provider owning the operation, and whether this compilation can execute against it.
    pub provider: bir::ProviderActivation,
    /// Canonical identity of the capability an invocation's authority must be decided against.
    pub required_capability: CanonicalSymbolId,
    /// Runtime requirements executing the operation imposes.
    pub runtime_requirements: Vec<AbiV0RuntimeRequirement>,
}

/// The provider operations this compilation knows about, keyed by the operation's canonical identity.
///
/// Keying on [`CanonicalSymbolId`] is what keeps the lookup free of source spellings: a local call, an import, and
/// an alias of one operation all present the same key, and no provider name, module spelling, or emitted Rust name
/// participates. An empty catalog is the ordinary case — a compilation with no provider operations lowers exactly
/// as it did before, because no call can match an entry that does not exist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ProviderOperationCatalog {
    operations: BTreeMap<CanonicalSymbolId, ProviderOperationRecord>,
}

impl ProviderOperationCatalog {
    /// Build an empty catalog, which admits no call as a provider operation.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Build the lowering catalogue from the compilation's checked provider plan.
    ///
    /// Provider manifests are the only producer of these entries. This constructor deliberately takes no source
    /// spelling inputs: selected provider records already carry the integrity-checked manifest and activation facts
    /// for the current compilation.
    pub(super) fn from_provider_plan(provider_plan: &ProviderPlan) -> Result<Self, String> {
        let mut catalog = Self::new();
        for provider in provider_plan.records() {
            let Some(manifest) = provider.manifest.as_deref() else {
                continue;
            };
            let state = if provider.enabled && provider.available {
                bir::ProviderActivationState::Active
            } else if provider.enabled {
                bir::ProviderActivationState::Unavailable
            } else {
                bir::ProviderActivationState::Disabled
            };
            for descriptor in &manifest.contract_metadata.provider.operation_descriptors {
                let Some(module_path) = descriptor.operation.module_path() else {
                    return Err(format!(
                        "provider `{}` publishes an operation without a module declaration identity",
                        provider.identity.stable_key()
                    ));
                };
                if !provider.namespace_claims.contains(module_path) {
                    return Err(format!(
                        "provider `{}` publishes operation `{}` outside its claimed module namespace",
                        provider.identity.stable_key(),
                        descriptor.operation.declaration_name
                    ));
                }
                if !descriptor.has_expected_kinds() {
                    return Err(format!(
                        "provider `{}` publishes an operation descriptor with unsupported declaration kinds",
                        provider.identity.stable_key()
                    ));
                }
                catalog.try_insert(
                    descriptor.operation.clone(),
                    ProviderOperationRecord {
                        provider: bir::ProviderActivation {
                            provider_key: provider.identity.stable_key(),
                            module_path: module_path.to_vec(),
                            state,
                        },
                        required_capability: descriptor.required_capability.clone(),
                        runtime_requirements: descriptor.runtime_requirements.clone(),
                    },
                )?;
            }
        }
        Ok(catalog)
    }

    /// Insert one checked descriptor, refusing duplicate canonical operation identities.
    ///
    /// A duplicate cannot be resolved by choosing whichever provider happened to be visited last. That would turn a
    /// checked provider plan into a mutable, order-dependent authority source, so callers receive an error before
    /// any Body IR is built.
    fn try_insert(&mut self, operation: CanonicalSymbolId, record: ProviderOperationRecord) -> Result<(), String> {
        if self.operations.contains_key(&operation) {
            return Err(format!(
                "duplicate provider operation identity `{}`",
                operation.declaration_name
            ));
        }
        self.operations.insert(operation, record);
        Ok(())
    }

    /// Return the record for `operation`, or `None` when this compilation knows no such provider operation.
    pub(super) fn get(&self, operation: &CanonicalSymbolId) -> Option<&ProviderOperationRecord> {
        self.operations.get(operation)
    }
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Return the provider-operation record a resolved call selected, when the catalog admits one.
    ///
    /// The borrow is taken from the catalog's own `'source` lifetime rather than from `&self`, so a caller can keep
    /// the record while lowering argument expressions through `&mut self`.
    ///
    /// A caller with no proven canonical identity never reaches here at all. That is fail-closed on purpose: an
    /// unresolved, trait-dispatched, or ambiguously overloaded callee has no identity to look up, and guessing one
    /// from the call-site spelling is precisely the source-resolution duplication this slice exists to avoid.
    pub(super) fn provider_operation_record(
        &self,
        operation: &CanonicalSymbolId,
    ) -> Option<&'source ProviderOperationRecord> {
        self.provider_operations.get(operation)
    }

    /// Lower an admitted provider-operation call into a [`bir::Callee::ProviderOperation`] statement.
    ///
    /// The admission checks run *before* any argument expression is lowered, matching the "check before partially
    /// lowering" precedent this module's siblings already follow: a refusal must not leave the operands of a call
    /// that never happens sitting in the emitted statement list.
    ///
    /// `operation` is the canonical identity the call resolved to, so the emitted plan names the same declaration
    /// the typechecker selected rather than the spelling written here. `name` is used only for refusal text.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_provider_operation(
        &mut self,
        name: &str,
        operation: &CanonicalSymbolId,
        record: &ProviderOperationRecord,
        declaration: DirectCallDeclaration,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        if let Some(description) = unsupported_provider_operation(operation, record) {
            return self.unsupported_operand(description, scope, hir_span_value, out);
        }

        // A provider operation must arrive with a fully resolved declared surface. Without one there is no slot
        // space for its inputs to be described against, so there is nothing a consumer could bind a scope value to.
        let Some(slots) = declaration.slots else {
            return self.unsupported_operand(
                format!("provider operation `{name}` whose declared parameters were not resolved"),
                scope,
                hir_span_value,
                out,
            );
        };
        let callee_label = format!("provider operation `{name}`");
        let planned = match plan_declared_args(&callee_label, &slots, args) {
            Ok(planned) => planned,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };

        // Every declared slot must carry an evaluated input. An omitted default is legal for an ordinary call, whose
        // default computation stays owned by the declaration, but a provider operation's plan claims to describe the
        // values its execution will actually see -- and a default this stage never evaluated is not one of them.
        if planned.len() != slots.len() {
            return self.unsupported_operand(
                format!("provider operation `{name}` called with an omitted parameter whose default this stage cannot evaluate"),
                scope,
                hir_span_value,
                out,
            );
        }

        let inputs = self.provider_operation_inputs(&planned);
        let (operands, _binding) = match self.lower_planned_args(&planned, slots.len(), scope, out) {
            Ok(bound) => bound,
            Err(description) => {
                return self.unsupported_operand(format!("{callee_label}: {description}"), scope, hir_span_value, out);
            }
        };

        // The operation's requirements are the enclosing body's requirements too: a body that invokes it cannot run
        // on a profile that does not provide them.
        for requirement in &record.runtime_requirements {
            self.record_runtime_requirement(requirement.clone());
        }

        let plan = bir::ProviderOperationPlan {
            operation: operation.clone(),
            provider: record.provider.clone(),
            required_capability: record.required_capability.clone(),
            runtime_requirements: record.runtime_requirements.clone(),
            inputs,
            call_span: hir_span_value,
        };
        let ty = self.resolve_ty(span);
        self.push_call_temp(
            bir::Callee::ProviderOperation(Box::new(plan)),
            fixed_elements(operands),
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Describe each planned argument as an evaluated input fact, in written source order.
    ///
    /// The written position is the index into `planned`, which [`Self::lower_planned_args`] also uses as its
    /// evaluation order, so an input's recorded position really is when its value was computed. Types come from the
    /// typechecker's own decision for the argument expression rather than from the declared parameter, because what
    /// an authority check needs to see is the value that was actually passed.
    fn provider_operation_inputs(
        &self,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
    ) -> Vec<bir::ProviderOperationInput> {
        planned
            .iter()
            .enumerate()
            .map(|(written_position, (slot, expr))| bir::ProviderOperationInput {
                slot: *slot,
                written_position,
                ty: self.resolve_ty(expr.span),
                span: hir_span(expr.span),
            })
            .collect()
    }
}
