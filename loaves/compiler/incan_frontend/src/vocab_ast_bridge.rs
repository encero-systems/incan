//! Adapter layer between compiler-internal AST and public `incan_vocab` AST.
//!
//! This module is the single boundary where compiler-internal AST types are translated to/from the stable public AST
//! contract exposed by `incan_vocab`.
//!
//! Design goals:
//! - keep `incan_vocab` types from leaking throughout frontend/typechecker/lowering internals
//! - provide explicit, typed failures for shapes that are currently unsupported
//! - keep mapping rules centralized so parser/desugarer/runtime evolution stays coherent

use sha2::{Digest, Sha256};

use crate::ast;

mod internal_to_public;
mod public_to_internal;

pub use internal_to_public::*;
pub use public_to_internal::*;

const CURRENT_FIELD_SENTINEL_IDENT: &str = "__incan_vocab_current_row";

/// Return whether `span` marks an AST node the compiler synthesized rather than one the source spelled.
///
/// Generated syntax retains its original source coordinates and carries an expansion context. Hidden helper imports
/// retain the empty default span. Source-spelling checks use this fact to leave compiler-authored names alone.
pub(crate) fn is_synthetic_span(span: ast::Span) -> bool {
    span == ast::Span::default() || span.expansion.is_some()
}

/// Seed compiler-generated syntax from its genuine source coordinates and any enclosing expansion context.
fn expansion_anchor(source: ast::Span) -> ast::Span {
    let mut identity = Sha256::new();
    identity.update(b"incan.compiler.syntax/1\0");
    identity.update(source.start.to_le_bytes());
    identity.update(source.end.to_le_bytes());
    if let Some(parent) = source.expansion {
        identity.update(parent);
    }
    source.with_expansion(Some(identity.finalize().into()))
}

/// Allocates unique spans for AST nodes synthesized from desugarer output.
///
/// Builtin vocabulary conversion assigns a deterministic context to each decoding position while retaining the
/// source anchor. Native providers use their unit, request and structured output path at the worker boundary.
#[derive(Debug, Clone)]
struct SyntheticSpanAllocator {
    anchor: ast::Span,
    next_node: usize,
}

impl SyntheticSpanAllocator {
    /// Create an allocator whose synthetic span range is anchored by the original vocab surface span.
    fn new(anchor: ast::Span) -> Self {
        Self {
            anchor: expansion_anchor(anchor),
            next_node: 0,
        }
    }

    /// Return the next unique synthetic span for one internal AST node generated from public vocab output.
    fn next(&mut self) -> ast::Span {
        let span = self.anchor.expansion_child(&format!("public-node:{}", self.next_node));
        self.next_node += 1;
        span
    }
}

/// Mapping failures produced by the AST bridge.
///
/// Each variant indicates:
/// - which direction failed (internal -> public or public -> internal), and
/// - whether the mismatch happened at statement or expression level.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum VocabAstBridgeError {
    /// Public declaration shape cannot be represented in the current internal AST bridge mapping.
    #[error("unsupported public declaration for vocab bridge: {0}")]
    UnsupportedPublicDeclaration(&'static str),
    /// Internal statement shape cannot be represented in current public AST.
    #[error("unsupported internal statement for vocab bridge: {0}")]
    UnsupportedInternalStatement(&'static str),
    /// Internal expression shape cannot be represented in current public AST.
    #[error("unsupported internal expression for vocab bridge: {0}")]
    UnsupportedInternalExpression(&'static str),
    /// Public statement shape cannot be represented in current internal AST bridge mapping.
    #[error("unsupported public statement for vocab bridge: {0}")]
    UnsupportedPublicStatement(&'static str),
    /// Public expression shape cannot be represented in current internal AST bridge mapping.
    #[error("unsupported public expression for vocab bridge: {0}")]
    UnsupportedPublicExpression(&'static str),
}

/// Map internal binary operators to public binary operators.
///
/// This is intentionally whitelist-based so the public bridge contract expands only when both sides agree on exact
/// operator semantics.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedInternalExpression`] when an internal operator is not represented in the
/// public bridge contract.
fn map_internal_binary_op(op: ast::BinaryOp) -> Result<incan_vocab::IncanBinaryOp, VocabAstBridgeError> {
    match op {
        ast::BinaryOp::Add => Ok(incan_vocab::IncanBinaryOp::Add),
        ast::BinaryOp::Sub => Ok(incan_vocab::IncanBinaryOp::Sub),
        ast::BinaryOp::Mul => Ok(incan_vocab::IncanBinaryOp::Mul),
        ast::BinaryOp::Div => Ok(incan_vocab::IncanBinaryOp::Div),
        ast::BinaryOp::FloorDiv => Ok(incan_vocab::IncanBinaryOp::FloorDiv),
        ast::BinaryOp::Mod => Ok(incan_vocab::IncanBinaryOp::Mod),
        ast::BinaryOp::Pow => Ok(incan_vocab::IncanBinaryOp::Pow),
        ast::BinaryOp::MatMul => Ok(incan_vocab::IncanBinaryOp::MatMul),
        ast::BinaryOp::PipeForward => Ok(incan_vocab::IncanBinaryOp::PipeForward),
        ast::BinaryOp::PipeBackward => Ok(incan_vocab::IncanBinaryOp::PipeBackward),
        ast::BinaryOp::BitAnd => Ok(incan_vocab::IncanBinaryOp::BitAnd),
        ast::BinaryOp::BitOr => Ok(incan_vocab::IncanBinaryOp::BitOr),
        ast::BinaryOp::BitXor => Ok(incan_vocab::IncanBinaryOp::BitXor),
        ast::BinaryOp::Shl => Ok(incan_vocab::IncanBinaryOp::Shl),
        ast::BinaryOp::Shr => Ok(incan_vocab::IncanBinaryOp::Shr),
        ast::BinaryOp::Eq => Ok(incan_vocab::IncanBinaryOp::Eq),
        ast::BinaryOp::NotEq => Ok(incan_vocab::IncanBinaryOp::NotEq),
        ast::BinaryOp::Lt => Ok(incan_vocab::IncanBinaryOp::Lt),
        ast::BinaryOp::Gt => Ok(incan_vocab::IncanBinaryOp::Gt),
        ast::BinaryOp::LtEq => Ok(incan_vocab::IncanBinaryOp::LtEq),
        ast::BinaryOp::GtEq => Ok(incan_vocab::IncanBinaryOp::GtEq),
        ast::BinaryOp::And => Ok(incan_vocab::IncanBinaryOp::And),
        ast::BinaryOp::Or => Ok(incan_vocab::IncanBinaryOp::Or),
        _ => Err(VocabAstBridgeError::UnsupportedInternalExpression(
            "binary operator is not currently bridgeable",
        )),
    }
}

/// Map public binary operators to internal binary operators.
///
/// This mirrors [`map_internal_binary_op`] and keeps the round-trip surface explicit instead of assuming every public
/// enum variant already has an internal lowering.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedPublicExpression`] when a public operator is not represented in the
/// internal bridge mapping.
fn map_public_binary_op(op: incan_vocab::IncanBinaryOp) -> Result<ast::BinaryOp, VocabAstBridgeError> {
    match op {
        incan_vocab::IncanBinaryOp::Add => Ok(ast::BinaryOp::Add),
        incan_vocab::IncanBinaryOp::Sub => Ok(ast::BinaryOp::Sub),
        incan_vocab::IncanBinaryOp::Mul => Ok(ast::BinaryOp::Mul),
        incan_vocab::IncanBinaryOp::Div => Ok(ast::BinaryOp::Div),
        incan_vocab::IncanBinaryOp::FloorDiv => Ok(ast::BinaryOp::FloorDiv),
        incan_vocab::IncanBinaryOp::Mod => Ok(ast::BinaryOp::Mod),
        incan_vocab::IncanBinaryOp::Pow => Ok(ast::BinaryOp::Pow),
        incan_vocab::IncanBinaryOp::MatMul => Ok(ast::BinaryOp::MatMul),
        incan_vocab::IncanBinaryOp::PipeForward => Ok(ast::BinaryOp::PipeForward),
        incan_vocab::IncanBinaryOp::PipeBackward => Ok(ast::BinaryOp::PipeBackward),
        incan_vocab::IncanBinaryOp::BitAnd => Ok(ast::BinaryOp::BitAnd),
        incan_vocab::IncanBinaryOp::BitOr => Ok(ast::BinaryOp::BitOr),
        incan_vocab::IncanBinaryOp::BitXor => Ok(ast::BinaryOp::BitXor),
        incan_vocab::IncanBinaryOp::Shl => Ok(ast::BinaryOp::Shl),
        incan_vocab::IncanBinaryOp::Shr => Ok(ast::BinaryOp::Shr),
        incan_vocab::IncanBinaryOp::Eq => Ok(ast::BinaryOp::Eq),
        incan_vocab::IncanBinaryOp::NotEq => Ok(ast::BinaryOp::NotEq),
        incan_vocab::IncanBinaryOp::Lt => Ok(ast::BinaryOp::Lt),
        incan_vocab::IncanBinaryOp::Gt => Ok(ast::BinaryOp::Gt),
        incan_vocab::IncanBinaryOp::LtEq => Ok(ast::BinaryOp::LtEq),
        incan_vocab::IncanBinaryOp::GtEq => Ok(ast::BinaryOp::GtEq),
        incan_vocab::IncanBinaryOp::And => Ok(ast::BinaryOp::And),
        incan_vocab::IncanBinaryOp::Or => Ok(ast::BinaryOp::Or),
        _ => Err(VocabAstBridgeError::UnsupportedPublicExpression(
            "binary operator is not currently bridgeable",
        )),
    }
}

/// Convert an internal compiler span to public `incan_vocab::Span`.
///
/// This is a pure shape conversion; source offsets are preserved exactly.
fn public_span(span: ast::Span) -> incan_vocab::Span {
    incan_vocab::Span {
        start: span.start,
        end: span.end,
    }
}

#[cfg(test)]
mod tests;
