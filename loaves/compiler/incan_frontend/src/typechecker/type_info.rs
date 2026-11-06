//! Lowering-facing typechecker artifact snapshots.
//!
//! This module contains the reusable semantic metadata that later compiler stages consume after typechecking. It keeps
//! the cross-phase snapshot surface separate from the main [`TypeChecker`](super::TypeChecker) orchestration state.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use sha2::{Digest, Sha256};

use crate::ast::{Expr, ParamKind, Span, Spanned, Visibility};
use crate::library_exports::CheckedParamDefault;
use crate::symbols::{
    CallableParam, ClassInfo, FunctionOverloadInfo, ImplementationTypeParamInfo, MethodInfo,
    NewtypePrimitiveConstraint, ResolvedType, TypeBoundInfo,
};
use incan_lang::interop::{CoercionPolicy, RustFunctionSig};
use incan_lang::lang::builtins::BuiltinFnId;
use incan_lang::lang::c_abi::{LinkCapabilityId, ScalarTypeId, link_capability_as_str, scalar_type_as_str};
use incan_lang::lang::surface::string_methods::StringMethodId;
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeCategory};
use incan_lang::lang::types::collections::{self as collection_types, CollectionTypeId};
use incan_semantics_core::{
    CanonicalStableDeclarationContext, CanonicalSymbolId, CompilerNodeId, IncanCallableParam, IncanCallableParamKind,
    IncanPrimitiveType, IncanType, SemanticFact, SemanticFactKind, SemanticFactStore, SemanticFactValue,
    SemanticRegistryEntry, SemanticRegistrySubjectKind, SemanticRegistryValue, SemanticSourceTarget,
    SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

use super::{ConstValue, const_eval, ownership_contracts};

mod binding_facts;
mod c_abi_facts;
mod call_facts;
mod declaration_artifacts;
mod fact_accessors;
mod registry_and_layout_facts;
mod rust_interop_facts;
mod semantic_artifacts;
mod semantic_facts;

pub use binding_facts::*;
pub use c_abi_facts::*;
pub use call_facts::*;
pub use declaration_artifacts::*;
pub use registry_and_layout_facts::*;
pub use rust_interop_facts::*;
pub use semantic_artifacts::*;
pub use semantic_facts::*;

/// Capture reusable typechecking output for later compiler stages.
///
/// This struct is the bridge that lets backend lowering/codegen consume the typechecker’s view of the program rather
/// than re-deriving types and semantics from the AST. The bridge is intentionally grouped by consumer contract: each
/// field names a semantic artifact family instead of exposing one flat collection of unrelated side channels.
///
/// ## Notes
/// - Expression types are keyed by `span` so downstream code can look them up without holding AST node identities.
/// - Const classification is recorded to support RFC 008 “Rust-native vs Frozen” const emission.
///
/// ## Examples
/// ```ignore
/// use incan_frontend::{lexer, parser, typechecker};
///
/// let tokens = lexer::lex("def foo() -> int: return 1")?;
/// let ast = parser::parse(&tokens)?;
/// let mut tc = typechecker::TypeChecker::new();
/// tc.check_program(&ast)?;
/// let info = tc.type_info();
/// // info.expr_type(...) can now be queried by spans.
/// ```
#[derive(Debug, Default, Clone)]
pub struct TypeCheckInfo {
    /// Optional migration-baseline owners, captured only by an explicitly requested baseline activation. They are not
    /// serialized, are not retained by idle workers, and never authorize a production record read.
    pub(crate) body_ir_baseline_inputs: Option<super::BodyIrBaselineInputs>,
    /// Trait hierarchy metadata consumed by trait impl and default-method lowering.
    pub traits: TraitArtifacts,
    /// Derive expansion metadata imported from dependency modules and manifests.
    pub derivations: DerivationArtifacts,
    /// Expression-local resolution facts keyed by source spans.
    pub expressions: ExpressionArtifacts,
    /// Source-reference resolution facts keyed by source spans.
    pub references: ReferenceArtifacts,
    /// Const evaluation facts needed by runtime and emission boundaries.
    pub consts: ConstArtifacts,
    /// Rust interop decisions that must be preserved exactly across lowering.
    pub rust: RustInteropArtifacts,
    /// Declaration-level binding rewrites and visibility facts consumed by lowering.
    pub declarations: DeclarationArtifacts,
    /// Checked typed declaration-registry definitions and descriptions.
    pub registry: RegistryArtifacts,
    /// Call-site semantic decisions selected by the typechecker.
    pub calls: CallArtifacts,
    /// Custom protocol decisions that lower into explicit runtime calls.
    pub protocols: ProtocolArtifacts,
    /// Checked C ABI declaration facts consumed by verification and code generation.
    pub c_abi: CAbiInteropArtifacts,
    /// Source import and alias paths accepted by typechecking, keyed by their active local binding.
    pub import_bindings: CheckedImportBindings,
    /// Active builtin type identities used only by the Body IR bridge; no checker decision or legacy consumer reads
    /// this snapshot.
    pub body_ir_runtime_types: BTreeMap<String, CanonicalSymbolId>,
    /// Atomic accepted-body refinements consumed only by Body IR; no checker or legacy consumer reads this group.
    pub(crate) body_ir_newtypes: super::native_newtype_facts::NativeNewtypeFacts,
    /// Accepted zero-argument iterator sum item types within the direct profile, keyed by the exact checked call span.
    ///
    /// Source newtype trait adoptions and source `sum` members need explicit dispatch and stay outside this snapshot.
    /// Only Body IR reads it; checking decisions and legacy-consumed facts are unchanged.
    pub(crate) body_ir_iterator_sum_items: BTreeMap<crate::ast::Span, ResolvedType>,
}

/// Checked source-path facts for active import-derived bindings.
///
/// These paths describe how source resolution accepted a local name. They intentionally do not replace canonical
/// symbol identities: a re-exported binding can retain its facade path here while its identity names the original
/// declaring module.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CheckedImportBindings {
    paths: BTreeMap<String, Vec<String>>,
}

impl CheckedImportBindings {
    /// Return the checked source path for one active local binding.
    pub fn path(&self, local_name: &str) -> Option<&[String]> {
        self.paths.get(local_name).map(Vec::as_slice)
    }

    /// Iterate active local bindings and their checked source paths in deterministic name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.paths.iter().map(|(name, path)| (name.as_str(), path.as_slice()))
    }

    /// Build checked import bindings from compiler-resolved local names and source paths.
    pub fn from_paths(paths: impl IntoIterator<Item = (String, Vec<String>)>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
        }
    }
}

/// The facts method resolution records for one call at the call's span: the method call it selected, the declaration it
/// reaches, the parameters it binds its arguments to and the type arguments it instantiates.
///
/// Taken with [`TypeCheckInfo::take_call_site_facts`] and put back with [`TypeCheckInfo::restore_call_site_facts`], so
/// a call the checker resolves on the program's behalf at a source expression's span, such as a `for` loop's
/// `__iter__` and `__next__` hooks at its iterable's span, leaves the facts of the call written there as they were.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CallSiteFacts {
    explicit_input_call: Option<super::ownership_contracts::InputCall>,
    checked_rust_call: Option<RustCheckedCall>,
    checked_rust_function_value: Option<RustCheckedCall>,
    body_ir_reexport_call: Option<BodyIrReexportCallInfo>,
    body_ir_string_slice_member: Option<CanonicalSymbolId>,
    inferred_constructor_type: Option<ResolvedType>,
    inferred_call_result_type: Option<ResolvedType>,
    method_call: Option<ResolvedMethodCall>,
    identity: Option<CanonicalSymbolId>,
    callable_params: Option<Vec<CallableParam>>,
    monomorph_type_args: Option<Vec<ResolvedType>>,
    body_ir_constructor_type: Option<ResolvedType>,
    body_ir_call_type_arguments: Option<Vec<ResolvedType>>,
    body_ir_call_result_type: Option<ResolvedType>,
    body_ir_factory_owner_type: Option<ResolvedType>,
    body_ir_async_call_target: Option<CanonicalSymbolId>,
    body_ir_self_result_method: Option<CanonicalSymbolId>,
    body_ir_self_method_identity: Option<CanonicalSymbolId>,
    body_ir_self_method_result_type: Option<ResolvedType>,
    body_ir_self_method_params: Option<Vec<CallableParam>>,
    body_ir_inferred_method_identity: Option<CanonicalSymbolId>,
}
