//! Producer-side public-closure audit over checked Body IR. No source is checked or resolved here.

use std::collections::{BTreeMap, BTreeSet};

use crate::body_ir::{
    AggregateKind, ArgumentElement, AssertionKind, Body, BodyIrModule, CallableParam, CallableParamDefault,
    CallableTarget, Callee, DictEntry, FieldlessEnumVariantTarget, FormatPart, Operand, Pattern, Place, PlaceElem,
    PlaceRoot, Rvalue, Statement, StatementKind, TryErrorRouting,
};
use crate::{CanonicalSymbolId, CompilerNodeId, IncanType, SymbolOrigin};

use super::CoverageReason;
use crate::canonical_module_identity;

/// Executable dependencies retained without exposing private or unresolved source declarations.
pub(super) struct PublicBody {
    pub body: Body,
    pub requirements: BTreeSet<CanonicalSymbolId>,
}

/// Bind a declaration's physical fragment address to the already-published canonical owner and span.
pub(super) fn declaration_id(identity: &CanonicalSymbolId) -> Result<CompilerNodeId, CoverageReason> {
    let module = canonical_module_identity(identity).ok_or(CoverageReason::UnresolvedReference)?;
    Ok(CompilerNodeId::declaration_span(
        &module,
        identity.declaration_span.start,
        identity.declaration_span.end,
    ))
}

/// Project one public body onto package-owned addresses and audit every retained computation and type.
///
/// Local slots and scope ordinals are fragment-local, not compiler-session symbol IDs. Their optional checker
/// identities are removed because execution uses the slots and a local identity may contain a session discriminant.
/// Declaration references keep the canonical identity minted by the declaring compilation.
pub(super) fn project_body(
    source: &Body,
    module: &BodyIrModule,
    library: &str,
    public: &BTreeSet<CanonicalSymbolId>,
) -> Result<PublicBody, CoverageReason> {
    let mut body = source.clone();
    let mut audit = PublicationAudit {
        library,
        public,
        nominal_types: BTreeMap::new(),
        requirements: BTreeSet::new(),
    };
    for declaration in &module.nominal_declarations {
        audit
            .nominal_types
            .insert(declaration.name.clone(), declaration.canonical.clone());
    }
    for declaration in &module.fieldless_enum_declarations {
        audit
            .nominal_types
            .insert(declaration.name.clone(), declaration.canonical.clone());
    }
    for declaration in &module.value_enum_declarations {
        audit
            .nominal_types
            .insert(declaration.name.clone(), declaration.canonical.clone());
    }
    let canonical = body.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
    audit.identity(canonical, false)?;
    body.decl_id = declaration_id(canonical)?;
    body.direct_call_id = body.decl_id.clone();
    audit.ty(&body.return_type)?;
    for local in &mut body.locals {
        local.identity = None;
        audit.ty(&local.ty)?;
    }
    audit.parameters(&mut body.params)?;
    audit.statements(&mut body.block.stmts)?;
    Ok(PublicBody {
        body,
        requirements: audit.requirements,
    })
}

/// One exhaustive pass owns publication's type/reference checks and artifact-local address projection.
struct PublicationAudit<'a> {
    library: &'a str,
    public: &'a BTreeSet<CanonicalSymbolId>,
    nominal_types: BTreeMap<String, CanonicalSymbolId>,
    requirements: BTreeSet<CanonicalSymbolId>,
}

impl PublicationAudit<'_> {
    /// Rebase only producer-local origins; a foreign reexport retains its declaring package identity.
    fn identity(&mut self, identity: &mut CanonicalSymbolId, required: bool) -> Result<(), CoverageReason> {
        if let SymbolOrigin::Module(module_path) = &identity.origin {
            identity.origin = SymbolOrigin::Package {
                library: self.library.to_owned(),
                module_path: module_path.clone(),
            };
        }
        if identity.scope_discriminant.is_some() {
            return Err(CoverageReason::UnresolvedReference);
        }
        match &identity.origin {
            SymbolOrigin::Package { library, .. } => {
                if library == self.library && !self.public.contains(identity) {
                    return Err(CoverageReason::PrivateDependency);
                }
                if required {
                    self.requirements.insert(identity.clone());
                }
                Ok(())
            }
            SymbolOrigin::Builtin => Ok(()),
            SymbolOrigin::RustCrate(_) | SymbolOrigin::Module(_) => Err(CoverageReason::UnsupportedConstruct),
        }
    }

    /// Admit concrete semantic types and retain the canonical declaration behind a local nominal type.
    fn ty(&mut self, ty: &IncanType) -> Result<(), CoverageReason> {
        match ty {
            IncanType::Primitive(_) | IncanType::Never | IncanType::Decimal { .. } => Ok(()),
            IncanType::Named(name) => {
                let mut identity = self
                    .nominal_types
                    .get(name)
                    .cloned()
                    .ok_or(CoverageReason::UnresolvedReference)?;
                self.identity(&mut identity, true)
            }
            IncanType::Generic { base, args } => {
                // These are the semantic model's compiler-owned generic constructors. User generic declarations
                // lack canonical type arguments in this version and stay explicitly uncovered.
                if incan_core::lang::types::collections::from_str(base).is_none() {
                    return Err(CoverageReason::UnresolvedReference);
                }
                for arg in args {
                    self.ty(arg)?;
                }
                Ok(())
            }
            IncanType::Tuple(items) => {
                for item in items {
                    self.ty(item)?;
                }
                Ok(())
            }
            IncanType::Function { params, return_type } => {
                for param in params {
                    self.ty(&param.ty)?;
                }
                self.ty(return_type)
            }
            IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => self.ty(inner),
            IncanType::TypeVar(_)
            | IncanType::SelfType
            | IncanType::RustInteropPath(_)
            | IncanType::Infer
            | IncanType::Unknown => Err(CoverageReason::UnresolvedReference),
        }
    }

    /// Defaults are declaration-owned computations, including defaults nested in closure parameters.
    fn parameters(&mut self, parameters: &mut [CallableParam]) -> Result<(), CoverageReason> {
        for parameter in parameters {
            self.ty(&parameter.ty)?;
            match &mut parameter.default {
                CallableParamDefault::Source(value) => {
                    self.statements(&mut value.stmts)?;
                    self.operand(&mut value.result)?;
                }
                CallableParamDefault::Required | CallableParamDefault::PartialPreset { .. } => {}
                CallableParamDefault::Unsupported { .. } => return Err(CoverageReason::UnsupportedConstruct),
            }
        }
        Ok(())
    }

    /// Inspect every explicit operand rather than only the statements that happen to call functions.
    fn operand(&mut self, operand: &mut Operand) -> Result<(), CoverageReason> {
        match operand {
            Operand::Place(value) => self.place(&mut value.place),
            Operand::Constant(_) => Ok(()),
        }
    }

    /// Canonical field references must be public; module storage requires an executable initialization contract.
    fn place(&mut self, place: &mut Place) -> Result<(), CoverageReason> {
        if matches!(place.root, PlaceRoot::Global(_)) {
            return Err(CoverageReason::UnsupportedConstruct);
        }
        for projection in &mut place.projection {
            match projection {
                PlaceElem::Field {
                    canonical: Some(identity),
                    ..
                } => self.identity(identity, false)?,
                PlaceElem::Field { canonical: None, .. } => {}
                PlaceElem::Index(value) => self.operand(value)?,
                PlaceElem::Slice { start, end, step } => {
                    for value in [start, end, step].into_iter().flatten() {
                        self.operand(value)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// All argument forms retain their source operand even when a runtime profile later refuses the shape.
    fn arguments(&mut self, arguments: &mut [ArgumentElement]) -> Result<(), CoverageReason> {
        for argument in arguments {
            match argument {
                ArgumentElement::One(value) | ArgumentElement::Named { operand: value, .. } => self.operand(value)?,
                ArgumentElement::Spread(value) => self.operand(&mut value.source)?,
            }
        }
        Ok(())
    }

    /// Keep enum and member addresses joined to public canonical identities.
    fn fieldless_variant(&mut self, target: &mut FieldlessEnumVariantTarget) -> Result<(), CoverageReason> {
        self.identity(&mut target.enum_canonical, true)?;
        self.identity(&mut target.variant_canonical, false)?;
        target.enum_declaration_id = declaration_id(&target.enum_canonical)?;
        target.variant_declaration_id = declaration_id(&target.variant_canonical)?;
        Ok(())
    }

    /// Match and assert patterns can depend on types even when the matching branch is never entered.
    fn pattern(&mut self, pattern: &mut Pattern) -> Result<(), CoverageReason> {
        match pattern {
            Pattern::Wildcard | Pattern::Var(_) | Pattern::Literal(_) => {}
            Pattern::Tuple(items) | Pattern::Or(items) | Pattern::Result { fields: items, .. } => {
                for item in items {
                    self.pattern(item)?;
                }
            }
            Pattern::Nominal { target, fields } => {
                self.identity(&mut target.canonical, true)?;
                target.direct_declaration_id = declaration_id(&target.canonical)?;
                for (_, pattern) in fields {
                    self.pattern(pattern)?;
                }
            }
            Pattern::FieldlessEnumVariant(target) => self.fieldless_variant(target)?,
            Pattern::Struct { .. } | Pattern::Enum { .. } => return Err(CoverageReason::UnsupportedConstruct),
        }
        Ok(())
    }

    /// Every deferred or branching computation participates in the same conservative public closure.
    fn rvalue(&mut self, value: &mut Rvalue) -> Result<(), CoverageReason> {
        match value {
            Rvalue::Use(value) | Rvalue::UnaryOp(_, value) => self.operand(value)?,
            Rvalue::BinaryOp(_, left, right) => {
                self.operand(left)?;
                self.operand(right)?;
            }
            Rvalue::IsInstance {
                value,
                value_ty,
                target,
            } => {
                self.operand(value)?;
                self.ty(value_ty)?;
                self.ty(&target.ty)?;
                if let Some(identity) = &mut target.canonical {
                    self.identity(identity, true)?;
                }
            }
            Rvalue::Aggregate(kind, arguments) => {
                if let AggregateKind::Constructor(target) = kind {
                    let identity = target.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
                    self.identity(identity, true)?;
                    target.direct_declaration_id = Some(declaration_id(identity)?);
                }
                self.arguments(arguments)?;
            }
            Rvalue::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.operand(key)?;
                            self.operand(value)?;
                        }
                        DictEntry::Spread(value) => self.operand(&mut value.source)?,
                    }
                }
            }
            Rvalue::ValueEnumVariant(target) => {
                self.identity(&mut target.enum_canonical, true)?;
                self.identity(&mut target.variant_canonical, false)?;
                target.enum_declaration_id = declaration_id(&target.enum_canonical)?;
                target.variant_declaration_id = declaration_id(&target.variant_canonical)?;
            }
            Rvalue::FieldlessEnumVariant(target) => self.fieldless_variant(target)?,
            Rvalue::ResultVariant(value) => {
                self.operand(&mut value.payload)?;
                for identity in value.canonical_types.values_mut() {
                    self.identity(identity, true)?;
                }
                self.ty(&value.ok_type)?;
                self.ty(&value.error_type)?;
            }
            Rvalue::Format(parts) => {
                for part in parts {
                    if let FormatPart::Expr { operand, .. } = part {
                        self.operand(operand)?;
                    }
                }
            }
            Rvalue::Closure {
                params,
                captured_operands,
                body,
            } => {
                self.parameters(params)?;
                for value in captured_operands {
                    self.operand(value)?;
                }
                self.statements(&mut body.stmts)?;
                self.operand(&mut body.result)?;
            }
            Rvalue::Generator {
                source,
                captured_operands,
                body,
            } => {
                self.operand(source)?;
                for value in captured_operands {
                    self.operand(value)?;
                }
                self.statements(&mut body.stmts)?;
            }
            Rvalue::Match { scrutinee, arms } => {
                self.operand(scrutinee)?;
                for arm in arms {
                    self.pattern(&mut arm.pattern)?;
                    self.statements(&mut arm.guard_stmts)?;
                    if let Some(value) = &mut arm.guard {
                        self.operand(value)?;
                    }
                    self.statements(&mut arm.body_stmts)?;
                    self.operand(&mut arm.result)?;
                }
            }
        }
        Ok(())
    }

    /// Traverse the complete statement vocabulary; unsupported nodes cannot become coverage merely by encoding.
    fn statements(&mut self, statements: &mut [Statement]) -> Result<(), CoverageReason> {
        for statement in statements {
            self.statement(statement)?;
        }
        Ok(())
    }

    /// Inspect one statement without hiding unsupported nodes behind successful codec serialization.
    fn statement(&mut self, statement: &mut Statement) -> Result<(), CoverageReason> {
        match &mut statement.kind {
            StatementKind::Assign { place, rvalue } => {
                self.place(place)?;
                self.rvalue(rvalue)?;
            }
            StatementKind::Call {
                destination,
                callee,
                args,
                ..
            } => {
                if let Some(place) = destination {
                    self.place(place)?;
                }
                self.arguments(args)?;
                match callee {
                    Callee::Function(CallableTarget::Named(target)) => {
                        for ty in &target.type_args {
                            self.ty(ty)?;
                        }
                        if target.builtin.is_none() {
                            let identity = target.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
                            self.identity(identity, true)?;
                            // Package dispatch is canonical throughout, including calls within the same fragment.
                            target.direct_call_id = None;
                        }
                    }
                    Callee::Function(CallableTarget::Local(target)) => self.place(&mut target.operand.place)?,
                    Callee::Method(target) => {
                        for ty in &target.type_args {
                            self.ty(ty)?;
                        }
                        if let Some(identity) = &mut target.canonical {
                            self.identity(identity, true)?;
                        }
                    }
                    Callee::Helper(_) => {}
                    // The activation state is a producer-session fact. Until the portable host-requirement
                    // contract exists it must not be exported as if it were the consumer's provider plan.
                    Callee::ProviderOperation(_) => return Err(CoverageReason::UnsupportedConstruct),
                }
            }
            StatementKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.operand(cond)?;
                self.statements(&mut then_block.stmts)?;
                if let Some(block) = else_block {
                    self.statements(&mut block.stmts)?;
                }
            }
            StatementKind::Loop { body } => self.statements(&mut body.stmts)?,
            StatementKind::Break { value } | StatementKind::Return { value } => {
                if let Some(value) = value {
                    self.operand(value)?;
                }
            }
            StatementKind::Await { destination, awaited } => {
                if let Some(place) = destination {
                    self.place(place)?;
                }
                self.operand(awaited)?;
            }
            StatementKind::Race { destination, arms } => {
                if let Some(place) = destination {
                    self.place(place)?;
                }
                for arm in arms {
                    self.operand(&mut arm.awaitable)?;
                    self.statements(&mut arm.body.stmts)?;
                    self.operand(&mut arm.result)?;
                }
            }
            StatementKind::Yield { value } | StatementKind::Expr { value } => self.operand(value)?,
            StatementKind::Assert { kind, message, .. } => {
                if let Some(message) = message {
                    self.operand(message)?;
                }
                match kind {
                    AssertionKind::Condition { cond } => self.operand(cond)?,
                    AssertionKind::Pattern { scrutinee, pattern } => {
                        self.operand(scrutinee)?;
                        self.pattern(pattern)?;
                    }
                    AssertionKind::Raises { call, .. } => self.operand(call)?,
                }
            }
            StatementKind::TryPropagate {
                destination,
                operand,
                error_routing,
            } => {
                self.place(destination)?;
                self.operand(operand)?;
                match error_routing {
                    TryErrorRouting::SameType { error_type } => self.ty(error_type)?,
                    TryErrorRouting::ConversionRequired { .. } | TryErrorRouting::Unresolved => {
                        return Err(CoverageReason::UnsupportedConstruct);
                    }
                }
            }
            StatementKind::IterNext {
                destination,
                iterator,
                protocol,
            } => {
                self.place(destination)?;
                self.operand(iterator)?;
                if !matches!(protocol, crate::body_ir::IterProtocol::Builtin) {
                    return Err(CoverageReason::UnsupportedConstruct);
                }
            }
            StatementKind::Drop { .. } | StatementKind::Continue => {}
            StatementKind::Unsupported { .. } => return Err(CoverageReason::UnsupportedConstruct),
        }
        Ok(())
    }
}
