//! References and calls resolved by rustc, retaining unresolved indirect calls as explicit lower-bound evidence.

use incan_codegraph::{
    CodegraphCallRecord, CodegraphLanguage, CodegraphProvenance, CodegraphRecord, CodegraphReferenceRecord,
};
use rustc_hir::{
    self as hir,
    def_id::{DefId, LocalDefId},
    intravisit::{self, Visitor},
};
use rustc_middle::ty::{self, TyCtxt, TypeckResults};

use crate::rust_graph_records::{id, identity, module_id, owner_id, path, span};

/// Visit HIR item owners and each checked body once. Synthetic opaque lifetimes are definitions but have no HIR
/// node, so only `definitions()` from the crate's HIR items may enter the signature visitor.
pub fn references(tcx: TyCtxt<'_>, records: &mut Vec<CodegraphRecord>) {
    for owner in tcx.hir_crate_items(()).definitions() {
        let mut visitor = Signature { tcx, owner, records };
        match tcx.hir_node_by_def_id(owner) {
            hir::Node::Item(item) => visitor.visit_item(item),
            hir::Node::TraitItem(item) => visitor.visit_trait_item(item),
            hir::Node::ImplItem(item) => visitor.visit_impl_item(item),
            _ => {}
        }
    }
    for owner in tcx.hir_body_owners() {
        let body = tcx.hir_body_owned_by(owner);
        let mut visitor = Body {
            tcx,
            owner,
            typeck: tcx.typeck(owner),
            records,
        };
        visitor.visit_body(body);
    }
}

/// Item signatures and type declarations carry HIR resolution without body-local type tables.
struct Signature<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    owner: LocalDefId,
    records: &'a mut Vec<CodegraphRecord>,
}

impl<'tcx> Visitor<'tcx> for Signature<'_, 'tcx> {
    /// Record type and trait paths in signatures; nested bodies are visited separately with their type tables.
    fn visit_path(&mut self, node: &hir::Path<'tcx>, hir_id: hir::HirId) {
        if let Some(target) = node.res.opt_def_id() {
            self.records.push(CodegraphRecord::Reference(CodegraphReferenceRecord {
                id: format!(
                    "signature-reference:{}:{}",
                    id(self.tcx, self.owner.to_def_id()),
                    hir_id.local_id.as_u32()
                ),
                language: CodegraphLanguage::Rust,
                module_id: module_id(self.tcx, self.owner.to_def_id()),
                owner_id: Some(owner_id(self.tcx, self.owner.to_def_id())),
                name: path(self.tcx, target),
                kind: "type".to_owned(),
                target_id: Some(id(self.tcx, target)),
                canonical_identity: None,
                canonical_owner: None,
                stable_identity: Some(identity(self.tcx, target)),
                span: span(self.tcx, node.span),
                provenance: CodegraphProvenance::Checked,
                degraded: false,
            }));
        }
        intravisit::walk_path(self, node);
    }
}

/// One body shares its owner's type tables; no spelling-based resolution or candidate search is performed.
struct Body<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    owner: LocalDefId,
    typeck: &'tcx TypeckResults<'tcx>,
    records: &'a mut Vec<CodegraphRecord>,
}

impl<'tcx> Visitor<'tcx> for Body<'_, 'tcx> {
    /// Paths carry rustc resolution and method calls use the receiver-sensitive type checking result.
    fn visit_expr(&mut self, expression: &'tcx hir::Expr<'tcx>) {
        match &expression.kind {
            hir::ExprKind::Path(qpath) => {
                if let Some(target) = self.typeck.qpath_res(qpath, expression.hir_id).opt_def_id() {
                    self.reference(expression, target, "identifier");
                }
            }
            hir::ExprKind::Call(callee, arguments) => {
                let target = match self.typeck.expr_ty(callee).kind() {
                    ty::FnDef(def, _) => Some(*def),
                    _ => None,
                };
                self.call(
                    expression,
                    target,
                    if target.is_some() { "function" } else { "indirect" },
                    arguments.len(),
                );
            }
            hir::ExprKind::MethodCall(_, receiver, arguments, _) => {
                let target = self.typeck.type_dependent_def_id(expression.hir_id);
                let kind = if matches!(
                    self.typeck.expr_ty(receiver).peel_refs().kind(),
                    ty::Dynamic(..) | ty::Param(_)
                ) {
                    "virtual_method"
                } else {
                    "method"
                };
                self.call(expression, target, kind, arguments.len());
                if let Some(target) = target {
                    self.reference(expression, target, kind);
                }
            }
            hir::ExprKind::Field(receiver, _) => {
                if let ty::Adt(adt, _) = self.typeck.expr_ty_adjusted(receiver).peel_refs().kind() {
                    if !adt.is_enum() {
                        let field = self.typeck.field_index(expression.hir_id);
                        self.reference(expression, adt.non_enum_variant().fields[field].did, "field");
                    }
                }
            }
            _ => {}
        }
        intravisit::walk_expr(self, expression);
    }

    /// Record type paths as well as value paths, using the same body type tables for associated paths.
    fn visit_ty(&mut self, node: &'tcx hir::Ty<'tcx, hir::AmbigArg>) {
        if let hir::TyKind::Path(qpath) = &node.kind {
            if let Some(target) = self.typeck.qpath_res(qpath, node.hir_id).opt_def_id() {
                self.records.push(CodegraphRecord::Reference(CodegraphReferenceRecord {
                    id: format!(
                        "reference:{}:{}",
                        id(self.tcx, self.owner.to_def_id()),
                        node.hir_id.local_id.as_u32()
                    ),
                    language: CodegraphLanguage::Rust,
                    module_id: module_id(self.tcx, self.owner.to_def_id()),
                    owner_id: Some(owner_id(self.tcx, self.owner.to_def_id())),
                    name: path(self.tcx, target),
                    kind: "type".to_owned(),
                    target_id: Some(id(self.tcx, target)),
                    canonical_identity: None,
                    canonical_owner: None,
                    stable_identity: Some(identity(self.tcx, target)),
                    span: span(self.tcx, node.span),
                    provenance: CodegraphProvenance::Checked,
                    degraded: false,
                }));
            }
        }
        intravisit::walk_ty(self, node);
    }
}

impl Body<'_, '_> {
    /// Preserve a resolved reference even if its definition is external to this unit's graph.
    fn reference(&mut self, expression: &hir::Expr<'_>, target: DefId, kind: &str) {
        self.records.push(CodegraphRecord::Reference(CodegraphReferenceRecord {
            id: format!(
                "reference:{}:{}",
                id(self.tcx, self.owner.to_def_id()),
                expression.hir_id.local_id.as_u32()
            ),
            language: CodegraphLanguage::Rust,
            module_id: module_id(self.tcx, self.owner.to_def_id()),
            owner_id: Some(owner_id(self.tcx, self.owner.to_def_id())),
            name: path(self.tcx, target),
            kind: kind.to_owned(),
            target_id: Some(id(self.tcx, target)),
            canonical_identity: None,
            canonical_owner: None,
            stable_identity: Some(identity(self.tcx, target)),
            span: span(self.tcx, expression.span),
            provenance: CodegraphProvenance::Checked,
            degraded: false,
        }));
    }

    /// Retain unknown function-pointer targets as `indirect`; a trait target names the requirement, not implementors.
    fn call(&mut self, expression: &hir::Expr<'_>, target: Option<DefId>, kind: &str, argument_count: usize) {
        self.records.push(CodegraphRecord::Call(CodegraphCallRecord {
            id: format!(
                "call:{}:{}",
                id(self.tcx, self.owner.to_def_id()),
                expression.hir_id.local_id.as_u32()
            ),
            language: CodegraphLanguage::Rust,
            module_id: module_id(self.tcx, self.owner.to_def_id()),
            owner_id: Some(owner_id(self.tcx, self.owner.to_def_id())),
            callee: target
                .map(|def| path(self.tcx, def))
                .unwrap_or_else(|| "<indirect>".to_owned()),
            kind: kind.to_owned(),
            argument_count,
            type_argument_count: 0,
            target_id: target.map(|def| id(self.tcx, def)),
            canonical_identity: None,
            canonical_owner: None,
            stable_identity: target.map(|def| identity(self.tcx, def)),
            span: span(self.tcx, expression.span),
            provenance: CodegraphProvenance::Checked,
            degraded: false,
        }));
    }
}
