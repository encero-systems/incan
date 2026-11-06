//! The publication audit that projects identities, types, places, rvalues and statements into published form.

use super::*;

impl<'a> PublicationAudit<'a> {
    /// Preserve a checked fragment-local reflection binder without admitting it as a callable package dependency.
    /// Both identities are projected together only after the owning generic frame has authenticated their relation.
    pub(super) fn generic_metadata_target(&mut self, target: &mut MethodTarget) -> Result<bool, CoverageReason> {
        if !matches!(
            target.reflection,
            Some(crate::body_ir::ReflectionOperation::GenericMetadata(_))
        ) {
            return Ok(false);
        }
        let callable = self
            .current_callable
            .as_ref()
            .ok_or(CoverageReason::UnresolvedReference)?;
        if !generic_metadata_belongs_to(target, callable, &self.type_parameters, self.nominal_declarations) {
            return Err(CoverageReason::UnresolvedReference);
        }
        let Some(crate::body_ir::ReflectionOperation::GenericMetadata(binder)) = &mut target.reflection else {
            return Err(CoverageReason::UnresolvedReference);
        };
        if let SymbolOrigin::Module(module_path) = &binder.origin {
            binder.origin = SymbolOrigin::Package {
                library: self.library.to_owned(),
                module_path: module_path.clone(),
            };
        }
        let member = target.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
        member.origin = binder.origin.clone();
        Ok(true)
    }

    /// Combine source-local declarations, retained checked references and the executable native storage inventory,
    /// keeping conflicting source identities separate and performing no new name resolution.
    pub(super) fn new(
        module: &'a BodyIrModule,
        library: &'a str,
        eligible: &'a BTreeSet<CanonicalSymbolId>,
        checked_nominal_types: &'a BTreeMap<String, CanonicalSymbolId>,
    ) -> Self {
        // ---- Declarations: source-owned nominal identities ----
        let mut nominal_types = BTreeMap::new();
        for declaration in &module.nominal_declarations {
            nominal_types.insert(declaration.name.clone(), declaration.canonical.clone());
        }
        for declaration in &module.fieldless_enum_declarations {
            nominal_types.insert(declaration.name.clone(), declaration.canonical.clone());
        }
        for declaration in &module.value_enum_declarations {
            nominal_types.insert(declaration.name.clone(), declaration.canonical.clone());
        }
        for declaration in &module.enum_declarations {
            nominal_types.insert(declaration.name.clone(), declaration.canonical.clone());
        }
        for declaration in &module.trait_declarations {
            nominal_types.insert(declaration.declaration_name.clone(), declaration.clone());
        }

        // ---- Decorators: distinct original-callable and replacement-storage permissions ----
        let mut original_callables = BTreeSet::new();
        let mut replacement_bindings = BTreeSet::new();
        for binding in &module.decorated_bindings {
            if !is_well_formed_source_binding(module, binding, library) {
                continue;
            }
            let Some(identity) = &binding.original_body.canonical else {
                continue;
            };
            original_callables.insert(identity.clone());
            replacement_bindings.insert(identity.clone());
        }

        // ---- Context: package and type auditing ----
        Self {
            current_callable: None,
            replacement_bindings,
            original_callables,
            trait_self: false,
            type_parameters: BTreeSet::new(),
            library,
            eligible,
            nominal_types,
            checked_nominal_types,
            value_enum_declarations: &module.value_enum_declarations,
            nominal_declarations: &module.nominal_declarations,
            native_types: crate::body_ir::held_rust_items(std::slice::from_ref(module))
                .into_iter()
                .map(|declaration| (declaration.name, declaration.canonical))
                .collect(),
            used_nominal_types: BTreeMap::new(),
            requirements: BTreeSet::new(),
            native_calls: Vec::new(),
            local_slots: BTreeSet::new(),
        }
    }

    /// Rebase only producer-local origins; a foreign reexport retains its declaring package identity. Native items
    /// require adapter validation against the selected Rust closure, never an Incan executable fragment.
    pub(super) fn identity(&mut self, identity: &mut CanonicalSymbolId, required: bool) -> Result<(), CoverageReason> {
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
                if library == self.library && !self.eligible.contains(identity) {
                    if let Some(mut owner) =
                        generated_value_enum_member_owner(identity, self.value_enum_declarations, self.library)
                    {
                        self.identity(&mut owner, true)?;
                    } else {
                        return Err(CoverageReason::PrivateDependency);
                    }
                }
                if required {
                    self.requirements.insert(identity.clone());
                }
                Ok(())
            }
            SymbolOrigin::Builtin => Ok(()),
            // Native identities require no Incan fragment. Their exact storage and callable signatures remain
            // subject to the adapter's metadata validation against the selected native closure.
            SymbolOrigin::RustCrate(_) => Ok(()),
            SymbolOrigin::Module(_) => Err(CoverageReason::UnsupportedConstruct),
        }
    }

    /// Admit concrete types and declared binders, retaining the canonical declaration behind each named type.
    ///
    /// Native unsized `str` and slice referents, and checked awaitable outputs, retain structural identity rather than
    /// a source nominal declaration. References may cross the body boundary; lowering still rejects unsized values in
    /// owned frame storage.
    pub(super) fn ty(&mut self, ty: &IncanType) -> Result<(), CoverageReason> {
        match ty {
            IncanType::NativePointer { .. } | IncanType::NativeCChar => Err(CoverageReason::UnsupportedConstruct),
            IncanType::SelfType if self.trait_self => Ok(()),
            IncanType::TypeVar(name) if self.type_parameters.contains(name) => Ok(()),
            IncanType::Primitive(_) | IncanType::Never | IncanType::Decimal { .. } | IncanType::NativeBuiltin(_) => {
                Ok(())
            }
            IncanType::RustInteropPath(path) if path == "str" => Ok(()),
            IncanType::Generic { base, args }
                if matches!(base.as_str(), "[]" | "Awaitable" | "$FrozenList" | "$FrozenSet") =>
            {
                match args.as_slice() {
                    [element] => self.ty(element),
                    _ => Err(CoverageReason::UnresolvedReference),
                }
            }
            // Native frozen storage keeps the checker's element carriers rather than applying the public frozen
            // surface's text conversion. Its internal constructor has no source declaration to demand.
            IncanType::Generic { base, args } if base == "$FrozenDict" => match args.as_slice() {
                [key, value] => {
                    self.ty(key)?;
                    self.ty(value)
                }
                _ => Err(CoverageReason::UnresolvedReference),
            },
            IncanType::NativeTraitObject { path, .. } if crate::body_ir::rust_item_identity(path).is_some() => Ok(()),
            IncanType::Named(name) => self.named_type(name),
            IncanType::Generic { base, args } => {
                // Compiler collections have no declaration requirement; a generic nominal retains its checked base.
                if incan_lang::lang::types::collections::from_str(base).is_none()
                    && base != incan_lang::lang::types::UNION_TYPE_NAME
                    && !(incan_lang::lang::traits::from_str(base) == Some(incan_lang::lang::traits::TraitId::Awaitable)
                        && args.len() == 1)
                {
                    self.named_type(base)?;
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
            IncanType::Ref(inner)
            | IncanType::RefMut(inner)
            | IncanType::TypeToken(inner)
            | IncanType::NativeArray { element: inner, .. } => self.ty(inner),
            IncanType::TypeVar(_)
            | IncanType::SelfType
            | IncanType::RustInteropPath(_)
            | IncanType::NativeTraitObject { .. }
            | IncanType::Infer
            | IncanType::Unknown
            | IncanType::Unsupported(_) => Err(CoverageReason::UnresolvedReference),
        }
    }

    /// Retain a named model or trait only when local declarations and checker bindings agree on its identity, or
    /// select an exact native identity already present in the executable storage inventory.
    pub(super) fn named_type(&mut self, name: &str) -> Result<(), CoverageReason> {
        let local = self.nominal_types.get(name);
        let checked = self.checked_nominal_types.get(name);
        if matches!((local, checked), (Some(local), Some(checked)) if local != checked) {
            return Err(CoverageReason::UnresolvedReference);
        }
        let mut identity = checked
            .or(local)
            .or_else(|| self.native_types.get(name))
            .cloned()
            .ok_or(CoverageReason::UnresolvedReference)?;
        self.identity(&mut identity, true)?;
        self.used_nominal_types.insert(name.to_owned(), identity);
        Ok(())
    }

    /// Audit declaration-owned defaults, including nested closure parameters, and retain every referenced frame slot.
    pub(super) fn parameters(&mut self, parameters: &mut [CallableParam]) -> Result<(), CoverageReason> {
        for parameter in parameters {
            self.local_slots.insert(parameter.local);
            self.ty(&parameter.ty)?;
            match &mut parameter.default {
                CallableParamDefault::Source(value) => {
                    self.statements(&mut value.stmts)?;
                    self.operand(&mut value.result)?;
                }
                CallableParamDefault::Required => {}
                CallableParamDefault::PartialPreset { capture } => {
                    self.local_slots.insert(*capture);
                }
                CallableParamDefault::Unsupported { .. } => {
                    return Err(CoverageReason::UnsupportedConstruct);
                }
            }
        }
        Ok(())
    }

    /// Inspect every explicit operand rather than only the statements that happen to call functions.
    pub(super) fn operand(&mut self, operand: &mut Operand) -> Result<(), CoverageReason> {
        match operand {
            Operand::Place(value) => self.place(&mut value.place),
            Operand::Constant(value) => self.constant(value),
        }
    }

    /// Scalar data keeps its existing contract; newtype values additionally require their exact retained owner.
    pub(super) fn constant(&mut self, constant: &mut crate::body_ir::Constant) -> Result<(), CoverageReason> {
        match constant {
            crate::body_ir::Constant::NumericNewtype(value) => self.numeric_newtype_constant(value),
            crate::body_ir::Constant::FrozenAggregate(_, elements) => {
                for element in elements {
                    self.constant(element)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Validate the source physical address before projection; a foreign inline value lacks this local authority.
    /// Missing or duplicate owner layouts stay refused instead of reconstructing their ABI from a type name.
    pub(super) fn numeric_newtype_constant(
        &mut self,
        value: &mut crate::body_ir::NumericNewtypeConstant,
    ) -> Result<(), CoverageReason> {
        let identity = value
            .constructor
            .canonical
            .as_ref()
            .ok_or(CoverageReason::UnresolvedReference)?;
        let mut owners = self
            .nominal_declarations
            .iter()
            .filter(|owner| &owner.canonical == identity);
        let owner = owners.next().ok_or(CoverageReason::UnresolvedReference)?;
        if owners.next().is_some() || !value.matches_layout(owner) {
            return Err(CoverageReason::UnresolvedReference);
        }
        let identity = value
            .constructor
            .canonical
            .as_mut()
            .ok_or(CoverageReason::UnresolvedReference)?;
        self.identity(identity, true)?;
        value.constructor.direct_declaration_id = Some(declaration_id(identity)?);
        Ok(())
    }

    /// Canonical fields and static storage require checked context; a requiring body cannot outlive missing
    /// initialization. Computed constants retain read-only authority. Callable replacement storage additionally
    /// requires its validated decorator binding.
    pub(super) fn place(&mut self, place: &mut Place) -> Result<(), CoverageReason> {
        if let PlaceRoot::Local(local) = &place.root {
            self.local_slots.insert(*local);
        }
        if let PlaceRoot::Global(value) = &mut place.root {
            if value.identity.kind != crate::SemanticSourceTargetKind::Static
                && !(value.identity.kind == crate::SemanticSourceTargetKind::Const
                    && value.write_policy == crate::body_ir::GlobalWritePolicy::ReadOnly)
                && !self.replacement_bindings.contains(&value.identity)
            {
                return Err(CoverageReason::UnsupportedConstruct);
            }
            self.identity(&mut value.identity, true)?;
            self.ty(&value.ty)?;
        }
        for projection in &mut place.projection {
            match projection {
                PlaceElem::Field {
                    canonical: Some(identity),
                    structural: false,
                    ..
                } => self.identity(identity, false)?,
                PlaceElem::Field {
                    canonical: None,
                    structural: true,
                    ..
                } => {}
                PlaceElem::Field { .. } => return Err(CoverageReason::UnresolvedReference),
                PlaceElem::UnionMember { ty } => self.ty(ty)?,
                PlaceElem::OptionPayload {
                    option_type,
                    payload_type,
                } => {
                    self.ty(option_type)?;
                    self.ty(payload_type)?;
                }
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
    pub(super) fn arguments(&mut self, arguments: &mut [ArgumentElement]) -> Result<(), CoverageReason> {
        for argument in arguments {
            match argument {
                ArgumentElement::One(value) | ArgumentElement::Named { operand: value, .. } => self.operand(value)?,
                ArgumentElement::Spread(value) => self.operand(&mut value.source)?,
            }
        }
        Ok(())
    }

    /// Keep enum and member addresses joined to checked canonical identities.
    pub(super) fn fieldless_variant(&mut self, target: &mut FieldlessEnumVariantTarget) -> Result<(), CoverageReason> {
        self.identity(&mut target.enum_canonical, true)?;
        self.identity(&mut target.variant_canonical, false)?;
        target.enum_declaration_id = declaration_id(&target.enum_canonical)?;
        target.variant_declaration_id = declaration_id(&target.variant_canonical)?;
        Ok(())
    }

    /// Match and assert patterns can depend on types even when the matching branch is never entered.
    pub(super) fn pattern(&mut self, pattern: &mut Pattern) -> Result<(), CoverageReason> {
        match pattern {
            Pattern::Var(local) => {
                self.local_slots.insert(local.local);
            }
            Pattern::Wildcard | Pattern::Literal(_) => {}
            Pattern::Tuple(items) | Pattern::Or(items) | Pattern::Result { fields: items, .. } => {
                for item in items {
                    self.pattern(item)?;
                }
            }
            Pattern::UnionMember { ty, fields } => {
                self.ty(ty)?;
                for field in fields {
                    self.pattern(field)?;
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
            Pattern::Enum { canonical, fields, .. } => {
                let identity = canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
                self.identity(identity, true)?;
                for field in fields {
                    self.pattern(field)?;
                }
            }
            Pattern::Struct { .. } => return Err(CoverageReason::UnsupportedConstruct),
        }
        Ok(())
    }

    /// Every deferred or branching computation participates in the same conservative public closure.
    pub(super) fn rvalue(&mut self, value: &mut Rvalue) -> Result<(), CoverageReason> {
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
                if let AggregateKind::EnumVariant(target) = kind {
                    let owner = target
                        .enum_canonical
                        .as_mut()
                        .ok_or(CoverageReason::UnresolvedReference)?;
                    self.identity(owner, true)?;
                    self.identity(&mut target.variant_canonical, false)?;
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
                if !value.has_complete_canonical_types() {
                    return Err(CoverageReason::UnresolvedReference);
                }
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
                if let Some(identity) = &mut body.original_callable {
                    if !self.original_callables.contains(identity) || self.current_callable.as_ref() != Some(identity) {
                        return Err(CoverageReason::UnresolvedReference);
                    }
                    self.identity(identity, true)?;
                }
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
    pub(super) fn statements(&mut self, statements: &mut [Statement]) -> Result<(), CoverageReason> {
        for statement in statements {
            self.statement(statement)?;
        }
        Ok(())
    }

    /// Audit the call-site SDK claim under its own Self binder; unresolved leaves cannot cross publication.
    pub(super) fn native_trait_selection(&mut self, target: &mut MethodTarget) -> Result<(), CoverageReason> {
        if !target.has_valid_native_trait_selection() {
            return Err(CoverageReason::UnresolvedReference);
        }
        let Some(selection) = &mut target.native_trait_delegation else {
            return Ok(());
        };
        self.identity(&mut selection.canonical, true)?;
        let inserted = self.type_parameters.insert("Self".to_owned());
        let result = selection
            .parameters
            .iter()
            .chain(std::iter::once(&selection.return_type))
            .chain(&selection.native_type_arguments)
            .try_for_each(|ty| self.ty(ty));
        if inserted {
            self.type_parameters.remove("Self");
        }
        result
    }

    /// Inspect one statement without hiding unsupported nodes behind successful codec serialization.
    pub(super) fn statement(&mut self, statement: &mut Statement) -> Result<(), CoverageReason> {
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
                self.callee(callee)?;
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
            StatementKind::Loop { body } | StatementKind::UnsafeRegion { body, .. } => {
                self.statements(&mut body.stmts)?
            }
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

    /// Audit the checked callee and retain exact native selections without exporting producer-session activation.
    ///
    /// Package calls keep canonical dispatch even within their own fragment. Generated generic field metadata
    /// retains only its authenticated fragment-local binder; provider, type-metadata, foreign and bridge targets
    /// remain refused until their portable execution requirements have a represented contract.
    pub(super) fn callee(&mut self, callee: &mut Callee) -> Result<(), CoverageReason> {
        match callee {
            Callee::Function(CallableTarget::Named(target)) => {
                for ty in &target.type_args {
                    self.ty(ty)?;
                }
                if target.builtin.is_none() {
                    let identity = target.canonical.as_mut().ok_or(CoverageReason::UnresolvedReference)?;
                    self.identity(identity, true)?;
                    if matches!(identity.origin, SymbolOrigin::RustCrate(_)) {
                        let selection = (identity.clone(), target.type_args.clone());
                        if !self.native_calls.contains(&selection) {
                            self.native_calls.push(selection);
                        }
                    }
                    target.direct_call_id = None;
                }
            }
            Callee::Function(CallableTarget::Local(target)) => self.place(&mut target.operand.place)?,
            Callee::Method(target) => {
                if self.generic_metadata_target(target)? {
                    return Ok(());
                }
                self.native_trait_selection(target)?;
                if let Some(arguments) = &target.trait_arguments {
                    for ty in arguments {
                        self.ty(ty)?;
                    }
                }
                for ty in &target.type_args {
                    self.ty(ty)?;
                }
                if let Some(identity) = &mut target.canonical {
                    self.identity(identity, true)?;
                }
            }
            Callee::Helper(_) => {}
            Callee::ProviderOperation(_)
            | Callee::TypeMetadata(_)
            | Callee::CheckedForeign(_)
            | Callee::CheckedBridge(_) => return Err(CoverageReason::UnsupportedConstruct),
        }
        Ok(())
    }
}
