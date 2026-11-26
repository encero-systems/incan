//! A complete scalar Body for tests that need one, with every optional fact absent.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_semantics_core::body_ir::{Block, Body, ScopeId};
use incan_semantics_core::{CompilerNodeId, HirSourceSpan, IncanPrimitiveType, IncanType};

/// A unit-returning body named `name` in module `app`, spanning bytes 0 to 10, with no statements.
pub fn empty_body(name: &str) -> Body {
    Body {
        nominal_instantiation: Vec::new(),
        trait_default_instantiation: None,
        type_parameters: Vec::new(),
        decl_id: CompilerNodeId::declaration("app", name),
        direct_call_id: CompilerNodeId::declaration_span("app", 0, 10),
        canonical: None,
        name: name.into(),
        span: HirSourceSpan::new(0, 10),
        return_type: IncanType::Primitive(IncanPrimitiveType::Unit),
        callable_representation: None,
        checked_input_profile: None,
        explicit_input_bindings: Vec::new(),
        returned_view: None,
        named_type_identities: Arc::new(BTreeMap::new()),
        locals: Vec::new(),
        params: Vec::new(),
        param_locals: Vec::new(),
        scopes: Vec::new(),
        block: Block {
            scope: ScopeId(0),
            stmts: Vec::new(),
        },
        runtime_requirements: Vec::new(),
        panic_facts: Vec::new(),
        is_async: false,
        retired_rust_facade_wire: None,
        native_attributes: Vec::new(),
    }
}
