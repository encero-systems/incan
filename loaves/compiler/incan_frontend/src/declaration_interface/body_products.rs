//! Accepted scalar bodies and checked source-module calls under the callable interface's existing identities.
//!
//! Calls retain their complete checked candidate/signature witnesses and exact current header inputs. Imports,
//! nominal types, defaults, mutable storage and scope-changing bodies refuse capture. Physical locators must match
//! too; a semantic digest alone never grants a hit.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use incan_lang::lang::types::numerics::{self, NumericTypeId};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::stable_identity::StableIdentityProjection;
use incan_semantics_core::{CanonicalStableDeclarationContext, CanonicalSymbolId};
use serde::{Deserialize, Serialize};

use super::callable_module::{CheckedCallableModule, CheckedCallableRead};
use super::{CheckedCallableInterface, CheckedDeclarationFingerprints, decode_record, encode_record, invalid};
use crate::ast::{
    CallArg, Declaration, Expr, FunctionDecl, Literal, ParamKind, Program, Span, Spanned, Statement, Type,
};
use crate::library_manifest::LibraryManifestError;
use crate::typechecker::TypeChecker;
use crate::vocab_ast_bridge::is_synthetic_span;

/// One accepted module's independently reusable scalar declarations. The private codec retains checked evidence,
/// never an AST that a consumer could mistake for an accepted declaration environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallableBodyProducts {
    namespace: Vec<(Span, String)>,
    interface: CheckedCallableModule,
    entries: Vec<BodyProduct>,
}

/// Accepted scalar headers and exact source inputs retained until the ordinary Body IR producer completes.
/// This witness avoids cloning a checker or building Body IR twice just to populate a cache.
pub struct CallableBodyInputs {
    namespace: Vec<(Span, String)>,
    interface: CheckedCallableModule,
    declarations: Vec<BodyInput>,
    contexts: Vec<(CanonicalSymbolId, CanonicalSymbolId, u32)>,
}

/// An accepted input and its explicit reads, awaiting the ordinary Body IR producer's completed result.
struct BodyInput {
    header: CheckedCallableInterface,
    span: Span,
    source: String,
    reads: Vec<ReadGuard>,
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
    source: String,
    reads: Vec<ReadGuard>,
    contexts: Vec<(CanonicalSymbolId, CanonicalSymbolId, u32)>,
    body: Body,
}

/// A producer-validated selection for exactly one parsed program. Only this module can construct a selection;
/// the checker compares its input again before skipping any body, and Body IR consumes the same retained products.
#[derive(Debug, Clone)]
pub struct ValidatedCallableBodies {
    program: Program,
    bodies: BTreeMap<Span, Body>,
    contexts: Vec<(CanonicalSymbolId, CanonicalSymbolId, u32)>,
    owner: incan_semantics_core::SymbolOrigin,
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
        if namespace(program) != self.namespace {
            return Ok(ValidatedCallableBodies {
                program: program.clone(),
                bodies: BTreeMap::new(),
                contexts: Vec::new(),
                owner: incan_semantics_core::SymbolOrigin::Builtin,
            });
        }
        let owner = incan_semantics_core::SymbolOrigin::Package {
            library: package.to_owned(),
            module_path: module.to_vec(),
        };

        // ---- Selection: admit exact untouched scalar bodies and every consumed source header ----
        let mut bodies = BTreeMap::new();
        let mut contexts = Vec::new();
        for (declaration, entry) in program
            .declarations
            .iter()
            .filter(|declaration| matches!(declaration.node, Declaration::Function(_)))
            .zip(&self.entries)
        {
            let Declaration::Function(function) = &declaration.node else {
                break;
            };
            let Some(reads) = admitted_reads(function, &names) else {
                break;
            };
            let input = source_slice(source, declaration.span)?;
            if entry.header.identity().origin == owner
                && entry.body.span == declaration.span
                && entry.source == input
                && matching_reads(&entry.reads, &reads, &headers)
            {
                bodies.insert(declaration.span, entry.body.clone());
                contexts.extend(entry.contexts.iter().cloned());
            }
        }

        // ---- Replay: bind accepted products to the exact parsed program ----
        Ok(ValidatedCallableBodies {
            program: program.clone(),
            bodies,
            contexts,
            owner,
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
        if self.entries.len() > self.namespace.len()
            || self
                .entries
                .iter()
                .zip(&self.namespace)
                .any(|(entry, (span, name))| entry.body.span != *span || entry.body.name != *name)
        {
            return Err(invalid(
                "callable products lost their complete physical declaration inventory",
            ));
        }
        let mut identities = BTreeSet::new();
        let mut locators = BTreeSet::new();
        for entry in &self.entries {
            let mut body = entry.body.clone();
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
        // ---- Admission: preserve ordinary diagnostics and the source-only namespace ----
        admitted_functions(program)?;
        if !checker.errors.is_empty() || !checker.warnings().is_empty() {
            return Err(invalid("callable products require accepted bodies without diagnostics"));
        }
        let interface = CheckedCallableModule::from_checked(program, checker)?;
        let checked_reads = interface.read_inventory()?;
        let headers = signature_inputs(program, source)?;
        let names = headers.keys().cloned().collect::<BTreeSet<_>>();

        // ---- Headers: stop before a body whose scope or nonlocal reads need a larger product ----
        let mut declarations = Vec::new();
        for declaration in &program.declarations {
            if matches!(declaration.node, Declaration::Docstring(_)) {
                continue;
            }
            let Declaration::Function(function) = &declaration.node else {
                return Err(invalid("callable products require source-only free functions"));
            };
            let Some(reads) = admitted_reads(function, &names) else {
                break;
            };
            declarations.push(BodyInput {
                header: CheckedCallableInterface::from_checked(function, declaration.span, checker)?,
                span: declaration.span,
                source: source_slice(source, declaration.span)?.to_owned(),
                reads: capture_reads(reads, &checked_reads, &headers)?,
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
            return Err(invalid(
                "callable products have no scalar prefix before an unsupported body",
            ));
        }
        Ok(Self {
            namespace: namespace(program),
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
            source,
            reads,
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
                source,
                reads,
                contexts,
                body,
            });
        }

        // ---- Publication: validate identity and meaning before exposing a product set ----
        let products = CallableBodyProducts {
            namespace: self.namespace,
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
            });
        if self.program == *program && Some(self.owner) == owner {
            checker.type_info.reused_callable_bodies = self.bodies;
            checker.type_info.reused_callable_contexts = self.contexts;
        }
    }

    /// Report the selected source-body count independently of actual checker and Body IR execution counters.
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// Report whether this validated selection contains any source-body products.
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }
}

/// Admit only a source namespace of scalar free-function signatures. Bodies before the first nonlocal read may
/// supply products; every later body stays on the full path, preserving physical lexical scope numbering.
fn admitted_functions(program: &Program) -> Result<(), LibraryManifestError> {
    if program.rust_module_path.is_some() || program.declarations.is_empty() {
        return Err(invalid("callable products require an ordinary nonempty source module"));
    }
    for declaration in &program.declarations {
        if is_synthetic_span(declaration.span) {
            return Err(invalid("callable products refuse expanded source declarations"));
        }
        if matches!(declaration.node, Declaration::Docstring(_)) {
            continue;
        }
        let Declaration::Function(function) = &declaration.node else {
            return Err(invalid(
                "callable products refuse imports and non-function declarations",
            ));
        };
        if !scalar_signature(function) {
            return Err(invalid(
                "callable products require undecorated immutable scalar signatures",
            ));
        }
    }
    Ok(())
}

/// Enumerate every nonlocal read of one scalar expression. Unsupported scope creation still ends the reusable
/// prefix, so skipped functions preserve the ordinary checker's physical function-scope numbering.
fn admitted_reads(function: &FunctionDecl, names: &BTreeSet<String>) -> Option<BTreeSet<String>> {
    let [statement] = function.body.as_slice() else {
        return None;
    };
    if !scalar_signature(function) || is_synthetic_span(statement.span) {
        return None;
    }
    let Statement::Return(Some(value)) = &statement.node else {
        return None;
    };
    let mut reads = BTreeSet::new();
    parameter_expression(value, function, names, &mut reads).then_some(reads)
}

/// Keep declaration collection independent of body reuse. A scalar prefix may precede an ordinary checked caller;
/// the first unsupported body and everything after it remain on the full path, preserving lexical scope numbering.
fn scalar_signature(function: &FunctionDecl) -> bool {
    if !function.decorators.is_empty()
        || numerics::from_str(&function.name).is_some()
        || !function.surface_modifiers.is_empty()
        || !function.type_params.is_empty()
        || !scalar_type(&function.return_type.node)
        || is_synthetic_span(function.return_type.span)
        || function.params.iter().any(|parameter| {
            parameter.node.is_mut
                || is_synthetic_span(parameter.span)
                || is_synthetic_span(parameter.node.ty.span)
                || parameter.node.kind != ParamKind::Normal
                || parameter.node.default.is_some()
                || !scalar_type(&parameter.node.ty.node)
        })
    {
        return false;
    }
    true
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

/// Recognize only the language's primitive scalar annotations; checker acceptance remains authoritative.
fn scalar_type(ty: &Type) -> bool {
    matches!(ty, Type::Simple(name) if matches!(numerics::from_str(name),
        Some(NumericTypeId::I64 | NumericTypeId::F64 | NumericTypeId::Bool)))
}

/// Enumerate the admitted expression grammar instead of assuming that an absent call edge proves no dependency.
fn parameter_expression(
    expression: &Spanned<Expr>,
    function: &FunctionDecl,
    names: &BTreeSet<String>,
    reads: &mut BTreeSet<String>,
) -> bool {
    if is_synthetic_span(expression.span) {
        return false;
    }
    match &expression.node {
        Expr::Ident(name) => function.params.iter().any(|parameter| parameter.node.name == *name),
        Expr::Literal(Literal::Int(_) | Literal::Float(_) | Literal::Bool(_)) => true,
        Expr::Binary(left, _, right) => {
            parameter_expression(left, function, names, reads) && parameter_expression(right, function, names, reads)
        }
        Expr::Unary(_, operand) | Expr::Paren(operand) => parameter_expression(operand, function, names, reads),
        Expr::Call(callee, arguments, values) if arguments.is_empty() && !is_synthetic_span(callee.span) => {
            let Expr::Ident(name) = &callee.node else {
                return false;
            };
            if !names.contains(name) || function.params.iter().any(|parameter| parameter.node.name == *name) {
                return false;
            }
            if !values.iter().all(|value| {
                matches!(value, CallArg::Positional(value)
                if parameter_expression(value, function, names, reads))
            }) {
                return false;
            }
            reads.insert(name.clone());
            true
        }
        _ => false,
    }
}

/// Retain exact signature inputs before any body check. Primitive-only annotations, the complete namespace and
/// unchanged compiler/dependency context make these the same inputs previously accepted by the producer.
fn signature_inputs(program: &Program, source: &str) -> Result<SignatureInputs, LibraryManifestError> {
    let mut headers: SignatureInputs = BTreeMap::new();
    for declaration in &program.declarations {
        let Declaration::Function(function) = &declaration.node else {
            continue;
        };
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
