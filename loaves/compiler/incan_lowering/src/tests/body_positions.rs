//! Body positions visit child statement spans before their owners and place each by UTF-8 line and column.

use incan_mir_plan_native::source_span;
use incan_semantics_core::HirSourceSpan;
use incan_semantics_core::body_ir::{Block, ScopeId, Statement, StatementKind};

use super::bodies::empty_body;
use crate::body_positions::body_position_map;

/// Child statement spans precede their owners; identity metadata contributes no positional nodes.
#[test]
fn native_body_positions_keep_nested_order_and_utf8_offsets() {
    let mut body = empty_body("walk");
    let child = Statement {
        kind: StatementKind::Continue,
        span: HirSourceSpan::new(7, 8),
    };
    let block = Block {
        scope: ScopeId(0),
        stmts: vec![child],
    };
    body.block.stmts = vec![Statement {
        kind: StatementKind::Loop { body: block },
        span: HirSourceSpan::new(3, 9),
    }];
    let owner = source_span("src/main.incn", 1, 1);
    let positions = body_position_map(&body, "é\nabcdx\n", "src/main.incn", &owner);
    assert_eq!(positions, vec![(1, 5), (1, 1), (0, 1)]);
}
