//! Accepted scalar bodies and checked source-module calls under the callable interface's existing identities.
//!
//! Calls retain their complete checked candidate/signature witnesses and exact current header inputs. Import and
//! nominal declarations remain exact environment inputs; their bodies and calls stay outside this scalar product.
//! Closed primitive literal defaults are retained; computed defaults and scope-changing bodies refuse capture.
//! Primitive mutable storage retains its checked
//! mutation contract and complete assignment operations. Physical locators must also match; a semantic digest alone
//! never grants a hit.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use incan_lang::lang::builtins::{self, BuiltinFnId};
use incan_lang::lang::types::numerics::{self, NumericTypeId};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::stable_identity::StableIdentityProjection;
use incan_semantics_core::{CanonicalStableDeclarationContext, CanonicalSymbolId};
use serde::{Deserialize, Serialize};

use super::callable_module::{CheckedCallableModule, CheckedCallableRead, current_candidates_match};
use super::{CheckedCallableInterface, CheckedDeclarationFingerprints, decode_record, encode_record, invalid};
use crate::ast::{
    BindingKind, CallArg, Declaration, Expr, FunctionDecl, Literal, ParamKind, Program, Span, Spanned, Statement, Type,
};
use crate::library_manifest::LibraryManifestError;
use crate::symbols::ResolvedType;
use crate::typechecker::TypeChecker;
use crate::vocab_ast_bridge::is_synthetic_span;

/// One accepted module's independently reusable scalar declarations. The private codec retains checked evidence,
/// never an AST that a consumer could mistake for an accepted declaration environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallableBodyProducts {
    namespace: Vec<(Span, String)>,
    environment: Vec<(Span, String)>,
    interface: CheckedCallableModule,
    entries: Vec<BodyProduct>,
}

/// Accepted scalar headers and exact source inputs retained until the ordinary Body IR producer completes.
/// This witness avoids cloning a checker or building Body IR twice just to populate a cache.
pub struct CallableBodyInputs {
    namespace: Vec<(Span, String)>,
    environment: Vec<(Span, String)>,
    interface: CheckedCallableModule,
    declarations: Vec<BodyInput>,
    contexts: Vec<(CanonicalSymbolId, CanonicalSymbolId, u32)>,
}

/// An accepted input and its explicit reads, awaiting the ordinary Body IR producer's completed result.
struct BodyInput {
    header: CheckedCallableInterface,
    span: Span,
    header_source: String,
    source: String,
    reads: Vec<ReadGuard>,
    declaration: Vec<Vec<String>>,
    local: Vec<Vec<String>>,
}

/// One complete positive lookup and the exact source headers that produced its previously accepted signatures.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadGuard {
    name: String,
    checked: CheckedCallableRead,
    inputs: Vec<(Span, String)>,
}

/// Before-check header inputs indexed by name; each vector retains every overload in source order.
type SignatureInputs = BTreeMap<String, Vec<(Span, String)>>;

/// The complete ordinary Body and its accepted input, bound to the existing callable contract and meaning digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BodyProduct {
    header: CheckedCallableInterface,
    fingerprints: CheckedDeclarationFingerprints,
    header_source: String,
    source: String,
    reads: Vec<ReadGuard>,
    declaration: Vec<Vec<String>>,
    local: Vec<Vec<String>>,
    contexts: Vec<(CanonicalSymbolId, CanonicalSymbolId, u32)>,
    body: Body,
}

/// A producer-validated selection for exactly one parsed program. Only this module can construct a selection;
/// the checker compares its input again before skipping any body, and Body IR consumes the same retained products.
#[derive(Debug, Clone)]
pub struct ValidatedCallableBodies {
    program: Program,
    declarations: BTreeMap<Span, BodyProduct>,
    bodies: BTreeMap<Span, BodyProduct>,
    owner: incan_semantics_core::SymbolOrigin,
    producer_package: String,
}

impl CallableBodyProducts {
    /// Bind native frames to their accepted own fingerprints and complete consumed-signature witnesses. The native
    /// owner additionally keys the full checked Body, profile and physical provenance before restoring a function.
    pub fn native_inputs(&self) -> Result<Vec<(incan_semantics_core::CompilerNodeId, String)>, LibraryManifestError> {
        self.entries
            .iter()
            .map(|entry| {
                let reads = entry.reads.iter().map(|read| &read.checked).collect::<Vec<_>>();
                let witness = serde_json::to_string(&(&entry.fingerprints, reads))
                    .map_err(|error| invalid(&error.to_string()))?;
                Ok((entry.body.direct_call_id.clone(), witness))
            })
            .collect()
    }

    /// Capture bodies only after successful checking and Body IR construction. Warnings and unsupported grammar
    /// refuse capture rather than disappear on the next invocation. The ordinary private Body fields must be empty.
    pub fn from_checked(
        program: &Program,
        source: &str,
        checker: &TypeChecker,
        module: &BodyIrModule,
    ) -> Result<Self, LibraryManifestError> {
        CallableBodyInputs::from_checked(program, source, checker)?.finish(module)
    }

    /// Select exact untouched declarations before checking. The caller's module owner must be identical; physical
    /// movement, any changed input, or an unsupported module is a miss, never a partial acceptance of new syntax.
    pub fn select(
        &self,
        program: &Program,
        source: &str,
        package: &str,
        module: &[String],
    ) -> Result<ValidatedCallableBodies, LibraryManifestError> {
        // ---- Admission: retain the complete current namespace and source provenance ----
        self.validate()?;
        admitted_functions(program)?;
        let headers = signature_inputs(program, source)?;
        let names = headers.keys().cloned().collect::<BTreeSet<_>>();
        let owner = self
            .entries
            .first()
            .ok_or_else(|| invalid("callable products have no producer identity"))?
            .header
            .identity()
            .origin
            .clone();
        let owner_matches = match &owner {
            incan_semantics_core::SymbolOrigin::Module(module_path) => module_path == module,
            incan_semantics_core::SymbolOrigin::Package { library, module_path } => {
                library == package && module_path == module
            }
            incan_semantics_core::SymbolOrigin::RustCrate(_) | incan_semantics_core::SymbolOrigin::Builtin => false,
        };
        if namespace(program) != self.namespace || environment_inputs(program, source)? != self.environment {
            return Ok(ValidatedCallableBodies {
                program: program.clone(),
                declarations: BTreeMap::new(),
                bodies: BTreeMap::new(),
                owner,
                producer_package: package.to_owned(),
            });
        }
        if !owner_matches {
            return Ok(ValidatedCallableBodies {
                program: program.clone(),
                declarations: BTreeMap::new(),
                bodies: BTreeMap::new(),
                owner,
                producer_package: package.to_owned(),
            });
        }

        // ---- Selection: admit exact untouched scalar bodies and every consumed source header ----
        let mut declarations = BTreeMap::new();
        let mut bodies = BTreeMap::new();
        let entries = self
            .entries
            .iter()
            .map(|entry| (entry.body.span, entry))
            .collect::<BTreeMap<_, _>>();
        for (declaration, entry) in program
            .declarations
            .iter()
            .filter(|declaration| matches!(declaration.node, Declaration::Function(_)))
            .filter_map(|declaration| entries.get(&declaration.span).map(|entry| (declaration, *entry)))
        {
            let Declaration::Function(function) = &declaration.node else {
                continue;
            };
            let Some(reads) = admitted_reads(function, &names) else {
                continue;
            };
            let Some(header_source) = header_source(&headers, &function.name, declaration.span) else {
                continue;
            };
            let input = source_slice(source, declaration.span)?;
            if entry.header.identity().origin == owner
                && entry.body.span == declaration.span
                && entry.header_source == header_source
            {
                declarations.insert(declaration.span, entry.clone());
                if entry.source == input && matching_reads(&entry.reads, &reads, &headers) {
                    bodies.insert(declaration.span, entry.clone());
                }
            }
        }

        // ---- Replay: bind accepted products to the exact parsed program ----
        Ok(ValidatedCallableBodies {
            program: program.clone(),
            declarations,
            bodies,
            owner,
            producer_package: package.to_owned(),
        })
    }

    /// Encode only a complete validated private product set, using the callable codec's checksummed framing.
    pub fn to_bytes(&self) -> Result<Vec<u8>, LibraryManifestError> {
        self.validate()?;
        encode_record(&serde_json::to_vec(self).map_err(|error| invalid(&error.to_string()))?)
    }

    /// Decode damaged, incomplete or unfamiliar records as a refusal the store owner treats as a cache miss.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LibraryManifestError> {
        let products: Self =
            serde_json::from_slice(decode_record(bytes)?).map_err(|error| invalid(&error.to_string()))?;
        products.validate()?;
        Ok(products)
    }

    /// Require unique locators and the exact interface/body meaning originally accepted by the producer.
    fn validate(&self) -> Result<(), LibraryManifestError> {
        let reads = self.interface.read_inventory()?;
        let namespace = self
            .namespace
            .iter()
            .map(|(span, name)| (*span, name))
            .collect::<BTreeMap<_, _>>();
        if self.entries.len() > self.namespace.len()
            || namespace.len() != self.namespace.len()
            || self
                .entries
                .iter()
                .any(|entry| namespace.get(&entry.body.span).copied() != Some(&entry.body.name))
        {
            return Err(invalid(
                "callable products lost their complete physical declaration inventory",
            ));
        }
        let mut identities = BTreeSet::new();
        let mut locators = BTreeSet::new();
        let mut owner = None;
        for entry in &self.entries {
            body_scope(&entry.contexts)?;
            if !resolved_scalar_header(&entry.header)? {
                return Err(invalid("scalar body product has a nonprimitive checked signature"));
            }
            if entry.declaration.is_empty() || entry.header_source.is_empty() {
                return Err(invalid("scalar body product has no declaration record"));
            }
            let mut body = entry.body.clone();
            if owner.get_or_insert_with(|| entry.header.identity().origin.clone()) != &entry.header.identity().origin {
                return Err(invalid("callable products contain multiple producer identities"));
            }
            if !identities.insert(entry.header.identity())
                || !locators.insert(body.span)
                || entry
                    .reads
                    .iter()
                    .any(|read| reads.get(&read.name) != Some(&read.checked))
                || entry.fingerprints != entry.header.fingerprints(&mut body, &entry.projection())?
            {
                return Err(invalid("callable product lost its accepted identity or meaning"));
            }
        }
        Ok(())
    }
}

impl CallableBodyInputs {
    /// Capture accepted headers and lexical ownership before consuming a successful checker. Unwitnessed reads and
    /// diagnostics refuse capture; missing evidence can never be interpreted as an empty dependency set.
    pub fn from_checked(program: &Program, source: &str, checker: &TypeChecker) -> Result<Self, LibraryManifestError> {
        Self::from_checked_with_package(program, source, checker, None)
    }

    /// Capture private products for an authored unit. The unit package is transport ownership for program modules;
    /// their canonical declarations remain module-owned.
    pub fn from_checked_for_unit(
        program: &Program,
        source: &str,
        checker: &TypeChecker,
        producer_package: &str,
    ) -> Result<Self, LibraryManifestError> {
        Self::from_checked_with_package(program, source, checker, Some(producer_package))
    }

    /// Capture accepted inputs while separating the private replay envelope from semantic symbol ownership.
    fn from_checked_with_package(
        program: &Program,
        source: &str,
        checker: &TypeChecker,
        producer_package: Option<&str>,
    ) -> Result<Self, LibraryManifestError> {
        // ---- Admission: preserve ordinary diagnostics and the complete source namespace ----
        admitted_functions(program)?;
        if !checker.errors.is_empty() || !checker.warnings().is_empty() {
            return Err(invalid("callable products require accepted bodies without diagnostics"));
        }
        let headers = signature_inputs(program, source)?;
        let names = headers.keys().cloned().collect::<BTreeSet<_>>();
        if !program.declarations.iter().any(|declaration| {
            matches!(&declaration.node, Declaration::Function(function) if admitted_reads(function, &names).is_some())
        }) {
            return Err(invalid("callable products have no admitted scalar body"));
        }
        let interface = CheckedCallableModule::from_checked_names(program, checker, &names, producer_package)?;
        let checked_reads = interface.read_inventory()?;

        // ---- Headers: unsupported bodies stay on the full path independently of scalar neighbors ----
        let mut declarations = Vec::new();
        for declaration in &program.declarations {
            let Declaration::Function(function) = &declaration.node else {
                continue;
            };
            let Some(reads) = admitted_reads(function, &names) else {
                continue;
            };
            let header = match producer_package {
                Some(package) => {
                    CheckedCallableInterface::from_checked_for_product(function, declaration.span, checker, package)?
                }
                None => CheckedCallableInterface::from_checked(function, declaration.span, checker)?,
            };
            if !resolved_scalar_header(&header)? {
                continue;
            }
            let Some(header_source) = header_source(&headers, &function.name, declaration.span) else {
                continue;
            };
            let Some(record) = checker.produced_declaration_record(declaration.span) else {
                continue;
            };
            declarations.push(BodyInput {
                header,
                span: declaration.span,
                header_source: header_source.to_owned(),
                source: source_slice(source, declaration.span)?.to_owned(),
                reads: capture_reads(reads, &checked_reads, &headers)?,
                declaration: record.clone(),
                local: checker
                    .available_local_record(declaration.span)
                    .cloned()
                    .unwrap_or_default(),
            });
        }

        // ---- Lexical evidence: retain the checker's owner and collision-local ordinal ----
        let facts = checker
            .type_info()
            .semantic_fact_store(&checker.current_module_path.clone().unwrap_or_default());
        let mut contexts = facts
            .subjects()
            .filter_map(|subject| {
                let identity = facts.symbol_identities_for(subject).next()?;
                let context = facts.stable_declaration_contexts_for(subject).next()?;
                Some((identity.clone(), context.owner.clone(), context.binding_ordinal))
            })
            .collect::<Vec<_>>();
        contexts.extend(checker.type_info().reused_callable_contexts.iter().cloned());
        if declarations.is_empty() {
            return Err(invalid("callable products have no admitted scalar body"));
        }
        Ok(Self {
            namespace: namespace(program),
            environment: environment_inputs(program, source)?,
            interface,
            declarations,
            contexts,
        })
    }

    /// Attach products from the actual Body IR pass, preserving each body's checked lexical projection.
    pub fn finish(self, module: &BodyIrModule) -> Result<CallableBodyProducts, LibraryManifestError> {
        // ---- Indexes: locate each body and lexical owner once ----
        let mut bodies = BTreeMap::new();
        for body in &module.bodies {
            match bodies.entry(body.span) {
                Entry::Vacant(entry) => {
                    entry.insert(Some(body));
                }
                Entry::Occupied(mut entry) => {
                    entry.insert(None);
                }
            }
        }
        let mut owners: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (identity, owner, ordinal) in self.contexts {
            owners
                .entry(owner.clone())
                .or_default()
                .push((identity, owner, ordinal));
        }

        // ---- Products: pair accepted headers with their complete ordinary bodies ----
        let mut entries = Vec::new();
        for BodyInput {
            header,
            span,
            header_source,
            source,
            reads,
            declaration,
            local,
        } in self.declarations
        {
            let mut body = bodies
                .get(&span)
                .copied()
                .flatten()
                .ok_or_else(|| invalid("callable product has no unique checked body"))?
                .clone();
            let contexts = body
                .canonical
                .as_ref()
                .and_then(|owner| owners.remove(owner))
                .unwrap_or_default();
            let projection = body_projection(&header, &body, &contexts, &reads);
            let fingerprints = header.fingerprints(&mut body, &projection)?;
            entries.push(BodyProduct {
                header,
                fingerprints,
                header_source,
                source,
                reads,
                declaration,
                local,
                contexts,
                body,
            });
        }

        // ---- Publication: validate identity and meaning before exposing a product set ----
        let products = CallableBodyProducts {
            namespace: self.namespace,
            environment: self.environment,
            interface: self.interface,
            entries,
        };
        products.validate()?;
        Ok(products)
    }
}

impl BodyProduct {
    /// Recover the producer's checked lexical correspondence; physical scope indices never become semantic keys.
    fn projection(&self) -> StableIdentityProjection {
        body_projection(&self.header, &self.body, &self.contexts, &self.reads)
    }
}

/// Reconstruct the existing stable-identity projection from explicitly retained checker evidence.
fn body_projection(
    header: &CheckedCallableInterface,
    body: &Body,
    contexts: &[(CanonicalSymbolId, CanonicalSymbolId, u32)],
    reads: &[ReadGuard],
) -> StableIdentityProjection {
    let mut projection = StableIdentityProjection::default();
    for (identity, owner, ordinal) in contexts {
        projection.contexts.insert(
            identity.clone(),
            CanonicalStableDeclarationContext {
                owner: owner.clone(),
                binding_ordinal: *ordinal,
            },
        );
    }
    if let (Some(canonical), Some(signature)) = (&body.canonical, &header.identity().signature) {
        projection.signatures.insert(canonical.clone(), signature.clone());
    }
    for read in reads {
        projection.signatures.extend(read.checked.signatures.iter().cloned());
    }
    projection
}

impl ValidatedCallableBodies {
    /// Consume evidence only for the exact parsed input selected by the store reader. A caller cannot install a
    /// product into another program or reuse the selection after mutating a declaration.
    pub(crate) fn install(self, program: &Program, checker: &mut TypeChecker) {
        let owner = checker
            .symbols
            .package_identity()
            .map(|package| incan_semantics_core::SymbolOrigin::Package {
                library: package.to_owned(),
                module_path: checker.current_module_path.clone().unwrap_or_default(),
            })
            .unwrap_or_else(|| {
                incan_semantics_core::SymbolOrigin::Module(checker.current_module_path.clone().unwrap_or_default())
            });
        if self.program != *program || self.owner != owner {
            return;
        }
        for declaration in &program.declarations {
            let Declaration::Function(function) = &declaration.node else {
                continue;
            };
            let Some(entry) = self.declarations.get(&declaration.span) else {
                continue;
            };
            let Ok(current) = CheckedCallableInterface::from_checked_for_product(
                function,
                declaration.span,
                checker,
                &self.producer_package,
            ) else {
                continue;
            };
            if current.identity() != entry.header.identity()
                || current.function().ok() != entry.header.function().ok()
                || entry
                    .reads
                    .iter()
                    .any(|read| !current_candidates_match(checker, &read.name, &read.checked))
            {
                continue;
            }
            checker.retain_declaration_record(declaration.span, entry.declaration.clone());
            if !entry.local.is_empty() {
                checker.retain_local_record(declaration.span, entry.local.clone());
            }
            let Some(entry) = self.bodies.get(&declaration.span) else {
                continue;
            };
            let Ok(scope) = body_scope(&entry.contexts) else {
                continue;
            };
            checker.type_info.reused_callable_scopes.insert(declaration.span, scope);
            checker
                .type_info
                .reused_callable_bodies
                .insert(declaration.span, entry.body.clone());
            checker
                .type_info
                .reused_callable_contexts
                .extend(entry.contexts.iter().cloned());
        }
    }

    /// Report the selected source-body count independently of actual checker and Body IR execution counters.
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// Report whether this validated selection contains any source-body products.
    pub fn is_empty(&self) -> bool {
        self.declarations.is_empty() && self.bodies.is_empty()
    }
}

/// Select one physical header from the complete overload-preserving signature inventory.
fn header_source<'a>(headers: &'a SignatureInputs, name: &str, span: Span) -> Option<&'a str> {
    headers
        .get(name)?
        .iter()
        .find_map(|(candidate, source)| (*candidate == span).then_some(source.as_str()))
}

/// Require ordinary physical declarations. Non-function declarations are retained as exact environment inputs;
/// individual scalar bodies still need complete live callable and lexical witnesses before skipping checking.
fn admitted_functions(program: &Program) -> Result<(), LibraryManifestError> {
    if program.declarations.is_empty() {
        return Err(invalid("callable products require an ordinary nonempty source module"));
    }
    for declaration in &program.declarations {
        if is_synthetic_span(declaration.span) {
            return Err(invalid("callable products refuse expanded source declarations"));
        }
    }
    Ok(())
}

/// Enumerate a straight-line scalar body's reads, admitting an initial literal docstring and primitive bindings.
/// Reassignment targets must already be bound; ordinary checking owns mutation acceptance before publication.
/// No admitted statement creates a nested scope, so skipped functions preserve physical scope numbering.
fn admitted_reads(function: &FunctionDecl, names: &BTreeSet<String>) -> Option<BTreeSet<String>> {
    let Some((statement, prefix)) = function.body.split_last() else {
        return None;
    };
    if !scalar_signature(function) || is_synthetic_span(statement.span) {
        return None;
    }
    let value = match &statement.node {
        Statement::Return(Some(value)) | Statement::Expr(value) => value,
        _ => return None,
    };
    let mut reads = BTreeSet::new();
    let mut bindings: BTreeSet<String> = function
        .params
        .iter()
        .map(|parameter| parameter.node.name.clone())
        .collect();
    for (index, statement) in prefix.iter().enumerate() {
        if is_synthetic_span(statement.span) {
            return None;
        }
        if index == 0
            && matches!(&statement.node, Statement::Expr(value)
            if !is_synthetic_span(value.span) && matches!(value.node, Expr::Literal(Literal::String(_))))
        {
            continue;
        }
        let Statement::Assignment(assignment) = &statement.node else {
            return None;
        };
        let existing = bindings.contains(&assignment.name);
        let admitted_target = match assignment.binding {
            BindingKind::Inferred => !existing || assignment.ty.is_none(),
            BindingKind::Let | BindingKind::Mutable => !existing,
            BindingKind::Reassign => existing && assignment.ty.is_none(),
        };
        if !admitted_target
            || is_synthetic_span(assignment.name_span)
            || assignment
                .ty
                .as_ref()
                .is_some_and(|ty| is_synthetic_span(ty.span) || !scalar_type(&ty.node))
            || !parameter_expression(&assignment.value, &bindings, names, &mut reads)
        {
            return None;
        }
        bindings.insert(assignment.name.clone());
    }
    parameter_expression(value, &bindings, names, &mut reads).then_some(reads)
}

/// Admit unexpanded primitive signatures with closed literal defaults. Computed defaults and generic environments
/// require additional read and instantiation witnesses and remain outside this product.
fn scalar_signature(function: &FunctionDecl) -> bool {
    if !function.decorators.is_empty()
        || numerics::from_str(&function.name).is_some()
        || !function.surface_modifiers.is_empty()
        || !function.type_params.is_empty()
        || !scalar_type(&function.return_type.node)
        || is_synthetic_span(function.return_type.span)
        || function.params.iter().any(|parameter| {
            is_synthetic_span(parameter.span)
                || is_synthetic_span(parameter.node.ty.span)
                || parameter.node.kind != ParamKind::Normal
                || parameter
                    .node
                    .default
                    .as_ref()
                    .is_some_and(|default| !literal_default(default))
                || !scalar_type(&parameter.node.ty.node)
        })
    {
        return false;
    }
    true
}

/// A literal default has no resolution reads, nested scopes or declaration-time effects. Successful ordinary
/// checking still owns its type acceptance, and the complete retained callable contract preserves its exact value.
fn literal_default(expression: &Spanned<Expr>) -> bool {
    !is_synthetic_span(expression.span)
        && matches!(
            expression.node,
            Expr::Literal(Literal::Int(_) | Literal::Float(_) | Literal::Bool(_))
        )
}

/// Retain the complete physical declaration inventory as a replay guard, never as a second semantic identity.
fn namespace(program: &Program) -> Vec<(Span, String)> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::Function(function) = &declaration.node else {
                return None;
            };
            Some((declaration.span, function.name.clone()))
        })
        .collect()
}

/// Retain exact non-function declaration inputs, including imports, layouts and module directives. They cannot
/// change underneath a previously accepted scalar body even when every free-function signature stays the same.
fn environment_inputs(program: &Program, source: &str) -> Result<Vec<(Span, String)>, LibraryManifestError> {
    program
        .declarations
        .iter()
        .filter(|declaration| !matches!(declaration.node, Declaration::Function(_)))
        .map(|declaration| Ok((declaration.span, source_slice(source, declaration.span)?.to_owned())))
        .collect()
}

/// Recover the single physical function scope retained by scalar parameters and locals. Multiple scopes would
/// require explicit lexical replay and are refused. Bodies without lexical identities need no scope correspondence.
fn body_scope(contexts: &[(CanonicalSymbolId, CanonicalSymbolId, u32)]) -> Result<Option<usize>, LibraryManifestError> {
    let scopes = contexts
        .iter()
        .filter_map(|(identity, _, _)| identity.scope_discriminant.map(|scope| scope.0))
        .collect::<BTreeSet<_>>();
    if scopes.len() > 1 {
        return Err(invalid("scalar body product contains multiple physical scopes"));
    }
    Ok(scopes.into_iter().next())
}

/// Recognize only the language's primitive scalar annotations; checker acceptance remains authoritative.
fn scalar_type(ty: &Type) -> bool {
    matches!(ty, Type::Unit)
        || matches!(ty, Type::Simple(name) if matches!(ResolvedType::builtin_simple(name),
            Some(ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool | ResolvedType::Str | ResolvedType::Unit)
            | Some(ResolvedType::Numeric(NumericTypeId::I64 | NumericTypeId::F64 | NumericTypeId::Bool))))
}

/// Require the checker's actual primitive types, not annotations that an import could shadow. These bodies cannot
/// read nominal layouts, trait implementations or imported values through a scalar-looking source spelling.
pub(super) fn resolved_scalar_header(header: &CheckedCallableInterface) -> Result<bool, LibraryManifestError> {
    let function = header.function()?;
    let scalar = |ty: &ResolvedType| {
        matches!(
            ty,
            ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool | ResolvedType::Str | ResolvedType::Unit
        ) || matches!(
            ty,
            ResolvedType::Numeric(NumericTypeId::I64 | NumericTypeId::F64 | NumericTypeId::Bool)
        )
    };
    Ok(function.type_params.is_empty()
        && !function.is_async
        && scalar(&function.return_type)
        && function.params.iter().all(|parameter| scalar(&parameter.ty)))
}

/// Enumerate the admitted expression grammar instead of assuming that an absent call edge proves no dependency.
fn parameter_expression(
    expression: &Spanned<Expr>,
    bindings: &BTreeSet<String>,
    names: &BTreeSet<String>,
    reads: &mut BTreeSet<String>,
) -> bool {
    if is_synthetic_span(expression.span) {
        return false;
    }
    match &expression.node {
        Expr::Ident(name) => bindings.contains(name),
        Expr::Literal(Literal::Int(_) | Literal::Float(_) | Literal::Bool(_) | Literal::String(_)) => true,
        Expr::Binary(left, _, right) => {
            parameter_expression(left, bindings, names, reads) && parameter_expression(right, bindings, names, reads)
        }
        Expr::Unary(_, operand) | Expr::Paren(operand) => parameter_expression(operand, bindings, names, reads),
        Expr::Call(callee, arguments, values) if arguments.is_empty() && !is_synthetic_span(callee.span) => {
            let Expr::Ident(name) = &callee.node else {
                return false;
            };
            if bindings.contains(name) {
                return false;
            }
            if !values.iter().all(|value| {
                matches!(value, CallArg::Positional(value)
                if parameter_expression(value, bindings, names, reads))
            }) {
                return false;
            }
            if names.contains(name) {
                reads.insert(name.clone());
                return true;
            }
            matches!(builtins::from_str(name), Some(BuiltinFnId::Print))
        }
        _ => false,
    }
}

/// Retain exact signature inputs before any body check. Primitive-only annotations, the complete namespace and
/// unchanged compiler/dependency context make these the same inputs previously accepted by the producer.
fn signature_inputs(program: &Program, source: &str) -> Result<SignatureInputs, LibraryManifestError> {
    let mut headers: SignatureInputs = BTreeMap::new();
    let mut unsupported = BTreeSet::new();
    for declaration in &program.declarations {
        let Declaration::Function(function) = &declaration.node else {
            continue;
        };
        if !scalar_signature(function) {
            unsupported.insert(function.name.clone());
            continue;
        }
        let body = function
            .body
            .first()
            .ok_or_else(|| invalid("callable signature has no source body locator"))?;
        let span = Span {
            end: body.span.start,
            ..declaration.span
        };
        let text = source_slice(source, span)?.to_owned();
        headers
            .entry(function.name.clone())
            .or_default()
            .push((declaration.span, text));
    }
    headers.retain(|name, _| !unsupported.contains(name));
    Ok(headers)
}

/// Attach only producer-owned candidate/signature witnesses and their exact source-header inputs.
fn capture_reads(
    names: BTreeSet<String>,
    checked: &BTreeMap<String, CheckedCallableRead>,
    headers: &SignatureInputs,
) -> Result<Vec<ReadGuard>, LibraryManifestError> {
    names
        .into_iter()
        .map(|name| {
            let read = checked
                .get(&name)
                .ok_or_else(|| invalid("source call has no checked candidate inventory"))?;
            let inputs = headers
                .get(&name)
                .ok_or_else(|| invalid("source call has no complete header input inventory"))?;
            Ok(ReadGuard {
                name,
                checked: read.clone(),
                inputs: inputs.clone(),
            })
        })
        .collect()
}

/// Require every current positive read and complete candidate header, including overloads. A missing witness,
/// changed signature or duplicate/incomplete read set is a miss before the checker can skip a body.
fn matching_reads(reads: &[ReadGuard], names: &BTreeSet<String>, headers: &SignatureInputs) -> bool {
    reads.len() == names.len()
        && reads.iter().map(|read| &read.name).eq(names.iter())
        && reads.iter().all(|read| headers.get(&read.name) == Some(&read.inputs))
}

/// Read source through checked UTF-8 boundaries, refusing malformed or stale locators rather than indexing blindly.
fn source_slice(source: &str, span: Span) -> Result<&str, LibraryManifestError> {
    source
        .get(span.start..span.end)
        .ok_or_else(|| invalid("callable input has invalid source coordinates"))
}
