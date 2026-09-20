//! RFC 120 conformance: canonical symbol identity at declaration sites and on resolved references.
//!
//! These tests pin the identity contract itself rather than any consumer: one compiler-owned identity is minted at each
//! declaration site, an import/alias/re-export binding carries its *target's* identity, same-spelled bindings in
//! different scopes stay distinct, and reference-side recording answers "do these two references mean the same thing"
//! structurally. Body IR's consumption of these facts is pinned separately in `crate::body_ir::tests`.
//!
//! One module per subject; `helpers` holds the fixture helpers the subjects share.

use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::traits::{self, TraitId};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use super::super::{CompileError, TypeChecker};
use crate::ast::{Declaration, Program, Span};
use crate::provider::ProviderPlan;
use crate::symbols::{ResolvedType, SymbolKind, TypeInfo};
use crate::{lexer, parser};
use std::sync::Arc;

mod helpers;

mod declarations_and_scopes;
mod duplicates_and_collisions;
mod imports_and_dependencies;
mod references_calls_and_patterns;

use helpers::{check, check_errors, identity_at, nth_span, parse, write_identity_at};
